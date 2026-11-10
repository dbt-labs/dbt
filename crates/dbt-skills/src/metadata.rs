//! dbt's bookkeeping inside the installed `SKILL.md`'s `metadata` frontmatter.
//!
//! The AgentSkills spec allows exactly six frontmatter fields and reserves
//! `metadata` for client-specific data; Claude Code does not act on its
//! contents. So dbt records what it installed there rather than in a sidecar
//! file, keeping the skill directory to exactly the files the package shipped.
//!
//! This record is informational — it lets dbt recognize its own installs so
//! `dbt clean` and re-runs never touch a skill the user wrote — and is always
//! written. Only the installed copy is ever modified; a package's own `SKILL.md`
//! is read and never changed.

use dbt_common::{ErrorCode, FsResult, fs_err};
use indexmap::IndexMap;

use crate::validate::split_frontmatter_parts;

/// Marks a `metadata` map as dbt's. dbt ignores skills without it.
pub const MANAGED_BY_KEY: &str = "dbt.managed_by";
const MANAGED_BY_VALUE: &str = "dbt";
const SOURCE_KEY: &str = "dbt.source";
const PACKAGE_KEY: &str = "dbt.package";
const VERSION_KEY: &str = "dbt.version";
const SOURCE_PATH_KEY: &str = "dbt.source_path";
const SOURCE_HASH_KEY: &str = "dbt.source_hash";
const INSTALLED_AT_KEY: &str = "dbt.installed_at";

/// What dbt records about a skill it installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillMetadata {
    /// `package` or `project`.
    pub source: String,
    /// `None` when the skill came from the project itself.
    pub package: Option<String>,
    pub version: Option<String>,
    pub source_path: String,
    /// `sha256:…` over the source directory at install time.
    pub source_hash: String,
    pub installed_at: String,
}

impl SkillMetadata {
    fn to_pairs(&self) -> Vec<(&'static str, String)> {
        let mut pairs = vec![
            (MANAGED_BY_KEY, MANAGED_BY_VALUE.to_string()),
            (SOURCE_KEY, self.source.clone()),
        ];
        if let Some(package) = &self.package {
            pairs.push((PACKAGE_KEY, package.clone()));
        }
        if let Some(version) = &self.version {
            pairs.push((VERSION_KEY, version.clone()));
        }
        pairs.push((SOURCE_PATH_KEY, self.source_path.clone()));
        pairs.push((SOURCE_HASH_KEY, self.source_hash.clone()));
        pairs.push((INSTALLED_AT_KEY, self.installed_at.clone()));
        pairs
    }
}

/// Write dbt's keys into `skill_md`'s `metadata` map, returning the new file.
///
/// Only the `metadata:` block is rewritten: everything outside it keeps its
/// exact bytes, so author comments and key order elsewhere survive. Only
/// `dbt.*` keys belong to dbt; an author's own `metadata` entries are carried
/// across losslessly, structured values (nested maps or sequences) included,
/// by re-serializing the merged map through the YAML writer rather than
/// flattening it to scalars.
pub fn inject(skill_md: &str, meta: &SkillMetadata) -> FsResult<String> {
    let (head, frontmatter, tail) = split_frontmatter_parts(skill_md).ok_or_else(|| {
        fs_err!(
            ErrorCode::InvalidSkill,
            "Cannot record dbt metadata: SKILL.md is missing '---' delimited frontmatter"
        )
    })?;

    if has_unsupported_metadata_shape(frontmatter) {
        return Err(fs_err!(
            ErrorCode::InvalidSkill,
            "Cannot record dbt metadata: SKILL.md declares 'metadata' in a form dbt cannot \
             rewrite safely (a multi-line flow mapping). Use a block mapping instead."
        ));
    }

    // Author entries first (dropping any stale `dbt.*` keys from a prior
    // install), then dbt's own keys. Both keep their YAML `Value`, so a
    // structured author value survives untouched.
    let mut merged = dbt_yaml::Mapping::new();
    for (key, value) in existing_metadata(frontmatter)? {
        if !key.starts_with("dbt.") {
            merged.insert(dbt_yaml::Value::string(key), value);
        }
    }
    for (key, value) in meta.to_pairs() {
        merged.insert(
            dbt_yaml::Value::string(key.to_string()),
            dbt_yaml::Value::string(value),
        );
    }

    // Re-serialize through the YAML writer rather than hand-emitting scalars:
    // it quotes whatever needs quoting (a hash or timestamp's `:`, an author's
    // `"`/`\`), so the block always parses back to the same map — dbt reading
    // its own install correctly depends on that round-trip.
    let block = write_metadata_block(merged)?;
    let stripped = strip_metadata_block(frontmatter);
    Ok(format!("{head}{stripped}{block}{tail}"))
}

