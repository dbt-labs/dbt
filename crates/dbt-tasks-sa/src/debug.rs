use dbt_adapter_core::AdapterType;
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use dbt_adbc::QueryCtx;
use dbt_agate::MappedSequence;
use dbt_common::cancellation::CancellationToken;
use dbt_common::io_args::{EvalArgs, LocalExecutionBackendKind, ReplayMode};
use dbt_common::tracing::dbt_emit::emit_info_progress_message;
use dbt_common::{ErrorCode, FsResult, fs_err};
use dbt_compilation::core::DbtLoadedProject;
use dbt_schemas::dbt_utils::resolve_package_quoting;
use dbt_schemas::schemas::common::ResolvedQuoting;
use dbt_schemas::schemas::profiles::{DbConfig, Execute};
use dbt_schemas::schemas::relations::default_resolved_quoting_for;
use dbt_tasks_core::lake_compute_catalog_attach::{
    LakeComputeCatalogAttachChecker, LakeComputeCatalogAttachOutcome, PatHygieneReport,
};
use dbt_tasks_core::lake_compute_mdls::{LakeComputeMdlsChecker, LakeComputeMdlsOutcome};
use dbt_tasks_core::lake_compute_propagation::{
    LakeComputePropagationChecker, LakeComputePropagationOutcome,
};
use dbt_telemetry::ProgressMessage;

// Action labels for debug command progress messages (without padding - formatter handles padding)
const ACTION_DEBUGGING: &str = "Debugging";
const ACTION_DEBUGGED: &str = "Debugged";
const ACTION_SKIPPED: &str = "Skipped";

// dbt-core event codes for JSON compatibility
const DBT_CORE_DEBUG_CMD_OUT: &str = "Z047";
const DBT_CORE_DEBUG_CMD_RESULT: &str = "Z048";

/// Renders `" (N.Ns)"` for a step's elapsed time, or nothing if it took
/// under a second -- fast steps don't need their timing called out.
fn duration_suffix(elapsed: Duration) -> String {
    if elapsed > Duration::from_secs(1) {
        format!(" ({:.1}s)", elapsed.as_secs_f64())
    } else {
        String::new()
    }
}

/// Renders the per-adapter prefix for a debug line: `"snowflake "` when a target
/// declares more than one adapter, and nothing when it declares one.
///
/// A single-adapter profile is the overwhelming majority, and its output should
/// read exactly as it did before `dbt debug` learned to check every adapter.
fn adapter_label(adapter_type: AdapterType, name_adapters: bool) -> String {
    if name_adapters {
        format!("{adapter_type} ")
    } else {
        String::new()
    }
}

/// Helper to create progress message
fn create_progress_msg(action: &str, target: &str) -> ProgressMessage {
    let dbt_core_event_code = if action == ACTION_DEBUGGED {
        DBT_CORE_DEBUG_CMD_RESULT.to_string()
    } else {
        DBT_CORE_DEBUG_CMD_OUT.to_string()
    };

    ProgressMessage::new_with_code(
        action.to_string(),
        target.to_string(),
        None,
        dbt_core_event_code,
    )
}

pub struct DebugArgs {
    pub target: Option<String>,
    pub connection: bool,
    pub local_execution_backend: LocalExecutionBackendKind,
    /// This invocation's id, sent as `adbc.dbt.run_id` on the queries the lake
    /// compute checks issue so dbt Compute can attribute them.
    pub invocation_id: String,
    /// The invocation's record/replay mode. Every adapter these checks build
    /// has to be created with it, or `--fs-record` captures nothing and
    /// `--fs-replay` goes to the network anyway.
    pub replay: Option<ReplayMode>,
    /// Checker for verifying lake-compute-to-native propagation, if this
    /// build has one registered. `None` means the check is skipped.
    pub lake_compute_propagation_checker: Option<Arc<dyn LakeComputePropagationChecker>>,
    /// Checker for verifying the catalogs declared in `catalogs.yml` are
    /// reachable from the lake compute target, if this build has one
    /// registered. `None` means the check is skipped.
    pub lake_compute_catalog_attach_checker: Option<Arc<dyn LakeComputeCatalogAttachChecker>>,
    /// Checker for the MDLS write + read-back round trip, if this build has one
    /// registered. `None` means the check is skipped.
    pub mdls_checker: Option<Arc<dyn LakeComputeMdlsChecker>>,
}

impl DebugArgs {
    pub fn from_eval_args(arg: &EvalArgs) -> Self {
        Self {
            target: arg.target.clone(),
            connection: arg.connection,
            local_execution_backend: arg.local_execution_backend,
            invocation_id: arg.io.invocation_id.to_string(),
            replay: arg.replay.clone(),
            lake_compute_propagation_checker: None,
            lake_compute_catalog_attach_checker: None,
            mdls_checker: None,
        }
    }
}

/// Ceiling for the whole dbt Compute section of `dbt debug`, enforced around
/// the one `spawn_blocking` that runs it. See the call site for why it lives
/// there and not inside the individual checks.
const LAKE_COMPUTE_CHECKS_TIMEOUT: Duration = Duration::from_secs(300);

