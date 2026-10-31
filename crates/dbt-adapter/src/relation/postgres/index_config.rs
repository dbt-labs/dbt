use std::sync::Arc;

use chrono::{DateTime, Utc};
use minijinja::arg_utils::ArgsIter;
use minijinja::value::{Enumerator, Object};
use minijinja::{Error as MinijinjaError, ErrorKind as MinijinjaErrorKind, Value};
use serde::Deserialize;

/// One entry of a model's `indexes` config.
///
/// PostgresIndexConfig https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-postgres/src/dbt/adapters/postgres/impl.py#L40
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PostgresIndexConfig {
    pub columns: Vec<String>,
    #[serde(default)]
    pub unique: bool,
    #[serde(rename = "type", default)]
    pub index_type: Option<String>,
}

impl PostgresIndexConfig {
    /// Reads an `indexes` entry; none for an absent one.
    pub fn parse(raw_index: &Value) -> Result<Option<Self>, MinijinjaError> {
        if raw_index.is_none() || raw_index.is_undefined() {
            return Ok(None);
        }
        Self::deserialize(raw_index.clone()).map(Some).map_err(|e| {
            MinijinjaError::new(
                MinijinjaErrorKind::InvalidOperation,
                format!("Could not parse index config: {e}"),
            )
        })
    }

    /// The index's name, `render` in Jinja: an md5 of its definition, the relation and the time, as dbt-postgres
    /// names it. The time keeps a rebuilt table's index from colliding with the one it replaces.
    pub fn index_name(&self, relation: &str, now: DateTime<Utc>) -> String {
        let mut inputs = self.columns.clone();
        inputs.push(relation.to_string());
        inputs.push(if self.unique { "True" } else { "False" }.to_string());
        inputs.push(
            self.index_type
                .clone()
                .unwrap_or_else(|| "None".to_string()),
        );
        inputs.push(isoformat(now));
        format!("{:x}", md5::compute(inputs.join("_")))
    }
}

/// Python's `datetime.isoformat()` of a naive UTC time: microseconds only when there are some.
fn isoformat(now: DateTime<Utc>) -> String {
    if now.timestamp_subsec_micros() == 0 {
        now.format("%Y-%m-%dT%H:%M:%S").to_string()
    } else {
        now.format("%Y-%m-%dT%H:%M:%S%.6f").to_string()
    }
}

impl Object for PostgresIndexConfig {
    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        match key.as_str() {
            Some("columns") => Some(Value::from(self.columns.clone())),
            Some("unique") => Some(Value::from(self.unique)),
            Some("type") => Some(
                self.index_type
                    .as_ref()
                    .map_or_else(|| Value::from(()), |t| Value::from(t.as_str())),
            ),
            Some("render") => {
                let this = Arc::clone(self);
                Some(Value::from_func_func("render", move |_state, args| {
                    let iter = ArgsIter::new("render", &["relation"], args);
                    let relation = iter.next_arg::<&Value>()?;
                    iter.finish()?;
                    Ok(Value::from(
                        this.index_name(&relation.to_string(), Utc::now()),
                    ))
                }))
            }
            _ => None,
        }
    }

    fn enumerate(self: &Arc<Self>) -> Enumerator {
        Enumerator::Str(&["columns", "unique", "type"])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn parse(raw: Value) -> Option<PostgresIndexConfig> {
        PostgresIndexConfig::parse(&raw).unwrap()
    }

    #[test]
    fn parses_columns_with_defaults() {
        let index = parse(Value::from_serialize(
            serde_json::json!({ "columns": ["a", "b"] }),
        ));
        assert_eq!(
            index,
            Some(PostgresIndexConfig {
                columns: vec!["a".to_string(), "b".to_string()],
                unique: false,
                index_type: None,
            })
        );
    }

    #[test]
    fn parses_unique_and_type() {
        let index = parse(Value::from_serialize(serde_json::json!({
            "columns": ["a"], "unique": true, "type": "hash"
        })))
        .unwrap();
        assert!(index.unique);
        assert_eq!(index.index_type.as_deref(), Some("hash"));
    }

    #[test]
    fn parses_none_as_no_index() {
        assert_eq!(parse(Value::from(())), None);
    }

    #[test]
    fn refuses_an_index_without_columns() {
        let raw = Value::from_serialize(serde_json::json!({ "unique": true }));
        assert!(PostgresIndexConfig::parse(&raw).is_err());
    }

    // Expected names are dbt-postgres's PostgresIndexConfig.render with its clock fixed at
    // 2026-10-31T12:00:00 UTC, for the relation "db"."s"."t".
    #[test]
    fn renders_the_name_dbt_postgres_does() {
        let index = PostgresIndexConfig {
            columns: vec!["a".to_string(), "b".to_string()],
            unique: true,
            index_type: Some("btree".to_string()),
        };
        let now = Utc.with_ymd_and_hms(2026, 10, 31, 12, 0, 0).unwrap();
        assert_eq!(
            index.index_name("\"db\".\"s\".\"t\"", now),
            "2644471fd91c5a8f391de536364ebfe1"
        );
    }

    #[test]
    fn renders_the_name_dbt_postgres_does_with_defaults() {
        let index = PostgresIndexConfig {
            columns: vec!["a".to_string()],
            unique: false,
            index_type: None,
        };
        let now = Utc.with_ymd_and_hms(2026, 10, 31, 12, 0, 0).unwrap();
        assert_eq!(
            index.index_name("\"db\".\"s\".\"t\"", now),
            "3f2ce2650d909c563bf6e4eb6d1dd1e3"
        );
    }

    #[test]
    fn writes_microseconds_only_when_there_are_some() {
        let whole = Utc.with_ymd_and_hms(2026, 10, 31, 12, 0, 0).unwrap();
        let fractional = whole + chrono::Duration::microseconds(5);
        assert_eq!(isoformat(whole), "2026-10-31T12:00:00");
        assert_eq!(isoformat(fractional), "2026-10-31T12:00:00.000005");
    }

    #[test]
    fn exposes_its_fields_to_jinja() {
        let index = Value::from_object(PostgresIndexConfig {
            columns: vec!["a".to_string()],
            unique: false,
            index_type: None,
        });
        assert_eq!(index.get_attr("unique").unwrap(), Value::from(false));
        assert!(index.get_attr("type").unwrap().is_none());
        assert_eq!(index.get_attr("columns").unwrap().len(), Some(1));
    }
}
