use std::time::Duration;

use dbt_common::FsResult;
use dbt_common::cancellation::CancellationToken;
use dbt_common::io_args::ReplayMode;
use dbt_schemas::schemas::profiles::DbConfig;

/// Outcome of a successful MDLS write + read-back round trip. Any failure of
/// either statement comes back as an error instead: a target that cannot be
/// written to is a setup problem the user must fix.
#[derive(Debug, Clone)]
pub struct LakeComputeMdlsOutcome {
    pub write_elapsed: Duration,
    pub read_elapsed: Duration,
    /// Whether the probe rode a `relation` bundle. False means no Snowflake
    /// credential was available, so it named the bearer-token namespace
    /// directly -- a weaker check, worth telling apart in the output.
    pub sent_propagation_bundle: bool,
}

/// Extension point for the `dbt debug` MDLS write + read-back check: create a
/// throwaway table in the lake compute target's own namespace, read it back,
/// and drop it.
///
/// This lives behind a trait because the probe has to ride the same
/// `relation` bundle a real model write sends -- without it the analyzer
/// sidecar cannot fold a not-yet-existing CREATE target into the MDLS
/// namespace -- and building one means minting a short-lived Snowflake
/// credential, which is not reachable from this crate. A build that doesn't support the check simply doesn't register
/// an implementation.
pub trait LakeComputeMdlsChecker: Send + Sync {
    /// `native_db_config` is the profile's active target, used to mint the
    /// credential the bundle's declared catalogs authenticate with; a
    /// non-Snowflake target mints nothing and the probe runs bundle-less.
    /// `lake_compute_db_config` is the target being written to.
    ///
    /// `database` and `schema` are the probe's target namespace, already
    /// resolved by the caller: the lake compute adapter's own values when it
    /// sets them, otherwise the profile's defaults (which `load_profiles`
    /// derives from the target's default adapter) -- the same fallback the
    /// parser applies to every node.
    ///
    /// `project_name` and `invocation_id` travel with each statement as
    /// `adbc.dbt.project` / `adbc.dbt.run_id` so dbt Compute can attribute the
    /// probe queries. `project_name` is `None` when no package has been loaded
    /// -- `dbt debug` and `dbt init` both run before that happens -- and the
    /// project option is then simply not sent.
    fn check_mdls_round_trip(
        &self,
        native_db_config: &DbConfig,
        lake_compute_db_config: &DbConfig,
        replay: Option<&ReplayMode>,
        database: &str,
        schema: &str,
        project_name: Option<&str>,
        invocation_id: &str,
        token: CancellationToken,
    ) -> FsResult<LakeComputeMdlsOutcome>;
}
