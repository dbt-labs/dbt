use dbt_common::{ErrorCode, FsError, FsResult, err, fs_err};
use std::sync::LazyLock;
use std::{collections::HashMap, fmt::Display, str::FromStr};

const _MATCHERS: &str = r"(?P<matcher>\>=|\>|\<|\<=|=)?";
const _NUM_NO_LEADING_ZEROS: &str = r"(0|[1-9]\d*)";
const _ALPHA: &str = r"(?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*)";
const _ALPHA_NO_LEADING_ZEROS: &str = r"(?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*)";

const _BASE_VERSION_REGEX: &str =
    r"(?P<major>(0|[1-9]\d*))\.(?P<minor>(0|[1-9]\d*))\.(?P<patch>(0|[1-9]\d*))";

const _VERSION_EXTRA_REGEX: &str = r"
(\-?
  (?P<prerelease>
    (?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*)(\.(?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*))*))?
(\+
  (?P<build>
    (?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*)(\.(?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*))*))?
";

const _VERSION_REGEX_PAT_STR: &str = r"(?x)
^
(?P<matcher>>=|<=|>|<|=)           # Capture >=, <=, >, <, or =
?
(?P<major>0|[1-9]\d*)\.(?P<minor>0|[1-9]\d*)\.(?P<patch>0|[1-9]\d*)  # Major.Minor.Patch
(?:\-?
  (?P<prerelease>
    (?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*)(?:\.(?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*))*))?  # Prerelease (optional)
(?:\+
  (?P<build>
    [0-9A-Za-z-]*(\.[0-9A-Za-z-]*)*))?  # Build metadata (optional)
$
";

static VERSION_REGEX: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(_VERSION_REGEX_PAT_STR).unwrap());

fn _cmp(a: &str, b: &str) -> std::cmp::Ordering {
    match (a > b) as i32 - (a < b) as i32 {
        0 => std::cmp::Ordering::Equal,
        1 => std::cmp::Ordering::Greater,
        -1 => std::cmp::Ordering::Less,
        _ => unreachable!(),
    }
}

#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub enum Matchers {
    GreaterThan,
    GreaterThanOrEqualTo,
    LessThan,
    LessThanOrEqualTo,
    #[default]
    Exact,
}

impl Display for Matchers {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Matchers::GreaterThan => write!(f, ">"),
            Matchers::GreaterThanOrEqualTo => write!(f, ">="),
            Matchers::LessThan => write!(f, "<"),
            Matchers::LessThanOrEqualTo => write!(f, "<="),
            Matchers::Exact => write!(f, "="),
        }
    }
}

impl TryFrom<&str> for Matchers {
    type Error = Box<FsError>;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            ">" => Ok(Self::GreaterThan),
            ">=" => Ok(Self::GreaterThanOrEqualTo),
            "<" => Ok(Self::LessThan),
            "<=" => Ok(Self::LessThanOrEqualTo),
            "=" => Ok(Self::Exact),
            _ => err!(ErrorCode::RuntimeError, "Invalid matcher: {value}"),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Version {
    Spec(VersionSpecifier),
    Range(VersionRange),
    String(String),
}

impl Version {
    pub fn is_prerelease(&self) -> bool {
        matches!(self, Version::Spec(spec) if spec.is_prerelease())
    }
}

#[derive(Default, Debug, Clone)]
pub struct VersionSpecifier {
    pub major: Option<u64>,
    pub minor: Option<u64>,
    pub patch: Option<u64>,
    pub prerelease: Option<String>,
    pub build: Option<String>,
    pub matcher: Matchers,
}

impl VersionSpecifier {
    pub fn to_version_string(&self, skip_matcher: bool) -> String {
        let prerelease = if let Some(prerelease) = &self.prerelease {
            format!("-{prerelease}")
        } else {
            String::new()
        };
        let build = if let Some(build) = &self.build {
            format!("+{build}")
        } else {
            String::new()
        };
        let matcher = if skip_matcher {
            String::new()
        } else {
            self.matcher.to_string()
        };
        format!(
            "{}{}.{}.{}{}{}",
            matcher,
            self.major.unwrap_or_default(),
            self.minor.unwrap_or_default(),
            self.patch.unwrap_or_default(),
            prerelease,
            build
        )
    }

    pub fn is_unbounded(&self) -> bool {
        self.major.is_none() && self.minor.is_none() && self.patch.is_none()
    }

    pub fn is_lower_bound(&self) -> bool {
        !self.is_unbounded()
            && (self.matcher == Matchers::GreaterThan
                || self.matcher == Matchers::GreaterThanOrEqualTo)
    }

