//! Relation and RelationConfig implementations for different data warehouses.

pub(crate) mod config;
pub use config::{BaseRelationChangeSet, BaseRelationConfig, ComponentConfig, RelationChangeSet};

pub mod bigquery;
pub mod databricks;
pub mod redshift;
pub mod snowflake;
pub mod spark;

pub mod factory;

mod relation_impl;
pub use relation_impl::{Relation, RelationStatic};

mod relation_object;
pub use relation_object::{
    RelationObject, StaticBaseRelation, StaticBaseRelationObject, canonical_relation_parts,
    create_relation, create_relation_from_node, create_relation_from_source, do_create_relation,
    is_parse_time_relation, render_effective_relation,
};

pub(crate) mod config_v2;
pub use config_v2::RelationConfig;

#[cfg(test)]
pub(crate) mod test_helpers;

#[cfg(test)]
mod tests {
    use chrono::{DateTime, NaiveDate, Utc};
    use dbt_schemas::dbt_types::RelationType;
    use dbt_schemas::{
        filter::{RunFilter, Sample},
        schemas::{common::ResolvedQuoting, relations::base::BaseRelation as _},
    };

    use crate::AdapterType;

    use super::*;

    #[test]
    fn test_render_with_run_filter_snowflake_adapter() {
        let relation = Relation::new(
            AdapterType::Snowflake,
            None::<String>,
            None::<String>,
            "my_table".to_owned(),
        )
        .with_quoting(ResolvedQuoting::disabled());
        let start = NaiveDate::from_ymd_opt(2024, 7, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();
        let end = NaiveDate::from_ymd_opt(2024, 7, 8)
            .unwrap()
            .and_hms_opt(18, 0, 0)
            .unwrap();

        let sample = Sample {
            start: Some(DateTime::<Utc>::from_naive_utc_and_offset(start, Utc)),
            end: Some(DateTime::<Utc>::from_naive_utc_and_offset(end, Utc)),
        };

        let run_filter = RunFilter {
            empty: false,
            sample: Some(sample),
        };
        let event_time = Some("created_at".to_string());

        let result = relation.render_with_run_filter(&run_filter, &event_time);
        assert_eq!(
            result,
            "(select * from my_table where created_at >= to_timestamp_tz('2024-07-01T00:00:00+00:00') and created_at < to_timestamp_tz('2024-07-08T18:00:00+00:00'))"
        );
    }

    // Regression for dbt-labs/dbt-fusion#1608: the source-side event-time
    // predicate on Snowflake must emit the explicit `+00:00` UTC offset so it
    // resolves consistently with the microbatch DELETE predicate in non-UTC
    // Snowflake sessions. Uses a non-midnight hour to mirror the reporter's
    // hourly batch window.
    #[test]
    fn test_render_with_run_filter_snowflake_microbatch_includes_utc_offset() {
        let relation = Relation::new(
            AdapterType::Snowflake,
            None::<String>,
            None::<String>,
            "stg_events".to_owned(),
        )
        .with_quoting(ResolvedQuoting::disabled());
        let start = NaiveDate::from_ymd_opt(2026, 4, 27)
            .unwrap()
            .and_hms_opt(16, 0, 0)
            .unwrap();
        let end = NaiveDate::from_ymd_opt(2026, 4, 27)
            .unwrap()
            .and_hms_opt(17, 0, 0)
            .unwrap();

        let run_filter = RunFilter {
            empty: false,
            sample: Some(Sample {
                start: Some(DateTime::<Utc>::from_naive_utc_and_offset(start, Utc)),
                end: Some(DateTime::<Utc>::from_naive_utc_and_offset(end, Utc)),
            }),
        };
        let event_time = Some("event_at".to_string());

        let result = relation.render_with_run_filter(&run_filter, &event_time);
        assert!(
            result.contains("to_timestamp_tz('2026-04-27T16:00:00+00:00')"),
            "expected source-side start predicate to carry +00:00, got: {result}"
        );
        assert!(
            result.contains("to_timestamp_tz('2026-04-27T17:00:00+00:00')"),
            "expected source-side end predicate to carry +00:00, got: {result}"
        );
    }

    #[test]
    fn test_render_with_run_filter_bigquery_adapter() {
        let relation = Relation::new(
            AdapterType::Bigquery,
            None::<String>,
            None::<String>,
            "my_table".to_owned(),
        )
        .with_quoting(ResolvedQuoting::disabled());
        let start = NaiveDate::from_ymd_opt(2024, 7, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();
        let end = NaiveDate::from_ymd_opt(2024, 7, 8)
            .unwrap()
            .and_hms_opt(18, 0, 0)
            .unwrap();

        let sample = Sample {
            start: Some(DateTime::<Utc>::from_naive_utc_and_offset(start, Utc)),
            end: Some(DateTime::<Utc>::from_naive_utc_and_offset(end, Utc)),
        };

        let run_filter = RunFilter {
            empty: false,
            sample: Some(sample),
        };
        let event_time = Some("created_at".to_string());

        let result = relation.render_with_run_filter(&run_filter, &event_time);
        assert_eq!(
            result,
            "(select * from my_table where cast(created_at as timestamp) >= '2024-07-01T00:00:00.000000Z' and cast(created_at as timestamp) < '2024-07-08T18:00:00.000000Z')"
        );
    }

    #[test]
    fn test_render_with_run_filter_redshift_adapter() {
        // relation impl in core doesn't seem to override this
        let relation = Relation::new(
            AdapterType::Redshift,
            None::<String>,
            None::<String>,
            "my_table".to_owned(),
        )
        .with_quoting(ResolvedQuoting::disabled());
        let start = NaiveDate::from_ymd_opt(2024, 7, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();
        let end = NaiveDate::from_ymd_opt(2024, 7, 8)
            .unwrap()
            .and_hms_opt(18, 0, 0)
            .unwrap();

        let sample = Sample {
            start: Some(DateTime::<Utc>::from_naive_utc_and_offset(start, Utc)),
            end: Some(DateTime::<Utc>::from_naive_utc_and_offset(end, Utc)),
        };

        let run_filter = RunFilter {
            empty: false,
            sample: Some(sample),
        };
        let event_time = Some("created_at".to_string());

        let result = relation.render_with_run_filter(&run_filter, &event_time);
        assert_eq!(
            result,
            "(select * from my_table where created_at >= '2024-07-01T00:00:00+00:00' and created_at < '2024-07-08T18:00:00+00:00')"
        );
    }

    #[test]
    fn test_render_with_run_filter_databricks_adapter() {
        // relation impl in dbt-databricks doesn't seem to override this
        let relation = Relation::new(
            AdapterType::Databricks, // ?
            None::<String>,
            None::<String>,
            "my_table".to_owned(),
        )
        .with_quoting(ResolvedQuoting::disabled());
        let start = NaiveDate::from_ymd_opt(2024, 7, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();
        let end = NaiveDate::from_ymd_opt(2024, 7, 8)
            .unwrap()
            .and_hms_opt(18, 0, 0)
            .unwrap();

        let sample = Sample {
            start: Some(DateTime::<Utc>::from_naive_utc_and_offset(start, Utc)),
            end: Some(DateTime::<Utc>::from_naive_utc_and_offset(end, Utc)),
        };

        let run_filter = RunFilter {
            empty: false,
            sample: Some(sample),
        };
        let event_time = Some("created_at".to_string());

        let result = relation.render_with_run_filter(&run_filter, &event_time);
        assert_eq!(
            result,
            "(select * from my_table where created_at >= '2024-07-01T00:00:00+00:00' and created_at < '2024-07-08T18:00:00+00:00')"
        );
    }

    #[test]
    fn test_do_create_relation_duckdb_includes_attached_catalog() {
        let relation = do_create_relation(
            AdapterType::DuckDB,
            "stocks_dev".to_string(),
            "main".to_string(),
            Some("files".to_string()),
            Some(RelationType::Table),
            ResolvedQuoting {
                database: true,
                schema: true,
                identifier: true,
            },
        )
        .unwrap();

        assert_eq!(
            relation.render_self_as_str(),
            "\"stocks_dev\".\"main\".\"files\""
        );
    }

    #[test]
    fn test_render_with_run_filter_clickhouse_adapter() {
        let relation = Relation::new(
            AdapterType::ClickHouse,
            None::<String>,
            "analytics".to_string(),
            "events".to_owned(),
        )
        .with_quoting(ResolvedQuoting::disabled());
        let start = NaiveDate::from_ymd_opt(2024, 7, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();
        let end = NaiveDate::from_ymd_opt(2024, 7, 8)
            .unwrap()
            .and_hms_opt(18, 0, 0)
            .unwrap();

        let sample = Sample {
            start: Some(DateTime::<Utc>::from_naive_utc_and_offset(start, Utc)),
            end: Some(DateTime::<Utc>::from_naive_utc_and_offset(end, Utc)),
        };

        let run_filter = RunFilter {
            empty: false,
            sample: Some(sample),
        };
        let event_time = Some("created_at".to_string());

        let result = relation.render_with_run_filter(&run_filter, &event_time);
        assert_eq!(
            result,
            "(select * from analytics.events where created_at >= parseDateTime64BestEffort('2024-07-01T00:00:00+00:00', 9) and created_at < parseDateTime64BestEffort('2024-07-08T18:00:00+00:00', 9))"
        );
    }
}
