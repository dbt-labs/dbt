//! Parsing and validation for a target's connection list in `profiles.yml`.
//!
//! A target's `outputs.<target>` entry accepts two shapes:
//!
//! ```yaml
//! outputs:
//!   dev:                          # legacy: the mapping is exactly one connection
//!     type: duckdb
//!     path: ./dev.db
//!
//!   prod:                         # a flat list declares several
//!     - type: snowflake
//!       default: true             # the project default, so snowflake is the
//!       account: abc123           # target's default adapter
//!     - type: bigquery
//!       method: service-account
//! ```
//!
//! A legacy mapping also accepts a `lakecompute:` sibling key as shorthand for
//! the two-entry list pairing it with lake compute:
//!
//! ```yaml
//!   staging:
//!     type: snowflake
//!     account: abc123
//!     lakecompute:                # equivalent to a second list entry with
//!       method: token             # `type: lakecompute` and these fields
//!       token: "{{ env_var('DBT_COMPUTE_AUTH_TOKEN') }}"
//! ```
//!
//! This exists only because dbt platform generates `profiles.yml` from a web
//! form that can produce a legacy mapping but not the list shape, while its
//! "extended attributes" mechanism can still append a sibling key to that
//! mapping -- so this is the one shape such a generated profile can express a
//! second, lake-compute connection in. See
//! [`split_legacy_lake_compute_extension`] and dbt-labs/fs#14234. It is a
//! transitional accommodation for that gap, not a general-purpose feature.
//!
//! All three resolve to the same thing: connections grouped by their `type:` into an
//! ordered list of [`AdapterConnections`], exactly one of which holds the default.
//! The legacy mapping yields a single adapter with a single connection, which is
//! the default -- so callers never branch on the shape.
//!
//! The list is flat rather than keyed by adapter type because `type:` already
//! identifies a connection's adapter, and it is the key `DbConfig` is tagged by.
//! Grouping is this module's job, not the author's: two connections of the same
//! type are legal and land under one adapter, where the consumer warns that only
//! the first is reachable.

use crate::error::{ProfileError, Result};
use crate::resolve::{ProfileEnvironment, render_target};

/// The key naming an adapter within a target's list.
const NAME_KEY: &str = "name";
/// The key marking an adapter as the target's default.
const DEFAULT_KEY: &str = "default";

/// The name an unnamed connection takes. `name:` exists only to tell several
/// connections apart, so the common case of one connection never writes it.
pub const DEFAULT_CONNECTION_NAME: &str = "default";

/// The key carrying a connection's adapter type. Required in the list shape --
/// it is what identifies the adapter -- and already required by the legacy shape,
/// since `DbConfig` is tagged by it.
const TYPE_KEY: &str = "type";

/// The lake compute adapter, as authors write it.
///
/// This is also `DbConfig`'s tag for the variant: `DbConfig` is
/// `#[serde(tag = "type", rename_all = "lowercase")]`, so each variant's tag is
/// its identifier lowercased -- `LakeCompute` becomes `lakecompute`. Because the
/// external name and the tag are the same string, lake compute needs no
/// special-casing here, unlike before this adapter's external name was renamed
/// from `lake_compute` to `lakecompute` to match.
const LAKE_COMPUTE_TYPE: &str = "lakecompute";

/// Lake compute's retired external names. Not accepted aliases.
///
/// `alt` was the name before the `lake_compute` rename; `lake_compute` was the
/// name before *this* rename. Both would otherwise deserialize successfully as
/// `DbConfig::LakeCompute` if passed straight through, so both are rejected
/// here rather than aliased, so they cannot keep working as undocumented
/// spellings.
const RETIRED_LAKE_COMPUTE_TYPES: &[&str] = &["alt", "lake_compute"];

