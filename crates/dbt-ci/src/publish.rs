use crate::args::PypiPublishArgs;
use crate::pack::normalize_wheel_name;
use crate::pyproject;
use crate::release_version::{semver_to_pep440, validate_release_version};
use crate::sdist::build_release_sdist;
use crate::utils::{backoff, cargo_workspace_root, is_transient, sha256_hex};
use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use bytes::Bytes;
use clap::ValueEnum;
use python_pkginfo::{Distribution, Metadata};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

type EnvGet<'a> = &'a dyn Fn(&str) -> Option<String>;

fn std_env(name: &str) -> Option<String> {
    env::var(name).ok().filter(|v| !v.is_empty())
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum Environment {
    Staging,
    Prod,
    TestPypi,
}

#[derive(Debug)]
pub(crate) enum PublishTarget {
    CodeArtifact(CodeArtifactTarget),
    Pypi { token: String, url: String },
}

#[derive(Debug)]
pub(crate) struct CodeArtifactTarget {
    pub(crate) domain: String,
    pub(crate) domain_owner: String,
    pub(crate) region: String,
    pub(crate) repository: String,
    pub(crate) profile: String,
}

impl PublishTarget {
    pub(crate) fn from_env(env: Environment) -> Result<Self> {
        Self::resolve(env, &std_env)
    }

    fn resolve(env: Environment, get: EnvGet<'_>) -> Result<Self> {
        match env {
            Environment::Staging => {
                CodeArtifactTarget::resolve("DBT_PYPI_STAGING", get).map(Self::CodeArtifact)
            }
            Environment::Prod => Ok(Self::Pypi {
                token: required(get, "DBT_PYPI_PROD_TOKEN")?,
                url: "https://upload.pypi.org/legacy/".into(),
            }),
            Environment::TestPypi => Ok(Self::Pypi {
                token: required(get, "DBT_PYPI_TEST_TOKEN")?,
                url: "https://test.pypi.org/legacy/".into(),
            }),
        }
    }
}

impl CodeArtifactTarget {
    fn resolve(prefix: &str, get: EnvGet<'_>) -> Result<Self> {
        let mut missing = Vec::new();
        let mut req = |suffix: &str| -> String {
            let name = format!("{prefix}{suffix}");
            match get(&name) {
                Some(v) => v,
                None => {
                    missing.push(name);
                    String::new()
                }
            }
        };
        let domain = req("_DOMAIN");
        let domain_owner = req("_DOMAIN_OWNER");
        let region = req("_REGION");
        let repository = req("_REPOSITORY");
        let profile = req("_PROFILE");
        if !missing.is_empty() {
            bail!(format_missing(&missing));
        }
        Ok(CodeArtifactTarget {
            domain,
            domain_owner,
            region,
            repository,
            profile,
        })
    }

    fn upload_url(&self) -> String {
        format!(
            "https://{}-{}.d.codeartifact.{}.amazonaws.com/pypi/{}/",
            self.domain, self.domain_owner, self.region, self.repository,
        )
    }
}

fn required(get: EnvGet<'_>, name: &str) -> Result<String> {
    get(name).ok_or_else(|| anyhow!(format_missing(&[name.to_string()])))
}

fn format_missing(names: &[String]) -> String {
    format!(
        "missing required environment variable{}:\n  {}",
        if names.len() == 1 { "" } else { "s" },
        names.join("\n  ")
    )
}

pub fn execute(args: PypiPublishArgs) -> ExitCode {
    let spec = match &args.project_dir {
        Some(dir) => pyproject::discover_at(dir),
        None => pyproject::discover(),
    };
    let mut spec = match spec {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e:#}");
            return ExitCode::from(2);
        }
    };
    if let Some(dir) = &args.runtime_metadata_from {
        if let Err(e) = spec.overlay_runtime_metadata(dir) {
            eprintln!("error: --runtime-metadata-from: {e:#}");
            return ExitCode::from(2);
        }
    }
    if let Some(v) = &args.version {
        if let Err(e) = validate_release_version(v) {
            eprintln!("error: --version {e:#}");
            return ExitCode::from(2);
        }
    }
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("error: failed to start async runtime: {e}");
            return ExitCode::from(2);
        }
    };

    // Build-only mode: assemble the sdist into `--sdist-out` and stop. The
    // release pipeline stages it to S3/CDN itself, so no upload target is needed.
    if let Some(out_dir) = &args.sdist_out {
        return match rt.block_on(build_only_sdist(&args, &spec, out_dir)) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e:#}");
                ExitCode::from(2)
            }
        };
    }

    let target = match PublishTarget::from_env(
        args.environment
            .expect("clap requires --environment unless --sdist-out"),
    ) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: {e:#}");
            return ExitCode::from(64);
        }
    };
    match rt.block_on(run_publish(&args, &spec, target)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::from(2)
        }
    }
}