/// Serialize the merged map as a top-level `metadata:` block, ending in a
/// newline so it splices cleanly ahead of the frontmatter's closing `---`.
fn write_metadata_block(metadata: dbt_yaml::Mapping) -> FsResult<String> {
    let mut root = dbt_yaml::Mapping::new();
    root.insert(
        dbt_yaml::Value::string("metadata".to_string()),
        dbt_yaml::Value::mapping(metadata),
    );
    let mut block = dbt_yaml::to_string(&dbt_yaml::Value::mapping(root)).map_err(|e| {
        fs_err!(
            ErrorCode::InvalidSkill,
            "Cannot record dbt metadata: failed to serialize the metadata block: {}",
            e
        )
    })?;
    if !block.ends_with('\n') {
        block.push('\n');
    }
    Ok(block)
}

/// Read dbt's keys back, or `None` when this skill is not dbt's.
///
/// A skill with no `metadata`, no `dbt.managed_by`, or unparseable frontmatter
/// is not ours to touch, so all three collapse to `None` rather than an error.
pub fn read(skill_md: &str) -> Option<SkillMetadata> {
    let (_, frontmatter, _) = split_frontmatter_parts(skill_md)?;
    let map = existing_metadata(frontmatter).ok()?;
    let get = |key: &str| map.get(key).and_then(|v| v.as_str()).map(str::to_string);
    if get(MANAGED_BY_KEY).as_deref() != Some(MANAGED_BY_VALUE) {
        return None;
    }
    Some(SkillMetadata {
        source: get(SOURCE_KEY).unwrap_or_default(),
        package: get(PACKAGE_KEY),
        version: get(VERSION_KEY),
        source_path: get(SOURCE_PATH_KEY).unwrap_or_default(),
        source_hash: get(SOURCE_HASH_KEY).unwrap_or_default(),
        installed_at: get(INSTALLED_AT_KEY).unwrap_or_default(),
    })
}

/// The `metadata` map already in this frontmatter, keyed by string with each
/// value kept as its parsed YAML so structured author values survive.
fn existing_metadata(frontmatter: &str) -> FsResult<IndexMap<String, dbt_yaml::Value>> {
    let parsed: dbt_yaml::Value = dbt_yaml::from_str(frontmatter).map_err(|e| {
        fs_err!(
            ErrorCode::InvalidSkill,
            "Cannot record dbt metadata: unparseable SKILL.md frontmatter: {}",
            e
        )
    })?;
    let Some(mapping) = parsed.as_mapping() else {
        return Ok(IndexMap::new());
    };
    let Some((_, value)) = mapping
        .iter()
        .find(|(key, _)| key.as_str() == Some("metadata"))
    else {
        return Ok(IndexMap::new());
    };
    let Some(entries) = value.as_mapping() else {
        // Claude Code drops a non-map `metadata`; treat it as absent.
        return Ok(IndexMap::new());
    };
    Ok(entries
        .iter()
        .filter_map(|(key, value)| Some((key.as_str()?.to_string(), value.clone())))
        .collect())
}

/// Whether `metadata` is declared in a shape `strip_metadata_block` would
/// mangle: a flow mapping spread over more than one line.
fn has_unsupported_metadata_shape(frontmatter: &str) -> bool {
    for line in frontmatter.lines() {
        let trimmed = line.trim_end_matches('\r');
        if let Some(rest) = trimmed.strip_prefix("metadata:") {
            let rest = rest.trim();
            return rest.starts_with('{') && !rest.ends_with('}');
        }
    }
    false
}

