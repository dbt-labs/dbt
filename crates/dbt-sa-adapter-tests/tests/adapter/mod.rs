//! Adapter method and feature tests, one file per adapter.
//!
//! Each test drives an adapter method or a feature (materializations, configs,
//! catalogs, ...) through a project under `tests/data/adapter/<feature>/`,
//! usually shared by every adapter that supports it. Regression tests for
//! reported bugs go in `repros`.

pub mod bigquery;
pub mod clickhouse;
pub mod databricks;
pub mod snowflake;
