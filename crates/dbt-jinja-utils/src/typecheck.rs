use crate::{jinja_environment::JinjaEnv, listener};
use dbt_adapter::AdapterType;
use dbt_common::{
    ErrorCode, FsError, FsResult, io_args::IoArgs, path::DbtPath,
    tracing::dbt_emit::emit_error_log_message,
};
use minijinja::{
    AdapterDispatchFunction, ErrorKind, Value,
    compiler::codegen::CodeGenerationProfile,
    constants::{DBT_AND_ADAPTERS_NAMESPACE, DIALECT, ROOT_PACKAGE_NAME, TARGET_PACKAGE_NAME},
    load_builtins_with_namespace,
};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::Path,
    sync::Arc,
};

#[allow(clippy::too_many_arguments)]
/// Typecheck a single template against the given Jinja environment.
///
/// When `skip_redundant_template_syntax_error_log` is true, template parse failures
/// ([`ErrorKind::SyntaxError`]) are not logged: use this when the same `content` is rendered
/// immediately afterward so the render path can emit a single diagnostic (e.g. dbt1502).
pub fn typecheck(
    arg_io: &IoArgs,
    env: Arc<JinjaEnv>,
    noqa_comments: &HashMap<DbtPath, HashSet<u32>>,
    jinja_typechecking_listener_factory: Arc<dyn listener::JinjaTypeCheckingEventListenerFactory>,
    target_package_name: Option<String>,
    root_package_name: &str,
    dbt_and_adapters_namespace: Value,
    relative_file_path: &Path,
    content: &str,
    offset: &dbt_common::CodeLocationWithFile,
    unique_id: &str,
    adapter_type: AdapterType,
    skip_redundant_template_syntax_error_log: bool,
) -> FsResult<()> {
    let function_signatures = env.jinja_function_registry.clone();
    let macro_namespace_registry = env.env.get_macro_namespace_registry();
    let builtins = load_builtins_with_namespace(macro_namespace_registry)
        .map_err(|e| FsError::from_jinja_err(e, "Failed to load built-ins"))?;

    let mut typecheck_resolved_context: BTreeMap<String, Value> = BTreeMap::new();
    AdapterDispatchFunction::instance().set_adapter_type(&adapter_type.to_string().into());

    if let Some(target_package_name) = target_package_name {
        typecheck_resolved_context.insert(
            TARGET_PACKAGE_NAME.to_string(),
            Value::from(target_package_name),
        );
    }
    typecheck_resolved_context.insert(
        ROOT_PACKAGE_NAME.to_string(),
        Value::from(root_package_name.to_string()),
    );
    typecheck_resolved_context.insert(
        DBT_AND_ADAPTERS_NAMESPACE.to_string(),
        dbt_and_adapters_namespace,
    );
    // The namespace is dialect-keyed, so the typechecker needs the dialect to
    // select from it. Sourced from the env rather than threaded through every
    // caller.
    if let Some(dialect) = env.env.get_dialect().cloned() {
        typecheck_resolved_context.insert(DIALECT.to_string(), dialect);
    }

    let profile = CodeGenerationProfile::TypeCheck(
        function_signatures.clone(),
        typecheck_resolved_context.clone(),
    );

    let listener = jinja_typechecking_listener_factory.create_listener(
        arg_io,
        offset.clone(),
        noqa_comments
            .get(&DbtPath::from(relative_file_path))
            .cloned(),
        unique_id,
    );

    let source = content.to_string();
    let tmpl = match env.env.template_from_named_str_with_profile(
        relative_file_path.to_str().unwrap(),
        &source,
        profile,
    ) {
        Ok(tmpl) => tmpl,
        Err(e) => {
            if skip_redundant_template_syntax_error_log && e.kind() == ErrorKind::SyntaxError {
                return Ok(());
            }
            emit_error_log_message(
                ErrorCode::Generic,
                format!("Failed to create template: {}", e),
            );
            return Ok(());
        }
    };

    let _ = tmpl.typecheck(
        function_signatures,
        builtins,
        listener.clone(),
        typecheck_resolved_context,
    );
    listener.flush();
    jinja_typechecking_listener_factory.destroy_listener(relative_file_path, listener);

    Ok(())
}