/// Remove a top-level `metadata:` mapping from `frontmatter`.
///
/// Only that one declaration is rewritten; every other line keeps its bytes.
/// Handles a block mapping and a single-line flow mapping; a multi-line flow
/// mapping is rejected earlier by `has_unsupported_metadata_shape`.
fn strip_metadata_block(frontmatter: &str) -> String {
    let mut out = String::new();
    let mut skipping_block = false;
    for line in frontmatter.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\n', '\r']);
        if skipping_block {
            // Continuation lines of a block mapping are indented; anything else
            // ends it.
            if trimmed.is_empty() || trimmed.starts_with(' ') || trimmed.starts_with('\t') {
                continue;
            }
            skipping_block = false;
        }
        if trimmed == "metadata:" {
            skipping_block = true;
            continue;
        }
        if trimmed.starts_with("metadata: ") || trimmed.starts_with("metadata:{") {
            // Single-line flow mapping or scalar, with or without a space after
            // the colon (`metadata: {...}` / `metadata:{...}`); drop just this
            // line. A multi-line flow mapping is rejected earlier by
            // `has_unsupported_metadata_shape`.
            continue;
        }
        out.push_str(line);
    }
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> SkillMetadata {
        SkillMetadata {
            source: "package".to_string(),
            package: Some("dbt_project_evaluator".to_string()),
            version: Some("1.5.0".to_string()),
            source_path: "skills/how-to".to_string(),
            source_hash: "sha256:abc".to_string(),
            installed_at: "2026-06-30T00:00:00Z".to_string(),
        }
    }

    const PLAIN: &str = "---\nname: foo\ndescription: A skill.\n---\n\n# Body\n";

    #[test]
    fn injects_a_metadata_block_and_reads_it_back() {
        let injected = inject(PLAIN, &sample()).unwrap();
        assert_eq!(read(&injected), Some(sample()));
    }

    #[test]
    fn injection_preserves_the_authors_other_frontmatter_and_body_verbatim() {
        let injected = inject(PLAIN, &sample()).unwrap();
        assert!(injected.contains("name: foo"), "{injected}");
        assert!(injected.contains("description: A skill."), "{injected}");
        assert!(injected.ends_with("\n# Body\n"), "{injected}");
        // Only the six spec fields may appear at the top level.
        assert!(
            !injected.contains("\ndbt."),
            "no top-level dbt keys: {injected}"
        );
    }

    #[test]
    fn a_skill_without_our_keys_is_not_ours() {
        assert_eq!(read(PLAIN), None);
    }

    #[test]
    fn someone_elses_metadata_is_not_ours() {
        let other = "---\nname: foo\ndescription: A skill.\nmetadata:\n  vendor.tool: other\n---\n";
        assert_eq!(read(other), None);
    }

    #[test]
    fn an_authors_existing_metadata_keys_survive_injection() {
        let authored = "---\nname: foo\ndescription: A skill.\nmetadata:\n  vendor.tier: gold\n---\n\n# Body\n";
        let injected = inject(authored, &sample()).unwrap();
        assert!(injected.contains("vendor.tier"), "{injected}");
        assert_eq!(read(&injected), Some(sample()));
    }

    #[test]
    fn injection_is_deterministic() {
        // Idempotency depends on regenerating byte-identical output.
        assert_eq!(
            inject(PLAIN, &sample()).unwrap(),
            inject(PLAIN, &sample()).unwrap()
        );
    }

    #[test]
    fn re_injecting_replaces_dbts_own_keys_rather_than_duplicating_them() {
        let once = inject(PLAIN, &sample()).unwrap();
        let twice = inject(&once, &sample()).unwrap();
        assert_eq!(once, twice);
    }

    #[test]
    fn a_value_containing_quotes_or_backslashes_still_round_trips() {
        // A source path (or an author's own metadata) can contain characters
        // that are special inside a double-quoted YAML scalar. dbt writes the
        // metadata itself, so an unescaped value here would be dbt producing
        // frontmatter it then fails to read back — misfiling its own install.
        let meta = SkillMetadata {
            source_path: r#"skills/we"ird\path"#.to_string(),
            ..sample()
        };
        let injected = inject(PLAIN, &meta).unwrap();
        assert_eq!(read(&injected), Some(meta));
    }

    #[test]
    fn a_skill_md_without_frontmatter_is_an_error() {
        assert!(inject("no frontmatter here\n", &sample()).is_err());
    }

    #[test]
    fn a_multiline_flow_metadata_mapping_is_refused_rather_than_mangled() {
        let awkward = "---\nname: foo\nmetadata: {\n  a: b\n}\n---\nbody\n";
        assert!(inject(awkward, &sample()).is_err());
    }

    #[test]
    fn a_single_line_flow_metadata_mapping_is_replaced_not_duplicated() {
        // A single-line flow mapping, with or without a space after the colon,
        // must be stripped so dbt does not emit a second `metadata:` key that
        // it then fails to read back.
        for authored in [
            "---\nname: foo\ndescription: A skill.\nmetadata: {vendor.tier: gold}\n---\nbody\n",
            "---\nname: foo\ndescription: A skill.\nmetadata:{vendor.tier: gold}\n---\nbody\n",
        ] {
            let injected = inject(authored, &sample()).unwrap();
            assert_eq!(injected.matches("metadata:").count(), 1, "{injected}");
            assert_eq!(read(&injected), Some(sample()), "{injected}");
        }
    }

    #[test]
    fn a_structured_author_metadata_value_survives_injection() {
        // A nested map or sequence under `metadata` must not be flattened away:
        // dbt owns only its `dbt.*` keys and carries everything else across
        // untouched.
        let authored = "---\nname: foo\ndescription: A skill.\nmetadata:\n  \
            vendor.config:\n    tier: gold\n    regions:\n      - us\n      - eu\n---\n\n# Body\n";
        let injected = inject(authored, &sample()).unwrap();

        // dbt's own record still reads back.
        assert_eq!(read(&injected), Some(sample()));

        // And the author's structured value is intact, not scalarized.
        let frontmatter = split_frontmatter_parts(&injected).unwrap().1;
        let meta = existing_metadata(frontmatter).unwrap();
        let config = meta.get("vendor.config").unwrap().as_mapping().unwrap();
        assert_eq!(
            config.get("tier").and_then(|v| v.as_str()),
            Some("gold"),
            "{injected}"
        );
        assert!(
            config.get("regions").unwrap().as_sequence().is_some(),
            "{injected}"
        );
    }

    #[test]
    fn allowed_tools_survives_injection() {
        let with_tools =
            "---\nname: foo\ndescription: A skill.\nallowed-tools: [Read, Grep]\n---\nbody\n";
        let injected = inject(with_tools, &sample()).unwrap();
        assert!(
            injected.contains("allowed-tools: [Read, Grep]"),
            "{injected}"
        );
        assert_eq!(read(&injected), Some(sample()));
    }
}
