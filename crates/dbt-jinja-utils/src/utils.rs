use dbt_adapter::Adapter;
use dbt_adapter::relation::render_effective_relation;
use dbt_adapter_core::AdapterType;
use dbt_common::{ErrorCode, FsError, fs_err};
use dbt_common::{FsResult, constants::DBT_CTE_PREFIX, error::MacroSpan, stdfs};
use dbt_frontend_common::{error::CodeLocation, span::Span};
use dbt_schemas::schemas::common::ResolvedQuoting;
use dbt_schemas::schemas::project::ResolvableConfig;
use dbt_schemas::schemas::telemetry::NodeType;
use dbt_schemas::schemas::{
    CommonAttributes, DbtModel, DbtSeed, DbtSnapshot, DbtTest, DbtUnitTest, InternalDbtNode,
};
use dbt_yaml::{Spanned, Value as YmlValue};
use minijinja::arg_utils::ArgParser;
use minijinja::constants::{ROOT_PACKAGE_NAME, TARGET_PACKAGE_NAME, TARGET_UNIQUE_ID, THREAD_ID};
use minijinja::{
    Error, ErrorKind, MacroSpans, State, Value,
    functions::debug,
    value::{Rest, Value as MinijinjaValue, ValueKind},
};
use regex::Regex;
use serde::Deserialize;
use sqlparser::dialect::GenericDialect;
use sqlparser::keywords::Keyword;
use sqlparser::tokenizer::{Location, Token, Tokenizer};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, LazyLock};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::Mutex,
};

use crate::listener::RenderingEventListenerFactory;
use minijinja::listener::RenderingEventListener;

use crate::{jinja_environment::JinjaEnv, phases::parse::sql_resource::SqlResource};

pub use dbt_jinja_vars::{DBT_INTERNAL_ENV_VAR_PREFIX, SECRET_ENV_VAR_PREFIX};

/// The version of dbt used in this crate
pub const DBT_VERSION: &str = "2.0.0"; // easter egg jokes for now

/// A lazy initialized mutex-protected hash map for storing environment variables
pub static ENV_VARS: LazyLock<Mutex<HashMap<String, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Key for [`TEMPLATE_CACHE`]: `(dialect, current_project, macro_name)`.
///
/// The dialect is part of the key because the final fallback resolves through the
/// per-dialect internal-package namespace: two adapters can define the same
/// unprefixed macro, so a Snowflake node and a DuckDB node must not share a
/// cached answer.
type TemplateCacheKey = (String, String, String);

/// Cache for template lookups. See [`TemplateCacheKey`] for why the dialect is
/// part of the key.
static TEMPLATE_CACHE: LazyLock<Mutex<HashMap<TemplateCacheKey, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Matches local quoted or unquoted CTE definitions that use dbt's ephemeral CTE
/// prefix, such as `with __dbt__cte__model as (...)` or
/// `with "__dbt__cte__model" as (...)`, so they can be distinguished from refs.
static LOCAL_EPHEMERAL_CTE_RE: LazyLock<Regex> = LazyLock::new(|| {
    let prefix = regex::escape(DBT_CTE_PREFIX);
    Regex::new(&format!(
        r#"(?i)(?:"{prefix}([[:alnum:]_]+)"|{prefix}([[:alnum:]_]+))\s*(?:\([^)]*\)\s*)?as\s*(?:(?:not\s+)?materialized\s+)?\("#
    ))
    .expect("valid local ephemeral CTE regex")
});

/// Converts a value to a boolean
pub fn as_bool(args: Value) -> Result<Value, Error> {
    let input = args.to_string();
    match input.parse::<i64>() {
        Ok(int_value) => Ok(Value::from(int_value != 0)),
        Err(_) => match input.parse::<f64>() {
            Ok(float_value) => Ok(Value::from(!float_value.is_nan() && float_value != 0.0)),
            Err(_) => match input.to_ascii_lowercase().as_str() {
                "true" => Ok(Value::from(true)),
                "false" => Ok(Value::from(false)),
                _ => Ok(Value::from(!input.is_empty())),
            },
        },
    }
}

/// Asserts a condition using Jinja
pub fn assert_minijinja(_state: &State, args: Rest<Value>) -> Result<Value, Error> {
    if args.is_empty() {
        return Err(Error::new(
            ErrorKind::InvalidOperation,
            "Expected at least one argument",
        ));
    }
    let expr = args[0].clone();
    let message = args.get(1).map_or_else(String::new, |v| v.to_string());
    let condition = as_bool(expr)?;
    if condition == Value::from(false) {
        eprintln!("error: {} assertion failed", &message);
    }
    Ok(Value::from(""))
}