/// Builds the download-at-install sdist into `out_dir` without uploading it.
async fn build_only_sdist(
    args: &PypiPublishArgs,
    spec: &pyproject::Spec,
    out_dir: &Path,
) -> Result<()> {
    let http = http_client()?;
    // clap's `requires` guarantees `--version` alongside `--download-base-url`,
    // which `--sdist-out` also requires.
    let base_url = args
        .download_base_url
        .as_deref()
        .context("--sdist-out requires --download-base-url")?;
    let version = args
        .version
        .as_deref()
        .expect("clap requires --version with --download-base-url");
    fs::create_dir_all(out_dir)
        .with_context(|| format!("create sdist out dir {}", out_dir.display()))?;
    let sdist = build_release_sdist(
        &http,
        spec,
        version,
        base_url,
        &args.targets,
        &args.python_tag,
        &args.abi_tag,
        out_dir,
    )
    .await?;
    eprintln!("✓ sdist: {}", sdist.display());
    Ok(())
}

/// Resolves what to publish — a freshly-built sdist (`--download-base-url`) or
/// local wheels — and uploads it to `target`.
async fn run_publish(
    args: &PypiPublishArgs,
    spec: &pyproject::Spec,
    target: PublishTarget,
) -> Result<()> {
    // `_tmp` keeps the sdist's temp dir alive until the upload finishes.
    let (dists, _tmp): (Vec<(PathBuf, DistKind)>, _) =
        if let Some(base_url) = &args.download_base_url {
            let http = http_client()?;
            // clap's `requires` guarantees `--version` is set alongside the base url.
            let version = args
                .version
                .as_deref()
                .expect("clap requires --version with --download-base-url");
            let tmp = tempfile::tempdir().context("create temp dir for sdist")?;
            let sdist = build_release_sdist(
                &http,
                spec,
                version,
                base_url,
                &args.targets,
                &args.python_tag,
                &args.abi_tag,
                tmp.path(),
            )
            .await?;
            (vec![(sdist, DistKind::Sdist)], Some(tmp))
        } else {
            let dist_dir = args
                .dist
                .clone()
                .unwrap_or_else(|| cargo_workspace_root().join("target").join("wheels"));
            let wheels = discover_wheels(&dist_dir, &spec.wheel_name, args.version.as_deref())?;
            if wheels.is_empty() {
                let suffix = args
                    .version
                    .as_deref()
                    .map(|v| format!(" at version {v}"))
                    .unwrap_or_default();
                bail!(
                    "no `{}` wheel(s){suffix} in {}.",
                    spec.wheel_name,
                    dist_dir.display()
                );
            }
            (
                wheels.into_iter().map(|w| (w, DistKind::Wheel)).collect(),
                None,
            )
        };

    eprintln!(
        "→ publishing {} `{}` artifact(s):",
        dists.len(),
        spec.wheel_name
    );
    for (path, _) in &dists {
        if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
            eprintln!("    {name}");
        }
    }

    match target {
        PublishTarget::CodeArtifact(t) => upload_codeartifact(&t, &dists).await,
        PublishTarget::Pypi { token, url } => upload_pypi(&token, &url, &dists).await,
    }
}

async fn upload_codeartifact(
    target: &CodeArtifactTarget,
    dists: &[(PathBuf, DistKind)],
) -> Result<()> {
    eprintln!(
        "→ aws codeartifact get-authorization-token (domain={}, region={}, profile={})",
        target.domain, target.region, target.profile,
    );
    let region = aws_config::Region::new(target.region.clone());
    let conf = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(region)
        .profile_name(&target.profile)
        .load()
        .await;
    let client = aws_sdk_codeartifact::Client::new(&conf);
    let token = client
        .get_authorization_token()
        .domain(&target.domain)
        .domain_owner(&target.domain_owner)
        .duration_seconds(900)
        .send()
        .await
        .context("aws codeartifact get-authorization-token")?
        .authorization_token
        .ok_or_else(|| anyhow!("codeartifact returned no authorization token"))?;

    let url = target.upload_url();
    let http = http_client()?;
    for (w, kind) in dists {
        upload_dist(&http, &url, "aws", &token, w, *kind).await?;
    }
    Ok(())
}

