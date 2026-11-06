//! Generates DuckDB initialization SQL from an [`AdapterConfig`].
//!
//! Produces statements in the same order as upstream dbt-duckdb:
//! 1. `INSTALL` + `LOAD` for each extension (including auto-injected `motherduck`)
//! 2. `CREATE OR REPLACE SECRET` for each secret
//! 3. `SET motherduck_token` (for MotherDuck paths, when resolved)
//! 4. `SET` for each setting
//! 5. `ATTACH IF NOT EXISTS` for each attachment
//!
//! FIXME: this module has nothing to do with authentication — it is DuckDB session
//! setup logic that ended up here because `dbt-auth` was the first crate that needed
//! it. `dbt-index` depends on `dbt-auth` solely to reach this function. It should
//! move to `dbt-adapter` or a small shared crate, but that requires untangling the
//! `dbt-index` → `dbt-adapter` dependency graph first.

use std::borrow::Cow;

use crate::AuthError;
use crate::config::{AdapterConfig, YmlValue};

pub enum DuckDbTarget<'a> {
    // COPY from local files.
    Plain { path: &'a str },
    // COPY from local files.
    MotherduckBrowserLogin { path: String },
    // COPY from local files.
    MotherduckTokenAuth { path: String, token: String },
    // No COPY from files. Seeds load by in-memory insert. Python models disabled.
    MotherduckSaasBrowserLogin { path: String },
    // No COPY from files. Seeds load by in-memory insert. Python models disabled.
    MotherduckSaasTokenAuth { path: String, token: String },
}

fn is_motherduck(path: &str) -> bool {
    path.get(..3).is_some_and(|p| p.eq_ignore_ascii_case("md:"))
        || path
            .get(..11)
            .is_some_and(|p| p.eq_ignore_ascii_case("motherduck:"))
}

fn parse_query(query: &str) -> impl Iterator<Item = (Cow<'_, str>, Cow<'_, str>)> {
    url::form_urlencoded::parse(query.as_bytes())
}

impl<'a> DuckDbTarget<'a> {
    pub fn from_config(config: &'a AdapterConfig) -> Result<Self, AuthError> {
        let raw = config
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let settings = Settings::from_config(config)?;
        let attachments = Attachments::from_config(config)?;
        if std::iter::once(&raw)
            .chain(attachments.paths.iter())
            .any(|p| is_motherduck(p))
        {
            let path = Self::attach_path(raw);

            // https://github.com/duckdb/dbt-duckdb/blob/67b43f1f86ef6b4252b184ebeb00b750a7e9a513/dbt/adapters/duckdb/credentials.py#L339
            let derived = match Self::database_name(raw) {
                name if name.is_empty() => "my_db".to_owned(),
                name => name,
            };
            match config.get("database").and_then(|v| v.as_str()) {
                None | Some("") => {}
                Some(requested) if requested == derived => {}
                Some(_) => {
                    return Err(AuthError::config(format!(
                        "Inconsistency detected between 'path' and 'database' fields in profile; \
                             the 'database' property must be set to '{derived}' to match the 'path'"
                    )));
                }
            }

            let target = if settings.is_saas_mode() {
                match Self::resolve_token(raw, config) {
                    Some(token) => DuckDbTarget::MotherduckSaasTokenAuth { path, token },
                    None => DuckDbTarget::MotherduckSaasBrowserLogin { path },
                }
            } else {
                match Self::resolve_token(raw, config) {
                    Some(token) => DuckDbTarget::MotherduckTokenAuth { path, token },
                    None => DuckDbTarget::MotherduckBrowserLogin { path },
                }
            };
            Ok(target)
        } else {
            // Matches upstream dbt-duckdb: `database:` for a local/in-memory
            // connection must match the name DuckDB derives from `path`, else
            // it's a hard config error (no aliasing on the primary connection).
            {
                if settings.is_saas_mode() {
                    return Err(AuthError::config(
                        "settings: motherduck_saas_mode requires a MotherDuck path (md: or motherduck:)",
                    ));
                }
                let path = match raw {
                    "" | ":memory:" => ":memory:",
                    _ => raw,
                };
                // `ducklake:` is a transparent prefix over the real path for
                // database-name derivation purposes (matches upstream).
                let derivation_path = path.strip_prefix("ducklake:").unwrap_or(path);
                let base = derivation_path
                    .rsplit(['/', '\\'])
                    .next()
                    .unwrap_or(derivation_path);
                let derived = if derivation_path == ":memory:" {
                    "memory"
                } else {
                    match base.rsplit_once('.') {
                        Some((stem, _)) if !stem.is_empty() => stem,
                        _ => base,
                    }
                };

                match config.get("database").and_then(|v| v.as_str()) {
                    None | Some("") => Ok(DuckDbTarget::Plain { path }),
                    Some(requested) if requested == derived => Ok(DuckDbTarget::Plain { path }),
                    Some(requested) if requested.eq_ignore_ascii_case("main") => {
                        Err(AuthError::config(
                            "database: 'main' is invalid. 'main' is DuckDB's reserved default database name"
                                .to_owned(),
                        ))
                    }
                    Some(_) => Err(AuthError::config(format!(
                        "Inconsistency detected between 'path' and 'database' fields in profile; \
                         the 'database' property must be set to '{derived}' to match the 'path'"
                    ))),
                }
            }
        }
    }

    // FIXME: token hierarchy is missing parts of the Python adapter (plugins/motherduck.py):
    // - token in attachment path query strings (Python reads these for attach-only MotherDuck)
    // - token in `plugins[].config` (Python passes MD keys from plugin config)
    // - bare `token` / `TOKEN` keys (Python's get_md_config_settings accepts them)
    // - `settings:` token is applied per cursor in Python; here it is set on the init connection
    fn resolve_token(path: &str, config: &AdapterConfig) -> Option<String> {
        if let Some(token) = config
            .get("settings")
            .and_then(|v| match v {
                YmlValue::Mapping(map, _) => map.get("motherduck_token"),
                _ => None,
            })
            .and_then(|v| v.as_str())
        {
            if !token.is_empty() {
                return Some(token.to_owned());
            }
        }

        let from_path = path.split_once('?').and_then(|(_, query)| {
            parse_query(query).find_map(|(key, value)| {
                (key == "motherduck_token" && !value.is_empty()).then(|| value.into_owned())
            })
        });
        from_path.or_else(|| std::env::var("MOTHERDUCK_TOKEN").ok())
    }

    /// Derive a database name from a MotherDuck path.
    fn database_name(path: &str) -> String {
        let stripped = if let Some(rest) = path.strip_prefix("motherduck:").or_else(|| {
            let lower = path.to_lowercase();
            if lower.starts_with("motherduck:") {
                Some(&path["motherduck:".len()..])
            } else {
                None
            }
        }) {
            rest
        } else if let Some(rest) = path.strip_prefix("md:").or_else(|| {
            let lower = path.to_lowercase();
            if lower.starts_with("md:") {
                Some(&path["md:".len()..])
            } else {
                None
            }
        }) {
            rest
        } else {
            path
        };

        stripped.split('?').next().unwrap_or("").to_owned()
    }

