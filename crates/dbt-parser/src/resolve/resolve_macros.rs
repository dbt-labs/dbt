use dbt_adapter_core::AdapterType;
use dbt_common::ErrorCode;
use dbt_common::FsResult;
use dbt_common::io_args::IoArgs;
use dbt_common::path::{DbtPath, resource_extension};
use dbt_common::stdfs::diff_paths;
use dbt_common::tracing::dbt_emit::{emit_warn_log_from_fs_error, emit_warn_log_message};
use dbt_common::tracing::event_info::store_event_attributes;
use dbt_common::tracing::span_info::record_span_status;
use dbt_common::{err, fs_err};
use dbt_jinja_utils::jinja_environment::JinjaEnv;
use dbt_jinja_utils::listener::DefaultJinjaTypeCheckEventListenerFactory;
use dbt_jinja_utils::phases::parse::sql_resource::SqlResource;
use dbt_jinja_utils::serde::into_typed_with_jinja;
use dbt_schemas::schemas::macros::DbtDocsMacro;
use dbt_schemas::schemas::macros::DbtMacro;
use dbt_schemas::schemas::macros::MacroArgument;
use dbt_schemas::schemas::macros::MacroConfig;
use dbt_schemas::schemas::macros::MacroDependsOn;
use dbt_schemas::schemas::properties::MacrosProperties;
use dbt_schemas::state::DbtAsset;
use dbt_telemetry::GenericOpExecuted;
use minijinja::Value as MinijinjaValue;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::resolve::resolve_properties::MinimalPropertiesEntry;

use crate::utils::parse_macro_statements;

/// Resolve docs macros from a list of docs macro files
pub fn resolve_docs_macros(
    io: &IoArgs,
    docs_macro_files: &[DbtAsset],
    embedded_contents: Option<&HashMap<DbtPath, String>>,
) -> FsResult<BTreeMap<String, DbtDocsMacro>> {
    let mut docs_map: BTreeMap<String, DbtDocsMacro> = BTreeMap::new();

    for docs_asset in docs_macro_files {
        if let Err(err) = process_docs_macro_file(io, &mut docs_map, docs_asset, embedded_contents)
        {
            let err = err.with_location(docs_asset.path.clone());
            emit_warn_log_from_fs_error(err);
        }
    }

    Ok(docs_map)
}

fn process_docs_macro_file(
    io: &IoArgs,
    docs_map: &mut BTreeMap<String, DbtDocsMacro>,
    docs_asset: &DbtAsset,
    embedded_contents: Option<&HashMap<DbtPath, String>>,
) -> FsResult<()> {
    let docs_file_path = docs_asset.base_path.join(&docs_asset.path);
    let docs_macro = read_file_content(&docs_asset.path, &docs_file_path, embedded_contents)?;

    let relative_docs_file_path = &diff_paths(&docs_file_path, &io.in_dir)?;
    let resources = parse_macro_statements(&docs_macro, relative_docs_file_path, &["docs"])?;
    if resources.is_empty() {
        return Ok(());
    }

    let package_name = &docs_asset.package_name;
    for resource in resources {
        match resource {
            SqlResource::Doc(name, span) => {
                let unique_id = format!("doc.{package_name}.{name}");
                let part = &docs_macro[span.start_offset as usize..span.end_offset as usize];
                if let Some(existing_doc) = docs_map.get(&unique_id) {
                    return err!(
                        ErrorCode::Unexpected,
                        "dbt found two docs with the same name: '{}' in files: '{}' and '{}'",
                        name,
                        docs_asset.path.display(),
                        existing_doc.path.display()
                    );
                }
                docs_map.insert(
                    unique_id.clone(),
                    DbtDocsMacro {
                        name: name.clone(),
                        package_name: package_name.to_string(),
                        path: DbtPath::from(&docs_asset.path),
                        original_file_path: DbtPath::from(relative_docs_file_path),
                        unique_id,
                        block_contents: part.trim().to_string(),
                    },
                );
            }
            _ => {
                return err!(
                    ErrorCode::Unexpected,
                    "Encountered unexpected resource in docs file: {}",
                    docs_asset.path.display()
                );
            }
        }
    }

    Ok(())
}

