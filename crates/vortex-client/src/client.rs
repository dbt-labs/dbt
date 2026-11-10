//! Client for sending messages to Vortex.
//!
//! A host builds a [`VortexProducerClient`] from its own [`VortexEnv`]. Hosts that
//! only need one can install their environment with [`init`] and use the free
//! [`log_proto`] function instead of holding the producer themselves.
//!
//! The default [`VortexEnv`] implementation reads these environment variables:
//! - `VORTEX_BASE_URL` (default `https://p.vx.dbt.com`): The base URL of the Vortex API.
//! - `VORTEX_INGEST_ENDPOINT` (default `/v1/ingest/protobuf`): The ingest endpoint of
//!   the Vortex API.
//! - `VORTEX_DEV_MODE` (default `false`): If true, the producer will write to a file instead of
//!   sending messages to an API endpoint.
//! - `VORTEX_DEV_MODE_OUTPUT_PATH` (default `/tmp/vortex_dev_mode_output.jsonl`): The path to the
//!   file where the producer will write the messages.  Note that this file will be appended to, not
//!   overwritten, so it can grow quite large!
use std::any::Any;
use std::fs;
use std::io::{self, Write as _};
use std::path::PathBuf;
use std::sync::{Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use http::HeaderValue;
use pbjson_types::Timestamp;
use prost::Message;
use proto_rust::v1::events::vortex::{VortexMessage, VortexMessageBatch};

use crate::env::VortexEnv;

#[cfg(test)]
use std::println as trace;
#[cfg(not(test))]
use tracing::trace;

const DEFAULT_FLUSH_INTERVAL: Duration = Duration::from_millis(500);
const DEFAULT_TARGET_BATCH_SIZE_BYTES: usize = 1024; // 1kb batches
const DEFAULT_BODY_SIZE_LIMIT_BYTES: usize = 4 * 1024 * 1024; // 4mb body size limit
const MAX_ENCODED_MESSAGE_SIZE_BYTES: usize = 2 * 1024 * 1024; // 2mb
const MIN_BACKOFF_MILLIS: u64 = 200;
const MAX_BACKOFF_MILLIS: u64 = 30_000;

#[cfg(debug_assertions)]
const LOG_PROTO_SHUTDOWN_MESSAGE: &str = "You're trying to log a message via \
Vortex, but the client is already shut down. This should be fixed, but on release \
builds the message will simply be dropped.";

#[derive(Debug)]
pub enum ProducerError {
    /// The client is in dev mode and cannot write messages to the log file.
    DevModeError(io::Error),
    /// Communication error with the Vortex HTTP endpoint.
    SendError(ureq::Error),
    /// Failed to join the worker thread during shutdown.
    ShutdownError(Box<dyn Any + Send + 'static>),
}

impl std::fmt::Display for ProducerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProducerError::DevModeError(e) => write!(f, "Vortex: dev mode error: {e}"),
            ProducerError::SendError(e) => write!(f, "Vortex: send error: {e}"),
            ProducerError::ShutdownError(e) => write!(f, "Vortex: shutdown error: {e:?}"),
        }
    }
}

impl std::error::Error for ProducerError {}

struct Batch {
    // configuration and the RNG
    target_batch_size: usize,
    body_size_limit: usize,
    flush_interval: Duration,
    rng: rand::rngs::ThreadRng,
    // mutable state
    messages: Vec<VortexMessage>,
    bytes_buffered: usize,
    last_error: Option<ureq::Error>,
    deadline: Option<Instant>,
    num_attempts: usize,
}

impl Batch {
    fn new(target_batch_size: usize, body_size_limit: usize, flush_interval: Duration) -> Self {
        debug_assert!(
            target_batch_size > 0,
            "target_batch_size must be greater than 0"
        );
        debug_assert!(
            body_size_limit >= 2 * target_batch_size,
            "body_size_limit must be much larger than target_batch_size"
        );
        Self {
            target_batch_size,
            body_size_limit,
            flush_interval,
            rng: rand::rng(),
            messages: Vec::new(),
            bytes_buffered: 0,
            last_error: None,
            deadline: None, // INVARIANT: deadline.is_none() implies is_empty()
            num_attempts: 0,
        }
    }

    fn for_agent(agent: &dyn SenderAgent) -> Self {
        Self::new(
            agent.target_batch_size(),
            agent.body_size_limit(),
            agent.flush_interval(),
        )
    }

    fn push(&mut self, message: Box<VortexMessage>) {
        let encoded_len = message.encoded_len();
        if encoded_len > self.body_size_limit.min(MAX_ENCODED_MESSAGE_SIZE_BYTES) {
            debug_assert!(
                false,
                "{} message is too large ({} bytes).",
                message
                    .any
                    .as_ref()
                    .map_or_else(|| "unknown", |a| a.type_url.as_str()),
                encoded_len
            );
            return; // silently drop the large message in release builds
        }
        if self.deadline.is_none() {
            debug_assert!(
                self.messages.is_empty(),
                "If deadline is None, messages must be empty"
            );
            // Set a deadline when the first message is added to the batch.
            self.deadline = Some(Instant::now() + self.flush_interval);
        }
        self.bytes_buffered += encoded_len;
        self.messages.push(*message);
    }

    fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    fn is_full(&self) -> bool {
        self.bytes_buffered >= self.target_batch_size
    }

    fn is_overdue(&self) -> bool {
        let overdue = self
            .deadline
            .map(|deadline| Instant::now() >= deadline)
            .unwrap_or(false);
        overdue || {
            // Even if the batch is not overdue for sending, we can still
            // send if it's full and we are not retrying with backoff.
            self.is_full() && self.num_attempts == 0
        }
    }

    fn clear(&mut self) -> Result<(), ureq::Error> {
        self.messages.clear();
        self.bytes_buffered = 0;
        let res = match self.last_error.take() {
            Some(error) => Err(error),
            None => Ok(()),
        };
        self.deadline = None;
        self.num_attempts = 0;
        res
    }

    fn clear_after_success(&mut self) {
        let _ = self.clear();
    }

