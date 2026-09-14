use core::fmt;
use indexmap::IndexMap;
use std::path::Path;
use std::str::FromStr;
use std::{any::Any, collections::BTreeMap, fmt::Display, path::PathBuf, sync::Arc};

use chrono::Utc;
use dbt_adapter_core::{AdapterType, MICROBATCH_SUPPORTED_ADAPTERS};
use dbt_common::constants::{DBT_COMPILED_DIR_NAME, DBT_RUN_DIR_NAME};
use dbt_common::io_args::{ComputeArg, StaticAnalysisKind, StaticAnalysisOffReason};
use dbt_common::path::{DbtPath, get_snapshot_write_path, get_target_write_path};
use dbt_common::tracing::dbt_emit::{emit_error_log_message, emit_warn_log_message};
use dbt_common::{ErrorCode, FsResult, Span, err};
use dbt_telemetry::{ExecutionPhase, NodeEvaluated, NodeProcessed, NodeType};
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;
type YmlValue = dbt_yaml::Value;
use crate::schemas::common::{
    ExternalTable, PersistDocsConfig, RowFilterConfig, hooks_equal, normalize_sql,
};
use crate::schemas::dbt_column::{DbtColumnRef, deserialize_dbt_columns, serialize_dbt_columns};
use crate::schemas::manifest::GrantAccessToTarget;
use crate::schemas::project::configs::common::log_state_mod_diff;
use crate::schemas::project::configs::common::{grants_equal, tags_eq_vec};
use crate::schemas::project::{TblProperties, WarehouseSpecificNodeConfig, same_warehouse_config};
use crate::schemas::relations::default_dbt_quoting_for;
use crate::schemas::serde::{PartitionsConfig, QueryTag, StringOrArrayOfStrings};
use crate::schemas::{
    common::{
        Access, ClusterConfig, DbtChecksum, DbtContract, DbtIncrementalStrategy,
        DbtMaterialization, Expect, FreshnessDefinition, Given, IncludeExclude, NodeDependsOn,
        PartitionConfig, ResolvedQuoting, ScheduleConfig, SchemaOrigin, SchemaRefreshInterval,
        SyncConfig,
    },
    macros::DbtMacro,
    manifest::common::DbtOwner,
    manifest::semantic_model::NodeRelation,
    manifest::{DbtMetric, DbtSavedQuery, DbtSemanticModel},
    project::{
        CheckConfig, DataTestConfig, ExposureConfig, FunctionConfig, ModelConfig, SeedConfig,
        SnapshotConfig, SnapshotMetaColumnNames, SourceConfig, UnitTestConfig,
    },
    properties::{
        FunctionArgument, FunctionReturnType, ModelConstraint, ModelFreshness, UnitTestOverrides,
    },
    ref_and_source::{
        DbtRef, DbtSourceWrapper, deserialize_dbt_function_refs, serialize_dbt_function_refs,
    },
    serde::StringOrInteger,
};
use dbt_yaml::{DbtSchema, Spanned, UntaggedEnumDeserialize};

#[derive(
    Default, Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum IntrospectionKind {
    #[default]
    None,
    Execute,
    This,
    UpstreamSchema,
    InternalSchema,
    ExternalSchema,
    Unknown,
}

impl IntrospectionKind {
    pub fn is_unsafe(&self) -> bool {
        matches!(
            self,
            IntrospectionKind::Execute
                | IntrospectionKind::InternalSchema
                | IntrospectionKind::ExternalSchema
                | IntrospectionKind::This
                | IntrospectionKind::Unknown
        )
    }

    pub fn is_safe(&self) -> bool {
        matches!(
            self,
            IntrospectionKind::None | IntrospectionKind::UpstreamSchema
        )
    }

    pub fn is_none(&self) -> bool {
        matches!(self, IntrospectionKind::None)
    }
}

impl Display for IntrospectionKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IntrospectionKind::None => write!(f, "none"),
            IntrospectionKind::Execute => write!(f, "execute"),
            IntrospectionKind::UpstreamSchema => write!(f, "upstream_schema"),
            IntrospectionKind::InternalSchema => write!(f, "internal_schema"),
            IntrospectionKind::ExternalSchema => write!(f, "external_schema"),
            IntrospectionKind::This => write!(f, "this"),
            IntrospectionKind::Unknown => write!(f, "unknown"),
        }
    }
}

impl FromStr for IntrospectionKind {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "none" => Ok(IntrospectionKind::None),
            "execute" => Ok(IntrospectionKind::Execute),
            "upstream_schema" => Ok(IntrospectionKind::UpstreamSchema),
            "internal_schema" => Ok(IntrospectionKind::InternalSchema),
            "external_schema" => Ok(IntrospectionKind::ExternalSchema),
            "this" => Ok(IntrospectionKind::This),
            _ => Err(()),
        }
    }
}

/// A wrapper enum that represents different types of dbt nodes.
///
/// This enum uses serde's tag-based deserialization to automatically determine
/// the correct variant based on the "resource_type" field in the JSON.
/// The resource_type values are converted to snake_case for matching.
///
/// # Example
///
/// ```rust
///
/// // Deserialize a node from Jinja
/// let node = minijinja_value_to_typed_struct::<InternalDbtNodeWrapper>(value).unwrap();
///
/// // Access the underlying node attributes
/// let attributes = node.as_internal_node();
/// ```
#[derive(Debug, Clone, Serialize, UntaggedEnumDeserialize)]
#[serde(tag = "resource_type")]
#[serde(rename_all = "snake_case")]
pub enum InternalDbtNodeWrapper {
    Model(Box<DbtModel>),
    Seed(Box<DbtSeed>),
    Test(Box<DbtTest>),
    UnitTest(Box<DbtUnitTest>),
    Source(Box<DbtSource>),
    Snapshot(Box<DbtSnapshot>),
    Exposure(Box<DbtExposure>),
    Function(Box<DbtFunction>),
}

impl InternalDbtNodeWrapper {
    /// Returns a reference to the underlying node as a trait object.
    ///
    /// This method allows accessing common functionality across all node types
    /// through the `InternalDbtNodeAttributes` trait.
    ///
    /// # Returns
    ///
    /// A reference to the node implementing `InternalDbtNodeAttributes`
    ///
    /// # Examples
    ///
    /// ```rust
    /// let node = InternalDbtNodeWrapper::Model(some_model);
    /// let attributes = node.as_internal_node();
    /// println!("Node name: {}", attributes.name());
    /// ```
    pub fn as_internal_node(&self) -> &dyn InternalDbtNodeAttributes {
        match self {
            InternalDbtNodeWrapper::Model(model) => model.as_ref(),
            InternalDbtNodeWrapper::Seed(seed) => seed.as_ref(),
            InternalDbtNodeWrapper::Test(test) => test.as_ref(),
            InternalDbtNodeWrapper::UnitTest(unit_test) => unit_test.as_ref(),
            InternalDbtNodeWrapper::Source(source) => source.as_ref(),
            InternalDbtNodeWrapper::Snapshot(snapshot) => snapshot.as_ref(),
            InternalDbtNodeWrapper::Exposure(exposure) => exposure.as_ref(),
            InternalDbtNodeWrapper::Function(function) => function.as_ref(),
        }
    }
}

/// The kind of path to resolve for a node.
///
/// Each variant corresponds to a well-known location a node can have on disk:
///
/// - [`Definition`](Self::Definition) — the original source file.
/// - [`Compiled`](Self::Compiled) — the compiled SQL under `target/compiled/`.
/// - [`Executable`](Self::Executable) — the run-ready SQL under `target/run/`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodePathKind {
    /// The original source file where the node is defined.
    Definition,
    /// The compiled SQL path (`target/compiled/…`).
    Compiled,
    /// The executable / run SQL path (`target/run/…`).
    Executable,
}

/// Maps an [`ExecutionPhase`] to the corresponding node path kind for phase-aware
/// reporting. The node path API still decides what each kind means for the
/// specific node type.
impl From<ExecutionPhase> for NodePathKind {
    fn from(phase: ExecutionPhase) -> Self {
        match phase {
            ExecutionPhase::Analyze => Self::Compiled,
            ExecutionPhase::Run => Self::Executable,
            _ => Self::Definition,
        }
    }
}

pub trait InternalDbtNode: Any + Send + Sync + fmt::Debug {
    fn common(&self) -> &CommonAttributes;
    fn base(&self) -> &NodeBaseAttributes;
    fn base_mut(&mut self) -> &mut NodeBaseAttributes;
    fn common_mut(&mut self) -> &mut CommonAttributes;
    fn version(&self) -> Option<StringOrInteger> {
        None
    }
    fn latest_version(&self) -> Option<StringOrInteger> {
        None
    }
    fn event_time(&self) -> Option<String> {
        None
    }
    fn get_group(&self) -> Option<String> {
        None
    }
    fn is_extended_model(&self) -> bool {
        false
    }
    fn has_freshness(&self) -> bool {
        false
    }
    /// The SLA this node is measured against. Empty for node types that carry none.
    fn freshness_criteria(&self) -> FreshnessDefinition {
        FreshnessDefinition::default()
    }
    fn is_versioned(&self) -> bool {
        false
    }
    fn defined_at(&self) -> Option<&dbt_common::CodeLocationWithFile> {
        None
    }
    fn resource_type(&self) -> NodeType;
    fn as_any(&self) -> &dyn Any;
    fn serialize(&self) -> YmlValue {
        self.serialize_with_mode(crate::schemas::serialization_utils::SerializationMode::OmitNone)
    }
    fn serialize_keep_none(&self) -> YmlValue {
        self.serialize_with_mode(crate::schemas::serialization_utils::SerializationMode::KeepNone)
    }
    fn serialize_with_mode(
        &self,
        mode: crate::schemas::serialization_utils::SerializationMode,
    ) -> YmlValue {
        let mut ret = self.serialize_inner(mode);
        if let YmlValue::Mapping(ref mut map, _) = ret {
            map.insert(
                YmlValue::string("resource_type".to_string()),
                YmlValue::string(self.resource_type().as_static_ref().to_string()),
            );
        }
        ret
    }
    fn serialize_inner(
        &self,
        mode: crate::schemas::serialization_utils::SerializationMode,
    ) -> YmlValue;

    // Selector functions
    fn has_same_config(&self, other: &dyn InternalDbtNode, adapter_type: AdapterType) -> bool;
    fn has_same_content(&self, other: &dyn InternalDbtNode, adapter_type: AdapterType) -> bool;
    fn has_same_body(&self, other: &dyn InternalDbtNode) -> bool {
        self.common().checksum == other.common().checksum
    }

    fn set_detected_introspection(&mut self, introspection: IntrospectionKind);
    fn introspection(&self) -> IntrospectionKind {
        IntrospectionKind::None
    }

    fn is_test(&self) -> bool {
        self.resource_type() == NodeType::Test
    }

    /// Returns the original (untruncated) test name if truncation occurred.
    /// For non-test nodes or tests without truncation, returns None.
    fn original_name(&self) -> Option<&str> {
        None
    }

    // Some node types are never considered new even if the previous state is missing. See
    // the same_contents method for Exposure, SavedQuery, SemanticModel and Metrics in
    // mantle: https://github.com/dbt-labs/dbt-mantle/blob/1a251ee081adf4c7af2ba38e7797b04e69d2a15f/core/dbt/contracts/graph/nodes.py
    fn is_never_new_if_previous_missing(&self) -> bool {
        self.resource_type() == NodeType::Exposure
            || self.resource_type() == NodeType::SavedQuery
            || self.resource_type() == NodeType::SemanticModel
            || self.resource_type() == NodeType::Metric
    }

    // Incremental strategy validation
    fn warn_on_microbatch(&self, _adapter_type: AdapterType) -> usize {
        0
    }

    /// Returns the path for the current node corresponding to `path_kind`, relative to `in_dir`.
    ///
    /// Callers that have an [`ExecutionPhase`] can convert it via `.into()`.
    ///
    /// For models, generic tests, unit tests, snapshots, analyses, functions, and seeds the path
    /// varies by kind (analyses only have a `Compiled` path — they are never materialized/run;
    /// functions have both `Compiled` and `Executable` paths; seeds only have an `Executable` run
    /// path beyond the definition path — they are never compiled):
    ///
    ///   - `Compiled`   - `target/compiled/{package}/{path_segment}` (snapshots use
    ///     their nested compiled artifact path)
    ///   - `Executable` - `target/run/{package}/{path_segment}/{alias}.sql`
    ///     (mirrors `write_file()` in `run_node_context.rs`; uses `alias` to
    ///     avoid ENAMETOOLONG on Linux for long generic test names)
    ///   - `Definition` - see `get_node_definition_path`
    ///
    /// For all other node types the definition path is returned.
    fn get_node_path(
        &self,
        path_kind: NodePathKind,
        in_dir: &Path,
        out_dir: &Path,
    ) -> std::borrow::Cow<'_, Path> {
        match (path_kind, self.resource_type()) {
            (
                NodePathKind::Compiled,
                NodeType::Model
                | NodeType::Test
                | NodeType::UnitTest
                | NodeType::Snapshot
                | NodeType::Analysis
                | NodeType::Function,
            )
            | (
                NodePathKind::Executable,
                NodeType::Model
                | NodeType::Test
                | NodeType::UnitTest
                | NodeType::Snapshot
                | NodeType::Function
                | NodeType::Seed,
            ) => {
                let abs = self.get_node_path_abs(path_kind, in_dir, out_dir);
                pathdiff::diff_paths(&abs, in_dir).unwrap_or(abs).into()
            }
            _ => self.get_node_definition_path(in_dir, out_dir),
        }
    }

    /// The kind of path to cite when this node errors during `phase`.
    ///
    /// Defaults to the definition path for every phase. Node types whose reporting is
    /// phase-accurate override this and opt in per phase.
    fn error_path_kind(&self, _phase: ExecutionPhase) -> NodePathKind {
        NodePathKind::Definition
    }

    /// Constructs the absolute path for the requested kind.
    ///
    /// `get_node_path` applies node-kind fallback for display/reporting paths. This lower-level
    /// helper is for callers that need a concrete artifact or definition path for filesystem use.
    fn get_node_path_abs(&self, path_kind: NodePathKind, in_dir: &Path, out_dir: &Path) -> PathBuf {
        let common = self.common();
        let executable_filename = || {
            // Generic test paths are synthesized and bounded before relation aliases are
            // rendered. Keep the bounded path filename separate from the relation alias.
            if self.resource_type() == NodeType::Test && self.defined_at().is_some() {
                if let Some(file_stem) = common.path.file_stem() {
                    return file_stem.to_string_lossy().into_owned();
                }
            }

            // Mirror write_file's filename rule for other node types: alias if non-empty,
            // else name.
            let alias = &self.base().alias;
            if alias.is_empty() {
                common.name.clone()
            } else {
                alias.clone()
            }
        };
        match path_kind {
            NodePathKind::Compiled => {
                if self.resource_type() == NodeType::Snapshot {
                    // Snapshots always use the many-to-one nested path — see get_snapshot_write_path.
                    return get_snapshot_write_path(
                        &out_dir.join(DBT_COMPILED_DIR_NAME),
                        &common.package_name,
                        &common.original_file_path,
                        &common.name,
                    );
                }
                let mut abs = get_target_write_path(
                    in_dir,
                    &out_dir.join(DBT_COMPILED_DIR_NAME),
                    &common.package_name,
                    &common.path,
                    &common.original_file_path,
                );
                if self.resource_type() == NodeType::UnitTest {
                    // Unit test SQL is synthesized at <alias>.sql, not at original_file_path (the YAML).
                    abs = abs.with_file_name(format!("{}.sql", executable_filename()));
                }
                abs
            }
            NodePathKind::Executable => {
                if self.resource_type() == NodeType::Snapshot {
                    // Same nested layout as the compiled path — see get_snapshot_write_path.
                    // Keeping target/run in step with target/compiled is what dbt-core does, and
                    // it is what lets Fusion reuse a target/ directory left by dbt-core v1
                    // instead of failing with EISDIR (dbt-core#15692).
                    return get_snapshot_write_path(
                        &out_dir.join(DBT_RUN_DIR_NAME),
                        &common.package_name,
                        &common.original_file_path,
                        &common.name,
                    );
                }
                get_target_write_path(
                    in_dir,
                    &out_dir.join(DBT_RUN_DIR_NAME),
                    &common.package_name,
                    &common.path,
                    &common.original_file_path,
                )
                .with_file_name(format!("{}.sql", executable_filename()))
            }
            NodePathKind::Definition => self.get_node_definition_path(in_dir, out_dir).into_owned(),
        }
    }

    /// Returns the definition path for the current node, relative to `in_dir`.
    ///
    /// This is where the node is defined in the project — independent of execution phase.
    ///
    /// - Snapshots: the raw YAML/SQL file where the snapshot is defined.
    /// - Generic tests: the YAML file where the test is declared (via `defined_at.file`).
    ///   `original_file_path` is a misnomer for tests — it points at the generated SQL —
    ///   so we prefer `defined_at` when available and fall back to it only if missing.
    /// - Models / unit tests / others: `original_file_path` (already correct).
    fn get_node_definition_path(
        &self,
        in_dir: &Path,
        _out_dir: &Path,
    ) -> std::borrow::Cow<'_, Path> {
        if self.resource_type() == NodeType::Test
            && let Some(defined_at) = self.defined_at()
        {
            // defined_at.file is the YAML file where the test is declared. It's stored
            // relative to project root in the YAML span; if it ever comes through absolute
            // (e.g. a different parser path), strip in_dir so callers get a consistent
            // relative form.
            let file = defined_at.file.as_path();
            let relative = pathdiff::diff_paths(file, in_dir).unwrap_or_else(|| file.to_owned());
            return relative.into();
        }
        self.common().original_file_path.as_path().into()
    }

    fn get_node_evaluated_event(
        &self,
        phase: ExecutionPhase,
        in_dir: &Path,
        out_dir: &Path,
    ) -> NodeEvaluated {
        let common = self.common();
        let base = self.base();
        let node_type = self.resource_type();

        let (database, schema, identifier) = (
            base.database.clone(),
            base.schema.clone(),
            Some(base.alias.clone()),
        );

        let custom_materialization = if let DbtMaterialization::Unknown(custom) = &base.materialized
        {
            Some(custom.clone())
        } else {
            None
        };

        // Keep model status/log display on the source definition path for readability.
        let path_kind = if node_type == NodeType::Model {
            NodePathKind::Definition
        } else {
            phase.into()
        };
        let relative_path = self
            .get_node_path(path_kind, in_dir, out_dir)
            .display()
            .to_string();
        let (defined_at_line, defined_at_column) = if path_kind == NodePathKind::Definition {
            self.defined_at()
                .map(|defined_at| (Some(defined_at.line), Some(defined_at.col)))
                .unwrap_or((None, None))
        } else {
            (None, None)
        };

        let node_checksum = common.checksum.to_checksum_string();

        // Use original_name for display if available (for truncated test names)
        NodeEvaluated::start(
            common.unique_id.clone(),
            self.original_name().unwrap_or(&common.name).to_string(),
            Some(database),
            Some(schema),
            identifier,
            Some((&base.materialized).into()),
            custom_materialization,
            node_type,
            phase,
            relative_path,
            defined_at_line,
            defined_at_column,
            node_checksum,
        )
    }

    fn get_node_processed_event(
        &self,
        last_phase: Option<ExecutionPhase>,
        in_dir: &Path,
        out_dir: &Path,
        in_selection: bool,
    ) -> NodeProcessed {
        self.get_node_processed_event_base(last_phase, in_dir, out_dir, in_selection)
    }

    fn get_node_processed_event_base(
        &self,
        last_phase: Option<ExecutionPhase>,
        in_dir: &Path,
        out_dir: &Path,
        in_selection: bool,
    ) -> NodeProcessed {
        let common = self.common();
        let base = self.base();
        let node_type = self.resource_type();

        let (database, schema, identifier) = (
            base.database.clone(),
            base.schema.clone(),
            Some(base.alias.clone()),
        );

        let custom_materialization = if let DbtMaterialization::Unknown(custom) = &base.materialized
        {
            Some(custom.clone())
        } else {
            None
        };

        // This is a quirk of historical reporting. For generic tests we report the yml source with line:col location,
        // but for all other node types we follow the logic described in `get_node_definition_path`.
        // TODO: streamline and ensure the path's reported via events align with what LSP is being sent
        let (relative_path, defined_at_line, defined_at_column) = self.defined_at().map_or_else(
            || {
                (
                    self.get_node_path(NodePathKind::Definition, in_dir, out_dir)
                        .display()
                        .to_string(),
                    None,
                    None,
                )
            },
            |defined_at| {
                let relative_path = if defined_at.file.is_absolute() {
                    defined_at
                        .file
                        .strip_prefix(in_dir)
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|_| defined_at.file.display().to_string())
                } else {
                    defined_at.file.display().to_string()
                };

                (relative_path, Some(defined_at.line), Some(defined_at.col))
            },
        );

        let node_checksum = common.checksum.to_checksum_string();

        NodeProcessed::start(
            common.unique_id.clone(),
            self.original_name().unwrap_or(&common.name).to_string(),
            Some(database),
            Some(schema),
            identifier,
            Some((&base.materialized).into()),
            custom_materialization,
            node_type,
            last_phase,
            relative_path,
            defined_at_line,
            defined_at_column,
            node_checksum,
            in_selection,
            self.get_group(),
        )
    }
}

pub trait InternalDbtNodeAttributes: InternalDbtNode {
    /// The adapter this node runs on, fully resolved at parse. Reads
    /// [`NodeBaseAttributes::adapter`], so every node type answers without the
    /// caller reaching into its own attr struct or supplying a fallback.
    fn node_adapter(&self) -> AdapterType {
        self.base().adapter
    }

    /// The compute platforms this node is published to after it materializes.
    /// Reads [`NodeBaseAttributes::propagate`], so every node type answers
    /// without the caller reaching into its own attr struct. Empty for a node
    /// that made no `+propagate` selection.
    fn node_propagate(&self) -> &[AdapterType] {
        &self.base().propagate
    }

    // Required Fields
    fn skip_generate_database_name_macro(&self) -> bool {
        false
    }

    fn database(&self) -> String {
        self.base().database.clone()
    }

    fn skip_generate_schema_name_macro(&self) -> bool {
        false
    }

    fn schema(&self) -> String {
        self.base().schema.clone()
    }

    fn unique_id(&self) -> String {
        self.common().unique_id.clone()
    }

    fn name(&self) -> String {
        self.common().name.clone()
    }

    fn name_span(&self) -> Span {
        self.common().name_span.clone()
    }

    fn description(&self) -> Option<String> {
        self.common().description.clone()
    }

    fn columns(&self) -> Vec<DbtColumnRef> {
        self.base().columns.clone()
    }

    /// The node's compiled SQL, when the caller has populated it.
    ///
    /// Not part of the serialized node: at run scope the Jinja `model` object loads compiled SQL
    /// lazily from disk, so it only reaches a typed node when something copies it across.
    fn compiled_code(&self) -> Option<&str> {
        None
    }

    fn alias(&self) -> String {
        self.base().alias.clone()
    }

    fn path(&self) -> PathBuf {
        self.common().path.to_path_buf()
    }

    fn original_file_path(&self) -> PathBuf {
        self.common().original_file_path.to_path_buf()
    }

    fn package_name(&self) -> String {
        self.common().package_name.clone()
    }

    fn materialized(&self) -> DbtMaterialization {
        self.base().materialized.clone()
    }

    fn quoting(&self) -> ResolvedQuoting {
        self.base().quoting
    }

    fn tags(&self) -> Vec<String> {
        self.common().tags.clone()
    }

    fn meta(&self) -> IndexMap<String, YmlValue> {
        self.common().meta.clone()
    }

    fn compute(&self) -> Option<ComputeArg> {
        self.base().compute
    }

    fn static_analysis(&self) -> Spanned<StaticAnalysisKind> {
        self.base().static_analysis.clone()
    }

    fn static_analysis_baseline(&self) -> Spanned<bool> {
        self.static_analysis().map(|static_analysis| {
            dbt_common::static_analysis::is_baseline_static_analysis(static_analysis)
        })
    }

    fn static_analysis_enabled(&self) -> Spanned<bool> {
        self.static_analysis().map(|static_analysis| {
            dbt_common::static_analysis::is_static_analysis_enabled(static_analysis)
        })
    }

    fn static_analysis_off_reason(&self) -> Option<StaticAnalysisOffReason> {
        self.base().static_analysis_off_reason
    }

    /// Returns the schema origin for this node.
    ///
    /// For sources with `schema_origin: local`, the schema is derived from
    /// YAML column definitions rather than fetched from the warehouse.
    /// Default is `Remote` for all node types except sources that explicitly
    /// configure `schema_origin: local`.
    fn schema_origin(&self) -> SchemaOrigin {
        SchemaOrigin::default()
    }

    /// Returns the schema refresh interval for this node.
    ///
    /// For sources with `sync.schema_refresh_interval` configured, returns
    /// the per-source TTL. Otherwise returns `None` to use the global default.
    fn schema_refresh_interval(&self) -> Option<SchemaRefreshInterval> {
        None
    }

    // Setters

    fn set_quoting(&mut self, quoting: ResolvedQuoting) {
        self.base_mut().quoting = quoting;
    }

    fn set_static_analysis(&mut self, static_analysis: Spanned<StaticAnalysisKind>) {
        self.base_mut().static_analysis = static_analysis;
    }

    fn set_static_analysis_off_reason(
        &mut self,
        static_analysis_off_reason: Option<StaticAnalysisOffReason>,
    ) {
        self.base_mut().static_analysis_off_reason = static_analysis_off_reason;
    }

    // Optional Fields
    fn get_access(&self) -> Option<Access> {
        None
    }

    /// Returns the search name for this node, following Python dbt patterns:
    /// - Models: name (or name.v{version} if versioned)
    /// - Sources: source_name.name
    /// - Others: name
    fn search_name(&self) -> String;

    /// Returns the selector string for this node, following Python dbt patterns:
    /// - Models/Seeds/Tests/Snapshots: use fqn joined with "."
    /// - Sources: "source:pkg.source_name.table_name"
    /// - Unit tests: "unit_test:pkg.versioned_name"
    fn selector_string(&self) -> String;

    /// Returns the file path for this node
    fn file_path(&self) -> String {
        self.common()
            .original_file_path
            .to_string_lossy()
            .to_string()
    }

    // TO BE DEPRECATED
    fn serialized_config(&self) -> YmlValue;
}

// Shared helper functions for has_same_content implementation across node types
// These match the dbt-core same_contents method logic for ParsedNode

fn same_body(self_common: &CommonAttributes, other_common: &CommonAttributes) -> bool {
    self_common.checksum == other_common.checksum
}

// Compare absorbed overloads for state:modified. Intentionally excludes
// `compiled_body` — that is a rendering artifact (the Jinja-rendered SQL, which
// can differ across runs even when the overload source is unchanged), so
// comparing it would flag `state:modified` spuriously. The overload's source
// (`raw_body`) and signature (`arguments`/`returns`) are what actually define it.
fn overloads_have_same_content(a: &[AbsorbedOverload], b: &[AbsorbedOverload]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b.iter()).all(|(x, y)| {
            x.defined_in == y.defined_in
                && x.arguments == y.arguments
                && x.returns == y.returns
                && x.description == y.description
                && x.raw_body == y.raw_body
        })
}

// Helper function to normalize descriptions: treat None and Some("") as equal
// and strip all whitespace for non-empty descriptions
fn canonicalize_typographic_quotes(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\u{201C}' | '\u{201D}' => '"',
            '\u{2018}' | '\u{2019}' => '\'',
            other => other,
        })
        .collect()
}

pub(crate) fn normalize_description(desc: &Option<String>) -> Option<String> {
    desc.as_deref().filter(|s| !s.is_empty()).map(|s| {
        canonicalize_typographic_quotes(s)
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect()
    })
}

pub(crate) fn same_persisted_description(
    self_common: &CommonAttributes,
    self_base: &NodeBaseAttributes,
    other_common: &CommonAttributes,
    other_base: &NodeBaseAttributes,
) -> bool {
    // Mirror dbt-core's `ParsedNode.same_persisted_description`: this check does NOT compare the
    // `persist_docs` config itself. As dbt-core notes, "the check on configs will handle the case
    // where we have different persist settings, so we only have to care about the cases where they
    // are the same." The `persist_docs` config is owned by the config check
    // (`check_configs_modified`), which compares `unrendered_config` for models/sources and so
    // treats environment-aware `persist_docs` as authoring-intent — matching dbt-core's
    // `same_config`. Comparing the rendered `persist_docs` here as well both double-counted that
    // config and reintroduced the environment-aware false positive from dbt-core#15286. Here we
    // only compare the persisted descriptions, gated on whether the *current* node renders
    // persistence on (dbt-core reads `self.config.persist_docs`).

    // Extract the persist settings for use below
    let self_persist_relation = self_base
        .persist_docs
        .as_ref()
        .and_then(|pd| pd.relation)
        .unwrap_or(false);

    let self_persist_columns = self_base
        .persist_docs
        .as_ref()
        .and_then(|pd| pd.columns)
        .unwrap_or(false);

    // If relation docs are persisted, compare descriptions
    if self_persist_relation
        && normalize_description(&self_common.description)
            != normalize_description(&other_common.description)
    {
        log_state_mod_diff(
            &self_common.unique_id,
            "persisted_description - relation and column descriptions",
            [("description", false, None)],
        );
        return false;
    }

    // If column docs are persisted, compare column descriptions
    if self_persist_columns {
        let self_column_descriptions: Vec<_> = self_base
            .columns
            .iter()
            .map(|column| {
                column
                    .description
                    .as_deref()
                    .map(canonicalize_typographic_quotes)
            })
            .collect();

        let other_column_descriptions: Vec<_> = other_base
            .columns
            .iter()
            .map(|column| {
                column
                    .description
                    .as_deref()
                    .map(canonicalize_typographic_quotes)
            })
            .collect();

        if !optional_string_vecs_equal(&self_column_descriptions, &other_column_descriptions) {
            log_state_mod_diff(
                &self_common.unique_id,
                "persisted_description - column descriptions",
                [(
                    "description",
                    false,
                    Some((
                        format!("{:?}", &self_column_descriptions),
                        format!("{:?}", &other_column_descriptions),
                    )),
                )],
            );
            return false;
        }
    }

    true
}

/// Compare two vectors of Option<String> treating semantic equivalents of "empty" as equal.
/// - `None` and `Some("")` are considered equal
/// - `Some("")` and `Some("   ")` (whitespace-only) are considered equal
/// - An empty vec `[]` is considered equal to a vec of all-empty strings `[Some(""), ...]`
/// - Vectors with the same content but different order are considered equal (order-independent)
/// - Vectors with duplicates are equal if the unique content matches (handles duplicate empty strings)
/// - Vectors with different non-empty content are considered different
fn optional_string_vecs_equal(a: &[Option<String>], b: &[Option<String>]) -> bool {
    /// Helper to check if an Option<String> is semantically empty
    fn is_empty(opt: &Option<String>) -> bool {
        match opt {
            None => true,
            Some(s) => s.trim().is_empty(),
        }
    }

    // Check if all elements in a slice are semantically empty
    fn all_empty(slice: &[Option<String>]) -> bool {
        slice.iter().all(is_empty)
    }

    /// Normalize an Option<String> to a canonical form for comparison:
    /// - Empty values (None, Some(""), whitespace-only) become None
    /// - Non-empty values are normalized using normalize_sql
    fn normalize_item(opt: &Option<String>) -> Option<String> {
        match opt {
            None => None,
            Some(s) if s.trim().is_empty() => None,
            Some(s) => Some(normalize_sql(s)),
        }
    }

    // Special case: if both are effectively "all empty", treat them as equal
    // This handles the case where one manifest has `[]` and the other has `[Some(""), Some(""), ...]`
    // Both represent "no meaningful column descriptions"
    if all_empty(a) && all_empty(b) {
        return true;
    }

    // Create normalized copies of both vectors
    let a_normalized: Vec<Option<String>> = a.iter().map(normalize_item).collect();
    let b_normalized: Vec<Option<String>> = b.iter().map(normalize_item).collect();

    // Determine which is shorter and which is longer
    let (shorter, longer) = if a_normalized.len() <= b_normalized.len() {
        (&a_normalized, &b_normalized)
    } else {
        (&b_normalized, &a_normalized)
    };

    // Create a set from the shorter vector for membership checking
    // This handles duplicates: if the longer vector has duplicate empty strings,
    // they will all be found in the set (as None after normalization)
    let shorter_set: std::collections::HashSet<Option<String>> = shorter.iter().cloned().collect();

    // Check that every element in the longer vector is present in the shorter set
    // If the longer vector has an element not in the set, the vectors are different
    let mut matched_count = 0;
    for item in longer.iter() {
        if !shorter_set.contains(item) {
            if item.is_none() || item.as_ref().unwrap().is_empty() {
                continue;
            }
            return false;
        }
        matched_count += 1;
    }
    if matched_count < shorter.len() {
        return false;
    }

    true
}

fn same_fqn(self_common: &CommonAttributes, other_common: &CommonAttributes) -> bool {
    let self_fqn = &self_common.fqn;
    let other_fqn = &other_common.fqn;

    // If they're exactly equal, return true
    if self_fqn == other_fqn {
        return true;
    }

    // Determine which is shorter and which is longer
    let (shorter, longer) = if self_fqn.len() <= other_fqn.len() {
        (self_fqn, other_fqn)
    } else {
        (other_fqn, self_fqn)
    };

    // Check if the shorter FQN matches a prefix of the longer FQN
    if longer.starts_with(shorter) {
        return true;
    }

    false
}

/// Compare the table names (last part after splitting by '.') from two relation_name Option values
pub fn same_relation_name(self_relation: &Option<String>, other_relation: &Option<String>) -> bool {
    match (self_relation, other_relation) {
        (None, None) => true,
        (None, Some(other)) => other.split('.').next_back() == Some(""),
        (Some(self_rel), None) => self_rel.split('.').next_back() == Some(""),
        (Some(self_rel), Some(other)) => {
            self_rel.split('.').next_back() == other.split('.').next_back()
        }
    }
}

// Helper function to compare PersistDocsConfig values
fn persist_docs_configs_equal(
    a: &Option<PersistDocsConfig>,
    b: &Option<PersistDocsConfig>,
) -> bool {
    // Helper to check if a PersistDocsConfig has all fields as None
    let is_empty_persist_docs =
        |pd: &PersistDocsConfig| -> bool { pd.columns.is_none() && pd.relation.is_none() };

    // First handle the special case where None equals Some with all None fields
    match (a, b) {
        (None, None) => return true,
        (None, Some(b_config)) if is_empty_persist_docs(b_config) => return true,
        (Some(a_config), None) if is_empty_persist_docs(a_config) => return true,
        _ => {}
    }

    // Extract values for comparison
    let a_persist_relation = a.as_ref().and_then(|pd| pd.relation).unwrap_or(false);
    let b_persist_relation = b.as_ref().and_then(|pd| pd.relation).unwrap_or(false);

    let a_persist_columns = a.as_ref().and_then(|pd| pd.columns).unwrap_or(false);
    let b_persist_columns = b.as_ref().and_then(|pd| pd.columns).unwrap_or(false);

    let relation_eq = a_persist_relation == b_persist_relation;
    let columns_eq = a_persist_columns == b_persist_columns;

    let result = relation_eq && columns_eq;

    if !result {
        log_state_mod_diff(
            "unique_id in next seed/snapshot log",
            "persist_docs_config",
            [
                (
                    "relation",
                    relation_eq,
                    Some((
                        format!("{:?}", &a_persist_relation),
                        format!("{:?}", &b_persist_relation),
                    )),
                ),
                (
                    "columns",
                    columns_eq,
                    Some((
                        format!("{:?}", &a_persist_columns),
                        format!("{:?}", &b_persist_columns),
                    )),
                ),
            ],
        );
    }

    result
}

