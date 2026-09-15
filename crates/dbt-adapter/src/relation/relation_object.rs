use dbt_adapter_core::AdapterType;
use dbt_common::{ErrorCode, FsError, FsResult, fs_err};
use dbt_schemas::dbt_types::RelationType;
use dbt_schemas::filter::RunFilter;
use dbt_schemas::schemas::common::{DbtQuoting, ResolvedQuoting};
use dbt_schemas::schemas::dbt_catalogs_v2::CatalogType;
use dbt_schemas::schemas::relations::base::BaseRelation;
use dbt_schemas::schemas::serde::minijinja_value_to_typed_struct;
use dbt_schemas::schemas::{DbtSource, InternalDbtNodeAttributes, InternalDbtNodeWrapper};
use dbt_yaml as yml;
use minijinja::arg_utils::{ArgParser, ArgsIter};
use minijinja::value::{Enumerator, Object, ValueKind};
use minijinja::{State, Value, listener::RenderingEventListener};
use serde::Deserialize;

use crate::relation::Relation;
use crate::relation::databricks::typed_constraint::TypedConstraint;
use crate::value::none_value;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::{fmt, ops::Deref};

/// A Wrapper type for BaseRelation
/// for any concrete Relation type to be used as Object in Jinja
#[derive(Clone)]
pub struct RelationObject {
    relation: Arc<dyn BaseRelation>,
    run_filter: Option<RunFilter>,
    event_time: Option<String>,
}

impl RelationObject {
    pub fn new(relation: Arc<dyn BaseRelation>) -> Self {
        Self {
            relation,
            run_filter: None,
            event_time: None,
        }
    }

    pub fn new_with_filter(
        relation: Arc<dyn BaseRelation>,
        run_filter: RunFilter,
        event_time: Option<String>,
    ) -> Self {
        Self {
            relation,
            run_filter: Some(run_filter),
            event_time,
        }
    }

    pub fn into_value(self) -> Value {
        Value::from_object(self)
    }

    pub fn inner(&self) -> Arc<dyn BaseRelation> {
        self.relation.clone()
    }

    /// Whether this relation is the placeholder returned during parsing.
    pub fn is_parse_time(&self) -> bool {
        self.relation
            .as_any()
            .downcast_ref::<Relation>()
            .is_some_and(|relation| relation.is_parse_time)
    }

    /// Create a new RelationObject with a run filter applied.
    ///
    /// This is used for microbatch execution to filter refs by event_time.
    pub fn with_filter(&self, run_filter: RunFilter, event_time: Option<String>) -> Self {
        let empty = run_filter.empty || self.run_filter.as_ref().is_some_and(|f| f.empty);
        Self {
            relation: self.relation.clone(),
            run_filter: Some(RunFilter {
                empty,
                ..run_filter
            }),
            event_time,
        }
    }

    pub fn has_filter(&self) -> bool {
        self.run_filter.is_some()
    }

    pub fn event_time(&self) -> Option<&str> {
        self.event_time.as_deref()
    }

    /// Databricks: enrich relation with constraints (for get_column_and_constraints_sql)
    fn relation_enrich(self: &Arc<Self>, args: &[Value]) -> Result<Value, minijinja::Error> {
        let dbx = self
            .relation
            .as_any()
            .downcast_ref::<Relation>()
            .ok_or_else(|| {
                minijinja::Error::new(
                    minijinja::ErrorKind::InvalidOperation,
                    "enrich is only available for Databricks relations",
                )
            })?;
        let constraints_val = args.first().cloned().unwrap_or_default();
        let constraints: Vec<TypedConstraint> = constraints_val
            .try_iter()
            .map_err(|e| {
                minijinja::Error::new(
                    minijinja::ErrorKind::InvalidOperation,
                    format!("enrich constraints must be iterable: {e}"),
                )
            })?
            .map(|v| {
                v.downcast_object_ref::<TypedConstraint>()
                    .cloned()
                    .ok_or_else(|| {
                        minijinja::Error::new(
                            minijinja::ErrorKind::InvalidOperation,
                            "enrich constraints must contain TypedConstraint objects",
                        )
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let enriched = dbx.enrich(&constraints);
        Ok(RelationObject::new(Arc::new(enriched)).into_value())
    }

    /// Databricks: render constraints DDL for CREATE TABLE
    fn relation_render_constraints_for_create(self: &Arc<Self>) -> Result<Value, minijinja::Error> {
        let dbx = self
            .relation
            .as_any()
            .downcast_ref::<Relation>()
            .ok_or_else(|| {
                minijinja::Error::new(
                    minijinja::ErrorKind::InvalidOperation,
                    "render_constraints_for_create is only available for Databricks relations",
                )
            })?;
        Ok(Value::from(dbx.render_constraints_for_create()))
    }
}

/// Always returns the unfiltered relation string (via [`BaseRelation::render_self_as_str`]),
/// reference: https://github.com/dbt-labs/dbt-adapters/blob/616a8d3cb595605872c011070c240e7a2b825d79/dbt-adapters/src/dbt/adapters/base/relation.py#L268-L269
fn render_without_filter(ro: &Arc<RelationObject>) -> Value {
    let rendered = ro.render_self_as_str();
    if rendered.is_empty() {
        none_value()
    } else {
        Value::from(rendered)
    }
}

impl fmt::Debug for RelationObject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.render_self_as_str())
    }
}

impl Deref for RelationObject {
    type Target = dyn BaseRelation;

    fn deref(&self) -> &Self::Target {
        self.relation.as_ref()
    }
}

impl From<Arc<dyn BaseRelation>> for RelationObject {
    fn from(relation: Arc<dyn BaseRelation>) -> Self {
        RelationObject::new(relation)
    }
}

impl From<Box<dyn BaseRelation>> for RelationObject {
    fn from(relation: Box<dyn BaseRelation>) -> Self {
        RelationObject::new(Arc::from(relation))
    }
}

impl Object for RelationObject {
    fn is_true(self: &Arc<Self>) -> bool {
        !self.is_parse_time()
    }

