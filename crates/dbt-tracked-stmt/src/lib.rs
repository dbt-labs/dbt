use core::fmt;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};

use adbc_core::PartitionedResult;
use adbc_core::error::Result;
use adbc_core::options::{OptionStatement, OptionValue};
use arrow_array::{RecordBatch, RecordBatchReader};
use arrow_schema::Schema;
use crossbeam_skiplist::SkipMap;
use dbt_adbc::Statement;
use dbt_adbc::semaphore::AcquireAllSemaphore;
use dbt_base::cancel::CancellationToken;

/// Generate a unique statement ID for each [TrackedStatement]
/// by incrementing this global atomic counter.
static NEXT_STMT_ID: AtomicU64 = AtomicU64::new(0);

/// A semaphore to ensure that during a cancellation sweep, no other thread
/// can drop the inner [Statement] of a [TrackedStatement].
static TRACKED_STMTS_SEMAPHORE: AcquireAllSemaphore = AcquireAllSemaphore::new(u32::MAX / 2 + 1);

/// A global map that tracks all [TrackedStatement]s created by the application.
///
/// The map is sorted (based on a lock-free skip list). This means iteration starts
/// from the oldest statement and goes to the newest one, including the statements
/// being created concurrently if any.
static TRACKED_STMTS: LazyLock<SkipMap<u64, TrackedEntry>> = LazyLock::new(SkipMap::new);

type MutStmtPtr = &'static mut (dyn Statement + 'static);

/// A type-erased fat pointer [1][2] that can hold a `dyn Statement` pointer.
///
/// This is a workaround before the stabilization of `ptr_metadata` in Rust [3].
///
/// [1] Also known as "wide pointer".
/// [2] https://doc.rust-lang.org/nomicon/exotic-sizes.html#dynamically-sized-types-dsts
/// [3] https://github.com/rust-lang/rust/issues/81513
#[repr(C)]
#[derive(Copy, Clone)]
struct ErasedFatPtr {
    data: usize,
    meta: usize,
}

impl ErasedFatPtr {
    #[inline(never)]
    unsafe fn new(ptr: MutStmtPtr) -> Self {
        debug_assert!(size_of::<MutStmtPtr>() == 2 * size_of::<usize>());
        // SAFETY: relying on the (arguably shaky) guarantee that fat pointers
        // are represented as a pair of machine words in memory.
        let (data, vtable): (usize, usize) = unsafe { std::mem::transmute(ptr) };
        ErasedFatPtr { data, meta: vtable }
    }

    /// Convert the fat pointer to a raw pointer.
    #[inline(never)]
    unsafe fn as_raw_ptr(&mut self) -> MutStmtPtr {
        // SAFETY: this is the reverse of `new`, which ensures that `data` and `meta`
        // are valid uintptrs to the data and vtable (fat-pointer metadata) respectively.
        unsafe { std::mem::transmute((self.data, self.meta)) }
    }
}

struct TrackedEntry {
    ptr: ErasedFatPtr,
    /// Token this statement was registered under; `None` means untracked by token.
    token: Option<CancellationToken>,
}

fn register_stmt(
    id: u64,
    stmt: Box<dyn Statement>,
    token: Option<CancellationToken>,
) -> &'static mut (dyn Statement + 'static) {
    // Leak the Box to get a 'static pointer and associate its
    // lifetime with the global static `TRACKED_STMTS` map.
    let ptr = Box::leak::<'static>(stmt);
    // SAFETY: the `ptr` is now leaked, we track its provenance in `ErasedFatPtr`,
    // and drop it manually when `unregister_stmt` is called from the destructor of
    // [TrackedStatement].
    let mut erased_ptr = unsafe { ErasedFatPtr::new(ptr) };
    TRACKED_STMTS.insert(
        id,
        TrackedEntry {
            ptr: erased_ptr,
            token,
        },
    );
    // SAFETY: we return a mutable reference to the pointer we received, now
    // we have a mutable alias to the original `Box<dyn Statement>`, but this
    // is safe because we are careful in how use access the `TRACKED_STMTS` map
    // such that we are never accessing the object through its mutable references
    // from more than one thread at a time.
    unsafe { erased_ptr.as_raw_ptr() }
}

/// Which tracked statements a cancellation sweep should act on.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum Sweep {
    /// Every tracked statement, regardless of token.
    All,
    /// Only statements whose token reports `is_cancelled()`.
    Cancelled,
}

pub struct StmtCancellationReport {
    pub stmt_count: usize,
    pub fail_count: usize,
    pub next_stmt_id: u64,
}

/// Iterate over all tracked statements and cancel them.
pub fn cancel_all_tracked_statements(from_stmt_id: u64) -> StmtCancellationReport {
    cancel_tracked_statements(from_stmt_id, Sweep::All)
}

