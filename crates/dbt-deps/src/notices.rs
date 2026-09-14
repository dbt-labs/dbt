use std::collections::HashSet;
use std::sync::Mutex;

use dbt_common::ErrorCode;
use dbt_common::tracing::dbt_emit::{emit_info_log_message, emit_warn_log_message};
use dbt_schemas::schemas::packages::{DbtPackageEntry, DbtPackages, DbtPackagesLock};

use crate::types::HubPinnedPackage;
use crate::utils::scrub_package_name_secret_env_vars;

const NEXTEST_ENV: &str = "NEXTEST";
const TEST_DEPS_LATEST_VERSION_ENV: &str = "TEST_DEPS_LATEST_VERSION";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum VersionCompatKind {
    Fusion(String),
    RequireDbtVersion { required: String, current: String },
}

/// Payload for a buffered deps notice; package / scrubbed / duplicate label lives in
/// [`PackageNotice::key`] so hub variants don't repeat `package`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PackageNoticeKind {
    HubRedirect {
        redirect_namespace: Option<String>,
        redirect_name: Option<String>,
    },
    HubDeprecated,
    HubVersionCompat(VersionCompatKind),
    HubUpdateAvailable {
        version: String,
        latest: String,
    },
    ScrubbedPackageName,
    DuplicatePackageName,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PackageNotice {
    pub(crate) key: String,
    pub(crate) kind: PackageNoticeKind,
}

impl PackageNoticeKind {
    fn category(&self) -> NoticeCategory {
        match self {
            PackageNoticeKind::HubRedirect { .. } | PackageNoticeKind::HubDeprecated => {
                NoticeCategory::HubDeprecation
            }
            PackageNoticeKind::HubVersionCompat(_) => NoticeCategory::HubVersionCompat,
            PackageNoticeKind::HubUpdateAvailable { .. } => NoticeCategory::HubUpdateAvailable,
            PackageNoticeKind::ScrubbedPackageName => NoticeCategory::Scrubbed,
            PackageNoticeKind::DuplicatePackageName => NoticeCategory::Duplicate,
        }
    }
}

impl PackageNotice {
    fn sort_key(&self) -> (u8, &str) {
        let ord = match &self.kind {
            PackageNoticeKind::HubRedirect { .. } => 0,
            PackageNoticeKind::HubDeprecated => 1,
            PackageNoticeKind::HubVersionCompat(_) => 2,
            PackageNoticeKind::HubUpdateAvailable { .. } => 3,
            PackageNoticeKind::ScrubbedPackageName => 4,
            PackageNoticeKind::DuplicatePackageName => 5,
        };
        (ord, self.key.as_str())
    }

    fn hub_package_key(&self) -> Option<&str> {
        match &self.kind {
            PackageNoticeKind::HubRedirect { .. }
            | PackageNoticeKind::HubDeprecated
            | PackageNoticeKind::HubVersionCompat(_)
            | PackageNoticeKind::HubUpdateAvailable { .. } => Some(self.key.as_str()),
            PackageNoticeKind::ScrubbedPackageName | PackageNoticeKind::DuplicatePackageName => {
                None
            }
        }
    }

    fn category(&self) -> NoticeCategory {
        self.kind.category()
    }