/// Logs a message using Jinja
pub fn log_minijinja(state: &State, args: Rest<Value>) -> String {
    let debug_str = debug(state, args);
    eprintln!("log: {}", &debug_str);
    "".to_owned()
}

/// escape a string with ascii <,>, &, ", ', /, ' with html substitutions
pub fn escape(s: &str) -> String {
    let mut output = String::with_capacity(s.len() * 2); // Reserve capacity for worst case

    for c in s.chars() {
        match c {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '"' => output.push_str("&quot;"),
            '\'' => output.push_str("&apos;"),
            '\\' => output.push_str("&#92;"),
            _ => output.push(c),
        }
    }
    output
}

/// unescape a string with html substitutions for  <,>, &, ", ', /, with their ascii equivalents
pub fn unescape(html: &str) -> String {
    let mut output = Vec::with_capacity(html.len());
    let html = html.as_bytes();
    let mut i = 0;
    while i < html.len() {
        let c = html[i];
        if c == b'&' {
            if let Some(end) = html[i..].iter().position(|&x| x == b';') {
                let entity = &html[i..i + end + 1];
                match entity {
                    b"&amp;" => output.push(b'&'),
                    b"&lt;" => output.push(b'<'),
                    b"&gt;" => output.push(b'>'),
                    b"&quot;" => output.push(b'"'),
                    b"&apos;" => output.push(b'\''),
                    b"&#91;" => output.push(b'['),
                    b"&#92;" => output.push(b'\\'),
                    b"&#93;" => output.push(b']'),
                    _ => {
                        output.extend_from_slice(entity);
                    }
                }
                i += end + 1;
            } else {
                output.push(c);
                i += 1;
            }
        } else {
            output.push(c);
            i += 1;
        }
    }
    String::from_utf8_lossy(&output).into_owned()
}