    fn call_method(
        self: &Arc<Self>,
        _state: &State,
        name: &str,
        args: &[Value],
        _listeners: &[std::rc::Rc<dyn RenderingEventListener>],
    ) -> Result<Value, minijinja::Error> {
        match name {
            "create_from" => self
                .create_from()
                .map(|r| Value::from_object(RelationObject::new(r))),
            "replace_path" => {
                let mut args = ArgParser::new(args, None);
                let database: Option<String> = args.consume_optional_only_from_kwargs("database");
                let schema: Option<String> = args.consume_optional_only_from_kwargs("schema");
                let identifier: Option<String> =
                    args.consume_optional_only_from_kwargs("identifier");
                self.replace_path(database, schema, identifier)
                    .map(|r| Value::from_object(RelationObject::new(r)))
            }
            "get" => {
                let mut args = ArgParser::new(args, None);
                let key: String = args.get("key").unwrap();
                let default: Option<Value> = args.get("default").ok();
                self.get(&key, default)
            }
            "render" => Ok(render_without_filter(self)),
            "derivative" => {
                let iter = ArgsIter::new("derivative", &["suffix", "relation_type"], args);
                let suffix = iter.next_arg::<&str>()?;
                let relation_type = iter.next_arg::<Option<&str>>()?;
                let interpret_suffix_as_full_identifier = iter
                    .next_kwarg::<Option<bool>>("interpret_suffix_as_full_identifier")?
                    .unwrap_or(false);
                iter.finish()?;
                let relation_type = relation_type
                    .filter(|s| !s.is_empty())
                    .map(RelationType::from);
                self.derivative(suffix, relation_type, interpret_suffix_as_full_identifier)
                    .map(|r| Value::from_object(RelationObject::new(r)))
            }
            "without_identifier" => self
                .without_identifier()
                .map(|r| Value::from_object(RelationObject::new(r))),
            "include" => {
                let mut args = ArgParser::new(args, None);
                let database: Option<bool> = args.consume_optional_only_from_kwargs("database");
                let schema: Option<bool> = args.consume_optional_only_from_kwargs("schema");
                let identifier: Option<bool> = args.consume_optional_only_from_kwargs("identifier");
                self.include(database, schema, identifier)
                    .map(|r| Value::from_object(RelationObject::new(r)))
            }
            "quote" => {
                let mut args = ArgParser::new(args, None);
                let database: Option<bool> = args.consume_optional_only_from_kwargs("database");
                let schema: Option<bool> = args.consume_optional_only_from_kwargs("schema");
                let identifier: Option<bool> = args.consume_optional_only_from_kwargs("identifier");
                self.quote(database, schema, identifier)
                    .map(|r| Value::from_object(RelationObject::new(r)))
            }
            "incorporate" => {
                let mut args = ArgParser::new(args, None);
                let path: Option<Value> = args.consume_optional_only_from_kwargs("path");
                let relation_type_val: Option<Value> =
                    args.consume_optional_only_from_kwargs("type");
                let location: Option<String> = args.consume_optional_only_from_kwargs("location");
                let relation_type = relation_type_val.and_then(|v| {
                    if v.is_none() || v.is_undefined() {
                        None
                    } else {
                        v.as_str().map(RelationType::from)
                    }
                });
                self.incorporate(path, relation_type, location)
                    .map(|r| Value::from_object(RelationObject::new(r)))
            }
            "information_schema" => {
                let iter = ArgsIter::new("information_schema", &["view_name"], args);
                // FIXME: An empty view name is actually illegal in BigQuery. What does it
                // mean to call this with `None` as an argument? Should we error instead?
                let view_name =
                    iter.next_kwarg_aliased::<Option<&str>>("view_name", &["identifier"])?;
                iter.finish()?;
                self.information_schema(view_name.unwrap_or_default())
                    .map(|r| Value::from_object(RelationObject::new(r)))
            }
            "relation_max_name_length" => self.relation_max_name_length().map(Value::from),
            // Below are available for Snowflake
            "get_ddl_prefix_for_create" => {
                let iter = ArgsIter::new(
                    "get_ddl_prefix_for_create",
                    &["model_config", "temporary"],
                    args,
                );
                let model_config = iter.next_arg::<Value>()?;
                let temporary = iter.next_arg::<bool>()?;
                iter.finish()?;
                self.get_ddl_prefix_for_create(model_config, temporary)
                    .map(Value::from)
            }
            "get_ddl_prefix_for_alter" => self.get_ddl_prefix_for_alter().map(Value::from),
            "needs_to_drop" => {
                let iter = ArgsIter::new("needs_to_drop", &["old_relation"], args);
                let value = iter.next_arg::<Value>()?;
                iter.finish()?;
                let old_relation = value
                    .downcast_object::<RelationObject>()
                    .map(|ro| ro.inner());
                self.needs_to_drop(old_relation).map(Value::from)
            }
            "get_iceberg_ddl_options" => {
                let iter = ArgsIter::new("get_iceberg_ddl_options", &["config"], args);
                let config = iter.next_arg::<Value>()?;
                iter.finish()?;
                self.get_iceberg_ddl_options(config).map(|opts| {
                    if opts.is_empty() {
                        none_value()
                    } else {
                        Value::from(opts)
                    }
                })
            }
            "dynamic_table_config_changeset" | "interactive_table_config_changeset" => {
                let iter = ArgsIter::new(name, &["relation_results", "relation_config"], args);
                let relation_results = iter.next_arg::<Value>()?;
                let relation_config = iter.next_arg::<Value>()?;
                iter.finish()?;
                self.dynamic_table_config_changeset(&relation_results, &relation_config)
            }
            "from_config" => {
                let iter = ArgsIter::new("from_config", &["config"], args);
                let config = iter.next_arg::<Value>()?;
                iter.finish()?;
                self.from_config(&config)
            }
            // Below are available for Databricks
            "is_hive_metastore" => Ok(Value::from(self.is_hive_metastore())),
            "enrich" => self.relation_enrich(args),
            "render_constraints_for_create" => self.relation_render_constraints_for_create(),
            // Below are available for BigQuery and Redshift
            "materialized_view_config_changeset" => {
                let iter = ArgsIter::new(
                    "materialized_view_config_changeset",
                    &["relation_results", "relation_config"],
                    args,
                );
                let relation_results = iter.next_arg::<Value>()?;
                let relation_config = iter.next_arg::<Value>()?;
                iter.finish()?;
                self.materialized_view_config_changeset(&relation_results, &relation_config)
            }
            _ => Err(minijinja::Error::new(
                minijinja::ErrorKind::UnknownMethod,
                format!("Unknown method on BaseRelationObject: '{name}'"),
            )),
        }
    }

    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        match key.as_str() {
            Some("database") => Some(Value::from(self.database())),
            Some("schema") => Some(Value::from(self.schema())),
            Some("identifier") | Some("name") | Some("table") => {
                Some(Value::from(self.identifier()))
            }

            Some("is_table") => Some(Value::from(self.is_table())),
            Some("is_delta") => Some(Value::from(self.is_delta())),
            Some("is_shallow_clone") => Some(Value::from(self.is_shallow_clone())),
            Some("alter_constraints") => {
                let dbx = self.relation.as_any().downcast_ref::<Relation>()?;
                Some(Value::from_iter(
                    dbx.alter_constraints
                        .iter()
                        .cloned()
                        .map(Value::from_object),
                ))
            }
            Some("create_constraints") => {
                let dbx = self.relation.as_any().downcast_ref::<Relation>()?;
                Some(Value::from_iter(
                    dbx.create_constraints
                        .iter()
                        .cloned()
                        .map(Value::from_object),
                ))
            }
            Some("is_view") => Some(Value::from(self.is_view())),
            Some("is_materialized_view") => Some(Value::from(self.is_materialized_view())),
            Some("is_metric_view") => Some(Value::from(self.is_metric_view())),
            Some("is_streaming_table") => Some(Value::from(self.is_streaming_table())),
            Some("is_dynamic_table") => Some(Value::from(self.is_dynamic_table())),
            Some("is_interactive_table") => Some(Value::from(self.is_interactive_table())),
            Some("is_iceberg_format") => Some(Value::from(self.is_iceberg_format())),
            Some("is_cte") => Some(Value::from(self.is_cte())),
            Some("is_pointer") => Some(Value::from(self.is_pointer())),
            Some("temporary") => Some(Value::from(self.is_temporary())),
            Some("type") => Some(Value::from_serialize(self.relation_type())),
            Some("can_be_renamed") => Some(Value::from(self.can_be_renamed())),
            Some("can_be_replaced") => Some(Value::from(self.can_be_replaced())),
            Some("MaterializedView") => {
                Some(Value::from(RelationType::MaterializedView.to_string()))
            }
            Some("Table") => Some(Value::from(RelationType::Table.to_string())),
            Some("DynamicTable") => Some(Value::from(RelationType::DynamicTable.to_string())),
            Some("InteractiveTable") => {
                Some(Value::from(RelationType::InteractiveTable.to_string()))
            }
            Some("StreamingTable") => Some(Value::from(RelationType::StreamingTable.to_string())),
            // the Jinja logics `if resolved.render is defined and resolved.render is callable `
            // in `macro build_ref_function` depends on this
            Some("render") => {
                let this = Arc::clone(self);
                Some(Value::from_func_func("render", move |_state, _args| {
                    Ok(render_without_filter(&this))
                }))
            }
            // BigQuery
            Some("location") => Some(Value::from(self.location())),
            Some("project") => Some(Value::from(self.database())),
            Some("dataset") => Some(Value::from(self.schema())),

            // ClickHouse
            Some("can_exchange") => Some(Value::from(self.can_exchange())),
            Some("mvs_pointing_to_it") => Some(Value::from_serialize(self.mvs_pointing_to_it())),
            Some("is_refreshable") => Some(Value::from(self.is_refreshable())),
            Some("refreshable_append") => Some(Value::from(self.refreshable_append())),

            _ => None,
        }
    }

