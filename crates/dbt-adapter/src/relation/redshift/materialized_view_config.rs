use crate::relation::{ComponentConfig, RelationChangeSet};

use dbt_agate::AgateTable;
use dbt_schemas::schemas::DbtModel;
use minijinja::Value;
use minijinja::value::Object;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::convert::TryFrom;
use std::fmt::{Display, Formatter};
use std::result::Result;
use std::sync::Arc;

use dbt_schemas::schemas::serde::StringOrArrayOfStrings;

/// Deserialization target for macro redshift__describe_materialized_view
// https://github.com/dbt-labs/dbt-adapters/blob/f492c919d3bd415bf5065b3cd8cd1af23562feb0/dbt-redshift/src/dbt/include/redshift/macros/relations/materialized_view/describe.sql#L1
#[derive(Debug, Clone)]
pub struct DescribeMaterializedViewResults {
    pub materialized_view: Arc<AgateTable>,
    pub columns: Arc<AgateTable>,
    pub query: Arc<AgateTable>,
}

impl TryFrom<&Value> for DescribeMaterializedViewResults {
    type Error = String;

    fn try_from(value: &Value) -> Result<Self, Self::Error> {
        let materialized_view = value
            .get_item(&Value::from_safe_string("materialized_view".into()))
            .map_err(|e| format!("Expected key `materialized_view`: {e}"))?
            .downcast_object::<AgateTable>()
            .ok_or("Failed to convert materialized_view to AgateTable")?;

        let columns = value
            .get_item(&Value::from_safe_string("columns".into()))
            .map_err(|e| format!("Expected key `columns`: {e}"))?
            .downcast_object::<AgateTable>()
            .ok_or("Failed to convert columns to AgateTable")?;

        let query = value
            .get_item(&Value::from_safe_string("query".into()))
            .map_err(|e| format!("Expected key `query`: {e}"))?
            .downcast_object::<AgateTable>()
            .ok_or("Failed to convert query to AgateTable")?;

        Ok(Self {
            materialized_view,
            columns,
            query,
        })
    }
}

/// Redshift distribution styles for materialized views
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum RedshiftDistStyle {
    #[default]
    Even,
    All,
    Auto,
    /// Column to use for distribution
    Key(String),
}

impl Display for RedshiftDistStyle {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Even => write!(f, "even"),
            Self::All => write!(f, "all"),
            Self::Auto => write!(f, "auto"),
            Self::Key(_) => write!(f, "key"),
        }
    }
}

impl From<&str> for RedshiftDistStyle {
    fn from(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "even" => Self::Even,
            "all" => Self::All,
            "auto" => Self::Auto,
            _ => Self::Key(s.to_string()),
        }
    }
}

impl From<String> for RedshiftDistStyle {
    fn from(s: String) -> Self {
        Self::from(s.as_str())
    }
}

// https://github.com/dbt-labs/dbt-adapters/blob/f492c919d3bd415bf5065b3cd8cd1af23562feb0/dbt-redshift/src/dbt/adapters/redshift/relation_configs/dist.py#L33
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RedshiftDistConfig {
    /// The type of data distribution style to use on the table/materialized view.
    pub diststyle: RedshiftDistStyle,
}

impl TryFrom<&DbtModel> for RedshiftDistConfig {
    type Error = String;
    // https://github.com/dbt-labs/dbt-adapters/blob/f492c919d3bd415bf5065b3cd8cd1af23562feb0/dbt-redshift/src/dbt/adapters/redshift/relation_configs/dist.py#L80
    fn try_from(model: &DbtModel) -> Result<Self, Self::Error> {
        let dist = model
            .__adapter_attr__
            .redshift_attr
            .as_ref()
            .and_then(|attr| attr.dist.as_ref());

        match dist {
            Some(StringOrArrayOfStrings::String(dist)) => Ok(RedshiftDistConfig {
                diststyle: dist.as_str().into(),
            }),
            // No `dist` set: materialized views default to EVEN (see `Default` below).
            None => Ok(Self::default()),
            // A list has no diststyle. This error is load-bearing: the MV create macro reads
            // `dist.diststyle` directly and never runs the `dist()` guard that covers tables.
            Some(StringOrArrayOfStrings::ArrayOfStrings(_)) => Err(
                "The 'dist' config must be a single value (e.g. dist: primary_key), not a list \
                 or other type. Redshift distribution key accepts only one column or one of: \
                 all, even, auto."
                    .to_string(),
            ),
        }
    }
}