/// Handles ephemeral model CTEs in SQL
///
/// This function processes SQL that contains DBT CTE prefixes, extracts model names,
/// reads the corresponding SQL files from the ephemeral directory, and incorporates them
/// as CTEs in the final SQL. It also adjusts macro spans to account for the added lines.
pub fn inject_and_persist_ephemeral_models(
    sql: String,
    macro_spans: &mut MacroSpans,
    model_name: &str,
    is_current_model_ephemeral: bool,
    ephemeral_dir: &Path,
) -> FsResult<String> {
    if !sql.contains(DBT_CTE_PREFIX) {
        // Write the ephemeral model to the ephemeral directory
        if is_current_model_ephemeral {
            let ephemeral_path = ephemeral_dir.join(format!("{model_name}.sql"));
            stdfs::create_dir_all(ephemeral_path.parent().unwrap())?;
            stdfs::write(
                ephemeral_path,
                format!("{DBT_CTE_PREFIX}{model_name} as (\n{sql}\n)"),
            )?;
        }
        return Ok(sql);
    }

    let mut final_sql = sql;
    let local_ephemeral_cte_names = extract_local_ephemeral_cte_names(&final_sql);
    let ephemeral_model_names = extract_ephemeral_model_names(&final_sql);

    // Read ephemeral SQL from ephemeral dir and build cumulative CTEs
    let sep = "\x00";
    let mut seen_ctes = HashSet::new();
    let mut all_ctes = Vec::new();

    for model_name in ephemeral_model_names {
        let ephemeral_path = ephemeral_dir.join(format!("{model_name}.sql"));
        let ephemeral_sql = match stdfs::read_to_string(&ephemeral_path) {
            Ok(ephemeral_sql) => ephemeral_sql,
            Err(err) if local_ephemeral_cte_names.contains(model_name) => {
                match stdfs::exists(&ephemeral_path) {
                    Ok(false) => continue,
                    Ok(true) => return Err(err),
                    Err(err) => return Err(err),
                }
            }
            Err(err) => return Err(err),
        };

        // Split existing CTEs and add any new ones
        let existing_ctes: Vec<String> = ephemeral_sql.split(sep).map(|s| s.to_string()).collect();
        for cte in existing_ctes {
            if !seen_ctes.contains(&cte) {
                seen_ctes.insert(cte.clone());
                all_ctes.push(cte);
            }
        }
    }
    // Write all CTEs up to this point for this model for next use.
    // this avoid graph walk for ephemeral models
    if is_current_model_ephemeral {
        let ephemeral_path = ephemeral_dir.join(format!("{model_name}.sql"));
        stdfs::create_dir_all(ephemeral_path.parent().unwrap())?;
        let cte_line = format!("{DBT_CTE_PREFIX}{model_name} as (\n{final_sql}\n)");
        all_ctes.push(cte_line);
        stdfs::write(ephemeral_path, all_ctes.join(sep))?;
        all_ctes.pop();
    }

    if all_ctes.is_empty() {
        return Ok(final_sql);
    }

    let ctes = all_ctes.join(", ");
    if let Some((sql, insertion)) = inject_ctes_into_existing_with(&final_sql, &ctes) {
        final_sql = sql;
        shift_macro_spans_after_insertion(macro_spans, insertion);
    } else {
        // Preserve the wrapper fallback for statements without a leading WITH clause.
        final_sql = format!(
            "with {ctes}\n--EPHEMERAL-SELECT-WRAPPER-START\nselect * from (\n{final_sql}\n--EPHEMERAL-SELECT-WRAPPER-END\n)"
        );
        let added_lines = ctes.lines().count() + 2;
        let added_offset = ctes.len() + 23;
        for span in macro_spans.items.iter_mut() {
            span.1.start_line += added_lines as u32;
            span.1.end_line += added_lines as u32;
            span.1.start_offset += added_offset as u32;
            span.1.end_offset += added_offset as u32;
        }
    }
    Ok(final_sql)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Insertion {
    line: u32,
    col: u32,
    offset: u32,
    added_lines: u32,
    added_bytes: u32,
    added_cols: u32,
    trailing_cols: u32,
}

/// Uses token locations to splice ephemeral CTEs into a leading WITH chain
/// without reformatting the warehouse-specific SQL.
fn inject_ctes_into_existing_with(sql: &str, ctes: &str) -> Option<(String, Insertion)> {
    let dialect = GenericDialect {};
    let mut tokens = Vec::new();
    // A later warehouse-specific token may be unsupported by the generic dialect.
    // The prefix tokens collected before that error are sufficient for this splice.
    let _ = Tokenizer::new(&dialect, sql).tokenize_with_location_into_buf(&mut tokens);
    let (with_index, with) = tokens
        .iter()
        .enumerate()
        .find(|(_, token)| !matches!(token.token, Token::Whitespace(_)))?;
    if !matches!(&with.token, Token::Word(word) if word.keyword == Keyword::WITH) {
        return None;
    }

    // Comments between WITH and RECURSIVE are trivia, so preserve them and
    // insert after the modifier.
    let next = tokens[with_index + 1..]
        .iter()
        .find(|token| !matches!(token.token, Token::Whitespace(_)));
    let insertion_token = match next {
        Some(token) if matches!(&token.token, Token::Word(word) if word.keyword == Keyword::RECURSIVE) => {
            token
        }
        _ => with,
    };
    let insertion_location = insertion_token.span.end;
    let offset = byte_offset_at_location(sql, insertion_location)?;
    let line = insertion_location.line.try_into().ok()?;
    let col = insertion_location.column.try_into().ok()?;
    let inserted = format!("  {ctes},");
    let added_lines = inserted.bytes().filter(|byte| *byte == b'\n').count() as u32;
    let added_cols = inserted.chars().count() as u32;
    let trailing_cols = inserted.rsplit('\n').next().unwrap().chars().count() as u32 + 1;

    let mut result = String::with_capacity(sql.len() + inserted.len());
    result.push_str(&sql[..offset]);
    result.push_str(&inserted);
    result.push_str(&sql[offset..]);

    Some((
        result,
        Insertion {
            line,
            col,
            offset: offset as u32,
            added_lines,
            added_bytes: inserted.len() as u32,
            added_cols,
            trailing_cols,
        },
    ))
}

fn byte_offset_at_location(sql: &str, target: Location) -> Option<usize> {
    let mut line = 1;
    let mut col = 1;
    for (offset, ch) in sql.char_indices() {
        if line == target.line && col == target.column {
            return Some(offset);
        }
        if ch == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line == target.line && col == target.column).then_some(sql.len())
}

fn shift_macro_spans_after_insertion(macro_spans: &mut MacroSpans, insertion: Insertion) {
    for (_, expanded) in &mut macro_spans.items {
        if expanded.end_offset <= insertion.offset {
            continue;
        }
        if expanded.start_offset >= insertion.offset {
            shift_span_position(
                &mut expanded.start_line,
                &mut expanded.start_col,
                &mut expanded.start_offset,
                insertion,
            );
        }
        shift_span_position(
            &mut expanded.end_line,
            &mut expanded.end_col,
            &mut expanded.end_offset,
            insertion,
        );
    }
}

fn shift_span_position(line: &mut u32, col: &mut u32, offset: &mut u32, insertion: Insertion) {
    if *offset < insertion.offset {
        return;
    }

    *offset += insertion.added_bytes;
    if *line == insertion.line {
        if insertion.added_lines == 0 {
            *col += insertion.added_cols;
        } else {
            *line += insertion.added_lines;
            *col = insertion.trailing_cols + (*col).saturating_sub(insertion.col);
        }
    } else {
        *line += insertion.added_lines;
    }
}

/// Extract all model names from `__dbt__cte__` references
fn extract_ephemeral_model_names(sql: &str) -> Vec<&str> {
    let mut seen = HashSet::new();

    // Split on DBT_CTE_PREFIX
    //  "WITH __dbt__cte__foo AS (SELECT 1), __dbt__cte__bar AS (SELECT 2), ..."
    //
    // becomes
    //  "WITH __dbt__cte__"
    //  "foo AS (SELECT 1), __dbt__cte__"
    //  "bar AS (SELECT 2), ..."
    sql.split(DBT_CTE_PREFIX)
        .skip(1) // first Item is a excluded, remaining Items begin with the model name
        .flat_map(|part| {
            // extract by splitting again, taking first Item
            part.split(|c: char| !c.is_alphanumeric() && c != '_')
                .next()
        })
        .filter(|&name| seen.insert(name)) // Deduplicate the extracted names
        .collect()
}

/// Extract model names from locally defined `__dbt__cte__` CTEs.
fn extract_local_ephemeral_cte_names(sql: &str) -> HashSet<&str> {
    LOCAL_EPHEMERAL_CTE_RE
        .captures_iter(sql)
        .filter_map(|captures| {
            // Capture 1 is the quoted CTE branch; capture 2 is the unquoted branch.
            captures
                .get(1)
                .or_else(|| captures.get(2))
                .map(|match_| match_.as_str())
        })
        .collect()
}

/// Renders SQL with Jinja macros
#[allow(clippy::too_many_arguments)]
pub fn render_sql(
    sql: &str,
    env: &JinjaEnv,
    ctx: &BTreeMap<String, Value>,
    listener_factory: &dyn RenderingEventListenerFactory,
    filename: &Path,
) -> FsResult<String> {
    let listeners =
        listener_factory.create_listener_bundle(filename, &CodeLocation::start_of_file(), sql);
    let result = env
        .env
        .render_named_str_with_tokenizer_listeners(
            filename.to_str().unwrap(),
            sql,
            ctx,
            &listeners,
            &[],
        )
        .map_err(|e| FsError::from_jinja_err(e, "Failed to render SQL"))?;
    for listener in listeners {
        listener_factory.destroy_listener(filename, listener);
    }

    Ok(result)
}

/// Renders SQL with Jinja macros, using caller-provided listeners
/// This allows callers to access listener state after rendering
/// (e.g., for MangledRefWarningPrinter to check for mangled refs)
#[allow(clippy::too_many_arguments)]
pub fn render_sql_with_listeners(
    sql: &str,
    env: &JinjaEnv,
    ctx: &BTreeMap<String, Value>,
    listeners: &[Rc<dyn RenderingEventListener>],
    tokenizer_listeners: &[Rc<dyn minijinja::listener::TokenizerEventListener>],
    filename: &Path,
) -> FsResult<String> {
    let result = env
        .env
        .render_named_str_with_tokenizer_listeners(
            filename.to_str().unwrap(),
            sql,
            ctx,
            listeners,
            tokenizer_listeners,
        )
        .map_err(|e| FsError::from_jinja_err(e, "Failed to render SQL"))?;

    Ok(result)
}

/// Converts a MacroSpans object to a vector of MacroSpan objects
pub fn macro_spans_to_macro_span_vec(macro_spans: &MacroSpans) -> Vec<MacroSpan> {
    spans_to_macro_span_vec(&macro_spans.items)
}

/// Converts raw source-map spans to MacroSpan objects.
pub fn raw_source_spans_to_macro_span_vec(macro_spans: &MacroSpans) -> Vec<MacroSpan> {
    spans_to_macro_span_vec(&macro_spans.raw_source_spans)
}

fn spans_to_macro_span_vec(
    spans: &[(minijinja::machinery::Span, minijinja::machinery::Span)],
) -> Vec<MacroSpan> {
    spans
        .iter()
        .map(|(source, expanded)| MacroSpan {
            macro_span: Span {
                start: CodeLocation {
                    line: source.start_line,
                    col: source.start_col,
                    index: source.start_offset,
                },
                stop: CodeLocation {
                    line: source.end_line,
                    col: source.end_col,
                    index: source.end_offset,
                },
            },
            expanded_span: Span {
                start: CodeLocation {
                    line: expanded.start_line,
                    col: expanded.start_col,
                    index: expanded.start_offset,
                },
                stop: CodeLocation {
                    line: expanded.end_line,
                    col: expanded.end_col,
                    index: expanded.end_offset,
                },
            },
        })
        .collect::<Vec<_>>()
}

/// This provides a 'get' method for object supports access via obj.get('key', 'default)
/// `map` is the inner data structure of this object
pub fn get_method(args: &[Value], map: &BTreeMap<String, Value>) -> Result<Value, Error> {
    let mut args = ArgParser::new(args, None);
    let name: String = args.get("name")?;
    let default = args
        .get_optional::<Value>("default")
        .unwrap_or_else(|| Value::from(""));

    Ok(match map.get(&name) {
        Some(val) if !val.is_none() => val.clone(),
        _ => default,
    })
}

/// Get catalog by relations for project
pub fn get_catalog_by_relations(
    env: &JinjaEnv,
    catalog_macro_name: &str,
    root_project_name: &str,
    current_project_name: &str,
    base_ctx: &BTreeMap<String, Value>,
    args: &[Value],
) -> FsResult<Value> {
    let template_name = find_macro_template(
        env,
        catalog_macro_name,
        root_project_name,
        current_project_name,
    )?;

    // Create a state object for rendering
    let template = env.get_template(&template_name)?;

    // Create a new state
    let new_state = template.eval_to_state(base_ctx, &[])?;

    // Call the macro
    let result = new_state
        .call_macro_raw(catalog_macro_name, args, &[])
        .map_err(|err| {
            Box::new(FsError::from_jinja_err(
                err,
                "Failed to run macro get_catalog_relations".to_string(),
            ))
        })?;
    // Return the result
    Ok(result)
}

/// Find the template for a given macro
pub fn find_macro_template(
    env: &JinjaEnv,
    macro_name: &str,
    root_project_name: &str,
    current_project_name: &str,
) -> FsResult<String> {
    let dialect = env
        .env
        .get_dialect()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "postgres".to_string());
    let cache_key = (
        dialect.clone(),
        current_project_name.to_string(),
        macro_name.to_string(),
    );

    // Check cache first - return early if found
    if let Ok(cache) = TEMPLATE_CACHE.lock()
        && let Some(template) = cache.get(&cache_key)
    {
        return Ok(template.clone());
    }
    // First try - check the current project
    let template_name = format!("{current_project_name}.{macro_name}");
    if env.has_template(&template_name) {
        // Cache and return
        if let Ok(mut cache) = TEMPLATE_CACHE.lock() {
            cache.insert(cache_key, template_name.clone());
        }
        return Ok(template_name);
    }

    // Second try - check the root project
    let template_name = format!("{root_project_name}.{macro_name}");
    if env.has_template(&template_name) {
        // Cache and return
        if let Ok(mut cache) = TEMPLATE_CACHE.lock() {
            cache.insert(cache_key, template_name.clone());
        }
        return Ok(template_name);
    }

    // Last attempt - check dbt internal package
    let dbt_and_adapters = env.get_dbt_and_adapters_namespace(&dialect);
    if let Some(package) = dbt_and_adapters.get(&Value::from(macro_name)) {
        let template_name = format!("{package}.{macro_name}");
        if env.has_template(&template_name) {
            // Cache and return
            if let Ok(mut cache) = TEMPLATE_CACHE.lock() {
                cache.insert(cache_key, template_name.clone());
            }
            return Ok(template_name);
        }
    }

    // Template not found in any location
    Err(fs_err!(
        ErrorCode::JinjaError,
        "Could not find template for {}",
        macro_name
    ))
}

