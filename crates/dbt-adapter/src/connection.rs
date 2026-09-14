use std::borrow::Cow;
use std::cell::RefCell;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use dbt_adapter_core::AdapterType;
use dbt_adapter_engine::ConnectionFactory;
use dbt_adbc::Connection;
use dbt_common::AdapterResult;
use dbt_common::cancellation::Cancellable;
use dbt_telemetry::AdapterConnectionClose;
use dbt_tracing::emit::emit_trace_event;
use minijinja::State;

use crate::AdapterEngine;
use crate::errors::AdapterError;

// Thread-local connection.
//
// This implementation provides an efficient connection management strategy:
// 1. Each thread maintains its own connection instance
// 2. Connections are reused across multiple operations within the same thread
// 3. This approach ensures proper transaction management within a DAG node
// 4. The ConnectionGuard wrapper ensures connections are returned to the thread-local
// 5. A connection stays in the slot after a node finishes, so it is found again
//    by the next node that runs on the same worker
//
// The number of live connections is therefore bounded by the number of
// `dbt-runtime` blocking-pool worker threads, because every connection is
// created by a worker thread (enforced by the `is_pool_worker` assertion in
// `AdbcEngine::new_connection`). Blocking I/O on a bounded pool is what limits
// how much work reaches the warehouse at once, so there is no separate
// connection-admission gate.
thread_local! {
    static CONNECTION: pri::TlsConnectionContainer = pri::TlsConnectionContainer::new();
}

/// Drop this thread's cached connection instead of leaving it in the slot, so a
/// connection left in the wrong scope by a failed `RESET USE` / warehouse
/// restore can't be handed to the next node scheduled on this thread.
pub fn drop_thread_local_connection() {
    let conn = CONNECTION.with(|c| c.take());
    drop(conn);
}

/// Take this thread's cached connection, if it can be used with `fingerprint`.
///
/// A cached connection whose config fingerprint doesn't match is dropped, so
/// connections are only reused among identical connection configurations.
fn take_tlocal_connection(
    node_id: Option<String>,
    fingerprint: u64,
) -> Option<Box<dyn Connection>> {
    let mut conn = CONNECTION.with(|c| c.take())?;
    if conn.fingerprint() != fingerprint {
        return None;
    }
    conn.update_node_id(node_id);
    Some(conn)
}

/// Park a connection in this thread's slot so the next task that runs here
/// finds it instead of opening another one.
fn put_tlocal_connection(conn: Box<dyn Connection>) {
    CONNECTION.with(|c| c.replace(Some(conn)));
}

/// Borrow the current thread-local connection or create one if it's not set yet.
///
/// A guard is returned. When destroyed, the guard returns the connection to
/// the thread-local variable. If another connection became the thread-local
/// in the mean time, that connection is dropped and the return proceeds as
/// normal.
#[tracing::instrument(skip(engine, state), level = "trace")]
pub(crate) fn borrow_tlocal_connection<'a>(
    engine: &dyn AdapterEngine,
    state: Option<&State>,
    node_id: Option<String>,
) -> AdapterResult<ConnectionGuard<'a>> {
    let config =
        crate::engine::resolve_connection_config(engine.adapter_type(), engine.get_config(), state);
    // Default (no override): reuse the cached fingerprint instead of a real
    // `Auth::configure()` pass.
    let fingerprint = match &config {
        Cow::Borrowed(_) => engine.fingerprint(),
        Cow::Owned(_) => engine.fingerprint_for_config(config.as_ref())?,
    };
    borrow_tlocal_connection_impl(
        engine.adapter_type(),
        state,
        node_id,
        fingerprint,
        |state, node_id| engine.new_connection(state, node_id),
    )
}

pub(crate) fn borrow_tlocal_connection_impl<'a>(
    adapter_type: AdapterType,
    state: Option<&State>,
    node_id: Option<String>,
    engine_fingerprint: u64,
    new_connection_fn: impl Fn(Option<&State>, Option<String>) -> AdapterResult<Box<dyn Connection>>,
) -> AdapterResult<ConnectionGuard<'a>> {
    let conn = match take_tlocal_connection(node_id.clone(), engine_fingerprint) {
        Some(conn) => conn,
        None => new_connection_fn(state, node_id)?,
    };
    let mut guard = ConnectionGuard::new(conn);
    // DuckDB connection are cheap to create, but if long-lived, prevent other processes (not
    // other threads!) from connecting to the same database file. Set the guard to not persist in
    // this case, so the connection is dropped immediately after use instead of being returned
    // to the thread-local. This gives other processes a chance to acquire a connection to the
    // same database file.
    guard.persist = adapter_type != AdapterType::DuckDB;
    Ok(guard)
}

