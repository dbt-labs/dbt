/// This module contains the functions implementations for the dbt jinja context
pub mod base;

/// This module contains the contract_mismatches function for comparing YAML and SQL column definitions
mod contract_error;

pub use base::*;
pub use dbt_jinja_vars::ConfiguredVar;