/// Generate a component name using the specified macro
pub fn generate_component_name(
    env: &JinjaEnv,
    component: &str,
    root_project_name: &str,
    current_project_name: &str,
    base_ctx: &BTreeMap<String, Value>,
    custom_name: Option<String>,
    node: Option<&dyn InternalDbtNode>,
) -> FsResult<String> {
    let macro_name = format!("generate_{component}_name");
    // find the macro template - this is now cached for performance
    let template_name =
        find_macro_template(env, &macro_name, root_project_name, current_project_name)?;

    // Create a state object for rendering
    let template = env.get_template(&template_name)?;

    // Create a new state
    let new_state = template.eval_to_state(base_ctx, &[])?;

    // Build the args
    let mut args = custom_name
        .map(|name| vec![Value::from(name)])
        .unwrap_or_else(|| vec![Value::from(())]); // If no custom name, pass in none so the macro reads from the target context
    if let Some(node) = node {
        let mut serialized = node.serialize_keep_none();
        // Strip resource-type prefix from path so node.path inside macros like
        // generate_schema_name matches dbt-core convention ("staging/model.sql"
        // not "models/staging/model.sql"). build_flat_graph does the same for
        // graph.nodes.
        let prefix = match node.resource_type() {
            NodeType::Model => "models",
            NodeType::Snapshot => "snapshots",
            NodeType::Seed => "seeds",
            NodeType::Analysis => "analyses",
            _ => "",
        };
        if !prefix.is_empty() {
            if let YmlValue::Mapping(ref mut map, _) = serialized {
                let path_key = YmlValue::string("path".to_string());
                if let Some(path_value) = map.get(&path_key) {
                    if let Some(path_str) = path_value.as_str() {
                        let stripped = Path::new(path_str)
                            .strip_prefix(prefix)
                            .map(|p| p.to_string_lossy().into_owned())
                            .unwrap_or_else(|_| path_str.to_string());
                        map.insert(path_key, YmlValue::string(stripped));
                    }
                }
            }
        }
        args.push(Value::from_serialize(serialized));
    }

    // Call the macro
    let result = new_state
        .call_macro_raw(macro_name.as_str(), &args, &[])
        .map(|v| match v.kind() {
            ValueKind::None => "".to_string(),
            _ => v.to_string().trim().to_string(),
        })
        .map_err(|err| {
            Box::new(FsError::from_jinja_err(
                err,
                format!("Failed to run macro {macro_name} for component {component}"),
            ))
        })?;
    // Return the result
    Ok(result)
}

