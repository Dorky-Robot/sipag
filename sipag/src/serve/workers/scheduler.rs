//! Worker scheduler — polls the board, dispatches eligible items.
//!
//! Wakes every 5s. For every project, scans KRs and tasks for label
//! matches against registered workers. Bounded concurrency
//! (`MAX_INFLIGHT`) keeps ollama from being hammered if many items
//! are tagged at once.

use crate::serve::state::AppState;
use crate::serve::workers::{
    publish_activity, registry, ItemKind, WorkerCtx, WorkerItem, WorkerKey,
};
use sipag_core::board::{list_project_names, list_tasks, KeyResult};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, Semaphore};

const TICK: Duration = Duration::from_secs(5);
const MAX_INFLIGHT: usize = 2;

pub async fn run(state: AppState) {
    let workers = registry();
    if workers.is_empty() {
        tracing::warn!("no workers registered; scheduler exiting");
        return;
    }

    let permits = Arc::new(Semaphore::new(MAX_INFLIGHT));
    let inflight: Arc<Mutex<HashSet<WorkerKey>>> = Arc::new(Mutex::new(HashSet::new()));

    let mut ticker = tokio::time::interval(TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        ticker.tick().await;
        if let Err(e) = scan_and_dispatch(&state, &workers, &permits, &inflight).await {
            tracing::warn!("scheduler tick error: {e}");
        }
    }
}

async fn scan_and_dispatch(
    state: &AppState,
    workers: &[Arc<dyn super::Worker>],
    permits: &Arc<Semaphore>,
    inflight: &Arc<Mutex<HashSet<WorkerKey>>>,
) -> anyhow::Result<()> {
    let project_names = list_project_names(&state.sipag_dir).unwrap_or_default();
    for project in project_names {
        // KRs.
        let krs = KeyResult::list(&state.sipag_dir, &project).unwrap_or_default();
        for kr in krs {
            for w in workers {
                if !kr.labels.iter().any(|l| l == w.label()) {
                    continue;
                }
                let key = WorkerKey {
                    worker: w.label().to_string(),
                    kind: ItemKind::KeyResult,
                    project: project.clone(),
                    id: kr.id,
                };
                if !inflight.lock().await.insert(key.clone()) {
                    continue; // already running
                }
                let item = WorkerItem {
                    kind: ItemKind::KeyResult,
                    project: project.clone(),
                    id: kr.id,
                    title: kr.title.clone(),
                    labels: kr.labels.clone(),
                };
                spawn_worker(
                    state.clone(),
                    w.clone(),
                    item,
                    key,
                    permits.clone(),
                    inflight.clone(),
                );
            }
        }

        // Tasks.
        let tasks = list_tasks(&state.sipag_dir, &project, None).unwrap_or_default();
        for task in tasks {
            for w in workers {
                if !task.labels.iter().any(|l| l == w.label()) {
                    continue;
                }
                let key = WorkerKey {
                    worker: w.label().to_string(),
                    kind: ItemKind::Task,
                    project: project.clone(),
                    id: task.id,
                };
                if !inflight.lock().await.insert(key.clone()) {
                    continue;
                }
                let item = WorkerItem {
                    kind: ItemKind::Task,
                    project: project.clone(),
                    id: task.id,
                    title: task.title.clone(),
                    labels: task.labels.clone(),
                };
                spawn_worker(
                    state.clone(),
                    w.clone(),
                    item,
                    key,
                    permits.clone(),
                    inflight.clone(),
                );
            }
        }
    }
    Ok(())
}

fn spawn_worker(
    state: AppState,
    worker: Arc<dyn super::Worker>,
    item: WorkerItem,
    key: WorkerKey,
    permits: Arc<Semaphore>,
    inflight: Arc<Mutex<HashSet<WorkerKey>>>,
) {
    tokio::spawn(async move {
        let permit = permits.acquire_owned().await;
        if permit.is_err() {
            inflight.lock().await.remove(&key);
            return;
        }
        let ctx = WorkerCtx {
            broker: state.broker.clone(),
            http: state.http.clone(),
            sipag_dir: state.sipag_dir.clone(),
        };
        publish_activity(&state.broker, "worker.start", worker.name(), &item);
        let res = worker.run(&ctx, item.clone()).await;
        if let Err(e) = res {
            tracing::warn!(
                "worker {} on {}/{} #{} failed: {e}",
                worker.name(),
                item.kind.as_str(),
                item.project,
                item.id
            );
            // Add `error` label so the user notices.
            let _ = mark_label_change(&state, &item, &[], &["error".to_string()]);
            let payload = serde_json::json!({
                "worker": worker.name(),
                "kind": item.kind.as_str(),
                "project": item.project,
                "id": item.id,
                "error": e.to_string(),
            });
            let _ = state
                .broker
                .publish("workers/activity", "worker.error", payload);
        }
        publish_activity(&state.broker, "worker.complete", worker.name(), &item);
        inflight.lock().await.remove(&key);
        drop(permit);
    });
}

/// Mutate the labels on the on-disk item. `add_labels` and
/// `remove_labels` are appended/stripped as label-set operations
/// (deduped).
pub fn mark_label_change(
    state: &AppState,
    item: &WorkerItem,
    add_labels: &[String],
    remove_labels: &[String],
) -> anyhow::Result<()> {
    use sipag_core::board::{KeyResult, Task};
    match item.kind {
        ItemKind::KeyResult => {
            let mut kr = KeyResult::load(&state.sipag_dir, &item.project, item.id)?;
            apply_label_change(&mut kr.labels, add_labels, remove_labels);
            kr.save(&state.sipag_dir, &item.project)?;
        }
        ItemKind::Task => {
            let mut t = Task::load(&state.sipag_dir, &item.project, item.id)?;
            apply_label_change(&mut t.labels, add_labels, remove_labels);
            t.updated = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
            t.save(&state.sipag_dir, &item.project)?;
        }
    }
    Ok(())
}

fn apply_label_change(labels: &mut Vec<String>, add_labels: &[String], remove_labels: &[String]) {
    labels.retain(|l| !remove_labels.contains(l));
    for add in add_labels {
        if !labels.iter().any(|l| l == add) {
            labels.push(add.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::apply_label_change;

    #[test]
    fn label_change_adds_and_removes() {
        let mut labels = vec!["research".to_string(), "priority".to_string()];
        apply_label_change(&mut labels, &["attention".into()], &["research".into()]);
        assert_eq!(
            labels,
            vec!["priority".to_string(), "attention".to_string()]
        );
    }

    #[test]
    fn label_change_dedupes_adds() {
        let mut labels = vec!["a".to_string()];
        apply_label_change(&mut labels, &["a".into(), "b".into()], &[]);
        assert_eq!(labels, vec!["a".to_string(), "b".to_string()]);
    }
}
