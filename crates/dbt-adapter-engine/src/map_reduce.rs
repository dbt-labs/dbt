//! A mini-framework for running parallel Key-to-Value tasks over a bounded
//! number of workers in dbt-runtime and reducing the results.

use dbt_adbc::Connection;
use dbt_base::cancel::{Cancellable, CancellationToken, CancelledError};
use dbt_runtime::{Handle, JoinHandle};
use futures::stream::{FuturesUnordered, StreamExt};
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::SendError;
use tokio::sync::oneshot;
use tracy_client::span;

use std::future::Future;
use std::panic;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

/// Encapsulates connection creation and recycling.
pub trait ConnectionFactory: Send + Sync {
    type Error;

    /// Create or recycle a connection. `node_id` identifies the node
    /// requesting the connection (used by some adapters for recycling affinity).
    ///
    /// This is always called from a blocking context (a worker thread of the
    /// `dbt_runtime` blocking pool), so implementations may block.
    fn new_connection(&self, node_id: Option<&str>) -> Result<Box<dyn Connection>, Self::Error>;

    /// Return a connection for potential reuse.
    fn recycle_connection(&self, conn: Box<dyn Connection>);
}

/// A function that maps a key to a computed value using a [Connection].
type MapF<Key, Value> = Box<dyn Fn(&'_ mut dyn Connection, &Key) -> Value + Send + Sync>;

/// A function that reduces a computed value into an accumulator.
type ReduceF<Acc, Key, Value, Error> =
    Box<dyn Fn(&mut Acc, Key, Value) -> Result<(), Error> + Send + Sync>;

struct MapReduceInner<Key, Value, Acc, Error>
where
    Key: Sized + Send,
    Value: Sized + Send + 'static,
    Acc: Sized + Default + Send + 'static,
    Error: Send,
{
    /// Connection factory for creating and recycling connections.
    connection_factory: Arc<dyn ConnectionFactory<Error = Cancellable<Error>>>,
    /// Node ID forwarded to the connection factory on each new_connection call.
    node_id: Option<String>,
    /// Function to map a key to a computed value using a [Connection].
    map_f: MapF<Key, Value>,
    /// Function to reduce a computed value into the accumulator.
    reduce_f: ReduceF<Acc, Key, Value, Cancellable<Error>>,

    /// The next key to be processed by any of the workers.
    key_counter: AtomicUsize,
    /// Total time spent in `task_count` tasks.
    total_task_time_us: AtomicU64,
    task_count: AtomicU64,
    /// Total connection creation time reported by `conn_count` workers.
    total_conn_time_us: AtomicU64,
    conn_count: AtomicU64,
}

impl<K, V, Acc, E> MapReduceInner<K, V, Acc, E>
where
    K: Sized + Send,
    V: Sized + Send + 'static,
    Acc: Sized + Default + Send + 'static,
    E: Send + 'static,
{
    fn new_connection(&self) -> Result<Box<dyn Connection>, Cancellable<E>> {
        // dbt_runtime worker threads create a connection (blocking) and then loop.
        // `ConnectionFactory` implementations may reuse thread-local connections
        // and since the number of workers is bounded by the runtime's max_parallelism,
        // we never create more than that many connection at once.
        debug_assert!(
            dbt_runtime::is_pool_worker(),
            "new_connection() must be called from a worker thread"
        );
        let _span = span!("MapReduceInner::new_connection");
        self.connection_factory
            .new_connection(self.node_id.as_deref())
    }

    fn recycle_connection(&self, conn: Box<dyn Connection>) {
        self.connection_factory.recycle_connection(conn);
    }

    /// Record a connection creation time reported by a worker.
    fn observe_conn_time(&self, worker: &WorkerHandle) {
        if let Some(conn_time) = worker.conn_time {
            self.conn_count.fetch_add(1, Ordering::SeqCst);
            self.total_conn_time_us
                .fetch_add(conn_time.as_micros() as u64, Ordering::SeqCst);
        }
    }

    fn map(&self, conn: &'_ mut dyn Connection, key: &K) -> V {
        let _span = span!("MapReduceInner::map");
        let start = std::time::Instant::now();
        let res = (self.map_f)(conn, key);
        let elapsed = start.elapsed();
        self.task_count.fetch_add(1, Ordering::SeqCst);
        self.total_task_time_us
            .fetch_add(elapsed.as_micros() as u64, Ordering::SeqCst);
        res
    }

    fn avg_conn_time_us(&self) -> f64 {
        // if an older conn_count or total_conn_time_us is loaded, the
        // average will be incorrect, but the error will be small
        let conn_count = self.conn_count.load(Ordering::SeqCst);
        self.total_conn_time_us.load(Ordering::SeqCst) as f64 / conn_count.max(1) as f64
    }

    fn avg_task_time_us(&self) -> f64 {
        // if an older task_count or total_task_time_us is loaded, the
        // average will be incorrect, but the error will be small
        let task_count = self.task_count.load(Ordering::SeqCst);
        self.total_task_time_us.load(Ordering::SeqCst) as f64 / task_count.max(1) as f64
    }
}

/// Run parallel Key-to-Value tasks in the `dbt_runtime` pool of blocking
/// threads and reduce the results into an accumulator.
///
/// Each worker owns one connection: it creates it in the blocking context and
/// reuses it until all keys have been claimed.
///
/// The number of workers is bounded by the number of keys and the parallelism
/// of the `dbt_runtime` pool. Connection creation and recycling are managed by
/// a [`ConnectionFactory`] implementation passed at construction time.
pub struct MapReduce<Key, Value, Acc, Error>
where
    Key: Sized + Clone + Send + Sync + 'static,
    Value: Sized + Send + 'static,
    Acc: Sized + Default + Send + 'static,
    Error: Send + 'static,
{
    inner: Arc<MapReduceInner<Key, Value, Acc, Error>>,
}

/// A worker running in the blocking pool, and the cost of the connection it
/// works with.
///
/// Awaiting a handle waits for the worker's work, delegating to the underlying
/// [`JoinHandle`].
struct WorkerHandle {
    handle: JoinHandle<Result<(), CancelledError>>,
    /// How long the worker took to create its connection.
    conn_time: Option<Duration>,
}

impl Future for WorkerHandle {
    type Output = <JoinHandle<Result<(), CancelledError>> as Future>::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.get_mut().handle).poll(cx)
    }
}