impl Default for RedshiftDistConfig {
    // Materialized views default to "even", not "auto"
    fn default() -> Self {
        Self {
            diststyle: RedshiftDistStyle::Even,
        }
    }
}

impl RedshiftDistConfig {
    pub fn requires_full_refresh(&self) -> bool {
        true
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum RedshiftSortStyle {
    #[default]
    Auto,
    Compound(Vec<String>),
    Interleaved(Vec<String>),
}

impl RedshiftSortStyle {
    fn default_with_columns(cols: Vec<String>) -> Self {
        Self::Compound(cols)
    }
}

impl TryFrom<(Option<&str>, Option<Vec<String>>)> for RedshiftSortStyle {
    type Error = String;

    fn try_from(value: (Option<&str>, Option<Vec<String>>)) -> Result<Self, Self::Error> {
        let (sort_type, sort_key) = value;
        if let Some(sort_type) = sort_type {
            match sort_type.to_ascii_lowercase().as_str() {
                "auto" => {
                    if sort_key.is_some() {
                        Err("sortkey cannot be given with a sortstyle of `auto`".to_string())
                    } else {
                        Ok(Self::Auto)
                    }
                }
                "compound" => {
                    if let Some(sort_key) = sort_key {
                        Ok(Self::Compound(sort_key))
                    } else {
                        Err("sortkey cannot be empty with a sortstyle of `compound`".to_string())
                    }
                }
                "interleaved" => {
                    if let Some(sort_key) = sort_key {
                        Ok(Self::Interleaved(sort_key))
                    } else {
                        Err("sortkey cannot be empty with a sortstyle of `interleaved`".to_string())
                    }
                }
                _ => Err(format!("unsupport sorttype:{sort_type}")),
            }
        } else if let Some(sort_key) = sort_key {
            Ok(Self::default_with_columns(sort_key))
        } else {
            Ok(Self::default())
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default, Eq)]
pub struct RedshiftSortConfig {
    pub sortstyle: RedshiftSortStyle,
}

impl TryFrom<&DbtModel> for RedshiftSortConfig {
    type Error = String;

    fn try_from(model: &DbtModel) -> Result<Self, Self::Error> {
        let redshift_config = &model
            .__adapter_attr__
            .redshift_attr
            .as_ref()
            .ok_or_else(|| "Redshift attributes not found".to_string())?;

        let sort = redshift_config.sort.as_ref().map(|s| match s {
            StringOrArrayOfStrings::String(single) => vec![single.clone()],
            StringOrArrayOfStrings::ArrayOfStrings(multiple) => multiple.clone(),
        });

        let sort_type = redshift_config.sort_type.as_deref();
        let maybe_sort_style = RedshiftSortStyle::try_from((sort_type, sort));
        match maybe_sort_style {
            Ok(sortstyle) => Ok(Self { sortstyle }),
            Err(e) => Err(format!("Failed to initialize RedshiftSortConfig: {e}")),
        }
    }
}

impl RedshiftSortConfig {
    pub fn requires_full_refresh(&self) -> bool {
        true
    }
}

/// This config follows the specs found here:
/// https://docs.aws.amazon.com/redshift/latest/dg/materialized-view-create-sql-command.html
/// Reference:
/// https://github.com/dbt-labs/dbt-adapters/blob/2a94cc75dba1f98fa5caff1f396f5af7ee444598/dbt-redshift/src/dbt/adapters/redshift/relation_configs/materialized_view.py#L32
// XXX: Deviation from core: raw SQL is not used here - that is already available within the macro as {{ sql }}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedshiftMaterializedViewConfig {
    /// Name of the materialized view.
    pub mv_name: String,
    /// Name of the schema containing the materialized view.
    pub schema_name: String,
    /// Name of the database containing the materialized view.
    pub database_name: String,
    /// Determines if the materialized view is included in automated and manual cluster snapshots.
    ///
    /// Note: We cannot currently query this from Redshift, which creates two issues:
    ///   - A model deployed with this set to `false` will rebuild every run because the database version will always look like `true`.
    ///   - To deploy this as a change from `false` to `true`, a full refresh must be issued since the database version will always look like `true` (unless there is another full refresh-triggering change).
    #[serde(default = "RedshiftMaterializedViewConfig::default_backup")]
    pub backup: bool,
    /// The distribution configuration for the data behind the materialized view, a combination of a `diststyle` and an optional `distkey`.
    ///
    /// Note: The default `diststyle` for materialized views is EVEN, despite the default in general being AUTO.
    #[serde(default)]
    pub dist: RedshiftDistConfig,
    /// The sort configuration for the data behind the materialized view, a combination of a `sortstyle` and an optional `sortkey`.
    #[serde(default)]
    pub sort: RedshiftSortConfig,
    /// Specifies whether the materialized view should be automatically refreshed with latest changes from its base tables.
    #[serde(default = "RedshiftMaterializedViewConfig::default_autorefresh")]
    pub autorefresh: bool, // xxx: configured as auto_refresh, used as autorefresh
}

impl TryFrom<&DbtModel> for RedshiftMaterializedViewConfig {
    type Error = String;

    fn try_from(model: &DbtModel) -> Result<Self, Self::Error> {
        let database_name = model.__base_attr__.database.clone();
        let schema_name = model.__base_attr__.schema.clone();
        let mv_name = model.__common_attr__.name.clone();

        let backup = model
            .__adapter_attr__
            .redshift_attr
            .as_ref()
            .and_then(|attr| attr.backup)
            .unwrap_or(true);

        let auto_refresh = model
            .__adapter_attr__
            .redshift_attr
            .as_ref()
            .and_then(|attr| attr.auto_refresh)
            .unwrap_or(false);

        let dist = RedshiftDistConfig::try_from(model)?;
        let sort = RedshiftSortConfig::try_from(model)?;

        Ok(Self {
            mv_name,
            schema_name,
            database_name,
            backup,
            autorefresh: auto_refresh,
            dist,
            sort,
        })
    }
}

fn get_string_by_name_from_agate_row(row: &Value, col_name: &str) -> Option<String> {
    if let Ok(cell_value) = row.get_attr(col_name) {
        cell_value.as_str().map(|s| s.to_string())
    } else {
        None
    }
}

// https://github.com/dbt-labs/dbt-adapters/blob/f492c919d3bd415bf5065b3cd8cd1af23562feb0/dbt-redshift/src/dbt/adapters/redshift/relation_configs/materialized_view.py#L155
impl TryFrom<DescribeMaterializedViewResults> for RedshiftMaterializedViewConfig {
    type Error = String;
    fn try_from(value: DescribeMaterializedViewResults) -> Result<Self, Self::Error> {
        let columns_table = value.columns;
        let _query = value
            .query
            .rows()
            .into_iter()
            .next()
            .ok_or("query table is empty")?;

        let mv = value
            .materialized_view
            .rows()
            .into_iter()
            .next()
            .ok_or("materialized_view table is empty")?;

        let mv_name = get_string_by_name_from_agate_row(&mv, "table")
            .ok_or("Failed to get table name from materialized_view")?;

        let schema_name = get_string_by_name_from_agate_row(&mv, "schema")
            .ok_or("Failed to get schema name from materialized_view")?;

        let database_name = get_string_by_name_from_agate_row(&mv, "database")
            .ok_or("Failed to get database name from materialized_view")?;

        let autorefresh =
            if let Some(autorefresh_str) = get_string_by_name_from_agate_row(&mv, "autorefresh") {
                match autorefresh_str.as_str() {
                    "t" => true,
                    "f" => false,
                    _ => autorefresh_str.parse::<bool>().unwrap_or(false),
                }
            } else {
                false // default
            };

        // https://github.com/dbt-labs/dbt-adapters/blob/f492c919d3bd415bf5065b3cd8cd1af23562feb0/dbt-redshift/src/dbt/adapters/redshift/relation_configs/dist.py#L113
        let dist = if let Some(diststyle_str) = get_string_by_name_from_agate_row(&mv, "diststyle")
        {
            // Split on "(" to get the base style (handles cases like "KEY(column1)" -> "KEY")
            let base_style = diststyle_str.split('(').next().unwrap_or("").to_lowercase();

            if diststyle_str.is_empty() {
                RedshiftDistConfig::default()
            } else if base_style == "key" {
                // Extract the column name from KEY(column1) format
                let distkey = if diststyle_str.starts_with("KEY(") && diststyle_str.ends_with(")") {
                    let open_paren = "KEY(".len();
                    let close_paren = diststyle_str.len() - ")".len();
                    diststyle_str[open_paren..close_paren].to_string()
                } else {
                    String::new()
                };

                RedshiftDistConfig {
                    diststyle: RedshiftDistStyle::Key(distkey),
                }
            } else {
                let parsed_dist: RedshiftDistStyle = base_style.as_str().into();
                RedshiftDistConfig {
                    diststyle: parsed_dist,
                }
            }
        } else {
            RedshiftDistConfig::default()
        };

        let sort = if columns_table.num_rows() > 0 {
            let mut sort_columns = Vec::new();

            // sort_columns = [row for row in columns.rows if row.get("sort_key_position", 0) > 0]
            for row in columns_table.rows().into_iter() {
                if let Ok(sort_pos_value) = row.get_attr("sort_key_position")
                    && let Some(sort_pos) = sort_pos_value.as_i64()
                    && sort_pos > 0
                    && let Ok(col_name_value) = row.get_attr("column_name")
                    && let Some(col_name) = col_name_value.as_str()
                {
                    sort_columns.push((sort_pos as usize, col_name.to_string()));
                }
            }

            // https://github.com/dbt-labs/dbt-adapters/blob/f492c919d3bd415bf5065b3cd8cd1af23562feb0/dbt-redshift/src/dbt/adapters/redshift/relation_configs/sort.py#L141
            if !sort_columns.is_empty() {
                sort_columns.sort_by_key(|(pos, _)| *pos);
                let sort_keys: Vec<String> =
                    sort_columns.into_iter().map(|(_, name)| name).collect();

                let sort_type = get_string_by_name_from_agate_row(&mv, "sorttype");
                let sortstyle =
                    RedshiftSortStyle::try_from((sort_type.as_deref(), Some(sort_keys)))
                        .map_err(|e| format!("Failed to parse sort configuration: {e}"))?;

                RedshiftSortConfig { sortstyle }
            } else {
                RedshiftSortConfig::default()
            }
        } else {
            RedshiftSortConfig::default()
        };

        // this can't be queried from Redshift
        let backup = Self::default_backup();

        Ok(Self {
            mv_name,
            schema_name,
            database_name,
            backup,
            dist,
            sort,
            autorefresh,
        })
    }
}

impl RedshiftMaterializedViewConfig {
    fn default_backup() -> bool {
        true
    }

    fn default_autorefresh() -> bool {
        false
    }

    /// Get the full path: `"database"."schema"."mv_name"`.
    ///
    /// Mirrors upstream Python `RedshiftMaterializedViewConfig.path`, which builds the
    /// path from components always quoted rendered by `_render_part` under `RedshiftQuotePolicy`
    /// (database/schema/identifier all default to True).
    /// https://github.com/dbt-labs/dbt-adapters/blob/fc39590f62cfe2c8b78ec96594a5b91ed334aebd/dbt-redshift/src/dbt/adapters/redshift/relation_configs/materialized_view.py#L100-L101
    pub fn path(&self) -> String {
        format!(
            "\"{}\".\"{}\".\"{}\"",
            self.database_name, self.schema_name, self.mv_name
        )
    }
}

#[derive(Debug)]
// https://github.com/dbt-labs/dbt-adapters/blob/f492c919d3bd415bf5065b3cd8cd1af23562feb0/dbt-redshift/src/dbt/adapters/redshift/relation_configs/materialized_view.py#L250
pub struct RedshiftMaterializedViewConfigChangeset {
    dist: Option<RedshiftDistConfig>,
    sort: Option<RedshiftSortConfig>,
    // XXX: intentional deviation for now - RedshiftAutoRefreshConfigChange didn't provide much value
    autorefresh: Option<bool>,
}

impl RedshiftMaterializedViewConfigChangeset {
    pub fn new(old: RedshiftMaterializedViewConfig, new: RedshiftMaterializedViewConfig) -> Self {
        let autorefresh = if old.autorefresh != new.autorefresh {
            Some(new.autorefresh)
        } else {
            None
        };

        let dist = if old.dist != new.dist {
            Some(new.dist)
        } else {
            None
        };

        let sort = if old.sort != new.sort {
            Some(new.sort)
        } else {
            None
        };

        RedshiftMaterializedViewConfigChangeset {
            dist,
            sort,
            autorefresh,
        }
    }
}

impl RelationChangeSet for RedshiftMaterializedViewConfigChangeset {
    // todo: revisit this abstraction
    fn changes(&self) -> &BTreeMap<String, Arc<dyn ComponentConfig>> {
        unimplemented!("Not available for Redshift")
    }

    fn requires_full_refresh(&self) -> bool {
        let autorefresh_requires_refresh = self.autorefresh.is_some_and(|_| true);

        let dist_requires_refresh = self
            .dist
            .as_ref()
            .is_some_and(|dist_config| dist_config.requires_full_refresh());

        let sort_requires_refresh = self
            .sort
            .as_ref()
            .is_some_and(|sort_config| sort_config.requires_full_refresh());

        autorefresh_requires_refresh || dist_requires_refresh || sort_requires_refresh
    }

    // todo: revisit this abstraction
    fn get_change(&self, _component_name: &str) -> Option<&dyn ComponentConfig> {
        unimplemented!("Not available for Redshift")
    }

    fn has_changes(&self) -> bool {
        self.dist.is_some() || self.sort.is_some() || self.autorefresh.is_some()
    }
}

impl Object for RedshiftMaterializedViewConfigChangeset {
    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        match key.as_str() {
            Some("autorefresh") => self.autorefresh.map(Value::from),
            Some("dist") => self
                .dist
                .as_ref()
                .map(|dist_config| Value::from_object(dist_config.clone())),
            Some("sort") => self
                .sort
                .as_ref()
                .map(|sort_config| Value::from_object(sort_config.clone())),
            _ => None,
        }
    }
}

impl Object for RedshiftMaterializedViewConfig {
    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        match key.as_str() {
            Some("path") => Some(Value::from(self.path())),
            Some("backup") => Some(Value::from(self.backup)),
            Some("dist") => Some(Value::from_object(self.dist.clone())),
            Some("sort") => Some(Value::from_object(self.sort.clone())),
            Some("autorefresh") => Some(Value::from(self.autorefresh)),
            Some("mv_name") => Some(Value::from(self.mv_name.clone())),
            Some("schema_name") => Some(Value::from(self.schema_name.clone())),
            Some("database_name") => Some(Value::from(self.database_name.clone())),
            _ => None,
        }
    }
}

