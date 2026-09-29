use std::path::PathBuf;

use super::*;

fn options() -> ExportOptions {
    ExportOptions {
        info_schema_dir: PathBuf::from("target/info_schema/v1"),
        output_dir: PathBuf::from("target"),
        duckdb_cdn_base: None,
        analytics_enabled: true,
    }
}

fn bootstrap(options: &ExportOptions) -> SiteBootstrap {
    SiteBootstrap::new(&Providers::default(), options)
}

fn payload(bootstrap: &SiteBootstrap) -> serde_json::Value {
    let tag = bootstrap.script_tag();
    let json = tag
        .trim_start_matches("<script>window.__DBT_DOCS__ = ")
        .trim_end_matches(";</script>");
    serde_json::from_str(json).expect("bootstrap payload is valid JSON")
}

/// The client resolves the data directory from this rather than hardcoding it.
///
/// It is the information schema's own versioned directory, read where it was
/// written — and version-scoped, which is the reason it cannot be a client-side
/// constant: the version belongs to the writer.
#[test]
fn payload_carries_the_data_directory() {
    let value = payload(&bootstrap(&options()));
    assert_eq!(value["data_dir"], format!("{}/", crate::export::data_dir()));
    assert_eq!(
        value["data_dir"],
        format!("info_schema/v{}/", dbt_index_core::INFO_SCHEMA_VERSION),
        "the shape the SPA resolves against document.baseURI"
    );
}

#[test]
fn payload_is_well_formed_json() {
    let value = payload(&bootstrap(&options()));
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["distribution"], "oss");
    assert_eq!(value["is_logged_in"], false);
    assert!(
        value["generated_at"]
            .as_str()
            .is_some_and(|s| s.ends_with('Z'))
    );
}

#[test]
fn cdn_base_defaults_to_the_pinned_bundle() {
    let value = payload(&bootstrap(&options()));
    assert_eq!(value["duckdb_cdn_base"], DEFAULT_DUCKDB_CDN_BASE);
    // Pinned so `selectBundle` cannot resolve a different engine than the one
    // the site was tested against.
    assert!(DEFAULT_DUCKDB_CDN_BASE.contains("@duckdb/duckdb-wasm@"));
}

#[test]
fn cdn_base_is_overridable_for_mirrors() {
    let mut options = options();
    options.duckdb_cdn_base = Some("https://mirror.internal/duckdb".to_string());
    let value = payload(&bootstrap(&options));
    assert_eq!(value["duckdb_cdn_base"], "https://mirror.internal/duckdb");
}

#[test]
fn consent_is_resolved_at_export_time() {
    let mut options = options();
    options.analytics_enabled = false;
    let value = payload(&bootstrap(&options));
    assert_eq!(value["telemetry"]["enabled"], false);
}

#[test]
fn carries_no_column_lineage_flag() {
    // The browser derives this from whether the artifact loaded. A second copy
    // here could disagree with the data.
    let value = payload(&bootstrap(&options()));
    assert!(value.get("has_column_lineage").is_none());
    assert!(
        !bootstrap(&options())
            .script_tag()
            .contains("column_lineage")
    );
}

#[test]
fn injects_at_the_placeholder_when_present() {
    let html = "<html><head><!--dbt-docs-bootstrap--><title>x</title></head></html>";
    let out = bootstrap(&options()).inject(html);
    assert!(!out.contains("<!--dbt-docs-bootstrap-->"));
    assert!(out.contains("window.__DBT_DOCS__"));
    // Replaced in place, ahead of the rest of head.
    let script_at = out.find("window.__DBT_DOCS__").unwrap();
    assert!(script_at < out.find("<title>").unwrap());
}

#[test]
fn falls_back_to_before_head_close() {
    let html = "<html><head><title>x</title></head><body></body></html>";
    let out = bootstrap(&options()).inject(html);
    let script_at = out.find("window.__DBT_DOCS__").unwrap();
    assert!(script_at < out.find("</head>").unwrap());
    assert!(out.contains("<body></body>"));
}

#[test]
fn falls_back_to_prepending_when_there_is_no_head() {
    let out = bootstrap(&options()).inject("<div id=\"root\"></div>");
    assert!(out.starts_with("<script>"));
    assert!(out.ends_with("<div id=\"root\"></div>"));
}

#[test]
fn escapes_markup_so_the_script_cannot_be_terminated_early() {
    let mut options = options();
    options.duckdb_cdn_base = Some("https://x/</script><script>alert(1)</script>&amp;".to_string());
    let tag = bootstrap(&options).script_tag();

    // Exactly the wrapper's own tags survive: the payload cannot break out,
    // and cannot reach the tokenizer's double-escaped state via a bare
    // `<script` either.
    assert_eq!(tag.matches("<script>").count(), 1, "{tag}");
    assert_eq!(tag.matches("</script>").count(), 1, "{tag}");
    // The payload itself carries no raw angle bracket at all, so no tokenizer
    // state can be reached from it.
    let body = tag
        .trim_start_matches("<script>")
        .trim_end_matches("</script>");
    assert!(!body.contains('<'), "{body}");
    assert!(!body.contains('>'), "{body}");

    // Escaped, not dropped: the value round-trips through JSON unchanged.
    let value = payload(&bootstrap(&options));
    assert_eq!(
        value["duckdb_cdn_base"],
        "https://x/</script><script>alert(1)</script>&amp;"
    );
}
