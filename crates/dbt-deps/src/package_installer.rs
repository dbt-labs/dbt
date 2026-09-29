//! Concurrent package installation: one trait impl per unpinned package kind.

use std::path::Path;

use dbt_common::tracing::dbt_emit::emit_info_log_message;
use dbt_common::tracing::span_info::{SpanStatusRecorder as _, update_span_attrs};
use dbt_common::{ErrorCode, FsResult, create_info_span, fs_err, stdfs, tokiofs};
use dbt_telemetry::{DepsPackageInstalled, PackageType};
use tracing::Instrument as _;
use vortex_events::package_install_event;

use crate::context::DepsOperationContext;
use crate::git_client::install_git_like_package;
use crate::hub_client::HubPackageVersion;
use crate::package_listing::UnpinnedPackage;
use crate::types::{
    GitUnpinnedPackage, HubUnpinnedPackage, LocalUnpinnedPackage, PrivateUnpinnedPackage,
    TarballUnpinnedPackage,
};
use crate::utils::{
    ensure_dir, make_tempdir, move_dir, read_and_validate_dbt_project, sanitize_git_url,
};

/// Discovered name/version, populated as install progresses so spans keep
/// partial progress on error.
#[derive(Default)]
pub(crate) struct InstallOutcome {
    pub name: Option<String>,
    pub version: Option<String>,
}

pub(crate) trait PackageInstaller {
    fn span_attrs(&self) -> DepsPackageInstalled;

    /// Telemetry kind ("hub", "git", ...).
    fn telemetry_kind(&self) -> &'static str;

    /// Stamp populated outcome fields onto span attrs.
    fn update_span_end(&self, out: &InstallOutcome, ev: &mut DepsPackageInstalled) {
        if let Some(n) = &out.name {
            ev.package_name = Some(n.clone());
        }
        if let Some(v) = &out.version {
            ev.package_version = Some(v.clone());
        }
    }

    /// Install work. Populate `out` as fields become known.
    async fn install_inner(
        &self,
        ctx: &DepsOperationContext<'_>,
        dest: &Path,
        out: &mut InstallOutcome,
    ) -> FsResult<()>;

    /// Owns span, telemetry, status recording, and `M016` finalization.
    async fn install(&self, ctx: &DepsOperationContext<'_>, dest: &Path) -> FsResult<()> {
        ctx.check_cancellation()?;
        let attrs = self.span_attrs();
        let span = create_info_span(attrs);

        let mut outcome = InstallOutcome::default();
        let inner = async { self.install_inner(ctx, dest, &mut outcome).await }
            .instrument(span.clone())
            .await;

        update_span_attrs(&span, |ev: &mut DepsPackageInstalled| {
            self.update_span_end(&outcome, ev);
        });

        let result = inner.record_status(&span);

        if result.is_ok() && ctx.io.send_anonymous_usage_stats {
            package_install_event(
                ctx.io.invocation_id.to_string(),
                outcome.name.unwrap_or_default(),
                outcome.version.unwrap_or_default(),
                self.telemetry_kind().to_string(),
            );
        }

        update_span_attrs(&span, |ev: &mut DepsPackageInstalled| {
            ev.dbt_core_event_code = "M016".to_string();
        });

        result
    }
}

impl PackageInstaller for HubUnpinnedPackage {
    fn span_attrs(&self) -> DepsPackageInstalled {
        DepsPackageInstalled::start(Some(self.package.clone()), PackageType::Hub, None, None)
    }

