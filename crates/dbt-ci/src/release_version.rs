//! `preview` is accepted as an alias for `rc` — both translate to PEP 440 `rcN`.

use anyhow::{Result, anyhow, bail};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PreReleaseLane {
    Alpha,
    Beta,
    Rc,
    Dev,
}

impl PreReleaseLane {
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "alpha" => Self::Alpha,
            "beta" => Self::Beta,
            "rc" | "preview" => Self::Rc,
            "dev" => Self::Dev,
            _ => return None,
        })
    }
}

#[derive(Debug)]
pub(crate) struct ParsedReleaseVersion<'a> {
    pub base: &'a str,
    pub prerelease: Option<(PreReleaseLane, &'a str)>,
}

impl ParsedReleaseVersion<'_> {
    pub(crate) fn to_pep440(&self) -> String {
        let Some((lane, n)) = self.prerelease else {
            return self.base.to_string();
        };
        match lane {
            PreReleaseLane::Alpha => format!("{}a{n}", self.base),
            PreReleaseLane::Beta => format!("{}b{n}", self.base),
            PreReleaseLane::Rc => format!("{}rc{n}", self.base),
            PreReleaseLane::Dev => format!("{}.dev{n}", self.base),
        }
    }
}

pub(crate) fn parse_release_version(semver: &str) -> Result<ParsedReleaseVersion<'_>> {
    if semver.contains('+') {
        bail!("SemVer build metadata not supported in {semver:?}");
    }
    let (base, pre) = match semver.split_once('-') {
        Some((b, p)) => (b, Some(p)),
        None => (semver, None),
    };
    let parts: Vec<&str> = base.split('.').collect();
    let well_formed = parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()));
    if !well_formed {
        bail!("expected X.Y.Z, got {base:?} in {semver:?}");
    }
    let Some(pre) = pre else {
        return Ok(ParsedReleaseVersion {
            base,
            prerelease: None,
        });
    };
    let (lane_str, number) = pre
        .split_once('.')
        .ok_or_else(|| anyhow!("expected `<lane>.N` after `-`, got {pre:?} in {semver:?}"))?;
    let lane = PreReleaseLane::parse(lane_str).ok_or_else(|| {
        anyhow!(
            "unsupported pre-release lane {lane_str:?} in {semver:?}; \
             accepted: alpha, beta, rc, preview, dev"
        )
    })?;
    if number.is_empty() || !number.chars().all(|c| c.is_ascii_digit()) {
        bail!("expected integer after `{lane_str}.`, got {number:?} in {semver:?}");
    }
    Ok(ParsedReleaseVersion {
        base,
        prerelease: Some((lane, number)),
    })
}

pub(crate) fn validate_release_version(semver: &str) -> Result<()> {
    parse_release_version(semver).map(|_| ())
}

pub(crate) fn semver_to_pep440(semver: &str) -> Result<String> {
    parse_release_version(semver).map(|p| p.to_pep440())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_accepts_clean_shapes() {
        assert!(validate_release_version("2.0.0").is_ok());
        assert!(validate_release_version("2.0.0-alpha.4").is_ok());
        assert!(validate_release_version("2.0.0-beta.2").is_ok());
        assert!(validate_release_version("2.0.0-rc.1").is_ok());
        assert!(validate_release_version("2.0.0-dev.5").is_ok());
    }

    #[test]
    fn validate_rejects_compound_prerelease() {
        assert!(validate_release_version("2.0.0-preview-nightly.176").is_err());
        assert!(validate_release_version("2.0.0-rc-staging.3").is_err());
    }

    #[test]
    fn validate_accepts_preview_lane() {
        assert!(validate_release_version("2.0.0-preview.1").is_ok());
        assert!(validate_release_version("2.0.0-preview.176").is_ok());
    }

    #[test]
    fn validate_rejects_unknown_lane() {
        assert!(validate_release_version("2.0.0-snapshot.1").is_err());
        assert!(validate_release_version("2.0.0-nightly.5").is_err());
    }

    #[test]
    fn validate_rejects_malformed() {
        assert!(validate_release_version("1.2").is_err());
        assert!(validate_release_version("1.2.3.4").is_err());
        assert!(validate_release_version("1.2.3-alpha").is_err());
        assert!(validate_release_version("1.2.3-alpha.").is_err());
        assert!(validate_release_version("1.2.3-alpha.x").is_err());
        assert!(validate_release_version("1.2.3-alpha.4.5").is_err());
        assert!(validate_release_version("1.2.3+build.5").is_err());
    }

    #[test]
    fn semver_to_pep440_passes_release_through() {
        assert_eq!(semver_to_pep440("2.0.0").unwrap(), "2.0.0");
    }

    #[test]
    fn semver_to_pep440_translates_prereleases() {
        assert_eq!(semver_to_pep440("2.0.0-alpha.1").unwrap(), "2.0.0a1");
        assert_eq!(semver_to_pep440("2.0.0-beta.7").unwrap(), "2.0.0b7");
        assert_eq!(semver_to_pep440("2.0.0-rc.3").unwrap(), "2.0.0rc3");
        assert_eq!(semver_to_pep440("2.0.0-preview.3").unwrap(), "2.0.0rc3");
        assert_eq!(semver_to_pep440("2.0.0-dev.42").unwrap(), "2.0.0.dev42");
    }

    #[test]
    fn semver_to_pep440_rejects_unknown_lanes() {
        assert!(semver_to_pep440("2.0.0-snapshot.1").is_err());
    }
}