/// Canonicalize a connection's `type:` and rewrite it in place, rejecting
/// retired lake-compute spellings and folding lake compute's case to its
/// canonical lowercase.
///
/// Every adapter's external name and `DbConfig` tag are the same string now,
/// so this only does real work for lake compute: `DbConfig`'s tag match is
/// exact-case, so `LAKECOMPUTE`/`LakeCompute` etc. must still be folded to
/// `lakecompute` -- both in the returned name and in `credentials[TYPE_KEY]`,
/// which is what `DbConfig` actually deserializes -- same as before this
/// adapter's external name was renamed to match the tag. Every other
/// adapter's rewrite is a no-op: it is handed back exactly what it was given.
fn canonicalize_adapter_type(
    credentials: &mut dbt_yaml::Mapping,
    adapter_type: &str,
) -> Result<String> {
    if RETIRED_LAKE_COMPUTE_TYPES
        .iter()
        .any(|s| adapter_type.eq_ignore_ascii_case(s))
    {
        return Err(ProfileError::UnacceptedAdapterType {
            written: adapter_type.to_owned(),
            expected: LAKE_COMPUTE_TYPE.to_owned(),
        });
    }
    let canonical = if adapter_type.eq_ignore_ascii_case(LAKE_COMPUTE_TYPE) {
        LAKE_COMPUTE_TYPE
    } else {
        adapter_type
    };
    credentials.insert(
        dbt_yaml::Value::from(TYPE_KEY),
        dbt_yaml::Value::from(canonical),
    );
    Ok(canonical.to_owned())
}

/// One connection under an adapter: a rendered credential set.
#[derive(Debug, Clone, PartialEq)]
pub struct TargetConnection {
    /// As written, or [`DEFAULT_CONNECTION_NAME`] when omitted. Entirely optional:
    /// nothing consumes it yet, since only one connection per adapter is
    /// reachable, so it exists to reserve the syntax rather than to select
    /// anything.
    pub name: String,
    /// Whether [`Self::name`] was written rather than defaulted. Only explicit
    /// names are checked for collisions.
    pub named: bool,
    /// Whether this connection is the project default. The adapter holding it is
    /// the target's default adapter.
    pub is_default: bool,
    /// The rendered connection config, carrying `type:` taken from the adapter it
    /// was declared under. This is exactly what `DbConfig` deserialization expects.
    pub credentials: dbt_yaml::Mapping,
}

/// One adapter a target declares, with its connections.
#[derive(Debug, Clone, PartialEq)]
pub struct AdapterConnections {
    /// The adapter type, as written in each connection's `type:`. Left a string
    /// deliberately:
    /// this crate has no `AdapterType`, and validating the name against the known
    /// set belongs to the consumer, which resolves it into a `DbConfig` anyway.
    pub adapter_type: String,
    /// Declaration order preserved. Never empty.
    pub connections: Vec<TargetConnection>,
    /// Index into [`Self::connections`] of the one that is actually used.
    ///
    /// The connection marked `default: true` if this adapter holds it, otherwise
    /// the first. Several connections may be declared but only this one is
    /// reachable; a consumer should warn when `connections.len() > 1`, which this
    /// crate cannot do itself as it has no logging.
    pub default_connection: usize,
}

impl AdapterConnections {
    /// The connection that is actually used.
    pub fn default_connection(&self) -> &TargetConnection {
        &self.connections[self.default_connection]
    }

    /// Whether this adapter declares connections that cannot be reached yet.
    pub fn has_unreachable_connections(&self) -> bool {
        self.connections.len() > 1
    }
}

/// Parse and validate a target's `outputs.<target>` into its adapters and their
/// connections.
///
/// Two shapes, told apart by the YAML node kind:
///
/// ```yaml
/// dev:                          # a mapping is the legacy single connection
///   type: duckdb
///   path: ./dev.db
///
/// prod:                         # a sequence is a flat connection list
///   - type: snowflake
///     default: true
///     account: abc123
///   - type: bigquery
///     method: service-account
/// ```
///
/// Sequence-versus-mapping is the discriminator, which is why the list is flat:
/// under the previous adapter-type-keyed shape the two forms were both mappings
/// and had to be told apart by probing for `type:`, and that probe was fragile --
/// `DuckDbConfig::attach` is itself a sequence of mappings with optional `type:`
/// fields. A node kind cannot be ambiguous.
///
/// Connections are grouped by `type:` in order of first appearance. Several of one
/// type is legal: they land under one adapter, and the consumer warns that only
/// the first is reachable.
///
/// YAML anchors and merge keys are already resolved when this runs: `resolve`
/// calls `apply_merge` on the whole document before extracting a target, so a
/// connection written as `- <<: *creds` arrives here as a plain mapping. That is
/// what lets a profile reuse one credential block across several targets.
pub fn parse_target_connections(
    profile: &str,
    target: &str,
    raw: &dbt_yaml::Value,
    penv: &ProfileEnvironment,
) -> Result<Vec<AdapterConnections>> {
    parse_connections(profile, target, raw, penv, ResolutionMode::Credentials)
}

