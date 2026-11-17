//! Lake Formation tags and data cell filters: dbt-athena's `LfTagsManager` and
//! `LfPermissions` (`lakeformation.py`), through the driver's `lakeformation.*`
//! operations.

use super::driver_ops::{AthenaOps, RelationParts};
use crate::errors::{AdapterError, AdapterErrorKind, AdapterResult};
use serde::Deserialize;
use serde_json::{Value as Json, json};
use std::collections::{BTreeMap, BTreeSet};

/// `LfTagsConfig`: the `lf_tags_config` model config.
#[derive(Debug, Default, Deserialize)]
pub struct LfTagsConfig {
    #[serde(default)]
    pub enabled: bool,
    pub tags: Option<BTreeMap<String, String>>,
    /// tag key -> tag value -> columns
    pub tags_columns: Option<BTreeMap<String, BTreeMap<String, Vec<String>>>>,
    /// Tag keys the table inherits from its database, left alone.
    pub inherited_tags: Option<Vec<String>>,
}

/// `LfGrantsConfig`: the `lf_grants` model config.
#[derive(Debug, Deserialize)]
pub struct LfGrantsConfig {
    pub data_cell_filters: DataCellFiltersConfig,
}

#[derive(Debug, Deserialize)]
pub struct DataCellFiltersConfig {
    #[serde(default)]
    pub enabled: bool,
    pub filters: BTreeMap<String, FilterConfig>,
}

#[derive(Debug, Deserialize)]
pub struct FilterConfig {
    pub row_filter: String,
    #[serde(default)]
    pub column_names: Vec<String>,
    #[serde(default)]
    pub principals: Vec<String>,
}

fn lf_error(msg: impl Into<String>) -> AdapterError {
    AdapterError::new(AdapterErrorKind::UnexpectedResult, msg)
}

fn tag_pairs<'t>(tags: impl IntoIterator<Item = (&'t String, &'t String)>) -> Json {
    Json::Array(
        tags.into_iter()
            .map(|(key, value)| json!({ "TagKey": key, "TagValues": [value] }))
            .collect(),
    )
}

/// `LfTagsManager._column_tags_to_remove`: the non-inherited tags on the columns,
/// as tag key -> tag value -> columns.
fn column_tags_to_remove(
    columns: &[Json],
    inherited: &BTreeSet<String>,
) -> BTreeMap<String, BTreeMap<String, Vec<String>>> {
    let mut to_remove: BTreeMap<String, BTreeMap<String, Vec<String>>> = BTreeMap::new();
    for column in columns {
        let name = column["Name"].as_str().unwrap_or_default();
        for tag in column["LFTags"].as_array().into_iter().flatten() {
            let key = tag["TagKey"].as_str().unwrap_or_default();
            if inherited.contains(key) {
                continue;
            }
            let value = tag["TagValues"][0].as_str().unwrap_or_default();
            to_remove
                .entry(key.to_string())
                .or_default()
                .entry(value.to_string())
                .or_default()
                .push(name.to_string());
        }
    }
    to_remove
}

/// `LfTagsManager._table_tags_to_remove`: the table tags neither configured nor
/// inherited, with all their values.
fn table_tags_to_remove(
    table_tags: &[Json],
    configured: &BTreeMap<String, String>,
    inherited: &BTreeSet<String>,
) -> Vec<Json> {
    table_tags
        .iter()
        .filter(|tag| {
            let key = tag["TagKey"].as_str().unwrap_or_default();
            !configured.contains_key(key) && !inherited.contains(key)
        })
        .map(|tag| json!({ "TagKey": tag["TagKey"], "TagValues": tag["TagValues"] }))
        .collect()
}

