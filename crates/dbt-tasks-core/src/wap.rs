//! Invocation-local planning for audited publication of Snowflake tables.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use dbt_adapter::relation::{create_relation, create_relation_from_node};
use dbt_adapter_core::AdapterType;
use dbt_common::hashing::code_hash;
use dbt_common::io_args::{ComputeArg, FsCommand, LocalExecutionBackendKind};
use dbt_common::{ErrorCode, FsResult, fs_err};
use dbt_dag::schedule::Schedule;
use dbt_schemas::dbt_types::RelationType;
use dbt_schemas::schemas::common::{DbtMaterialization, ResolvedQuoting, StoreFailuresAs};
use dbt_schemas::schemas::relations::base::BaseRelation;
use dbt_schemas::schemas::telemetry::NodeType;
use dbt_schemas::schemas::{DbtModel, InternalDbtNode, InternalDbtNodeAttributes, Nodes};

use crate::RunTasksArgs;

/// A canonical model and the private execution identity used by this invocation.
#[derive(Debug, Clone)]
pub struct WapModel {
    pub model: Arc<DbtModel>,
    pub candidate_identifier: String,
    pub audit_ids: BTreeSet<String>,
}

impl WapModel {
    pub fn public_relation(&self) -> FsResult<Box<dyn BaseRelation>> {
        create_relation_from_node(self.model.node_adapter(), self.model.as_ref(), None)
    }

    pub fn candidate_relation(&self) -> FsResult<Box<dyn BaseRelation>> {
        self.candidate_relation_with_quoting(self.model.quoting())
    }

    fn candidate_relation_with_quoting(
        &self,
        quoting: ResolvedQuoting,
    ) -> FsResult<Box<dyn BaseRelation>> {
        create_relation(
            self.model.node_adapter(),
            self.model.database(),
            self.model.schema(),
            Some(self.candidate_identifier.clone()),
            Some(RelationType::Table),
            quoting,
        )
    }

    /// The built-in materialization creates its table using the adapter's quoting.
    pub fn validate_materialization_quoting(
        &self,
        effective_quoting: ResolvedQuoting,
    ) -> FsResult<()> {
        let expected = relation_identity(self.candidate_relation()?.as_ref())?;
        let actual = relation_identity(
            self.candidate_relation_with_quoting(effective_quoting)?
                .as_ref(),
        )?;
        if actual != expected {
            return Err(fs_err!(
                ErrorCode::InvalidConfig,
                "WAP model '{}': the table materialization resolves a different database or schema under the profile's quoting policy; align the model and profile quoting settings",
                self.model.common().unique_id
            ));
        }
        Ok(())
    }

    /// Create the scoped model used for candidate rendering and materialization.
    /// The manifest's model is never mutated.
    pub fn execution_model(&self) -> FsResult<DbtModel> {
        let mut model = self.model.as_ref().clone();
        model.__base_attr__.alias = self.candidate_identifier.clone();
        model.deprecated_config.alias = Some(self.candidate_identifier.clone());
        model.__base_attr__.relation_name = Some(self.candidate_relation()?.render_self_as_str());
        // Access is applied to the public relation only after the audit passes.
        model.deprecated_config.grants = Default::default();
        let warehouse = &mut model.deprecated_config.__warehouse_specific_config__;
        warehouse.copy_grants = Some(false);
        warehouse.copy_tags = Some(false);
        if let Some(snowflake) = model.__adapter_attr__.snowflake_attr.as_mut() {
            snowflake.copy_grants = Some(false);
            snowflake.copy_tags = Some(false);
        }
        Ok(model)
    }

    fn relation_names(&self) -> FsResult<([String; 3], [String; 3])> {
        Ok((
            relation_identity(self.public_relation()?.as_ref())?,
            relation_identity(self.candidate_relation()?.as_ref())?,
        ))
    }
}

/// Keep canonical components separate and compare their normalized strings exactly:
/// CanonicalFqn's Ident equality ignores case even for quoted relation names.
fn relation_identity(relation: &dyn BaseRelation) -> FsResult<[String; 3]> {
    let canonical = relation.get_canonical_fqn()?;
    Ok([canonical.catalog(), canonical.schema(), canonical.table()]
        .map(|part| part.as_str().to_owned()))
}

