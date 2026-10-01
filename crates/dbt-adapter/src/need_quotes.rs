use dbt_adapter_core::AdapterType;

/// Returns true if the identifier SHOULD be quoted when formatting to
/// source code to preserve the casing of the provided identifier in this
/// backend's dialect.
pub fn need_quotes(adapter_type: AdapterType, id: &str) -> bool {
    dbt_adapter_sql::ident::must_be_quoted(id, adapter_type)
        // In Snowflake, unquoted identifiers are normalized to uppercase,
        // therefore if the identifier contains any lowercase characters, it
        // needs to be quoted to preserve the original casing.
        || (matches!(adapter_type, AdapterType::Snowflake) && id.chars().any(|c| c.is_ascii_lowercase()))
        // In Redshift, unquoted identifiers are normalized to lowercase,
        // therefore if the identifier contains any uppercase characters, it
        // needs to be quoted to preserve the original casing.
        || (matches!(adapter_type, AdapterType::Redshift) && id.chars().any(|c| c.is_ascii_uppercase()))
}
