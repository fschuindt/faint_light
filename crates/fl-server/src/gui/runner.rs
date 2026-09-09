//! Start/stop control over the embedded HTTP server.
//!
//! The engine and its index cache outlive a stop, so changing the port or
//! the bind address is instant instead of costing another pass over
//! gigabytes of index files. Only a change the engine itself depends on —
//! the index directory, the cache budget, the timeout — rebuilds it.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard};

use crate::{serve, Backend, Config, ServerHandle};

/// Shared handle to the embedded server. Cloneable so the UI thread and the
/// worker doing the slow start can both hold one.
#[derive(Clone)]
pub struct Runner(Arc<Mutex<Inner>>);

struct Inner {
    rt: tokio::runtime::Runtime,
    backend: Option<Backend>,
    listener: Option<ServerHandle>,
}

impl Runner {
    pub fn new() -> std::io::Result<Runner> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("fl-http")
            .build()?;
        Ok(Runner(Arc::new(Mutex::new(Inner {
            rt,
            backend: None,
            listener: None,
        }))))
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Build (or reuse) the solve engine and bind the address.
    ///
    /// Loading index files takes seconds to tens of seconds, so call this
    /// off the UI thread. Returns the address actually bound.
    pub fn start(&self, cfg: Config) -> Result<SocketAddr, String> {
        let mut inner = self.lock();
        if inner.listener.is_some() {
            return Err("the server is already running".into());
        }
        if inner
            .backend
            .as_ref()
            .is_none_or(|b| b.config.engine_differs(&cfg))
        {
            // Drop the old engine before building the new one: two index
            // caches at once is the one way this program runs out of memory.
            inner.backend = None;
            inner.backend = Some(Backend::new(cfg.clone())?);
        }
        let state = inner
            .backend
            .as_ref()
            .expect("backend built above")
            .state
            .clone();
        let addr = SocketAddr::new(cfg.bind, cfg.port);
        let listener = serve(inner.rt.handle(), state, addr)
            .map_err(|e| format!("cannot bind {addr}: {e}"))?;
        let bound = listener.addr();
        inner.listener = Some(listener);
        Ok(bound)
    }

    /// Stop serving. The engine stays loaded, ready for the next start.
    pub fn stop(&self) {
        if let Some(listener) = self.lock().listener.take() {
            listener.stop();
        }
    }

    pub fn is_running(&self) -> bool {
        self.lock().listener.is_some()
    }

    pub fn address(&self) -> Option<SocketAddr> {
        self.lock().listener.as_ref().map(|l| l.addr())
    }
}