    /// Adjust the deadline for the next retry with exponential backoff.
    fn backoff(&mut self) {
        use rand::Rng as _;
        // backoff() is called after a failed send operation, so
        // we should have at least one message in the batch.
        debug_assert!(!self.is_empty());
        // Exponential backoff with jitter: MIN_BACKOFF_MILLIS * 2**num_attempts + jitter.
        let backoff_millis = MIN_BACKOFF_MILLIS
            .saturating_mul(1u64 << self.num_attempts.min(63))
            .saturating_add(self.rng.random_range(0..=MIN_BACKOFF_MILLIS))
            .min(MAX_BACKOFF_MILLIS);
        let backoff = Duration::from_millis(backoff_millis);
        self.deadline = Some(Instant::now() + backoff);
        self.num_attempts += 1;
    }

    fn prune(&mut self) {
        debug_assert!(
            self.messages.len() >= 2,
            "prune: batch must have at least 2 messages to prune."
        );
        let mut i = 0;
        let mut new_bytes_buffered = self.bytes_buffered;
        while i < self.messages.len() {
            new_bytes_buffered -= self.messages[i].encoded_len();
            if new_bytes_buffered < self.body_size_limit {
                self.messages.drain(0..=i);
                self.bytes_buffered = new_bytes_buffered;
                break;
            } else {
                i += 1;
            }
        }
        // This is a consequence of not letting messages larger than the
        // body_size_limit into the batch in the first place.
        debug_assert!(!self.is_empty(), "batch must not be empty after pruning.");
    }

    fn encode_for_sending(&mut self) -> Vec<u8> {
        debug_assert!(!self.is_empty(), "send_batch: batch must be non-empty.");
        if self.bytes_buffered > self.body_size_limit {
            self.prune();
        }
        // Override the client sent timestamp for each message that goes into the batch.
        let now = VortexProducerClient::current_timestamp();
        self.messages.iter_mut().for_each(|msg| {
            msg.vortex_client_sent_at = Some(now);
        });

        let mut message_batch = VortexMessageBatch {
            request_id: uuid::Uuid::new_v4().to_string(),
            payload: Vec::new(),
        };
        // Consume these messages into a batch's payload and then swap them back to
        // the original messages vector to enable retries from the caller if needed.
        std::mem::swap(&mut self.messages, &mut message_batch.payload);
        let body = message_batch.encode_to_vec();
        std::mem::swap(&mut self.messages, &mut message_batch.payload);
        body
    }

    fn on_error(&mut self, error: ureq::Error) {
        #[allow(clippy::single_match)]
        match error {
            ureq::Error::StatusCode(_) => {
                // Status codes should be handled in `on_response`
                // because `http_status_as_error` is not enabled.
                unreachable!("`http_status_as_error` is not enabled.")
            }
            _ => (),
        }
        self.last_error = Some(error);
        self.backoff();
    }

    fn on_response(&mut self, status: http::StatusCode, text: String) {
        if status.is_success() {
            self.clear_after_success();
            trace!("Successfully sent telemetry batch.");
        } else {
            self.backoff();
            trace!("Failed to send batch of messages: {status}: {text}");
            self.last_error = Some(ureq::Error::StatusCode(status.as_u16()));
        }
    }
}

/// Abstract interface for the HTTP agent that sends batches of messages to the Vortex endpoint.
trait SenderAgent: Send {
    /// Return the threshold for considering a batch full.
    fn target_batch_size(&self) -> usize;

    /// This is the maximum size of the body that can be sent in a single request.
    ///
    /// It should be much larger than the target batch size to accommodate more
    /// messages after a post-failure backoff period. When the batch reaches this size,
    /// messages start being dropped to avoid exceeding the limit.
    fn body_size_limit(&self) -> usize;

    /// The interval after which the batch should be flushed, even if it is not full yet.
    ///
    /// During retries, the backoff period is honored and it may exceed this interval.
    fn flush_interval(&self) -> Duration;

    /// Send a batch of messages to the Vortex endpoint.
    ///
    /// Errors are accumulated in the mutable `Batch` parameter. The batch is
    /// cleared after a successful send, otherwise the batch accumulates errors
    /// and preserves messages to enable retries. `false` is returned when the
    /// send operation failed, and `true` when it succeeded. We don't return
    /// the error directly because the error is consumed by the batch and caller
    /// only needs to know whether the send was successful or not.
    ///
    /// PRECONDITION: !batch.is_empty()
    fn send_batch(&self, batch: &mut Batch) -> bool;
}

struct BatchSenderAgentImpl {
    agent: ureq::Agent,
    endpoint: http::Uri,
    vortex_client_platform: HeaderValue,
}

impl BatchSenderAgentImpl {
    pub fn new(
        endpoint: http::Uri,
        vortex_client_platform: HeaderValue,
        env: &dyn VortexEnv,
    ) -> Self {
        const TIMEOUT_PER_CALL: Duration = Duration::from_secs(30);

        let config = {
            let mut config_builder = ureq::config::Config::builder()
                .http_status_as_error(false)
                .timeout_per_call(Some(TIMEOUT_PER_CALL));
            if let Some(tls_config) = env.tls_config() {
                config_builder = config_builder.tls_config(tls_config);
            }
            config_builder.build()
        };

        let agent = ureq::Agent::new_with_config(config);
        Self {
            agent,
            endpoint,
            vortex_client_platform,
        }
    }
}

impl SenderAgent for BatchSenderAgentImpl {
    fn target_batch_size(&self) -> usize {
        DEFAULT_TARGET_BATCH_SIZE_BYTES
    }

    fn body_size_limit(&self) -> usize {
        DEFAULT_BODY_SIZE_LIMIT_BYTES
    }

    fn flush_interval(&self) -> Duration {
        DEFAULT_FLUSH_INTERVAL
    }

    fn send_batch(&self, batch: &mut Batch) -> bool {
        let body = batch.encode_for_sending();
        let result = self
            .agent
            .post(self.endpoint.clone())
            .header("Content-Type", "application/vnd.google.protobuf")
            .header(
                "X-Vortex-Client-Platform",
                self.vortex_client_platform.clone(),
            )
            .send(body);

        match result {
            Ok(response) => {
                let status = response.status();
                let text = response.into_body().read_to_string().unwrap_or_default();
                batch.on_response(status, text);
                true
            }
            Err(e) => {
                batch.on_error(e);
                false
            }
        }
    }
}

