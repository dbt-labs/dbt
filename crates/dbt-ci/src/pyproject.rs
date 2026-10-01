//! Reads `pyproject.toml` at the cargo workspace root — source of truth for the
//! wheel name and PEP 621 metadata that `pack` stamps into each wheel.

use crate::utils::cargo_workspace_root;
use anyhow::{Context, Result, anyhow};
use std::fs;
use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, Item};

#[derive(Debug)]
pub(crate) struct Spec {
    pub(crate) wheel_name: String,
    pub(crate) pyproject_dir: PathBuf,
    pub(crate) summary: Option<String>,
    pub(crate) requires_python: Option<String>,
    /// PEP 621 `dependencies`, emitted as `Requires-Dist`. Installers that trust an
    /// sdist's static metadata (uv does; pip re-derives it from the built wheel)
    /// install nothing at all when these are dropped.
    pub(crate) dependencies: Vec<String>,
    pub(crate) classifiers: Vec<String>,
    pub(crate) urls: Vec<(String, String)>,
    pub(crate) authors: Vec<Author>,
    pub(crate) license: Option<String>,
    /// Long description body (PEP 621 `readme`).
    pub(crate) description: Option<String>,
    pub(crate) description_content_type: Option<String>,
}

#[derive(Debug, Default)]
pub(crate) struct Author {
    pub(crate) name: Option<String>,
    pub(crate) email: Option<String>,
}

impl Spec {
    /// Takes `requires-python` and `dependencies` from the `pyproject.toml` under
    /// `dir`, leaving descriptive metadata alone — for the sdist, whose rich
    /// metadata is the workspace root's but whose wheel maturin builds elsewhere.
    pub(crate) fn overlay_runtime_metadata(&mut self, dir: &Path) -> Result<()> {
        let wheel_spec = discover_at(dir)?;
        self.requires_python = wheel_spec.requires_python;
        self.dependencies = wheel_spec.dependencies;
        Ok(())
    }
}

pub(crate) fn discover() -> Result<Spec> {
    parse(cargo_workspace_root())
}

/// Like [`discover`] but reads the `pyproject.toml` under `dir` (relative paths
/// resolve against the workspace root), for packages that don't live at the root.
pub(crate) fn discover_at(dir: &Path) -> Result<Spec> {
    let dir = if dir.is_absolute() {
        dir.to_path_buf()
    } else {
        cargo_workspace_root().join(dir)
    };
    parse(dir)
}

fn parse(pyproject_dir: PathBuf) -> Result<Spec> {
    let pp_path = pyproject_dir.join("pyproject.toml");
    let text = fs::read_to_string(&pp_path)
        .with_context(|| format!("failed to read {}", pp_path.display()))?;
    let doc: DocumentMut = text
        .parse()
        .with_context(|| format!("failed to parse {}", pp_path.display()))?;

    let project = doc
        .get("project")
        .ok_or_else(|| anyhow!("{}: missing `[project]` table", pp_path.display()))?;

    let wheel_name = project
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("{}: missing `[project].name`", pp_path.display()))?
        .to_string();

    let summary = project
        .get("description")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    let requires_python = project
        .get("requires-python")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    let dependencies = string_array(project, "dependencies");
    let classifiers = string_array(project, "classifiers");

    let urls = project
        .get("urls")
        .and_then(|t| t.as_table_like())
        .map(|t| {
            t.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.to_string(), s.to_string())))
                .collect()
        })
        .unwrap_or_default();

    let authors = project
        .get("authors")
        .map(parse_authors)
        .unwrap_or_default();

    let license = project
        .get("license")
        .map(|v| parse_license(v, &pyproject_dir))
        .transpose()
        .with_context(|| format!("{}: `[project].license`", pp_path.display()))?
        .flatten();

    let (description, description_content_type) = project
        .get("readme")
        .map(|v| parse_readme(v, &pyproject_dir))
        .transpose()
        .with_context(|| format!("{}: `[project].readme`", pp_path.display()))?
        .unwrap_or((None, None));

    Ok(Spec {
        wheel_name,
        pyproject_dir,
        summary,
        requires_python,
        dependencies,
        classifiers,
        urls,
        authors,
        license,
        description,
        description_content_type,
    })
}

