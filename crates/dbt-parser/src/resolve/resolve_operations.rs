use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, atomic::AtomicBool},
};

use crate::utils::NoOpConfig;
use dbt_adapter_core::AdapterType;
use dbt_common::{CodeLocationWithFile, ErrorCode, FsResult, fs_err, io_args::StaticAnalysisKind};
use dbt_common::{path::DbtPath, tracing::dbt_emit::emit_warn_log_from_fs_error};
use dbt_jinja_utils::{
    jinja_environment::JinjaEnv,
    listener::DefaultRenderingEventListenerFactory,
    phases::parse::{build_resolve_model_context, sql_resource::SqlResource},
    utils::render_sql,
};
use dbt_schemas::{
    schemas::{
        CommonAttributes, NodeBaseAttributes,
        common::{DbtChecksum, DbtQuoting},
        manifest::DbtOperation,
        project::DbtProject,
        ref_and_source::{DbtRef, DbtSourceWrapper},
    },
    state::DbtRuntimeConfig,
};
use dbt_yaml::Spanned;
use minijinja::constants::TARGET_PACKAGE_NAME;

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn resolve_operations(
    dbt_project: &DbtProject,
    package_base_path: &Path,
    project_root: &Path,
    jinja_env: &Arc<JinjaEnv>,
    global_static_analysis: Option<StaticAnalysisKind>,
    adapter_type: AdapterType,
    database: &str,
    schema: &str,
    root_project_quoting: DbtQuoting,
    package_runtime_config: Arc<DbtRuntimeConfig>,
    root_runtime_config: Arc<DbtRuntimeConfig>,
) -> FsResult<(Vec<Spanned<DbtOperation>>, Vec<Spanned<DbtOperation>>)> {
    let mut on_run_start = Vec::new();
    let mut on_run_end = Vec::new();

    for start in dbt_project.on_run_start.iter() {
        let operations: Vec<Spanned<String>> = start.clone().into();
        on_run_start.extend(new_operation(
            "on-run-start",
            &operations,
            dbt_project,
            package_base_path,
            project_root,
            jinja_env,
            global_static_analysis,
            adapter_type,
            database,
            schema,
            &root_project_quoting,
            &package_runtime_config,
            &root_runtime_config,
        )?);
    }

    for end in dbt_project.on_run_end.iter() {
        let operations: Vec<Spanned<String>> = end.clone().into();
        on_run_end.extend(new_operation(
            "on-run-end",
            &operations,
            dbt_project,
            package_base_path,
            project_root,
            jinja_env,
            global_static_analysis,
            adapter_type,
            database,
            schema,
            &root_project_quoting,
            &package_runtime_config,
            &root_runtime_config,
        )?);
    }

    Ok((on_run_start, on_run_end))
}

