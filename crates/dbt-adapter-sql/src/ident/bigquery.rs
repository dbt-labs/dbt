use dbt_adapter_core::AdapterType;

use crate::is_keyword_ignore_ascii_case;

super::define_adapter_ident!(AdapterType::Bigquery);

/// Returns true iff `id` has the shape of a BigQuery system view name (e.g.
/// `INFORMATION_SCHEMA.TABLES`) that can be rendered unquoted: non-empty,
/// only ASCII uppercase letters and `_`, and not a keyword.
///
/// This checks the shape only, not membership in BigQuery's set of system views.
pub fn is_sys_identifier(id: &str) -> bool {
    !id.is_empty()
        && id.chars().all(|c| c == '_' || c.is_ascii_uppercase())
        && is_keyword_ignore_ascii_case(id, AdapterType::Bigquery).is_none()
}

#[cfg(test)]
mod tests {
    use super::{Ident, is_sys_identifier};

    #[test]
    fn bigquery_sys_identifier() {
        assert!(is_sys_identifier("TABLES"));
        assert!(is_sys_identifier("JOBS_BY_PROJECT"));
        assert!(!is_sys_identifier(""));
        assert!(!is_sys_identifier("SELECT"));
        assert!(!is_sys_identifier("Tables"));
        assert!(!is_sys_identifier("FOO$BAR"));
    }

    #[test]
    fn bigquery_ident_binds_backend() {
        assert_eq!(Ident::new("TABLES").to_string(), "TABLES");
        assert_eq!(Ident::new("region-us").to_string(), "`region-us`");
        assert_eq!(Ident::from_unquoted(r"a\b`c").to_string(), r"`a\\b\`c`");
        assert_eq!(Ident::new("SELECT").as_ref(), "SELECT");
    }
}
