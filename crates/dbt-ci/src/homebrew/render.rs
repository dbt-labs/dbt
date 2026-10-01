//! Renders a Homebrew formula from pre-built release tarballs.
//!
//! Inputs:
//!   - A directory of `{prefix}{version}-{target}.tar.gz` tarballs (one per
//!     cargo target triple).
//!   - A URL template that resolves to the public download URL of each tarball.
//!   - PEP 621 metadata from the workspace `pyproject.toml` (name, license,
//!     homepage, summary).
//!
//! Output: a single `.rb` file with `on_macos`/`on_linux` × `on_arm`/`on_intel`
//! blocks for the four brew-supported targets. Windows tarballs in the input
//! dir are ignored.

use crate::args::HomebrewRenderArgs;
use crate::pyproject::{self, Spec};
use crate::release_version::validate_release_version;
use crate::utils::cargo_workspace_root;
use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

pub fn execute(args: HomebrewRenderArgs) -> ExitCode {
    match run(args) {
        Ok(out) => {
            eprintln!("✓ {}", out.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::from(2)
        }
    }
}

fn run(args: HomebrewRenderArgs) -> Result<PathBuf> {
    validate_release_version(&args.version)
        .with_context(|| format!("invalid --version {:?}", args.version))?;

    let spec = pyproject::discover()?;
    let formula_name = args
        .formula_name
        .clone()
        .unwrap_or_else(|| spec.wheel_name.clone());

    // clap's `conflicts_with` + `required_unless_present` enforces that
    // exactly one input is provided — but we exhaustively pattern-match
    // anyway to make the source-of-truth obvious to readers.
    let (tarballs, source_desc) = match (&args.tarballs_dir, &args.sha256sums) {
        (Some(dir), None) => (
            collect_tarballs_from_dir(dir, &args.tarball_prefix, &args.version)?,
            format!("dir {}", dir.display()),
        ),
        (None, Some(file)) => (
            collect_tarballs_from_sha256sums(file, &args.tarball_prefix, &args.version)?,
            format!("manifest {}", file.display()),
        ),
        (None, None) | (Some(_), Some(_)) => {
            unreachable!("clap should reject neither/both --tarballs-dir/--sha256sums")
        }
    };
    if tarballs.is_empty() {
        bail!(
            "no `{prefix}{version}-*.tar.gz` tarballs found in {source_desc}",
            prefix = args.tarball_prefix,
            version = args.version,
        );
    }

    let platforms = resolve_platforms(&tarballs, &args.url_template, &args.version);
    if platforms.is_empty() {
        bail!(
            "no brew-supported targets found in {source_desc} (need at least one of: \
             aarch64-apple-darwin, x86_64-apple-darwin, aarch64-unknown-linux-gnu, x86_64-unknown-linux-gnu)",
        );
    }

    // `install_as` defaults to the formula name — preserves the historic
    // behavior where `bin.install "X"` is renamed to match the formula's
    // filename when binary-name and formula-name differ.
    let install_as = args
        .install_as
        .clone()
        .unwrap_or_else(|| formula_name.clone());

    let ctx = FormulaCtx {
        class_name: pascal_case(&formula_name),
        binary_name: args.binary_name.clone(),
        install_as,
        version: args.version.clone(),
        homepage: pick_homepage(&spec),
        description: spec.summary.clone(),
        license: spec.license.clone(),
        conflicts_with: args.conflicts_with.clone(),
        platforms,
    };
    let rendered = render_formula(&ctx);

    let out_path = args.out.unwrap_or_else(|| {
        cargo_workspace_root()
            .join("target/homebrew/Formula")
            .join(format!("{formula_name}.rb"))
    });
    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    fs::write(&out_path, rendered).with_context(|| format!("write {}", out_path.display()))?;
    Ok(out_path)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Tarball {
    /// Cargo target triple, e.g. `aarch64-apple-darwin`.
    target_triple: String,
    /// Filename without leading path, e.g. `fs-v2.0.0-aarch64-apple-darwin.tar.gz`.
    filename: String,
    /// Lowercase hex SHA256 of the tarball bytes.
    sha256: String,
}

/// Walks `dir` recursively, hashing each matching tarball. Recursion handles
/// the `actions/download-artifact` layout where each artifact lands in its own
/// subdirectory (e.g. `aarch64-apple-darwin-release/fs-v...tar.gz`). Symlinked
/// dirs are not followed to keep the walk bounded.
fn collect_tarballs_from_dir(dir: &Path, prefix: &str, version: &str) -> Result<Vec<Tarball>> {
    let leader = format!("{prefix}{version}-");
    let mut out = Vec::new();
    let mut stack: Vec<PathBuf> = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let entries = fs::read_dir(&d).with_context(|| format!("read {}", d.display()))?;
        for entry in entries {
            let entry = entry?;
            let file_type = entry.file_type()?;
            let p = entry.path();
            if file_type.is_dir() {
                stack.push(p);
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            let Some(name) = p.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            let Some(rest) = name.strip_prefix(&leader) else {
                continue;
            };
            let Some(target_triple) = rest.strip_suffix(".tar.gz") else {
                continue;
            };
            let sha256 = sha256_hex_file(&p).with_context(|| format!("sha256 {}", p.display()))?;
            out.push(Tarball {
                target_triple: target_triple.to_string(),
                filename: name.to_string(),
                sha256,
            });
        }
    }
    out.sort_by(|a, b| a.target_triple.cmp(&b.target_triple));
    Ok(out)
}

/// Parses a `sha256sum`-format manifest (`<hex>  <filename>` per line — also
/// accepts `<hex> *<filename>` binary-mode and the GNU-coreutils blank-line /
/// trailing-whitespace variants). Only lines matching the tarball prefix +
/// version are kept; wheels, zips, etc. are silently skipped.
fn collect_tarballs_from_sha256sums(
    path: &Path,
    prefix: &str,
    version: &str,
) -> Result<Vec<Tarball>> {
    let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let leader = format!("{prefix}{version}-");
    let mut out = Vec::new();
    for (raw, lineno) in text.lines().zip(1u32..) {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // `<hex>  <filename>` — split on first whitespace run.
        let mut parts = line.splitn(2, char::is_whitespace);
        let (Some(sha), Some(rest)) = (parts.next(), parts.next()) else {
            bail!("{}:{lineno}: malformed line {:?}", path.display(), raw);
        };
        if sha.len() != 64 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
            bail!(
                "{}:{lineno}: expected 64-char hex sha256, got {:?}",
                path.display(),
                sha,
            );
        }
        // `sha256sum -b` prepends a single `*` to the filename. Some
        // implementations leave extra whitespace before the name. Strip both.
        let trimmed = rest.trim_start();
        let filename = trimmed.strip_prefix('*').unwrap_or(trimmed);
        let Some(after_leader) = filename.strip_prefix(&leader) else {
            continue;
        };
        let Some(target_triple) = after_leader.strip_suffix(".tar.gz") else {
            continue;
        };
        out.push(Tarball {
            target_triple: target_triple.to_string(),
            filename: filename.to_string(),
            sha256: sha.to_ascii_lowercase(),
        });
    }
    out.sort_by(|a, b| a.target_triple.cmp(&b.target_triple));
    Ok(out)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BrewOs {
    Macos,
    Linux,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BrewArch {
    Arm,
    Intel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Platform {
    os: BrewOs,
    arch: BrewArch,
    url: String,
    sha256: String,
}

fn target_to_brew(triple: &str) -> Option<(BrewOs, BrewArch)> {
    Some(match triple {
        "aarch64-apple-darwin" => (BrewOs::Macos, BrewArch::Arm),
        "x86_64-apple-darwin" => (BrewOs::Macos, BrewArch::Intel),
        "aarch64-unknown-linux-gnu" => (BrewOs::Linux, BrewArch::Arm),
        "x86_64-unknown-linux-gnu" => (BrewOs::Linux, BrewArch::Intel),
        _ => return None,
    })
}

fn resolve_platforms(tarballs: &[Tarball], url_template: &str, version: &str) -> Vec<Platform> {
    let mut out = Vec::new();
    for t in tarballs {
        let Some((os, arch)) = target_to_brew(&t.target_triple) else {
            continue; // skip windows / unknown
        };
        let url = expand_url_template(url_template, &t.filename, version, &t.target_triple);
        out.push(Platform {
            os,
            arch,
            url,
            sha256: t.sha256.clone(),
        });
    }
    // Stable order: macos arm, macos intel, linux arm, linux intel.
    out.sort_by_key(|p| {
        (
            matches!(p.os, BrewOs::Linux),
            matches!(p.arch, BrewArch::Intel),
        )
    });
    out
}

fn expand_url_template(template: &str, filename: &str, version: &str, target: &str) -> String {
    template
        .replace("{filename}", filename)
        .replace("{version}", version)
        .replace("{target}", target)
}

fn sha256_hex_file(path: &Path) -> Result<String> {
    let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex_encode(&hasher.finalize()))
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// PascalCase for Homebrew formula class names: `dbt-core` → `DbtCore`,
/// `dbt` → `Dbt`. Numbers and other non-letter chars are kept verbatim.
fn pascal_case(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut next_upper = true;
    for c in name.chars() {
        if c == '-' || c == '_' || c == '.' || c == ' ' {
            next_upper = true;
            continue;
        }
        if next_upper {
            out.extend(c.to_uppercase());
            next_upper = false;
        } else {
            out.extend(c.to_lowercase());
        }
    }
    out
}

fn pick_homepage(spec: &Spec) -> Option<String> {
    // Prefer an explicit "Homepage" URL; fall back to the first one.
    spec.urls
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("homepage"))
        .map(|(_, v)| v.clone())
        .or_else(|| spec.urls.first().map(|(_, v)| v.clone()))
}

struct FormulaCtx {
    class_name: String,
    /// Filename of the binary inside the tarball — used as the LHS of
    /// `bin.install`. Doesn't have to match either the formula name or the
    /// final installed name.
    binary_name: String,
    /// Name the binary should be installed as. If different from
    /// `binary_name`, renders as `bin.install "<binary>" => "<install_as>"`.
    /// Otherwise plain `bin.install "<binary>"`.
    install_as: String,
    /// Required even though brew can often parse it from the URL — brew's
    /// auto-extractor mangles prerelease suffixes (e.g. `2.0.1-preview.5`
    /// becomes `2.0.1-pre`, `2.0.0-dev.9` becomes `2.0.0`).
    version: String,
    homepage: Option<String>,
    description: Option<String>,
    license: Option<String>,
    conflicts_with: Vec<String>,
    platforms: Vec<Platform>,
}

fn render_formula(ctx: &FormulaCtx) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "class {} < Formula", ctx.class_name);
    if let Some(d) = &ctx.description {
        let _ = writeln!(out, "  desc {}", ruby_string(d));
    }
    if let Some(h) = &ctx.homepage {
        let _ = writeln!(out, "  homepage {}", ruby_string(h));
    }
    let _ = writeln!(out, "  version {}", ruby_string(&ctx.version));
    let _ = writeln!(out, "  license {}", render_license(ctx.license.as_deref()));

    out.push('\n');
    let macos: Vec<&Platform> = ctx
        .platforms
        .iter()
        .filter(|p| p.os == BrewOs::Macos)
        .collect();
    let linux: Vec<&Platform> = ctx
        .platforms
        .iter()
        .filter(|p| p.os == BrewOs::Linux)
        .collect();

    if !macos.is_empty() {
        let _ = writeln!(out, "  on_macos do");
        emit_arch_blocks(&mut out, &macos, "    ");
        let _ = writeln!(out, "  end");
    }
    if !linux.is_empty() {
        if !macos.is_empty() {
            out.push('\n');
        }
        let _ = writeln!(out, "  on_linux do");
        emit_arch_blocks(&mut out, &linux, "    ");
        let _ = writeln!(out, "  end");
    }

    out.push('\n');
    for other in &ctx.conflicts_with {
        let _ = writeln!(
            out,
            "  conflicts_with {}, because: \"both formulas install the `{}` binary\"",
            ruby_string(other),
            ctx.install_as,
        );
    }
    if !ctx.conflicts_with.is_empty() {
        out.push('\n');
    }
    let _ = writeln!(out, "  def install");
    if ctx.binary_name == ctx.install_as {
        let _ = writeln!(out, "    bin.install {}", ruby_string(&ctx.binary_name));
    } else {
        let _ = writeln!(
            out,
            "    bin.install {} => {}",
            ruby_string(&ctx.binary_name),
            ruby_string(&ctx.install_as),
        );
    }
    let _ = writeln!(out, "  end");

    out.push('\n');
    let _ = writeln!(out, "  test do");
    let _ = writeln!(
        out,
        "    assert_match version.to_s, shell_output(\"#{{bin}}/{invoked} --version\")",
        invoked = ctx.install_as,
    );
    let _ = writeln!(out, "  end");
    let _ = writeln!(out, "end");
    out
}

fn emit_arch_blocks(out: &mut String, platforms: &[&Platform], indent: &str) {
    let arm = platforms.iter().find(|p| p.arch == BrewArch::Arm);
    let intel = platforms.iter().find(|p| p.arch == BrewArch::Intel);
    if let Some(p) = arm {
        let _ = writeln!(out, "{indent}on_arm do");
        let _ = writeln!(out, "{indent}  url {}", ruby_string(&p.url));
        let _ = writeln!(out, "{indent}  sha256 {}", ruby_string(&p.sha256));
        let _ = writeln!(out, "{indent}end");
    }
    if let Some(p) = intel {
        let _ = writeln!(out, "{indent}on_intel do");
        let _ = writeln!(out, "{indent}  url {}", ruby_string(&p.url));
        let _ = writeln!(out, "{indent}  sha256 {}", ruby_string(&p.sha256));
        let _ = writeln!(out, "{indent}end");
    }
}

/// Homebrew accepts SPDX identifiers as `license "SPDX"`. Anything that doesn't
/// look like a clean SPDX (whitespace, newlines, or empty) gets the escape
/// hatch `:cannot_represent`.
fn render_license(license: Option<&str>) -> String {
    let Some(raw) = license else {
        return ":cannot_represent".to_string();
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.chars().any(|c| c.is_whitespace()) {
        return ":cannot_represent".to_string();
    }
    if trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '+'))
    {
        ruby_string(trimmed)
    } else {
        ":cannot_represent".to_string()
    }
}