/// Clear template cache (primarily for testing purposes)
pub fn clear_template_cache() {
    if let Ok(mut cache) = TEMPLATE_CACHE.lock() {
        cache.clear();
    }
}

/// Generate a relation name from database, schema, alias
pub fn generate_relation_name(
    parse_adapter: Arc<Adapter>,
    database: &str,
    schema: &str,
    identifier: &str,
    quote_config: ResolvedQuoting,
) -> FsResult<String> {
    generate_relation_name_with_target(
        parse_adapter,
        database,
        schema,
        identifier,
        quote_config,
        None,
    )
}

/// Generate a relation name using a node's resolved destination.
pub fn generate_relation_name_with_target(
    parse_adapter: Arc<Adapter>,
    database: &str,
    schema: &str,
    identifier: &str,
    quote_config: ResolvedQuoting,
    target: Option<AdapterType>,
) -> FsResult<String> {
    render_effective_relation(
        parse_adapter.adapter_type(),
        database,
        schema,
        identifier,
        quote_config,
        target,
    )
}

type NodeId = String;
/// Returns the metadata of the current model from the given Jinja execution state
pub fn node_metadata_from_state(state: &State) -> Option<(NodeId, PathBuf)> {
    match state.lookup("model", &[]) {
        Some(node) => {
            if let Ok(model) = DbtModel::deserialize(&node) {
                Some((
                    model.__common_attr__.unique_id,
                    model.__common_attr__.original_file_path.to_path_buf(),
                ))
            } else if let Ok(test) = DbtTest::deserialize(&node) {
                Some((
                    test.__common_attr__.unique_id,
                    test.__common_attr__.original_file_path.to_path_buf(),
                ))
            } else if let Ok(snapshot) = DbtSnapshot::deserialize(&node) {
                Some((
                    snapshot.__common_attr__.unique_id,
                    snapshot.__common_attr__.original_file_path.to_path_buf(),
                ))
            } else if let Ok(seed) = DbtSeed::deserialize(&node) {
                Some((
                    seed.__common_attr__.unique_id,
                    seed.__common_attr__.original_file_path.to_path_buf(),
                ))
            } else if let Ok(unit_test) = DbtUnitTest::deserialize(&node) {
                Some((
                    unit_test.__common_attr__.unique_id,
                    unit_test.__common_attr__.original_file_path.to_path_buf(),
                ))
            } else {
                // Fallback: direct attribute extraction for Object types
                // (e.g. LazyModelWrapper) where full deserialization fails
                let unique_id = node
                    .get_attr("unique_id")
                    .ok()
                    .and_then(|v| v.as_str().map(|s| s.to_string()));
                let file_path = node
                    .get_attr("original_file_path")
                    .ok()
                    .and_then(|v| v.as_str().map(PathBuf::from));
                unique_id.zip(file_path)
            }
        }
        None => None,
    }
}

