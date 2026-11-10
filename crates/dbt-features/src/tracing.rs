use std::sync::Arc;
use std::sync::atomic::AtomicPtr;

use dbt_common::tracing::TelemetryHandle;
use dbt_common::tracing::TracingConfigProvider;
use dbt_common::tracing::error::TracingError;
use dbt_common::tracing::noop_tracing_config_provider;

pub struct TracingFeature {
    pub config_provider: Arc<dyn TracingConfigProvider>,
    shutdown_handle: AtomicPtr<TelemetryHandle>,
}

impl Default for TracingFeature {
    fn default() -> Self {
        Self {
            config_provider: noop_tracing_config_provider().into(),
            shutdown_handle: AtomicPtr::new(std::ptr::null_mut()),
        }
    }
}

impl TracingFeature {
    pub fn with_config_provider(mut self, provider: Arc<dyn TracingConfigProvider>) -> Self {
        self.config_provider = provider;
        self
    }

    pub fn with_shutdown_handle(self, handle: TelemetryHandle) -> Self {
        self.shutdown_handle.store(
            Box::into_raw(Box::new(handle)),
            std::sync::atomic::Ordering::SeqCst,
        );
        self
    }

    pub fn shutdown_once(&self) -> Result<(), Vec<TracingError>> {
        let handle_ptr = self
            .shutdown_handle
            .swap(std::ptr::null_mut(), std::sync::atomic::Ordering::SeqCst);
        if !handle_ptr.is_null() {
            let handle = unsafe { Box::from_raw(handle_ptr) };
            handle.shutdown_once()
        } else {
            Ok(())
        }
    }
}
