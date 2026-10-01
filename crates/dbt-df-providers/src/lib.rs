//! DataFusion provider implementations backed by the dbt schema store.
//!
//! These types bridge the gap between canonicalized schemas persisted by
//! `dbt-schema-store` and DataFusion's catalog/table abstractions.  They lazily
//! provision catalogs, schemas, and table providers so that the Fusion runtime
//! can rely on consistent schema resolution without pre-registering every table.

pub mod arrow_sendable;
pub mod catalog_list;
pub mod context;
pub mod dbt_csv_mem_table;
pub mod delayed_table;
pub mod in_memory_providers;
pub mod seed_io;
