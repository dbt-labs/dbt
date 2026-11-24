use std::fmt;

use dbt_adapter_core::AdapterType;

super::define_adapter_ident!(AdapterType::ClickHouse);

/// ClickHouse qualified relation name = (<schema> '.') ? <identifier>
pub struct QualifiedRelationName(Option<Ident>, Ident);

impl QualifiedRelationName {
    pub fn new(schema: impl Into<String>, identifier: impl Into<String>) -> Self {
        let schema = schema.into();
        let schema = if schema.is_empty() {
            None
        } else {
            Some(Ident::from_unquoted(schema))
        };
        let identifier = Ident::from_unquoted(identifier.into());
        Self(schema, identifier)
    }
}

impl fmt::Display for QualifiedRelationName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.0, &self.1) {
            (None, identifier) => write!(f, "{identifier}"),
            (Some(schema), identifier) => write!(f, "{schema}.{identifier}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clickhouse_ident_binds_backend() {
        assert_eq!(Ident::new("my_table").to_string(), "my_table");
        assert_eq!(Ident::new("my-table").to_string(), r#""my-table""#);
        assert_eq!(Ident::new("join").to_string(), "join");
        assert_eq!(
            Ident::from_string("my_table", true).to_string(),
            r#""my_table""#
        );
        assert_eq!(
            Ident::from_unquoted(r#"a`b"c\d"#).to_string(),
            r#""a`b""c\\d""#
        );
    }

    #[test]
    fn clickhouse_qualified_relation_name_escapes_schema_and_identifier() {
        assert_eq!(
            QualifiedRelationName::new("", "my_table").to_string(),
            r#""my_table""#
        );
        assert_eq!(
            QualifiedRelationName::new(r#"schema\"; SELECT 1; --"#, r#"table\"; SELECT 2; --"#)
                .to_string(),
            r#""schema\\""; SELECT 1; --"."table\\""; SELECT 2; --""#
        );
    }
}