fn same_database_representation_model(
    self_base: &NodeBaseAttributes,
    other_base: &NodeBaseAttributes,
    self_config: &ModelConfig,
    other_config: &ModelConfig,
) -> bool {
    // dbt-core compares the unrendered config representation (database/schema/alias)
    // to avoid treating target-derived rendering differences as modifications.
    //
    // If unrendered config is not available (older Fusion nodes/artifacts), fall back to the
    // existing behavior for compatibility.
    let self_uc = &self_base.unrendered_config;
    let other_uc = &other_base.unrendered_config;

    // Use dbt-core/Mantle semantics whenever possible: compare database/schema/alias using
    // `unrendered_config`, treating missing keys as `None`.
    fn get<'a>(m: &'a BTreeMap<String, YmlValue>, k: &str) -> Option<&'a str> {
        m.get(k).and_then(|v| v.as_str())
    }
    let db_eq = get(self_uc, "database") == get(other_uc, "database");
    let schema_eq = get(self_uc, "schema") == get(other_uc, "schema");
    let alias_eq = get(self_uc, "alias") == get(other_uc, "alias");
    let uc_eq = db_eq && schema_eq && alias_eq;

    if uc_eq {
        return true;
    }

    // Fallback: Compare rendered database/schema/alias from the deprecated_config (Fusion legacy behavior).
    self_config.same_database_representation(other_config)
}

/// `latest_version` equality, tolerant of the two ways one version can be spelled.
///
/// `StringOrInteger` derives `PartialEq` *structurally*, so `String("1") != Integer(1)`. Real
/// dbt-core writes `latest_version` as a JSON **number** and Fusion's `From<String>` parses a
/// numeric string into `Integer`, so a current run against a dbt-core manifest happens to match —
/// but a manifest written by an older Fusion, or a hand-authored fixture using `"1"`, would not,
/// and every versioned model in the project would read as modified. Compare the canonical
/// (`Display`) spelling instead, which is also the spelling a pinned `ref()` uses.
fn latest_version_eq(a: &Option<StringOrInteger>, b: &Option<StringOrInteger>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.to_string() == b.to_string(),
        _ => false,
    }
}

/// `deprecation_date` equality, compared as a **date** rather than as a string.
///
/// dbt-core compares `datetime` objects, so date semantics — not spelling — is the faithful
/// transcription here. The two sides can legitimately disagree on spelling in two ways:
///
/// - Fusion normalizes an authored date to RFC 3339 on the parse side
///   (`normalize_deprecation_date`), so an authored `2099-01-01` faces a manifest's
///   `2099-01-01T00:00:00+00:00`.
/// - The normalizations do not agree on the *offset* of a naive authored date: Fusion reads it as
///   UTC, while dbt-core's `normalize_date` re-interprets it in the **system** time zone
///   (dbt-mantle `core/dbt/contracts/graph/unparsed.py:1012-1024`). A Mantle-produced previous
///   manifest can therefore carry `2020-01-01T00:00:00-05:00` where this run parses
///   `2020-01-01T00:00:00+00:00` from the same authored `2020-01-01`.
///
/// So two parseable values are equal when they name the same instant *or* the same wall-clock
/// time. String equality is the fast path, and the fallback when either side fails to parse.
fn deprecation_date_eq(a: &Option<String>, b: &Option<String>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            a == b
                || match (
                    crate::schemas::common::parse_deprecation_date(a),
                    crate::schemas::common::parse_deprecation_date(b),
                ) {
                    (Some(a), Some(b)) => a == b || a.naive_local() == b.naive_local(),
                    _ => false,
                }
        }
        _ => false,
    }
}

fn same_database_representation_seed(self_config: &SeedConfig, other_config: &SeedConfig) -> bool {
    // Compare database, schema, alias from the deprecated_config
    /*self_config.database == other_config.database
    && self_config.schema == other_config.schema*/
    let alias_eq = self_config.alias == other_config.alias;

    if !alias_eq {
        log_state_mod_diff(
            "unique_id in next seed log",
            "seed_database_representation",
            [(
                "alias",
                alias_eq,
                Some((
                    format!("{:?}", &self_config.alias),
                    format!("{:?}", &other_config.alias),
                )),
            )],
        );
    }

    alias_eq
}

fn same_database_representation_snapshot(
    self_base: &NodeBaseAttributes,
    other_base: &NodeBaseAttributes,
    self_config: &SnapshotConfig,
    other_config: &SnapshotConfig,
) -> bool {
    // dbt-core compares the unrendered config representation (database/schema/alias)
    // to avoid treating target-derived rendering differences as modifications.
    //
    // If unrendered config is not available (older Fusion nodes/artifacts), fall back to the
    // existing behavior for compatibility.
    let self_uc = &self_base.unrendered_config;
    let other_uc = &other_base.unrendered_config;

    // Use dbt-core/Mantle semantics whenever possible: compare database/schema/alias using
    // `unrendered_config`, treating missing keys as `None`.
    fn get<'a>(m: &'a BTreeMap<String, YmlValue>, k: &str) -> Option<&'a str> {
        m.get(k).and_then(|v| v.as_str())
    }
    let db_eq = get(self_uc, "database") == get(other_uc, "database");
    let schema_eq = get(self_uc, "schema") == get(other_uc, "schema");
    let alias_eq = get(self_uc, "alias") == get(other_uc, "alias");
    let uc_eq = db_eq && schema_eq && alias_eq;

    if uc_eq {
        return true;
    }

    // Fallback: Compare rendered database/schema/alias from the deprecated_config (Fusion legacy behavior).
    let alias_eq = self_config.alias == other_config.alias;

    if !alias_eq {
        log_state_mod_diff(
            "unique_id in next snapshot log",
            "snapshot_database_representation",
            [(
                "alias",
                alias_eq,
                Some((
                    format!("{:?}", &self_config.alias),
                    format!("{:?}", &other_config.alias),
                )),
            )],
        );
    }

    alias_eq
}

impl InternalDbtNode for DbtModel {
    fn common(&self) -> &CommonAttributes {
        &self.__common_attr__
    }

    fn base(&self) -> &NodeBaseAttributes {
        &self.__base_attr__
    }

    fn version(&self) -> Option<StringOrInteger> {
        self.__model_attr__.version.clone()
    }

    fn latest_version(&self) -> Option<StringOrInteger> {
        self.__model_attr__.latest_version.clone()
    }

    fn event_time(&self) -> Option<String> {
        self.__model_attr__.event_time.clone()
    }

    fn is_versioned(&self) -> bool {
        self.__model_attr__.version.is_some()
    }

    fn is_extended_model(&self) -> bool {
        self.__base_attr__.extended_model
    }

    fn has_freshness(&self) -> bool {
        self.__model_attr__
            .freshness
            .as_ref()
            .is_some_and(ModelFreshness::has_sla)
    }

    /// Projects `ModelFreshness`'s SLA fields onto `FreshnessDefinition`.
    /// `build_after` is a scheduling rule and is excluded.
    fn freshness_criteria(&self) -> FreshnessDefinition {
        let Some(freshness) = self.__model_attr__.freshness.as_ref() else {
            return FreshnessDefinition::default();
        };
        FreshnessDefinition {
            error_after: freshness.error_after.clone(),
            warn_after: freshness.warn_after.clone(),
            filter: freshness.filter.clone(),
            loaded_at_field: freshness.loaded_at_field.clone(),
            loaded_at_query: freshness.loaded_at_query.clone(),
        }
    }

    fn resource_type(&self) -> NodeType {
        NodeType::Model
    }

    fn base_mut(&mut self) -> &mut NodeBaseAttributes {
        &mut self.__base_attr__
    }

    fn common_mut(&mut self) -> &mut CommonAttributes {
        &mut self.__common_attr__
    }
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn serialize_inner(
        &self,
        mode: crate::schemas::serialization_utils::SerializationMode,
    ) -> YmlValue {
        crate::schemas::serialization_utils::serialize_with_mode(self, mode)
    }

    fn has_same_config(&self, other: &dyn InternalDbtNode, _adapter_type: AdapterType) -> bool {
        if let Some(other_model) = other.as_any().downcast_ref::<DbtModel>() {
            let deprecated_config_eq = self
                .deprecated_config
                .same_config(&other_model.deprecated_config);

            if !deprecated_config_eq {
                log_state_mod_diff(
                    &self.__common_attr__.unique_id,
                    "model_config",
                    [("deprecated_config", deprecated_config_eq, None)],
                );
            }

            deprecated_config_eq
        } else {
            false
        }
    }

    fn has_same_content(&self, other: &dyn InternalDbtNode, _adapter_type: AdapterType) -> bool {
        // TODO: the checksum for extended model is always different in mantle and fusion, dig more into this
        if self.is_extended_model() {
            return true;
        }

        if let Some(other_model) = other.as_any().downcast_ref::<DbtModel>() {
            // Equivalent to dbt-core's same_contents method for ParsedNode
            let same_body_result = same_body(&self.__common_attr__, &other_model.__common_attr__);
            // Config is owned by `check_configs_modified`; calling `has_same_config` here would
            // double-count it. This check is therefore restricted to non-config fields.
            let same_persisted_desc_result = same_persisted_description(
                &self.__common_attr__,
                &self.__base_attr__,
                &other_model.__common_attr__,
                &other_model.__base_attr__,
            );
            let same_fqn_result = same_fqn(&self.__common_attr__, &other_model.__common_attr__);
            let same_db_repr_result = same_database_representation_model(
                &self.__base_attr__,
                &other_model.__base_attr__,
                &self.deprecated_config,
                &other_model.deprecated_config,
            );
            let same_contract_result = self.same_contract(other_model);
            let same_ref_repr_result = self.same_ref_representation(other_model);

            let result = same_body_result
                && same_persisted_desc_result
                && same_fqn_result
                && same_db_repr_result
                && same_contract_result
                && same_ref_repr_result;

            if !result {
                log_state_mod_diff(
                    &self.__common_attr__.unique_id,
                    "model",
                    [
                        ("same_body", same_body_result, None),
                        (
                            "same_persisted_description",
                            same_persisted_desc_result,
                            None,
                        ),
                        (
                            "same_fqn",
                            same_fqn_result,
                            Some((
                                format!("{:?}", &self.common().fqn),
                                format!("{:?}", &other_model.common().fqn),
                            )),
                        ),
                        ("same_database_representation", same_db_repr_result, None),
                        ("same_contract", same_contract_result, None),
                        ("same_ref_representation", same_ref_repr_result, None),
                    ],
                );
            }

            result
        } else {
            false
        }
    }
    fn set_detected_introspection(&mut self, introspection: IntrospectionKind) {
        self.__model_attr__.introspection = introspection;
    }
    fn introspection(&self) -> IntrospectionKind {
        self.__model_attr__.introspection
    }

    fn warn_on_microbatch(&self, adapter_type: AdapterType) -> usize {
        let uses_microbatch =
            self.__model_attr__.incremental_strategy == Some(DbtIncrementalStrategy::Microbatch);
        let forces_concurrent_batches = self.deprecated_config.concurrent_batches == Some(true);

        usize::from(
            uses_microbatch
                && forces_concurrent_batches
                && !MICROBATCH_SUPPORTED_ADAPTERS.contains(&adapter_type),
        )
    }

    fn get_group(&self) -> Option<String> {
        self.__model_attr__.group.clone()
    }
}

impl InternalDbtNodeAttributes for DbtModel {
    fn get_access(&self) -> Option<Access> {
        Some(self.__model_attr__.access.clone())
    }

    fn compiled_code(&self) -> Option<&str> {
        self.__model_attr__.compiled_code.as_deref()
    }

    fn search_name(&self) -> String {
        if let Some(version) = &self.__model_attr__.version {
            format!("{}.v{}", self.__common_attr__.name, version)
        } else {
            self.__common_attr__.name.clone()
        }
    }

    fn selector_string(&self) -> String {
        self.__common_attr__.fqn.join(".")
    }

    fn serialized_config(&self) -> YmlValue {
        dbt_yaml::to_value(&self.deprecated_config).expect("Failed to serialize to YAML")
    }

    fn schema_refresh_interval(&self) -> Option<SchemaRefreshInterval> {
        self.__model_attr__
            .sync
            .as_ref()
            .and_then(|sync| sync.schema_refresh_interval.clone())
    }
}

/// Helper function to compare materialized fields for seeds, treating None and default Seed materialization as equivalent
fn seed_materialized_eq(a: &Option<DbtMaterialization>, b: &Option<DbtMaterialization>) -> bool {
    use crate::schemas::common::DbtMaterialization;
    // Default value for seeds is always "seed"
    // See https://github.com/dbt-labs/dbt-core/blob/main/core/dbt/artifacts/resources/v1/seed.py#L16
    let default_seed_materialized = DbtMaterialization::Seed;

    match (a, b) {
        // Both None
        (None, None) => true,
        // Both Some - direct comparison
        (Some(a_val), Some(b_val)) => a_val == b_val,
        // One None, one Some - check if the Some value equals default seed materialization
        (None, Some(b_val)) => b_val == &default_seed_materialized,
        (Some(a_val), None) => a_val == &default_seed_materialized,
    }
}

/// Helper functions for smart comparison of SeedConfig fields that considers
/// None vs Some(empty) representations as equivalent
fn seed_configs_equal(left: &SeedConfig, right: &SeedConfig) -> bool {
    // Compare each field with smart empty comparison
    let column_types_eq = btree_map_equal(&left.column_types, &right.column_types);
    let docs_eq = docs_config_equal(&left.docs, &right.docs);
    let enabled_eq = left.enabled == right.enabled;
    let grants_eq = grants_equal(&left.grants, &right.grants);
    let quote_columns_eq = left.quote_columns == right.quote_columns;
    // left.delimiter == right.delimiter && // TODO: re-enable when no longer using mantle/core manifests in IA
    let event_time_eq = left.event_time == right.event_time;
    let full_refresh_eq = left.full_refresh == right.full_refresh;
    let meta_eq = indexmap_yml_value_equal(&left.meta, &right.meta);
    let persist_docs_eq = persist_docs_configs_equal(&left.persist_docs, &right.persist_docs);
    let post_hook_eq = hooks_equal(&left.post_hook, &right.post_hook);
    let pre_hook_eq = hooks_equal(&left.pre_hook, &right.pre_hook);
    // quoting_equal(&left.quoting, &right.quoting) && // TODO: re-enable when no longer using mantle/core manifests in IA
    let materialized_eq = seed_materialized_eq(&left.materialized, &right.materialized);
    let warehouse_config_eq = same_warehouse_config(
        &left.__warehouse_specific_config__,
        &right.__warehouse_specific_config__,
    );

    let result = column_types_eq
        && docs_eq
        && enabled_eq
        && grants_eq
        && quote_columns_eq
        && event_time_eq
        && full_refresh_eq
        && meta_eq
        && persist_docs_eq
        && post_hook_eq
        && pre_hook_eq
        && materialized_eq
        && warehouse_config_eq;

    if !result {
        log_state_mod_diff(
            "unique_id in next seed_config log",
            "seed_config",
            [
                (
                    "column_types",
                    column_types_eq,
                    Some((
                        format!("{:?}", &left.column_types),
                        format!("{:?}", &right.column_types),
                    )),
                ),
                (
                    "docs",
                    docs_eq,
                    Some((format!("{:?}", &left.docs), format!("{:?}", &right.docs))),
                ),
                (
                    "enabled",
                    enabled_eq,
                    Some((
                        format!("{:?}", &left.enabled),
                        format!("{:?}", &right.enabled),
                    )),
                ),
                (
                    "grants",
                    grants_eq,
                    Some((
                        format!("{:?}", &left.grants),
                        format!("{:?}", &right.grants),
                    )),
                ),
                (
                    "quote_columns",
                    quote_columns_eq,
                    Some((
                        format!("{:?}", &left.quote_columns),
                        format!("{:?}", &right.quote_columns),
                    )),
                ),
                (
                    "event_time",
                    event_time_eq,
                    Some((
                        format!("{:?}", &left.event_time),
                        format!("{:?}", &right.event_time),
                    )),
                ),
                (
                    "full_refresh",
                    full_refresh_eq,
                    Some((
                        format!("{:?}", &left.full_refresh),
                        format!("{:?}", &right.full_refresh),
                    )),
                ),
                (
                    "meta",
                    meta_eq,
                    Some((format!("{:?}", &left.meta), format!("{:?}", &right.meta))),
                ),
                ("persist_docs", persist_docs_eq, None),
                (
                    "post_hook",
                    post_hook_eq,
                    Some((
                        format!("{:?}", &left.post_hook),
                        format!("{:?}", &right.post_hook),
                    )),
                ),
                (
                    "pre_hook",
                    pre_hook_eq,
                    Some((
                        format!("{:?}", &left.pre_hook),
                        format!("{:?}", &right.pre_hook),
                    )),
                ),
                (
                    "materialized",
                    materialized_eq,
                    Some((
                        format!("{:?}", &left.materialized),
                        format!("{:?}", &right.materialized),
                    )),
                ),
                ("warehouse_config", warehouse_config_eq, None),
            ],
        );
    }

    result
}

/// Compare BTreeMap<Spanned<String>, String> considering None vs Some(empty) as equal
fn btree_map_equal(
    left: &Option<BTreeMap<Spanned<String>, String>>,
    right: &Option<BTreeMap<Spanned<String>, String>>,
) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(l), Some(r)) => {
            // Convert both maps to lowercase for case-insensitive comparison
            let l_normalized: BTreeMap<String, String> = l
                .iter()
                .map(|(k, v)| (k.to_lowercase(), v.to_lowercase()))
                .collect();
            let r_normalized: BTreeMap<String, String> = r
                .iter()
                .map(|(k, v)| (k.to_lowercase(), v.to_lowercase()))
                .collect();
            l_normalized == r_normalized
        }
        (None, Some(r)) => r.is_empty(),
        (Some(l), None) => l.is_empty(),
    }
}

/// Compare IndexMap<String, YmlValue> considering None vs Some(empty) as equal
fn indexmap_yml_value_equal(
    left: &Option<IndexMap<String, YmlValue>>,
    right: &Option<IndexMap<String, YmlValue>>,
) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(l), Some(r)) => l == r,
        (None, Some(r)) => r.is_empty(),
        (Some(l), None) => l.is_empty(),
    }
}

/// Compare DocsConfig considering None vs Some(default) as equal
fn docs_config_equal(
    left: &Option<crate::schemas::common::DocsConfig>,
    right: &Option<crate::schemas::common::DocsConfig>,
) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(l), Some(r)) => l == r,
        (
            None,
            Some(crate::schemas::common::DocsConfig {
                show: true,
                node_color: None,
            }),
        ) => true,
        (
            Some(crate::schemas::common::DocsConfig {
                show: true,
                node_color: None,
            }),
            None,
        ) => true,
        _ => false,
    }
}

/// Compare DbtQuoting structures with more nuanced logic
/// This handles cases where one side has values and the other has all None fields
fn quoting_equal(
    left: &Option<crate::schemas::common::DbtQuoting>,
    right: &Option<crate::schemas::common::DbtQuoting>,
    adapter_type: AdapterType,
) -> bool {
    use crate::schemas::common::DbtQuoting;

    let default = default_dbt_quoting_for(adapter_type);
    let resolve = |q: Option<&DbtQuoting>| -> DbtQuoting {
        let q = q.cloned().unwrap_or_default();
        DbtQuoting {
            database: Some(q.database.unwrap_or_else(|| {
                default
                    .database
                    .expect("default quoting must have database")
            })),
            schema: Some(
                q.schema
                    .unwrap_or_else(|| default.schema.expect("default quoting must have schema")),
            ),
            identifier: Some(q.identifier.unwrap_or_else(|| {
                default
                    .identifier
                    .expect("default quoting must have identifier")
            })),
            snowflake_ignore_case: Some(q.snowflake_ignore_case.unwrap_or_else(|| {
                default
                    .snowflake_ignore_case
                    .expect("default quoting must have snowflake_ignore_case")
            })),
        }
    };

    resolve(left.as_ref()) == resolve(right.as_ref())
}

pub(crate) fn test_alias_config_equal(
    left_config_alias: Option<&str>,
    right_config_alias: Option<&str>,
    left_resolved_alias: &str,
    right_resolved_alias: &str,
) -> bool {
    if left_config_alias == right_config_alias {
        return true;
    }

    let generated_alias_eq = match (left_config_alias, right_config_alias) {
        (None, Some(right)) => right == right_resolved_alias,
        (Some(left), None) => left == left_resolved_alias,
        _ => false,
    };

    generated_alias_eq && left_resolved_alias == right_resolved_alias
}

impl InternalDbtNode for DbtSeed {
    fn resource_type(&self) -> NodeType {
        NodeType::Seed
    }

    fn event_time(&self) -> Option<String> {
        self.deprecated_config.event_time.clone()
    }

    fn common(&self) -> &CommonAttributes {
        &self.__common_attr__
    }

    fn common_mut(&mut self) -> &mut CommonAttributes {
        &mut self.__common_attr__
    }

    fn base(&self) -> &NodeBaseAttributes {
        &self.__base_attr__
    }
    fn base_mut(&mut self) -> &mut NodeBaseAttributes {
        &mut self.__base_attr__
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn serialize_inner(
        &self,
        mode: crate::schemas::serialization_utils::SerializationMode,
    ) -> YmlValue {
        crate::schemas::serialization_utils::serialize_with_mode(self, mode)
    }

    fn has_same_config(&self, other: &dyn InternalDbtNode, _adapter_type: AdapterType) -> bool {
        if let Some(other_model) = other.as_any().downcast_ref::<DbtSeed>() {
            let deprecated_config_eq =
                seed_configs_equal(&self.deprecated_config, &other_model.deprecated_config);

            if !deprecated_config_eq {
                log_state_mod_diff(
                    &self.__common_attr__.unique_id,
                    "seed_config",
                    [("deprecated_config", deprecated_config_eq, None)],
                );
            }

            deprecated_config_eq
        } else {
            false
        }
    }

    fn has_same_content(&self, other: &dyn InternalDbtNode, _adapter_type: AdapterType) -> bool {
        if let Some(other_seed) = other.as_any().downcast_ref::<DbtSeed>() {
            // Equivalent to dbt-core's same_contents method for ParsedNode
            // TODO: Seeds might have path based checksum. When they do,
            // warnings are logged about this. Implement these warnings later
            // after confirming they make sense. See:
            //https://github.com/dbt-labs/dbt-core/blob/b75d5e701ef4dc2d7a98c5301ef63ecfc02eae15/core/dbt/contracts/graph/nodes.py#L900-L933
            let mut same_body_result =
                same_body(&self.__common_attr__, &other_seed.__common_attr__);

            // Migration fallback: the new text-mode seed hash normalizes CRLF -> LF,
            // so a freshly computed seed checksum might not match one stored by the
            // old binary-mode hashing even when the file is unchanged. Recompute the
            // current seed with the legacy hash and compare to the previous checksum,
            // mirroring dbt-core's `same_seeds` fallback.
            if !same_body_result {
                if let (DbtChecksum::Object(self_cs), Some(root_path)) = (
                    &self.__common_attr__.checksum,
                    self.__seed_attr__.root_path.as_ref(),
                ) {
                    if self_cs.name == "sha256" {
                        let seed_path = root_path.join(&self.__common_attr__.path);
                        if let Ok(bytes) = std::fs::read(&seed_path) {
                            let legacy = DbtChecksum::seed_content_checksum_legacy(&bytes);
                            same_body_result = legacy == other_seed.__common_attr__.checksum;
                        }
                    }
                }
            }

            // Config is owned by `check_configs_modified`; calling `has_same_config` here would
            // double-count it. This check is therefore restricted to non-config fields.
            let same_persisted_desc_result = same_persisted_description(
                &self.__common_attr__,
                &self.__base_attr__,
                &other_seed.__common_attr__,
                &other_seed.__base_attr__,
            );
            let same_fqn_result = same_fqn(&self.__common_attr__, &other_seed.__common_attr__);
            let same_db_repr_result = same_database_representation_seed(
                &self.deprecated_config,
                &other_seed.deprecated_config,
            );

            // For seeds, same_contract always returns true in dbt-core
            // Seeds don't have complex contract validation like models,
            // so we do not need to do a contract check for seeds.

            let result = same_body_result
                && same_persisted_desc_result
                && same_fqn_result
                && same_db_repr_result;

            if !result {
                log_state_mod_diff(
                    &self.__common_attr__.unique_id,
                    "seed",
                    [
                        ("same_body", same_body_result, None),
                        (
                            "same_persisted_description",
                            same_persisted_desc_result,
                            None,
                        ),
                        (
                            "same_fqn",
                            same_fqn_result,
                            Some((
                                format!("{:?}", &self.common().fqn),
                                format!("{:?}", &other_seed.common().fqn),
                            )),
                        ),
                        ("same_database_representation", same_db_repr_result, None),
                    ],
                );
            }

            result
        } else {
            false
        }
    }
    fn set_detected_introspection(&mut self, _introspection: IntrospectionKind) {
        panic!("DbtSeed does not support setting detected_unsafe");
    }
}

impl InternalDbtNodeAttributes for DbtSeed {
    fn search_name(&self) -> String {
        self.__common_attr__.name.clone()
    }

    fn selector_string(&self) -> String {
        self.__common_attr__.fqn.join(".")
    }

    fn serialized_config(&self) -> YmlValue {
        dbt_yaml::to_value(&self.deprecated_config).expect("Failed to serialize to YAML")
    }
}

impl InternalDbtNode for DbtTest {
    fn common(&self) -> &CommonAttributes {
        &self.__common_attr__
    }

    fn base(&self) -> &NodeBaseAttributes {
        &self.__base_attr__
    }

    fn resource_type(&self) -> NodeType {
        NodeType::Test
    }

    fn base_mut(&mut self) -> &mut NodeBaseAttributes {
        &mut self.__base_attr__
    }

    fn common_mut(&mut self) -> &mut CommonAttributes {
        &mut self.__common_attr__
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn defined_at(&self) -> Option<&dbt_common::CodeLocationWithFile> {
        self.defined_at.as_ref()
    }

    fn original_name(&self) -> Option<&str> {
        self.__test_attr__.original_name.as_deref()
    }

    fn serialize_inner(
        &self,
        mode: crate::schemas::serialization_utils::SerializationMode,
    ) -> YmlValue {
        crate::schemas::serialization_utils::serialize_with_mode(self, mode)
    }

    fn has_same_config(&self, other: &dyn InternalDbtNode, _adapter_type: AdapterType) -> bool {
        if let Some(other) = other.as_any().downcast_ref::<DbtTest>() {
            // Rendered comparison of a data test node's config fields — the Stage-2 fallback of
            // `check_configs_modified`.
            // The checks here are meant to be for the same keys as in the Stage-1 unrendered checks
            // directed by `DBTTEST_CONFIG_MODIFIERS`.
            let c = &self.deprecated_config;
            let o = &other.deprecated_config;

            let severity_eq = c.severity == o.severity;
            let where_eq = c.where_ == o.where_;
            let limit_eq = c.limit == o.limit;
            let fail_calc_eq = c.fail_calc == o.fail_calc;
            let warn_if_eq = c.warn_if == o.warn_if;
            let error_if_eq = c.error_if == o.error_if;
            let store_failures_eq = c.store_failures == o.store_failures;
            let store_failures_as_eq = c.store_failures_as == o.store_failures_as;
            let sql_header_eq = c.sql_header == o.sql_header;

            let result = severity_eq
                && where_eq
                && limit_eq
                && fail_calc_eq
                && warn_if_eq
                && error_if_eq
                && store_failures_eq
                && store_failures_as_eq
                && sql_header_eq;

            if !result {
                log_state_mod_diff(
                    &self.__common_attr__.unique_id,
                    "test_config",
                    [
                        (
                            "severity",
                            severity_eq,
                            Some((format!("{:?}", &c.severity), format!("{:?}", &o.severity))),
                        ),
                        (
                            "where",
                            where_eq,
                            Some((format!("{:?}", &c.where_), format!("{:?}", &o.where_))),
                        ),
                        (
                            "limit",
                            limit_eq,
                            Some((format!("{:?}", &c.limit), format!("{:?}", &o.limit))),
                        ),
                        (
                            "fail_calc",
                            fail_calc_eq,
                            Some((format!("{:?}", &c.fail_calc), format!("{:?}", &o.fail_calc))),
                        ),
                        (
                            "warn_if",
                            warn_if_eq,
                            Some((format!("{:?}", &c.warn_if), format!("{:?}", &o.warn_if))),
                        ),
                        (
                            "error_if",
                            error_if_eq,
                            Some((format!("{:?}", &c.error_if), format!("{:?}", &o.error_if))),
                        ),
                        (
                            "store_failures",
                            store_failures_eq,
                            Some((
                                format!("{:?}", &c.store_failures),
                                format!("{:?}", &o.store_failures),
                            )),
                        ),
                        (
                            "store_failures_as",
                            store_failures_as_eq,
                            Some((
                                format!("{:?}", &c.store_failures_as),
                                format!("{:?}", &o.store_failures_as),
                            )),
                        ),
                        (
                            "sql_header",
                            sql_header_eq,
                            Some((
                                format!("{:?}", &c.sql_header),
                                format!("{:?}", &o.sql_header),
                            )),
                        ),
                    ],
                );
            }

            result
        } else {
            false
        }
    }

    fn has_same_content(&self, other: &dyn InternalDbtNode, _adapter_type: AdapterType) -> bool {
        if let Some(other_test) = other.as_any().downcast_ref::<DbtTest>() {
            let same_fqn_result = self.common().fqn == other_test.common().fqn;

            // dbt-core: `GenericTestNode.same_contents` overrides `ParsedNode`'s to
            // `same_config and same_fqn` (nodes.py:1170-1174) -- a generic test's body is just
            // the macro-call text, not authored content, so body is deliberately excluded.
            // `SingularTestNode` has no such override, so it inherits `ParsedNode.same_contents`,
            // which ANDs `same_body` (nodes.py:373-374,402-416). `test_metadata` is populated only
            // for generic tests (see `DbtTestAttr`), so it's the same discriminator dbt-core's
            // class split encodes.
            if self.__test_attr__.test_metadata.is_some() {
                // Generic test: unchanged, fqn only.
                if !same_fqn_result {
                    log_state_mod_diff(
                        &self.__common_attr__.unique_id,
                        "test",
                        [(
                            "same_fqn",
                            same_fqn_result,
                            Some((
                                format!("{:?}", &self.common().fqn),
                                format!("{:?}", &other_test.common().fqn),
                            )),
                        )],
                    );
                }

                same_fqn_result
            } else {
                // Singular test: also consult body (checksum-based, like `DbtModel`'s).
                //
                // Deliberately stop at `same_fqn && same_body`. `ParsedNode.same_contents` also
                // ANDs relation, persisted-description and contract, but do NOT add those checks
                // here: `is_modified`'s `Any` case (`prev_state/mod.rs`) already ORs
                // `check_relation_modified`, `check_persisted_descriptions_modified`,
                // `check_modified_macros` and `check_contract_modified` for every node type,
                // singular tests included. Re-adding them here would double-count -- the
                // selection result would be unchanged, but the `[state_mod_diff]` trace would
                // misattribute which sub-check actually caught the difference.
                let same_body_result =
                    same_body(&self.__common_attr__, &other_test.__common_attr__);
                let result = same_fqn_result && same_body_result;

                if !result {
                    log_state_mod_diff(
                        &self.__common_attr__.unique_id,
                        "test",
                        [
                            (
                                "same_fqn",
                                same_fqn_result,
                                Some((
                                    format!("{:?}", &self.common().fqn),
                                    format!("{:?}", &other_test.common().fqn),
                                )),
                            ),
                            ("same_body", same_body_result, None),
                        ],
                    );
                }

                result
            }
        } else {
            false
        }
    }
    fn set_detected_introspection(&mut self, introspection: IntrospectionKind) {
        self.__test_attr__.introspection = introspection;
    }
    fn introspection(&self) -> IntrospectionKind {
        self.__test_attr__.introspection
    }

    fn get_group(&self) -> Option<String> {
        self.__test_attr__.group.clone()
    }
}

impl InternalDbtNodeAttributes for DbtTest {
    fn search_name(&self) -> String {
        self.__common_attr__.name.clone()
    }

    fn selector_string(&self) -> String {
        self.__common_attr__.fqn.join(".")
    }

    fn serialized_config(&self) -> YmlValue {
        dbt_yaml::to_value(&self.deprecated_config).expect("Failed to serialize to YAML")
    }
}

impl InternalDbtNode for DbtUnitTest {
    fn common(&self) -> &CommonAttributes {
        &self.__common_attr__
    }

    fn base(&self) -> &NodeBaseAttributes {
        &self.__base_attr__
    }

    fn resource_type(&self) -> NodeType {
        NodeType::UnitTest
    }

    fn base_mut(&mut self) -> &mut NodeBaseAttributes {
        &mut self.__base_attr__
    }

    fn common_mut(&mut self) -> &mut CommonAttributes {
        &mut self.__common_attr__
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn defined_at(&self) -> Option<&dbt_common::CodeLocationWithFile> {
        self.defined_at.as_ref()
    }

    fn serialize_inner(
        &self,
        mode: crate::schemas::serialization_utils::SerializationMode,
    ) -> YmlValue {
        crate::schemas::serialization_utils::serialize_with_mode(self, mode)
    }

    // Retained only for the `InternalDbtNode` trait; no longer consulted for state:modified
    // selection, since config is a non-trigger for unit tests (see `check_configs_modified`).
    fn has_same_config(&self, other: &dyn InternalDbtNode, _adapter_type: AdapterType) -> bool {
        if let Some(other) = other.as_any().downcast_ref::<DbtUnitTest>() {
            let self_config = &self.deprecated_config;
            let other_config = &other.deprecated_config;

            let enabled_eq = self_config.enabled == other_config.enabled;
            // `static_analysis` is a Fusion-only, invocation-driven value (e.g. set by
            // `--static-analysis`) with no dbt-core equivalent, so it can never be a
            // legitimate dbt-core `state:modified` trigger and is deliberately excluded
            // from this comparison (see `base_config_excluded_keys`, parity-exclude).

            let result = enabled_eq;

            if !result {
                log_state_mod_diff(
                    &self.__common_attr__.unique_id,
                    "unit_test_config",
                    [(
                        "enabled",
                        enabled_eq,
                        Some((
                            format!("{:?}", &self_config.enabled),
                            format!("{:?}", &other_config.enabled),
                        )),
                    )],
                );
            }

            result
        } else {
            false
        }
    }

    fn has_same_content(&self, other: &dyn InternalDbtNode, _adapter_type: AdapterType) -> bool {
        if let Some(other_unit_test) = other.as_any().downcast_ref::<DbtUnitTest>() {
            let same_fqn_result = self.common().fqn == other_unit_test.common().fqn;
            // TODO: replace the dummy implementation of same_unit_test_definition with a real comparison
            let same_definition_result = same_unit_test_definition(self, other_unit_test);

            let result = same_fqn_result && same_definition_result;

            if !result {
                log_state_mod_diff(
                    &self.__common_attr__.unique_id,
                    "unit_test",
                    [(
                        "same_fqn",
                        same_fqn_result,
                        Some((
                            format!("{:?}", &self.common().fqn),
                            format!("{:?}", &other_unit_test.common().fqn),
                        )),
                    )],
                );
            }

            result
        } else {
            false
        }
    }
    fn set_detected_introspection(&mut self, _introspection: IntrospectionKind) {
        panic!("DbtUnitTest does not support setting detected_unsafe");
    }
}

fn same_unit_test_definition(_self_ut: &DbtUnitTest, _other_ut: &DbtUnitTest) -> bool {
    // This dummy implementation makes explicit Fusion's current unit-test content under-selection:
    true
    // TODO: Implement parity with dbt-core's flagging a unit test on a checksum over
    // model/versions/given/expect/overrides (+ fixture rows) — build_unit_test_checksum,
    // dbt-mantle core/dbt/contracts/graph/nodes.py:1226.
    // HAZARD when fixing: do NOT naively compare `checksum`. dbt-core manifests carry a real
    // unit-test checksum while Fusion emits DbtChecksum::default(); comparing them directly would
    // over-select every unit test against a dbt-core state manifest. The fix must populate Fusion's
    // unit-test checksum consistently first, then compare.
}

impl InternalDbtNodeAttributes for DbtUnitTest {
    fn search_name(&self) -> String {
        // Based on Python implementation, unit tests can have a versioned name
        if let Some(version) = &self.__unit_test_attr__.version {
            format!("{}_v{}", self.__common_attr__.name, version)
        } else {
            self.__common_attr__.name.clone()
        }
    }

