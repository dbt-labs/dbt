//! The coding agents dbt can install skills for, and where each one reads them
//! from.

use std::collections::BTreeSet;
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use dbt_common::ErrorCode;
use dbt_common::tracing::dbt_emit::emit_warn_log_message;
use dbt_common::warn_error_options::project_flags_get_value;

/// The de-facto cross-tool skills directory, and the destination for every
/// provider that reads it (dbt Wizard, Codex, OpenAI, Cursor, and Gemini, which
/// reads it as an alias for its own `.gemini/skills`).
pub const DEFAULT_SKILLS_DIR: &str = ".agents/skills";
/// Claude Code only discovers skills under its own directory.
pub const CLAUDE_SKILLS_DIR: &str = ".claude/skills";

/// A coding agent dbt knows how to install skills for.
///
/// The set is fixed and small, so it is an enum rather than free-form strings:
/// once a user's `ai_provider` value has been parsed into one of these, every
/// downstream step is total and no code has to re-handle "what if it isn't a
/// provider we know". Adding a provider means adding a variant and its two
/// `match` arms, which the compiler will insist on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AiProvider {
    /// dbt's own agent harness.
    Wizard,
    Claude,
    Openai,
    Codex,
    Cursor,
    Gemini,
}

impl AiProvider {
    /// Every provider dbt knows about, in the order shown to users.
    pub const ALL: [AiProvider; 6] = [
        AiProvider::Wizard,
        AiProvider::Claude,
        AiProvider::Openai,
        AiProvider::Codex,
        AiProvider::Cursor,
        AiProvider::Gemini,
    ];

