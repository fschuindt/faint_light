use std::collections::{HashMap, VecDeque};
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

/// A file a solve produced and a client may come back for: the solved FITS,
/// the sky chart.
#[derive(Debug, Clone)]
pub struct Artifact {
    pub content_type: &'static str,
    /// Download name, for Content-Disposition.
    pub filename: String,
    pub bytes: Arc<Vec<u8>>,
}

/// How much of the artifact cache we keep. Solved FITS files are as large as
/// the images that produced them, so this is a byte budget rather than a
/// count: clients fetch them moments after solving, and an unbounded map
/// would be a memory leak with a slow fuse.
const ARTIFACT_BUDGET: usize = 256 << 20;

/// In-memory job store. Jobs are ephemeral — a restart loses history, which
/// is fine for a home plate-solving box (clients poll within minutes).
#[derive(Default)]
pub struct Store {
    next: AtomicU64,
    subs: RwLock<HashMap<u64, Submission>>,
    jobs: RwLock<HashMap<u64, Job>>,
    artifacts: RwLock<Artifacts>,
}

#[derive(Default)]
struct Artifacts {
    /// Insertion-ordered so the oldest is the first evicted.
    items: VecDeque<((u64, &'static str), Artifact)>,
    bytes: usize,
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

    /// Store a job's output file, evicting the oldest ones once the cache
    /// exceeds its byte budget.
    pub fn put_artifact(&self, job: u64, kind: &'static str, artifact: Artifact) {
        let mut a = self.artifacts.write().unwrap();
        // Re-solving a job replaces its output rather than stacking another.
        a.items.retain(|((j, k), _)| (*j, *k) != (job, kind));
        a.items.push_back(((job, kind), artifact));
        a.bytes = a.items.iter().map(|(_, art)| art.bytes.len()).sum();
        while a.bytes > ARTIFACT_BUDGET && a.items.len() > 1 {
            if let Some((_, dropped)) = a.items.pop_front() {
                a.bytes = a.bytes.saturating_sub(dropped.bytes.len());
            }
        }
    }

    pub fn artifact(&self, job: u64, kind: &str) -> Option<Artifact> {
        self.artifacts
            .read()
            .unwrap()
            .items
            .iter()
            .find(|((j, k), _)| *j == job && *k == kind)
            .map(|(_, art)| art.clone())
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
    let d = t.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default();
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

    fn artifact(len: usize) -> Artifact {
        Artifact {
            content_type: "application/fits",
            filename: "x.fits".into(),
            bytes: Arc::new(vec![0u8; len]),
        }
    }

    #[test]
    fn artifacts_are_retrievable_and_replaceable() {
        let s = Store::default();
        s.put_artifact(1, "fits", artifact(10));
        s.put_artifact(1, "skyview", artifact(20));
        assert_eq!(s.artifact(1, "fits").unwrap().bytes.len(), 10);
        assert_eq!(s.artifact(1, "skyview").unwrap().bytes.len(), 20);
        assert!(s.artifact(2, "fits").is_none());
        // Re-solving the same job replaces rather than accumulates.
        s.put_artifact(1, "fits", artifact(30));
        assert_eq!(s.artifact(1, "fits").unwrap().bytes.len(), 30);
        assert_eq!(s.artifacts.read().unwrap().items.len(), 2);
    }

    #[test]
    fn the_oldest_artifacts_are_evicted_past_the_budget() {
        let s = Store::default();
        let big = ARTIFACT_BUDGET / 2 + 1;
        for job in 1..=3 {
            s.put_artifact(job, "fits", artifact(big));
        }
        assert!(s.artifact(1, "fits").is_none(), "oldest should be gone");
        assert!(s.artifact(3, "fits").is_some(), "newest must survive");
        assert!(s.artifacts.read().unwrap().bytes <= ARTIFACT_BUDGET);
    }

    #[test]
    fn an_oversized_artifact_is_still_served_once() {
        // A single file larger than the whole budget must not evict itself.
        let s = Store::default();
        s.put_artifact(1, "fits", artifact(ARTIFACT_BUDGET + 1));
        assert!(s.artifact(1, "fits").is_some());
    }
}