    pub fn is_upper_bound(&self) -> bool {
        !self.is_unbounded()
            && (self.matcher == Matchers::LessThan || self.matcher == Matchers::LessThanOrEqualTo)
    }

    pub fn is_exact(&self) -> bool {
        !self.is_unbounded() && self.matcher == Matchers::Exact
    }

    pub fn is_prerelease(&self) -> bool {
        self.prerelease.is_some()
    }

    pub fn _nat_cmp(a: &Option<String>, b: &Option<String>) -> std::cmp::Ordering {
        let a = a.as_deref().unwrap_or_default();
        let b = b.as_deref().unwrap_or_default();
        let a_parts = a.split('.').collect::<Vec<&str>>();
        let b_parts = b.split('.').collect::<Vec<&str>>();
        for (a_part, b_part) in a_parts.iter().zip(b_parts.iter()) {
            let cmp = match (a_part.parse::<u32>(), b_part.parse::<u32>()) {
                (Ok(a_part), Ok(b_part)) => a_part.cmp(&b_part),
                (Ok(_), Err(_)) => std::cmp::Ordering::Less,
                (Err(_), Ok(_)) => std::cmp::Ordering::Greater,
                (Err(_), Err(_)) => _cmp(a_part, b_part),
            };
            if cmp != std::cmp::Ordering::Equal {
                return cmp;
            }
        }
        _cmp(a.len().to_string().as_str(), b.len().to_string().as_str())
    }
}

impl PartialEq for VersionSpecifier {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == std::cmp::Ordering::Equal
    }
}

impl Eq for VersionSpecifier {}

impl PartialOrd for VersionSpecifier {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for VersionSpecifier {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        if self.is_unbounded() || other.is_unbounded() {
            return std::cmp::Ordering::Equal;
        }
        let major_cmp = self.major.cmp(&other.major);
        if major_cmp != std::cmp::Ordering::Equal {
            return major_cmp;
        }
        let minor_cmp = self.minor.cmp(&other.minor);
        if minor_cmp != std::cmp::Ordering::Equal {
            return minor_cmp;
        }
        let patch_cmp = self.patch.cmp(&other.patch);
        if patch_cmp != std::cmp::Ordering::Equal {
            return patch_cmp;
        }
        // Then compare the prerelease version
        if self.prerelease.is_some() || other.prerelease.is_some() {
            if self.prerelease.is_none() {
                if self.matcher == Matchers::LessThan {
                    return std::cmp::Ordering::Less;
                } else {
                    return std::cmp::Ordering::Greater;
                }
            } else if other.prerelease.is_none() {
                return std::cmp::Ordering::Less;
            }
            let prcmp = Self::_nat_cmp(&self.prerelease, &other.prerelease);
            if prcmp != std::cmp::Ordering::Equal {
                return prcmp;
            }
        }
        let equal = (self.matcher == Matchers::GreaterThanOrEqualTo
            && other.matcher == Matchers::LessThanOrEqualTo)
            || (self.matcher == Matchers::LessThanOrEqualTo
                && other.matcher == Matchers::GreaterThanOrEqualTo);
        if equal {
            return std::cmp::Ordering::Equal;
        }

        let less = (self.matcher == Matchers::LessThan
            && other.matcher == Matchers::LessThanOrEqualTo)
            || (other.matcher == Matchers::GreaterThan
                && self.matcher == Matchers::GreaterThanOrEqualTo)
            || (self.is_lower_bound() && other.is_upper_bound());
        if less {
            return std::cmp::Ordering::Less;
        }

        let greater = (other.matcher == Matchers::LessThan
            && self.matcher == Matchers::LessThanOrEqualTo)
            || (self.matcher == Matchers::GreaterThan
                && other.matcher == Matchers::GreaterThanOrEqualTo)
            || (other.is_lower_bound() && self.is_upper_bound());
        if greater {
            return std::cmp::Ordering::Greater;
        }

        std::cmp::Ordering::Equal
    }
}

impl Display for VersionSpecifier {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.to_version_string(false))
    }
}

