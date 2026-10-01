//! Source-available [SourcesExtractor] implementation.

use dbt_adapter_core::AdapterType;
use dbt_common::adapter::dialect_of;
use dbt_frontend_common::FullyQualifiedName;
use dbt_frontend_common::error::{CodeLocation, ErrorCode, FrontendError, FrontendResult};
use dbt_frontend_common::named_reference::NamedReference;
use dbt_frontend_common::sources_extractor::SourcesExtractor;

#[derive(Default)]
pub struct DefaultSourcesExtractor;

fn normalize_default_catalog_schema(
    adapter_type: AdapterType,
    default_catalog: &str,
    default_schema: &str,
) -> FrontendResult<(String, String)> {
    let dialect = dialect_of(adapter_type).ok_or_else(|| {
        Box::new(FrontendError::new(
            ErrorCode::NotSupported,
            CodeLocation::default(),
            format!("Dialect not found for adapter type {}", adapter_type),
        ))
    })?;
    // Normalize the default catalog/schema through the dialect's identifier
    // parser so unquoted names get the same case-folding the dialect applies
    // (e.g. Snowflake uppercases unquoted identifiers). Without this, a
    // lowercase default_catalog such as "development" ends up in the FQN as
    // a quoted "development", which the run-cache service's canonicalization
    // treats as a case-sensitive identifier and fails to match against the
    // stored uppercase "DEVELOPMENT" form.
    let normalized_catalog = dialect
        .parse_identifier(default_catalog)
        .map(|id| id.to_value())
        .unwrap_or_else(|_| default_catalog.to_string());
    let normalized_schema = dialect
        .parse_identifier(default_schema)
        .map(|id| id.to_value())
        .unwrap_or_else(|_| default_schema.to_string());
    Ok((normalized_catalog, normalized_schema))
}

impl SourcesExtractor for DefaultSourcesExtractor {
    fn extract_upstreams(
        &self,
        adapter_type: AdapterType,
        sql: &str,
        default_catalog: &str,
        default_schema: &str,
        _quoted_name_ignore_case: bool,
    ) -> FrontendResult<Vec<NamedReference<FullyQualifiedName>>> {
        let (normalized_catalog, normalized_schema) =
            normalize_default_catalog_schema(adapter_type, default_catalog, default_schema)?;
        Ok(crate::extract_sources::extract_sources_from_str(
            sql,
            adapter_type,
            &normalized_catalog,
            &normalized_schema,
        )
        .map_err(|e| {
            Box::new(FrontendError::new(
                ErrorCode::Unexpected,
                CodeLocation::default(),
                e.to_string(),
            ))
        })?
        .into_iter()
        .map(|entity| entity.into())
        .collect::<Vec<_>>())
    }

    fn extract_standalone_expression_upstreams(
        &self,
        adapter_type: AdapterType,
        sql: &str,
        default_catalog: &str,
        default_schema: &str,
        _quoted_name_ignore_case: bool,
    ) -> FrontendResult<Vec<NamedReference<FullyQualifiedName>>> {
        let (normalized_catalog, normalized_schema) =
            normalize_default_catalog_schema(adapter_type, default_catalog, default_schema)?;
        Ok(
            crate::extract_sources::extract_sources_from_standalone_expression(
                sql,
                adapter_type,
                &normalized_catalog,
                &normalized_schema,
            )
            .map_err(|e| {
                Box::new(FrontendError::new(
                    ErrorCode::Unexpected,
                    CodeLocation::default(),
                    e.to_string(),
                ))
            })?
            .into_iter()
            .map(|entity| entity.into())
            .collect::<Vec<_>>(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standalone_expression_normalizes_default_catalog_and_schema() {
        let sources = DefaultSourcesExtractor
            .extract_standalone_expression_upstreams(
                AdapterType::Snowflake,
                "(select max(id) from users)",
                "development",
                "analytics",
                false,
            )
            .unwrap();

        assert_eq!(sources.len(), 1);
        let source = sources.into_iter().next().unwrap().into_inner();
        assert_eq!(source.catalog().name(), "DEVELOPMENT");
        assert_eq!(source.schema().name(), "ANALYTICS");
        assert_eq!(source.table().name(), "USERS");
    }
}
