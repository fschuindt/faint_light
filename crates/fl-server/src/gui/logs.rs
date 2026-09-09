//! Log capture for the GUI's Logs tab.
//!
//! The GUI installs this as `tracing`'s writer, so the tab shows exactly
//! what the headless binary prints to the console — same events, same
//! filter, no second logging path to keep in step.
//!
//! Lines are also appended to a file next to the settings, so the tab still
//! has the history of previous runs. Clear empties both.

use std::collections::VecDeque;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// How many lines the tab keeps in memory. A long solving session is chatty
/// and the buffer is only ever read by a human scrolling back.
pub const CAPACITY: usize = 5_000;

/// How large the log file may grow before the oldest half is dropped. Small
/// enough to stay cheap to load at startup, large enough to hold several
/// nights of solving.
const FILE_LIMIT: u64 = 4 << 20;

/// A bounded, shareable line buffer, optionally backed by a file.
#[derive(Clone, Default)]
pub struct LogBuffer {
    lines: Arc<Mutex<Lines>>,
}

#[derive(Default)]
struct Lines {
    lines: VecDeque<String>,
    /// Total lines ever appended, so a reader can tell what it has missed.
    version: u64,
    /// Where lines are persisted, when persistence is on.
    file: Option<PathBuf>,
    /// Bytes written since the last size check, to keep the check cheap.
    unchecked: u64,
}

impl LogBuffer {
    pub fn new() -> LogBuffer {
        LogBuffer::default()
    }

    /// Append to `path` from now on, after loading whatever it already holds.
    ///
    /// Failures are reported and then ignored: a log that cannot be written
    /// is not a reason to refuse to start.
    pub fn persist_to(&self, path: PathBuf) {
        if let Some(dir) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(dir) {
                eprintln!("faint_light: {}: {e}", dir.display());
                return;
            }
        }
        let existing = std::fs::read_to_string(&path).unwrap_or_default();
        let mut l = self.lines.lock().unwrap_or_else(|e| e.into_inner());
        let tail: Vec<&str> = existing.lines().rev().take(CAPACITY).collect();
        for line in tail.into_iter().rev() {
            l.lines.push_back(line.to_string());
        }
        // Version counts lines seen, so a reader that starts at 0 gets the
        // restored history as its first splice.
        l.version = l.lines.len() as u64;
        l.unchecked = FILE_LIMIT; // force a size check on the first write
        l.file = Some(path);
    }

    /// The file lines are appended to, if any.
    pub fn file(&self) -> Option<PathBuf> {
        self.lines
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .file
            .clone()
    }

    fn push(&self, line: String) {
        let mut l = self.lines.lock().unwrap_or_else(|e| e.into_inner());
        l.append_to_file(&line);
        if l.lines.len() == CAPACITY {
            l.lines.pop_front();
        }
        l.lines.push_back(line);
        l.version += 1;
    }

    /// The whole buffer as text, with the version it was taken at.
    pub fn snapshot(&self) -> (u64, String) {
        let l = self.lines.lock().unwrap_or_else(|e| e.into_inner());
        let mut out = String::new();
        for line in &l.lines {
            out.push_str(line);
            out.push('\n');
        }
        (l.version, out)
    }

    /// The lines appended since `version`, so the tab can splice rather than
    /// rebuild. `None` means more lines were dropped than are still held,
    /// i.e. the caller fell too far behind: take a `snapshot`.
    pub fn appended_since(&self, version: u64) -> Option<(u64, String)> {
        let l = self.lines.lock().unwrap_or_else(|e| e.into_inner());
        let new = l.version.saturating_sub(version) as usize;
        if new > l.lines.len() {
            return None;
        }
        let mut out = String::new();
        for line in l.lines.iter().skip(l.lines.len() - new) {
            out.push_str(line);
            out.push('\n');
        }
        Some((l.version, out))
    }

    pub fn version(&self) -> u64 {
        self.lines.lock().unwrap_or_else(|e| e.into_inner()).version
    }

    /// Empty the tab and the file behind it. Clearing is the only thing that
    /// discards history, which is what makes the file worth keeping.
    pub fn clear(&self) {
        let mut l = self.lines.lock().unwrap_or_else(|e| e.into_inner());
        l.lines.clear();
        l.version += 1;
        if let Some(path) = &l.file {
            if let Err(e) = std::fs::write(path, "") {
                eprintln!("faint_light: cannot clear {}: {e}", path.display());
            }
        }
        l.unchecked = 0;
    }
}

