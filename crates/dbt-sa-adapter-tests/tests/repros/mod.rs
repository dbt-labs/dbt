//! Bug regression tests, one file per adapter.
//!
//! Each test reproduces a reported, fixed bug from a project under
//! `tests/data/repros/<repro>/`, so the bug stays fixed. Link the issue in the
//! test's doc comment. Tests of adapter methods and features go in `adapter`.

pub mod bigquery;
pub mod databricks;
pub mod redshift;
pub mod snowflake;
