//! This module contains the scope guard for resolving models.

use dbt_adapter_core::AdapterType;
use indexmap::IndexMap;
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Debug,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{Arc, Mutex, atomic::AtomicBool},
};

use chrono::TimeZone;
use chrono_tz::{Europe::London, Tz};
use dbt_adapter::{cast_util::THIS_RELATION_KEY, load_store::ResultStore};
use dbt_common::{
    io_args::StaticAnalysisKind, path::DbtPath, serde_utils::convert_yml_to_value_map,
};
use dbt_frontend_common::error::CodeLocation;
use dbt_schemas::schemas::{
    DbtModelAttr, InternalDbtNode, IntrospectionKind,
    common::{Access, ResolvedQuoting},
    nodes::AdapterAttr,
    project::{ModelConfig, ResolvableConfig},
};
use dbt_schemas::{
    dbt_types::RelationType,
    schemas::{
        CommonAttributes, DbtModel, NodeBaseAttributes,
        common::{DbtChecksum, DbtQuoting, NodeDependsOn},
        serde::{NodeVersion, yml_value_to_minijinja},
    },
    state::DbtRuntimeConfig,
};
use minijinja::{
    Error as MinijinjaError, ErrorKind as MinijinjaErrorKind, State,
    arg_utils::ArgParser,
    constants::{CURRENT_PATH, CURRENT_SPAN, TARGET_UNIQUE_ID},
    listener::RenderingEventListener,
    machinery::Span,
    value::{
        Enumerator, Object, ObjectRepr, Value as MinijinjaValue, ValueKind,
        function_object::FunctionObject,
    },
};
use minijinja_contrib::modules::{py_datetime::datetime::PyDateTime, pytz::PytzTimezone};
use serde::Serialize;

use dbt_jinja_ctx::{
    JinjaObject, ParseExecute, ResolveModelCtx, to_jinja_btreemap, to_model_context_map,
};

use crate::{
    invocation_graph::invocation_graph, phases::MacroLookupContext, serde::into_typed_with_error,
};

use super::sql_resource::SqlResource;

