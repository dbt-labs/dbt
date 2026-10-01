use crate::args::BumpCargoVersionArgs;
use crate::release_version::validate_release_version;
use crate::utils::cargo_workspace_root;
use anyhow::{Context, Result, anyhow};
use std::fs;
use std::path::Path;
use std::process::{Command, ExitCode};
use toml_edit::{DocumentMut, value};

pub fn execute(args: BumpCargoVersionArgs) -> ExitCode {
    if let Err(e) = validate_release_version(&args.version) {
        eprintln!("error: {e:#}");
        return ExitCode::from(2);
    }
    let version = &args.version;
    eprintln!("→ version → {version}");

    let cargo_root = cargo_workspace_root();
    let workspace_cargo = cargo_root.join("Cargo.toml");
    if !workspace_cargo.is_file() {
        eprintln!(
            "error: workspace `Cargo.toml` not found at `{}`.",
            workspace_cargo.display()
        );
        return ExitCode::from(2);
    }

    if let Err(e) = write_workspace_version(&workspace_cargo, version) {
        eprintln!("error: {}: {e:#}", workspace_cargo.display());
        return ExitCode::from(2);
    }
    eprintln!("✓ {}", workspace_cargo.display());

    if !args.no_lockfile {
        eprintln!("→ cargo update --workspace --offline");
        let mut cmd = Command::new("cargo");
        cmd.current_dir(&cargo_root)
            .args(["update", "--workspace", "--offline"]);
        match cmd.status() {
            Ok(s) if s.success() => {}
            _ => {
                // Version write already succeeded; lockfile refresh is best-effort.
                eprintln!(
                    "warning: `cargo update --workspace --offline` failed. \
                     Version edits are applied; rerun lockfile refresh manually."
                );
            }
        }
    }

    ExitCode::SUCCESS
}

fn write_workspace_version(path: &Path, new_value: &str) -> Result<()> {
    let src = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let mut doc: DocumentMut = src.parse().context("parse Cargo.toml")?;
    let workspace = doc
        .get_mut("workspace")
        .and_then(|w| w.as_table_like_mut())
        .ok_or_else(|| anyhow!("no `[workspace]` table"))?;
    let package = workspace
        .get_mut("package")
        .and_then(|p| p.as_table_like_mut())
        .ok_or_else(|| anyhow!("no `[workspace.package]` table"))?;
    package.insert("version", value(new_value));
    fs::write(path, doc.to_string()).with_context(|| format!("write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_workspace_version_overwrites_existing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("Cargo.toml");
        fs::write(
            &path,
            r#"[workspace.package]
edition = "2024"
version = "2.0.0-preview-nightly.176"
authors = ["dbt Labs"]
"#,
        )
        .unwrap();
        write_workspace_version(&path, "2.0.0-rc.1").unwrap();
        let after = fs::read_to_string(&path).unwrap();
        assert!(after.contains(r#"version = "2.0.0-rc.1""#));
        assert!(!after.contains("preview-nightly"));
        assert!(after.contains(r#"edition = "2024""#));
        assert!(after.contains(r#"authors = ["dbt Labs"]"#));
    }

    #[test]
    fn write_workspace_version_errors_when_table_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("Cargo.toml");
        fs::write(&path, "[package]\nname = \"x\"\n").unwrap();
        let err = write_workspace_version(&path, "2.0.0")
            .unwrap_err()
            .to_string();
        assert!(err.contains("[workspace]"), "got: {err}");
    }
}
