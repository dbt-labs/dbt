use std::collections::HashMap;

use arrow_schema::{Field, Schema};

/// Arrow field metadata key for relation-specific identifier matching semantics.
pub const IDENTIFIER_CASE_SENSITIVITY_METADATA_KEY: &str = "DBT:identifier_case_sensitivity";
const IDENTIFIER_POLICY_FIELD_NAME_METADATA_KEY: &str = "DBT:identifier_policy_field_name";

/// Identifier matching semantics reported by a relation's metadata provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentifierCaseSensitivity {
    CaseSensitive,
    CaseInsensitive,
}

impl IdentifierCaseSensitivity {
    const CASE_SENSITIVE: &'static str = "case_sensitive";
    const CASE_INSENSITIVE: &'static str = "case_insensitive";

    pub fn from_field(field: &Field) -> Option<Self> {
        if field
            .metadata()
            .get(IDENTIFIER_POLICY_FIELD_NAME_METADATA_KEY)
            .is_none_or(|original_name| original_name != field.name())
        {
            return None;
        }
        match field
            .metadata()
            .get(IDENTIFIER_CASE_SENSITIVITY_METADATA_KEY)
            .map(String::as_str)
        {
            Some(Self::CASE_SENSITIVE) => Some(Self::CaseSensitive),
            Some(Self::CASE_INSENSITIVE) => Some(Self::CaseInsensitive),
            _ => None,
        }
    }

    pub fn apply_to_schema(self, schema: &Schema) -> Schema {
        let value = match self {
            Self::CaseSensitive => Self::CASE_SENSITIVE,
            Self::CaseInsensitive => Self::CASE_INSENSITIVE,
        };
        let fields = schema
            .fields()
            .iter()
            .map(|field| {
                let mut metadata: HashMap<String, String> = field.metadata().clone();
                metadata.insert(
                    IDENTIFIER_CASE_SENSITIVITY_METADATA_KEY.to_string(),
                    value.to_string(),
                );
                metadata.insert(
                    IDENTIFIER_POLICY_FIELD_NAME_METADATA_KEY.to_string(),
                    field.name().to_string(),
                );
                field.as_ref().clone().with_metadata(metadata)
            })
            .collect::<Vec<_>>();

        Schema::new_with_metadata(fields, schema.metadata().clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_schema::DataType;

    #[test]
    fn applies_identifier_semantics_without_discarding_field_metadata() {
        let field = Field::new("id", DataType::Int64, false).with_metadata(HashMap::from([(
            "existing".to_string(),
            "value".to_string(),
        )]));
        let schema = Schema::new_with_metadata(
            vec![field],
            HashMap::from([("schema".to_string(), "metadata".to_string())]),
        );

        let annotated = IdentifierCaseSensitivity::CaseInsensitive.apply_to_schema(&schema);
        let annotated_field = annotated.field(0);

        assert_eq!(
            IdentifierCaseSensitivity::from_field(annotated_field),
            Some(IdentifierCaseSensitivity::CaseInsensitive)
        );
        assert_eq!(annotated_field.metadata().get("existing").unwrap(), "value");
        assert_eq!(annotated.metadata().get("schema").unwrap(), "metadata");

        let renamed = annotated_field.as_ref().clone().with_name("renamed");
        assert_eq!(IdentifierCaseSensitivity::from_field(&renamed), None);
    }
}