/// Cancel every tracked statement whose own [CancellationToken] is cancelled.
pub fn cancel_cancelled_tracked_statements() -> StmtCancellationReport {
    cancel_tracked_statements(0, Sweep::Cancelled)
}

fn cancel_tracked_statements(from_stmt_id: u64, sweep: Sweep) -> StmtCancellationReport {
    let mut stmt_count = 0;
    let mut fail_count = 0;
    let mut next_stmt_id = from_stmt_id;

    if !TRACKED_STMTS.is_empty() {
        let _all_permits = TRACKED_STMTS_SEMAPHORE.acquire_all();
        for entry in TRACKED_STMTS.iter() {
            let stmt_id = *entry.key();
            if stmt_id < from_stmt_id {
                continue;
            }
            next_stmt_id = stmt_id + 1;
            let tracked = entry.value();
            if sweep == Sweep::Cancelled
                && !tracked.token.as_ref().is_some_and(|t| t.is_cancelled())
            {
                continue;
            }
            let mut erased_ptr = tracked.ptr;
            // SAFETY: all Drop handlers are blocked by the semaphore, so we
            // can dereference pointers extracted from `TRACKED_STMTS`.
            let stmt = unsafe { erased_ptr.as_raw_ptr() };
            // There is a RISK here though! `Statement::cancel()` can be called
            // from the thread running the sweep concurrently with other operations
            // running on the thread that the [Statement] is confined to. Only the
            // Drop handler is blocked by the semaphore. This is acceptable because:
            //
            // 1) the point of a sweep is to tell the database servers to cancel
            //    potentially expensive long-running queries, and the caller has
            //    already decided the results are not wanted.
            // 2) most implementations of `Statement::cancel()` are just forwarding
            //    calls to the underlying database driver, which is expected to
            //    handle concurrent cancellations gracefully.
            let res = stmt.cancel();
            stmt_count += 1;
            if res.is_err() {
                fail_count += 1;
            }
        }
    }
    StmtCancellationReport {
        stmt_count,
        fail_count,
        next_stmt_id,
    }
}

/// De-registers a statement from the global `TRACKED_STMTS` map and drops it.
///
/// IMPORTANT: must be called from the destructor of [TrackedStatement] which,
/// other than `TRACKED_STMTS`, is the only owner of the [Box<dyn Statement>]
/// alieased in `TRACKED_STMTS` at the destructor call time. [Statement]s are
/// [Send] but not [Sync], so this is always called from the thread to which
/// the [Statement] is currently confined to.
fn unregister_stmt(id: u64) {
    let _permit = TRACKED_STMTS_SEMAPHORE.acquire();
    if let Some(entry) = TRACKED_STMTS.remove(&id) {
        let mut erased_ptr = entry.value().ptr;
        // SAFETY: the drop handler is called by the thread to which the
        // [Statement] is confined to and the semaphore ensures that no
        // cancellation sweep is reading the `TRACKED_STMTS` map. And if the
        // pointer is still in the map, it means that the statement is still alive.
        let stmt = unsafe {
            let ptr = erased_ptr.as_raw_ptr();
            Box::from_raw(ptr)
        };
        drop(stmt);
    }
}

pub struct TrackedStatement {
    stmt_id: u64,
    inner_ptr: &'static mut dyn Statement,
}

impl Drop for TrackedStatement {
    fn drop(&mut self) {
        unregister_stmt(self.stmt_id);
    }
}

impl TrackedStatement {
    pub fn new(stmt: Box<dyn Statement>) -> Self {
        Self::with_token(stmt, None)
    }

    /// Track `stmt` under `token`, so [cancel_cancelled_tracked_statements] cancels
    /// it once that token is cancelled.
    pub fn with_token(stmt: Box<dyn Statement>, token: Option<CancellationToken>) -> Self {
        let stmt_id = NEXT_STMT_ID.fetch_add(1, Ordering::SeqCst);
        let ptr = register_stmt(stmt_id, stmt, token);
        Self {
            inner_ptr: ptr,
            stmt_id,
        }
    }

    #[inline]
    fn inner(&self) -> &dyn Statement {
        self.inner_ptr
    }

    #[inline]
    fn inner_mut(&mut self) -> &mut dyn Statement {
        self.inner_ptr
    }
}

