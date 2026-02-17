use anyhow::Result;
use serde::Deserialize;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
pub struct TaskState {
    pub task_id: u64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub branch: String,
    pub status: String,
    #[serde(default)]
    pub started_at: Option<String>,
    #[serde(default)]
    pub finished_at: Option<String>,
    #[serde(default)]
    pub pr_url: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

pub struct DaemonState {
    pub pid: Option<u32>,
    pub alive: bool,
}

pub struct FullState {
    pub daemon: DaemonState,
    pub tasks: Vec<TaskState>,
}

pub fn read_state(project_dir: &Path) -> Result<FullState> {
    let run_dir = project_dir.join(".sipag.d");

    let daemon = read_daemon_state(&run_dir);
    let mut tasks = read_task_states(&run_dir);

    // Crash detection: if a task says "running"/"claimed"/"pushing" but its PID file
    // is gone or the process is dead, mark it as failed
    for task in &mut tasks {
        if matches!(task.status.as_str(), "claimed" | "running" | "pushing") {
            let pid_file = run_dir.join("workers").join(format!("{}.pid", task.task_id));
            if !is_process_alive_from_pid_file(&pid_file) {
                task.status = "failed".to_string();
                if task.error.is_none() {
                    task.error = Some("worker died".to_string());
                }
            }
        }
    }

    // Sort: active tasks first (by started_at), then finished
    tasks.sort_by(|a, b| {
        let a_active = is_active(&a.status);
        let b_active = is_active(&b.status);
        match (a_active, b_active) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => {
                // Within same group, sort by task_id descending (newest first)
                b.task_id.cmp(&a.task_id)
            }
        }
    });

    Ok(FullState { daemon, tasks })
}

fn is_active(status: &str) -> bool {
    matches!(status, "claimed" | "running" | "pushing")
}

fn read_daemon_state(run_dir: &Path) -> DaemonState {
    let pid_file = run_dir.join("sipag.pid");
    match fs::read_to_string(&pid_file) {
        Ok(content) => {
            let pid: Option<u32> = content.trim().parse().ok();
            let alive = pid.map_or(false, |p| is_pid_alive(p));
            DaemonState { pid, alive }
        }
        Err(_) => DaemonState {
            pid: None,
            alive: false,
        },
    }
}

fn read_task_states(run_dir: &Path) -> Vec<TaskState> {
    let workers_dir = run_dir.join("workers");
    let mut tasks = Vec::new();

    let entries = match fs::read_dir(&workers_dir) {
        Ok(entries) => entries,
        Err(_) => return tasks,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("json") {
            if let Ok(content) = fs::read_to_string(&path) {
                match serde_json::from_str::<TaskState>(&content) {
                    Ok(task) => tasks.push(task),
                    Err(_) => continue,
                }
            }
        }
    }

    tasks
}

fn is_process_alive_from_pid_file(pid_file: &Path) -> bool {
    match fs::read_to_string(pid_file) {
        Ok(content) => {
            let pid: u32 = match content.trim().parse() {
                Ok(p) => p,
                Err(_) => return false,
            };
            is_pid_alive(pid)
        }
        Err(_) => false,
    }
}

fn is_pid_alive(pid: u32) -> bool {
    // kill -0 equivalent: check if process exists
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

pub fn read_log_file(project_dir: &Path, task_id: u64) -> String {
    let log_path = project_dir
        .join(".sipag.d")
        .join("logs")
        .join(format!("worker-{}.log", task_id));
    fs::read_to_string(&log_path).unwrap_or_default()
}