/// Resolve macros from a list of macro files
pub fn resolve_macros(
    macro_files: &[&DbtAsset],
    embedded_contents: Option<&HashMap<DbtPath, String>>,
) -> FsResult<HashMap<String, DbtMacro>> {
    let mut nodes = HashMap::new();

    for dbt_asset in macro_files {
        let DbtAsset {
            path: macro_file,
            original_path: _original_path,
            base_path,
            package_name,
        } = dbt_asset;
        let ext = resource_extension(macro_file).map(str::to_ascii_lowercase);
        if ext.as_deref() == Some("jinja") || ext.as_deref() == Some("sql") {
            let macro_file_path = DbtPath::from(base_path.join(macro_file));
            let macro_sql = read_file_content(macro_file, &macro_file_path, embedded_contents)?;
            let relative_macro_file_path = DbtPath::from(diff_paths(&macro_file_path, base_path)?);
            let resources = parse_macro_statements(
                &macro_sql,
                &relative_macro_file_path,
                &["macro", "test", "materialization", "snapshot"],
            )?;

            if resources.is_empty() {
                continue;
            }

            for resource in resources {
                match resource {
                    SqlResource::Test(name, span, args, macro_name_span) => {
                        let unique_id = format!("macro.{package_name}.{name}");
                        let split_macro_sql =
                            &macro_sql[span.start_offset as usize..span.end_offset as usize];

                        let dbt_macro = DbtMacro {
                            name: name.clone(),
                            package_name: package_name.clone(),
                            path: DbtPath::from(macro_file),
                            original_file_path: relative_macro_file_path.clone(),
                            absolute_path: macro_file_path.clone(),
                            span: Some(span),
                            unique_id: unique_id.clone(),
                            macro_sql: split_macro_sql.to_string(),
                            depends_on: MacroDependsOn { macros: vec![] },
                            // Description is patched from YAML schema files via apply_macro_patches
                            description: String::new(),
                            meta: BTreeMap::new(),
                            docs: None,
                            config: MacroConfig::default(),
                            patch_path: None,
                            funcsign: None,
                            supported_languages: None,
                            args: args.clone(),
                            arguments: vec![],
                            macro_name_span: Some(macro_name_span),
                            __other__: BTreeMap::new(),
                        };

                        nodes.insert(unique_id, dbt_macro);
                    }
                    SqlResource::Macro(name, span, func_sign, args, macro_name_span) => {
                        let unique_id = format!("macro.{package_name}.{name}");
                        let split_macro_sql =
                            &macro_sql[span.start_offset as usize..span.end_offset as usize];

                        let dbt_macro = DbtMacro {
                            name: name.clone(),
                            package_name: package_name.clone(),
                            path: DbtPath::from(macro_file),
                            original_file_path: relative_macro_file_path.clone(),
                            absolute_path: macro_file_path.clone(),
                            span: Some(span),
                            unique_id: unique_id.clone(),
                            macro_sql: split_macro_sql.to_string(),
                            depends_on: MacroDependsOn { macros: vec![] },
                            // Description is patched from YAML schema files via apply_macro_patches
                            description: String::new(),
                            meta: BTreeMap::new(),
                            docs: None,
                            config: MacroConfig::default(),
                            patch_path: None,
                            funcsign: func_sign.clone(),
                            supported_languages: None,
                            args: args.clone(),
                            arguments: vec![],
                            macro_name_span: Some(macro_name_span),
                            __other__: BTreeMap::new(),
                        };

                        nodes.insert(unique_id, dbt_macro);
                    }
                    SqlResource::Materialization(
                        name,
                        _,
                        supported_languages,
                        span,
                        macro_name_span,
                    ) => {
                        let split_macro_sql =
                            &macro_sql[span.start_offset as usize..span.end_offset as usize];
                        // TODO: Return the adapter type with the SqlResource (for now, default always)
                        let unique_id = format!("macro.{package_name}.{name}");
                        let dbt_macro = DbtMacro {
                            name: name.clone(),
                            package_name: package_name.clone(),
                            path: DbtPath::from(macro_file),
                            original_file_path: relative_macro_file_path.clone(),
                            absolute_path: macro_file_path.clone(),
                            span: Some(span),
                            unique_id: unique_id.clone(),
                            macro_sql: split_macro_sql.to_string(),
                            depends_on: MacroDependsOn { macros: vec![] },
                            description: String::new(),
                            meta: BTreeMap::new(),
                            docs: None,
                            config: MacroConfig::default(),
                            patch_path: None,
                            funcsign: None,
                            supported_languages,
                            args: vec![],
                            arguments: vec![],
                            macro_name_span: Some(macro_name_span),
                            __other__: BTreeMap::new(),
                        };

                        nodes.insert(unique_id, dbt_macro);
                    }
                    SqlResource::Snapshot(name, span, macro_name_span) => {
                        let unique_id = format!("snapshot.{package_name}.{name}");
                        let split_macro_sql =
                            &macro_sql[span.start_offset as usize..span.end_offset as usize];

                        let dbt_macro = DbtMacro {
                            name: name.clone(),
                            package_name: package_name.clone(),
                            path: DbtPath::from(macro_file),
                            original_file_path: relative_macro_file_path.clone(),
                            absolute_path: macro_file_path.clone(),
                            span: Some(span),
                            unique_id: unique_id.clone(),
                            macro_sql: split_macro_sql.to_string(),
                            depends_on: MacroDependsOn { macros: vec![] },
                            // Description is patched from YAML schema files via apply_macro_patches
                            description: String::new(),
                            meta: BTreeMap::new(),
                            docs: None,
                            config: MacroConfig::default(),
                            patch_path: None,
                            funcsign: None,
                            supported_languages: None,
                            args: vec![],
                            arguments: vec![],
                            macro_name_span: Some(macro_name_span),
                            __other__: BTreeMap::new(),
                        };

                        nodes.insert(unique_id, dbt_macro);
                    }
                    _ => {
                        return err!(
                            ErrorCode::MacroSyntaxInvalid,
                            "Refs, sources, configs and other resources are not allowed in macros. Path: {}",
                            macro_file.display()
                        );
                    }
                }
            }
        }
    }

    Ok(nodes)
}