impl FromStr for VersionSpecifier {
    type Err = Box<FsError>;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let captures = VERSION_REGEX
            .captures(s)
            .ok_or_else(|| fs_err!(ErrorCode::RuntimeError, "Error parsing semver version {s}"))?;
        let matcher = captures.name("matcher").map(|m| m.as_str().to_string());
        let major = captures
            .name("major")
            .and_then(|m| m.as_str().parse::<u64>().ok());
        let minor = captures
            .name("minor")
            .and_then(|m| m.as_str().parse::<u64>().ok());
        let patch = captures
            .name("patch")
            .and_then(|m| m.as_str().parse::<u64>().ok());
        let prerelease = captures.name("prerelease").map(|m| m.as_str().to_string());
        let build = captures.name("build").map(|m| m.as_str().to_string());
        Ok(Self {
            major,
            minor,
            patch,
            prerelease,
            build,
            matcher: if matcher.is_none() {
                Matchers::Exact
            } else {
                Matchers::try_from(matcher.as_deref().unwrap())?
            },
        })
    }
}

#[derive(Default, Debug, Clone)]
pub struct VersionRange {
    pub start: VersionSpecifier,
    pub end: VersionSpecifier,
}

impl VersionRange {
    pub fn try_combine_exact(
        a: VersionSpecifier,
        b: VersionSpecifier,
    ) -> FsResult<VersionSpecifier> {
        if a.cmp(&b) == std::cmp::Ordering::Equal {
            Ok(a)
        } else {
            err!(
                ErrorCode::RuntimeError,
                "Cannot combine non-exact versions: {a} and {b}"
            )
        }
    }

    pub fn try_combine_lower_bound_with_exact(
        lower: VersionSpecifier,
        exact: VersionSpecifier,
    ) -> FsResult<VersionSpecifier> {
        let comparison = lower.cmp(&exact);
        if comparison == std::cmp::Ordering::Less
            || (comparison == std::cmp::Ordering::Equal
                && lower.matcher == Matchers::GreaterThanOrEqualTo)
        {
            Ok(exact)
        } else {
            err!(
                ErrorCode::RuntimeError,
                "Versions are not compatible: {lower} and {exact}"
            )
        }
    }

    pub fn try_combine_lower_bound(
        a: VersionSpecifier,
        b: VersionSpecifier,
    ) -> FsResult<VersionSpecifier> {
        if b.is_unbounded() {
            return Ok(a);
        } else if a.is_unbounded() {
            return Ok(b);
        }

        if !(a.is_exact() || b.is_exact()) {
            let comparison = a.cmp(&b);
            if comparison == std::cmp::Ordering::Less {
                Ok(b)
            } else {
                Ok(a)
            }
        } else if a.is_exact() {
            Self::try_combine_lower_bound_with_exact(b, a)
        } else {
            Self::try_combine_lower_bound_with_exact(a, b)
        }
    }

    pub fn try_combine_upper_bound_with_exact(
        upper: VersionSpecifier,
        exact: VersionSpecifier,
    ) -> FsResult<VersionSpecifier> {
        let comparison = upper.cmp(&exact);
        if comparison == std::cmp::Ordering::Greater
            || (comparison == std::cmp::Ordering::Equal
                && upper.matcher == Matchers::LessThanOrEqualTo)
        {
            Ok(exact)
        } else {
            err!(
                ErrorCode::RuntimeError,
                "Versions are not compatible: {upper} and {exact}"
            )
        }
    }

    pub fn try_combine_upper_bound(
        a: VersionSpecifier,
        b: VersionSpecifier,
    ) -> FsResult<VersionSpecifier> {
        if b.is_unbounded() {
            return Ok(a);
        } else if a.is_unbounded() {
            return Ok(b);
        }

        if !(a.is_exact() || b.is_exact()) {
            let comparison = a.cmp(&b);
            if comparison == std::cmp::Ordering::Greater {
                Ok(b)
            } else {
                Ok(a)
            }
        } else if a.is_exact() {
            Self::try_combine_upper_bound_with_exact(b, a)
        } else {
            Self::try_combine_upper_bound_with_exact(a, b)
        }
    }

    pub fn reduce(&self, other: &VersionRange) -> FsResult<VersionRange> {
        let (start, end) = if self.start.is_exact() && other.start.is_exact() {
            let start = Self::try_combine_exact(self.start.clone(), other.start.clone())?;
            (start.clone(), start)
        } else {
            (
                Self::try_combine_lower_bound(self.start.clone(), other.start.clone())?,
                Self::try_combine_upper_bound(self.end.clone(), other.end.clone())?,
            )
        };

        if start.cmp(&end) == std::cmp::Ordering::Greater {
            return err!(
                ErrorCode::RuntimeError,
                "Invalid range: {start:?} and {end:?}"
            );
        }

        Ok(Self { start, end })
    }
}