    fn enumerate(self: &Arc<Self>) -> Enumerator {
        Enumerator::Str(&[
            "database",
            "schema",
            "identifier",
            "is_table",
            "is_view",
            "is_materialized_view",
            "is_metric_view",
            "is_streaming_table",
            "is_cte",
            "is_pointer",
            "can_be_renamed",
            "can_be_replaced",
            "name",
        ])
    }

    fn render(self: &Arc<Self>, f: &mut fmt::Formatter<'_>) -> fmt::Result
    where
        Self: Sized + 'static,
    {
        let rendered = match self.run_filter {
            Some(ref run_filter) if run_filter.enabled() => {
                self.render_with_run_filter(run_filter, &self.event_time)
            }
            _ => self.render_self_as_str(),
        };

        let jinja_render = if rendered.is_empty() {
            "None"
        } else {
            &rendered
        };

        write!(f, "{}", jinja_render)
    }
}

/// Whether a Jinja value contains a parse-time relation placeholder.
pub fn is_parse_time_relation(value: &Value) -> bool {
    value
        .downcast_object_ref::<RelationObject>()
        .is_some_and(RelationObject::is_parse_time)
}

/// Creates a relation based on the adapter type
///
/// This is supposed to be used in places that are invoked by the Jinja rendering process
pub fn do_create_relation(
    adapter_type: AdapterType,
    database: String,
    schema: String,
    identifier: Option<String>,
    relation_type: Option<RelationType>,
    custom_quoting: ResolvedQuoting,
) -> Result<Box<dyn BaseRelation>, minijinja::Error> {
    Relation::new(adapter_type, Some(database), Some(schema), identifier)
        .with_relation_type(relation_type)
        .with_quoting(custom_quoting)
        .validate()
        .map(|r| Box::new(r) as Box<dyn BaseRelation>)
}

/// Creates a relation based on the adapter type
///
/// This is a wrapper around the [create_relation] function
/// that is supposed to be used outside the context of Jinja
pub fn create_relation(
    adapter_type: AdapterType,
    database: String,
    schema: String,
    identifier: Option<String>,
    relation_type: Option<RelationType>,
    custom_quoting: ResolvedQuoting,
) -> FsResult<Box<dyn BaseRelation>> {
    let result = do_create_relation(
        adapter_type,
        database,
        schema,
        identifier,
        relation_type,
        custom_quoting,
    )
    .map_err(|e| FsError::from_jinja_err(e, "Failed to create relation"))?;
    Ok(result)
}

pub fn create_relation_from_source(
    adapter_type: AdapterType,
    database: String,
    schema: String,
    identifier: String,
    custom_quoting: ResolvedQuoting,
    source: &DbtSource,
) -> FsResult<Box<dyn BaseRelation>> {
    // A source's `catalog_name` (when configured) names the `catalogs.yml`
    // entry this source is actually read through -- e.g. an AWS Glue
    // catalog for dbt Compute. It takes over as the relation's leading
    // identifier so `database` can stay a descriptive label instead of
    // having to spell the catalog's own name (the old, implicit coupling).
    let database = source
        .__source_attr__
        .catalog_name
        .clone()
        .unwrap_or(database);

    if adapter_type == AdapterType::DuckDB
        && let Some(external) = duckdb_external_location_for_source(source)?
    {
        return Ok(Box::new(
            Relation::new(AdapterType::DuckDB, database, schema, identifier)
                .with_quoting(custom_quoting)
                .with_external(external),
        ));
    }

    create_relation(
        adapter_type,
        database,
        schema,
        Some(identifier),
        None,
        custom_quoting,
    )
}

pub fn create_relation_from_node(
    adapter_type: AdapterType,
    node: &dyn InternalDbtNodeAttributes,
    _sample_config: Option<RunFilter>,
) -> FsResult<Box<dyn BaseRelation>> {
    create_relation(
        adapter_type,
        node.database(),
        node.schema(),
        Some(node.base().alias.clone()), // all identifiers are consolidated to alias in InternalDbtNode
        Some(RelationType::from(node.materialized())),
        node.quoting(),
    )
}

fn duckdb_external_location_for_source(source: &DbtSource) -> FsResult<Option<String>> {
    let Some(external_location) = source_config_value(source, "external_location") else {
        return Ok(None);
    };

    let formatter = source_config_value(source, "formatter").unwrap_or_else(|| "newstyle".into());
    let context = source_format_context(source);
    let formatted = match formatter.as_str() {
        "newstyle" => format_newstyle(&external_location, &context),
        "oldstyle" => format_oldstyle(&external_location, &context),
        "template" => format_template(&external_location, &context),
        other => {
            return Err(fs_err!(
                ErrorCode::InvalidConfig,
                "Formatter {other} not recognized. Must be one of 'newstyle', 'oldstyle', or 'template'."
            ));
        }
    };

    let with_root = prefix_local_filesystem_root(source, &formatted);
    Ok(Some(quote_duckdb_external_location(&with_root)))
}

fn source_config_value(source: &DbtSource, key: &str) -> Option<String> {
    let from_meta = source.common().meta.get(key).and_then(yml_value_as_string);
    match key {
        "external_location" => source
            .deprecated_config
            .external_location
            .clone()
            .or(from_meta),
        "formatter" => source.deprecated_config.formatter.clone().or(from_meta),
        _ => from_meta,
    }
}

fn yml_value_as_string(value: &yml::Value) -> Option<String> {
    value.as_str().map(ToString::to_string)
}

fn source_format_context(source: &DbtSource) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("name".to_string(), source.common().name.clone()),
        (
            "identifier".to_string(),
            source.__source_attr__.identifier.clone(),
        ),
        ("schema".to_string(), source.base().schema.clone()),
        ("database".to_string(), source.base().database.clone()),
        (
            "source_name".to_string(),
            source.__source_attr__.source_name.clone(),
        ),
    ])
}