impl<K, V, Acc, E> MapReduce<K, V, Acc, E>
where
    K: Sized + Clone + Send + Sync + 'static,
    V: Sized + Send + 'static,
    Acc: Sized + Default + Send + 'static,
    E: Send + 'static,
{
    pub fn new(
        connection_factory: Box<dyn ConnectionFactory<Error = Cancellable<E>>>,
        map_f: MapF<K, V>,
        reduce_f: ReduceF<Acc, K, V, Cancellable<E>>,
        node_id: Option<String>,
    ) -> Self {
        let inner = MapReduceInner {
            connection_factory: connection_factory.into(),
            node_id,
            map_f,
            reduce_f,
            key_counter: AtomicUsize::new(0),
            total_task_time_us: AtomicU64::new(0),
            task_count: AtomicU64::new(0),
            total_conn_time_us: AtomicU64::new(0),
            conn_count: AtomicU64::new(0),
        };
        Self {
            inner: Arc::new(inner),
        }
    }

    /// Spawn a worker in the blocking pool, and return its pending
    /// [`WorkerHandle`].
    ///
    /// The worker starts right away: it creates its own connection — in the
    /// blocking context, where blocking is allowed — and reports how long that
    /// took through a one-shot channel, so the driver can await it and take the
    /// cost of opening connections into account when deciding to spawn more
    /// workers.
    ///
    /// The pending report is all the caller gets, and it owns the worker's
    /// handle until it resolves. The handles of workers that never connect are
    /// therefore dropped — detaching those workers — when the driver drops the
    /// reports it is still waiting for.
    #[inline(never)]
    fn spawn_worker(
        &self,
        tx: mpsc::UnboundedSender<(K, V)>,
        keys: Arc<Vec<K>>,
        token: &CancellationToken,
    ) -> impl Future<Output = Result<WorkerHandle, Cancellable<E>>> + Send + 'static {
        let inner = Arc::clone(&self.inner); // clone needed to move it into lambda
        let token = token.clone(); // clone needed to move it into lambda
        let (conn_tx, conn_rx) = oneshot::channel::<Result<Duration, Cancellable<E>>>();
        // A worker opens a connection and then loops, reusing the connection until
        // all keys have been claimed. The worker returns a CancelledError if:
        // - the receiver has been dropped or closed
        // - the cancellation token is cancelled
        let work = move || -> Result<(), CancelledError> {
            let start = std::time::Instant::now();
            let mut conn = match inner.new_connection() {
                Ok(conn) => {
                    // Ignoring the send error is fine: the thread driving the
                    // map-reduce loops may have stopped waiting for this report,
                    // but the connection is still usable.
                    let _ = conn_tx.send(Ok(start.elapsed()));
                    conn
                }
                Err(err) => {
                    // The thread driving the map-reduce loops decides if a
                    // connection failure is fatal: it is for the first worker,
                    // but later workers are only speculative, so this one simply
                    // has no work to do.
                    let _ = conn_tx.send(Err(err));
                    return Ok(());
                }
            };
            loop {
                let i = inner.key_counter.fetch_add(1, Ordering::SeqCst);
                if i >= keys.len() {
                    // No more keys to process, recycle connection and exit.
                    inner.recycle_connection(conn);
                    return Ok(());
                }
                let key = keys[i].clone();
                let value = inner.map(&mut *conn, &key);
                match tx.send((key, value)) {
                    Ok(()) => (),
                    Err(SendError(_)) => {
                        // The receiver has been dropped or closed (due to
                        // cancellation), so we fail with a CancelledError. We also
                        // don't worry about recycling the connection.
                        return Err(CancelledError);
                    }
                }

                if token.is_cancelled() {
                    return Err(CancelledError);
                    // And don't worry about recycling the connection since we're shutting down.
                }
            }
        };
        let mut handle = WorkerHandle {
            handle: dbt_runtime::spawn_blocking(work),
            conn_time: None,
        };
        async move {
            match conn_rx.await {
                Ok(Ok(conn_time)) => {
                    // Now that the connection is created, we can decorate the
                    // worker's handle with the time it took to create the
                    // connection and finally let the caller be able to await
                    // the worker's work post connection creation.
                    handle.conn_time = Some(conn_time);
                    Ok(handle)
                }
                // The worker reported that it could not connect, and returned
                // right after doing so: there is no work left to wait for, so
                // its handle is dropped here.
                Ok(Err(err)) => Err(err),
                // The worker dropped its sender without reporting, so it never
                // reached the point of having a connection. Joining it here is
                // what turns a panic on the way to that point into an error,
                // instead of losing it along with the detached task.
                Err(_recv_error) => {
                    let cancelled = match handle.await {
                        Ok(Ok(()) | Err(CancelledError)) => CancelledError.into(),
                        Err(join_error) => cancelled_from_join_error(join_error).into(),
                    };
                    Err(cancelled)
                }
            }
        }
    }

    /// Reduce a computed value into an accumulator.
    fn reduce(&self, acc: &mut Acc, key: K, value: V) -> Result<(), Cancellable<E>> {
        (self.inner.reduce_f)(acc, key, value)
    }

    async fn wait_for_all_keys_claimed(
        &self,
        key_count: usize,
        token: &CancellationToken,
    ) -> Result<(), CancelledError> {
        while self.inner.key_counter.load(Ordering::SeqCst) < key_count {
            tokio::time::sleep(Duration::from_secs(1)).await;
            token.check_cancellation()?;
        }
        Ok(())
    }

    /// Run all tasks in parallel with at most `max_parallelism` workers.
    async fn do_run(
        self,
        keys: Arc<Vec<K>>,
        token: CancellationToken,
    ) -> Result<Acc, Cancellable<E>> {
        let mut acc = Acc::default();
        if keys.is_empty() {
            return Ok(acc);
        }

        let mut recv_buffer = Vec::new();
        let (tx, mut rx) = mpsc::unbounded_channel::<(K, V)>();

        let max_parallelism = keys.len().min(Handle::current().max_parallelism());

        // Every worker sits in exactly one of these two sets. After connecting,
        // a worker is moved to the `running` set.
        let mut connecting = FuturesUnordered::new();
        let mut running = FuturesUnordered::new();
        let transition_to_running = |worker: WorkerHandle| {
            self.inner.observe_conn_time(&worker);
            running.push(worker);
        };

        // Start one or two workers speculatively.
        for _ in 0..max_parallelism.min(2) {
            connecting.push(self.spawn_worker(tx.clone(), Arc::clone(&keys), &token));
        }

        // To start, ensure there is at least one connection open and one task
        // enqueued. Even if all the other connections fail, we can still keep
        // making progress by reusing this one.
        let worker = connecting
            .next()
            .await
            .expect("at least one worker is always spawned")?;
        transition_to_running(worker);

        while self.inner.key_counter.load(Ordering::SeqCst) < keys.len() {
            // Workers still opening a connection are speculative: an existing worker
            // may claim every key while a new connection is still opening. Stop
            // waiting once that connection can no longer help with this work.
            if !connecting.is_empty() {
                tokio::select! {
                    Some(res) = connecting.next() => {
                        // A worker that could not connect has already finished and
                        // handed back no handle, so there is nothing to wait for.
                        if let Ok(worker) = res {
                            transition_to_running(worker);
                        }
                    }
                    claimed = self.wait_for_all_keys_claimed(keys.len(), &token) => claimed?,
                }
            }

            let n_workers = connecting.len() + running.len();
            if n_workers < max_parallelism {
                let remaining_keys = {
                    let key_counter = self.inner.key_counter.load(Ordering::SeqCst);
                    if key_counter < keys.len() {
                        keys.len() - key_counter
                    } else {
                        0
                    }
                };

                const K: f64 = 1.5; // sensitivity factor
                if (remaining_keys as f64 * self.inner.avg_task_time_us()) / (n_workers as f64)
                    > (self.inner.avg_conn_time_us() * K)
                {
                    connecting.push(self.spawn_worker(tx.clone(), Arc::clone(&keys), &token));
                    continue;
                }
            }

            if !rx.is_empty() {
                let n = rx.recv_many(&mut recv_buffer, n_workers).await;
                debug_assert!(recv_buffer.len() == n);
                for _ in 0..n {
                    let (key, value) = recv_buffer.pop().unwrap();
                    self.reduce(&mut acc, key, value)?;
                }
            } else if self.inner.key_counter.load(Ordering::SeqCst) < keys.len() {
                let us = self.inner.avg_conn_time_us().floor() as u64;
                let duration = Duration::from_micros(us).min(Duration::from_secs(1));
                tokio::time::sleep(duration).await;
            }

            token.check_cancellation()?;
        }
        drop(tx);

        // All keys are claimed by now, and a worker always reports its connection
        // before claiming a key, so a worker that hasn't reported yet cannot produce
        // any value anymore. Dropping the still pending to connect drops those workers'
        // handles with them, detaching them instead of waiting on a connection that
        // may take arbitrarily long to open: the blocking task finishes on its own.
        drop(connecting);

        // Wait for all the workers that own a connection to finish...
        while let Some(res) = running.next().await {
            match res {
                Ok(Ok(())) => (),
                Ok(Err(CancelledError)) => {
                    return Err(CancelledError.into());
                }
                Err(join_error) => {
                    return Err(cancelled_from_join_error(join_error).into());
                }
            }
            token.check_cancellation()?;
        }
        // All keys are claimed but some workers might be hanging on connection
        // creation. Close the receiver so thay can't send more values, then...
        rx.close();
        // ...reduce the map function results.
        loop {
            let n = rx.recv_many(&mut recv_buffer, max_parallelism).await;
            if n == 0 {
                break;
            }
            for _ in 0..n {
                let (key, value) = recv_buffer.pop().unwrap();
                self.reduce(&mut acc, key, value)?;
            }
            token.check_cancellation()?;
        }

        Ok(acc)
    }

    pub fn run(
        self,
        keys: Arc<Vec<K>>,
        token: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<Acc, Cancellable<E>>> + Send>> {
        let future = self.do_run(keys, token);
        Box::pin(future)
    }
}