/// Read file content from the embedded cache or from disk.
fn read_file_content(
    relative_path: &Path,
    absolute_path: &Path,
    embedded_contents: Option<&HashMap<DbtPath, String>>,
) -> FsResult<String> {
    match embedded_contents.and_then(|m| m.get(&DbtPath::from(relative_path))) {
        Some(content) => Ok(content.clone()),
        None => fs::read_to_string(absolute_path).map_err(|e| {
            fs_err!(
                code => ErrorCode::IoError,
                loc => absolute_path.to_path_buf(),
                "Failed to read file: {}", e
            )
        }),
    }
}

/// Returns true if the given type string is a valid dbt macro argument type.
///
/// Grammar (dbt Core v1.10+):
/// ```text
/// type      = atom ("|" atom)*
/// atom      = primitive | "list" "[" type "]" | "optional" "[" type "]"
///           | "dict" "[" type "," type "]"
/// primitive = "string" | "str" | "boolean" | "bool" | "integer" | "int"
///           | "float" | "any" | "relation" | "column"
/// ```
/// Union (`|`) and container separators (`,`) are only recognised at bracket
/// depth 0, so `list[str|int]` and `dict[str, list[int|bool]]` are both valid.
pub fn is_valid_macro_arg_type(s: &str) -> bool {
    /// Split `s` on `sep` only at bracket depth 0.
    fn split_shallow(s: &str, sep: char) -> Vec<&str> {
        let mut parts = Vec::new();
        let mut depth: usize = 0;
        let mut start = 0;
        for (i, c) in s.char_indices() {
            match c {
                '[' => depth += 1,
                ']' => depth = depth.saturating_sub(1),
                _ if c == sep && depth == 0 => {
                    parts.push(&s[start..i]);
                    start = i + c.len_utf8();
                }
                _ => {}
            }
        }
        parts.push(&s[start..]);
        parts
    }

    /// A type is one or more atoms joined by `|` at depth 0.
    fn parse_type(s: &str) -> bool {
        let s = s.trim();
        split_shallow(s, '|')
            .iter()
            .all(|part| parse_atom(part.trim()))
    }

    /// An atom is a primitive or a bracketed container.
    fn parse_atom(s: &str) -> bool {
        let s = s.trim();
        if matches!(
            s,
            "string"
                | "str"
                | "boolean"
                | "bool"
                | "integer"
                | "int"
                | "float"
                | "any"
                | "relation"
                | "column"
                | "list"
                | "dict"
                | "optional"
        ) {
            return true;
        }
        if let Some(bracket) = s.find('[') {
            if !s.ends_with(']') {
                return false;
            }
            let prefix = s[..bracket].trim();
            let inner = &s[bracket + 1..s.len() - 1];
            match prefix {
                "list" | "optional" => parse_type(inner),
                "dict" => {
                    let parts = split_shallow(inner, ',');
                    parts.len() == 2 && parse_type(parts[0].trim()) && parse_type(parts[1].trim())
                }
                _ => false,
            }
        } else {
            false
        }
    }

    parse_type(&s.to_lowercase())
}

