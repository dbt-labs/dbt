//! https://github.com/databricks/dbt-databricks/blob/main/dbt/adapters/databricks/relation_configs/refresh.py

use crate::errors::{AdapterError, AdapterResult};
use crate::relation::config_v2::{
    ComponentConfig, ComponentConfigLoader, SimpleComponentConfigImpl, impl_loader,
};
use crate::relation::databricks::config::{
    DatabricksRelationMetadata, DatabricksRelationMetadataKey,
};
use dbt_schemas::schemas::DbtModel;
use dbt_schemas::schemas::InternalDbtNodeAttributes;
use minijinja::value::Value;
use regex::Regex;
use serde::Serialize;
use std::borrow::Cow;
use std::sync::LazyLock;

pub(crate) const TYPE_NAME: &str = "refresh";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RefreshMode {
    Manual,
    Cron,
    Every,
    OnUpdate,
}

#[derive(Debug, Default, Clone, Eq, Serialize)]
pub(crate) struct Config {
    pub cron: Option<String>,
    pub time_zone_value: Option<String>,
    pub every: Option<String>,
    pub on_update: bool,
    pub at_most_every: Option<String>,
    auto_refreshed: bool,
    // True when both the current and desired states are scheduled. The materialization uses this
    // to choose between ADD SCHEDULE and ALTER SCHEDULE.
    pub is_altered: bool,
}

impl Config {
    pub(crate) fn mode(&self) -> RefreshMode {
        if self.cron.is_some() {
            RefreshMode::Cron
        } else if self.every.is_some() {
            RefreshMode::Every
        } else if self.on_update {
            RefreshMode::OnUpdate
        } else {
            RefreshMode::Manual
        }
    }
}

impl PartialEq for Config {
    // Reference: https://github.com/databricks/dbt-databricks/blob/183843b08287f3f57f78d7cf583ba77c9fbab0a4/dbt/adapters/databricks/relation_configs/refresh.py#L141
    fn eq(&self, other: &Self) -> bool {
        if self.mode() != other.mode() {
            return false;
        }

        match self.mode() {
            RefreshMode::Manual => true,
            RefreshMode::Cron => {
                self.cron == other.cron
                    && normalize_time_zone(self.time_zone_value.as_deref())
                        == normalize_time_zone(other.time_zone_value.as_deref())
            }
            RefreshMode::Every => match (self.every.as_deref(), other.every.as_deref()) {
                (Some(desired), Some(current)) => {
                    normalize_every_interval(desired)
                        .expect("EVERY interval must be validated before comparison")
                        == normalize_every_interval(current)
                            .expect("EVERY interval must be validated before comparison")
                }
                _ => unreachable!("EVERY mode requires an interval"),
            },
            RefreshMode::OnUpdate => {
                match (
                    self.at_most_every.as_deref(),
                    other.at_most_every.as_deref(),
                ) {
                    (None, None) => true,
                    (Some(desired), Some(current)) => {
                        match (
                            parse_interval_seconds(desired),
                            parse_interval_seconds(current),
                        ) {
                            (Ok(desired), Ok(current)) => desired == current,
                            _ => desired == current,
                        }
                    }
                    _ => false,
                }
            }
        }
    }
}

/// Component for Databricks refresh schedule.
pub type Refresh = SimpleComponentConfigImpl<Config>;

fn diff(desired_state: &Config, current_state: &Config) -> Option<Config> {
    if desired_state != current_state {
        let mut change = desired_state.clone();
        change.is_altered = desired_state.mode() != RefreshMode::Manual
            && current_state.mode() != RefreshMode::Manual;
        Some(change)
    } else {
        None
    }
}

fn normalize_time_zone(time_zone: Option<&str>) -> Cow<'_, str> {
    let time_zone = time_zone.unwrap_or("UTC");
    if time_zone.eq_ignore_ascii_case("UTC") || time_zone.eq_ignore_ascii_case("ETC/UTC") {
        Cow::Borrowed("UTC")
    } else {
        Cow::Owned(time_zone.to_uppercase())
    }
}

