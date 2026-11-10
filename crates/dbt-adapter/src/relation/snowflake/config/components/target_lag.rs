use dbt_common::{AdapterError, AdapterResult};
use dbt_schemas::schemas::InternalDbtNodeAttributes;
use minijinja::Value;

use super::snowflake_attr;
use crate::relation::snowflake::config::{
    SnowflakeDescribeResults, get_string_by_name_from_record_batch,
};
use crate::{
    relation::config_v2::{
        ComponentConfig, ComponentConfigLoader, SimpleComponentConfigImpl, diff, impl_loader,
    },
    value::none_value,
};

pub(crate) const TYPE_NAME: &str = "target_lag";

/// Component for Snowflake dynamic table target lag
pub(crate) type TargetLag = SimpleComponentConfigImpl<Option<String>>;

fn to_jinja(v: &Option<String>) -> Value {
    v.as_ref().map(Value::from).unwrap_or_else(none_value)
}

/// A lag value reduced to a form that can be compared across Snowflake's normalization.
#[derive(Debug, PartialEq, Eq)]
enum CanonicalLag {
    /// Lag is inherited from consumers rather than being a duration.
    Downstream,
    Seconds(u64),
}

/// Reduce a lag value to canonical seconds.
///
/// Returns `None` for anything unrecognized; callers fall back to comparing the raw strings
/// rather than guessing at a value they can't parse. Deliberately does NOT reject sub-minute
/// lags: Snowflake enforces its own 60-second floor (error `002755`) and surfacing that
/// server-side error is the intended behavior.
fn canonicalize(value: &str) -> Option<CanonicalLag> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("downstream") {
        return Some(CanonicalLag::Downstream);
    }

    let (count, unit) = value.split_once(char::is_whitespace)?;
    let count: u64 = count.parse().ok()?;
    // Accept both singular and plural spellings, as Snowflake does.
    let seconds_per_unit = match unit.trim().to_ascii_lowercase().trim_end_matches('s') {
        "second" => 1,
        "minute" => 60,
        "hour" => 3600,
        "day" => 86400,
        _ => return None,
    };

    count
        .checked_mul(seconds_per_unit)
        .map(CanonicalLag::Seconds)
}

/// Snowflake normalizes lag units on readback: a table configured with `'60 seconds'` is
/// reported by `SHOW` as `'1 minute'`. Comparing the raw strings therefore reports a change
/// on every run and emits a no-op `ALTER ... SET TARGET_LAG`, so both sides are reduced to a
/// canonical form first.
fn diff_target_lag(desired: &Option<String>, current: &Option<String>) -> Option<Option<String>> {
    diff::optional_by(desired, current, lag_eq)
}

fn lag_eq(desired: &str, current: &str) -> bool {
    match (canonicalize(desired), canonicalize(current)) {
        (Some(desired), Some(current)) => desired == current,
        _ => desired.trim().eq_ignore_ascii_case(current.trim()),
    }
}

fn new_component(target_lag: Option<String>) -> TargetLag {
    TargetLag {
        type_name: TYPE_NAME,
        diff_fn: diff_target_lag,
        to_jinja_fn: to_jinja,
        value: target_lag,
    }
}

fn from_remote_state(results: &SnowflakeDescribeResults) -> AdapterResult<TargetLag> {
    let batch = &results.record_batch;
    let target_lag = match get_string_by_name_from_record_batch(batch, "target_lag") {
        Ok(s) if !s.is_empty() => Some(s),
        _ => None,
    };
    Ok(new_component(target_lag))
}

/// `scheduler` and `target_lag` are mutually constraining, but only for relation types that
/// support `scheduler` at all — see `TargetLagWithoutSchedulerLoader`.
///
/// Reference: https://docs.getdbt.com/reference/resource-configs/snowflake-configs?version=2.0#how-target_lag-interacts-with-scheduler
/// https://github.com/dbt-labs/dbt-adapters/blob/d3c6fd0118ed353a1bd825d8a6f9468e7939d7a2/dbt-snowflake/src/dbt/adapters/snowflake/relation_configs/dynamic_table.py#L170-L179
fn validate_scheduler_interaction(
    scheduler: Option<&str>,
    target_lag: &Option<String>,
) -> AdapterResult<()> {
    let Some(scheduler) = scheduler else {
        return Ok(());
    };
    if scheduler.eq_ignore_ascii_case("enable") && target_lag.is_none() {
        return Err(AdapterError::new(
            dbt_common::AdapterErrorKind::Configuration,
            "Invalid dynamic table config: `scheduler=ENABLE` requires `target_lag`.",
        ));
    } else if scheduler.eq_ignore_ascii_case("disable") && target_lag.is_some() {
        return Err(AdapterError::new(
            dbt_common::AdapterErrorKind::Configuration,
            "Invalid dynamic table config: `scheduler=DISABLE` requires `target_lag` to be omitted.",
        ));
    }
    Ok(())
}

