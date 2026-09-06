use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::SystemTime;

use fl_solve::Solution;

#[derive(Debug, Clone)]
pub enum JobStatus {
    Solving,
    Success(Box<Solution>),
    Failure(String),
}

#[derive(Debug, Clone)]
pub struct Job {
    pub status: JobStatus,
    pub filename: String,
    pub started: SystemTime,
    pub finished: Option<SystemTime>,
}

#[derive(Debug, Clone)]
pub struct Submission {
    pub job: u64,
    pub received: SystemTime,
}

/// In-memory job store. Jobs are ephemeral — a restart loses history, which
/// is fine for a home plate-solving box (clients poll within minutes).
#[derive(Default)]
pub struct Store {
    next: AtomicU64,
    subs: RwLock<HashMap<u64, Submission>>,
    jobs: RwLock<HashMap<u64, Job>>,
    /// Rendered sky-chart SVGs by job id (skyview endpoint).
    charts: RwLock<HashMap<u64, Arc<String>>>,
}

impl Store {
    /// Create a submission and its job in one step (nova delays job
    /// creation; clients handle both, and immediate creation needs no
    /// state machine).
    pub fn create(&self, filename: String) -> (u64, u64) {
        let sub_id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let job_id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let now = SystemTime::now();
        self.subs.write().unwrap().insert(
            sub_id,
            Submission {
                job: job_id,
                received: now,
            },
        );
        self.jobs.write().unwrap().insert(
            job_id,
            Job {
                status: JobStatus::Solving,
                filename,
                started: now,
                finished: None,
            },
        );
        (sub_id, job_id)
    }

    pub fn submission(&self, id: u64) -> Option<Submission> {
        self.subs.read().unwrap().get(&id).cloned()
    }

    pub fn job(&self, id: u64) -> Option<Job> {
        self.jobs.read().unwrap().get(&id).cloned()
    }

    pub fn put_chart(&self, job_id: u64, svg: String) {
        self.charts.write().unwrap().insert(job_id, Arc::new(svg));
    }

    pub fn chart(&self, job_id: u64) -> Option<Arc<String>> {
        self.charts.read().unwrap().get(&job_id).cloned()
    }

    pub fn finish(&self, job_id: u64, status: JobStatus) {
        if let Some(job) = self.jobs.write().unwrap().get_mut(&job_id) {
            job.status = status;
            job.finished = Some(SystemTime::now());
        }
    }
}

/// nova-style timestamp: "YYYY-MM-DD HH:MM:SS.ffffff".
pub fn format_time(t: SystemTime) -> String {
    let d = t
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = d.as_secs() as i64;
    let micros = d.subsec_micros();
    let (days, rem) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let dd = doy - (153 * mp + 2) / 5 + 1;
    let mm = if mp < 10 { mp + 3 } else { mp - 9 };
    let yy = if mm <= 2 { y + 1 } else { y };
    format!("{yy:04}-{mm:02}-{dd:02} {h:02}:{m:02}:{s:02}.{micros:06}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn time_formatting() {
        let t = SystemTime::UNIX_EPOCH + Duration::from_secs(1_752_100_000);
        // 2025-07-09 22:26:40 UTC
        assert_eq!(format_time(t), "2025-07-09 22:26:40.000000");
    }

    #[test]
    fn create_and_finish() {
        let s = Store::default();
        let (sub, job) = s.create("a.fits".into());
        assert_eq!(s.submission(sub).unwrap().job, job);
        assert!(matches!(s.job(job).unwrap().status, JobStatus::Solving));
        s.finish(job, JobStatus::Failure("x".into()));
        assert!(matches!(s.job(job).unwrap().status, JobStatus::Failure(_)));
        assert!(s.job(job).unwrap().finished.is_some());
    }
}