/// Render a reference or source string and return the corresponding SqlResource
pub fn render_extract_ref_or_source_expr<T: ResolvableConfig<T>>(
    jinja_env: &JinjaEnv,
    resolve_model_context: &BTreeMap<String, Value>,
    sql_resources: Arc<Mutex<Vec<SqlResource<T>>>>,
    ref_str: &Spanned<String>,
) -> FsResult<SqlResource<T>> {
    let span = ref_str.span();
    let ref_str = ref_str.clone().into_inner();
    let expr = jinja_env
        .compile_expression(ref_str.as_str())
        .map_err(|e| {
            e.with_location(dbt_common::CodeLocationWithFile::new_with_arc(
                span.start.line as u32,
                span.start.column as u32,
                span.start.index as u32,
                span.filename.clone().unwrap_or_default(),
            ))
        })?;
    let _ = expr.eval(resolve_model_context, &[])?;
    // Remove from Mutex and return last item
    let mut sql_resources = sql_resources.lock().unwrap();
    let sql_resource = sql_resources.pop().ok_or_else(|| {
        fs_err!(
            code => ErrorCode::InvalidConfig,
            loc => span.clone(),
            "Exposure depends_on entry `{}` does not call ref(), source(), or metric(). \
             Each depends_on entry in your exposure YAML must use one of those forms, \
             e.g. `ref('my_model')` or `source('my_schema', 'my_table')`.",
            ref_str,
        )
    })?;
    Ok(sql_resource)
}