/// Builds a context for resolving models
#[allow(clippy::too_many_arguments)]
pub fn build_resolve_model_context<T: ResolvableConfig<T> + Serialize + 'static>(
    config: &T,
    root_overlay_forces_enabled: bool,
    adapter_type: AdapterType,
    database: &str,
    schema: &str,
    model_name: &str,
    fqn: Vec<String>,
    package_name: &str,
    root_project_name: &str,
    package_quoting: DbtQuoting,
    runtime_config: Arc<DbtRuntimeConfig>,
    root_runtime_config: Arc<DbtRuntimeConfig>,
    sql_resources: Arc<Mutex<Vec<SqlResource<T>>>>,
    execute_exists: Arc<AtomicBool>,
    display_path: &Path,
    model_path: &Path,
    global_static_analysis: Option<StaticAnalysisKind>,
) -> BTreeMap<String, MinijinjaValue> {
    // Create a relation for 'this' using config values
    let sql_resources_clone = sql_resources.clone();
    let this_relation = ResolveThisFunction {
        relation: dbt_adapter::relation::RelationObject::new(Arc::from(
            dbt_adapter::relation::do_create_relation(
                adapter_type,
                database.to_string(),
                schema.to_string(),
                Some(model_name.to_string()),
                None,
                package_quoting
                    .try_into()
                    .expect("Failed to convert quoting to resolved quoting"),
            )
            .unwrap(),
        ))
        .into_value(),
        sql_resources: sql_resources_clone,
    };
    let this_value = MinijinjaValue::from_object(this_relation);

    // Create a BTreeMap for builtins
    let mut builtins = BTreeMap::new();

    // Create ref function
    let sql_resources_clone = sql_resources.clone();
    let ref_function = ResolveRefFunction {
        database: database.to_string(),
        schema: schema.to_string(),
        adapter_type,
        sql_resources: sql_resources_clone,
        root_runtime_config: root_runtime_config.clone(),
        package_quoting,
    };
    let ref_value = MinijinjaValue::from_object(ref_function);
    builtins.insert("ref".to_string(), ref_value.clone());

    // Create source function
    let source_function = ResolveSourceFunction {
        database: database.to_string(),
        schema: schema.to_string(),
        sql_resources: sql_resources.clone(),
        root_runtime_config: root_runtime_config.clone(),
        adapter_type,
        package_quoting,
    };
    let source_value = MinijinjaValue::from_object(source_function);
    builtins.insert("source".to_string(), source_value.clone());

    // Create function function
    let function_function = ResolveFunctionFunction {
        database: database.to_string(),
        schema: schema.to_string(),
        sql_resources: sql_resources.clone(),
        root_runtime_config: root_runtime_config.clone(),
        adapter_type,
        package_quoting,
    };
    let function_value = MinijinjaValue::from_object(function_function);
    builtins.insert("function".to_string(), function_value.clone());

    let metric_function = ResolveMetricFunction {
        sql_resources: sql_resources.clone(),
        root_runtime_config,
    };
    let metric_value = MinijinjaValue::from_object(metric_function);
    builtins.insert("metric".to_string(), metric_value.clone());

    let package_dependency = if package_name == root_project_name {
        None
    } else {
        Some(package_name.to_string())
    };
    // Pre-finalization read: used to initialize the Jinja context before rendering starts,
    // before the root overlay has been applied. The optimistic `true` default is intentional.
    let is_enabled = config.get_enabled_with_default();
    let config_value = MinijinjaValue::from_object(ParseConfig {
        enabled: is_enabled,
        root_overlay_forces_enabled,
        sql_resources: sql_resources.clone(),
        package_dependency: package_dependency.clone(),
        error_path: Some(display_path.to_path_buf()),
        adapter_type,
    });
    builtins.insert(
        "config".to_string(),
        MinijinjaValue::from_object(ParseConfig {
            enabled: is_enabled,
            root_overlay_forces_enabled,
            sql_resources,
            package_dependency,
            error_path: Some(display_path.to_path_buf()),
            adapter_type,
        }),
    );

    // TODO (Ani): Make this more extensible and depending on the resouce it could be model, macro, or source
    let model = DbtModel {
        __common_attr__: CommonAttributes {
            name: model_name.to_owned(),
            package_name: package_name.to_owned(),
            path: DbtPath::from(model_path),
            name_span: dbt_common::Span::default(),
            original_file_path: DbtPath::from(display_path),
            patch_path: None,
            unique_id: format!("{package_name}.{model_name}"),
            fqn,
            description: None,
            raw_code: None,
            checksum: DbtChecksum::default(),
            language: None,
            tags: vec![],
            classifiers: vec![],
            meta: IndexMap::new(),
        },
        __base_attr__: NodeBaseAttributes {
            database: database.to_string(),
            schema: schema.to_string(),
            alias: model_name.to_string(),
            relation_name: None,
            // A placeholder node built during parse, before `+adapter` is resolved;
            // the target default is the only answer available here.
            adapter: adapter_type,
            propagate: Vec::new(),
            effective_propagation_target: None,
            materialized: ModelConfig::default_materialized(),
            static_analysis: global_static_analysis.unwrap_or_default().into(),
            static_analysis_off_reason: None,
            compute: None,
            enabled: true,
            extended_model: false,
            persist_docs: None,
            quoting: ResolvedQuoting::trues(),
            quoting_ignore_case: false,
            columns: vec![],
            depends_on: NodeDependsOn {
                macros: vec![],
                nodes: vec![],
                nodes_with_ref_location: vec![],
            },
            refs: vec![],
            sources: vec![],
            functions: vec![],
            metrics: vec![],
            unrendered_config: Default::default(),
        },
        __model_attr__: DbtModelAttr {
            introspection: IntrospectionKind::None,
            version: None,
            latest_version: None,
            constraints: vec![],
            deprecation_date: None,
            primary_key: vec![],
            time_spine: None,
            access: Access::default(),
            group: None,
            incremental_strategy: None,
            freshness: None,
            state: None,
            contract: None,
            event_time: None,
            catalog_name: None,
            table_format: None,
            sync: None,
            compiled_code: None,
        },
        __adapter_attr__: AdapterAttr::default(),
        __other__: BTreeMap::new(),
        deprecated_config: ModelConfig::default(),
    };

    let mut model_map = convert_yml_to_value_map(InternalDbtNode::serialize(&model));
    // Stub `DbtModel` uses `ModelConfig::default()` for `config` in YAML serialization. At parse
    // time, kwargs to `config(...)` (e.g. `post_hook=my_macro(model)`) are evaluated while
    // rendering; macros must see the merged node config (`properties_config` / `BaseConfig`),
    // matching dbt-core (dbt-fusion#1414).
    //
    // Use `dbt_yaml::to_value` + `yml_value_to_minijinja` — same pipeline as
    // `DbtModel::serialized_config()` — not `MinijinjaValue::from_serialize`, so later
    // `dbt_yaml::to_value(model)` → `InternalDbtNodeWrapper::deserialize` in adapter helpers
    // (`get_view_options`, `get_config_from_model`, …) round-trips correctly.
    let config_yml = dbt_yaml::to_value(config)
        .expect("Failed to serialize merged node config to dbt_yaml::Value for parse model.config");
    model_map.insert("config".to_owned(), yml_value_to_minijinja(config_yml));
    model_map.insert(
        "batch".to_owned(),
        MinijinjaValue::from_object(init_batch_context()),
    );

    let result_store = ResultStore::default();
    let mut packages: BTreeSet<String> = runtime_config.dependencies.keys().cloned().collect();
    packages.insert(root_project_name.to_string());

    // Object-typed slots are wrapped via `MinijinjaValue::from_object(...)` /
    // `MinijinjaValue::from_function(closure)` HERE rather than in the typed
    // ctx struct, because going through serde's `serialize_map` /
    // `serialize_seq` paths can change the underlying Object's concrete type.
    // `builtins` must keep its BTreeMap shape for downstream downcasts, while
    // `model` is intentionally mutable to match dbt Core's dict behavior.
    let ctx = ResolveModelCtx {
        this: this_value,
        ref_fn: ref_value,
        source: source_value,
        function: function_value,
        metric: metric_value,
        config: config_value,
        model: MinijinjaValue::from_dyn_object(to_model_context_map(model_map)),
        builtins: MinijinjaValue::from_object(builtins),
        // The invocation-wide `graph` mapping, empty at this point — matching
        // dbt-core, where parse-time `graph` is `manifest.flat_graph` before
        // `build_flat_graph()` has filled it in. Handing out the shared
        // mapping (rather than `UNDEFINED`) is what lets a macro stash scratch
        // state on `graph` during parsing and read it back from a later parse
        // render or from a hook. dbt-labs/fs#13454.
        graph: MinijinjaValue::from_dyn_object(invocation_graph()),
        store_result: MinijinjaValue::from_function(result_store.store_result()),
        load_result: MinijinjaValue::from_function(result_store.load_result()),
        store_raw_result: MinijinjaValue::from_function(result_store.store_raw_result()),
        execute: JinjaObject::new(ParseExecute::new(execute_exists)),
        context: JinjaObject::new(MacroLookupContext::new(
            root_project_name.to_string(),
            None,
            packages,
        )),
        target_unique_id: format!("{package_name}.{model_name}"),
        current_path: display_path.to_string_lossy().into_owned(),
        current_span: MinijinjaValue::from_serialize(Span::default()),
    };

    // Sanity: the constants downstream code uses to look up these keys must
    // match the field-rename strings on `ResolveModelCtx`. Compile-time
    // assertions; no runtime cost.
    debug_assert_eq!(TARGET_UNIQUE_ID, "TARGET_UNIQUE_ID");
    debug_assert_eq!(CURRENT_PATH, "__minijinja_current_path");
    debug_assert_eq!(CURRENT_SPAN, "__minijinja_current_span");

    to_jinja_btreemap(&ctx)
}