#[derive(Default)]
pub struct WorkerThread {
    join_handle: Option<JoinHandle<Result<(), ureq::Error>>>,
}

impl WorkerThread {
    fn new(join_handle: Option<JoinHandle<Result<(), ureq::Error>>>) -> Self {
        Self { join_handle }
    }

    /// An empty slot, `const` so hosts can hold it in a `static`.
    pub const fn empty() -> Self {
        Self { join_handle: None }
    }

    pub fn is_some(&self) -> bool {
        self.join_handle.is_some()
    }

    pub(crate) fn take(&mut self) -> WorkerThread {
        let join_handle = self.join_handle.take();
        WorkerThread { join_handle }
    }

    /// Call after logging a message with is_shutdown=true to wait for the worker
    /// thread to finish processing messages.
    pub(crate) fn join_on_shutdown(self) -> Result<(), ProducerError> {
        match self.join_handle {
            Some(handle) => match handle.join() {
                Ok(Ok(())) => Ok(()),
                Ok(Err(e)) => Err(ProducerError::SendError(e)),
                Err(e) => Err(ProducerError::ShutdownError(e)),
            },
            None => Ok(()), // dev-mode or already shut down
        }
    }
}

pub struct VortexProducerClient {
    sender: mpsc::Sender<(Box<VortexMessage>, bool)>,
    thread_handle: WorkerThread,
    /// Path to the file where messages will be written in dev mode.
    ///
    /// Only set in development mode. MUST be `None` in production.
    dev_mode_output_path: Option<PathBuf>,
    /// Dev-mode output writer, used to write messages to a file in development mode.
    dev_mode_output_writer: Mutex<Result<fs::File, io::Error>>,
}

impl VortexProducerClient {
    pub fn from_env(env: &dyn VortexEnv) -> Self {
        let endpoint = {
            let base_url = env.base_url();
            let ingest_endpoint = env.ingest_endpoint();
            let full_url = format!("{base_url}{ingest_endpoint}");
            full_url
                .parse::<http::Uri>()
                .expect("Failed to parse Vortex endpoint URL")
        };
        let vortex_client_platform = {
            // Construct the X-Vortex-Client-Platform header with service, client, and proto library
            // information. Format:
            //
            //     {service}/{version} {client}/{version} {proto_library}/{version}
            //
            // This helps identify the client platform and its components for monitoring and debugging.
            let service_name = env.service_name();
            let service_version = env.service_version();
            // TODO: Change this to the actual version of the proto-rust library.
            let proto_version = "unknown";
            #[allow(clippy::uninlined_format_args)]
            let header_value_string = format!(
                "{}/{} {}/{} {}/{}",
                service_name,
                service_version,
                "vortex-client-rust",
                env!("CARGO_PKG_VERSION"),
                "proto-rust",
                proto_version
            );
            HeaderValue::from_str(&header_value_string)
                .expect("Failed to create X-Vortex-Client-Platform header value")
        };
        let dev_mode_output_path = {
            if env.dev_mode() {
                Some(env.dev_mode_output_path())
            } else {
                None
            }
        };
        let agent = BatchSenderAgentImpl::new(endpoint, vortex_client_platform, env);
        let agent: Box<dyn SenderAgent> = Box::new(agent);
        Self::new(agent, dev_mode_output_path)
    }

    fn new(agent: Box<dyn SenderAgent>, dev_mode_output_path: Option<PathBuf>) -> Self {
        let dev_mode_output_writer = if let Some(path) = &dev_mode_output_path {
            match fs::OpenOptions::new().create(true).append(true).open(path) {
                Ok(file) => Mutex::new(Ok(file)),
                Err(e) => Mutex::new(Err(e)),
            }
        } else {
            let e = io::Error::other(
                "Trying to write JSON, but client is not in dev-mode.".to_string(),
            );
            Mutex::new(Err(e))
        };

        let (sender, receiver) = mpsc::channel();

        let mut client = Self {
            sender,
            thread_handle: Default::default(),
            dev_mode_output_path,
            dev_mode_output_writer,
        };
        if !client.is_in_dev_mode() {
            let join_handle = Some(thread::spawn(move || worker_thread_loop(agent, receiver)));
            client.thread_handle = WorkerThread::new(join_handle);
        };
        client
    }

    pub fn take_thread_handle(&mut self) -> WorkerThread {
        debug_assert!(
            self.is_in_dev_mode() || self.thread_handle.is_some(),
            "take_thread_handle() must be called only once."
        );
        self.thread_handle.take()
    }

    pub fn is_in_dev_mode(&self) -> bool {
        self.dev_mode_output_path.is_some()
    }

