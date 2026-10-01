use crate::errors::{AdapterError, AdapterErrorKind, AdapterResult};
use crate::relation::config_v2::{
    ComponentConfig, ComponentConfigLoader, SimpleComponentConfigImpl, diff, impl_loader,
};

use arrow_schema::Schema;
use chrono::{DateTime, TimeDelta, Utc, format::SecondsFormat};
use dbt_adbc::duration::parse_duration;
use dbt_schemas::schemas::{DbtModel, InternalDbtNodeAttributes};
use minijinja::value::{Value, ValueMap};
use minijinja_contrib::modules::py_datetime::datetime::PyDateTime;
use serde::Serialize;

#[derive(Debug, Default, Clone, Serialize, PartialEq)]
pub(crate) struct Config {
    pub(crate) enable: bool,
    pub(crate) interval_min: f64,
    pub(crate) max_staleness: String,
    pub(crate) expiration: Option<DateTime<Utc>>,
}

// Reference: https://github.com/dbt-labs/dbt-adapters/blob/2a94cc75dba1f98fa5caff1f396f5af7ee444598/dbt-bigquery/src/dbt/adapters/bigquery/relation_configs/_options.py#L29
fn to_jinja(cfg: &Config) -> Value {
    let mut vm = ValueMap::from([
        (Value::from("enable_refresh"), Value::from(cfg.enable)),
        (
            Value::from("refresh_interval_minutes"),
            Value::from(cfg.interval_min),
        ),
    ]);

    if !cfg.max_staleness.is_empty() {
        vm.insert(
            Value::from("max_staleness"),
            Value::from(cfg.max_staleness.clone()),
        );
    }

    if let Some(expiration) = cfg.expiration {
        vm.insert(
            Value::from("expiration_timestamp"),
            Value::from(format!(
                "TIMESTAMP '{}'",
                expiration.to_rfc3339_opts(SecondsFormat::Micros, true)
            )),
        );
    }

    vm.into()
}

pub(crate) const TYPE_NAME: &str = "refresh";

/// Component for BigQuery refresh config
pub(crate) type Refresh = SimpleComponentConfigImpl<Config>;

const DEFAULT_ENABLE: bool = true;
const DEFAULT_INTERVAL_MIN: f64 = 30.0;

fn new_component(cfg: Config) -> Refresh {
    Refresh {
        type_name: TYPE_NAME,
        diff_fn: diff::desired_state,
        to_jinja_fn: to_jinja,
        value: cfg,
    }
}

fn from_remote_state(schema: &Schema) -> AdapterResult<Refresh> {
    let cfg = Config {
        enable: schema
            .metadata
            .get("MaterializedView.EnableRefresh")
            .map(|s| s.parse::<bool>().unwrap_or(DEFAULT_ENABLE))
            .unwrap_or(DEFAULT_ENABLE),
        interval_min: schema
            .metadata
            .get("MaterializedView.RefreshInterval")
            .map(|s| {
                parse_duration(s)
                    .map(|v| v.as_secs() as f64 / 60.0)
                    .unwrap_or(DEFAULT_INTERVAL_MIN)
            })
            .unwrap_or(DEFAULT_INTERVAL_MIN),
        // NOTE: dbt set this to None, but the ADBC driver provides it under
        // MaterializedView.MaxStaleness.
        //
        // This is a deviation from Core.
        // https://github.com/dbt-labs/dbt-adapters/blob/2a94cc75dba1f98fa5caff1f396f5af7ee444598/dbt-bigquery/src/dbt/adapters/bigquery/relation_configs/_options.py#L142
        max_staleness: schema
            .metadata
            .get("MaterializedView.MaxStaleness")
            .map(|s| s.to_owned())
            .unwrap_or_else(|| "".to_owned()),
        expiration: schema.metadata.get("ExpirationTime").map(|s| {
            DateTime::parse_from_rfc3339(s)
                .expect("'ExpirationTime' does not conform to RFC-3339. This is a driver bug.")
                .to_utc()
        }),
    };

    Ok(new_component(cfg))
}

