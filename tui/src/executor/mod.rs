mod classifier;
mod tailer;

pub use classifier::{LogKind, LogLine};
pub use tailer::CurrentLog;

use std::{fs, path::Path, process::Child, time::Instant};

/// Manages the `sipag start` child process and its live log stream.
pub struct ExecutorState {
    /// The sipag-start child process.
    pub process: Option<Child>,
    /// Log file being tailed right now (None between tasks).
    pub current: Option<CurrentLog>,
    /// All accumulated log lines (across all tasks in this run).
    pub log_lines: Vec<LogLine>,
    /// Raw scroll offset (line index of first visible line).
    pub scroll: usize,
    /// If true, always show the bottom of the log.
    pub auto_scroll: bool,
    /// Height of the log pane viewport (updated by ui::render).
    pub viewport_height: u16,
    /// True once the process has exited.
    pub finished: bool,
}

impl ExecutorState {
    pub fn new(process: Option<Child>) -> Self {
        Self {
            process,
            current: None,
            log_lines: Vec::new(),
            scroll: 0,
            auto_scroll: true,
            viewport_height: 20,
            finished: false,
        }
    }

    /// Drive one poll cycle: process check → log completion → new log → tail → scroll.
    pub fn poll(&mut self, sipag_dir: &Path) {
        self.check_process();
        self.check_log_completed(sipag_dir);
        self.find_new_log(sipag_dir);
        self.tail_current();
        self.update_scroll();
    }

    // ── Sub-operations ────────────────────────────────────────────────────────

    /// Check if the child process has exited and mark `finished` accordingly.
    fn check_process(&mut self) {
        if let Some(ref mut child) = self.process {
            match child.try_wait() {
                Ok(Some(_)) | Err(_) => {
                    self.process = None;
                    self.finished = true;
                }
                Ok(None) => {} // still running
            }
        } else {
            self.finished = true;
        }
    }

    /// If the current log path no longer exists, the task completed.
    /// Drain remaining bytes, append a summary line, and clear `current`.
    fn check_log_completed(&mut self, sipag_dir: &Path) {
        // Collect all data we need from `current` inside a scoped borrow.
        let task_done = if let Some(ref cur) = self.current {
            if cur.log_path.exists() {
                None // still running
            } else {
                let stem = cur.log_path.file_stem().unwrap_or_default().to_os_string();
                Some((
                    stem,
                    cur.pos,
                    cur.task_title.clone(),
                    cur.started.elapsed(),
                ))
            }
        } else {
            return;
        };

        let Some((stem, pos, title, duration)) = task_done else {
            return;
        };

        // Determine outcome from done/ or failed/ directories.
        let done_log = sipag_dir.join("done").join(&stem).with_extension("log");
        let failed_log = sipag_dir.join("failed").join(&stem).with_extension("log");

        let (success, source_log) = if done_log.exists() {
            (true, Some(done_log))
        } else if failed_log.exists() {
            (false, Some(failed_log))
        } else {
            (false, None)
        };

        // Drain any bytes written after our last read.
        if let Some(ref log_path) = source_log {
            let buf = tailer::drain_log(log_path, pos);
            for line in buf.lines() {
                self.log_lines.push(LogLine::classify(line));
            }
        }

        // Append a human-readable summary line.
        let secs = duration.as_secs();
        let (mins, secs) = (secs / 60, secs % 60);
        let summary = if success {
            format!("✓ Done: {title} ({mins}m {secs}s)")
        } else {
            format!("✗ Failed: {title} ({mins}m {secs}s)")
        };
        self.log_lines.push(LogLine {
            text: summary,
            kind: LogKind::Summary(success),
        });
        self.current = None;

        if self.auto_scroll {
            self.scroll = self.log_lines.len().saturating_sub(1);
        }
    }

    /// Look for a new `.log` file in `running/` and start tailing it.
    fn find_new_log(&mut self, sipag_dir: &Path) {
        if self.current.is_some() || self.finished {
            return;
        }
        let running_dir = sipag_dir.join("running");
        let Ok(entries) = fs::read_dir(&running_dir) else {
            return;
        };
        let log_path = entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|x| x == "log"));

        let Some(path) = log_path else {
            return;
        };

        let md_path = path.with_extension("md");
        let (task_title, _) = crate::app::parse_task_brief(&md_path);

        // Header line so the user can see where one task ends and another begins.
        self.log_lines.push(LogLine {
            text: format!("── {task_title} ──"),
            kind: LogKind::Normal,
        });

        if let Ok(f) = fs::File::open(&path) {
            self.current = Some(CurrentLog {
                task_title,
                log_path: path,
                file: f,
                pos: 0,
                started: Instant::now(),
            });
        }
    }

    /// Read any new bytes from the currently tailed log file.
    fn tail_current(&mut self) {
        let Some(ref mut cur) = self.current else {
            return;
        };
        let buf = cur.read_new();
        if !buf.is_empty() {
            for line in buf.lines() {
                self.log_lines.push(LogLine::classify(line));
            }
        }
    }

    /// If auto-scroll is enabled, keep the scroll pinned to the last visible line.
    fn update_scroll(&mut self) {
        if self.auto_scroll && !self.log_lines.is_empty() {
            let visible = self.viewport_height as usize;
            self.scroll = self.log_lines.len().saturating_sub(visible);
        }
    }
}