    #[cfg(not(test))]
    fn current_timestamp() -> Timestamp {
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO);
        Timestamp {
            seconds: now.as_secs() as i64,
            nanos: now.subsec_nanos() as i32,
        }
    }

    #[cfg(test)]
    fn current_timestamp() -> Timestamp {
        Timestamp {
            seconds: 0,
            nanos: 0,
        }
    }

    /// Main entrypoint for logging messages to Vortex.
    ///
    /// Caller should ignore the return error. This function is non-blocking in production
    /// and only returns an error when the client is in dev-mode logging to a file.
    #[inline(always)]
    pub fn log_proto<T: Message + prost::Name + serde::Serialize>(
        &self,
        message: T,
    ) -> Result<(), ProducerError> {
        self.do_log_proto(message, false) // can only fail in dev mode
            .map_err(ProducerError::DevModeError)
    }

    /// Logs the last message to Vortex and shuts down the client.
    pub fn log_proto_and_shutdown<T: Message + prost::Name + serde::Serialize>(
        &self,
        worker_thread: &Mutex<WorkerThread>,
        shutdown_message: T,
    ) -> Result<(), ProducerError> {
        self.do_log_proto(shutdown_message, true) // can only fail in dev mode
            .map_err(ProducerError::DevModeError)?;
        // Wait for the worker thread to finish processing messages. Since a
        // shutdown message has been sent, it will NOT block indefinitely.
        let worker_thread = {
            let mut lock = worker_thread.lock().unwrap();
            lock.take()
        };
        worker_thread.join_on_shutdown()
    }

    #[inline(always)]
    fn do_log_proto<T: Message + prost::Name + serde::Serialize>(
        &self,
        message: T,
        is_shutdown: bool,
    ) -> Result<(), io::Error> {
        if self.is_in_dev_mode() {
            // This code should never run in prod, so using unwrap_or_default to avoid panics.
            let json_value = serde_json::to_value(&message).unwrap_or_default();
            self.do_log_proto_in_dev(T::PACKAGE, T::NAME, json_value)
        } else {
            self.do_log_proto_in_prod(T::PACKAGE, T::NAME, message.encode_to_vec(), is_shutdown);
            Ok(())
        }
    }

    /// Logs a protobuf message to Vortex in development mode.
    ///
    /// This private function is type-erased so it does not have to be specialized
    /// for different message types, thus saving on binary size and compilation time
    /// for every `log_proto` callsite.
    #[inline(never)]
    fn do_log_proto_in_dev(
        &self,
        package: &'static str,
        name: &'static str,
        json_message: serde_json::Value,
    ) -> Result<(), io::Error> {
        /// A message that will be written to the dev-mode output file as JSON.
        #[derive(Debug, Clone, serde::Serialize)]
        struct VortexDevModeMessage {
            type_url: String,
            message: serde_json::Value,
        }
        let dev_mode_msg = VortexDevModeMessage {
            type_url: format!("/{package}.{name}"),
            message: json_message,
        };
        let mut json_payload = serde_json::to_string(&dev_mode_msg).unwrap_or_default();
        json_payload.push('\n');

        let mut writer_res_lock_guard = self.dev_mode_output_writer.lock().unwrap();
        match &mut *writer_res_lock_guard {
            Ok(writer) => writer.write_all(json_payload.as_bytes()),
            Err(e) => {
                // we can't clone io::Error, so we create a custom one that carries the kind
                let e = io::Error::new(e.kind(), e.to_string());
                Err(e)
            }
        }
    }

    /// Logs a protobuf message to Vortex in production mode.
    ///
    /// This private function is type-erased so it does not have to be specialized
    /// for different message types, thus saving on binary size and compilation time
    /// for every `log_proto` callsite.
    #[inline(never)]
    fn do_log_proto_in_prod(
        &self,
        package: &'static str,
        name: &'static str,
        serialized_value: Vec<u8>,
        is_shutdown: bool,
    ) {
        let any = pbjson_types::Any {
            type_url: format!("/{package}.{name}"),
            value: serialized_value.into(),
        };
        let msg = box_any_message(any);
        #[cfg(debug_assertions)]
        if self.sender.send((msg, is_shutdown)).is_err() {
            eprintln!("{LOG_PROTO_SHUTDOWN_MESSAGE} type_url=/{package}.{name}");
        }
        #[cfg(not(debug_assertions))]
        let _ = self.sender.send((msg, is_shutdown));
    }
}

fn box_any_message(any: pbjson_types::Any) -> Box<VortexMessage> {
    let vortex_event_url = any.type_url.clone();
    Box::new(VortexMessage {
        any: Some(any),
        vortex_event_created_at: Some(VortexProducerClient::current_timestamp()),
        vortex_client_sent_at: None,
        vortex_backend_received_at: None,
        vortex_backend_processed_at: None,
        vortex_event_id: uuid::Uuid::new_v4().to_string(),
        vortex_event_url,
    })
}

/// Worker thread loop for processing messages from the channel and sending them in batches.
/// If messages stay in a partial batch for a while, they will be sent even before the
/// full batch is completed to ensure timely delivery. If the batch has messages that are
/// being retried after a failure, a deadline will be honored even if the batch is full.
/// If and only if the batch grows too large during a backoff period, it will be pruned
/// to the HTTP body size limit accepted by the Vortex endpoint.
///
/// IMPORTANT: messages are only dropped if two anomalies occur at the same time:
/// 1) the backend is not responsive for a while and the client is forced to backoff;
/// 2) the number of events logged during the backoff is so large that the batch
///    grows larger than the HTTP body size limit set by the Vortex endpoint. This
///    limit is much higher than the target batch size.
///
///
/// ```
///                ┌───────────────────┐
///                ▼                   │
///    ┌──────────────┐  event or   ┌────────────┐  non-empty  ┌─────────┐
/// ──▶│ WaitForEvent │ ───────────▶│ Processing │ ───────────▶│ Sending │
///    └──────────────┘  timeout    └────────────┘    batch    └─────────┘
///                                    │      ▲                   │
///                                    │      └───────────────────┘
///                                    ▼
///                              ┌────────────┐
///                              │ Terminated │
///                              └────────────┘
/// ```
fn worker_thread_loop(
    agent: Box<dyn SenderAgent>,
    receiver: mpsc::Receiver<(Box<VortexMessage>, bool)>,
) -> Result<(), ureq::Error> {
    enum State {
        Processing,
        Sending,
        WaitForEvent,
        Terminated,
    }
    let next_state = |event: (Option<Box<VortexMessage>>, bool),
                      shutdown_flag: &mut bool,
                      batch: &mut Batch|
     -> State {
        match event {
            (Some(msg), is_shutdown) => {
                *shutdown_flag |= is_shutdown;
                batch.push(msg);
                if batch.is_overdue() {
                    State::Sending
                } else {
                    State::Processing
                }
            }
            (None, is_shutdown) => {
                *shutdown_flag |= is_shutdown;
                if *shutdown_flag {
                    if !batch.is_empty() {
                        State::Sending // Send the *last* batch.
                    } else {
                        State::Terminated // Batch and queue are empty.
                    }
                } else {
                    // Not shutting down, so we take deadline into account.
                    if batch.is_overdue() {
                        State::Sending // Send the *full* or *overdue* batch.
                    } else {
                        // Enter the "WaitForEvent" state: batch is partially filled
                        // and the queue was empty, so we wait for more messages.
                        State::WaitForEvent
                    }
                }
            }
        }
    };

    let mut state = State::Processing;
    let mut shutdown_flag = false;
    let mut batch = Batch::for_agent(agent.as_ref());
    loop {
        match state {
            // "Processing" state: peek the queue to see if there are any messages.
            State::Processing => {
                let event = peek_queue(&receiver);
                state = next_state(event, &mut shutdown_flag, &mut batch);
            }
            State::Sending => {
                let sent = agent.send_batch(&mut batch);
                state = if shutdown_flag && !sent {
                    State::Terminated // If shutting down, give up on first send failure.
                } else {
                    State::Processing // Come back to "Processing" state after "Sending".
                };
            }
            // "WaitForEvent" state: wait for an event or timeout.
            State::WaitForEvent => {
                state = match wait_for_event(&receiver, batch.deadline) {
                    WaitForEvent::Event(event) => next_state(event, &mut shutdown_flag, &mut batch),
                    WaitForEvent::Timeout => {
                        // Go back to "Processing" state: re-consider sending the partial batch.
                        State::Processing
                    }
                };
            }
            State::Terminated => break,
        }
    }
    // Take the last error, if any, from the batch.
    batch.clear()
}