/// `FilterConfig.to_api_repr`
fn filter_table_data(
    catalog_id: &str,
    relation: &RelationParts,
    name: &str,
    filter: &FilterConfig,
) -> Json {
    json!({
        "TableCatalogId": catalog_id,
        "DatabaseName": relation.schema,
        "TableName": relation.identifier,
        "Name": name,
        "RowFilter": { "FilterExpression": filter.row_filter },
        "ColumnNames": filter.column_names,
        "ColumnWildcard": { "ExcludedColumnNames": [] },
    })
}

/// `FilterConfig.to_update`: the row filter or the column set changed.
fn filter_changed(filter: &FilterConfig, existing: &Json) -> bool {
    let existing_columns: BTreeSet<&str> = existing["ColumnNames"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Json::as_str)
        .collect();
    let columns: BTreeSet<&str> = filter.column_names.iter().map(String::as_str).collect();
    existing["RowFilter"]["FilterExpression"].as_str() != Some(filter.row_filter.as_str())
        || existing_columns != columns
}

impl AthenaOps<'_, '_, '_> {
    /// Add, remove or grant, and fail on any per-tag or per-entry failure the call reports,
    /// as `_parse_and_log_lf_response` does for the tag calls.
    fn lf_call(&self, operation: &str, payload: Json, what: &str) -> AdapterResult<Json> {
        let response = self.call(operation, payload)?;
        let failures = response["Failures"].as_array().cloned().unwrap_or_default();
        if failures.is_empty() {
            return Ok(response);
        }
        let details = failures
            .iter()
            .map(|f| {
                let tag = f["LFTag"]["TagKey"].as_str().unwrap_or_default();
                let error = f["Error"]["ErrorMessage"].as_str().unwrap_or_default();
                format!("{tag} {error}").trim().to_string()
            })
            .collect::<Vec<_>>()
            .join("; ");
        Err(lf_error(format!("Failed to {what}: {details}")))
    }

    /// `LfTagsManager.process_lf_tags_database`
    pub fn add_lf_tags_to_database(
        &self,
        schema: &str,
        tags: &BTreeMap<String, String>,
    ) -> AdapterResult<()> {
        if tags.is_empty() {
            return Ok(());
        }
        self.lf_call(
            "lakeformation.add_lf_tags_to_resource",
            json!({ "Resource": { "Database": { "Name": schema } }, "LFTags": tag_pairs(tags) }),
            &format!("add LF tags to {schema}"),
        )?;
        Ok(())
    }

    /// `LfTagsManager.process_lf_tags`: drop the column tags and the table tags the
    /// config no longer names (inherited ones stay), then apply the configured ones.
    pub fn add_lf_tags(
        &self,
        relation: &RelationParts,
        config: &LfTagsConfig,
    ) -> AdapterResult<()> {
        let (database, table) = (&relation.schema, &relation.identifier);
        let resource = json!({ "Table": { "DatabaseName": database, "Name": table } });
        let columns_resource = |columns: &[String]| {
            json!({ "TableWithColumns": {
                "DatabaseName": database, "Name": table, "ColumnNames": columns,
            } })
        };
        let inherited: BTreeSet<String> = config.inherited_tags.iter().flatten().cloned().collect();
        let configured = config.tags.clone().unwrap_or_default();
        let existing = self.call(
            "lakeformation.get_resource_lf_tags",
            json!({ "Resource": resource }),
        )?;

        let on_columns = existing["LFTagsOnColumns"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        for (key, values) in column_tags_to_remove(&on_columns, &inherited) {
            for (value, columns) in values {
                self.lf_call(
                    "lakeformation.remove_lf_tags_from_resource",
                    json!({
                        "Resource": columns_resource(&columns),
                        "LFTags": [{ "TagKey": key, "TagValues": [value] }],
                    }),
                    &format!(
                        "remove LF tag {key}={value} from {database}.{table} columns {columns:?}"
                    ),
                )?;
            }
        }

        let on_table = existing["LFTagsOnTable"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let stale = table_tags_to_remove(&on_table, &configured, &inherited);
        if !stale.is_empty() {
            self.lf_call(
                "lakeformation.remove_lf_tags_from_resource",
                json!({ "Resource": resource, "LFTags": stale }),
                &format!("remove LF tags from {database}.{table}"),
            )?;
        }
        if !configured.is_empty() {
            self.lf_call(
                "lakeformation.add_lf_tags_to_resource",
                json!({ "Resource": resource, "LFTags": tag_pairs(&configured) }),
                &format!("add LF tags to {database}.{table}"),
            )?;
        }

        for (key, values) in config.tags_columns.iter().flatten() {
            for (value, columns) in values {
                self.lf_call(
                    "lakeformation.add_lf_tags_to_resource",
                    json!({
                        "Resource": columns_resource(columns),
                        "LFTags": [{ "TagKey": key, "TagValues": [value] }],
                    }),
                    &format!("add LF tag {key}={value} to {database}.{table} columns {columns:?}"),
                )?;
            }
        }
        Ok(())
    }

    /// `LfPermissions.process_filters` and `process_permissions`: the table's data
    /// cell filters and their SELECT grantees become exactly the configured ones.
    pub fn apply_lf_grants(
        &self,
        relation: &RelationParts,
        config: &LfGrantsConfig,
    ) -> AdapterResult<()> {
        let catalog_id = self
            .catalog_id(relation.database.as_deref())?
            .ok_or_else(|| {
                lf_error(format!(
                    "lf_grants: no Glue catalog id for database {:?}",
                    relation.database
                ))
            })?;
        let (database, table) = (&relation.schema, &relation.identifier);
        let filters = &config.data_cell_filters.filters;

        let listed = self.call(
            "lakeformation.list_data_cells_filter",
            json!({ "Table": { "CatalogId": catalog_id, "DatabaseName": database, "Name": table } }),
        )?;
        let current: BTreeMap<String, Json> = listed["DataCellsFilters"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|f| Some((f["Name"].as_str()?.to_string(), f.clone())))
            .collect();

        for (name, existing) in &current {
            if !filters.contains_key(name) {
                self.call(
                    "lakeformation.delete_data_cells_filter",
                    json!({
                        "TableCatalogId": existing["TableCatalogId"],
                        "DatabaseName": existing["DatabaseName"],
                        "TableName": existing["TableName"],
                        "Name": name,
                    }),
                )?;
            }
        }
        for (name, filter) in filters {
            let table_data = filter_table_data(&catalog_id, relation, name, filter);
            match current.get(name) {
                None => {
                    self.call(
                        "lakeformation.create_data_cells_filter",
                        json!({ "TableData": table_data }),
                    )?;
                }
                Some(existing) if filter_changed(filter, existing) => {
                    self.call(
                        "lakeformation.update_data_cells_filter",
                        json!({ "TableData": table_data }),
                    )?;
                }
                Some(_) => {}
            }
        }

        for (name, filter) in filters {
            let resource = json!({ "DataCellsFilter": {
                "TableCatalogId": catalog_id, "DatabaseName": database,
                "TableName": table, "Name": name,
            } });
            let permissions = self.call(
                "lakeformation.list_permissions",
                json!({ "Resource": resource }),
            )?;
            let current: BTreeSet<String> = permissions["PrincipalResourcePermissions"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|p| p["Principal"]["DataLakePrincipalIdentifier"].as_str())
                .map(str::to_string)
                .collect();
            let wanted: BTreeSet<String> = filter.principals.iter().cloned().collect();
            let entries = |principals: Vec<&String>| {
                Json::Array(
                    principals
                        .into_iter()
                        .enumerate()
                        .map(|(id, principal)| {
                            json!({
                                "Id": id.to_string(),
                                "Principal": { "DataLakePrincipalIdentifier": principal },
                                "Resource": resource,
                                "Permissions": ["SELECT"],
                                "PermissionsWithGrantOption": [],
                            })
                        })
                        .collect(),
                )
            };
            let to_revoke: Vec<&String> = current.difference(&wanted).collect();
            if !to_revoke.is_empty() {
                self.lf_call(
                    "lakeformation.batch_revoke_permissions",
                    json!({ "CatalogId": catalog_id, "Entries": entries(to_revoke) }),
                    &format!("revoke SELECT on data cell filter {name}"),
                )?;
            }
            let to_grant: Vec<&String> = wanted.difference(&current).collect();
            if !to_grant.is_empty() {
                self.lf_call(
                    "lakeformation.batch_grant_permissions",
                    json!({ "CatalogId": catalog_id, "Entries": entries(to_grant) }),
                    &format!("grant SELECT on data cell filter {name}"),
                )?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_tags_to_remove_groups_by_tag_and_skips_inherited() {
        let columns = vec![
            json!({ "Name": "email", "LFTags": [
                { "TagKey": "pii", "TagValues": ["true"] },
                { "TagKey": "domain", "TagValues": ["sales"] },
            ] }),
            json!({ "Name": "phone", "LFTags": [{ "TagKey": "pii", "TagValues": ["true"] }] }),
        ];
        let inherited = BTreeSet::from(["domain".to_string()]);
        let to_remove = column_tags_to_remove(&columns, &inherited);
        assert_eq!(
            to_remove,
            BTreeMap::from([(
                "pii".to_string(),
                BTreeMap::from([(
                    "true".to_string(),
                    vec!["email".to_string(), "phone".to_string()]
                )])
            )])
        );
    }

    #[test]
    fn table_tags_to_remove_keeps_configured_and_inherited() {
        let table = vec![
            json!({ "TagKey": "tier", "TagValues": ["gold"] }),
            json!({ "TagKey": "domain", "TagValues": ["sales"] }),
            json!({ "TagKey": "old", "TagValues": ["x", "y"] }),
        ];
        let configured = BTreeMap::from([("tier".to_string(), "lab".to_string())]);
        let inherited = BTreeSet::from(["domain".to_string()]);
        assert_eq!(
            table_tags_to_remove(&table, &configured, &inherited),
            vec![json!({ "TagKey": "old", "TagValues": ["x", "y"] })]
        );
    }

    #[test]
    fn filter_changed_compares_row_filter_and_column_set() {
        let filter = FilterConfig {
            row_filter: "country = 'IT'".to_string(),
            column_names: vec!["b".to_string(), "a".to_string()],
            principals: vec![],
        };
        let same = json!({ "RowFilter": { "FilterExpression": "country = 'IT'" }, "ColumnNames": ["a", "b"] });
        let other_row =
            json!({ "RowFilter": { "FilterExpression": "true" }, "ColumnNames": ["a", "b"] });
        let other_columns =
            json!({ "RowFilter": { "FilterExpression": "country = 'IT'" }, "ColumnNames": ["a"] });
        assert!(!filter_changed(&filter, &same));
        assert!(filter_changed(&filter, &other_row));
        assert!(filter_changed(&filter, &other_columns));
    }

    #[test]
    fn configs_parse_the_dbt_athena_shapes() {
        let tags: LfTagsConfig = serde_json::from_value(json!({
            "enabled": true,
            "tags": { "tier": "lab" },
            "tags_columns": { "pii": { "true": ["email"] } },
            "inherited_tags": ["domain"],
        }))
        .unwrap();
        assert!(tags.enabled);
        assert_eq!(tags.tags_columns.unwrap()["pii"]["true"], vec!["email"]);

        let grants: LfGrantsConfig = serde_json::from_value(json!({
            "data_cell_filters": { "enabled": true, "filters": {
                "it_only": { "row_filter": "country = 'IT'", "principals": ["arn:aws:iam::1:role/r"] },
            } },
        }))
        .unwrap();
        let filter = &grants.data_cell_filters.filters["it_only"];
        assert!(filter.column_names.is_empty());
        assert_eq!(filter.principals, vec!["arn:aws:iam::1:role/r"]);
    }
}