/// A connection wrapper that automatically returns the connection to the thread local when dropped.
/// This ensures that for a single thread, a connection is reused across multiple operations.
pub struct ConnectionGuard<'a> {
    conn: Option<Box<dyn Connection>>,
    /// Whether to return the connection to the thread-local on drop.
    persist: bool,
    _phantom: PhantomData<&'a ()>,
}

impl ConnectionGuard<'_> {
    fn new(conn: Box<dyn Connection>) -> Self {
        Self {
            conn: Some(conn),
            persist: true,
            _phantom: PhantomData,
        }
    }
}
impl Deref for ConnectionGuard<'_> {
    type Target = Box<dyn Connection>;

    fn deref(&self) -> &Self::Target {
        self.conn.as_ref().unwrap()
    }
}
impl DerefMut for ConnectionGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.conn.as_mut().unwrap()
    }
}
impl Drop for ConnectionGuard<'_> {
    fn drop(&mut self) {
        if self.persist {
            let conn = self.conn.take();
            CONNECTION.with(|c| c.replace(conn));
        } else if let Some(conn) = self.conn.as_ref() {
            emit_trace_event(|| {
                (
                    AdapterConnectionClose {
                        repr: format!("{conn:?}"),
                    }
                    .into(),
                    None,
                )
            });
        }
    }
}

mod pri {
    use super::*;

    /// A wrapper around a [Connection] stored in thread-local storage
    ///
    /// The point of this struct is to avoid calling the `Drop` destructor on
    /// the wrapped [Connection] during process exit, which dead locks on
    /// Windows.
    pub(super) struct TlsConnectionContainer(RefCell<Option<Box<dyn Connection>>>);

    impl TlsConnectionContainer {
        pub(super) fn new() -> Self {
            TlsConnectionContainer(RefCell::new(None))
        }

        pub(super) fn replace(&self, conn: Option<Box<dyn Connection>>) {
            let prev = self.take();
            *self.0.borrow_mut() = conn;
            if let Some(prev_conn) = prev {
                // We should avoid nested borrows because they mean we are creating more
                // than one connection when one would be sufficient. But if we reached
                // this branch, we did exactly that (!).
                //
                //     {
                //       let outer_guard = adapter.borrow_tlocal_connection()?;
                //       f(outer_guard.as_mut());  // Pass the conn as ref. GOOD.
                //       {
                //         // We tried to borrow, but a new connection had to
                //         // be created. BAD.
                //         let inner_guard = adapter.borrow_tlocal_connection()?;
                //         ...
                //       }  // Connection from inner_guard returns to CONNECTION.
                //     }  // Connection from outer_guard is returning to CONNECTION,
                //        // but one was already there -- the one from inner_guard.
                //
                // We hope to not reach this branch, but if we do, just close the
                // previous connection and move on.
                drop(prev_conn);
                // An assert could be added here to help finding code that creates
                // a connection instead of taking one as a parameter so that the
                // outermost caller can pass the thread-local one by reference.
                too_many_tlocal_connections();
            }
        }

        pub(super) fn take(&self) -> Option<Box<dyn Connection>> {
            self.0.borrow_mut().take()
        }
    }

    impl Drop for TlsConnectionContainer {
        fn drop(&mut self) {
            std::mem::forget(self.take());
        }
    }

    #[inline(never)]
    fn too_many_tlocal_connections() {
        // set a breakpoint on this function to find where nested connections guards are created
        debug_assert!(false, "nested connection guards detected");
    }
}

/// Connection factory that hands a worker thread its cached connection, or
/// creates one via an [`AdapterEngine`] if that thread has none.
///
/// [`MapReduce`] calls both methods from the same worker thread, so a
/// connection returned by `recycle_connection` is the one the next task on that
/// worker picks up.
///
/// How many connections are created concurrently is decided by [MapReduce]
/// from the number of keys and the parallelism of the `dbt_runtime` pool.
pub struct AdapterConnectionFactory {
    engine: Arc<dyn AdapterEngine>,
}

