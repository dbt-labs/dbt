//! Embedded Jinja macros shipped with the classifier crate.
//!
//! These macros are registered into the project-wide `JinjaEnv` at startup
//! under the synthetic package name `dbt_classification`.  They implement
//! the Snowflake-tag round-trip described in
//! `propagation_of_snowflake_tags.md`:
//!
//! - `dbt_classification.fetch_snowflake_column_tags` — Phase 1
//! - `dbt_classification.apply_snowflake_column_tag`  — Phase 3
//!
//! The macros are dialect-specific (Snowflake) but the loader is
//! adapter-agnostic — gating happens at the call site in `dbt-tasks`.
//! Registering Snowflake-only macros on, say, a DuckDB build is harmless;
//! they only get invoked when the run-time path explicitly looks them up.

// RustEmbed's derive expansion internally uses `Path::canonicalize` at
// build time, which trips this workspace's disallowed-methods lint.
// The call is in generated code and only runs during compilation, so
// suppress the lint at the enclosing module level.
#[allow(clippy::disallowed_methods)]
mod assets {
    use rust_embed::RustEmbed;

    #[derive(RustEmbed)]
    #[folder = "src/macros/"]
    pub struct PropagationMacroAssets;
}

/// The synthetic dbt package name under which these macros are registered.
/// Callers use `dbt_classification.<macro_name>` to look them up via
/// `run_operation` or `JinjaEnv::get_template`.
pub const PACKAGE_NAME: &str = "dbt_classification";

/// Returns `(template_name, sql_content)` pairs for every embedded SQL macro.
///
/// The `template_name` follows dbt's standard convention
/// `package.macro_name`, where `package` is `dbt_classification` and
/// `macro_name` is the SQL file's stem (e.g. `fetch_column_tags.sql` →
/// `dbt_classification.fetch_column_tags`).  The same string is what
/// `run_operation` looks up.
///
/// The loader is intentionally flat — it does not encode the
/// `snowflake/` subdirectory in the template name, because the macros
/// are dialect-specific by content but namespaced by package.  The
/// caller is responsible for only invoking these on Snowflake.
pub fn propagation_macro_templates() -> Vec<(String, String)> {
    use assets::PropagationMacroAssets;
    PropagationMacroAssets::iter()
        .filter(|p| p.ends_with(".sql"))
        .filter_map(|p| {
            let content = PropagationMacroAssets::get(p.as_ref())?;
            let sql = String::from_utf8_lossy(&content.data).into_owned();
            let macro_name = std::path::Path::new(p.as_ref())
                .file_stem()?
                .to_str()?
                .to_string();
            // Inside each .sql file the actual macro defines its own name
            // (e.g. `fetch_snowflake_column_tags`).  The template name we
            // register here is the package + the macro's stable callable
            // name, which mirrors the file stem.  The two diverge in two
            // of our macros: `fetch_column_tags.sql` defines
            // `fetch_snowflake_column_tags` and `apply_column_tag.sql`
            // defines `apply_snowflake_column_tag`.  We use the macro's
            // callable name (not the file stem) for the template name so
            // callers can do `run_operation("dbt_classification.fetch_snowflake_column_tags", …)`.
            let template_basename = match macro_name.as_str() {
                "fetch_column_tags" => "fetch_snowflake_column_tags",
                "apply_column_tag" => "apply_snowflake_column_tag",
                other => other,
            };
            Some((format!("{PACKAGE_NAME}.{template_basename}"), sql))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embeds_both_macros() {
        let templates = propagation_macro_templates();
        let names: Vec<&str> = templates.iter().map(|(n, _)| n.as_str()).collect();
        assert!(
            names.contains(&"dbt_classification.fetch_snowflake_column_tags"),
            "got: {names:?}"
        );
        assert!(
            names.contains(&"dbt_classification.apply_snowflake_column_tag"),
            "got: {names:?}"
        );
    }

    #[test]
    fn fetch_macro_content_references_tag_references_all_columns() {
        let templates = propagation_macro_templates();
        let (_, sql) = templates
            .iter()
            .find(|(n, _)| n == "dbt_classification.fetch_snowflake_column_tags")
            .expect("fetch macro present");
        assert!(sql.contains("TAG_REFERENCES_ALL_COLUMNS"));
    }

    #[test]
    fn apply_macro_content_references_alter_relation_set_tag() {
        let templates = propagation_macro_templates();
        let (_, sql) = templates
            .iter()
            .find(|(n, _)| n == "dbt_classification.apply_snowflake_column_tag")
            .expect("apply macro present");
        assert!(sql.contains("ALTER"));
        assert!(sql.contains("SET TAG"));
    }
}