impl Statement for TrackedStatement {
    fn bind(&mut self, batch: RecordBatch) -> Result<()> {
        self.inner_mut().bind(batch)
    }
    fn bind_stream(&mut self, reader: Box<dyn RecordBatchReader + Send>) -> Result<()> {
        self.inner_mut().bind_stream(reader)
    }
    fn execute<'a>(&'a mut self) -> Result<Box<dyn RecordBatchReader + Send + 'a>> {
        self.inner_mut().execute()
    }
    fn execute_update(&mut self) -> Result<Option<i64>> {
        self.inner_mut().execute_update()
    }
    fn execute_schema(&mut self) -> Result<Schema> {
        self.inner_mut().execute_schema()
    }
    fn execute_partitions(&mut self) -> Result<PartitionedResult> {
        self.inner_mut().execute_partitions()
    }
    fn get_parameter_schema(&self) -> Result<Schema> {
        self.inner().get_parameter_schema()
    }
    fn prepare(&mut self) -> Result<()> {
        self.inner_mut().prepare()
    }
    fn set_sql_query(&mut self, sql: &str) -> Result<()> {
        self.inner_mut().set_sql_query(sql)
    }
    fn set_substrait_plan(&mut self, plan: &[u8]) -> Result<()> {
        self.inner_mut().set_substrait_plan(plan)
    }
    fn cancel(&mut self) -> Result<()> {
        self.inner_mut().cancel()
    }

    // adbc_core::Optionable<Option = OptionStatement> functions -----------------------------

    fn set_option(&mut self, key: OptionStatement, value: OptionValue) -> Result<()> {
        self.inner_mut().set_option(key, value)
    }
    fn get_option_string(&self, key: OptionStatement) -> Result<String> {
        self.inner().get_option_string(key)
    }
    fn get_option_bytes(&self, key: OptionStatement) -> Result<Vec<u8>> {
        self.inner().get_option_bytes(key)
    }
    fn get_option_int(&self, key: OptionStatement) -> Result<i64> {
        self.inner().get_option_int(key)
    }
    fn get_option_double(&self, key: OptionStatement) -> Result<f64> {
        self.inner().get_option_double(key)
    }

    fn debug_fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner().debug_fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use adbc_core::error::{Error, Status};
    use dbt_base::cancel::CancellationTokenSource;

    use super::*;

    /// `TRACKED_STMTS` is process-global, so the tests below must not sweep it
    /// while another test still has statements registered.
    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// A [Statement] that only records whether `cancel()` was called.
    struct FakeStatement(Arc<AtomicBool>);

    impl Statement for FakeStatement {
        fn bind(&mut self, _batch: RecordBatch) -> Result<()> {
            unimplemented!()
        }
        fn bind_stream(&mut self, _reader: Box<dyn RecordBatchReader + Send>) -> Result<()> {
            unimplemented!()
        }
        fn execute<'a>(&'a mut self) -> Result<Box<dyn RecordBatchReader + Send + 'a>> {
            Err(Error::with_message_and_status("no", Status::NotImplemented))
        }
        fn execute_update(&mut self) -> Result<Option<i64>> {
            unimplemented!()
        }
        fn execute_schema(&mut self) -> Result<Schema> {
            unimplemented!()
        }
        fn execute_partitions(&mut self) -> Result<PartitionedResult> {
            unimplemented!()
        }
        fn get_parameter_schema(&self) -> Result<Schema> {
            unimplemented!()
        }
        fn prepare(&mut self) -> Result<()> {
            unimplemented!()
        }
        fn set_sql_query(&mut self, _sql: &str) -> Result<()> {
            Ok(())
        }
        fn set_substrait_plan(&mut self, _plan: &[u8]) -> Result<()> {
            unimplemented!()
        }
        fn cancel(&mut self) -> Result<()> {
            self.0.store(true, Ordering::Release);
            Ok(())
        }
    }

    fn tracked(token: Option<CancellationToken>) -> (TrackedStatement, Arc<AtomicBool>) {
        let cancelled = Arc::new(AtomicBool::new(false));
        let stmt = Box::new(FakeStatement(Arc::clone(&cancelled)));
        (TrackedStatement::with_token(stmt, token), cancelled)
    }

    #[test]
    fn scoped_sweep_only_cancels_statements_of_cancelled_tokens() {
        let _guard = TEST_LOCK.lock().unwrap();
        let mine = CancellationTokenSource::new();
        let theirs = CancellationTokenSource::new();
        let (_mine_stmt, mine_cancelled) = tracked(Some(mine.token()));
        let (_theirs_stmt, theirs_cancelled) = tracked(Some(theirs.token()));
        let (_untracked_stmt, untokened_cancelled) = tracked(None);

        // Nothing is cancelled yet, so a sweep is a no-op.
        assert_eq!(cancel_cancelled_tracked_statements().stmt_count, 0);

        mine.cancel();
        assert_eq!(cancel_cancelled_tracked_statements().stmt_count, 1);
        assert!(mine_cancelled.load(Ordering::Acquire));
        assert!(!theirs_cancelled.load(Ordering::Acquire));
        assert!(!untokened_cancelled.load(Ordering::Acquire));
    }

    #[test]
    fn dropping_a_tracked_statement_unregisters_it() {
        let _guard = TEST_LOCK.lock().unwrap();
        let cst = CancellationTokenSource::new();
        let (stmt, cancelled) = tracked(Some(cst.token()));
        drop(stmt);
        cst.cancel();

        assert_eq!(cancel_cancelled_tracked_statements().stmt_count, 0);
        assert!(!cancelled.load(Ordering::Acquire));
    }
}