/// Validated WAP work for the selected models in a single invocation.
#[derive(Debug, Clone, Default)]
pub struct WapPlan {
    pub models: BTreeMap<String, WapModel>,
}

impl WapPlan {
    /// Discover all required audits before any warehouse writes are scheduled.
    pub fn build(
        args: &RunTasksArgs,
        schedule: &Schedule<String>,
        nodes: &Nodes,
    ) -> FsResult<Self> {
        let mut plan = Self::default();
        for unique_id in &schedule.selected_nodes {
            let Some(model) = nodes.models.get(unique_id) else {
                continue;
            };
            if !model.deprecated_config.wap.unwrap_or(false) {
                continue;
            }
            model.validate_wap_config(model.node_adapter())?;
            match args.command {
                FsCommand::Run | FsCommand::Clone => {
                    return Err(fs_err!(
                        ErrorCode::InvalidConfig,
                        "Model '{unique_id}' has wap=true; use dbt build to audit and publish it"
                    ));
                }
                FsCommand::Build => {}
                // Read-only commands and standalone tests use canonical relations.
                _ => continue,
            }
            if args.empty || args.sample.is_some() || !args.sample_renaming.is_empty() {
                return Err(fs_err!(
                    ErrorCode::InvalidConfig,
                    "Model '{unique_id}': wap=true does not support --empty or --sample"
                ));
            }
            if args.local_execution_backend != LocalExecutionBackendKind::Remote
                || model
                    .base()
                    .compute
                    .is_some_and(|compute| compute != ComputeArg::Remote)
                || !model.node_propagate().is_empty()
                || args.infer_schemas_and_typeless
            {
                return Err(fs_err!(
                    ErrorCode::InvalidConfig,
                    "Model '{unique_id}': wap=true requires execution on Snowflake without compute propagation or schema-only inference"
                ));
            }
            let audit_ids = Self::required_audit_ids(unique_id, nodes)?;
            if audit_ids.is_empty() {
                return Err(fs_err!(
                    ErrorCode::InvalidConfig,
                    "Model '{unique_id}': wap=true requires at least one enabled single-relation data test"
                ));
            }
            let missing = audit_ids
                .difference(&schedule.selected_nodes)
                .map(String::as_str)
                .collect::<Vec<_>>();
            if !missing.is_empty() {
                return Err(fs_err!(
                    ErrorCode::InvalidConfig,
                    "Model '{unique_id}': all WAP audits must be selected; missing: {}",
                    missing.join(", ")
                ));
            }
            for audit_id in &audit_ids {
                let audit = &nodes.tests[audit_id];
                if audit.node_adapter() != AdapterType::Snowflake
                    || audit
                        .base()
                        .compute
                        .is_some_and(|compute| compute != ComputeArg::Remote)
                {
                    return Err(fs_err!(
                        ErrorCode::InvalidConfig,
                        "WAP audit '{audit_id}' must execute on Snowflake"
                    ));
                }
            }
            let candidate_identifier = format!(
                "__DBT_WAP_{}_{}",
                args.io.invocation_id.simple(),
                code_hash(unique_id)
            )
            .to_ascii_uppercase();
            plan.models.insert(
                unique_id.clone(),
                WapModel {
                    model: Arc::clone(model),
                    candidate_identifier,
                    audit_ids,
                },
            );
        }
        plan.validate_relation_names(nodes)?;
        Ok(plan)
    }

    fn validate_relation_names(&self, nodes: &Nodes) -> FsResult<()> {
        for (model_id, entry) in &self.models {
            let (target_name, candidate_name) = entry.relation_names()?;
            for (other_id, other) in nodes.iter() {
                if !other.base().enabled
                    || other.node_adapter() != AdapterType::Snowflake
                    || !matches!(
                        other.resource_type(),
                        NodeType::Model | NodeType::Seed | NodeType::Snapshot | NodeType::Source
                    )
                    || other.materialized() == DbtMaterialization::Ephemeral
                {
                    continue;
                }
                let other_relation =
                    create_relation_from_node(AdapterType::Snowflake, other, None)?;
                let other_name = relation_identity(other_relation.as_ref())?;
                if other_name == candidate_name {
                    return Err(fs_err!(
                        ErrorCode::InvalidConfig,
                        "WAP candidate for '{model_id}' collides with relation of '{other_id}'"
                    ));
                }
                if other_id != model_id
                    && other.resource_type() != NodeType::Source
                    && other_name == target_name
                {
                    return Err(fs_err!(
                        ErrorCode::InvalidConfig,
                        "WAP model '{model_id}' shares its public relation with '{other_id}'"
                    ));
                }
            }
        }
        Ok(())
    }

