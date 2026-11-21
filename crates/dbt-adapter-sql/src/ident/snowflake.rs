use dbt_adapter_core::AdapterType;

super::define_adapter_ident!(AdapterType::Snowflake);

#[cfg(test)]
mod tests {
    use super::Ident;

    #[test]
    fn snowflake_ident_binds_backend() {
        assert_eq!(Ident::new("MY_TABLE").to_string(), "MY_TABLE");
        assert_eq!(Ident::new("my-table").to_string(), r#""my-table""#);
        assert_eq!(Ident::new("select").to_string(), r#""select""#);
        assert_eq!(Ident::from_unquoted(r#"a"b"#).to_string(), r#""a""b""#);
    }
}
