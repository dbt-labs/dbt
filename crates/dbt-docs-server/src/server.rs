use std::io;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use tracing::info;

use crate::DocsServeArgs;
use crate::assets::serve_assets;
use crate::providers::Providers;
use crate::resolve_info_schema_dir;
use crate::state::AppState;

/// Run the docs server. Must be called from within a tokio runtime —
/// `dbt-main` already initialises one before dispatching commands, so
/// this crate intentionally does not build its own. The caller is
/// responsible for constructing the [`Providers`]; the SA crate itself
/// never touches `dbt-index` or any other proprietary surface.
pub async fn run_with_args(args: Arc<DocsServeArgs>, providers: Providers) -> io::Result<()> {
    let data_dir = resolve_info_schema_dir(&args);
    let state = Arc::new(AppState::new(
        data_dir,
        providers,
        args.has_dbt_state,
        args.send_anonymous_usage_stats,
    ));
    serve(args, state).await
}

async fn serve(args: Arc<DocsServeArgs>, state: Arc<AppState>) -> io::Result<()> {
    serve_with_shutdown(args, state, shutdown_signal()).await
}

async fn serve_with_shutdown<F>(
    args: Arc<DocsServeArgs>,
    state: Arc<AppState>,
    shutdown: F,
) -> io::Result<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    // No routes. The site is static: `index.html`, hashed assets, and the parquet
    // the browser queries itself. Everything the `/api/v1/*` handlers used to compute
    // now runs client-side against that parquet, so there is nothing left to serve but
    // files.
    let app = Router::new();

    // A generated site wins over the embedded bundle: it is the only source with
    // the bootstrap injected and the parquet artifacts beside it.
    let app = match args.site_dir.clone() {
        Some(site_dir) => app.fallback(move |uri| {
            let site_dir = site_dir.clone();
            async move { crate::assets::serve_site_dir(&site_dir, uri).await }
        }),
        None => app.fallback(serve_assets),
    }
    .with_state(state.clone());

    let bind = format!("{}:{}", args.host, args.port);
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    let local_addr = listener.local_addr()?;
    let url = format!("http://{local_addr}");

    match &args.site_dir {
        Some(site_dir) => eprintln!("dbt docs serve: serving site {}", site_dir.display()),
        None => eprintln!("dbt docs serve: serving the embedded bundle (no generated site)"),
    }
    eprintln!("dbt docs serve: serving from {}", state.data_dir.display());
    eprintln!("dbt docs serve: listening on {url}");
    info!(target: "dbt_docs_server", data_dir = %state.data_dir.display(), %url, "started");

    if !args.no_open {
        if let Err(err) = try_open_browser(&url) {
            eprintln!("dbt docs serve: could not open browser ({err}); visit {url} manually");
        }
    }

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await?;
    Ok(())
}

/// Upper bound on how long to drain in-flight requests after a shutdown
/// signal before forcing exit. Kept under the typical Kubernetes
/// `terminationGracePeriodSeconds` (30s) so we exit cleanly before SIGKILL.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(25);

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(_) => {
                std::future::pending::<()>().await;
                return;
            }
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = term.recv() => {},
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }

    // Nothing to flush on shutdown any more: analytics is emitted by the browser
    // straight to the collector (ADR-10), so the server holds no buffered events.
    info!(
        target: "dbt_docs_server",
        grace_secs = SHUTDOWN_GRACE.as_secs(),
        "shutdown signal received; draining in-flight requests"
    );
    eprintln!(
        "dbt docs serve: shutdown signal received; draining in-flight requests (grace {}s)",
        SHUTDOWN_GRACE.as_secs()
    );

    // Bound the drain: force exit if in-flight requests do not complete in time.
    tokio::spawn(async {
        tokio::time::sleep(SHUTDOWN_GRACE).await;
        eprintln!("dbt docs serve: drain grace period elapsed; forcing exit");
        std::process::exit(0);
    });
}

fn try_open_browser(url: &str) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(url).status()?;
        Ok(())
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open").arg(url).status()?;
        Ok(())
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .status()?;
        Ok(())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = url;
        Err(io::Error::other("auto-open not supported on this platform"))
    }
}

#[cfg(test)]
#[path = "server_tests.rs"]
mod server_tests;