/// A PEP 621 array-of-strings field; absent or non-string entries are skipped.
fn string_array(project: &Item, key: &str) -> Vec<String> {
    project
        .get(key)
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|i| i.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Accepts both `authors = [{ name = "x" }, ...]` (inline-table array) and
/// `[[project.authors]]\nname = "x"` (array-of-tables).
fn parse_authors(item: &Item) -> Vec<Author> {
    if let Some(arr) = item.as_array() {
        return arr
            .iter()
            .filter_map(|v| v.as_inline_table())
            .map(|t| Author {
                name: t.get("name").and_then(|x| x.as_str()).map(str::to_string),
                email: t.get("email").and_then(|x| x.as_str()).map(str::to_string),
            })
            .collect();
    }
    if let Some(arr) = item.as_array_of_tables() {
        return arr
            .iter()
            .map(|t| Author {
                name: t.get("name").and_then(|x| x.as_str()).map(str::to_string),
                email: t.get("email").and_then(|x| x.as_str()).map(str::to_string),
            })
            .collect();
    }
    Vec::new()
}

/// PEP 621 `license`: SPDX string `"MIT"`, `{ text = "..." }`, or
/// `{ file = "LICENSE" }` (read relative to `dir`).
fn parse_license(item: &Item, dir: &Path) -> Result<Option<String>> {
    if let Some(s) = item.as_str() {
        return Ok(Some(s.to_string()));
    }
    if let Some(t) = item.as_table_like() {
        if let Some(text) = t.get("text").and_then(|v| v.as_str()) {
            return Ok(Some(text.to_string()));
        }
        if let Some(file) = t.get("file").and_then(|v| v.as_str()) {
            return Ok(Some(read_relative(dir, file)?));
        }
    }
    Ok(None)
}

/// PEP 621 `readme`: string filename, `{ file = ... }`, or
/// `{ text = ..., content-type = ... }`. Returns `(body, content-type)`.
fn parse_readme(item: &Item, dir: &Path) -> Result<(Option<String>, Option<String>)> {
    if let Some(s) = item.as_str() {
        let body = read_relative(dir, s)?;
        return Ok((Some(body), Some(content_type_for(s).to_string())));
    }
    if let Some(t) = item.as_table_like() {
        if let Some(text) = t.get("text").and_then(|v| v.as_str()) {
            let ct = t
                .get("content-type")
                .and_then(|v| v.as_str())
                .unwrap_or("text/plain")
                .to_string();
            return Ok((Some(text.to_string()), Some(ct)));
        }
        if let Some(file) = t.get("file").and_then(|v| v.as_str()) {
            let body = read_relative(dir, file)?;
            let ct = t
                .get("content-type")
                .and_then(|v| v.as_str())
                .unwrap_or_else(|| content_type_for(file))
                .to_string();
            return Ok((Some(body), Some(ct)));
        }
    }
    Ok((None, None))
}

fn read_relative(dir: &Path, rel: &str) -> Result<String> {
    let path = dir.join(rel);
    fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))
}

fn content_type_for(filename: &str) -> &'static str {
    let lower = filename.to_ascii_lowercase();
    if lower.ends_with(".md") || lower.ends_with(".markdown") {
        "text/markdown"
    } else if lower.ends_with(".rst") {
        "text/x-rst"
    } else {
        "text/plain"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_reads_project_name() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("pyproject.toml"),
            r#"
[project]
name = "my-thing"
"#,
        )
        .unwrap();

        let spec = parse(tmp.path().to_path_buf()).unwrap();
        assert_eq!(spec.wheel_name, "my-thing");
        assert_eq!(spec.pyproject_dir, tmp.path());
        assert!(spec.summary.is_none());
        assert!(spec.dependencies.is_empty());
        assert!(spec.authors.is_empty());
        assert!(spec.license.is_none());
        assert!(spec.description.is_none());
    }

    #[test]
    fn overlay_runtime_metadata_takes_only_runtime_fields() {
        let root = tempfile::tempdir().unwrap();
        fs::write(
            root.path().join("pyproject.toml"),
            r#"
[project]
name = "dbt-oss"
description = "Build analytics the way engineers build applications"
requires-python = ">=3.9"
classifiers = ["Programming Language :: Rust"]
"#,
        )
        .unwrap();

        let wheel = tempfile::tempdir().unwrap();
        fs::write(
            wheel.path().join("pyproject.toml"),
            r#"
[project]
name = "dbt-core"
requires-python = ">=3.10"
dependencies = ["mashumaro[msgpack]>=3.14"]
"#,
        )
        .unwrap();

        let mut spec = parse(root.path().to_path_buf()).unwrap();
        spec.overlay_runtime_metadata(wheel.path()).unwrap();

        assert_eq!(spec.requires_python.as_deref(), Some(">=3.10"));
        assert_eq!(spec.dependencies, vec!["mashumaro[msgpack]>=3.14"]);
        assert_eq!(spec.wheel_name, "dbt-oss");
        assert_eq!(
            spec.summary.as_deref(),
            Some("Build analytics the way engineers build applications")
        );
        assert_eq!(spec.classifiers.len(), 1);
    }

    #[test]
    fn parse_reads_full_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("pyproject.toml"),
            r#"
