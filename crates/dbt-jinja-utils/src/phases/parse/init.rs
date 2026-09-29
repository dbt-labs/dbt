//! This module contains the functions for initializing the Jinja environment for the parse phase.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

use chrono::DateTime;
use chrono_tz::Tz;
use dbt_adapter::{Adapter, sql_types::DefaultTypeOps};
use dbt_adapter_core::*;
use dbt_common::{ErrorCode, FsResult, fs_err, io_args::IoArgs};
use dbt_jinja_ctx::{GlobalCore, JinjaObject, ResolveCore, to_jinja_btreemap};
use dbt_jinja_vars::DbtVars;
use dbt_schemas::schemas::{
    common::DbtQuoting,
    dbt_catalogs::DbtCatalogs,
    profiles::{DbConfig, TargetContext},
};
use indexmap::IndexMap;
use minijinja::{
    AutoEscape, Environment, dispatch_object::THREAD_LOCAL_DEPENDENCIES, macro_unit::MacroUnit,
    value::Value as MinijinjaValue,
};
use minijinja_contrib::modules::{py_datetime::datetime::PyDateTime, pytz::PytzTimezone};

use crate::{
    environment_builder::{JinjaEnvBuilder, MacroUnitsWrapper},
    flags::Flags,
    functions::ConfiguredVar,
    invocation_args::InvocationArgs,
    jinja_environment::JinjaEnv,
    listener::{DefaultJinjaTypeCheckEventListenerFactory, JinjaTypeCheckingEventListenerFactory},
    phases::utils::build_target_context_map,
};

/// Factory for the Jinja services used during an invocation.
///
/// Encapsulates how the parse/render environment and its per-invocation
/// type-checking listener factory are created, so callers depend on this trait
/// rather than on concrete construction. The default implementation builds the
/// environment exactly like [`initialize_parse_jinja_environment`]; implementors
/// may override [`register_extra_functions`](JinjaFactory::register_extra_functions)
/// to add functions once the environment is built.
pub trait JinjaFactory: Send + Sync {
    /// Build the parse-phase Jinja environment, then register any extra functions.
    #[allow(clippy::too_many_arguments)]
    fn create_parse_jinja_environment(
        &self,
        project_name: &str,
        profile: &str,
        target: &str,
        adapter_type: AdapterType,
        db_config: DbConfig,
        // Every adapter type the active target declares. The default adapter's
        // type is `adapter_type`; the rest are here so their internal macro
        // packages are treated as internal and get their own namespace.
        target_adapter_types: Vec<AdapterType>,
        package_quoting: DbtQuoting,
        macro_units: BTreeMap<String, Vec<MacroUnit>>,
        vars: BTreeMap<String, IndexMap<String, DbtVars>>,
        cli_vars: BTreeMap<String, dbt_yaml::Value>,
        flags: BTreeMap<String, MinijinjaValue>,
        run_started_at: DateTime<Tz>,
        invocation_args: &InvocationArgs,
        all_package_names: BTreeSet<String>,
        io_args: IoArgs,
        catalogs: Option<Arc<DbtCatalogs>>,
    ) -> FsResult<JinjaEnv> {
        let mut env = initialize_parse_jinja_environment(
            project_name,
            profile,
            target,
            adapter_type,
            db_config,
            target_adapter_types,
            package_quoting,
            macro_units,
            vars,
            cli_vars,
            flags,
            run_started_at,
            invocation_args,
            all_package_names,
            io_args,
            catalogs,
        )?;
        self.register_extra_functions(&mut env.env);
        Ok(env)
    }

    /// Register additional functions into a freshly-built environment.
    /// Defaults to a no-op.
    fn register_extra_functions(&self, _env: &mut Environment) {}

    /// Create the per-invocation type-checking event listener factory.
    fn create_type_checking_listener_factory(
        &self,
    ) -> Arc<dyn JinjaTypeCheckingEventListenerFactory> {
        Arc::new(DefaultJinjaTypeCheckEventListenerFactory::default())
    }
}

/// Default [`JinjaFactory`].
#[derive(Debug, Default)]
pub struct DefaultJinjaFactory;

impl JinjaFactory for DefaultJinjaFactory {}