impl Display for VersionRange {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let mut range_vec = Vec::new();
        if self.start.is_unbounded() && self.end.is_unbounded() {
            return write!(f, "ANY");
        }
        if !self.start.is_unbounded() {
            range_vec.push(self.start.to_version_string(false));
        }
        if !self.end.is_unbounded() {
            range_vec.push(self.end.to_version_string(false));
        }
        write!(f, "{}", range_vec.join(", "))
    }
}

impl From<VersionSpecifier> for VersionRange {
    fn from(spec: VersionSpecifier) -> Self {
        match spec.matcher {
            Matchers::Exact => Self {
                start: spec.clone(),
                end: spec,
            },
            Matchers::GreaterThan | Matchers::GreaterThanOrEqualTo => Self {
                start: spec,
                end: VersionSpecifier::default(),
            },
            Matchers::LessThan | Matchers::LessThanOrEqualTo => Self {
                start: VersionSpecifier::default(),
                end: spec,
            },
        }
    }
}

pub fn reduce_versions(versions: &[Version]) -> FsResult<VersionRange> {
    let mut version_specifiers = vec![];
    for version in versions {
        match version {
            Version::Spec(spec) => {
                if spec.is_unbounded() {
                    continue;
                }
                version_specifiers.push(spec.clone());
            }
            Version::Range(range) => {
                if !range.start.is_unbounded() {
                    version_specifiers.push(range.start.clone());
                }
                if !range.end.is_unbounded() {
                    version_specifiers.push(range.end.clone());
                }
            }
            Version::String(value) => {
                version_specifiers.push(VersionSpecifier::from_str(value)?);
            }
        }
    }
    if version_specifiers.is_empty() {
        return Ok(VersionRange {
            start: VersionSpecifier::default(),
            end: VersionSpecifier::default(),
        });
    }
    let mut to_return: VersionRange = version_specifiers.pop().unwrap().into();
    for spec in version_specifiers {
        to_return = to_return.reduce(&spec.into())?;
    }
    Ok(to_return)
}

pub fn versions_compatible(versions: &[Version]) -> bool {
    if versions.len() == 1 {
        return true;
    }
    reduce_versions(versions).is_ok()
}

pub fn resolve_to_specific_version(
    range: &VersionRange,
    available_versions: &[String],
) -> FsResult<Option<String>> {
    let mut max_version: Option<VersionSpecifier> = None;
    let mut max_version_string = None;
    for version_str in available_versions {
        let version = VersionSpecifier::from_str(version_str)?;
        if versions_compatible(&[
            Version::Spec(version.clone()),
            Version::Spec(range.start.clone()),
            Version::Spec(range.end.clone()),
        ]) && (max_version.is_none()
            || max_version.as_ref().unwrap().cmp(&version) == std::cmp::Ordering::Less)
        {
            max_version = Some(version);
            max_version_string = Some(version_str.to_string());
        }
    }
    Ok(max_version_string)
}