    fn selector_string(&self) -> String {
        format!(
            "unit_test:{}.{}",
            self.__common_attr__.package_name,
            self.search_name()
        )
    }

    fn serialized_config(&self) -> YmlValue {
        dbt_yaml::to_value(&self.deprecated_config).expect("Failed to serialize to YAML")
    }
}

impl InternalDbtNode for DbtSource {
    fn common(&self) -> &CommonAttributes {
        &self.__common_attr__
    }

    fn base(&self) -> &NodeBaseAttributes {
        &self.__base_attr__
    }

    fn resource_type(&self) -> NodeType {
        NodeType::Source
    }

    fn freshness_criteria(&self) -> FreshnessDefinition {
        self.__source_attr__.freshness.clone().unwrap_or_default()
    }

    fn event_time(&self) -> Option<String> {
        self.deprecated_config.event_time.clone()
    }

    fn get_node_processed_event(
        &self,
        last_phase: Option<ExecutionPhase>,
        in_dir: &Path,
        out_dir: &Path,
        in_selection: bool,
    ) -> NodeProcessed {
        let mut event =
            self.get_node_processed_event_base(last_phase, in_dir, out_dir, in_selection);
        event.source_name = Some(self.__source_attr__.source_name.clone());
        event
    }

    fn base_mut(&mut self) -> &mut NodeBaseAttributes {
        &mut self.__base_attr__
    }

    fn common_mut(&mut self) -> &mut CommonAttributes {
        &mut self.__common_attr__
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn serialize_inner(
        &self,
        mode: crate::schemas::serialization_utils::SerializationMode,
    ) -> YmlValue {
        crate::schemas::serialization_utils::serialize_with_mode(self, mode)
    }

    fn has_same_config(&self, other: &dyn InternalDbtNode, adapter_type: AdapterType) -> bool {
        if let Some(other_source) = other.as_any().downcast_ref::<DbtSource>() {
            // Merged fields are scattered across the DbtSource struct, while unmerged
            // fields are in deprecated_config.
            let self_config = &self.deprecated_config;
            let other_config = &other_source.deprecated_config;

            // Compare each field individually
            let enabled_eq = self.__base_attr__.enabled == other_source.__base_attr__.enabled;

            let event_time_eq = self_config.event_time == other_config.event_time;

            // Rendered freshness equality (None ≈ empty-FreshnessDefinition). Shared with the
            // `check_source_freshness_modified` state:modified sub-check via `same_freshness_value`.
            let freshness_eq = self.same_freshness_value(other_source);

            let quoting_eq = quoting_equal(
                &self.__source_attr__.user_quoting,
                &other_source.__source_attr__.user_quoting,
                adapter_type,
            );

            let loaded_at_field_eq = Self::loaded_at_str_eq(
                &self.__source_attr__.loaded_at_field,
                &other_source.__source_attr__.loaded_at_field,
            );

            let loaded_at_query_result = Self::loaded_at_str_eq(
                &self.__source_attr__.loaded_at_query,
                &other_source.__source_attr__.loaded_at_query,
            );

            // `static_analysis` is a Fusion-only, invocation-driven value (e.g. set by
            // `--static-analysis`) with no dbt-core equivalent, so it can never be a
            // legitimate dbt-core `state:modified` trigger and is deliberately excluded
            // from this comparison (see `base_config_excluded_keys`, parity-exclude).
            let warehouse_config_eq = same_warehouse_config(
                &self_config.__warehouse_specific_config__,
                &other_config.__warehouse_specific_config__,
            );

            let result = enabled_eq
                && event_time_eq
                && freshness_eq
                && quoting_eq
                && loaded_at_field_eq
                && loaded_at_query_result
                && warehouse_config_eq;

            if !result {
                log_state_mod_diff(
                    &self.__common_attr__.unique_id,
                    "source_config",
                    [
                        (
                            "enabled",
                            enabled_eq,
                            Some((
                                format!("{:?}", &self.__base_attr__.enabled),
                                format!("{:?}", &other_source.__base_attr__.enabled),
                            )),
                        ),
                        (
                            "event_time",
                            event_time_eq,
                            Some((
                                format!("{:?}", &self_config.event_time),
                                format!("{:?}", &other_config.event_time),
                            )),
                        ),
                        (
                            "freshness",
                            freshness_eq,
                            Some((
                                format!("{:?}", &self.__source_attr__.freshness),
                                format!("{:?}", &other_source.__source_attr__.freshness),
                            )),
                        ),
                        (
                            "quoting",
                            quoting_eq,
                            Some((
                                format!("{:?}", &self.__source_attr__.user_quoting),
                                format!("{:?}", &other_source.__source_attr__.user_quoting),
                            )),
                        ),
                        (
                            "loaded_at_field",
                            loaded_at_field_eq,
                            Some((
                                format!("{:?}", &self.__source_attr__.loaded_at_field),
                                format!("{:?}", &other_source.__source_attr__.loaded_at_field),
                            )),
                        ),
                        (
                            "loaded_at_query",
                            loaded_at_query_result,
                            Some((
                                format!("{:?}", &self.__source_attr__.loaded_at_query),
                                format!("{:?}", &other_source.__source_attr__.loaded_at_query),
                            )),
                        ),
                        ("warehouse_config", warehouse_config_eq, None),
                    ],
                );
            }

            result
        } else {
            false
        }
    }

    fn has_same_content(&self, other: &dyn InternalDbtNode, _adapter_type: AdapterType) -> bool {
        if let Some(other_source) = other.as_any().downcast_ref::<DbtSource>() {
            // Sources have no SQL body. Config is owned by `check_configs_modified` and relation
            // identity by `check_relation_modified`; calling `has_same_config` here would
            // double-count both. This check is therefore restricted to fields WITHOUT a dedicated
            // owner: table identifier (last segment of `relation_name`), fqn, loader, and the
            // RENDERED `freshness` (+ `loaded_at_field`).
            //
            // FRESHNESS is compared here — RENDERED — to mirror dbt-core's
            // `SourceDefinition.same_freshness` (dbt-mantle `core/dbt/contracts/graph/nodes.py:1444`),
            // which is a distinct conjunct of `same_contents` alongside `same_config`. #11463 routed
            // source config through a wholesale UNRENDERED comparison (`check_configs_modified`),
            // faithful to dbt-core for every key it also compares unrendered via `same_config` — but
            // freshness is the lone exception dbt-core ALSO compares rendered, so the unrendered-only
            // path stopped selecting an env-aware `freshness`/`loaded_at_field` that renders
            // differently per target (a change dbt-core still selects under full `state:modified`).
            // Comparing it here restores that fidelity while keeping the logic node-local: dbt-core
            // reaches `same_freshness` only through full `same_contents`, and Fusion reaches
            // `has_same_content` only through `check_modified_content` (the full-`state:modified`
            // arm) — never under `state:modified.configs`, which routes to `check_configs_modified`
            // and keeps freshness compared unrendered there, matching dbt-core's `same_config`.
            //
            // (This mirrors a dbt-core inconsistency: #10675 migrated database/schema to unrendered
            // behind `state_modified_compare_more_unrendered_values`, but the older `same_freshness`
            // was never gated. If dbt-core gates it on that flag, this comparison could be dropped
            // and freshness left entirely to the unrendered `check_configs_modified` path.)
            let same_relation_name_result = same_relation_name(
                &self.__base_attr__.relation_name,
                &other_source.__base_attr__.relation_name,
            );
            let same_fqn_result = self.__common_attr__.fqn == other_source.__common_attr__.fqn;
            let same_loader_result =
                self.__source_attr__.loader == other_source.__source_attr__.loader;
            let same_freshness_result = self.same_rendered_freshness(other_source);

            let result = same_relation_name_result
                && same_fqn_result
                && same_loader_result
                && same_freshness_result;

            if !result {
                log_state_mod_diff(
                    &self.__common_attr__.unique_id,
                    "source",
                    [
                        ("same_relation_name", same_relation_name_result, None),
                        (
                            "same_fqn",
                            same_fqn_result,
                            Some((
                                format!("{:?}", &self.__common_attr__.fqn),
                                format!("{:?}", &other_source.__common_attr__.fqn),
                            )),
                        ),
                        (
                            "same_loader",
                            same_loader_result,
                            Some((
                                format!("{:?}", &self.__source_attr__.loader),
                                format!("{:?}", &other_source.__source_attr__.loader),
                            )),
                        ),
                        (
                            "same_freshness",
                            same_freshness_result,
                            Some((
                                format!(
                                    "{:?} / loaded_at_field={:?}",
                                    &self.__source_attr__.freshness,
                                    &self.__source_attr__.loaded_at_field
                                ),
                                format!(
                                    "{:?} / loaded_at_field={:?}",
                                    &other_source.__source_attr__.freshness,
                                    &other_source.__source_attr__.loaded_at_field
                                ),
                            )),
                        ),
                    ],
                );
            }

            result
        } else {
            false
        }
    }
    fn set_detected_introspection(&mut self, _introspection: IntrospectionKind) {
        panic!("DbtSource does not support setting detected_unsafe");
    }
}

impl InternalDbtNodeAttributes for DbtSource {
    fn search_name(&self) -> String {
        format!(
            "{}.{}",
            self.__source_attr__.source_name, self.__common_attr__.name
        )
    }

    fn selector_string(&self) -> String {
        format!(
            "source:{}.{}.{}",
            self.__common_attr__.package_name,
            self.__source_attr__.source_name,
            self.__common_attr__.name
        )
    }

    fn serialized_config(&self) -> YmlValue {
        dbt_yaml::to_value(&self.deprecated_config).expect("Failed to serialize DbtModel")
    }

    fn schema_origin(&self) -> SchemaOrigin {
        self.__source_attr__.schema_origin
    }

    fn schema_refresh_interval(&self) -> Option<SchemaRefreshInterval> {
        self.__source_attr__
            .sync
            .as_ref()
            .and_then(|s| s.schema_refresh_interval.clone())
    }
}

impl InternalDbtNode for DbtSnapshot {
    fn common(&self) -> &CommonAttributes {
        &self.__common_attr__
    }

    fn base(&self) -> &NodeBaseAttributes {
        &self.__base_attr__
    }

    fn resource_type(&self) -> NodeType {
        NodeType::Snapshot
    }

    fn event_time(&self) -> Option<String> {
        self.deprecated_config.event_time.clone()
    }

    fn base_mut(&mut self) -> &mut NodeBaseAttributes {
        &mut self.__base_attr__
    }

    fn common_mut(&mut self) -> &mut CommonAttributes {
        &mut self.__common_attr__
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn defined_at(&self) -> Option<&dbt_common::CodeLocationWithFile> {
        if self.__common_attr__.name_span.start.line > 0 {
            Some(&self.__common_attr__.name_span.start)
        } else {
            None
        }
    }

    fn serialize_inner(
        &self,
        mode: crate::schemas::serialization_utils::SerializationMode,
    ) -> YmlValue {
        crate::schemas::serialization_utils::serialize_with_mode(self, mode)
    }

    fn has_same_config(&self, other: &dyn InternalDbtNode, adapter_type: AdapterType) -> bool {
        if let Some(other_snapshot) = other.as_any().downcast_ref::<DbtSnapshot>() {
            let self_config = &self.deprecated_config;
            let other_config = &other_snapshot.deprecated_config;

            // Snapshot-specific Configuration
            let alias_eq = self_config.alias == other_config.alias;
            let materialized_eq = self_config.materialized == other_config.materialized;
            let strategy_eq = self_config.strategy == other_config.strategy;
            let unique_key_eq = self_config.unique_key == other_config.unique_key;
            let check_cols_eq = self_config.check_cols == other_config.check_cols;
            let updated_at_eq = self_config.updated_at == other_config.updated_at;
            let dbt_valid_to_current_eq =
                self_config.dbt_valid_to_current == other_config.dbt_valid_to_current;

            let snapshot_meta_column_names_eq = {
                use crate::schemas::project::configs::snapshot_config::SnapshotMetaColumnNames;

                // Helper to check if a SnapshotMetaColumnNames has all fields as None
                let is_empty_snapshot_meta = |meta: &SnapshotMetaColumnNames| -> bool {
                    meta.dbt_scd_id.is_none()
                        && meta.dbt_updated_at.is_none()
                        && meta.dbt_valid_from.is_none()
                        && meta.dbt_valid_to.is_none()
                        && meta.dbt_is_deleted.is_none()
                };

                match (
                    &self_config.snapshot_meta_column_names,
                    &other_config.snapshot_meta_column_names,
                ) {
                    (None, None) => true,
                    (None, Some(other_meta)) => is_empty_snapshot_meta(other_meta),
                    (Some(self_meta), None) => is_empty_snapshot_meta(self_meta),
                    (Some(self_meta), Some(other_meta)) => self_meta == other_meta,
                }
            };

            let hard_deletes_eq = self_config.hard_deletes == other_config.hard_deletes;
            let target_database_eq = self_config.target_database == other_config.target_database;
            let target_schema_eq = self_config.target_schema == other_config.target_schema;

            // General Configuration
            let enabled_eq = self_config.enabled == other_config.enabled;
            let pre_hook_eq = hooks_equal(&self_config.pre_hook, &other_config.pre_hook);
            let post_hook_eq = hooks_equal(&self_config.post_hook, &other_config.post_hook);
            let persist_docs_eq =
                persist_docs_configs_equal(&self_config.persist_docs, &other_config.persist_docs);
            let meta_eq = indexmap_yml_value_equal(&self_config.meta, &other_config.meta);
            let grants_eq = grants_equal(&self_config.grants, &other_config.grants);
            let event_time_eq = self_config.event_time == other_config.event_time;
            let quoting_eq =
                quoting_equal(&self_config.quoting, &other_config.quoting, adapter_type);
            // `static_analysis` is a Fusion-only, invocation-driven value (e.g. set by
            // `--static-analysis`) with no dbt-core equivalent, so it can never be a
            // legitimate dbt-core `state:modified` trigger and is deliberately excluded
            // from this comparison (see `base_config_excluded_keys`, parity-exclude).
            let quote_columns_eq = self_config.quote_columns == other_config.quote_columns;
            let invalidate_hard_deletes_eq =
                self_config.invalidate_hard_deletes == other_config.invalidate_hard_deletes;
            // dbt State configs (mirror `ModelConfig::same_config`).
            let state_eq = self_config.state == other_config.state;

            // Adapter specific configs
            let warehouse_config_eq = same_warehouse_config(
                &self_config.__warehouse_specific_config__,
                &other_config.__warehouse_specific_config__,
            );

            let result = alias_eq
                && materialized_eq
                && strategy_eq
                && unique_key_eq
                && check_cols_eq
                && updated_at_eq
                && dbt_valid_to_current_eq
                && snapshot_meta_column_names_eq
                && hard_deletes_eq
                && target_database_eq
                && target_schema_eq
                // General Configuration
                && enabled_eq
                && pre_hook_eq
                && post_hook_eq
                && persist_docs_eq
                && meta_eq
                && grants_eq
                && event_time_eq
                && quoting_eq
                && quote_columns_eq
                && invalidate_hard_deletes_eq
                && state_eq
                // Adapter specific configs
                && warehouse_config_eq;

            if !result {
                log_state_mod_diff(
                    &self.__common_attr__.unique_id,
                    "snapshot_config",
                    [
                        (
                            "alias",
                            alias_eq,
                            Some((
                                format!("{:?}", &self_config.alias),
                                format!("{:?}", &other_config.alias),
                            )),
                        ),
                        (
                            "materialized",
                            materialized_eq,
                            Some((
                                format!("{:?}", &self_config.materialized),
                                format!("{:?}", &other_config.materialized),
                            )),
                        ),
                        (
                            "strategy",
                            strategy_eq,
                            Some((
                                format!("{:?}", &self_config.strategy),
                                format!("{:?}", &other_config.strategy),
                            )),
                        ),
                        (
                            "unique_key",
                            unique_key_eq,
                            Some((
                                format!("{:?}", &self_config.unique_key),
                                format!("{:?}", &other_config.unique_key),
                            )),
                        ),
                        (
                            "check_cols",
                            check_cols_eq,
                            Some((
                                format!("{:?}", &self_config.check_cols),
                                format!("{:?}", &other_config.check_cols),
                            )),
                        ),
                        (
                            "updated_at",
                            updated_at_eq,
                            Some((
                                format!("{:?}", &self_config.updated_at),
                                format!("{:?}", &other_config.updated_at),
                            )),
                        ),
                        (
                            "dbt_valid_to_current",
                            dbt_valid_to_current_eq,
                            Some((
                                format!("{:?}", &self_config.dbt_valid_to_current),
                                format!("{:?}", &other_config.dbt_valid_to_current),
                            )),
                        ),
                        (
                            "snapshot_meta_column_names",
                            snapshot_meta_column_names_eq,
                            Some((
                                format!("{:?}", &self_config.snapshot_meta_column_names),
                                format!("{:?}", &other_config.snapshot_meta_column_names),
                            )),
                        ),
                        (
                            "hard_deletes",
                            hard_deletes_eq,
                            Some((
                                format!("{:?}", &self_config.hard_deletes),
                                format!("{:?}", &other_config.hard_deletes),
                            )),
                        ),
                        (
                            "target_database",
                            target_database_eq,
                            Some((
                                format!("{:?}", &self_config.target_database),
                                format!("{:?}", &other_config.target_database),
                            )),
                        ),
                        (
                            "target_schema",
                            target_schema_eq,
                            Some((
                                format!("{:?}", &self_config.target_schema),
                                format!("{:?}", &other_config.target_schema),
                            )),
                        ),
                        (
                            "enabled",
                            enabled_eq,
                            Some((
                                format!("{:?}", &self_config.enabled),
                                format!("{:?}", &other_config.enabled),
                            )),
                        ),
                        (
                            "pre_hook",
                            pre_hook_eq,
                            Some((
                                format!("{:?}", &self_config.pre_hook),
                                format!("{:?}", &other_config.pre_hook),
                            )),
                        ),
                        (
                            "post_hook",
                            post_hook_eq,
                            Some((
                                format!("{:?}", &self_config.post_hook),
                                format!("{:?}", &other_config.post_hook),
                            )),
                        ),
                        ("persist_docs", persist_docs_eq, None),
                        (
                            "meta",
                            meta_eq,
                            Some((
                                format!("{:?}", &self_config.meta),
                                format!("{:?}", &other_config.meta),
                            )),
                        ),
                        (
                            "grants",
                            grants_eq,
                            Some((
                                format!("{:?}", &self_config.grants),
                                format!("{:?}", &other_config.grants),
                            )),
                        ),
                        (
                            "event_time",
                            event_time_eq,
                            Some((
                                format!("{:?}", &self_config.event_time),
                                format!("{:?}", &other_config.event_time),
                            )),
                        ),
                        (
                            "quoting",
                            quoting_eq,
                            Some((
                                format!("{:?}", &self_config.quoting),
                                format!("{:?}", &other_config.quoting),
                            )),
                        ),
                        (
                            "quote_columns",
                            quote_columns_eq,
                            Some((
                                format!("{:?}", &self_config.quote_columns),
                                format!("{:?}", &other_config.quote_columns),
                            )),
                        ),
                        (
                            "invalidate_hard_deletes",
                            invalidate_hard_deletes_eq,
                            Some((
                                format!("{:?}", &self_config.invalidate_hard_deletes),
                                format!("{:?}", &other_config.invalidate_hard_deletes),
                            )),
                        ),
                        (
                            "state",
                            state_eq,
                            Some((
                                format!("{:?}", &self_config.state),
                                format!("{:?}", &other_config.state),
                            )),
                        ),
                        ("warehouse_config", warehouse_config_eq, None),
                    ],
                );
            }

            result
        } else {
            false
        }
    }

    fn has_same_content(&self, other: &dyn InternalDbtNode, _adapter_type: AdapterType) -> bool {
        if let Some(other_snapshot) = other.as_any().downcast_ref::<DbtSnapshot>() {
            // Equivalent to dbt-core's same_contents method for ParsedNode
            let same_body_result =
                same_body(&self.__common_attr__, &other_snapshot.__common_attr__);
            // Config is owned by `check_configs_modified`; calling `has_same_config` here would
            // double-count it. This check is therefore restricted to non-config fields.
            let same_persisted_desc_result = same_persisted_description(
                &self.__common_attr__,
                &self.__base_attr__,
                &other_snapshot.__common_attr__,
                &other_snapshot.__base_attr__,
            );
            let same_fqn_result = same_fqn(&self.__common_attr__, &other_snapshot.__common_attr__);
            let same_db_repr_result = same_database_representation_snapshot(
                &self.__base_attr__,
                &other_snapshot.__base_attr__,
                &self.deprecated_config,
                &other_snapshot.deprecated_config,
            );

            // For snapshots, same_contract always returns true in dbt-core,
            // so we do not need to do a contract check for snapshots.
            // See: https://github.com/dbt-labs/dbt-core/blob/main/core/dbt/contracts/graph/nodes.py#L374

            let result = same_body_result
                && same_persisted_desc_result
                && same_fqn_result
                && same_db_repr_result;

            if !result {
                log_state_mod_diff(
                    &self.__common_attr__.unique_id,
                    "snapshot",
                    [
                        ("same_body", same_body_result, None),
                        (
                            "same_persisted_description",
                            same_persisted_desc_result,
                            None,
                        ),
                        (
                            "same_fqn",
                            same_fqn_result,
                            Some((
                                format!("{:?}", &self.common().fqn),
                                format!("{:?}", &other_snapshot.common().fqn),
                            )),
                        ),
                        ("same_database_representation", same_db_repr_result, None),
                    ],
                );
            }

            result
        } else {
            false
        }
    }

    fn set_detected_introspection(&mut self, introspection: IntrospectionKind) {
        self.__snapshot_attr__.introspection = introspection;
    }

    fn introspection(&self) -> IntrospectionKind {
        self.__snapshot_attr__.introspection
    }
}

impl InternalDbtNodeAttributes for DbtSnapshot {
    fn skip_generate_schema_name_macro(&self) -> bool {
        self.deprecated_config.target_schema.is_some()
    }

    fn schema(&self) -> String {
        // prefer legacy config
        self.deprecated_config
            .target_schema
            .clone()
            .unwrap_or_else(|| self.base().schema.clone())
    }

    fn skip_generate_database_name_macro(&self) -> bool {
        self.deprecated_config.target_database.is_some()
    }

    fn database(&self) -> String {
        // prefer legacy config
        self.deprecated_config
            .target_database
            .clone()
            .unwrap_or_else(|| self.base().database.clone())
    }

    fn tags(&self) -> Vec<String> {
        self.__common_attr__.tags.clone()
    }

    fn meta(&self) -> IndexMap<String, YmlValue> {
        self.__common_attr__.meta.clone()
    }

    fn search_name(&self) -> String {
        self.__common_attr__.name.clone()
    }

    fn selector_string(&self) -> String {
        self.__common_attr__.fqn.join(".")
    }

    fn serialized_config(&self) -> YmlValue {
        let mut value =
            dbt_yaml::to_value(&self.deprecated_config).expect("Failed to serialize to YAML");

        if let YmlValue::Mapping(ref mut map, _) = value {
            let has_meta_column_names =
                matches!(map.get("snapshot_meta_column_names"), Some(v) if !v.is_null());
            if has_meta_column_names {
                let column_names = self
                    .__snapshot_attr__
                    .snapshot_meta_column_names
                    .to_defaulted_column_names();
                map.insert("snapshot_meta_column_names".into(), column_names.clone());
                if !map.contains_key("snapshot_table_column_names") {
                    map.insert("snapshot_table_column_names".into(), column_names);
                }
            }
        }
        value
    }

