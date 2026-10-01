use chrono::{DateTime, Utc};
use dbt_telemetry::{QueryExecuted, QueryOutcome};
use std::fmt::Write as _;
use std::time::SystemTime;

use super::duration::format_duration_for_summary;

/// Format a query log event from QueryExecuted attributes and timing information
pub fn format_query_log(
    query_data: &QueryExecuted,
    start_time: SystemTime,
    end_time: SystemTime,
) -> String {
    let mut buf = String::new();

    writeln!(
        &mut buf,
        "-- created_at: {}",
        DateTime::<Utc>::from(start_time).to_rfc3339()
    )
    .unwrap();
    writeln!(
        &mut buf,
        "-- finished_at: {}",
        DateTime::<Utc>::from(end_time).to_rfc3339()
    )
    .unwrap();
    if let Ok(dur) = end_time.duration_since(start_time) {
        writeln!(&mut buf, "-- elapsed: {}", format_duration_for_summary(dur)).unwrap();
    }
    writeln!(
        &mut buf,
        "-- outcome: {}",
        query_data.query_outcome().as_ref()
    )
    .unwrap();

    if query_data.query_outcome() == QueryOutcome::Error {
        if let Some(vendor_code) = query_data.query_error_vendor_code {
            writeln!(&mut buf, "-- error vendor code: {vendor_code}").unwrap();
        }

        if let Some(adapter_message) = query_data.query_error_adapter_message.as_deref() {
            // Adapter errors are routinely multi-line: DuckDB (and so lake
            // compute) appends candidate bindings and a `LINE n: ... ^` excerpt
            // of the offending statement. Every line has to carry the comment
            // marker, or the continuation lines read as SQL -- which is what made
            // this file look like the header had swallowed the statement and left
            // only its last line behind.
            let mut lines = adapter_message.lines();
            let first = lines.next().unwrap_or_default();
            writeln!(&mut buf, "-- error message: {first}").unwrap();
            for line in lines {
                if line.is_empty() {
                    writeln!(&mut buf, "--").unwrap();
                } else {
                    writeln!(&mut buf, "--   {line}").unwrap();
                }
            }
        }
    }
    writeln!(&mut buf, "-- dialect: {}", query_data.adapter_type.as_str()).unwrap();

    let node_id = query_data.unique_id.as_deref().unwrap_or("not available");
    writeln!(&mut buf, "-- node_id: {node_id}").unwrap();

    let query_id = query_data.query_id.as_deref().unwrap_or("not available");
    writeln!(&mut buf, "-- query_id: {query_id}").unwrap();

    match query_data.query_description.as_deref() {
        Some(desc) => writeln!(&mut buf, "-- desc: {desc}").unwrap(),
        None => writeln!(&mut buf, "-- desc: not provided").unwrap(),
    }

    write!(&mut buf, "{}", query_data.sql).unwrap();
    if !query_data.sql.ends_with(";") {
        write!(&mut buf, ";").unwrap();
    }

    // Close with a newline of our own. The sink appends exactly one newline per
    // entry (`TelemetryPrettyWriterLayer::on_span_end` -> `writer.writeln`), so
    // without this the statement butts straight up against the next entry's
    // `-- created_at:` header. That reads as though this entry has no statement
    // and the statement above belongs to the following header -- the whole file
    // looks like headers with the SQL missing.
    writeln!(&mut buf).unwrap();

    buf
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn errored(adapter_message: &str) -> String {
        let mut query = QueryExecuted::start(
            "select 1\nfrom t".to_string(),
            "hash".to_string(),
            "lake_compute".to_string(),
            None,
            Some("execute adapter call".to_string()),
        );
        query.set_query_outcome(QueryOutcome::Error);
        query.query_error_vendor_code = Some(0);
        query.query_error_adapter_message = Some(adapter_message.to_string());

        let start = SystemTime::UNIX_EPOCH;
        format_query_log(&query, start, start + Duration::from_secs(1))
    }

    /// Every line of the header has to be a SQL comment: the statement itself is
    /// the only uncommented text in the file, and readers (people and editors
    /// alike) rely on that to tell the two apart.
    #[test]
    fn a_multiline_adapter_error_stays_inside_sql_comments() {
        let log = errored(
            "Internal: [BinderException] Binder Error: Referenced column \"__\" not found!\n\
             Candidate bindings: \"_fivetran_synced\"\n\
             \n\
             LINE 15: ...     from \"db\".\"asana\".\"project_task\" as source_table",
        );

        let (header, sql) = log
            .split_once("select 1")
            .expect("statement is logged in full");
        assert!(
            header.lines().all(|line| line.starts_with("--")),
            "uncommented header line in:\n{header}"
        );
        assert!(header.contains("--   Candidate bindings: \"_fivetran_synced\""));
        assert!(header.contains(
            "--   LINE 15: ...     from \"db\".\"asana\".\"project_task\" as source_table"
        ));
        assert_eq!(sql, "\nfrom t;\n");
    }

    /// The sink appends one newline per entry, so an entry that does not end in
    /// one leaves its statement butted against the next entry's `created_at:`
    /// header. Every entry read as a header block with no statement, and the
    /// statement above it looked like it belonged to the following header.
    #[test]
    fn an_entry_ends_with_a_blank_line_once_the_sink_adds_its_own_newline() {
        let entry = errored("boom");
        assert!(entry.ends_with(";\n"), "no trailing newline: {entry:?}");

        // What the file holds for two consecutive entries: one `writeln` each.
        let file = format!("{entry}\n{entry}\n");
        assert!(
            file.contains(";\n\n-- created_at:"),
            "entries are not separated by a blank line:\n{file}"
        );
    }

    #[test]
    fn a_single_line_adapter_error_is_unchanged() {
        assert!(
            errored("Catalog Error: Table with name asana__task does not exist!").contains(
                "-- error message: Catalog Error: Table with name asana__task does not exist!\n"
            )
        );
    }
}