/// Returns a message and the shutdown flag peeked from the queue without blocking.
///
/// When the sender is dropped, and all messages have been processed,
/// the shutdown flag is returned as `true`.
fn peek_queue(
    receiver: &mpsc::Receiver<(Box<VortexMessage>, bool)>,
) -> (Option<Box<VortexMessage>>, bool) {
    match receiver.try_recv() {
        Ok((msg, is_shutdown)) => (Some(msg), is_shutdown),
        Err(mpsc::TryRecvError::Empty) => (None, false),
        Err(mpsc::TryRecvError::Disconnected) => (None, true),
    }
}

enum WaitForEvent {
    /// Message and shutdown flag received from the channel.
    ///
    /// The shutdown message comes with the shutdown flag set to `true`.
    /// If the sender has disconnected, `wait_for_event` will return no
    /// message and the shutdown flag set to `true`.
    Event((Option<Box<VortexMessage>>, bool)),
    /// No message received, but the channel is still open.
    Timeout,
}

/// `recv_timeout` or `recv` from the `mpsc::Receiver` of events.
///
/// This function only blocks indefinitely when the `deadline` is `None`. And
/// that's only the case when the batch of messages has no messages in it, so
/// waking up only when there are new messages in the queue is the right thing
/// to do. This guarantees retries, timely delivery, and progress in the worker
/// thread loop with the minimal number of wakeups.
fn wait_for_event(
    receiver: &mpsc::Receiver<(Box<VortexMessage>, bool)>,
    deadline: Option<Instant>,
) -> WaitForEvent {
    if let Some(deadline) = deadline {
        let timeout = deadline.saturating_duration_since(Instant::now());
        match receiver.recv_timeout(timeout) {
            Ok((message, is_shutdown)) => WaitForEvent::Event((Some(message), is_shutdown)),
            Err(mpsc::RecvTimeoutError::Timeout) => WaitForEvent::Timeout,
            Err(mpsc::RecvTimeoutError::Disconnected) => WaitForEvent::Event((None, true)),
        }
    } else {
        match receiver.recv() {
            Ok((message, is_shutdown)) => WaitForEvent::Event((Some(message), is_shutdown)),
            Err(mpsc::RecvError) => WaitForEvent::Event((None, true)),
        }
    }
}

/// Tests for the Vortex client.
///
/// If you're working on the Vortex client, you can run more strict assertions
/// with the `scheduling-tests` feature enabled. These asserts can't be enabled
/// by default because if the CI machines are overloaded, threads might take
/// longer to get CPU time and that affects asserts on batch sizes and deadlines.
///
/// ```sh
/// cargo test -p vortex-client --features scheduling-tests
/// ```
///
/// But the external behavior of the Vortex is eventually consistent: even if the
/// batches sent differ in size, eventually all events are sent successfully and/or
/// retried appropriately.
#[cfg(test)]
mod tests {
    use super::*;

    use http::StatusCode;
    use std::sync::Arc;

    const TARGET_BATCH_SIZE_BYTES: usize = 128;
    const FLUSH_INTERVAL: Duration = Duration::from_millis(16);
    const LONG_FLUSH_INTERVAL: Duration = Duration::from_secs(300);

    /// Safety margin for some time-releated assertions.
    ///
    /// We can never assume a thread will get scheduled immediately by the OS,
    /// (Linux, Windows, and macOS are not real-time operating systems), so
    /// when asserting that "something happens within a certain time" we should
    /// allow for some delay.
    ///
    /// IMPORTANT: increase this in case of flakeness in tests.
    const MARGIN: Duration = Duration::from_millis(MIN_BACKOFF_MILLIS / 2);

    #[derive(prost::Message, Clone, serde::Serialize)]
    struct TestMessage {
        #[prost(string, tag = "1")]
        field: String,
    }

    impl prost::Name for TestMessage {
        const NAME: &'static str = "TestMessage";
        const PACKAGE: &'static str = "test";
    }

    fn test_message(i: usize) -> TestMessage {
        let field = format!("Message {i}");
        TestMessage { field }
    }

    fn boxed_message<T: Message + prost::Name + serde::Serialize>(
        message: T,
    ) -> Box<VortexMessage> {
        let any = pbjson_types::Any {
            type_url: T::type_url(),
            value: message.encode_to_vec().into(),
        };
        box_any_message(any)
    }

    fn boxed_test_message(i: usize) -> Box<VortexMessage> {
        boxed_message(test_message(i))
    }

