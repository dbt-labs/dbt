//! The dbt version shown to users and packages.
//!
//! dbt Core exposes PEP 440 versions (`1.10.0b1`), and packages unpack
//! `dbt_version.split('.')` into exactly three parts. Fusion builds carry Cargo
//! semver (`2.1.0-alpha.4`) or nightly versions (`2.1.0.dev20261124.1`) that
//! split into more parts, so they are converted before they are shown.

use std::sync::LazyLock;

/// `DBT_VERSION` at build time (set by release builds), else the Cargo version.
const BUILD_VERSION: &str = match option_env!("DBT_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};

static DBT_VERSION: LazyLock<String> = LazyLock::new(|| to_pep440(BUILD_VERSION));

/// The dbt version for the Jinja `dbt_version` global and artifact
/// `metadata.dbt_version`: valid PEP 440 with exactly three dot-separated parts.
pub fn dbt_version() -> &'static str {
    DBT_VERSION.as_str()
}

/// Converts a build version to PEP 440 with exactly three dot-separated parts.
///
/// - `X.Y.Z-alpha.N`, `-beta.N`, `-rc.N`, `-dev.N` become `X.Y.ZaN`, `bN`,
///   `rcN`, `devN`. `-preview.N` becomes `rcN`, as for the published wheels.
/// - `X.Y.Z.devD` becomes `X.Y.ZdevD` (PEP 440 implicit separator).
/// - A same-day rebuild number (`X.Y.Z.devD.N`) becomes the local label `+N`.
/// - Any other suffix becomes `dev0` plus a local label, so it still sorts
///   before the final release.
/// - Local labels use `-` instead of `.`, which PEP 440 and dbt Core's semver
///   parser both accept.
///
/// A version that does not start with `X.Y.Z` is returned unchanged; neither
/// Cargo nor the release planner produce one.
fn to_pep440(version: &str) -> String {
    let (head, build) = version.split_once('+').unwrap_or((version, ""));
    let Some((release, suffix)) = split_release(head) else {
        return version.to_string();
    };
    let mut local = Vec::new();
    let known = if let Some(pre) = suffix.strip_prefix('-') {
        match pre.split_once('.') {
            Some(("alpha", n)) if is_number(n) => Some(format!("a{n}")),
            Some(("beta", n)) if is_number(n) => Some(format!("b{n}")),
            Some(("rc" | "preview", n)) if is_number(n) => Some(format!("rc{n}")),
            Some(("dev", n)) if is_number(n) => Some(format!("dev{n}")),
            _ => None,
        }
    } else if let Some(dev) = suffix.strip_prefix(".dev") {
        match dev.split_once('.') {
            None if is_number(dev) => Some(format!("dev{dev}")),
            Some((date, n)) if is_number(date) && is_number(n) => {
                local.push(n);
                Some(format!("dev{date}"))
            }
            _ => None,
        }
    } else if !suffix.contains('.') {
        // Empty, or already PEP 440 such as `a4`.
        Some(suffix.to_string())
    } else {
        None
    };
    let suffix = known.unwrap_or_else(|| {
        local.push(suffix.trim_start_matches(['-', '.']));
        "dev0".to_string()
    });
    if !build.is_empty() {
        local.push(build);
    }
    let local = local.join("-").replace(['.', '_'], "-");
    if local.is_empty() {
        format!("{release}{suffix}")
    } else {
        format!("{release}{suffix}+{local}")
    }
}

/// Splits `X.Y.Z...` into `("X.Y.Z", "...")`.
fn split_release(version: &str) -> Option<(&str, &str)> {
    let mut end = 0;
    for part in 0..3 {
        let digits = version[end..]
            .bytes()
            .take_while(u8::is_ascii_digit)
            .count();
        end += digits;
        if digits == 0 || (part < 2 && version.as_bytes().get(end) != Some(&b'.')) {
            return None;
        }
        if part < 2 {
            end += 1;
        }
    }
    Some(version.split_at(end))
}

fn is_number(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every form the build can produce, with its expected output.
    const FORMS: &[(&str, &str)] = &[
        ("2.0.12", "2.0.12"),
        ("2.1.0-alpha.4", "2.1.0a4"),
        ("2.1.0-beta.2", "2.1.0b2"),
        ("2.1.0-rc.1", "2.1.0rc1"),
        ("2.1.0a4", "2.1.0a4"),
        ("2.1.0b2", "2.1.0b2"),
        ("2.1.0.dev20261124", "2.1.0dev20261124"),
        ("2.1.0.dev20261016.1", "2.1.0dev20261016+1"),
        (
            "2.1.0-nightly.20261124.g1a2b3c4",
            "2.1.0dev0+nightly-20261124-g1a2b3c4",
        ),
        ("2.0.0-preview.111", "2.0.0rc111"),
        ("2.1.0-dev.5", "2.1.0dev5"),
        ("2026.8.4+2f16d1c", "2026.8.4+2f16d1c"),
        ("2.1.0-alpha.4+abc.1", "2.1.0a4+abc-1"),
        ("2.1.0.1", "2.1.0dev0+1"),
    ];

    #[test]
    fn converts_every_build_form() {
        for (input, expected) in FORMS {
            assert_eq!(to_pep440(input), *expected, "{input}");
        }
    }

    #[test]
    fn every_form_has_three_dot_parts() {
        let outputs = FORMS.iter().map(|(input, _)| to_pep440(input));
        for version in outputs.chain([dbt_version().to_string()]) {
            assert_eq!(version.split('.').count(), 3, "{version}");
        }
    }
}