fn parse_interval_parts(value: &str) -> AdapterResult<(u64, Cow<'static, str>)> {
    static QUANTITY_REGEX: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)^\s*(\d+)\s+([A-Z]+)\s*$").unwrap());

    let captures = QUANTITY_REGEX.captures(value).ok_or_else(|| {
        AdapterError::from_config(format!(
            "Cannot parse interval {value:?}; expected '<integer> <unit>'."
        ))
    })?;
    let quantity = captures[1].parse::<u64>().map_err(|_| {
        AdapterError::from_config(format!("Interval quantity is too large in {value:?}."))
    })?;
    let unit = &captures[2];
    let singular = unit.strip_suffix(['S', 's']).unwrap_or(unit);
    let unit = ["SECOND", "MINUTE", "HOUR", "DAY", "WEEK"]
        .into_iter()
        .find(|canonical| singular.eq_ignore_ascii_case(canonical))
        .map_or_else(|| Cow::Owned(singular.to_uppercase()), Cow::Borrowed);
    Ok((quantity, unit))
}

fn parse_interval_seconds(value: &str) -> AdapterResult<u64> {
    let (quantity, unit) = parse_interval_parts(value)?;
    let seconds_per_unit = match unit.as_ref() {
        "SECOND" => 1,
        "MINUTE" => 60,
        "HOUR" => 3_600,
        "DAY" => 86_400,
        "WEEK" => 604_800,
        _ => {
            return Err(AdapterError::from_config(format!(
                "Unknown interval unit in {value:?}; supported: SECOND, MINUTE, HOUR, DAY, WEEK (singular or plural)."
            )));
        }
    };
    quantity
        .checked_mul(seconds_per_unit)
        .ok_or_else(|| AdapterError::from_config(format!("Interval is too large in {value:?}.")))
}

fn normalize_every_interval(value: &str) -> AdapterResult<(u64, Cow<'static, str>)> {
    let (quantity, unit) = parse_interval_parts(value)?;
    match unit.as_ref() {
        "HOUR" | "DAY" | "WEEK" => Ok((quantity, unit)),
        _ => Err(AdapterError::from_config(format!(
            "Cannot parse `every` value {value:?}; expected '<integer> {{HOURS|DAYS|WEEKS}}'."
        ))),
    }
}

fn validate(cfg: &Config) -> AdapterResult<()> {
    let modes_set = [cfg.cron.is_some(), cfg.every.is_some(), cfg.on_update]
        .into_iter()
        .filter(|is_set| *is_set)
        .count();
    if modes_set > 1 {
        return Err(AdapterError::from_config(
            "Refresh schedule must specify at most one of cron / every / on_update.",
        ));
    }
    if cfg.time_zone_value.is_some() && cfg.cron.is_none() {
        return Err(AdapterError::from_config(
            "`time_zone_value` is only valid when `cron` is set.",
        ));
    }
    if let Some(every) = &cfg.every {
        normalize_every_interval(every)?;
    }
    if let Some(at_most_every) = &cfg.at_most_every {
        if !cfg.on_update {
            return Err(AdapterError::from_config(
                "`at_most_every` is only valid when `on_update` is true.",
            ));
        }
        let seconds = parse_interval_seconds(at_most_every)?;
        if seconds < 60 {
            return Err(AdapterError::from_config(format!(
                "`at_most_every` must be at least 60 seconds (1 minute); got {at_most_every:?} ({seconds}s)."
            )));
        }
    }
    Ok(())
}

fn new_component(cfg: Config) -> Refresh {
    Refresh {
        type_name: TYPE_NAME,
        diff_fn: diff,
        to_jinja_fn: |v| Value::from_serialize(v),
        value: Config {
            auto_refreshed: matches!(cfg.mode(), RefreshMode::Every | RefreshMode::OnUpdate),
            ..cfg
        },
    }
}