    /// Failure storage must not overwrite a public or working WAP table.
    /// Test materializations use `api.Relation.create` with the adapter's quoting,
    /// which can differ from the test node's configured quoting.
    pub fn validate_test_storage_relations(
        &self,
        nodes: &Nodes,
        effective_quoting: ResolvedQuoting,
    ) -> FsResult<()> {
        for (model_id, entry) in &self.models {
            let (target_name, candidate_name) = entry.relation_names()?;
            for (test_id, test) in &nodes.tests {
                let config = &test.deprecated_config;
                if !test.base().enabled
                    || config.enabled == Some(false)
                    || test.node_adapter() != AdapterType::Snowflake
                    || !config.store_failures.unwrap_or(matches!(
                        config.store_failures_as,
                        Some(StoreFailuresAs::Table | StoreFailuresAs::View)
                    ))
                {
                    continue;
                }
                let storage = create_relation(
                    AdapterType::Snowflake,
                    test.database(),
                    test.schema(),
                    Some(test.base().alias.clone()),
                    None,
                    effective_quoting,
                )?;
                let storage_name = relation_identity(storage.as_ref())?;
                if storage_name == target_name || storage_name == candidate_name {
                    return Err(fs_err!(
                        ErrorCode::InvalidConfig,
                        "Failure storage for test '{test_id}' collides with the public or candidate table of WAP model '{model_id}'"
                    ));
                }
            }
        }
        Ok(())
    }

    /// Required enabled audits, using the full manifest rather than pruned selection.
    pub fn required_audit_ids(model_id: &str, nodes: &Nodes) -> FsResult<BTreeSet<String>> {
        let mut audit_ids = BTreeSet::new();
        for (test_id, test) in &nodes.tests {
            if !test.base().enabled || test.deprecated_config.enabled == Some(false) {
                continue;
            }
            let dependencies = &test.base().depends_on.nodes;
            let owned = match test.__test_attr__.attached_node.as_deref() {
                Some(owner) => owner == model_id,
                None => dependencies.iter().any(|dependency| dependency == model_id),
            };
            if !owned {
                continue;
            }
            if dependencies.is_empty()
                || dependencies.iter().any(|dependency| dependency != model_id)
            {
                return Err(fs_err!(
                    ErrorCode::InvalidConfig,
                    "WAP audit '{test_id}' for '{model_id}' must depend only on that model; multi-relation audits are not supported"
                ));
            }
            audit_ids.insert(test_id.clone());
        }
        Ok(audit_ids)
    }

    pub fn model(&self, unique_id: &str) -> Option<&WapModel> {
        self.models.get(unique_id)
    }

    pub fn audit_owner(&self, unique_id: &str) -> Option<&WapModel> {
        self.models
            .values()
            .find(|model| model.audit_ids.contains(unique_id))
    }

