use dbt_common::FsResult;
use dbt_common::cancellation::CancellationToken;
use dbt_common::io_args::ReplayMode;
use dbt_schemas::schemas::profiles::DbConfig;

#[derive(Debug, Clone)]
pub struct PatHygieneReport {
    pub quoted_user: String,
    pub cached_ttl_remaining_secs: Option<i64>,
    pub live_token_count: usize,
    pub live_dbt_compute_count: usize,
    pub in_filecache_count: usize,
    pub cap: usize,
}

/// Outcome of asking a lake compute target to attach the catalogs a
/// project declares in `catalogs.yml`.
#[derive(Debug, Clone)]
pub enum LakeComputeCatalogAttachOutcome {
    /// Every declared catalog attached. Carries the catalog names that were
    /// checked, in declaration order, so the caller can name them in its
    /// output.
    Attached {
        catalogs: Vec<String>,
        pat_hygiene: Option<PatHygieneReport>,
    },
    /// The project declares no catalogs this check applies to, so no attach
    /// was attempted -- but the credential every write needs was obtained
    /// first, so that much is verified. Not a failure.
    MintedOnly {
        pat_hygiene: Option<PatHygieneReport>,
        /// False when a still-live cached credential was reused, meaning the
        /// mint itself was not exercised and so is not what this verified.
        freshly_minted: bool,
    },
    /// The project declares no catalogs this check applies to and the target
    /// mints no credential either, so nothing was attempted. Not a failure.
    NothingToCheck {
        pat_hygiene: Option<PatHygieneReport>,
    },
}

/// Extension point for checking, during `dbt debug`, that the catalogs a
/// project declares are reachable and authorized from a lake compute
/// target -- a cheaper, earlier failure than exercising a full write. A build
/// that doesn't support this check simply doesn't register an implementation
/// (see `lake_compute_catalog_attach_checker()` on the CLI hooks it's wired through).
pub trait LakeComputeCatalogAttachChecker: Send + Sync {
    /// `native_db_config` is the profile's active target, used to obtain a
    /// short-lived credential for the declared catalogs; `lake_compute_db_config` is
    /// the target asked to perform the attach.
    fn check_catalog_attach(
        &self,
        native_db_config: &DbConfig,
        lake_compute_db_config: &DbConfig,
        replay: Option<&ReplayMode>,
        token: CancellationToken,
    ) -> FsResult<LakeComputeCatalogAttachOutcome>;
}