    #[test]
    #[allow(clippy::cognitive_complexity)]
    fn test_batch() {
        const MAX_BACKOFF: Duration = Duration::from_secs(30);
        let mut batch = Batch::new(
            TARGET_BATCH_SIZE_BYTES,
            TARGET_BATCH_SIZE_BYTES * 2,
            FLUSH_INTERVAL,
        );
        assert!(batch.is_empty());
        assert!(!batch.is_full());
        assert!(!batch.is_overdue());
        assert!(batch.deadline.is_none());

        let message = boxed_test_message(0);
        batch.push(message);
        assert!(!batch.is_empty());
        assert!(!batch.is_full());
        assert!(!batch.is_overdue());
        assert!(batch.deadline.is_some());
        if let Some(deadline) = batch.deadline {
            assert!(deadline <= Instant::now() + DEFAULT_FLUSH_INTERVAL + MARGIN);
        }

        thread::sleep(batch.flush_interval);
        assert!(
            batch.is_overdue(),
            "batch should be overdue after deadline expires"
        );

        let mut prev_deadline = batch.deadline.unwrap();
        batch.backoff();
        assert_eq!(
            batch.num_attempts, 1,
            "backoff should increment the attempt count"
        );
        assert!(
            batch.deadline.unwrap() > prev_deadline,
            "backoff should extend the deadline"
        );
        for i in 2..100 {
            prev_deadline = batch.deadline.unwrap();
            batch.backoff();
            assert_eq!(batch.num_attempts, i);
            assert!(batch.deadline.unwrap() >= prev_deadline);
            assert!(batch.deadline.unwrap() <= Instant::now() + MAX_BACKOFF);
        }

        batch.clear_after_success();
        assert!(batch.is_empty());
        assert!(!batch.is_full());
        assert!(!batch.is_overdue());
        assert!(batch.deadline.is_none());
        assert_eq!(batch.num_attempts, 0);

        for i in 0.. {
            let message = test_message(i);
            batch.push(boxed_message(message));
            if batch.is_full() {
                break;
            }
        }
        assert!(batch.is_overdue(), "a full batch is considered overdue");
        batch.backoff();
        assert!(
            !batch.is_overdue(),
            "a full batch that failed to send must respect the backoff"
        );
        thread::sleep(
            batch
                .deadline
                .unwrap()
                .saturating_duration_since(Instant::now()),
        );
        assert!(
            batch.is_overdue(),
            "a batch is eventually overdue for retry"
        );
    }

    #[derive(Default)]
    struct SentBatches {
        failed: Vec<Vec<VortexMessage>>,
        sent: Vec<Vec<VortexMessage>>,
    }

    #[derive(Default)]
    struct SentBatchesHandle {
        inner: Mutex<SentBatches>,
    }
    impl SentBatchesHandle {
        fn with_lock(&self, fnc: impl FnOnce(&SentBatches)) {
            let lock = self.inner.lock().unwrap();
            fnc(&lock);
        }

        fn push_failed_attempt(&self, messages: Vec<VortexMessage>) {
            let mut messages = messages;
            for msg in &mut messages {
                assert!(
                    msg.vortex_client_sent_at.is_some(),
                    "message must have a sent timestamp."
                );
                // clear the sent at timestamps to simplify the test assertions
                msg.vortex_client_sent_at = None;
            }
            let mut lock = self.inner.lock().unwrap();
            lock.failed.push(messages);
        }

        fn push(&self, messages: Vec<VortexMessage>) {
            let mut messages = messages;
            for msg in &mut messages {
                assert!(
                    msg.vortex_client_sent_at.is_some(),
                    "message must have a sent timestamp."
                );
                // clear the sent at timestamps to simplify the test assertions
                msg.vortex_client_sent_at = None;
            }
            let mut lock = self.inner.lock().unwrap();
            lock.sent.push(messages);
        }

        fn poll_until(
            &self,
            interval: Duration,
            predicate: impl Fn(&SentBatches) -> bool,
        ) -> Duration {
            const MAX_POLL_DURATION: Duration = Duration::from_secs(30);
            let start = Instant::now();
            while {
                let lock = self.inner.lock().unwrap();
                !predicate(&lock)
            } {
                thread::sleep(interval);
                if start.elapsed() >= MAX_POLL_DURATION {
                    panic!(
                        "SentBatchesHandle::poll_until(): timed out waiting for condition to be met."
                    );
                }
            }
            start.elapsed()
        }
    }

    enum ServerMode {
        AlwaysSucceed,
        Unreachable,
        BoundedSuccesses(usize),
        #[allow(dead_code)]
        BoundedFailures(usize),
    }

    struct TestSenderAgent {
        max_batch_size: usize,
        flush_interval: Duration,
        mode: ServerMode,
        sent_batches: Arc<SentBatchesHandle>,
        num_successes: Mutex<usize>, // for BoundedSuccesses mode
        num_failures: Mutex<usize>,  // for BoundedFailures mode
    }
    impl TestSenderAgent {
        fn new(mode: ServerMode, max_batch_size: usize, flush_interval: Duration) -> Self {
            // For test purposes, we set body_size_limit to be twice the max_batch_size,
            // so we don't need it explicitly configured here.
            Self {
                max_batch_size,
                flush_interval,
                mode,
                sent_batches: Arc::new(SentBatchesHandle::default()),
                num_successes: Mutex::new(0),
                num_failures: Mutex::new(0),
            }
        }

        fn with_server_mode(mode: ServerMode) -> Self {
            Self::new(mode, TARGET_BATCH_SIZE_BYTES, FLUSH_INTERVAL)
        }

        fn with_long_flush_interval(mode: ServerMode) -> Self {
            Self::new(mode, TARGET_BATCH_SIZE_BYTES, LONG_FLUSH_INTERVAL)
        }

        fn sent_batches_handle(&self) -> Arc<SentBatchesHandle> {
            Arc::clone(&self.sent_batches)
        }
    }
    impl SenderAgent for TestSenderAgent {
        fn target_batch_size(&self) -> usize {
            self.max_batch_size
        }
        fn body_size_limit(&self) -> usize {
            self.max_batch_size * 2
        }
        fn flush_interval(&self) -> Duration {
            self.flush_interval
        }
        fn send_batch(&self, batch: &mut Batch) -> bool {
            assert!(!batch.is_empty(), "send_batch: batch must be non-empty.");
            let _body = batch.encode_for_sending();
            // XXX: _body contains more than bytes_buffered (UUID, message count, etc.),
            // but we enforce the limit only on bytes_buffered.
            debug_assert!(
                batch.bytes_buffered <= self.body_size_limit() + 128,
                "send_batch: batch size exceeds body_size_limit."
            );
            match self.mode {
                ServerMode::AlwaysSucceed => {
                    self.sent_batches.push(batch.messages.clone());
                    batch.on_response(StatusCode::OK, "OK".to_string());
                    true
                }
                ServerMode::Unreachable => {
                    self.sent_batches
                        .push_failed_attempt(batch.messages.clone());
                    batch.on_error(ureq::Error::ConnectionFailed);
                    false
                }
                ServerMode::BoundedSuccesses(max_successes) => {
                    let success = {
                        let mut num_successes = self.num_successes.lock().unwrap();
                        if *num_successes < max_successes {
                            *num_successes += 1;
                            true
                        } else {
                            false
                        }
                    };
                    if success {
                        self.sent_batches.push(batch.messages.clone());
                        batch.on_response(StatusCode::OK, "OK".to_string());
                        true
                    } else {
                        self.sent_batches
                            .push_failed_attempt(batch.messages.clone());
                        batch.on_response(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "Vortex server has failed".to_string(),
                        );
                        false
                    }
                }
                ServerMode::BoundedFailures(max_failures) => {
                    let fail = {
                        let mut num_failures = self.num_failures.lock().unwrap();
                        if *num_failures < max_failures {
                            *num_failures += 1;
                            true
                        } else {
                            false
                        }
                    };
                    if fail {
                        self.sent_batches
                            .push_failed_attempt(batch.messages.clone());
                        batch.on_response(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "Vortex server has failed".to_string(),
                        );
                        false
                    } else {
                        self.sent_batches.push(batch.messages.clone());
                        batch.on_response(StatusCode::OK, "OK".to_string());
                        true
                    }
                }
            }
        }
    }

