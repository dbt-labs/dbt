use dbt_adapter_core::AdapterType;

super::define_adapter_ident!(AdapterType::Databricks);

#[cfg(test)]
mod tests {
    use super::Ident;

    #[test]
    fn databricks_ident_binds_backend() {
        assert_eq!(Ident::new("my_table").to_string(), "my_table");
        assert_eq!(Ident::new("my-table").to_string(), "`my-table`");
        assert_eq!(Ident::new("join").to_string(), "`join`");
        assert_eq!(Ident::from_unquoted("a`b").to_string(), "`a``b`");
    }

    #[test]
    fn databricks_ident_from_string_quotes_unsafe_values() {
        assert_eq!(
            Ident::from_string("safe_name", false).to_string(),
            "safe_name"
        );
        assert_eq!(
            Ident::from_string("table`; SELECT 1; --", false).to_string(),
            "`table``; SELECT 1; --`"
        );
    }
}