async fn upload_pypi(token: &str, url: &str, dists: &[(PathBuf, DistKind)]) -> Result<()> {
    let http = http_client()?;
    for (w, kind) in dists {
        upload_dist(&http, url, "__token__", token, w, *kind).await?;
    }
    Ok(())
}

fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(concat!("dbt-ci/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("build http client")
}

/// Mirrors twine's upload payload, reading metadata from the artifact's
/// METADATA/PKG-INFO. Handles both wheels and sdists.
async fn upload_dist(
    http: &reqwest::Client,
    url: &str,
    user: &str,
    password: &str,
    dist: &Path,
    kind: DistKind,
) -> Result<()> {
    let filename = dist
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow!("non-UTF8 distribution filename: {}", dist.display()))?
        .to_string();

    let metadata = Distribution::new(dist)
        .with_context(|| format!("read metadata from {}", dist.display()))?
        .metadata()
        .clone();

    // Wheels carry name/version/pyversion in the filename; the sdist's from PKG-INFO.
    let (parsed, filetype) = match kind {
        DistKind::Wheel => {
            let parsed = parse_wheel_filename(&filename)
                .ok_or_else(|| anyhow!("wheel filename not in PEP 491 form: {filename}"))?;
            (parsed, "bdist_wheel")
        }
        DistKind::Sdist => (
            ParsedWheel {
                name: metadata.name.clone(),
                version: metadata.version.clone(),
                pyversion: "source".to_string(),
            },
            "sdist",
        ),
    };

    let bytes: Bytes = fs::read(dist)
        .with_context(|| format!("read {}", dist.display()))?
        .into();
    let md5_digest = format!("{:x}", md5::compute(&bytes));
    let sha256_digest = sha256_hex(&bytes);

    eprintln!(
        "→ POST {url} ({filename}, {} bytes, sha256={sha256_digest})",
        bytes.len(),
    );

    let auth = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}"))
    );

    // Form is not Clone; rebuild per attempt.
    let max_attempts: u32 = 4;
    let mut attempt: u32 = 0;
    loop {
        attempt += 1;
        let form = build_upload_form(
            &parsed,
            filetype,
            &metadata,
            &md5_digest,
            &sha256_digest,
            &filename,
            &bytes,
        )?;
        let send = http
            .post(url)
            .header("Authorization", &auth)
            .header("Accept", "application/json")
            .multipart(form)
            .send()
            .await;

        match send {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    eprintln!("✓ {filename} → {status}");
                    return Ok(());
                }
                if status.is_server_error() && attempt < max_attempts {
                    let delay = backoff(attempt);
                    eprintln!(
                        "warning: upload attempt {attempt}/{max_attempts} for {filename} got {status}; retrying in {}ms",
                        delay.as_millis(),
                    );
                    tokio::time::sleep(delay).await;
                    continue;
                }
                let body = resp.text().await.unwrap_or_default();
                if is_already_exists(status, &body) {
                    eprintln!(
                        "• {filename} already published at {url}; treating as success ({status})"
                    );
                    return Ok(());
                }
                bail!("upload {filename} failed: {status}\n{body}");
            }
            Err(e) if is_transient(&e) && attempt < max_attempts => {
                let delay = backoff(attempt);
                eprintln!(
                    "warning: upload attempt {attempt}/{max_attempts} for {filename} failed: {e}; retrying in {}ms",
                    delay.as_millis(),
                );
                tokio::time::sleep(delay).await;
                continue;
            }
            Err(e) => {
                return Err(anyhow::Error::new(e).context(format!("POST {url}")));
            }
        }
    }
}