#[derive(Clone, Copy)]
pub(crate) enum ResolutionMode {
    Credentials,
    Metadata,
}

impl ResolutionMode {
    fn render(self, raw: &dbt_yaml::Value, penv: &ProfileEnvironment) -> Result<dbt_yaml::Mapping> {
        match (self, raw.as_mapping()) {
            (Self::Metadata, Some(mapping)) => {
                let selection = mapping
                    .iter()
                    .filter(|(key, _)| {
                        matches!(key.as_str(), Some(TYPE_KEY | NAME_KEY | DEFAULT_KEY))
                    })
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect::<dbt_yaml::Mapping>();
                render_target(&dbt_yaml::Value::from(selection), penv)
            }
            _ => render_target(raw, penv),
        }
    }
}

pub(crate) fn parse_connections(
    profile: &str,
    target: &str,
    raw: &dbt_yaml::Value,
    penv: &ProfileEnvironment,
    mode: ResolutionMode,
) -> Result<Vec<AdapterConnections>> {
    let raw_connections: Vec<dbt_yaml::Value> = match raw {
        dbt_yaml::Value::Sequence(entries, _) => entries.clone(),
        dbt_yaml::Value::Mapping(mapping, _) => {
            match split_legacy_lake_compute_extension(mapping) {
                // dbt platform's extended-attributes hack: treat it as the
                // two-entry list it stands in for.
                Some((primary, lake_compute)) => vec![primary, lake_compute],
                // The ordinary legacy shape: the whole block is one connection.
                None => return parse_legacy_target(raw, penv, mode),
            }
        }
        _ => {
            return Err(ProfileError::TargetNotConnectionList {
                profile: profile.to_owned(),
                target: target.to_owned(),
            });
        }
    };

    if raw_connections.is_empty() {
        return Err(ProfileError::EmptyConnectionList {
            profile: profile.to_owned(),
            target: target.to_owned(),
        });
    }

    let mut adapters: Vec<AdapterConnections> = Vec::new();
    for (index, entry) in raw_connections.iter().enumerate() {
        let (adapter_type, connection) =
            parse_connection(profile, target, index, entry, penv, mode)?;

        match adapters.iter_mut().find(|a| a.adapter_type == adapter_type) {
            Some(adapter) => {
                if let Some(name) = explicit_duplicate_name(adapter, &connection) {
                    return Err(ProfileError::DuplicateConnectionName {
                        profile: profile.to_owned(),
                        target: target.to_owned(),
                        adapter: adapter_type,
                        connection: name,
                    });
                }
                adapter.connections.push(connection);
            }
            None => adapters.push(AdapterConnections {
                adapter_type,
                connections: vec![connection],
                // Provisional; `resolve_default_connection` sets it once the whole
                // target is known, since the marker is target-wide.
                default_connection: 0,
            }),
        }
    }

    resolve_default_connection(profile, target, &mut adapters)?;
    Ok(adapters)
}

/// The name `connection` collides with, considering only names written explicitly.
///
/// `name:` is optional, so several unnamed connections of one type all carry
/// [`DEFAULT_CONNECTION_NAME`] and must not collide with each other -- that is the
/// ordinary case the consumer warns about, not an error. Two connections the author
/// *named* the same thing is a mistake worth reporting.
fn explicit_duplicate_name(
    adapter: &AdapterConnections,
    connection: &TargetConnection,
) -> Option<String> {
    if !connection.named {
        return None;
    }
    adapter
        .connections
        .iter()
        .find(|existing| existing.named && existing.name == connection.name)
        .map(|existing| existing.name.clone())
}