/// Batch Context (stubbing this on the fly for now. We'll need to implement this in the future)
fn init_batch_context() -> BTreeMap<String, MinijinjaValue> {
    // TODO: batch map should have valid event_time_start and event_time_end
    // for now, we are just using now
    let datetime = London.with_ymd_and_hms(2025, 1, 1, 1, 1, 1).unwrap();
    let mut batch_map = BTreeMap::new();
    batch_map.insert("id".to_string(), MinijinjaValue::from(""));
    batch_map.insert(
        "event_time_start".to_string(),
        MinijinjaValue::from_object(PyDateTime::new_aware(
            datetime,
            Some(PytzTimezone::new(Tz::UTC)),
        )),
    );
    batch_map.insert(
        "event_time_end".to_string(),
        MinijinjaValue::from_object(PyDateTime::new_aware(
            datetime,
            Some(PytzTimezone::new(Tz::UTC)),
        )),
    );
    batch_map
}

#[derive(Debug)]
struct ResolveRefFunction<T: ResolvableConfig<T> + 'static> {
    database: String,
    schema: String,
    adapter_type: AdapterType,
    sql_resources: Arc<Mutex<Vec<SqlResource<T>>>>,
    root_runtime_config: Arc<DbtRuntimeConfig>,
    package_quoting: DbtQuoting,
}

impl<T: ResolvableConfig<T>> Object for ResolveRefFunction<T> {
    fn get_value(self: &Arc<Self>, key: &MinijinjaValue) -> Option<MinijinjaValue> {
        match key.as_str()? {
            "config" => Some(MinijinjaValue::from_dyn_object(
                self.root_runtime_config.clone(),
            )),
            "function_name" => Some(MinijinjaValue::from("ref")),
            _ => None,
        }
    }

    fn call(
        self: &Arc<Self>,
        state: &State<'_, '_>,
        args: &[MinijinjaValue],
        _listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<MinijinjaValue, MinijinjaError> {
        if args.is_empty() || args.len() > 3 {
            return Err(MinijinjaError::new(
                MinijinjaErrorKind::InvalidOperation,
                "invalid number of arguments for ref macro",
            ));
        }
        let mut parser = ArgParser::new(args, None);

        let name: String;
        let mut package: Option<String> = None;

        if parser.positional_len() == 1 {
            name = parser.get::<String>("")?;
        } else if parser.positional_len() == 2 {
            let package_arg = parser.get::<String>("")?;
            let name_arg = parser.get::<String>("")?;
            package = Some(package_arg);
            name = name_arg;
        } else {
            return Err(MinijinjaError::new(
                MinijinjaErrorKind::InvalidOperation,
                "ref() takes at most 2 positional arguments",
            ));
        }

        // Check for version in kwargs — preserve original literal type (int, float, or string)
        let version = parser.consume_optional_either_from_kwargs::<NodeVersion>("version", "v");

        let model_name = name;
        let namespace = package;
        let span = state.current_instruction_span();
        let location = CodeLocation::new(span.start_line, span.start_col, span.start_offset);
        self.sql_resources.lock().unwrap().push(SqlResource::Ref((
            model_name.clone(),
            namespace,
            version,
            location,
        )));

        let relation = dbt_adapter::relation::RelationObject::new(Arc::from(
            dbt_adapter::relation::do_create_relation(
                self.adapter_type,
                self.database.clone(),
                self.schema.clone(),
                Some(model_name),
                None,
                self.package_quoting
                    .try_into()
                    .expect("Failed to convert quoting to resolved quoting"),
            )
            .unwrap(),
        ))
        .into_value();
        // At resolve time, fqn do not have to be accurate
        Ok(relation)
    }
}

#[derive(Debug)]
struct ResolveSourceFunction<T: ResolvableConfig<T>> {
    database: String,
    schema: String,
    adapter_type: AdapterType,
    sql_resources: Arc<Mutex<Vec<SqlResource<T>>>>,
    root_runtime_config: Arc<DbtRuntimeConfig>,
    package_quoting: DbtQuoting,
}

impl<T: ResolvableConfig<T>> Object for ResolveSourceFunction<T> {
    fn get_value(self: &Arc<Self>, key: &MinijinjaValue) -> Option<MinijinjaValue> {
        match key.as_str()? {
            "config" => Some(MinijinjaValue::from_dyn_object(
                self.root_runtime_config.clone(),
            )),
            "function_name" => Some(MinijinjaValue::from("source")),
            _ => None,
        }
    }