fn cancelled_from_join_error(err: dbt_runtime::JoinError) -> CancelledError {
    if err.is_cancelled() {
        CancelledError
    } else if err.is_panic() {
        panic::resume_unwind(err.into_panic());
    } else {
        unreachable!("JoinError's are either due to cancellation or panic");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_adbc::Statement;
    use std::sync::atomic::AtomicUsize;
    use std::sync::{Arc, Mutex};

    struct DummyConnection;

    impl Connection for DummyConnection {
        fn new_statement(&mut self) -> adbc_core::error::Result<Box<dyn Statement>> {
            unreachable!("test map function does not execute statements")
        }

        fn cancel(&mut self) -> adbc_core::error::Result<()> {
            Ok(())
        }

        fn commit(&mut self) -> adbc_core::error::Result<()> {
            Ok(())
        }

        fn rollback(&mut self) -> adbc_core::error::Result<()> {
            Ok(())
        }
    }

    struct BlockingSecondConnectionFactory {
        calls: AtomicUsize,
        second_requested: Mutex<Option<std::sync::mpsc::Sender<()>>>,
        release_second: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
    }

    impl ConnectionFactory for BlockingSecondConnectionFactory {
        type Error = Cancellable<()>;

        fn new_connection(
            &self,
            _node_id: Option<&str>,
        ) -> Result<Box<dyn Connection>, Self::Error> {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 1 {
                if let Some(tx) = self.second_requested.lock().unwrap().take() {
                    let _ = tx.send(());
                }
                let rx = self
                    .release_second
                    .lock()
                    .unwrap()
                    .take()
                    .expect("second connection blocker should be present");
                let _ = rx.recv();
            }
            Ok(Box::new(DummyConnection))
        }

        fn recycle_connection(&self, _conn: Box<dyn Connection>) {}
    }

    /// A worker that panics on the way to a connection never reports one, so its
    /// handle is never handed to the driver. The panic still has to surface
    /// rather than disappear with the detached task.
    #[dbt_runtime::test]
    #[should_panic(expected = "connection creation blew up")]
    async fn map_reduce_propagates_a_panic_from_connection_creation() {
        struct PanickingConnectionFactory;

        impl ConnectionFactory for PanickingConnectionFactory {
            type Error = Cancellable<()>;

            fn new_connection(
                &self,
                _node_id: Option<&str>,
            ) -> Result<Box<dyn Connection>, Self::Error> {
                panic!("connection creation blew up")
            }

            fn recycle_connection(&self, _conn: Box<dyn Connection>) {}
        }

        let map_reduce = MapReduce::new(
            Box::new(PanickingConnectionFactory),
            Box::new(|_conn: &mut dyn Connection, key: &u32| *key),
            Box::new(|acc: &mut u32, _key: u32, value: u32| {
                *acc += value;
                Ok(())
            }),
            None,
        );

        let _ = map_reduce
            .run(Arc::new(vec![1]), CancellationToken::never_cancels())
            .await;
    }

    #[dbt_runtime::test]
    async fn map_reduce_does_not_wait_for_extra_connection_before_reducing_completed_work() {
        struct ReleaseOnDrop(Option<std::sync::mpsc::Sender<()>>);

        impl Drop for ReleaseOnDrop {
            fn drop(&mut self) {
                if let Some(tx) = self.0.take() {
                    let _ = tx.send(());
                }
            }
        }

        let (second_requested_tx, second_requested_rx) = std::sync::mpsc::channel();
        let (release_second_tx, release_second_rx) = std::sync::mpsc::channel();
        let release_second = ReleaseOnDrop(Some(release_second_tx));
        let (mapped_last_tx, mapped_last_rx) = oneshot::channel();
        let mapped_last_tx = Arc::new(Mutex::new(Some(mapped_last_tx)));
        let second_requested_rx = Arc::new(Mutex::new(Some(second_requested_rx)));

        let factory = BlockingSecondConnectionFactory {
            calls: AtomicUsize::new(0),
            second_requested: Mutex::new(Some(second_requested_tx)),
            release_second: Mutex::new(Some(release_second_rx)),
        };
        let map_f = {
            let mapped_last_tx = Arc::clone(&mapped_last_tx);
            let second_requested_rx = Arc::clone(&second_requested_rx);
            Box::new(move |_conn: &mut dyn Connection, key: &u32| {
                if *key == 1 {
                    let rx = second_requested_rx
                        .lock()
                        .unwrap()
                        .take()
                        .expect("second connection waiter should be present");
                    let _ = rx.recv();
                }
                if *key == 2
                    && let Some(tx) = mapped_last_tx.lock().unwrap().take()
                {
                    let _ = tx.send(());
                }
                *key
            })
        };
        let reduce_f = Box::new(|acc: &mut u32, _key: u32, value: u32| {
            *acc += value;
            Ok(())
        });
        let map_reduce = MapReduce::new(Box::new(factory), map_f, reduce_f, None);

        let handle = tokio::spawn(async move {
            map_reduce
                .run(Arc::new(vec![1, 2]), CancellationToken::never_cancels())
                .await
        });
        tokio::time::timeout(Duration::from_secs(1), mapped_last_rx)
            .await
            .expect("first worker did not process all keys")
            .expect("first worker should process all keys");

        let result = tokio::time::timeout(Duration::from_secs(3), handle).await;
        drop(release_second);
        let acc = result
            .expect("MapReduce waited for an unused extra connection")
            .expect("MapReduce task panicked")
            .expect("MapReduce should succeed");
        assert_eq!(acc, 3);
    }
}