    /// Strip URL query parameters from a MotherDuck attach path.
    fn attach_path(path: &str) -> String {
        path.split_once('?')
            .map(|(base, _)| base.to_owned())
            .unwrap_or_else(|| path.to_owned())
    }
}

/// The repository an extension is installed from.
enum ExtensionRepository<'a> {
    /// Bare `INSTALL`, which uses DuckDB's default repository.
    Default,
    /// `INSTALL ... FROM "<name>"`, e.g. `community` or `core_nightly`.
    Named(&'a str),
}

struct Extension<'a> {
    name: &'a str,
    repository: ExtensionRepository<'a>,
}

impl<'a> Extension<'a> {
    const OBJECT_FIELDS: &'static [&'static str] = &["name", "repo"];

    /// Parses `extensions[i]`: a bare name, or a `{name, repo?}` mapping as in dbt-duckdb.
    // https://github.com/duckdb/dbt-duckdb/blob/6c010fc97bd5bba793f7132991edd001362e756b/dbt/adapters/duckdb/environments/__init__.py#L166-L177
    fn from_item(i: usize, item: &'a YmlValue) -> Result<Self, AuthError> {
        let map = match item {
            // Bare names keep their legacy handling: sanitized at render, skipped if empty.
            YmlValue::String(name, _) => {
                return Ok(Extension {
                    name,
                    repository: ExtensionRepository::Default,
                });
            }
            YmlValue::Mapping(map, _) => map,
            _ => {
                return Err(AuthError::config(format!(
                    "extensions[{i}]: expected an extension name or a mapping with 'name' and optional 'repo'"
                )));
            }
        };
        if let Some(unknown) = map
            .keys()
            .find(|k| !k.as_str().is_some_and(|k| Self::OBJECT_FIELDS.contains(&k)))
        {
            return Err(AuthError::config(match unknown.as_str() {
                Some(key) => format!("extensions[{i}]: unknown field '{key}'"),
                None => format!("extensions[{i}]: field names must be strings"),
            }));
        }

        let name = match map.get("name") {
            Some(YmlValue::String(name, _)) if is_extension_name(name) => name,
            Some(_) => {
                return Err(AuthError::config(format!(
                    "extensions[{i}].name: expected a non-empty string of ASCII letters, digits and '_'"
                )));
            }
            None => {
                return Err(AuthError::config(format!(
                    "extensions[{i}]: missing required field 'name'"
                )));
            }
        };
        // `repo: null` is the same as omitting it, matching `DuckDbExtensionObject::repo`.
        let repository = match map.get("repo") {
            None | Some(YmlValue::Null(_)) => ExtensionRepository::Default,
            Some(YmlValue::String(repo, _)) if is_repository_name(repo) => {
                ExtensionRepository::Named(repo)
            }
            Some(_) => {
                return Err(AuthError::config(format!(
                    "extensions[{i}].repo: expected a repository name of ASCII letters, digits, '_' and '-'"
                )));
            }
        };
        Ok(Extension { name, repository })
    }

    /// `INSTALL` (unless `install` is false) and `LOAD` for this extension.
    fn render(&self, install: bool, out: &mut Vec<String>) {
        let name = sanitize_identifier(self.name);
        if name.is_empty() {
            return;
        }
        if install {
            out.push(match self.repository {
                ExtensionRepository::Default => format!("INSTALL {name}"),
                ExtensionRepository::Named(repo) => format!("INSTALL {name} FROM \"{repo}\""),
            });
        }
        // A bare `LOAD` finds extensions installed from the built-in and community repositories.
        // https://github.com/duckdb/duckdb/blob/d54cbbe3df17e8e5ce68312f483b4155836112aa/src/main/extension/extension_load.cpp#L485-L487
        out.push(format!("LOAD {name}"));
    }
}

struct Extensions<'a> {
    items: Vec<Extension<'a>>,
}

impl<'a> Extensions<'a> {
    fn from_config(config: &'a AdapterConfig) -> Result<Self, AuthError> {
        match config.get("extensions") {
            Some(YmlValue::Sequence(seq, _)) => {
                let items = seq
                    .iter()
                    .enumerate()
                    .map(|(i, item)| Extension::from_item(i, item))
                    .collect::<Result<_, _>>()?;
                Ok(Extensions { items })
            }
            None => Ok(Extensions { items: vec![] }),
            Some(other) => Err(AuthError::config(format!(
                "extensions: expected a sequence, got {other:?}"
            ))),
        }
    }

    /// `INSTALL`/`LOAD` for the configured extensions. MotherDuck targets get `motherduck`
    /// auto-injected if absent; under SaaS, file access is locked at open and only `LOAD` can run.
    fn render(&self, target: &DuckDbTarget<'_>) -> Vec<String> {
        let (is_motherduck, install) = match target {
            DuckDbTarget::Plain { .. } => (false, true),
            DuckDbTarget::MotherduckBrowserLogin { .. }
            | DuckDbTarget::MotherduckTokenAuth { .. } => (true, true),
            DuckDbTarget::MotherduckSaasBrowserLogin { .. }
            | DuckDbTarget::MotherduckSaasTokenAuth { .. } => (true, false),
        };
        let motherduck = (is_motherduck
            && !self
                .items
                .iter()
                .any(|ext| ext.name.eq_ignore_ascii_case("motherduck")))
        .then_some(Extension {
            name: "motherduck",
            repository: ExtensionRepository::Default,
        });

        let mut out = Vec::with_capacity((self.items.len() + 1) * 2);
        for ext in motherduck.iter().chain(&self.items) {
            ext.render(install, &mut out);
        }
        out
    }
}

struct Secrets<'a> {
    items: Vec<&'a YmlValue>,
}

impl<'a> Secrets<'a> {
    fn from_config(config: &'a AdapterConfig) -> Result<Self, AuthError> {
        match config.get("secrets") {
            Some(YmlValue::Sequence(seq, _)) => Ok(Secrets {
                items: seq.iter().collect(),
            }),
            None => Ok(Secrets { items: vec![] }),
            Some(other) => Err(AuthError::config(format!(
                "secrets: expected a sequence, got {other:?}"
            ))),
        }
    }

