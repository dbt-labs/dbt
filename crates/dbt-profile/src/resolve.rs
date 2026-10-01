use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, OnceLock};

use minijinja::Environment;
use minijinja::listener::RenderingEventListener;
use regex::Regex;
use serde::Serialize;

use crate::adapters::{AdapterConnections, ResolutionMode, parse_connections};
use crate::error::{ProfileError, Result};
use crate::jinja::{ProfileContext, profile_environment};

// ---------------------------------------------------------------------------
// ProfileEnvironment — the shared Jinja env + context
// ---------------------------------------------------------------------------

/// The Jinja environment and rendering context needed for profile resolution.
/// Self-contained — no dependency on `dbt-jinja-utils` or `dbt-common`.
pub struct ProfileEnvironment {
    pub env: Environment<'static>,
    pub ctx: ProfileContext,
    env_overrides: Arc<BTreeMap<String, String>>,
}

impl ProfileEnvironment {
    /// Build the profile-resolution environment.
    ///
    /// `vars` are the CLI `--vars` values, exposed as `var(...)` in Jinja.
    /// `env_var(...)` reads from `std::env`, resolving secret placeholders after rendering.
    pub fn new(vars: BTreeMap<String, dbt_yaml::Value>) -> Self {
        Self::with_env_overrides(vars, BTreeMap::new())
    }

    /// Use caller-supplied values ahead of the inherited process environment,
    /// without modifying it. The caller owns any filtering policy.
    pub fn with_env_overrides(
        vars: BTreeMap<String, dbt_yaml::Value>,
        overrides: BTreeMap<String, String>,
    ) -> Self {
        let overrides = Arc::new(overrides);
        Self {
            env: profile_environment(),
            ctx: ProfileContext::new(vars, Arc::clone(&overrides)),
            env_overrides: overrides,
        }
    }
}

// ---------------------------------------------------------------------------
// ResolveArgs / ResolvedProfile — public types
// ---------------------------------------------------------------------------

/// Arguments for resolving a dbt profile.
#[derive(Debug, Clone, Default)]
pub struct ResolveArgs {
    /// Override the profiles directory (equivalent to `--profiles-dir`).
    /// Searched first; if unset falls back to `project_dir` then `~/.dbt/`.
    pub profiles_dir: Option<PathBuf>,

    /// Path to the dbt project directory.
    /// Used to read `dbt_project.yml` for the default profile name (standalone mode only).
    pub project_dir: Option<PathBuf>,

    /// Explicit profile name (equivalent to `--profile`).
    /// If unset in standalone mode, read from `dbt_project.yml`.
    pub profile: Option<String>,

    /// Explicit target name (equivalent to `--target` / `DBT_TARGET`).
    /// If unset, uses the profile's `target:` field, defaulting to `"default"`.
    pub target: Option<String>,

    /// Variables to inject into the Jinja context as `var(...)`.
    /// Equivalent to `--vars '{"key": "value"}'`.
    pub vars: BTreeMap<String, dbt_yaml::Value>,

    /// Caller-scoped values, taking precedence over the inherited environment.
    pub env_overrides: BTreeMap<String, String>,
}

/// A fully-resolved dbt profile ready for use.
#[derive(Debug, Clone)]
pub struct ResolvedProfile {
    /// The profile name that was resolved.
    pub profile_name: String,
    /// The target name that was selected.
    pub target_name: String,
    /// The adapter type string (e.g. `"snowflake"`, `"postgres"`, `"duckdb"`).
    pub adapter_type: String,
    /// The rendered target output as a YAML mapping — the source of truth for
    /// connection configuration. Flows directly to `AdapterConfig` / `dbt-auth`.
    ///
    /// This is the *default* adapter's config. When the target declares several
    /// adapters, read [`Self::adapters`] to reach the others.
    pub credentials: dbt_yaml::Mapping,
    /// Every adapter the target declares, in declaration order, keyed by adapter
    /// type, each with its connections. A target using the legacy
    /// single-adapter shape yields one entry with one connection, so callers
    /// never branch on the `profiles.yml` shape.
    ///
    /// This is where non-default adapters — including an alternate compute
    /// target — are reached. There is no separate field for them.
    pub adapters: Vec<AdapterConnections>,
    /// Path to the `profiles.yml` file that was loaded.
    pub profile_path: PathBuf,
}

