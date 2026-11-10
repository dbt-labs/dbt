//! `dbt show --info` / `--inline` with `{{ info_schema() }}`.
//!
//! These invocations query `target/info_schema/` through DuckDB, not the
//! warehouse and not the engine index. `--info <view>` is sugar for
//! `select * from {{ info_schema('<view>') }}`.

use std::path::Path;

use dbt_common::cancellation::CancellationToken;
use dbt_common::io_args;
use dbt_common::pretty_string::make_title;
use dbt_common::tracing::emit::emit_info_event;
use dbt_common::{ErrorCode, FsResult, err, fs_err};
use dbt_index_core::info_schema::schema::INFO_SCHEMA;
use dbt_index_core::info_schema::spec::Ns;
use dbt_pretty_table::{make_column_names, pretty_data_table};
use dbt_tasks_core::pretty_table::from_pretty_table_error;
use dbt_telemetry::{ShowDataOutput, ShowDataOutputFormat};
use minijinja::Value;

use crate::check_adapter::{open_epoch_adapter, query_index};

/// Template name handed to the Jinja parser. It only ever appears in a parse error,
/// which this predicate discards, so it is a label for debugging rather than output.
const INLINE_TEMPLATE_NAME: &str = "<show --inline>";

/// True when a `dbt show` invocation reads `target/info_schema/` rather than the
/// warehouse: `--info <view>`, or `--inline` SQL that calls `info_schema()`.
///
/// This picks the execution engine, so it lives here rather than on `ShowArgs`:
/// answering it needs the Jinja parser, and `dbt-clap-core` neither has minijinja
/// nor should acquire it to make a routing decision.
pub fn queries_info_schema(info: Option<&str>, inline: Option<&str>) -> bool {
    info.is_some() || inline.is_some_and(inline_calls_info_schema)
}

/// Statically discover an `info_schema()` call in `--inline` SQL.
///
/// `find_static_string_arg_calls` runs the real Jinja parser and walks the AST, so
/// `{# {{ info_schema('x') }} #}`, a `'info_schema('` literal, a `-- comment`, and a
/// warehouse UDF named `my_info_schema(` are all correctly *not* calls. Nothing is
/// evaluated, so this needs no context and cannot fail on `{{ ref('x') }}`.
///
/// Two ways it declines to match, both landing on the warehouse:
///
/// - a call whose arguments are not string literals (`{{ info_schema(v) }}`), which
///   the static walk deliberately skips;
/// - a template that does not parse, which the warehouse path then reports as the
///   render error it is.
///
/// Under-matching is the right direction for a predicate that picks the engine. The
/// project env defines `info_schema` too, so an undetected call still renders — to
/// `dbt.models`, which the warehouse then rejects as a missing schema. Loud, and the
/// SQL naming `dbt.models` points at the cause. Over-matching is the bad direction:
/// it answers a warehouse question from local parquet without saying so.
fn inline_calls_info_schema(sql: &str) -> bool {
    minijinja::Environment::new()
        .find_static_string_arg_calls(INLINE_TEMPLATE_NAME, sql, &["info_schema"])
        .is_ok_and(|calls| !calls.is_empty())
}

/// Wrap `sql` so `--limit` applies to whatever the user wrote.
///
/// The newlines are load-bearing: user SQL ending in a `-- comment` would otherwise
/// swallow the closing paren and the `limit`, leaving `select * from (` and a
/// "syntax error at end of input" that names nothing the user typed.
fn wrap_with_limit(sql: String, limit: Option<usize>) -> String {
    match limit.filter(|n| *n > 0) {
        Some(n) => format!("select * from (\n{sql}\n) as _show_info limit {n}"),
        None => sql,
    }
}

