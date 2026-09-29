use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::oneshot;

use super::serve_with_shutdown;
use crate::DocsServeArgs;
use crate::providers::Providers;
use crate::state::AppState;

fn test_args() -> Arc<DocsServeArgs> {
    Arc::new(DocsServeArgs {
        target_path: None,
        host: "127.0.0.1".to_string(),
        port: 0,
        no_open: true,
        has_dbt_state: false,
        send_anonymous_usage_stats: true,
        site_dir: None,
    })
}

fn test_state() -> Arc<AppState> {
    Arc::new(AppState::new(
        PathBuf::from("/tmp"),
        Providers::default(),
        false,
        true,
    ))
}

#[dbt_runtime::test]
async fn shuts_down_cleanly_on_signal() {
    let (tx, rx) = oneshot::channel::<()>();
    let handle = tokio::spawn(serve_with_shutdown(test_args(), test_state(), async {
        let _ = rx.await;
    }));

    // Trigger the injected shutdown future.
    tx.send(()).unwrap();

    let result = tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("server did not shut down within timeout")
        .expect("server task panicked");
    assert!(result.is_ok(), "expected clean shutdown, got {result:?}");
}

#[dbt_runtime::test]
async fn stays_up_until_signal() {
    let (_tx, rx) = oneshot::channel::<()>();
    let mut handle = tokio::spawn(serve_with_shutdown(test_args(), test_state(), async {
        let _ = rx.await;
    }));

    // Without firing the shutdown future, the server must keep running.
    let result = tokio::time::timeout(Duration::from_millis(300), &mut handle).await;
    assert!(result.is_err(), "server resolved before shutdown signal");

    handle.abort();
}