    fn call(
        self: &Arc<Self>,
        state: &State<'_, '_>,
        args: &[MinijinjaValue],
        _listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<MinijinjaValue, MinijinjaError> {
        let mut parser = ArgParser::new(args, None);
        if args.len() == 2 {
            let name = parser.get::<String>("name")?;
            let table_name = parser.get::<String>("table_name")?;
            let span = state.current_instruction_span();
            let location = CodeLocation::new(span.start_line, span.start_col, span.start_offset);
            // https://github.com/dbt-labs/dbt-core/blob/8a8857a85c0cc66c7e3de9eb7e9ca7fd63d553a4/core/dbt/context/providers.py#L666
            // at parse time dbt collects the source but returns a relation populated with the current model
            // TODO: Support Compile+Runtime Source Resolving
            self.sql_resources
                .lock()
                .unwrap()
                .push(SqlResource::Source((
                    name,
                    table_name.to_string(),
                    location,
                )));

            // At resolve time, fqn do not have to be accurate
            Ok(dbt_adapter::relation::RelationObject::new(Arc::from(
                dbt_adapter::relation::do_create_relation(
                    self.adapter_type,
                    self.database.clone(),
                    self.schema.clone(),
                    Some(table_name),
                    Some(RelationType::External),
                    self.package_quoting
                        .try_into()
                        .expect("Failed to convert quoting to resolved quoting"),
                )
                .unwrap(),
            ))
            .into_value())
        } else {
            Err(MinijinjaError::new(
                MinijinjaErrorKind::InvalidOperation,
                "source requires 2 string arguments",
            ))
        }
    }
}

#[derive(Debug)]
struct ResolveFunctionFunction<T: ResolvableConfig<T>> {
    database: String,
    schema: String,
    adapter_type: AdapterType,
    sql_resources: Arc<Mutex<Vec<SqlResource<T>>>>,
    root_runtime_config: Arc<DbtRuntimeConfig>,
    package_quoting: DbtQuoting,
}

impl<T: ResolvableConfig<T>> Object for ResolveFunctionFunction<T> {
    fn get_value(self: &Arc<Self>, key: &MinijinjaValue) -> Option<MinijinjaValue> {
        match key.as_str()? {
            "config" => Some(MinijinjaValue::from_dyn_object(
                self.root_runtime_config.clone(),
            )),
            "function_name" => Some(MinijinjaValue::from("function")),
            _ => None,
        }
    }

    fn call(
        self: &Arc<Self>,
        state: &State<'_, '_>,
        args: &[MinijinjaValue],
        _listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<MinijinjaValue, MinijinjaError> {
        if args.is_empty() || args.len() > 2 {
            return Err(MinijinjaError::new(
                MinijinjaErrorKind::InvalidOperation,
                "invalid number of arguments for function macro",
            ));
        }
        let mut parser = ArgParser::new(args, None);

        let name: String;
        let mut package: Option<String> = None;

        if parser.positional_len() == 1 {
            name = parser.get::<String>("")?;
        } else if parser.positional_len() == 2 {
            let package_arg = parser.get::<String>("")?;
            let name_arg = parser.get::<String>("")?;
            package = Some(package_arg);
            name = name_arg;
        } else {
            return Err(MinijinjaError::new(
                MinijinjaErrorKind::InvalidOperation,
                "function() takes at most 2 positional arguments",
            ));
        }

        let function_name = name;
        let namespace = package;
        let span = state.current_instruction_span();
        let location = CodeLocation::new(span.start_line, span.start_col, span.start_offset);
        self.sql_resources
            .lock()
            .unwrap()
            .push(SqlResource::Function((
                function_name.clone(),
                namespace,
                location,
            )));

        let relation = dbt_adapter::relation::do_create_relation(
            self.adapter_type,
            self.database.clone(),
            self.schema.clone(),
            Some(function_name),
            None,
            self.package_quoting
                .try_into()
                .expect("Failed to convert quoting to resolved quoting"),
        )
        .unwrap();

        // Create a FunctionObject instead of returning the relation directly
        let qualified_name = relation.render_self_as_str();
        let function_object = FunctionObject::new(qualified_name);

        Ok(function_object.into_value())
    }
}

#[derive(Debug)]
struct ResolveMetricFunction<T: ResolvableConfig<T>> {
    sql_resources: Arc<Mutex<Vec<SqlResource<T>>>>,
    root_runtime_config: Arc<DbtRuntimeConfig>,
}

impl<T: ResolvableConfig<T>> Object for ResolveMetricFunction<T> {
    fn get_value(self: &Arc<Self>, key: &MinijinjaValue) -> Option<MinijinjaValue> {
        match key.as_str()? {
            "config" => Some(MinijinjaValue::from_dyn_object(
                self.root_runtime_config.clone(),
            )),
            "function_name" => Some(MinijinjaValue::from("metric")),
            _ => None,
        }
    }

    fn call(
        self: &Arc<Self>,
        _state: &State<'_, '_>,
        args: &[MinijinjaValue],
        _listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<MinijinjaValue, MinijinjaError> {
        if args.is_empty() || args.len() > 3 {
            return Err(MinijinjaError::new(
                MinijinjaErrorKind::InvalidOperation,
                "invalid number of arguments for metric macro",
            ));
        }
        let mut parser = ArgParser::new(args, None);
        // If there are two positional args, the first is the package name and the second is the model name
        let arg0 = parser.get::<String>("")?;
        let arg1 = parser.get_optional::<String>("");
        let (package_name, metric_name) = match (arg0, arg1) {
            (package_name, Some(metric_name)) => (Some(package_name), metric_name),
            (metric_name, None) => (None, metric_name),
        };

        // Push the SqlResource with all available information
        self.sql_resources
            .lock()
            .unwrap()
            .push(SqlResource::Metric((
                metric_name.clone(),
                package_name.clone(),
            )));

        // Create and return the DbtMetricReference
        Ok(MinijinjaValue::from_object(ParseMetricReference {
            metric_name,
            _package_name: package_name,
        }))
    }
}

/// A struct that represents a parse metric reference object returned by the `metric` macro during parsing
pub struct ParseMetricReference {
    /// Name of the metric, e.g. `metric('metric_name')`
    pub metric_name: String,
    /// Name of the package, if provided e.g. `metric('package_name', 'metric_name')`
    pub _package_name: Option<String>,
}

impl Object for ParseMetricReference {
    fn repr(self: &Arc<Self>) -> ObjectRepr {
        ObjectRepr::Plain
    }
}

impl Debug for ParseMetricReference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.metric_name)
    }
}

/// Permissive parse-time stub returned ONLY for `config.get('meta')`.
///
/// Absorbs further `.get`/attr access (so dbt-autofix's
/// `config.get('meta').custom_key` migration pattern doesn't crash at parse)
/// and renders as the empty string. Fusion does static-analysis compile of
/// materialization bodies that dbt-core only renders at run-time via
/// `RuntimeConfigObject`, so we need a parse-time fallback here that
/// dbt-core doesn't — see
/// `~/git/dbt-autofix/manual_fixes/custom_configuration.md` for the
/// recommended migration path that depends on this behavior.
///
/// All other `config.get(name)` calls return `""` (matching dbt-core's
/// `ParseConfigObject.get` at `core/dbt/context/providers.py:554`) and do
/// not need this stub.
#[derive(Debug)]
struct ParseConfigValue;

impl Object for ParseConfigValue {
    fn get_value(self: &Arc<Self>, _key: &MinijinjaValue) -> Option<MinijinjaValue> {
        Some(MinijinjaValue::from_object(ParseConfigValue))
    }

