//! Index collection + page-cache budget management.
//!
//! All index files in the configured directory are mmapped at startup (cheap:
//! address space only). The user-configurable budget caps how many bytes we
//! actively pin/prefetch into RAM (`madvise(WILLNEED)`); the OS page cache
//! does the rest. Prewarm priority follows solve activity: the indexes whose
//! quad scales match the last successful solve are warmed first.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use crate::{IndexFile, Result};

pub struct IndexCache {
    /// All indexes, sorted by ascending quad scale (finest first).
    pub indexes: Vec<Arc<IndexFile>>,
    budget_bytes: usize,
    warmed_bytes: AtomicUsize,
}

impl IndexCache {
    /// Open every `*.fits`/`*.fit` file in `dir` that parses as an index.
    /// Files that fail to parse are skipped with a warning on stderr.
    pub fn open_dir(dir: &Path, budget_gb: f64) -> Result<IndexCache> {
        let mut indexes = Vec::new();
        let mut entries: Vec<_> = std::fs::read_dir(dir)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                matches!(
                    p.extension().and_then(|e| e.to_str()),
                    Some("fits") | Some("fit")
                )
            })
            .collect();
        entries.sort();
        for path in entries {
            match IndexFile::open(&path) {
                Ok(idx) => indexes.push(Arc::new(idx)),
                Err(e) => eprintln!("faint_light: skipping {}: {e}", path.display()),
            }
        }
        indexes.sort_by(|a, b| {
            a.meta
                .scale_lower_rad
                .total_cmp(&b.meta.scale_lower_rad)
                .then(a.meta.index_id.cmp(&b.meta.index_id))
        });
        Ok(IndexCache {
            indexes,
            budget_bytes: (budget_gb * 1e9) as usize,
            warmed_bytes: AtomicUsize::new(0),
        })
    }

    pub fn total_bytes(&self) -> usize {
        self.indexes.iter().map(|i| i.byte_size()).sum()
    }

    /// Prefetch indexes in `priority` order (indices into `self.indexes`)
    /// until the RAM budget is spent. Advisory: repeated calls re-advise,
    /// which keeps hot files resident under memory pressure.
    pub fn prewarm(&self, priority: impl Iterator<Item = usize>) {
        self.warmed_bytes.store(0, Ordering::Relaxed);
        for i in priority {
            let Some(idx) = self.indexes.get(i) else { continue };
            let used = self.warmed_bytes.load(Ordering::Relaxed);
            if used + idx.byte_size() > self.budget_bytes {
                continue;
            }
            idx.prewarm();
            self.warmed_bytes.fetch_add(idx.byte_size(), Ordering::Relaxed);
        }
    }
}
