use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::Arc;

use fl_solve::{Calibration, Engine, Solution, SolveHints};

use crate::store::{JobStatus, Store};

pub struct Task {
    pub job: u64,
    /// The upload, shared: an endpoint that also returns a FITS still needs
    /// the original bytes after the worker is done with them.
    pub bytes: Arc<Vec<u8>>,
    pub hints: SolveHints,
}

pub enum AnySolver {
    Real(Box<Engine>),
    /// Returns a canned calibration after a short delay — lets the full API
    /// be integration-tested without index files (FAINT_LIGHT_FAKE=1).
    Fake,
}

/// Spawn the solve worker on a dedicated OS thread. The queue is bounded:
/// a home server solving for one imaging rig never queues deep, and
/// upload backpressure beats memory bloat.
pub fn spawn(store: Arc<Store>, solver: AnySolver) -> SyncSender<Task> {
    let (tx, rx) = sync_channel::<Task>(8);
    std::thread::Builder::new()
        .name("fl-solver".into())
        .spawn(move || {
            while let Ok(task) = rx.recv() {
                let t0 = std::time::Instant::now();
                let result = match &solver {
                    AnySolver::Real(engine) => engine.solve_image(&task.bytes, &task.hints),
                    AnySolver::Fake => {
                        std::thread::sleep(std::time::Duration::from_millis(300));
                        Ok(fake_solution())
                    }
                };
                match result {
                    Ok(sol) => {
                        tracing::info!(
                            job = task.job,
                            ms = t0.elapsed().as_millis() as u64,
                            ra = sol.calibration.ra,
                            dec = sol.calibration.dec,
                            pixscale = sol.calibration.pixscale,
                            "job solved"
                        );
                        store.finish(task.job, JobStatus::Success(Box::new(sol)));
                    }
                    Err(e) => {
                        tracing::warn!(
                            job = task.job,
                            ms = t0.elapsed().as_millis() as u64,
                            "job failed: {e}"
                        );
                        store.finish(task.job, JobStatus::Failure(e.to_string()));
                    }
                }
            }
        })
        .expect("spawn solver thread");
    tx
}

fn fake_solution() -> Solution {
    let wcs = fl_solve::tanwcs::TanWcs {
        crval: [83.94, -5.74],
        crpix: [1000.0, 750.0],
        cd: [[-0.0006, 0.0], [0.0, 0.0006]],
    };
    Solution {
        width: 2000.0,
        height: 1500.0,
        calibration: Calibration {
            ra: 83.94,
            dec: -5.74,
            radius: 1.87,
            pixscale: 2.21,
            orientation: 93.77,
            parity: 1.0,
        },
        wcs,
        logodds: 200.0,
        nmatch: 42,
        index_id: 0,
    }
}
