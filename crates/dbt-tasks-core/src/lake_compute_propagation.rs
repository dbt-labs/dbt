use dbt_common::FsResult;
use dbt_common::cancellation::CancellationToken;
use dbt_common::io_args::ReplayMode;
use dbt_schemas::schemas::common::ResolvedQuoting;
use dbt_schemas::schemas::profiles::DbConfig;

/// Outcome of verifying that a write made through a lake compute
/// target has become visible via the profile's native connection (e.g. a
/// linked-catalog round trip).
#[derive(Debug, Clone)]
pub enum LakeComputePropagationOutcome {
    /// The probe object was visible through the native connection.
    Verified,
    /// The probe object was not yet visible after waiting. Not necessarily a
    /// failure: propagation to the native connection can be asynchronous.
    NotYetVisible {
        waited_secs: u64,
        configured_refresh_secs: Option<u64>,
    },
}

/// Extension point for verifying propagation between a lake compute
/// target and the profile's native connection during `dbt debug`. A build
/// that doesn't support this check simply doesn't register an implementation
/// (see `lake_compute_propagation_checker()` on the CLI hooks it's wired through).
pub trait LakeComputePropagationChecker: Send + Sync {
    // `probe_database` is the Snowflake database the probe write should
    // become visible in via the native connection. It does not need to be a
    // pre-declared catalog-linked database (`catalogs.yml`'s
    // `catalog_database` / `catalog_linked_database`): the checker builds
    // its own throwaway catalog bundle for the probe, so any database the
    // lake compute target can write to and the native connection can read
    // from works.
    fn check_lake_compute_propagation(
        &self,
        native_db_config: &DbConfig,
        lake_compute_db_config: &DbConfig,
        probe_database: &str,
        probe_schema: &str,
        // The quoting policy a real lake compute model targeting this
        // database/schema would resolve to (project `quoting:` config filled
        // in with adapter defaults) -- passed in rather than defaulted here
        // so the probe's relation is quoted exactly like a real write's.
        probe_quoting: ResolvedQuoting,
        replay: Option<&ReplayMode>,
        token: CancellationToken,
    ) -> FsResult<LakeComputePropagationOutcome>;
}
