//! Assembly of the download-at-install sdist: a tiny `.tar.gz` carrying the
//! embedded PEP 517 backend and an `assets.json` manifest of per-platform wheel
//! filenames + sha256s. See `templates/sdist_build_backend.py`.

use crate::pack::{normalize_wheel_name, render_metadata, target_to_platform_tag, wheel_filename};
use crate::pyproject::Spec;
use crate::release_version::semver_to_pep440;
use crate::utils::{backoff, is_transient, require_https, sha256_hex};
use anyhow::{Context, Result, anyhow, bail};
use bytes::Bytes;
use python_pkginfo::Metadata;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs::File;
use std::io::{Cursor, Read as _};
use std::path::{Path, PathBuf};

/// Embedded PEP 517 backend baked into every sdist.
const SDIST_BACKEND_PY: &str = include_str!("../templates/sdist_build_backend.py");

/// Import package name of the embedded backend; shared by the bundled paths and
/// the pyproject `build-backend` line.
const SDIST_BACKEND_PKG: &str = "_dbt_sa_build";

/// A prebuilt wheel pinned by the sdist manifest.
#[derive(Debug)]
pub(crate) struct WheelAsset {
    /// PyPI platform tag (e.g. `manylinux_2_28_x86_64`); the manifest key.
    pub(crate) platform_tag: String,
    pub(crate) filename: String,
    pub(crate) sha256_hex: String,
}

#[derive(serde::Serialize)]
struct AssetsManifest<'a> {
    name: &'a str,
    version: &'a str,
    base_url: &'a str,
    /// Install-time notice the backend prints before downloading; see
    /// [`install_notice`]. Absent for distributions that don't need one.
    #[serde(skip_serializing_if = "Option::is_none")]
    notice: Option<&'a str>,
    wheels: BTreeMap<&'a str, AssetEntry<'a>>,
}

#[derive(serde::Serialize)]
struct AssetEntry<'a> {
    filename: &'a str,
    sha256: &'a str,
}

/// Builds the sdist `{dist}-{version}.tar.gz`; `wheels` pins the per-platform
/// filename + sha256 the manifest references.
pub(crate) fn build_sdist(
    spec: &Spec,
    version_pep440: &str,
    wheels: &[WheelAsset],
    base_url: &str,
    out_dir: &Path,
) -> Result<PathBuf> {
    require_https(base_url)?;
    let dist = normalize_wheel_name(&spec.wheel_name);
    let root = format!("{dist}-{version_pep440}");
    let sdist_path = out_dir.join(format!("{root}.tar.gz"));

    let pyproject = render_sdist_pyproject(spec, version_pep440);
    let pkg_info = render_metadata(spec, version_pep440);
    let notice = install_notice(&dist, version_pep440);
    let assets = render_assets_json(spec, version_pep440, base_url, wheels, notice.as_deref())?;

    let entries: Vec<(String, Vec<u8>)> = vec![
        (format!("{root}/pyproject.toml"), pyproject.into_bytes()),
        (format!("{root}/PKG-INFO"), pkg_info.into_bytes()),
        (
            format!("{root}/{SDIST_BACKEND_PKG}/__init__.py"),
            SDIST_BACKEND_PY.as_bytes().to_vec(),
        ),
        (
            format!("{root}/{SDIST_BACKEND_PKG}/assets.json"),
            assets.into_bytes(),
        ),
    ];

    write_targz(&sdist_path, &entries)?;
    Ok(sdist_path)
}

/// Builds the release sdist by fetching each `--target`'s wheel from `base_url`
/// and hashing the live bytes, so the manifest matches what installers fetch.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn build_release_sdist(
    http: &reqwest::Client,
    spec: &Spec,
    version: &str,
    base_url: &str,
    targets: &[String],
    python_tag: &str,
    abi_tag: &str,
    out_dir: &Path,
) -> Result<PathBuf> {
    if targets.is_empty() {
        bail!("--download-base-url requires at least one --target");
    }
    // Reject a non-https base url before any fetch, so we never pull wheels over
    // an insecure transport (the assembly-time check in `build_sdist` is too late).
    require_https(base_url)?;
    let version_pep440 = semver_to_pep440(version)?;
    let dist = normalize_wheel_name(&spec.wheel_name);
    let base = base_url.trim_end_matches('/');

    let mut wheels = Vec::with_capacity(targets.len());
    for triple in targets {
        let platform_tag = target_to_platform_tag(triple)
            .ok_or_else(|| anyhow!("unsupported --target {triple:?}"))?;
        let filename = wheel_filename(&dist, &version_pep440, python_tag, abi_tag, &platform_tag);
        let url = format!("{base}/{filename}");
        eprintln!("→ GET {url}");
        let bytes = download(http, &url).await?;
        let digest = sha256_hex(bytes.as_ref());
        eprintln!("✓ {filename} ({} bytes, sha256={digest})", bytes.len());
        check_wheel_metadata_agrees(spec, &filename, bytes.as_ref())?;
        wheels.push(WheelAsset {
            platform_tag,
            filename,
            sha256_hex: digest,
        });
    }

    build_sdist(spec, &version_pep440, &wheels, base_url, out_dir)
}