    fn telemetry_kind(&self) -> &'static str {
        "hub"
    }

    /// Hub keeps span `package_name` as the unpinned id; stamps version only.
    fn update_span_end(&self, out: &InstallOutcome, ev: &mut DepsPackageInstalled) {
        if let Some(v) = &out.version {
            ev.package_version = Some(v.clone());
        }
    }

    async fn install_inner(
        &self,
        ctx: &DepsOperationContext<'_>,
        dest: &Path,
        out: &mut InstallOutcome,
    ) -> FsResult<()> {
        let resolved = self.resolved(&ctx.hub_registry).await?;
        let pinned = &resolved.pinned;
        let metadata = &resolved.version;

        ctx.notices.collect(pinned);

        // Stamp now so the span keeps version even if the download fails.
        out.name = Some(pinned.name.clone());
        out.version = Some(pinned.version.clone());

        let (tarball_url, expected_sha1) = select_hub_download(
            ctx.use_v2_compatible_package_downloads,
            ctx.require_hub_verified_downloads,
            metadata,
            &pinned.name,
            &pinned.version,
        )?;

        let final_path = dest.join(&metadata.name);
        ensure_dir(&final_path).await?;

        if let Err(e) = ctx
            .tarball_client
            .download_and_extract_tarball(
                &tarball_url,
                &final_path,
                true,
                None,
                &[],
                expected_sha1.as_deref(),
            )
            .await
        {
            let _ = tokiofs::remove_dir_all(&final_path).await;
            return Err(e);
        }

        Ok(())
    }
}

/// Select the tarball URL and the sha1 to verify it against.
///
/// `downloads.sha1` describes the upstream tarball, whose gzip output is not
/// byte-stable, so it's not a usable substitute for the mirror's own sha1.
fn select_hub_download(
    use_v2_compatible_package_downloads: bool,
    require_hub_verified_downloads: bool,
    metadata: &HubPackageVersion,
    name: &str,
    version: &str,
) -> FsResult<(String, Option<String>)> {
    let (tarball_url, sha1) = if use_v2_compatible_package_downloads
        && let Some(fusion_compatibility) = &metadata.fusion_compatibility
        && let Some(hub_fusion_compatible_download) =
            &fusion_compatibility.fusion_compatible_download
        && let Some(fusion_compatible_download_url) = &hub_fusion_compatible_download.tarball
    {
        emit_info_log_message(format!(
            "Installing the v2-compatible download from Package Hub for {name}@{version}",
        ));
        (
            fusion_compatible_download_url.clone(),
            hub_fusion_compatible_download.sha1.clone(),
        )
    } else if require_hub_verified_downloads {
        let hub_mirror = metadata.downloads.hub.as_ref().ok_or_else(|| {
            fs_err!(
                ErrorCode::RuntimeError,
                "Package Hub did not provide a verifiable mirror download for {name}@{version}, \
                 but require_hub_verified_downloads is enabled. Disable the flag to install the \
                 unverified upstream tarball instead."
            )
        })?;
        (hub_mirror.tarball.clone(), hub_mirror.sha1.clone())
    } else {
        (metadata.downloads.tarball.clone(), None)
    };

    if !require_hub_verified_downloads {
        return Ok((tarball_url, None));
    }
    let sha1 = sha1.ok_or_else(|| {
        fs_err!(
            ErrorCode::RuntimeError,
            "Package Hub did not provide a sha1 checksum for {name}@{version}, but \
             require_hub_verified_downloads is enabled. Disable the flag to install without \
             checksum verification."
        )
    })?;
    Ok((tarball_url, Some(sha1)))
}

impl PackageInstaller for GitUnpinnedPackage {
    fn span_attrs(&self) -> DepsPackageInstalled {
        DepsPackageInstalled::start(
            None,
            PackageType::Git,
            None,
            Some(sanitize_git_url(self.git.as_str())),
        )
    }

    fn telemetry_kind(&self) -> &'static str {
        "git"
    }

    async fn install_inner(
        &self,
        ctx: &DepsOperationContext<'_>,
        dest: &Path,
        out: &mut InstallOutcome,
    ) -> FsResult<()> {
        install_git_like(
            ctx,
            dest,
            &self.git,
            &self.revisions,
            &self.subdirectory,
            out,
        )
        .await
    }
}

impl PackageInstaller for PrivateUnpinnedPackage {
    fn span_attrs(&self) -> DepsPackageInstalled {
        DepsPackageInstalled::start(
            None,
            PackageType::Private,
            None,
            Some(sanitize_git_url(&self.private)),
        )
    }

    fn telemetry_kind(&self) -> &'static str {
        "private"
    }

    async fn install_inner(
        &self,
        ctx: &DepsOperationContext<'_>,
        dest: &Path,
        out: &mut InstallOutcome,
    ) -> FsResult<()> {
        install_git_like(
            ctx,
            dest,
            &self.private,
            &self.revisions,
            &self.subdirectory,
            out,
        )
        .await?;
        if let Some(name) = self.name.clone() {
            out.name = Some(name);
        }
        Ok(())
    }
}