fn from_remote_state(results: &DatabricksRelationMetadata) -> AdapterResult<Refresh> {
    let Some(describe_extended) = results.get(&DatabricksRelationMetadataKey::DescribeExtended)
    else {
        return Err(AdapterError::from_config(
            "Could not find describe extended results for refresh schedule.",
        ));
    };

    static CRON_REGEX: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^CRON '(.*)' AT TIME ZONE '(.*)'$").unwrap());
    static EVERY_REGEX: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)^EVERY (\d+ (?:HOURS?|DAYS?|WEEKS?))$").unwrap());
    static TRIGGER_REGEX: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)^TRIGGER ON UPDATE(?: AT MOST EVERY INTERVAL (\d+) SECONDS?)?$").unwrap()
    });

    for row in describe_extended.rows() {
        if let (Ok(key_val), Ok(value_val)) =
            (row.get_item(&Value::from(0)), row.get_item(&Value::from(1)))
            && let (Some(key_str), Some(value_str)) = (key_val.as_str(), value_val.as_str())
            && key_str == "Refresh Schedule"
        {
            let cfg = if value_str == "MANUAL" {
                Config::default()
            } else if let Some(captures) = CRON_REGEX.captures(value_str) {
                Config {
                    cron: Some(captures[1].to_string()),
                    time_zone_value: Some(captures[2].to_string()),
                    ..Default::default()
                }
            } else if let Some(captures) = EVERY_REGEX.captures(value_str) {
                Config {
                    every: Some(captures[1].to_uppercase()),
                    ..Default::default()
                }
            } else if let Some(captures) = TRIGGER_REGEX.captures(value_str) {
                Config {
                    on_update: true,
                    at_most_every: captures
                        .get(1)
                        .map(|seconds| format!("{} SECOND", seconds.as_str())),
                    ..Default::default()
                }
            } else {
                return Err(AdapterError::from_config(format!(
                    "Could not parse refresh schedule from describe extended: {value_str:?}. Please file an issue at https://github.com/dbt-labs/dbt/issues."
                )));
            };
            validate(&cfg)?;
            return Ok(new_component(cfg));
        }
    }

    Err(AdapterError::from_config(
        "Could not find Refresh Schedule in describe extended. Please file an issue at https://github.com/dbt-labs/dbt/issues.",
    ))
}

fn from_local_config(relation_config: &dyn InternalDbtNodeAttributes) -> AdapterResult<Refresh> {
    let schedule = relation_config
        .as_any()
        .downcast_ref::<DbtModel>()
        .and_then(|model| model.__adapter_attr__.databricks_attr.as_ref())
        .and_then(|attr| attr.schedule.as_ref());

    let cfg = schedule.map_or_else(Config::default, |schedule| Config {
        cron: schedule.cron.clone(),
        time_zone_value: schedule.time_zone_value.clone(),
        every: schedule.every.clone(),
        on_update: schedule.on_update.unwrap_or(false),
        at_most_every: schedule.at_most_every.clone(),
        ..Default::default()
    });
    validate(&cfg)?;
    Ok(new_component(cfg))
}

impl_loader!(Refresh, DatabricksRelationMetadata);