#[allow(clippy::cognitive_complexity)]
pub async fn debug(
    arg: &DebugArgs,
    loaded_project: &DbtLoadedProject,
    token: CancellationToken,
) -> FsResult<()> {
    let db_config = loaded_project
        .dbt_state()
        .dbt_profile
        .default_db_config()
        .clone();

    let mut all_debug_checks_passed = true;

    // profile info
    let profile_display = format!("profile: {}", arg.target.clone().unwrap_or_default());
    emit_info_progress_message(create_progress_msg(ACTION_DEBUGGING, &profile_display));

    // dbt version
    let dbt_version_display = format!("dbt version: {}", env!("CARGO_PKG_VERSION"));
    emit_info_progress_message(create_progress_msg(ACTION_DEBUGGING, &dbt_version_display));

    // platform info
    let platform_info_display = format!(
        "platform: {} {} ({})",
        std::env::consts::OS,
        std::env::consts::ARCH,
        std::env::consts::FAMILY
    );
    emit_info_progress_message(create_progress_msg(
        ACTION_DEBUGGING,
        &platform_info_display,
    ));

    let default_adapter = loaded_project.dbt_state().dbt_profile.default_adapter;
    let execute = Execute::from_compute_flag(arg.local_execution_backend);
    let adapter_info_display = format!("adapter type: {} ({})", default_adapter, execute);
    emit_info_progress_message(create_progress_msg(ACTION_DEBUGGING, &adapter_info_display));

    // Skip dependency info if --connection is set
    if arg.connection {
        emit_info_progress_message(create_progress_msg(
            ACTION_SKIPPED,
            "steps before connection testing",
        ));
    } else {
        // dependency info
        let dependencies = ["git"];
        let mut dependency_displays = Vec::new();
        for dep in dependencies {
            let status = if dependency_installed(dep).await? {
                format!("{dep}: OK")
            } else {
                all_debug_checks_passed = false;
                format!("{dep}: ERROR")
            };
            dependency_displays.push(status);
        }

        emit_info_progress_message(create_progress_msg(
            ACTION_DEBUGGING,
            &format!("dependencies:\n  {}", dependency_displays.join("\n  ")),
        ));
    }

    // Every declared adapter is checked, not just the target's default: a node can
    // select any of them with `+adapter`, so a run is only as healthy as the least
    // reachable one. Declaration order, and the default is not special-cased --
    // it is simply one of the entries.
    //
    // A failure does not abort the loop. `dbt debug` exists to report everything
    // that is wrong in one pass, so each adapter is checked and the first error is
    // returned at the end, which keeps the non-zero exit a single-adapter profile
    // has always produced.
    let adapters: Vec<(AdapterType, DbConfig)> = loaded_project
        .dbt_state()
        .dbt_profile
        .adapters
        .iter()
        .map(|(adapter_type, adapter)| (*adapter_type, adapter.config().clone()))
        .collect();
    // Only worth naming which adapter a line is about when there is more than one,
    // so a single-adapter profile's output is unchanged.
    let name_adapters = adapters.len() > 1;
    let mut first_connection_error = None;
    let mut unreachable_adapters = Vec::new();

    for (adapter_type, adapter_db_config) in &adapters {
        let label = adapter_label(*adapter_type, name_adapters);

        // Format connection details, omitting any secrets via into_connection_mapping().
        let mapping = adapter_db_config.to_connection_mapping().unwrap();
        let connection_details = serde_json::to_string_pretty(&mapping)?
            .trim_matches('{')
            .trim_matches('}')
            .trim()
            .to_string();

        emit_info_progress_message(create_progress_msg(
            ACTION_DEBUGGING,
            &format!("{label}connection:\n  {connection_details}"),
        ));

        // Sidecar/DuckDB mode doesn't connect to a remote warehouse; skip the connection test.
        if execute == Execute::Sidecar {
            emit_info_progress_message(create_progress_msg(
                ACTION_SKIPPED,
                &format!("{label}local connection test"),
            ));
            continue;
        }

        match debug_adapter_connection(
            *adapter_type,
            adapter_db_config,
            &label,
            loaded_project,
            arg.replay.as_ref(),
            &token,
        )
        .await
        {
            Ok(()) => {}
            Err(e) => {
                all_debug_checks_passed = false;
                unreachable_adapters.push(*adapter_type);
                emit_info_progress_message(create_progress_msg(
                    ACTION_DEBUGGING,
                    &format!("{label}connection test: ERROR"),
                ));
                first_connection_error.get_or_insert(e);
            }
        }
    }

    // Lake Compute (dbt-compute / MDLS) checks: only when the profile
    // declares an adapter of type `lake_compute` in the active target's adapter list.
    // Independent of whichever target is currently active/selected.
    if let Some(lake_compute_db_config) = loaded_project
        .dbt_state()
        .dbt_profile
        .adapter(AdapterType::LakeCompute)
        .cloned()
    {
        // Every one of these probes goes through the lake compute connection, so running
        // them after that connection failed only replaces the real diagnostic with
        // a cascade of derived ones.
        if unreachable_adapters.contains(&AdapterType::LakeCompute) {
            emit_info_progress_message(create_progress_msg(
                ACTION_SKIPPED,
                "dbt Compute checks (the lake compute connection is unreachable)",
            ));
        } else {
            // Read here rather than in `debug_lake_compute`: none of it opens a
            // connection, and doing it on this side keeps `DbtLoadedProject` --
            // which is borrowed all the way up from `dbt-main` -- out of the
            // `'static` worker closure below.
            let dbt_state = loaded_project.dbt_state();
            let (mdls_database, mdls_schema) = resolve_probe_namespace(
                &lake_compute_db_config,
                &dbt_state.dbt_profile.database,
                &dbt_state.dbt_profile.schema,
            );
            let (mdls_database, mdls_schema) = (mdls_database.to_string(), mdls_schema.to_string());
            // `root_project_name()` indexes `packages[0]`, and both `dbt debug`
            // and `dbt init` can run before any package is loaded -- the same
            // guard `send_vortex_telemetry_if_possible` uses on this path.
            let project_name = (!dbt_state.packages.is_empty())
                .then(|| loaded_project.root_project_name().to_string());
            // The propagation probe below builds its own throwaway catalog
            // bundle, so it doesn't need a catalog-linked database declared
            // in `catalogs.yml` -- any Snowflake database the lake compute
            // target can write to and the native connection can read from
            // works. Reuse the namespace already resolved for the MDLS
            // probe above instead of gating on a declared CLD.
            let propagation_database = mdls_database.clone();
            // Same quoting a real lake compute model gets absent a node-level
            // `+quoting` override: the root project's `quoting:` config
            // filled in with lake compute's adapter defaults (see
            // `resolver.rs`'s `root_project_quoting`). Packages can be empty
            // this early (see `project_name` above), in which case there is
            // no project override to read and the adapter default applies
            // as-is.
            let propagation_quoting = if dbt_state.packages.is_empty() {
                default_resolved_quoting_for(AdapterType::LakeCompute)
            } else {
                ResolvedQuoting::try_from(resolve_package_quoting(
                    *dbt_state.root_project().quoting,
                    AdapterType::LakeCompute,
                ))
                .unwrap_or_else(|_| default_resolved_quoting_for(AdapterType::LakeCompute))
            };

            let catalog_attach_checker = arg.lake_compute_catalog_attach_checker.clone();
            let mdls_checker = arg.mdls_checker.clone();
            let propagation_checker = arg.lake_compute_propagation_checker.clone();
            let native_db_config = db_config.clone();
            let invocation_id = arg.invocation_id.clone();
            let worker_token = token.clone();
            let reply = arg.replay.clone();

            // Every probe in the section opens a connection, so the whole
            // section runs on one worker. The timeout lives here, around that
            // single dispatch, because the checks themselves are synchronous
            // and can no longer await one of their own: it is a ceiling for
            // "dbt Compute never answered at all", and it is deliberately
            // above the sum of the per-call ceilings it replaced (90s each for
            // the attach probe and the MDLS statements, 120s for
            // propagation) so nothing that used to finish in time can newly
            // time out.
            //
            // As before, a timeout abandons the work rather than cancelling
            // it: a blocking task cannot be interrupted, so the worker stays
            // busy until the driver returns.
            match tokio::time::timeout(
                LAKE_COMPUTE_CHECKS_TIMEOUT,
                dbt_runtime::spawn_blocking(move || {
                    debug_lake_compute(
                        catalog_attach_checker,
                        mdls_checker,
                        propagation_checker,
                        native_db_config,
                        lake_compute_db_config,
                        mdls_database,
                        mdls_schema,
                        project_name,
                        invocation_id,
                        propagation_database,
                        propagation_quoting,
                        reply.as_ref(),
                        worker_token,
                    )
                }),
            )
            .await
            {
                Ok(Ok(result)) => result?,
                Ok(Err(join_err)) => {
                    return Err(fs_err!(
                        ErrorCode::Generic,
                        "dbt Compute checks panicked: {join_err}"
                    ));
                }
                Err(_elapsed) => {
                    return Err(fs_err!(
                        ErrorCode::Generic,
                        "dbt Compute did not respond to the checks within {}s (hard \
                         client-side timeout).",
                        LAKE_COMPUTE_CHECKS_TIMEOUT.as_secs()
                    ));
                }
            }
        }
    }

    if let Some(e) = first_connection_error {
        return Err(e);
    }

    if all_debug_checks_passed {
        emit_info_progress_message(create_progress_msg(ACTION_DEBUGGED, "All checks passed!"));
    }

    Ok(())
}

