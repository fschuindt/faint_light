mod api;
mod config;
mod store;
mod worker;

use std::sync::Arc;

use tracing_subscriber::EnvFilter;

use crate::api::AppState;
use crate::config::Config;
use crate::store::Store;
use crate::worker::AnySolver;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cfg = Config::from_env();
    tracing::info!(?cfg, "faint_light starting");

    let solver = if cfg.fake {
        tracing::warn!("FAINT_LIGHT_FAKE=1 — serving canned solutions");
        AnySolver::Fake
    } else {
        let engine = fl_solve::Engine::new(fl_solve::EngineConfig {
            index_dir: cfg.index_dir.clone(),
            cache_gb: cfg.cache_gb,
            default_scale_lo: cfg.scale_lo,
            default_scale_hi: cfg.scale_hi,
            solve_timeout: cfg.solve_timeout,
            ..Default::default()
        });
        match engine {
            Ok(e) => {
                tracing::info!(indexes = e.index_count(), "solve engine ready");
                AnySolver::Real(Box::new(e))
            }
            Err(e) => {
                eprintln!("faint_light: {e}");
                eprintln!("hint: set FAINT_LIGHT_INDEX_DIR to a directory of astrometry.net index files");
                std::process::exit(1);
            }
        }
    };

    let store = Arc::new(Store::default());
    let tx = worker::spawn(store.clone(), solver);
    let state = AppState {
        store,
        tx,
        solve_timeout: cfg.solve_timeout,
    };
    let app = api::router(state);

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    rt.block_on(async move {
        let addr = std::net::SocketAddr::from(([0, 0, 0, 0], cfg.port));
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .unwrap_or_else(|e| panic!("bind {addr}: {e}"));
        tracing::info!("listening on http://{addr}");
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = tokio::signal::ctrl_c().await;
                tracing::info!("shutting down");
            })
            .await
            .expect("server");
    });
}