/// Apply macro patches from YAML schema files to the resolved macros.
/// This updates description and patch_path fields based on YAML macro definitions.
///
/// `dependency_package_name` is `Some` when `package_name` is not the root project, which
/// downgrades strict-parse diagnostics from the render.
///
/// When `validate_macro_args` is true (from dbt_project.yml flags), this function also:
/// - Warns when YAML argument names don't match the actual Jinja macro parameters
/// - Warns when YAML argument `type` values use unsupported or malformed type syntax
/// - Infers undocumented parameters from the Jinja definition and adds them to `arguments`
pub fn apply_macro_patches(
    macros: &mut BTreeMap<String, DbtMacro>,
    macro_properties: &BTreeMap<String, MinimalPropertiesEntry>,
    package_name: &str,
    jinja_env: &JinjaEnv,
    base_ctx: &BTreeMap<String, MinijinjaValue>,
    dependency_package_name: Option<&str>,
    validate_macro_args: bool,
) -> FsResult<()> {
    for (macro_name, props_entry) in macro_properties {
        // Build the unique_id to look up the macro
        let unique_id = format!("macro.{package_name}.{macro_name}");

        // Check if this macro exists in our resolved macros
        if let Some(dbt_macro) = macros.get_mut(&unique_id) {
            // Parse the macro properties with Jinja rendering (for doc blocks)
            let macro_props: MacrosProperties = into_typed_with_jinja(
                props_entry.schema_value.clone(),
                false,
                jinja_env,
                base_ctx,
                &[],
                dependency_package_name,
                true,
            )?;

            // Update description if provided
            if let Some(description) = macro_props.description {
                dbt_macro.description = description;
            }

            // Merge meta: top-level and config.meta are merged, config.meta wins on conflicts
            let top_meta = macro_props.meta.unwrap_or_default();
            let config_meta = macro_props
                .config
                .as_ref()
                .and_then(|c| c.meta.clone())
                .unwrap_or_default();
            if !top_meta.is_empty() || !config_meta.is_empty() {
                let mut merged = top_meta;
                merged.extend(config_meta);
                dbt_macro.meta = merged.into_iter().collect();
            }

            // Update docs if provided (config.docs takes precedence over top-level docs)
            let docs = macro_props
                .config
                .as_ref()
                .and_then(|c| c.docs.clone())
                .or(macro_props.docs);
            if docs.is_some() {
                dbt_macro.docs = docs;
            }

            // Reference: https://github.com/dbt-labs/dbt-mantle/blob/144da7909580abfaac1604b956c2e423d1baf2ad/core/dbt/parser/schemas.py#L1506-L1509
            dbt_macro.config.meta = dbt_macro.meta.clone();
            dbt_macro.config.docs = dbt_macro.docs.clone().unwrap_or_default();

            // Update arguments if provided in YAML
            if let Some(yml_arguments) = macro_props.arguments {
                let mut arguments: Vec<MacroArgument> = yml_arguments
                    .into_iter()
                    .map(|arg| MacroArgument {
                        name: arg.name,
                        type_: arg.type_,
                        description: arg.description.unwrap_or_default(),
                    })
                    .collect();

                if validate_macro_args {
                    let jinja_arg_names: Vec<&str> =
                        dbt_macro.args.iter().map(|a| a.name.as_str()).collect();

                    // Warn about YAML argument names not present in Jinja definition
                    for yml_arg in &arguments {
                        if !jinja_arg_names.contains(&yml_arg.name.as_str()) {
                            emit_warn_log_message(
                                ErrorCode::ValidateMacroArgs,
                                format!(
                                    "{}: Macro \"{macro_name}\": documented argument \"{}\" not found \
                                     in macro definition",
                                    props_entry.relative_path.display(),
                                    yml_arg.name
                                ),
                            );
                        }
                    }

                    // Warn about invalid type strings
                    for yml_arg in &arguments {
                        if let Some(type_str) = &yml_arg.type_ {
                            if !is_valid_macro_arg_type(type_str) {
                                emit_warn_log_message(
                                    ErrorCode::ValidateMacroArgs,
                                    format!(
                                        "{}: Macro \"{macro_name}\": argument \"{}\" has unsupported \
                                         type \"{type_str}\". Supported types are: string, str, \
                                         boolean, bool, integer, int, float, any, relation, \
                                         column, list[T], dict[K,V], optional[T], T1|T2|...",
                                        props_entry.relative_path.display(),
                                        yml_arg.name
                                    ),
                                );
                            }
                        }
                    }

                    // Infer undocumented Jinja args and append them to the arguments list
                    let documented_names: HashSet<String> =
                        arguments.iter().map(|a| a.name.clone()).collect();
                    for jinja_arg in &dbt_macro.args {
                        if !documented_names.contains(&jinja_arg.name) {
                            arguments.push(MacroArgument {
                                name: jinja_arg.name.clone(),
                                type_: None,
                                description: String::new(),
                            });
                        }
                    }
                }

                dbt_macro.arguments = arguments;
            } else if validate_macro_args && !dbt_macro.args.is_empty() {
                // No YAML arguments at all — infer all from Jinja definition
                dbt_macro.arguments = dbt_macro
                    .args
                    .iter()
                    .map(|a| MacroArgument {
                        name: a.name.clone(),
                        type_: None,
                        description: String::new(),
                    })
                    .collect();
            }

            // Set patch_path to indicate this macro was patched from a YAML file
            // Format: package_name://path/to/schema.yml
            let patch_path = PathBuf::from(format!(
                "{}://{}",
                package_name,
                props_entry.relative_path.display()
            ));
            dbt_macro.patch_path = Some(patch_path);
        } else {
            // Emit a warning when YAML references a macro that doesn't exist
            emit_warn_log_message(
                ErrorCode::MacroPatchNotFound,
                format!(
                    "Found patch for macro \"{}\" which was not found",
                    macro_name
                ),
            );
        }
    }

    Ok(())
}