fn format_newstyle(template: &str, context: &BTreeMap<String, String>) -> String {
    context
        .iter()
        .fold(template.to_string(), |acc, (key, value)| {
            acc.replace(&format!("{{{key}}}"), value)
        })
}

fn format_oldstyle(template: &str, context: &BTreeMap<String, String>) -> String {
    context
        .iter()
        .fold(template.to_string(), |acc, (key, value)| {
            acc.replace(&format!("%({key})s"), value)
        })
}

fn format_template(template: &str, context: &BTreeMap<String, String>) -> String {
    context
        .iter()
        .fold(template.to_string(), |acc, (key, value)| {
            acc.replace(&format!("${{{key}}}"), value)
                .replace(&format!("${key}"), value)
        })
}

fn prefix_local_filesystem_root(source: &DbtSource, location: &str) -> String {
    let Some(root) = duckdb_local_filesystem_root(source) else {
        return location.to_string();
    };
    let trimmed = location.trim();
    if trimmed.starts_with('\'')
        || trimmed.starts_with('/')
        || trimmed.contains("://")
        || looks_like_function_call(trimmed)
    {
        return location.to_string();
    }

    format!(
        "{}/{}",
        root.trim_end_matches('/'),
        trimmed.trim_start_matches('/')
    )
}

fn duckdb_local_filesystem_root(source: &DbtSource) -> Option<String> {
    let catalog_name = source
        .deprecated_config
        .__warehouse_specific_config__
        .catalog_name
        .as_deref()?;
    let catalogs = crate::load_catalogs::fetch_catalogs()?;
    let view = catalogs.view_v2().ok()?;
    let catalog = view
        .catalogs
        .iter()
        .find(|catalog| catalog.name.eq_ignore_ascii_case(catalog_name))?;
    if catalog.catalog_type != CatalogType::LocalFilesystem {
        return None;
    }
    let duckdb = catalog.config_block("duckdb")?;
    duckdb
        .get(yml::Value::from("root_path"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
}

fn quote_duckdb_external_location(location: &str) -> String {
    let trimmed = location.trim();
    if trimmed.starts_with('\'') || looks_like_function_call(trimmed) {
        trimmed.to_string()
    } else {
        format!("'{}'", trimmed.replace('\'', "''"))
    }
}

fn looks_like_function_call(value: &str) -> bool {
    let Some(open_idx) = value.find('(') else {
        return false;
    };
    value.ends_with(')')
        && value[..open_idx]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[derive(Debug)]
struct QuotePolicyObject(ResolvedQuoting);

impl Object for QuotePolicyObject {
    fn call_method(
        self: &Arc<Self>,
        _state: &State,
        name: &str,
        args: &[Value],
        _listeners: &[std::rc::Rc<dyn RenderingEventListener>],
    ) -> Result<Value, minijinja::Error> {
        match name {
            "get_part" => {
                let iter = ArgsIter::new("QuotePolicy.args", &[], args);
                let name = iter.next_kwarg::<String>("name")?;
                iter.finish()?;

                match name.as_str() {
                    "database" => Ok(Value::from(self.0.database)),
                    "schema" => Ok(Value::from(self.0.schema)),
                    "identifier" => Ok(Value::from(self.0.identifier)),
                    _ => Err(minijinja::Error::new(
                        minijinja::ErrorKind::InvalidArgument,
                        format!("'{name}' is not a valid argument"),
                    )),
                }
            }
            _ => Err(minijinja::Error::new(
                minijinja::ErrorKind::UnknownMethod,
                format!("Unknown method on DefaultQuotePolicyObject: '{name}'"),
            )),
        }
    }
}

/// A Wrapper type for StaticBaseRelation
/// for any concrete StaticBaseRelation type to be used as Object in Jinja
/// to expose static methods via api.Relation
#[derive(Debug, Clone)]
pub struct StaticBaseRelationObject(Arc<dyn StaticBaseRelation>);

impl StaticBaseRelationObject {
    pub fn new(relation: Arc<dyn StaticBaseRelation>) -> Self {
        Self(relation)
    }
}

impl Deref for StaticBaseRelationObject {
    type Target = dyn StaticBaseRelation;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

impl Object for StaticBaseRelationObject {
    fn call_method(
        self: &Arc<Self>,
        _state: &State,
        name: &str,
        args: &[Value],
        _listeners: &[std::rc::Rc<dyn RenderingEventListener>],
    ) -> Result<Value, minijinja::Error> {
        match name {
            "create" => self.create(args),
            "scd_args" => self.scd_args(args),
            // // The following is required by BigQuery materialized_views
            "materialized_view_from_relation_config" => {
                if self.0.get_adapter_type() != AdapterType::Bigquery.as_ref() {
                    return Err(minijinja::Error::new(
                        minijinja::ErrorKind::InvalidOperation,
                        "'materialized_view_from_relation_config' can only be invoked using the BigQuery adapter",
                    ));
                }

                let iter = ArgsIter::new(
                    "Relation.materialized_view_from_relation_config",
                    &["local_config"],
                    args,
                );
                let local_config_value = iter.next_arg::<&Value>()?;
                iter.finish()?;

                let local_config = minijinja_value_to_typed_struct::<InternalDbtNodeWrapper>(
                    local_config_value.clone(),
                )
                .map_err(|e| {
                    minijinja::Error::new(
                        minijinja::ErrorKind::SerdeDeserializeError,
                        format!(
                            "get_table_options: Failed to deserialize InternalDbtNodeWrapper: {e}"
                        ),
                    )
                })?;

                let loader =
                    crate::relation::bigquery::config::relation_types::materialized_view::new_loader();
                let relation_config = loader
                    .from_local_config(local_config.as_internal_node())
                    .map_err(|err| {
                        minijinja::Error::new(
                            minijinja::ErrorKind::InvalidOperation,
                            format!("error while loading local materialized view config: {err}"),
                        )
                    })?;
                Ok(Value::from_object(relation_config))
            }
            "get_default_quote_policy" => {
                let iter = ArgsIter::new("Relation.get_default_quote_policy", &[], args);
                iter.finish()?;
                Ok(Value::from_object(QuotePolicyObject(
                    self.0.get_default_quoting(),
                )))
            }
            _ => Err(minijinja::Error::new(
                minijinja::ErrorKind::UnknownMethod,
                format!("Unknown method on StaticBaseRelationObject: '{name}'"),
            )),
        }
    }
}

/// Trait for static methods on relations
pub trait StaticBaseRelation: fmt::Debug + Send + Sync {
    /// Create a new relation from the given arguments
    fn try_new(
        &self,
        database: Option<String>,
        schema: Option<String>,
        identifier: Option<String>,
        relation_type: Option<RelationType>,
        custom_quoting: Option<ResolvedQuoting>,
        temporary: Option<bool>,
    ) -> Result<Value, minijinja::Error>;

    fn get_adapter_type(&self) -> String;

    fn get_default_quoting(&self) -> ResolvedQuoting;

    /// Create a new relation from the given arguments
    /// impl for api.Relation.create
    fn create(&self, args: &[Value]) -> Result<Value, minijinja::Error> {
        let iter = ArgsIter::new("Relation.create", &[], args);
        let database = iter.next_kwarg::<Option<String>>("database")?;
        let schema = iter.next_kwarg::<Option<String>>("schema")?;
        let identifier = iter.next_kwarg::<Option<String>>("identifier")?;
        let relation_type = iter.next_kwarg::<Option<Value>>("type")?;
        let custom_quoting = iter.next_kwarg::<Option<Value>>("quote_policy")?;
        let temporary = iter.next_kwarg::<Option<bool>>("temporary")?;
        iter.finish()?;

        // error is intentionally silenced
        let custom_quoting = custom_quoting
            .and_then(|v| DbtQuoting::deserialize(v).ok())
            // when missing, defaults to be non-quoted
            .map(|v| ResolvedQuoting {
                database: v.database.unwrap_or_default(),
                identifier: v.identifier.unwrap_or_default(),
                schema: v.schema.unwrap_or_default(),
            });

        self.try_new(
            database,
            schema,
            identifier,
            relation_type.and_then(|v: Value| {
                if v.is_none() || v.is_undefined() {
                    None
                } else {
                    Some(RelationType::from(v.as_str().unwrap_or_default()))
                }
            }),
            custom_quoting,
            temporary,
        )
    }

    fn scd_args(&self, args: &[Value]) -> Result<Value, minijinja::Error> {
        let iter = ArgsIter::new("Relation.scd_args", &[], args);
        let primary_key = iter.next_kwarg::<Value>("primary_key")?;
        let updated_at = iter.next_kwarg::<String>("updated_at")?;
        iter.finish()?;

        let mut scd_args = vec![];
        match primary_key.kind() {
            ValueKind::Seq => {
                scd_args.extend(primary_key.try_iter()?.enumerate().map(|s| s.1.to_string()));
            }
            ValueKind::String => {
                scd_args.push(primary_key.as_str().unwrap().to_string());
            }
            _ => {
                return Err(minijinja::Error::new(
                    minijinja::ErrorKind::InvalidOperation,
                    format!(
                        "'primary_key' has a wrong type in StaticBaseRelationObject: '{primary_key}'"
                    ),
                ));
            }
        }
        scd_args.push(updated_at);
        Ok(Value::from(scd_args))
    }
}

#[cfg(test)]
mod tests {
    use crate::relation::factory::create_static_relation;

    use super::*;
    use dbt_schemas::schemas::relations::DEFAULT_RESOLVED_QUOTING;
    use minijinja_contrib::testing::jinja_assert;

    fn source_with_meta_location(location: &str) -> DbtSource {
        let mut source = DbtSource::default();
        source.__common_attr__.name = "orders".to_string();
        source
            .__common_attr__
            .meta
            .insert("external_location".to_string(), yml::Value::from(location));
        source.__base_attr__.database = "main".to_string();
        source.__base_attr__.schema = "raw".to_string();
        source.__base_attr__.alias = "orders".to_string();
        source.__base_attr__.quoting = DEFAULT_RESOLVED_QUOTING;
        source.__source_attr__.identifier = "orders".to_string();
        source.__source_attr__.source_name = "raw".to_string();
        source
    }

    #[test]
    fn duckdb_source_external_location_formats_and_quotes_path() {
        let source = source_with_meta_location("data/{name}.csv");

        let relation = create_relation_from_source(
            AdapterType::DuckDB,
            "main".to_string(),
            "raw".to_string(),
            "orders".to_string(),
            DEFAULT_RESOLVED_QUOTING,
            &source,
        )
        .unwrap();

        assert_eq!(relation.render_self_as_str(), "'data/orders.csv'");
    }

    #[test]
    fn duckdb_source_external_location_keeps_function_call_unquoted() {
        let mut source = source_with_meta_location("ignored/{name}.csv");
        source.deprecated_config.external_location = Some("read_csv('orders.csv')".to_string());

        let relation = create_relation_from_source(
            AdapterType::DuckDB,
            "main".to_string(),
            "raw".to_string(),
            "orders".to_string(),
            DEFAULT_RESOLVED_QUOTING,
            &source,
        )
        .unwrap();

        assert_eq!(relation.render_self_as_str(), "read_csv('orders.csv')");
    }

    #[test]
    fn source_catalog_name_overrides_database_in_relation() {
        let mut source = source_with_meta_location("ignored/{name}.csv");
        source.__source_attr__.catalog_name = Some("GLUE_SOURCE".to_string());

        let relation = create_relation_from_source(
            AdapterType::Snowflake,
            "main".to_string(),
            "raw".to_string(),
            "orders".to_string(),
            DEFAULT_RESOLVED_QUOTING,
            &source,
        )
        .unwrap();

        assert_eq!(
            relation.render_self_as_str(),
            "\"GLUE_SOURCE\".\"raw\".\"orders\""
        );
    }

    #[test]
    fn source_without_catalog_name_keeps_database() {
        let source = source_with_meta_location("ignored/{name}.csv");

        let relation = create_relation_from_source(
            AdapterType::Snowflake,
            "main".to_string(),
            "raw".to_string(),
            "orders".to_string(),
            DEFAULT_RESOLVED_QUOTING,
            &source,
        )
        .unwrap();

        assert_eq!(relation.render_self_as_str(), "\"main\".\"raw\".\"orders\"");
    }

    #[test]
    fn databricks_relation_exposes_constraints_to_jinja() {
        let relation = Relation::new(
            AdapterType::Databricks,
            "main".to_string(),
            "default".to_string(),
            "child_model".to_string(),
        )
        .with_relation_type(RelationType::Table)
        .with_quoting(DEFAULT_RESOLVED_QUOTING)
        .enrich(&[
            TypedConstraint::PrimaryKey {
                name: Some("pk_id".to_string()),
                columns: vec!["id".to_string()],
                expression: None,
            },
            TypedConstraint::Check {
                name: Some("check_id_positive".to_string()),
                expression: "id > 0".to_string(),
                columns: Some(vec!["id".to_string()]),
            },
        ]);

        jinja_assert(
            RelationObject::new(Arc::new(relation)),
            r#"
            {%- for c in obj.create_constraints %}
            create {{ c.type }} | {{ c.render() }}
            {%- endfor %}
            {%- for c in obj.alter_constraints %}
            alter {{ c.type }} | {{ c.render() }}
            {%- endfor %}
            "#,
            r#"
            create primary_key | CONSTRAINT pk_id PRIMARY KEY (id)
            alter check | CONSTRAINT check_id_positive CHECK (id > 0)
            "#,
        );
    }

    #[test]
    fn databricks_metric_view_relation_is_visible_to_jinja() {
        let relation = Relation::new(
            AdapterType::Databricks,
            "main".to_string(),
            "default".to_string(),
            "order_metrics".to_string(),
        )
        .with_relation_type(RelationType::MetricView)
        .with_quoting(DEFAULT_RESOLVED_QUOTING);

        jinja_assert(
            RelationObject::new(Arc::new(relation)),
            "{{ obj.is_metric_view }} | {{ obj.type }}",
            "True | metric_view",
        );
    }

    #[test]
    fn do_create_relation_clickhouse_normalizes_database_to_empty_string() {
        let relation = do_create_relation(
            AdapterType::ClickHouse,
            "ignored".to_string(),
            "analytics".to_string(),
            Some("events".to_string()),
            Some(RelationType::Table),
            ResolvedQuoting {
                database: true,
                schema: true,
                identifier: true,
            },
        )
        .unwrap();

        assert_eq!(relation.render_self_as_str(), "`analytics`.`events`");
        assert_eq!(relation.database(), Some(""));
    }

    #[test]
    fn interactive_table_exposes_its_own_jinja_flag_and_type_name() {
        let relation = do_create_relation(
            AdapterType::Snowflake,
            "d".to_string(),
            "s".to_string(),
            Some("i".to_string()),
            Some(RelationType::InteractiveTable),
            DEFAULT_RESOLVED_QUOTING,
        )
        .unwrap();
        let relation = Arc::new(RelationObject::new(relation.into()));

        for (key, expected) in [
            ("is_interactive_table", true),
            ("is_dynamic_table", false),
            ("is_table", false),
            ("is_view", false),
        ] {
            assert_eq!(
                relation.get_value(&Value::from(key)),
                Some(Value::from(expected)),
                "{key}"
            );
        }

        assert_eq!(
            relation.get_value(&Value::from("InteractiveTable")),
            Some(Value::from("interactive_table"))
        );
        assert_eq!(
            relation.get_value(&Value::from("type")),
            Some(Value::from("interactive_table"))
        );
    }

    /// Asserts on `ErrorKind`, not the message: both this arm and the catch-all name the called
    /// method in their message text, so a message-only assertion would pass either way.
    #[test]
    fn interactive_table_config_changeset_alias_is_reachable_by_name() {
        let relation = do_create_relation(
            AdapterType::Snowflake,
            "d".to_string(),
            "s".to_string(),
            Some("i".to_string()),
            Some(RelationType::InteractiveTable),
            DEFAULT_RESOLVED_QUOTING,
        )
        .unwrap();
        let relation = Arc::new(RelationObject::new(relation.into()));

        let env = minijinja::Environment::new();
        let state = env.empty_state();
        let err = relation
            .call_method(&state, "interactive_table_config_changeset", &[], &[])
            .unwrap_err();

        assert_eq!(err.kind(), minijinja::ErrorKind::MissingArgument, "{err}");
    }

    #[test]
    fn derivative_appends_suffix_and_overrides_type_for_clickhouse() {
        let relation = do_create_relation(
            AdapterType::ClickHouse,
            "ignored".to_string(),
            "analytics".to_string(),
            Some("events".to_string()),
            Some(RelationType::Table),
            ResolvedQuoting {
                database: true,
                schema: true,
                identifier: true,
            },
        )
        .unwrap();

        // suffix appended to identifier, type overridden, database stays empty
        let mv = relation
            .derivative("_mv", Some(RelationType::MaterializedView), false)
            .unwrap();
        assert_eq!(mv.render_self_as_str(), "`analytics`.`events_mv`");
        assert_eq!(mv.database(), Some(""));
        assert_eq!(mv.relation_type(), Some(RelationType::MaterializedView));

        // interpret_suffix_as_full_identifier replaces the identifier entirely,
        // and omitting relation_type inherits the source relation's type
        let full = relation.derivative("custom_name", None, true).unwrap();
        assert_eq!(full.render_self_as_str(), "`analytics`.`custom_name`");
        assert_eq!(full.relation_type(), Some(RelationType::Table));
    }

    #[test]
    fn derivative_via_call_method_accepts_single_argument() {
        // The ClickHouse snapshot macro calls `target.derivative('__snapshot_upsert')`
        // with a single positional argument (dbt-clickhouse snapshot.sql), so
        // relation_type must be optional at the Jinja boundary and default to the
        // source relation's type.
        let relation = do_create_relation(
            AdapterType::ClickHouse,
            "ignored".to_string(),
            "analytics".to_string(),
            Some("events".to_string()),
            Some(RelationType::Table),
            ResolvedQuoting {
                database: true,
                schema: true,
                identifier: true,
            },
        )
        .unwrap();

        let obj = Arc::new(RelationObject::from(relation));
        let env = minijinja::Environment::new();
        let state = State::new_for_env(&env);

        let derived = obj
            .call_method(
                &state,
                "derivative",
                &[Value::from("__snapshot_upsert")],
                &[],
            )
            .unwrap();
        let derived = derived
            .downcast_object_ref::<RelationObject>()
            .expect("derivative should return a relation")
            .inner();
        assert_eq!(
            derived.render_self_as_str(),
            "`analytics`.`events__snapshot_upsert`"
        );
        assert_eq!(derived.relation_type(), Some(RelationType::Table));

        // Two positional args still override the type
        let derived = obj
            .call_method(
                &state,
                "derivative",
                &[Value::from("_mv"), Value::from("materialized_view")],
                &[],
            )
            .unwrap();
        let derived = derived
            .downcast_object_ref::<RelationObject>()
            .expect("derivative should return a relation")
            .inner();
        assert_eq!(derived.render_self_as_str(), "`analytics`.`events_mv`");
        assert_eq!(
            derived.relation_type(),
            Some(RelationType::MaterializedView)
        );
    }

    #[test]
    fn static_relation_snowflake_get_default_quote_policy_passes_with_get_part() {
        let obj = create_static_relation(
            AdapterType::Snowflake,
            ResolvedQuoting {
                database: true,
                schema: false,
                identifier: true,
            },
        )
        .unwrap();

        let env = minijinja::Environment::new();
        let state = State::new_for_env(&env);

        let result = obj
            .call_method(&state, "get_default_quote_policy", &[], &[])
            .unwrap();

        let database = result
            .call_method(&state, "get_part", &[Value::from("database")], &[])
            .unwrap()
            .is_true();
        let schema = result
            .call_method(&state, "get_part", &[Value::from("schema")], &[])
            .unwrap()
            .is_true();
        let identifier = result
            .call_method(&state, "get_part", &[Value::from("identifier")], &[])
            .unwrap()
            .is_true();

        // snowflake defaults
        assert!(!database);
        assert!(!schema);
        assert!(!identifier);
    }

    #[test]
    fn static_relation_other_get_default_quote_policy_passes_with_get_part() {
        let obj = create_static_relation(
            AdapterType::Postgres,
            ResolvedQuoting {
                database: true,
                schema: false,
                identifier: true,
            },
        )
        .unwrap();

        let env = minijinja::Environment::new();
        let state = State::new_for_env(&env);

        let result = obj
            .call_method(&state, "get_default_quote_policy", &[], &[])
            .unwrap();

        let database = result
            .call_method(&state, "get_part", &[Value::from("database")], &[])
            .unwrap()
            .is_true();
        let schema = result
            .call_method(&state, "get_part", &[Value::from("schema")], &[])
            .unwrap()
            .is_true();
        let identifier = result
            .call_method(&state, "get_part", &[Value::from("identifier")], &[])
            .unwrap()
            .is_true();

        // other defaults
        assert!(database);
        assert!(schema);
        assert!(identifier);
    }

    #[test]
    fn static_relation_get_default_quote_policy_fails_with_one_or_more_arguments() {
        let obj = create_static_relation(
            AdapterType::ClickHouse,
            ResolvedQuoting {
                database: true,
                schema: true,
                identifier: true,
            },
        )
        .unwrap();

        let env = minijinja::Environment::new();
        let state = State::new_for_env(&env);

        let result = obj
            .call_method(
                &state,
                "get_default_quote_policy",
                &[Value::from("bad arg")],
                &[],
            )
            .unwrap_err();

        assert_eq!(
            result.detail().unwrap(),
            "Relation.get_default_quote_policy() takes exactly zero positional arguments (1 given)"
        );
    }

    #[test]
    fn static_relation_get_default_quote_policy_with_get_part_should_fail() {
        let obj = create_static_relation(
            AdapterType::ClickHouse,
            ResolvedQuoting {
                database: true,
                schema: false,
                identifier: true,
            },
        )
        .unwrap();

        let env = minijinja::Environment::new();
        let state = State::new_for_env(&env);

        let result = obj
            .call_method(&state, "get_default_quote_policy", &[], &[])
            .unwrap();

        let result_get_part = result
            .call_method(&state, "get_part", &[], &[])
            .unwrap_err();
        assert_eq!(
            result_get_part.detail().unwrap(),
            "missing keyword argument 'name'"
        );

        let result_get_part = result
            .call_method(
                &state,
                "get_part",
                &[Value::from("schema"), Value::from("schema")],
                &[],
            )
            .unwrap_err();
        assert_eq!(
            result_get_part.detail().unwrap(),
            "QuotePolicy.args() takes from 0 to 1 positional arguments but 2 were given"
        );

        let result_get_part = result
            .call_method(&state, "get_part", &[Value::from("bad")], &[])
            .unwrap_err();
        assert_eq!(
            result_get_part.detail().unwrap(),
            "'bad' is not a valid argument"
        );
    }

    #[test]
    fn clickhouse_relation_exposes_catalog_state_attributes() {
        let relation = Relation::new(
            AdapterType::ClickHouse,
            None,
            Some("analytics".to_string()),
            Some("events".to_string()),
        )
        .with_relation_type(RelationType::Table)
        .with_quoting(DEFAULT_RESOLVED_QUOTING)
        .with_mvs_pointing_to_it(vec![BTreeMap::from([
            ("schema".to_string(), "analytics".to_string()),
            ("name".to_string(), "events_mv".to_string()),
            ("sql".to_string(), "select 1".to_string()),
        ])])
        .with_is_refreshable(true)
        .with_can_exchange(true)
        .validate()
        .unwrap();
        let obj = Arc::new(RelationObject::new(Arc::new(relation)));

        assert!(
            obj.get_value(&Value::from("can_exchange"))
                .unwrap()
                .is_true()
        );
        let mvs = obj.get_value(&Value::from("mvs_pointing_to_it")).unwrap();
        assert_eq!(mvs.len(), Some(1));
        assert_eq!(
            mvs.get_item_by_index(0)
                .unwrap()
                .get_attr("name")
                .unwrap()
                .as_str(),
            Some("events_mv")
        );
        assert!(
            obj.get_value(&Value::from("is_refreshable"))
                .unwrap()
                .is_true()
        );
        assert!(
            !obj.get_value(&Value::from("refreshable_append"))
                .unwrap()
                .is_true()
        );

        let bare = Relation::new(
            AdapterType::ClickHouse,
            None,
            Some("analytics".to_string()),
            Some("plain".to_string()),
        )
        .validate()
        .unwrap();
        let bare = Arc::new(RelationObject::new(Arc::new(bare)));
        assert_eq!(
            bare.get_value(&Value::from("mvs_pointing_to_it"))
                .unwrap()
                .len(),
            Some(0)
        );
        assert!(
            !bare
                .get_value(&Value::from("is_refreshable"))
                .unwrap()
                .is_true()
        );
        assert!(
            !bare
                .get_value(&Value::from("can_exchange"))
                .unwrap()
                .is_true()
        );
    }
}
