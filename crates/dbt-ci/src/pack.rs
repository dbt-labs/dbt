use crate::args::PackArgs;
use crate::pyproject::{self, Spec};
use crate::release_version::parse_release_version;
use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use zip::CompressionMethod;
use zip::write::{SimpleFileOptions, ZipWriter};

pub fn execute(args: PackArgs) -> ExitCode {
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::from(2)
        }
    }
}

fn run(args: PackArgs) -> Result<()> {
    let version_pep440 = parse_release_version(&args.version)
        .with_context(|| format!("invalid --version {:?}", args.version))?
        .to_pep440();

    let spec = pyproject::discover()?;
    let bin_name = args
        .bin_name
        .clone()
        .unwrap_or_else(|| spec.wheel_name.clone());
    let out_dir = args
        .out
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| spec.pyproject_dir.join("target/wheels"));
    fs::create_dir_all(&out_dir).with_context(|| format!("create {}", out_dir.display()))?;

    eprintln!("→ wheel name:   {}", spec.wheel_name);
    eprintln!("→ wheel ver:    {version_pep440}");
    eprintln!("→ bin name:     {bin_name}");
    eprintln!("→ binaries dir: {}", args.binaries_dir.display());
    eprintln!("→ out:          {}", out_dir.display());

    let binaries = collect_binaries(&args.binaries_dir)?;
    if binaries.is_empty() {
        bail!(
            "no binaries found under {} (expected files named by cargo target triple)",
            args.binaries_dir.display()
        );
    }

    for bin in &binaries {
        let path = pack_wheel(&spec, &version_pep440, &bin_name, bin, &out_dir)
            .with_context(|| format!("pack {}", bin.path.display()))?;
        eprintln!("✓ {}", path.display());
    }
    eprintln!(
        "\n{} wheel(s) written to {}",
        binaries.len(),
        out_dir.display()
    );
    Ok(())
}

#[derive(Debug)]
struct Binary {
    path: PathBuf,
    target_triple: String,
    is_windows: bool,
}

fn collect_binaries(dir: &Path) -> Result<Vec<Binary>> {
    let entries = fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))?;
    let mut out = Vec::new();
    for entry in entries {
        let entry = entry?;
        let p = entry.path();
        if !p.is_file() {
            continue;
        }
        let Some(name) = p.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        let (triple, is_windows) = match name.strip_suffix(".exe") {
            Some(stem) => (stem.to_string(), true),
            None => (name.to_string(), false),
        };
        if target_to_platform_tag(&triple).is_none() {
            continue;
        }
        out.push(Binary {
            path: p,
            target_triple: triple,
            is_windows,
        });
    }
    out.sort_by(|a, b| a.target_triple.cmp(&b.target_triple));
    Ok(out)
}

/// PEP 491 wheel filename; the single source of truth shared by `pack` and `sdist`.
/// `python_tag`/`abi_tag` are the interpreter/ABI compatibility tags — `py3`/`none`
/// for the binary CLI wheel, `cp310`/`abi3` for the maturin `dbt-core` extension.
pub(crate) fn wheel_filename(
    dist_normalized: &str,
    version_pep440: &str,
    python_tag: &str,
    abi_tag: &str,
    platform_tag: &str,
) -> String {
    format!("{dist_normalized}-{version_pep440}-{python_tag}-{abi_tag}-{platform_tag}.whl")
}