/// Initialize a Jinja environment for the parse phase.
#[allow(clippy::too_many_arguments)]
pub fn initialize_parse_jinja_environment(
    project_name: &str,
    profile: &str,
    target: &str,
    adapter_type: AdapterType,
    db_config: DbConfig,
    target_adapter_types: Vec<AdapterType>,
    package_quoting: DbtQuoting,
    macro_units: BTreeMap<String, Vec<MacroUnit>>,
    vars: BTreeMap<String, IndexMap<String, DbtVars>>,
    cli_vars: BTreeMap<String, dbt_yaml::Value>,
    flags: BTreeMap<String, MinijinjaValue>,
    run_started_at: DateTime<Tz>,
    invocation_args: &InvocationArgs,
    all_package_names: BTreeSet<String>,
    io_args: IoArgs,
    catalogs: Option<Arc<DbtCatalogs>>,
) -> FsResult<JinjaEnv> {
    // Set the thread local dependencies
    THREAD_LOCAL_DEPENDENCIES.get_or_init(|| Mutex::new(all_package_names));
    let adapter_config_mapping = db_config.to_mapping().unwrap();
    let database = db_config.get_database().cloned();
    let schema = db_config.get_schema().cloned();
    let target_context = TargetContext::try_from(db_config)
        .map_err(|e| fs_err!(ErrorCode::InvalidConfig, "{}", &e))?;
    let target_context = Arc::new(build_target_context_map(profile, target, target_context));

    let mut prj_flags = Flags::from_project_flags(flags);
    let inv_flags = Flags::from_invocation_args(invocation_args.to_dict());
    let joined_flags = prj_flags.join(inv_flags);

    let invocation_args_dict = invocation_args_to_dict(invocation_args, &prj_flags);

    let type_formatter = Arc::new(DefaultTypeOps::new(adapter_type));
    let adapter = Arc::new(Adapter::new_parse_phase_adapter(
        adapter_type,
        adapter_config_mapping,
        package_quoting,
        type_formatter,
        catalogs,
    ));

    let resolve_core = ResolveCore {
        global_core: GlobalCore {
            run_started_at: JinjaObject::new(PyDateTime::new_aware(
                run_started_at,
                Some(PytzTimezone::new(Tz::UTC)),
            )),
            target: target_context.clone(),
            flags: MinijinjaValue::from_object(joined_flags),
        },
        project_name: project_name.to_string(),
        env: target_context,
        invocation_args_dict,
        invocation_id: invocation_args.invocation_id.to_string(),
        var: JinjaObject::new(ConfiguredVar::new(vars, cli_vars)),
        database,
        schema,
        write: MinijinjaValue::NONE,
    };

    // Globals must be registered BEFORE `try_with_macros` so its replay-mode
    // detection (`invocation_args_dict.replay`) sees them — otherwise the
    // elementary upload-artifacts macros are not no-op'd in replay and the
    // private-dependency replay goldie drifts on temp-table SQL.
    let mut env = JinjaEnvBuilder::new()
        .with_undefined_behavior(minijinja::UndefinedBehavior::AllowAll)
        .with_adapter(adapter)
        .with_extra_dialects(
            target_adapter_types
                .iter()
                .map(|t| t.as_ref().to_string())
                .collect(),
        )
        .with_root_package(project_name.to_string())
        .with_globals(to_jinja_btreemap(&resolve_core))
        .with_warn_error_options(invocation_args.warn_error_options.clone())
        .with_io_args(io_args)
        .try_with_macros(MacroUnitsWrapper::new(macro_units))?
        .build();

    // Checks render during parse, so `info_schema()` has to resolve here — not just in the check
    // runner's environment, where it originally lived when checks were discovered at run time.
    env.add_global("info_schema", crate::info_schema::make_info_schema_fn());

    // This ensures consistent quoting behavior for rendering same string in call block vs directly
    // example: {{ target.database }} vs {{ call statement(None, auto_begin=false, fetch_result=false) }}{{ target.database }}{{ endcall }}
    // if we directly render the first one, it will add double quotes around the string, but the second one will not.
    env.env.set_auto_escape_callback(|_| AutoEscape::None);
    Ok(env)
}

fn invocation_args_to_dict(
    args: &InvocationArgs,
    flags: &Flags,
) -> BTreeMap<String, MinijinjaValue> {
    /* Port of: https://github.com/dbt-labs/dbt-core/blob/62757f198761ca3a8b8700535bc8c28f84d5c5d5/core/dbt/utils/utils.py#L332
    * ```python
    def args_to_dict(args):
        var_args = vars(args).copy()
        # update the args with the flags, which could also come from environment
        # variables or project_flags
        flag_dict = flags.get_flag_dict()
        var_args.update(flag_dict)
        dict_args = {}
        # remove args keys that clutter up the dictionary
        for key in var_args:
            if key.lower() in var_args and key == key.upper():
                # skip all capped keys being introduced by Flags in dbt.cli.flags
                continue
            if key in ["cls", "mp_context"]:
                continue
            if var_args[key] is None:
                continue
            # TODO: add more default_false_keys
            default_false_keys = (
                "debug",
                "full_refresh",
                "fail_fast",
                "warn_error",
                "single_threaded",
                "log_cache_events",
                "store_failures",
                "use_experimental_parser",
            )
            default_empty_yaml_dict_keys = ("vars", "warn_error_options")
            if key in default_false_keys and var_args[key] is False:
                continue
            if key in default_empty_yaml_dict_keys and var_args[key] == "{}":
                continue
            # this was required for a test case
            if isinstance(var_args[key], PosixPath) or isinstance(var_args[key], WindowsPath):
                var_args[key] = str(var_args[key])
            if isinstance(var_args[key], WarnErrorOptionsV2):
                var_args[key] = var_args[key].to_dict()

            dict_args[key] = var_args[key]
        return dict_args
    * ```
    */
    let mut var_args = args.to_dict();
    // update the args with the flags, which could also come from environment
    // variables or project_flags
    var_args.extend(flags.to_dict());
    // remove args keys that clutter up the dictionary
    let mut dict_args = BTreeMap::new();
    for (key, value) in var_args.iter() {
        if var_args.contains_key(&key.to_lowercase()) && key == &key.to_uppercase() {
            // skip all capped keys being introduced by Flags in dbt.cli.flags
            continue;
        }
        if key == "cls" || key == "mp_context" {
            continue;
        }
        if value.is_none() {
            continue;
        }

        // The rest of the filtering logic from dbt-core is irrelevant

        dict_args.insert(key.clone(), value.clone());
    }
    dict_args
}