impl Object for RedshiftDistConfig {
    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        match key.as_str() {
            Some("diststyle") => Some(Value::from(self.diststyle.to_string())),
            Some("distkey") => match &self.diststyle {
                RedshiftDistStyle::Key(key) => Some(Value::from(key.clone())),
                _ => None,
            },
            _ => None,
        }
    }
}

impl Object for RedshiftSortConfig {
    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        match key.as_str() {
            Some("sortkey") => match &self.sortstyle {
                RedshiftSortStyle::Compound(keys) | RedshiftSortStyle::Interleaved(keys) => {
                    Some(Value::from(keys.clone()))
                }
                RedshiftSortStyle::Auto => None,
            },
            Some("sortstyle") => Some(Value::from(match &self.sortstyle {
                RedshiftSortStyle::Auto => "auto",
                RedshiftSortStyle::Compound(_) => "compound",
                RedshiftSortStyle::Interleaved(_) => "interleaved",
            })),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{RedshiftDistStyle, RedshiftMaterializedViewConfig};
    use dbt_schemas::schemas::nodes::RedshiftAttr;
    use dbt_schemas::schemas::serde::StringOrArrayOfStrings;
    use dbt_schemas::schemas::{AdapterAttr, DbtModel};

    fn model_with_dist(dist: Option<StringOrArrayOfStrings>) -> DbtModel {
        DbtModel {
            __adapter_attr__: AdapterAttr::default().with_redshift_attr(Some(Box::new(
                RedshiftAttr {
                    dist,
                    ..Default::default()
                },
            ))),
            ..Default::default()
        }
    }

    /// A list-valued `dist` has no diststyle and must fail the build; the absent and scalar
    /// cases guard the `unwrap_or_default()` -> `?` change on this path.
    #[test]
    fn redshift_materialized_view_config_resolves_dist_shapes() {
        let absent = RedshiftMaterializedViewConfig::try_from(&model_with_dist(None))
            .expect("a materialized view without `dist` must still build");
        assert_eq!(absent.dist.diststyle, RedshiftDistStyle::Even);

        let scalar = RedshiftMaterializedViewConfig::try_from(&model_with_dist(Some(
            StringOrArrayOfStrings::String("id".to_string()),
        )))
        .expect("a scalar `dist` must resolve");
        assert_eq!(
            scalar.dist.diststyle,
            RedshiftDistStyle::Key("id".to_string())
        );

        let list = RedshiftMaterializedViewConfig::try_from(&model_with_dist(Some(
            StringOrArrayOfStrings::ArrayOfStrings(vec!["id".to_string()]),
        )))
        .expect_err("a list-valued `dist` has no diststyle");
        assert!(
            list.contains("The 'dist' config must be a single value"),
            "unexpected error: {list}"
        );
    }
}
