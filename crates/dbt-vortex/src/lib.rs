use std::sync::{LazyLock, Mutex};

use prost::Message;

use vortex_client::client::ProducerError;
use vortex_client::client::VortexProducerClient;
use vortex_client::client::WorkerThread;
use vortex_client::env::DefaultVortexEnv;

static WORKER_THREAD: Mutex<WorkerThread> = Mutex::new(WorkerThread::empty());

static PRODUCER: LazyLock<VortexProducerClient> = LazyLock::new(|| {
    let env = DefaultVortexEnv::new("fusion", env!("CARGO_PKG_VERSION"));
    let mut client = VortexProducerClient::from_env(&env);
    let handle = client.take_thread_handle();
    debug_assert!(
        client.is_in_dev_mode() || handle.is_some(),
        "Worker thread must be spawned by VortexProducerClient::from_env()"
    );
    let mut lock = WORKER_THREAD.lock().unwrap();
    *lock = handle;
    client
});

/// Main entrypoint for logging messages to Vortex.
///
/// Caller should ignore the return error. This function is non-blocking in production
/// and only returns an error when the client is in dev-mode logging to a file.
#[inline(always)]
pub fn log_proto<T: Message + prost::Name + serde::Serialize>(
    message: T,
) -> Result<(), ProducerError> {
    PRODUCER.log_proto(message) // can only fail in dev mode
}

/// Logs the last message to Vortex and shuts down the client.
pub fn log_proto_and_shutdown<T: Message + prost::Name + serde::Serialize>(
    shutdown_message: T,
) -> Result<(), ProducerError> {
    PRODUCER.log_proto_and_shutdown(&WORKER_THREAD, shutdown_message)
}

pub fn vortex_producer_is_running() -> bool {
    let lock = WORKER_THREAD.lock().unwrap();
    lock.is_some()
}