/// Ruby double-quoted string with conservative escaping: backslash, double
/// quote, and `#` (to neutralize accidental `#{…}` interpolation).
fn ruby_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '#' => out.push_str("\\#"),
            '\n' => out.push_str("\\n"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct TempDir(PathBuf);
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn tmpdir(stem: &str) -> TempDir {
        static N: AtomicUsize = AtomicUsize::new(0);
        let n = N.fetch_add(1, Ordering::SeqCst);
        let p = env::temp_dir().join(format!("dbt-ci-brew-{stem}-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        TempDir(p)
    }

    #[test]
    fn pascal_case_kebab_to_pascal() {
        assert_eq!(pascal_case("dbt"), "Dbt");
        assert_eq!(pascal_case("dbt-core"), "DbtCore");
        assert_eq!(pascal_case("dbt_core"), "DbtCore");
        assert_eq!(pascal_case("DBT-CORE"), "DbtCore");
        assert_eq!(pascal_case("foo-bar-baz"), "FooBarBaz");
    }

    #[test]
    fn expand_url_template_substitutes_all_placeholders() {
        let got = expand_url_template(
            "https://cdn/{filename}",
            "fs-v2.0.0-aarch64-apple-darwin.tar.gz",
            "2.0.0",
            "aarch64-apple-darwin",
        );
        assert_eq!(got, "https://cdn/fs-v2.0.0-aarch64-apple-darwin.tar.gz");
        let got = expand_url_template(
            "https://cdn/{version}/{target}.tar.gz",
            "ignored.tar.gz",
            "2.0.0",
            "aarch64-apple-darwin",
        );
        assert_eq!(got, "https://cdn/2.0.0/aarch64-apple-darwin.tar.gz");
    }

    #[test]
    fn target_to_brew_maps_four_supported_triples() {
        assert_eq!(
            target_to_brew("aarch64-apple-darwin"),
            Some((BrewOs::Macos, BrewArch::Arm))
        );
        assert_eq!(
            target_to_brew("x86_64-apple-darwin"),
            Some((BrewOs::Macos, BrewArch::Intel))
        );
        assert_eq!(
            target_to_brew("aarch64-unknown-linux-gnu"),
            Some((BrewOs::Linux, BrewArch::Arm))
        );
        assert_eq!(
            target_to_brew("x86_64-unknown-linux-gnu"),
            Some((BrewOs::Linux, BrewArch::Intel))
        );
        assert!(target_to_brew("x86_64-pc-windows-msvc").is_none());
    }

    #[test]
    fn collect_tarballs_picks_matching_prefix_and_version() {
        let tmp = tmpdir("collect");
        fs::write(tmp.0.join("fs-v2.0.0-aarch64-apple-darwin.tar.gz"), b"a").unwrap();
        fs::write(tmp.0.join("fs-v2.0.0-x86_64-apple-darwin.tar.gz"), b"b").unwrap();
        fs::write(
            tmp.0.join("fs-v2.0.0-x86_64-unknown-linux-gnu.tar.gz"),
            b"c",
        )
        .unwrap();
        fs::write(
            tmp.0.join("fs-v2.0.0-aarch64-unknown-linux-gnu.tar.gz"),
            b"d",
        )
        .unwrap();
        // Noise — wrong version, wrong prefix, wrong suffix:
        fs::write(tmp.0.join("fs-v1.9.0-aarch64-apple-darwin.tar.gz"), b"x").unwrap();
        fs::write(tmp.0.join("other-v2.0.0-aarch64-apple-darwin.tar.gz"), b"x").unwrap();
        fs::write(tmp.0.join("fs-v2.0.0-x86_64-pc-windows-msvc.zip"), b"x").unwrap();
        fs::write(
            tmp.0.join("fs-v2.0.0-aarch64-apple-darwin.tar.gz.sha256"),
            b"x",
        )
        .unwrap();

        let got = collect_tarballs_from_dir(&tmp.0, "fs-v", "2.0.0").unwrap();
        let triples: Vec<&str> = got.iter().map(|t| t.target_triple.as_str()).collect();
        assert_eq!(
            triples,
            vec![
                "aarch64-apple-darwin",
                "aarch64-unknown-linux-gnu",
                "x86_64-apple-darwin",
                "x86_64-unknown-linux-gnu",
            ]
        );
    }

    #[test]
    fn collect_tarballs_walks_subdirectories() {
        // Mirrors the `gh run download` / `actions/download-artifact@v4` layout
        // where each artifact gets its own subdir.
        let tmp = tmpdir("collect-nested");
        for triple in [
            "aarch64-apple-darwin",
            "x86_64-apple-darwin",
            "aarch64-unknown-linux-gnu",
            "x86_64-unknown-linux-gnu",
        ] {
            let sub = tmp.0.join(format!("{triple}-release"));
            fs::create_dir_all(&sub).unwrap();
            fs::write(sub.join(format!("fs-v2.0.0-{triple}.tar.gz")), b"x").unwrap();
            // Extra noise inside the subdir
            fs::write(sub.join("checksums.txt"), b"x").unwrap();
        }
        let got = collect_tarballs_from_dir(&tmp.0, "fs-v", "2.0.0").unwrap();
        let triples: Vec<&str> = got.iter().map(|t| t.target_triple.as_str()).collect();
        assert_eq!(
            triples,
            vec![
                "aarch64-apple-darwin",
                "aarch64-unknown-linux-gnu",
                "x86_64-apple-darwin",
                "x86_64-unknown-linux-gnu",
            ]
        );
    }

    #[test]
    fn collect_tarballs_from_sha256sums_parses_standard_format() {
        let tmp = tmpdir("sha256sums-parse");
        let manifest = tmp.0.join("SHA256SUMS");
        // Mix of formats: text-mode (`  `), binary-mode (` *`), wheels and
        // zips that should be filtered out, plus a comment line.
        fs::write(
            &manifest,
            "# header comment ignored\n\
             0000000000000000000000000000000000000000000000000000000000000001  fs-v2.0.0-aarch64-apple-darwin.tar.gz\n\
             0000000000000000000000000000000000000000000000000000000000000002 *fs-v2.0.0-x86_64-apple-darwin.tar.gz\n\
             0000000000000000000000000000000000000000000000000000000000000003  fs-v2.0.0-aarch64-unknown-linux-gnu.tar.gz\n\
             0000000000000000000000000000000000000000000000000000000000000004  fs-v2.0.0-x86_64-unknown-linux-gnu.tar.gz\n\
             aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  fs-v2.0.0-py3-none-manylinux_2_28_x86_64.whl\n\
             bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb  fs-v2.0.0-x86_64-pc-windows-msvc.zip\n\
             cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc  fs-v1.9.0-aarch64-apple-darwin.tar.gz\n\
             \n",
        )
        .unwrap();
        let got = collect_tarballs_from_sha256sums(&manifest, "fs-v", "2.0.0").unwrap();
        let summarize: Vec<(String, String)> = got
            .iter()
            .map(|t| (t.target_triple.clone(), t.sha256.clone()))
            .collect();
        assert_eq!(
            summarize,
            vec![
                (
                    "aarch64-apple-darwin".into(),
                    "0000000000000000000000000000000000000000000000000000000000000001".into()
                ),
                (
                    "aarch64-unknown-linux-gnu".into(),
                    "0000000000000000000000000000000000000000000000000000000000000003".into()
                ),
                (
                    "x86_64-apple-darwin".into(),
                    "0000000000000000000000000000000000000000000000000000000000000002".into()
                ),
                (
                    "x86_64-unknown-linux-gnu".into(),
                    "0000000000000000000000000000000000000000000000000000000000000004".into()
                ),
            ]
        );
    }

    #[test]
    fn collect_tarballs_from_sha256sums_rejects_bad_lines() {
        let tmp = tmpdir("sha256sums-bad");
        let manifest = tmp.0.join("SHA256SUMS");
        // Truncated hex (too short)
        fs::write(
            &manifest,
            "deadbeef  fs-v2.0.0-aarch64-apple-darwin.tar.gz\n",
        )
        .unwrap();
        let err = collect_tarballs_from_sha256sums(&manifest, "fs-v", "2.0.0")
            .unwrap_err()
            .to_string();
        assert!(err.contains("expected 64-char hex"), "got: {err}");
    }

    #[test]
    fn collect_tarballs_from_sha256sums_uppercases_to_lower() {
        let tmp = tmpdir("sha256sums-upper");
        let manifest = tmp.0.join("SHA256SUMS");
        fs::write(
            &manifest,
            "AAAA1111BBBB2222CCCC3333DDDD4444EEEE5555FFFF66667777888899990000  fs-v2.0.0-aarch64-apple-darwin.tar.gz\n",
        )
        .unwrap();
        let got = collect_tarballs_from_sha256sums(&manifest, "fs-v", "2.0.0").unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(
            got[0].sha256,
            "aaaa1111bbbb2222cccc3333dddd4444eeee5555ffff66667777888899990000"
        );
    }

    #[test]
    fn sha256_hex_file_matches_known_digest() {
        let tmp = tmpdir("sha");
        let p = tmp.0.join("blob");
        fs::write(&p, b"abc").unwrap();
        // sha256("abc") = ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad
        assert_eq!(
            sha256_hex_file(&p).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        );
    }

    #[test]
    fn ruby_string_escapes_dangerous_chars() {
        assert_eq!(ruby_string("hello"), r#""hello""#);
        assert_eq!(ruby_string(r#"with"quote"#), r#""with\"quote""#);
        assert_eq!(ruby_string(r"with\back"), r#""with\\back""#);
        assert_eq!(ruby_string("with#{}"), r#""with\#{}""#);
    }

    #[test]
    fn render_license_uses_spdx_when_clean() {
        assert_eq!(render_license(Some("Apache-2.0")), "\"Apache-2.0\"");
        assert_eq!(render_license(Some("MIT")), "\"MIT\"");
    }

    #[test]
    fn render_license_falls_back_for_freeform() {
        assert_eq!(
            render_license(Some("Proprietary — internal use only")),
            ":cannot_represent"
        );
        assert_eq!(render_license(Some("")), ":cannot_represent");
        assert_eq!(render_license(None), ":cannot_represent");
    }

    fn fixture_ctx_dbt() -> FormulaCtx {
        FormulaCtx {
            class_name: "Dbt".into(),
            binary_name: "dbt".into(),
            install_as: "dbt".into(),
            version: "2.0.0-preview.1".into(),
            homepage: Some("https://github.com/dbt-labs/dbt-fusion".into()),
            description: Some("dbt — data build tool".into()),
            license: None,
            conflicts_with: Vec::new(),
            platforms: vec![
                Platform {
                    os: BrewOs::Macos,
                    arch: BrewArch::Arm,
                    url: "https://cdn/fs-v2.0.0-preview.1-aarch64-apple-darwin.tar.gz".into(),
                    sha256: "a".repeat(64),
                },
                Platform {
                    os: BrewOs::Macos,
                    arch: BrewArch::Intel,
                    url: "https://cdn/fs-v2.0.0-preview.1-x86_64-apple-darwin.tar.gz".into(),
                    sha256: "b".repeat(64),
                },
                Platform {
                    os: BrewOs::Linux,
                    arch: BrewArch::Arm,
                    url: "https://cdn/fs-v2.0.0-preview.1-aarch64-unknown-linux-gnu.tar.gz".into(),
                    sha256: "c".repeat(64),
                },
                Platform {
                    os: BrewOs::Linux,
                    arch: BrewArch::Intel,
                    url: "https://cdn/fs-v2.0.0-preview.1-x86_64-unknown-linux-gnu.tar.gz".into(),
                    sha256: "d".repeat(64),
                },
            ],
        }
    }

    #[test]
    fn render_formula_includes_all_four_platforms() {
        let out = render_formula(&fixture_ctx_dbt());
        assert!(out.starts_with("class Dbt < Formula\n"));
        assert!(out.contains("desc \"dbt — data build tool\"\n"));
        assert!(out.contains("homepage \"https://github.com/dbt-labs/dbt-fusion\"\n"));
        assert!(out.contains("version \"2.0.0-preview.1\"\n"));
        assert!(out.contains("license :cannot_represent\n"));
        assert!(!out.contains("conflicts_with"));
        assert!(out.contains("on_macos do\n"));
        assert!(out.contains("on_arm do\n"));
        assert!(out.contains("on_intel do\n"));
        assert!(out.contains("on_linux do\n"));
        assert!(out.contains("aarch64-apple-darwin"));
        assert!(out.contains("x86_64-apple-darwin"));
        assert!(out.contains("aarch64-unknown-linux-gnu"));
        assert!(out.contains("x86_64-unknown-linux-gnu"));
        assert!(out.contains("bin.install \"dbt\"\n"));
        assert!(out.contains("shell_output(\"#{bin}/dbt --version\")"));
        assert!(out.ends_with("end\n"));
    }

    #[test]
    fn render_formula_renames_when_install_as_differs_from_binary() {
        // Tarball ships `dbt-legacy-name`, brew installs it as `dbt`,
        // formula filename is `dbt-core.rb`. All three names distinct.
        // dbt-core's own formula no longer needs this (its tarball ships
        // `dbt`), but the rename-on-install path is still supported.
        let mut ctx = fixture_ctx_dbt();
        ctx.class_name = "DbtCore".into();
        ctx.binary_name = "dbt-legacy-name".into();
        ctx.install_as = "dbt".into();
        let out = render_formula(&ctx);
        assert!(out.contains("class DbtCore < Formula\n"));
        assert!(out.contains("bin.install \"dbt-legacy-name\" => \"dbt\"\n"));
        // Test invocation uses the installed name, not the formula name.
        assert!(out.contains("shell_output(\"#{bin}/dbt --version\")"));
    }

    #[test]
    fn render_formula_emits_conflicts_with_clauses_referencing_installed_name() {
        let mut ctx = fixture_ctx_dbt();
        ctx.class_name = "DbtCore".into();
        ctx.binary_name = "dbt-legacy-name".into();
        ctx.install_as = "dbt".into();
        ctx.conflicts_with = vec!["dbt".into()];
        let out = render_formula(&ctx);
        assert!(
            out.contains(
                "conflicts_with \"dbt\", because: \"both formulas install the `dbt` binary\""
            ),
            "got: {out}"
        );
    }

    #[test]
    fn render_formula_omits_conflicts_block_when_list_is_empty() {
        // Default fixture has no conflicts.
        let out = render_formula(&fixture_ctx_dbt());
        assert!(!out.contains("conflicts_with"));
    }

    #[test]
    fn render_formula_omits_macos_block_when_no_macos_platforms() {
        let mut ctx = fixture_ctx_dbt();
        ctx.platforms.retain(|p| p.os == BrewOs::Linux);
        let out = render_formula(&ctx);
        assert!(!out.contains("on_macos do"));
        assert!(out.contains("on_linux do"));
    }
}