/// Parse one entry of a target's connection list.
///
/// Returns the adapter type it declares alongside the connection, since grouping
/// is the caller's job.
fn parse_connection(
    profile: &str,
    target: &str,
    index: usize,
    entry: &dbt_yaml::Value,
    penv: &ProfileEnvironment,
    mode: ResolutionMode,
) -> Result<(String, TargetConnection)> {
    if !matches!(entry, dbt_yaml::Value::Mapping(_, _)) {
        return Err(ProfileError::ConnectionNotMapping {
            profile: profile.to_owned(),
            target: target.to_owned(),
            index,
        });
    }

    // Render first so `name`, `default` and `type` may all be templated, then
    // split the two list-only keys out of what becomes the connection config.
    // `type:` stays in: it is what `DbConfig` is tagged by.
    let mut credentials = mode.render(entry, penv)?;

    let adapter_type = credentials
        .get(TYPE_KEY)
        .and_then(|v| v.as_str())
        .map(str::to_owned)
        .filter(|t| !t.trim().is_empty())
        .ok_or_else(|| ProfileError::ConnectionMissingType {
            profile: profile.to_owned(),
            target: target.to_owned(),
            index,
        })?;
    let adapter_type = canonicalize_adapter_type(&mut credentials, &adapter_type)?;

    let (name, named) = match credentials.shift_remove(NAME_KEY) {
        Some(dbt_yaml::Value::String(name, _)) if !name.trim().is_empty() => (name, true),
        None => (DEFAULT_CONNECTION_NAME.to_owned(), false),
        // Present but not a usable string -- a typo worth reporting rather than
        // silently treating as unnamed.
        _ => {
            return Err(ProfileError::ConnectionNameNotString {
                profile: profile.to_owned(),
                target: target.to_owned(),
                index,
            });
        }
    };

    let is_default = match credentials.shift_remove(DEFAULT_KEY) {
        None => false,
        Some(value) => {
            parse_default_flag(&value).ok_or_else(|| ProfileError::ConnectionDefaultNotBool {
                profile: profile.to_owned(),
                target: target.to_owned(),
                adapter: adapter_type.clone(),
                connection: name.clone(),
            })?
        }
    };

    Ok((
        adapter_type,
        TargetConnection {
            name,
            named,
            is_default,
            credentials,
        },
    ))
}

/// The legacy shape: the target block *is* one connection, named
/// [`DEFAULT_CONNECTION_NAME`], under the adapter its `type:` names.
fn parse_legacy_target(
    raw: &dbt_yaml::Value,
    penv: &ProfileEnvironment,
    mode: ResolutionMode,
) -> Result<Vec<AdapterConnections>> {
    let mut credentials = mode.render(raw, penv)?;
    let adapter_type = credentials
        .get(TYPE_KEY)
        .and_then(|v| v.as_str())
        .ok_or(ProfileError::NoAdapterType)?
        .to_owned();
    let adapter_type = canonicalize_adapter_type(&mut credentials, &adapter_type)?;

    Ok(vec![AdapterConnections {
        adapter_type,
        connections: vec![TargetConnection {
            name: DEFAULT_CONNECTION_NAME.to_owned(),
            named: false,
            is_default: true,
            credentials,
        }],
        default_connection: 0,
    }])
}

/// Split dbt platform's "extended attributes" lake-compute hack out of an
/// otherwise-legacy target mapping, into the equivalent two-entry list shape.
///
/// dbt platform generates `profiles.yml` from a web form that only supports one
/// warehouse connection per target, so it cannot produce the list shape a
/// Snowflake + lake compute pairing needs. It does support "extended
/// attributes" -- arbitrary extra keys appended to a generated target -- so a
/// `lakecompute:` sibling key smuggles the second connection in that way
/// instead. This turns that sibling into the `[primary, lake_compute]` list the
/// rest of this module already knows how to parse, so nothing downstream has
/// to know the shape came from the hack rather than the real list syntax.
///
/// This is a transitional accommodation for dbt platform's UI gap, not a
/// general-purpose feature -- it applies to `lakecompute` specifically because
/// that is what platform customers are currently unable to configure any other
/// way. See dbt-labs/fs#14234.
///
/// Returns `None` -- deferring to the ordinary legacy path -- when there is no
/// `lakecompute:` mapping sibling to split out, or when the target's own
/// `type:` is already lake compute (appending lake compute to itself is
/// nonsensical, so it is left for the ordinary legacy path to accept or reject
/// as-is rather than special-cased here).
fn split_legacy_lake_compute_extension(
    mapping: &dbt_yaml::Mapping,
) -> Option<(dbt_yaml::Value, dbt_yaml::Value)> {
    let lake_compute_block = mapping.get(LAKE_COMPUTE_TYPE)?;
    if !matches!(lake_compute_block, dbt_yaml::Value::Mapping(_, _)) {
        // Not this hack's shape -- e.g. some other adapter's config
        // legitimately has an unrelated field named `lakecompute`.
        return None;
    }
    let already_lake_compute = mapping
        .get(TYPE_KEY)
        .and_then(|v| v.as_str())
        .is_some_and(|t| t.eq_ignore_ascii_case(LAKE_COMPUTE_TYPE));
    if already_lake_compute {
        return None;
    }

    let mut primary = mapping.clone();
    let lake_compute_block = primary
        .shift_remove(LAKE_COMPUTE_TYPE)
        .expect("checked present above");
    // The legacy shape's one connection was always the implicit default; make
    // that explicit now that it is joining a list, which requires the marker
    // to resolve one unambiguously.
    primary.insert(
        dbt_yaml::Value::from(DEFAULT_KEY),
        dbt_yaml::Value::from(true),
    );

    let mut lake_compute = match lake_compute_block {
        dbt_yaml::Value::Mapping(m, _) => m,
        _ => unreachable!("checked present and a mapping above"),
    };
    lake_compute.insert(
        dbt_yaml::Value::from(TYPE_KEY),
        dbt_yaml::Value::from(LAKE_COMPUTE_TYPE),
    );

    Some((
        dbt_yaml::Value::from(primary),
        dbt_yaml::Value::from(lake_compute),
    ))
}