    pub fn contains_node(&self, unique_id: &str) -> bool {
        self.models.contains_key(unique_id) || self.audit_owner(unique_id).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_adapter::relation::{RelationObject, factory::create_static_relation};
    use dbt_schemas::schemas::DbtTest;

    fn fixtures() -> (RunTasksArgs, Schedule<String>, Nodes) {
        let args = RunTasksArgs {
            command: FsCommand::Build,
            ..Default::default()
        };
        let mut nodes = Nodes::default();
        let mut model = DbtModel::default();
        model.__common_attr__.unique_id = "model.pkg.orders".to_owned();
        model.__common_attr__.language = Some("sql".to_owned());
        model.__base_attr__.enabled = true;
        model.__base_attr__.materialized = DbtMaterialization::Table;
        model.__base_attr__.database = "DB".to_owned();
        model.__base_attr__.schema = "PUBLIC".to_owned();
        model.__base_attr__.alias = "ORDERS".to_owned();
        model.deprecated_config.wap = Some(true);
        nodes.models.insert(model.unique_id(), Arc::new(model));
        let mut test = DbtTest::default();
        test.__common_attr__.unique_id = "test.pkg.orders_not_null".to_owned();
        test.__base_attr__.enabled = true;
        test.__base_attr__.depends_on.nodes = vec!["model.pkg.orders".to_owned()];
        test.__test_attr__.attached_node = Some("model.pkg.orders".to_owned());
        nodes.tests.insert(test.unique_id(), Arc::new(test));
        let schedule = Schedule {
            selected_nodes: BTreeSet::from([
                "model.pkg.orders".to_owned(),
                "test.pkg.orders_not_null".to_owned(),
            ]),
            ..Default::default()
        };
        (args, schedule, nodes)
    }

    #[test]
    fn candidates_are_scoped_and_leave_manifest_relations_unchanged() {
        let (mut args, schedule, nodes) = fixtures();
        let first = WapPlan::build(&args, &schedule, &nodes).unwrap();
        let entry = first.model("model.pkg.orders").unwrap();
        let candidate = entry.execution_model().unwrap();
        assert_ne!(candidate.base().alias, entry.model.base().alias);
        assert_eq!(candidate.base().schema, entry.model.base().schema);
        assert_eq!(candidate.unique_id(), entry.model.unique_id());
        assert_eq!(nodes.models["model.pkg.orders"].base().alias, "ORDERS");
        assert!(first.audit_owner("test.pkg.orders_not_null").is_some());
        args.io.invocation_id = "00000000-0000-0000-0000-000000000001".parse().unwrap();
        let second = WapPlan::build(&args, &schedule, &nodes).unwrap();
        assert_ne!(
            entry.candidate_identifier,
            second.models["model.pkg.orders"].candidate_identifier
        );
    }

    #[test]
    fn candidate_rendering_preserves_quoted_database_and_schema_names() {
        let (args, schedule, mut nodes) = fixtures();
        let model = Arc::make_mut(nodes.models.get_mut("model.pkg.orders").unwrap());
        model.__base_attr__.database = "DB\"name".to_owned();
        model.__base_attr__.schema = "schema.with.dot".to_owned();
        model.__base_attr__.alias = "Ord\"ers".to_owned();
        let plan = WapPlan::build(&args, &schedule, &nodes).unwrap();
        let entry = &plan.models["model.pkg.orders"];

        let candidate = entry.execution_model().unwrap();
        let candidate_sql = entry.candidate_relation().unwrap().render_self_as_str();
        assert_eq!(
            candidate_sql,
            format!(
                "\"DB\"\"name\".\"schema.with.dot\".\"{}\"",
                entry.candidate_identifier
            )
        );
        assert_eq!(
            candidate.base().relation_name.as_deref(),
            Some(candidate_sql.as_str())
        );
        assert_eq!(
            entry.public_relation().unwrap().render_self_as_str(),
            "\"DB\"\"name\".\"schema.with.dot\".\"Ord\"\"ers\""
        );
        assert_eq!(nodes.models["model.pkg.orders"].base().alias, "Ord\"ers");
    }

    #[test]
    fn materialization_quoting_guard_matches_jinja_relation_creation() {
        let env = minijinja::Environment::new();
        let expression = env
            .compile_expression(
                "api.Relation.create(database=database, schema=schema, identifier=identifier, type='table')",
            )
            .unwrap();
        for (database, schema, model_quoting, effective_quoting, compatible) in [
            (
                "DB",
                "PUBLIC",
                ResolvedQuoting::trues(),
                ResolvedQuoting::falses(),
                true,
            ),
            (
                "Db",
                "PUBLIC",
                ResolvedQuoting::trues(),
                ResolvedQuoting::falses(),
                false,
            ),
            (
                "DB",
                "Public",
                ResolvedQuoting::trues(),
                ResolvedQuoting::falses(),
                false,
            ),
            (
                "db",
                "public",
                ResolvedQuoting::falses(),
                ResolvedQuoting::trues(),
                false,
            ),
            (
                "db",
                "public",
                ResolvedQuoting::trues(),
                ResolvedQuoting::trues(),
                true,
            ),
        ] {
            let (args, schedule, mut nodes) = fixtures();
            let model = Arc::make_mut(nodes.models.get_mut("model.pkg.orders").unwrap());
            model.__base_attr__.database = database.to_owned();
            model.__base_attr__.schema = schema.to_owned();
            model.__base_attr__.quoting = model_quoting;
            let plan = WapPlan::build(&args, &schedule, &nodes).unwrap();
            let entry = &plan.models["model.pkg.orders"];
            let api = BTreeMap::from([(
                "Relation",
                create_static_relation(AdapterType::Snowflake, effective_quoting).unwrap(),
            )]);
            let value = expression
                .eval(
                    minijinja::context! {
                        api => api,
                        database => database,
                        schema => schema,
                        identifier => entry.candidate_identifier.as_str(),
                    },
                    &[],
                )
                .unwrap();
            let actual = value
                .downcast_object_ref::<RelationObject>()
                .unwrap()
                .inner();
            let expected = entry.candidate_relation().unwrap();
            assert_eq!(
                relation_identity(actual.as_ref()).unwrap()
                    == relation_identity(expected.as_ref()).unwrap(),
                compatible,
                "database={database}, schema={schema}",
            );
            assert_eq!(
                entry
                    .validate_materialization_quoting(effective_quoting)
                    .is_ok(),
                compatible,
                "database={database}, schema={schema}",
            );
        }
    }

    #[test]
    fn missing_and_empty_audit_selections_fail_closed() {
        let (args, mut schedule, mut nodes) = fixtures();
        schedule.selected_nodes.remove("test.pkg.orders_not_null");
        assert!(
            WapPlan::build(&args, &schedule, &nodes)
                .unwrap_err()
                .to_string()
                .contains("all WAP audits")
        );
        nodes.tests.clear();
        assert!(
            WapPlan::build(&args, &schedule, &nodes)
                .unwrap_err()
                .to_string()
                .contains("at least one")
        );
    }

    #[test]
    fn generic_test_owner_is_attachment_not_every_dependency() {
        let (args, schedule, mut nodes) = fixtures();
        let mut relationship = DbtTest::default();
        relationship.__common_attr__.unique_id = "test.pkg.downstream_relationship".to_owned();
        relationship.__base_attr__.enabled = true;
        relationship.__base_attr__.depends_on.nodes = vec![
            "model.pkg.orders".to_owned(),
            "model.pkg.downstream".to_owned(),
        ];
        relationship.__test_attr__.attached_node = Some("model.pkg.downstream".to_owned());
        nodes
            .tests
            .insert(relationship.unique_id(), Arc::new(relationship.clone()));
        let plan = WapPlan::build(&args, &schedule, &nodes).unwrap();
        assert!(
            plan.audit_owner("test.pkg.downstream_relationship")
                .is_none()
        );
        relationship.__test_attr__.attached_node = Some("model.pkg.orders".to_owned());
        nodes
            .tests
            .insert(relationship.unique_id(), Arc::new(relationship));
        assert!(
            WapPlan::build(&args, &schedule, &nodes)
                .unwrap_err()
                .to_string()
                .contains("multi-relation")
        );
    }

    #[test]
    fn singular_audits_support_one_relation_and_reject_ambiguous_ownership() {
        let (args, schedule, mut nodes) = fixtures();
        let audit = Arc::make_mut(nodes.tests.get_mut("test.pkg.orders_not_null").unwrap());
        audit.__test_attr__.attached_node = None;
        assert!(WapPlan::build(&args, &schedule, &nodes).is_ok());
        Arc::make_mut(nodes.tests.get_mut("test.pkg.orders_not_null").unwrap())
            .__base_attr__
            .depends_on
            .nodes
            .push("source.pkg.raw".to_owned());
        assert!(WapPlan::build(&args, &schedule, &nodes).is_err());
    }

    #[test]
    fn disabled_audits_are_not_required() {
        let (args, schedule, mut nodes) = fixtures();
        let mut disabled = nodes.tests["test.pkg.orders_not_null"].as_ref().clone();
        disabled.__common_attr__.unique_id = "test.pkg.disabled".to_owned();
        disabled.deprecated_config.enabled = Some(false);
        disabled
            .__base_attr__
            .depends_on
            .nodes
            .push("model.pkg.other".to_owned());
        nodes.tests.insert(disabled.unique_id(), Arc::new(disabled));
        assert!(WapPlan::build(&args, &schedule, &nodes).is_ok());
    }

    #[test]
    fn commands_cannot_bypass_audits() {
        let (mut args, schedule, nodes) = fixtures();
        for command in [FsCommand::Run, FsCommand::Clone] {
            args.command = command;
            assert!(WapPlan::build(&args, &schedule, &nodes).is_err());
        }
        for command in [FsCommand::Test, FsCommand::Compile] {
            args.command = command;
            assert!(
                WapPlan::build(&args, &schedule, &nodes)
                    .unwrap()
                    .models
                    .is_empty()
            );
        }
        args.command = FsCommand::Build;
        args.empty = true;
        assert!(WapPlan::build(&args, &schedule, &nodes).is_err());
        args.empty = false;
        args.sample = Some("10 rows".to_owned());
        assert!(WapPlan::build(&args, &schedule, &nodes).is_err());
        args.sample = None;
        args.local_execution_backend = LocalExecutionBackendKind::Inline;
        assert!(WapPlan::build(&args, &schedule, &nodes).is_err());
    }

    #[test]
    fn test_only_builds_and_unselected_wap_parents_use_public_relations() {
        let (args, mut schedule, mut nodes) = fixtures();
        schedule.selected_nodes.remove("model.pkg.orders");
        schedule
            .frontier_nodes
            .insert("model.pkg.orders".to_owned());
        assert!(
            WapPlan::build(&args, &schedule, &nodes)
                .unwrap()
                .models
                .is_empty()
        );

        schedule
            .selected_nodes
            .insert("model.pkg.orders".to_owned());
        schedule.frontier_nodes.clear();
        Arc::make_mut(nodes.models.get_mut("model.pkg.orders").unwrap())
            .deprecated_config
            .wap = Some(false);
        nodes.tests.clear();
        assert!(
            WapPlan::build(&args, &schedule, &nodes)
                .unwrap()
                .models
                .is_empty()
        );
    }

    #[test]
    fn public_relation_collisions_respect_snowflake_identifier_case() {
        let (args, schedule, mut nodes) = fixtures();
        let mut other = nodes.models["model.pkg.orders"].as_ref().clone();
        other.__common_attr__.unique_id = "model.pkg.other".to_owned();
        other.__base_attr__.alias = "orders".to_owned();
        other.__base_attr__.quoting.identifier = false;
        nodes.models.insert(other.unique_id(), Arc::new(other));
        assert!(
            WapPlan::build(&args, &schedule, &nodes)
                .unwrap_err()
                .to_string()
                .contains("shares its public relation")
        );
        Arc::make_mut(nodes.models.get_mut("model.pkg.other").unwrap())
            .__base_attr__
            .quoting
            .identifier = true;
        let plan = WapPlan::build(&args, &schedule, &nodes);
        assert!(plan.is_ok(), "{plan:?}");
    }

    #[test]
    fn relation_identity_preserves_component_boundaries() {
        let (args, schedule, mut nodes) = fixtures();
        let model = Arc::make_mut(nodes.models.get_mut("model.pkg.orders").unwrap());
        model.__base_attr__.database = "A.B".to_owned();
        model.__base_attr__.schema = "C".to_owned();
        let mut other = model.clone();
        other.__common_attr__.unique_id = "model.pkg.other".to_owned();
        other.__base_attr__.database = "A".to_owned();
        other.__base_attr__.schema = "B.C".to_owned();
        nodes.models.insert(other.unique_id(), Arc::new(other));
        assert!(WapPlan::build(&args, &schedule, &nodes).is_ok());
    }

    #[test]
    fn audit_failure_storage_cannot_replace_public_or_candidate_tables() {
        let (args, schedule, mut nodes) = fixtures();
        let plan = WapPlan::build(&args, &schedule, &nodes).unwrap();
        let candidate = plan.models["model.pkg.orders"].candidate_identifier.clone();
        for alias in ["ORDERS", candidate.as_str()] {
            for (store_failures, store_failures_as) in [
                (Some(true), None),
                (Some(true), Some(StoreFailuresAs::Table)),
                (None, Some(StoreFailuresAs::View)),
            ] {
                let audit = Arc::make_mut(nodes.tests.get_mut("test.pkg.orders_not_null").unwrap());
                audit.__base_attr__.database = "DB".to_owned();
                audit.__base_attr__.schema = "PUBLIC".to_owned();
                audit.__base_attr__.alias = alias.to_ascii_lowercase();
                audit.__base_attr__.quoting.identifier = true;
                audit.deprecated_config.store_failures = store_failures;
                audit.deprecated_config.store_failures_as = store_failures_as;
                let error = plan
                    .validate_test_storage_relations(
                        &nodes,
                        ResolvedQuoting {
                            database: true,
                            schema: true,
                            identifier: false,
                        },
                    )
                    .unwrap_err();
                assert!(error.to_string().contains("test.pkg.orders_not_null"));
            }
        }
    }

    #[test]
    fn distinct_failure_storage_and_non_storing_tests_remain_supported() {
        let (args, schedule, mut nodes) = fixtures();
        let plan = WapPlan::build(&args, &schedule, &nodes).unwrap();
        for storage_type in [StoreFailuresAs::Table, StoreFailuresAs::View] {
            let audit = Arc::make_mut(nodes.tests.get_mut("test.pkg.orders_not_null").unwrap());
            audit.__base_attr__.database = "DB".to_owned();
            audit.__base_attr__.schema = "PUBLIC".to_owned();
            audit.__base_attr__.alias = "ORDERS_FAILURES".to_owned();
            audit.deprecated_config.store_failures = Some(true);
            audit.deprecated_config.store_failures_as = Some(storage_type);
            assert!(
                plan.validate_test_storage_relations(&nodes, ResolvedQuoting::trues())
                    .is_ok()
            );
        }

        let audit = Arc::make_mut(nodes.tests.get_mut("test.pkg.orders_not_null").unwrap());
        audit.__base_attr__.alias = "ORDERS".to_owned();
        audit.deprecated_config.store_failures = Some(false);
        audit.deprecated_config.store_failures_as = Some(StoreFailuresAs::Ephemeral);
        assert!(
            plan.validate_test_storage_relations(&nodes, ResolvedQuoting::trues())
                .is_ok()
        );

        let mut disabled = nodes.tests["test.pkg.orders_not_null"].as_ref().clone();
        disabled.__common_attr__.unique_id = "test.pkg.disabled_storage".to_owned();
        disabled.deprecated_config.store_failures = Some(true);
        disabled.deprecated_config.store_failures_as = Some(StoreFailuresAs::Table);
        disabled.deprecated_config.enabled = Some(false);
        nodes.tests.insert(disabled.unique_id(), Arc::new(disabled));
        assert!(
            plan.validate_test_storage_relations(&nodes, ResolvedQuoting::trues())
                .is_ok()
        );
    }

    #[test]
    fn storage_collision_check_uses_runtime_quoting_and_all_enabled_tests() {
        let (args, schedule, mut nodes) = fixtures();
        let plan = WapPlan::build(&args, &schedule, &nodes).unwrap();
        let mut other = nodes.tests["test.pkg.orders_not_null"].as_ref().clone();
        other.__common_attr__.unique_id = "test.pkg.other_storage".to_owned();
        other.__test_attr__.attached_node = Some("model.pkg.other".to_owned());
        other.__base_attr__.depends_on.nodes = vec!["model.pkg.other".to_owned()];
        other.__base_attr__.database = "DB".to_owned();
        other.__base_attr__.schema = "PUBLIC".to_owned();
        other.__base_attr__.alias = "orders".to_owned();
        other.__base_attr__.quoting.identifier = false;
        other.deprecated_config.store_failures = Some(true);
        nodes.tests.insert(other.unique_id(), Arc::new(other));
        assert!(
            plan.validate_test_storage_relations(&nodes, ResolvedQuoting::trues())
                .is_ok()
        );
        assert!(
            plan.validate_test_storage_relations(&nodes, ResolvedQuoting::falses())
                .unwrap_err()
                .to_string()
                .contains("test.pkg.other_storage")
        );
    }
}