fn build_upload_form(
    parsed: &ParsedWheel,
    filetype: &str,
    metadata: &Metadata,
    md5_digest: &str,
    sha256_digest: &str,
    filename: &str,
    bytes: &Bytes,
) -> Result<reqwest::multipart::Form> {
    // Bytes::clone is O(1) (shared buffer), so retries don't re-copy the artifact.
    let len = bytes.len() as u64;
    let body: reqwest::Body = bytes.clone().into();
    let file_part = reqwest::multipart::Part::stream_with_length(body, len)
        .file_name(filename.to_string())
        .mime_str("application/octet-stream")
        .context("build multipart file part")?;

    let mut form = reqwest::multipart::Form::new()
        .text(":action", "file_upload")
        .text("protocol_version", "1")
        .text("metadata_version", metadata.metadata_version.clone())
        .text("name", parsed.name.clone())
        .text("version", parsed.version.clone())
        .text("filetype", filetype.to_string())
        .text("pyversion", parsed.pyversion.clone())
        .text("md5_digest", md5_digest.to_string())
        .text("sha256_digest", sha256_digest.to_string());

    for (name, value) in metadata_form_fields(metadata) {
        form = form.text(name, value);
    }

    Ok(form.part("content", file_part))
}

/// Field names follow twine's convention — repeating fields are singular
/// (`platform`, `supported_platform`, `license_file`, `provides_extra`) even
/// though python-pkginfo names them as plurals.
fn metadata_form_fields(m: &Metadata) -> Vec<(&'static str, String)> {
    let mut out: Vec<(&'static str, String)> = Vec::new();

    let single: &[(&'static str, Option<&str>)] = &[
        ("summary", m.summary.as_deref()),
        ("description", m.description.as_deref()),
        (
            "description_content_type",
            m.description_content_type.as_deref(),
        ),
        ("author", m.author.as_deref()),
        ("author_email", m.author_email.as_deref()),
        ("maintainer", m.maintainer.as_deref()),
        ("maintainer_email", m.maintainer_email.as_deref()),
        ("license", m.license.as_deref()),
        ("license_expression", m.license_expression.as_deref()),
        ("home_page", m.home_page.as_deref()),
        ("download_url", m.download_url.as_deref()),
        ("keywords", m.keywords.as_deref()),
        ("requires_python", m.requires_python.as_deref()),
    ];
    for (name, value) in single {
        if let Some(v) = *value {
            out.push((*name, v.to_string()));
        }
    }

    let repeating: &[(&'static str, &[String])] = &[
        ("classifiers", &m.classifiers),
        ("platform", &m.platforms),
        ("supported_platform", &m.supported_platforms),
        ("project_urls", &m.project_urls),
        ("requires_dist", &m.requires_dist),
        ("provides_dist", &m.provides_dist),
        ("obsoletes_dist", &m.obsoletes_dist),
        ("requires_external", &m.requires_external),
        ("provides_extra", &m.provides_extras),
        ("license_file", &m.license_files),
        ("dynamic", &m.dynamic),
    ];
    for (name, values) in repeating {
        for v in *values {
            out.push((*name, v.clone()));
        }
    }

    out
}

/// Treat a re-upload of an existing wheel as success so a retry after partial
/// publish doesn't brick the run. Warehouse returns 400 with `File already
/// exists.`; CodeArtifact returns 409.
fn is_already_exists(status: reqwest::StatusCode, body: &str) -> bool {
    use reqwest::StatusCode;
    if status != StatusCode::BAD_REQUEST && status != StatusCode::CONFLICT {
        return false;
    }
    let lower = body.to_ascii_lowercase();
    lower.contains("already exists") || lower.contains("file name reuse")
}

/// Which kind of distribution a file is — set by its producer, so the uploader
/// needn't re-derive it from the filename.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DistKind {
    Wheel,
    Sdist,
}

#[derive(Debug, PartialEq, Eq)]
struct ParsedWheel {
    name: String,
    version: String,
    pyversion: String,
}

/// PEP 491: `{dist}-{version}(-{build})?-{py}-{abi}-{plat}.whl`.
fn parse_wheel_filename(filename: &str) -> Option<ParsedWheel> {
    let stem = filename.strip_suffix(".whl")?;
    let parts: Vec<&str> = stem.split('-').collect();
    let (name, version, pyversion) = match parts.len() {
        5 => (parts[0], parts[1], parts[2]),
        6 => (parts[0], parts[1], parts[3]),
        _ => return None,
    };
    Some(ParsedWheel {
        name: name.to_string(),
        version: version.to_string(),
        pyversion: pyversion.to_string(),
    })
}

pub(crate) fn discover_wheels(
    dir: &Path,
    wheel_name: &str,
    version: Option<&str>,
) -> Result<Vec<PathBuf>> {
    let normalized_name = normalize_wheel_name(wheel_name);
    let want_version = version.map(semver_to_pep440).transpose()?;
    let entries =
        fs::read_dir(dir).with_context(|| format!("failed to read wheel dir {}", dir.display()))?;
    let mut wheels = Vec::new();
    for entry in entries {
        let entry = entry.with_context(|| format!("reading {}", dir.display()))?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        let Some(parsed) = parse_wheel_filename(name) else {
            continue;
        };
        if normalize_wheel_name(&parsed.name) != normalized_name {
            continue;
        }
        if let Some(want) = &want_version {
            if &parsed.version != want {
                continue;
            }
        }
        wheels.push(path);
    }
    wheels.sort();
    Ok(wheels)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env_from(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).into(), (*v).into()))
            .collect()
    }

    #[test]
    fn codeartifact_target_resolve_reads_all_fields() {
        let env = env_from(&[
            ("PFX_DOMAIN", "test-domain"),
            ("PFX_DOMAIN_OWNER", "111122223333"),
            ("PFX_REGION", "us-west-2"),
            ("PFX_REPOSITORY", "test-repo"),
            ("PFX_PROFILE", "test-profile"),
        ]);
        let t = CodeArtifactTarget::resolve("PFX", &|k| env.get(k).cloned()).unwrap();
        assert_eq!(t.domain, "test-domain");
        assert_eq!(t.domain_owner, "111122223333");
        assert_eq!(t.region, "us-west-2");
        assert_eq!(t.repository, "test-repo");
        assert_eq!(t.profile, "test-profile");
    }

    #[test]
    fn codeartifact_target_resolve_reports_all_missing_at_once() {
        let env = env_from(&[("PFX_DOMAIN", "set"), ("PFX_REGION", "set")]);
        let err = CodeArtifactTarget::resolve("PFX", &|k| env.get(k).cloned())
            .unwrap_err()
            .to_string();
        assert!(err.contains("PFX_DOMAIN_OWNER"));
        assert!(err.contains("PFX_REPOSITORY"));
        assert!(err.contains("PFX_PROFILE"));
        assert!(!err.contains("PFX_DOMAIN\n"));
        assert!(!err.contains("PFX_REGION\n"));
    }

    #[test]
    fn codeartifact_upload_url_is_assembled_from_fields() {
        let t = CodeArtifactTarget {
            domain: "fusion".into(),
            domain_owner: "111122223333".into(),
            region: "us-west-2".into(),
            repository: "wheels".into(),
            profile: "release".into(),
        };
        assert_eq!(
            t.upload_url(),
            "https://fusion-111122223333.d.codeartifact.us-west-2.amazonaws.com/pypi/wheels/"
        );
    }

    #[test]
    fn publish_target_resolves_prod_to_pypi_upload_url() {
        let env = env_from(&[("DBT_PYPI_PROD_TOKEN", "pypi-token-xyz")]);
        match PublishTarget::resolve(Environment::Prod, &|k| env.get(k).cloned()).unwrap() {
            PublishTarget::Pypi { token, url } => {
                assert_eq!(token, "pypi-token-xyz");
                assert_eq!(url, "https://upload.pypi.org/legacy/");
            }
            other => panic!("expected Pypi variant, got {other:?}"),
        }
    }

    #[test]
    fn publish_target_resolves_test_pypi_to_legacy_url() {
        let env = env_from(&[("DBT_PYPI_TEST_TOKEN", "pypi-test-token")]);
        match PublishTarget::resolve(Environment::TestPypi, &|k| env.get(k).cloned()).unwrap() {
            PublishTarget::Pypi { token, url } => {
                assert_eq!(token, "pypi-test-token");
                assert_eq!(url, "https://test.pypi.org/legacy/");
            }
            other => panic!("expected Pypi variant, got {other:?}"),
        }
    }

    #[test]
    fn publish_target_resolves_prod_missing_token_errors() {
        let env: HashMap<String, String> = HashMap::new();
        let err = PublishTarget::resolve(Environment::Prod, &|k| env.get(k).cloned())
            .unwrap_err()
            .to_string();
        assert!(err.contains("DBT_PYPI_PROD_TOKEN"));
    }

    #[test]
    fn is_already_exists_matches_warehouse_400() {
        use reqwest::StatusCode;
        assert!(is_already_exists(
            StatusCode::BAD_REQUEST,
            "400 Bad Request from https://upload.pypi.org/legacy/: File already exists. See https://pypi.org/help/#file-name-reuse"
        ));
    }

    #[test]
    fn is_already_exists_matches_codeartifact_409() {
        use reqwest::StatusCode;
        assert!(is_already_exists(
            StatusCode::CONFLICT,
            "An asset with the name dbt_sa_cli-2.0.0a1-py3-none-macosx_11_0_arm64.whl already exists."
        ));
    }

    #[test]
    fn is_already_exists_ignores_other_4xx_bodies() {
        use reqwest::StatusCode;
        assert!(!is_already_exists(
            StatusCode::FORBIDDEN,
            "Forbidden: invalid token"
        ));
        assert!(!is_already_exists(
            StatusCode::BAD_REQUEST,
            "Malformed metadata: missing Name header"
        ));
    }

    #[test]
    fn parse_wheel_filename_extracts_name_version_pyversion() {
        let p = parse_wheel_filename("dbt_sa_cli-2.0.0a1-py3-none-macosx_11_0_arm64.whl").unwrap();
        assert_eq!(
            p,
            ParsedWheel {
                name: "dbt_sa_cli".into(),
                version: "2.0.0a1".into(),
                pyversion: "py3".into(),
            }
        );
    }

    #[test]
    fn parse_wheel_filename_handles_build_tag() {
        let p = parse_wheel_filename("dbt_sa_cli-2.0.0a1-1-py3-none-manylinux_2_28_x86_64.whl")
            .unwrap();
        assert_eq!(p.name, "dbt_sa_cli");
        assert_eq!(p.version, "2.0.0a1");
        assert_eq!(p.pyversion, "py3");
    }

    #[test]
    fn parse_wheel_filename_rejects_non_wheel() {
        assert!(parse_wheel_filename("dbt_sa_cli-2.0.0a1.tar.gz").is_none());
        assert!(parse_wheel_filename("dbt_sa_cli-2.0.0a1-py3.whl").is_none());
    }

    fn populate_wheel_dir(dir: &Path, names: &[&str]) {
        for n in names {
            fs::write(dir.join(n), "").unwrap();
        }
    }

    fn file_names(paths: &[PathBuf]) -> Vec<String> {
        paths
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn discover_wheels_filters_by_wheel_name() {
        let tmp = tempfile::tempdir().unwrap();
        populate_wheel_dir(
            tmp.path(),
            &[
                "dbt_sa_cli-2.0.0a1-py3-none-macosx_11_0_arm64.whl",
                "dbt_sa_cli-2.0.0a1-py3-none-manylinux_2_17_x86_64.whl",
                "dbt_sa_cli-2.0.0a2-py3-none-macosx_11_0_arm64.whl",
                "dbt-2.0.0a1-py3-none-macosx_11_0_arm64.whl",
                "dbt_sa_cli-2.0.0a1.txt",
            ],
        );
        let found = discover_wheels(tmp.path(), "dbt-sa-cli", None).unwrap();
        assert_eq!(
            file_names(&found),
            vec![
                "dbt_sa_cli-2.0.0a1-py3-none-macosx_11_0_arm64.whl",
                "dbt_sa_cli-2.0.0a1-py3-none-manylinux_2_17_x86_64.whl",
                "dbt_sa_cli-2.0.0a2-py3-none-macosx_11_0_arm64.whl",
            ]
        );
    }

    fn metadata_from(src: &str) -> Metadata {
        Metadata::parse(src.as_bytes()).expect("parse test METADATA")
    }

    fn field_values<'a>(fields: &'a [(&'static str, String)], name: &str) -> Vec<&'a str> {
        fields
            .iter()
            .filter(|(n, _)| *n == name)
            .map(|(_, v)| v.as_str())
            .collect()
    }

    const SAMPLE_METADATA: &str = "\
Metadata-Version: 2.4
Name: dbt-mongodb
Version: 0.0.1.dev1
Summary: Reserved namespace for a future dbt Labs project.
Project-URL: Homepage, https://github.com/dbt-labs
Project-URL: Repository, https://github.com/dbt-labs/repo
Author-email: dbt Labs <info@dbtlabs.com>
Maintainer-email: dbt Labs <info@dbtlabs.com>
License-Expression: Apache-2.0
License-File: LICENSE
Keywords: dbt,dbt-labs,placeholder
Classifier: Development Status :: 2 - Pre-Alpha
Classifier: Intended Audience :: Developers
Requires-Python: >=3.10
Requires-Dist: pydantic>=2.0
Description-Content-Type: text/markdown

# dbt-mongodb

This package name is reserved by dbt Labs.
";

    #[test]
    fn metadata_form_fields_includes_description_and_content_type() {
        // The original bug: PyPI's project page renders blank without `description`
        // and `description_content_type`, even when METADATA inside the wheel is valid.
        let fields = metadata_form_fields(&metadata_from(SAMPLE_METADATA));
        let desc = field_values(&fields, "description");
        assert_eq!(desc.len(), 1, "expected exactly one description field");
        assert!(desc[0].contains("# dbt-mongodb"));
        assert!(desc[0].contains("reserved by dbt Labs"));
        assert_eq!(
            field_values(&fields, "description_content_type"),
            vec!["text/markdown"]
        );
    }

    #[test]
    fn metadata_form_fields_emits_repeating_headers_separately() {
        let fields = metadata_form_fields(&metadata_from(SAMPLE_METADATA));
        assert_eq!(
            field_values(&fields, "classifiers"),
            vec![
                "Development Status :: 2 - Pre-Alpha",
                "Intended Audience :: Developers",
            ]
        );
        assert_eq!(
            field_values(&fields, "project_urls"),
            vec![
                "Homepage, https://github.com/dbt-labs",
                "Repository, https://github.com/dbt-labs/repo",
            ]
        );
        assert_eq!(
            field_values(&fields, "requires_dist"),
            vec!["pydantic>=2.0"]
        );
        assert_eq!(field_values(&fields, "license_file"), vec!["LICENSE"]);
    }

    #[test]
    fn metadata_form_fields_uses_warehouse_field_names() {
        // python-pkginfo names them `platforms`/`supported_platforms`/`provides_extras`/
        // `license_files`, but Warehouse expects the singular form in the multipart body.
        let md = Metadata {
            platforms: vec!["any".into()],
            supported_platforms: vec!["linux".into()],
            provides_extras: vec!["dev".into()],
            license_files: vec!["LICENSE".into()],
            ..Metadata::default()
        };
        let fields = metadata_form_fields(&md);
        assert_eq!(field_values(&fields, "platform"), vec!["any"]);
        assert_eq!(field_values(&fields, "supported_platform"), vec!["linux"]);
        assert_eq!(field_values(&fields, "provides_extra"), vec!["dev"]);
        assert_eq!(field_values(&fields, "license_file"), vec!["LICENSE"]);
        assert!(field_values(&fields, "platforms").is_empty());
        assert!(field_values(&fields, "license_files").is_empty());
    }

    #[test]
    fn metadata_form_fields_skips_absent_single_valued_fields() {
        let md = Metadata {
            summary: Some("hi".into()),
            ..Metadata::default()
        };
        let fields = metadata_form_fields(&md);
        assert_eq!(field_values(&fields, "summary"), vec!["hi"]);
        for absent in [
            "description",
            "description_content_type",
            "author",
            "author_email",
            "license",
            "license_expression",
            "home_page",
            "keywords",
            "requires_python",
        ] {
            assert!(
                field_values(&fields, absent).is_empty(),
                "{absent} should be omitted when None"
            );
        }
    }

    #[test]
    fn discover_wheels_filters_by_version_when_requested() {
        let tmp = tempfile::tempdir().unwrap();
        populate_wheel_dir(
            tmp.path(),
            &[
                "dbt_sa_cli-2.0.0a1-py3-none-macosx_11_0_arm64.whl",
                "dbt_sa_cli-2.0.0a1-py3-none-manylinux_2_17_x86_64.whl",
                "dbt_sa_cli-2.0.0a2-py3-none-macosx_11_0_arm64.whl", // stale
            ],
        );
        let found = discover_wheels(tmp.path(), "dbt-sa-cli", Some("2.0.0-alpha.1")).unwrap();
        assert_eq!(
            file_names(&found),
            vec![
                "dbt_sa_cli-2.0.0a1-py3-none-macosx_11_0_arm64.whl",
                "dbt_sa_cli-2.0.0a1-py3-none-manylinux_2_17_x86_64.whl",
            ],
        );
    }
}
