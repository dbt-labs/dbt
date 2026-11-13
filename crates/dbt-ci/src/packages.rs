//! Fetch pinned optional packages and stage release and wheel payloads.

use crate::utils::{require_https, sha256_hex};
use anyhow::{Context, Result, bail};
use clap::Args;
use dbt_dist::packages::PackageManifest;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Args, Debug)]
pub struct PackagesArgs {
    #[arg(long)]
    pub target: String,
    /// Release tag version without the `v` (PEP 440, e.g. `2.1.0a2`); names the output files.
    #[arg(long)]
    pub version: String,
    #[arg(long, default_value = "https://public.cdn.getdbt.com")]
    pub cdn_url: String,
    /// Receives the libraries for CLI archives; archive it as `lib/dbt/`.
    #[arg(long)]
    pub staging_dir: PathBuf,
    /// Wheel data directory (maturin `data`); libraries land in `<dir>/data/lib/dbt/`.
    #[arg(long)]
    pub wheel_data_dir: Option<PathBuf>,
    #[arg(long, default_value = ".")]
    pub out_dir: PathBuf,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ReleasePackages {
    pub target: String,
    pub packages: Vec<ReleasePackage>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ReleasePackage {
    pub name: String,
    pub install_as: String,
    pub channels: Vec<String>,
    pub filename: String,
    pub sha256: String,
}

pub fn execute(args: PackagesArgs) -> ExitCode {
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("create package download runtime")
        .and_then(|runtime| runtime.block_on(stage(args)));
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

async fn stage(args: PackagesArgs) -> Result<()> {
    require_https(&args.cdn_url)?;
    validate_filename_version(&args.version)?;
    let manifest =
        PackageManifest::bundled().map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let client = reqwest::Client::new();
    // A reused runner may hold stale files.
    if args.staging_dir.exists() {
        fs::remove_dir_all(&args.staging_dir)?;
    }
    fs::create_dir_all(&args.staging_dir)?;
    fs::create_dir_all(&args.out_dir)?;
    let mut release = ReleasePackages {
        target: args.target.clone(),
        packages: Vec::new(),
    };
    for package in manifest.package {
        let Some(checksum) = package.checksums.get(&args.target) else {
            continue;
        };
        let url = format!(
            "{}/{}",
            args.cdn_url.trim_end_matches('/'),
            package.cdn_path(&args.target)
        );
        let bytes = client
            .get(&url)
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;
        if sha256_hex(&bytes) != *checksum {
            bail!("checksum mismatch for {} ({})", package.name, args.target);
        }
        let temp = tempfile::tempdir()?;
        let size = zstd_safe::get_frame_content_size(&bytes)
            .map_err(|error| anyhow::anyhow!("invalid zstd frame: {error:?}"))?
            .context("zstd payload has no declared size")?;
        if size > 512 * 1024 * 1024 {
            bail!("optional package exceeds 512 MiB");
        }
        let mut decoded = vec![0; usize::try_from(size)?];
        let written = zstd_safe::DCtx::create()
            .decompress(decoded.as_mut_slice(), bytes.as_ref())
            .map_err(|code| anyhow::anyhow!("zstd decode: {}", zstd_safe::get_error_name(code)))?;
        decoded.truncate(written);
        let install_as = package.filename(&args.target);
        let library = temp.path().join(&install_as);
        fs::write(&library, &decoded)?;
        if package
            .channels
            .iter()
            .any(|channel| matches!(channel.as_str(), "standalone" | "native-update" | "winget"))
        {
            fs::copy(&library, args.staging_dir.join(&install_as))?;
        }
        if package.channels.iter().any(|channel| channel == "pypi") {
            if let Some(root) = &args.wheel_data_dir {
                let destination = root
                    .join("data")
                    .join(dbt_dist::packages::LIBRARY_DIRECTORY);
                fs::create_dir_all(&destination)?;
                fs::copy(&library, destination.join(&install_as))?;
            }
        }
        // Homebrew unpacks tar.gz natively.
        let filename = format!(
            "fs-package-{}-v{}-{}.tar.gz",
            package.name, args.version, args.target
        );
        let archive_path = args.out_dir.join(&filename);
        let output = fs::File::create(&archive_path)?;
        let encoder = flate2::write::GzEncoder::new(output, flate2::Compression::default());
        let mut archive = tar::Builder::new(encoder);
        let mut header = tar::Header::new_gnu();
        header.set_size(decoded.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(0);
        header.set_cksum();
        archive.append_data(&mut header, &install_as, decoded.as_slice())?;
        archive.into_inner()?.finish()?;
        release.packages.push(ReleasePackage {
            name: package.name,
            install_as,
            channels: package.channels,
            filename,
            sha256: sha256_hex(&fs::read(&archive_path)?),
        });
    }
    fs::write(
        args.out_dir.join(format!(
            "fs-packages-v{}-{}.json",
            args.version, args.target
        )),
        serde_json::to_vec_pretty(&release)?,
    )?;
    Ok(())
}

/// The version only names output files, which consumers look up by the release tag.
fn validate_filename_version(version: &str) -> Result<()> {
    let well_formed = version.starts_with(|c: char| c.is_ascii_digit())
        && version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'));
    if !well_formed {
        bail!("invalid --version {version:?}: expected a release tag version such as 2.1.0a2");
    }
    Ok(())
}