    fn schema_refresh_interval(&self) -> Option<SchemaRefreshInterval> {
        self.__snapshot_attr__
            .sync
            .as_ref()
            .and_then(|s| s.schema_refresh_interval.clone())
    }
}

impl InternalDbtNode for DbtExposure {
    fn common(&self) -> &CommonAttributes {
        &self.__common_attr__
    }
    fn base(&self) -> &NodeBaseAttributes {
        &self.__base_attr__
    }
    fn base_mut(&mut self) -> &mut NodeBaseAttributes {
        &mut self.__base_attr__
    }
    fn common_mut(&mut self) -> &mut CommonAttributes {
        &mut self.__common_attr__
    }
    fn resource_type(&self) -> NodeType {
        NodeType::Exposure
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn serialize_inner(
        &self,
        mode: crate::schemas::serialization_utils::SerializationMode,
    ) -> YmlValue {
        crate::schemas::serialization_utils::serialize_with_mode(self, mode)
    }
    fn has_same_config(&self, other: &dyn InternalDbtNode, _adapter_type: AdapterType) -> bool {
        if let Some(other_exposure) = other.as_any().downcast_ref::<DbtExposure>() {
            let enabled_eq =
                self.deprecated_config.enabled == other_exposure.deprecated_config.enabled;

            if !enabled_eq {
                log_state_mod_diff(
                    &self.__common_attr__.unique_id,
                    "exposure_config",
                    [(
                        "enabled",
                        enabled_eq,
                        Some((
                            format!("{:?}", &self.deprecated_config.enabled),
                            format!("{:?}", &other_exposure.deprecated_config.enabled),
                        )),
                    )],
                );
            }

            enabled_eq
        } else {
            false
        }
    }
    fn has_same_content(&self, other: &dyn InternalDbtNode, _adapter_type: AdapterType) -> bool {
        if let Some(other_exposure) = other.as_any().downcast_ref::<DbtExposure>() {
            let same_name_result = self.__common_attr__.name == other_exposure.__common_attr__.name;
            let same_fqn_result = self.__common_attr__.fqn == other_exposure.__common_attr__.fqn;
            let same_owner_result =
                self.__exposure_attr__.owner == other_exposure.__exposure_attr__.owner;
            let same_maturity_result =
                self.__exposure_attr__.maturity == other_exposure.__exposure_attr__.maturity;
            let same_url_result =
                self.__exposure_attr__.url == other_exposure.__exposure_attr__.url;
            let same_label_result =
                self.__exposure_attr__.label == other_exposure.__exposure_attr__.label;

            let result = same_name_result
                && same_fqn_result
                && same_owner_result
                && same_maturity_result
                && same_url_result
                && same_label_result;

            if !result {
                log_state_mod_diff(
                    &self.__common_attr__.unique_id,
                    "exposure",
                    [
                        (
                            "same_name",
                            same_name_result,
                            Some((
                                format!("{:?}", &self.__common_attr__.name),
                                format!("{:?}", &other_exposure.__common_attr__.name),
                            )),
                        ),
                        (
                            "same_fqn",
                            same_fqn_result,
                            Some((
                                format!("{:?}", &self.__common_attr__.fqn),
                                format!("{:?}", &other_exposure.__common_attr__.fqn),
                            )),
                        ),
                        (
                            "same_owner",
                            same_owner_result,
                            Some((
                                format!("{:?}", &self.__exposure_attr__.owner),
                                format!("{:?}", &other_exposure.__exposure_attr__.owner),
                            )),
                        ),
                        (
                            "same_maturity",
                            same_maturity_result,
                            Some((
                                format!("{:?}", &self.__exposure_attr__.maturity),
                                format!("{:?}", &other_exposure.__exposure_attr__.maturity),
                            )),
                        ),
                        (
                            "same_url",
                            same_url_result,
                            Some((
                                format!("{:?}", &self.__exposure_attr__.url),
                                format!("{:?}", &other_exposure.__exposure_attr__.url),
                            )),
                        ),
                        (
                            "same_label",
                            same_label_result,
                            Some((
                                format!("{:?}", &self.__exposure_attr__.label),
                                format!("{:?}", &other_exposure.__exposure_attr__.label),
                            )),
                        ),
                    ],
                );
            }

            result
        } else {
            false
        }
    }
    fn set_detected_introspection(&mut self, _introspection: IntrospectionKind) {
        panic!("DbtExposure does not support setting detected_unsafe");
    }
}

impl InternalDbtNode for DbtCheck {
    fn common(&self) -> &CommonAttributes {
        &self.__common_attr__
    }
    fn base(&self) -> &NodeBaseAttributes {
        &self.__base_attr__
    }
    fn base_mut(&mut self) -> &mut NodeBaseAttributes {
        &mut self.__base_attr__
    }
    fn common_mut(&mut self) -> &mut CommonAttributes {
        &mut self.__common_attr__
    }
    fn resource_type(&self) -> NodeType {
        NodeType::Check
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn serialize_inner(
        &self,
        mode: crate::schemas::serialization_utils::SerializationMode,
    ) -> YmlValue {
        crate::schemas::serialization_utils::serialize_with_mode(self, mode)
    }
    /// Rendered-config comparison for `state:modified`. Checks are on "Approach B" — see the
    /// `NodeType::Check` arm in `prev_state::check_configs_modified` for why the Stage-1 unrendered
    /// shortcut is not used.
    fn has_same_config(&self, other: &dyn InternalDbtNode, _adapter_type: AdapterType) -> bool {
        if let Some(other_check) = other.as_any().downcast_ref::<DbtCheck>() {
            let enabled_eq =
                self.deprecated_config.enabled == other_check.deprecated_config.enabled;
            let severity_eq =
                self.deprecated_config.severity == other_check.deprecated_config.severity;
            let selection_filter_on_eq = self.deprecated_config.selection_filter_on
                == other_check.deprecated_config.selection_filter_on;
            // Resolved tags live on common_attr, not the config wrapper (cf. DbtSavedQuery).
            let tags_eq = tags_eq_vec(
                &self.__common_attr__.tags,
                &other_check.__common_attr__.tags,
            );
            // `meta` is compared for the same reason models compare it: a user editing `meta:`
            // expects the node to be selected, and this was the one key where a check behaved
            // differently from a model. Read from common_attr, like tags: that field holds the
            // resolved value and is not an `Option`, so plain equality is enough --
            // `indexmap_yml_value_equal` exists to treat `None` and an empty map as equal, which
            // only arises for the `Option` config wrappers.
            let meta_eq = self.__common_attr__.meta == other_check.__common_attr__.meta;

            let result = enabled_eq && severity_eq && selection_filter_on_eq && tags_eq && meta_eq;

            if !result {
                log_state_mod_diff(
                    &self.__common_attr__.unique_id,
                    "check_config",
                    [
                        (
                            "enabled",
                            enabled_eq,
                            Some((
                                format!("{:?}", &self.deprecated_config.enabled),
                                format!("{:?}", &other_check.deprecated_config.enabled),
                            )),
                        ),
                        (
                            "severity",
                            severity_eq,
                            Some((
                                format!("{:?}", &self.deprecated_config.severity),
                                format!("{:?}", &other_check.deprecated_config.severity),
                            )),
                        ),
                        (
                            "selection_filter_on",
                            selection_filter_on_eq,
                            Some((
                                format!("{:?}", &self.deprecated_config.selection_filter_on),
                                format!("{:?}", &other_check.deprecated_config.selection_filter_on),
                            )),
                        ),
                        (
                            "tags",
                            tags_eq,
                            Some((
                                format!("{:?}", &self.__common_attr__.tags),
                                format!("{:?}", &other_check.__common_attr__.tags),
                            )),
                        ),
                        (
                            "meta",
                            meta_eq,
                            Some((
                                format!("{:?}", &self.__common_attr__.meta),
                                format!("{:?}", &other_check.__common_attr__.meta),
                            )),
                        ),
                    ],
                );
            }

            result
        } else {
            false
        }
    }
    /// Content comparison for `state:modified`.
    ///
    /// Must include the checksum. `has_same_body` is NOT consulted by plain `state:modified`:
    /// `is_modified`'s `Any` arm calls `check_modified_content` (and so this method), while
    /// `check_body_modified` only runs for the explicit `state:modified.body` sub-selector. An
    /// earlier version of this compared only identity and phase, on the assumption that
    /// `has_same_body` covered the SQL — with the result that editing a check's SQL did not mark it
    /// modified and `--select state:modified` reported "Nothing to do". `DbtAnalysis`, the closest
    /// analogue, compares the checksum here for the same reason.
    ///
    /// Phase is also compared: a check whose inferred phase moved is materially different even if
    /// its text is byte-identical, which can happen when the tables it references change.
    fn has_same_content(&self, other: &dyn InternalDbtNode, _adapter_type: AdapterType) -> bool {
        if let Some(other_check) = other.as_any().downcast_ref::<DbtCheck>() {
            let same_checksum_result =
                self.__common_attr__.checksum == other_check.__common_attr__.checksum;
            let same_name_result = self.__common_attr__.name == other_check.__common_attr__.name;
            let same_fqn_result = self.__common_attr__.fqn == other_check.__common_attr__.fqn;

            let result = same_checksum_result && same_name_result && same_fqn_result;

            if !result {
                log_state_mod_diff(
                    &self.__common_attr__.unique_id,
                    "check",
                    [
                        (
                            "same_checksum",
                            same_checksum_result,
                            Some((
                                format!("{:?}", &self.__common_attr__.checksum),
                                format!("{:?}", &other_check.__common_attr__.checksum),
                            )),
                        ),
                        (
                            "same_name",
                            same_name_result,
                            Some((
                                format!("{:?}", &self.__common_attr__.name),
                                format!("{:?}", &other_check.__common_attr__.name),
                            )),
                        ),
                        (
                            "same_fqn",
                            same_fqn_result,
                            Some((
                                format!("{:?}", &self.__common_attr__.fqn),
                                format!("{:?}", &other_check.__common_attr__.fqn),
                            )),
                        ),
                    ],
                );
            }

            result
        } else {
            false
        }
    }
    fn set_detected_introspection(&mut self, _introspection: IntrospectionKind) {
        panic!("DbtCheck does not support setting detected_unsafe");
    }
}

impl InternalDbtNodeAttributes for DbtCheck {
    fn materialized(&self) -> DbtMaterialization {
        self.__base_attr__.materialized.clone()
    }
    fn quoting(&self) -> ResolvedQuoting {
        self.__base_attr__.quoting
    }
    fn tags(&self) -> Vec<String> {
        self.__common_attr__.tags.clone()
    }
    fn meta(&self) -> IndexMap<String, YmlValue> {
        self.__common_attr__.meta.clone()
    }
    fn serialized_config(&self) -> YmlValue {
        dbt_yaml::to_value(&self.deprecated_config).expect("Failed to serialize to YAML")
    }
    fn search_name(&self) -> String {
        self.__common_attr__.name.clone()
    }
    fn selector_string(&self) -> String {
        self.__common_attr__.fqn.join(".")
    }
}

impl InternalDbtNodeAttributes for DbtExposure {
    fn materialized(&self) -> DbtMaterialization {
        self.__base_attr__.materialized.clone()
    }
    fn quoting(&self) -> ResolvedQuoting {
        self.__base_attr__.quoting
    }
    fn tags(&self) -> Vec<String> {
        self.__common_attr__.tags.clone()
    }
    fn meta(&self) -> IndexMap<String, YmlValue> {
        self.__common_attr__.meta.clone()
    }
    fn serialized_config(&self) -> YmlValue {
        dbt_yaml::to_value(&self.deprecated_config).expect("Failed to serialize to YAML")
    }
    fn search_name(&self) -> String {
        self.__common_attr__.name.clone()
    }
    fn selector_string(&self) -> String {
        self.__common_attr__.fqn.join(".")
    }
}

impl InternalDbtNode for DbtSemanticModel {
    fn common(&self) -> &CommonAttributes {
        &self.__common_attr__
    }
    fn base(&self) -> &NodeBaseAttributes {
        &self.__base_attr__
    }
    fn base_mut(&mut self) -> &mut NodeBaseAttributes {
        &mut self.__base_attr__
    }
    fn common_mut(&mut self) -> &mut CommonAttributes {
        &mut self.__common_attr__
    }
    fn resource_type(&self) -> NodeType {
        NodeType::SemanticModel
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn serialize_inner(
        &self,
        mode: crate::schemas::serialization_utils::SerializationMode,
    ) -> YmlValue {
        crate::schemas::serialization_utils::serialize_with_mode(self, mode)
    }
    fn has_same_config(&self, other: &dyn InternalDbtNode, _adapter_type: AdapterType) -> bool {
        if let Some(_other_semantic_model) = other.as_any().downcast_ref::<DbtSemanticModel>() {
            // TODO: implement proper config comparison when needed
            true
        } else {
            false
        }
    }
    // This function only compares a subset of the DbSemanticModel node, similar to what
    // dbt-core does in SemanticModel.same_contents(). See:
    // https://github.com/dbt-labs/dbt-core/blob/906e07c1f2161aaf8873f17ba323221a3cf48c9f/core/dbt/contracts/graph/nodes.py#L1585-L1602
    // TODO: group is not compared while it is in dbt-core. SemanticModel group is not implemented in dbt-fusion.
    fn has_same_content(&self, other: &dyn InternalDbtNode, _adapter_type: AdapterType) -> bool {
        if let Some(other_semantic_model) = other.as_any().downcast_ref::<DbtSemanticModel>() {
            // NOTE: config is owned by `check_configs_modified` (a sibling sub-check OR'd into
            // `is_modified`), so it must not be re-checked here. The wholesale
            // `same_deprecated_config` comparison below still lives in content pending the
            // follow-up that moves it into `has_same_config` (see the plan's deferrals).
            let same_model_result = self.__semantic_model_attr__.model
                == other_semantic_model.__semantic_model_attr__.model;
            let same_description_result = self.__common_attr__.description
                == other_semantic_model.__common_attr__.description;
            let same_entities_result = self.__semantic_model_attr__.entities
                == other_semantic_model.__semantic_model_attr__.entities;
            let same_dimensions_result = self.__semantic_model_attr__.dimensions
                == other_semantic_model.__semantic_model_attr__.dimensions;
            let same_measures_result = self.__semantic_model_attr__.measures
                == other_semantic_model.__semantic_model_attr__.measures;
            let same_deprecated_config_result =
                self.deprecated_config == other_semantic_model.deprecated_config;
            let same_primary_entity_result = self.__semantic_model_attr__.primary_entity
                == other_semantic_model.__semantic_model_attr__.primary_entity;

            let result = same_model_result
                && same_description_result
                && same_entities_result
                && same_dimensions_result
                && same_measures_result
                && same_deprecated_config_result
                && same_primary_entity_result;

            if !result {
                log_state_mod_diff(
                    &self.__common_attr__.unique_id,
                    "semantic_model",
                    [
                        (
                            "same_model",
                            same_model_result,
                            Some((
                                format!("{:?}", &self.__semantic_model_attr__.model),
                                format!(
                                    "{:?}",
                                    &other_semantic_model.__semantic_model_attr__.model
                                ),
                            )),
                        ),
                        (
                            "same_description",
                            same_description_result,
                            Some((
                                format!("{:?}", &self.__common_attr__.description),
                                format!("{:?}", &other_semantic_model.__common_attr__.description),
                            )),
                        ),
                        (
                            "same_entities",
                            same_entities_result,
                            Some((
                                format!("{:?}", &self.__semantic_model_attr__.entities),
                                format!(
                                    "{:?}",
                                    &other_semantic_model.__semantic_model_attr__.entities
                                ),
                            )),
                        ),
                        (
                            "same_dimensions",
                            same_dimensions_result,
                            Some((
                                format!("{:?}", &self.__semantic_model_attr__.dimensions),
                                format!(
                                    "{:?}",
                                    &other_semantic_model.__semantic_model_attr__.dimensions
                                ),
                            )),
                        ),
                        (
                            "same_measures",
                            same_measures_result,
                            Some((
                                format!("{:?}", &self.__semantic_model_attr__.measures),
                                format!(
                                    "{:?}",
                                    &other_semantic_model.__semantic_model_attr__.measures
                                ),
                            )),
                        ),
                        (
                            "same_deprecated_config",
                            same_deprecated_config_result,
                            Some((
                                format!("{:?}", &self.deprecated_config),
                                format!("{:?}", &other_semantic_model.deprecated_config),
                            )),
                        ),
                        (
                            "same_primary_entity",
                            same_primary_entity_result,
                            Some((
                                format!("{:?}", &self.__semantic_model_attr__.primary_entity),
                                format!(
                                    "{:?}",
                                    &other_semantic_model.__semantic_model_attr__.primary_entity
                                ),
                            )),
                        ),
                    ],
                );
            }

            result
        } else {
            false
        }
    }
    fn set_detected_introspection(&mut self, _introspection: IntrospectionKind) {
        panic!("DbtSemanticModel does not support setting detected_unsafe");
    }
}

impl InternalDbtNodeAttributes for DbtSemanticModel {
    fn set_quoting(&mut self, _quoting: ResolvedQuoting) {
        unimplemented!("")
    }
    fn set_static_analysis(&mut self, _static_analysis: Spanned<StaticAnalysisKind>) {}
    fn search_name(&self) -> String {
        self.name()
    }
    fn selector_string(&self) -> String {
        format!(
            "semantic_model:{}.{}",
            self.package_name(),
            self.search_name()
        )
    }
    fn serialized_config(&self) -> YmlValue {
        dbt_yaml::to_value(&self.deprecated_config).expect("Failed to serialize to YAML")
    }
}

impl InternalDbtNode for DbtMetric {
    fn common(&self) -> &CommonAttributes {
        &self.__common_attr__
    }
    fn common_mut(&mut self) -> &mut CommonAttributes {
        &mut self.__common_attr__
    }
    fn base(&self) -> &NodeBaseAttributes {
        &self.__base_attr__
    }
    fn base_mut(&mut self) -> &mut NodeBaseAttributes {
        &mut self.__base_attr__
    }
    fn resource_type(&self) -> NodeType {
        NodeType::Metric
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn serialize_inner(
        &self,
        mode: crate::schemas::serialization_utils::SerializationMode,
    ) -> YmlValue {
        crate::schemas::serialization_utils::serialize_with_mode(self, mode)
    }
    fn has_same_config(&self, other: &dyn InternalDbtNode, _adapter_type: AdapterType) -> bool {
        if let Some(other_metric) = other.as_any().downcast_ref::<DbtMetric>() {
            let self_config = &self.deprecated_config;
            let other_config = &other_metric.deprecated_config;

            let enabled_eq = self_config.enabled == other_config.enabled;
            let group_eq = self_config.group == other_config.group;

            let result = enabled_eq && group_eq;

            if !result {
                log_state_mod_diff(
                    &self.__common_attr__.unique_id,
                    "metric_config",
                    [
                        (
                            "enabled",
                            enabled_eq,
                            Some((
                                format!("{:?}", &self_config.enabled),
                                format!("{:?}", &other_config.enabled),
                            )),
                        ),
                        (
                            "group",
                            group_eq,
                            Some((
                                format!("{:?}", &self_config.group),
                                format!("{:?}", &other_config.group),
                            )),
                        ),
                    ],
                );
            }

            result
        } else {
            false
        }
    }
    // This function only compares a subset of the DbMetric node, mirroring dbt-core's
    // `Metric.same_contents` (`type`, `description`, `label`, `config`). See:
    // https://github.com/dbt-labs/dbt-mantle/blob/0153865/core/dbt/contracts/graph/nodes.py#L1672-L1725
    //
    // dbt-core intentionally does NOT compare `filter`, `metadata`, or `type_params`
    // (its `same_filter`/`same_metadata`/`same_type_params` all `return True` as TODOs), so we
    // drop them here to match — comparing them over-selects on manifest round-trip noise
    // (dbt-core#15513). `fqn` is not part of `same_contents` either (renames are caught by node
    // presence). Revisit the dropped fields if dbt-core implements them.
    fn has_same_content(&self, other: &dyn InternalDbtNode, _adapter_type: AdapterType) -> bool {
        if let Some(other_metric) = other.as_any().downcast_ref::<DbtMetric>() {
            // NOTE: config is owned by `check_configs_modified` (a sibling sub-check OR'd into
            // `is_modified`, which for metrics compares `!has_same_config`), so it is not
            // re-checked here — avoiding the double-count.
            let same_type_result =
                self.__metric_attr__.metric_type == other_metric.__metric_attr__.metric_type;
            let same_description_result =
                self.__common_attr__.description == other_metric.__common_attr__.description;
            let same_label_result =
                self.__metric_attr__.label == other_metric.__metric_attr__.label;

            let result = same_type_result && same_description_result && same_label_result;

            if !result {
                log_state_mod_diff(
                    &self.__common_attr__.unique_id,
                    "metric",
                    [
                        (
                            "same_type",
                            same_type_result,
                            Some((
                                format!("{:?}", &self.__metric_attr__.metric_type),
                                format!("{:?}", &other_metric.__metric_attr__.metric_type),
                            )),
                        ),
                        (
                            "same_description",
                            same_description_result,
                            Some((
                                format!("{:?}", &self.__common_attr__.description),
                                format!("{:?}", &other_metric.__common_attr__.description),
                            )),
                        ),
                        (
                            "same_label",
                            same_label_result,
                            Some((
                                format!("{:?}", &self.__metric_attr__.label),
                                format!("{:?}", &other_metric.__metric_attr__.label),
                            )),
                        ),
                    ],
                );
            }

            result
        } else {
            false
        }
    }
    fn set_detected_introspection(&mut self, _introspection: IntrospectionKind) {
        panic!("DbtMetric does not support setting detected_unsafe");
    }
}

impl InternalDbtNodeAttributes for DbtMetric {
    fn set_quoting(&mut self, _quoting: ResolvedQuoting) {
        unimplemented!("")
    }
    fn set_static_analysis(&mut self, _static_analysis: Spanned<StaticAnalysisKind>) {}
    fn search_name(&self) -> String {
        self.name()
    }
    fn selector_string(&self) -> String {
        format!("metric:{}.{}", self.package_name(), self.search_name())
    }
    fn serialized_config(&self) -> YmlValue {
        dbt_yaml::to_value(&self.deprecated_config).expect("Failed to serialize to YAML")
    }
}

impl InternalDbtNode for DbtSavedQuery {
    fn common(&self) -> &CommonAttributes {
        &self.__common_attr__
    }
    fn base(&self) -> &NodeBaseAttributes {
        &self.__base_attr__
    }
    fn common_mut(&mut self) -> &mut CommonAttributes {
        &mut self.__common_attr__
    }
    fn base_mut(&mut self) -> &mut NodeBaseAttributes {
        &mut self.__base_attr__
    }
    fn resource_type(&self) -> NodeType {
        NodeType::SavedQuery
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn serialize_inner(
        &self,
        mode: crate::schemas::serialization_utils::SerializationMode,
    ) -> YmlValue {
        crate::schemas::serialization_utils::serialize_with_mode(self, mode)
    }
    fn has_same_config(&self, other: &dyn InternalDbtNode, _adapter_type: AdapterType) -> bool {
        if let Some(other_saved_query) = other.as_any().downcast_ref::<DbtSavedQuery>() {
            let self_config = &self.deprecated_config;
            let other_config = &other_saved_query.deprecated_config;

            let cache_eq = self_config.cache == other_config.cache;
            let enabled_eq = self_config.enabled == other_config.enabled;
            let export_as_eq = self_config.export_as == other_config.export_as;
            let schema_eq = self_config.schema == other_config.schema;
            let group_eq = self_config.group == other_config.group;

            let result = cache_eq && enabled_eq && export_as_eq && schema_eq && group_eq;

            if !result {
                log_state_mod_diff(
                    &self.__common_attr__.unique_id,
                    "saved_query_config",
                    [
                        (
                            "cache",
                            cache_eq,
                            Some((
                                format!("{:?}", &self_config.cache),
                                format!("{:?}", &other_config.cache),
                            )),
                        ),
                        (
                            "enabled",
                            enabled_eq,
                            Some((
                                format!("{:?}", &self_config.enabled),
                                format!("{:?}", &other_config.enabled),
                            )),
                        ),
                        (
                            "export_as",
                            export_as_eq,
                            Some((
                                format!("{:?}", &self_config.export_as),
                                format!("{:?}", &other_config.export_as),
                            )),
                        ),
                        (
                            "schema",
                            schema_eq,
                            Some((
                                format!("{:?}", &self_config.schema),
                                format!("{:?}", &other_config.schema),
                            )),
                        ),
                        (
                            "group",
                            group_eq,
                            Some((
                                format!("{:?}", &self_config.group),
                                format!("{:?}", &other_config.group),
                            )),
                        ),
                    ],
                );
            }

            result
        } else {
            false
        }
    }
    fn has_same_content(&self, other: &dyn InternalDbtNode, adapter_type: AdapterType) -> bool {
        if let Some(other_saved_query) = other.as_any().downcast_ref::<DbtSavedQuery>() {
            let same_config_result = self.has_same_config(other, adapter_type);
            let same_description_result =
                self.__common_attr__.description == other_saved_query.__common_attr__.description;
            let same_fqn_result = self.__common_attr__.fqn == other_saved_query.__common_attr__.fqn;
            let same_label_result =
                self.__saved_query_attr__.label == other_saved_query.__saved_query_attr__.label;
            let same_tags_result = tags_eq_vec(
                &self.__common_attr__.tags,
                &other_saved_query.__common_attr__.tags,
            );
            let same_exports_result =
                self.__saved_query_attr__.exports == other_saved_query.__saved_query_attr__.exports;
            let same_group_by_result = self.__saved_query_attr__.query_params.group_by
                == other_saved_query.__saved_query_attr__.query_params.group_by;
            let same_where_result = self.__saved_query_attr__.query_params.where_
                == other_saved_query.__saved_query_attr__.query_params.where_;

            let result = same_config_result
                && same_description_result
                && same_fqn_result
                && same_label_result
                && same_tags_result
                && same_exports_result
                && same_group_by_result
                && same_where_result;

            if !result {
                log_state_mod_diff(
                    &self.__common_attr__.unique_id,
                    "saved_query",
                    [
                        ("same_config", same_config_result, None),
                        (
                            "same_description",
                            same_description_result,
                            Some((
                                format!("{:?}", &self.__common_attr__.description),
                                format!("{:?}", &other_saved_query.__common_attr__.description),
                            )),
                        ),
                        (
                            "same_fqn",
                            same_fqn_result,
                            Some((
                                format!("{:?}", &self.__common_attr__.fqn),
                                format!("{:?}", &other_saved_query.__common_attr__.fqn),
                            )),
                        ),
                        (
                            "same_label",
                            same_label_result,
                            Some((
                                format!("{:?}", &self.__saved_query_attr__.label),
                                format!("{:?}", &other_saved_query.__saved_query_attr__.label),
                            )),
                        ),
                        (
                            "same_tags",
                            same_tags_result,
                            Some((
                                format!("{:?}", &self.__common_attr__.tags),
                                format!("{:?}", &other_saved_query.__common_attr__.tags),
                            )),
                        ),
                        (
                            "same_exports",
                            same_exports_result,
                            Some((
                                format!("{:?}", &self.__saved_query_attr__.exports),
                                format!("{:?}", &other_saved_query.__saved_query_attr__.exports),
                            )),
                        ),
                        (
                            "same_group_by",
                            same_group_by_result,
                            Some((
                                format!("{:?}", &self.__saved_query_attr__.query_params.group_by),
                                format!(
                                    "{:?}",
                                    &other_saved_query.__saved_query_attr__.query_params.group_by
                                ),
                            )),
                        ),
                        (
                            "same_where",
                            same_where_result,
                            Some((
                                format!("{:?}", &self.__saved_query_attr__.query_params.where_),
                                format!(
                                    "{:?}",
                                    &other_saved_query.__saved_query_attr__.query_params.where_
                                ),
                            )),
                        ),
                    ],
                );
            }

            result
        } else {
            false
        }
    }
    fn set_detected_introspection(&mut self, _introspection: IntrospectionKind) {
        panic!("DbtSavedQuery does not support setting detected_unsafe");
    }
}

impl InternalDbtNodeAttributes for DbtSavedQuery {
    fn set_quoting(&mut self, _quoting: ResolvedQuoting) {
        unimplemented!("")
    }
    fn set_static_analysis(&mut self, _static_analysis: Spanned<StaticAnalysisKind>) {}
    fn search_name(&self) -> String {
        self.name()
    }
    fn selector_string(&self) -> String {
        format!("saved_query:{}.{}", self.package_name(), self.search_name())
    }
    fn serialized_config(&self) -> YmlValue {
        dbt_yaml::to_value(&self.deprecated_config).expect("Failed to serialize to YAML")
    }
}

impl InternalDbtNode for DbtFunction {
    fn common(&self) -> &CommonAttributes {
        &self.__common_attr__
    }

    fn base(&self) -> &NodeBaseAttributes {
        &self.__base_attr__
    }

    fn resource_type(&self) -> NodeType {
        NodeType::Function
    }

    fn base_mut(&mut self) -> &mut NodeBaseAttributes {
        &mut self.__base_attr__
    }

    fn common_mut(&mut self) -> &mut CommonAttributes {
        &mut self.__common_attr__
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn serialize_inner(
        &self,
        mode: crate::schemas::serialization_utils::SerializationMode,
    ) -> YmlValue {
        crate::schemas::serialization_utils::serialize_with_mode(self, mode)
    }

    fn has_same_config(&self, other: &dyn InternalDbtNode, _adapter_type: AdapterType) -> bool {
        if let Some(other_function) = other.as_any().downcast_ref::<DbtFunction>() {
            let deprecated_config_eq = self
                .deprecated_config
                .same_config(&other_function.deprecated_config);

            if !deprecated_config_eq {
                log_state_mod_diff(
                    &self.__common_attr__.unique_id,
                    "function_config",
                    [("deprecated_config", deprecated_config_eq, None)],
                );
            }

            deprecated_config_eq
        } else {
            false
        }
    }

    fn has_same_content(&self, other: &dyn InternalDbtNode, _adapter_type: AdapterType) -> bool {
        if let Some(other_function) = other.as_any().downcast_ref::<DbtFunction>() {
            // TODO: checksum-only under-selects relative to dbt-core.
            // `FunctionNode.same_contents` also ANDs `same_arguments`/`same_returns`/`same_overloads`
            // (dbt-mantle core/dbt/contracts/graph/nodes.py:1764-1783), so a signature-only change
            // in schema.yml (arguments/returns/overloads) that leaves the SQL body checksum
            // unchanged is not selected. See the ignored marker test
            // `test_function_signature_change_not_selected_content_gap`.
            // HAZARD when fixing: compare arguments/returns/overloads *structurally* against what
            // dbt-core manifests carry — do not just fold them into the checksum.
            let same_checksum_result =
                self.__common_attr__.checksum == other_function.__common_attr__.checksum;
            let same_overloads = overloads_have_same_content(
                &self.__function_attr__.overloads,
                &other_function.__function_attr__.overloads,
            );

            let result = same_checksum_result && same_overloads;

            if !result {
                log_state_mod_diff(
                    &self.__common_attr__.unique_id,
                    "function",
                    [
                        (
                            "same_checksum",
                            same_checksum_result,
                            Some((
                                format!("{:?}", &self.__common_attr__.checksum),
                                format!("{:?}", &other_function.__common_attr__.checksum),
                            )),
                        ),
                        (
                            "same_overloads",
                            same_overloads,
                            Some((
                                format!("{} overloads", self.__function_attr__.overloads.len()),
                                format!(
                                    "{} overloads",
                                    other_function.__function_attr__.overloads.len()
                                ),
                            )),
                        ),
                    ],
                );
            }

            result
        } else {
            false
        }
    }

    fn set_detected_introspection(&mut self, _introspection: IntrospectionKind) {
        // Functions don't support introspection in the same way as models
        // This could be a no-op or we could add introspection support later
    }

    fn get_group(&self) -> Option<String> {
        self.__function_attr__.group.clone()
    }
}

impl InternalDbtNodeAttributes for DbtFunction {
    fn static_analysis(&self) -> Spanned<StaticAnalysisKind> {
        self.__base_attr__.static_analysis.clone()
    }

    fn set_quoting(&mut self, quoting: ResolvedQuoting) {
        self.__base_attr__.quoting = quoting;
    }

    fn set_static_analysis(&mut self, static_analysis: Spanned<StaticAnalysisKind>) {
        self.__base_attr__.static_analysis = static_analysis;
    }

    fn get_access(&self) -> Option<Access> {
        Some(self.__function_attr__.access.clone())
    }

    fn search_name(&self) -> String {
        self.__common_attr__.name.clone()
    }

    fn selector_string(&self) -> String {
        self.__common_attr__.fqn.join(".")
    }

    fn serialized_config(&self) -> YmlValue {
        dbt_yaml::to_value(&self.deprecated_config).expect("Failed to serialize to YAML")
    }
}

// Only the path methods are overridden below. `common()`/`base()` and the
// `get_node_evaluated_event` default that reads them remain `unimplemented!()`:
// macros live in a separate `Nodes.macros` map and are never iterated as
// `&dyn InternalDbtNode` into the runnable/event APIs, so those paths are unreachable.
impl InternalDbtNode for DbtMacro {
    fn common(&self) -> &CommonAttributes {
        unimplemented!("macro common attributes access")
    }
    fn base(&self) -> &NodeBaseAttributes {
        unimplemented!("macro base attributes access")
    }
    fn base_mut(&mut self) -> &mut NodeBaseAttributes {
        unimplemented!("macro base attributes mutation")
    }
    fn common_mut(&mut self) -> &mut CommonAttributes {
        unimplemented!("macro common attributes mutation")
    }
    fn resource_type(&self) -> NodeType {
        NodeType::Macro
    }
    /// Overridden because the default reads `self.common()`, which panics for
    /// `DbtMacro` (a flat struct with no `CommonAttributes`). Uses `absolute_path`
    /// when available (parse path); falls back to `original_file_path` for
    /// macros loaded from manifest where `absolute_path` is not set (serde-skipped).
    fn get_node_definition_path(
        &self,
        in_dir: &Path,
        _out_dir: &Path,
    ) -> std::borrow::Cow<'_, Path> {
        if self.has_absolute_path() {
            pathdiff::diff_paths(&self.absolute_path, in_dir)
                .map(std::borrow::Cow::Owned)
                .unwrap_or_else(|| self.original_file_path.as_path().into())
        } else {
            self.original_file_path.as_path().into()
        }
    }
    /// Overridden so direct callers don't hit the default's `self.common()`,
    /// which panics for `DbtMacro`. Macros have no compiled/run artifact, so
    /// every kind resolves to the definition path.
    fn get_node_path_abs(
        &self,
        _path_kind: NodePathKind,
        in_dir: &Path,
        _out_dir: &Path,
    ) -> PathBuf {
        if self.has_absolute_path() {
            self.absolute_path.to_path_buf()
        } else {
            in_dir.join(&self.original_file_path)
        }
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn serialize_inner(
        &self,
        mode: crate::schemas::serialization_utils::SerializationMode,
    ) -> YmlValue {
        crate::schemas::serialization_utils::serialize_with_mode(self, mode)
    }
    fn has_same_config(&self, _other: &dyn InternalDbtNode, _adapter_type: AdapterType) -> bool {
        unimplemented!("macro config comparison")
    }
    fn has_same_content(&self, other: &dyn InternalDbtNode, _adapter_type: AdapterType) -> bool {
        if let Some(other_macro) = other.as_any().downcast_ref::<DbtMacro>() {
            let same_macro_sql_result = self.macro_sql == other_macro.macro_sql;

            let result = same_macro_sql_result;

            if !result {
                log_state_mod_diff(
                    &self.unique_id,
                    "macro",
                    [(
                        "same_macro_sql",
                        same_macro_sql_result,
                        Some((
                            format!("{:?}", &self.macro_sql),
                            format!("{:?}", &other_macro.macro_sql),
                        )),
                    )],
                );
            }

            result
        } else {
            false
        }
    }
    fn set_detected_introspection(&mut self, _introspection: IntrospectionKind) {
        panic!("DbtMacro does not support setting detected_unsafe");
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Nodes {
    pub models: BTreeMap<String, Arc<DbtModel>>,
    pub seeds: BTreeMap<String, Arc<DbtSeed>>,
    pub tests: BTreeMap<String, Arc<DbtTest>>,
    pub unit_tests: BTreeMap<String, Arc<DbtUnitTest>>,
    pub sources: BTreeMap<String, Arc<DbtSource>>,
    pub snapshots: BTreeMap<String, Arc<DbtSnapshot>>,
    pub analyses: BTreeMap<String, Arc<DbtAnalysis>>,
    pub exposures: BTreeMap<String, Arc<DbtExposure>>,
    pub semantic_models: BTreeMap<String, Arc<DbtSemanticModel>>,
    pub metrics: BTreeMap<String, Arc<DbtMetric>>,
    pub saved_queries: BTreeMap<String, Arc<DbtSavedQuery>>,
    pub groups: BTreeMap<String, Arc<DbtGroup>>,
    pub functions: BTreeMap<String, Arc<DbtFunction>>,
    pub checks: BTreeMap<String, Arc<DbtCheck>>,
    pub macros: BTreeMap<String, Arc<DbtMacro>>,
    /// The root project name. Used to resolve the `package:this` selector.
    pub project_name: Option<String>,
}

impl Nodes {
    pub fn deep_clone(&self) -> Self {
        // TODO: Instead of cloning node into an Arc, implement
        //       node as an Arc from the outset. Note that this
        //.      has the potential for a huge blast radius,
        //.      hence why we leave it as a TODO for when we
        //.      have the bandwidth to do it.
        //       See: https://github.com/dbt-labs/fs/pull/8760#discussion_r2965956960
        let models = self
            .models
            .iter()
            .map(|(id, node)| (id.clone(), Arc::new((**node).clone())))
            .collect();
        let seeds = self
            .seeds
            .iter()
            .map(|(id, node)| (id.clone(), Arc::new((**node).clone())))
            .collect();
        let tests = self
            .tests
            .iter()
            .map(|(id, node)| (id.clone(), Arc::new((**node).clone())))
            .collect();
        let unit_tests = self
            .unit_tests
            .iter()
            .map(|(id, node)| (id.clone(), Arc::new((**node).clone())))
            .collect();
        let sources = self
            .sources
            .iter()
            .map(|(id, node)| (id.clone(), Arc::new((**node).clone())))
            .collect();
        let snapshots = self
            .snapshots
            .iter()
            .map(|(id, node)| (id.clone(), Arc::new((**node).clone())))
            .collect();
        let analyses = self
            .analyses
            .iter()
            .map(|(id, node)| (id.clone(), Arc::new((**node).clone())))
            .collect();
        let exposures = self
            .exposures
            .iter()
            .map(|(id, node)| (id.clone(), Arc::new((**node).clone())))
            .collect();
        let semantic_models = self
            .semantic_models
            .iter()
            .map(|(id, node)| (id.clone(), Arc::new((**node).clone())))
            .collect();
        let metrics = self
            .metrics
            .iter()
            .map(|(id, node)| (id.clone(), Arc::new((**node).clone())))
            .collect();
        let saved_queries = self
            .saved_queries
            .iter()
            .map(|(id, node)| (id.clone(), Arc::new((**node).clone())))
            .collect();
        let groups = self
            .groups
            .iter()
            .map(|(id, node)| (id.clone(), Arc::new((**node).clone())))
            .collect();
        let functions = self
            .functions
            .iter()
            .map(|(id, node)| (id.clone(), Arc::new((**node).clone())))
            .collect();
        let checks = self
            .checks
            .iter()
            .map(|(id, node)| (id.clone(), Arc::new((**node).clone())))
            .collect();
        let macros = self
            .macros
            .iter()
            .map(|(id, node)| (id.clone(), Arc::new((**node).clone())))
            .collect();
        Nodes {
            models,
            seeds,
            tests,
            unit_tests,
            sources,
            snapshots,
            analyses,
            exposures,
            semantic_models,
            metrics,
            saved_queries,
            groups,
            functions,
            checks,
            macros,
            project_name: self.project_name.clone(),
        }
    }

    /// The default selection set: every node considered when no `--select` is given.
    ///
    /// NOTE: the name is a misnomer — this is not restricted to materializable nodes, and never has
    /// been (exposures, semantic models, metrics and saved queries are all here and none of them
    /// are materialized). Anything omitted here is invisible to a bare `dbt build`, which is why
    /// checks belong in it.
    pub fn materializable_keys(&self) -> impl Iterator<Item = &String> {
        self.models
            .keys()
            .chain(self.seeds.keys())
            .chain(self.tests.keys())
            .chain(self.unit_tests.keys())
            .chain(self.sources.keys())
            .chain(self.snapshots.keys())
            .chain(self.analyses.keys())
            .chain(self.exposures.keys())
            .chain(self.functions.keys())
            .chain(self.checks.keys())
            .chain(self.semantic_models.keys())
            .chain(self.metrics.keys())
            .chain(self.saved_queries.keys())
    }

    pub fn get_node(&self, unique_id: &str) -> Option<&dyn InternalDbtNodeAttributes> {
        self.models
            .get(unique_id)
            .map(|n| Arc::as_ref(n) as &dyn InternalDbtNodeAttributes)
            .or_else(|| {
                self.seeds
                    .get(unique_id)
                    .map(|n| Arc::as_ref(n) as &dyn InternalDbtNodeAttributes)
            })
            .or_else(|| {
                self.tests
                    .get(unique_id)
                    .map(|n| Arc::as_ref(n) as &dyn InternalDbtNodeAttributes)
            })
            .or_else(|| {
                self.unit_tests
                    .get(unique_id)
                    .map(|n| Arc::as_ref(n) as &dyn InternalDbtNodeAttributes)
            })
            .or_else(|| {
                self.sources
                    .get(unique_id)
                    .map(|n| Arc::as_ref(n) as &dyn InternalDbtNodeAttributes)
            })
            .or_else(|| {
                self.snapshots
                    .get(unique_id)
                    .map(|n| Arc::as_ref(n) as &dyn InternalDbtNodeAttributes)
            })
            .or_else(|| {
                self.analyses
                    .get(unique_id)
                    .map(|n| Arc::as_ref(n) as &dyn InternalDbtNodeAttributes)
            })
            .or_else(|| {
                self.exposures
                    .get(unique_id)
                    .map(|n| Arc::as_ref(n) as &dyn InternalDbtNodeAttributes)
            })
            .or_else(|| {
                self.functions
                    .get(unique_id)
                    .map(|n| Arc::as_ref(n) as &dyn InternalDbtNodeAttributes)
            })
            .or_else(|| {
                self.checks
                    .get(unique_id)
                    .map(|n| Arc::as_ref(n) as &dyn InternalDbtNodeAttributes)
            })
            .or_else(|| {
                self.semantic_models
                    .get(unique_id)
                    .map(|n| Arc::as_ref(n) as &dyn InternalDbtNodeAttributes)
            })
            .or_else(|| {
                self.metrics
                    .get(unique_id)
                    .map(|n| Arc::as_ref(n) as &dyn InternalDbtNodeAttributes)
            })
            .or_else(|| {
                self.saved_queries
                    .get(unique_id)
                    .map(|n| Arc::as_ref(n) as &dyn InternalDbtNodeAttributes)
            })
    }

    pub fn get_node_owned(&self, unique_id: &str) -> Option<Arc<dyn InternalDbtNodeAttributes>> {
        self.models
            .get(unique_id)
            .map(|n| n.clone() as Arc<dyn InternalDbtNodeAttributes>)
            .or_else(|| {
                self.seeds
                    .get(unique_id)
                    .map(|n| n.clone() as Arc<dyn InternalDbtNodeAttributes>)
            })
            .or_else(|| {
                self.tests
                    .get(unique_id)
                    .map(|n| n.clone() as Arc<dyn InternalDbtNodeAttributes>)
            })
            .or_else(|| {
                self.unit_tests
                    .get(unique_id)
                    .map(|n| n.clone() as Arc<dyn InternalDbtNodeAttributes>)
            })
            .or_else(|| {
                self.sources
                    .get(unique_id)
                    .map(|n| n.clone() as Arc<dyn InternalDbtNodeAttributes>)
            })
            .or_else(|| {
                self.snapshots
                    .get(unique_id)
                    .map(|n| n.clone() as Arc<dyn InternalDbtNodeAttributes>)
            })
            .or_else(|| {
                self.analyses
                    .get(unique_id)
                    .map(|n| n.clone() as Arc<dyn InternalDbtNodeAttributes>)
            })
            .or_else(|| {
                self.exposures
                    .get(unique_id)
                    .map(|n| n.clone() as Arc<dyn InternalDbtNodeAttributes>)
            })
            .or_else(|| {
                self.functions
                    .get(unique_id)
                    .map(|n| n.clone() as Arc<dyn InternalDbtNodeAttributes>)
            })
            .or_else(|| {
                self.checks
                    .get(unique_id)
                    .map(|n| n.clone() as Arc<dyn InternalDbtNodeAttributes>)
            })
            .or_else(|| {
                self.semantic_models
                    .get(unique_id)
                    .map(|n| n.clone() as Arc<dyn InternalDbtNodeAttributes>)
            })
            .or_else(|| {
                self.metrics
                    .get(unique_id)
                    .map(|n| n.clone() as Arc<dyn InternalDbtNodeAttributes>)
            })
            .or_else(|| {
                self.saved_queries
                    .get(unique_id)
                    .map(|n| n.clone() as Arc<dyn InternalDbtNodeAttributes>)
            })
    }

    /// Check if a node exists in the graph.
    /// Used with [`Nodes::materializable_keys`], so intent could be to only check for materializable nodes.
    /// TODO: Determine if this function should be updated to only check for materializable nodes.
    pub fn contains(&self, unique_id: &str) -> bool {
        self.models.contains_key(unique_id)
            || self.seeds.contains_key(unique_id)
            || self.tests.contains_key(unique_id)
            || self.unit_tests.contains_key(unique_id)
            || self.sources.contains_key(unique_id)
            || self.snapshots.contains_key(unique_id)
            || self.analyses.contains_key(unique_id)
            || self.exposures.contains_key(unique_id)
            || self.semantic_models.contains_key(unique_id)
            || self.metrics.contains_key(unique_id)
            || self.functions.contains_key(unique_id)
            || self.checks.contains_key(unique_id)
            || self.saved_queries.contains_key(unique_id)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &dyn InternalDbtNodeAttributes)> + '_ {
        self.models
            .iter()
            .map(|(id, node)| (id, Arc::as_ref(node) as &dyn InternalDbtNodeAttributes))
            .chain(
                self.seeds
                    .iter()
                    .map(|(id, node)| (id, Arc::as_ref(node) as &dyn InternalDbtNodeAttributes)),
            )
            .chain(
                self.tests
                    .iter()
                    .map(|(id, node)| (id, Arc::as_ref(node) as &dyn InternalDbtNodeAttributes)),
            )
            .chain(
                self.unit_tests
                    .iter()
                    .map(|(id, node)| (id, Arc::as_ref(node) as &dyn InternalDbtNodeAttributes)),
            )
            .chain(
                self.sources
                    .iter()
                    .map(|(id, node)| (id, Arc::as_ref(node) as &dyn InternalDbtNodeAttributes)),
            )
            .chain(
                self.snapshots
                    .iter()
                    .map(|(id, node)| (id, Arc::as_ref(node) as &dyn InternalDbtNodeAttributes)),
            )
            .chain(
                self.analyses
                    .iter()
                    .map(|(id, node)| (id, Arc::as_ref(node) as &dyn InternalDbtNodeAttributes)),
            )
            .chain(
                self.exposures
                    .iter()
                    .map(|(id, node)| (id, Arc::as_ref(node) as &dyn InternalDbtNodeAttributes)),
            )
            .chain(
                self.functions
                    .iter()
                    .map(|(id, node)| (id, Arc::as_ref(node) as &dyn InternalDbtNodeAttributes)),
            )
            .chain(
                self.checks
                    .iter()
                    .map(|(id, node)| (id, Arc::as_ref(node) as &dyn InternalDbtNodeAttributes)),
            )
            .chain(
                self.semantic_models
                    .iter()
                    .map(|(id, node)| (id, Arc::as_ref(node) as &dyn InternalDbtNodeAttributes)),
            )
            .chain(
                self.metrics
                    .iter()
                    .map(|(id, node)| (id, Arc::as_ref(node) as &dyn InternalDbtNodeAttributes)),
            )
            .chain(
                self.saved_queries
                    .iter()
                    .map(|(id, node)| (id, Arc::as_ref(node) as &dyn InternalDbtNodeAttributes)),
            )
    }

    pub fn into_iter(
        &self,
    ) -> impl Iterator<Item = (String, Arc<dyn InternalDbtNodeAttributes>)> + '_ {
        let models = self
            .models
            .iter()
            .map(|(id, node)| (id.clone(), upcast(node.clone())));
        let seeds = self
            .seeds
            .iter()
            .map(|(id, node)| (id.clone(), upcast(node.clone())));
        let tests = self
            .tests
            .iter()
            .map(|(id, node)| (id.clone(), upcast(node.clone())));
        let unit_tests = self
            .unit_tests
            .iter()
            .map(|(id, node)| (id.clone(), upcast(node.clone())));
        let sources = self
            .sources
            .iter()
            .map(|(id, node)| (id.clone(), upcast(node.clone())));
        let snapshots = self
            .snapshots
            .iter()
            .map(|(id, node)| (id.clone(), upcast(node.clone())));
        let analyses = self
            .analyses
            .iter()
            .map(|(id, node)| (id.clone(), upcast(node.clone())));
        let exposures = self
            .exposures
            .iter()
            .map(|(id, node)| (id.clone(), upcast(node.clone())));
        let functions = self
            .functions
            .iter()
            .map(|(id, node)| (id.clone(), upcast(node.clone())));
        let checks = self
            .checks
            .iter()
            .map(|(id, node)| (id.clone(), upcast(node.clone())));
        let semantic_models = self
            .semantic_models
            .iter()
            .map(|(id, node)| (id.clone(), upcast(node.clone())));
        let metrics = self
            .metrics
            .iter()
            .map(|(id, node)| (id.clone(), upcast(node.clone())));
        let saved_queries = self
            .saved_queries
            .iter()
            .map(|(id, node)| (id.clone(), upcast(node.clone())));

        models
            .chain(seeds)
            .chain(tests)
            .chain(unit_tests)
            .chain(sources)
            .chain(snapshots)
            .chain(analyses)
            .chain(exposures)
            .chain(functions)
            .chain(checks)
            .chain(semantic_models)
            .chain(metrics)
            .chain(saved_queries)
    }

    pub fn iter_values_mut(
        &mut self,
    ) -> impl Iterator<Item = &mut dyn InternalDbtNodeAttributes> + '_ {
        let map_models = self
            .models
            .values_mut()
            .map(|arc| Arc::make_mut(arc) as &mut dyn InternalDbtNodeAttributes);
        let map_seeds = self
            .seeds
            .values_mut()
            .map(|arc| Arc::make_mut(arc) as &mut dyn InternalDbtNodeAttributes);
        let map_tests = self
            .tests
            .values_mut()
            .map(|arc| Arc::make_mut(arc) as &mut dyn InternalDbtNodeAttributes);
        let map_unit_tests = self
            .unit_tests
            .values_mut()
            .map(|arc| Arc::make_mut(arc) as &mut dyn InternalDbtNodeAttributes);
        let map_sources = self
            .sources
            .values_mut()
            .map(|arc| Arc::make_mut(arc) as &mut dyn InternalDbtNodeAttributes);
        let map_snapshots = self
            .snapshots
            .values_mut()
            .map(|arc| Arc::make_mut(arc) as &mut dyn InternalDbtNodeAttributes);
        let map_analyses = self
            .analyses
            .values_mut()
            .map(|arc| Arc::make_mut(arc) as &mut dyn InternalDbtNodeAttributes);
        let map_exposures = self
            .exposures
            .values_mut()
            .map(|arc| Arc::make_mut(arc) as &mut dyn InternalDbtNodeAttributes);
        let map_functions = self
            .functions
            .values_mut()
            .map(|arc| Arc::make_mut(arc) as &mut dyn InternalDbtNodeAttributes);
        let map_checks = self
            .checks
            .values_mut()
            .map(|arc| Arc::make_mut(arc) as &mut dyn InternalDbtNodeAttributes);
        let map_semantic_models = self
            .semantic_models
            .values_mut()
            .map(|arc| Arc::make_mut(arc) as &mut dyn InternalDbtNodeAttributes);
        let map_metrics = self
            .metrics
            .values_mut()
            .map(|arc| Arc::make_mut(arc) as &mut dyn InternalDbtNodeAttributes);
        let map_saved_queries = self
            .saved_queries
            .values_mut()
            .map(|arc| Arc::make_mut(arc) as &mut dyn InternalDbtNodeAttributes);

        map_models
            .chain(map_seeds)
            .chain(map_tests)
            .chain(map_unit_tests)
            .chain(map_sources)
            .chain(map_snapshots)
            .chain(map_analyses)
            .chain(map_exposures)
            .chain(map_functions)
            .chain(map_checks)
            .chain(map_semantic_models)
            .chain(map_metrics)
            .chain(map_saved_queries)
    }