    pub(crate) fn emit(&self) {
        let package = self.key.as_str();
        match &self.kind {
            PackageNoticeKind::HubRedirect {
                redirect_namespace,
                redirect_name,
            } => {
                let msg = match (redirect_namespace.as_ref(), redirect_name.as_ref()) {
                    (Some(ns), Some(name)) => format!(
                        "Package '{package}' has been moved to '{ns}/{name}'. Please update your package reference."
                    ),
                    (Some(ns), None) => format!(
                        "Package '{package}' has been moved to namespace '{ns}'. Please update your package reference."
                    ),
                    (None, Some(name)) => format!(
                        "Package '{package}' has been renamed to '{name}'. Please update your package reference."
                    ),
                    (None, None) => return,
                };
                emit_warn_log_message(ErrorCode::PackageRedirectDeprecation, msg);
            }
            PackageNoticeKind::HubDeprecated => {
                emit_warn_log_message(
                    ErrorCode::HubPackageDeprecated,
                    format!(
                        "Package '{package}' has been deprecated. Consider finding an alternative package."
                    ),
                );
            }
            PackageNoticeKind::HubVersionCompat(kind) => {
                let msg = match kind {
                    VersionCompatKind::Fusion(status) => format!(
                        "Package '{package}' may not be compatible with your dbt version: {status}. \
                         Check Package Hub (https://hub.getdbt.com) for compatible versions \
                         or contact the package maintainer."
                    ),
                    VersionCompatKind::RequireDbtVersion { required, current } => format!(
                        "Package '{package}' requires dbt version {required}, but current version is {current}. \
                         This package may not be compatible with your dbt version."
                    ),
                };
                emit_warn_log_message(ErrorCode::PackageVersionMismatch, msg);
            }
            PackageNoticeKind::HubUpdateAvailable { version, latest } => {
                emit_info_log_message(format!(
                    "Updated version available for {package}@{version}: {latest}"
                ));
            }
            PackageNoticeKind::ScrubbedPackageName => {
                emit_warn_log_message(
                    ErrorCode::DepsScrubbedPackageName,
                    format!(
                        "Detected secret env var in {package}. dbt will write a scrubbed representation to the lock file. This will cause issues with subsequent 'dbt deps' using the lock file, requiring 'dbt deps --upgrade'"
                    ),
                );
            }
            PackageNoticeKind::DuplicatePackageName => {
                emit_warn_log_message(
                    ErrorCode::DepsDuplicatePackage,
                    format!(
                        "Duplicate package name '{package}' found in dependencies. Keeping the first occurrence. \
                     This will be an error in a future version of dbt."
                    ),
                );
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NoticeCategory {
    HubDeprecation,
    HubVersionCompat,
    HubUpdateAvailable,
    Scrubbed,
    Duplicate,
}

/// Per-run gating for what notices the buffer accepts. Applied at
/// [`NoticeBuffer::record`] so dropped notices are never stored.
#[derive(Debug, Clone, Copy)]
pub(crate) struct EmitPolicy {
    pub version_check: bool,
    pub is_test: bool,
    pub test_warnings_enabled: bool,
}

impl Default for EmitPolicy {
    fn default() -> Self {
        Self {
            version_check: true,
            is_test: false,
            test_warnings_enabled: false,
        }
    }
}

impl EmitPolicy {
    pub(crate) fn from_inputs(version_check: bool) -> Self {
        let (is_test, test_warnings_enabled) = if cfg!(debug_assertions) {
            (
                std::env::var(NEXTEST_ENV).is_ok(),
                std::env::var(TEST_DEPS_LATEST_VERSION_ENV).is_ok(),
            )
        } else {
            (false, false)
        };
        Self {
            version_check,
            is_test,
            test_warnings_enabled,
        }
    }

    fn admits(&self, category: NoticeCategory) -> bool {
        match category {
            NoticeCategory::HubVersionCompat => self.version_check && !self.is_test,
            NoticeCategory::HubUpdateAvailable => !self.is_test || self.test_warnings_enabled,
            NoticeCategory::HubDeprecation
            | NoticeCategory::Scrubbed
            | NoticeCategory::Duplicate => true,
        }
    }
}

#[derive(Default)]
pub struct NoticeBuffer {
    inner: Mutex<Vec<PackageNotice>>,
    policy: EmitPolicy,
}

impl NoticeBuffer {
    pub(crate) fn new(policy: EmitPolicy) -> Self {
        Self {
            inner: Mutex::new(Vec::new()),
            policy,
        }
    }

    pub(crate) fn record(&self, notice: PackageNotice) {
        if !self.policy.admits(notice.category()) {
            return;
        }
        self.inner.lock().unwrap().push(notice);
    }

    /// Pull every notice this source produces into the buffer. Single entry
    /// for any type that knows what notices it owns.
    pub(crate) fn collect<S: NoticeSource>(&self, source: &S) {
        source.record_into(self);
    }

    pub(crate) fn drain(&self) -> Vec<PackageNotice> {
        std::mem::take(&mut *self.inner.lock().unwrap())
    }
}

/// Implemented by any type that knows what notices it produces. Pair with
/// [`NoticeBuffer::collect`]; recording stays on the buffer side.
pub(crate) trait NoticeSource {
    fn record_into(&self, buffer: &NoticeBuffer);
}

impl NoticeSource for DbtPackages {
    fn record_into(&self, buffer: &NoticeBuffer) {
        for entry in &self.packages {
            let url = match entry {
                DbtPackageEntry::Git(g) => Some(g.git.as_str()),
                DbtPackageEntry::Tarball(t) => Some(t.tarball.as_str()),
                _ => None,
            };
            if let Some(url) = url
                && let Some(scrubbed) = scrub_package_name_secret_env_vars(url)
            {
                buffer.record(PackageNotice {
                    key: scrubbed.into_owned(),
                    kind: PackageNoticeKind::ScrubbedPackageName,
                });
            }
        }
    }
}

impl NoticeSource for HubPinnedPackage {
    fn record_into(&self, buffer: &NoticeBuffer) {
        if self.version != self.version_latest {
            buffer.record(PackageNotice {
                key: self.name.clone(),
                kind: PackageNoticeKind::HubUpdateAvailable {
                    version: self.version.clone(),
                    latest: self.version_latest.clone(),
                },
            });
        }
    }
}

/// Filter by lock membership, sort and dedup by `(category, key)`. Policy
/// gating happened at record time. Returned `Vec` is in emit order.
pub(crate) fn prepare_for_emit(
    notices: Vec<PackageNotice>,
    lock: &DbtPackagesLock,
) -> Vec<PackageNotice> {
    let in_lock: HashSet<String> = lock.packages.iter().map(|p| p.package_name()).collect();

    let mut filtered: Vec<PackageNotice> = notices
        .into_iter()
        .filter(|n| match n.hub_package_key() {
            Some(pkg) => in_lock.contains(pkg),
            None => true,
        })
        .collect();

    filtered.sort_by(|a, b| a.sort_key().cmp(&b.sort_key()));
    filtered.dedup_by(|a, b| a.sort_key() == b.sort_key());
    filtered
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_schemas::schemas::packages::{DbtPackageLock, HubPackageLock, PackageVersion};

    fn hub_entry(name: &str) -> DbtPackageLock {
        DbtPackageLock::Hub(HubPackageLock {
            package: name.to_string(),
            name: name.to_string(),
            version: PackageVersion::String("1.0.0".to_string()),
        })
    }

    fn lock_with(names: &[&str]) -> DbtPackagesLock {
        DbtPackagesLock {
            packages: names.iter().map(|n| hub_entry(n)).collect(),
            sha1_hash: String::new(),
        }
    }

    #[test]
    fn dedups_same_category_and_package() {
        let lock = lock_with(&["dbt_date"]);
        let notices = vec![
            PackageNotice {
                key: "dbt_date".into(),
                kind: PackageNoticeKind::HubRedirect {
                    redirect_namespace: Some("godatadriven".into()),
                    redirect_name: Some("dbt_date".into()),
                },
            },
            PackageNotice {
                key: "dbt_date".into(),
                kind: PackageNoticeKind::HubRedirect {
                    redirect_namespace: Some("godatadriven".into()),
                    redirect_name: Some("dbt_date".into()),
                },
            },
        ];
        let out = prepare_for_emit(notices, &lock);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn keeps_different_categories_for_same_package() {
        let lock = lock_with(&["pkg"]);
        let notices = vec![
            PackageNotice {
                key: "pkg".into(),
                kind: PackageNoticeKind::HubRedirect {
                    redirect_namespace: Some("ns".into()),
                    redirect_name: None,
                },
            },
            PackageNotice {
                key: "pkg".into(),
                kind: PackageNoticeKind::HubDeprecated,
            },
            PackageNotice {
                key: "pkg".into(),
                kind: PackageNoticeKind::HubUpdateAvailable {
                    version: "1.0.0".into(),
                    latest: "1.1.0".into(),
                },
            },
        ];
        let out = prepare_for_emit(notices, &lock);
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn filters_hub_notices_for_packages_not_in_lock() {
        let lock = lock_with(&["kept"]);
        let notices = vec![
            PackageNotice {
                key: "kept".into(),
                kind: PackageNoticeKind::HubDeprecated,
            },
            PackageNotice {
                key: "dropped".into(),
                kind: PackageNoticeKind::HubDeprecated,
            },
        ];
        let out = prepare_for_emit(notices, &lock);
        assert_eq!(out.len(), 1);
        match &out[0].kind {
            PackageNoticeKind::HubDeprecated => assert_eq!(out[0].key, "kept"),
            _ => panic!(),
        }
    }

    #[test]
    fn scrubbed_and_duplicate_bypass_lock_filter() {
        let lock = lock_with(&[]);
        let notices = vec![
            PackageNotice {
                key: "git+https://x.com/y".into(),
                kind: PackageNoticeKind::ScrubbedPackageName,
            },
            PackageNotice {
                key: "dup".into(),
                kind: PackageNoticeKind::DuplicatePackageName,
            },
        ];
        let out = prepare_for_emit(notices, &lock);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn record_drops_notices_the_policy_rejects() {
        let policy = EmitPolicy {
            version_check: false,
            ..EmitPolicy::default()
        };
        let buf = NoticeBuffer::new(policy);
        buf.record(PackageNotice {
            key: "pkg".into(),
            kind: PackageNoticeKind::HubVersionCompat(VersionCompatKind::RequireDbtVersion {
                required: ">=1.0".into(),
                current: "0.9".into(),
            }),
        });
        buf.record(PackageNotice {
            key: "pkg".into(),
            kind: PackageNoticeKind::HubDeprecated,
        });
        let stored = buf.drain();
        assert_eq!(stored.len(), 1);
        assert!(matches!(stored[0].kind, PackageNoticeKind::HubDeprecated));
    }

    #[test]
    fn is_test_drops_version_compat_and_update_available_but_not_deprecation() {
        let policy = EmitPolicy {
            is_test: true,
            ..EmitPolicy::default()
        };
        let buf = NoticeBuffer::new(policy);
        buf.record(PackageNotice {
            key: "pkg".into(),
            kind: PackageNoticeKind::HubVersionCompat(VersionCompatKind::RequireDbtVersion {
                required: ">=1.0".into(),
                current: "0.9".into(),
            }),
        });
        buf.record(PackageNotice {
            key: "pkg".into(),
            kind: PackageNoticeKind::HubUpdateAvailable {
                version: "1.0.0".into(),
                latest: "1.1.0".into(),
            },
        });
        buf.record(PackageNotice {
            key: "pkg".into(),
            kind: PackageNoticeKind::HubDeprecated,
        });
        let stored = buf.drain();
        assert_eq!(stored.len(), 1);
        assert!(matches!(stored[0].kind, PackageNoticeKind::HubDeprecated));
    }

    #[test]
    fn test_warnings_enabled_re_admits_update_available() {
        let policy = EmitPolicy {
            is_test: true,
            test_warnings_enabled: true,
            ..EmitPolicy::default()
        };
        let buf = NoticeBuffer::new(policy);
        buf.record(PackageNotice {
            key: "pkg".into(),
            kind: PackageNoticeKind::HubUpdateAvailable {
                version: "1.0.0".into(),
                latest: "1.1.0".into(),
            },
        });
        assert_eq!(buf.drain().len(), 1);
    }

    #[test]
    fn deterministic_order_across_categories() {
        let lock = lock_with(&["a", "b"]);
        let notices = vec![
            PackageNotice {
                key: "a".into(),
                kind: PackageNoticeKind::DuplicatePackageName,
            },
            PackageNotice {
                key: "b".into(),
                kind: PackageNoticeKind::HubUpdateAvailable {
                    version: "1.0".into(),
                    latest: "1.1".into(),
                },
            },
            PackageNotice {
                key: "url".into(),
                kind: PackageNoticeKind::ScrubbedPackageName,
            },
            PackageNotice {
                key: "a".into(),
                kind: PackageNoticeKind::HubDeprecated,
            },
            PackageNotice {
                key: "b".into(),
                kind: PackageNoticeKind::HubRedirect {
                    redirect_namespace: Some("ns".into()),
                    redirect_name: None,
                },
            },
        ];
        let out = prepare_for_emit(notices, &lock);
        let categories: Vec<u8> = out.iter().map(|n| n.sort_key().0).collect();
        assert_eq!(categories, vec![0, 1, 3, 4, 5]);
    }

    #[test]
    fn record_is_thread_safe() {
        use std::sync::Arc;
        let buf = Arc::new(NoticeBuffer::default());
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let b = buf.clone();
                std::thread::spawn(move || {
                    b.record(PackageNotice {
                        key: format!("pkg{i}"),
                        kind: PackageNoticeKind::DuplicatePackageName,
                    });
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(buf.drain().len(), 8);
    }
}