fn from_local_config(relation_config: &dyn InternalDbtNodeAttributes) -> AdapterResult<TargetLag> {
    let snowflake_config = snowflake_attr(relation_config)?;
    let target_lag = snowflake_config.target_lag.clone();
    validate_scheduler_interaction(snowflake_config.scheduler.as_deref(), &target_lag)?;
    Ok(new_component(target_lag))
}

/// `from_local_config` without the `scheduler` cross-validation.
fn from_local_config_without_scheduler(
    relation_config: &dyn InternalDbtNodeAttributes,
) -> AdapterResult<TargetLag> {
    Ok(new_component(
        snowflake_attr(relation_config)?.target_lag.clone(),
    ))
}

impl_loader!(TargetLag, SnowflakeDescribeResults);

/// `TargetLagLoader` for relation types that do not support the `scheduler` config.
///
/// Interactive tables deliberately omit `scheduler` from their component set, so they must
/// not be rejected by a `scheduler` <-> `target_lag` constraint for a key whose value they
/// will never emit.
pub(crate) struct TargetLagWithoutSchedulerLoader;

impl ComponentConfigLoader<SnowflakeDescribeResults> for TargetLagWithoutSchedulerLoader {
    #[cfg(test)]
    fn type_name(&self) -> &'static str {
        TYPE_NAME
    }

    fn from_remote_state(
        &self,
        remote_state: &SnowflakeDescribeResults,
    ) -> AdapterResult<Box<dyn ComponentConfig>> {
        Ok(Box::new(from_remote_state(remote_state)?))
    }

    fn from_local_config(
        &self,
        relation_config: &dyn InternalDbtNodeAttributes,
    ) -> AdapterResult<Box<dyn ComponentConfig>> {
        Ok(Box::new(from_local_config_without_scheduler(
            relation_config,
        )?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relation::snowflake::config::test_helpers;

    #[test]
    fn from_remote_state_none() {
        let remote_state = test_helpers::make_remote_config(test_helpers::TestDynamicTableConfig {
            target_lag: None,
            ..Default::default()
        });
        let loaded = from_remote_state(&remote_state).unwrap();
        assert!(loaded.value.is_none());
    }

    #[test]
    fn from_remote_state_empty() {
        let remote_state = test_helpers::make_remote_config(test_helpers::TestDynamicTableConfig {
            target_lag: Some(""),
            ..Default::default()
        });
        let loaded = from_remote_state(&remote_state).unwrap();
        assert!(loaded.value.is_none());
    }

    #[test]
    fn from_remote_state_hours() {
        let remote_state = test_helpers::make_remote_config(test_helpers::TestDynamicTableConfig {
            target_lag: Some("5 hours"),
            ..Default::default()
        });
        let loaded = from_remote_state(&remote_state).unwrap();
        assert!(loaded.value.is_some());
        assert_eq!(loaded.value.unwrap(), "5 hours");
    }

    #[test]
    fn canonicalize_equates_normalized_readback_units() {
        for (configured, readback) in [
            ("60 seconds", "1 minute"),
            ("120 seconds", "2 minutes"),
            ("60 minutes", "1 hour"),
            ("48 hours", "2 days"),
            ("1 MINUTES", "1 minute"),
            ("  1 minute  ", "1 minute"),
        ] {
            assert_eq!(
                canonicalize(configured),
                canonicalize(readback),
                "{configured} should canonicalize equal to {readback}"
            );
            assert!(
                diff_target_lag(&Some(configured.to_owned()), &Some(readback.to_owned())).is_none(),
                "{configured} vs {readback} should not be a change"
            );
        }
    }

    #[test]
    fn canonicalize_distinguishes_genuinely_different_lags() {
        assert_ne!(canonicalize("60 seconds"), canonicalize("2 minutes"));
        assert_eq!(
            diff_target_lag(&Some("2 minutes".to_owned()), &Some("1 minute".to_owned())),
            Some(Some("2 minutes".to_owned()))
        );
    }

    #[test]
    fn canonicalize_passes_downstream_through() {
        assert_eq!(canonicalize("DOWNSTREAM"), Some(CanonicalLag::Downstream));
        assert_eq!(canonicalize("downstream"), Some(CanonicalLag::Downstream));
        assert!(
            diff_target_lag(
                &Some("DOWNSTREAM".to_owned()),
                &Some("downstream".to_owned())
            )
            .is_none()
        );
        assert_ne!(canonicalize("DOWNSTREAM"), canonicalize("1 minute"));
    }

    #[test]
    fn canonicalize_does_not_enforce_the_sixty_second_minimum() {
        // Snowflake rejects sub-minute lag itself (002755); that error is the intended
        // behavior, so the client must not pre-empt it by rewriting or rejecting the value.
        assert_eq!(canonicalize("30 seconds"), Some(CanonicalLag::Seconds(30)));
        assert_eq!(
            diff_target_lag(&Some("30 seconds".to_owned()), &None),
            Some(Some("30 seconds".to_owned()))
        );
    }

    #[test]
    fn unparseable_lag_falls_back_to_string_comparison() {
        assert_eq!(canonicalize("every other tuesday"), None);
        assert!(
            diff_target_lag(
                &Some("every other tuesday".to_owned()),
                &Some("EVERY OTHER TUESDAY".to_owned())
            )
            .is_none()
        );
        assert!(
            diff_target_lag(
                &Some("every other tuesday".to_owned()),
                &Some("1 minute".to_owned())
            )
            .is_some()
        );
    }

    #[test]
    fn overflowing_lag_falls_back_to_string_comparison() {
        // A count large enough to overflow the seconds conversion must fall back to the
        // string compare, not wrap around into some unrelated small duration.
        let huge = format!("{} days", u64::MAX);
        assert_eq!(canonicalize(&huge), None);
        assert!(diff_target_lag(&Some(huge.clone()), &Some(huge.clone())).is_none());
        assert!(diff_target_lag(&Some(huge), &Some("1 minute".to_owned())).is_some());

        // The same count in seconds needs no multiplication, so it still canonicalizes.
        assert_eq!(
            canonicalize(&format!("{} seconds", u64::MAX)),
            Some(CanonicalLag::Seconds(u64::MAX))
        );
    }

    #[test]
    fn diff_none_to_none_is_no_change() {
        assert!(diff_target_lag(&None, &None).is_none());
    }

    #[test]
    fn diff_set_or_unset_is_a_change() {
        assert_eq!(
            diff_target_lag(&Some("1 minute".to_owned()), &None),
            Some(Some("1 minute".to_owned()))
        );
        assert_eq!(
            diff_target_lag(&None, &Some("1 minute".to_owned())),
            Some(None)
        );
    }

    #[test]
    fn without_scheduler_loader_skips_scheduler_validation() {
        // The same config the dynamic-table path rejects below in
        // `from_local_state_none_and_scheduler_invalid`.
        let local_state = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            scheduler: Some("ENABLE"),
            target_lag: None,
            ..Default::default()
        });
        assert!(from_local_config(&local_state).is_err());
        assert!(from_local_config_without_scheduler(&local_state).is_ok());
    }

    #[test]
    fn without_scheduler_loader_still_reads_target_lag() {
        let local_state = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            scheduler: Some("ENABLE"),
            target_lag: Some("1 hour"),
            ..Default::default()
        });
        let loaded = from_local_config_without_scheduler(&local_state).unwrap();
        assert_eq!(loaded.value.as_deref(), Some("1 hour"));
    }

    #[test]
    fn from_local_state_none_and_scheduler_omitted() {
        let local_state = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            scheduler: None,
            target_lag: None,
            ..Default::default()
        });
        let loaded = from_local_config(&local_state).unwrap();
        assert!(loaded.value.is_none());
    }

    #[test]
    fn from_local_state_none_and_scheduler_ok() {
        let local_state = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            scheduler: Some("DISABLE"),
            target_lag: None,
            ..Default::default()
        });
        let loaded = from_local_config(&local_state).unwrap();
        assert!(loaded.value.is_none());
    }

    #[test]
    fn from_local_state_none_and_scheduler_invalid() {
        let local_state = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            scheduler: Some("ENABLE"),
            target_lag: None,
            ..Default::default()
        });
        let err = from_local_config(&local_state);
        assert!(err.is_err_and(|e| {
            e.message()
                .contains("Invalid dynamic table config: `scheduler=ENABLE`")
        }))
    }

    #[test]
    fn from_local_state_some_and_scheduler_omitted() {
        let local_state = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            scheduler: None,
            target_lag: Some("1 hour"),
            ..Default::default()
        });
        let loaded = from_local_config(&local_state).unwrap();
        assert!(loaded.value.is_some());
        assert_eq!(loaded.value.unwrap(), "1 hour");
    }

    #[test]
    fn from_local_state_some_and_scheduler_ok() {
        let local_state = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            scheduler: Some("ENABLE"),
            target_lag: Some("1 hour"),
            ..Default::default()
        });
        let loaded = from_local_config(&local_state).unwrap();
        assert!(loaded.value.is_some());
        assert_eq!(loaded.value.unwrap(), "1 hour");
    }

    #[test]
    fn from_local_state_some_and_scheduler_invalid() {
        let local_state = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            scheduler: Some("DISABLE"),
            target_lag: Some("1 hour"),
            ..Default::default()
        });
        let err = from_local_config(&local_state);
        assert!(err.is_err_and(|e| {
            e.message()
                .contains("Invalid dynamic table config: `scheduler=DISABLE`")
        }))
    }
}