/// Packs one binary into a wheel under `out_dir`, returning the wheel path.
fn pack_wheel(
    spec: &Spec,
    version_pep440: &str,
    bin_name: &str,
    bin: &Binary,
    out_dir: &Path,
) -> Result<PathBuf> {
    let platform_tag = target_to_platform_tag(&bin.target_triple)
        .ok_or_else(|| anyhow!("unsupported target triple {:?}", bin.target_triple))?;
    let dist = normalize_wheel_name(&spec.wheel_name);
    // The CLI wheel wraps a prebuilt binary — interpreter-agnostic, so `py3-none`.
    let wheel_filename = wheel_filename(&dist, version_pep440, "py3", "none", &platform_tag);
    let wheel_path = out_dir.join(&wheel_filename);
    let dist_info = format!("{dist}-{version_pep440}.dist-info");
    let data_scripts = format!("{dist}-{version_pep440}.data/scripts");

    let bin_bytes = fs::read(&bin.path).with_context(|| format!("read {}", bin.path.display()))?;
    let script_name = if bin.is_windows {
        format!("{bin_name}.exe")
    } else {
        bin_name.to_string()
    };

    let mut entries: Vec<(String, Vec<u8>, u32)> = Vec::new();
    entries.push((format!("{data_scripts}/{script_name}"), bin_bytes, 0o755));
    entries.push((
        format!("{dist_info}/METADATA"),
        render_metadata(spec, version_pep440).into_bytes(),
        0o644,
    ));
    entries.push((
        format!("{dist_info}/WHEEL"),
        render_wheel_file(&platform_tag).into_bytes(),
        0o644,
    ));

    let record_path = format!("{dist_info}/RECORD");
    let record = render_record(&entries, &record_path);
    entries.push((record_path, record.into_bytes(), 0o644));

    write_zip(&wheel_path, &entries)?;
    Ok(wheel_path)
}

fn write_zip(out_path: &Path, entries: &[(String, Vec<u8>, u32)]) -> Result<()> {
    let file = File::create(out_path).with_context(|| format!("create {}", out_path.display()))?;
    let mut zip = ZipWriter::new(file);
    for (path, bytes, mode) in entries {
        let opts = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .compression_level(Some(9))
            .unix_permissions(*mode);
        zip.start_file(path, opts)
            .with_context(|| format!("zip start_file {path}"))?;
        zip.write_all(bytes)
            .with_context(|| format!("zip write {path}"))?;
    }
    zip.finish().context("zip finish")?;
    Ok(())
}

/// PEP 376 RECORD: `path,sha256=<b64>,<size>` per file; RECORD self-line is `path,,`.
fn render_record(entries: &[(String, Vec<u8>, u32)], record_path: &str) -> String {
    let mut out = String::new();
    for (path, bytes, _) in entries {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        let digest = hasher.finalize();
        let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
        let _ = writeln!(out, "{path},sha256={b64},{}", bytes.len());
    }
    out.push_str(record_path);
    out.push_str(",,\n");
    out
}

pub(crate) fn render_metadata(spec: &Spec, version_pep440: &str) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "Metadata-Version: 2.1");
    let _ = writeln!(out, "Name: {}", spec.wheel_name);
    let _ = writeln!(out, "Version: {version_pep440}");
    if let Some(s) = &spec.summary {
        let _ = writeln!(out, "Summary: {s}");
    }
    if let Some(rp) = &spec.requires_python {
        let _ = writeln!(out, "Requires-Python: {rp}");
    }
    for dep in &spec.dependencies {
        let _ = writeln!(out, "Requires-Dist: {dep}");
    }
    if let Some(l) = &spec.license {
        let _ = writeln!(out, "License: {}", flatten_for_header(l));
    }
    for c in &spec.classifiers {
        let _ = writeln!(out, "Classifier: {c}");
    }
    for (label, url) in &spec.urls {
        let _ = writeln!(out, "Project-URL: {label}, {url}");
    }
    for author in &spec.authors {
        match (&author.name, &author.email) {
            (Some(n), Some(e)) => {
                let _ = writeln!(out, "Author-email: {n} <{e}>");
            }
            (Some(n), None) => {
                let _ = writeln!(out, "Author: {n}");
            }
            (None, Some(e)) => {
                let _ = writeln!(out, "Author-email: <{e}>");
            }
            (None, None) => {}
        }
    }
    if let Some(ct) = &spec.description_content_type {
        let _ = writeln!(out, "Description-Content-Type: {ct}");
    }
    if let Some(body) = &spec.description {
        // Blank line separates headers from body, per PEP 566 / RFC 822.
        out.push('\n');
        out.push_str(body);
        if !body.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

/// RFC 822 headers are single-line; collapse newlines in a freeform value
/// (multi-line license texts) to spaces.
fn flatten_for_header(s: &str) -> String {
    s.replace(['\n', '\r'], " ").trim().to_string()
}

fn render_wheel_file(platform_tag: &str) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "Wheel-Version: 1.0");
    let _ = writeln!(out, "Generator: dbt-ci");
    let _ = writeln!(out, "Root-Is-Purelib: false");
    let _ = writeln!(out, "Tag: py3-none-{platform_tag}");
    out
}