/// Resolve the target-wide `default: true` marker into a default connection per
/// adapter, erroring when the target is ambiguous.
///
/// The marker does double duty: the connection it sits on is the project default,
/// and the adapter holding it is the target's default adapter. A target declaring
/// one adapter may omit it — there is only one answer — but anything wider must
/// say, because which adapter every unannotated node runs on should not depend on
/// YAML ordering.
///
/// Within an adapter that does not hold the marker, the first connection is used.
fn resolve_default_connection(
    profile: &str,
    target: &str,
    adapters: &mut [AdapterConnections],
) -> Result<()> {
    let marked: Vec<String> = adapters
        .iter()
        .flat_map(|a| {
            a.connections
                .iter()
                .filter(|c| c.is_default)
                .map(move |c| format!("{}.{}", a.adapter_type, c.name))
        })
        .collect();

    match marked.len() {
        1 => {}
        0 if adapters.len() == 1 => {
            adapters[0].connections[0].is_default = true;
        }
        0 => {
            return Err(ProfileError::NoDefaultConnection {
                profile: profile.to_owned(),
                target: target.to_owned(),
                adapters: adapters.iter().map(|a| a.adapter_type.clone()).collect(),
            });
        }
        _ => {
            return Err(ProfileError::MultipleDefaultConnections {
                profile: profile.to_owned(),
                target: target.to_owned(),
                connections: marked,
            });
        }
    }

    for adapter in adapters.iter_mut() {
        adapter.default_connection = adapter
            .connections
            .iter()
            .position(|c| c.is_default)
            .unwrap_or(0);
    }
    Ok(())
}

