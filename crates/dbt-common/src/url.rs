//! Helpers for inserting a gateway/service subdomain label into a dbt Cloud
//! account host across the various host shapes dbt Cloud uses (Multi-Cell,
//! Multi-Cell staging, Multi-Tenant US/EMEA/AU, Single-Tenant, devspace,
//! vanity).

use crate::{ErrorCode, FsResult, fs_err};

/// Insert `label` as a new subdomain into `host`, following dbt Cloud's
/// account-host conventions.
///
/// `host` may be a bare host or a URL (scheme and path, if present, are
/// stripped). The returned value is always a bare host (no scheme).
///
/// Idempotent: if `label` is already one of `host`'s dot-separated
/// segments, `host` is returned unchanged rather than inserting the label
/// a second time.
///
/// For example, with `label = "semantic-layer"`:
/// - MC: `{account_prefix}.dbt.com` -> `{account_prefix}.semantic-layer.dbt.com`
/// - MC staging: `{account_prefix}.us.staging.dbt.com` -> `{account_prefix}.semantic-layer.us.staging.dbt.com`
/// - US MT: `cloud.getdbt.com` -> `semantic-layer.cloud.getdbt.com`
/// - EMEA MT: `emea.dbt.com` -> `semantic-layer.emea.dbt.com`
/// - AU MT: `au.dbt.com` -> `semantic-layer.au.dbt.com`
/// - ST: `{account_prefix}.singletenant.getdbt.com` -> `semantic-layer.{account_prefix}.singletenant.getdbt.com`
/// - devspace: `{namespace}.dev.dbt.com` -> `{namespace}.semantic-layer.dev.dbt.com`
pub fn insert_gateway_label(host: &str, label: &str) -> FsResult<String> {
    // Normalize to just the host without scheme or path
    let host = if let Ok(url) = url::Url::parse(host) {
        url.host_str().unwrap_or(host).to_string()
    } else {
        host.trim_end_matches('/')
            .split('/')
            .next()
            .unwrap_or(host)
            .to_string()
    };
    if host.split('.').any(|segment| segment == label) {
        return Ok(host);
    }

    let host_error = fs_err!(ErrorCode::InvalidConfig, "dbt host is incorrect");

    if host.ends_with("getdbt.com") {
        Ok(format!("{label}.{host}"))
    } else if host.ends_with("dbt.com") {
        let mut labels: Vec<&str> = host.split('.').collect();
        if labels.len() < 3 {
            return Err(host_error);
        }
        let first = labels[0];
        // Region-level MT hosts (no account prefix) get the label prefixed.
        // Account- or namespace-scoped hosts get the label inserted after the first label.
        let region_hosts = ["emea", "au"];
        if labels.len() == 3 && region_hosts.contains(&first) {
            // e.g. emea.dbt.com -> {label}.emea.dbt.com
            labels.insert(0, label);
        } else {
            // e.g. acme.dbt.com, rr558.us.staging.dbt.com, ns.dev.dbt.com
            labels.insert(1, label);
        }
        Ok(labels.join("."))
    } else {
        Err(host_error)
    }
}

#[cfg(test)]
mod tests {
    use super::insert_gateway_label;

    /// Both labels are exercised throughout to prove the function is generic
    /// and not hardcoded to any one caller's gateway name.
    const LABELS: [&str; 2] = ["dwg", "semantic-layer"];

    #[test]
    fn mc() {
        for label in LABELS {
            assert_eq!(
                insert_gateway_label("acme.dbt.com", label).unwrap(),
                format!("acme.{label}.dbt.com")
            );
        }
    }

    #[test]
    fn mc_staging() {
        for label in LABELS {
            assert_eq!(
                insert_gateway_label("rr558.us.staging.dbt.com", label).unwrap(),
                format!("rr558.{label}.us.staging.dbt.com")
            );
        }
    }

    #[test]
    fn us_mt() {
        for label in LABELS {
            assert_eq!(
                insert_gateway_label("cloud.getdbt.com", label).unwrap(),
                format!("{label}.cloud.getdbt.com")
            );
        }
    }

    #[test]
    fn emea_mt() {
        for label in LABELS {
            assert_eq!(
                insert_gateway_label("emea.dbt.com", label).unwrap(),
                format!("{label}.emea.dbt.com")
            );
        }
    }

    #[test]
    fn au_mt() {
        for label in LABELS {
            assert_eq!(
                insert_gateway_label("au.dbt.com", label).unwrap(),
                format!("{label}.au.dbt.com")
            );
        }
    }

    #[test]
    fn singletenant() {
        for label in LABELS {
            assert_eq!(
                insert_gateway_label("acme.singletenant.getdbt.com", label).unwrap(),
                format!("{label}.acme.singletenant.getdbt.com")
            );
        }
    }

    #[test]
    fn devspace() {
        for label in LABELS {
            assert_eq!(
                insert_gateway_label("ns.dev.dbt.com", label).unwrap(),
                format!("ns.{label}.dev.dbt.com")
            );
        }
    }

    #[test]
    fn vanity() {
        for label in LABELS {
            assert_eq!(
                insert_gateway_label("pv117.us1.dbt.com", label).unwrap(),
                format!("pv117.{label}.us1.dbt.com")
            );
        }
    }

    #[test]
    fn strips_scheme() {
        assert_eq!(
            insert_gateway_label("https://cloud.getdbt.com", "dwg").unwrap(),
            "dwg.cloud.getdbt.com"
        );
        assert_eq!(
            insert_gateway_label("https://emea.dbt.com", "semantic-layer").unwrap(),
            "semantic-layer.emea.dbt.com"
        );
    }

    #[test]
    fn already_has_label_is_unchanged() {
        // insert_gateway_label is idempotent: if the label is already
        // present, it's returned unchanged instead of being inserted again.
        assert_eq!(
            insert_gateway_label("acme.dwg.dbt.com", "dwg").unwrap(),
            "acme.dwg.dbt.com"
        );
        assert_eq!(
            insert_gateway_label("eq165.semantic-layer.us1.dbt.com", "semantic-layer").unwrap(),
            "eq165.semantic-layer.us1.dbt.com"
        );
    }

    #[test]
    fn unrecognized_host_errors() {
        // Too few labels to insert into: `dbt.com` alone has no account/region prefix.
        assert!(insert_gateway_label("dbt.com", "dwg").is_err());
        // Doesn't end in `dbt.com` or `getdbt.com` at all.
        assert!(insert_gateway_label("example.com", "dwg").is_err());
    }
}
