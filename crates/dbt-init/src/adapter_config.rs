//! Re-export of the profile/adapter configuration schemas.
//!
//! The implementation lives in the `dbt-profile-schemas` crate; this module
//! re-exports it so existing `crate::adapter_config::*` paths keep working.
pub use dbt_profile_schemas::*;