    fn call_method(
        self: &Arc<Self>,
        _state: &State<'_, '_>,
        _name: &str,
        _args: &[MinijinjaValue],
        _listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<MinijinjaValue, MinijinjaError> {
        Ok(MinijinjaValue::from_object(ParseConfigValue))
    }

    fn render(self: &Arc<Self>, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "")
    }
}

/// A struct that represents a parse config object to be used during parsing.
#[derive(Debug)]
pub struct ParseConfig<T: ResolvableConfig<T> + 'static> {
    /// A pointer to a vector of sql resources to be collected during parsing
    pub sql_resources: Arc<Mutex<Vec<SqlResource<T>>>>,
    /// Whether the model is enabled (based on upstream config)
    pub enabled: bool,
    /// Whether the root project's config overlay explicitly sets `enabled: true` for this node.
    pub root_overlay_forces_enabled: bool,
    // Current package name
    pub package_dependency: Option<String>,
    /// Error path to be used for error reporting
    pub error_path: Option<PathBuf>,
    /// The target's adapter type, used to canonicalize adapter config-key aliases.
    /// [dbt-core's `credentials.translate_aliases`]
    pub adapter_type: AdapterType,
}

impl<T: ResolvableConfig<T>> ParseConfig<T> {
    /// Shared logic for turning a set of config kwargs into a typed config
    /// and pushing it as a `SqlResource::ConfigCall`. Used by both
    /// `config(...)` (via `call`) and `config.set(name, value)` (via
    /// `call_method`), matching dbt-core's `ParseConfigObject.set`
    /// (`core/dbt/context/providers.py:550-551`), which is defined as
    /// `def set(self, name, value): return self.__call__({name: value})`.
    fn apply_config(
        self: &Arc<Self>,
        state: &State<'_, '_>,
        mut kwargs: BTreeMap<String, MinijinjaValue>,
    ) -> Result<MinijinjaValue, MinijinjaError> {
        let enabled = if !kwargs.contains_key("enabled") {
            kwargs.insert("enabled".to_string(), MinijinjaValue::from(self.enabled));
            self.enabled
        } else {
            kwargs.get("enabled").unwrap().is_true()
        };
        // Canonicalize adapter config-key aliases
        let kwargs =
            dbt_adapter_core::config_aliases::canonicalize_config_keys(self.adapter_type, kwargs)
                .map_err(|dup| {
                MinijinjaError::new(
                    MinijinjaErrorKind::InvalidOperation,
                    format!(
                        "Config keys `{}` and `{}` both resolve to `{}` for adapter '{}'; a \
                     project cannot set the same underlying config key two different ways in \
                     the same place.",
                        dup.key_a, dup.key_b, dup.canonical, self.adapter_type,
                    ),
                )
            })?;
        // TODO: propgate span info for individual args
        let span = {
            let Span {
                start_line,
                start_col,
                start_offset,
                end_line,
                end_col,
                end_offset,
            } = state.current_instruction_span();
            dbt_yaml::Span {
                start: dbt_yaml::Marker::new(
                    start_offset as usize,
                    start_line as usize,
                    start_col as usize,
                ),
                end: dbt_yaml::Marker::new(
                    end_offset as usize,
                    end_line as usize,
                    end_col as usize,
                ),
                filename: self.error_path.as_ref().map(|p| Arc::new(p.to_path_buf())),
            }
        };

        let mut mapping = dbt_yaml::Mapping::with_capacity(kwargs.len());
        for (key, value) in kwargs.into_iter() {
            if value.is_undefined() {
                // dbt Core names the key too: `at path ['alias']: Undefined is not valid`
                return Err(minijinja::Error::new(
                    minijinja::ErrorKind::InvalidOperation,
                    format!(
                        "config requires all arguments to be defined, but '{key}' is undefined"
                    ),
                ));
            }

            let value = if let Some(dyn_obj) = value.as_object()
                && let Some(pydatetime) = dyn_obj.downcast::<PyDateTime>()
            {
                dbt_yaml::to_value(pydatetime.chrono_dt())
            } else {
                dbt_yaml::to_value(value)
            }
            .map_err(|e| {
                MinijinjaError::new(
                    MinijinjaErrorKind::InvalidOperation,
                    format!("Failed to serialize config into yaml: {e}"),
                )
            })?
            .with_span(span.clone());

            mapping.insert(dbt_yaml::Value::String(key, span.clone()), value);
        }

        let yaml_value = dbt_yaml::Value::Mapping(mapping, span);
        let config: T = into_typed_with_error(
            yaml_value,
            true,
            self.package_dependency.as_deref(),
            self.error_path.clone(),
        )
        .map_err(|e| {
            MinijinjaError::new(
                MinijinjaErrorKind::InvalidOperation,
                format!("Failed to parse node configuration: {e}"),
            )
        })?;
        self.sql_resources
            .lock()
            .unwrap()
            .push(SqlResource::ConfigCall(Box::new(config)));
        // The root project's overlay outranks this inline config, so skipping the rest of the
        // render would drop refs, rendered SQL and macro spans the enabled node still needs.
        if !enabled && !self.root_overlay_forces_enabled {
            return Err(MinijinjaError::new(
                MinijinjaErrorKind::DisabledModel,
                "Model is disabled".to_string(),
            ));
        }
        Ok(MinijinjaValue::UNDEFINED)
    }
}