impl ResolvedProfile {
    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.credentials.get(key).and_then(|v| v.as_str())
    }

    /// The adapter a node's `+adapter` config selects, if the target declares it.
    pub fn adapter_by_type(&self, adapter_type: &str) -> Option<&AdapterConnections> {
        self.adapters
            .iter()
            .find(|a| a.adapter_type.eq_ignore_ascii_case(adapter_type))
    }

    /// The target's default adapter — the one used by nodes that select none.
    ///
    /// The adapter holding the connection marked `default: true`, which is why the
    /// marker is resolved target-wide rather than per adapter.
    pub fn default_adapter(&self) -> &AdapterConnections {
        self.adapters
            .iter()
            .find(|a| a.connections.iter().any(|c| c.is_default))
            .expect("a resolved profile always has exactly one default connection")
    }

    /// The adapters a node may select *other* than the default, in declaration
    /// order.
    pub fn non_default_adapters(&self) -> impl Iterator<Item = &AdapterConnections> {
        let default = self.default_adapter().adapter_type.clone();
        self.adapters
            .iter()
            .filter(move |a| a.adapter_type != default)
    }

    pub fn database(&self) -> Option<&str> {
        self.get_str("database")
            .or_else(|| self.get_str("dbname"))
            .or_else(|| self.get_str("project"))
    }

    pub fn schema(&self) -> Option<&str> {
        self.get_str("schema").or_else(|| self.get_str("dataset"))
    }

    pub fn credentials_json(&self) -> std::result::Result<String, serde_json::Error> {
        let json_map: BTreeMap<String, serde_json::Value> = self
            .credentials
            .iter()
            .filter_map(|(k, v)| {
                let key = k.as_str()?.to_owned();
                let val = yaml_value_to_json(v);
                Some((key, val))
            })
            .collect();
        serde_json::to_string(&json_map)
    }
}

// ---------------------------------------------------------------------------
// High-level: resolve() — self-contained entrypoint
// ---------------------------------------------------------------------------

/// Selection metadata only; this does not validate connection credentials.
#[derive(Debug, Clone)]
pub struct ProfileMetadata {
    pub profile_name: String,
    pub target_name: String,
    pub adapter_type: String,
    pub profile_path: PathBuf,
}

/// Identify the selected adapter without requiring connection credentials.
pub fn resolve_metadata(args: &ResolveArgs) -> Result<ProfileMetadata> {
    let resolved = resolve_args(args, ResolutionMode::Metadata)?;
    Ok(ProfileMetadata {
        profile_name: resolved.profile_name,
        target_name: resolved.target_name,
        adapter_type: resolved.adapter_type,
        profile_path: resolved.profile_path,
    })
}

/// Resolve a dbt profile from `profiles.yml` in one call.
///
/// Creates its own `ProfileEnvironment` internally. For callers that need
/// to share the environment (e.g. `dbt-loader` rendering `dbt_project.yml`
/// with the same env), use the building blocks directly:
/// [`ProfileEnvironment::new`], [`find_profiles_path`], [`resolve_target`],
/// [`render_target`].
pub fn resolve(args: &ResolveArgs) -> Result<ResolvedProfile> {
    resolve_args(args, ResolutionMode::Credentials)
}

fn resolve_args(args: &ResolveArgs, mode: ResolutionMode) -> Result<ResolvedProfile> {
    let penv =
        ProfileEnvironment::with_env_overrides(args.vars.clone(), args.env_overrides.clone());
    let profile_path = find_profiles_path(args.profiles_dir.as_deref())?;
    let profile_name = resolve_profile_name(args)?;

    resolve_profile(
        &penv,
        &profile_path,
        &profile_name,
        args.target.as_deref(),
        mode,
    )
}

/// Resolve a profile using an externally-provided [`ProfileEnvironment`].
pub fn resolve_with_env(
    penv: &ProfileEnvironment,
    profile_path: &Path,
    profile_name: &str,
    target_override: Option<&str>,
) -> Result<ResolvedProfile> {
    resolve_profile(
        penv,
        profile_path,
        profile_name,
        target_override,
        ResolutionMode::Credentials,
    )
}