impl PackageInstaller for LocalUnpinnedPackage {
    fn span_attrs(&self) -> DepsPackageInstalled {
        DepsPackageInstalled::start(
            self.name.clone(),
            PackageType::Local,
            None,
            Some(sanitize_git_url(&self.local.to_string_lossy())),
        )
    }

    fn telemetry_kind(&self) -> &'static str {
        "local"
    }

    async fn install_inner(
        &self,
        ctx: &DepsOperationContext<'_>,
        dest: &Path,
        out: &mut InstallOutcome,
    ) -> FsResult<()> {
        let package_path = ctx.io.in_dir.join(&self.local);
        let install_path = dest.join(self.name.as_ref().unwrap());
        let relative_package_path = stdfs::diff_paths(&package_path, dest)?;
        stdfs::symlink(&relative_package_path, &install_path)?;
        out.name = Some(
            self.name
                .clone()
                .unwrap_or_else(|| package_path.display().to_string()),
        );
        Ok(())
    }
}

impl PackageInstaller for TarballUnpinnedPackage {
    fn span_attrs(&self) -> DepsPackageInstalled {
        DepsPackageInstalled::start(
            None,
            PackageType::Tarball,
            None,
            Some(sanitize_git_url(&self.tarball)),
        )
    }

    fn telemetry_kind(&self) -> &'static str {
        "tarball"
    }

    async fn install_inner(
        &self,
        ctx: &DepsOperationContext<'_>,
        dest: &Path,
        out: &mut InstallOutcome,
    ) -> FsResult<()> {
        // Same filesystem as `dest` so the final rename is atomic.
        let tmp_extract = make_tempdir(Some(dest))?;
        let extract_path = tmp_extract.path().join("package");
        ensure_dir(&extract_path).await?;

        if let Err(e) = ctx
            .tarball_client
            .download_and_extract_tarball(&self.tarball, &extract_path, true, None, &[], None)
            .await
        {
            let _ = tokiofs::remove_dir_all(&extract_path).await;
            return Err(e);
        }

        let dbt_project =
            read_and_validate_dbt_project(&extract_path, false, ctx.jinja_env, ctx.vars).await?;
        let project_name = dbt_project.name;
        out.name = Some(project_name.clone());
        out.version = Some("tarball".to_string());
        move_dir(&extract_path, &dest.join(&project_name)).await?;

        Ok(())
    }
}

/// Shared install path for git + private packages.
async fn install_git_like(
    ctx: &DepsOperationContext<'_>,
    dest: &Path,
    repo_url: &str,
    revisions: &[String],
    subdirectory: &Option<String>,
    out: &mut InstallOutcome,
) -> FsResult<()> {
    let tmp_dir = make_tempdir(Some(dest))?;
    let download_dir = tmp_dir.path().join("git_pkg");
    ensure_dir(&download_dir).await?;
    let sha = revisions.last().cloned().unwrap_or_default();
    let (checkout_path, commit_sha) =
        install_git_like_package(ctx, repo_url, &sha, subdirectory, &download_dir).await?;
    out.version = Some(commit_sha);

    // Warnings already emitted during resolve; suppress here.
    let dbt_project =
        read_and_validate_dbt_project(&checkout_path, false, ctx.jinja_env, ctx.vars).await?;
    out.name = Some(dbt_project.name.clone());
    move_dir(&checkout_path, &dest.join(&dbt_project.name)).await?;
    drop(tmp_dir);

    Ok(())
}

impl UnpinnedPackage {
    pub(crate) async fn install(
        &self,
        ctx: &DepsOperationContext<'_>,
        dest: &Path,
    ) -> FsResult<()> {
        match self {
            UnpinnedPackage::Hub(p) => p.install(ctx, dest).await,
            UnpinnedPackage::Git(p) => p.install(ctx, dest).await,
            UnpinnedPackage::Local(p) => p.install(ctx, dest).await,
            UnpinnedPackage::Private(p) => p.install(ctx, dest).await,
            UnpinnedPackage::Tarball(p) => p.install(ctx, dest).await,
        }
    }
}