    pub fn get_value_mut(&mut self, unique_id: &str) -> Option<&mut dyn InternalDbtNodeAttributes> {
        self.models
            .get_mut(unique_id)
            .map(|arc| Arc::make_mut(arc) as &mut dyn InternalDbtNodeAttributes)
            .or_else(|| {
                self.seeds
                    .get_mut(unique_id)
                    .map(|arc| Arc::make_mut(arc) as &mut dyn InternalDbtNodeAttributes)
            })
            .or_else(|| {
                self.tests
                    .get_mut(unique_id)
                    .map(|arc| Arc::make_mut(arc) as &mut dyn InternalDbtNodeAttributes)
            })
            .or_else(|| {
                self.unit_tests
                    .get_mut(unique_id)
                    .map(|arc| Arc::make_mut(arc) as &mut dyn InternalDbtNodeAttributes)
            })
            .or_else(|| {
                self.sources
                    .get_mut(unique_id)
                    .map(|arc| Arc::make_mut(arc) as &mut dyn InternalDbtNodeAttributes)
            })
            .or_else(|| {
                self.snapshots
                    .get_mut(unique_id)
                    .map(|arc| Arc::make_mut(arc) as &mut dyn InternalDbtNodeAttributes)
            })
            .or_else(|| {
                self.analyses
                    .get_mut(unique_id)
                    .map(|arc| Arc::make_mut(arc) as &mut dyn InternalDbtNodeAttributes)
            })
            .or_else(|| {
                self.exposures
                    .get_mut(unique_id)
                    .map(|arc| Arc::make_mut(arc) as &mut dyn InternalDbtNodeAttributes)
            })
            .or_else(|| {
                self.functions
                    .get_mut(unique_id)
                    .map(|arc| Arc::make_mut(arc) as &mut dyn InternalDbtNodeAttributes)
            })
            .or_else(|| {
                self.checks
                    .get_mut(unique_id)
                    .map(|arc| Arc::make_mut(arc) as &mut dyn InternalDbtNodeAttributes)
            })
            .or_else(|| {
                self.semantic_models
                    .get_mut(unique_id)
                    .map(|arc| Arc::make_mut(arc) as &mut dyn InternalDbtNodeAttributes)
            })
            .or_else(|| {
                self.metrics
                    .get_mut(unique_id)
                    .map(|arc| Arc::make_mut(arc) as &mut dyn InternalDbtNodeAttributes)
            })
            .or_else(|| {
                self.saved_queries
                    .get_mut(unique_id)
                    .map(|arc| Arc::make_mut(arc) as &mut dyn InternalDbtNodeAttributes)
            })
    }

    pub fn get_by_relation_name(
        &self,
        relation_name: &str,
    ) -> Option<&dyn InternalDbtNodeAttributes> {
        self.models
            .values()
            .find(|model| model.base().relation_name == Some(relation_name.to_string()))
            .map(|arc| Arc::as_ref(arc) as &dyn InternalDbtNodeAttributes)
            .or_else(|| {
                self.seeds
                    .values()
                    .find(|seed| seed.base().relation_name == Some(relation_name.to_string()))
                    .map(|arc| Arc::as_ref(arc) as &dyn InternalDbtNodeAttributes)
            })
            .or_else(|| {
                self.tests
                    .values()
                    // test.base().relation_name is always None
                    .find(|test| test.relation_name() == relation_name)
                    .map(|arc| Arc::as_ref(arc) as &dyn InternalDbtNodeAttributes)
            })
            .or_else(|| {
                self.unit_tests
                    .values()
                    .find(|unit_test| unit_test.relation_name() == relation_name)
                    .map(|arc| Arc::as_ref(arc) as &dyn InternalDbtNodeAttributes)
            })
            .or_else(|| {
                self.sources
                    .values()
                    .find(|source| source.base().relation_name == Some(relation_name.to_string()))
                    .map(|arc| Arc::as_ref(arc) as &dyn InternalDbtNodeAttributes)
            })
            .or_else(|| {
                self.snapshots
                    .values()
                    .find(|snapshot| {
                        snapshot.base().relation_name == Some(relation_name.to_string())
                    })
                    .map(|arc| Arc::as_ref(arc) as &dyn InternalDbtNodeAttributes)
            })
            .or_else(|| {
                self.analyses
                    .values()
                    .find(|analysis| {
                        analysis.base().relation_name == Some(relation_name.to_string())
                    })
                    .map(|arc| Arc::as_ref(arc) as &dyn InternalDbtNodeAttributes)
            })
            .or_else(|| {
                self.exposures
                    .values()
                    .find(|exposure| {
                        exposure.base().relation_name == Some(relation_name.to_string())
                    })
                    .map(|arc| Arc::as_ref(arc) as &dyn InternalDbtNodeAttributes)
            })
            .or_else(|| {
                self.functions
                    .values()
                    .find(|function| {
                        function.base().relation_name == Some(relation_name.to_string())
                    })
                    .map(|arc| Arc::as_ref(arc) as &dyn InternalDbtNodeAttributes)
            })
            .or_else(|| {
                self.semantic_models
                    .values()
                    .find(|semantic_model| {
                        semantic_model.base().relation_name == Some(relation_name.to_string())
                    })
                    .map(|arc| Arc::as_ref(arc) as &dyn InternalDbtNodeAttributes)
            })
            .or_else(|| {
                self.metrics
                    .values()
                    .find(|metric| metric.base().relation_name == Some(relation_name.to_string()))
                    .map(|arc| Arc::as_ref(arc) as &dyn InternalDbtNodeAttributes)
            })
            .or_else(|| {
                self.saved_queries
                    .values()
                    .find(|saved_query| {
                        saved_query.base().relation_name == Some(relation_name.to_string())
                    })
                    .map(|arc| Arc::as_ref(arc) as &dyn InternalDbtNodeAttributes)
            })
    }

    pub fn extend(&mut self, other: Nodes) {
        self.models.extend(other.models);
        self.seeds.extend(other.seeds);
        self.tests.extend(other.tests);
        self.unit_tests.extend(other.unit_tests);
        self.sources.extend(other.sources);
        self.snapshots.extend(other.snapshots);
        self.analyses.extend(other.analyses);
        self.exposures.extend(other.exposures);
        self.semantic_models.extend(other.semantic_models);
        self.metrics.extend(other.metrics);
        self.saved_queries.extend(other.saved_queries);
        self.groups.extend(other.groups);
        self.functions.extend(other.functions);
        self.checks.extend(other.checks);
    }

    pub fn warn_on_custom_materializations(&self) -> FsResult<()> {
        let mut custom_materializations: Vec<(String, String)> = Vec::new();

        for (_, node) in self.iter() {
            if let DbtMaterialization::Unknown(custom) = node.materialized() {
                custom_materializations.push((node.common().unique_id.clone(), custom));
            }
        }

        if !custom_materializations.is_empty() {
            let mut message = "Custom materialization macros are not supported. Found custom materializations in the following nodes:\n".to_string();
            for (unique_id, materialization) in &custom_materializations {
                message.push_str(&format!(
                    "  - {unique_id} (materialization: {materialization})\n"
                ));
            }

            return err!(ErrorCode::UnsupportedFeature, "{}", message);
        }
        Ok(())
    }

    pub fn warn_on_microbatch(&self, adapter_type: AdapterType) -> FsResult<()> {
        let models_forcing_concurrent_batches: usize = self
            .iter()
            .map(|(_, node)| node.warn_on_microbatch(adapter_type))
            .sum();

        if models_forcing_concurrent_batches > 0 {
            let maybe_plural_count_of_models = if models_forcing_concurrent_batches == 1 {
                "1 microbatch model".to_string()
            } else {
                format!("{models_forcing_concurrent_batches} microbatch models")
            };

            return err!(
                ErrorCode::InvalidConcurrentBatchesConfig,
                "Found {maybe_plural_count_of_models} with the `concurrent_batches` config set to true, but the {} adapter does not support running batches concurrently. Batches will be run sequentially.",
                adapter_type
            );
        }

        Ok(())
    }
}

fn upcast<T: InternalDbtNodeAttributes + 'static>(
    arc: Arc<T>,
) -> Arc<dyn InternalDbtNodeAttributes> {
    arc
}

/// Serialize Option<String> as empty string when None, otherwise as the string value.
/// This ensures the field is always present in serialized output.
pub fn serialize_none_as_empty_string<S>(
    value: &Option<String>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    match value {
        Some(s) => serializer.serialize_str(s),
        None => serializer.serialize_str(""),
    }
}

/// Serialize Option<StringOrArrayOfStrings> (or a newtype wrapping it, e.g. `Tags`/`Classifiers`)
/// as empty array when None, otherwise as an array. This ensures the field is always present as a
/// list in serialized output, which is required for Jinja macros that call
/// `obj.config.tags.extend(...)` or similar list operations.
/// See: https://github.com/dbt-labs/dbt-fusion/issues/1198
pub fn serialize_none_as_empty_list<S, T>(value: &T, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
    T: super::serde::AsStringOrArrayOfStrings,
{
    use super::serde::StringOrArrayOfStrings::{ArrayOfStrings, String as SingleString};
    use serde::ser::SerializeSeq;

    match value.as_string_or_array_of_strings() {
        Some(ArrayOfStrings(vec)) => vec.serialize(serializer),
        Some(SingleString(s)) => {
            let mut seq = serializer.serialize_seq(Some(1))?;
            seq.serialize_element(s)?;
            seq.end()
        }
        None => {
            let seq = serializer.serialize_seq(Some(0))?;
            seq.end()
        }
    }
}

/// Deserialize Option<String>, treating empty string as None for consistency.
pub fn deserialize_empty_string_as_none<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s: Option<String> = Option::deserialize(deserializer)?;
    Ok(match s {
        Some(s) if s.is_empty() => None,
        other => other,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct CommonAttributes {
    // Identifiers
    pub unique_id: String,
    pub name: String,
    pub package_name: String,
    pub fqn: Vec<String>,

    // Paths
    pub path: DbtPath,

    /// The original file path where this node was defined
    ///
    /// **NOTE**: For [DbtTest] nodes, this is currently the path to the
    /// generated SQL file, *NOT* the path to the YAML file where the test was
    /// defined!
    pub original_file_path: DbtPath,

    #[serde(
        default,
        serialize_with = "serialize_none_as_empty_string",
        deserialize_with = "deserialize_empty_string_as_none"
    )]
    pub raw_code: Option<String>,
    pub patch_path: Option<DbtPath>,
    pub name_span: Span,

    // Checksum
    pub checksum: DbtChecksum,
    pub language: Option<String>,

    // Meta
    pub description: Option<String>,

    // Tags, Classifiers, and Meta
    pub tags: Vec<String>,
    // `classifiers` was added after many node recordings/manifests were
    // captured; default to empty so deserializing older payloads (e.g. replay
    // fixtures, prior manifests) that lack the field does not fail.
    #[serde(default)]
    pub classifiers: Vec<String>,
    pub meta: IndexMap<String, YmlValue>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct NodeBaseAttributes {
    // Identifiers
    #[serde(default)]
    pub database: String,
    pub schema: String,
    pub alias: String,
    pub relation_name: Option<String>,

    // Resolved Quoting
    pub quoting: ResolvedQuoting,
    // TODO: Potentially add ignore_case to ResolvedQuoting
    pub quoting_ignore_case: bool,
    pub materialized: DbtMaterialization,
    /// The adapter this node runs on: its `+adapter` selection if it made one,
    /// inherited from its subject if it is a test, and otherwise the target's
    /// default. Resolved at parse like `quoting` and `materialized`, so nothing
    /// downstream has to know the target default to answer "where does this run".
    ///
    /// Not part of `ManifestNodeBaseAttributes`, so it does not reach the manifest;
    /// it is serialized only into the partial-parse cache, which a `dbt_version`
    /// change invalidates.
    pub adapter: AdapterType,
    /// The compute platforms this node's output is pushed to after it
    /// materializes, from `+propagate`. Empty unless the node asked for it.
    ///
    /// Distinct from [`Self::adapter`], which says where the node *runs*: a node
    /// runs in exactly one place but can be published to several. Carried here
    /// for the same reason `adapter` is -- the run layer has no profile and no
    /// config, and reads placement off the node.
    ///
    /// Skipped when empty, unlike `adapter`. `adapter` always carries a value
    /// worth recording; `propagate` is empty for almost every node in almost
    /// every project, so serializing `[]` unconditionally would add a field to
    /// every node in the partial-parse cache and in every dev-trace payload
    /// that serializes a node, to say nothing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub propagate: Vec<AdapterType>,
    pub static_analysis: Spanned<StaticAnalysisKind>,
    #[serde(skip_deserializing, default)]
    pub static_analysis_off_reason: Option<StaticAnalysisOffReason>,
    #[serde(default)]
    pub compute: Option<ComputeArg>,
    pub enabled: bool,
    #[serde(skip_serializing, default = "crate::schemas::common::default_false")]
    pub extended_model: bool,

    // Documentation persistence configuration
    pub persist_docs: Option<PersistDocsConfig>,

    // Derived
    #[serde(
        default,
        serialize_with = "serialize_dbt_columns",
        deserialize_with = "deserialize_dbt_columns"
    )]
    pub columns: Vec<DbtColumnRef>,

    // Raw Refs, Source, and Metric Dependencies from SQL
    #[serde(default)]
    pub refs: Vec<DbtRef>,
    #[serde(default)]
    pub sources: Vec<DbtSourceWrapper>,
    #[serde(
        default,
        serialize_with = "serialize_dbt_function_refs",
        deserialize_with = "deserialize_dbt_function_refs"
    )]
    pub functions: Vec<DbtRef>,
    #[serde(default)]
    pub metrics: Vec<Vec<String>>,

    // Resolved Dependencies
    pub depends_on: NodeDependsOn,