    #[test]
    fn test_log_is_non_blocking_on_unreachable_server() {
        let agent = TestSenderAgent::with_long_flush_interval(ServerMode::Unreachable);
        let sent_batches_handle = agent.sent_batches_handle();
        let mut client = VortexProducerClient::new(Box::new(agent), None);

        client.do_log_proto(test_message(0), false).unwrap();
        client.do_log_proto(test_message(1), true).unwrap(); // shutdown message
        client.do_log_proto(test_message(2), false).unwrap(); // after shutdown message (logged)

        let start = Instant::now();
        let handle = client.take_thread_handle();
        handle.join_on_shutdown().unwrap_err(); // wait for the worker thread to finish
        let elapsed = start.elapsed();
        assert!(
            elapsed < MARGIN,
            "Worker thread should not block indefinitely on retries when server is unreachable"
        );

        sent_batches_handle.with_lock(|batches| {
            assert_eq!(batches.failed.len(), 1);
            assert_eq!(batches.sent.len(), 0);
        });
    }

    #[test]
    #[allow(unused_variables)]
    fn test_log_is_non_blocking_after_shutdown() {
        let agent = TestSenderAgent::with_long_flush_interval(ServerMode::AlwaysSucceed);
        let sent_batches_handle = agent.sent_batches_handle();
        let mut client = VortexProducerClient::new(Box::new(agent), None);

        client.do_log_proto(test_message(0), false).unwrap();
        client.do_log_proto(test_message(1), false).unwrap();
        client.do_log_proto(test_message(2), false).unwrap();
        client.do_log_proto(test_message(3), false).unwrap(); // TARGET_BATCH_SIZE_BYTES reached
        client.do_log_proto(test_message(4), false).unwrap();
        client.do_log_proto(test_message(5), true).unwrap(); // shutdown message
        client.do_log_proto(test_message(6), false).unwrap(); // after shutdown message (might be logged)

        let start = Instant::now();
        let handle = client.take_thread_handle();
        handle.join_on_shutdown().unwrap(); // wait for the worker thread to finish
        #[cfg(feature = "scheduling-tests")]
        {
            let elapsed = start.elapsed();
            assert!(
                elapsed < MARGIN,
                "Worker thread should not block indefinitely on retries when server is unreachable"
            );
        }

        // this prints a warning in debug mode, but does not do anything in release mode
        let _ = client.do_log_proto(test_message(7), false);

        #[cfg(feature = "scheduling-tests")]
        sent_batches_handle.with_lock(|batches| {
            assert_eq!(batches.sent.len(), 2);
            assert_eq!(
                batches.sent,
                vec![
                    vec![
                        *boxed_test_message(0),
                        *boxed_test_message(1),
                        *boxed_test_message(2),
                        *boxed_test_message(3), // TARGET_BATCH_SIZE_BYTES reached
                    ],
                    vec![
                        *boxed_test_message(4),
                        *boxed_test_message(5), // shutdown message
                        *boxed_test_message(6),
                    ]
                ]
            );
        });
    }

    #[test]
    #[allow(unused_variables)]
    fn test_batch_is_sent_after_flush_interval() {
        let agent = TestSenderAgent::with_server_mode(ServerMode::AlwaysSucceed);
        let flush_interval = agent.flush_interval();
        let sent_batches_handle = agent.sent_batches_handle();
        let mut client = VortexProducerClient::new(Box::new(agent), None);

        client.do_log_proto(test_message(0), false).unwrap();
        client.do_log_proto(test_message(1), false).unwrap();
        let elapsed = sent_batches_handle.poll_until(FLUSH_INTERVAL / 2, |batches| {
            batches.sent.len() == 1 && batches.sent[0].len() == 2
        });
        #[cfg(feature = "scheduling-tests")]
        assert!(elapsed >= flush_interval);

        client.do_log_proto(test_message(2), false).unwrap();
        client.do_log_proto(test_message(3), true).unwrap(); // shutdown message
        client.do_log_proto(test_message(4), false).unwrap(); // after shutdown message (might be logged)

        client.take_thread_handle().join_on_shutdown().unwrap();

        sent_batches_handle.with_lock(|batches| {
            assert!(batches.sent.len() >= 2);
            #[cfg(feature = "scheduling-tests")]
            {
                assert_eq!(
                    batches.sent,
                    vec![
                        vec![
                            *boxed_test_message(0),
                            *boxed_test_message(1), // FLUSH_INTERVAL reached
                        ],
                        vec![
                            *boxed_test_message(2),
                            *boxed_test_message(3), // shutdown message
                            *boxed_test_message(4),
                        ]
                    ]
                );
            }
            #[cfg(not(feature = "scheduling-tests"))]
            {
                let mut count = 0;
                for batch in &batches.sent {
                    count += batch.len();
                }
                assert!(
                    count >= 4,
                    "Expected at least 4 messages in total, but got {count}"
                );
            }
        });
    }

