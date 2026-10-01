//! Parsed representations of dbt node payload types stored in `parse/nodes` parquet.
//!
//! Both `metadata_to_duckdb` (SQL upsert path) and `metadata_to_parquet` (Arrow write
//! path) extract the same fields from the same JSON payload keys. This module owns that
//! extraction once; each consumer converts the resulting struct into its own output format.

use serde_json::Value;

// ---------------------------------------------------------------------------
// Arrow helpers shared by both ingest paths
// ---------------------------------------------------------------------------

pub fn str_col(col: Option<&arrow_array::StringArray>, i: usize) -> Option<String> {
    use arrow_array::Array;
    col.filter(|c| !c.is_null(i))
        .map(|c| c.value(i).to_string())
}

pub fn list_col(col: Option<&arrow_array::ListArray>, i: usize) -> Vec<String> {
    use arrow_array::Array;
    let Some(list) = col else { return vec![] };
    if list.is_null(i) {
        return vec![];
    }
    let offsets = list.value_offsets();
    let start = offsets[i] as usize;
    let end = offsets[i + 1] as usize;
    let values = list.values();
    let str_arr = values.as_any().downcast_ref::<arrow_array::StringArray>();
    let Some(s) = str_arr else { return vec![] };
    (start..end)
        .filter_map(|j| {
            if s.is_null(j) {
                None
            } else {
                Some(s.value(j).to_string())
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Shared JSON helpers
// ---------------------------------------------------------------------------

fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(|v| v.as_str()).map(|s| s.to_string())
}

fn str_field_default<'a>(v: &'a Value, key: &str, default: &'a str) -> String {
    v.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or(default)
        .to_string()
}

fn vec_of_strings(v: &Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str())
                .map(|s| s.to_string())
                .collect()
        })
        .unwrap_or_default()
}

fn depends_on_nodes(payload: &Value) -> Vec<String> {
    payload
        .get("depends_on")
        .and_then(|d| d.get("nodes"))
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str())
                .map(|s| s.to_string())
                .collect()
        })
        .unwrap_or_default()
}

fn depends_on_macros(payload: &Value) -> Vec<String> {
    payload
        .get("depends_on")
        .and_then(|d| d.get("macros"))
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str())
                .map(|s| s.to_string())
                .collect()
        })
        .unwrap_or_default()
}

fn fqn_from_base(payload: &Value) -> Vec<String> {
    payload
        .get("__base_attr__")
        .and_then(|b| b.get("fqn"))
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str())
                .map(|s| s.to_string())
                .collect()
        })
        .unwrap_or_default()
}

