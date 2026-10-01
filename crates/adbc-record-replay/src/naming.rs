use adbc_core::error::{Error as AdbcError, Result as AdbcResult, Status as AdbcStatus};
use regex::Regex;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;

use crate::COUNTERS;

pub fn cleanup_schema_name(input: &str) -> String {
    let re = Regex::new(r"___.*?___").unwrap();
    re.replace_all(input, "").to_string()
}

/// Strips real wall-clock content embedded in SQL that has no node context
/// to hash against instead (`checksum8` is the only key such a statement
/// gets, so anything time-based inside it must be normalized away, or the
/// same statement gets a different recording key every run). Currently just
/// `dbt_compute_<millis>`, the token name
/// `mint_snowflake_catalog_credential` mints for Horizon catalog access.
fn cleanup_ephemeral_timestamps(input: &str) -> String {
    let re = Regex::new(r"dbt_compute_\d+").unwrap();
    re.replace_all(input, "dbt_compute_TIMESTAMP").to_string()
}

/// Masks the quoted Snowflake username in `alter user "<user>" add/remove
/// programmatic access token ...` (`mint_snowflake_catalog_credential` /
/// `drop_minted_token` in `compute_platform.rs`). This statement has no node
/// context, so `checksum8` is its only recording key -- and the username is
/// whichever real Snowflake identity recorded the fixture, which will never
/// match another engineer's identity or the `fake_user`/`FAKE_USER` fallback
/// used at pure-replay time. Normalize it away for the same reason as the
/// timestamp above, rather than requiring every recorder to share one
/// identity.
fn cleanup_alter_user_identifier(input: &str) -> String {
    let re = Regex::new(r#"(?i)(alter user )"[^"]+""#).unwrap();
    re.replace_all(input, r#"$1"MASKED_USER""#).to_string()
}

/// Masks the quoted Snowflake username in `show user programmatic access
/// tokens for user "<user>"` (`pat_hygiene_report` in `compute_platform.rs`),
/// for the same reason as `cleanup_alter_user_identifier` above -- this
/// statement has no node context either, so `checksum8` is its only
/// recording key, and it embeds whichever real identity recorded the
/// fixture.
fn cleanup_show_user_pat_identifier(input: &str) -> String {
    let re = Regex::new(r#"(?i)(show user programmatic access tokens for user )"[^"]+""#).unwrap();
    re.replace_all(input, r#"$1"MASKED_USER""#).to_string()
}

/// Masks the volatile suffix in the temp table names of the ClickHouse
/// `EXCHANGE TABLES` capability probe (`__dbt_exchange_test_<n>_<pid>_<nanos>`).
/// The suffix is a process id plus a wall-clock nanosecond timestamp, so no
/// two runs ever emit the literal same name. Normalize it away for the same
/// reason as the timestamp above.
fn cleanup_exchange_probe_tables(input: &str) -> String {
    let re = Regex::new(r"__dbt_exchange_test_(\d+)_\d+_\d+").unwrap();
    re.replace_all(input, "__dbt_exchange_test_${1}_MASKED_ID")
        .to_string()
}

/// Masks the volatile suffix in `dbt debug`'s MDLS write/read-back probe
/// table name (`__dbt_debug_probe_<nanos>`, `debug_mdls.rs`). The suffix is
/// a fresh wall-clock nanosecond timestamp generated on every invocation --
/// record or replay alike -- so no two runs ever emit the literal same
/// name. Normalize it away for the same reason as the timestamp above.
fn cleanup_debug_probe_table(input: &str) -> String {
    let re = Regex::new(r"__dbt_debug_probe_\d+").unwrap();
    re.replace_all(input, "__dbt_debug_probe_MASKED_ID")
        .to_string()
}

/// Masks the volatile suffix in `dbt debug`'s Snowflake propagation probe
/// table name (`__dbt_debug_propagation_<nanos>`, `debug_propagation.rs`).
/// Same reasoning as `cleanup_debug_probe_table` above: a fresh wall-clock
/// nanosecond timestamp on every invocation means no two runs ever emit the
/// literal same name, so it must be normalized away before hashing.
fn cleanup_debug_propagation_probe_table(input: &str) -> String {
    let re = Regex::new(r"__dbt_debug_propagation_\d+").unwrap();
    re.replace_all(input, "__dbt_debug_propagation_MASKED_ID")
        .to_string()
}

fn checksum8(input: &str) -> String {
    let input = cleanup_schema_name(input);
    let input = cleanup_ephemeral_timestamps(&input);
    let input = cleanup_alter_user_identifier(&input);
    let input = cleanup_show_user_pat_identifier(&input);
    let input = cleanup_exchange_probe_tables(&input);
    let input = cleanup_debug_probe_table(&input);
    let input = cleanup_debug_propagation_probe_table(&input);
    let mut hasher = DefaultHasher::new();
    input.hash(&mut hasher);
    let hash = hasher.finish();
    format!("{hash:x}")[..8.min(format!("{hash:x}").len())].to_string()
}

pub fn compute_file_name(
    recordings_dir: &Path,
    node_id: Option<&String>,
    sql: Option<&str>,
    metadata: bool,
) -> AdbcResult<String> {
    let id = match node_id {
        Some(node_id) => {
            if metadata {
                debug_assert!(
                    sql.is_some(),
                    "A Statement with metadata must have a SQL query"
                );
                format!("{}-{}", node_id, checksum8(sql.unwrap()))
            } else {
                node_id.to_owned()
            }
        }
        None => match sql {
            Some(sql) => checksum8(sql),
            None => {
                return Err(AdbcError::with_message_and_status(
                    "Neither node id nor sql was set in the query context",
                    AdbcStatus::Internal,
                ));
            }
        },
    };

    let dir_counters = COUNTERS.entry(recordings_dir.to_path_buf()).or_default();
    let mut entry = dir_counters.entry(id.clone()).or_insert(0);
    let file_name = format!("{}-{}", id, *entry);
    *entry += 1;

    Ok(file_name)
}

pub fn compute_file_name_for_table_schema(
    recordings_dir: &Path,
    node_id: Option<&str>,
    catalog: Option<&str>,
    db_schema: Option<&str>,
    table_name: &str,
) -> String {
    let fqn = format!(
        "{}.{}.{}",
        catalog.unwrap_or("_"),
        db_schema.unwrap_or("_"),
        table_name
    );
    let hash = checksum8(&fqn);
    let counter_key = match node_id {
        Some(node_id) => format!("{node_id}.get_table_schema.{hash}"),
        None => format!("get_table_schema.{hash}"),
    };
    let dir_counters = COUNTERS.entry(recordings_dir.to_path_buf()).or_default();
    let mut entry = dir_counters.entry(counter_key.clone()).or_insert(0);
    let file_name = format!("{counter_key}-{}", *entry);
    *entry += 1;
    file_name
}

pub fn compute_file_name_for_get_objects(
    recordings_dir: &Path,
    node_id: Option<&str>,
    catalog: Option<&str>,
    db_schema: Option<&str>,
    table_name: Option<&str>,
    table_type: Option<&[&str]>,
    column_name: Option<&str>,
) -> String {
    let key = format!(
        "{}.{}.{}.{}.{}",
        catalog.unwrap_or("_"),
        db_schema.unwrap_or("_"),
        table_name.unwrap_or("_"),
        table_type
            .map(|t| t.join(","))
            .unwrap_or_else(|| "_".to_string()),
        column_name.unwrap_or("_"),
    );
    let hash = checksum8(&key);
    let counter_key = match node_id {
        Some(node_id) => format!("{node_id}.get_objects.{hash}"),
        None => format!("get_objects.{hash}"),
    };
    let dir_counters = COUNTERS.entry(recordings_dir.to_path_buf()).or_default();
    let mut entry = dir_counters.entry(counter_key.clone()).or_insert(0);
    let file_name = format!("{counter_key}-{}", *entry);
    *entry += 1;
    file_name
}