impl AdapterConnectionFactory {
    pub fn new(engine: Arc<dyn AdapterEngine>) -> Self {
        Self { engine }
    }
}

impl ConnectionFactory for AdapterConnectionFactory {
    type Error = Cancellable<AdapterError>;

    fn new_connection(&self, node_id: Option<&str>) -> Result<Box<dyn Connection>, Self::Error> {
        let node_id = node_id.map(|s| s.to_string());
        match take_tlocal_connection(node_id.clone(), self.engine.fingerprint()) {
            Some(conn) => Ok(conn),
            None => self
                .engine
                .new_connection(None, node_id)
                .map_err(Cancellable::Error),
        }
    }

    fn recycle_connection(&self, conn: Box<dyn Connection>) {
        // DuckDB connections must not outlive this call: a long-lived one keeps
        // other processes from opening the same database file. Dropping `conn`
        // here closes it instead of parking it in the thread-local slot, which
        // is also why `borrow_tlocal_connection_impl` never caches one.
        if self.engine.adapter_type() == AdapterType::DuckDB {
            drop(conn);
        } else {
            put_tlocal_connection(conn);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::adapter_impl::AdapterImpl;
    use crate::engine::NoopConnection;
    use crate::sql_types::DefaultTypeOps;
    use crate::stmt_splitter::DefaultStmtSplitter;
    use dbt_schemas::schemas::relations::DEFAULT_RESOLVED_QUOTING;
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn make_conn() -> Box<dyn Connection> {
        Box::new(NoopConnection)
    }

    fn mock_engine(adapter_type: AdapterType) -> Arc<dyn AdapterEngine> {
        let adapter = AdapterImpl::new_mock(
            adapter_type,
            BTreeMap::new(),
            DEFAULT_RESOLVED_QUOTING,
            Arc::new(DefaultTypeOps::new(adapter_type)),
            Arc::new(DefaultStmtSplitter),
        );
        Arc::clone(adapter.engine())
    }

    #[test]
    fn borrow_tlocal_connection_no_override_uses_cached_fingerprint() {
        run_on_fresh_thread(|| {
            CONNECTION.with(|c| assert!(c.take().is_none()));
            let engine = mock_engine(AdapterType::Databricks);

            let guard = borrow_tlocal_connection(engine.as_ref(), None, None).unwrap();
            drop(guard);
            drop_thread_local_connection();
        });
    }

    #[test]
    fn borrow_tlocal_connection_databricks_override_uses_fingerprint_for_config() {
        run_on_fresh_thread(|| {
            CONNECTION.with(|c| assert!(c.take().is_none()));
            let backend = crate::adapter::adapter_factory::backend_of(AdapterType::Databricks);
            let auth: Arc<dyn dbt_auth::Auth> = dbt_auth::auth_for_backend(backend).into();
            let config = dbt_auth::AdapterConfig::new(dbt_yaml::Mapping::from_iter([(
                "compute".into(),
                true.into(),
            )]));
            let engine: Arc<dyn AdapterEngine> = Arc::new(crate::engine::AdbcEngine::new_mock(
                AdapterType::Databricks,
                auth,
                config,
                DEFAULT_RESOLVED_QUOTING,
                Arc::new(DefaultTypeOps::new(AdapterType::Databricks)),
                Arc::new(DefaultStmtSplitter),
                Arc::new(crate::cache::RelationCache::default()),
                BTreeMap::new(),
            ));

            let mut env = minijinja::Environment::new();
            let databricks_attr = BTreeMap::from([("databricks_compute", "large_warehouse")]);
            let model = BTreeMap::from([("databricks_attr", databricks_attr)]);
            env.add_global("model", minijinja::value::Value::from_serialize(&model));
            let state = State::new_for_env(&env);

            // `compute` block + matching override: exercises `fingerprint_for_config`,
            // which must short-circuit in mock mode (no real auth data in `config`).
            let guard = borrow_tlocal_connection(engine.as_ref(), Some(&state), None).unwrap();
            drop(guard);
            drop_thread_local_connection();
        });
    }

    #[test]
    fn tls_container_stores_tlocal_connections() {
        let c = pri::TlsConnectionContainer::new();
        assert!(c.take().is_none());
        c.replace(Some(make_conn()));
        assert!(c.take().is_some());
        assert!(c.take().is_none());
    }

    // A fresh worker thread isolates TLS state, and is also the only place a
    // connection may be created: `new_connection` asserts it. Nothing else is
    // shared between these cases now that connections live only in the slot of
    // the thread that opened them, so they need no lock against each other.
    fn run_on_fresh_thread(f: impl FnOnce() + Send + 'static) {
        dbt_runtime::testing::block_on_worker(f);
    }

    #[test]
    fn tlocal_connection_lifecycle() {
        run_on_fresh_thread(|| {
            // Ensure thread-local starts empty
            CONNECTION.with(|c| {
                let conn = c.take();
                assert!(conn.is_none());
            });

            let new_connection_calls = AtomicU64::new(0);
            let new_connection_fn = |_: Option<&State>, _: Option<String>| {
                new_connection_calls.fetch_add(1, Ordering::Relaxed);
                Ok(make_conn())
            };
            let guard = borrow_tlocal_connection_impl(
                AdapterType::Snowflake,
                None,
                None,
                0,
                new_connection_fn,
            );
            assert_eq!(new_connection_calls.load(Ordering::Relaxed), 1);
            CONNECTION.with(|c| {
                // Connection is taken, so thread-local should be empty
                assert!(c.take().is_none());
            });
            drop(guard);
            CONNECTION.with(|c| {
                // Connection should be returned to thread-local after guard is dropped
                let conn = c.take();
                assert!(conn.is_some());
                c.replace(conn); // put it back for the next borrow
            });

            // The connection stays in the slot once the node that used it is
            // done, so the next node scheduled on this thread borrows it
            // instead of opening another one.
            let _guard = borrow_tlocal_connection_impl(
                AdapterType::Snowflake,
                None,
                None,
                0,
                new_connection_fn,
            );
            assert_eq!(
                new_connection_calls.load(Ordering::Relaxed),
                1,
                "connection should be reused from the thread-local slot"
            );
        });
    }

    #[test]
    fn drop_thread_local_connection_empties_the_slot() {
        run_on_fresh_thread(|| {
            // Put a connection in the thread-local
            CONNECTION.with(|c| {
                c.replace(Some(make_conn()));
            });

            // Ensure the connection is in the thread-local
            CONNECTION.with(|c| {
                let conn = c.take();
                assert!(conn.is_some());
                c.replace(conn); // put it back
            });

            drop_thread_local_connection();

            // Ensure thread-local is empty (connection was dropped)
            CONNECTION.with(|c| {
                let conn = c.take();
                assert!(conn.is_none());
            });
        });
    }

    // A fingerprint mismatch must discard and recreate the connection — this is
    // what routes a model to a different `databricks_compute`.
    #[test]
    fn mismatched_fingerprint_forces_new_connection() {
        run_on_fresh_thread(|| {
            CONNECTION.with(|c| assert!(c.take().is_none()));

            let calls = AtomicU64::new(0);
            let new_connection_fn = |_: Option<&State>, _: Option<String>| {
                calls.fetch_add(1, Ordering::Relaxed);
                Ok(make_conn())
            };
            // `make_conn()` fingerprints as 0, so any non-zero value here is a
            // deliberate mismatch — reused across both scenarios below.
            let mismatched_fingerprint = 42;

            CONNECTION.with(|c| c.replace(Some(make_conn())));
            let guard = borrow_tlocal_connection_impl(
                AdapterType::Databricks,
                None,
                None,
                mismatched_fingerprint,
                new_connection_fn,
            )
            .unwrap();
            assert_eq!(
                calls.load(Ordering::Relaxed),
                1,
                "non-matching thread-local connection should be discarded"
            );
            drop(guard);
            drop_thread_local_connection();

            // A matching fingerprint (0) reuses the cached connection instead.
            CONNECTION.with(|c| c.replace(Some(make_conn())));
            let guard = borrow_tlocal_connection_impl(
                AdapterType::Databricks,
                None,
                None,
                0,
                new_connection_fn,
            )
            .unwrap();
            assert_eq!(
                calls.load(Ordering::Relaxed),
                1,
                "matching thread-local connection should be reused, not recreated"
            );
            drop(guard);
        });
    }
}