impl Lines {
    fn append_to_file(&mut self, line: &str) {
        let Some(path) = self.file.clone() else {
            return;
        };
        self.unchecked += line.len() as u64 + 1;
        if self.unchecked >= 64 << 10 {
            self.unchecked = 0;
            self.trim_file(&path);
        }
        let appended = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .and_then(|mut f| writeln!(f, "{line}"));
        if let Err(e) = appended {
            // Stop trying, or every subsequent line repeats the complaint.
            eprintln!("faint_light: cannot write {}: {e}", path.display());
            self.file = None;
        }
    }

    /// Once the file passes the limit, rewrite it with its newer half.
    fn trim_file(&self, path: &std::path::Path) {
        let Ok(meta) = std::fs::metadata(path) else {
            return;
        };
        if meta.len() <= FILE_LIMIT {
            return;
        }
        let Ok(text) = std::fs::read_to_string(path) else {
            return;
        };
        let keep: String = text
            .lines()
            .skip(text.lines().count() / 2)
            .flat_map(|l| [l, "\n"])
            .collect();
        if let Err(e) = std::fs::write(path, keep) {
            eprintln!("faint_light: cannot trim {}: {e}", path.display());
        }
    }
}

/// `tracing_subscriber`'s writer end: splits what the formatter emits into
/// lines and appends them.
pub struct LogWriter {
    buf: LogBuffer,
    partial: Vec<u8>,
}

impl io::Write for LogWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.partial.extend_from_slice(data);
        while let Some(nl) = self.partial.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.partial.drain(..=nl).collect();
            let text = String::from_utf8_lossy(&line);
            self.buf.push(text.trim_end().to_string());
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuffer {
    type Writer = LogWriter;

    fn make_writer(&'a self) -> LogWriter {
        LogWriter {
            buf: self.clone(),
            partial: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "fl-logs-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn splits_writes_into_lines() {
        let buf = LogBuffer::new();
        let mut w = LogWriter {
            buf: buf.clone(),
            partial: Vec::new(),
        };
        // A partial line stays buffered until its newline arrives.
        w.write_all(b"INFO first\nINFO sec").unwrap();
        assert_eq!(buf.snapshot().1, "INFO first\n");
        w.write_all(b"ond\n").unwrap();
        assert_eq!(buf.snapshot().1, "INFO first\nINFO second\n");
    }

    #[test]
    fn appends_incrementally_until_the_reader_falls_behind() {
        let buf = LogBuffer::new();
        buf.push("one".into());
        let (v, text) = buf.appended_since(0).unwrap();
        assert_eq!((v, text.as_str()), (1, "one\n"));
        buf.push("two".into());
        assert_eq!(buf.appended_since(v).unwrap().1, "two\n");
        // Nothing new is an empty splice, not a rebuild.
        assert_eq!(buf.appended_since(2).unwrap().1, "");
        // Past capacity the history is gone and the caller must resync.
        for i in 0..CAPACITY + 5 {
            buf.push(format!("line {i}"));
        }
        assert!(buf.appended_since(2).is_none());
    }

    #[test]
    fn drops_the_oldest_lines_past_capacity() {
        let buf = LogBuffer::new();
        for i in 0..CAPACITY + 10 {
            buf.push(format!("line {i}"));
        }
        let (version, text) = buf.snapshot();
        assert_eq!(version as usize, CAPACITY + 10);
        assert!(text.starts_with("line 10\n"));
        assert_eq!(text.lines().count(), CAPACITY);
    }

    #[test]
    fn a_new_session_reads_back_what_the_last_one_wrote() {
        let dir = temp_dir("persist");
        let path = dir.join("faint-light.log");

        let first = LogBuffer::new();
        first.persist_to(path.clone());
        first.push("from the first run".into());
        drop(first);

        let second = LogBuffer::new();
        second.persist_to(path.clone());
        assert_eq!(second.snapshot().1, "from the first run\n");
        second.push("from the second run".into());
        assert_eq!(
            second.snapshot().1,
            "from the first run\nfrom the second run\n"
        );

        // Clearing empties the tab and the file together.
        second.clear();
        assert_eq!(second.snapshot().1, "");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
        let third = LogBuffer::new();
        third.persist_to(path);
        assert_eq!(third.snapshot().1, "");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_file_is_trimmed_rather_than_grown_without_bound() {
        let dir = temp_dir("trim");
        let path = dir.join("faint-light.log");
        // Start over the limit; the first write triggers the size check.
        let long = "x".repeat(1024);
        let mut seed = String::new();
        while seed.len() as u64 <= FILE_LIMIT {
            seed.push_str(&long);
            seed.push('\n');
        }
        std::fs::write(&path, &seed).unwrap();

        let buf = LogBuffer::new();
        buf.persist_to(path.clone());
        buf.push("after the trim".into());
        let on_disk = std::fs::metadata(&path).unwrap().len();
        assert!(on_disk < FILE_LIMIT, "file should have been trimmed");
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .ends_with("after the trim\n"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