/// Accept `default: true` and the string form Jinja rendering produces
/// (`default: "{{ ... }}"` renders to a string, not a bool).
fn parse_default_flag(value: &dbt_yaml::Value) -> Option<bool> {
    match value {
        dbt_yaml::Value::Bool(flag, _) => Some(*flag),
        dbt_yaml::Value::String(raw, _) => match raw.trim().to_ascii_lowercase().as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod connection_tests {
    use super::*;

    fn penv() -> ProfileEnvironment {
        ProfileEnvironment::new(Default::default())
    }

    fn parse(yaml: &str) -> Result<Vec<AdapterConnections>> {
        let raw: dbt_yaml::Value = dbt_yaml::from_str(yaml).expect("valid yaml fixture");
        parse_target_connections("my_profile", "prod", &raw, &penv())
    }

    fn types(adapters: &[AdapterConnections]) -> Vec<&str> {
        adapters.iter().map(|a| a.adapter_type.as_str()).collect()
    }

    /// The default is identified target-wide, so it names both the adapter and the
    /// connection.
    fn default_of(adapters: &[AdapterConnections]) -> (&str, &str) {
        let adapter = adapters
            .iter()
            .find(|a| a.connections.iter().any(|c| c.is_default))
            .expect("exactly one default connection");
        (
            adapter.adapter_type.as_str(),
            adapter.default_connection().name.as_str(),
        )
    }

    // -- shapes ------------------------------------------------------------

    /// Every project that exists today uses this shape, so it is the whole
    /// compatibility surface for the feature.
    #[test]
    fn legacy_mapping_is_one_adapter_with_one_default_connection() {
        let adapters = parse("type: duckdb\npath: ./dev.db\n").expect("legacy shape should parse");

        assert_eq!(types(&adapters), vec!["duckdb"]);
        assert_eq!(adapters[0].connections.len(), 1);
        assert_eq!(default_of(&adapters), ("duckdb", DEFAULT_CONNECTION_NAME));
    }

    /// A flat list: one adapter per distinct `type:`, in order of first appearance.
    #[test]
    fn a_connection_list_yields_one_adapter_per_type_in_declaration_order() {
        let adapters = parse(
            "- type: snowflake\n  default: true\n  account: abc\n\
             - type: bigquery\n  method: service-account\n\
             - type: lakecompute\n  base_url: https://example.invalid\n",
        )
        .expect("list shape should parse");

        assert_eq!(
            types(&adapters),
            vec!["snowflake", "bigquery", "lakecompute"]
        );
        assert!(adapters.iter().all(|a| a.connections.len() == 1));
        assert_eq!(
            default_of(&adapters),
            ("snowflake", DEFAULT_CONNECTION_NAME)
        );
    }

    /// Sequence-versus-mapping is the discriminator, so a legacy block whose own
    /// config contains a sequence is still read as legacy. `attach` is the real
    /// case: a list of mappings that may each carry `type:`, which is exactly what
    /// made probing for `type:` fragile under the old keyed shape.
    #[test]
    fn a_legacy_block_containing_a_sequence_is_still_legacy() {
        let adapters = parse(
            "type: duckdb\npath: ./dev.db\nattach:\n  - path: other.db\n    type: ducklake\n",
        )
        .expect("legacy shape should parse");

        assert_eq!(types(&adapters), vec!["duckdb"]);
        assert!(
            adapters[0]
                .default_connection()
                .credentials
                .get("attach")
                .is_some(),
            "the adapter's own config must survive untouched"
        );
    }

    #[test]
    fn a_target_that_is_neither_mapping_nor_list_is_rejected() {
        let err = parse("just-a-string\n").expect_err("a scalar target is not a shape");
        assert!(matches!(err, ProfileError::TargetNotConnectionList { .. }));
    }

    #[test]
    fn an_empty_connection_list_is_rejected() {
        let err = parse("[]\n").expect_err("an empty list declares nothing");
        assert!(matches!(err, ProfileError::EmptyConnectionList { .. }));
    }

    #[test]
    fn a_non_mapping_connection_is_rejected() {
        let err = parse("- type: duckdb\n- nope\n").expect_err("a scalar is not a connection");
        assert!(matches!(err, ProfileError::ConnectionNotMapping { .. }));
    }

    // -- type ---------------------------------------------------------------

    /// `type:` identifies the adapter now that there is no key to take it from, so
    /// a connection without one cannot be placed.
    #[test]
    fn a_connection_without_a_type_is_rejected() {
        let err = parse("- account: abc\n").expect_err("no type means no adapter");
        assert!(matches!(err, ProfileError::ConnectionMissingType { .. }));
    }

    #[test]
    fn type_survives_into_the_credentials() {
        let adapters = parse("- type: duckdb\n  path: ./dev.db\n").expect("should parse");

        assert_eq!(
            adapters[0]
                .default_connection()
                .credentials
                .get("type")
                .and_then(|v| v.as_str()),
            Some("duckdb"),
            "`DbConfig` is tagged by `type:`, so it must stay in the config"
        );
    }

    /// Lake compute's external name is case-insensitive like every other
    /// adapter's, and (unlike before this adapter's external name was renamed
    /// to `lakecompute`) is identical to `DbConfig`'s tag, so it needs no
    /// special handling to report correctly or to reach `DbConfig` correctly.
    #[test]
    fn lake_compute_is_case_insensitive() {
        for written in ["lakecompute", "LAKECOMPUTE"] {
            let adapters = parse(&format!(
                "- type: {written}
  base_url: https://example.invalid
"
            ))
            .unwrap_or_else(|e| panic!("`type: {written}` should parse: {e}"));

            assert_eq!(
                types(&adapters),
                vec!["lakecompute"],
                "`type: {written}` must report as the external name"
            );
            assert_eq!(
                adapters[0]
                    .default_connection()
                    .credentials
                    .get("type")
                    .and_then(|v| v.as_str()),
                Some("lakecompute"),
                "`type: {written}` must be handed to `DbConfig` as its tag"
            );
        }
    }

    /// `lakecompute` is the only spelling authors may write. `lake_compute` and
    /// `alt` are its retired names; both deserialize if passed through, so both
    /// must be rejected, in both target shapes, or they keep working as
    /// undocumented aliases.
    #[test]
    fn the_retired_lake_compute_spellings_are_rejected() {
        for written in RETIRED_LAKE_COMPUTE_TYPES {
            for yaml in [
                format!("- type: {written}\n  base_url: https://example.invalid\n"),
                format!("type: {written}\nbase_url: https://example.invalid\n"),
            ] {
                let Err(err) = parse(&yaml) else {
                    panic!("`type: {written}` must be rejected, but it parsed");
                };
                assert!(
                    matches!(
                        &err,
                        ProfileError::UnacceptedAdapterType { expected, .. }
                            if expected == "lakecompute"
                    ),
                    "expected an error naming the accepted spelling, got: {err}"
                );
            }
        }
    }

    // -- dbt platform's extended-attributes lake_compute hack ---------------

    /// dbt platform can only generate one connection per target, so a
    /// `lakecompute:` sibling key on an otherwise-legacy mapping is the only
    /// way its "extended attributes" mechanism can add a second one. This must
    /// parse exactly like the equivalent two-entry list.
    #[test]
    fn a_legacy_mapping_with_a_lake_compute_sibling_splits_into_two_adapters() {
        let adapters = parse(
            "type: snowflake
account: abc123
lakecompute:
  method: token
  token: test-token
  threads: 4
",
        )
        .expect("the sibling-key shape should parse");

        assert_eq!(types(&adapters), vec!["snowflake", "lakecompute"]);
        assert_eq!(
            default_of(&adapters),
            ("snowflake", DEFAULT_CONNECTION_NAME)
        );

        let lake_compute = &adapters[1];
        assert_eq!(lake_compute.connections.len(), 1);
        assert_eq!(
            lake_compute
                .default_connection()
                .credentials
                .get("type")
                .and_then(|v| v.as_str()),
            Some("lakecompute"),
            "the split-out connection must still be canonicalized"
        );
        assert_eq!(
            lake_compute
                .default_connection()
                .credentials
                .get("threads")
                .and_then(|v| v.as_i64()),
            Some(4),
            "the sibling block's own fields must survive the split"
        );
    }

    /// A `lakecompute:` field that isn't a mapping (e.g. some unrelated
    /// adapter's scalar config field that happens to share the name) is not
    /// this hack's shape, so it must fall through to the ordinary legacy path
    /// untouched rather than erroring.
    #[test]
    fn a_non_mapping_lake_compute_field_is_not_treated_as_the_extension() {
        let adapters = parse("type: duckdb\npath: ./dev.db\nlakecompute: true\n")
            .expect("should still parse as ordinary legacy shape");

        assert_eq!(types(&adapters), vec!["duckdb"]);
    }

    /// Appending the hack's sibling key to a connection whose own `type:` is
    /// already `lakecompute` is nonsensical; it must not be split, leaving it
    /// for the ordinary legacy path to handle as a single connection.
    #[test]
    fn a_lake_compute_target_with_a_lake_compute_sibling_is_not_split() {
        let adapters = parse(
            "type: lakecompute
base_url: https://example.invalid
lakecompute:
  threads: 4
",
        )
        .expect("should still parse as ordinary legacy shape");

        assert_eq!(types(&adapters), vec!["lakecompute"]);
    }

    /// Two connections of one type are legal and land under one adapter -- the
    /// consumer warns that only the first is reachable, which is why
    /// `has_unreachable_connections` exists.
    #[test]
    fn two_connections_of_one_type_group_under_one_adapter() {
        let adapters = parse(
            "- type: snowflake\n  default: true\n  account: first\n\
             - type: snowflake\n  account: second\n",
        )
        .expect("several connections of one type are legal");

        assert_eq!(types(&adapters), vec!["snowflake"]);
        assert_eq!(adapters[0].connections.len(), 2);
        assert!(adapters[0].has_unreachable_connections());
        assert_eq!(
            adapters[0]
                .default_connection()
                .credentials
                .get("account")
                .and_then(|v| v.as_str()),
            Some("first"),
            "the marked connection is the reachable one"
        );
    }

    // -- name ---------------------------------------------------------------

    /// `name:` is entirely optional, including when several connections share a
    /// type -- there is nothing to disambiguate for, since nothing consumes it.
    #[test]
    fn several_unnamed_connections_of_one_type_are_accepted() {
        let adapters = parse(
            "- type: snowflake\n  default: true\n  account: first\n\
             - type: snowflake\n  account: second\n",
        )
        .expect("unnamed connections must not collide with each other");

        assert!(adapters[0].connections.iter().all(|c| !c.named));
        assert!(
            adapters[0]
                .connections
                .iter()
                .all(|c| c.name == DEFAULT_CONNECTION_NAME)
        );
    }

    #[test]
    fn an_explicit_name_is_kept() {
        let adapters =
            parse("- type: duckdb\n  name: local\n  path: ./dev.db\n").expect("should parse");

        let connection = adapters[0].default_connection();
        assert_eq!(connection.name, "local");
        assert!(connection.named);
        assert!(
            connection.credentials.get("name").is_none(),
            "`name` is a list-only key and must not reach the credentials"
        );
    }

    /// Only names written explicitly are checked, so this catches a real mistake
    /// without rejecting the ordinary unnamed case above.
    #[test]
    fn duplicate_explicit_names_within_one_adapter_are_rejected() {
        let err = parse(
            "- type: snowflake\n  name: warehouse\n  default: true\n\
             - type: snowflake\n  name: warehouse\n",
        )
        .expect_err("two connections named the same is a mistake");
        assert!(matches!(err, ProfileError::DuplicateConnectionName { .. }));
    }

    /// The same name under *different* adapters is fine: names are scoped to the
    /// adapter they group under.
    #[test]
    fn the_same_name_under_different_adapters_is_accepted() {
        let adapters = parse(
            "- type: snowflake\n  name: main\n  default: true\n\
             - type: bigquery\n  name: main\n",
        )
        .expect("names are per-adapter");

        assert_eq!(types(&adapters), vec!["snowflake", "bigquery"]);
    }

    #[test]
    fn a_non_string_name_is_rejected() {
        let err = parse("- type: duckdb\n  name: []\n").expect_err("a list is not a name");
        assert!(matches!(err, ProfileError::ConnectionNameNotString { .. }));
    }

    // -- the default marker --------------------------------------------------

    #[test]
    fn a_single_adapter_may_omit_the_default_marker() {
        let adapters = parse("- type: duckdb\n  path: ./dev.db\n").expect("should parse");
        assert_eq!(default_of(&adapters), ("duckdb", DEFAULT_CONNECTION_NAME));
    }

    /// Which adapter every unannotated node runs on must not depend on YAML
    /// ordering, so a target declaring several has to say.
    #[test]
    fn several_adapters_must_mark_a_default() {
        let err = parse("- type: snowflake\n- type: bigquery\n")
            .expect_err("ambiguous default must be rejected");
        assert!(matches!(err, ProfileError::NoDefaultConnection { .. }));
    }

    #[test]
    fn two_default_markers_are_rejected() {
        let err = parse("- type: snowflake\n  default: true\n- type: bigquery\n  default: true\n")
            .expect_err("two defaults is ambiguous");
        assert!(matches!(
            err,
            ProfileError::MultipleDefaultConnections { .. }
        ));
    }

    /// The marker does double duty: it picks the connection *and* the adapter.
    #[test]
    fn the_marker_selects_the_default_adapter_not_just_the_connection() {
        let adapters = parse(
            "- type: snowflake\n  account: abc\n\
             - type: bigquery\n  default: true\n  method: service-account\n",
        )
        .expect("should parse");

        assert_eq!(
            default_of(&adapters).0,
            "bigquery",
            "the marker's adapter is the default even when declared second"
        );
    }

    #[test]
    fn an_unmarked_adapter_uses_its_first_connection() {
        let adapters = parse(
            "- type: snowflake\n  default: true\n  account: abc\n\
             - type: bigquery\n  method: first\n\
             - type: bigquery\n  method: second\n",
        )
        .expect("should parse");

        let bigquery = adapters
            .iter()
            .find(|a| a.adapter_type == "bigquery")
            .expect("bigquery declared");
        assert_eq!(
            bigquery
                .default_connection()
                .credentials
                .get("method")
                .and_then(|v| v.as_str()),
            Some("first")
        );
    }

    #[test]
    fn a_non_boolean_default_is_rejected() {
        let err = parse("- type: duckdb\n  default: nope\n")
            .expect_err("a non-boolean default is a mistake");
        assert!(matches!(err, ProfileError::ConnectionDefaultNotBool { .. }));
    }
}
