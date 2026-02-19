use anyhow::Result;
use serde::Deserialize;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
pub struct TaskState {
    #[serde(deserialize_with = "deserialize_task_id")]
    pub task_id: String,
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
    /// Which project this task belongs to (populated at read time)
    #[serde(default)]
    pub project: String,
}

/// Deserialize task_id from either a number or a string
fn deserialize_task_id<'de, D>(deserializer: D) -> std::result::Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de;

    struct TaskIdVisitor;

    impl<'de> de::Visitor<'de> for TaskIdVisitor {
        type Value = String;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a string or number")
        }

        fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Self::Value, E> {
            Ok(v.to_string())
        }

        fn visit_string<E: de::Error>(self, v: String) -> std::result::Result<Self::Value, E> {
            Ok(v)
        }

        fn visit_u64<E: de::Error>(self, v: u64) -> std::result::Result<Self::Value, E> {
            Ok(v.to_string())
        }

        fn visit_i64<E: de::Error>(self, v: i64) -> std::result::Result<Self::Value, E> {
            Ok(v.to_string())
        }
    }

    deserializer.deserialize_any(TaskIdVisitor)
}

pub struct DaemonState {
    pub pid: Option<u32>,
    pub alive: bool,
}

pub struct FullState {
    pub daemon: DaemonState,
    pub tasks: Vec<TaskState>,
}

/// Read state from the new ~/.sipag/ layout (multi-project)
pub fn read_state(sipag_home: &Path, project_filter: &Option<String>) -> Result<FullState> {
    let daemon = read_daemon_state(sipag_home);
    let mut tasks = Vec::new();

    let projects_dir = sipag_home.join("projects");
    if projects_dir.exists() {
        let entries = fs::read_dir(&projects_dir)?;
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let slug = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();

            // Apply project filter
            if let Some(ref filter) = project_filter {
                if &slug != filter {
                    continue;
                }
            }

            let mut project_tasks = read_task_states(&path);
            // Tag tasks with project slug
            for t in &mut project_tasks {
                t.project = slug.clone();
            }

            // Crash detection
            for task in &mut project_tasks {
                if matches!(task.status.as_str(), "claimed" | "running" | "pushing") {
                    let pid_file = path.join("workers").join(format!("{}.pid", task.task_id));
                    if !is_process_alive_from_pid_file(&pid_file) {
                        task.status = "failed".to_string();
                        if task.error.is_none() {
                            task.error = Some("worker died".to_string());
                        }
                    }
                }
            }

            tasks.extend(project_tasks);
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

/// Read state from the legacy .sipag.d/ layout (single project)
pub fn read_state_legacy(project_dir: &Path) -> Result<FullState> {
    let run_dir = project_dir.join(".sipag.d");

    let daemon = read_daemon_state(&run_dir);
    let mut tasks = read_task_states_from_run_dir(&run_dir);

    // Crash detection
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

    tasks.sort_by(|a, b| {
        let a_active = is_active(&a.status);
        let b_active = is_active(&b.status);
        match (a_active, b_active) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => b.task_id.cmp(&a.task_id),
        }
    });

    Ok(FullState { daemon, tasks })
}

fn is_active(status: &str) -> bool {
    matches!(status, "claimed" | "running" | "pushing")
}

fn read_daemon_state(dir: &Path) -> DaemonState {
    let pid_file = dir.join("sipag.pid");
    match fs::read_to_string(&pid_file) {
        Ok(content) => {
            let pid: Option<u32> = content.trim().parse().ok();
            let alive = pid.map_or(false, is_pid_alive);
            DaemonState { pid, alive }
        }
        Err(_) => DaemonState {
            pid: None,
            alive: false,
        },
    }
}

/// Read task states from new layout: project_dir/workers/*.json
fn read_task_states(project_dir: &Path) -> Vec<TaskState> {
    let workers_dir = project_dir.join("workers");
    read_json_files(&workers_dir)
}

/// Read task states from legacy layout: run_dir/workers/*.json
fn read_task_states_from_run_dir(run_dir: &Path) -> Vec<TaskState> {
    let workers_dir = run_dir.join("workers");
    read_json_files(&workers_dir)
}

fn read_json_files(workers_dir: &Path) -> Vec<TaskState> {
    let mut tasks = Vec::new();

    let entries = match fs::read_dir(workers_dir) {
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

pub fn read_log_file(sipag_home: &Path, project: &str, task_id: &str) -> String {
    let log_path = sipag_home
        .join("projects")
        .join(project)
        .join("logs")
        .join(format!("worker-{}.log", task_id));
    fs::read_to_string(&log_path).unwrap_or_default()
}

pub fn read_log_file_legacy(project_dir: &Path, task_id: &str) -> String {
    let log_path = project_dir
        .join(".sipag.d")
        .join("logs")
        .join(format!("worker-{}.log", task_id));
    fs::read_to_string(&log_path).unwrap_or_default()
}