#[allow(clippy::too_many_arguments)]
fn new_operation(
    operation_type: &str,
    operations: &[Spanned<String>],
    dbt_project: &DbtProject,
    _package_base_path: &Path,
    _project_root: &Path,
    jinja_env: &Arc<JinjaEnv>,
    global_static_analysis: Option<StaticAnalysisKind>,
    adapter_type: AdapterType,
    database: &str,
    schema: &str,
    root_project_quoting: &DbtQuoting,
    package_runtime_config: &Arc<DbtRuntimeConfig>,
    root_runtime_config: &Arc<DbtRuntimeConfig>,
) -> FsResult<Vec<Spanned<DbtOperation>>> {
    let project_name = &dbt_project.name;
    // Hook operations always anchor on the package's own dbt_project.yml, regardless
    // of whether the package is the root project or imported via dbt_packages. dbt-core
    // emits `./dbt_project.yml` here verbatim — package-internal, not root-relative —
    // so the compiled output lands at `target/compiled/<pkg>/dbt_project.yml/hooks/…`.
    // Mirror that to keep manifest parity (downstream consumers of `original_file_path`,
    // e.g. dbt-core's `--use-v2-parser` path, otherwise nest compiled hooks at the
    // wrong location).
    let original_file_path = PathBuf::from("./dbt_project.yml");

    // Map with index
    let mut resolved_operations = Vec::new();

    for (index, operation_sql_spanned) in operations.iter().enumerate() {
        let name = format!("{project_name}-{operation_type}-{index}");
        let unique_id = format!("operation.{project_name}.{name}");
        let operation_sql = operation_sql_spanned.as_ref();

        // Create the base operation
        let mut operation = DbtOperation {
            __common_attr__: CommonAttributes {
                name: name.clone(),
                package_name: project_name.to_string(),
                // Hook `path` carries the `.sql` extension in dbt-core's manifest so
                // compiled output writes to `hooks/<name>.sql`. Without it, fusion's
                // path renders as a directory and the compiled file lands at
                // `hooks/<name>` with no extension.
                path: DbtPath::from("hooks").join(format!("{name}.sql")),
                original_file_path: DbtPath::from(&original_file_path),
                unique_id,
                fqn: vec![project_name.to_string(), "hooks".to_string(), name.clone()],
                // v1's --use-v2-parser runtime discovers hooks purely by this tag
                // (get_hooks_by_tags in task/run.py); without it hooks silently never run.
                tags: vec![operation_type.to_string()],
                checksum: DbtChecksum::hash(operation_sql.trim().as_bytes()),
                raw_code: Some(operation_sql.to_string()),
                language: Some("sql".to_string()),
                // dbt-core tags each hook with its own hook type.
                tags: vec![operation_type.to_string()],
                // Stored as a plain field (unlike `Spanned<T>`'s own span) so it
                // survives the `--partial-parse` cache's JSON round-trip; see
                // `run_operation_on_run` in dbt-tasks-sa, which relies on this to
                // attribute hook-execution errors back to their real location.
                name_span: dbt_common::Span::from_serde_span(
                    operation_sql_spanned.span().clone(),
                    original_file_path.clone(),
                ),
                ..Default::default()
            },
            __base_attr__: NodeBaseAttributes {
                alias: name,
                database: database.to_string(),
                schema: schema.to_string(),
                // A hook is not a node the user configures, so it runs on the
                // target default. This must be set rather than defaulted: the
                // adapter decides which dialect's internal-macro namespace the
                // hook resolves `dbt.run_query` and friends through, and a wrong
                // one yields an empty namespace rather than an error.
                adapter: adapter_type,
                // This node type has no `+propagate` config; nothing is published.
                propagate: Vec::new(),
                ..Default::default()
            },
            __other__: BTreeMap::new(),
        };

        // Skip empty operations
        if !operation_sql.trim().is_empty() {
            // Render and extract dependencies
            let sql_resources: Arc<Mutex<Vec<SqlResource<NoOpConfig>>>> =
                Arc::new(Mutex::new(Vec::new()));
            let execute_exists = Arc::new(AtomicBool::new(false));

            // Build operation context with tracking functions
            let mut operation_ctx = BTreeMap::new();
            operation_ctx.extend(build_resolve_model_context(
                &NoOpConfig {},
                false,
                adapter_type,
                database,
                schema,
                &operation.__common_attr__.name,
                vec![
                    package_runtime_config.inner.project_name.clone(),
                    "hooks".to_string(),
                    operation.__common_attr__.name.clone(),
                ],
                &operation.__common_attr__.package_name,
                &root_runtime_config.inner.project_name,
                *root_project_quoting,
                package_runtime_config.clone(),
                root_runtime_config.clone(),
                sql_resources.clone(),
                execute_exists,
                &operation.__common_attr__.original_file_path,
                &PathBuf::new(),
                global_static_analysis,
                None,
            ));

            // Set TARGET_PACKAGE_NAME for var lookups
            operation_ctx.insert(
                TARGET_PACKAGE_NAME.to_string(),
                minijinja::Value::from(operation.__common_attr__.package_name.clone()),
            );

            // Wrap operation SQL with reset_span() to provide proper span context for error messages
            let instruction_with_span = format!(
                "{{% do reset_span('{}', {}, {}, {}, {}, {}, {}) %}}\n{}",
                operation
                    .__common_attr__
                    .original_file_path
                    .to_string_lossy(),
                operation_sql_spanned.span().start.line as u32,
                operation_sql_spanned.span().start.column as u32,
                operation_sql_spanned.span().start.index as u32,
                operation_sql_spanned.span().end.line as u32,
                operation_sql_spanned.span().end.column as u32,
                operation_sql_spanned.span().end.index as u32,
                operation_sql,
            );

            // Render the operation SQL
            let listener_factory = DefaultRenderingEventListenerFactory::default();
            match render_sql(
                &instruction_with_span,
                jinja_env.as_ref(),
                &operation_ctx,
                &listener_factory,
                &operation.__common_attr__.original_file_path,
            ) {
                Ok(_) => {
                    // Extract refs and sources from sql_resources
                    let resources = sql_resources.lock().unwrap().clone();
                    for resource in resources {
                        match resource {
                            SqlResource::Ref((name, package, version, location)) => {
                                operation.__base_attr__.refs.push(DbtRef {
                                    name,
                                    package,
                                    version,
                                    location: Some(CodeLocationWithFile::new(
                                        location.line,
                                        location.col,
                                        location.index,
                                        operation.__common_attr__.original_file_path.to_path_buf(),
                                    )),
                                });
                            }
                            SqlResource::Source((source_name, table_name, location)) => {
                                operation.__base_attr__.sources.push(DbtSourceWrapper {
                                    source: vec![source_name, table_name],
                                    location: Some(CodeLocationWithFile::new(
                                        location.line,
                                        location.col,
                                        location.index,
                                        operation.__common_attr__.original_file_path.to_path_buf(),
                                    )),
                                });
                            }
                            _ => {
                                // Ignore other resource types
                            }
                        }
                    }

                    // Mark operation with static_analysis: Unsafe so it will always defer
                    operation.__base_attr__.static_analysis = StaticAnalysisKind::Unsafe.into();
                }
                Err(err) => {
                    // Log rendering error but don't fail the build
                    let err = fs_err!(
                        ErrorCode::Generic,
                        "Operation '{}' failed to render: {}",
                        operation.__common_attr__.name,
                        err.to_string()
                    )
                    .with_location(operation.__common_attr__.original_file_path.to_path_buf());
                    emit_warn_log_from_fs_error(err);
                }
            }
        }

        // Add the operation (with or without rendering)
        resolved_operations.push(operation_sql_spanned.clone().map(|_| operation));
    }

    Ok(resolved_operations)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use chrono_tz::Tz;
    use dbt_common::io_args::IoArgs;
    use dbt_jinja_utils::invocation_args::InvocationArgs;
    use dbt_jinja_utils::phases::parse::init::initialize_parse_jinja_environment;
    use dbt_schemas::schemas::profiles::PostgresDbConfig;
    use dbt_schemas::schemas::relations::DEFAULT_DBT_QUOTING;
    use dbt_schemas::schemas::serde::StringOrInteger;
    use std::collections::{BTreeMap, BTreeSet};

    fn runtime_config_named(project_name: &str) -> Arc<DbtRuntimeConfig> {
        let mut cfg = DbtRuntimeConfig::default();
        cfg.inner.project_name = project_name.to_string();
        Arc::new(cfg)
    }

    fn test_jinja_env() -> Arc<JinjaEnv> {
        let invocation_args = InvocationArgs::default();
        let tz_now: chrono::DateTime<Tz> = Utc::now().with_timezone(&Tz::UTC);
        Arc::new(
            initialize_parse_jinja_environment(
                "root_project",
                "profile",
                "target",
                AdapterType::Postgres,
                PostgresDbConfig {
                    port: Some(StringOrInteger::Integer(5432)),
                    database: Some("postgres".to_string()),
                    host: Some("localhost".to_string()),
                    user: Some("postgres".to_string()),
                    password: Some("postgres".to_string()),
                    schema: Some("schema".to_string()),
                    ..Default::default()
                }
                .into(),
                vec![AdapterType::Postgres],
                DEFAULT_DBT_QUOTING,
                BTreeMap::new(),
                BTreeMap::new(),
                BTreeMap::new(),
                BTreeMap::new(),
                tz_now,
                &invocation_args,
                BTreeSet::from(["root_project".to_string(), "dep_pkg".to_string()]),
                IoArgs::default(),
                None,
            )
            .unwrap(),
        )
    }

    /// Regression: `resolve_operations` previously received a single `Arc<DbtRuntimeConfig>`
    /// parameter misleadingly named `root_runtime_config`, but the caller in `resolver.rs`
    /// actually passed each package's OWN config there. So `context.project_name` inside a
    /// dependency package's on-run-start/on-run-end hook resolved to the dependency's own name
    /// instead of the true root project's, unlike dbt Core (`generate_runtime_model_context`
    /// always uses the root `RuntimeConfig`). This is the same root-vs-package gap as #13819,
    /// found in the process of fixing it — split the parameter into a genuinely
    /// package-scoped one (for the hook's own `fqn[0]`) and a genuinely root-scoped one (for
    /// `context.project_name` and `ref`/`source`/`metric`'s `.config`).
    #[test]
    fn dependency_package_hook_reads_root_project_name() {
        let dbt_project: DbtProject = dbt_yaml::from_str(
            r#"
name: dep_pkg
version: '1.0'
config-version: 2
on-run-start:
  - "{{ ref(context.project_name) }}"
"#,
        )
        .unwrap();

        let jinja_env = test_jinja_env();
        let package_runtime_config = runtime_config_named("dep_pkg");
        let root_runtime_config = runtime_config_named("root_project");

        let (on_run_start, _on_run_end) = resolve_operations(
            &dbt_project,
            Path::new("dep_pkg"),
            Path::new("/tmp/project"),
            &jinja_env,
            None,
            AdapterType::Postgres,
            "db",
            "schema",
            DEFAULT_DBT_QUOTING,
            package_runtime_config,
            root_runtime_config,
        )
        .unwrap();

        let hook = on_run_start
            .first()
            .expect("on-run-start hook should be resolved")
            .as_ref();
        let hook_ref = hook
            .__base_attr__
            .refs
            .first()
            .expect("ref(context.project_name) should be recorded");
        assert_eq!(
            hook_ref.name, "root_project",
            "context.project_name inside a dependency package's hook must resolve to the \
             ROOT project's name, not the dependency's own"
        );
    }
}