/// Fails the release when the sdist's static metadata disagrees with a wheel it
/// points at, in either direction. pip survives such a mismatch (it builds the
/// wheel and reads the real metadata), but uv trusts the sdist's PKG-INFO and
/// never re-checks — so a wrong `Requires-Python` installs on an unsupported
/// interpreter and dies at import on an ABI symbol, and a missing `Requires-Dist`
/// (in either direction) installs the wrong dependency set and dies on first run.
fn check_wheel_metadata_agrees(spec: &Spec, filename: &str, wheel: &[u8]) -> Result<()> {
    let Some(metadata) = wheel_metadata(wheel)
        .with_context(|| format!("read METADATA from {filename}"))?
        .map(|raw| Metadata::parse(&raw))
        .transpose()
        .with_context(|| format!("parse METADATA from {filename}"))?
    else {
        bail!("{filename} has no *.dist-info/METADATA");
    };

    let expected_python = spec.requires_python.as_deref();
    let actual_python = metadata.requires_python.as_deref();
    if actual_python != expected_python {
        bail!(
            "{filename} declares Requires-Python {actual:?} but the sdist says \
             {expected:?}; point `--pyproject-dir` at the pyproject that built \
             these wheels",
            actual = actual_python.unwrap_or("(absent)"),
            expected = expected_python.unwrap_or("(absent)"),
        );
    }

    let expected_deps: std::collections::BTreeSet<&str> =
        spec.dependencies.iter().map(|d| d.trim()).collect();
    let actual_deps: std::collections::BTreeSet<&str> =
        metadata.requires_dist.iter().map(|d| d.trim()).collect();
    if expected_deps != actual_deps {
        bail!(
            "{filename}'s Requires-Dist disagrees with the sdist: sdist deps={:?}, \
             wheel deps={:?}",
            expected_deps,
            actual_deps,
        );
    }
    Ok(())
}

/// The `*.dist-info/METADATA` bytes from a wheel held in memory.
fn wheel_metadata(wheel: &[u8]) -> Result<Option<Vec<u8>>> {
    let mut archive = zip::ZipArchive::new(Cursor::new(wheel)).context("open wheel as zip")?;
    let name = archive
        .file_names()
        .find(|n| {
            n.ends_with("/METADATA")
                && n.split('/')
                    .next()
                    .is_some_and(|d| d.ends_with(".dist-info"))
        })
        .map(str::to_string);
    let Some(name) = name else {
        return Ok(None);
    };

    let mut file = archive.by_name(&name)?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;
    Ok(Some(buf))
}

/// GETs `url`, retrying transient failures and 5xx. A 4xx (e.g. a missing wheel)
/// is fatal — better to fail than ship an sdist pointing at a missing wheel.
async fn download(http: &reqwest::Client, url: &str) -> Result<Bytes> {
    let max_attempts: u32 = 4;
    let mut attempt: u32 = 0;
    loop {
        attempt += 1;
        match http.get(url).send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    return resp
                        .bytes()
                        .await
                        .with_context(|| format!("read body from {url}"));
                }
                if status.is_server_error() && attempt < max_attempts {
                    let delay = backoff(attempt);
                    eprintln!(
                        "warning: GET {url} got {status}; retrying in {}ms",
                        delay.as_millis()
                    );
                    tokio::time::sleep(delay).await;
                    continue;
                }
                bail!("GET {url} failed: {status}");
            }
            Err(e) if is_transient(&e) && attempt < max_attempts => {
                let delay = backoff(attempt);
                eprintln!(
                    "warning: GET {url} failed: {e}; retrying in {}ms",
                    delay.as_millis()
                );
                tokio::time::sleep(delay).await;
                continue;
            }
            Err(e) => return Err(anyhow::Error::new(e).context(format!("GET {url}"))),
        }
    }
}