impl<T: ResolvableConfig<T>> Object for ParseConfig<T> {
    /// Implement the call method on the config object
    fn call(
        self: &Arc<Self>,
        state: &State<'_, '_>,
        args: &[MinijinjaValue],
        _listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<MinijinjaValue, MinijinjaError> {
        let mut args = ArgParser::new(args, None);
        // If there is a positional argument, it must be a map
        let kwargs = if args.positional_len() == 1 {
            let positional_val: MinijinjaValue = args.next_positional::<MinijinjaValue>()?;
            if positional_val.kind() != ValueKind::Map {
                return Err(MinijinjaError::new(
                    MinijinjaErrorKind::InvalidOperation,
                    format!(
                        "Invalid config argument kind specified: {}",
                        positional_val.kind()
                    ),
                ));
            }
            positional_val
                .as_object()
                .unwrap()
                .try_iter_pairs()
                .expect("Invalid config object specified")
                .map(|(k, v)| {
                    (
                        k.as_str()
                            .expect("Invalid config object specified. Keys must be strings")
                            .to_string(),
                        v,
                    )
                })
                .collect()
        } else {
            args.drain_kwargs()
        };

        self.apply_config(state, kwargs)
    }

    fn call_method(
        self: &Arc<Self>,
        state: &State<'_, '_>,
        name: &str,
        args: &[MinijinjaValue],
        _listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<MinijinjaValue, MinijinjaError> {
        match name {
            // `config.get(name)` at parse time:
            //   - name == "meta": returns a permissive chainable stub
            //     (ParseConfigValue) so dbt-autofix's
            //     `config.get('meta').custom_key` migration pattern
            //     (~/git/dbt-autofix/manual_fixes/custom_configuration.md)
            //     doesn't crash. Fusion does static-analysis compile of
            //     materialization bodies that dbt-core only renders at
            //     run-time with `RuntimeConfigObject`, so we need a
            //     parse-time fallback here that dbt-core doesn't.
            //   - any other name with a meaningful caller default: returns
            //     the default verbatim. Fusion extension above Python,
            //     never breaks parity-correct templates.
            //   - any other name with no default: returns "". Matches
            //     dbt-core's `ParseConfigObject.get` exactly
            //     (core/dbt/context/providers.py:554:
            //     `def get(self, name, default=None, validator=None):
            //          return ""`).
            //
            // The compile-phase equivalent is
            // `CompileConfig.call_method("get")` in
            // fs/sa/crates/dbt-jinja-utils/src/phases/compile/compile_config.rs,
            // which does real lookup against the finalized config.
            "get" => {
                let mut args = ArgParser::new(args, None);
                let name: String = args.get("name")?;
                let default = args.get_optional::<MinijinjaValue>("default");

                if name == "meta" {
                    return Ok(MinijinjaValue::from_object(ParseConfigValue));
                }
                match default {
                    Some(d) if !d.is_undefined() && !d.is_none() => Ok(d),
                    _ => Ok(MinijinjaValue::from("")),
                }
            }
            // `config.set(name, value)` at parse time is equivalent to
            // `config(**{name: value})`, matching dbt-core's
            // `ParseConfigObject.set` (core/dbt/context/providers.py:550-551):
            // `def set(self, name, value): return self.__call__({name: value})`.
            "set" => {
                let mut args = ArgParser::new(args, None);
                let name: String = args.get("name")?;
                let value: MinijinjaValue = args.get("value")?;
                let mut kwargs = BTreeMap::new();
                kwargs.insert(name, value);
                self.apply_config(state, kwargs)
            }
            // At compile time, this will throw an error if the config required does not exist
            "require" => {
                let mut args = ArgParser::new(args, None);
                let _: String = args.get("name")?;
                Ok(MinijinjaValue::from(""))
            }
            // At parse time, this just returns an empty string (for consistency with compile/run configs)
            "meta_get" => {
                let mut args = ArgParser::new(args, None);
                let _: String = args.get("name")?;
                Ok(MinijinjaValue::from(""))
            }
            // At parse time, this just returns an empty string (for consistency with compile/run configs)
            "meta_require" => {
                let mut args = ArgParser::new(args, None);
                let _: String = args.get("name")?;
                Ok(MinijinjaValue::from(""))
            }
            // At parse time, return false (no column docs persistence during parsing)
            "persist_column_docs" => Ok(MinijinjaValue::from(false)),
            // At parse time, return false (no relation docs persistence during parsing)
            "persist_relation_docs" => Ok(MinijinjaValue::from(false)),
            _ => Err(MinijinjaError::new(
                MinijinjaErrorKind::UnknownMethod,
                format!("Unknown method on parse: {name}"),
            )),
        }
    }
}

#[derive(Debug)]
struct ResolveThisFunction<T: ResolvableConfig<T> + 'static> {
    relation: MinijinjaValue,
    sql_resources: Arc<Mutex<Vec<SqlResource<T>>>>,
}

impl<T: ResolvableConfig<T>> Object for ResolveThisFunction<T> {
    fn call_method(
        self: &Arc<Self>,
        state: &State<'_, '_>,
        name: &str,
        args: &[MinijinjaValue],
        listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<MinijinjaValue, MinijinjaError> {
        self.relation
            .as_object()
            .expect("Failed to convert relation to object")
            .call_method(state, name, args, listeners)
    }

    fn get_value(self: &Arc<Self>, key: &MinijinjaValue) -> Option<MinijinjaValue> {
        // This is a special case for the this relation macro to be able to downcast this to a RelationObject
        if key.as_str() == Some(THIS_RELATION_KEY) {
            return Some(self.relation.clone());
        }
        // TODO: This can be more fine-grained (i.e. if the user only asks for database etc.)
        if key.as_str()? == "database" || key.as_str()? == "schema" {
            self.sql_resources.lock().unwrap().push(SqlResource::This);
        }
        self.relation
            .as_object()
            .expect("Failed to convert relation to object")
            .get_value(key)
    }

    fn enumerate(self: &Arc<Self>) -> Enumerator {
        self.relation
            .as_object()
            .expect("Failed to convert relation to object")
            .enumerate()
    }

    fn render(self: &Arc<Self>, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.sql_resources.lock().unwrap().push(SqlResource::This);
        self.relation
            .as_object()
            .expect("Failed to convert relation to object")
            .render(f)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use dbt_schemas::schemas::relations::DEFAULT_DBT_QUOTING;
    use dbt_test_primitives::assert_contains;
    #[test]
    fn test_resolve_source_function_rendering() {
        let sql_resources = Arc::new(Mutex::new(Vec::new()));

        // Create a minijinja environment to test rendering
        let mut env = minijinja::Environment::new();

        let source_function: ResolveSourceFunction<ModelConfig> = ResolveSourceFunction {
            database: "test_db".to_string(),
            schema: "test_schema".to_string(),
            sql_resources,
            root_runtime_config: Arc::new(DbtRuntimeConfig::default()),
            adapter_type: AdapterType::Postgres,
            package_quoting: DEFAULT_DBT_QUOTING,
        };
        let source_value = MinijinjaValue::from_object(source_function);
        env.add_global("source", source_value);

        // Create a template that uses the source function
        let template = env
            .template_from_str("{{ source('my_source', 'my_table').render() }}")
            .unwrap();

        // Render the template
        let result = template.render(minijinja::context!(), &[]).unwrap();

        assert_contains!(result, "test_db");
        assert_contains!(result, "test_schema");
        assert_contains!(result, "my_table");
    }

    /// Creates a ParseConfig for use in unit tests.
    fn make_test_parse_config() -> ParseConfig<ModelConfig> {
        ParseConfig {
            sql_resources: Arc::new(Mutex::new(Vec::new())),
            enabled: true,
            root_overlay_forces_enabled: false,
            package_dependency: None,
            error_path: None,
            adapter_type: AdapterType::Postgres,
        }
    }

    /// Regression test: config.get('key', 'default') should return the default
    /// value during parse phase, not a ParseConfigValue stub.
    /// Without this, passing the result to adapter.get_relation(identifier=...)
    /// fails with "incompatible type ParseConfigValue; value is not a string".
    #[test]
    fn test_config_get_with_default_returns_default() {
        let mut env = minijinja::Environment::new();

        let config = make_test_parse_config();
        env.add_global("config", MinijinjaValue::from_object(config));

        // config.get('alias', 'my_fallback') should render as "my_fallback"
        let template = env
            .template_from_str("{{ config.get('alias', 'my_fallback') }}")
            .unwrap();
        let result = template.render(minijinja::context!(), &[]).unwrap();

        assert_eq!(result, "my_fallback");
    }

    /// Regression test: the rendered default from config.get must be usable as
    /// a string argument (i.e. pass Value::as_str()).  This simulates what
    /// adapter.get_relation(identifier=config.get('alias', 'tbl')) does.
    #[test]
    fn test_config_get_default_is_string_typed() {
        let mut env = minijinja::Environment::new();

        let config = make_test_parse_config();
        env.add_global("config", MinijinjaValue::from_object(config));

        // Evaluate config.get with a default and inspect the Value directly
        let expr = env
            .compile_expression("config.get('alias', 'tbl')")
            .unwrap();
        let value = expr.eval(minijinja::context!(), &[]).unwrap();

        assert!(
            value.as_str().is_some(),
            "config.get with a default must return a string-typed Value, got: {:?}",
            value
        );
        assert_eq!(value.as_str().unwrap(), "tbl");
    }

    /// Verify that `config.get(name)` without a default returns an empty
    /// string-typed value (matching dbt-core's `ParseConfigObject.get` at
    /// providers.py:554). Renders as `""` and passes `Value::as_str()` so
    /// downstream consumers like `adapter.get_relation(database=...)` don't
    /// crash.
    #[test]
    fn test_config_get_without_default_returns_empty_string() {
        let mut env = minijinja::Environment::new();

        let config = make_test_parse_config();
        env.add_global("config", MinijinjaValue::from_object(config));

        let template = env.template_from_str("{{ config.get('alias') }}").unwrap();
        let result = template.render(minijinja::context!(), &[]).unwrap();
        assert_eq!(result, "");

        let value = env
            .compile_expression("config.get('alias')")
            .unwrap()
            .eval(minijinja::context!(), &[])
            .unwrap();
        assert!(
            value.as_str().is_some(),
            "config.get without a default must be a string-typed Value, got: {:?}",
            value,
        );
        assert_eq!(value.as_str().unwrap(), "");
    }

    /// `config.get('alias')` after `{{ config(alias='renamed') }}` deposit
    /// returns `""` at parse — deposited values are NOT surfaced through
    /// parse-time `config.get`. Matches dbt-core's `ParseConfigObject.get`
    /// (providers.py:554), which ignores kwargs deposited via
    /// `ContextConfig.add_config_call` and unconditionally returns `""`.
    /// Deposited values become accessible at compile time via
    /// `CompileConfig.call_method("get")` (the Fusion equivalent of
    /// `RuntimeConfigObject.get`).
    #[test]
    fn test_parse_config_get_non_meta_returns_empty_string_matching_python() {
        let mut env = minijinja::Environment::new();
        let config = make_test_parse_config();
        env.add_global("config", MinijinjaValue::from_object(config));

        let template = env
            .template_from_str("{% set _ = config(alias='renamed') %}|{{ config.get('alias') }}|")
            .unwrap();
        let result = template.render(minijinja::context!(), &[]).unwrap();
        assert_eq!(result, "||");
    }

    /// `config.get('not_set', 'fallback')` returns the default verbatim.
    /// Fusion-specific extension above Python's
    /// `ParseConfigObject.get` (which always returns `""`) — strictly more
    /// useful and never breaks parity-correct templates.
    #[test]
    fn test_parse_config_get_non_meta_with_default_returns_default() {
        let mut env = minijinja::Environment::new();
        let config = make_test_parse_config();
        env.add_global("config", MinijinjaValue::from_object(config));

        let template = env
            .template_from_str("{{ config.get('not_set', 'fallback') }}")
            .unwrap();
        let result = template.render(minijinja::context!(), &[]).unwrap();
        assert_eq!(result, "fallback");
    }

    /// `config.get('meta').get('any_key')` chains through a permissive stub
    /// at parse — supports dbt-autofix's `config.get('meta').custom_key`
    /// migration pattern
    /// (~/git/dbt-autofix/manual_fixes/custom_configuration.md). The stub
    /// absorbs further `.get`/attr access and renders as `""`.
    #[test]
    fn test_parse_config_get_meta_returns_chainable_permissive_stub() {
        let mut env = minijinja::Environment::new();
        let config = make_test_parse_config();
        env.add_global("config", MinijinjaValue::from_object(config));

        let template = env
            .template_from_str("|{{ config.get('meta').get('any_key') }}|")
            .unwrap();
        let result = template.render(minijinja::context!(), &[]).unwrap();
        assert_eq!(result, "||");

        // Attribute access also absorbed (the autofix doc's
        // `config.get('meta').cold_storage_date_type` shape).
        let template2 = env
            .template_from_str("|{{ config.get('meta').cold_storage_date_type }}|")
            .unwrap();
        let result2 = template2.render(minijinja::context!(), &[]).unwrap();
        assert_eq!(result2, "||");
    }

    /// fs#13424: `+target_catalog:` has no dedicated Fusion field (unlike `catalog`/`database`),
    /// so it can only resolve via a raw config-key rename before typing -- which is exactly
    /// what the inline `{{ config(...) }}` layer does. Canonicalizing `target_catalog` into
    /// `target_database` (dbt-core's `Credentials._ALIASES`) here is what turns a previously
    /// unrecognized key into a resolved one.
    #[test]
    fn test_parse_config_inline_target_catalog_alias_resolves_on_databricks_snapshot() {
        use dbt_schemas::schemas::project::SnapshotConfig;

        let sql_resources = Arc::new(Mutex::new(Vec::new()));
        let config = ParseConfig::<SnapshotConfig> {
            sql_resources: sql_resources.clone(),
            enabled: true,
            root_overlay_forces_enabled: false,
            package_dependency: None,
            error_path: None,
            adapter_type: AdapterType::Databricks,
        };

        let mut env = minijinja::Environment::new();
        env.add_global("config", MinijinjaValue::from_object(config));
        let template = env
            .template_from_str("{{ config(target_catalog='cat1') }}")
            .unwrap();
        template
            .render(minijinja::context!(), &[])
            .expect("`target_catalog` must not raise an unrecognized-key error");

        let resources = sql_resources.lock().unwrap();
        let SqlResource::ConfigCall(cfg) = resources.last().expect("config call recorded") else {
            panic!("expected a ConfigCall resource");
        };
        assert_eq!(cfg.target_database, Some("cat1".to_string()));
    }

    /// fs#13424: postgres' `dbname` -> `database` alias likewise has no dedicated field of its
    /// own and resolves only via the same raw config-key rename.
    #[test]
    fn test_parse_config_inline_dbname_alias_resolves_on_postgres() {
        let sql_resources = Arc::new(Mutex::new(Vec::new()));
        let config = ParseConfig::<ModelConfig> {
            sql_resources: sql_resources.clone(),
            enabled: true,
            root_overlay_forces_enabled: false,
            package_dependency: None,
            error_path: None,
            adapter_type: AdapterType::Postgres,
        };

        let mut env = minijinja::Environment::new();
        env.add_global("config", MinijinjaValue::from_object(config));
        let template = env
            .template_from_str("{{ config(dbname='pgdb') }}")
            .unwrap();
        template
            .render(minijinja::context!(), &[])
            .expect("`dbname` must not raise an unrecognized-key error");

        let resources = sql_resources.lock().unwrap();
        let SqlResource::ConfigCall(cfg) = resources.last().expect("config call recorded") else {
            panic!("expected a ConfigCall resource");
        };
        assert_eq!(
            cfg.database.clone().into_inner().flatten(),
            Some("pgdb".to_string())
        );
    }

    /// Two keys in the same inline `{{ config(...) }}` call that canonicalize to the same
    /// field is a `DuplicateAliasError` in dbt-core (`Translator.translate_mapping`); Fusion
    /// must reject it too rather than silently picking one.
    #[test]
    fn test_parse_config_inline_duplicate_alias_key_errors() {
        let sql_resources = Arc::new(Mutex::new(Vec::new()));
        let config = ParseConfig::<ModelConfig> {
            sql_resources,
            enabled: true,
            root_overlay_forces_enabled: false,
            package_dependency: None,
            error_path: None,
            adapter_type: AdapterType::Databricks,
        };

        let mut env = minijinja::Environment::new();
        env.add_global("config", MinijinjaValue::from_object(config));
        let template = env
            .template_from_str("{{ config(catalog='a', database='b') }}")
            .unwrap();
        let err = template
            .render(minijinja::context!(), &[])
            .expect_err("`catalog` and `database` both resolving to `database` must error");
        assert_contains!(err.to_string(), "database");
    }
}