/// Render `--info` / info-schema `--inline` to SQL, query `target/info_schema/`,
/// and emit the same show table event warehouse `dbt show` uses.
pub fn run_show_info_schema(
    info: Option<&str>,
    inline: Option<&str>,
    metadata_dir: &Path,
    format: io_args::DisplayFormat,
    limit: Option<usize>,
    token: CancellationToken,
) -> FsResult<()> {
    let sql = render_show_info_sql(info, inline)?;
    // Reads the project metadata directly, so there is no artifact to generate first and
    // nothing to go stale. What can still be missing is the metadata itself: a project that
    // has only ever been parsed has written no epochs.
    if !metadata_dir.exists() {
        return Err(fs_err!(
            ErrorCode::InfoSchemaUnavailable,
            "no project metadata at {} — run a command that writes it, such as \
             `dbt build`, `dbt compile`, or `dbt parse --write-metadata`",
            metadata_dir.display()
        ));
    }
    let adapter = open_epoch_adapter(metadata_dir, token)
        .map_err(|e| fs_err!(ErrorCode::InfoSchemaUnavailable, "{e}"))?;

    let limited_sql = wrap_with_limit(sql, limit);
    // The query keeps `InvalidArgument`: an unknown view or a syntax error is the
    // argument, which is what that code is for.
    let batches = query_index(&adapter, &limited_sql)
        .map_err(|e| fs_err!(ErrorCode::InvalidArgument, "{e}"))?;
    let schema = batches.first().map(|b| b.schema()).ok_or_else(|| {
        fs_err!(
            ErrorCode::Generic,
            "information schema query returned no schema"
        )
    })?;

    let display_format = match format {
        io_args::DisplayFormat::Table => dbt_pretty_table::DisplayFormat::Table,
        io_args::DisplayFormat::Csv => dbt_pretty_table::DisplayFormat::Csv,
        io_args::DisplayFormat::Tsv => dbt_pretty_table::DisplayFormat::Tsv,
        io_args::DisplayFormat::Json => dbt_pretty_table::DisplayFormat::Json,
        io_args::DisplayFormat::NdJson => dbt_pretty_table::DisplayFormat::NdJson,
        io_args::DisplayFormat::Yml => dbt_pretty_table::DisplayFormat::Yml,
        io_args::DisplayFormat::Selector => dbt_pretty_table::DisplayFormat::Selector,
        io_args::DisplayFormat::Name => dbt_pretty_table::DisplayFormat::Name,
        io_args::DisplayFormat::Path => dbt_pretty_table::DisplayFormat::Path,
    };
    let column_names = make_column_names(schema.as_ref());
    let node_name = info.unwrap_or("inline").to_string();
    let title = make_title("Query", &format!("info_schema_{node_name}"));
    let table = pretty_data_table(
        &title,
        "",
        &column_names,
        batches.as_slice(),
        display_format,
        limit,
        true,
        None,
    )
    .map_err(from_pretty_table_error)?;

    let output_format = match format {
        io_args::DisplayFormat::Table => ShowDataOutputFormat::Text,
        io_args::DisplayFormat::Csv => ShowDataOutputFormat::Csv,
        io_args::DisplayFormat::Tsv => ShowDataOutputFormat::Tsv,
        io_args::DisplayFormat::Json => ShowDataOutputFormat::Json,
        io_args::DisplayFormat::NdJson => ShowDataOutputFormat::Ndjson,
        io_args::DisplayFormat::Yml => ShowDataOutputFormat::Yml,
        _ => {
            return err!(
                ErrorCode::UnsupportedFeature,
                "DisplayFormat::{:?} is not supported for show command",
                format
            );
        }
    };
    let event = ShowDataOutput::new_with_default_code(
        output_format,
        table,
        node_name,
        true,
        None,
        column_names,
    );
    emit_info_event(event, None);
    Ok(())
}

fn render_show_info_sql(info: Option<&str>, inline: Option<&str>) -> FsResult<String> {
    if let Some(view) = info {
        return qualify_show_view(view);
    }
    let Some(inline) = inline else {
        return Err(fs_err!(
            ErrorCode::InvalidArgument,
            "show --info requires a view name, or --inline SQL that calls info_schema()"
        ));
    };
    let mut env = minijinja::Environment::new();
    env.add_global("info_schema", make_show_info_schema_fn());
    env.render_str(inline, (), &[]).map_err(|e| {
        fs_err!(
            ErrorCode::InvalidArgument,
            "failed to render --inline info_schema SQL: {e}"
        )
    })
}

fn qualify_show_view(view: &str) -> FsResult<String> {
    match public_info_schema_table(view) {
        Some(spec) => Ok(format!("select * from {}", spec.qualified_name())),
        None => Err(fs_err!(
            ErrorCode::InvalidArgument,
            "unknown information schema view '{view}'. Available views: {}",
            public_info_schema_names()
        )),
    }
}

fn public_info_schema_table(
    view: &str,
) -> Option<&'static dbt_index_core::info_schema::spec::TableSpec> {
    INFO_SCHEMA
        .iter()
        .find(|t| t.ns != Ns::DbtInternal && t.name == view)
}

