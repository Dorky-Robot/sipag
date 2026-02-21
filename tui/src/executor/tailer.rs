/// Log-file tailing — reusable, not coupled to executor specifics.
use std::{
    fs,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::Instant,
};

/// Tracks the log file currently being tailed.
pub struct CurrentLog {
    pub task_title: String,
    pub log_path: PathBuf,
    pub file: fs::File,
    /// Byte offset of the last read position.
    pub pos: u64,
    pub started: Instant,
}

impl CurrentLog {
    /// Seek to `pos`, read any new bytes, update `pos`, and return the new content.
    pub fn read_new(&mut self) -> String {
        let _ = self.file.seek(SeekFrom::Start(self.pos));
        let mut buf = String::new();
        if self.file.read_to_string(&mut buf).is_ok() {
            self.pos += buf.len() as u64;
        }
        buf
    }
}

/// Open `log_path`, seek to `pos`, and read the rest.
/// Used when a task completes and its log moves to done/ or failed/.
pub fn drain_log(log_path: &Path, pos: u64) -> String {
    let Ok(mut f) = fs::File::open(log_path) else {
        return String::new();
    };
    let _ = f.seek(SeekFrom::Start(pos));
    let mut buf = String::new();
    if f.read_to_string(&mut buf).is_ok() {
        buf
    } else {
        String::new()
    }
}