/// Typecheck each macro body to discover macro→macro call edges and backfill
/// `depends_on.macros` on every macro node.
///
/// Must be called after the Jinja env is fully built (so all macros are
/// registered) and after `apply_macro_patches`. Snapshot stubs
/// (unique_id starts with `"snapshot."`) are skipped — they have their own
/// processing path and no meaningful Jinja body to analyse.
#[tracing::instrument(
    level = "debug",
    skip_all,
    fields(
        _e = ?store_event_attributes(GenericOpExecuted::new(
            "resolve.typecheck_macros".to_string(),
            "typechecking macros".to_string(),
            None,
        )),
    )
)]
pub fn typecheck_macros(
    io: &IoArgs,
    macros: &mut BTreeMap<String, DbtMacro>,
    jinja_env: Arc<JinjaEnv>,
    adapter_type: AdapterType,
    root_package_name: &str,
    dbt_and_adapters_namespace: MinijinjaValue,
) -> FsResult<()> {
    let factory = Arc::new(DefaultJinjaTypeCheckEventListenerFactory::default());
    let noqa = HashMap::new();

    // Some macro bodies (especially third-party/vendored ones) can hit panics
    // deep inside the type-inference VM. Fault-isolate each macro so a single
    // bad macro can't crash the whole parse/run, and suppress the default
    // panic hook's backtrace since these are expected/handled failures.
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    for (unique_id, dbt_macro) in macros.iter() {
        if unique_id.starts_with("snapshot.") {
            continue;
        }
        let file_path = dbt_macro.original_file_path.as_path().to_path_buf();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = dbt_jinja_utils::typecheck::typecheck(
                io,
                jinja_env.clone(),
                &noqa,
                factory.clone(),
                // `macro_namespace_template_resolver` resolves a default-valued
                // macro parameter's own signature via `{target_package}.{name}`
                // first; passing `None` here made every "current package"
                // candidate fall back to the hardcoded `"dbt"` package, so any
                // package macro (e.g. `dbt_utils.default__unpivot`) with a
                // default-valued argument couldn't resolve its own signature
                // and hit a codegen path that pushes no value for that
                // argument at all -- a stack underflow at `compile_assignment`
                // once typechecking reached it. Passing the macro's real
                // package here lets that resolution succeed.
                Some(dbt_macro.package_name.clone()),
                root_package_name,
                dbt_and_adapters_namespace.clone(),
                &file_path,
                &dbt_macro.macro_sql,
                &dbt_common::CodeLocationWithFile::new(1, 1, 0, file_path.clone()),
                unique_id,
                adapter_type,
                true,
            );
        }));
    }
    std::panic::set_hook(prev_hook);

    let all_depends_on = factory.depends_on();
    for (unique_id, dbt_macro) in macros.iter_mut() {
        if let Some(deps) = all_depends_on.get(unique_id) {
            dbt_macro.depends_on.macros = deps.iter().cloned().collect();
        }
    }

    // Compute the transitive closure of "reaches an introspective adapter
    // call" over the macro call graph (`all_depends_on`), seeded from the
    // macros observed directly calling one (`direct_introspective`). Used by
    // `JinjaRenderMode::Symbolic` to treat a whole macro call as an opaque
    // taint boundary -- see `JinjaEnv::introspective_macros`'s doc comment
    // for why call-site-level fine-grained propagation alone isn't enough.
    let mut introspective: HashSet<String> =
        factory.direct_introspective().iter().cloned().collect();
    let mut worklist: Vec<String> = introspective.iter().cloned().collect();
    while let Some(callee) = worklist.pop() {
        for (caller, callees) in all_depends_on.iter() {
            if callees.contains(&callee) && introspective.insert(caller.clone()) {
                worklist.push(caller.clone());
            }
        }
    }
    jinja_env.set_introspective_macros(introspective);

    // `instrument` opens the span but never writes dbt's span status; this
    // function has a single exit, so record success here.
    record_span_status(&tracing::Span::current(), None);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_common::tracing::fs_error_log::FsErrorLog;
    use dbt_tracing::{
        SeverityNumber,
        init::create_tracing_subcriber_with_layer,
        layer::ConsumerLayer,
        test_support::mocks::{TestLayer, test_data_layer},
    };
    use dbt_yaml::Span;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::tempdir;

    #[test]
    fn invalid_markdown_doc_reports_warning_and_continues() -> FsResult<()> {
        let tmp_dir = tempdir().unwrap();
        let base_path = tmp_dir.path().to_path_buf();
        fs::create_dir_all(base_path.join("models")).unwrap();

        let (test_layer, _, _, log_records) = TestLayer::new();
        let subscriber = create_tracing_subcriber_with_layer(
            tracing::level_filters::LevelFilter::TRACE,
            test_data_layer(
                1,
                None,
                false,
                std::iter::empty(),
                std::iter::once(Box::new(test_layer) as ConsumerLayer),
            ),
            &[],
        )
        .expect("test tracing subscriber should be valid");

        struct InvalidCase<'a> {
            name: &'a str,
            files: Vec<(&'a str, &'a str)>,
            expected_code: ErrorCode,
            expected_warning_paths: Vec<&'a str>,
        }

        let invalid_cases = vec![
            InvalidCase {
                name: "missing_endblock",
                files: vec![("missing_endblock.md", "{% docs broken %}missing endblock")],
                expected_code: ErrorCode::MacroSyntaxInvalid,
                expected_warning_paths: vec!["models/missing_endblock.md"],
            },
            InvalidCase {
                name: "duplicate_name",
                files: vec![
                    (
                        "dup_first.md",
                        r#"
                    {% docs dup_doc %}
                    first
                    {% enddocs %}
                    "#,
                    ),
                    (
                        "dup_second.md",
                        r#"
                    {% docs dup_doc %}
                    second
                    {% enddocs %}
                    "#,
                    ),
                ],
                expected_code: ErrorCode::Unexpected,
                expected_warning_paths: vec!["models/dup_second.md"],
            },
        ];

        tracing::subscriber::with_default(subscriber, || {
            let io_args = IoArgs {
                in_dir: base_path.clone(),
                ..Default::default()
            };

            for case in invalid_cases {
                let mut assets = Vec::new();
                for (file_name, content) in &case.files {
                    let file_path = PathBuf::from(format!("models/{file_name}"));
                    fs::write(base_path.join(&file_path), content).unwrap();
                    assets.push(DbtAsset {
                        base_path: base_path.clone(),
                        original_path: file_path.clone(),
                        path: file_path,
                        package_name: "pkg".to_string(),
                    });
                }

                let previous_record_count = log_records.lock().unwrap().len();
                let _ = resolve_docs_macros(&io_args, &assets, None)?;
                let records = log_records.lock().unwrap();
                let warnings = &records[previous_record_count..];
                assert_eq!(
                    warnings.len(),
                    case.expected_warning_paths.len(),
                    "expected one warning for case {}",
                    case.name
                );
                for (warning, expected_path) in
                    warnings.iter().zip(case.expected_warning_paths.iter())
                {
                    assert_eq!(warning.severity_number, SeverityNumber::Warn);
                    let warning = warning
                        .attributes
                        .downcast_ref::<FsErrorLog>()
                        .expect("warning should retain its FsError");
                    assert_eq!(warning.get_fs_error().code, case.expected_code);
                    assert_eq!(
                        warning
                            .get_fs_error()
                            .location
                            .as_ref()
                            .map(|loc| loc.file.as_ref().clone()),
                        Some(PathBuf::from(expected_path)),
                        "expected warning location for case {}",
                        case.name
                    );
                }
            }

            // Positive case
            let valid_path = PathBuf::from("models/valid_doc.md");
            fs::write(
                base_path.join(&valid_path),
                "{% docs ok_doc %}all good{% enddocs %}",
            )
            .unwrap();

            let docs_asset = DbtAsset {
                base_path: base_path.clone(),
                original_path: valid_path.clone(),
                path: valid_path,
                package_name: "pkg".to_string(),
            };

            let previous_record_count = log_records.lock().unwrap().len();
            let docs = resolve_docs_macros(&io_args, &[docs_asset], None)?;
            assert!(
                docs.contains_key("doc.pkg.ok_doc"),
                "expected valid doc to be collected"
            );
            assert_eq!(
                log_records.lock().unwrap().len(),
                previous_record_count,
                "did not expect warnings for valid doc"
            );

            Ok(())
        })
    }

    #[test]
    fn test_is_valid_macro_arg_type_primitives() {
        for ty in &[
            "string", "str", "boolean", "bool", "integer", "int", "float", "any", "relation",
            "column",
        ] {
            assert!(
                is_valid_macro_arg_type(ty),
                "{ty} should be a valid primitive type"
            );
        }
        assert!(!is_valid_macro_arg_type("text"));
        assert!(!is_valid_macro_arg_type("varchar"));
        assert!(!is_valid_macro_arg_type(""));
    }

    #[test]
    fn test_is_valid_macro_arg_type_containers() {
        assert!(is_valid_macro_arg_type("list[string]"));
        assert!(is_valid_macro_arg_type("list[int]"));
        assert!(is_valid_macro_arg_type("optional[boolean]"));
        assert!(is_valid_macro_arg_type("dict[str, int]"));
        assert!(is_valid_macro_arg_type("dict[str, list[int]]"));
        assert!(is_valid_macro_arg_type("list[dict[str, int]]"));
        assert!(is_valid_macro_arg_type("optional[list[string]]"));

        // Invalid containers
        assert!(!is_valid_macro_arg_type("list[]"));
        assert!(!is_valid_macro_arg_type("list[unknown]"));
        assert!(!is_valid_macro_arg_type("dict[str]")); // missing second type arg
        assert!(!is_valid_macro_arg_type("map[str, int]")); // unsupported container
        assert!(!is_valid_macro_arg_type("list[str")); // missing closing bracket
    }

    #[test]
    // Regression test: the extension guard in resolve_macros used a case-sensitive OsStr
    // comparison, so files named SNAPSHOT.SQL (uppercase) were silently skipped. The
    // {% snapshot %} block was never parsed, no snapshot.* macro entry was produced, and
    // resolve_snapshots therefore never registered the snapshot node — causing dbt1048
    // "Ref not found" on any model that ref()s it.
    fn test_resolve_macros_uppercase_sql_extension() -> FsResult<()> {
        let tmp = tempdir().unwrap();
        let base_path = tmp.path().to_path_buf();
        fs::create_dir_all(base_path.join("snapshots")).unwrap();

        let snapshot_sql = r#"
{% snapshot snapshot_actual %}
{{
    config(
        schema='snapshots',
        unique_key='id',
        strategy='timestamp',
        updated_at='updated_at',
    )
}}
select 1 as id, current_timestamp as updated_at
{% endsnapshot %}
"#;

        // Write the snapshot file with an UPPERCASE .SQL extension — the triggering condition.
        let asset_path = PathBuf::from("snapshots/SNAPSHOT.SQL");
        fs::write(base_path.join(&asset_path), snapshot_sql).unwrap();

        let asset = DbtAsset {
            base_path,
            original_path: asset_path.clone(),
            path: asset_path,
            package_name: "test_pkg".to_string(),
        };

        let result = resolve_macros(&[&asset], None)?;

        // parse_macro_statements prefixes snapshot block names with "snapshot_", so
        // {% snapshot snapshot_actual %} becomes macro uid "snapshot.pkg.snapshot_snapshot_actual".
        // resolve_snapshots then strips that prefix to recover "snapshot_actual" as the node name.
        assert!(
            result.contains_key("snapshot.test_pkg.snapshot_snapshot_actual"),
            "snapshot defined in SNAPSHOT.SQL (uppercase extension) was not registered — \
             the extension guard in resolve_macros is case-sensitive and skipped the file. \
             Got keys: {:?}",
            result.keys().collect::<Vec<_>>()
        );

        Ok(())
    }

    #[test]
    fn test_apply_macro_patches_populates_config_meta_and_docs() -> FsResult<()> {
        let jinja_env = JinjaEnv::new(minijinja::Environment::new());
        let base_ctx: BTreeMap<String, MinijinjaValue> = BTreeMap::new();

        let unique_id = "macro.test_pkg.my_macro".to_string();
        let dummy_span = minijinja::machinery::Span::default();
        let mut macros = BTreeMap::from([(
            unique_id.clone(),
            DbtMacro {
                name: "my_macro".to_string(),
                package_name: "test_pkg".to_string(),
                path: DbtPath::from("macros/my_macro.sql"),
                original_file_path: DbtPath::from("macros/my_macro.sql"),
                absolute_path: DbtPath::default(),
                span: Some(dummy_span),
                unique_id: unique_id.clone(),
                macro_sql: "{% macro my_macro() %}{% endmacro %}".to_string(),
                depends_on: MacroDependsOn::default(),
                description: String::new(),
                meta: BTreeMap::new(),
                docs: None,
                config: MacroConfig::default(),
                patch_path: None,
                funcsign: None,
                supported_languages: None,
                args: vec![],
                arguments: vec![],
                macro_name_span: Some(dummy_span),
                __other__: BTreeMap::new(),
            },
        )]);

        let yaml_str = "name: my_macro\nmeta:\n  owner: alice\ndocs:\n  show: false\n";
        let schema_value: dbt_yaml::Value = dbt_yaml::from_str(yaml_str).unwrap();

        use crate::resolve::resolve_properties::MinimalPropertiesEntry;
        let props_entry = MinimalPropertiesEntry {
            name: "my_macro".to_string(),
            name_span: Span::default(),
            relative_path: PathBuf::from("macros/schema.yml"),
            schema_value,
            table_value: None,
            version_info: None,
            duplicate_paths: vec![],
        };
        let macro_properties = BTreeMap::from([("my_macro".to_string(), props_entry)]);

        apply_macro_patches(
            &mut macros,
            &macro_properties,
            "test_pkg",
            &jinja_env,
            &base_ctx,
            None,
            false,
        )?;

        let patched = macros.get(&unique_id).expect("macro still present");

        // config.meta mirrors the patched meta
        let owner = patched
            .config
            .meta
            .get("owner")
            .expect("owner key in config.meta");
        assert_eq!(owner.as_str(), Some("alice"));

        // config.docs mirrors the patched docs (show: false)
        assert!(
            !patched.config.docs.show,
            "config.docs.show should be false"
        );

        Ok(())
    }

    #[test]
    fn typecheck_macros_isolates_panic_and_still_populates_other_macros() -> FsResult<()> {
        use minijinja::compiler::typecheck::FunctionRegistry;
        use minijinja::machinery::Span as MinijinjaSpan;
        use minijinja::{Argument, DynTypeObject, Environment, Type, UserDefinedFunctionType};

        fn make_macro(unique_id: &str, sql: &str) -> DbtMacro {
            let path = DbtPath::from(PathBuf::from(format!("macros/{unique_id}.sql")));
            DbtMacro {
                name: unique_id.rsplit('.').next().unwrap().to_string(),
                package_name: "pkg".to_string(),
                path: path.clone(),
                original_file_path: path,
                absolute_path: DbtPath::from(PathBuf::new()),
                span: None,
                unique_id: unique_id.to_string(),
                macro_sql: sql.to_string(),
                depends_on: MacroDependsOn { macros: vec![] },
                description: String::new(),
                meta: BTreeMap::new(),
                docs: None,
                config: MacroConfig::default(),
                patch_path: None,
                funcsign: None,
                supported_languages: None,
                args: vec![],
                arguments: vec![],
                macro_name_span: None,
                __other__: BTreeMap::new(),
            }
        }

        // Register a "helper" macro directly in the function registry so calls to
        // it are resolved without going through the (deliberately broken, see
        // below) macro-namespace resolver.
        let mut function_registry: FunctionRegistry = FunctionRegistry::new();
        function_registry.insert(
            "helper".to_string(),
            DynTypeObject::new(Arc::new(UserDefinedFunctionType::new(
                "helper",
                Vec::<Argument>::new(),
                Type::Any { hard: false },
                Path::new("macros/helper.sql"),
                &MinijinjaSpan::default(),
                "macro.pkg.helper",
            ))),
        );

        let mut jinja_env = JinjaEnv::new(Environment::new());
        jinja_env.jinja_function_registry = Arc::new(function_registry);
        let jinja_env = Arc::new(jinja_env);

        // Alphabetically, the panicking macro sorts between the other two so we
        // exercise both "processed before" and "processed after" ordering.
        let mut macros = BTreeMap::new();
        macros.insert(
            "macro.pkg.a_before".to_string(),
            make_macro("macro.pkg.a_before", "{{ helper() }}"),
        );
        macros.insert(
            "macro.pkg.m_panicking".to_string(),
            // Calls a name that isn't in the function registry, forcing a fall
            // through to the macro-namespace resolver below.
            make_macro("macro.pkg.m_panicking", "{{ some_unregistered_macro() }}"),
        );
        macros.insert(
            "macro.pkg.z_after".to_string(),
            make_macro("macro.pkg.z_after", "{{ helper() }}"),
        );

        let io = IoArgs::default();
        // Deliberately malformed: `macro_namespace_template_resolver` expects
        // this value to be a `ValueMap` object and unwraps it unconditionally,
        // which panics for any other value (e.g. this plain string) once a
        // macro body calls an unresolved macro name.
        let bad_dbt_and_adapters_namespace = MinijinjaValue::from("not-a-valuemap");

        typecheck_macros(
            &io,
            &mut macros,
            jinja_env,
            AdapterType::Postgres,
            "pkg",
            bad_dbt_and_adapters_namespace,
        )?;

        assert!(
            macros["macro.pkg.m_panicking"].depends_on.macros.is_empty(),
            "panicking macro should keep empty depends_on.macros instead of crashing the run"
        );
        assert_eq!(
            macros["macro.pkg.a_before"].depends_on.macros,
            vec!["macro.pkg.helper".to_string()],
            "macro processed before the panicking one should still be typechecked correctly"
        );
        assert_eq!(
            macros["macro.pkg.z_after"].depends_on.macros,
            vec!["macro.pkg.helper".to_string()],
            "macro processed after the panicking one should still be typechecked correctly"
        );

        Ok(())
    }

    #[test]
    fn typecheck_macros_flags_direct_introspective_call() -> FsResult<()> {
        use minijinja::Environment;

        fn make_macro(unique_id: &str, sql: &str) -> DbtMacro {
            let path = DbtPath::from(PathBuf::from(format!("macros/{unique_id}.sql")));
            DbtMacro {
                name: unique_id.rsplit('.').next().unwrap().to_string(),
                package_name: "pkg".to_string(),
                path: path.clone(),
                original_file_path: path,
                absolute_path: DbtPath::from(PathBuf::new()),
                span: None,
                unique_id: unique_id.to_string(),
                macro_sql: sql.to_string(),
                depends_on: MacroDependsOn { macros: vec![] },
                description: String::new(),
                meta: BTreeMap::new(),
                docs: None,
                config: MacroConfig::default(),
                patch_path: None,
                funcsign: None,
                supported_languages: None,
                args: vec![],
                arguments: vec![],
                macro_name_span: None,
                __other__: BTreeMap::new(),
            }
        }

        let jinja_env = Arc::new(JinjaEnv::new(Environment::new()));

        let mut macros = BTreeMap::new();
        macros.insert(
            "macro.pkg.introspects".to_string(),
            make_macro("macro.pkg.introspects", "{{ adapter.execute('select 1') }}"),
        );
        macros.insert(
            "macro.pkg.plain".to_string(),
            make_macro("macro.pkg.plain", "{{ 1 + 1 }}"),
        );

        let io = IoArgs::default();
        typecheck_macros(
            &io,
            &mut macros,
            jinja_env.clone(),
            AdapterType::Postgres,
            "pkg",
            MinijinjaValue::from(()),
        )?;

        assert!(jinja_env.is_introspective_macro("macro.pkg.introspects"));
        assert!(!jinja_env.is_introspective_macro("macro.pkg.plain"));

        Ok(())
    }
}