    // FIXME: replace with typed Secret variants per issue #7834 (S3, GCS, R2, Azure, HuggingFace).
    // Currently passes all unknown fields through as SQL params, same as the original main logic.
    fn render(&self) -> Vec<String> {
        self.items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| {
                let YmlValue::Mapping(map, _) = item else {
                    return None;
                };
                let secret_type = sanitize_identifier(map.get("type").and_then(|v| v.as_str())?);
                let name = map
                    .get("name")
                    .and_then(|v| v.as_str())
                    .map(sanitize_identifier)
                    .unwrap_or_else(|| format!("__dbt_secret_{i}"));
                let persistent = map
                    .get("persistent")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let persist_kw = if persistent { " PERSISTENT" } else { "" };

                let mut params = vec![format!("TYPE {secret_type}")];
                if let Some(provider) = map.get("provider").and_then(|v| v.as_str()) {
                    params.push(format!("PROVIDER {}", sanitize_identifier(provider)));
                }
                if let Some(scope) = map.get("scope").and_then(|v| v.as_str()) {
                    params.push(format!("SCOPE '{}'", escape_single_quotes(scope)));
                }
                const RESERVED: &[&str] = &["type", "name", "persistent", "provider", "scope"];
                for (k, v) in map.iter() {
                    if let Some(key) = k.as_str().filter(|k| !RESERVED.contains(k)) {
                        let key_upper = sanitize_identifier(key).to_uppercase();
                        if !key_upper.is_empty() {
                            params.push(format!("{key_upper} {}", yml_value_to_sql_literal(v)));
                        }
                    }
                }
                Some(format!(
                    "CREATE OR REPLACE{persist_kw} SECRET {name} ({})",
                    params.join(", ")
                ))
            })
            .collect()
    }
}

/// `settings:` from the profile, rendered as `SET key = value` on the init connection.
///
/// Scope matters:
/// - Plain `SET` is session-scoped. The value lives only on the init connection and
///   is gone once it closes. Later connections will not see it.
/// - `SET GLOBAL` writes the instance's config. Every later connection on this
///   database sees it, until something overwrites it.
/// - A setting can also be global-only, in which case plain `SET` writes globally.
///   Use `SET GLOBAL` when the value must reach later connections.
///
/// Assumption: one dbt invocation uses one profile. MotherDuck semantics cannot
/// safely live next to plain DuckDB semantics in the same run.
///
// FIXME: validate settings at parse time, like secrets.
struct Settings<'a> {
    keys: Vec<String>,
    values: Vec<&'a YmlValue>,
}

impl Settings<'_> {
    fn settings(&self) -> impl Iterator<Item = (&String, &&YmlValue)> {
        self.keys.iter().zip(self.values.iter())
    }

    fn is_saas_mode(&self) -> bool {
        self.settings().any(|(key, value)| {
            key == "motherduck_saas_mode"
                && (value.as_bool() == Some(true) || value.as_str() == Some("true"))
        })
    }
}

impl<'a> Settings<'a> {
    fn from_config(config: &'a AdapterConfig) -> Result<Self, AuthError> {
        match config.get("settings") {
            Some(YmlValue::Mapping(map, _)) => {
                let (keys, values) = map
                    .iter()
                    .filter_map(|(k, value)| k.as_str().map(|key| (key, value)))
                    .filter(|(key, _)| *key != "motherduck_token")
                    .filter_map(|(key, value)| {
                        let key = sanitize_identifier(key);
                        (!key.is_empty()).then_some((key, value))
                    })
                    .unzip();
                Ok(Settings { keys, values })
            }
            None => Ok(Settings {
                keys: vec![],
                values: vec![],
            }),
            Some(other) => Err(AuthError::config(format!(
                "settings: expected a mapping, got {other:?}"
            ))),
        }
    }

    fn render(&self) -> Vec<String> {
        debug_assert_eq!(self.keys.len(), self.values.len());
        self.keys
            .iter()
            .zip(self.values.iter())
            .filter(|(key, _)| !(key.as_str() == "motherduck_saas_mode" && self.is_saas_mode()))
            .map(|(key, value)| {
                let literal = yml_value_to_sql_literal(value);
                format!("SET {key} = {literal}")
            })
            .collect()
    }
}

const ATTACHMENT_FIELDS: &[&str] = &["path", "alias", "type", "read_only"];

/// If any attachment is `md:` / `motherduck:` prefixed, the `motherduck` extension is
/// auto-loaded and the profile is treated as a MotherDuck setup, even when the main
/// path was not intended to be MotherDuck.
struct Attachments<'a> {
    paths: Vec<&'a str>,
    aliases: Vec<Option<String>>,
    db_types: Vec<Option<String>>,
    read_onlys: Vec<bool>,
}

