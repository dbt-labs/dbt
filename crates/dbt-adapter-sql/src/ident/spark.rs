use std::fmt;

use dbt_adapter_core::AdapterType;

super::define_adapter_ident!(AdapterType::Spark);

/// Spark namespace = (<catalog> '.') ? <schema>
pub struct Namespace(Option<Ident>, Ident);

impl Namespace {
    pub fn new(catalog: impl Into<String>, schema: impl Into<String>) -> Self {
        let catalog = catalog.into();
        let catalog = if catalog.is_empty() {
            None
        } else {
            Some(Ident::from_unquoted(catalog))
        };
        let schema = Ident::from_unquoted(schema.into());
        Self(catalog, schema)
    }
}

impl fmt::Display for Namespace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.0, &self.1) {
            (None, schema) => write!(f, "{}", schema),
            (Some(catalog), schema) => write!(f, "{catalog}.{schema}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spark_ident_binds_backend() {
        assert_eq!(Ident::new("my_table").to_string(), "my_table");
        assert_eq!(Ident::new("my-table").to_string(), "`my-table`");
        assert_eq!(Ident::new("join").to_string(), "join");
        assert_eq!(
            Ident::from_string("my_table", true).to_string(),
            "`my_table`"
        );
        assert_eq!(
            Ident::from_unquoted(r#"a`b"c\d"#).to_string(),
            r#"`a``b"c\d`"#
        );
    }

    #[test]
    fn spark_namespace_escapes_catalog_and_schema() {
        assert_eq!(
            Namespace::new(r"catalog\`; SELECT 1; --", r"schema\`; SELECT 2; --").to_string(),
            r"`catalog\``; SELECT 1; --`.`schema\``; SELECT 2; --`"
        );
    }
}