impl RefreshLoader {
    pub fn new_component_type_erased(
        cron: Option<String>,
        time_zone_value: Option<String>,
        every: Option<String>,
        on_update: bool,
        at_most_every: Option<String>,
    ) -> Box<dyn ComponentConfig> {
        let cfg = Config {
            cron,
            time_zone_value,
            every,
            on_update,
            at_most_every,
            ..Default::default()
        };
        validate(&cfg).expect("recorded refresh config must be valid");
        Box::new(new_component(cfg))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relation::databricks::config::test_helpers;
    use dbt_agate::AgateTable;
    use indexmap::IndexMap;

    fn create_mock_describe_extended_table(schedule_info: Option<&str>) -> AgateTable {
        let comment_text = schedule_info.unwrap_or("MANUAL");
        test_helpers::create_mock_describe_extended_table([], [("Refresh Schedule", comment_text)])
    }

    fn create_mock_dbt_model(cron: Option<&str>, time_zone: Option<&str>) -> DbtModel {
        let cfg = test_helpers::TestModelConfig {
            cron: cron.map(|s| s.to_string()),
            time_zone: time_zone.map(|s| s.to_string()),
            ..Default::default()
        };

        test_helpers::create_mock_dbt_model(cfg)
    }

    #[test]
    fn test_diff_no_change() {
        let config = Config {
            cron: None,
            time_zone_value: Some("UTC".to_string()),
            is_altered: false,
            ..Default::default()
        };
        let diff = diff(&config, &config);
        assert!(diff.is_none());
    }

    #[test]
    fn test_diff_new_cron() {
        let old = Config {
            cron: None,
            time_zone_value: Some("UTC".to_string()),
            is_altered: false,
            ..Default::default()
        };
        let new = Config {
            cron: Some("* * * * *".to_string()),
            time_zone_value: Some("UTC".to_string()),
            is_altered: false,
            ..Default::default()
        };
        let diff = diff(&new, &old).unwrap();
        assert_eq!(diff.cron, Some("* * * * *".to_string()));
        assert_eq!(diff.time_zone_value, Some("UTC".to_string()));
        assert!(!diff.is_altered);
    }

    #[test]
    fn test_diff_changed_cron_and_timezone() {
        let old = Config {
            cron: Some("* * * * *".to_string()),
            time_zone_value: Some("UTC".to_string()),
            is_altered: false,
            ..Default::default()
        };
        let new = Config {
            cron: Some("*/60 * * * *".to_string()),
            time_zone_value: Some("UTC-01:00".to_string()),
            is_altered: false,
            ..Default::default()
        };
        let diff = diff(&new, &old).unwrap();
        assert_eq!(diff.cron, Some("*/60 * * * *".to_string()));
        assert_eq!(diff.time_zone_value, Some("UTC-01:00".to_string()));
        assert!(diff.is_altered);
    }

    #[test]
    fn test_from_remote_state_manual() {
        let table = create_mock_describe_extended_table(None); // MANUAL by default
        let results = IndexMap::from([(DatabricksRelationMetadataKey::DescribeExtended, table)]);
        let config = from_remote_state(&results).unwrap();

        assert_eq!(config.value.cron, None);
        assert_eq!(config.value.time_zone_value, None);
    }

    #[test]
    fn test_from_remote_state_cron_schedule() {
        let table =
            create_mock_describe_extended_table(Some("CRON '0 */6 * * *' AT TIME ZONE 'UTC'"));
        let results = IndexMap::from([(DatabricksRelationMetadataKey::DescribeExtended, table)]);
        let config = from_remote_state(&results).unwrap();

        assert_eq!(config.value.cron, Some("0 */6 * * *".to_string()));
        assert_eq!(config.value.time_zone_value, Some("UTC".to_string()));
    }

    #[test]
    fn test_from_local_config_with_schedule() {
        let model = create_mock_dbt_model(Some("0 */6 * * *"), Some("UTC"));
        let config = from_local_config(&model).unwrap();

        assert_eq!(config.value.cron, Some("0 */6 * * *".to_string()));
        assert_eq!(config.value.time_zone_value, Some("UTC".to_string()));
    }

    #[test]
    fn test_from_local_config_cron_only() {
        let model = create_mock_dbt_model(Some("0 */12 * * *"), None);
        let config = from_local_config(&model).unwrap();

        assert_eq!(config.value.cron, Some("0 */12 * * *".to_string()));
        assert_eq!(config.value.time_zone_value, None);
    }

    #[test]
    fn test_from_local_config_no_schedule() {
        let mut model = create_mock_dbt_model(None, None);
        let config = from_local_config(&model).unwrap();

        assert_eq!(config.value.cron, None);
        assert_eq!(config.value.time_zone_value, None);
        assert!(!config.value.auto_refreshed);

        model
            .__adapter_attr__
            .databricks_attr
            .as_mut()
            .unwrap()
            .schedule = Some(dbt_yaml::from_str("{}").unwrap());
        let config = from_local_config(&model).unwrap();
        assert_eq!(config.value.cron, None);
        assert_eq!(config.value.time_zone_value, None);
        assert_eq!(config.value.every, None);
        assert!(!config.value.on_update);
        assert!(!config.value.auto_refreshed);
    }

    fn config(
        cron: Option<&str>,
        time_zone: Option<&str>,
        every: Option<&str>,
        on_update: bool,
        at_most_every: Option<&str>,
    ) -> Config {
        Config {
            cron: cron.map(str::to_string),
            time_zone_value: time_zone.map(str::to_string),
            every: every.map(str::to_string),
            on_update,
            at_most_every: at_most_every.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn equality_normalizes_cron_time_zones() {
        let implicit_utc = config(Some("*/5 * * * *"), None, None, false, None);
        let utc = config(Some("*/5 * * * *"), Some("utc"), None, false, None);
        let etc_utc = config(Some("*/5 * * * *"), Some("Etc/UTC"), None, false, None);

        assert_eq!(implicit_utc, utc);
        assert_eq!(utc, etc_utc);
        assert!(diff(&implicit_utc, &etc_utc).is_none());
        for time_zone in [None, Some("UTC"), Some("utc"), Some("Etc/UTC")] {
            assert!(matches!(
                normalize_time_zone(time_zone),
                Cow::Borrowed("UTC")
            ));
        }
    }

    #[test]
    fn equality_normalizes_every_units() {
        let singular = config(None, None, Some("1 day"), false, None);
        let plural = config(None, None, Some("1 DAYS"), false, None);
        assert_eq!(singular, plural);

        let different_unit = config(None, None, Some("1 WEEK"), false, None);
        assert_ne!(singular, different_unit);

        for (value, expected) in [
            ("1 seconds", "SECOND"),
            ("1 MINUTE", "MINUTE"),
            ("1 Hours", "HOUR"),
            ("1 day", "DAY"),
            ("1 WEEKS", "WEEK"),
        ] {
            let (quantity, unit) = parse_interval_parts(value).unwrap();
            assert_eq!(quantity, 1);
            assert!(matches!(unit, Cow::Borrowed(unit) if unit == expected));
        }
    }

    #[test]
    fn equality_normalizes_on_update_intervals() {
        let minutes = config(None, None, None, true, Some("15 MINUTES"));
        let seconds = config(None, None, None, true, Some("900 SECOND"));
        assert_eq!(minutes, seconds);

        let bare = config(None, None, None, true, None);
        assert_ne!(bare, minutes);
    }

    #[test]
    fn equality_distinguishes_modes_and_ignores_render_hints() {
        let manual = config(None, None, None, false, None);
        let mut cron = config(Some("*/5 * * * *"), None, None, false, None);
        let every = config(None, None, Some("2 HOURS"), false, None);
        let on_update = config(None, None, None, true, None);

        assert_ne!(manual, cron);
        assert_ne!(cron, every);
        assert_ne!(every, on_update);
        let original = cron.clone();
        cron.is_altered = true;
        cron.auto_refreshed = true;
        assert_eq!(original, cron);
    }

    #[test]
    fn diff_marks_add_alter_and_drop() {
        let states = [
            config(None, None, None, false, None),
            config(Some("*/5 * * * *"), None, None, false, None),
            config(None, None, Some("2 HOURS"), false, None),
            config(None, None, None, true, Some("15 MINUTES")),
        ]
        .map(|cfg| new_component(cfg).value);
        for (desired_index, desired) in states.iter().enumerate() {
            for (current_index, current) in states.iter().enumerate() {
                let change = diff(desired, current);
                if desired_index == current_index {
                    assert!(change.is_none());
                } else {
                    let change = change.unwrap();
                    assert_eq!(change, *desired);
                    assert_eq!(change.is_altered, desired_index != 0 && current_index != 0);
                    assert_eq!(
                        Value::from_serialize(&change)
                            .get_attr("auto_refreshed")
                            .unwrap(),
                        Value::from(desired_index >= 2)
                    );
                }
            }
        }
    }

    #[test]
    fn validation_rejects_invalid_mode_combinations() {
        let cases = [
            (
                config(Some("*/5 * * * *"), None, Some("2 HOURS"), false, None),
                "at most one",
            ),
            (
                config(None, Some("UTC"), None, false, None),
                "time_zone_value",
            ),
            (
                config(None, None, None, false, Some("15 MINUTES")),
                "at_most_every",
            ),
        ];
        for (cfg, message) in cases {
            assert!(validate(&cfg).unwrap_err().to_string().contains(message));
        }
    }

    #[test]
    fn validation_rejects_invalid_intervals() {
        for invalid in ["30 SECONDS", "59 SECONDS"] {
            let cfg = config(None, None, None, true, Some(invalid));
            assert!(
                validate(&cfg)
                    .unwrap_err()
                    .to_string()
                    .contains("at least 60 seconds")
            );
        }
    }

    #[test]
    fn parses_remote_automatic_schedules() {
        let cases = [
            (
                "EVERY 1 DAYS",
                config(None, None, Some("1 DAYS"), false, None),
            ),
            (
                "every 8 weeks",
                config(None, None, Some("8 WEEKS"), false, None),
            ),
            ("TRIGGER ON UPDATE", config(None, None, None, true, None)),
            (
                "TRIGGER ON UPDATE AT MOST EVERY INTERVAL 900 SECOND",
                config(None, None, None, true, Some("900 SECOND")),
            ),
        ];

        for (value, expected) in cases {
            let table = create_mock_describe_extended_table(Some(value));
            let results =
                IndexMap::from([(DatabricksRelationMetadataKey::DescribeExtended, table)]);
            assert_eq!(
                serde_json::to_value(from_remote_state(&results).unwrap().value).unwrap(),
                serde_json::to_value(new_component(expected).value).unwrap(),
                "schedule: {value}"
            );
        }
    }

    #[test]
    fn rejects_unknown_remote_schedule_shapes() {
        let table = create_mock_describe_extended_table(Some("invalid description"));
        let results = IndexMap::from([(DatabricksRelationMetadataKey::DescribeExtended, table)]);
        assert!(
            from_remote_state(&results)
                .unwrap_err()
                .to_string()
                .contains("Could not parse refresh schedule")
        );
    }

    #[test]
    fn invalid_every_is_rejected() {
        let model = test_helpers::create_mock_dbt_model(test_helpers::TestModelConfig {
            every: Some("2 MINUTES".to_string()),
            ..Default::default()
        });
        assert!(
            from_local_config(&model)
                .unwrap_err()
                .to_string()
                .contains("expected '<integer> {HOURS|DAYS|WEEKS}'")
        );
    }

    #[test]
    fn refresh_jinja_fields_and_recorded_config_match() {
        let refresh = new_component(config(None, None, None, true, Some("15 MINUTES")));
        assert_eq!(
            serde_json::to_value(refresh.to_jinja()).unwrap(),
            serde_json::json!({
                "cron": null,
                "time_zone_value": null,
                "every": null,
                "on_update": true,
                "at_most_every": "15 MINUTES",
                "auto_refreshed": true,
                "is_altered": false,
            })
        );
        let recorded = RefreshLoader::new_component_type_erased(
            None,
            None,
            None,
            true,
            Some("900 SECOND".to_string()),
        );
        assert!(refresh.diff_from(Some(recorded.as_ref())).is_none());
    }

    #[test]
    fn different_cron_time_zones_produce_a_diff() {
        let desired = config(
            Some("0 0 * * * ? *"),
            Some("America/Los_Angeles"),
            None,
            false,
            None,
        );
        let current = config(
            Some("0 0 * * * ? *"),
            Some("America/New_York"),
            None,
            false,
            None,
        );
        assert!(diff(&desired, &current).unwrap().is_altered);
    }

    #[test]
    fn loads_local_schedule_modes_and_auto_refresh_state() {
        let every_model = test_helpers::create_mock_dbt_model(test_helpers::TestModelConfig {
            every: Some("2 HOURS".to_string()),
            ..Default::default()
        });
        let every = from_local_config(&every_model).unwrap();
        assert_eq!(every.value.mode(), RefreshMode::Every);
        assert_eq!(
            every.to_jinja().get_attr("auto_refreshed").unwrap(),
            Value::from(true)
        );

        let on_update_model = test_helpers::create_mock_dbt_model(test_helpers::TestModelConfig {
            on_update: true,
            at_most_every: Some("15 MINUTES".to_string()),
            ..Default::default()
        });
        let on_update = from_local_config(&on_update_model).unwrap();
        assert_eq!(on_update.value.mode(), RefreshMode::OnUpdate);
        assert_eq!(
            on_update.to_jinja().get_attr("auto_refreshed").unwrap(),
            Value::from(true)
        );
    }
}