fn resolve_profile(
    penv: &ProfileEnvironment,
    profile_path: &Path,
    profile_name: &str,
    target_override: Option<&str>,
    mode: ResolutionMode,
) -> Result<ResolvedProfile> {
    let raw_yaml = std::fs::read_to_string(profile_path)?;
    let sanitized = sanitize_yml(&raw_yaml);
    let mut doc: dbt_yaml::Value =
        dbt_yaml::from_str(sanitized).map_err(|e| ProfileError::Yaml {
            path: profile_path.to_path_buf(),
            source: e,
        })?;
    doc.apply_merge().map_err(|e| ProfileError::Yaml {
        path: profile_path.to_path_buf(),
        source: e,
    })?;

    // dbt-core allows a top-level `config` key (for `send_anonymous_usage_stats`).
    // We reject it to match DbtProfilesIntermediate validation in dbt-schemas.
    if doc.get("config").is_some() {
        return Err(ProfileError::Other(
            "Unexpected 'config' key in profiles.yml".to_string(),
        ));
    }

    let profile_val = doc
        .get(profile_name)
        .ok_or_else(|| ProfileError::ProfileMissing {
            profile: profile_name.to_owned(),
            path: profile_path.to_path_buf(),
        })?;

    let target_name = resolve_target(target_override, profile_val, penv)?;

    let outputs = profile_val
        .get("outputs")
        .ok_or_else(|| ProfileError::NoOutputs {
            profile: profile_name.to_owned(),
        })?;

    let target_raw = outputs
        .get(&target_name)
        .ok_or_else(|| ProfileError::TargetMissing {
            profile: profile_name.to_owned(),
            target: target_name.clone(),
        })?;

    // Both `profiles.yml` shapes collapse to the same structure here; the legacy
    // single-adapter mapping yields one adapter with one connection, so
    // `credentials` and `adapter_type` below keep their existing meaning for every
    // caller.
    let adapters = parse_connections(profile_name, &target_name, target_raw, penv, mode)?;

    let default_adapter = adapters
        .iter()
        .find(|a| a.connections.iter().any(|c| c.is_default))
        .expect("parse_target_connections guarantees exactly one default connection");

    let credentials = default_adapter.default_connection().credentials.clone();
    let adapter_type = default_adapter.adapter_type.clone();

    Ok(ResolvedProfile {
        profile_name: profile_name.to_owned(),
        target_name,
        adapter_type,
        credentials,
        adapters,
        profile_path: profile_path.to_path_buf(),
    })
}

// ---------------------------------------------------------------------------
// Building blocks — exposed for dbt-loader composition
// ---------------------------------------------------------------------------

const PROFILES_YML: &str = "profiles.yml";

/// Find the `profiles.yml` file by searching standard locations.
///
/// Search order:
/// 1. `profiles_dir` (e.g. from `--profiles-dir`) — exclusive, no fallback.
/// 2. Current working directory.
/// 3. `~/.dbt/`.
///
/// When `profiles_dir` is set, only that directory is searched. This matches
/// dbt semantics where an explicit `--profiles-dir` that lacks profiles.yml
/// must error.
pub fn find_profiles_path(profiles_dir: Option<&Path>) -> Result<PathBuf> {
    let mut searched = Vec::new();

    if let Some(dir) = profiles_dir {
        let p = dir.join(PROFILES_YML);
        if p.exists() {
            return Ok(p);
        }
        searched.push(p);
        // When profiles_dir is explicitly set, do NOT fall back — error instead
        return Err(ProfileError::NotFound {
            searched,
            explicit_profiles_dir: true,
        });
    }

    if let Ok(cwd) = std::env::current_dir() {
        let p = cwd.join(PROFILES_YML);
        if p.exists() {
            return Ok(p);
        }
        searched.push(p);
    }

    if let Some(home) = dirs::home_dir() {
        let p = home.join(".dbt").join(PROFILES_YML);
        if p.exists() {
            return Ok(p);
        }
        searched.push(p);
    }

    Err(ProfileError::NotFound {
        searched,
        explicit_profiles_dir: false,
    })
}

/// Resolve the target name from overrides, `DBT_TARGET`, or the profile's
/// `target:` field (which may itself contain Jinja).
pub fn resolve_target(
    target_override: Option<&str>,
    profile_val: &dbt_yaml::Value,
    penv: &ProfileEnvironment,
) -> Result<String> {
    if let Some(t) = target_override {
        return Ok(t.to_owned());
    }

    if let Some(t) = lookup_env(&penv.env_overrides, "DBT_TARGET").filter(|s| !s.is_empty()) {
        return Ok(t);
    }

    if let Some(raw_str) = profile_val.get("target").and_then(|v| v.as_str()) {
        if raw_str.contains("{{") || raw_str.contains("{%") {
            let rendered = penv
                .env
                .render_str(raw_str, &penv.ctx, &[])
                .map_err(ProfileError::Jinja)?;
            return render_secrets(&rendered, &penv.env_overrides);
        }
        return Ok(raw_str.to_owned());
    }

    Ok("default".to_owned())
}

