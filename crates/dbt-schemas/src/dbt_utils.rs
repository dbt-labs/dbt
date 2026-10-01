use std::path::{Path, PathBuf};

use dbt_adapter_core::AdapterType;
use dbt_common::{ErrorCode, FsResult, err};
use dbt_yaml::Spanned;

use crate::{
    constants::DBT_BASE_SCHEMAS_URL, schemas::common::DbtQuoting,
    schemas::relations::default_dbt_quoting_for,
};

pub fn get_prefix(x: &Path, y: &Path) -> PathBuf {
    let x_components: Vec<_> = x.components().collect();
    let y_components: Vec<_> = y.components().collect();

    if y_components.len() > x_components.len() {
        return PathBuf::from(".");
    }

    for (x_comp, y_comp) in x_components.iter().rev().zip(y_components.iter().rev()) {
        if x_comp != y_comp {
            return PathBuf::from(".");
        }
    }

    let prefix_length = x_components.len() - y_components.len();
    x_components[..prefix_length]
        .iter()
        .map(|comp| comp.as_os_str())
        .collect::<PathBuf>()
}

pub fn get_dbt_schema_version(name: &str, version: i16) -> String {
    format!("{DBT_BASE_SCHEMAS_URL}/dbt/{name}/v{version}.json")
}

/// Fill any field the user left unset with `adapter_type`'s default.
///
/// Idempotent and field-wise, so it can be applied once per package (seeding the
/// project config hierarchy) or once per node (after the node's `+adapter` is
/// known) with the same meaning. A node that selects an adapter other than the
/// target's default is resolved against *its* adapter, which is why this must
/// stay field-wise rather than all-or-nothing.
pub fn resolve_package_quoting(
    quoting: Option<DbtQuoting>,
    adapter_type: AdapterType,
) -> DbtQuoting {
    // The last layer in the precedence chain; `default_dbt_quoting_for` is the one
    // source of truth for what an adapter type defaults to.
    quoting
        .unwrap_or_default()
        .filled_from(&default_dbt_quoting_for(adapter_type))
}

/// Validate a delimiter
pub fn validate_delimiter(spanned_delimiter: &Option<Spanned<String>>) -> FsResult<Option<String>> {
    if let Some(delimiter) = spanned_delimiter.as_ref() {
        if delimiter.is_empty() {
            return Ok(None);
        } else if delimiter.len() != 1 || !delimiter.chars().next().unwrap().is_ascii() {
            return err!(
                code => ErrorCode::InvalidConfig,
                loc => delimiter.span().clone(),
                "Delimiter '{}' must be exactly one ascii character",
                delimiter.as_ref()
            );
        } else {
            return Ok(Some(delimiter.clone().into_inner()));
        }
    }

    Ok(None)
}

#[cfg(test)]
mod quoting_tests {
    use super::*;

    /// The point of resolving per node: the *same* authored quoting yields a
    /// different result depending on which adapter the node runs on. If this
    /// collapsed, a node selecting `+adapter` would silently render identifiers
    /// with the target default's rules.
    #[test]
    fn the_same_authored_quoting_resolves_per_adapter() {
        let authored = None;
        let on_snowflake = resolve_package_quoting(authored, AdapterType::Snowflake);
        let on_lake_compute = resolve_package_quoting(authored, AdapterType::LakeCompute);
        let on_duckdb = resolve_package_quoting(authored, AdapterType::DuckDB);

        // Snowflake and lake compute do not quote; DuckDB does. Same authored value in,
        // different resolved value out.
        assert_eq!(on_snowflake.identifier, Some(false));
        assert_eq!(on_lake_compute.identifier, Some(false));
        assert_eq!(on_duckdb.identifier, Some(true));
        assert_ne!(
            on_lake_compute, on_duckdb,
            "`lake_compute` has its own policy and must not resolve as DuckDB"
        );
    }

    /// Anything the user actually wrote survives an adapter switch untouched;
    /// only the unset fields take the adapter default. This is why the config
    /// hierarchy is seeded with nothing rather than a resolved value: a filled-in
    /// default is indistinguishable from something the user set.
    #[test]
    fn authored_fields_survive_the_adapter_switch() {
        let authored = Some(DbtQuoting {
            database: Some(false),
            schema: None,
            identifier: None,
            snowflake_ignore_case: None,
        });

        let on_duckdb = resolve_package_quoting(authored, AdapterType::DuckDB);
        assert_eq!(
            on_duckdb.database,
            Some(false),
            "user-set field must be kept"
        );
        assert_eq!(
            on_duckdb.schema,
            Some(true),
            "unset field takes DuckDB's default"
        );
        assert_eq!(on_duckdb.identifier, Some(true));

        // The same authored `database: false` survives onto an adapter whose
        // default disagrees, and the unset fields follow that adapter.
        let on_lake_compute = resolve_package_quoting(authored, AdapterType::LakeCompute);
        assert_eq!(
            on_lake_compute.database,
            Some(false),
            "user-set field must be kept"
        );
        assert_eq!(
            on_lake_compute.schema,
            Some(false),
            "unset field takes lake compute's default"
        );
    }

    /// Applied once per package and then again per node, so it has to be a
    /// no-op the second time.
    #[test]
    fn resolution_is_idempotent() {
        let once = resolve_package_quoting(None, AdapterType::LakeCompute);
        let twice = resolve_package_quoting(Some(once), AdapterType::LakeCompute);
        assert_eq!(once, twice);
    }
}