#[cfg(test)]
mod select_hub_download_tests {
    use super::*;
    use crate::hub_client::{
        FusionHubPackageDownloads, HubPackageDownloads, HubPackageFusionCompatibility,
        HubPackageVersion,
    };

    const ORIGINAL_URL: &str = "https://codeload.github.com/org/pkg/tar.gz/1.0.0";
    const HUB_MIRROR_URL: &str = "https://mirror.example-cdn.com/packages/org/pkg/tar.gz/1.0.0";
    const FUSION_URL: &str = "https://codeload.github.com/org/pkg/tar.gz/1.0.0-fusion";
    const HUB_SHA1: &str = "1111111111111111111111111111111111111111";
    const FUSION_SHA1: &str = "2222222222222222222222222222222222222222";

    /// Outer `None` omits the download; inner `None` omits just its sha1.
    fn metadata(
        hub_mirror_sha1: Option<Option<&str>>,
        fusion_sha1: Option<Option<&str>>,
    ) -> HubPackageVersion {
        HubPackageVersion {
            name: "pkg".to_string(),
            downloads: HubPackageDownloads {
                tarball: ORIGINAL_URL.to_string(),
                hub: hub_mirror_sha1.map(|sha1| {
                    Box::new(HubPackageDownloads {
                        tarball: HUB_MIRROR_URL.to_string(),
                        sha1: sha1.map(str::to_string),
                        hub: None,
                    })
                }),
                ..Default::default()
            },
            fusion_compatibility: fusion_sha1.map(|sha1| HubPackageFusionCompatibility {
                fusion_compatible_download: Some(FusionHubPackageDownloads {
                    tarball: Some(FUSION_URL.to_string()),
                    sha1: sha1.map(str::to_string),
                }),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn select(
        use_v2: bool,
        require_hub_verified: bool,
        metadata: &HubPackageVersion,
    ) -> FsResult<(String, Option<String>)> {
        select_hub_download(use_v2, require_hub_verified, metadata, "pkg", "1.0.0")
    }

    #[test]
    fn verified_no_fusion_uses_hub_mirror_and_its_sha1() {
        let metadata = metadata(Some(Some(HUB_SHA1)), None);
        assert_eq!(
            select(false, true, &metadata).unwrap(),
            (HUB_MIRROR_URL.to_string(), Some(HUB_SHA1.to_string()))
        );
    }

    #[test]
    fn verified_fusion_available_uses_fusion_url_and_sha1() {
        let metadata = metadata(Some(Some(HUB_SHA1)), Some(Some(FUSION_SHA1)));
        assert_eq!(
            select(true, true, &metadata).unwrap(),
            (FUSION_URL.to_string(), Some(FUSION_SHA1.to_string()))
        );
    }

    #[test]
    fn verified_fusion_without_sha1_is_an_error() {
        let metadata = metadata(Some(Some(HUB_SHA1)), Some(None));
        let err = select(true, true, &metadata).unwrap_err();
        assert!(
            err.to_string().contains("did not provide a sha1 checksum"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn verified_hub_mirror_without_sha1_is_an_error() {
        let metadata = metadata(Some(None), None);
        let err = select(false, true, &metadata).unwrap_err();
        assert!(
            err.to_string().contains("did not provide a sha1 checksum"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn verified_without_hub_mirror_is_an_error() {
        let metadata = metadata(None, None);
        let err = select(false, true, &metadata).unwrap_err();
        assert!(
            err.to_string()
                .contains("did not provide a verifiable mirror download"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn unverified_fusion_available_uses_fusion_url_without_sha1() {
        let metadata = metadata(Some(Some(HUB_SHA1)), Some(Some(FUSION_SHA1)));
        assert_eq!(
            select(true, false, &metadata).unwrap(),
            (FUSION_URL.to_string(), None)
        );
    }

    #[test]
    fn unverified_no_fusion_uses_original_url_without_sha1() {
        let metadata = metadata(Some(Some(HUB_SHA1)), None);
        assert_eq!(
            select(false, false, &metadata).unwrap(),
            (ORIGINAL_URL.to_string(), None)
        );
    }
}
