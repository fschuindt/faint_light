//! The faint_light server as a library.
//!
//! Two APIs share one solver:
//! - `/nova` — the nova.astrometry.net contract, for NINA and friends.
//! - `/api/v1` — Faint Light's own API. The root is reserved for the web UI
//!   that will front it.
//!
//! `main.rs` is a thin wrapper over this; the optional GUI (`--features
//! gui`) drives the same pieces, which is why starting the HTTP listener is
//! separable from building the solve engine: the GUI stops and starts the
//! listener whenever the user changes the port, and reloading gigabytes of
//! index files each time would make that unusable.

// Handlers carry a ready-made HTTP response as their error type, which is
// exactly as large as a response should be.
#![allow(clippy::result_large_err)]

pub mod config;
pub mod form;
#[cfg(feature = "gui")]
pub mod gui;
pub mod nova;
pub mod solve;
pub mod state;
pub mod store;
pub mod v1;
pub mod worker;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::DefaultBodyLimit;
use axum::routing::get;
use axum::Router;
use tokio::runtime::Handle;

/// The released version, e.g. `0.1.0a`.
///
/// `Cargo.toml` has to hold strict semver, which cannot express a suffix
/// like `0.1.0a`, so the human-facing version lives in `/VERSION` and this
/// is what every user-visible surface prints.
pub fn version() -> &'static str {
    include_str!("../../../VERSION").trim()
}

pub use crate::config::Config;
pub use crate::state::AppState;
pub use crate::store::{Job, JobStatus, Store};
pub use crate::worker::AnySolver;

pub fn router(state: AppState) -> Router {
    Router::new()
        .nest("/nova", nova::router())
        .merge(v1::router())
        .route("/", get(index))
        .layer(DefaultBodyLimit::max(256 << 20))
        .with_state(state)
}

async fn index() -> String {
    format!(
        "faint_light {}\n\nPOST /api/v1/solve   plate solve an image\n     \
         /nova           nova.astrometry.net-compatible API\n",
        version()
    )
}

/// The expensive half of the server: the solve engine, the worker thread it
/// runs on, and the job store. Build it once; bind and unbind listeners
/// against it as often as you like.
pub struct Backend {
    pub state: AppState,
    /// The configuration this was built from, so a caller can tell whether a
    /// restart needs a new engine or just a new listener.
    pub config: Config,
}

impl Backend {
    /// Load the index files and start the solve worker.
    pub fn new(cfg: Config) -> Result<Backend, String> {
        let solver = if cfg.fake {
            tracing::warn!("FAINT_LIGHT_FAKE=1 - serving canned solutions");
            AnySolver::Fake
        } else {
            let engine = fl_solve::Engine::new(fl_solve::EngineConfig {
                index_dir: cfg.index_dir.clone(),
                cache_gb: cfg.cache_gb,
                default_scale_lo: cfg.scale_lo,
                default_scale_hi: cfg.scale_hi,
                solve_timeout: cfg.solve_timeout,
                ..Default::default()
            })
            .map_err(|e| e.to_string())?;
            tracing::info!(indexes = engine.index_count(), "solve engine ready");
            AnySolver::Real(Box::new(engine))
        };
        let store = Arc::new(Store::default());
        let tx = worker::spawn(store.clone(), solver);
        Ok(Backend {
            state: AppState {
                store,
                tx,
                solve_timeout: cfg.solve_timeout,
            },
            config: cfg,
        })
    }
}

/// A bound, serving HTTP listener. Dropping it leaves the server running;
/// call [`ServerHandle::stop`] to shut it down.
pub struct ServerHandle {
    addr: SocketAddr,
    shutdown: tokio::sync::oneshot::Sender<()>,
}

impl ServerHandle {
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Ask the server to stop accepting connections and finish in-flight
    /// requests.
    pub fn stop(self) {
        let _ = self.shutdown.send(());
    }
}

/// Bind `addr` and serve both APIs on `rt`.
///
/// The socket is bound synchronously so that "port already in use" comes
/// back here rather than in a background task nobody is watching.
pub fn serve(rt: &Handle, state: AppState, addr: SocketAddr) -> std::io::Result<ServerHandle> {
    let listener = std::net::TcpListener::bind(addr)?;
    listener.set_nonblocking(true)?;
    let addr = listener.local_addr()?;
    let app = router(state);
    let (tx, rx) = tokio::sync::oneshot::channel();
    rt.spawn(async move {
        let listener = match tokio::net::TcpListener::from_std(listener) {
            Ok(l) => l,
            Err(e) => {
                tracing::error!("listener setup failed: {e}");
                return;
            }
        };
        tracing::info!("listening on http://{addr}");
        let served = axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = rx.await;
            })
            .await;
        match served {
            Ok(()) => tracing::info!("stopped listening on {addr}"),
            Err(e) => tracing::error!("server on {addr} stopped: {e}"),
        }
    });
    Ok(ServerHandle { addr, shutdown: tx })
}