/// Render a profile target output mapping through Jinja, returning the
/// resolved credentials as a loose `dbt_yaml::Mapping`.
pub fn render_target(
    target_val: &dbt_yaml::Value,
    penv: &ProfileEnvironment,
) -> Result<dbt_yaml::Mapping> {
    let rendered = render_value_recursive(&penv.env, &penv.ctx, target_val, &penv.env_overrides)?;
    match rendered {
        dbt_yaml::Value::Mapping(m, _) => Ok(m),
        _ => Err(ProfileError::Other(
            "rendered target output is not a mapping".to_owned(),
        )),
    }
}

// ---------------------------------------------------------------------------
// Profile name resolution (private — standalone mode only)
// ---------------------------------------------------------------------------

fn resolve_profile_name(args: &ResolveArgs) -> Result<String> {
    if let Some(name) = &args.profile {
        return Ok(name.clone());
    }

    if let Some(name) = args
        .project_dir
        .as_ref()
        .and_then(|d| read_profile_from_project(d))
    {
        return Ok(name);
    }

    Err(ProfileError::NoProfileName)
}

fn read_profile_from_project(project_dir: &Path) -> Option<String> {
    let path = project_dir.join("dbt_project.yml");
    let raw = std::fs::read_to_string(path).ok()?;
    let doc: dbt_yaml::Value = dbt_yaml::from_str(&raw).ok()?;
    doc.get("profile")?.as_str().map(String::from)
}

// ---------------------------------------------------------------------------
// Recursive Jinja rendering over YAML values
// ---------------------------------------------------------------------------

fn render_value_recursive<S: Serialize>(
    env: &Environment<'_>,
    ctx: &S,
    value: &dbt_yaml::Value,
    overrides: &BTreeMap<String, String>,
) -> Result<dbt_yaml::Value> {
    let listeners: &[Rc<dyn RenderingEventListener>] = &[];

    match value {
        dbt_yaml::Value::String(s, span) => {
            let has_jinja = s.contains("{{") || s.contains("{%");
            let rendered = if has_jinja {
                env.render_str(s, ctx, listeners)
                    .map_err(ProfileError::Jinja)?
            } else {
                s.clone()
            };
            let resolved = render_secrets(&rendered, overrides)?;
            if !has_jinja && resolved == rendered {
                Ok(value.clone())
            } else {
                match dbt_yaml::from_str::<dbt_yaml::Value>(&resolved) {
                    Ok(parsed) => match &parsed {
                        dbt_yaml::Value::String(_, _) => {
                            Ok(dbt_yaml::Value::String(resolved, span.clone()))
                        }
                        _ => Ok(parsed),
                    },
                    Err(_) => Ok(dbt_yaml::Value::String(resolved, span.clone())),
                }
            }
        }
        dbt_yaml::Value::Mapping(map, span) => {
            let mut new_map = dbt_yaml::Mapping::new();
            for (k, v) in map.iter() {
                new_map.insert(k.clone(), render_value_recursive(env, ctx, v, overrides)?);
            }
            Ok(dbt_yaml::Value::Mapping(new_map, span.clone()))
        }
        dbt_yaml::Value::Sequence(seq, span) => {
            let rendered: std::result::Result<Vec<_>, _> = seq
                .iter()
                .map(|v| render_value_recursive(env, ctx, v, overrides))
                .collect();
            Ok(dbt_yaml::Value::Sequence(rendered?, span.clone()))
        }
        _ => Ok(value.clone()),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn lookup_env(overrides: &BTreeMap<String, String>, name: &str) -> Option<String> {
    lookup_env_override(overrides, name)
        .cloned()
        .or_else(|| std::env::var(name).ok())
}

pub(crate) fn lookup_env_override<'a>(
    overrides: &'a BTreeMap<String, String>,
    name: &str,
) -> Option<&'a String> {
    scoped_env_value(overrides, name, cfg!(windows))
}

fn scoped_env_value<'a>(
    overrides: &'a BTreeMap<String, String>,
    name: &str,
    case_insensitive: bool,
) -> Option<&'a String> {
    if case_insensitive {
        // Command::envs applies entries in iteration order, so the last
        // equivalent key wins even when an earlier key matches exactly.
        overrides
            .iter()
            .rev()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value)
    } else {
        overrides.get(name)
    }
}

