use std::sync::mpsc::SyncSender;
use std::sync::Arc;
use std::time::Duration;

use crate::store::Store;
use crate::worker::Task;

/// What every handler needs: the job store and the queue into the solver.
#[derive(Clone)]
pub struct AppState {
    pub store: Arc<Store>,
    pub tx: SyncSender<Task>,
    /// Per-job solve budget. The synchronous endpoints wait this long, plus
    /// queueing grace, before giving up.
    pub solve_timeout: Duration,
}
