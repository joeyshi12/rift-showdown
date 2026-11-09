use rift_server::{load_state, router, spawn_maintenance, Config};
use std::path::PathBuf;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into())).init();
    let cfg = Config {
        data_dir: PathBuf::from(std::env::var("RIFT_DATA").unwrap_or_else(|_| "data".into())),
        dist_dir: PathBuf::from(std::env::var("RIFT_DIST").unwrap_or_else(|_| "dist".into())),
        img_cache: std::env::var("RIFT_IMG_CACHE").ok().filter(|v| !v.trim().is_empty()).map(PathBuf::from),
    };
    let bind = std::env::var("RIFT_BIND").unwrap_or_else(|_| "127.0.0.1:8080".into());
    let state = load_state(&cfg).unwrap_or_else(|e| {
        eprintln!("failed to start: {e}");
        std::process::exit(1);
    });
    spawn_maintenance(&state);
    let app = router(state, cfg.dist_dir.clone());
    let listener = tokio::net::TcpListener::bind(&bind).await.unwrap_or_else(|e| {
        eprintln!("cannot bind {bind}: {e}");
        std::process::exit(1);
    });
    tracing::info!("Rift Showdown listening on http://{bind} (dist: {})", cfg.dist_dir.display());
    // Stop at once on Ctrl+C or SIGTERM (docker stop). Rooms and matches only live
    // in memory, so there is nothing to drain.
    tokio::select! {
        r = axum::serve(listener, app) => r.unwrap(),
        _ = shutdown_signal() => tracing::info!("shutting down"),
    }
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
}
