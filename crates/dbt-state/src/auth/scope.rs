use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode};
use serde::Deserialize;

use crate::service_client::RunCacheServiceError;

const RUNCACHE_ORG_SCOPE_PREFIX: &str = "runcache:scope:org:";
const RUNCACHE_APP_SCOPE_PREFIX: &str = "runcache:scope:app:";

#[derive(Debug, Clone)]
pub struct Scope {
    org_scopes: Vec<String>,
    app_scopes: Vec<String>,
}

impl Scope {
    pub fn from_string(scope: &str) -> Result<Self, RunCacheServiceError> {
        let mut org_scopes = Vec::new();
        let mut app_scopes = Vec::new();

        for part in scope.split_whitespace() {
            let part = part.to_string();
            if part.starts_with(RUNCACHE_ORG_SCOPE_PREFIX) {
                validate_scope_format(&part)?;
                org_scopes.push(part);
            } else if part.starts_with(RUNCACHE_APP_SCOPE_PREFIX) {
                validate_scope_format(&part)?;
                app_scopes.push(part);
            }
        }

        Ok(Self {
            org_scopes,
            app_scopes,
        })
    }

    pub fn org_ids(&self) -> Vec<String> {
        let mut ids = Vec::new();
        for s in &self.org_scopes {
            if let Some(id) = extract_org_id(s) {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }
        ids
    }

    pub fn is_org_id_in_scope(&self, org_id: &str) -> bool {
        let ids = self.org_ids();
        ids.iter().any(|id| id == org_id || id == "*")
    }

    pub fn is_org_id_disabled(&self, org_id: &str) -> bool {
        if self.is_org_id_in_scope(org_id) {
            return false;
        }
        self.app_scopes
            .iter()
            .filter_map(|s| extract_org_id(s))
            .any(|id| id == org_id)
    }

    /// Organization IDs present in `:app:` scopes but absent from `:org:` scopes.
    ///
    /// These are organizations the user is associated with but whose access has
    /// been disabled.
    pub fn disabled_org_ids(&self) -> Vec<String> {
        let active = self.org_ids();
        let mut ids = Vec::new();
        for s in &self.app_scopes {
            if let Some(id) = extract_org_id(s) {
                if !active.contains(&id) && !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }
        ids
    }
}

fn validate_scope_format(scope: &str) -> Result<(), RunCacheServiceError> {
    let parts: Vec<&str> = scope.split(':').collect();
    if parts.len() < 5 || parts[3].is_empty() {
        return Err(RunCacheServiceError::Auth(format!(
            "invalid OAuth scope '{scope}'"
        )));
    }
    Ok(())
}

fn extract_org_id(scope: &str) -> Option<String> {
    let parts: Vec<&str> = scope.split(':').collect();
    if parts.len() >= 5 && !parts[3].is_empty() {
        Some(parts[3].to_string())
    } else {
        None
    }
}

pub fn determine_org_id(
    scope: &Scope,
    configured_org_id: Option<&str>,
) -> Result<String, RunCacheServiceError> {
    if let Some(configured) = configured_org_id {
        if scope.is_org_id_in_scope(configured) {
            return Ok(configured.to_string());
        }
        if scope.is_org_id_disabled(configured) {
            return Err(RunCacheServiceError::OrgDisabled {
                org_id: configured.to_string(),
            });
        }
        return Err(RunCacheServiceError::Auth(format!(
            "OAuth token does not grant access to configured organization '{configured}'"
        )));
    }

    let org_ids = scope.org_ids();
    match org_ids.as_slice() {
        [id] if id != "*" => Ok(id.clone()),
        [] => {
            // A single disabled org and no configured org_id is unambiguous: surface
            // the disabled error rather than the generic "specify an org" message.
            if let [org_id] = scope.disabled_org_ids().as_slice() {
                return Err(RunCacheServiceError::OrgDisabled {
                    org_id: org_id.clone(),
                });
            }
            Err(RunCacheServiceError::Auth(
                "Cannot determine the account ID. Please make sure dbt State is enabled in your account.".to_string(),
            ))
        }
        _ => Err(RunCacheServiceError::Auth(
            "The OAuth token grants access to multiple or wildcard organization scopes; specify which organization to use by setting 'state-org-id' in the 'dbt-cloud' config block"
                .to_string(),
        )),
    }
}

#[derive(Debug, Deserialize)]
pub struct JwtClaims {
    pub scope: Option<String>,
}

pub fn jwt_claims(id_token: &str) -> Result<JwtClaims, RunCacheServiceError> {
    let mut validation = Validation::new(Algorithm::HS256);
    validation.insecure_disable_signature_validation();
    validation.validate_exp = false;
    validation.validate_aud = false;
    validation.required_spec_claims.clear();

    decode::<JwtClaims>(id_token, &DecodingKey::from_secret(&[]), &validation)
        .map(|token| token.claims)
        .map_err(|err| RunCacheServiceError::Auth(format!("invalid OAuth JWT claims: {err}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{EncodingKey, Header, encode};
    use serde::Serialize;

    #[test]
    fn org_id_is_inferred_from_single_token_scope() {
        let scope = Scope::from_string("runcache:scope:org:test-org:admin").unwrap();
        assert_eq!(determine_org_id(&scope, None).unwrap(), "test-org");
    }

    #[test]
    fn configured_org_id_must_be_in_token_scope() {
        let scope =
            Scope::from_string("runcache:scope:org:other-org:admin runcache:scope:org:*:admin")
                .unwrap();
        assert_eq!(
            determine_org_id(&scope, Some("test-org")).unwrap(),
            "test-org"
        );

        let scope = Scope::from_string("runcache:scope:org:other-org:admin").unwrap();
        let err = determine_org_id(&scope, Some("test-org")).unwrap_err();
        assert!(
            err.to_string()
                .contains("does not grant access to configured organization")
        );
    }

    #[test]
    fn org_id_requires_config_for_ambiguous_token_scope() {
        let scope = Scope::from_string(
            "runcache:scope:org:first-org:admin runcache:scope:org:second-org:admin",
        )
        .unwrap();
        let err = determine_org_id(&scope, None).unwrap_err();
        assert!(err.to_string().contains("state-org-id"));
    }

    #[test]
    fn determine_org_id_errors_when_token_has_no_org_scope() {
        let scope = Scope::from_string("").unwrap();
        let err = determine_org_id(&scope, None).unwrap_err();
        assert!(
            err.to_string()
                .contains("Please make sure dbt State is enabled in your account")
        );
    }

    #[test]
    fn disabled_org_ids_lists_app_only_orgs() {
        let scope = Scope::from_string(
            "runcache:scope:app:active-org:developer runcache:scope:org:active-org:developer",
        )
        .unwrap();
        assert!(scope.disabled_org_ids().is_empty());

        let scope = Scope::from_string("runcache:scope:app:disabled-org:developer").unwrap();
        assert_eq!(scope.disabled_org_ids(), vec!["disabled-org".to_string()]);

        let scope = Scope::from_string(
            "runcache:scope:app:active-org:developer runcache:scope:org:active-org:developer \
             runcache:scope:app:disabled-org:developer",
        )
        .unwrap();
        assert_eq!(scope.disabled_org_ids(), vec!["disabled-org".to_string()]);

        let scope = Scope::from_string("").unwrap();
        assert!(scope.disabled_org_ids().is_empty());
    }

    #[test]
    fn determine_org_id_errors_when_configured_org_is_disabled() {
        let scope = Scope::from_string("runcache:scope:app:disabled-org:developer").unwrap();
        let err = determine_org_id(&scope, Some("disabled-org")).unwrap_err();
        assert!(matches!(
            err,
            RunCacheServiceError::OrgDisabled { ref org_id } if org_id == "disabled-org"
        ));
    }

    #[test]
    fn determine_org_id_errors_when_single_disabled_org_and_no_config() {
        let scope = Scope::from_string("runcache:scope:app:disabled-org:developer").unwrap();
        let err = determine_org_id(&scope, None).unwrap_err();
        assert!(matches!(
            err,
            RunCacheServiceError::OrgDisabled { ref org_id } if org_id == "disabled-org"
        ));
    }

    #[test]
    fn is_org_id_disabled_when_only_app_scope_is_present() {
        let scope = Scope::from_string("runcache:scope:app:disabled-org:developer").unwrap();
        assert!(scope.is_org_id_disabled("disabled-org"));
        assert!(!scope.is_org_id_in_scope("disabled-org"));
    }

    #[test]
    fn is_org_id_not_disabled_when_org_scope_is_present() {
        let scope = Scope::from_string(
            "runcache:scope:org:active-org:admin runcache:scope:app:active-org:developer",
        )
        .unwrap();
        assert!(!scope.is_org_id_disabled("active-org"));
        assert!(scope.is_org_id_in_scope("active-org"));
    }

    #[test]
    fn is_org_id_not_disabled_when_wildcard_org_scope_is_present() {
        let scope =
            Scope::from_string("runcache:scope:org:*:admin runcache:scope:app:other-org:developer")
                .unwrap();
        assert!(!scope.is_org_id_disabled("other-org"));
    }

    #[test]
    fn invalid_scope_format_is_rejected() {
        let err = Scope::from_string("runcache:scope:org:").unwrap_err();
        assert!(err.to_string().contains("invalid OAuth scope"));
    }

    #[test]
    fn jwt_scope_is_read_from_id_token_claims() {
        let scope_str = "runcache:scope:org:test-org:admin";
        let id_token = jwt_with_scope(scope_str);
        assert_eq!(
            jwt_claims(&id_token).unwrap().scope.as_deref(),
            Some(scope_str)
        );
    }

    fn jwt_with_scope(scope: &str) -> String {
        #[derive(Serialize)]
        struct Claims<'a> {
            scope: &'a str,
        }
        encode(
            &Header::default(),
            &Claims { scope },
            &EncodingKey::from_secret(b"test-secret"),
        )
        .unwrap()
    }
}
