//! `SKILL.md` validation, per the AgentSkills specification.
//!
//! A skill is a directory containing a `SKILL.md` whose YAML frontmatter carries
//! a `name` (which must equal the directory name) and a `description`. dbt never
//! mutates a source `SKILL.md` — validation is read-only.

use std::path::Path;

use dbt_common::{ErrorCode, FsResult, fs_err, stdfs};

/// The file that marks a directory as a skill. Matched by exact basename.
pub const SKILL_FILE: &str = "SKILL.md";

/// The frontmatter fields dbt reads. Everything else is passed through untouched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillFrontmatter {
    pub name: String,
    pub description: String,
}

/// Split a `SKILL.md` into `(through-opening-delimiter, frontmatter, remainder)`.
///
/// The frontmatter must open on the first line with `---` and close with a line
/// that is exactly `---`. Returning the surrounding slices lets a caller rebuild
/// the file byte-for-byte around a rewritten frontmatter; callers that only need
/// the block use [`split_frontmatter`]. This is the one frontmatter parser in
/// the crate — `metadata` reads and rewrites through it too.
pub(crate) fn split_frontmatter_parts(contents: &str) -> Option<(&str, &str, &str)> {
    let bom_len = if contents.starts_with('\u{feff}') {
        '\u{feff}'.len_utf8()
    } else {
        0
    };
    let without_bom = &contents[bom_len..];
    let (open, rest) = if let Some(rest) = without_bom.strip_prefix("---\n") {
        ("---\n", rest)
    } else {
        ("---\r\n", without_bom.strip_prefix("---\r\n")?)
    };

    let mut offset = 0usize;
    loop {
        let end = rest[offset..]
            .find('\n')
            .map(|i| offset + i)
            .unwrap_or(rest.len());
        if rest[offset..end].trim_end_matches('\r') == "---" {
            let head_len = bom_len + open.len();
            return Some((&contents[..head_len], &rest[..offset], &rest[offset..]));
        }
        if end == rest.len() {
            return None;
        }
        offset = end + 1;
    }
}

/// The YAML frontmatter block of a `SKILL.md`, or `None` if it has none.
fn split_frontmatter(contents: &str) -> Option<&str> {
    split_frontmatter_parts(contents).map(|(_, frontmatter, _)| frontmatter)
}

/// Parse and validate the frontmatter of a `SKILL.md`.
///
/// `skill_dir` is the directory containing the file; its basename must equal the
/// frontmatter `name`.
pub fn validate_skill(skill_dir: &Path) -> FsResult<SkillFrontmatter> {
    let skill_file = skill_dir.join(SKILL_FILE);
    let contents = stdfs::read_to_string(&skill_file)?;

    let dir_name = skill_dir
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            fs_err!(
                ErrorCode::InvalidSkill,
                "Skill directory has no readable name: {}",
                skill_dir.display()
            )
        })?;

    let frontmatter = split_frontmatter(&contents).ok_or_else(|| {
        fs_err!(
            ErrorCode::InvalidSkill,
            "{} is missing YAML frontmatter; a skill must open with a '---' delimited block",
            skill_file.display()
        )
    })?;

    let parsed: dbt_yaml::Value = dbt_yaml::from_str(frontmatter).map_err(|e| {
        fs_err!(
            ErrorCode::InvalidSkill,
            "Failed to parse YAML frontmatter in {}: {}",
            skill_file.display(),
            e
        )
    })?;

    let field = |key: &str| -> Option<String> {
        parsed
            .as_mapping()
            .and_then(|mapping| {
                mapping
                    .iter()
                    .find(|(candidate, _)| candidate.as_str() == Some(key))
            })
            .and_then(|(_, value)| value.as_str())
            .map(str::to_string)
    };

    let name = field("name").ok_or_else(|| {
        fs_err!(
            ErrorCode::InvalidSkill,
            "{} frontmatter is missing the required 'name' field",
            skill_file.display()
        )
    })?;
    let description = field("description").ok_or_else(|| {
        fs_err!(
            ErrorCode::InvalidSkill,
            "{} frontmatter is missing the required 'description' field",
            skill_file.display()
        )
    })?;

    if name != dir_name {
        return Err(fs_err!(
            ErrorCode::InvalidSkill,
            "{} declares name '{}' but lives in directory '{}'; the AgentSkills spec requires them to match",
            skill_file.display(),
            name,
            dir_name
        ));
    }

    Ok(SkillFrontmatter { name, description })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_skill(root: &Path, name: &str, contents: &str) -> std::path::PathBuf {
        let dir = root.join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(SKILL_FILE), contents).unwrap();
        dir
    }

    #[test]
    fn accepts_a_well_formed_skill() {
        let tmp = TempDir::new().unwrap();
        let dir = write_skill(
            tmp.path(),
            "do-a-thing",
            "---\nname: do-a-thing\ndescription: Does a thing.\n---\n\n# Body\n",
        );

        let frontmatter = validate_skill(&dir).unwrap();
        assert_eq!(frontmatter.name, "do-a-thing");
        assert_eq!(frontmatter.description, "Does a thing.");
    }

    #[test]
    fn rejects_a_name_that_does_not_match_the_directory() {
        let tmp = TempDir::new().unwrap();
        let dir = write_skill(
            tmp.path(),
            "do-a-thing",
            "---\nname: something-else\ndescription: Does a thing.\n---\n",
        );

        let err = validate_skill(&dir).unwrap_err();
        assert!(err.to_string().contains("requires them to match"), "{err}");
    }

    #[test]
    fn rejects_missing_frontmatter() {
        let tmp = TempDir::new().unwrap();
        let dir = write_skill(tmp.path(), "do-a-thing", "# Just a heading\n");

        let err = validate_skill(&dir).unwrap_err();
        assert!(
            err.to_string().contains("missing YAML frontmatter"),
            "{err}"
        );
    }

    #[test]
    fn rejects_missing_required_fields() {
        let tmp = TempDir::new().unwrap();
        let dir = write_skill(tmp.path(), "do-a-thing", "---\nname: do-a-thing\n---\n");

        let err = validate_skill(&dir).unwrap_err();
        assert!(err.to_string().contains("'description'"), "{err}");
    }

    #[test]
    fn handles_crlf_line_endings() {
        let tmp = TempDir::new().unwrap();
        let dir = write_skill(
            tmp.path(),
            "do-a-thing",
            "---\r\nname: do-a-thing\r\ndescription: Does a thing.\r\n---\r\n",
        );

        assert_eq!(validate_skill(&dir).unwrap().name, "do-a-thing");
    }
}