fn owner_fields(owner: &Value) -> (Option<String>, Option<String>) {
    let name = owner
        .get("name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let email = owner
        .get("email")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    (name, email)
}

// ---------------------------------------------------------------------------
// Parsed structs
// ---------------------------------------------------------------------------

pub struct ParsedMetric {
    pub name: String,
    pub label: Option<String>,
    pub metric_type: Option<String>,
    pub description: Option<String>,
    pub package_name: String,
    pub file_path: String,
    pub original_file_path: String,
    pub fqn: Vec<String>,
    pub type_params: Option<Value>,
    pub metric_filter: Option<Value>,
    pub time_granularity: Option<String>,
    pub input_metric_names: Vec<String>,
    pub depends_on_nodes: Vec<String>,
    pub depends_on_macros: Vec<String>,
    pub group_name: Option<String>,
    pub tags: Vec<String>,
    pub meta: Option<Value>,
    pub config: Option<Value>,
    pub created_at: Option<f64>,
}

impl ParsedMetric {
    pub fn from_payload(uid: &str, payload: &Value, tags: &[String]) -> Self {
        let _ = uid;
        let attr = payload
            .get("__metric_attr__")
            .cloned()
            .unwrap_or(Value::Null);
        let common = payload
            .get("__common_attr__")
            .cloned()
            .unwrap_or(Value::Null);

        let name = common
            .get("name")
            .or_else(|| payload.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let metric_type = attr.get("metric_type").and_then(|v| {
            v.as_str().map(|s| s.to_string()).or_else(|| {
                v.as_object()
                    .and_then(|m| m.keys().next())
                    .map(|k| k.to_string())
            })
        });
        let input_metric_names: Vec<String> = attr
            .get("type_params")
            .and_then(|tp| tp.get("input_measures").or_else(|| tp.get("input_metrics")))
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|m| {
                        m.as_str().map(|s| s.to_string()).or_else(|| {
                            m.get("name")
                                .and_then(|n| n.as_str())
                                .map(|s| s.to_string())
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        Self {
            name,
            label: str_field(&attr, "label"),
            metric_type,
            description: str_field(&common, "description"),
            package_name: str_field_default(&common, "package_name", ""),
            file_path: str_field_default(&common, "path", ""),
            original_file_path: str_field_default(&common, "original_file_path", ""),
            fqn: fqn_from_base(payload),
            type_params: attr.get("type_params").cloned(),
            metric_filter: attr.get("filter").cloned(),
            time_granularity: str_field(&attr, "time_granularity"),
            input_metric_names,
            depends_on_nodes: depends_on_nodes(payload),
            depends_on_macros: depends_on_macros(payload),
            group_name: str_field(&attr, "group"),
            tags: tags.to_vec(),
            meta: attr.get("meta").cloned(),
            config: attr.get("config").cloned(),
            created_at: attr.get("created_at").and_then(|v| v.as_f64()),
        }
    }
}

pub struct SemanticEntity {
    pub name: String,
    pub entity_type: String,
    pub description: Option<String>,
    pub label: Option<String>,
    pub entity_role: Option<String>,
    pub expr: Option<String>,
}

pub struct SemanticMeasure {
    pub name: String,
    pub agg: String,
    pub description: Option<String>,
    pub label: Option<String>,
    pub expr: Option<String>,
    pub create_metric: bool,
    pub agg_time_dimension: Option<String>,
    pub agg_params: Option<Value>,
    pub non_additive_dimension: Option<Value>,
}

pub struct SemanticDimension {
    pub name: String,
    pub dimension_type: String,
    pub description: Option<String>,
    pub label: Option<String>,
    pub expr: Option<String>,
    pub is_partition: bool,
    pub time_granularity: Option<String>,
    pub validity_params: Option<Value>,
}

pub struct ParsedSemanticModel {
    pub name: Option<String>,
    pub label: Option<String>,
    pub description: Option<String>,
    pub model_ref: String,
    pub node_relation: Option<Value>,
    pub primary_entity: Option<String>,
    pub defaults: Option<Value>,
    pub fqn: Vec<String>,
    pub depends_on_nodes: Vec<String>,
    pub depends_on_macros: Vec<String>,
    pub group_name: Option<String>,
    pub created_at: Option<f64>,
    pub entities: Vec<SemanticEntity>,
    pub measures: Vec<SemanticMeasure>,
    pub dimensions: Vec<SemanticDimension>,
}

impl ParsedSemanticModel {
    pub fn from_payload(payload: &Value) -> Self {
        let attr = payload
            .get("__semantic_model_attr__")
            .cloned()
            .unwrap_or(Value::Null);
        let common = payload
            .get("__common_attr__")
            .cloned()
            .unwrap_or(Value::Null);

        let entities = attr
            .get("entities")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|ent| {
                        let name = ent.get("name").and_then(|v| v.as_str())?.to_string();
                        let entity_type = ent
                            .get("entity_type")
                            .or_else(|| ent.get("type"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown")
                            .to_string();
                        Some(SemanticEntity {
                            name,
                            entity_type,
                            description: str_field(ent, "description"),
                            label: str_field(ent, "label"),
                            entity_role: str_field(ent, "role"),
                            expr: str_field(ent, "expr"),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        let measures = attr
            .get("measures")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|meas| {
                        let name = meas.get("name").and_then(|v| v.as_str())?.to_string();
                        Some(SemanticMeasure {
                            name,
                            agg: str_field_default(meas, "agg", "sum"),
                            description: str_field(meas, "description"),
                            label: str_field(meas, "label"),
                            expr: str_field(meas, "expr"),
                            create_metric: meas
                                .get("create_metric")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false),
                            agg_time_dimension: str_field(meas, "agg_time_dimension"),
                            agg_params: meas.get("agg_params").cloned(),
                            non_additive_dimension: meas.get("non_additive_dimension").cloned(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        let dimensions = attr
            .get("dimensions")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|dim| {
                        let name = dim.get("name").and_then(|v| v.as_str())?.to_string();
                        let dimension_type = dim
                            .get("dimension_type")
                            .or_else(|| dim.get("type"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("categorical")
                            .to_string();
                        let time_granularity = dim
                            .get("type_params")
                            .and_then(|tp| tp.get("time_granularity"))
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                        Some(SemanticDimension {
                            name,
                            dimension_type,
                            description: str_field(dim, "description"),
                            label: str_field(dim, "label"),
                            expr: str_field(dim, "expr"),
                            is_partition: dim
                                .get("is_partition")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false),
                            time_granularity,
                            validity_params: dim.get("validity_params").cloned(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        Self {
            name: common
                .get("name")
                .or_else(|| payload.get("name"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            label: str_field(&attr, "label"),
            description: str_field(&common, "description"),
            model_ref: str_field_default(&attr, "model", ""),
            node_relation: attr.get("node_relation").cloned(),
            primary_entity: str_field(&attr, "primary_entity"),
            defaults: attr.get("defaults").cloned(),
            fqn: fqn_from_base(payload),
            depends_on_nodes: depends_on_nodes(payload),
            depends_on_macros: depends_on_macros(payload),
            group_name: str_field(&attr, "group"),
            created_at: attr.get("created_at").and_then(|v| v.as_f64()),
            entities,
            measures,
            dimensions,
        }
    }
}

pub struct ParsedSavedQuery {
    pub name: String,
    pub label: Option<String>,
    pub description: Option<String>,
    pub package_name: String,
    pub file_path: String,
    pub original_file_path: String,
    pub fqn: Vec<String>,
    pub query_params: Option<Value>,
    pub exports: Option<Value>,
    pub depends_on_nodes: Vec<String>,
    pub depends_on_macros: Vec<String>,
    pub tags: Vec<String>,
    pub group_name: Option<String>,
    pub created_at: Option<f64>,
}

impl ParsedSavedQuery {
    pub fn from_payload(payload: &Value) -> Self {
        let attr = payload
            .get("__saved_query_attr__")
            .cloned()
            .unwrap_or(Value::Null);
        let common = payload
            .get("__common_attr__")
            .cloned()
            .unwrap_or(Value::Null);
        let tags: Vec<String> = payload
            .get("config")
            .and_then(|c| c.get("tags"))
            .or_else(|| payload.get("tags"))
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str())
                    .map(|s| s.to_string())
                    .collect()
            })
            .unwrap_or_default();
        Self {
            name: common
                .get("name")
                .or_else(|| payload.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            label: str_field(&attr, "label"),
            description: str_field(&common, "description"),
            package_name: str_field_default(&common, "package_name", ""),
            file_path: str_field_default(&common, "path", ""),
            original_file_path: str_field_default(&common, "original_file_path", ""),
            fqn: fqn_from_base(payload),
            query_params: attr.get("query_params").cloned(),
            exports: attr.get("exports").cloned(),
            depends_on_nodes: depends_on_nodes(payload),
            depends_on_macros: depends_on_macros(payload),
            tags,
            group_name: str_field(&attr, "group"),
            created_at: attr.get("created_at").and_then(|v| v.as_f64()),
        }
    }
}

pub struct ParsedExposure {
    pub name: String,
    pub exposure_type: String,
    pub label: Option<String>,
    pub owner_name: Option<String>,
    pub owner_email: Option<String>,
    pub url: Option<String>,
    pub maturity: Option<String>,
    pub description: Option<String>,
    pub package_name: String,
    pub file_path: String,
    pub original_file_path: String,
    pub fqn: Vec<String>,
    pub depends_on_nodes: Vec<String>,
    pub depends_on_macros: Vec<String>,
    pub tags: Vec<String>,
    pub created_at: Option<f64>,
}

impl ParsedExposure {
    pub fn from_payload(payload: &Value, tags: &[String]) -> Self {
        let attr = payload
            .get("__exposure_attr__")
            .cloned()
            .unwrap_or(Value::Null);
        let common = payload
            .get("__common_attr__")
            .cloned()
            .unwrap_or(Value::Null);
        let owner = attr.get("owner").cloned().unwrap_or(Value::Null);
        let (owner_name, owner_email) = owner_fields(&owner);
        Self {
            name: common
                .get("name")
                .or_else(|| payload.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            exposure_type: str_field_default(&attr, "type", "dashboard"),
            label: str_field(&attr, "label"),
            owner_name,
            owner_email,
            url: str_field(&attr, "url"),
            maturity: str_field(&attr, "maturity"),
            description: str_field(&common, "description"),
            package_name: str_field_default(&common, "package_name", ""),
            file_path: str_field_default(&common, "path", ""),
            original_file_path: str_field_default(&common, "original_file_path", ""),
            fqn: fqn_from_base(payload),
            depends_on_nodes: depends_on_nodes(payload),
            depends_on_macros: depends_on_macros(payload),
            tags: tags.to_vec(),
            created_at: attr.get("created_at").and_then(|v| v.as_f64()),
        }
    }
}

pub struct ParsedGroup {
    pub name: String,
    pub description: Option<String>,
    pub owner_name: Option<String>,
    pub owner_email: Option<String>,
}

impl ParsedGroup {
    pub fn from_payload(payload: &Value) -> Self {
        let attr = payload
            .get("__group_attr__")
            .cloned()
            .unwrap_or(Value::Null);
        let common = payload
            .get("__common_attr__")
            .cloned()
            .unwrap_or(Value::Null);
        let owner = attr.get("owner").cloned().unwrap_or(Value::Null);
        let (owner_name, owner_email) = owner_fields(&owner);
        Self {
            name: common
                .get("name")
                .or_else(|| payload.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            description: str_field(&common, "description"),
            owner_name,
            owner_email,
        }
    }
}

pub struct ParsedMacro {
    pub name: String,
    pub package_name: String,
    pub file_path: String,
    pub original_file_path: String,
    pub macro_sql: Option<String>,
    pub description: Option<String>,
    pub depends_on_macros: Vec<String>,
    pub supported_languages: Vec<String>,
    pub arguments: Option<Value>,
    pub docs_show: bool,
    pub patch_path: Option<String>,
    pub meta: Option<Value>,
    pub created_at: Option<f64>,
}

impl ParsedMacro {
    pub fn from_payload(payload: &Value) -> Self {
        // Macros serialize flat (no __attr__ nesting)
        Self {
            name: str_field_default(payload, "name", ""),
            package_name: str_field_default(payload, "package_name", ""),
            file_path: str_field_default(payload, "path", ""),
            original_file_path: str_field_default(payload, "original_file_path", ""),
            macro_sql: str_field(payload, "macro_sql"),
            description: str_field(payload, "description"),
            depends_on_macros: payload
                .get("depends_on")
                .and_then(|d| d.get("macros"))
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|s| s.as_str())
                        .map(|s| s.to_string())
                        .collect()
                })
                .unwrap_or_default(),
            supported_languages: vec_of_strings(payload, "supported_languages"),
            arguments: payload
                .get("arguments")
                .or_else(|| payload.get("args"))
                .cloned(),
            docs_show: payload
                .get("docs")
                .and_then(|d| d.get("show"))
                .and_then(|v| v.as_bool())
                .unwrap_or(true),
            patch_path: str_field(payload, "patch_path"),
            meta: payload.get("meta").cloned(),
            created_at: payload.get("created_at").and_then(|v| v.as_f64()),
        }
    }
}

pub struct ParsedDoc {
    pub name: String,
    pub package_name: String,
    pub file_path: String,
    pub original_file_path: String,
    pub block_contents: Option<String>,
}

impl ParsedDoc {
    pub fn from_payload(payload: &Value) -> Self {
        // Docs serialize flat (no __attr__ nesting), like macros above.
        Self {
            name: str_field_default(payload, "name", ""),
            package_name: str_field_default(payload, "package_name", ""),
            // `DbtDocsMacro` calls it `path`; the column is `file_path`.
            file_path: str_field_default(payload, "path", ""),
            original_file_path: str_field_default(payload, "original_file_path", ""),
            block_contents: str_field(payload, "block_contents"),
        }
    }
}

pub struct ParsedUnitTest {
    pub name: String,
    pub model: String,
    pub description: Option<String>,
    pub package_name: String,
    pub file_path: String,
    pub original_file_path: String,
    pub fqn: Vec<String>,
    pub given: Option<Value>,
    pub expect: Option<Value>,
    pub overrides: Option<Value>,
    pub versions: Option<Value>,
    pub version: Option<String>,
    pub schema_name: String,
    pub depends_on_nodes: Vec<String>,
    pub depends_on_macros: Vec<String>,
}

impl ParsedUnitTest {
    pub fn from_payload(payload: &Value) -> Self {
        let attr = payload
            .get("__unit_test_attr__")
            .cloned()
            .unwrap_or(Value::Null);
        let common = payload
            .get("__common_attr__")
            .cloned()
            .unwrap_or(Value::Null);
        let version = attr.get("version").and_then(|v| {
            v.as_str()
                .map(|s| s.to_string())
                .or_else(|| v.as_i64().map(|n| n.to_string()))
        });
        Self {
            name: common
                .get("name")
                .or_else(|| payload.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            model: str_field_default(&attr, "model", ""),
            description: str_field(&common, "description"),
            package_name: str_field_default(&common, "package_name", ""),
            file_path: str_field_default(&common, "path", ""),
            original_file_path: str_field_default(&common, "original_file_path", ""),
            fqn: fqn_from_base(payload),
            given: attr.get("given").cloned(),
            expect: attr.get("expect").cloned(),
            overrides: attr.get("overrides").cloned(),
            versions: attr.get("versions").cloned(),
            version,
            schema_name: str_field_default(&common, "schema", ""),
            depends_on_nodes: depends_on_nodes(payload),
            depends_on_macros: depends_on_macros(payload),
        }
    }
}

pub struct ParsedTimeSpine {
    pub primary_column: Option<String>,
    pub primary_granularity: Option<String>,
    pub custom_granularities: Option<Value>,
    pub node_relation: Option<Value>,
}

impl ParsedTimeSpine {
    pub fn from_payload(payload: &Value) -> Self {
        let primary_col = payload.get("primary_column");
        let primary_column = primary_col
            .and_then(|c| c.get("name"))
            .and_then(|v| v.as_str())
            .or_else(|| primary_col.and_then(|v| v.as_str()))
            .map(|s| s.to_string());
        let primary_granularity = primary_col
            .and_then(|c| c.get("time_granularity"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        Self {
            primary_column,
            primary_granularity,
            custom_granularities: payload.get("custom_granularities").cloned(),
            node_relation: payload.get("node_relation").cloned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Regression: `name` must come from `__common_attr__.name`, not
    /// `__metric_attr__` (which has no `name` field in the schema).
    ///
    /// Before the fix, metrics written via the parquet ingest path had empty
    /// names because `ParsedMetric::from_payload` looked in `__metric_attr__`
    /// first and `__metric_attr__` never carries `name`.
    #[test]
    fn metric_name_read_from_common_attr() {
        let payload = json!({
            "__common_attr__": {
                "unique_id": "metric.jaffle_shop.total_revenue",
                "name": "total_revenue",
                "package_name": "jaffle_shop",
                "fqn": ["jaffle_shop", "total_revenue"],
                "path": "models/metrics.yml",
                "original_file_path": "models/metrics.yml"
            },
            "__metric_attr__": {
                "label": "Total Revenue",
                "metric_type": "simple",
                "created_at": 1_747_432_300.5
            },
            "__base_attr__": {}
        });
        let m = ParsedMetric::from_payload("metric.jaffle_shop.total_revenue", &payload, &[]);
        assert_eq!(
            m.name, "total_revenue",
            "name must be populated from __common_attr__.name"
        );
        assert_eq!(m.package_name, "jaffle_shop");
    }

    /// Fallback: `name` from top-level payload when `__common_attr__` is absent
    /// (legacy payload shapes from older Fusion versions).
    #[test]
    fn metric_name_falls_back_to_top_level() {
        let payload = json!({
            "name": "fallback_metric",
            "__metric_attr__": {
                "metric_type": "simple",
                "created_at": 0.0
            }
        });
        let m = ParsedMetric::from_payload("metric.pkg.fallback_metric", &payload, &[]);
        assert_eq!(m.name, "fallback_metric");
    }

    /// Guard: empty `__metric_attr__` (which had no `name` field) must not
    /// silently produce an empty name when `__common_attr__` is present.
    #[test]
    fn metric_name_not_read_from_metric_attr() {
        let payload = json!({
            "__common_attr__": { "name": "correct_name" },
            "__metric_attr__": { "name": "wrong_name_should_be_ignored" }
        });
        let m = ParsedMetric::from_payload("metric.pkg.x", &payload, &[]);
        assert_eq!(
            m.name, "correct_name",
            "__common_attr__.name must take precedence"
        );
    }

    /// `ParsedDoc` must read every column `dbt.docs` declares, not just `name`
    /// and `block_contents`.
    ///
    /// The docs-v2 overview page picks the winning `__overview__` block by
    /// package, so a NULL `package_name` makes a user-authored overview
    /// indistinguishable from the injected default. Note `file_path` comes from
    /// the payload's `path` key — `DbtDocsMacro` names the field `path`, and
    /// reading `file_path` instead yields an empty string rather than an error.
    #[test]
    fn parsed_doc_reads_package_and_paths() {
        let payload = json!({
            "name": "__overview__",
            "package_name": "jaffle_shop",
            "path": "overview.md",
            "original_file_path": "models/overview.md",
            "unique_id": "doc.jaffle_shop.__overview__",
            "block_contents": "# Jaffle Shop"
        });
        let d = ParsedDoc::from_payload(&payload);
        assert_eq!(d.name, "__overview__");
        assert_eq!(d.package_name, "jaffle_shop");
        assert_eq!(d.file_path, "overview.md", "file_path reads the `path` key");
        assert_eq!(d.original_file_path, "models/overview.md");
        assert_eq!(d.block_contents.as_deref(), Some("# Jaffle Shop"));
    }

    /// A payload from before those fields were read must still parse, so an
    /// index written by an older Fusion keeps loading (the columns are nullable
    /// and simply stay empty).
    #[test]
    fn parsed_doc_tolerates_a_legacy_payload() {
        let payload = json!({ "name": "__overview__", "block_contents": "hi" });
        let d = ParsedDoc::from_payload(&payload);
        assert_eq!(d.name, "__overview__");
        assert_eq!(d.package_name, "");
        assert_eq!(d.file_path, "");
        assert_eq!(d.original_file_path, "");
    }
}