    /// The name users write in `ai_provider`.
    pub const fn as_str(self) -> &'static str {
        match self {
            AiProvider::Wizard => "wizard",
            AiProvider::Claude => "claude",
            AiProvider::Openai => "openai",
            AiProvider::Codex => "codex",
            AiProvider::Cursor => "cursor",
            AiProvider::Gemini => "gemini",
        }
    }

    /// The directory this provider reads skills from, relative to the project root.
    pub const fn destination(self) -> &'static str {
        match self {
            AiProvider::Claude => CLAUDE_SKILLS_DIR,
            AiProvider::Wizard
            | AiProvider::Openai
            | AiProvider::Codex
            | AiProvider::Cursor
            | AiProvider::Gemini => DEFAULT_SKILLS_DIR,
        }
    }

    /// Comma-separated list of every provider, for diagnostics.
    pub fn all_names() -> String {
        AiProvider::ALL
            .iter()
            .map(|provider| provider.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

impl fmt::Display for AiProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Returned when `ai_provider` names something dbt has no destination for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownAiProvider(pub String);

impl FromStr for AiProvider {
    type Err = UnknownAiProvider;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let normalized = value.trim().to_ascii_lowercase();
        AiProvider::ALL
            .into_iter()
            .find(|provider| provider.as_str() == normalized)
            .ok_or_else(|| UnknownAiProvider(value.trim().to_string()))
    }
}

/// Parse raw `ai_provider` values into known providers.
///
/// Unknown values warn and are dropped rather than failing: an `ai_provider`
/// dbt doesn't recognize should not break package installation, and a newer dbt
/// may well know it.
pub fn parse_providers(raw: &[String]) -> Vec<AiProvider> {
    raw.iter()
        .filter_map(|value| match value.parse::<AiProvider>() {
            Ok(provider) => Some(provider),
            Err(UnknownAiProvider(name)) => {
                emit_warn_log_message(
                    ErrorCode::UnknownAiProvider,
                    format!(
                        "Unknown ai_provider '{name}'; no skills will be installed for it. \
                         Known providers: {}.",
                        AiProvider::all_names()
                    ),
                );
                None
            }
        })
        .collect()
}

/// Distinct destination directories for the given providers, relative to the
/// project root.
///
/// Providers that read the same directory — every provider but Claude shares
/// `.agents/skills/` — collapse to one destination, so dbt writes each skill
/// there once.
pub fn resolve_destinations(providers: &[AiProvider]) -> Vec<PathBuf> {
    let mut seen = BTreeSet::new();
    providers
        .iter()
        .map(|provider| PathBuf::from(provider.destination()))
        .filter(|dir| seen.insert(dir.clone()))
        .collect()
}

/// Resolve the raw `ai_provider` setting from the CLI/env value and the
/// project's `flags:` block.
///
/// CLI (which clap also populates from `DBT_AI_PROVIDER`) wins over
/// `dbt_project.yml`, matching how every other dual-source flag resolves.
/// Accepts either a single string or a list in the project file.
///
/// Values are left as written so the caller can tell "unset" apart from "set to
/// something dbt doesn't recognize" — the two produce different warnings. Use
/// [`parse_providers`] to turn the result into [`AiProvider`]s.
pub fn resolve_ai_provider(
    from_cli: Option<&[String]>,
    project_flags: Option<&dbt_yaml::Value>,
) -> Option<Vec<String>> {
    if let Some(from_cli) = from_cli
        && !from_cli.is_empty()
    {
        return Some(from_cli.to_vec());
    }

    let value = project_flags.and_then(|flags| project_flags_get_value(flags, "ai_provider"))?;
    let providers = match value {
        dbt_yaml::Value::Sequence(values, _) => values
            .iter()
            .filter_map(|value| value.as_str().map(str::to_string))
            .collect(),
        other => other.as_str().map(|s| vec![s.to_string()])?,
    };

    let providers: Vec<String> = providers
        .into_iter()
        .filter(|provider: &String| !provider.trim().is_empty())
        .collect();

    (!providers.is_empty()).then_some(providers)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| v.to_string()).collect()
    }

    #[test]
    fn claude_reads_its_own_dir_and_everyone_else_shares_agents() {
        assert_eq!(AiProvider::Claude.destination(), CLAUDE_SKILLS_DIR);
        for provider in [
            AiProvider::Wizard,
            AiProvider::Openai,
            AiProvider::Codex,
            AiProvider::Cursor,
            AiProvider::Gemini,
        ] {
            assert_eq!(provider.destination(), DEFAULT_SKILLS_DIR, "{provider}");
        }
    }

    #[test]
    fn every_provider_round_trips_through_its_name() {
        for provider in AiProvider::ALL {
            assert_eq!(provider.as_str().parse::<AiProvider>(), Ok(provider));
        }
    }

    #[test]
    fn parsing_is_case_and_whitespace_insensitive() {
        assert_eq!("  Claude ".parse::<AiProvider>(), Ok(AiProvider::Claude));
        assert_eq!("CODEX".parse::<AiProvider>(), Ok(AiProvider::Codex));
    }

    #[test]
    fn an_unrecognized_name_reports_itself_trimmed() {
        assert_eq!(
            "  nope ".parse::<AiProvider>(),
            Err(UnknownAiProvider("nope".to_string()))
        );
    }

    #[test]
    fn unknown_providers_are_dropped_and_known_ones_kept() {
        let parsed = parse_providers(&raw(&["claude", "definitely-not-a-harness", "wizard"]));
        assert_eq!(parsed, vec![AiProvider::Claude, AiProvider::Wizard]);
    }

    fn dest(dir: &str) -> PathBuf {
        PathBuf::from(dir)
    }

    #[test]
    fn providers_that_share_a_directory_are_deduplicated() {
        let providers = [
            AiProvider::Wizard,
            AiProvider::Codex,
            AiProvider::Cursor,
            AiProvider::Claude,
        ];
        assert_eq!(
            resolve_destinations(&providers),
            vec![dest(DEFAULT_SKILLS_DIR), dest(CLAUDE_SKILLS_DIR)]
        );
    }

    #[test]
    fn gemini_shares_the_agents_directory_with_the_other_providers() {
        // Gemini reads `.agents/skills` as an alias, so it collapses onto the
        // same shared destination rather than a directory of its own.
        assert_eq!(
            resolve_destinations(&[AiProvider::Wizard, AiProvider::Gemini]),
            vec![dest(DEFAULT_SKILLS_DIR)]
        );
    }

    #[test]
    fn only_unknown_providers_yields_no_destinations() {
        let parsed = parse_providers(&raw(&["definitely-not-a-harness"]));
        assert!(parsed.is_empty());
        assert!(resolve_destinations(&parsed).is_empty());
    }

    #[test]
    fn cli_wins_over_project_flags() {
        let flags: dbt_yaml::Value = dbt_yaml::from_str("ai_provider: claude").unwrap();
        assert_eq!(
            resolve_ai_provider(Some(&raw(&["wizard"])), Some(&flags)),
            Some(raw(&["wizard"]))
        );
    }

    #[test]
    fn project_flags_accept_a_bare_string() {
        let flags: dbt_yaml::Value = dbt_yaml::from_str("ai_provider: claude").unwrap();
        assert_eq!(
            resolve_ai_provider(None, Some(&flags)),
            Some(raw(&["claude"]))
        );
    }

    #[test]
    fn project_flags_accept_a_list() {
        let flags: dbt_yaml::Value =
            dbt_yaml::from_str("ai_provider:\n  - claude\n  - wizard\n").unwrap();
        assert_eq!(
            resolve_ai_provider(None, Some(&flags)),
            Some(raw(&["claude", "wizard"]))
        );
    }

    #[test]
    fn unset_everywhere_resolves_to_none() {
        assert_eq!(resolve_ai_provider(None, None), None);
        let flags: dbt_yaml::Value = dbt_yaml::from_str("something_else: true").unwrap();
        assert_eq!(resolve_ai_provider(Some(&[]), Some(&flags)), None);
    }

    #[test]
    fn set_but_unrecognized_is_distinguishable_from_unset() {
        // The two produce different warnings, so the raw resolution must not
        // collapse them.
        let flags: dbt_yaml::Value = dbt_yaml::from_str("ai_provider: nope").unwrap();
        assert_eq!(
            resolve_ai_provider(None, Some(&flags)),
            Some(raw(&["nope"]))
        );
        assert!(parse_providers(&raw(&["nope"])).is_empty());
    }
}