    /// Unrendered (configured) values used for dbt-core compatible `state:*` comparisons.
    ///
    /// In dbt-core manifests, this is emitted as `__base_attr__.unrendered_config` and is used
    /// by `state:modified` to ignore target-derived rendering differences for database/schema/alias.
    #[serde(default)]
    pub unrendered_config: BTreeMap<String, YmlValue>,
}

/// Hand-written rather than derived because [`AdapterType`] has no `Default`, and
/// deliberately so: this PR removed the implicit adapter defaults that let a caller
/// resolve against the wrong warehouse by omission. Picking one here is a
/// construction convenience for tests and nothing more -- every node built by the
/// resolvers has its adapter set explicitly. Adding a field to the struct breaks
/// this impl, which is the point: the new field needs a deliberate default too.
impl Default for NodeBaseAttributes {
    fn default() -> Self {
        Self {
            database: String::default(),
            schema: String::default(),
            alias: String::default(),
            relation_name: None,
            quoting: ResolvedQuoting::default(),
            quoting_ignore_case: false,
            materialized: DbtMaterialization::default(),
            adapter: AdapterType::Snowflake,
            propagate: Vec::new(),
            static_analysis: Spanned::default(),
            static_analysis_off_reason: None,
            compute: None,
            enabled: false,
            extended_model: false,
            persist_docs: None,
            columns: Vec::new(),
            refs: Vec::new(),
            sources: Vec::new(),
            functions: Vec::new(),
            metrics: Vec::new(),
            depends_on: NodeDependsOn::default(),
            unrendered_config: BTreeMap::default(),
        }
    }
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct DbtSeed {
    pub __common_attr__: CommonAttributes,

    pub __base_attr__: NodeBaseAttributes,

    pub __seed_attr__: DbtSeedAttr,

    // To be deprecated
    #[serde(rename = "config")]
    pub deprecated_config: SeedConfig,

    pub __other__: BTreeMap<String, YmlValue>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct DbtSeedAttr {
    #[serde(default, skip_serializing_if = "is_false")]
    pub quote_columns: bool,
    pub column_types: Option<BTreeMap<Spanned<String>, String>>,
    pub delimiter: Option<String>,
    pub root_path: Option<PathBuf>,
    pub catalog_name: Option<String>,
}

fn is_false(b: &bool) -> bool {
    !b
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct DbtExposure {
    pub __common_attr__: CommonAttributes,

    pub __base_attr__: NodeBaseAttributes,

    pub __exposure_attr__: DbtExposureAttr,

    // To be deprecated
    #[serde(rename = "config")]
    pub deprecated_config: ExposureConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq, DbtSchema)]
#[serde(rename_all = "lowercase")]
pub enum ExposureType {
    #[default]
    Dashboard,
    Notebook,
    Analysis,
    #[serde(rename = "ml")]
    ML,
    Application,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct DbtExposureAttr {
    pub owner: DbtOwner,
    pub label: Option<String>,
    pub maturity: Option<String>,
    #[serde(rename = "type")]
    pub type_: ExposureType,
    pub url: Option<String>,
    pub unrendered_config: BTreeMap<String, YmlValue>,
    #[serde(default)]
    pub created_at: f64,
}

/// A project quality check: user-authored SQL over the dbt metadata index.
///
/// A check is a node but not a relation — it is never materialized and has no `ref()`s, so its
/// `__base_attr__` carries no meaningful database/schema/alias and its `depends_on` stays empty.
#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct DbtCheck {
    pub __common_attr__: CommonAttributes,

    pub __base_attr__: NodeBaseAttributes,

    pub __check_attr__: DbtCheckAttr,

    // To be deprecated
    #[serde(rename = "config")]
    pub deprecated_config: CheckConfig,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct DbtCheckAttr {
    /// The check's Jinja-rendered SQL, carried from parse to execution.
    ///
    /// A check is fully rendered during parse — that is what phase inference reads — so it has no
    /// render task to produce compiled SQL later. Keeping the rendered text on the node means the
    /// query executed is byte-identical to the one the phase was inferred from.
    ///
    /// Serialized: the incremental parse cache (`target/private/metadata/parse`) reconstructs check nodes
    /// from this representation and then *executes* them, so dropping the rendered text here makes
    /// every warm run report "no rendered SQL". (It does duplicate `raw_code` for checks that
    /// contain no Jinja, which is the accepted cost of the round-trip.)
    pub compiled_sql: Option<String>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct DbtGroup {
    pub __common_attr__: CommonAttributes,
    pub __base_attr__: NodeBaseAttributes,
    pub __group_attr__: DbtGroupAttr,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct DbtGroupAttr {
    pub owner: DbtOwner,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct DbtUnitTest {
    pub __common_attr__: CommonAttributes,

    pub __base_attr__: NodeBaseAttributes,

    pub __unit_test_attr__: DbtUnitTestAttr,

    #[serde(rename = "_event_status")]
    pub field_event_status: Option<BTreeMap<String, YmlValue>>,
    #[serde(rename = "_pre_injected_sql")]
    pub field_pre_injected_sql: Option<String>,
    pub tested_node_unique_id: Option<String>,
    pub this_input_node_unique_id: Option<String>,

    /// YAML span of the unit test's `name:` declaration. Used by the analyzer
    /// to anchor error locations at the unit-test entry in the source YAML.
    /// `None` on warm-start parquet reload (not yet persisted across sessions).
    pub defined_at: Option<dbt_common::CodeLocationWithFile>,

    // To be deprecated
    #[serde(rename = "config")]
    pub deprecated_config: UnitTestConfig,
}

impl DbtUnitTest {
    pub fn relation_name(&self) -> String {
        format!(
            "{}.{}.{}",
            self.__base_attr__.database, self.__base_attr__.schema, self.__base_attr__.alias
        )
    }
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub struct DbtUnitTestAttr {
    pub model: String,
    /// The adapter of the model under test, so the unit test runs where its
    /// subject runs. Inherited only -- a unit test has no `+adapter` of its own.
    pub given: Vec<Given>,
    pub expect: Expect,
    pub versions: Option<IncludeExclude>,
    pub version: Option<StringOrInteger>,
    pub overrides: Option<UnitTestOverrides>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct DbtTest {
    pub __common_attr__: CommonAttributes,
    pub __base_attr__: NodeBaseAttributes,
    pub __test_attr__: DbtTestAttr,
    pub __adapter_attr__: AdapterAttr,
    pub defined_at: Option<dbt_common::CodeLocationWithFile>,

    // not to be confused with __common_attr__.original_file_path, which is the path to the generated sql file
    pub manifest_original_file_path: DbtPath,

    // To be deprecated
    #[serde(rename = "config")]
    pub deprecated_config: DataTestConfig,

    pub __other__: BTreeMap<String, YmlValue>,
}

/// Keys in a data test that count as a change for `--select state:modified`.
/// Based on dbt-core's `TestConfig.same_contents`, which fully overrides `BaseConfig.same_contents`
/// used for other node types.
/// This list is used directly in the Stage-1 check of unrendered configs in `check_configs_modified`
/// and is coordinated with the Stage-2 check of rendered configs in `DbtTest::has_same_config`.
pub(crate) const DBTTEST_CONFIG_MODIFIERS: &[&str] = &[
    "severity",
    "where",
    "limit",
    "fail_calc",
    "warn_if",
    "error_if",
    "store_failures",
    "store_failures_as",
    "sql_header",
    // The data-test `state` config is deliberately NOT a modifier: dbt-core's
    // `TestConfig.same_contents` is a fixed allowlist that does not compare it, so treating it as a
    // modifier would over-select against a dbt-core-produced state manifest. If dbt-core ever starts
    // comparing it, add it here and in `DbtTest::has_same_config` together (see the ignored
    // `data_test_state_not_promoted_to_modifier_conformance_marker`).
];

impl DbtTest {
    pub fn relation_name(&self) -> String {
        format!(
            "{}.{}.{}",
            self.__base_attr__.database, self.__base_attr__.schema, self.__base_attr__.alias
        )
    }
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct DbtTestAttr {
    /// The column this test is attached to, from its position in the schema file. `None`
    /// for a model-level test; a `column_name` macro kwarg lives in `test_metadata.kwargs`.
    pub column_name: Option<String>,
    pub attached_node: Option<String>,
    pub test_metadata: Option<TestMetadata>,
    pub file_key_name: Option<String>,
    /// dbt State configs (restricted subset), read by the run-cache at run time.
    pub state: Option<crate::schemas::properties::DataTestState>,
    #[serde(skip_serializing, default = "default_introspection")]
    pub introspection: IntrospectionKind,
    /// The original (untruncated) test name, if truncation occurred.
    /// When test names exceed 63 characters, dbt truncates to `<first 30 chars>_<md5 hash>`.
    /// This field stores the original name for selector matching purposes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_name: Option<String>,
    #[serde(skip)]
    pub group: Option<String>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct TestMetadata {
    pub name: String,
    #[serde(default)]
    pub kwargs: BTreeMap<String, YmlValue>,
    pub namespace: Option<String>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct DbtSnapshot {
    pub __common_attr__: CommonAttributes,
    pub __base_attr__: NodeBaseAttributes,
    pub __snapshot_attr__: DbtSnapshotAttr,
    pub __adapter_attr__: AdapterAttr,

    /// To be deprecated
    #[serde(rename = "config")]
    pub deprecated_config: SnapshotConfig,
    // TODO: Deprecate compiled and compiled_code fields (This field is used by the materialization
    // macro when this node is passed into the jinja context)
    pub compiled: Option<bool>,
    pub compiled_code: Option<String>,

    pub __other__: BTreeMap<String, YmlValue>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct DbtSnapshotAttr {
    pub snapshot_meta_column_names: SnapshotMetaColumnNames,
    #[serde(skip_serializing, default = "default_introspection")]
    pub introspection: IntrospectionKind,
    pub sync: Option<SyncConfig>,
    /// dbt State configs, read by the run-cache at run time.
    pub state: Option<crate::schemas::properties::ModelState>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct DbtSource {
    pub __common_attr__: CommonAttributes,

    pub __base_attr__: NodeBaseAttributes,

    // Source Specific Attributes
    pub __source_attr__: DbtSourceAttr,

    // To be deprecated
    #[serde(rename = "config")]
    pub deprecated_config: SourceConfig,

    // Other untyped (or rather externally typed) keys that can be used by dbt packages.
    // For example, the `external` key is used by `dbt-external-tables`, but it's not
    // explicitly typed by dbt itself.
    // See: https://github.com/dbt-labs/dbt-external-tables
    pub __other__: BTreeMap<String, YmlValue>,
}
impl DbtSource {
    pub fn base(&self) -> &NodeBaseAttributes {
        &self.__base_attr__
    }
    pub fn common(&self) -> &CommonAttributes {
        &self.__common_attr__
    }
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct DbtSourceAttr {
    pub identifier: String,
    pub source_name: String,
    pub source_description: String,
    pub loader: String,
    pub loaded_at_field: Option<String>,
    pub loaded_at_query: Option<String>,
    /// User-supplied quoting (source-level merged with table-level), with no
    /// project/adapter defaults applied. Mirrors `source.quoting.merged(table.quoting)`
    /// in dbt-core and is what gets serialized to the manifest. The resolved
    /// quoting used for SQL generation lives on `NodeBaseAttributes.quoting`.
    pub user_quoting: Option<crate::schemas::common::DbtQuoting>,
    #[serialize_always]
    pub freshness: Option<FreshnessDefinition>,
    /// Specifies where the schema metadata originates: 'remote' (default) or 'local'
    #[serde(default)]
    pub schema_origin: SchemaOrigin,
    pub sync: Option<SyncConfig>,
    /// Reference: https://github.com/dbt-labs/dbt-mantle/blob/da5abca4f829b167bd1b1d5c6666c12cd8c719c0/core/dbt/artifacts/resources/v1/source_definition.py#L85-L86
    pub unrendered_database: Option<String>,
    pub unrendered_schema: Option<String>,
    /// Reference: https://github.com/dbt-labs/dbt-mantle/blob/da5abca4f829b167bd1b1d5c6666c12cd8c719c0/core/dbt/artifacts/resources/v1/source_definition.py#L74-L75
    pub external: Option<ExternalTable>,
    /// Names a `catalogs.yml` entry this source should be read through
    /// (e.g. an AWS Glue catalog for dbt Compute). When set, it replaces
    /// `database` as the leading identifier in the source's compiled
    /// relation, so `database` can stay a purely descriptive label instead
    /// of doubling as the catalog's alias. Same config as the existing
    /// model/seed `catalog_name`, just not previously wired up for reads.
    pub catalog_name: Option<String>,
}

impl DbtSource {
    /// `None ≈ Some("")` normalized string equality, used for the freshness string attributes
    /// (`loaded_at_field`, `loaded_at_query`): old manifests may serialize an unset value as
    /// either `null` or `""`.
    fn loaded_at_str_eq(a: &Option<String>, b: &Option<String>) -> bool {
        match (a, b) {
            (None, None) => true,
            (None, Some(v)) | (Some(v), None) => v.is_empty(),
            (Some(a), Some(b)) => a == b,
        }
    }

    /// Rendered equality of the `freshness` attribute alone, treating `None` as equivalent to a
    /// `FreshnessDefinition` whose rules are all empty (old manifests serialized either form).
    fn same_freshness_value(&self, other: &DbtSource) -> bool {
        use crate::schemas::common::FreshnessDefinition;

        // A FreshnessDefinition is "empty" when it carries no rules (all fields None).
        let is_empty_freshness = |f: &FreshnessDefinition| -> bool {
            match (&f.error_after, &f.warn_after, &f.filter) {
                (Some(error), Some(warn), None) => {
                    error.count.is_none()
                        && error.period.is_none()
                        && warn.count.is_none()
                        && warn.period.is_none()
                }
                (None, None, None) => true,
                _ => false,
            }
        };

        match (
            &self.__source_attr__.freshness,
            &other.__source_attr__.freshness,
        ) {
            (None, None) => true,
            (None, Some(b_val)) => is_empty_freshness(b_val),
            (Some(a_val), None) => is_empty_freshness(a_val),
            (Some(a_val), Some(b_val)) => {
                let error_after_eq = match (&a_val.error_after, &b_val.error_after) {
                    (None, None) => true,
                    (None, Some(v)) | (Some(v), None) => v.is_empty(),
                    (Some(a), Some(b)) => a == b,
                };
                let warn_after_eq = match (&a_val.warn_after, &b_val.warn_after) {
                    (None, None) => true,
                    (None, Some(v)) | (Some(v), None) => v.is_empty(),
                    (Some(a), Some(b)) => a == b,
                };
                let filter_eq = match (&a_val.filter, &b_val.filter) {
                    (None, None) => true,
                    (None, Some(v)) | (Some(v), None) => v.is_empty(),
                    (Some(a), Some(b)) => a == b,
                };

                error_after_eq && warn_after_eq && filter_eq
            }
        }
    }

    /// Rendered `freshness` + `loaded_at_field` equality — mirrors dbt-core's
    /// `SourceDefinition.same_freshness` (dbt-mantle `core/dbt/contracts/graph/nodes.py:1444`:
    /// `self.freshness == other.freshness and self.loaded_at_field == other.loaded_at_field`).
    ///
    /// Consumed by `DbtSource::has_same_content`, the full-`state:modified` conjunct that restores
    /// the rendered freshness comparison dropped for source freshness by #11463 (see its doc).
    fn same_rendered_freshness(&self, other: &DbtSource) -> bool {
        self.same_freshness_value(other)
            && Self::loaded_at_str_eq(
                &self.__source_attr__.loaded_at_field,
                &other.__source_attr__.loaded_at_field,
            )
    }

    pub fn get_base_attr(&self) -> NodeBaseAttributes {
        self.__base_attr__.clone()
    }
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct DbtModel {
    pub __common_attr__: CommonAttributes,

    pub __base_attr__: NodeBaseAttributes,

    pub __model_attr__: DbtModelAttr,

    pub __adapter_attr__: AdapterAttr,

    // TO BE DEPRECATED
    #[serde(rename = "config")]
    pub deprecated_config: ModelConfig,

    pub __other__: BTreeMap<String, YmlValue>,
}

impl DbtModel {
    /// Transcribes dbt-core's `ModelNode.same_ref_representation` (dbt-mantle
    /// `core/dbt/contracts/graph/nodes.py:684-691`), which is ANDed into `ModelNode.same_contents`
    /// at `:677-682`:
    ///
    /// ```python
    /// def same_ref_representation(self, old: "ModelNode") -> bool:
    ///     return (
    ///         self.latest_version == old.latest_version   # changing it may break downstream unpinned refs
    ///         and self.access == old.access               # changes may lead to ref-related parsing errors
    ///         and self.deprecation_date == old.deprecation_date
    ///     )
    /// ```
    ///
    /// All three are node attributes (`__model_attr__`), compared rendered and ungated by
    /// `state_modified_compare_more_unrendered_values` — a change to any of them changes what a
    /// downstream `ref()` resolves to, independently of any config comparison.
    ///
    /// **This is not a double-count with `check_configs_modified`**, even though `access` is also
    /// compared there (Stage 1 via `unrendered_config`, Stage 2 via `deprecated_config.access`), and
    /// it must not be "fixed": here we compare `__model_attr__.access` — the **node attribute** —
    /// while `check_configs_modified` compares `ModelConfig.access` — the **config key**. dbt-core
    /// compares both too: `access` is a `ModelConfig` field with no `CompareBehavior.Exclude`, so
    /// `BaseConfig.same_contents` sees it *and* `same_ref_representation` sees the node attribute.
    /// The redundancy is transcribed deliberately.
    pub fn same_ref_representation(&self, old: &DbtModel) -> bool {
        let current = &self.__model_attr__;
        let previous = &old.__model_attr__;

        let same_latest_version =
            latest_version_eq(&current.latest_version, &previous.latest_version);
        // Non-`Option` on both sides: `manifest_model_to_dbt_model` already resolved
        // config key / node attribute / `Protected` default, so no `access_eq`-style tolerance
        // belongs here. `test_versioned_ref_representation_sparse_state_state_modified` and
        // `manifest_model_access_falls_back_to_node_attribute` guard that.
        let same_access = current.access == previous.access;
        let same_deprecation_date =
            deprecation_date_eq(&current.deprecation_date, &previous.deprecation_date);

        let result = same_latest_version && same_access && same_deprecation_date;

        if !result {
            log_state_mod_diff(
                &self.__common_attr__.unique_id,
                "model_ref_representation",
                [
                    (
                        "latest_version",
                        same_latest_version,
                        Some((
                            format!("{:?}", &current.latest_version),
                            format!("{:?}", &previous.latest_version),
                        )),
                    ),
                    (
                        "access",
                        same_access,
                        Some((current.access.to_string(), previous.access.to_string())),
                    ),
                    (
                        "deprecation_date",
                        same_deprecation_date,
                        Some((
                            format!("{:?}", &current.deprecation_date),
                            format!("{:?}", &previous.deprecation_date),
                        )),
                    ),
                ],
            );
        }

        result
    }

    pub fn same_contract(&self, old: &DbtModel) -> bool {
        match (&self.__model_attr__.contract, &old.__model_attr__.contract) {
            (Some(current_contract), Some(old_contract)) => {
                self.same_contract_both_present(old, old_contract, current_contract)
            }
            (Some(_self_contract), None) => false,
            (None, Some(old_contract)) => self.same_contract_removed(old, old_contract),
            (None, None) => true,
        }
    }

    // If a previous state contract and current state contract are both present,
    // compare them for changes.
    fn same_contract_both_present(
        &self,
        old: &DbtModel,
        old_contract: &DbtContract,
        current_contract: &DbtContract,
    ) -> bool {
        // Contract was not previously enforced
        if !old_contract.enforced && !current_contract.enforced {
            // No change -- same_contract: True
            return true;
        }
        if !old_contract.enforced && current_contract.enforced {
            // Now it's enforced. This is a change, but not a breaking change -- same_contract: False
            return false;
        }

        // Otherwise: The contract was previously enforced, and we need to check for changes.
        // Happy path: The contract is still being enforced, and the checksums are identical.
        if current_contract.enforced && current_contract.checksum == old_contract.checksum {
            // No change -- same_contract: True
            return true;
        }

        // Otherwise: There has been a change.
        // We need to determine if it is a **breaking** change.
        // These are the categories of breaking changes:
        let mut contract_enforced_disabled: bool = false;
        let mut columns_removed: Vec<String> = Vec::new();
        let mut column_type_changes: Vec<BTreeMap<String, String>> = Vec::new();
        let mut enforced_column_constraint_removed: Vec<BTreeMap<String, YmlValue>> = Vec::new(); // column_name, constraint_type
        let mut enforced_model_constraint_removed: Vec<BTreeMap<String, YmlValue>> = Vec::new(); // constraint_type, columns
        let mut materialization_changed: Vec<String> = Vec::new();

        if old_contract.enforced && !current_contract.enforced {
            // Breaking change: the contract was previously enforced, and it no longer is
            contract_enforced_disabled = true;
        }
        let mut column_constraints_exist: bool = false;

        // Helper function to check if materialization enforces constraints
        let materialization_enforces_constraints = |mat: &DbtMaterialization| -> bool {
            matches!(
                mat,
                DbtMaterialization::Table | DbtMaterialization::Incremental
            )
        };

        // Next, compare each column from the previous contract (old.columns)
        for old_value in old.__base_attr__.columns.iter() {
            // Has this column been removed?
            if !self.__base_attr__.columns.contains(old_value) {
                columns_removed.push(old_value.name.clone());
            }
            // Has this column's data type changed?
            else if let Some(current_column) = self
                .__base_attr__
                .columns
                .iter()
                .find(|col| col == &old_value)
            {
                if old_value.data_type != current_column.data_type {
                    let mut type_change = BTreeMap::new();
                    type_change.insert("column_name".to_string(), old_value.name.clone());
                    type_change.insert(
                        "previous_column_type".to_string(),
                        old_value
                            .data_type
                            .clone()
                            .unwrap_or_else(|| "unknown".to_string()),
                    );
                    type_change.insert(
                        "current_column_type".to_string(),
                        current_column
                            .data_type
                            .clone()
                            .unwrap_or_else(|| "unknown".to_string()),
                    );
                    column_type_changes.push(type_change);
                }
            }

            // track if there are any column level constraints for the materialization check later
            if !old_value.constraints.is_empty() {
                column_constraints_exist = true;
            }

            // Have enforced columns level constraints changed?
            // Constraints are only enforced for table and incremental materializations.
            // We only really care if the old node was one of those materializations for breaking changes
            if let Some(current_column) = self
                .__base_attr__
                .columns
                .iter()
                .find(|col| col == &old_value)
            {
                if old_value.constraints != current_column.constraints
                    && materialization_enforces_constraints(&old.materialized())
                {
                    for old_constraint in &old_value.constraints {
                        if !current_column.constraints.contains(old_constraint) {
                            let mut constraint_removed = BTreeMap::new();
                            constraint_removed.insert(
                                "column_name".to_string(),
                                YmlValue::string(old_value.name.clone()),
                            );
                            constraint_removed.insert(
                                "constraint_name".to_string(),
                                YmlValue::string(old_constraint.name.clone().unwrap_or_default()),
                            );
                            constraint_removed.insert(
                                "constraint_type".to_string(),
                                YmlValue::string(format!("{:?}", old_constraint.type_)),
                            );
                            enforced_column_constraint_removed.push(constraint_removed);
                        }
                    }
                }
            }
        }

        // Now compare the model level constraints
        if old.__model_attr__.constraints != self.__model_attr__.constraints
            && materialization_enforces_constraints(&old.materialized())
        {
            for old_constraint in &old.__model_attr__.constraints {
                if !self.__model_attr__.constraints.contains(old_constraint) {
                    let mut constraint_removed = BTreeMap::new();
                    constraint_removed.insert(
                        "constraint_name".to_string(),
                        YmlValue::string(old_constraint.name.clone().unwrap_or_default()),
                    );
                    constraint_removed.insert(
                        "constraint_type".to_string(),
                        YmlValue::string(format!("{:?}", old_constraint.type_)),
                    );
                    enforced_model_constraint_removed.push(constraint_removed);
                }
            }
        }

        // Check for relevant materialization changes.
        if materialization_enforces_constraints(&old.materialized())
            && !materialization_enforces_constraints(&self.materialized())
            && (!old.__model_attr__.constraints.is_empty() || column_constraints_exist)
        {
            materialization_changed = vec![
                format!("{:?}", old.materialized()),
                format!("{:?}", self.materialized()),
            ];
        }

        // If a column has been added, it will be missing in the old.columns, and present in self.columns
        // That's a change (caught by the different checksums), but not a breaking change

        // Did we find any changes that we consider breaking? If there's an enforced contract, that's
        // a warning unless the model is versioned, then it's an error.
        if contract_enforced_disabled
            || !columns_removed.is_empty()
            || !column_type_changes.is_empty()
            || !enforced_model_constraint_removed.is_empty()
            || !enforced_column_constraint_removed.is_empty()
            || !materialization_changed.is_empty()
        {
            let mut breaking_changes = Vec::new();

            if contract_enforced_disabled {
                breaking_changes.push(
                        "Contract enforcement was removed: Previously, this model had an enforced contract. It is no longer configured to enforce its contract, and this is a breaking change.".to_string()
                    );
            }

            if !columns_removed.is_empty() {
                let columns_removed_str = columns_removed.join("\n    - ");
                breaking_changes.push(format!(
                    "Columns were removed: \n    - {columns_removed_str}"
                ));
            }

            if !column_type_changes.is_empty() {
                let column_type_changes_str = column_type_changes
                    .iter()
                    .map(|c| {
                        format!(
                            "{} ({} -> {})",
                            c.get("column_name")
                                .map(|s| s.as_str())
                                .unwrap_or("unknown"),
                            c.get("previous_column_type")
                                .map(|s| s.as_str())
                                .unwrap_or("unknown"),
                            c.get("current_column_type")
                                .map(|s| s.as_str())
                                .unwrap_or("unknown")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n    - ");
                breaking_changes.push(format!(
                    "Columns with data_type changes: \n    - {column_type_changes_str}"
                ));
            }

            if !enforced_column_constraint_removed.is_empty() {
                let column_constraint_changes_str = enforced_column_constraint_removed
                    .iter()
                    .map(|c| {
                        let constraint_name = c
                            .get("constraint_name")
                            .and_then(|v| v.as_str())
                            .unwrap_or_else(|| {
                                c.get("constraint_type")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("unknown")
                            });
                        let column_name = c
                            .get("column_name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown");
                        format!("'{constraint_name}' constraint on column {column_name}")
                    })
                    .collect::<Vec<_>>()
                    .join("\n    - ");
                breaking_changes.push(format!(
                        "Enforced column level constraints were removed: \n    - {column_constraint_changes_str}"
                    ));
            }

            if !enforced_model_constraint_removed.is_empty() {
                let model_constraint_changes_str = enforced_model_constraint_removed
                    .iter()
                    .map(|c| {
                        let constraint_name = c
                            .get("constraint_name")
                            .and_then(|v| v.as_str())
                            .unwrap_or_else(|| {
                                c.get("constraint_type")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("unknown")
                            });
                        let columns = c
                            .get("columns")
                            .and_then(|v| v.as_sequence())
                            .map(|seq| {
                                seq.iter()
                                    .filter_map(|v| v.as_str())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            })
                            .unwrap_or_else(|| "unknown".to_string());
                        format!("'{constraint_name}' constraint on columns {columns}")
                    })
                    .collect::<Vec<_>>()
                    .join("\n    - ");
                breaking_changes.push(format!(
                        "Enforced model level constraints were removed: \n    - {model_constraint_changes_str}"
                    ));
            }

            if !materialization_changed.is_empty() {
                let materialization_changes_str = format!(
                    "{} -> {}",
                    materialization_changed
                        .first()
                        .map(|s| s.as_str())
                        .unwrap_or("unknown"),
                    materialization_changed
                        .get(1)
                        .map(|s| s.as_str())
                        .unwrap_or("unknown")
                );
                breaking_changes.push(format!(
                        "Materialization changed with enforced constraints: \n    - {materialization_changes_str}"
                    ));
            }

            // Generate warning or error depending on if the model is versioned
            let reasons = breaking_changes.join("\n  - ");
            if old.is_versioned() {
                self.log_contract_breaking_change_error(&reasons, &old.__common_attr__.name);
            } else {
                self.log_unversioned_breaking_change_warning(
                    &reasons,
                    &old.__common_attr__.name,
                    old.file_path(),
                );
            }
        }
        // Otherwise, the contract has changed -- same_contract: False
        false
    }
    fn same_contract_removed(&self, old: &DbtModel, old_contract: &DbtContract) -> bool {
        // If the contract wasn't previously enforced, no contract change has occurred
        if !old_contract.enforced {
            return true;
        }
        // Removed node is past its deprecation_date, so deletion does not constitute a contract change
        if let Some(deprecation_date_str) = &old.__model_attr__.deprecation_date
            && let Some(deprecation_date) =
                crate::schemas::common::parse_deprecation_date(deprecation_date_str)
            && deprecation_date < Utc::now()
        {
            return true;
        }

        // Disabled, deleted, or renamed node with previously enforced contract.
        let breaking_change = if old.__base_attr__.enabled {
            format!(
                "Contracted model '{}' was deleted or renamed.",
                old.__common_attr__.unique_id
            )
        } else {
            format!(
                "Contracted model '{}' was disabled.",
                old.__common_attr__.unique_id
            )
        };

        self.log_contract_breaking_change_error(&breaking_change, &old.__common_attr__.name);

        // Otherwise, the contract has changed -- same_contract: False
        false
    }

    fn log_contract_breaking_change_error(&self, breaking_change: &String, node_name: &String) {
        let error_message = format!(
            "While comparing to previous project state, dbt detected a breaking change to an enforced contract.\n  - {breaking_change}\n\
            Consider making an additive (non-breaking) change instead, if possible.\n\
            Otherwise, create a new model version: https://docs.getdbt.com/docs/collaborate/govern/model-versions"
        );

        let breaking_change_message =
            format!("Breaking Change to Contract for model '{node_name}': {error_message}");
        emit_error_log_message(ErrorCode::InvalidConfig, breaking_change_message);
    }

    fn log_unversioned_breaking_change_warning(
        &self,
        breaking_change: &String,
        node_name: &String,
        file_path: String,
    ) {
        let warning_message = format!(
            "Breaking change to contracted, unversioned model {node_name} ({file_path})\
            \nWhile comparing to previous project state, dbt detected a breaking change to an unversioned model.\
            \n  - {breaking_change}\n"
        );

        emit_warn_log_message(ErrorCode::UnversionedBreakingChange, warning_message);
    }
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct AdapterAttr {
    pub snowflake_attr: Option<Box<SnowflakeAttr>>,
    pub databricks_attr: Option<Box<DatabricksAttr>>,
    pub bigquery_attr: Option<Box<BigQueryAttr>>,
    pub redshift_attr: Option<Box<RedshiftAttr>>,
}

impl AdapterAttr {
    pub fn with_snowflake_attr(mut self, snowflake_attr: Option<Box<SnowflakeAttr>>) -> Self {
        self.snowflake_attr = snowflake_attr;
        self
    }

    pub fn with_databricks_attr(mut self, databricks_attr: Option<Box<DatabricksAttr>>) -> Self {
        self.databricks_attr = databricks_attr;
        self
    }

    pub fn with_bigquery_attr(mut self, bigquery_attr: Option<Box<BigQueryAttr>>) -> Self {
        self.bigquery_attr = bigquery_attr;
        self
    }

    pub fn with_redshift_attr(mut self, redshift_attr: Option<Box<RedshiftAttr>>) -> Self {
        self.redshift_attr = redshift_attr;
        self
    }

    /// Creates a new [AdapterAttr] from a given [&WarehouseSpecificNodeConfig] and adapter type string
    pub fn from_config_and_dialect(
        config: &WarehouseSpecificNodeConfig,
        adapter_type: AdapterType,
    ) -> Self {
        // TODO: Make all None inputs result in a None for that attr
        // TODO: Creation of configs should be delegated to the adapter specific attr struct
        // Here we naively initialize the resolved adapter attributes.
        // This allows propagation of configurations and switching on the adapter type
        // allows us to save memory for unused configurations.
        //
        // A further optimization can be done by only setting inner fields of [AdapterAttr] if at least one field of the
        // input is not None
        match adapter_type {
            AdapterType::Snowflake => {
                AdapterAttr::default().with_snowflake_attr(Some(Box::new(SnowflakeAttr {
                    table_tag: config.table_tag.clone(),
                    partition_by: config.partition_by.clone(),
                    cluster_by: config.cluster_by.clone(),
                    row_access_policy: config.row_access_policy.clone(),
                    adapter_properties: config.adapter_properties.clone(),
                    external_volume: config.external_volume.clone(),
                    base_location_root: config.base_location_root.clone(),
                    base_location_subpath: config.base_location_subpath.clone(),
                    change_tracking: config.change_tracking,
                    data_retention_time_in_days: config.data_retention_time_in_days,
                    max_data_extension_time_in_days: config.max_data_extension_time_in_days,
                    storage_serialization_policy: config.storage_serialization_policy.clone(),
                    iceberg_version: config.iceberg_version,
                    target_file_size: config.target_file_size.clone(),
                    target_lag: config.target_lag.clone(),
                    snowflake_initialization_warehouse: config
                        .snowflake_initialization_warehouse
                        .clone(),
                    snowflake_warehouse: config.snowflake_warehouse.clone(),
                    refresh_warehouse: config.refresh_warehouse.clone(),
                    immutable_where: config.immutable_where.clone(),
                    refresh_mode: config.refresh_mode.clone(),
                    initialize: config.initialize.clone(),
                    scheduler: config.scheduler.clone(),
                    tmp_relation_type: config.tmp_relation_type.clone(),
                    query_tag: config.query_tag.clone(),
                    automatic_clustering: config.automatic_clustering,
                    copy_grants: config.copy_grants,
                    copy_tags: config.copy_tags,
                    secure: config.secure,
                    transient: config.transient,
                })))
            }
            AdapterType::Postgres => AdapterAttr::default(),
            AdapterType::Bigquery => {
                AdapterAttr::default().with_bigquery_attr(Some(Box::new(BigQueryAttr {
                    description: config.description.clone(),
                    adapter_properties: config.adapter_properties.clone(),
                    external_volume: config.external_volume.clone(),
                    base_location_root: config.base_location_root.clone(),
                    base_location_subpath: config.base_location_subpath.clone(),
                    storage_uri: config.storage_uri.clone(),
                    file_format: config.file_format.clone(),
                    partition_by: config.partition_by.clone(),
                    cluster_by: config.cluster_by.clone(),
                    hours_to_expiration: config.hours_to_expiration.clone().into_inner().flatten(),
                    job_execution_timeout_seconds: config.job_execution_timeout_seconds,
                    reservation: config.reservation.clone(),
                    labels: config.labels.clone(),
                    labels_from_meta: config.labels_from_meta,
                    kms_key_name: config.kms_key_name.clone(),
                    resource_tags: config.resource_tags.clone(),
                    require_partition_filter: config.require_partition_filter,
                    partition_expiration_days: config.partition_expiration_days,
                    grant_access_to: config.grant_access_to.clone(),
                    partitions: config.partitions.clone(),
                    enable_refresh: config.enable_refresh,
                    refresh_interval_minutes: config.refresh_interval_minutes,
                    max_staleness: config.max_staleness.clone(),
                    jar_file_uri: config.jar_file_uri.clone(),
                    timeout: config.timeout,
                })))
            }
            AdapterType::Redshift => {
                AdapterAttr::default().with_redshift_attr(Some(Box::new(RedshiftAttr {
                    auto_refresh: config.auto_refresh,
                    backup: config.backup,
                    bind: config.bind,
                    dist: config.dist.clone(),
                    sort: config.sort.clone(),
                    sort_type: config.sort_type.clone(),
                })))
            }
            AdapterType::Databricks => {
                AdapterAttr::default().with_databricks_attr(Some(Box::new(DatabricksAttr {
                    adapter_properties: config.adapter_properties.clone(),
                    partition_by: config.partition_by.clone(),
                    file_format: config.file_format.clone(),
                    location_root: config.location_root.clone(),
                    use_uniform: config.use_uniform,
                    tblproperties: config.tblproperties.clone(),
                    include_full_name_in_path: config.include_full_name_in_path,
                    liquid_clustered_by: config.liquid_clustered_by.clone(),
                    auto_liquid_cluster: config.auto_liquid_cluster,
                    zorder: config.zorder.clone(),
                    skip_optimize: config.skip_optimize,
                    clustered_by: config.clustered_by.clone(),
                    buckets: config.buckets,
                    catalog: config.catalog.clone(),
                    databricks_tags: config.databricks_tags.clone(),
                    query_tags: config.query_tags.clone(),
                    compression: config.compression.clone(),
                    databricks_compute: config.databricks_compute.clone(),
                    target_alias: config.target_alias.clone(),
                    source_alias: config.source_alias.clone(),
                    matched_condition: config.matched_condition.clone(),
                    not_matched_condition: config.not_matched_condition.clone(),
                    not_matched_by_source_condition: config.not_matched_by_source_condition.clone(),
                    not_matched_by_source_action: config.not_matched_by_source_action.clone(),
                    merge_with_schema_evolution: config.merge_with_schema_evolution,
                    skip_matched_step: config.skip_matched_step,
                    skip_not_matched_step: config.skip_not_matched_step,
                    unique_tmp_table_suffix: config.unique_tmp_table_suffix,
                    row_filter: config.row_filter.clone(),
                    schedule: config.schedule.as_ref().map(|s| s.to_schedule_config()),
                })))
            }
            _ => {
                // Unknown input, populate ALL adapter attributes to maximize compatibility downstream
                AdapterAttr::default()
                    .with_snowflake_attr(Some(Box::new(SnowflakeAttr {
                        table_tag: config.table_tag.clone(),
                        partition_by: config.partition_by.clone(),
                        cluster_by: config.cluster_by.clone(),
                        row_access_policy: config.row_access_policy.clone(),
                        adapter_properties: config.adapter_properties.clone(),
                        external_volume: config.external_volume.clone(),
                        base_location_root: config.base_location_root.clone(),
                        base_location_subpath: config.base_location_subpath.clone(),
                        change_tracking: config.change_tracking,
                        data_retention_time_in_days: config.data_retention_time_in_days,
                        max_data_extension_time_in_days: config.max_data_extension_time_in_days,
                        storage_serialization_policy: config.storage_serialization_policy.clone(),
                        iceberg_version: config.iceberg_version,
                        target_file_size: config.target_file_size.clone(),
                        target_lag: config.target_lag.clone(),
                        snowflake_initialization_warehouse: config
                            .snowflake_initialization_warehouse
                            .clone(),
                        snowflake_warehouse: config.snowflake_warehouse.clone(),
                        refresh_warehouse: config.refresh_warehouse.clone(),
                        immutable_where: config.immutable_where.clone(),
                        refresh_mode: config.refresh_mode.clone(),
                        initialize: config.initialize.clone(),
                        scheduler: config.scheduler.clone(),
                        tmp_relation_type: config.tmp_relation_type.clone(),
                        query_tag: config.query_tag.clone(),
                        automatic_clustering: config.automatic_clustering,
                        copy_grants: config.copy_grants,
                        copy_tags: config.copy_tags,
                        secure: config.secure,
                        transient: config.transient,
                    })))
                    .with_bigquery_attr(Some(Box::new(BigQueryAttr {
                        description: config.description.clone(),
                        adapter_properties: config.adapter_properties.clone(),
                        external_volume: config.external_volume.clone(),
                        base_location_root: config.base_location_root.clone(),
                        base_location_subpath: config.base_location_subpath.clone(),
                        storage_uri: config.storage_uri.clone(),
                        file_format: config.file_format.clone(),
                        partition_by: config.partition_by.clone(),
                        cluster_by: config.cluster_by.clone(),
                        hours_to_expiration: config
                            .hours_to_expiration
                            .clone()
                            .into_inner()
                            .flatten(),
                        job_execution_timeout_seconds: config.job_execution_timeout_seconds,
                        reservation: config.reservation.clone(),
                        labels: config.labels.clone(),
                        labels_from_meta: config.labels_from_meta,
                        kms_key_name: config.kms_key_name.clone(),
                        resource_tags: config.resource_tags.clone(),
                        require_partition_filter: config.require_partition_filter,
                        partition_expiration_days: config.partition_expiration_days,
                        grant_access_to: config.grant_access_to.clone(),
                        partitions: config.partitions.clone(),
                        enable_refresh: config.enable_refresh,
                        refresh_interval_minutes: config.refresh_interval_minutes,
                        max_staleness: config.max_staleness.clone(),
                        jar_file_uri: config.jar_file_uri.clone(),
                        timeout: config.timeout,
                    })))
                    .with_redshift_attr(Some(Box::new(RedshiftAttr {
                        auto_refresh: config.auto_refresh,
                        backup: config.backup,
                        bind: config.bind,
                        dist: config.dist.clone(),
                        sort: config.sort.clone(),
                        sort_type: config.sort_type.clone(),
                    })))
                    .with_databricks_attr(Some(Box::new(DatabricksAttr {
                        adapter_properties: config.adapter_properties.clone(),
                        partition_by: config.partition_by.clone(),
                        file_format: config.file_format.clone(),
                        location_root: config.location_root.clone(),
                        use_uniform: config.use_uniform,
                        tblproperties: config.tblproperties.clone(),
                        include_full_name_in_path: config.include_full_name_in_path,
                        liquid_clustered_by: config.liquid_clustered_by.clone(),
                        auto_liquid_cluster: config.auto_liquid_cluster,
                        zorder: config.zorder.clone(),
                        skip_optimize: config.skip_optimize,
                        clustered_by: config.clustered_by.clone(),
                        buckets: config.buckets,
                        catalog: config.catalog.clone(),
                        databricks_tags: config.databricks_tags.clone(),
                        query_tags: config.query_tags.clone(),
                        compression: config.compression.clone(),
                        databricks_compute: config.databricks_compute.clone(),
                        target_alias: config.target_alias.clone(),
                        source_alias: config.source_alias.clone(),
                        matched_condition: config.matched_condition.clone(),
                        not_matched_condition: config.not_matched_condition.clone(),
                        not_matched_by_source_condition: config
                            .not_matched_by_source_condition
                            .clone(),
                        not_matched_by_source_action: config.not_matched_by_source_action.clone(),
                        merge_with_schema_evolution: config.merge_with_schema_evolution,
                        skip_matched_step: config.skip_matched_step,
                        skip_not_matched_step: config.skip_not_matched_step,
                        unique_tmp_table_suffix: config.unique_tmp_table_suffix,
                        row_filter: config.row_filter.clone(),
                        schedule: config.schedule.as_ref().map(|s| s.to_schedule_config()),
                    })))
            }
        }
    }
}

/// A resolved Snowflake configuration
#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SnowflakeAttr {
    pub adapter_properties: Option<BTreeMap<String, YmlValue>>,
    pub partition_by: Option<PartitionConfig>,
    pub cluster_by: Option<ClusterConfig>,
    pub table_tag: Option<String>,
    pub row_access_policy: Option<String>,
    pub external_volume: Option<String>,
    pub base_location_root: Option<String>,
    pub base_location_subpath: Option<String>,
    pub change_tracking: Option<bool>,
    pub data_retention_time_in_days: Option<u64>,
    pub max_data_extension_time_in_days: Option<u64>,
    pub storage_serialization_policy: Option<String>,
    pub iceberg_version: Option<u64>,
    pub target_file_size: Option<String>,
    pub target_lag: Option<String>,
    pub snowflake_initialization_warehouse: Option<String>,
    pub snowflake_warehouse: Option<String>,
    pub refresh_warehouse: Option<String>,
    pub immutable_where: Option<String>,
    pub refresh_mode: Option<String>,
    pub initialize: Option<String>,
    pub scheduler: Option<String>,
    pub tmp_relation_type: Option<String>,
    pub query_tag: Option<QueryTag>,
    pub automatic_clustering: Option<bool>,
    pub copy_grants: Option<bool>,
    pub copy_tags: Option<bool>,
    pub secure: Option<bool>,
    pub transient: Option<bool>,
}

/// A resolved Databricks configuration
#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DatabricksAttr {
    pub adapter_properties: Option<BTreeMap<String, YmlValue>>,
    pub partition_by: Option<PartitionConfig>,
    pub file_format: Option<String>,
    pub location_root: Option<String>,
    pub use_uniform: Option<bool>,
    pub tblproperties: Option<TblProperties>,
    pub include_full_name_in_path: Option<bool>,
    pub liquid_clustered_by: Option<StringOrArrayOfStrings>,
    pub auto_liquid_cluster: Option<bool>,
    pub zorder: Option<StringOrArrayOfStrings>,
    pub skip_optimize: Option<bool>,
    pub clustered_by: Option<StringOrArrayOfStrings>,
    pub buckets: Option<i64>,
    pub catalog: Option<String>,
    pub databricks_tags: Option<IndexMap<String, YmlValue>>,
    pub query_tags: Option<String>,
    pub compression: Option<String>,
    pub databricks_compute: Option<String>,
    pub target_alias: Option<String>,
    pub source_alias: Option<String>,
    pub matched_condition: Option<String>,
    pub not_matched_condition: Option<String>,
    pub not_matched_by_source_condition: Option<String>,
    pub not_matched_by_source_action: Option<String>,
    pub merge_with_schema_evolution: Option<bool>,
    pub skip_matched_step: Option<bool>,
    pub skip_not_matched_step: Option<bool>,
    pub row_filter: Option<RowFilterConfig>,
    pub unique_tmp_table_suffix: Option<bool>,
    pub schedule: Option<ScheduleConfig>,
}

/// A resolved BigQuery configuration
#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BigQueryAttr {
    pub description: Option<String>,
    pub adapter_properties: Option<BTreeMap<String, YmlValue>>,
    pub external_volume: Option<String>,
    pub file_format: Option<String>,
    pub base_location_root: Option<String>,
    pub base_location_subpath: Option<String>,
    pub storage_uri: Option<String>,
    pub partition_by: Option<PartitionConfig>,
    pub cluster_by: Option<ClusterConfig>,
    pub hours_to_expiration: Option<StringOrInteger>,
    pub job_execution_timeout_seconds: Option<u64>,
    pub reservation: Option<String>,
    pub labels: Option<IndexMap<String, String>>,
    pub labels_from_meta: Option<bool>,
    pub kms_key_name: Option<String>,
    pub resource_tags: Option<IndexMap<String, String>>,
    pub require_partition_filter: Option<bool>,
    pub partition_expiration_days: Option<u64>,
    pub grant_access_to: Option<Vec<GrantAccessToTarget>>,
    pub partitions: Option<PartitionsConfig>,
    pub enable_refresh: Option<bool>,
    pub refresh_interval_minutes: Option<f64>,
    pub max_staleness: Option<String>,
    pub jar_file_uri: Option<String>,
    pub timeout: Option<u64>,
}

/// A resolved Redshift configuration
#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RedshiftAttr {
    pub auto_refresh: Option<bool>,
    pub backup: Option<bool>,
    pub bind: Option<bool>,
    pub dist: Option<StringOrArrayOfStrings>,
    pub sort: Option<StringOrArrayOfStrings>,
    pub sort_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DbtModelAttr {
    pub access: Access,
    pub group: Option<String>,
    #[serde(skip_serializing, default = "default_introspection")]
    pub introspection: IntrospectionKind,
    pub contract: Option<DbtContract>,
    pub incremental_strategy: Option<DbtIncrementalStrategy>,
    pub freshness: Option<ModelFreshness>,
    pub state: Option<crate::schemas::properties::ModelState>,
    pub version: Option<StringOrInteger>,
    pub latest_version: Option<StringOrInteger>,
    pub constraints: Vec<ModelConstraint>,
    pub deprecation_date: Option<String>,
    // TODO: Investigate why primary_key is needed here (constraints already exist)
    pub primary_key: Vec<String>,
    pub time_spine: Option<TimeSpine>,
    pub event_time: Option<String>,
    // TODO(anna): See if we _need_ to put these here, or if they can somehow be added to AdapterAttr.
    pub catalog_name: Option<String>,
    pub table_format: Option<String>,
    pub sync: Option<SyncConfig>,
    /// Compiled SQL, populated only by callers that need to diff it (see
    /// [`InternalDbtNodeAttributes::compiled_code`]). Transient: never round-trips through the
    /// manifest.
    #[serde(skip)]
    pub compiled_code: Option<String>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct DbtFunction {
    pub __common_attr__: CommonAttributes,

    pub __base_attr__: NodeBaseAttributes,

    pub __function_attr__: DbtFunctionAttr,

    // To be deprecated
    #[serde(rename = "config")]
    pub deprecated_config: FunctionConfig,

    pub __other__: BTreeMap<String, YmlValue>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct AbsorbedOverload {
    pub defined_in: String,
    pub arguments: Option<Vec<FunctionArgument>>,
    pub returns: Option<FunctionReturnType>,
    pub description: Option<String>,
    pub raw_body: Option<String>,
    pub compiled_body: Option<String>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct DbtFunctionAttr {
    pub access: Access,
    pub group: Option<String>,
    pub language: Option<String>,
    pub on_configuration_change: Option<String>,
    pub returns: Option<FunctionReturnType>,
    pub arguments: Option<Vec<FunctionArgument>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overloads: Vec<AbsorbedOverload>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct TimeSpine {
    pub node_relation: NodeRelation,
    pub primary_column: TimeSpinePrimaryColumn,
    pub custom_granularities:
        Vec<crate::schemas::properties::model_properties::TimeSpineCustomGranularity>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct TimeSpinePrimaryColumn {
    pub name: String,
    pub time_granularity: crate::schemas::dbt_column::Granularity,
}

fn default_introspection() -> IntrospectionKind {
    IntrospectionKind::None
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use serde::Deserialize;

    use super::{
        AbsorbedOverload, DbtAnalysis, DbtExposure, DbtFunction, DbtMacro, DbtModel, DbtSeed,
        DbtSnapshot, DbtSource, DbtTest, InternalDbtNode, InternalDbtNodeAttributes, ModelConfig,
        ModelFreshness, NodePathKind, hooks_equal, normalize_description,
        persist_docs_configs_equal, quoting_equal, test_alias_config_equal,
    };
    use crate::schemas::common::{
        FreshnessPeriod, FreshnessRules, Hooks, ModelFreshnessRules, PersistDocsConfig,
    };
    use crate::schemas::manifest::{DbtMetric, DbtOperation, DbtSavedQuery};
    use crate::schemas::project::SnapshotMetaColumnNames;
    use dbt_adapter_core::AdapterType;
    use dbt_common::path::{DbtPath, path_separator_eq};
    use dbt_telemetry::ExecutionPhase;
    use dbt_yaml::Verbatim;

    type YmlValue = dbt_yaml::Value;

    fn snapshot_with_paths(name: &str, path: &str, original_file_path: &str) -> DbtSnapshot {
        let mut snapshot = DbtSnapshot::default();
        snapshot.__common_attr__.name = name.to_string();
        snapshot.__common_attr__.package_name = "pkg".to_string();
        snapshot.__common_attr__.path = DbtPath::from(path);
        snapshot.__common_attr__.original_file_path = DbtPath::from(original_file_path);
        snapshot.__base_attr__.alias = name.to_string();
        snapshot
    }

    #[test]
    fn yaml_snapshot_node_paths_are_phase_accurate() {
        let snapshot = snapshot_with_paths(
            "daily_orders",
            "snapshots/daily_orders.sql",
            "snapshots/snapshots.yml",
        );
        let in_dir = Path::new("/workspace");
        let out_dir = Path::new("/workspace/target");

        assert_eq!(
            snapshot
                .get_node_path(NodePathKind::Definition, in_dir, out_dir)
                .as_ref(),
            Path::new("snapshots/snapshots.yml")
        );
        assert_eq!(
            snapshot.get_node_path_abs(NodePathKind::Compiled, in_dir, out_dir),
            PathBuf::from(
                "/workspace/target/compiled/pkg/snapshots/snapshots.yml/daily_orders.sql"
            )
        );
        assert_eq!(
            snapshot
                .get_node_path(NodePathKind::Executable, in_dir, out_dir)
                .as_ref(),
            Path::new("target/run/pkg/snapshots/snapshots.yml/daily_orders.sql")
        );
    }

    fn analysis_with_paths(name: &str, original_file_path: &str) -> DbtAnalysis {
        let mut analysis = DbtAnalysis::default();
        analysis.__common_attr__.name = name.to_string();
        analysis.__common_attr__.package_name = "pkg".to_string();
        analysis.__common_attr__.path = DbtPath::from(original_file_path);
        analysis.__common_attr__.original_file_path = DbtPath::from(original_file_path);
        analysis.__base_attr__.alias = name.to_string();
        analysis
    }

    #[test]
    fn generic_test_executable_path_uses_bounded_path_filename() {
        let mut test = DbtTest::default();
        test.__common_attr__.name = "full_generic_test_name".to_string();
        test.__common_attr__.package_name = "pkg".to_string();
        test.__common_attr__.path = DbtPath::from(
            "generic_tests/not_null_model_column_4f8cc80baaff29183132758d312c335a.sql",
        );
        test.__common_attr__.original_file_path = DbtPath::from("models/schema.yml");
        test.__base_attr__.alias = "full_generic_test_name_expanded_by_project_macro".to_string();
        test.defined_at = Some(dbt_common::CodeLocationWithFile::new(
            1,
            1,
            0,
            "models/schema.yml",
        ));

        let actual = test.get_node_path_abs(
            NodePathKind::Executable,
            Path::new("/workspace"),
            Path::new("/workspace/target"),
        );

        let expected = Path::new("/workspace")
            .join("target")
            .join("run")
            .join("pkg")
            .join("models")
            .join("schema.yml")
            .join("generic_tests")
            .join("not_null_model_column_4f8cc80baaff29183132758d312c335a.sql");
        assert_eq!(actual, expected);
    }

    #[test]
    fn analysis_node_paths_are_phase_accurate() {
        let analysis = analysis_with_paths("my_analysis", "analyses/my_analysis.sql");
        let in_dir = Path::new("/workspace");
        let out_dir = Path::new("/workspace/target");

        // Parse / Render: raw SQL file (original_file_path).
        assert_eq!(
            analysis
                .get_node_path(NodePathKind::Definition, in_dir, out_dir)
                .as_ref(),
            Path::new("analyses/my_analysis.sql")
        );

        // Analysis: compiled SQL file.
        let actual_compiled_path = analysis.get_node_path(NodePathKind::Compiled, in_dir, out_dir);
        let actual_compiled_path = actual_compiled_path.to_string_lossy();
        let expected_compiled_path = "target/compiled/pkg/analyses/my_analysis.sql";
        assert!(
            path_separator_eq(&actual_compiled_path, expected_compiled_path),
            "left: {actual_compiled_path:?}\nright: {expected_compiled_path:?}"
        );

        // Analyses are never materialized/run: Executable falls back to the definition path.
        assert_eq!(
            analysis
                .get_node_path(NodePathKind::Executable, in_dir, out_dir)
                .as_ref(),
            Path::new("analyses/my_analysis.sql")
        );
    }

    fn function_with_paths(name: &str, original_file_path: &str) -> DbtFunction {
        let mut function = DbtFunction::default();
        function.__common_attr__.name = name.to_string();
        function.__common_attr__.package_name = "pkg".to_string();
        function.__common_attr__.path = DbtPath::from(original_file_path);
        function.__common_attr__.original_file_path = DbtPath::from(original_file_path);
        function.__base_attr__.alias = name.to_string();
        function
    }

    #[test]
    fn function_node_paths_are_phase_accurate() {
        let function = function_with_paths("my_function", "functions/my_function.sql");
        let in_dir = Path::new("/workspace");
        let out_dir = Path::new("/workspace/target");

        // Parse / Render: raw SQL file (original_file_path).
        assert_eq!(
            function
                .get_node_path(NodePathKind::Definition, in_dir, out_dir)
                .as_ref(),
            Path::new("functions/my_function.sql")
        );

        // Analysis: compiled SQL file.
        let actual_compiled_path = function.get_node_path(NodePathKind::Compiled, in_dir, out_dir);
        let actual_compiled_path = actual_compiled_path.to_string_lossy();
        let expected_compiled_path = "target/compiled/pkg/functions/my_function.sql";
        assert!(
            path_separator_eq(&actual_compiled_path, expected_compiled_path),
            "left: {actual_compiled_path:?}\nright: {expected_compiled_path:?}"
        );

        // Materialization: run path (functions are materialized, unlike analyses).
        let actual_run_path = function.get_node_path(NodePathKind::Executable, in_dir, out_dir);
        let actual_run_path = actual_run_path.to_string_lossy();
        let expected_run_path = "target/run/pkg/functions/my_function.sql";
        assert!(
            path_separator_eq(&actual_run_path, expected_run_path),
            "left: {actual_run_path:?}\nright: {expected_run_path:?}"
        );
    }

    #[test]
    fn function_overload_state_modified_ignores_compiled_body() {
        // Build two copies of the same function whose single overload differs only
        // in `compiled_body` (the rendered artifact) — same source, same signature.
        let make_function = |compiled_body: &str, raw_body: &str| {
            let mut function = function_with_paths("null_if_empty", "functions/null_if_empty.sql");
            function.__function_attr__.overloads = vec![AbsorbedOverload {
                defined_in: "null_if_empty_int".to_string(),
                arguments: None,
                returns: None,
                description: None,
                raw_body: Some(raw_body.to_string()),
                compiled_body: Some(compiled_body.to_string()),
            }];
            function
        };

        // compiled_body differs, everything else identical → NOT state:modified.
        let a = make_function("SELECT 1 -- rendered A", "SELECT {{ x }}");
        let b = make_function("SELECT 1 -- rendered B", "SELECT {{ x }}");
        assert!(
            a.has_same_content(&b, AdapterType::Snowflake),
            "a compiled_body-only difference must not flag the function as state:modified"
        );

        // Overload source (raw_body) differs → state:modified.
        let c = make_function("SELECT 1", "SELECT {{ x }}");
        let d = make_function("SELECT 1", "SELECT {{ y }}");
        assert!(
            !c.has_same_content(&d, AdapterType::Snowflake),
            "an overload raw_body change must flag the function as state:modified"
        );
    }

    fn seed_with_paths(name: &str, original_file_path: &str) -> DbtSeed {
        let mut seed = DbtSeed::default();
        seed.__common_attr__.name = name.to_string();
        seed.__common_attr__.package_name = "pkg".to_string();
        seed.__common_attr__.path = DbtPath::from(original_file_path);
        seed.__common_attr__.original_file_path = DbtPath::from(original_file_path);
        seed.__base_attr__.alias = name.to_string();
        seed
    }

    #[test]
    fn seed_node_paths_are_phase_accurate() {
        let seed = seed_with_paths("my_seed", "seeds/my_seed.csv");
        let in_dir = Path::new("/workspace");
        let out_dir = Path::new("/workspace/target");

        // Parse / Render: raw CSV file (original_file_path).
        assert_eq!(
            seed.get_node_path(NodePathKind::Definition, in_dir, out_dir)
                .as_ref(),
            Path::new("seeds/my_seed.csv")
        );

        // Analysis: falls back to the raw CSV file — seeds are NOT in the Compiled arm.
        assert_eq!(
            seed.get_node_path(NodePathKind::Compiled, in_dir, out_dir)
                .as_ref(),
            Path::new("seeds/my_seed.csv")
        );

        // Materialization: run path (the `.csv` is replaced by `{alias}.sql`, not nested).
        let actual_run_path = seed.get_node_path(NodePathKind::Executable, in_dir, out_dir);
        let actual_run_path = actual_run_path.to_string_lossy();
        let expected_run_path = "target/run/pkg/seeds/my_seed.sql";
        assert!(
            path_separator_eq(&actual_run_path, expected_run_path),
            "left: {actual_run_path:?}\nright: {expected_run_path:?}"
        );
    }

    fn source_with_paths(name: &str, original_file_path: &str) -> DbtSource {
        let mut source = DbtSource::default();
        source.__common_attr__.name = name.to_string();
        source.__common_attr__.package_name = "pkg".to_string();
        source.__common_attr__.path = DbtPath::from(original_file_path);
        source.__common_attr__.original_file_path = DbtPath::from(original_file_path);
        source.__base_attr__.alias = name.to_string();
        source
    }

    #[test]
    fn source_node_paths_are_phase_accurate() {
        let source = source_with_paths("raw_customers", "models/__sources.yml");
        let in_dir = Path::new("/workspace");
        let out_dir = Path::new("/workspace/target");

        for kind in [
            NodePathKind::Definition,
            NodePathKind::Compiled,
            NodePathKind::Executable,
        ] {
            assert_eq!(
                source.get_node_path(kind, in_dir, out_dir).as_ref(),
                Path::new("models/__sources.yml"),
                "kind {kind:?} should fall back to the raw YAML definition path",
            );
        }

        // The source freshness path unification relies on get_node_path(Definition)
        // returning the same value the freshness call sites previously read from
        // __common_attr__.path (parser sets path == original_file_path for sources).
        assert_eq!(
            source
                .get_node_path(NodePathKind::Definition, in_dir, out_dir)
                .as_ref(),
            source.__common_attr__.path.as_path(),
        );
    }

    fn exposure_with_paths(name: &str, original_file_path: &str) -> DbtExposure {
        let mut exposure = DbtExposure::default();
        exposure.__common_attr__.name = name.to_string();
        exposure.__common_attr__.package_name = "pkg".to_string();
        exposure.__common_attr__.path = DbtPath::from(original_file_path);
        exposure.__common_attr__.original_file_path = DbtPath::from(original_file_path);
        exposure.__base_attr__.alias = name.to_string();
        exposure
    }

    #[test]
    fn exposure_node_paths_are_phase_accurate() {
        let exposure = exposure_with_paths("weekly_dashboard", "models/exposures.yml");
        let in_dir = Path::new("/workspace");
        let out_dir = Path::new("/workspace/target");

        for kind in [
            NodePathKind::Definition,
            NodePathKind::Compiled,
            NodePathKind::Executable,
        ] {
            assert_eq!(
                exposure.get_node_path(kind, in_dir, out_dir).as_ref(),
                Path::new("models/exposures.yml"),
                "kind {kind:?} should fall back to the raw YAML definition path",
            );
        }

        assert_eq!(
            exposure
                .get_node_path(NodePathKind::Definition, in_dir, out_dir)
                .as_ref(),
            exposure.__common_attr__.path.as_path(),
        );
    }

    fn metric_with_paths(name: &str, original_file_path: &str) -> DbtMetric {
        let mut metric = DbtMetric::default();
        metric.__common_attr__.name = name.to_string();
        metric.__common_attr__.package_name = "pkg".to_string();
        metric.__common_attr__.path = DbtPath::from(original_file_path);
        metric.__common_attr__.original_file_path = DbtPath::from(original_file_path);
        metric.__base_attr__.alias = name.to_string();
        metric
    }

    #[test]
    fn metric_node_paths_are_phase_accurate() {
        let metric = metric_with_paths("revenue", "models/metrics.yml");
        let in_dir = Path::new("/workspace");
        let out_dir = Path::new("/workspace/target");

        for kind in [
            NodePathKind::Definition,
            NodePathKind::Compiled,
            NodePathKind::Executable,
        ] {
            assert_eq!(
                metric.get_node_path(kind, in_dir, out_dir).as_ref(),
                Path::new("models/metrics.yml"),
                "kind {kind:?} should fall back to the raw YAML definition path",
            );
        }

        assert_eq!(
            metric
                .get_node_path(NodePathKind::Definition, in_dir, out_dir)
                .as_ref(),
            metric.__common_attr__.path.as_path(),
        );
    }

    fn saved_query_with_paths(name: &str, original_file_path: &str) -> DbtSavedQuery {
        let mut saved_query = DbtSavedQuery::default();
        saved_query.__common_attr__.name = name.to_string();
        saved_query.__common_attr__.package_name = "pkg".to_string();
        saved_query.__common_attr__.path = DbtPath::from(original_file_path);
        saved_query.__common_attr__.original_file_path = DbtPath::from(original_file_path);
        saved_query.__base_attr__.alias = name.to_string();
        saved_query
    }

    #[test]
    fn saved_query_node_paths_are_phase_accurate() {
        let saved_query = saved_query_with_paths("daily_metrics", "models/saved_queries.yml");
        let in_dir = Path::new("/workspace");
        let out_dir = Path::new("/workspace/target");

        for kind in [
            NodePathKind::Definition,
            NodePathKind::Compiled,
            NodePathKind::Executable,
        ] {
            assert_eq!(
                saved_query.get_node_path(kind, in_dir, out_dir).as_ref(),
                Path::new("models/saved_queries.yml"),
                "kind {kind:?} should fall back to the raw YAML definition path",
            );
        }

        assert_eq!(
            saved_query
                .get_node_path(NodePathKind::Definition, in_dir, out_dir)
                .as_ref(),
            saved_query.__common_attr__.path.as_path(),
        );
    }

    fn operation_with_paths(name: &str, original_file_path: &str) -> DbtOperation {
        let mut operation = DbtOperation::default();
        operation.__common_attr__.name = name.to_string();
        operation.__common_attr__.package_name = "pkg".to_string();
        operation.__common_attr__.path = DbtPath::from("hooks").join(format!("{name}.sql"));
        operation.__common_attr__.original_file_path = DbtPath::from(original_file_path);
        operation
    }

    #[test]
    fn operation_node_paths_are_phase_accurate() {
        // on-run-start/on-run-end hooks are declared in dbt_project.yml; every phase reports the
        // raw project file (operations fall through to the definition path in get_node_path).
        let operation = operation_with_paths("on_run_start_0", "./dbt_project.yml");
        let in_dir = Path::new("/workspace");
        let out_dir = Path::new("/workspace/target");

        for kind in [
            NodePathKind::Definition,
            NodePathKind::Compiled,
            NodePathKind::Executable,
        ] {
            assert_eq!(
                operation.get_node_path(kind, in_dir, out_dir).as_ref(),
                Path::new("./dbt_project.yml"),
                "kind {kind:?} should fall back to the raw dbt_project.yml definition path",
            );
        }

        // The compiled artifact path is what the run_operation writer relies on: the hook .sql is
        // nested under the raw dbt_project.yml definition path (many-to-one layout).
        assert_eq!(
            operation.get_node_path_abs(NodePathKind::Compiled, in_dir, out_dir),
            PathBuf::from(
                "/workspace/target/compiled/pkg/dbt_project.yml/hooks/on_run_start_0.sql"
            )
        );
    }

    fn macro_with_paths(name: &str, original_file_path: &str) -> DbtMacro {
        DbtMacro {
            name: name.to_string(),
            package_name: "pkg".to_string(),
            path: DbtPath::from(original_file_path),
            original_file_path: DbtPath::from(original_file_path),
            ..Default::default()
        }
    }

    #[test]
    fn macro_node_paths_are_phase_accurate() {
        let dbt_macro = macro_with_paths("my_macro", "macros/my_macro.sql");
        let in_dir = Path::new("/workspace");
        let out_dir = Path::new("/workspace/target");

        for kind in [
            NodePathKind::Definition,
            NodePathKind::Compiled,
            NodePathKind::Executable,
        ] {
            assert_eq!(
                dbt_macro.get_node_path(kind, in_dir, out_dir).as_ref(),
                Path::new("macros/my_macro.sql"),
                "kind {kind:?} should resolve to the raw .sql definition path",
            );
        }
    }

    #[test]
    fn legacy_sql_snapshot_node_paths_preserve_nested_compiled_layout() {
        let snapshot = snapshot_with_paths(
            "snap_non_matching",
            "snapshots/snap_non_matching.sql",
            "snapshots/snap_collide.sql",
        );
        let in_dir = Path::new("/workspace");
        let out_dir = Path::new("/workspace/target");

        assert_eq!(
            snapshot
                .get_node_path(NodePathKind::Definition, in_dir, out_dir)
                .as_ref(),
            Path::new("snapshots/snap_collide.sql")
        );
        assert_eq!(
            snapshot.get_node_path_abs(NodePathKind::Compiled, in_dir, out_dir),
            PathBuf::from(
                "/workspace/target/compiled/pkg/snapshots/snap_collide.sql/snap_non_matching.sql"
            )
        );
        assert_eq!(
            snapshot
                .get_node_path(NodePathKind::Executable, in_dir, out_dir)
                .as_ref(),
            Path::new("target/run/pkg/snapshots/snap_collide.sql/snap_non_matching.sql")
        );
    }

    /// Two `{% snapshot %}` blocks in one file: the one whose name matches the filename would
    /// take the one-to-one branch of `get_target_write_path` and land on a flat file, while its
    /// sibling needs that same path to be a directory. Both compiled and run paths must nest.
    ///
    /// The same nesting is what dbt-core v1 writes, so a `target/` directory left behind by v1
    /// is reusable by Fusion instead of blowing up with EISDIR (dbt-core#15692).
    #[test]
    fn colliding_snapshots_in_one_file_both_nest_under_the_source_file() {
        let in_dir = Path::new("/workspace");
        let out_dir = Path::new("/workspace/target");

        // Name matches the source filename — the case that used to produce a flat file.
        let matching = snapshot_with_paths("snaps", "snapshots/snaps.sql", "snapshots/snaps.sql");
        // Sibling block in the same file — always took the nested branch.
        let sibling = snapshot_with_paths("other", "snapshots/other.sql", "snapshots/snaps.sql");

        for (snapshot, name) in [(&matching, "snaps"), (&sibling, "other")] {
            assert_eq!(
                snapshot.get_node_path_abs(NodePathKind::Compiled, in_dir, out_dir),
                PathBuf::from(format!(
                    "/workspace/target/compiled/pkg/snapshots/snaps.sql/{name}.sql"
                )),
            );
            assert_eq!(
                snapshot
                    .get_node_path(NodePathKind::Executable, in_dir, out_dir)
                    .as_ref(),
                Path::new(&format!("target/run/pkg/snapshots/snaps.sql/{name}.sql")),
            );
        }
    }

    /// An `alias` config must not move the run artifact: dbt-core derives snapshot write paths
    /// from `name`, never the alias, and `target/run` has to mirror `target/compiled`.
    #[test]
    fn snapshot_run_path_ignores_alias() {
        let mut snapshot =
            snapshot_with_paths("my_snap", "snapshots/my_snap.sql", "snapshots/my_snap.sql");
        snapshot.__base_attr__.alias = "aliased_elsewhere".to_string();

        assert_eq!(
            snapshot
                .get_node_path(
                    NodePathKind::Executable,
                    Path::new("/workspace"),
                    Path::new("/workspace/target")
                )
                .as_ref(),
            Path::new("target/run/pkg/snapshots/my_snap.sql/my_snap.sql")
        );
    }

    #[test]
    fn snapshot_node_evaluated_paths_are_phase_accurate() {
        let in_dir = Path::new("/workspace");
        let out_dir = Path::new("/workspace/target");

        let yaml_snapshot = snapshot_with_paths(
            "daily_orders",
            "snapshots/daily_orders.sql",
            "snapshots/snapshots.yml",
        );
        assert_snapshot_evaluated_paths(
            &yaml_snapshot,
            in_dir,
            out_dir,
            "snapshots/snapshots.yml",
            "target/compiled/pkg/snapshots/snapshots.yml/daily_orders.sql",
            "target/run/pkg/snapshots/snapshots.yml/daily_orders.sql",
        );

        let legacy_sql_snapshot = snapshot_with_paths(
            "snap_non_matching",
            "snapshots/snap_non_matching.sql",
            "snapshots/snap_collide.sql",
        );
        assert_snapshot_evaluated_paths(
            &legacy_sql_snapshot,
            in_dir,
            out_dir,
            "snapshots/snap_collide.sql",
            "target/compiled/pkg/snapshots/snap_collide.sql/snap_non_matching.sql",
            "target/run/pkg/snapshots/snap_collide.sql/snap_non_matching.sql",
        );
    }

    #[test]
    fn snapshot_node_evaluated_definition_paths_include_yaml_location() {
        let in_dir = Path::new("/workspace");
        let out_dir = Path::new("/workspace/target");
        let mut snapshot = snapshot_with_paths(
            "daily_orders",
            "snapshots/daily_orders.sql",
            "snapshots/snapshots.yml",
        );
        snapshot.__common_attr__.name_span = dbt_common::Span {
            start: dbt_common::CodeLocationWithFile::new(2, 5, 18, "snapshots/snapshots.yml"),
            stop: Default::default(),
        };

        for phase in [ExecutionPhase::Parse, ExecutionPhase::Render] {
            let event = snapshot.get_node_evaluated_event(phase, in_dir, out_dir);
            assert_eq!(event.relative_path, "snapshots/snapshots.yml");
            assert_eq!(event.defined_at_line, Some(2));
            assert_eq!(event.defined_at_col, Some(5));
        }

        for phase in [ExecutionPhase::Analyze, ExecutionPhase::Run] {
            let event = snapshot.get_node_evaluated_event(phase, in_dir, out_dir);
            assert_eq!(event.defined_at_line, None);
            assert_eq!(event.defined_at_col, None);
        }
    }

    fn assert_snapshot_evaluated_paths(
        snapshot: &DbtSnapshot,
        in_dir: &Path,
        out_dir: &Path,
        definition_path: &str,
        compiled_path: &str,
        run_path: &str,
    ) {
        for phase in [ExecutionPhase::Parse, ExecutionPhase::Render] {
            assert_eq!(
                snapshot
                    .get_node_evaluated_event(phase, in_dir, out_dir)
                    .relative_path,
                definition_path
            );
        }
        let actual_compiled_path = snapshot
            .get_node_evaluated_event(ExecutionPhase::Analyze, in_dir, out_dir)
            .relative_path;
        assert!(
            path_separator_eq(&actual_compiled_path, compiled_path),
            "left: {actual_compiled_path:?}\nright: {compiled_path:?}"
        );

        let actual_run_path = snapshot
            .get_node_evaluated_event(ExecutionPhase::Run, in_dir, out_dir)
            .relative_path;
        assert!(
            path_separator_eq(&actual_run_path, run_path),
            "left: {actual_run_path:?}\nright: {run_path:?}"
        );
    }

    #[test]
    fn snapshot_serialized_config_defaults_partial_meta_column_names() {
        let meta_column_names = SnapshotMetaColumnNames {
            dbt_valid_from: Some("snapshot_valid_from_custom".to_string()),
            dbt_valid_to: Some("snapshot_valid_to_custom".to_string()),
            ..Default::default()
        };

        let mut snapshot = DbtSnapshot::default();
        snapshot.deprecated_config.snapshot_meta_column_names = Some(meta_column_names.clone());
        snapshot.__snapshot_attr__.snapshot_meta_column_names = meta_column_names;

        let config = snapshot.serialized_config();
        let YmlValue::Mapping(config, _) = config else {
            panic!("expected serialized snapshot config to be a map");
        };
        let meta = config
            .get("snapshot_meta_column_names")
            .expect("snapshot_meta_column_names should be present");
        let legacy = config
            .get("snapshot_table_column_names")
            .expect("snapshot_table_column_names should be present");
        assert_eq!(meta, legacy);

        let YmlValue::Mapping(meta, _) = meta else {
            panic!("expected snapshot_meta_column_names to be a map");
        };
        assert_eq!(
            meta.get("dbt_scd_id").and_then(|v| v.as_str()),
            Some("dbt_scd_id")
        );
        assert_eq!(
            meta.get("dbt_valid_from").and_then(|v| v.as_str()),
            Some("snapshot_valid_from_custom")
        );
        assert_eq!(
            meta.get("dbt_valid_to").and_then(|v| v.as_str()),
            Some("snapshot_valid_to_custom")
        );
        assert_eq!(
            meta.get("dbt_is_deleted").and_then(|v| v.as_str()),
            Some("dbt_is_deleted")
        );
    }

    #[test]
    fn test_hooks_equal_none_vs_empty_array() {
        // Test that None is equal to Some(ArrayOfStrings([]))
        let none_hooks: Verbatim<Option<Hooks>> = Verbatim::from(None);
        let empty_array_hooks: Verbatim<Option<Hooks>> =
            Verbatim::from(Some(Hooks::ArrayOfStrings(vec![])));

        // Test both directions
        assert!(hooks_equal(&none_hooks, &empty_array_hooks));
        assert!(hooks_equal(&empty_array_hooks, &none_hooks));
    }

    #[test]
    fn test_persist_docs_configs_equal_none_vs_empty() {
        // Test that None is equal to Some(PersistDocsConfig { columns: None, relation: None })
        let none_config: Option<PersistDocsConfig> = None;
        let empty_config: Option<PersistDocsConfig> = Some(PersistDocsConfig {
            columns: None,
            relation: None,
        });

        // Test both directions
        assert!(persist_docs_configs_equal(&none_config, &empty_config));
        assert!(persist_docs_configs_equal(&empty_config, &none_config));

        // Also test that two None values are equal
        assert!(persist_docs_configs_equal(&none_config, &none_config));

        // And that two empty configs are equal
        assert!(persist_docs_configs_equal(&empty_config, &empty_config));
    }

    #[test]
    fn test_normalize_description_none_vs_empty_string() {
        let none_desc: Option<String> = None;
        let empty_desc: Option<String> = Some(String::new());

        assert_eq!(
            normalize_description(&none_desc),
            normalize_description(&empty_desc)
        );
    }

    #[test]
    fn test_normalize_description_whitespace_insensitive() {
        let desc_with_spaces = Some("alpha beta".to_string());
        let desc_with_mixed_whitespace = Some("\n  alpha\tbeta \n".to_string());

        assert_eq!(
            normalize_description(&desc_with_spaces),
            normalize_description(&desc_with_mixed_whitespace)
        );
    }

    #[test]
    fn test_quoting_equal_various_cases() {
        use crate::schemas::common::DbtQuoting;

        // Case 1: None vs Some with all fields None
        let none_quoting: Option<DbtQuoting> = None;
        let all_none_quoting: Option<DbtQuoting> = Some(DbtQuoting {
            database: None,
            identifier: None,
            schema: None,
            snowflake_ignore_case: None,
        });

        assert!(quoting_equal(
            &none_quoting,
            &all_none_quoting,
            AdapterType::Snowflake
        ));
        assert!(quoting_equal(
            &all_none_quoting,
            &none_quoting,
            AdapterType::Snowflake
        ));

        // Case 2: None vs Some with all fields Some(false)
        let all_false_quoting: Option<DbtQuoting> = Some(DbtQuoting {
            database: Some(false),
            identifier: Some(false),
            schema: Some(false),
            snowflake_ignore_case: Some(false),
        });

        assert!(quoting_equal(
            &none_quoting,
            &all_false_quoting,
            AdapterType::Snowflake
        ));
        assert!(quoting_equal(
            &all_false_quoting,
            &none_quoting,
            AdapterType::Snowflake
        ));

        // Case 3: Some with all None vs Some with all Some(false)
        assert!(quoting_equal(
            &all_none_quoting,
            &all_false_quoting,
            AdapterType::Snowflake
        ));
        assert!(quoting_equal(
            &all_false_quoting,
            &all_none_quoting,
            AdapterType::Snowflake
        ));

        // Case 4: Mixed None and Some(false) should be equal
        let mixed_quoting_1: Option<DbtQuoting> = Some(DbtQuoting {
            database: Some(false),
            identifier: None,
            schema: Some(false),
            snowflake_ignore_case: None,
        });

        let mixed_quoting_2: Option<DbtQuoting> = Some(DbtQuoting {
            database: None,
            identifier: Some(false),
            schema: None,
            snowflake_ignore_case: Some(false),
        });

        assert!(quoting_equal(
            &mixed_quoting_1,
            &mixed_quoting_2,
            AdapterType::Snowflake
        ));

        // Case 5: Some(true) should NOT be equal to None or Some(false)
        let some_true_quoting: Option<DbtQuoting> = Some(DbtQuoting {
            database: Some(true),
            identifier: Some(false),
            schema: None,
            snowflake_ignore_case: Some(false),
        });

        assert!(!quoting_equal(
            &some_true_quoting,
            &none_quoting,
            AdapterType::Snowflake
        ));
        assert!(!quoting_equal(
            &some_true_quoting,
            &all_none_quoting,
            AdapterType::Snowflake
        ));
        assert!(!quoting_equal(
            &some_true_quoting,
            &all_false_quoting,
            AdapterType::Snowflake
        ));

        // Case 6: Two identical configs with Some(true) should be equal
        let another_true_quoting: Option<DbtQuoting> = Some(DbtQuoting {
            database: Some(true),
            identifier: Some(false),
            schema: None,
            snowflake_ignore_case: Some(false),
        });

        assert!(quoting_equal(
            &some_true_quoting,
            &another_true_quoting,
            AdapterType::Snowflake
        ));
    }

    /// The freshness path moved from passing `&source.deprecated_config` to
    /// `&node.serialized_config()`, which runs the config through `to_value` one
    /// extra time. A source's `config` dict is Jinja-visible while rendering
    /// `loaded_at_query` and `collect_freshness`, and `SourceConfig` carries a
    /// `Verbatim`, an `Omissible`, and a custom `serialize_with`, so the extra hop
    /// has to be a no-op.
    #[test]
    fn source_serialized_config_matches_direct_config_serialization() {
        use dbt_common::serde_utils::Omissible;

        use crate::schemas::common::{FreshnessDefinition, FreshnessPeriod, FreshnessRules};
        use dbt_yaml::Verbatim;

        let cases = [
            // `freshness` omitted entirely, `meta` unset (hits serialize_none_as_empty_map).
            Omissible::Omitted,
            // Explicit `freshness: null`, i.e. opted out.
            Omissible::Present(None),
            Omissible::Present(Some(FreshnessDefinition {
                warn_after: Some(FreshnessRules {
                    count: Some(12),
                    period: Some(FreshnessPeriod::hour),
                }),
                filter: Some("id > 0".to_string()),
                ..Default::default()
            })),
        ];

        for freshness in cases {
            let mut source = DbtSource::default();
            source.deprecated_config.enabled = Some(true);
            source.deprecated_config.meta = None;
            source.deprecated_config.freshness = freshness;
            source.deprecated_config.loaded_at_field = Some("updated_at".to_string());
            source.deprecated_config.loaded_at_query =
                Verbatim::from(Some("select max(updated_at) from {{ this }}".to_string()));

            let direct = dbt_yaml::to_value(&source.deprecated_config).unwrap();
            let via_trait = dbt_yaml::to_value(source.serialized_config()).unwrap();
            assert_eq!(
                direct, via_trait,
                "serialized_config() is not a no-op; the Jinja-visible source config changed shape"
            );
        }
    }

    #[test]
    fn test_alias_config_equal_treats_generated_alias_as_unchanged() {
        let generated =
            Some("dbt_utils_unique_combination_o_f03ffd6673790704c4a0250ea2d96410".to_string());

        assert!(test_alias_config_equal(
            None,
            generated.as_deref(),
            "dbt_utils_unique_combination_o_f03ffd6673790704c4a0250ea2d96410",
            "dbt_utils_unique_combination_o_f03ffd6673790704c4a0250ea2d96410",
        ));
        assert!(test_alias_config_equal(
            generated.as_deref(),
            None,
            "dbt_utils_unique_combination_o_f03ffd6673790704c4a0250ea2d96410",
            "dbt_utils_unique_combination_o_f03ffd6673790704c4a0250ea2d96410",
        ));
    }

    #[test]
    fn test_alias_config_equal_detects_changed_resolved_alias() {
        assert!(!test_alias_config_equal(
            None,
            Some("old_alias"),
            "new_alias",
            "old_alias",
        ));
    }

    #[test]
    fn test_deserialize_wo_meta() {
        let config: YmlValue = dbt_yaml::from_str(
            r#"
            enabled: true
            "#,
        )
        .expect("Failed to deserialize model config");

        let config = ModelConfig::deserialize(config);
        if let Err(err) = config {
            panic!("Could not deserialize and failed with the following error: {err}");
        }
    }

    fn model_with_freshness(freshness: Option<ModelFreshness>) -> DbtModel {
        let mut model = DbtModel::default();
        model.__model_attr__.freshness = freshness;
        model
    }

    #[test]
    fn has_freshness_false_when_unset_or_build_after_only() {
        assert!(!model_with_freshness(None).has_freshness());

        let build_after_only = ModelFreshness {
            build_after: Some(ModelFreshnessRules {
                count: Some(1),
                period: Some(FreshnessPeriod::day),
                updates_on: None,
            }),
            ..Default::default()
        };
        assert!(!model_with_freshness(Some(build_after_only)).has_freshness());
    }

    #[test]
    fn has_freshness_false_when_rule_object_is_empty() {
        let empty_warn = ModelFreshness {
            warn_after: Some(FreshnessRules::default()),
            ..Default::default()
        };
        assert!(!model_with_freshness(Some(empty_warn)).has_freshness());

        let empty_error = ModelFreshness {
            error_after: Some(FreshnessRules::default()),
            ..Default::default()
        };
        assert!(!model_with_freshness(Some(empty_error)).has_freshness());
    }

    #[test]
    fn has_freshness_true_when_warn_after_set() {
        let freshness = ModelFreshness {
            warn_after: Some(FreshnessRules {
                count: Some(24),
                period: Some(FreshnessPeriod::hour),
            }),
            ..Default::default()
        };
        assert!(model_with_freshness(Some(freshness)).has_freshness());
    }

    #[test]
    fn has_freshness_true_when_error_after_set() {
        let freshness = ModelFreshness {
            error_after: Some(FreshnessRules {
                count: Some(48),
                period: Some(FreshnessPeriod::hour),
            }),
            ..Default::default()
        };
        assert!(model_with_freshness(Some(freshness)).has_freshness());
    }

    mod optional_string_vecs_equal_tests {
        use super::super::optional_string_vecs_equal;

        #[test]
        fn test_empty_vecs_are_equal() {
            let a: Vec<Option<String>> = vec![];
            let b: Vec<Option<String>> = vec![];
            assert!(optional_string_vecs_equal(&a, &b));
        }

        #[test]
        fn test_different_lengths_with_content_are_not_equal() {
            // Different lengths with non-empty content should NOT be equal
            let a = vec![Some("a".to_string())];
            let b = vec![Some("a".to_string()), Some("b".to_string())];
            assert!(!optional_string_vecs_equal(&a, &b));
        }

        #[test]
        fn test_empty_vec_vs_vec_of_empty_strings_are_equal() {
            // This is the key fix: empty vec [] should equal vec of all-empty strings
            // Scenario from state:modified false positive:
            // - self_value: "[]" (current manifest has no columns)
            // - other_value: "[Some(\"\"), Some(\"\"), ...]" (state has columns with empty descriptions)
            let current_manifest: Vec<Option<String>> = vec![];
            let state_manifest = vec![
                Some("".to_string()),
                Some("".to_string()),
                Some("".to_string()),
            ];
            assert!(optional_string_vecs_equal(
                &current_manifest,
                &state_manifest
            ));
            assert!(optional_string_vecs_equal(
                &state_manifest,
                &current_manifest
            ));
        }

        #[test]
        fn test_empty_vec_vs_vec_of_none_are_equal() {
            let a: Vec<Option<String>> = vec![];
            let b = vec![None, None, None];
            assert!(optional_string_vecs_equal(&a, &b));
            assert!(optional_string_vecs_equal(&b, &a));
        }

        #[test]
        fn test_empty_vec_vs_vec_of_whitespace_are_equal() {
            let a: Vec<Option<String>> = vec![];
            let b = vec![Some("   ".to_string()), Some("\t\n".to_string())];
            assert!(optional_string_vecs_equal(&a, &b));
            assert!(optional_string_vecs_equal(&b, &a));
        }

        #[test]
        fn test_empty_vec_vs_vec_with_content_are_not_equal() {
            // Empty vec should NOT equal a vec that has actual content
            let a: Vec<Option<String>> = vec![];
            let b = vec![Some("actual description".to_string())];
            assert!(!optional_string_vecs_equal(&a, &b));
            assert!(!optional_string_vecs_equal(&b, &a));
        }

        #[test]
        fn test_mixed_empty_and_content_vs_empty_vec_not_equal() {
            // If the state has some actual content, it should NOT match empty
            let a: Vec<Option<String>> = vec![];
            let b = vec![
                Some("".to_string()),
                Some("primary key".to_string()),
                Some("".to_string()),
            ];
            assert!(!optional_string_vecs_equal(&a, &b));
            assert!(!optional_string_vecs_equal(&b, &a));
        }

        #[test]
        fn test_none_equals_empty_string() {
            let a = vec![None];
            let b = vec![Some("".to_string())];
            assert!(optional_string_vecs_equal(&a, &b));
            assert!(optional_string_vecs_equal(&b, &a));
        }

        #[test]
        fn test_none_equals_whitespace_only() {
            let a = vec![None];
            let b = vec![Some("   ".to_string())];
            assert!(optional_string_vecs_equal(&a, &b));
            assert!(optional_string_vecs_equal(&b, &a));
        }

        #[test]
        fn test_empty_string_equals_whitespace_only() {
            let a = vec![Some("".to_string())];
            let b = vec![Some("   \t\n".to_string())];
            assert!(optional_string_vecs_equal(&a, &b));
        }

        #[test]
        fn test_non_empty_with_different_whitespace_are_equal() {
            // normalize_sql removes all whitespace, so these should be equal
            let a = vec![Some("hello world".to_string())];
            let b = vec![Some("hello  world".to_string())];
            assert!(optional_string_vecs_equal(&a, &b));
        }

        #[test]
        fn test_non_empty_different_content_are_not_equal() {
            let a = vec![Some("foo".to_string())];
            let b = vec![Some("bar".to_string())];
            assert!(!optional_string_vecs_equal(&a, &b));
        }

        #[test]
        fn test_empty_vs_non_empty_are_not_equal() {
            let a = vec![None];
            let b = vec![Some("content".to_string())];
            assert!(!optional_string_vecs_equal(&a, &b));
            assert!(!optional_string_vecs_equal(&b, &a));
        }

        #[test]
        fn test_multiple_elements_one_mismatch() {
            let a = vec![
                Some("same".to_string()),
                Some("different1".to_string()),
                Some("same".to_string()),
            ];
            let b = vec![
                Some("same".to_string()),
                Some("different2".to_string()),
                Some("same".to_string()),
            ];
            assert!(!optional_string_vecs_equal(&a, &b));
        }

        #[test]
        fn test_same_content_different_order_are_equal() {
            // Vectors with same content but different order should be equal
            // This is the column ordering issue from state:modified false positives
            let state_manifest = vec![
                Some("User ID".to_string()),
                Some("Email address".to_string()),
                Some("Created timestamp".to_string()),
            ];
            let current_manifest = vec![
                Some("Email address".to_string()),
                Some("Created timestamp".to_string()),
                Some("User ID".to_string()),
            ];
            assert!(optional_string_vecs_equal(
                &state_manifest,
                &current_manifest
            ));
            assert!(optional_string_vecs_equal(
                &current_manifest,
                &state_manifest
            ));
        }

        #[test]
        fn test_same_content_different_order_with_normalization() {
            // Same content, different order, with whitespace differences that get normalized
            let a = vec![
                Some("hello world".to_string()),
                Some("foo bar".to_string()),
                Some("test".to_string()),
            ];
            let b = vec![
                Some("test".to_string()),
                Some("hello  world".to_string()), // extra space - should normalize to same
                Some("foo bar".to_string()),
            ];
            assert!(optional_string_vecs_equal(&a, &b));
        }

        #[test]
        fn test_same_content_different_order_with_empty_values() {
            // Same content with some empty values, different order
            let a = vec![
                Some("description1".to_string()),
                None,
                Some("description2".to_string()),
                Some("".to_string()),
            ];
            let b = vec![
                Some("".to_string()),
                Some("description2".to_string()),
                None,
                Some("description1".to_string()),
            ];
            assert!(optional_string_vecs_equal(&a, &b));
        }

        #[test]
        fn test_different_content_same_length_not_equal() {
            // Different content should NOT be equal even with sorting
            let a = vec![
                Some("alpha".to_string()),
                Some("beta".to_string()),
                Some("gamma".to_string()),
            ];
            let b = vec![
                Some("alpha".to_string()),
                Some("beta".to_string()),
                Some("delta".to_string()), // different from gamma
            ];
            assert!(!optional_string_vecs_equal(&a, &b));
        }

        #[test]
        fn test_real_world_column_ordering_issue() {
            // Real-world scenario from DocuSign telemetry:
            // State has columns from catalog in one order, current has them in YAML order
            let other_value = vec![
                Some("Unique identifier for the AAD group".to_string()),
                Some("Unique identifier for the organizational entity".to_string()),
                Some("Timestamp when created".to_string()),
                Some("The user who created".to_string()),
            ];
            let self_value = vec![
                Some("Unique identifier for the AAD group".to_string()),
                Some("Unique identifier for the organizational entity".to_string()),
                Some("The user who created".to_string()), // swapped order
                Some("Timestamp when created".to_string()), // swapped order
            ];
            assert!(optional_string_vecs_equal(&other_value, &self_value));
        }

        #[test]
        fn test_real_world_column_descriptions() {
            // Simulates the real issue: duplicate column definitions with different descriptions
            let current = vec![
                Some("Primary key".to_string()),
                Some("The order in which this user was created under the carrier.".to_string()),
            ];
            let previous = vec![
                Some("Primary key".to_string()),
                Some("The order in which this user was created under the carrier.".to_string()),
            ];
            assert!(optional_string_vecs_equal(&current, &previous));
        }

        #[test]
        fn test_duplicate_empty_strings_are_equal() {
            // This tests the "duplicate columns" issue from NBIM telemetry
            // State (other_value) has duplicate empty strings for multiple columns
            // Current (self_value) has fewer empty strings
            // Both should be equal because the unique content is the same (all empty)
            let other_value = vec![
                Some("".to_string()),
                Some("".to_string()),
                Some("".to_string()),
                Some("".to_string()),
                Some("".to_string()),
            ];
            let self_value = vec![Some("".to_string()), Some("".to_string())];
            assert!(optional_string_vecs_equal(&other_value, &self_value));
            assert!(optional_string_vecs_equal(&self_value, &other_value));
        }

        #[test]
        fn test_duplicate_content_with_different_lengths_are_equal() {
            // If the longer vector only has duplicates of content in the shorter,
            // they should be equal
            let shorter = vec![Some("a".to_string()), Some("b".to_string())];
            let longer = vec![
                Some("a".to_string()),
                Some("b".to_string()),
                Some("a".to_string()), // duplicate
                Some("b".to_string()), // duplicate
            ];
            assert!(optional_string_vecs_equal(&shorter, &longer));
            assert!(optional_string_vecs_equal(&longer, &shorter));
        }

        #[test]
        fn test_extra_empty_values_in_longer_are_equal() {
            // Extra None/empty values in the longer vector should not cause inequality
            // This is the key fix for the duplicate columns issue where state has extra empty descriptions
            let shorter = vec![Some("a".to_string()), Some("b".to_string())];
            let longer = vec![Some("a".to_string()), Some("b".to_string()), None];
            assert!(optional_string_vecs_equal(&shorter, &longer));
            assert!(optional_string_vecs_equal(&longer, &shorter));

            // Also test with multiple extra empty values
            let longer_multiple = vec![
                Some("a".to_string()),
                Some("b".to_string()),
                None,
                Some("".to_string()),
                Some("   ".to_string()), // whitespace-only
            ];
            assert!(optional_string_vecs_equal(&shorter, &longer_multiple));
            assert!(optional_string_vecs_equal(&longer_multiple, &shorter));
        }

        #[test]
        fn test_longer_with_new_content_not_equal() {
            // If the longer vector has content not in the shorter, they should NOT be equal
            let shorter = vec![Some("a".to_string()), Some("b".to_string())];
            let longer = vec![
                Some("a".to_string()),
                Some("b".to_string()),
                Some("c".to_string()), // NEW content not in shorter
            ];
            assert!(!optional_string_vecs_equal(&shorter, &longer));
            assert!(!optional_string_vecs_equal(&longer, &shorter));
        }

        #[test]
        fn test_real_world_duplicate_empty_column_descriptions() {
            // Real scenario from NBIM telemetry:
            // State has many columns with empty descriptions (duplicates)
            // Current has fewer columns but all descriptions are also empty
            let state_manifest = vec![
                None,
                Some("".to_string()),
                None,
                Some("  ".to_string()), // whitespace only
                Some("".to_string()),
                None,
                Some("".to_string()),
            ];
            let current_manifest = vec![Some("".to_string()), None, Some("   ".to_string())];
            assert!(optional_string_vecs_equal(
                &state_manifest,
                &current_manifest
            ));
            assert!(optional_string_vecs_equal(
                &current_manifest,
                &state_manifest
            ));
        }
    }
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct DbtAnalysis {
    pub __common_attr__: CommonAttributes,

    pub __base_attr__: NodeBaseAttributes,

    pub __analysis_attr__: DbtAnalysisAttr,

    /// To be deprecated
    #[serde(rename = "config")]
    pub deprecated_config: super::project::configs::analysis_config::AnalysesConfig,

    pub __other__: BTreeMap<String, YmlValue>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct DbtAnalysisAttr {
    // Analysis nodes have minimal attributes since they're for ad-hoc querying
    // Most functionality comes from the base attributes and config
}

impl InternalDbtNode for DbtAnalysis {
    fn common(&self) -> &CommonAttributes {
        &self.__common_attr__
    }

    fn base(&self) -> &NodeBaseAttributes {
        &self.__base_attr__
    }

    fn base_mut(&mut self) -> &mut NodeBaseAttributes {
        &mut self.__base_attr__
    }

    fn common_mut(&mut self) -> &mut CommonAttributes {
        &mut self.__common_attr__
    }

    fn resource_type(&self) -> NodeType {
        NodeType::Analysis
    }

    /// Analysis-phase errors on an analysis cite its compiled SQL under `target/compiled/`.
    fn error_path_kind(&self, phase: ExecutionPhase) -> NodePathKind {
        phase.into()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn serialize_inner(
        &self,
        mode: crate::schemas::serialization_utils::SerializationMode,
    ) -> YmlValue {
        crate::schemas::serialization_utils::serialize_with_mode(self, mode)
    }

    fn has_same_config(&self, other: &dyn InternalDbtNode, _adapter_type: AdapterType) -> bool {
        if let Some(other_analysis) = other.as_any().downcast_ref::<DbtAnalysis>() {
            let self_config = &self.deprecated_config;
            let other_config = &other_analysis.deprecated_config;

            let enabled_eq = self_config.enabled == other_config.enabled;
            // `static_analysis` is a Fusion-only, invocation-driven value (e.g. set by
            // `--static-analysis`) with no dbt-core equivalent, so it can never be a
            // legitimate dbt-core `state:modified` trigger and is deliberately excluded
            // from this comparison (see `base_config_excluded_keys`, parity-exclude).

            let result = enabled_eq;

            if !result {
                log_state_mod_diff(
                    &self.__common_attr__.unique_id,
                    "analysis_config",
                    [(
                        "enabled",
                        enabled_eq,
                        Some((
                            format!("{:?}", &self_config.enabled),
                            format!("{:?}", &other_config.enabled),
                        )),
                    )],
                );
            }

            result
        } else {
            false
        }
    }

    fn has_same_content(&self, other: &dyn InternalDbtNode, _adapter_type: AdapterType) -> bool {
        if let Some(other_analysis) = other.as_any().downcast_ref::<DbtAnalysis>() {
            let same = self.__common_attr__.checksum == other_analysis.__common_attr__.checksum;
            if !same {
                log_state_mod_diff(
                    &self.__common_attr__.unique_id,
                    "analysis_content",
                    [(
                        "content",
                        false,
                        Some((
                            self.__common_attr__.checksum.to_checksum_string(),
                            other_analysis.__common_attr__.checksum.to_checksum_string(),
                        )),
                    )],
                );
            }
            same
        } else {
            false
        }
    }

    fn set_detected_introspection(&mut self, _introspection: IntrospectionKind) {
        panic!("DbtAnalysis does not support setting detected_unsafe");
    }
}

impl InternalDbtNodeAttributes for DbtAnalysis {
    fn static_analysis(&self) -> Spanned<StaticAnalysisKind> {
        self.__base_attr__.static_analysis.clone()
    }

    fn set_quoting(&mut self, quoting: ResolvedQuoting) {
        self.__base_attr__.quoting = quoting;
    }

    fn set_static_analysis(&mut self, static_analysis: Spanned<StaticAnalysisKind>) {
        self.__base_attr__.static_analysis = static_analysis;
    }

    fn get_access(&self) -> Option<Access> {
        None // Analysis nodes don't have access controls
    }

    fn search_name(&self) -> String {
        self.__common_attr__.name.clone()
    }

    fn selector_string(&self) -> String {
        format!("analysis:{}", self.__common_attr__.name)
    }

    fn serialized_config(&self) -> YmlValue {
        dbt_yaml::to_value(&self.deprecated_config).expect("Failed to serialize to YAML")
    }
}
// Saved queries don't have a relation, individual exports do.
pub fn is_invalid_for_relation_comparison(node: &dyn InternalDbtNode) -> bool {
    node.resource_type() == NodeType::UnitTest || node.resource_type() == NodeType::SavedQuery
}

#[cfg(test)]
mod seed_has_same_content_tests {
    use std::path::PathBuf;

    use dbt_adapter_core::AdapterType;
    use dbt_common::path::DbtPath;

    use crate::schemas::common::{DbtChecksum, DbtChecksumObject};
    use crate::schemas::nodes::{CommonAttributes, DbtSeed, DbtSeedAttr, InternalDbtNode};

    // ─────────────────────────────────────────────────────────────────
    // Helpers
    // ─────────────────────────────────────────────────────────────────

    fn sha256(digest: &str) -> DbtChecksum {
        DbtChecksum::Object(DbtChecksumObject {
            name: "sha256".to_string(),
            checksum: digest.to_string(),
        })
    }

    fn path_checksum(path: &str) -> DbtChecksum {
        DbtChecksum::Object(DbtChecksumObject {
            name: "path".to_string(),
            checksum: path.to_string(),
        })
    }

    /// Build a minimal `DbtSeed` with the supplied checksum. All other
    /// fields that `has_same_content` inspects (fqn, config, description)
    /// are left at their `Default` so paired seeds compare equal on every
    /// dimension except the one being tested.
    fn seed_with_checksum(checksum: DbtChecksum) -> DbtSeed {
        DbtSeed {
            __common_attr__: CommonAttributes {
                checksum,
                unique_id: "seed.test.demo_seed".to_string(),
                name: "demo_seed".to_string(),
                package_name: "test".to_string(),
                fqn: vec!["test".to_string(), "demo_seed".to_string()],
                path: DbtPath::from("seeds/demo_seed.csv"),
                original_file_path: DbtPath::from("seeds/demo_seed.csv"),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// Like `seed_with_checksum` but also sets `root_path` on `__seed_attr__`
    /// so the legacy-fallback code-path can locate the file.
    fn seed_with_checksum_and_root(checksum: DbtChecksum, root: PathBuf) -> DbtSeed {
        DbtSeed {
            __common_attr__: CommonAttributes {
                checksum,
                unique_id: "seed.test.demo_seed".to_string(),
                name: "demo_seed".to_string(),
                package_name: "test".to_string(),
                fqn: vec!["test".to_string(), "demo_seed".to_string()],
                path: DbtPath::from("seeds/demo_seed.csv"),
                original_file_path: DbtPath::from("seeds/demo_seed.csv"),
                ..Default::default()
            },
            __seed_attr__: DbtSeedAttr {
                root_path: Some(root),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    // ─────────────────────────────────────────────────────────────────
    // has_same_content — body (checksum) comparisons
    // ─────────────────────────────────────────────────────────────────

    #[test]
    fn same_sha256_is_not_modified() {
        let current = seed_with_checksum(sha256("abc123"));
        let previous = seed_with_checksum(sha256("abc123"));
        // Identical content hash → NOT state:modified
        assert!(current.has_same_content(&previous as &dyn InternalDbtNode, AdapterType::DuckDB));
    }

    #[test]
    fn different_sha256_is_modified() {
        let current = seed_with_checksum(sha256("abc123"));
        let previous = seed_with_checksum(sha256("def456"));
        // Different content hash → IS state:modified
        assert!(!current.has_same_content(&previous as &dyn InternalDbtNode, AdapterType::DuckDB));
    }

    #[test]
    fn same_path_checksum_is_not_modified() {
        // Both sides use path-based checksum (seed exceeds the limit on both runs).
        // Same path → NOT modified (dbt-core's same_seeds behaviour for oversized seeds).
        let current = seed_with_checksum(path_checksum("seeds/demo_seed.csv"));
        let previous = seed_with_checksum(path_checksum("seeds/demo_seed.csv"));
        assert!(current.has_same_content(&previous as &dyn InternalDbtNode, AdapterType::DuckDB));
    }

    #[test]
    fn different_path_checksums_is_modified() {
        // Seed moved to a different path → IS modified.
        let current = seed_with_checksum(path_checksum("seeds/demo_seed.csv"));
        let previous = seed_with_checksum(path_checksum("seeds/old_seed.csv"));
        assert!(!current.has_same_content(&previous as &dyn InternalDbtNode, AdapterType::DuckDB));
    }

    #[test]
    fn sha256_vs_path_is_modified() {
        // Checksum *type* changed (e.g. limit was raised so the current run
        // now hashes content, but the previous manifest stored a path checksum).
        // The checksums can never be equal → IS modified.
        let current = seed_with_checksum(sha256("abc123"));
        let previous = seed_with_checksum(path_checksum("seeds/demo_seed.csv"));
        assert!(!current.has_same_content(&previous as &dyn InternalDbtNode, AdapterType::DuckDB));
    }

    #[test]
    fn downcast_fails_for_non_seed_returns_false() {
        use crate::schemas::nodes::DbtModel;
        let current = seed_with_checksum(sha256("abc123"));
        let non_seed = DbtModel::default();
        assert!(!current.has_same_content(&non_seed as &dyn InternalDbtNode, AdapterType::DuckDB));
    }

    // ─────────────────────────────────────────────────────────────────
    // Legacy-fallback
    // ─────────────────────────────────────────────────────────────────

    /// If the new text-mode hash (CRLF→LF normalised) differs
    /// from the old binary-mode hash stored in a previous-state manifest,
    /// `has_same_content` re-hashes the file with the legacy method and
    /// accepts it as "unchanged" when the legacy hash matches.
    #[test]
    fn legacy_fallback_accepts_unchanged_crlf_seed() {
        use std::io::Write;

        // Write a CRLF seed to a temp file.
        let dir = std::env::temp_dir().join(format!(
            "dbt_seed_legacy_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        ));
        let seed_path = dir.join("seeds").join("demo_seed.csv");
        std::fs::create_dir_all(seed_path.parent().unwrap()).unwrap();
        {
            let mut f = std::fs::File::create(&seed_path).unwrap();
            f.write_all(b"id,name\r\n1,Alice\r\n2,Bob\r\n").unwrap();
        }

        // Simulate "previous-state manifest produced by the old binary-mode hasher".
        let legacy_checksum = DbtChecksum::seed_content_checksum_legacy(
            &std::fs::read(&seed_path).expect("failed to read seed file"),
        );

        // Simulate "current run uses the new text-mode hasher (CRLF→LF)".
        // These two hashes differ for CRLF content.
        let new_checksum = DbtChecksum::seed_content_checksum(std::io::BufReader::new(
            std::fs::File::open(&seed_path).unwrap(),
        ))
        .expect("new hash failed");

        // Confirm the premise: the two hashes differ for CRLF content.
        assert_ne!(
            new_checksum, legacy_checksum,
            "expected CRLF hashes to differ between text-mode and binary-mode"
        );

        // current = newly hashed (text-mode), previous = old manifest (legacy)
        let current = seed_with_checksum_and_root(new_checksum, dir.clone());
        let previous = seed_with_checksum(legacy_checksum);

        // The fallback should re-hash with legacy and accept the seed as unchanged.
        assert!(
            current.has_same_content(&previous as &dyn InternalDbtNode, AdapterType::DuckDB),
            "legacy fallback should treat an unchanged CRLF seed as not-modified"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Cross-platform guard: if both hashes already agree (LF-only content or
    /// non-Windows platform), the fallback must not spuriously change the result.
    #[test]
    fn legacy_fallback_does_not_change_result_when_seeds_truly_differ() {
        // Use in-memory seeds — no filesystem needed.
        // current has sha256("abc"), previous has sha256("xyz"): a genuine change.
        let current = seed_with_checksum(sha256("aaaa"));
        let previous = seed_with_checksum(sha256("bbbb"));
        // Even if the fallback were to run, it cannot conjure a matching hash —
        // `root_path` is None so the filesystem code is skipped entirely.
        assert!(!current.has_same_content(&previous as &dyn InternalDbtNode, AdapterType::DuckDB));
    }

    /// Guards the coordination between the two data-test config comparisons in
    /// `check_configs_modified`, which encode the SAME dbt-core `TestConfig` modifier set in two
    /// different representations that can silently drift:
    ///   * Stage 1 (unrendered) — `unrendered_configs_eq` iterates `DBTTEST_CONFIG_MODIFIERS`
    ///     mechanically, so it stays in sync by construction.
    ///   * Stage 2 (rendered)   — `DbtTest::has_same_config` compares typed config fields BY HAND;
    ///     nothing links it back to the constant, so adding/removing a modifier in one place without
    ///     the other is an easy mistake.
    ///
    /// For every data-test config key, assert that Stage 2 treats the key as a change IFF the key is
    /// in `DBTTEST_CONFIG_MODIFIERS`. This is bidirectional: it fails on an add/remove in EITHER
    /// representation (a modifier only in the constant, or a field compared only in Stage 2).
    #[test]
    #[allow(clippy::type_complexity)]
    fn data_test_modifier_set_agrees_across_stage1_and_stage2() {
        use super::{DBTTEST_CONFIG_MODIFIERS, DataTestConfig, DbtTest};
        use crate::schemas::common::{
            DbtMaterialization, DbtQuoting, Severity, StoreFailuresAs, UpdatesOn,
        };
        use crate::schemas::properties::DataTestState;
        use crate::schemas::serde::StringOrArrayOfStrings;

        // Each case mutates exactly one config field away from its default so that Stage 2's
        // rendered comparison can observe it. `expected` is derived from the Stage-1 allowlist, so
        // this table never hard-codes the modifier/non-modifier split — it only has to exercise the
        // field. When you add a field to `DataTestConfig`, add a case here too; the coverage check
        // below fails if any `DBTTEST_CONFIG_MODIFIERS` key is left unexercised.
        let cases: Vec<(&str, Box<dyn Fn(&mut DataTestConfig)>)> = vec![
            // --- dbt-core `TestConfig` modifiers: MUST count as a change ---
            ("severity", Box::new(|c| c.severity = Some(Severity::Warn))),
            ("where", Box::new(|c| c.where_ = Some("id > 0".to_string()))),
            ("limit", Box::new(|c| c.limit = Some(10))),
            (
                "fail_calc",
                Box::new(|c| c.fail_calc = Some("count(*)".to_string())),
            ),
            (
                "warn_if",
                Box::new(|c| c.warn_if = Some("!= 0".to_string())),
            ),
            (
                "error_if",
                Box::new(|c| c.error_if = Some("!= 0".to_string())),
            ),
            (
                "store_failures",
                Box::new(|c| c.store_failures = Some(true)),
            ),
            (
                "store_failures_as",
                Box::new(|c| c.store_failures_as = Some(StoreFailuresAs::Table)),
            ),
            (
                "sql_header",
                Box::new(|c| c.sql_header = Some("set x = 1".to_string())),
            ),
            // --- non-modifiers: dbt-core `TestConfig` ignores these, so they must NOT count ---
            ("enabled", Box::new(|c| c.enabled = Some(false))),
            (
                "alias",
                Box::new(|c| c.alias = Some("an_alias".to_string())),
            ),
            (
                "database",
                Box::new(|c| c.database = Some("a_db".to_string())),
            ),
            ("group", Box::new(|c| c.group = Some("a_group".to_string()))),
            (
                "tags",
                Box::new(|c| {
                    c.tags = crate::schemas::project::configs::config_merge::Tags(Some(
                        StringOrArrayOfStrings::String("a_tag".to_string()),
                    ))
                }),
            ),
            ("full_refresh", Box::new(|c| c.full_refresh = Some(true))),
            (
                "materialized",
                Box::new(|c| c.materialized = Some(DbtMaterialization::View)),
            ),
            (
                "quoting",
                Box::new(|c| c.quoting = Some(DbtQuoting::default())),
            ),
            (
                "meta",
                Box::new(|c| {
                    let mut m = indexmap::IndexMap::new();
                    m.insert("owner".to_string(), dbt_yaml::to_value("bob").unwrap());
                    c.meta = Some(m);
                }),
            ),
            // `state` is honored at run time but is deliberately not a `state:modified` modifier
            // (see the note on `DBTTEST_CONFIG_MODIFIERS`); exercised here to pin it as a non-modifier.
            (
                "state",
                Box::new(|c| {
                    c.state = Some(DataTestState {
                        require_fresh_data_from: Some(UpdatesOn::All),
                        evaluate_volatile_sql: Some(true),
                        compare_unrendered_code: None,
                    })
                }),
            ),
        ];

        // Coverage: every Stage-1 modifier must be exercised, else a modifier could drift untested.
        for key in DBTTEST_CONFIG_MODIFIERS {
            assert!(
                cases.iter().any(|(k, _)| k == key),
                "DBTTEST_CONFIG_MODIFIERS key `{key}` is not exercised by this test; add a case."
            );
        }

        for (key, mutate) in &cases {
            let base = DbtTest::default();
            let mut mutated = base.clone();
            mutate(&mut mutated.deprecated_config);

            // `has_same_config` returns false when Stage 2 considers the config changed.
            let stage2_is_modifier =
                !base.has_same_config(&mutated as &dyn InternalDbtNode, AdapterType::DuckDB);
            let stage1_is_modifier = DBTTEST_CONFIG_MODIFIERS.contains(key);

            assert_eq!(
                stage2_is_modifier, stage1_is_modifier,
                "Data-test config key `{key}` disagrees between the two `state:modified` config \
                 comparisons: Stage 1 (`DBTTEST_CONFIG_MODIFIERS`, unrendered) says \
                 modifier={stage1_is_modifier}, but Stage 2 (`DbtTest::has_same_config`, rendered) \
                 says modifier={stage2_is_modifier}. The two must agree. To reconcile: if `{key}` \
                 should count as a data-test change, add it to BOTH `DBTTEST_CONFIG_MODIFIERS` and \
                 the field comparisons in `DbtTest::has_same_config`; if it should not, remove it \
                 from both (both live in nodes.rs)."
            );
        }
    }

    /// Ignored marker: data-test `state` is not a `state:modified` modifier today because dbt-core's
    /// `TestConfig.same_contents` does not compare it. It asserts the promoted behavior (`state` in
    /// `DBTTEST_CONFIG_MODIFIERS`), which does not hold yet. If dbt-core starts comparing these fields,
    /// add `state` to the modifier set and `DbtTest::has_same_config`, then un-ignore this test.
    #[test]
    #[ignore = "conformance-gated: dbt-core TestConfig does not compare data-test state configs"]
    fn data_test_state_not_promoted_to_modifier_conformance_marker() {
        assert!(
            super::DBTTEST_CONFIG_MODIFIERS.contains(&"state"),
            "data-test `state` is not a `state:modified` modifier; see the note on \
             DBTTEST_CONFIG_MODIFIERS for the conformance rationale."
        );
    }
}

/// Tolerance tests for the three comparators behind `DbtModel::same_ref_representation`
/// (dbt-core's `ModelNode.same_ref_representation`, dbt-mantle
/// `core/dbt/contracts/graph/nodes.py:684-691`). Each tolerance exists because the previous
/// manifest can come from a producer we do not control (real dbt-core, an older Fusion), so naive
/// `==` on the three fields would report phantom modifications.
#[cfg(test)]
mod model_same_ref_representation_tests {
    use super::{
        Access, DbtModel, InternalDbtNode, StringOrInteger, deprecation_date_eq, latest_version_eq,
    };
    use dbt_adapter_core::AdapterType;

    fn model() -> DbtModel {
        let mut model = DbtModel::default();
        model.__common_attr__.unique_id = "model.test.table_model.v1".to_string();
        model.__common_attr__.name = "table_model".to_string();
        model.__common_attr__.package_name = "test".to_string();
        model
    }

    fn same_content(a: &DbtModel, b: &DbtModel) -> bool {
        a.has_same_content(b as &dyn InternalDbtNode, AdapterType::Snowflake)
    }

    // --- comparator 1: `latest_version` (StringOrInteger, structurally compared) ---

    /// All four cross combinations of the two spellings of version `1`. Real dbt-core writes a JSON
    /// number; a hand-authored fixture or an older Fusion manifest can carry a string.
    #[test]
    fn latest_version_eq_normalizes_across_string_and_integer() {
        let int = || Some(StringOrInteger::Integer(1));
        let str_ = || Some(StringOrInteger::String("1".to_string()));

        assert!(latest_version_eq(&int(), &int()));
        assert!(latest_version_eq(&str_(), &str_()));
        assert!(
            latest_version_eq(&int(), &str_()),
            "`Integer(1)` and `String(\"1\")` are the same version; \
             `StringOrInteger` only derives `PartialEq` structurally"
        );
        assert!(latest_version_eq(&str_(), &int()));
    }

    #[test]
    fn latest_version_eq_still_sees_a_real_bump() {
        assert!(!latest_version_eq(
            &Some(StringOrInteger::Integer(2)),
            &Some(StringOrInteger::Integer(1))
        ));
        assert!(
            !latest_version_eq(
                &Some(StringOrInteger::Integer(2)),
                &Some(StringOrInteger::String("1".to_string()))
            ),
            "the normalization must not swallow a genuine version change"
        );
    }

    #[test]
    fn latest_version_eq_handles_absence() {
        assert!(latest_version_eq(&None, &None));
        assert!(!latest_version_eq(
            &Some(StringOrInteger::Integer(1)),
            &None
        ));
        assert!(!latest_version_eq(
            &None,
            &Some(StringOrInteger::Integer(1))
        ));
    }

    // --- comparator 3: `deprecation_date` (date semantics, not string equality) ---

    /// Fusion normalizes an authored date to RFC 3339 on the parse side, so the authored and
    /// manifest spellings of one date routinely differ.
    #[test]
    fn deprecation_date_eq_tolerates_authored_vs_normalized_spelling() {
        assert!(deprecation_date_eq(
            &Some("2099-01-01".to_string()),
            &Some("2099-01-01T00:00:00+00:00".to_string())
        ));
        assert!(deprecation_date_eq(
            &Some("2099-01-01T00:00:00Z".to_string()),
            &Some("2099-01-01T00:00:00+00:00".to_string())
        ));
    }

    /// dbt-core's `normalize_date` re-interprets a naive authored date in the **system** time zone,
    /// while Fusion reads it as UTC — so the same authored `2020-01-01` reaches this comparator as
    /// two different offsets whenever the previous manifest was produced by dbt-core outside UTC.
    #[test]
    fn deprecation_date_eq_tolerates_a_mantle_local_offset() {
        assert!(deprecation_date_eq(
            &Some("2020-01-01T00:00:00+00:00".to_string()),
            &Some("2020-01-01T00:00:00-05:00".to_string())
        ));
    }

    /// dbt-core compares `datetime` objects, so two spellings of one instant are not a change.
    #[test]
    fn deprecation_date_eq_tolerates_equal_instants() {
        assert!(deprecation_date_eq(
            &Some("2020-01-01T05:00:00+00:00".to_string()),
            &Some("2020-01-01T00:00:00-05:00".to_string())
        ));
    }

    #[test]
    fn deprecation_date_eq_still_sees_a_real_change() {
        assert!(!deprecation_date_eq(
            &Some("2099-01-01".to_string()),
            &Some("2098-01-01".to_string())
        ));
        assert!(!deprecation_date_eq(&Some("2099-01-01".to_string()), &None));
        assert!(!deprecation_date_eq(&None, &Some("2099-01-01".to_string())));
        assert!(deprecation_date_eq(&None, &None));
    }

    /// Unparseable values fall back to string equality rather than comparing equal by accident.
    #[test]
    fn deprecation_date_eq_falls_back_to_string_equality() {
        assert!(deprecation_date_eq(
            &Some("not-a-date".to_string()),
            &Some("not-a-date".to_string())
        ));
        assert!(!deprecation_date_eq(
            &Some("not-a-date".to_string()),
            &Some("2099-01-01".to_string())
        ));
    }

    // --- the conjunct as wired into `has_same_content` ---

    #[test]
    fn latest_version_change_is_state_modified() {
        let mut current = model();
        current.__model_attr__.version = Some(StringOrInteger::Integer(1));
        current.__model_attr__.latest_version = Some(StringOrInteger::Integer(2));
        let mut previous = current.clone();
        previous.__model_attr__.latest_version = Some(StringOrInteger::Integer(1));

        assert!(
            !same_content(&current, &previous),
            "a `latest_version` bump changes what an unpinned `ref()` resolves to; \
             dbt-core's `same_ref_representation` reports it as modified"
        );
    }

    #[test]
    fn access_change_is_state_modified() {
        let current = model();
        let mut previous = current.clone();
        previous.__model_attr__.access = Access::Public;

        assert!(
            !same_content(&current, &previous),
            "the `access` node attribute is compared by `same_ref_representation`, \
             independently of the `access` config key owned by `check_configs_modified`"
        );
    }

    #[test]
    fn deprecation_date_change_is_state_modified() {
        let mut current = model();
        current.__model_attr__.deprecation_date = Some("2099-01-01T00:00:00+00:00".to_string());
        let previous = model();

        assert!(
            !same_content(&current, &previous),
            "adding a `deprecation_date` is a modification"
        );
    }

    /// The migration-path shape: a previous manifest that omits `access`, `latest_version` and
    /// `deprecation_date` deserializes to the same defaults the current node carries, so nothing
    /// here may report modified. `manifest_model_to_dbt_model` applies dbt-core's `Protected`
    /// default when neither `config.access` nor the node-level `access` is present, which is why
    /// the node-attribute comparison needs no `access_eq`-style tolerance of its own.
    #[test]
    fn sparse_previous_manifest_is_not_state_modified() {
        let current = model();
        let previous = model();

        assert_eq!(current.__model_attr__.access, Access::Protected);
        assert!(current.__model_attr__.latest_version.is_none());
        assert!(current.__model_attr__.deprecation_date.is_none());
        assert!(same_content(&current, &previous));
    }
}