/// Minimal pyproject wiring up the embedded backend; rich metadata lives in PKG-INFO.
fn render_sdist_pyproject(spec: &Spec, version_pep440: &str) -> String {
    let mut out = String::new();
    out.push_str("[build-system]\n");
    out.push_str("requires = [\"packaging>=24\"]\n");
    let _ = writeln!(out, "build-backend = {SDIST_BACKEND_PKG:?}");
    out.push_str("backend-path = [\".\"]\n\n");
    out.push_str("[project]\n");
    // `{:?}` yields quoted, escaped TOML basic strings.
    let _ = writeln!(out, "name = {:?}", spec.wheel_name);
    let _ = writeln!(out, "version = {version_pep440:?}");
    if let Some(rp) = &spec.requires_python {
        let _ = writeln!(out, "requires-python = {rp:?}");
    }
    // Must mirror the downloaded wheel's deps: uv trusts sdist metadata rather
    // than rebuilding to read the wheel's, so omitting these installs broken.
    if !spec.dependencies.is_empty() {
        out.push_str("dependencies = [\n");
        for dep in &spec.dependencies {
            let _ = writeln!(out, "  {dep:?},");
        }
        out.push_str("]\n");
    }
    out
}

/// Normalized distribution name whose pre-releases carry the namespace notice.
const NOTICE_DIST: &str = "dbt_core";

/// Major version from which `dbt` / `dbt-oss` are the preferred names. `dbt-core`
/// keeps shipping in parallel at this major; the notice is a pointer, not an EOL.
const NOTICE_MAJOR_FLOOR: u64 = 2;

/// Install-time notice embedded in the sdist manifest, printed by the backend
/// before it downloads a wheel.
///
/// Only `dbt-core` 2.x *pre-releases* get one: a resolver never picks a
/// pre-release on its own, so reaching this build means the user asked for it
/// explicitly (`--pre`, or a `dbt-core==2.0.0rc…` pin) and should be pointed at
/// the `dbt` / `dbt-oss` distributions instead.
fn install_notice(dist: &str, version_pep440: &str) -> Option<String> {
    if dist != NOTICE_DIST {
        return None;
    }
    let (major, is_prerelease) = pep440_major_and_prerelease(version_pep440)?;
    if major < NOTICE_MAJOR_FLOOR || !is_prerelease {
        return None;
    }
    Some(format!(
        "\
========================================================================
WARNING: you are installing a pre-release of `dbt-core` ({version_pep440}).

Starting with 2.x, `dbt` and `dbt-oss` are the recommended package names
going forward. Unless you specifically need the `dbt-core` name, install
one of these instead:

    pip install dbt
    pip install dbt-oss    # Apache-2.0 licensed

You are seeing this because you passed `--pre`, or pinned a version like
`dbt-core=={version_pep440}`.
========================================================================",
    ))
}

/// Splits the `X.Y.Z[{a,b,rc}N | .devN]` versions [`semver_to_pep440`] emits
/// into `(major, is_prerelease)`. `None` if the release segment isn't numeric.
fn pep440_major_and_prerelease(version_pep440: &str) -> Option<(u64, bool)> {
    let release_end = version_pep440
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(version_pep440.len());
    // `.devN` leaves the separating dot behind in the release segment.
    let release = version_pep440[..release_end].trim_end_matches('.');
    let major = release.split('.').next()?.parse().ok()?;
    Some((major, release_end != version_pep440.len()))
}

fn render_assets_json(
    spec: &Spec,
    version_pep440: &str,
    base_url: &str,
    wheels: &[WheelAsset],
    notice: Option<&str>,
) -> Result<String> {
    let mut map: BTreeMap<&str, AssetEntry> = BTreeMap::new();
    for w in wheels {
        let entry = AssetEntry {
            filename: &w.filename,
            sha256: &w.sha256_hex,
        };
        if map.insert(w.platform_tag.as_str(), entry).is_some() {
            bail!(
                "two binaries map to platform tag {:?}; the sdist manifest can only \
                 reference one wheel per platform",
                w.platform_tag
            );
        }
    }
    let manifest = AssetsManifest {
        name: &spec.wheel_name,
        version: version_pep440,
        base_url,
        notice,
        wheels: map,
    };
    let mut json = serde_json::to_string_pretty(&manifest).context("serialize assets.json")?;
    json.push('\n');
    Ok(json)
}

