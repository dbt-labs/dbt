use chrono::{DateTime, Utc};
use dbt_telemetry::{QueryExecuted, QueryOutcome};
use std::fmt;
use std::time::SystemTime;

use super::duration::SummaryDuration;

/// Displays a query log entry from QueryExecuted attributes and timing information
pub struct QueryLogEntry<'a> {
    pub query: &'a QueryExecuted,
    pub start_time: SystemTime,
    pub end_time: SystemTime,
}

impl fmt::Display for QueryLogEntry<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let query_data = self.query;
        let (start_time, end_time) = (self.start_time, self.end_time);

        writeln!(f, "-- created_at: {}", Rfc3339(start_time))?;
        writeln!(f, "-- finished_at: {}", Rfc3339(end_time))?;
        if let Ok(dur) = end_time.duration_since(start_time) {
            writeln!(f, "-- elapsed: {}", SummaryDuration(dur))?;
        }
        writeln!(f, "-- outcome: {}", query_data.query_outcome().as_ref())?;

        if query_data.query_outcome() == QueryOutcome::Error {
            if let Some(vendor_code) = query_data.query_error_vendor_code {
                writeln!(f, "-- error vendor code: {vendor_code}")?;
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
                writeln!(f, "-- error message: {first}")?;
                for line in lines {
                    if line.is_empty() {
                        writeln!(f, "--")?;
                    } else {
                        writeln!(f, "--   {line}")?;
                    }
                }
            }
        }
        writeln!(f, "-- dialect: {}", query_data.adapter_type.as_str())?;

        let node_id = query_data.unique_id.as_deref().unwrap_or("not available");
        writeln!(f, "-- node_id: {node_id}")?;

        let query_id = query_data.query_id.as_deref().unwrap_or("not available");
        writeln!(f, "-- query_id: {query_id}")?;

        match query_data.query_description.as_deref() {
            Some(desc) => writeln!(f, "-- desc: {desc}")?,
            None => writeln!(f, "-- desc: not provided")?,
        }

        f.write_str(&query_data.sql)?;
        if !query_data.sql.ends_with(";") {
            f.write_str(";")?;
        }

        // Close with a newline of our own. The sink appends exactly one newline per
        // entry (`write_query_log_record` -> `writer.writeln`), so
        // without this the statement butts straight up against the next entry's
        // `-- created_at:` header. That reads as though this entry has no statement
        // and the statement above belongs to the following header -- the whole file
        // looks like headers with the SQL missing.
        writeln!(f)
    }
}

/// Displays a timestamp as RFC 3339 with an explicit `+00:00` offset, e.g.
/// `2026-01-02T03:04:05.678901+00:00`.
struct Rfc3339(SystemTime);

impl fmt::Display for Rfc3339 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let datetime: DateTime<Utc> = self.0.into();
        // Same as `to_rfc3339` for years 0 to 9999.
        // Formatting the naive time skips the offset string `DateTime::format` allocates.
        datetime
            .naive_utc()
            .format("%Y-%m-%dT%H:%M:%S%.f+00:00")
            .write_to(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indoc::indoc;
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
        QueryLogEntry {
            query: &query,
            start_time: start,
            end_time: start + Duration::from_secs(1),
        }
        .to_string()
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
    fn an_entry_is_formatted_in_full() {
        let query = QueryExecuted {
            sql: "select *\nfrom analytics.customers".to_string(),
            sql_hash: "hash".to_string(),
            adapter_type: "snowflake".to_string(),
            query_description: Some("select customers".to_string()),
            query_id: Some("01b2c3d4-0000-1111".to_string()),
            unique_id: Some("model.jaffle_shop.customers".to_string()),
            query_outcome: QueryOutcome::Success as i32,
            ..Default::default()
        };
        let start = SystemTime::UNIX_EPOCH + Duration::new(1_767_323_045, 678_901_000);
        let entry = QueryLogEntry {
            query: &query,
            start_time: start,
            end_time: start + Duration::from_millis(1_870),
        };

        assert_eq!(
            entry.to_string(),
            indoc! {"
                -- created_at: 2026-01-02T03:04:05.678901+00:00
                -- finished_at: 2026-01-02T03:04:07.548901+00:00
                -- elapsed: 1.9s
                -- outcome: success
                -- dialect: snowflake
                -- node_id: model.jaffle_shop.customers
                -- query_id: 01b2c3d4-0000-1111
                -- desc: select customers
                select *
                from analytics.customers;
            "}
        );
    }

    #[test]
    fn timestamps_match_to_rfc3339() {
        for nanos in [0, 1, 5_000_000, 678_901_000, 999_999_999] {
            let time = SystemTime::UNIX_EPOCH + Duration::new(1_767_323_045, nanos);
            assert_eq!(
                Rfc3339(time).to_string(),
                DateTime::<Utc>::from(time).to_rfc3339()
            );
        }
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