pub fn filter_installable<I>(versions: I, install_prerelease: bool) -> FsResult<Vec<String>>
where
    I: IntoIterator,
    I::Item: AsRef<str>,
{
    let mut installable = vec![];
    let mut installable_map = HashMap::new();
    for version_str in versions {
        let version = VersionSpecifier::from_str(version_str.as_ref())?;
        if install_prerelease || !version.is_prerelease() {
            installable_map.insert(version.to_string(), version_str.as_ref().to_string());
            installable.push(version);
        }
    }
    installable.sort();
    Ok(installable
        .into_iter()
        .map(|v| installable_map.get(&v.to_string()).unwrap().to_string())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_to_specific_version() {
        let range = VersionRange {
            start: VersionSpecifier::from_str(">=0.2.0").unwrap(),
            end: VersionSpecifier::from_str("<0.3.0").unwrap(),
        };

        let compatible_version = vec![
            Version::Spec(VersionSpecifier::from_str("0.4.0").unwrap()),
            Version::Spec(VersionSpecifier::from_str(">=0.2.0").unwrap()),
            Version::Spec(VersionSpecifier::from_str("<0.3.0").unwrap()),
        ];
        assert!(!versions_compatible(compatible_version.as_slice()));

        let available_versions = vec![
            "0.1.0".to_string(),
            "0.2.0".to_string(),
            "0.3.0".to_string(),
            "0.4.0".to_string(),
        ];
        let actual = resolve_to_specific_version(&range, &available_versions).unwrap();
        let expected = Some("0.2.0".to_string());
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_parse_version_specifiers() {
        let pairs = [
            (">=1.0.0", ">=1.0.0"),
            ("<=1.0.0", "<=1.0.0"),
            (">1.0.0", ">1.0.0"),
            ("<1.0.0", "<1.0.0"),
            ("=1.0.0", "=1.0.0"),
            ("1.0.0", "=1.0.0"),
            ("0.0.2", "=0.0.2"),
            ("1.2.0a1", "=1.2.0-a1"),
        ];

        for p in pairs {
            let version = VersionSpecifier::from_str(p.0).unwrap();
            assert_eq!(
                p.1,
                version.to_string(),
                "Parsed '{}', expected '{}'.",
                p.0,
                p.1
            );
        }
    }

    #[test]
    fn test_reduce_versions_compatible() {
        let compatible_pairs = [("0.0.1", "0.0.1"), (">0.0.1", "0.0.2")];

        for pair in compatible_pairs {
            let str_pair = vec![
                Version::String(pair.0.to_string()),
                Version::String(pair.1.to_string()),
            ];
            assert!(
                versions_compatible(str_pair.as_slice()),
                "Expected '{}' and '{}' to be compatible.",
                pair.0,
                pair.1
            );
        }
    }

    #[test]
    fn test_reduce_versions_incompatible() {
        let incompatible_pairs = [("0.0.1", "0.0.2"), ("0.4.5a1", "0.4.5a2")];

        for pair in incompatible_pairs {
            let str_pair = vec![
                Version::String(pair.0.to_string()),
                Version::String(pair.1.to_string()),
            ];
            assert!(
                !versions_compatible(str_pair.as_slice()),
                "Expected '{}' and '{}' to be compatible.",
                pair.0,
                pair.1
            );
        }
    }

    #[test]
    fn test_filter_installable_prerelease() {
        let installable = [
            "1.1.0",
            "1.2.0a1",
            "1.0.0",
            "2.1.0-alpha",
            "2.2.0asdf",
            "2.1.0",
            "2.2.0",
            "2.2.0-fishtown-beta",
            "2.2.0-2",
        ];

        let actual = filter_installable(installable, true).unwrap();

        let expected = vec![
            "1.0.0",
            "1.1.0",
            "1.2.0a1",
            "2.1.0-alpha",
            "2.1.0",
            "2.2.0-2",
            "2.2.0asdf",
            "2.2.0-fishtown-beta",
            "2.2.0",
        ];

        assert_eq!(actual, expected);
    }

    #[test]
    fn test_filter_installable() {
        let installable = [
            "1.1.0",
            "1.2.0a1",
            "1.0.0",
            "2.1.0-alpha",
            "2.2.0asdf",
            "2.1.0",
            "2.2.0",
            "2.2.0-fishtown-beta",
        ];

        let actual = filter_installable(installable, false).unwrap();
        let expected = vec!["1.0.0", "1.1.0", "2.1.0", "2.2.0"];
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_complex_but_valid_versions() {
        let valid_versions = [
            "0.0.4",
            "1.2.3",
            "10.20.30",
            "1.1.2-prerelease+meta",
            "1.1.2+meta",
            "1.1.2+meta-valid",
            "1.0.0-alpha",
            "1.0.0-beta",
            "1.0.0-alpha.beta",
            "1.0.0-alpha.beta.1",
            "1.0.0-alpha.1",
            "1.0.0-alpha0.valid",
            "1.0.0-alpha.0valid",
            "1.0.0-alpha-a.b-c-somethinglong+build.1-aef.1-its-okay",
            "1.0.0-rc.1+build.1",
            "2.0.0-rc.1+build.123",
            "1.2.3-beta",
            "10.2.3-DEV-SNAPSHOT",
            "1.2.3-SNAPSHOT-123",
            "1.0.0",
            "2.0.0",
            "1.1.7",
            "2.0.0+build.1848",
            "2.0.1-alpha.1227",
            "1.0.0-alpha+beta",
            "1.2.3----RC-SNAPSHOT.12.9.1--.12+788",
            "1.2.3----R-S.12.9.1--.12+meta",
            "1.2.3----RC-SNAPSHOT.12.9.1--.12",
            "1.0.0+0.build.1-rc.10000aaa-kk-0.1",
            "99999999999999999999999.999999999999999999.99999999999999999",
            "1.0.0-0A.is.legal",
        ];

        for v in valid_versions {
            assert!(
                VersionSpecifier::from_str(v).is_ok(),
                "{v} should be a valid semver, but failed to parse."
            )
        }
    }
}