/// Detects & extract the name of the dependency package from the current context.
pub fn dependency_package_name_from_ctx<'a>(
    jinja_env: &'_ JinjaEnv,
    ctx: &'a BTreeMap<String, Value>,
) -> Option<&'a str> {
    // Try getting the root package name from the context. If it doesn't exist, err on the
    // side of caution and assume that we can't deduce whether the local package is the root package.
    let root_package_name = jinja_env
        .get_global(ROOT_PACKAGE_NAME)
        .and_then(|root| root.as_str().map(|root| root.to_string()))?;

    ctx.get(TARGET_PACKAGE_NAME)
        .and_then(|local| local.as_str())
        .and_then(|local_name| {
            if local_name != root_package_name {
                Some(local_name)
            } else {
                None
            }
        })
}

/// Add common context vars for tasks
pub fn add_task_context(
    base_context: &mut BTreeMap<String, Value>,
    common: &CommonAttributes,
    thread_id: &i32,
) {
    base_context.insert(
        TARGET_PACKAGE_NAME.to_string(),
        MinijinjaValue::from(common.package_name.clone()),
    );
    base_context.insert(
        TARGET_UNIQUE_ID.to_string(),
        MinijinjaValue::from(common.unique_id.clone()),
    );
    base_context.insert(
        THREAD_ID.to_string(),
        MinijinjaValue::from(format!("Thread-{}", thread_id)),
    );
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use minijinja::{
        Environment, MacroSpans, context, listener::RenderingEventListener, machinery::Span,
    };

    use crate::listener::DefaultRenderingEventListener;

    use super::{
        inject_ctes_into_existing_with, raw_source_spans_to_macro_span_vec,
        shift_macro_spans_after_insertion,
    };

    #[test]
    fn injects_ctes_into_existing_with_chain() {
        let sql = "WITH student_attendance AS (\n    SELECT 1\n)\nSELECT * FROM student_attendance";
        let ctes = "__dbt__cte__valid_class as (\nSELECT 1\n)";

        let (actual, _) = inject_ctes_into_existing_with(sql, ctes).unwrap();

        assert_eq!(
            actual,
            "WITH  __dbt__cte__valid_class as (\nSELECT 1\n), student_attendance AS (\n    SELECT 1\n)\nSELECT * FROM student_attendance"
        );
    }

    #[test]
    fn injects_after_recursive_keyword_and_preserves_leading_comments() {
        let sql = "-- model comment\n/* unicode: ☃ */\nWITH RECURSIVE paths AS (SELECT 1)\nSELECT * FROM paths";

        let (actual, _) = inject_ctes_into_existing_with(sql, "injected AS (SELECT 2)").unwrap();

        assert_eq!(
            actual,
            "-- model comment\n/* unicode: ☃ */\nWITH RECURSIVE  injected AS (SELECT 2), paths AS (SELECT 1)\nSELECT * FROM paths"
        );
    }

    #[test]
    fn injects_after_recursive_keyword_separated_by_comments() {
        for (sql, expected) in [
            (
                "WITH /* keep */ RECURSIVE paths AS (SELECT 1) SELECT * FROM paths",
                "WITH /* keep */ RECURSIVE  injected AS (SELECT 2), paths AS (SELECT 1) SELECT * FROM paths",
            ),
            (
                "WITH -- keep\nRECURSIVE paths AS (SELECT 1) SELECT * FROM paths",
                "WITH -- keep\nRECURSIVE  injected AS (SELECT 2), paths AS (SELECT 1) SELECT * FROM paths",
            ),
        ] {
            let (actual, _) =
                inject_ctes_into_existing_with(sql, "injected AS (SELECT 2)").unwrap();

            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn ignores_with_outside_a_leading_keyword() {
        for sql in [
            "SELECT 'WITH value'",
            "SELECT 1 -- WITH ignored",
            "(WITH nested AS (SELECT 1) SELECT * FROM nested)",
            "\"WITH\" AS identifier",
        ] {
            assert!(inject_ctes_into_existing_with(sql, "injected AS (SELECT 2)").is_none());
        }
    }

    #[test]
    fn shifts_only_macro_spans_after_the_insertion() {
        let sql = "WITH existing AS (\n    SELECT value\n)";
        let (_, insertion) =
            inject_ctes_into_existing_with(sql, "injected AS (\nSELECT 1\n)").unwrap();
        let source = Span::default();
        let before = Span {
            start_line: 1,
            start_col: 1,
            start_offset: 0,
            end_line: 1,
            end_col: 4,
            end_offset: 3,
        };
        let after = Span {
            start_line: 1,
            start_col: 6,
            start_offset: 5,
            end_line: 1,
            end_col: 14,
            end_offset: 13,
        };
        let ending_at_insertion = Span {
            start_line: 1,
            start_col: 1,
            start_offset: 0,
            end_line: 1,
            end_col: 5,
            end_offset: 4,
        };
        let crossing_insertion = Span {
            start_line: 1,
            start_col: 1,
            start_offset: 0,
            end_line: 1,
            end_col: 14,
            end_offset: 13,
        };
        let mut macro_spans = MacroSpans {
            items: vec![
                (source, before),
                (source, ending_at_insertion),
                (source, crossing_insertion),
                (source, after),
            ],
            ..Default::default()
        };

        shift_macro_spans_after_insertion(&mut macro_spans, insertion);

        assert_eq!(macro_spans.items[0].1, before);
        assert_eq!(macro_spans.items[1].1, ending_at_insertion);
        assert_eq!(
            macro_spans.items[2].1.start_line,
            crossing_insertion.start_line
        );
        assert_eq!(
            macro_spans.items[2].1.start_col,
            crossing_insertion.start_col
        );
        assert_eq!(
            macro_spans.items[2].1.start_offset,
            crossing_insertion.start_offset
        );
        assert_eq!(
            macro_spans.items[2].1.end_offset,
            crossing_insertion.end_offset + insertion.added_bytes
        );
        assert_eq!(macro_spans.items[3].1.start_line, 3);
        assert_eq!(macro_spans.items[3].1.start_col, 4);
        assert_eq!(
            macro_spans.items[3].1.start_offset,
            after.start_offset + insertion.added_bytes
        );
    }

    #[test]
    fn shifts_same_line_unicode_macro_span_by_character_columns() {
        let sql = "/* ☃ */ WITH existing AS (SELECT value)";
        let existing_offset = sql.find("existing").unwrap();
        let existing_col = sql[..existing_offset].chars().count() as u32 + 1;
        let expanded = Span {
            start_line: 1,
            start_col: existing_col,
            start_offset: existing_offset as u32,
            end_line: 1,
            end_col: existing_col + "existing".len() as u32,
            end_offset: (existing_offset + "existing".len()) as u32,
        };
        let (actual, insertion) =
            inject_ctes_into_existing_with(sql, "injected AS (SELECT '☃')").unwrap();
        let mut macro_spans = MacroSpans {
            items: vec![(Span::default(), expanded)],
            ..Default::default()
        };

        shift_macro_spans_after_insertion(&mut macro_spans, insertion);

        let actual_offset = actual.find("existing").unwrap();
        let actual_col = actual[..actual_offset].chars().count() as u32 + 1;
        assert_eq!(macro_spans.items[0].1.start_offset, actual_offset as u32);
        assert_eq!(macro_spans.items[0].1.start_col, actual_col);
    }

    #[test]
    fn raw_source_spans_track_rendered_loop_lines() {
        let source = "{% for value in values -%}\nselect {{ value }}\n{% endfor %}";
        let env = Environment::new();
        let listener = Rc::new(DefaultRenderingEventListener::new(false));
        let listeners: Vec<Rc<dyn RenderingEventListener>> = vec![listener.clone()];

        env.render_named_str(
            "loop.sql",
            source,
            context!(values => vec!["a", "b"]),
            &listeners,
        )
        .unwrap();

        let macro_spans = listener.macro_spans.borrow();
        let raw_source_spans = raw_source_spans_to_macro_span_vec(&macro_spans);

        assert!(
            raw_source_spans.iter().any(|span| {
                span.macro_span.start.line == 2 && span.expanded_span.start.line == 1
            })
        );
        assert!(
            raw_source_spans.iter().any(|span| {
                span.macro_span.start.line == 2 && span.expanded_span.start.line == 2
            })
        );
    }
}
