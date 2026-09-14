//! Classifies the stdout of a `dbt --version` invocation into a
//! distribution and generation. Kept as its own crate, independent of
//! `dbt-dist`'s `dbt-common` dependency, so lightweight consumers (e.g.
//! `dbt-index`, or `wizard/dbt-codex`) can classify a `dbt` binary's version
//! banner without pulling in `dbt-dist`'s full self-update/uninstall
//! dependency tree.

use serde::{Deserialize, Serialize};

/// PyPI's legacy dbt v1 namespace, also used by dbt-dist for its
/// self-managed-upgrade fallback.
pub const DBT_CORE_PACKAGE_NAME: &str = "dbt-core";

/// PyPI's OSS-only dbt v2 namespace.
pub const DBT_OSS_PACKAGE_NAME: &str = "dbt-oss";

/// CLI-brand/package names that classify as [`Distribution::Oss`]; anything
/// else is treated as the proprietary distribution.
pub const UPGRADABLE_TARGET_NAMES: [&str; 2] = [DBT_CORE_PACKAGE_NAME, DBT_OSS_PACKAGE_NAME];

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Distribution {
    /// The full, proprietary dbt v2 distribution.
    #[serde(rename = "dbt")]
    Dbt,
    /// The open-source dbt v2 distribution.
    #[serde(rename = "dbt-oss")]
    Oss,
    /// The legacy, v1-only dbt-core distribution.
    #[serde(rename = "dbt-core")]
    Core,
    #[serde(rename = "cloud-cli")]
    CloudCLI,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Generation {
    V1,
    V2,
    NotApplicable,
}

/// The version installed, per a v1 `Core:` block's `- installed: X.Y.Z`
/// line.
fn extract_v1_installed_version(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .find_map(|line| line.trim().strip_prefix("- installed:"))
        .map(|v| v.trim().to_string())
}

/// Classifies the stdout of a `dbt --version` invocation into a
/// `(generation, distribution, version)` triple, or `None` if the output
/// doesn't look like any known `dbt` banner. Handles both the `dbt-fusion
/// X.Y.Z` banner and the renamed `dbt X.Y.Z` banner as `Generation::V2`.
pub fn classify_version_output(stdout: &str) -> Option<(Generation, Distribution, Option<String>)> {
    if stdout.contains("Core:") {
        return Some((
            Generation::V1,
            Distribution::Core,
            extract_v1_installed_version(stdout),
        ));
    }
    if stdout.starts_with("dbt Cloud CLI") {
        return Some((Generation::NotApplicable, Distribution::CloudCLI, None));
    }
    // Validation check: dbt-oss, dbt (proprietary), and the Cloud CLI all
    // contain "dbt" in the output.
    if !stdout.contains("dbt") {
        return None;
    }
    let mut parts = stdout.split_whitespace();
    let name = parts.next()?;
    let version = parts.next()?;
    if !version.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    Some((
        Generation::V2,
        distribution_from_name(name),
        Some(version.to_string()),
    ))
}

/// Classifies a CLI-brand name (the same string printed as the leading token
/// of a v2 binary's `--version` banner, and injected into the running
/// process as its own `command_name`) into a [Distribution]. Any name in
/// [`UPGRADABLE_TARGET_NAMES`] is OSS; everything else is proprietary.
pub fn distribution_from_name(name: &str) -> Distribution {
    if UPGRADABLE_TARGET_NAMES.contains(&name) {
        Distribution::Oss
    } else {
        Distribution::Dbt
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distribution_serializes_to_spec_contract() {
        assert_eq!(
            serde_json::to_string(&Distribution::Dbt).unwrap(),
            "\"dbt\""
        );
        assert_eq!(
            serde_json::to_string(&Distribution::Oss).unwrap(),
            "\"dbt-oss\""
        );
        assert_eq!(
            serde_json::to_string(&Distribution::Core).unwrap(),
            "\"dbt-core\""
        );
        assert_eq!(
            serde_json::to_string(&Distribution::CloudCLI).unwrap(),
            "\"cloud-cli\""
        );
    }

    #[test]
    fn generation_serializes_to_spec_contract() {
        assert_eq!(serde_json::to_string(&Generation::V1).unwrap(), "\"v1\"");
        assert_eq!(serde_json::to_string(&Generation::V2).unwrap(), "\"v2\"");
    }

    const V1_VERSION_OUTPUT: &str = "\
Core:
  - installed: 1.12.0
  - latest:    1.12.0 - Up to date!

Plugins:
";

    #[test]
    fn classify_version_output_v1_core_block_is_core() {
        assert_eq!(
            classify_version_output(V1_VERSION_OUTPUT),
            Some((
                Generation::V1,
                Distribution::Core,
                Some("1.12.0".to_string())
            ))
        );
    }

    #[test]
    fn classify_version_output_v2_banner_is_dbt() {
        assert_eq!(
            classify_version_output("dbt-fusion 2.0.0-preview.196\n"),
            Some((
                Generation::V2,
                Distribution::Dbt,
                Some("2.0.0-preview.196".to_string())
            ))
        );
    }

    #[test]
    fn classify_version_output_v2_banner_without_fusion_branding_is_still_dbt() {
        // The banner's display name is cosmetic and may change (e.g. drop
        // "fusion"); anything other than the OSS build's `dbt-core`/`dbt-oss`
        // name is treated as the proprietary distribution.
        assert_eq!(
            classify_version_output("dbt 2.0.0-preview.196\n"),
            Some((
                Generation::V2,
                Distribution::Dbt,
                Some("2.0.0-preview.196".to_string())
            ))
        );
    }

    #[test]
    fn classify_version_output_v2_dbt_core_banner_is_oss() {
        // `dbt-sa-cli` (the OSS-only v2 build) brands its `--version` banner
        // as `dbt-core` on already-installed preview builds.
        assert_eq!(
            classify_version_output("dbt-core 2.0.0-preview.200\n"),
            Some((
                Generation::V2,
                Distribution::Oss,
                Some("2.0.0-preview.200".to_string())
            ))
        );
    }

    #[test]
    fn classify_version_output_v2_dbt_oss_banner_is_oss() {
        // `dbt-sa-cli` (the OSS-only v2 build) brands its `--version` banner
        // as `dbt-oss`.
        assert_eq!(
            classify_version_output("dbt-oss 2.0.0-preview.200\n"),
            Some((
                Generation::V2,
                Distribution::Oss,
                Some("2.0.0-preview.200".to_string())
            ))
        );
    }

    #[test]
    fn classify_version_output_dbt_cloud_cli() {
        assert_eq!(
            classify_version_output(
                "dbt Cloud CLI - 0.40.18 (aa58f643af1725e279e559883b75cf9e26596d51 2026-06-18T20:34:06Z)\n"
            ),
            Some((Generation::NotApplicable, Distribution::CloudCLI, None))
        );
    }

    #[test]
    fn classify_version_output_none_for_unrecognized_output() {
        assert_eq!(classify_version_output("not a dbt binary\n"), None);
        assert_eq!(classify_version_output(""), None);
    }
}
