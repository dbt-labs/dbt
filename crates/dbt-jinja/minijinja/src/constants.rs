//! Constants for the Minijinja library

/// The name of the global namespace registry
/// the dict of it should be package_name -> [macro_name, macro_name, ...]
pub const MACRO_NAMESPACE_REGISTRY: &str = "MACRO_NAMESPACE_REGISTRY";
/// The name of the root project namespace
pub const ROOT_PROJECT_NAMESPACE: &str = "ROOT_PROJECT_NAMESPACE";
/// The name of the non-internal packages namespace
pub const NON_INTERNAL_PACKAGES: &str = "NON_INTERNAL_PACKAGES";
/// The name of the dbt and adapters namespace. Holds `dialect -> (macro_name ->
/// package)`; select a dialect's namespace with
/// [`crate::dispatch_object::dbt_and_adapters_namespace_for`].
pub const DBT_AND_ADAPTERS_NAMESPACE: &str = "DBT_AND_ADAPTERS_NAMESPACE";
/// The adapter dialect a template is being rendered or typechecked for. Set as an
/// environment global for rendering, and carried in the typecheck context so the
/// typechecker resolves internal macros the same way dispatch does.
pub const DIALECT: &str = "dialect";
/// The name of the root package name
pub const ROOT_PACKAGE_NAME: &str = "ROOT_PACKAGE_NAME";
/// The name of the local package
pub const TARGET_PACKAGE_NAME: &str = "TARGET_PACKAGE_NAME";
/// The name of the executing unique id
pub const TARGET_UNIQUE_ID: &str = "TARGET_UNIQUE_ID";
/// the dict of it should be package_name.macro_name -> macro_unit
pub const MACRO_TEMPLATE_REGISTRY: &str = "MACRO_TEMPLATE_REGISTRY";
/// The order of macro dispatch per macro namespace
pub const MACRO_DISPATCH_ORDER: &str = "MACRO_DISPATCH_ORDER";
/// The default schema for tests
pub const DEFAULT_TEST_SCHEMA: &str = "dbt_test__audit";
/// The name of the current path
pub const CURRENT_PATH: &str = "__minijinja_current_path";
/// The name of the current span
pub const CURRENT_SPAN: &str = "__minijinja_current_span";
/// The current execution phase
pub const CURRENT_EXECUTION_PHASE: &str = "__minijinja_current_execution_phase";
/// The name of the dbt packages directory
pub const DBT_PACKAGES_DIR_NAME: &str = "dbt_packages";
/// The name of the dbt internal packages directory
pub const DBT_INTERNAL_PACKAGES_DIR_NAME: &str = "dbt_internal_packages";
/// "Unique worker id" that represents something similar to thead_id in python
pub const THREAD_ID: &str = "thread_id";