fn from_local_config(relation_config: &dyn InternalDbtNodeAttributes) -> AdapterResult<Refresh> {
    let cfg = match relation_config.as_any().downcast_ref::<DbtModel>() {
        None => Config {
            enable: DEFAULT_ENABLE,
            interval_min: DEFAULT_INTERVAL_MIN,
            ..Default::default()
        },
        Some(model) => {
            let model_cfg = model
                .__adapter_attr__
                .bigquery_attr
                .as_ref()
                .ok_or_else(|| {
                    AdapterError::new(
                        AdapterErrorKind::Configuration,
                        "relation config needs to be BigQuery model".to_string(),
                    )
                })?;

            Config {
                enable: model_cfg.enable_refresh.unwrap_or(DEFAULT_ENABLE),
                interval_min: model_cfg
                    .refresh_interval_minutes
                    .unwrap_or(DEFAULT_INTERVAL_MIN),
                max_staleness: model_cfg
                    .max_staleness
                    .as_ref()
                    .map(|s| s.to_owned())
                    .unwrap_or_else(|| "".to_string()),
                // Reference: https://github.com/dbt-labs/dbt-adapters/blob/2a94cc75dba1f98fa5caff1f396f5af7ee444598/dbt-bigquery/src/dbt/adapters/bigquery/relation_configs/_options.py#L129
                // hours_to_expiration is loosely typed (may be a non-numeric
                // string such as "null"); only a parseable integer yields a
                // materialized-view expiration.
                expiration: model_cfg
                    .hours_to_expiration
                    .as_ref()
                    .and_then(|v| v.to_string().parse::<i64>().ok())
                    .map(|v| Utc::now() + TimeDelta::hours(v)),
            }
        }
    };
    Ok(new_component(cfg))
}

impl_loader!(Refresh, Schema);

impl RefreshLoader {
    pub fn new_component_type_erased(cfg: Config) -> Box<dyn ComponentConfig> {
        Box::new(new_component(cfg))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relation::bigquery::config::test_helpers::{
        TestTableConfig, make_driver_data, make_local_config,
    };

    #[test]
    fn from_remote_state_disabled() {
        let driver_data = make_driver_data(TestTableConfig {
            enable_refresh: Some(false),
            ..Default::default()
        });
        let loaded = from_remote_state(&driver_data).unwrap();
        assert!(!loaded.value.enable);
    }

    #[test]
    fn from_remote_state_enabled() {
        let driver_data = make_driver_data(TestTableConfig {
            enable_refresh: Some(true),
            refresh_interval_minutes: 1234.00,
            max_staleness: "2025-01-01T00:00:00+00:00",
            expiration_ns: 123456,
            ..Default::default()
        });
        let loaded = from_remote_state(&driver_data).unwrap();
        assert!(loaded.value.enable);
        assert_eq!(loaded.value.interval_min, 1234.00);
        assert_eq!(loaded.value.max_staleness, "2025-01-01T00:00:00+00:00");
        assert_eq!(
            loaded
                .value
                .expiration
                .unwrap()
                .timestamp_nanos_opt()
                .unwrap(),
            123456
        );
    }

    #[test]
    fn from_local_config_not_configured() {
        let local_data = make_local_config(Default::default());
        let loaded = from_local_config(&local_data).unwrap();
        assert!(loaded.value.enable);
        assert_eq!(loaded.value.interval_min, DEFAULT_INTERVAL_MIN);
    }

    #[test]
    fn from_local_config_disabled() {
        let local_data = make_local_config(TestTableConfig {
            enable_refresh: Some(false),
            ..Default::default()
        });
        let loaded = from_local_config(&local_data).unwrap();
        assert!(!loaded.value.enable);
    }

    #[test]
    fn from_local_config_custom_configs() {
        let now_ns = Utc::now().timestamp_nanos_opt().unwrap();
        let expiration_ns = 24 * 60 * 60 * 1_000_000_000;
        let local_data = make_local_config(TestTableConfig {
            enable_refresh: Some(true),
            refresh_interval_minutes: 1234.00,
            max_staleness: "2025-01-01T00:00:00+00:00",
            expiration_ns,
            ..Default::default()
        });
        let loaded = from_local_config(&local_data).unwrap();
        assert!(loaded.value.enable);
        assert_eq!(loaded.value.interval_min, 1234.00);
        assert_eq!(loaded.value.max_staleness, "2025-01-01T00:00:00+00:00");

        let loaded_ns = loaded
            .value
            .expiration
            .unwrap()
            .timestamp_nanos_opt()
            .unwrap();

        let delta_ns = 2 * 1_000_000_000;
        assert!((loaded_ns as i64 - now_ns - expiration_ns as i64).abs() < delta_ns);
    }
}
