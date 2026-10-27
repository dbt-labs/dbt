use std::str::FromStr;

use crate::semver::{Version, VersionSpecifier, versions_compatible};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequireDbtVersionCheck {
    Compatible,
    Incompatible { required: String, running: String },
    Unparseable { text: String },
    RunningUnparseable { running: String },
}

pub fn check_require_dbt_version<S: AsRef<str>>(
    clauses: &[S],
    running: &str,
) -> RequireDbtVersionCheck {
    let mut pieces = Vec::new();
    let mut authored = Vec::new();
    for clause in clauses {
        let clause = clause.as_ref().trim();
        if clause.is_empty() {
            continue;
        }
        let mut any = false;
        for piece in clause.split(',') {
            let piece = piece.trim();
            if piece.is_empty() {
                continue;
            }
            pieces.push(piece);
            any = true;
        }
        if any {
            authored.push(clause);
        }
    }
    if pieces.is_empty() {
        return RequireDbtVersionCheck::Compatible;
    }

    // A prerelease build stamp must not be compared, or a spec that excludes
    // that stamp would warn the user about their require-dbt-version.
    let pin = match VersionSpecifier::from_str(&format!("={running}")) {
        Ok(spec) if spec.prerelease.is_none() => spec,
        _ => {
            return RequireDbtVersionCheck::RunningUnparseable {
                running: running.to_string(),
            };
        }
    };

    let mut specs = Vec::with_capacity(pieces.len() + 1);
    for piece in pieces {
        match VersionSpecifier::from_str(piece) {
            Ok(spec) => specs.push(Version::Spec(spec)),
            Err(_) => {
                return RequireDbtVersionCheck::Unparseable {
                    text: piece.to_string(),
                };
            }
        }
    }
    specs.push(Version::Spec(pin));

    if versions_compatible(&specs) {
        RequireDbtVersionCheck::Compatible
    } else {
        let required = if authored.len() == 1 {
            authored[0].to_string()
        } else {
            format!("[{}]", authored.join(", "))
        };
        RequireDbtVersionCheck::Incompatible {
            required,
            running: running.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{RequireDbtVersionCheck, check_require_dbt_version};

    fn check(clauses: &[&str], running: &str) -> RequireDbtVersionCheck {
        check_require_dbt_version(clauses, running)
    }

    #[test]
    fn empty_input_is_compatible() {
        assert_eq!(check(&[], "2.0.8"), RequireDbtVersionCheck::Compatible);
        assert_eq!(check(&[""], "2.0.8"), RequireDbtVersionCheck::Compatible);
        assert_eq!(
            check(&["  ,  "], "2.0.8"),
            RequireDbtVersionCheck::Compatible
        );
    }

    #[test]
    fn lower_bound_that_includes_running_is_compatible() {
        assert_eq!(
            check(&[">=1.0.0"], "2.0.8"),
            RequireDbtVersionCheck::Compatible
        );
    }

    #[test]
    fn lower_bound_that_excludes_running_is_incompatible() {
        assert_eq!(
            check(&[">=99.0.0"], "2.0.8"),
            RequireDbtVersionCheck::Incompatible {
                required: ">=99.0.0".to_string(),
                running: "2.0.8".to_string(),
            }
        );
    }

    #[test]
    fn separate_clauses_are_anded_and_shown_as_a_list() {
        assert_eq!(
            check(&[">=1.0.0", "<2.0.0"], "2.0.8"),
            RequireDbtVersionCheck::Incompatible {
                required: "[>=1.0.0, <2.0.0]".to_string(),
                running: "2.0.8".to_string(),
            }
        );
    }

    #[test]
    fn comma_inside_one_clause_is_an_and() {
        assert_eq!(
            check(&[">=1.0.0, <2.0.0"], "1.6.0"),
            RequireDbtVersionCheck::Compatible
        );
        assert_eq!(
            check(&[">=1.0.0, <2.0.0"], "2.0.8"),
            RequireDbtVersionCheck::Incompatible {
                required: ">=1.0.0, <2.0.0".to_string(),
                running: "2.0.8".to_string(),
            }
        );
    }

    #[test]
    fn partial_version_is_unparseable() {
        assert_eq!(
            check(&[">=1.6"], "2.0.8"),
            RequireDbtVersionCheck::Unparseable {
                text: ">=1.6".to_string(),
            }
        );
    }

    #[test]
    fn first_bad_piece_stops_the_check() {
        assert_eq!(
            check(&[">=1.0.0, nope"], "2.0.8"),
            RequireDbtVersionCheck::Unparseable {
                text: "nope".to_string(),
            }
        );
    }

    #[test]
    fn bare_version_is_an_exact_pin() {
        assert_eq!(
            check(&["2.0.8"], "2.0.8"),
            RequireDbtVersionCheck::Compatible
        );
        assert_eq!(
            check(&["2.0.8"], "2.0.9"),
            RequireDbtVersionCheck::Incompatible {
                required: "2.0.8".to_string(),
                running: "2.0.9".to_string(),
            }
        );
    }

    #[test]
    fn surrounding_whitespace_is_trimmed() {
        assert_eq!(
            check(&["  >=1.0.0 , <3.0.0  "], "2.0.8"),
            RequireDbtVersionCheck::Compatible
        );
    }

    #[test]
    fn prerelease_running_version_is_not_compared() {
        assert_eq!(
            check(&[">=1.0.0"], "2.0.0-beta.2"),
            RequireDbtVersionCheck::RunningUnparseable {
                running: "2.0.0-beta.2".to_string(),
            }
        );
    }
}
