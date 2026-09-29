use std::path::PathBuf;
use std::sync::RwLock;

use crate::warn_error_options::WarnErrorOptions;

/// Serves as a handle to tracing configuration that is mutable at runtime.
pub trait TracingConfigProvider: Send + Sync {
    fn set_warn_error_options(&self, warn_error_options: WarnErrorOptions);

    /// Call `f` with the configured [WarnErrorOptions].
    fn with_warn_error_options(&self, f: &mut dyn FnMut(&WarnErrorOptions));

    fn set_file_log_path(&self, file_log_path: Option<PathBuf>);
    fn get_file_log_path(&self) -> Option<PathBuf>;
}

struct NoOpTracingConfigProvider;

impl TracingConfigProvider for NoOpTracingConfigProvider {
    fn set_warn_error_options(&self, _warn_error_options: WarnErrorOptions) {}

    fn with_warn_error_options(&self, f: &mut dyn FnMut(&WarnErrorOptions)) {
        f(&WarnErrorOptions::default());
    }

    fn set_file_log_path(&self, _file_log_path: Option<PathBuf>) {}

    fn get_file_log_path(&self) -> Option<PathBuf> {
        None
    }
}

pub fn noop_tracing_config_provider() -> Box<dyn TracingConfigProvider> {
    Box::new(NoOpTracingConfigProvider)
}

// TODO: move to dbt-features or incorporate it into a bigger struct
#[derive(Default)]
pub struct FsTracingConfigProvider {
    pub warn_error_options: RwLock<WarnErrorOptions>,
    pub file_log_path: RwLock<Option<PathBuf>>,
}

impl FsTracingConfigProvider {
    pub fn from_warn_error_options(warn_error_options: WarnErrorOptions) -> Self {
        Self {
            warn_error_options: RwLock::new(warn_error_options),
            ..Default::default()
        }
    }
}

impl TracingConfigProvider for FsTracingConfigProvider {
    fn set_warn_error_options(&self, warn_error_options: WarnErrorOptions) {
        *self
            .warn_error_options
            .write()
            .expect("warn_error_options lock should not be poisoned") = warn_error_options;
    }

    fn with_warn_error_options(&self, f: &mut dyn FnMut(&WarnErrorOptions)) {
        let options = self
            .warn_error_options
            .read()
            .expect("warn_error_options lock should not be poisoned");
        f(&options);
    }

    fn set_file_log_path(&self, file_log_path: Option<PathBuf>) {
        let mut write = self.file_log_path.write().unwrap();
        *write = file_log_path;
    }

    fn get_file_log_path(&self) -> Option<PathBuf> {
        let read = self.file_log_path.read().unwrap();
        read.to_owned()
    }
}