[project]
name = "dbt-sa-cli"
description = "dbt fusion standalone analyzer CLI"
authors = [{ name = "dbt Labs", email = "info@dbtlabs.com" }]
requires-python = ">=3.9"
dependencies = ["mashumaro[msgpack]>=3.14", "packaging>=24"]
license = "Apache-2.0"
readme = "README.md"
classifiers = [
  "Programming Language :: Rust",
  "Development Status :: 4 - Beta",
]

[project.urls]
Homepage = "https://getdbt.com"
Repository = "https://github.com/dbt-labs/dbt-fusion"
"#,
        )
        .unwrap();
        fs::write(
            tmp.path().join("README.md"),
            "# dbt-sa-cli\n\nLong description body.\n",
        )
        .unwrap();

        let spec = parse(tmp.path().to_path_buf()).unwrap();
        assert_eq!(spec.wheel_name, "dbt-sa-cli");
        assert_eq!(
            spec.summary.as_deref(),
            Some("dbt fusion standalone analyzer CLI")
        );
        assert_eq!(spec.requires_python.as_deref(), Some(">=3.9"));
        assert_eq!(
            spec.dependencies,
            vec!["mashumaro[msgpack]>=3.14", "packaging>=24"]
        );
        assert_eq!(spec.classifiers.len(), 2);
        assert_eq!(spec.urls.len(), 2);
        assert_eq!(spec.authors.len(), 1);
        assert_eq!(spec.authors[0].name.as_deref(), Some("dbt Labs"));
        assert_eq!(spec.license.as_deref(), Some("Apache-2.0"));
        assert_eq!(
            spec.description.as_deref(),
            Some("# dbt-sa-cli\n\nLong description body.\n")
        );
        assert_eq!(
            spec.description_content_type.as_deref(),
            Some("text/markdown")
        );
    }

    #[test]
    fn parse_handles_array_of_tables_authors() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("pyproject.toml"),
            r#"
[project]
name = "x"

[[project.authors]]
name = "Alice"
email = "alice@example.com"

[[project.authors]]
name = "Bob"
"#,
        )
        .unwrap();
        let spec = parse(tmp.path().to_path_buf()).unwrap();
        assert_eq!(spec.authors.len(), 2);
        assert_eq!(spec.authors[0].name.as_deref(), Some("Alice"));
        assert_eq!(spec.authors[0].email.as_deref(), Some("alice@example.com"));
        assert_eq!(spec.authors[1].name.as_deref(), Some("Bob"));
        assert!(spec.authors[1].email.is_none());
    }

    #[test]
    fn parse_reads_readme_as_inline_text() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("pyproject.toml"),
            r#"
[project]
name = "x"
readme = { text = "inline body", content-type = "text/plain" }
"#,
        )
        .unwrap();
        let spec = parse(tmp.path().to_path_buf()).unwrap();
        assert_eq!(spec.description.as_deref(), Some("inline body"));
        assert_eq!(spec.description_content_type.as_deref(), Some("text/plain"));
    }

    #[test]
    fn parse_reads_license_text_table() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("pyproject.toml"),
            r#"
[project]
name = "x"
license = { text = "Proprietary — internal use only" }
"#,
        )
        .unwrap();
        let spec = parse(tmp.path().to_path_buf()).unwrap();
        assert_eq!(
            spec.license.as_deref(),
            Some("Proprietary — internal use only")
        );
    }

    #[test]
    fn parse_reads_license_file() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("pyproject.toml"),
            r#"
[project]
name = "x"
license = { file = "LICENSE.txt" }
"#,
        )
        .unwrap();
        fs::write(tmp.path().join("LICENSE.txt"), "MIT License...\n").unwrap();
        let spec = parse(tmp.path().to_path_buf()).unwrap();
        assert_eq!(spec.license.as_deref(), Some("MIT License...\n"));
    }

    #[test]
    fn parse_errors_when_project_name_missing() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("pyproject.toml"),
            r#"
[project]
description = "missing name"
"#,
        )
        .unwrap();
        let err = parse(tmp.path().to_path_buf()).unwrap_err().to_string();
        assert!(err.contains("`[project].name`"), "got: {err}");
    }
}