impl<'a> Attachments<'a> {
    fn from_config(config: &'a AdapterConfig) -> Result<Self, AuthError> {
        match config.get("attach") {
            Some(YmlValue::Sequence(seq, _)) => {
                let mut paths = Vec::with_capacity(seq.len());
                let mut aliases = Vec::with_capacity(seq.len());
                let mut db_types = Vec::with_capacity(seq.len());
                let mut read_onlys = Vec::with_capacity(seq.len());

                for (i, item) in seq.iter().enumerate() {
                    let YmlValue::Mapping(map, _) = item else {
                        return Err(AuthError::config(format!(
                            "attach: item {i} must be a mapping, got {item:?}"
                        )));
                    };
                    if let Some(unknown) = map
                        .iter()
                        .filter_map(|(k, _)| k.as_str())
                        .find(|k| !ATTACHMENT_FIELDS.contains(k))
                    {
                        return Err(AuthError::config(format!(
                            "attach: item {i}: unknown field '{unknown}'"
                        )));
                    }
                    let path = map.get("path").and_then(|v| v.as_str()).ok_or_else(|| {
                        AuthError::config(format!(
                            "attach: item {i}: missing required field 'path'"
                        ))
                    })?;

                    let sanitized_field = |key: &str| -> Result<Option<String>, AuthError> {
                        let Some(raw) = map.get(key).and_then(|v| v.as_str()) else {
                            return Ok(None);
                        };
                        let sanitized = sanitize_identifier(raw);
                        if sanitized.is_empty() {
                            return Err(AuthError::config(format!(
                                "attach: item {i}: {key} '{raw}' sanitizes to an empty identifier"
                            )));
                        }
                        Ok(Some(sanitized))
                    };

                    paths.push(path);
                    aliases.push(sanitized_field("alias")?);
                    db_types.push(sanitized_field("type")?);
                    read_onlys.push(
                        map.get("read_only")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false),
                    );
                }

                Ok(Attachments {
                    paths,
                    aliases,
                    db_types,
                    read_onlys,
                })
            }
            None => Ok(Attachments {
                paths: vec![],
                aliases: vec![],
                db_types: vec![],
                read_onlys: vec![],
            }),
            Some(other) => Err(AuthError::config(format!(
                "attach: expected a sequence, got {other:?}"
            ))),
        }
    }

    fn render(&self) -> Vec<String> {
        debug_assert_eq!(self.paths.len(), self.aliases.len());
        debug_assert_eq!(self.paths.len(), self.db_types.len());
        debug_assert_eq!(self.paths.len(), self.read_onlys.len());

        (0..self.paths.len())
            .map(|i| {
                let path_escaped = escape_single_quotes(self.paths[i]);
                let mut sql = format!("ATTACH IF NOT EXISTS '{path_escaped}'");

                if let Some(alias) = &self.aliases[i] {
                    sql.push_str(&format!(" AS {alias}"));
                }

                let mut opts = Vec::new();
                if let Some(t) = &self.db_types[i] {
                    opts.push(format!("TYPE {t}"));
                }
                if self.read_onlys[i] {
                    opts.push("READ_ONLY".to_owned());
                }
                if !opts.is_empty() {
                    sql.push_str(&format!(" ({})", opts.join(", ")));
                }

                sql
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Top-level entry point
// ---------------------------------------------------------------------------

pub fn motherduck_saas_mode_query() -> &'static str {
    "select coalesce(bool_or(try_cast(value as boolean)), false) as saas_mode from duckdb_settings() where name = 'motherduck_saas_mode'"
}

/// Generate DuckDB initialization SQL statements from the adapter config.
///
/// Returns an ordered list of SQL strings ready for sequential execution.
/// When the path is a MotherDuck connection (`md:` / `motherduck:`), the
/// `motherduck` extension is auto-installed/loaded and the token is injected.
pub fn generate_duckdb_init_sql(config: &AdapterConfig) -> Result<Vec<String>, AuthError> {
    let target = DuckDbTarget::from_config(config)?;
    let extensions = Extensions::from_config(config)?;
    let secrets = Secrets::from_config(config)?;
    let settings = Settings::from_config(config)?;
    let attachments = Attachments::from_config(config)?;

    let mut out = extensions.render(&target);
    out.extend(secrets.render());
    out.extend(settings.render());
    out.extend(attachments.render());
    Ok(out)
}

// ---------------------------------------------------------------------------
// SQL literal helpers
// ---------------------------------------------------------------------------

/// Keep only ASCII alphanumeric and underscore characters (SQL injection prevention).
fn sanitize_identifier(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect()
}

/// Whether `name` is non-empty and unchanged by [`sanitize_identifier`].
fn is_extension_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Whether `name` is a valid extension repository name, which never needs escaping inside
/// double quotes. DuckDB applies the same rule to user-provided repository names:
// https://github.com/duckdb/duckdb/blob/d54cbbe3df17e8e5ce68312f483b4155836112aa/src/main/extension/extension_repository_manager.cpp#L252-L263
fn is_repository_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Escape single quotes for SQL string literals (`'` → `''`).
fn escape_single_quotes(s: &str) -> String {
    s.replace('\'', "''")
}

/// Convert a [`YmlValue`] to a SQL literal.
///
/// - Strings → `'escaped'`
/// - Numbers / Bools → bare
/// - Null → `NULL`
/// - Sequences / Mappings → serialized as string
fn yml_value_to_sql_literal(v: &YmlValue) -> String {
    match v {
        YmlValue::String(s, _) => format!("'{}'", escape_single_quotes(s)),
        YmlValue::Number(n, _) => n.to_string(),
        YmlValue::Bool(b, _) => b.to_string(),
        YmlValue::Null(_) => "NULL".to_owned(),
        _ => {
            // Fallback: serialize as a quoted string.
            // All YmlValue variants are serializable, so this should never fail.
            let s = match dbt_yaml::to_string(v) {
                Ok(s) => s,
                Err(e) => {
                    debug_assert!(false, "YmlValue serialization failed: {e}");
                    return "NULL".to_owned();
                }
            };
            let s = s.trim_end_matches('\n');
            format!("'{}'", escape_single_quotes(s))
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn config_from_yaml(yaml: &str) -> AdapterConfig {
        let value: YmlValue = dbt_yaml::from_str(yaml).unwrap();
        let mapping = match value {
            YmlValue::Mapping(m, _) => m,
            _ => panic!("expected mapping"),
        };
        AdapterConfig::new(mapping)
    }

    #[test]
    fn test_empty_config() {
        let config = AdapterConfig::default();
        let stmts = generate_duckdb_init_sql(&config).unwrap();
        assert!(stmts.is_empty());
    }

    #[test]
    fn test_extensions() {
        let config = config_from_yaml(
            r#"
extensions:
  - httpfs
  - parquet
"#,
        );
        let stmts = generate_duckdb_init_sql(&config).unwrap();
        assert_eq!(
            stmts,
            vec![
                "INSTALL httpfs",
                "LOAD httpfs",
                "INSTALL parquet",
                "LOAD parquet",
            ]
        );
    }

    #[test]
    fn test_settings_string_and_number() {
        let config = config_from_yaml(
            r#"
settings:
  memory_limit: "2GB"
  threads: 4
  enable_progress_bar: true
"#,
        );
        let stmts = generate_duckdb_init_sql(&config).unwrap();
        assert_eq!(stmts.len(), 3);
        assert!(stmts.contains(&"SET memory_limit = '2GB'".to_string()));
        assert!(stmts.contains(&"SET threads = 4".to_string()));
        assert!(stmts.contains(&"SET enable_progress_bar = true".to_string()));
    }

    #[test]
    fn test_secret_with_name_and_provider() {
        let config = config_from_yaml(
            r#"
secrets:
  - type: s3
    name: my_s3_secret
    provider: credential_chain
    scope: "s3://my-bucket"
    region: us-east-1
"#,
        );
        let stmts = generate_duckdb_init_sql(&config).unwrap();
        assert_eq!(stmts.len(), 1);
        let sql = &stmts[0];
        assert!(sql.starts_with("CREATE OR REPLACE SECRET my_s3_secret ("));
        assert!(sql.contains("TYPE s3"));
        assert!(sql.contains("PROVIDER credential_chain"));
        assert!(sql.contains("SCOPE 's3://my-bucket'"));
        assert!(sql.contains("REGION 'us-east-1'"));
    }

    #[test]
    fn test_secret_without_name() {
        let config = config_from_yaml(
            r#"
secrets:
  - type: s3
    key_id: fake_key
"#,
        );
        let stmts = generate_duckdb_init_sql(&config).unwrap();
        assert_eq!(stmts.len(), 1);
        let sql = &stmts[0];
        assert!(sql.contains("SECRET __dbt_secret_0"));
        assert!(sql.contains("TYPE s3"));
        assert!(sql.contains("KEY_ID 'fake_key'"));
    }

    #[test]
    fn test_persistent_secret() {
        let config = config_from_yaml(
            r#"
secrets:
  - type: s3
    persistent: true
    key_id: my_key
    secret: my_secret
"#,
        );
        let stmts = generate_duckdb_init_sql(&config).unwrap();
        assert_eq!(stmts.len(), 1);
        let sql = &stmts[0];
        assert!(sql.starts_with("CREATE OR REPLACE PERSISTENT SECRET __dbt_secret_0 ("));
        assert!(sql.contains("TYPE s3"));
        assert!(sql.contains("KEY_ID 'my_key'"));
        assert!(sql.contains("SECRET 'my_secret'"));
    }

    #[test]
    fn test_secret_sql_injection_in_scope() {
        let config = config_from_yaml(
            r#"
secrets:
  - type: s3
    scope: "s3://bucket'; DROP TABLE users; --"
"#,
        );
        let stmts = generate_duckdb_init_sql(&config).unwrap();
        assert_eq!(stmts.len(), 1);
        let sql = &stmts[0];
        // Single quotes should be escaped
        assert!(sql.contains("SCOPE 's3://bucket''; DROP TABLE users; --'"));
    }

    #[test]
    fn test_attachment_minimal() {
        let config = config_from_yaml(
            r#"
attach:
  - path: ":memory:"
"#,
        );
        let stmts = generate_duckdb_init_sql(&config).unwrap();
        assert_eq!(stmts, vec!["ATTACH IF NOT EXISTS ':memory:'"]);
    }

    #[test]
    fn test_attachment_all_options() {
        let config = config_from_yaml(
            r#"
attach:
  - path: /data/external.db
    alias: ext
    type: duckdb
    read_only: true
"#,
        );
        let stmts = generate_duckdb_init_sql(&config).unwrap();
        assert_eq!(stmts.len(), 1);
        assert_eq!(
            stmts[0],
            "ATTACH IF NOT EXISTS '/data/external.db' AS ext (TYPE duckdb, READ_ONLY)"
        );
    }

    #[test]
    fn test_attachment_path_escaping() {
        let config = config_from_yaml(
            r#"
attach:
  - path: "/data/it's a db.duckdb"
    alias: weird
"#,
        );
        let stmts = generate_duckdb_init_sql(&config).unwrap();
        assert_eq!(stmts.len(), 1);
        assert_eq!(
            stmts[0],
            "ATTACH IF NOT EXISTS '/data/it''s a db.duckdb' AS weird"
        );
    }

    #[test]
    fn test_ordering_extensions_secrets_settings_attachments() {
        let config = config_from_yaml(
            r#"
extensions:
  - httpfs
settings:
  memory_limit: "2GB"
secrets:
  - type: s3
    key_id: k
attach:
  - path: ":memory:"
    alias: scratch
"#,
        );
        let stmts = generate_duckdb_init_sql(&config).unwrap();
        // Order: extensions, secrets, settings, attachments
        assert_eq!(stmts[0], "INSTALL httpfs");
        assert_eq!(stmts[1], "LOAD httpfs");
        assert!(stmts[2].starts_with("CREATE OR REPLACE"));
        assert!(stmts[3].starts_with("SET memory_limit"));
        assert!(stmts[4].starts_with("ATTACH IF NOT EXISTS"));
    }

    #[test]
    fn test_full_config() {
        let config = config_from_yaml(
            r#"
path: /tmp/test.db
extensions:
  - httpfs
  - parquet
settings:
  memory_limit: "2GB"
secrets:
  - type: s3
    key_id: fake_key
    secret: fake_secret
    region: us-east-1
attach:
  - path: ":memory:"
    alias: scratch
"#,
        );
        let stmts = generate_duckdb_init_sql(&config).unwrap();
        // 2 extensions * 2 stmts + 1 secret + 1 setting + 1 attachment = 7
        assert_eq!(stmts.len(), 7);
        assert_eq!(stmts[0], "INSTALL httpfs");
        assert_eq!(stmts[1], "LOAD httpfs");
        assert_eq!(stmts[2], "INSTALL parquet");
        assert_eq!(stmts[3], "LOAD parquet");
        assert!(stmts[4].contains("TYPE s3"));
        assert!(stmts[5].starts_with("SET memory_limit"));
        assert!(stmts[6].starts_with("ATTACH IF NOT EXISTS"));
    }

    // -----------------------------------------------------------------------
    // `database:` config for local/in-memory paths (dbt-labs/fs#14196)
    // -----------------------------------------------------------------------

    #[test]
    fn test_local_path_database_mismatch_hard_fails() {
        let config = config_from_yaml(
            r#"
path: "/tmp/scratch_file.duckdb"
database: "totally_different_name"
"#,
        );
        let err = generate_duckdb_init_sql(&config).unwrap_err();
        assert!(
            matches!(&err, AuthError::Config(msg) if msg == "Inconsistency detected between 'path' and 'database' fields in profile; the 'database' property must be set to 'scratch_file' to match the 'path'"),
            "{err:?}"
        );
    }

    #[test]
    fn test_memory_path_database_mismatch_hard_fails() {
        let config = config_from_yaml(
            r#"
database: "my_catalog"
"#,
        );
        let err = generate_duckdb_init_sql(&config).unwrap_err();
        assert!(
            matches!(&err, AuthError::Config(msg) if msg == "Inconsistency detected between 'path' and 'database' fields in profile; the 'database' property must be set to 'memory' to match the 'path'"),
            "{err:?}"
        );
    }

    #[test]
    fn test_local_path_database_matching_derived_name_succeeds() {
        let config = config_from_yaml(
            r#"
path: "/tmp/scratch_file.duckdb"
database: "scratch_file"
"#,
        );
        let stmts = generate_duckdb_init_sql(&config).unwrap();
        assert!(
            stmts.is_empty(),
            "database: matches derived name, no attach needed: {stmts:?}"
        );
    }

    #[test]
    fn test_local_path_without_database_config_unchanged() {
        let config = config_from_yaml(
            r#"
path: "/tmp/scratch_file.duckdb"
"#,
        );
        let stmts = generate_duckdb_init_sql(&config).unwrap();
        assert!(
            stmts.is_empty(),
            "no database: set, no attach should be emitted: {stmts:?}"
        );
    }

    #[test]
    fn test_local_database_matching_main_file_stem_succeeds() {
        let config = config_from_yaml(
            r#"
path: "/tmp/main.duckdb"
database: "main"
"#,
        );
        let stmts = generate_duckdb_init_sql(&config).unwrap();
        assert!(
            stmts.is_empty(),
            "database: 'main' matches the derived name for main.duckdb, no attach needed: {stmts:?}"
        );
    }

    #[test]
    fn test_ducklake_prefix_stripped_for_database_derivation() {
        let config = config_from_yaml(
            r#"
path: "ducklake:/tmp/scratch_file.duckdb"
database: "scratch_file"
"#,
        );
        let stmts = generate_duckdb_init_sql(&config).unwrap();
        assert!(
            stmts.is_empty(),
            "database: matches the name derived from the path under the ducklake: prefix: {stmts:?}"
        );
    }

    #[test]
    fn test_local_database_mismatch_hard_fails_without_sanitizing() {
        let config = config_from_yaml(
            r#"
path: "/tmp/scratch_file.duckdb"
database: "weird name!"
"#,
        );
        let err = generate_duckdb_init_sql(&config).unwrap_err();
        assert!(
            matches!(&err, AuthError::Config(msg) if msg == "Inconsistency detected between 'path' and 'database' fields in profile; the 'database' property must be set to 'scratch_file' to match the 'path'"),
            "{err:?}"
        );
    }

    #[test]
    fn test_sanitize_identifier() {
        assert_eq!(sanitize_identifier("normal_name"), "normal_name");
        assert_eq!(sanitize_identifier("has spaces"), "hasspaces");
        assert_eq!(sanitize_identifier("has;semicolons"), "hassemicolons");
        assert_eq!(sanitize_identifier("DROP TABLE--"), "DROPTABLE");
    }

    #[test]
    fn test_escape_single_quotes() {
        assert_eq!(escape_single_quotes("no quotes"), "no quotes");
        assert_eq!(escape_single_quotes("it's"), "it''s");
        assert_eq!(escape_single_quotes("a''b"), "a''''b");
    }

    #[test]
    fn test_empty_extension_name_skipped() {
        let config = config_from_yaml(
            r#"
extensions:
  - ""
  - httpfs
"#,
        );
        let stmts = generate_duckdb_init_sql(&config).unwrap();
        assert_eq!(stmts, vec!["INSTALL httpfs", "LOAD httpfs"]);
    }

    #[test]
    fn test_extension_sql_injection_sanitized() {
        let config = config_from_yaml(
            r#"
extensions:
  - "httpfs; DROP TABLE users"
"#,
        );
        let stmts = generate_duckdb_init_sql(&config).unwrap();
        // Semicolons and spaces stripped by sanitize_identifier
        assert_eq!(stmts[0], "INSTALL httpfsDROPTABLEusers");
        assert_eq!(stmts[1], "LOAD httpfsDROPTABLEusers");
    }

    #[test]
    fn test_extension_dictionaries_mixed_with_strings() {
        let config = config_from_yaml(
            r#"
extensions:
  - httpfs
  - name: parquet
  - name: jev
    repo: community
  - name: uc_catalog
    repo: core_nightly
"#,
        );
        assert_eq!(
            generate_duckdb_init_sql(&config).unwrap(),
            vec![
                "INSTALL httpfs",
                "LOAD httpfs",
                "INSTALL parquet",
                "LOAD parquet",
                "INSTALL jev FROM \"community\"",
                "LOAD jev",
                "INSTALL uc_catalog FROM \"core_nightly\"",
                "LOAD uc_catalog",
            ]
        );
    }

    #[test]
    fn test_extension_null_repo_uses_default_repository() {
        let config = config_from_yaml(
            r#"
extensions:
  - name: jev
    repo: null
"#,
        );
        assert_eq!(
            generate_duckdb_init_sql(&config).unwrap(),
            vec!["INSTALL jev", "LOAD jev"]
        );
    }

    #[test]
    fn test_extension_repository_name_is_quoted_verbatim() {
        let config = config_from_yaml(
            r#"
extensions:
  - name: a5
    repo: query-farm_2
  - name: jev
    repo: Community
"#,
        );
        // Case is left for DuckDB to resolve, as with `INSTALL ... FROM` in SQL.
        assert_eq!(
            generate_duckdb_init_sql(&config).unwrap(),
            vec![
                "INSTALL a5 FROM \"query-farm_2\"",
                "LOAD a5",
                "INSTALL jev FROM \"Community\"",
                "LOAD jev",
            ]
        );
    }

    #[test]
    fn test_extension_dictionary_errors_name_the_field() {
        for (item, expected) in [
            (
                "{repo: community}",
                "extensions[1]: missing required field 'name'",
            ),
            ("{name: 7}", "extensions[1].name: expected"),
            ("{name: null}", "extensions[1].name: expected"),
            ("{name: ''}", "extensions[1].name: expected"),
            (
                "{name: 'jev; DROP TABLE t'}",
                "extensions[1].name: expected",
            ),
            ("{name: [jev]}", "extensions[1].name: expected"),
            ("{name: jev, repo: ''}", "extensions[1].repo: expected"),
            ("{name: jev, repo: 7}", "extensions[1].repo: expected"),
            ("{name: jev, repo: true}", "extensions[1].repo: expected"),
            (
                "{name: jev, repo: [community]}",
                "extensions[1].repo: expected",
            ),
            (
                "{name: jev, repo: 'community; DROP TABLE t'}",
                "extensions[1].repo: expected",
            ),
            (
                "{name: jev, repo: 'comm\"unity'}",
                "extensions[1].repo: expected",
            ),
            (
                "{name: jev, repo: 'https://extensions.invalid'}",
                "extensions[1].repo: expected",
            ),
            (
                "{name: jev, repo: \"'https://extensions.invalid'\"}",
                "extensions[1].repo: expected",
            ),
            (
                "{name: jev, version: v1}",
                "extensions[1]: unknown field 'version'",
            ),
            (
                "{name: jev, 1: x}",
                "extensions[1]: field names must be strings",
            ),
            (
                "7",
                "extensions[1]: expected an extension name or a mapping",
            ),
            (
                "null",
                "extensions[1]: expected an extension name or a mapping",
            ),
            (
                "[jev]",
                "extensions[1]: expected an extension name or a mapping",
            ),
        ] {
            let config = config_from_yaml(&format!("extensions: [httpfs, {item}]"));
            let err = generate_duckdb_init_sql(&config).err().unwrap();
            assert!(err.msg().starts_with(expected), "{item}: {}", err.msg());
            assert!(!err.msg().contains("DROP"), "{item}: {}", err.msg());
            assert!(!err.msg().contains("invalid'"), "{item}: {}", err.msg());
        }
    }

    #[test]
    fn test_motherduck_dictionary_counts_as_present() {
        for entry in ["{name: motherduck}", "{name: MotherDuck, repo: core}"] {
            let config =
                config_from_yaml(&format!("path: 'md:my_db'\nextensions: [httpfs, {entry}]"));
            let stmts = generate_duckdb_init_sql(&config).unwrap();
            let motherduck_loads = stmts
                .iter()
                .filter(|s| s.eq_ignore_ascii_case("LOAD motherduck"))
                .count();
            assert_eq!(motherduck_loads, 1, "{entry}: {stmts:?}");
            assert_eq!(stmts[0], "INSTALL httpfs", "{entry}");
        }
    }

    #[test]
    fn test_motherduck_auto_injected_once_before_dictionaries() {
        let config = config_from_yaml(
            r#"
path: "md:my_db"
extensions:
  - name: jev
    repo: community
"#,
        );
        assert_eq!(
            generate_duckdb_init_sql(&config).unwrap(),
            vec![
                "INSTALL motherduck",
                "LOAD motherduck",
                "INSTALL jev FROM \"community\"",
                "LOAD jev",
            ]
        );
    }

    #[test]
    fn test_saas_mode_skips_install_for_dictionaries() {
        let config = config_from_yaml(
            r#"
path: "md:my_db"
settings:
  motherduck_saas_mode: true
extensions:
  - httpfs
  - name: jev
    repo: community
"#,
        );
        assert_eq!(
            generate_duckdb_init_sql(&config).unwrap(),
            vec!["LOAD motherduck", "LOAD httpfs", "LOAD jev"]
        );
    }

    #[test]
    fn test_dictionary_extensions_precede_secrets_settings_and_attachments() {
        let config = config_from_yaml(
            r#"
extensions:
  - name: jev
    repo: community
secrets:
  - type: s3
settings:
  threads: 4
attach:
  - path: /data/other.db
"#,
        );
        assert_eq!(
            generate_duckdb_init_sql(&config).unwrap(),
            vec![
                "INSTALL jev FROM \"community\"",
                "LOAD jev",
                "CREATE OR REPLACE SECRET __dbt_secret_0 (TYPE s3)",
                "SET threads = 4",
                "ATTACH IF NOT EXISTS '/data/other.db'",
            ]
        );
    }

    #[test]
    fn test_secret_type_only() {
        let config = config_from_yaml(
            r#"
secrets:
  - type: s3
"#,
        );
        let stmts = generate_duckdb_init_sql(&config).unwrap();
        assert_eq!(stmts.len(), 1);
        assert_eq!(
            stmts[0],
            "CREATE OR REPLACE SECRET __dbt_secret_0 (TYPE s3)"
        );
    }

    #[test]
    fn test_multiple_attachments_ordering() {
        let config = config_from_yaml(
            r#"
attach:
  - path: /data/first.db
    alias: first
  - path: /data/second.db
    alias: second
  - path: ":memory:"
    alias: scratch
"#,
        );
        let stmts = generate_duckdb_init_sql(&config).unwrap();
        assert_eq!(stmts.len(), 3);
        assert_eq!(stmts[0], "ATTACH IF NOT EXISTS '/data/first.db' AS first");
        assert_eq!(stmts[1], "ATTACH IF NOT EXISTS '/data/second.db' AS second");
        assert_eq!(stmts[2], "ATTACH IF NOT EXISTS ':memory:' AS scratch");
    }

    #[test]
    fn test_setting_value_with_single_quotes() {
        let config = config_from_yaml(
            r#"
settings:
  custom_setting: "it's a value"
"#,
        );
        let stmts = generate_duckdb_init_sql(&config).unwrap();
        assert_eq!(stmts.len(), 1);
        assert_eq!(stmts[0], "SET custom_setting = 'it''s a value'");
    }

    #[test]
    fn test_settings_only_no_extensions() {
        let config = config_from_yaml(
            r#"
settings:
  memory_limit: "4GB"
  threads: 8
"#,
        );
        let stmts = generate_duckdb_init_sql(&config).unwrap();
        assert_eq!(stmts.len(), 2);
        assert!(stmts.contains(&"SET memory_limit = '4GB'".to_owned()));
        assert!(stmts.contains(&"SET threads = 8".to_owned()));
    }

    #[test]
    fn test_database_name() {
        assert_eq!(DuckDbTarget::database_name("md:my_db"), "my_db");
        assert_eq!(DuckDbTarget::database_name("md:"), "");
        assert_eq!(DuckDbTarget::database_name("motherduck:sales"), "sales");
        assert_eq!(
            DuckDbTarget::database_name("md:my_db?motherduck_token=tok123"),
            "my_db"
        );
    }

    #[test]
    fn test_attach_path_strips_query() {
        assert_eq!(
            DuckDbTarget::attach_path("md:my_db?motherduck_token=tok"),
            "md:my_db"
        );
        assert_eq!(
            DuckDbTarget::attach_path("motherduck:sales?user=1"),
            "motherduck:sales"
        );
        assert_eq!(DuckDbTarget::attach_path("md:plain"), "md:plain");
    }

    #[test]
    fn test_motherduck_auto_extension() {
        let config = config_from_yaml(
            r#"
path: "md:my_db"
"#,
        );
        let stmts = generate_duckdb_init_sql(&config).unwrap();
        assert_eq!(stmts[0], "INSTALL motherduck");
        assert_eq!(stmts[1], "LOAD motherduck");
    }

    #[test]
    fn test_motherduck_auto_extension_not_duplicated() {
        let config = config_from_yaml(
            r#"
path: "md:my_db"
extensions:
  - motherduck
  - httpfs
"#,
        );
        let stmts = generate_duckdb_init_sql(&config).unwrap();
        // Should not have duplicate INSTALL/LOAD motherduck
        let install_count = stmts.iter().filter(|s| *s == "INSTALL motherduck").count();
        assert_eq!(install_count, 1);
    }

    #[test]
    fn test_motherduck_token_not_set_for_local_path() {
        let config = config_from_yaml(
            r#"
path: "/tmp/local.duckdb"
settings:
  motherduck_token: "my_secret_token"
"#,
        );
        let stmts = generate_duckdb_init_sql(&config).unwrap();
        assert!(
            !stmts.iter().any(|s| s.starts_with("SET motherduck_token")),
            "motherduck_token should not be emitted for local DuckDB paths"
        );
    }

    #[test]
    fn test_resolve_motherduck_token_from_settings() {
        let config = config_from_yaml(
            r#"
path: "md:my_db"
settings:
  motherduck_token: "my_secret_token"
"#,
        );
        assert_eq!(
            DuckDbTarget::resolve_token(
                config
                    .get("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default(),
                &config
            ),
            Some("my_secret_token".to_owned())
        );
    }

    #[test]
    fn test_resolve_motherduck_token_from_path_query() {
        let config = config_from_yaml(
            r#"
path: "md:my_db?motherduck_token=tok_from_path"
"#,
        );
        assert_eq!(
            DuckDbTarget::resolve_token(
                config
                    .get("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default(),
                &config
            ),
            Some("tok_from_path".to_owned())
        );
    }

    #[test]
    fn test_resolve_motherduck_token_settings_wins() {
        // When token is in both settings and path, settings wins
        let config = config_from_yaml(
            r#"
path: "md:my_db?motherduck_token=tok_from_path"
settings:
  motherduck_token: "tok_from_settings"
"#,
        );
        assert_eq!(
            DuckDbTarget::resolve_token(
                config
                    .get("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default(),
                &config
            ),
            Some("tok_from_settings".to_owned())
        );
    }

    // -----------------------------------------------------------------------
    // yml_value_to_sql_literal untested paths
    // -----------------------------------------------------------------------

    #[test]
    fn test_null_setting_value_emits_null_literal() {
        let config = config_from_yaml(
            r#"
settings:
  my_null: ~
"#,
        );
        let stmts = generate_duckdb_init_sql(&config).unwrap();
        assert_eq!(stmts, vec!["SET my_null = NULL"]);
    }

    #[test]
    fn test_sequence_setting_value_uses_fallback_literal() {
        // Exercises the Sequence/Mapping fallback path in yml_value_to_sql_literal.
        let config = config_from_yaml(
            r#"
settings:
  my_list:
    - foo
    - bar
"#,
        );
        let stmts = generate_duckdb_init_sql(&config).unwrap();
        assert_eq!(stmts.len(), 1);
        // Should be a quoted string (not crash, not NULL)
        assert!(
            stmts[0].starts_with("SET my_list = '"),
            "expected quoted fallback literal: {}",
            stmts[0]
        );
    }

    #[test]
    fn test_mapping_setting_value_uses_fallback_literal() {
        let config = config_from_yaml(
            r#"
settings:
  my_map:
    key: value
"#,
        );
        let stmts = generate_duckdb_init_sql(&config).unwrap();
        assert_eq!(stmts.len(), 1);
        assert!(
            stmts[0].starts_with("SET my_map = '"),
            "expected quoted fallback literal: {}",
            stmts[0]
        );
    }

    // -----------------------------------------------------------------------
    // Empty token in settings falls through to other sources
    // -----------------------------------------------------------------------

    #[test]
    fn test_empty_settings_token_falls_through_to_path_query() {
        let config = config_from_yaml(
            r#"
path: "md:my_db?motherduck_token=tok_from_path"
settings:
  motherduck_token: ""
"#,
        );
        // Empty settings token should be skipped; path query should win
        assert_eq!(
            DuckDbTarget::resolve_token(
                config
                    .get("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default(),
                &config
            ),
            Some("tok_from_path".to_owned())
        );
    }

    #[test]
    fn test_empty_settings_token_with_no_other_source_returns_none() {
        let config = config_from_yaml(
            r#"
path: "md:my_db"
settings:
  motherduck_token: ""
"#,
        );
        assert_eq!(
            DuckDbTarget::resolve_token(
                config
                    .get("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default(),
                &config
            ),
            None
        );
    }

    // -----------------------------------------------------------------------
    // Secret edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn test_non_mapping_secret_item_is_skipped() {
        // A non-mapping secret item now causes an error in parse()
        let config = config_from_yaml(
            r#"
secrets:
  - "not a mapping"
  - type: s3
    key_id: real_key
"#,
        );
        let stmts = generate_duckdb_init_sql(&config).unwrap();
        assert_eq!(stmts.len(), 1);
        assert!(stmts[0].contains("KEY_ID 'real_key'"));
    }

    #[test]
    fn test_secret_missing_type_is_skipped() {
        let config = config_from_yaml(
            r#"
secrets:
  - key_id: some_key
"#,
        );
        assert!(generate_duckdb_init_sql(&config).unwrap().is_empty());
    }

    // -----------------------------------------------------------------------
    // Attachment edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn test_non_mapping_attach_item_is_skipped() {
        // A non-mapping attach item now causes an error in parse()
        let config = config_from_yaml(
            r#"
attach:
  - "not a mapping"
  - path: /data/real.db
    alias: real
"#,
        );
        assert!(generate_duckdb_init_sql(&config).is_err());
    }

    #[test]
    fn test_attach_missing_path_is_skipped() {
        // Missing `path` is now a hard error from serde
        let config = config_from_yaml(
            r#"
attach:
  - alias: scratch
"#,
        );
        assert!(generate_duckdb_init_sql(&config).is_err());
    }

    #[test]
    fn test_attach_read_only_false_no_type_produces_no_parens() {
        let config = config_from_yaml(
            r#"
attach:
  - path: /data/real.db
    read_only: false
"#,
        );
        let stmts = generate_duckdb_init_sql(&config).unwrap();
        assert_eq!(stmts.len(), 1);
        assert_eq!(stmts[0], "ATTACH IF NOT EXISTS '/data/real.db'");
    }

    // -----------------------------------------------------------------------
    // Malformed top-level shapes
    // -----------------------------------------------------------------------

    #[test]
    fn test_extensions_as_scalar_produces_no_statements() {
        // extensions as scalar is now a hard error
        let config = config_from_yaml(
            r#"
extensions: httpfs
"#,
        );
        assert!(generate_duckdb_init_sql(&config).is_err());
    }

    #[test]
    fn test_settings_as_sequence_produces_no_statements() {
        // settings as sequence is now a hard error
        let config = config_from_yaml(
            r#"
settings:
  - foo
  - bar
"#,
        );
        assert!(generate_duckdb_init_sql(&config).is_err());
    }

    #[test]
    fn test_secrets_as_scalar_produces_no_statements() {
        // secrets as scalar is now a hard error
        let config = config_from_yaml(
            r#"
secrets: my_secret
"#,
        );
        assert!(generate_duckdb_init_sql(&config).is_err());
    }

    // -----------------------------------------------------------------------
    // MotherDuck alias edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn test_saas_setting_rejected_without_motherduck_path() {
        let config = config_from_yaml(
            r#"
path: "/tmp/local.duckdb"
settings:
  motherduck_saas_mode: true
"#,
        );
        let err = generate_duckdb_init_sql(&config).unwrap_err();
        assert!(
            format!("{err:?}").contains("motherduck_saas_mode"),
            "{err:?}"
        );
    }

    #[test]
    fn test_saas_setting_rejected_when_only_plain_attachments() {
        let config = config_from_yaml(
            r#"
path: "/tmp/local.duckdb"
attach:
  - path: "/tmp/other.duckdb"
settings:
  motherduck_saas_mode: "true"
"#,
        );
        assert!(generate_duckdb_init_sql(&config).is_err());
    }

    #[test]
    fn test_saas_setting_false_accepted_without_motherduck_path() {
        let config = config_from_yaml(
            r#"
path: "/tmp/local.duckdb"
settings:
  motherduck_saas_mode: false
"#,
        );
        assert!(generate_duckdb_init_sql(&config).is_ok());
    }

    #[test]
    fn test_saas_setting_accepted_with_motherduck_attachment() {
        let config = config_from_yaml(
            r#"
path: "/tmp/local.duckdb"
attach:
  - path: "md:shared_db"
settings:
  motherduck_saas_mode: true
"#,
        );
        assert!(generate_duckdb_init_sql(&config).is_ok());
    }

    #[test]
    fn test_parse_query_cases() {
        assert_eq!(parse_query("").count(), 0);

        let encoded: Vec<_> = parse_query("motherduck_token=ab%2Bc+d%3D").collect();
        assert_eq!(encoded.len(), 1);
        assert_eq!(encoded[0].0, "motherduck_token");
        assert_eq!(encoded[0].1, "ab+c d=");

        let decoded: Vec<_> = parse_query("motherduck_token=abc123").collect();
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].1, "abc123");
        assert!(matches!(decoded[0].1, Cow::Borrowed(_)));
    }

    #[test]
    fn test_motherduck_path_is_not_attached_in_init_sql() {
        let config = config_from_yaml(
            r#"
path: "md:my_db"
"#,
        );
        let stmts = generate_duckdb_init_sql(&config).unwrap();
        assert!(!stmts.iter().any(|s| s.contains("md:my_db")), "{stmts:?}");
        assert!(!stmts.iter().any(|s| s.starts_with("USE ")), "{stmts:?}");
    }
}