/// Connects to one adapter and runs `select 1 as id`, plus the Snowflake-only
/// `allow_id_token` probe.
///
/// `label` names the adapter when a target declares more than one, and is empty
/// otherwise so single-adapter output reads as it always has.
async fn debug_adapter_connection(
    adapter_type: AdapterType,
    db_config: &DbConfig,
    label: &str,
    loaded_project: &DbtLoadedProject,
    replay: Option<&ReplayMode>,
    token: &CancellationToken,
) -> FsResult<()> {
    // dbt-auth has no notion of self_signed_jwt; the native connection it
    // builds is identical to keypair's. Normalize a throwaway copy for the
    // connection test only -- db_config itself still needs to read as
    // self_signed_jwt for the PAT-hygiene report below.
    let connection_test_config = if let DbConfig::Snowflake(snowflake) = db_config
        && snowflake.method.as_deref() == Some("self_signed_jwt")
    {
        let mut snowflake = snowflake.clone();
        snowflake.method = Some("keypair".to_string());
        DbConfig::Snowflake(snowflake)
    } else {
        db_config.clone()
    };
    let mut config_as_mapping = connection_test_config.to_mapping().unwrap();
    // set a short timeout for the connection test to fail fast if there are issues
    config_as_mapping
        .entry("connect_timeout".into())
        .or_insert("1s".into());

    // Attempt connection using 'select 1 as id'
    let base_adapter = loaded_project.init_base_adapter(
        adapter_type,
        config_as_mapping,
        replay.cloned(),
        token.clone(),
    )?;

    // Everything below issues a query, so it runs on a `dbt-runtime` worker:
    // every database connection must be created by one. Only the adapter, the
    // label and one flag are needed there -- `init_base_adapter` above opens
    // nothing by itself.
    let snowflake_externalbrowser = matches!(db_config, DbConfig::Snowflake(inner)
        if inner.authenticator.as_deref() == Some("externalbrowser"));
    let label = label.to_owned();
    dbt_runtime::spawn_blocking(move || -> FsResult<()> {
        let sql = "select 1 as id";
        let ctx = QueryCtx::default();
        let connection_test_started = Instant::now();
        base_adapter
            .execute_without_state(Some(&ctx), sql, false, None)
            .map_err(|e| fs_err!(ErrorCode::AuthenticationFailed, "dbt was unable to connect to the database configured for the `{}` adapter.\nThe following error was returned:\n\n{}\n\nCheck your database credentials and try again. For more information, visit:\nhttps://docs.getdbt.com/docs/core/connect-data-platform/connection-profiles", adapter_type, e))?;
        let connection_test_elapsed = connection_test_started.elapsed();

        // Check for allow_id_token parameter when using Snowflake with externalbrowser
        if snowflake_externalbrowser {
            let sql = "SHOW PARAMETERS LIKE 'ALLOW_ID_TOKEN' IN ACCOUNT";

            let allow_token_id = match base_adapter
                .execute_without_state(Some(&ctx), sql, true, None)
                .map_err(|e| fs_err!(ErrorCode::AuthenticationFailed, "{}", e))
            {
                Ok((_result, agate_table)) => {
                    let columns = agate_table.columns().values();

                    if let Some(value_column) = columns.get(1) {
                        if let Ok(value) = value_column.get_item_by_index(0) {
                            let value_str = value.as_str().unwrap_or("");
                            Some(value_str.eq_ignore_ascii_case("true"))
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                }
                Err(_e) => None,
            };

            // The LSP relies on the contents of this debug line to determine whether to
            // show a tip. It matches on a substring, so the adapter label may precede it.
            let allow_token_id_result = match allow_token_id {
                    Some(true) => "Enabled".to_string(),
                    Some(false) => "Disabled. Consider enabling the Snowflake system parameter allow_id_token, to open fewer browser tabs during authentication. See https://docs.getdbt.com/docs/local/connect-data-platform/snowflake-setup?version=2.0#supported-authentication-types for more info.".to_string(),
                    None => "Unable to confirm. Consider enabling the Snowflake system parameter allow_id_token, to open fewer browser tabs during authentication. See https://docs.getdbt.com/docs/local/connect-data-platform/snowflake-setup?version=2.0#supported-authentication-types for more info.".to_string(),
                };

            emit_info_progress_message(create_progress_msg(
                ACTION_DEBUGGING,
                &format!("{label}externalbrowser connection caching: {allow_token_id_result}"),
            ));
        }

        emit_info_progress_message(create_progress_msg(
            ACTION_DEBUGGING,
            &format!(
                "{label}connection test: OK{}",
                duration_suffix(connection_test_elapsed)
            ),
        ));

        Ok(())
    })
    .await
    .map_err(|e| fs_err!(ErrorCode::Generic, "spawn_blocking join error: {e}"))?
}

/// Runs the Lake Compute checks that are specific to lake compute: declared-catalog
/// attach, MDLS write/read-back, and (if a checker is registered)
/// native-connection propagation.
///
/// Connecting to `lake_compute` is not one of them -- that is a plain connection test, and
/// the per-adapter loop in [`debug`] runs it for every declared adapter.
#[allow(clippy::too_many_arguments)]
fn debug_lake_compute(
    catalog_attach_checker: Option<Arc<dyn LakeComputeCatalogAttachChecker>>,
    mdls_checker: Option<Arc<dyn LakeComputeMdlsChecker>>,
    propagation_checker: Option<Arc<dyn LakeComputePropagationChecker>>,
    native_db_config: DbConfig,
    lake_compute_db_config: DbConfig,
    mdls_database: String,
    mdls_schema: String,
    project_name: Option<String>,
    invocation_id: String,
    propagation_database: String,
    propagation_quoting: ResolvedQuoting,
    replay: Option<&ReplayMode>,
    token: CancellationToken,
) -> FsResult<()> {
    // No adapter is built here: the `lake_compute` connection round trip is a
    // plain connection test the per-adapter loop above already ran for every
    // declared adapter including this one, and each check below builds the
    // adapter it needs itself (they each send their own catalog bundle). What
    // follows is only what is genuinely specific to lake compute.

    // 1. Declared-catalog attach. Runs before the write tests below because it
    // is the cheapest check that can fail on a misconfigured catalog, and a
    // catalog that cannot be attached makes everything after it moot.
    match &catalog_attach_checker {
        None => {
            emit_info_progress_message(create_progress_msg(
                ACTION_SKIPPED,
                "catalog attach test (unavailable in this build)",
            ));
        }
        Some(checker) => {
            let attach_started = Instant::now();
            let outcome = checker.check_catalog_attach(
                &native_db_config,
                &lake_compute_db_config,
                replay,
                token.clone(),
            )?;
            emit_info_progress_message(create_progress_msg(
                ACTION_DEBUGGING,
                &format!(
                    "{}{}",
                    format_catalog_attach_outcome(&outcome),
                    duration_suffix(attach_started.elapsed())
                ),
            ));
            if let Some(report) = pat_hygiene_of(&outcome) {
                for line in format_pat_hygiene_lines(report) {
                    emit_info_progress_message(create_progress_msg(ACTION_DEBUGGING, &line));
                }
            }
        }
    }

    // 2. MDLS write + read-back, in an already-authorized namespace (the lake compute
    // target's configured database/schema). Namespace-level DDL is
    // deliberately avoided: creating a new namespace is denied for the
    // Polaris principal used here and (confirmed empirically) can hang
    // rather than fail fast, so this only exercises table create/drop
    // within a namespace that must already exist.
    //
    // Behind a checker for one reason: the probe has to ride the same catalog
    // bundle a real model write sends, or it tests strictly less than a write
    // does and can pass while every write fails. Building one needs the
    // Snowflake credential mint, which this crate cannot reach.
    match &mdls_checker {
        None => {
            emit_info_progress_message(create_progress_msg(
                ACTION_SKIPPED,
                "MDLS write/read-back test (unavailable in this build)",
            ));
        }
        Some(checker) => {
            let outcome = checker.check_mdls_round_trip(
                &native_db_config,
                &lake_compute_db_config,
                replay,
                &mdls_database,
                &mdls_schema,
                project_name.as_deref(),
                &invocation_id,
                token.clone(),
            )?;
            for line in format_mdls_outcome(&outcome) {
                emit_info_progress_message(create_progress_msg(ACTION_DEBUGGING, &line));
            }
        }
    }

    // 3. Snowflake propagation: runs whenever a checker is registered. It
    // does not require a catalog-linked database declared in
    // `catalogs.yml` -- the checker builds its own throwaway catalog bundle
    // for the probe write.
    match &propagation_checker {
        None => {
            emit_info_progress_message(create_progress_msg(
                ACTION_SKIPPED,
                "Snowflake propagation test (unavailable in this build)",
            ));
        }
        Some(checker) => {
            emit_info_progress_message(create_progress_msg(
                ACTION_DEBUGGING,
                "Snowflake propagation test (this mints a short-lived credential and waits \
                 for dbt Compute to confirm the write is visible in Snowflake; can take up \
                 to a minute)...",
            ));
            let propagation_started = Instant::now();
            let outcome = checker.check_lake_compute_propagation(
                &native_db_config,
                &lake_compute_db_config,
                &propagation_database,
                &mdls_schema,
                propagation_quoting,
                replay,
                token,
            )?;
            let propagation_elapsed = propagation_started.elapsed();
            emit_info_progress_message(create_progress_msg(
                ACTION_DEBUGGING,
                &format!(
                    "{}{}",
                    format_propagation_outcome(&outcome),
                    duration_suffix(propagation_elapsed)
                ),
            ));
        }
    }

    Ok(())
}

/// The namespace the MDLS probe writes to: the lake compute adapter's own
/// `database`/`schema` when it sets them, otherwise the profile's.
///
/// A secondary adapter that configures neither inherits the target's defaults,
/// which `load_profiles` resolved from the *default* adapter -- the same
/// defaults the parser hands every node (`resolver.rs` reads
/// `dbt_profile.database`/`.schema` directly). Without this, a lake compute
/// output that relies on those defaults gets a partially-qualified reference,
/// which dbt Compute does not fold into the MDLS namespace at all.
fn resolve_probe_namespace<'a>(
    lake_compute_db_config: &'a DbConfig,
    profile_database: &'a str,
    profile_schema: &'a str,
) -> (&'a str, &'a str) {
    (
        lake_compute_db_config
            .get_database()
            .map(String::as_str)
            .unwrap_or(profile_database),
        lake_compute_db_config
            .get_schema()
            .map(String::as_str)
            .unwrap_or(profile_schema),
    )
}

/// Renders a [`LakeComputeMdlsOutcome`] as the `dbt debug` progress lines --
/// one per statement, as before, so the output reads as it always has. A
/// failed write or read-back never reaches here; it comes back as an error.
fn format_mdls_outcome(outcome: &LakeComputeMdlsOutcome) -> Vec<String> {
    // Worth calling out: with no Snowflake credential the probe cannot send a
    // `relation`, so the analyzer has to resolve the write target on its own
    // and this tests less than a real model write does.
    let bundle_note = if outcome.sent_propagation_bundle {
        ""
    } else {
        " (no propagation bundle sent)"
    };
    vec![
        format!(
            "MDLS write test: OK{}{}",
            bundle_note,
            duration_suffix(outcome.write_elapsed)
        ),
        format!(
            "MDLS read-back test: OK{}",
            duration_suffix(outcome.read_elapsed)
        ),
    ]
}

/// Renders an [`LakeComputeCatalogAttachOutcome`] as the `dbt debug` progress line.
/// A failed attach never reaches here -- it comes back as an error, since a
/// catalog that cannot be attached is a setup problem the user must fix.
fn format_catalog_attach_outcome(outcome: &LakeComputeCatalogAttachOutcome) -> String {
    match outcome {
        LakeComputeCatalogAttachOutcome::NothingToCheck { .. } => {
            "catalog attach test: skipped (no declared catalogs to check)".to_string()
        }
        LakeComputeCatalogAttachOutcome::MintedOnly {
            freshly_minted: true,
            ..
        } => "PAT mint test: OK (no declared catalogs to attach)".to_string(),
        LakeComputeCatalogAttachOutcome::MintedOnly {
            freshly_minted: false,
            ..
        } => "PAT mint test: skipped (reused a still-live cached PAT; no declared catalogs to attach)"
            .to_string(),
        LakeComputeCatalogAttachOutcome::Attached { catalogs, .. } => {
            format!("catalog attach test: OK ({})", catalogs.join(", "))
        }
    }
}

fn pat_hygiene_of(outcome: &LakeComputeCatalogAttachOutcome) -> Option<&PatHygieneReport> {
    match outcome {
        LakeComputeCatalogAttachOutcome::NothingToCheck { pat_hygiene }
        | LakeComputeCatalogAttachOutcome::MintedOnly { pat_hygiene, .. }
        | LakeComputeCatalogAttachOutcome::Attached { pat_hygiene, .. } => pat_hygiene.as_ref(),
    }
}

const PAT_CLEANUP_RECOMMENDATION_THRESHOLD: usize = 10;

fn format_pat_hygiene_lines(report: &PatHygieneReport) -> Vec<String> {
    let ttl_line = match report.cached_ttl_remaining_secs {
        Some(secs) => format!(
            "PAT hygiene: your current PAT has {} left",
            format_ttl_secs(secs)
        ),
        None => "PAT hygiene: no cached PAT yet".to_string(),
    };
    let total_line = format!(
        "This user has {} of {} total PATs on Snowflake.",
        report.live_token_count, report.cap
    );
    let untracked = report
        .live_dbt_compute_count
        .saturating_sub(report.in_filecache_count);
    let filecache_line = if untracked > 0 {
        format!(
            "This machine's local PAT cache tracks {} of {}. The remaining {untracked} may be from another machine, or left over from a cache that's since been cleared.",
            report.in_filecache_count, report.live_dbt_compute_count
        )
    } else {
        format!(
            "This machine's local PAT cache tracks {} of {}.",
            report.in_filecache_count, report.live_dbt_compute_count
        )
    };
    let mut lines = vec![ttl_line, total_line, filecache_line];
    if report.live_token_count > PAT_CLEANUP_RECOMMENDATION_THRESHOLD {
        lines.push(format!(
            "Consider dropping unused ones: run SHOW USER PROGRAMMATIC ACCESS TOKENS FOR USER {} to list names, then ALTER USER {} REMOVE PROGRAMMATIC ACCESS TOKEN <name-from-that-list>; for each one to drop",
            report.quoted_user, report.quoted_user
        ));
    }
    lines
}

fn format_ttl_secs(remaining_secs: i64) -> String {
    let days = remaining_secs.max(0) / (24 * 60 * 60);
    if days >= 1 {
        format!("{days}d")
    } else {
        format!("{}h", remaining_secs.max(0) / (60 * 60))
    }
}

/// Renders an [`LakeComputePropagationOutcome`] as the `dbt debug` progress line.
/// `NotYetVisible` is reported informationally, not as a failure, since
/// catalog-integration propagation is inherently asynchronous.
fn format_propagation_outcome(outcome: &LakeComputePropagationOutcome) -> String {
    match outcome {
        LakeComputePropagationOutcome::Verified => "Snowflake propagation test: OK".to_string(),
        LakeComputePropagationOutcome::NotYetVisible {
            waited_secs,
            configured_refresh_secs,
        } => {
            let refresh_note = configured_refresh_secs
                .map(|s| {
                    format!(" your catalog integration's refresh interval is configured at {s}s;")
                })
                .unwrap_or_default();
            format!(
                "Snowflake propagation test: not yet visible after {waited_secs}s.{refresh_note} this may just need more time, not necessarily a failure."
            )
        }
    }
}

async fn dependency_installed(dependency: &str) -> FsResult<bool> {
    Ok(Command::new(dependency)
        .arg("--help")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[dbt_runtime::test]
    async fn test_dependency_not_installed() {
        let result = dependency_installed("not_installed").await.unwrap();
        assert!(!result);
    }

    #[test]
    fn duration_suffix_hides_fast_steps() {
        assert_eq!(duration_suffix(Duration::from_millis(500)), "");
        assert_eq!(duration_suffix(Duration::from_secs(1)), "");
    }

    #[test]
    fn duration_suffix_shows_slow_steps() {
        assert_eq!(duration_suffix(Duration::from_millis(1500)), " (1.5s)");
        assert_eq!(duration_suffix(Duration::from_secs(90)), " (90.0s)");
    }

    #[test]
    fn adapter_label_is_empty_for_a_single_adapter_target() {
        assert_eq!(adapter_label(AdapterType::Snowflake, false), "");
    }

    #[test]
    fn adapter_label_names_the_adapter_when_several_are_declared() {
        assert_eq!(adapter_label(AdapterType::Snowflake, true), "snowflake ");
        assert_eq!(
            adapter_label(AdapterType::LakeCompute, true),
            "lakecompute "
        );
    }

    /// The VS Code extension decides whether to show the `allow_id_token` tip by
    /// testing whether a debug line *contains* one of these strings
    /// (`lsp/src/ExtensionManager.ts`). Prefixing the line with an adapter name is
    /// therefore safe, but only as long as the substring itself is untouched --
    /// which is what this pins.
    #[test]
    fn the_adapter_label_does_not_break_the_lsp_substring_match() {
        let line = format!(
            "{}externalbrowser connection caching: Disabled. Consider enabling...",
            adapter_label(AdapterType::Snowflake, true)
        );
        assert!(line.contains("externalbrowser connection caching: Disabled"));
    }

    /// A lake compute output that configures neither `database` nor `schema`
    /// inherits the target's defaults, exactly as a node does. Without this the
    /// probe sends a partially-qualified reference, which dbt Compute leaves
    /// unfolded -- surfacing as an opaque "Catalog ... does not exist".
    #[test]
    fn probe_namespace_falls_back_to_the_profile_defaults() {
        let config = DbConfig::LakeCompute(Box::default());
        assert_eq!(
            resolve_probe_namespace(&config, "profile_db", "profile_sch"),
            ("profile_db", "profile_sch")
        );
    }

    /// The adapter's own values win when it sets them.
    #[test]
    fn probe_namespace_prefers_the_adapters_own_values() {
        let config = DbConfig::LakeCompute(Box::new(
            dbt_schemas::schemas::profiles::LakeComputeConfig {
                database: Some("adapter_db".to_string()),
                schema: Some("adapter_sch".to_string()),
                ..Default::default()
            },
        ));
        assert_eq!(
            resolve_probe_namespace(&config, "profile_db", "profile_sch"),
            ("adapter_db", "adapter_sch")
        );
    }

    /// Both statements get their own line, in order, as they did when this
    /// check ran inline -- the checker now returns after both have run, so the
    /// lines are emitted together rather than one per statement.
    #[test]
    fn format_mdls_outcome_reports_both_statements() {
        let lines = format_mdls_outcome(&LakeComputeMdlsOutcome {
            write_elapsed: Duration::from_millis(1500),
            read_elapsed: Duration::from_millis(200),
            sent_propagation_bundle: true,
        });
        assert_eq!(
            lines,
            vec!["MDLS write test: OK (1.5s)", "MDLS read-back test: OK"]
        );
    }

    /// A run that could not send a `relation` bundle tests strictly less than a
    /// real model write does, so the output has to say so.
    #[test]
    fn format_mdls_outcome_flags_a_run_with_no_propagation_bundle() {
        let lines = format_mdls_outcome(&LakeComputeMdlsOutcome {
            write_elapsed: Duration::ZERO,
            read_elapsed: Duration::ZERO,
            sent_propagation_bundle: false,
        });
        assert_eq!(lines[0], "MDLS write test: OK (no propagation bundle sent)");
    }

    #[test]
    fn format_catalog_attach_outcome_lists_checked_catalogs() {
        let msg = format_catalog_attach_outcome(&LakeComputeCatalogAttachOutcome::Attached {
            catalogs: vec!["mdls_horizon".to_string(), "native_db".to_string()],
            pat_hygiene: None,
        });
        assert_eq!(msg, "catalog attach test: OK (mdls_horizon, native_db)");
    }

    #[test]
    fn format_catalog_attach_outcome_nothing_to_check() {
        let msg = format_catalog_attach_outcome(&LakeComputeCatalogAttachOutcome::NothingToCheck {
            pat_hygiene: None,
        });
        assert!(msg.contains("no declared catalogs to check"));
    }

    /// A project with no declared catalogs still reports the mint, since that
    /// is the part its first write depends on.
    #[test]
    fn format_catalog_attach_outcome_minted_only_reports_a_fresh_mint_as_ok() {
        let msg = format_catalog_attach_outcome(&LakeComputeCatalogAttachOutcome::MintedOnly {
            pat_hygiene: None,
            freshly_minted: true,
        });
        assert_eq!(msg, "PAT mint test: OK (no declared catalogs to attach)");
    }

    /// A cache hit never ran the mint DDL, so it must not be reported as
    /// having verified it.
    #[test]
    fn format_catalog_attach_outcome_minted_only_does_not_claim_ok_on_a_cache_hit() {
        let msg = format_catalog_attach_outcome(&LakeComputeCatalogAttachOutcome::MintedOnly {
            pat_hygiene: None,
            freshly_minted: false,
        });
        assert!(msg.starts_with("PAT mint test: skipped"));
        assert!(msg.contains("cached PAT"));
    }

    #[test]
    fn format_pat_hygiene_lines_reports_ttl_total_and_filecache_when_fully_tracked() {
        let lines = format_pat_hygiene_lines(&PatHygieneReport {
            quoted_user: "\"DBT_USER\"".to_string(),
            cached_ttl_remaining_secs: Some(3 * 24 * 60 * 60),
            live_token_count: 2,
            live_dbt_compute_count: 2,
            in_filecache_count: 2,
            cap: 15,
        });
        assert_eq!(
            lines,
            vec![
                "PAT hygiene: your current PAT has 3d left".to_string(),
                "This user has 2 of 15 total PATs on Snowflake.".to_string(),
                "This machine's local PAT cache tracks 2 of 2.".to_string(),
            ]
        );
    }

    #[test]
    fn format_pat_hygiene_lines_reports_untracked_tokens_below_the_threshold() {
        let lines = format_pat_hygiene_lines(&PatHygieneReport {
            quoted_user: "\"DBT_USER\"".to_string(),
            cached_ttl_remaining_secs: Some(3 * 24 * 60 * 60),
            live_token_count: 2,
            live_dbt_compute_count: 2,
            in_filecache_count: 1,
            cap: 15,
        });
        assert_eq!(lines.len(), 3);
        assert!(lines[2].contains("This machine's local PAT cache tracks 1 of 2."));
        assert!(lines[2].contains("The remaining 1 may be from another machine"));
        assert!(!lines.iter().any(|line| line.contains("ALTER USER")));
    }

    #[test]
    fn format_pat_hygiene_lines_recommends_cleanup_above_the_threshold() {
        let lines = format_pat_hygiene_lines(&PatHygieneReport {
            quoted_user: "\"DBT_USER\"".to_string(),
            cached_ttl_remaining_secs: None,
            live_token_count: 13,
            live_dbt_compute_count: 13,
            in_filecache_count: 1,
            cap: 15,
        });
        assert!(lines[0].contains("no cached PAT yet"));
        assert!(lines[1].contains("This user has 13 of 15 total PATs on Snowflake."));
        assert_eq!(lines.len(), 4);
        assert!(lines[2].contains("This machine's local PAT cache tracks 1 of 13."));
        assert!(lines[2].contains("The remaining 12 may be from another machine"));
        assert!(lines[3].contains(
            "SHOW USER PROGRAMMATIC ACCESS TOKENS FOR USER \"DBT_USER\" to list names, then ALTER USER \"DBT_USER\" REMOVE PROGRAMMATIC ACCESS TOKEN"
        ));
    }

    #[test]
    fn format_pat_hygiene_lines_cap_count_is_account_wide_not_dbt_compute_only() {
        // 20 total PATs on the account (over cap), only 2 are dbt-compute's --
        // the cap line must use the account-wide count, the filecache line
        // must use the dbt-compute-only count.
        let lines = format_pat_hygiene_lines(&PatHygieneReport {
            quoted_user: "\"DBT_USER\"".to_string(),
            cached_ttl_remaining_secs: Some(3 * 24 * 60 * 60),
            live_token_count: 20,
            live_dbt_compute_count: 2,
            in_filecache_count: 2,
            cap: 15,
        });
        assert!(lines[1].contains("This user has 20 of 15 total PATs on Snowflake."));
        assert!(lines[2].contains("This machine's local PAT cache tracks 2 of 2."));
    }

    #[test]
    fn format_propagation_outcome_verified() {
        assert_eq!(
            format_propagation_outcome(&LakeComputePropagationOutcome::Verified),
            "Snowflake propagation test: OK"
        );
    }

    #[test]
    fn format_propagation_outcome_not_yet_visible_with_refresh_interval() {
        let msg = format_propagation_outcome(&LakeComputePropagationOutcome::NotYetVisible {
            waited_secs: 90,
            configured_refresh_secs: Some(3600),
        });
        assert!(msg.contains("not yet visible after 90s"));
        assert!(msg.contains("refresh interval is configured at 3600s"));
        assert!(msg.contains("not necessarily a failure"));
    }

    #[test]
    fn format_propagation_outcome_not_yet_visible_without_refresh_interval() {
        let msg = format_propagation_outcome(&LakeComputePropagationOutcome::NotYetVisible {
            waited_secs: 90,
            configured_refresh_secs: None,
        });
        assert!(msg.contains("not yet visible after 90s"));
        assert!(!msg.contains("refresh interval"));
    }
}