/// Resolve secret sentinels using the same scoped environment as `env_var`.
/// Kept here to avoid a dependency on `dbt-jinja-utils`.
fn render_secrets(s: &str, overrides: &BTreeMap<String, String>) -> Result<String> {
    use dbt_jinja_vars::{SECRET_ENV_VAR_PREFIX, SECRET_PLACEHOLDER};

    if !s.contains(SECRET_ENV_VAR_PREFIX) {
        return Ok(s.to_owned());
    }

    static RE: OnceLock<Regex> = OnceLock::new();
    let pattern = SECRET_PLACEHOLDER
        .replace("{}", &format!("({SECRET_ENV_VAR_PREFIX}.*?)"))
        .replace("$", r"\$");
    let re = RE.get_or_init(|| Regex::new(&pattern).expect("valid secret placeholder regex"));

    let mut result = s.to_owned();
    for caps in re.captures_iter(s) {
        let var_name = &caps[1];
        let full_match = &caps[0];
        match lookup_env(overrides, var_name) {
            Some(value) => result = result.replace(full_match, &value),
            None => {
                return Err(ProfileError::Other(format!(
                    "Secret environment variable '{var_name}' not found"
                )));
            }
        }
    }
    Ok(result)
}

// FIXME(jason): Duplicated from dbt_jinja_utils::serde::dbt_sanitize_yml because
// IoArgs poisons that module's public API.
fn sanitize_yml(input: &str) -> &str {
    // Remove any UTF-8 BOM from the beginning of the input string -- the YAML
    // parser confuses it with a document separator.
    let input = input.strip_prefix('\u{feff}').unwrap_or(input);

    // Trim trailing whitespace.
    let input = input.trim_end();

    // If the first non-empty line has leading whitespace, trim leading
    // whitespace as well.
    if let Some(first_non_empty_line) = input.lines().find(|line| !line.trim().is_empty())
        && first_non_empty_line
            .chars()
            .next()
            .map(|c| c.is_whitespace())
            .unwrap_or(false)
    {
        input.trim_start()
    } else {
        input
    }
}

fn yaml_value_to_json(v: &dbt_yaml::Value) -> serde_json::Value {
    match v {
        dbt_yaml::Value::Null(_) => serde_json::Value::Null,
        dbt_yaml::Value::Bool(b, _) => serde_json::Value::Bool(*b),
        dbt_yaml::Value::Number(n, _) => {
            if let Some(i) = n.as_i64() {
                serde_json::Value::Number(i.into())
            } else if let Some(u) = n.as_u64() {
                serde_json::Value::Number(u.into())
            } else if let Some(f) = n.as_f64() {
                serde_json::json!(f)
            } else {
                serde_json::Value::String(n.to_string())
            }
        }
        dbt_yaml::Value::String(s, _) => serde_json::Value::String(s.clone()),
        dbt_yaml::Value::Sequence(seq, _) => {
            serde_json::Value::Array(seq.iter().map(yaml_value_to_json).collect())
        }
        dbt_yaml::Value::Mapping(map, _) => {
            let obj: serde_json::Map<String, serde_json::Value> = map
                .iter()
                .filter_map(|(k, v)| Some((k.as_str()?.to_owned(), yaml_value_to_json(v))))
                .collect();
            serde_json::Value::Object(obj)
        }
        dbt_yaml::Value::Tagged(tagged, _) => yaml_value_to_json(&tagged.value),
        dbt_yaml::Value::Timestamp(timestamp, _) => {
            serde_json::Value::String(timestamp.to_string())
        }
    }
}

#[cfg(test)]
mod scoped_env_tests {
    use super::scoped_env_value;
    use std::collections::BTreeMap;

    #[test]
    fn windows_scoped_environment_uses_case_insensitive_keys() {
        let overrides = BTreeMap::from([("dbt_target".into(), "prod".into())]);
        assert_eq!(
            scoped_env_value(&overrides, "DBT_TARGET", true).map(String::as_str),
            Some("prod")
        );
    }

    #[test]
    fn windows_scoped_environment_last_equivalent_key_wins() {
        let overrides = BTreeMap::from([
            ("DBT_TARGET".into(), "dev".into()),
            ("dbt_target".into(), "prod".into()),
        ]);
        for name in ["DBT_TARGET", "dbt_target", "Dbt_Target"] {
            assert_eq!(
                scoped_env_value(&overrides, name, true).map(String::as_str),
                Some("prod")
            );
        }
    }

    #[test]
    fn unix_scoped_environment_preserves_case_and_empty_values() {
        let overrides = BTreeMap::from([
            ("DBT_TARGET".into(), "".into()),
            ("dbt_target".into(), "prod".into()),
        ]);
        assert_eq!(
            scoped_env_value(&overrides, "DBT_TARGET", false).map(String::as_str),
            Some("")
        );
        assert_eq!(
            scoped_env_value(&overrides, "dbt_target", false).map(String::as_str),
            Some("prod")
        );
        assert_eq!(scoped_env_value(&overrides, "Dbt_Target", false), None);
    }
}
