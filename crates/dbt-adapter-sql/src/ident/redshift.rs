use dbt_adapter_core::AdapterType;

super::define_adapter_ident!(AdapterType::Redshift);

#[cfg(test)]
mod tests {
    use super::Ident;

    #[test]
    fn redshift_ident_binds_backend() {
        assert_eq!(Ident::new("my_table").to_string(), "my_table");
        assert_eq!(Ident::new("my-table").to_string(), r#""my-table""#);
        assert_eq!(Ident::new("join").to_string(), r#""join""#);
        assert_eq!(Ident::from_unquoted(r#"a"b"#).to_string(), r#""a""b""#);
    }
}
