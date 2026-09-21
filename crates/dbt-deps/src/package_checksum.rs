use std::{collections::BTreeMap, path::Path};

use dbt_common::FsResult;
use dbt_jinja_utils::{
    jinja_environment::JinjaEnv,
    phases::load::LoadContext,
    serde::{into_typed_with_jinja, value_from_file_async},
};
use dbt_schemas::schemas::packages::DbtPackageEntry;
use serde_json::{Map, Value};
use sha1::Digest;

use crate::steps::DbtPackageType;

pub(crate) fn fusion_sha1_hash_packages(
    packages: &[DbtPackageEntry],
    use_v2_compatible_package_downloads: bool,
) -> String {
    let mut package_strs = packages
        .iter()
        .map(|package| serde_json::to_string(package).unwrap())
        .collect::<Vec<_>>();
    package_strs.sort();
    // Add flag for installing v2-compatible downloads from Package Hub to hash
    // so changing the flag will trigger a fresh deps install.
    // Only use true so existing package lock files don't need updates.
    if use_v2_compatible_package_downloads {
        package_strs.push(format!(
            "use_v2_compatible_package_downloads: {use_v2_compatible_package_downloads}"
        ));
    }
    sha1_hex(&package_strs.join("\n"))
}

/// Calculate dbt Core's checksum for common package declarations.
///
/// dbt Core reference:
/// <https://github.com/dbt-labs/dbt-core/blob/d97d5c5b711e61eb8c486083aa1c84ec041808cf/core/dbt/task/deps.py#L42-L56>
///
/// Core renders Jinja in `packages.yml` before hashing but leaves `dependencies.yml` literal.
///
/// Compatibility is intentionally best effort. Python number coercion and float formatting
/// are not reproduced and may cause checksum mismatches. Other YAML/schema
/// differences are not normalized: PyYAML boolean aliases (`yes`/`no`/`on`/`off`) are unsupported,
/// and schema defaults/coercions are reproduced only for the known package fields below.
/// Unsupported shapes return `None`; other differences can yield a checksum mismatch, causing
/// normal resolution. Package loading may reject declarations outside Fusion's supported schema.
pub(crate) async fn core_sha1_hash_package_file(
    package_definition_path: &Path,
    package_type: DbtPackageType,
    jinja_env: &JinjaEnv,
    vars: &BTreeMap<String, dbt_yaml::Value>,
) -> FsResult<Option<String>> {
    // Package loading already reported YAML diagnostics; do not repeat them for this fallback.
    let unrendered = value_from_file_async(package_definition_path, false, None).await?;
    let rendered = match package_type {
        DbtPackageType::PackageYml => {
            let deps_context = LoadContext::new(vars.clone());
            into_typed_with_jinja(
                unrendered.clone(),
                true,
                jinja_env,
                &deps_context,
                &[],
                None,
                false,
            )?
        }
        DbtPackageType::DependenciesYml => unrendered.clone(),
    };

    let Some(unrendered_packages) = unrendered
        .get("packages")
        .and_then(|value| value.as_sequence())
    else {
        return Ok(Some(sha1_hex("")));
    };
    let Some(rendered_packages) = rendered
        .get("packages")
        .and_then(|value| value.as_sequence())
    else {
        return Ok(None);
    };
    if unrendered_packages.len() != rendered_packages.len() {
        return Ok(None);
    }

    let mut package_strs = rendered_packages
        .iter()
        .zip(unrendered_packages)
        .map(|(rendered, unrendered)| {
            core_package_value(rendered, unrendered).map(|value| python_json(&value))
        })
        .collect::<Option<Vec<_>>>();
    let Some(ref mut package_strs) = package_strs else {
        return Ok(None);
    };
    package_strs.sort();
    Ok(Some(sha1_hex(&package_strs.join("\n"))))
}

fn core_package_value(rendered: &dbt_yaml::Value, unrendered: &dbt_yaml::Value) -> Option<Value> {
    let mut rendered = yaml_object(rendered)?;
    let unrendered = yaml_object(unrendered)?;
    let source_keys = ["package", "git", "local", "private", "tarball"];
    let sources = source_keys
        .iter()
        .filter(|key| rendered.contains_key(**key))
        .copied()
        .collect::<Vec<_>>();
    if sources.len() != 1 || !unrendered.contains_key(sources[0]) {
        return None;
    }

    let allowed_keys: &[&str] = match sources[0] {
        "package" => &["package", "version", "install_prerelease", "name"],
        "git" => &["git", "revision", "warn-unpinned", "subdirectory", "name"],
        "local" => &["local", "name"],
        "private" => &[
            "private",
            "provider",
            "revision",
            "warn-unpinned",
            "subdirectory",
            "name",
        ],
        "tarball" => &["tarball", "name"],
        _ => return None,
    };
    if rendered
        .keys()
        .chain(unrendered.keys())
        .any(|key| !allowed_keys.contains(&key.as_str()))
    {
        return None;
    }

    match sources[0] {
        "package" => {
            rendered
                .entry("install_prerelease".to_string())
                .or_insert(Value::Bool(false));
        }
        "git" => insert_null_defaults(
            &mut rendered,
            &["revision", "warn-unpinned", "subdirectory"],
        ),
        "private" => insert_null_defaults(
            &mut rendered,
            &["provider", "revision", "warn-unpinned", "subdirectory"],
        ),
        "local" | "tarball" => {}
        _ => return None,
    }

    if rendered.get("name").is_some_and(Value::is_null) {
        rendered.remove("name");
    }
    rendered.insert("unrendered".to_string(), Value::Object(unrendered));
    Some(Value::Object(rendered))
}

fn yaml_object(value: &dbt_yaml::Value) -> Option<Map<String, Value>> {
    serde_json::to_value(value).ok()?.as_object().cloned()
}

fn insert_null_defaults(value: &mut Map<String, Value>, keys: &[&str]) {
    for key in keys {
        value.entry((*key).to_string()).or_insert(Value::Null);
    }
}

fn sha1_hex(value: &str) -> String {
    format!("{:x}", sha1::Sha1::digest(value.as_bytes()))
}

fn python_json(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => python_json_string(value),
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(python_json)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Object(values) => {
            let mut entries = values.iter().collect::<Vec<_>>();
            entries.sort_by_key(|(key, _)| *key);
            format!(
                "{{{}}}",
                entries
                    .into_iter()
                    .map(|(key, value)| format!(
                        "{}: {}",
                        python_json_string(key),
                        python_json(value)
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    }
}

fn python_json_string(value: &str) -> String {
    let mut result = String::with_capacity(value.len() + 2);
    result.push('"');
    for character in value.chars() {
        match character {
            '"' => result.push_str("\\\""),
            '\\' => result.push_str("\\\\"),
            '\u{0008}' => result.push_str("\\b"),
            '\u{000c}' => result.push_str("\\f"),
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            '\t' => result.push_str("\\t"),
            '\u{0000}'..='\u{001f}' => write_unicode_escape(&mut result, character as u16),
            '\u{0020}'..='\u{007e}' => result.push(character),
            '\u{007f}'..='\u{ffff}' => write_unicode_escape(&mut result, character as u16),
            _ => {
                let codepoint = character as u32 - 0x1_0000;
                write_unicode_escape(&mut result, 0xd800 | ((codepoint >> 10) as u16));
                write_unicode_escape(&mut result, 0xdc00 | ((codepoint & 0x3ff) as u16));
            }
        }
    }
    result.push('"');
    result
}

fn write_unicode_escape(result: &mut String, value: u16) {
    use std::fmt::Write;
    write!(result, "\\u{value:04x}").unwrap();
}