    #[test]
    #[cfg(feature = "scheduling-tests")]
    fn test_batch_is_sent_after_retries() {
        let agent = TestSenderAgent::with_server_mode(ServerMode::BoundedFailures(2));
        let flush_interval = agent.flush_interval();
        let sent_batches_handle = agent.sent_batches_handle();
        let mut client = VortexProducerClient::new(Box::new(agent), None);

        client.do_log_proto(test_message(0), false).unwrap();
        client.do_log_proto(test_message(1), false).unwrap();
        sent_batches_handle.poll_until(flush_interval / 2, |batches| {
            assert!(batches.sent.is_empty());
            // 1 is observable here because flush_interval/2 < MIN_BACKOFF_MILLIS.
            batches.failed.len() == 1
        });
        client.do_log_proto(test_message(2), false).unwrap();
        sent_batches_handle.poll_until(flush_interval / 2, |batches| {
            // we expect the server to fail again, so the batch is not sent yet
            assert!(batches.sent.is_empty());
            batches.failed.len() == 2
        });
        sent_batches_handle.poll_until(flush_interval / 2, |batches| {
            // we expect the server to send 3 events after the backoff period
            assert!(batches.failed.len() == 2);
            if batches.sent.len() == 1 {
                assert_eq!(
                    batches.sent[0].len(),
                    3,
                    "all 3 events should be sent in a batch"
                );
                true
            } else {
                false
            }
        });
        client.do_log_proto(test_message(3), true).unwrap(); // shutdown message
        client.do_log_proto(test_message(4), false).unwrap(); // after shutdown message (might be logged)
        client.take_thread_handle().join_on_shutdown().unwrap();

        sent_batches_handle.with_lock(|batches| {
            assert_eq!(batches.sent.len(), 2);
            assert_eq!(
                batches.sent,
                vec![
                    vec![
                        *boxed_test_message(0),
                        *boxed_test_message(1),
                        *boxed_test_message(2), // after backoff for 2 failures
                    ],
                    vec![*boxed_test_message(3), *boxed_test_message(4),]
                ]
            );
        });
    }

    #[test]
    #[allow(unused_variables, clippy::len_zero)]
    fn test_terminates_after_permanent_failure() {
        let agent = TestSenderAgent::with_server_mode(ServerMode::BoundedSuccesses(2));
        let flush_interval = agent.flush_interval();
        let sent_batches_handle = agent.sent_batches_handle();
        let mut client = VortexProducerClient::new(Box::new(agent), None);

        client.do_log_proto(test_message(0), false).unwrap();
        client.do_log_proto(test_message(1), false).unwrap();
        client.do_log_proto(test_message(2), false).unwrap();
        client.do_log_proto(test_message(3), false).unwrap();
        let elapsed =
            sent_batches_handle.poll_until(flush_interval / 2, |batches| batches.sent.len() >= 1);
        #[cfg(feature = "scheduling-tests")]
        assert!(elapsed < flush_interval + MARGIN); // TARGET_BATCH_SIZE_BYTES reached

        // After 1 success, the server will keep failing, so let's simulate a burst of messages.
        for i in 4..=14 {
            client.do_log_proto(test_message(i), false).unwrap();
        }
        // Poll for the next and last success.
        sent_batches_handle.poll_until(flush_interval / 2, |batches| batches.sent.len() >= 2);
        // Look at the next 2 failures.
        sent_batches_handle.poll_until(flush_interval / 2, |batches| {
            if batches.failed.len() >= 2 {
                #[cfg(feature = "scheduling-tests")]
                assert_eq!(
                    batches.failed,
                    vec![
                        // The first failure is a full batch with TARGET_BATCH_SIZE_BYTES.
                        vec![
                            *boxed_test_message(8),
                            *boxed_test_message(9),
                            *boxed_test_message(10),
                            *boxed_test_message(11),
                        ],
                        // Due to backoff, the second failure also has all the messages
                        // peeked from the queue during the backoff period.
                        vec![
                            // "Message 8" was dropped, to make the batch smaller than BODY_SIZE_LIMIT.
                            *boxed_test_message(9),
                            *boxed_test_message(10),
                            *boxed_test_message(11),
                            *boxed_test_message(12),
                            *boxed_test_message(13),
                            *boxed_test_message(14),
                        ],
                    ]
                );
                true
            } else {
                false
            }
        });

        // During the backoff period before the 3rd failure, we log more messages.
        client.do_log_proto(test_message(15), false).unwrap();
        client.do_log_proto(test_message(16), false).unwrap();
        client.do_log_proto(test_message(17), false).unwrap();
        sent_batches_handle.poll_until(flush_interval / 2, |batches| {
            if batches.failed.len() >= 3 {
                #[cfg(feature = "scheduling-tests")]
                assert_eq!(
                    batches.failed[2],
                    vec![
                        // "Message 8" was dropped before.
                        // Now "Message 9" and "Message 10" are dropped.
                        *boxed_test_message(11),
                        *boxed_test_message(12),
                        *boxed_test_message(13),
                        *boxed_test_message(14),
                        *boxed_test_message(15),
                        *boxed_test_message(16),
                        *boxed_test_message(17),
                    ],
                );
                true
            } else {
                false
            }
        });

        // During the backoff period we burst and log a shutdown message. We expect a 4th
        // failure in the request attempt made as soon as the shutdown message is received.
        for i in 18..=30 {
            client.do_log_proto(test_message(i), false).unwrap();
        }
        client.do_log_proto(test_message(31), true).unwrap();
        let elapsed = sent_batches_handle.poll_until(flush_interval / 2, |batches| {
            if batches.failed.len() >= 4 {
                #[cfg(feature = "scheduling-tests")]
                assert_eq!(
                    batches.failed[3],
                    vec![
                        // A bunch of messages from the final burst are dropped.
                        *boxed_test_message(25),
                        *boxed_test_message(26),
                        *boxed_test_message(27),
                        *boxed_test_message(28),
                        *boxed_test_message(29),
                        *boxed_test_message(30),
                        *boxed_test_message(31), // shutdown message get a fair chance
                    ],
                );
                true
            } else {
                false
            }
        });
        #[cfg(feature = "scheduling-tests")]
        // The backoff at this point is much longer than 4 * MIN_BACKOFF_MILLIS,
        // but we expect the shutdown message to be sent as soon as the OS wakes up the
        // thread with that final shutdown message in the queue.
        assert!(
            elapsed < Duration::from_millis(4 * MIN_BACKOFF_MILLIS),
            "shutdown cancels the backoff and sends the shutdown message immediately"
        );

        client.take_thread_handle().join_on_shutdown().unwrap_err();
    }
}
