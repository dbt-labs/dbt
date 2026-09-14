//! Deserializing `dbt_project.yml` fragments the way the rest of dbt does.
//!
//! `dbt_yaml::from_str` is not enough on its own: dbt's project config structs
//! rely on dbt-yaml's "dunder flattening" (`__additional_properties__` standing
//! in for `#[serde(flatten)]`), which only kicks in via `Value::into_typed`.
//! Everything here reads files that may contain a `skills:` block, so it all
//! goes through that path.

use serde::de::DeserializeOwned;

/// Deserialize a YAML string, honoring dbt-yaml's dunder flattening.
///
/// Unrecognized keys are ignored rather than reported — callers here are
/// reading a `dbt_project.yml` that the project loader validates properly
/// elsewhere, and duplicating its warnings would double up on every run.
pub fn from_str<T: DeserializeOwned>(input: &str) -> Result<T, dbt_yaml::Error> {
    // Strip a leading UTF-8 BOM before parsing, exactly as the project loader
    // does (see `dbt-jinja-utils`' serde and `dbt-profile`'s resolve): a
    // package `dbt_project.yml` authored on Windows can carry one, and dbt-yaml
    // would otherwise read it as part of the first key and fail.
    let input = input.strip_prefix('\u{feff}').unwrap_or(input);
    let value: dbt_yaml::Value = dbt_yaml::from_str(input)?;
    into_typed(value)
}

/// As [`from_str`], for an already-parsed value.
pub fn into_typed<T: DeserializeOwned>(value: dbt_yaml::Value) -> Result<T, dbt_yaml::Error> {
    value.into_typed(|_path, _key, _value| {}, |_value| Ok(None))
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_schemas::schemas::project::{DbtProject, ProjectSkillConfig};

    #[test]
    fn reads_a_nested_skills_block() {
        let config: ProjectSkillConfig =
            from_str("my_package:\n  my-skill:\n    +enabled: false\n").unwrap();
        let package = config
            .__additional_properties__
            .get("my_package")
            .and_then(|v| v.as_ref())
            .expect("package key");
        let skill = package
            .__additional_properties__
            .get("my-skill")
            .and_then(|v| v.as_ref())
            .expect("skill key");
        assert_eq!(skill.enabled, Some(false));
    }

    #[test]
    fn reads_a_whole_project_with_skill_settings() {
        let project: DbtProject = from_str(
            "name: my_project\nskill-paths: [\"agent-skills\"]\n\
             skills:\n  my_project:\n    +enabled: false\n",
        )
        .unwrap();

        assert_eq!(project.skill_paths, Some(vec!["agent-skills".to_string()]));
        assert!(project.skills.is_some());
    }

    #[test]
    fn unknown_keys_are_ignored() {
        let project: DbtProject =
            from_str("name: my_project\nsomething-we-do-not-know: true\n").unwrap();
        assert_eq!(project.name, "my_project");
    }

    #[test]
    fn a_leading_utf8_bom_is_ignored() {
        // A dbt_project.yml saved with a BOM (common on Windows) must parse the
        // same as one without, rather than folding the BOM into the first key.
        let project: DbtProject = from_str("\u{feff}name: my_project\n").unwrap();
        assert_eq!(project.name, "my_project");
    }
}
