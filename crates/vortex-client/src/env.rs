//! Host-supplied configuration for the Vortex producer.

use std::env;
use std::ffi::OsStr;
use std::path::PathBuf;

pub(crate) const DEFAULT_BASE_URL: &str = "https://p.vx.dbt.com";
pub(crate) const DEFAULT_INGEST_ENDPOINT: &str = "/v1/ingest/protobuf";
pub(crate) const DEFAULT_DEV_MODE_OUTPUT_PATH: &str = "/tmp/vortex_dev_mode_output.jsonl";

const TRUE_VALUES: [&str; 4] = ["1", "true", "yes", "on"];

fn is_true_value(value: &OsStr) -> bool {
    TRUE_VALUES.iter().any(|v| value.eq_ignore_ascii_case(v))
}

/// Everything the producer needs to know about the process it runs in.
///
/// Only the two identity methods are required; the rest default to the `VORTEX_*`
/// environment variables, so a host that just needs to name itself can use
/// [`StdVortexEnv`].
pub trait VortexEnv: Send + Sync {
    /// Attributes events to a host in the `X-Vortex-Client-Platform` header,
    /// e.g. `"fusion"`. Must not vary between invocations.
    fn service_name(&self) -> &str;

    fn service_version(&self) -> &str;

    fn base_url(&self) -> String {
        env::var("VORTEX_BASE_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.to_string())
    }

    fn ingest_endpoint(&self) -> String {
        env::var("VORTEX_INGEST_ENDPOINT").unwrap_or_else(|_| DEFAULT_INGEST_ENDPOINT.to_string())
    }

    /// Appends events to a local file instead of sending them. True for any of
    /// `1`, `true`, `yes`, `on` (case-insensitive).
    fn dev_mode(&self) -> bool {
        env::var_os("VORTEX_DEV_MODE").is_some_and(|v| is_true_value(&v))
    }

    fn dev_mode_output_path(&self) -> PathBuf {
        env::var("VORTEX_DEV_MODE_OUTPUT_PATH")
            .unwrap_or_else(|_| DEFAULT_DEV_MODE_OUTPUT_PATH.to_string())
            .into()
    }

    /// `None` leaves `ureq`'s own configuration in place. Hosts that must use the
    /// OS certificate store return a `RootCerts::PlatformVerifier` config.
    fn tls_config(&self) -> Option<ureq::tls::TlsConfig> {
        None
    }
}

/// A [`VortexEnv`] that names its host and takes every other value from the
/// environment.
#[derive(Debug, Clone)]
pub struct DefaultVortexEnv {
    service_name: String,
    service_version: String,
}

impl DefaultVortexEnv {
    pub fn new(service_name: impl Into<String>, service_version: impl Into<String>) -> Self {
        Self {
            service_name: service_name.into(),
            service_version: service_version.into(),
        }
    }
}

impl VortexEnv for DefaultVortexEnv {
    fn service_name(&self) -> &str {
        &self.service_name
    }

    fn service_version(&self) -> &str {
        &self.service_version
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FullyOverriddenEnv;

    impl VortexEnv for FullyOverriddenEnv {
        fn service_name(&self) -> &str {
            "test-host"
        }
        fn service_version(&self) -> &str {
            "1.2.3"
        }
        fn base_url(&self) -> String {
            "http://127.0.0.1:8083".to_string()
        }
        fn ingest_endpoint(&self) -> String {
            "/internal/v1/ingest/protobuf".to_string()
        }
        fn dev_mode(&self) -> bool {
            true
        }
        fn dev_mode_output_path(&self) -> PathBuf {
            PathBuf::from("/dev/null")
        }
    }

    #[test]
    fn test_overrides_are_honored() {
        let env: &dyn VortexEnv = &FullyOverriddenEnv;
        assert_eq!(env.service_name(), "test-host");
        assert_eq!(env.service_version(), "1.2.3");
        assert_eq!(env.base_url(), "http://127.0.0.1:8083");
        assert_eq!(env.ingest_endpoint(), "/internal/v1/ingest/protobuf");
        assert!(env.dev_mode());
        assert_eq!(env.dev_mode_output_path(), PathBuf::from("/dev/null"));
    }

    #[test]
    fn test_dev_mode_is_parsed_case_insensitively() {
        for value in ["1", "true", "TRUE", "True", "yes", "on", "ON"] {
            assert!(
                is_true_value(OsStr::new(value)),
                "{value:?} should enable dev mode"
            );
        }
        for value in ["", "0", "false", "False", "no", "off", "nonsense"] {
            assert!(
                !is_true_value(OsStr::new(value)),
                "{value:?} should not enable dev mode"
            );
        }
    }

    /// The only test in the crate that touches the environment: keep it that way
    /// so it cannot race with a sibling test sharing the process.
    #[test]
    fn test_defaults_read_the_environment() {
        let env = DefaultVortexEnv::new("test-host", "1.2.3");

        // SAFETY: no sibling test reads these variables.
        unsafe {
            env::remove_var("VORTEX_BASE_URL");
            env::remove_var("VORTEX_INGEST_ENDPOINT");
            env::remove_var("VORTEX_DEV_MODE");
            env::remove_var("VORTEX_DEV_MODE_OUTPUT_PATH");
        }
        assert_eq!(env.base_url(), DEFAULT_BASE_URL);
        assert_eq!(env.ingest_endpoint(), DEFAULT_INGEST_ENDPOINT);
        assert!(!env.dev_mode());
        assert_eq!(
            env.dev_mode_output_path(),
            PathBuf::from(DEFAULT_DEV_MODE_OUTPUT_PATH)
        );

        // SAFETY: as above. The `set_var` lint targets stray `GOLDIE_UPDATE`
        // assignments; these are the producer's own variables.
        #[allow(clippy::disallowed_methods)]
        unsafe {
            env::set_var("VORTEX_BASE_URL", "http://localhost:9999");
            env::set_var("VORTEX_INGEST_ENDPOINT", "/ingest");
            env::set_var("VORTEX_DEV_MODE", "true");
            env::set_var("VORTEX_DEV_MODE_OUTPUT_PATH", "/tmp/events.jsonl");
        }
        assert_eq!(env.base_url(), "http://localhost:9999");
        assert_eq!(env.ingest_endpoint(), "/ingest");
        assert!(env.dev_mode());
        assert_eq!(
            env.dev_mode_output_path(),
            PathBuf::from("/tmp/events.jsonl")
        );
    }
}