fn public_info_schema_names() -> String {
    INFO_SCHEMA
        .iter()
        .filter(|t| t.ns != Ns::DbtInternal)
        .map(|t| t.name)
        .collect::<Vec<_>>()
        .join(", ")
}

fn make_show_info_schema_fn() -> Value {
    use minijinja::value::Kwargs;
    use minijinja::{Error as MjError, ErrorKind};
    Value::from_function(
        move |args: &[Value], _kwargs: Kwargs| -> Result<Value, MjError> {
            let view = args.first().and_then(|v| v.as_str()).ok_or_else(|| {
                MjError::new(
                    ErrorKind::InvalidOperation,
                    "info_schema(view): first argument must be a view name string",
                )
            })?;
            let spec = public_info_schema_table(view).ok_or_else(|| {
                MjError::new(
                    ErrorKind::InvalidOperation,
                    format!(
                        "info_schema: '{view}' is not available to dbt show. \
                         Available views: {}",
                        public_info_schema_names()
                    ),
                )
            })?;
            Ok(Value::from(spec.qualified_name()))
        },
    )
}

#[cfg(test)]
mod tests {
    use super::{inline_calls_info_schema, queries_info_schema, wrap_with_limit};

    /// This predicate picks the execution engine, so a false positive answers a
    /// warehouse question from local parquet. Everything here is a routing case.
    #[test]
    fn only_a_real_jinja_call_routes_to_the_information_schema() {
        for sql in [
            "select * from {{ info_schema('models') }}",
            "select * from {{info_schema ( 'models' )}}",
            "{% set v = info_schema('models') %}select * from {{ v }}",
            "{% if true %}select * from {{ info_schema('models') }}{% endif %}",
        ] {
            assert!(inline_calls_info_schema(sql), "expected a call: {sql}");
        }

        for sql in [
            // A warehouse UDF whose name merely ends in `info_schema`.
            "select * from utils.my_info_schema('x')",
            // Mentioned, never called: a SQL comment, a string literal, bare SQL.
            "select 1 -- todo: use info_schema()",
            "select 'info_schema(' as s",
            "select 1 as one",
            // A Jinja comment: parsed as a comment, so the call inside is not one.
            "select 1 {# {{ info_schema('models') }} #}",
            // Non-literal argument: the static walk skips it, so this lands on the
            // warehouse and fails there by name rather than being answered locally.
            "{% set v = 'models' %}select * from {{ info_schema(v) }}",
            // Unparseable: the warehouse path reports the render error.
            "select * from {{ info_schema('models')",
            // `{% raw %}` is emitted verbatim, never evaluated.
            "{% raw %}{{ info_schema('models') }}{% endraw %}",
        ] {
            assert!(!inline_calls_info_schema(sql), "expected no call: {sql}");
        }
    }

    /// The static walk visits both arms of an `{% if %}`, but the render takes only
    /// the live one. A call in a dead branch therefore routes to the information
    /// schema and then renders warehouse SQL, which DuckDB rejects by name. Pinned
    /// because it is the one case where this predicate over-matches; it stays
    /// acceptable only because the failure is loud (unqualified names cannot
    /// resolve to `dbt.*`, whose views live outside DuckDB's `main` search path).
    #[test]
    fn a_call_in_a_dead_branch_still_routes() {
        assert!(inline_calls_info_schema(
            "{% if false %}{{ info_schema('models') }}{% else %}select * from wh{% endif %}"
        ));
    }

    /// A trailing `-- comment` used to swallow the closing paren and the limit,
    /// failing with "syntax error at end of input" against SQL the user never wrote.
    #[test]
    fn limit_wrapping_survives_a_trailing_line_comment() {
        let wrapped = wrap_with_limit("select 1 -- why".to_string(), Some(10));
        assert!(
            wrapped.ends_with("\n) as _show_info limit 10"),
            "the closing paren must start its own line: {wrapped}"
        );
        assert_eq!(wrap_with_limit("select 1".to_string(), None), "select 1");
        assert_eq!(wrap_with_limit("select 1".to_string(), Some(0)), "select 1");
    }

    #[test]
    fn info_routes_without_inline_and_plain_inline_does_not() {
        assert!(queries_info_schema(Some("models"), None));
        assert!(!queries_info_schema(None, Some("select 1 as one")));
        assert!(!queries_info_schema(None, None));
    }
}