fn write_targz(out_path: &Path, entries: &[(String, Vec<u8>)]) -> Result<()> {
    let file = File::create(out_path).with_context(|| format!("create {}", out_path.display()))?;
    let enc = flate2::write::GzEncoder::new(file, flate2::Compression::best());
    let mut builder = tar::Builder::new(enc);
    for (path, bytes) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        // Fixed mtime keeps the sdist byte-reproducible across builds.
        header.set_mtime(0);
        builder
            .append_data(&mut header, path, bytes.as_slice())
            .with_context(|| format!("tar append {path}"))?;
    }
    builder
        .into_inner()
        .context("finish tar")?
        .finish()
        .context("finish gzip")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn sample_spec(dir: &Path) -> Spec {
        named_spec(dir, "dbt-sa-cli")
    }

    fn named_spec(dir: &Path, name: &str) -> Spec {
        Spec {
            wheel_name: name.to_string(),
            pyproject_dir: dir.to_path_buf(),
            summary: Some("dbt fusion standalone analyzer CLI".to_string()),
            requires_python: Some(">=3.9".to_string()),
            dependencies: vec!["mashumaro[msgpack]>=3.14".to_string()],
            classifiers: vec![],
            urls: vec![],
            authors: vec![],
            license: None,
            description: None,
            description_content_type: None,
        }
    }

    fn read_targz(path: &Path) -> BTreeMap<String, String> {
        let file = File::open(path).unwrap();
        let dec = flate2::read::GzDecoder::new(file);
        let mut ar = tar::Archive::new(dec);
        let mut out = BTreeMap::new();
        for entry in ar.entries().unwrap() {
            let mut entry = entry.unwrap();
            let name = entry.path().unwrap().to_string_lossy().into_owned();
            let mut body = String::new();
            entry.read_to_string(&mut body).unwrap();
            out.insert(name, body);
        }
        out
    }

    /// A wheel carrying `METADATA` with the given headers.
    fn wheel_with_metadata(headers: &str) -> Vec<u8> {
        use std::io::Write as _;
        use zip::write::{SimpleFileOptions, ZipWriter};

        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file(
            "dbt_sa_cli-2.0.0a1.dist-info/METADATA",
            SimpleFileOptions::default(),
        )
        .unwrap();
        zip.write_all(
            format!("Metadata-Version: 2.1\nName: dbt-sa-cli\nVersion: 2.0.0a1\n{headers}")
                .as_bytes(),
        )
        .unwrap();
        zip.finish().unwrap().into_inner()
    }

    #[test]
    fn cross_check_accepts_a_wheel_that_agrees() {
        let tmp = tempfile::tempdir().unwrap();
        let spec = sample_spec(tmp.path());
        let wheel = wheel_with_metadata(
            "Requires-Python: >=3.9\nRequires-Dist: mashumaro[msgpack]>=3.14\n",
        );

        check_wheel_metadata_agrees(&spec, "w.whl", &wheel).unwrap();
    }

    #[test]
    fn cross_check_rejects_a_narrower_requires_python() {
        let tmp = tempfile::tempdir().unwrap();
        let spec = sample_spec(tmp.path());
        // The real defect: an sdist promising 3.9 in front of abi3-py310 wheels.
        let wheel = wheel_with_metadata(
            "Requires-Python: >=3.10\nRequires-Dist: mashumaro[msgpack]>=3.14\n",
        );

        let err = check_wheel_metadata_agrees(&spec, "w.whl", &wheel)
            .unwrap_err()
            .to_string();
        assert!(err.contains("Requires-Python"), "got: {err}");
    }

    #[test]
    fn cross_check_rejects_a_wheel_missing_a_promised_dependency() {
        let tmp = tempfile::tempdir().unwrap();
        let spec = sample_spec(tmp.path());
        let wheel = wheel_with_metadata("Requires-Python: >=3.9\n");

        let err = check_wheel_metadata_agrees(&spec, "w.whl", &wheel)
            .unwrap_err()
            .to_string();
        assert!(err.contains("Requires-Dist"), "got: {err}");
    }

    #[test]
    fn build_sdist_bundles_backend_and_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let spec = sample_spec(dir);
        let wheels = vec![
            WheelAsset {
                platform_tag: "manylinux_2_28_x86_64".to_string(),
                filename: "dbt_sa_cli-2.0.0a1-py3-none-manylinux_2_28_x86_64.whl".to_string(),
                sha256_hex: "aa".repeat(32),
            },
            WheelAsset {
                platform_tag: "macosx_11_0_arm64".to_string(),
                filename: "dbt_sa_cli-2.0.0a1-py3-none-macosx_11_0_arm64.whl".to_string(),
                sha256_hex: "bb".repeat(32),
            },
        ];

        let path =
            build_sdist(&spec, "2.0.0a1", &wheels, "https://example.com/dl/v2/", dir).unwrap();
        assert_eq!(
            path.file_name().unwrap().to_str().unwrap(),
            "dbt_sa_cli-2.0.0a1.tar.gz"
        );

        let files = read_targz(&path);
        for expected in [
            "dbt_sa_cli-2.0.0a1/pyproject.toml",
            "dbt_sa_cli-2.0.0a1/PKG-INFO",
            "dbt_sa_cli-2.0.0a1/_dbt_sa_build/__init__.py",
            "dbt_sa_cli-2.0.0a1/_dbt_sa_build/assets.json",
        ] {
            assert!(files.contains_key(expected), "missing {expected}");
        }

        let pyproject = &files["dbt_sa_cli-2.0.0a1/pyproject.toml"];
        assert!(pyproject.contains("build-backend = \"_dbt_sa_build\""));
        assert!(pyproject.contains("name = \"dbt-sa-cli\""));
        assert!(pyproject.contains("version = \"2.0.0a1\""));
        assert!(pyproject.contains("requires-python = \">=3.9\""));
        assert!(
            pyproject.contains("\"mashumaro[msgpack]>=3.14\","),
            "sdist pyproject dropped dependencies: {pyproject}"
        );

        let pkg_info = &files["dbt_sa_cli-2.0.0a1/PKG-INFO"];
        assert!(
            pkg_info.contains("Requires-Dist: mashumaro[msgpack]>=3.14"),
            "sdist PKG-INFO dropped dependencies: {pkg_info}"
        );

        let backend = &files["dbt_sa_cli-2.0.0a1/_dbt_sa_build/__init__.py"];
        assert!(backend.contains("def build_wheel("));

        let assets: serde_json::Value =
            serde_json::from_str(&files["dbt_sa_cli-2.0.0a1/_dbt_sa_build/assets.json"]).unwrap();
        assert_eq!(assets["name"], "dbt-sa-cli");
        assert_eq!(assets["version"], "2.0.0a1");
        assert_eq!(assets["base_url"], "https://example.com/dl/v2/");
        assert_eq!(
            assets["wheels"]["manylinux_2_28_x86_64"]["filename"],
            "dbt_sa_cli-2.0.0a1-py3-none-manylinux_2_28_x86_64.whl"
        );
        assert_eq!(
            assets["wheels"]["macosx_11_0_arm64"]["sha256"],
            "bb".repeat(32)
        );
    }

    #[test]
    fn build_sdist_declares_wheel_dependencies() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let mut spec = sample_spec(dir);
        spec.wheel_name = "dbt-oss".to_string();
        spec.requires_python = Some(">=3.10".to_string());
        spec.dependencies = vec!["mashumaro[msgpack]>=3.14".to_string()];
        let wheels = vec![WheelAsset {
            platform_tag: "macosx_11_0_arm64".to_string(),
            filename: "dbt_oss-2.0.0a1-cp310-abi3-macosx_11_0_arm64.whl".to_string(),
            sha256_hex: "cc".repeat(32),
        }];

        let path = build_sdist(&spec, "2.0.0a1", &wheels, "https://example.com/dl/", dir).unwrap();
        let files = read_targz(&path);

        let pyproject = &files["dbt_oss-2.0.0a1/pyproject.toml"];
        assert!(pyproject.contains("requires-python = \">=3.10\""));
        assert!(
            pyproject.contains("dependencies = [\n  \"mashumaro[msgpack]>=3.14\",\n]"),
            "sdist pyproject must declare the wheel's deps, got:\n{pyproject}"
        );

        let pkg_info = &files["dbt_oss-2.0.0a1/PKG-INFO"];
        assert!(pkg_info.contains("Requires-Dist: mashumaro[msgpack]>=3.14"));
        assert!(pkg_info.contains("Requires-Python: >=3.10"));
    }

    #[test]
    fn build_sdist_rejects_duplicate_platform_tag() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let spec = sample_spec(dir);
        // Both windows-msvc and windows-gnu map to win_amd64.
        let wheels = vec![
            WheelAsset {
                platform_tag: "win_amd64".to_string(),
                filename: "a.whl".to_string(),
                sha256_hex: "aa".repeat(32),
            },
            WheelAsset {
                platform_tag: "win_amd64".to_string(),
                filename: "b.whl".to_string(),
                sha256_hex: "bb".repeat(32),
            },
        ];
        let err = build_sdist(&spec, "2.0.0", &wheels, "https://example.com", dir).unwrap_err();
        assert!(err.to_string().contains("win_amd64"));
    }

    #[test]
    fn build_sdist_rejects_non_https_base_url() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let spec = sample_spec(dir);
        let wheels = vec![WheelAsset {
            platform_tag: "win_amd64".to_string(),
            filename: "a.whl".to_string(),
            sha256_hex: "aa".repeat(32),
        }];
        let err = build_sdist(&spec, "2.0.0", &wheels, "http://example.com", dir).unwrap_err();
        assert!(err.to_string().contains("https"));
    }

    #[test]
    fn install_notice_targets_dbt_core_prereleases() {
        for version in ["2.0.0rc1", "2.0.0a4", "2.0.0b2", "2.0.0.dev5", "3.1.0rc2"] {
            let notice = install_notice("dbt_core", version)
                .unwrap_or_else(|| panic!("expected a notice for {version}"));
            assert!(notice.contains(version), "{version} not named in notice");
            assert!(notice.contains("pip install dbt\n"));
            assert!(notice.contains("pip install dbt-oss"));
        }
        // Final releases, older majors, and the other distributions stay quiet.
        assert!(install_notice("dbt_core", "2.0.0").is_none());
        assert!(install_notice("dbt_core", "1.11.0rc1").is_none());
        assert!(install_notice("dbt_oss", "2.0.0rc1").is_none());
        assert!(install_notice("dbt_core_experimental_parser", "2.0.0rc1").is_none());
    }

    #[test]
    fn pep440_major_and_prerelease_parses_lanes() {
        assert_eq!(pep440_major_and_prerelease("2.0.0"), Some((2, false)));
        assert_eq!(pep440_major_and_prerelease("2.0.0rc1"), Some((2, true)));
        assert_eq!(pep440_major_and_prerelease("2.0.0a4"), Some((2, true)));
        assert_eq!(pep440_major_and_prerelease("2.0.0b2"), Some((2, true)));
        assert_eq!(pep440_major_and_prerelease("2.0.0.dev5"), Some((2, true)));
        assert_eq!(pep440_major_and_prerelease("10.2.3"), Some((10, false)));
        assert_eq!(pep440_major_and_prerelease("rc1"), None);
    }

    #[test]
    fn build_sdist_embeds_notice_only_for_dbt_core_prereleases() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let wheels = vec![WheelAsset {
            platform_tag: "macosx_11_0_arm64".to_string(),
            filename: "dbt_core-2.0.0rc1-cp310-abi3-macosx_11_0_arm64.whl".to_string(),
            sha256_hex: "aa".repeat(32),
        }];

        let notice = manifest_notice(
            &named_spec(dir, "dbt-core"),
            "2.0.0rc1",
            &wheels,
            dir.join("pre"),
        );
        assert!(notice.unwrap().contains("pip install dbt-oss"));

        // Only the pre-release lane and the name gate it; both other cases ship
        // a manifest with no `notice` key at all.
        assert!(
            manifest_notice(
                &named_spec(dir, "dbt-core"),
                "2.0.0",
                &wheels,
                dir.join("final"),
            )
            .is_none()
        );
        assert!(
            manifest_notice(
                &named_spec(dir, "dbt-oss"),
                "2.0.0rc1",
                &wheels,
                dir.join("oss"),
            )
            .is_none()
        );
    }

    /// Builds an sdist into a fresh `out_dir` and reads back its manifest notice.
    fn manifest_notice(
        spec: &Spec,
        version_pep440: &str,
        wheels: &[WheelAsset],
        out_dir: PathBuf,
    ) -> Option<String> {
        std::fs::create_dir_all(&out_dir).unwrap();
        let path = build_sdist(
            spec,
            version_pep440,
            wheels,
            "https://example.com/dl",
            &out_dir,
        )
        .unwrap();
        let files = read_targz(&path);
        let dist = normalize_wheel_name(&spec.wheel_name);
        let assets: serde_json::Value = serde_json::from_str(
            &files[&format!("{dist}-{version_pep440}/_dbt_sa_build/assets.json")],
        )
        .unwrap();
        assets
            .get("notice")
            .map(|n| n.as_str().unwrap().to_string())
    }
}
