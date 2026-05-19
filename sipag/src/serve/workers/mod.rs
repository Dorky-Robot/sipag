//! Autonomous worker fleet.
//!
//! Workers react to labels on KRs and tasks. The scheduler polls every
//! few seconds, finds items whose labels match a registered worker,
//! and dispatches them with bounded concurrency. Each worker:
//!
//! 1. Publishes `worker.start` to `workers/activity`.
//! 2. Does its work, posting `worker.progress` events to the item's
//!    `discourse` topic.
//! 3. On success, removes its trigger label. On failure, adds the
//!    `error` label and emits a `worker.error` event. Either way, it
//!    publishes `worker.complete` to `workers/activity`.
//!
//! Choreography over orchestration: workers don't know about each
//! other. The label set on an item is the only signal.

pub mod expand;
pub mod research;
pub mod scheduler;

use crate::serve::state::AppState;
use anyhow::Result;
use sipag_pubsub::Broker;
use std::sync::Arc;

/// Identifies an in-flight item so the scheduler doesn't dispatch the
/// same KR/task twice while a worker is running.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct WorkerKey {
    pub worker: String,
    pub kind: ItemKind,
    pub project: String,
    pub id: u64,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub enum ItemKind {
    KeyResult,
    Task,
}

impl ItemKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ItemKind::KeyResult => "key-results",
            ItemKind::Task => "tasks",
        }
    }
}

/// Concrete item passed to a worker. The worker decides what context
/// to gather; the scheduler provides identity.
#[derive(Debug, Clone)]
pub struct WorkerItem {
    pub kind: ItemKind,
    pub project: String,
    pub id: u64,
    pub title: String,
    /// Snapshot of labels at scheduling time. Carried through so workers
    /// can introspect the trigger context without re-loading.
    #[allow(dead_code)]
    pub labels: Vec<String>,
}

impl WorkerItem {
    /// Topic the worker publishes its progress / completion to.
    pub fn discourse_topic(&self) -> String {
        format!(
            "{}/{}/{}/discourse",
            self.kind.as_str(),
            self.project,
            self.id
        )
    }
}

/// What a worker needs at run time. Not Clone — workers receive a
/// reference. Built once per scheduler tick.
pub struct WorkerCtx {
    pub broker: Broker,
    pub http: reqwest::Client,
    pub sipag_dir: std::path::PathBuf,
}

/// A worker is a label-triggered async function. Implementations live
/// in their own modules under `workers/`. The trait is object-safe so
/// workers can be stored in a `Vec<Arc<dyn Worker>>`.
#[async_trait::async_trait]
pub trait Worker: Send + Sync {
    /// Trigger label. Items with this label are eligible for dispatch.
    fn label(&self) -> &'static str;

    /// Worker name for telemetry — usually the same as `label()`.
    fn name(&self) -> &'static str {
        self.label()
    }

    /// Execute one unit of work. Errors are surfaced as `worker.error`
    /// events; the scheduler doesn't restart on failure.
    async fn run(&self, ctx: &WorkerCtx, item: WorkerItem) -> Result<()>;
}

/// Registry of all workers. The scheduler iterates this list each
/// tick; the WS endpoint and HTMX label routes don't see it directly.
pub fn registry() -> Vec<Arc<dyn Worker>> {
    vec![
        Arc::new(research::ResearchWorker),
        Arc::new(expand::ExpandWorker),
    ]
}

/// Spawn the scheduler task. No-op when workers aren't enabled (the
/// caller checks the flag — this is just a thin wrapper for
/// readability in `serve/mod.rs`).
pub fn spawn_scheduler(state: AppState) {
    tokio::spawn(scheduler::run(state));
}

/// Publish a `workers/activity` lifecycle event.
pub fn publish_activity(broker: &Broker, kind: &str, worker: &str, item: &WorkerItem) {
    let payload = serde_json::json!({
        "worker": worker,
        "kind": item.kind.as_str(),
        "project": item.project,
        "id": item.id,
        "title": item.title,
    });
    if let Err(e) = broker.publish("workers/activity", kind, payload) {
        tracing::warn!("failed to publish activity event: {e}");
    }
}

/// Publish a `worker.progress` event to the item's discourse topic.
pub fn publish_progress(broker: &Broker, item: &WorkerItem, worker: &str, message: &str) {
    let payload = serde_json::json!({
        "worker": worker,
        "message": message,
    });
    if let Err(e) = broker.publish(&item.discourse_topic(), "worker.progress", payload) {
        tracing::warn!("failed to publish progress event: {e}");
    }
}