/// PEP 503: lowercase, collapse runs of `-`/`_`/`.` to a single `_`.
pub(crate) fn normalize_wheel_name(name: &str) -> String {
    let lower = name.to_lowercase();
    let mut out = String::with_capacity(lower.len());
    let mut last_was_sep = false;
    for c in lower.chars() {
        if c == '-' || c == '_' || c == '.' {
            if !last_was_sep {
                out.push('_');
                last_was_sep = true;
            }
        } else {
            out.push(c);
            last_was_sep = false;
        }
    }
    out
}

pub(crate) fn target_to_platform_tag(triple: &str) -> Option<String> {
    Some(match triple {
        "x86_64-unknown-linux-gnu" => "manylinux_2_28_x86_64".to_string(),
        "aarch64-unknown-linux-gnu" => "manylinux_2_28_aarch64".to_string(),
        "i686-unknown-linux-gnu" => "manylinux_2_28_i686".to_string(),
        "x86_64-unknown-linux-musl" => "musllinux_1_2_x86_64".to_string(),
        "aarch64-unknown-linux-musl" => "musllinux_1_2_aarch64".to_string(),
        "x86_64-apple-darwin" => "macosx_10_12_x86_64".to_string(),
        "aarch64-apple-darwin" => "macosx_11_0_arm64".to_string(),
        "x86_64-pc-windows-msvc" | "x86_64-pc-windows-gnu" => "win_amd64".to_string(),
        "i686-pc-windows-msvc" => "win32".to_string(),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pyproject::Author;
    use std::io::Read;
    use zip::ZipArchive;

    #[test]
    fn normalize_wheel_name_collapses_separators() {
        assert_eq!(normalize_wheel_name("dbt-sa-cli"), "dbt_sa_cli");
        assert_eq!(normalize_wheel_name("dbt.sa_cli"), "dbt_sa_cli");
        assert_eq!(normalize_wheel_name("DBT-SA-CLI"), "dbt_sa_cli");
        assert_eq!(normalize_wheel_name("dbt--sa..cli"), "dbt_sa_cli");
        assert_eq!(normalize_wheel_name("plain"), "plain");
    }

    #[test]
    fn wheel_filename_carries_interpreter_and_abi_tags() {
        // Binary CLI wheel: interpreter-agnostic.
        assert_eq!(
            wheel_filename("dbt", "2.0.0", "py3", "none", "manylinux_2_28_x86_64"),
            "dbt-2.0.0-py3-none-manylinux_2_28_x86_64.whl"
        );
        // maturin abi3 extension wheel (the `dbt-core` package).
        assert_eq!(
            wheel_filename("dbt_core", "2.0.0", "cp310", "abi3", "macosx_11_0_arm64"),
            "dbt_core-2.0.0-cp310-abi3-macosx_11_0_arm64.whl"
        );
    }

    #[test]
    fn target_to_platform_tag_handles_known_triples() {
        assert_eq!(
            target_to_platform_tag("x86_64-unknown-linux-gnu").as_deref(),
            Some("manylinux_2_28_x86_64")
        );
        assert_eq!(
            target_to_platform_tag("aarch64-unknown-linux-gnu").as_deref(),
            Some("manylinux_2_28_aarch64")
        );
        assert_eq!(
            target_to_platform_tag("x86_64-apple-darwin").as_deref(),
            Some("macosx_10_12_x86_64")
        );
        assert_eq!(
            target_to_platform_tag("aarch64-apple-darwin").as_deref(),
            Some("macosx_11_0_arm64")
        );
        assert_eq!(
            target_to_platform_tag("x86_64-pc-windows-msvc").as_deref(),
            Some("win_amd64")
        );
    }

    #[test]
    fn target_to_platform_tag_rejects_unknown() {
        assert!(target_to_platform_tag("riscv64gc-unknown-linux-gnu").is_none());
    }

    #[test]
    fn collect_binaries_picks_only_triple_named_files() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        fs::write(dir.join("x86_64-unknown-linux-gnu"), b"\x7fELF").unwrap();
        fs::write(dir.join("x86_64-pc-windows-msvc.exe"), b"MZ").unwrap();
        fs::write(dir.join("README.md"), b"hi").unwrap();
        fs::write(dir.join(".DS_Store"), b"junk").unwrap();
        fs::write(dir.join("LICENSE-MIT-2020"), b"license body").unwrap();
        fs::write(dir.join("notes-for-the-reviewer.txt"), b"txt").unwrap();
        let bins = collect_binaries(dir).unwrap();
        assert_eq!(bins.len(), 2);
        let triples: Vec<&str> = bins.iter().map(|b| b.target_triple.as_str()).collect();
        assert!(triples.contains(&"x86_64-unknown-linux-gnu"));
        assert!(triples.contains(&"x86_64-pc-windows-msvc"));
        let win = bins
            .iter()
            .find(|b| b.target_triple == "x86_64-pc-windows-msvc")
            .unwrap();
        assert!(win.is_windows);
    }

    #[test]
    fn pack_wheel_emits_wheel_with_expected_structure() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let bin_path = dir.join("x86_64-unknown-linux-gnu");
        fs::write(&bin_path, b"#!/bin/sh\necho hi\n").unwrap();
        let bin = Binary {
            path: bin_path,
            target_triple: "x86_64-unknown-linux-gnu".to_string(),
            is_windows: false,
        };
        let spec = Spec {
            wheel_name: "dbt-sa-cli".to_string(),
            pyproject_dir: dir.to_path_buf(),
            summary: Some("dbt fusion standalone analyzer CLI".to_string()),
            requires_python: Some(">=3.9".to_string()),
            dependencies: vec!["mashumaro[msgpack]>=3.14".to_string()],
            classifiers: vec!["Programming Language :: Rust".to_string()],
            urls: vec![("Homepage".to_string(), "https://getdbt.com".to_string())],
            authors: vec![Author {
                name: Some("dbt Labs".to_string()),
                email: Some("info@dbtlabs.com".to_string()),
            }],
            license: Some("Apache-2.0".to_string()),
            description: Some("# dbt-sa-cli\n\nLong body.\n".to_string()),
            description_content_type: Some("text/markdown".to_string()),
        };

        let out = pack_wheel(&spec, "2.0.0a1", "dbt-sa-cli", &bin, dir).unwrap();
        assert!(out.exists());
        assert_eq!(
            out.file_name().unwrap().to_str().unwrap(),
            "dbt_sa_cli-2.0.0a1-py3-none-manylinux_2_28_x86_64.whl"
        );

        let file = File::open(&out).unwrap();
        let mut zip = ZipArchive::new(file).unwrap();
        let mut paths: Vec<String> = (0..zip.len())
            .map(|i| zip.by_index(i).unwrap().name().to_string())
            .collect();
        paths.sort();
        assert_eq!(
            paths,
            vec![
                "dbt_sa_cli-2.0.0a1.data/scripts/dbt-sa-cli".to_string(),
                "dbt_sa_cli-2.0.0a1.dist-info/METADATA".to_string(),
                "dbt_sa_cli-2.0.0a1.dist-info/RECORD".to_string(),
                "dbt_sa_cli-2.0.0a1.dist-info/WHEEL".to_string(),
            ]
        );

        let mut metadata = String::new();
        zip.by_name("dbt_sa_cli-2.0.0a1.dist-info/METADATA")
            .unwrap()
            .read_to_string(&mut metadata)
            .unwrap();
        assert!(metadata.contains("Name: dbt-sa-cli"));
        assert!(metadata.contains("Version: 2.0.0a1"));
        assert!(metadata.contains("Summary: dbt fusion standalone analyzer CLI"));
        assert!(metadata.contains("Requires-Python: >=3.9"));
        // Dropping these leaves an installer that trusts static metadata with a
        // dependency-free install.
        assert!(metadata.contains("Requires-Dist: mashumaro[msgpack]>=3.14"));
        assert!(metadata.contains("Classifier: Programming Language :: Rust"));
        assert!(metadata.contains("Project-URL: Homepage, https://getdbt.com"));
        assert!(metadata.contains("Author-email: dbt Labs <info@dbtlabs.com>"));
        assert!(metadata.contains("License: Apache-2.0"));
        assert!(metadata.contains("Description-Content-Type: text/markdown"));
        // Body sits after a blank line, RFC 822 style.
        assert!(metadata.contains("\n\n# dbt-sa-cli\n\nLong body.\n"));

        let mut wheel_file = String::new();
        zip.by_name("dbt_sa_cli-2.0.0a1.dist-info/WHEEL")
            .unwrap()
            .read_to_string(&mut wheel_file)
            .unwrap();
        assert!(wheel_file.contains("Tag: py3-none-manylinux_2_28_x86_64"));
        assert!(wheel_file.contains("Root-Is-Purelib: false"));

        let mut record = String::new();
        zip.by_name("dbt_sa_cli-2.0.0a1.dist-info/RECORD")
            .unwrap()
            .read_to_string(&mut record)
            .unwrap();
        assert!(record.contains("dbt_sa_cli-2.0.0a1.data/scripts/dbt-sa-cli,sha256="));
        assert!(record.contains("dbt_sa_cli-2.0.0a1.dist-info/METADATA,sha256="));
        assert!(record.contains("dbt_sa_cli-2.0.0a1.dist-info/WHEEL,sha256="));
        assert!(record.contains("dbt_sa_cli-2.0.0a1.dist-info/RECORD,,\n"));
    }

    #[test]
    fn pack_wheel_windows_appends_exe_to_script() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let bin_path = dir.join("x86_64-pc-windows-msvc.exe");
        fs::write(&bin_path, b"MZ\x00\x00").unwrap();
        let bin = Binary {
            path: bin_path,
            target_triple: "x86_64-pc-windows-msvc".to_string(),
            is_windows: true,
        };
        let spec = Spec {
            wheel_name: "dbt-sa-cli".to_string(),
            pyproject_dir: dir.to_path_buf(),
            summary: None,
            requires_python: None,
            dependencies: Vec::new(),
            classifiers: vec![],
            urls: vec![],
            authors: vec![],
            license: None,
            description: None,
            description_content_type: None,
        };
        let out = pack_wheel(&spec, "2.0.0", "dbt-sa-cli", &bin, dir).unwrap();
        assert_eq!(
            out.file_name().unwrap().to_str().unwrap(),
            "dbt_sa_cli-2.0.0-py3-none-win_amd64.whl"
        );
        let file = File::open(&out).unwrap();
        let zip = ZipArchive::new(file).unwrap();
        let paths: Vec<&str> = zip.file_names().collect();
        assert!(paths.contains(&"dbt_sa_cli-2.0.0.data/scripts/dbt-sa-cli.exe"));
    }
}
