use dbt_common::FsResult;
use dbt_common::cancellation::{CancellationReport, CancellationTokenSource, TIMEOUT_AFTER_CTRL_C};
use dbt_common::fail_fast::FailFast;
use dbt_tracked_stmt::{StmtCancellationReport, cancel_all_tracked_statements};

use std::pin::Pin;
use std::time::{Duration, Instant};

/// Runs the given future while also listening for Ctrl+C (or fail-fast) signals.
///
/// Pass `fail_fast_flag=true` is the user has invoked dbt with the `--fail-fast` flag.
///
/// PRE-CONDITION: !fail_fast_flag || !fail_fast.has_triggered()
pub fn run_future_with_ctrlc_support<'a>(
    cst: CancellationTokenSource,
    future: Pin<Box<dyn Future<Output = FsResult<()>> + Send + 'a>>,
    fail_fast: FailFast,
    fail_fast_flag: bool,
) -> Pin<Box<dyn Future<Output = FsResult<Option<CancellationReport>>> + Send + 'a>> {
    use futures::future::{Either, select};

    let ctrl_c = {
        // There are two sources of cancellation: the cancellation signal
        // (triggered by Ctrl+C on Unix systems or by task cancellation on
        // Windows) and the fail-fast signal (triggered by critical errors in
        // any task). We listen to both signals concurrently and trigger the
        // same cancellation logic.
        let ctrl_c = tokio::signal::ctrl_c();
        debug_assert!(
            !fail_fast_flag || !fail_fast.has_triggered(),
            "run_future_with_ctrlc_support called with fail-fast already triggered"
        );
        let fail_fast = fail_fast.subscribe(fail_fast_flag);
        async move {
            futures::pin_mut!(ctrl_c, fail_fast);
            #[allow(clippy::let_and_return)]
            let did_fail_fast = match select(ctrl_c, fail_fast).await {
                Either::Left((Ok(_), _)) => false,
                Either::Right((Ok(_), _)) => true,
                // TODO: forward the error here and handle errors in the select() below
                Either::Left((Err(_), _)) | Either::Right((Err(_), _)) => true,
            };
            did_fail_fast // true if fail-fast triggered
        }
    };

    Box::pin(async move {
        futures::pin_mut!(ctrl_c, future);
        match select(ctrl_c, future).await {
            Either::Left((did_fail_fast, ctrl_c_future)) => {
                let ctrl_c_t0 = Instant::now();
                // Ctrl+C was received, we need to handle it gracefully.

                // Cancel all tokens issued by the main CancellationTokenSource. This
                // makes the token passed to `execute_fs_and_shutdown` return true on
                // `is_cancelled()` calls. Long-running operations and async chains
                // should periodically check the token and stop their execution handling
                // cancellation gracefully.
                cst.cancel();

                let token = cst.token();
                let cancel_stmts = tokio::task::spawn_blocking(move || {
                    let mut stmt_count = 0;
                    let mut fail_count = 0;
                    // Actively cancel all the running database operations. This tells
                    // data warehouses to stop processing potentially long-running and
                    // expensive queries.
                    let mut from_stmt_id = 0;
                    loop {
                        let report = cancel_all_tracked_statements(from_stmt_id);
                        debug_assert!(
                            report.fail_count == 0,
                            "cancel_all_tracked_statements() not expected to fail"
                        );
                        stmt_count += report.stmt_count;
                        fail_count += report.fail_count;
                        if report.stmt_count == 0 {
                            if token.is_cancelled() {
                                break;
                            }
                            std::thread::sleep(Duration::from_millis(10));
                        }
                        from_stmt_id = report.next_stmt_id;
                    }
                    StmtCancellationReport {
                        stmt_count,
                        fail_count,
                        next_stmt_id: from_stmt_id,
                    }
                });
                let timed_out = tokio::time::timeout(TIMEOUT_AFTER_CTRL_C, ctrl_c_future)
                    .await
                    .is_err();
                let final_wait_duration = ctrl_c_t0.elapsed();

                // Cancel the loop in cancel_stmts if no more statements
                // have been registered during the final wait.
                cst.cancel();
                // Wait for the last round of cancel_all_tracked_statements() to finish.
                let stmt_cancel_report = cancel_stmts.await.unwrap();
                let stmt_cancel_duration = ctrl_c_t0.elapsed() - final_wait_duration;
                let report = CancellationReport {
                    fail_fast_triggered: did_fail_fast,
                    timeout: TIMEOUT_AFTER_CTRL_C,
                    final_wait_duration,
                    timed_out,
                    stmt_cancel_duration,
                    stmt_cancel_count: stmt_cancel_report.stmt_count,
                    stmt_cancel_fail_count: stmt_cancel_report.fail_count,
                };
                Ok(Some(report))
            }
            Either::Right((res, _)) => {
                // Uninterrupted execution, just return the result without a cancellation report.
                let () = res?;
                Ok(None)
            }
        }
    })
}
