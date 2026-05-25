//! Lens-worker scheduler (Phase 1 #3 in `docs/modules.md`).
//!
//! Walks a registered set of [`Lens`]es and fires each one's
//! [`LensWorker::run_with_tools`] when its [`TriggerPolicy`] says
//! the lens is due. The dispatcher itself is sipag-binary-internal —
//! lens-workers, the corpus, and the bridge client all live in
//! their own crates; this module orchestrates them.
//!
//! ## Trigger coverage (v1)
//!
//! - [`TriggerPolicy::Schedule`] — supported. Fires every `interval`
//!   `Duration`. A lens that has never fired runs on the first tick
//!   it's evaluated against.
//! - [`TriggerPolicy::Threshold`] — **skipped with a warn**. Domain-
//!   specific signal sources (katulong event windows, project commit
//!   counts, etc.) are the bridge-worker's territory; the scheduler
//!   can't evaluate them without the corresponding subscriber.
//! - [`TriggerPolicy::ModelDecide`] — **skipped with a warn**. Open
//!   question in `docs/modules.md` §10 — formalize once telemetry is
//!   in.
//!
//! ## Concurrency (v1)
//!
//! Fires are **serial** within a tick. If a lens-worker hangs gemma,
//! subsequent lenses in the same tick wait. Replace with spawned
//! per-lens fires (with proper in-flight tracking) when telemetry
//! shows lenses overlapping at scale.
//!
//! **Corpus mutex contention.** [`spawn`] takes the `Arc<Mutex<Corpus>>`
//! lock for the duration of an entire `run_one_tick` call (including
//! gemma round-trips inside `LensWorker::run_with_tools`). Today the
//! scheduler is the only consumer of the corpus, so contention is
//! invisible. Future consumers (web UI search endpoints, bridge
//! worker, …) added to the corpus mutex will block during tick
//! windows — promote to RwLock or a write-through channel if that
//! latency becomes operator-visible.
//!
//! **Cadence semantic.** `last_fired` is set after the fire RETURNS,
//! not when it starts. A 60s-interval lens that takes 90s to fire
//! won't queue up an instant re-fire; the next due window starts
//! at fire-end, so actual cadence drifts later under load. Fast
//! lenses see a clean `interval`-spaced cadence.
//!
//! **Known v1 limitations (not addressed in the scheduler PR):**
//! - **Panic safety:** a `LensWorker::run_with_tools` panic (vs Err)
//!   aborts the spawned scheduler task. Lens-workers shouldn't panic
//!   in practice (all error paths are `Result`-typed), but a future
//!   PR should wrap each fire in a `tokio::task::spawn` + `JoinHandle::await`
//!   supervisor so one bad lens can't kill the whole loop.
//! - **No cancellation:** the spawned task runs until process exit.
//!   `shutdown_signal()` in `serve/mod.rs` triggers axum's graceful
//!   shutdown but doesn't reach the scheduler. Tokio runtime drop
//!   at process exit aborts cleanly enough for v1.
//!
//! ## State (v1)
//!
//! `last_fired` lives in memory. A `sipag serve` restart resets the
//! schedule clock — every lens re-fires on the first tick post-restart.
//! Acceptable for the small periodic Schedule case; persist when the
//! cadence + lens count make repeat-on-restart visible to operators.

use sipag_corpus::{Corpus, Embedder};
use sipag_lens::{
    ChatBackend, Lens, LensWorker, ModelResolver, StructuralAction, TriggerPolicy,
    DEFAULT_TOOL_ITERATIONS,
};
use std::path::Path;
use std::time::Duration;
// Use `tokio::time::Instant` (not `std::time::Instant`) for the
// scheduler clock so tests using `tokio::time::pause()` /
// `advance()` can drive the scheduler against virtual time.
// `tokio::time::Instant` falls through to the OS clock in
// production; the pause-aware behavior is test-only.
use tokio::time::Instant;
use tracing::{debug, info, warn};

/// What we hand the model on each scheduled tick. Doubles as the
/// "user turn" of the chat for scheduled lens-workers; the lens's
/// `prompt_text` is the system turn that says what to look for.
const SCHEDULER_TICK_INPUT: &str = "Run your scheduled reasoning cycle. \
Use corpus.search to find recent observations relevant to your lens \
before emitting your final output.";

/// How often [`spawn`]'s tick loop polls the registry. Bounded by
/// the smallest interesting `Schedule` interval; ~30s gives reasonable
/// jitter without burning CPU on idle ticks.
pub const TICK_INTERVAL: Duration = Duration::from_secs(30);

/// One row in the scheduler's registry.
struct RegisteredLens {
    lens: Lens,
    last_fired: Option<Instant>,
}

/// The scheduler proper. Holds the lens registry + per-lens last-fired
/// state; dependencies (chat backend, embedder, corpus) are injected
/// into each [`run_one_tick`](Self::run_one_tick) call so tests can
/// drive the scheduler against fakes without rewiring the type.
pub struct LensScheduler {
    lenses: Vec<RegisteredLens>,
    resolver: ModelResolver,
}

/// Per-tick summary returned by [`LensScheduler::run_one_tick`].
/// Tests assert on this; production wiring logs via `info!`.
#[derive(Debug, Default, Clone)]
pub struct SchedulerTick {
    /// Lens names that fired this tick (regardless of outcome).
    pub fired: Vec<String>,
    /// `(lens_name, error_message)` for each failure.
    pub errors: Vec<(String, String)>,
    /// Lens names whose `TriggerPolicy` variant the scheduler can't
    /// evaluate today (Threshold / ModelDecide). Logged once per tick
    /// so a misconfigured lens stays visible.
    pub skipped_unsupported: Vec<String>,
}

impl LensScheduler {
    /// Build a scheduler over the supplied (already-loaded) lenses.
    /// Retired lenses are filtered out at construction so the runtime
    /// loop doesn't need to re-check on every tick. Logs a one-time
    /// `warn!` per lens whose trigger variant the scheduler can't
    /// evaluate today — so an operator who drops a Threshold lens
    /// into `~/.sipag/lenses/` learns at boot that it won't fire
    /// (rather than waiting for the much-later tick telemetry).
    pub fn new(lenses: Vec<Lens>, resolver: ModelResolver) -> Self {
        let lenses: Vec<RegisteredLens> = lenses
            .into_iter()
            .filter(|l| !l.retired)
            .map(|lens| RegisteredLens {
                lens,
                last_fired: None,
            })
            .collect();
        for rl in &lenses {
            match &rl.lens.trigger {
                TriggerPolicy::Schedule { .. } => {}
                TriggerPolicy::Threshold => {
                    warn!(
                        lens = %rl.lens.name,
                        "scheduler: lens has TriggerPolicy::Threshold which the scheduler can't \
                         evaluate today — will surface in skipped_unsupported telemetry until \
                         §9 Phase 2 #7 (SSE subscriber) lands a bridge-worker that can drive it"
                    );
                }
                TriggerPolicy::ModelDecide => {
                    warn!(
                        lens = %rl.lens.name,
                        "scheduler: lens has TriggerPolicy::ModelDecide which the scheduler can't \
                         evaluate today — will surface in skipped_unsupported telemetry until \
                         the §10 model-decide formalization lands"
                    );
                }
            }
        }
        Self { lenses, resolver }
    }

    /// Total registered (non-retired) lenses.
    pub fn len(&self) -> usize {
        self.lenses.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lenses.is_empty()
    }

    /// Evaluate every registered lens against the current clock and
    /// fire those whose triggers say they're due. Returns a
    /// [`SchedulerTick`] summary.
    ///
    /// Serial within a tick — see module doc.
    ///
    /// `last_fired` advances **on every fire attempt regardless of
    /// outcome**, and is anchored to the moment the fire RETURNS (not
    /// when it started). A broken lens that consistently errors will
    /// respect its `interval` instead of busy-looping; a slow lens
    /// won't queue up an instant re-fire after a long run. See the
    /// "Cadence semantic" module-doc note.
    pub async fn run_one_tick<B: ChatBackend>(
        &mut self,
        backend: &B,
        embedder: &dyn Embedder,
        corpus: &mut Corpus,
    ) -> SchedulerTick {
        let now = Instant::now();
        let mut summary = SchedulerTick::default();

        // Single pass: classify each lens, accumulate `to_fire` indices
        // and `skipped_unsupported` names in one go.
        let mut to_fire: Vec<usize> = Vec::new();
        for (i, rl) in self.lenses.iter().enumerate() {
            match Self::evaluate(rl, now) {
                FireDecision::Fire => to_fire.push(i),
                FireDecision::Wait => {}
                FireDecision::Unsupported => {
                    debug!(
                        lens = %rl.lens.name,
                        trigger = ?rl.lens.trigger,
                        "scheduler: skipping lens with unsupported trigger"
                    );
                    summary.skipped_unsupported.push(rl.lens.name.clone());
                }
            }
        }

        for idx in to_fire {
            let name = self.lenses[idx].lens.name.clone();
            let result = self.fire(idx, backend, embedder, corpus).await;
            // Always advance the clock so an errored lens respects
            // its interval rather than busy-retrying every tick.
            self.lenses[idx].last_fired = Some(Instant::now());
            match result {
                Ok(()) => summary.fired.push(name),
                Err(err) => {
                    let msg = err.to_string();
                    warn!(lens = %name, error = %msg, "scheduler: lens run failed");
                    summary.errors.push((name.clone(), msg));
                    summary.fired.push(name);
                }
            }
        }
        summary
    }

    fn evaluate(rl: &RegisteredLens, now: Instant) -> FireDecision {
        match &rl.lens.trigger {
            TriggerPolicy::Schedule { interval } => {
                let due = match rl.last_fired {
                    None => true,
                    Some(t) => now.duration_since(t) >= *interval,
                };
                if due {
                    FireDecision::Fire
                } else {
                    FireDecision::Wait
                }
            }
            TriggerPolicy::Threshold | TriggerPolicy::ModelDecide => FireDecision::Unsupported,
        }
    }

    async fn fire<B: ChatBackend>(
        &mut self,
        idx: usize,
        backend: &B,
        embedder: &dyn Embedder,
        corpus: &mut Corpus,
    ) -> Result<(), sipag_lens::LensError> {
        let lens = &self.lenses[idx].lens;
        info!(lens = %lens.name, "scheduler: firing lens");
        let worker = LensWorker::new(lens, backend, &self.resolver);
        let actions = worker
            .run_with_tools(
                SCHEDULER_TICK_INPUT,
                corpus,
                embedder,
                DEFAULT_TOOL_ITERATIONS,
            )
            .await?;
        // Structural verbs (SuggestStance / AskHuman / ProposeTask)
        // need dispatching to UI surfaces. v1 doesn't dispatch them —
        // log at `warn!` so an operator who sees a lens producing
        // ProposeTask / AskHuman knows the UI surface is missing
        // (rather than the action being silently dropped under
        // default `info!` filter levels).
        for a in &actions {
            match a {
                StructuralAction::Observe { .. } => {}
                other => {
                    warn!(
                        lens = %lens.name,
                        action = ?other,
                        "scheduler: structural verb produced but not yet dispatched (v1) — UI affordance is a follow-up PR"
                    );
                }
            }
        }
        info!(
            lens = %lens.name,
            actions = actions.len(),
            "scheduler: lens completed"
        );
        Ok(())
    }
}

enum FireDecision {
    Fire,
    Wait,
    /// Trigger variant the scheduler doesn't know how to evaluate
    /// (Threshold / ModelDecide).
    Unsupported,
}

/// Walk `dir` (`~/.sipag/lenses/` in production) and parse every
/// `*.toml` file as a [`Lens`]. Tolerates malformed files — logs a
/// `warn!` and continues, mirroring [`sipag_corpus::Corpus::open`]'s
/// per-line tolerance. A single bad lens shouldn't take down the
/// whole registry on boot.
///
/// **Symlink-skip:** entries that resolve via symlink are skipped
/// with a `warn!`. The lens dir is operator-owned and the trust
/// boundary is operator-internal, but skipping symlinks avoids
/// surprising behavior if a future feature lets non-operator
/// processes drop files into the dir.
///
/// **Name-dedup:** lenses are deduplicated by `name`. Later
/// duplicates are skipped with a `warn!` — operator-visible enough
/// that an accidental `cp kr-1.toml kr-1-copy.toml` shows up
/// instead of silently double-firing the lens every interval.
/// Filesystem `read_dir` order is unspecified, so "later" means
/// "later in iteration order"; in practice the operator should
/// rename / delete the duplicate rather than rely on which one wins.
///
/// Returns an empty Vec if `dir` doesn't exist (operator hasn't set
/// up any lenses yet — not an error).
pub async fn load_lenses_from_dir(dir: &Path) -> Vec<Lens> {
    use std::collections::HashSet;

    let mut out = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(e) => e,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            debug!(dir = %dir.display(), "scheduler: lens dir absent — no lenses loaded");
            return out;
        }
        Err(err) => {
            warn!(
                dir = %dir.display(),
                error = %err,
                "scheduler: failed to read lens dir"
            );
            return out;
        }
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        let file_type = match entry.file_type().await {
            Ok(ft) => ft,
            Err(err) => {
                warn!(
                    path = %path.display(),
                    error = %err,
                    "scheduler: failed to stat lens file; skipping"
                );
                continue;
            }
        };
        if file_type.is_symlink() {
            warn!(
                path = %path.display(),
                "scheduler: lens file is a symlink — skipping (operator-owned dir trust boundary)"
            );
            continue;
        }
        let content = match tokio::fs::read_to_string(&path).await {
            Ok(c) => c,
            Err(err) => {
                warn!(
                    path = %path.display(),
                    error = %err,
                    "scheduler: failed to read lens file; skipping"
                );
                continue;
            }
        };
        let lens: Lens = match toml::from_str(&content) {
            Ok(l) => l,
            Err(err) => {
                warn!(
                    path = %path.display(),
                    error = %err,
                    "scheduler: malformed lens TOML; skipping"
                );
                continue;
            }
        };
        if !seen.insert(lens.name.clone()) {
            warn!(
                path = %path.display(),
                name = %lens.name,
                "scheduler: lens name already seen in this dir — skipping duplicate"
            );
            continue;
        }
        out.push(lens);
    }
    out
}

/// Spawn the tick loop on the tokio runtime. Each tick locks the
/// corpus, runs one scheduler pass, and releases. Returns
/// immediately; the loop runs until process exit (no cancellation
/// today — operator restarts `sipag serve` to stop it).
pub fn spawn<B>(
    mut scheduler: LensScheduler,
    backend: B,
    embedder: sipag_corpus::BridgeEmbedder,
    corpus: std::sync::Arc<tokio::sync::Mutex<Corpus>>,
) where
    B: ChatBackend + 'static,
{
    tokio::spawn(async move {
        if scheduler.is_empty() {
            info!("scheduler: no lenses registered — tick loop will idle (operator can drop TOML files into ~/.sipag/lenses/ then restart)");
        } else {
            info!(lenses = scheduler.len(), "scheduler: tick loop starting");
        }
        let mut ticker = tokio::time::interval(TICK_INTERVAL);
        // `interval` fires immediately on first poll; that's exactly
        // the "run any never-fired lens on the first tick" semantic
        // we want.
        loop {
            ticker.tick().await;
            let mut c = corpus.lock().await;
            let summary = scheduler.run_one_tick(&backend, &embedder, &mut c).await;
            if !summary.fired.is_empty() || !summary.errors.is_empty() {
                info!(
                    fired = ?summary.fired,
                    errors = summary.errors.len(),
                    skipped_unsupported = ?summary.skipped_unsupported,
                    "scheduler: tick completed"
                );
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use sipag_corpus::CorpusResult;
    use sipag_lens::{LensResult, LensSource, ModelChoice};
    use tempfile::TempDir;

    // ── canned chat backend for tests ──────────────────────────────
    //
    // Returns a fixed reply (a valid LensOutput with one Observe) so
    // the scheduler can drive the full LensWorker::run_with_tools path
    // without spinning up a real bridge.

    struct CannedBackend {
        reply: String,
    }

    impl CannedBackend {
        fn new(reply: impl Into<String>) -> Self {
            Self {
                reply: reply.into(),
            }
        }
    }

    #[async_trait]
    impl ChatBackend for CannedBackend {
        async fn chat(&self, _model: &str, _system: &str, _user: &str) -> LensResult<String> {
            Ok(self.reply.clone())
        }
    }

    /// Hash-derived deterministic embedder so the scheduler-driven
    /// LensWorker can write Observes with non-empty embeddings
    /// without spinning up ollama.
    struct FakeEmbedder {
        dim: usize,
    }

    #[async_trait]
    impl Embedder for FakeEmbedder {
        async fn embed(&self, text: &str) -> CorpusResult<Vec<f32>> {
            let bytes = text.as_bytes();
            let mut out = Vec::with_capacity(self.dim);
            for i in 0..self.dim {
                let v = if bytes.is_empty() {
                    0.0
                } else {
                    bytes[i % bytes.len()] as f32 / 255.0
                };
                out.push(v);
            }
            Ok(out)
        }
    }

    fn test_lens(name: &str, trigger: TriggerPolicy) -> Lens {
        Lens {
            name: name.into(),
            prompt_text: "you are a test lens".into(),
            source: LensSource::ProjectMeta {
                name: "test".into(),
            },
            model: ModelChoice::Default,
            trigger,
            retired: false,
        }
    }

    fn finished_lens_output() -> String {
        serde_json::json!({
            "actions": [
                { "verb": "observe", "content": "scheduled tick produced an observation", "tags": [] }
            ]
        })
        .to_string()
    }

    async fn fresh_corpus() -> (TempDir, Corpus) {
        let dir = TempDir::new().unwrap();
        let corpus = Corpus::open(dir.path()).await.unwrap();
        (dir, corpus)
    }

    // ── feature-requirement tests ──────────────────────────────────
    //
    // Each test pins what the scheduler PROMISES to its operator:
    // - Schedule fires lenses on cadence;
    // - never-fired lenses fire on the first tick;
    // - retired lenses are never fired;
    // - Threshold / ModelDecide trigger variants are skipped with
    //   visible "unsupported" telemetry (not silently dropped);
    // - corpus writes happen through the supplied embedder so
    //   subsequent corpus.search calls can find scheduled Observes;
    // - lens loader tolerates malformed files (single bad lens
    //   doesn't take down the registry).

    #[tokio::test]
    async fn schedule_lens_fires_on_first_tick() {
        let (_dir, mut corpus) = fresh_corpus().await;
        let backend = CannedBackend::new(finished_lens_output());
        let embedder = FakeEmbedder { dim: 4 };
        let resolver = ModelResolver::with_defaults();
        let lens = test_lens(
            "scheduled-1h",
            TriggerPolicy::Schedule {
                interval: Duration::from_secs(3600),
            },
        );
        let mut scheduler = LensScheduler::new(vec![lens], resolver);

        let summary = scheduler
            .run_one_tick(&backend, &embedder, &mut corpus)
            .await;
        assert_eq!(summary.fired, vec!["scheduled-1h".to_string()]);
        assert!(summary.errors.is_empty());
        assert!(summary.skipped_unsupported.is_empty());
        // Observe landed in the corpus with a non-empty embedding.
        assert_eq!(corpus.len(), 1);
        let item = corpus.get(1).unwrap();
        assert!(
            !item.embedding.is_empty(),
            "scheduler must run lenses with the supplied embedder so Observes are searchable"
        );
    }

    #[tokio::test]
    async fn schedule_lens_does_not_refire_within_interval() {
        let (_dir, mut corpus) = fresh_corpus().await;
        let backend = CannedBackend::new(finished_lens_output());
        let embedder = FakeEmbedder { dim: 4 };
        let resolver = ModelResolver::with_defaults();
        // Long interval relative to wall-clock between calls.
        let lens = test_lens(
            "slow-lens",
            TriggerPolicy::Schedule {
                interval: Duration::from_secs(600),
            },
        );
        let mut scheduler = LensScheduler::new(vec![lens], resolver);

        let first = scheduler
            .run_one_tick(&backend, &embedder, &mut corpus)
            .await;
        assert_eq!(first.fired.len(), 1);
        let second = scheduler
            .run_one_tick(&backend, &embedder, &mut corpus)
            .await;
        assert!(
            second.fired.is_empty(),
            "lens with 600s interval must not refire on a second back-to-back tick"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn schedule_lens_refires_after_interval_elapses() {
        let (_dir, mut corpus) = fresh_corpus().await;
        let backend = CannedBackend::new(finished_lens_output());
        let embedder = FakeEmbedder { dim: 4 };
        let resolver = ModelResolver::with_defaults();
        let lens = test_lens(
            "fires-every-60s",
            TriggerPolicy::Schedule {
                interval: Duration::from_secs(60),
            },
        );
        let mut scheduler = LensScheduler::new(vec![lens], resolver);

        // Tick 1: never fired → fires.
        let first = scheduler
            .run_one_tick(&backend, &embedder, &mut corpus)
            .await;
        assert_eq!(first.fired.len(), 1);

        // Advance the virtual clock past the interval.
        tokio::time::advance(Duration::from_secs(61)).await;

        // Tick 2: 61s elapsed → fires again.
        let second = scheduler
            .run_one_tick(&backend, &embedder, &mut corpus)
            .await;
        assert_eq!(
            second.fired.len(),
            1,
            "lens should refire after the configured interval elapses"
        );
    }

    #[tokio::test]
    async fn threshold_trigger_skipped_with_visible_telemetry() {
        let (_dir, mut corpus) = fresh_corpus().await;
        let backend = CannedBackend::new(finished_lens_output());
        let embedder = FakeEmbedder { dim: 4 };
        let resolver = ModelResolver::with_defaults();
        let lens = test_lens("threshold-lens", TriggerPolicy::Threshold);
        let mut scheduler = LensScheduler::new(vec![lens], resolver);

        let summary = scheduler
            .run_one_tick(&backend, &embedder, &mut corpus)
            .await;
        assert!(
            summary.fired.is_empty(),
            "Threshold lenses are not fired by the scheduler"
        );
        assert_eq!(
            summary.skipped_unsupported,
            vec!["threshold-lens".to_string()],
            "Threshold lenses should surface in skipped_unsupported telemetry"
        );
    }

    #[tokio::test]
    async fn model_decide_trigger_skipped_with_visible_telemetry() {
        let (_dir, mut corpus) = fresh_corpus().await;
        let backend = CannedBackend::new(finished_lens_output());
        let embedder = FakeEmbedder { dim: 4 };
        let resolver = ModelResolver::with_defaults();
        let lens = test_lens("decider", TriggerPolicy::ModelDecide);
        let mut scheduler = LensScheduler::new(vec![lens], resolver);

        let summary = scheduler
            .run_one_tick(&backend, &embedder, &mut corpus)
            .await;
        assert!(summary.fired.is_empty());
        assert_eq!(summary.skipped_unsupported, vec!["decider".to_string()]);
    }

    #[tokio::test]
    async fn retired_lens_is_not_registered() {
        let (_dir, mut corpus) = fresh_corpus().await;
        let backend = CannedBackend::new(finished_lens_output());
        let embedder = FakeEmbedder { dim: 4 };
        let resolver = ModelResolver::with_defaults();
        let mut retired = test_lens(
            "old-lens",
            TriggerPolicy::Schedule {
                interval: Duration::from_secs(60),
            },
        );
        retired.retired = true;
        let active = test_lens(
            "fresh-lens",
            TriggerPolicy::Schedule {
                interval: Duration::from_secs(60),
            },
        );
        let mut scheduler = LensScheduler::new(vec![retired, active], resolver);
        assert_eq!(
            scheduler.len(),
            1,
            "retired lens should be filtered out at registration"
        );

        let summary = scheduler
            .run_one_tick(&backend, &embedder, &mut corpus)
            .await;
        assert_eq!(summary.fired, vec!["fresh-lens".to_string()]);
    }

    #[tokio::test]
    async fn lens_run_failure_recorded_but_does_not_abort_tick() {
        // A backend that returns garbage causes LensWorker::parse_output
        // (and parse_tool_call) to both fail → LensError::OutputParse
        // bubbles out. The scheduler records the error and continues
        // to the next lens.
        let (_dir, mut corpus) = fresh_corpus().await;
        let bad_backend = CannedBackend::new("I refuse to comply with structured output");
        let embedder = FakeEmbedder { dim: 4 };
        let resolver = ModelResolver::with_defaults();
        let lens_a = test_lens(
            "fails",
            TriggerPolicy::Schedule {
                interval: Duration::from_secs(60),
            },
        );
        let mut scheduler = LensScheduler::new(vec![lens_a], resolver);

        let summary = scheduler
            .run_one_tick(&bad_backend, &embedder, &mut corpus)
            .await;
        assert_eq!(summary.fired, vec!["fails".to_string()]);
        assert_eq!(
            summary.errors.len(),
            1,
            "garbage backend reply must surface as a recorded error"
        );
        assert_eq!(summary.errors[0].0, "fails");
    }

    #[tokio::test]
    async fn errored_lens_still_advances_last_fired() {
        // Even if the run errors, `last_fired` should advance so the
        // scheduler doesn't busy-loop on a broken lens within a single
        // wall-clock tick window. Failure backoff is per-interval, not
        // per-tick.
        let (_dir, mut corpus) = fresh_corpus().await;
        let bad_backend = CannedBackend::new("garbage");
        let embedder = FakeEmbedder { dim: 4 };
        let resolver = ModelResolver::with_defaults();
        let lens_a = test_lens(
            "broken",
            TriggerPolicy::Schedule {
                interval: Duration::from_secs(3600),
            },
        );
        let mut scheduler = LensScheduler::new(vec![lens_a], resolver);

        let first = scheduler
            .run_one_tick(&bad_backend, &embedder, &mut corpus)
            .await;
        assert_eq!(first.errors.len(), 1);
        let second = scheduler
            .run_one_tick(&bad_backend, &embedder, &mut corpus)
            .await;
        assert!(
            second.fired.is_empty(),
            "broken lens should NOT refire within its interval just because it errored"
        );
    }

    // ── lens loader tests ──────────────────────────────────────────

    #[tokio::test]
    async fn load_lenses_returns_empty_when_dir_absent() {
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("does-not-exist");
        let lenses = load_lenses_from_dir(&missing).await;
        assert!(lenses.is_empty());
    }

    #[tokio::test]
    async fn load_lenses_parses_valid_toml() {
        let dir = TempDir::new().unwrap();
        let lens_path = dir.path().join("kr-1.toml");
        tokio::fs::write(
            &lens_path,
            r#"
name = "kr-1"
prompt_text = "Watch for progress on this KR."
retired = false

[source]
kind = "steering_entry"
id = "kr/1"

[model]
type = "default"

[trigger]
kind = "schedule"
interval = 600
"#,
        )
        .await
        .unwrap();
        let lenses = load_lenses_from_dir(dir.path()).await;
        assert_eq!(lenses.len(), 1);
        assert_eq!(lenses[0].name, "kr-1");
        match &lenses[0].trigger {
            TriggerPolicy::Schedule { interval } => {
                assert_eq!(*interval, Duration::from_secs(600));
            }
            other => panic!("expected Schedule, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn load_lenses_skips_malformed_toml() {
        let dir = TempDir::new().unwrap();
        tokio::fs::write(dir.path().join("bad.toml"), "this is = not [valid toml")
            .await
            .unwrap();
        tokio::fs::write(
            dir.path().join("good.toml"),
            r#"
name = "good"
prompt_text = "ok"
retired = false

[source]
kind = "project_meta"
name = "pattern-spotter"

[model]
type = "default"

[trigger]
kind = "schedule"
interval = 300
"#,
        )
        .await
        .unwrap();
        let lenses = load_lenses_from_dir(dir.path()).await;
        assert_eq!(
            lenses.len(),
            1,
            "malformed lens should be skipped without taking down the whole registry"
        );
        assert_eq!(lenses[0].name, "good");
    }

    #[tokio::test]
    async fn load_lenses_ignores_non_toml_files() {
        let dir = TempDir::new().unwrap();
        tokio::fs::write(dir.path().join("README"), "ignore me")
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("notes.md"), "also ignore")
            .await
            .unwrap();
        let lenses = load_lenses_from_dir(dir.path()).await;
        assert!(lenses.is_empty());
    }

    #[tokio::test]
    async fn load_lenses_returns_retired_so_scheduler_can_filter() {
        // Loader is "what's on disk"; the scheduler decides retirement
        // policy. Pinning this separation prevents a refactor from
        // accidentally double-filtering or losing retired entries
        // that lens UI surfaces want to display.
        let dir = TempDir::new().unwrap();
        tokio::fs::write(
            dir.path().join("retired.toml"),
            r#"
name = "old"
prompt_text = "x"
retired = true

[source]
kind = "project_meta"
name = "old"

[model]
type = "default"

[trigger]
kind = "schedule"
interval = 60
"#,
        )
        .await
        .unwrap();
        let lenses = load_lenses_from_dir(dir.path()).await;
        assert_eq!(lenses.len(), 1);
        assert!(lenses[0].retired);
        // And the scheduler filters it out at registration.
        let scheduler = LensScheduler::new(lenses, ModelResolver::with_defaults());
        assert!(scheduler.is_empty());
    }

    // ── round-1 review-fix coverage ─────────────────────────────────

    #[tokio::test]
    async fn load_lenses_dedupes_by_name_not_by_path() {
        // Three files: two share `name = "dup"`, one has a unique
        // `name = "unique"`. The dedup is name-scoped, so the
        // duplicate is dropped but the differently-named lens loads
        // alongside the surviving copy of "dup". This pins both
        // halves of the dedup contract: same-name → drop, different-
        // name in same dir → both load.
        let dir = TempDir::new().unwrap();
        let body = |name: &str| {
            format!(
                r#"
name = "{name}"
prompt_text = "x"
retired = false

[source]
kind = "project_meta"
name = "m"

[model]
type = "default"

[trigger]
kind = "schedule"
interval = 60
"#
            )
        };
        tokio::fs::write(dir.path().join("a.toml"), body("dup"))
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("b.toml"), body("dup"))
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("c.toml"), body("unique"))
            .await
            .unwrap();
        let lenses = load_lenses_from_dir(dir.path()).await;
        assert_eq!(
            lenses.len(),
            2,
            "duplicate names dropped; differently-named lenses keep loading"
        );
        let names: std::collections::HashSet<String> =
            lenses.iter().map(|l| l.name.clone()).collect();
        assert!(names.contains("dup"));
        assert!(names.contains("unique"));
    }

    #[tokio::test]
    async fn multi_lens_tick_serializes_fires_and_first_failure_does_not_abort_the_rest() {
        // Three lenses all due on the first tick. First emits garbage
        // (parse error in run_with_tools); second + third return
        // valid LensOutput. Pin both the "errored lens doesn't abort"
        // and the "all due lenses are evaluated this tick" promises.
        let (_dir, mut corpus) = fresh_corpus().await;
        let embedder = FakeEmbedder { dim: 4 };
        let resolver = ModelResolver::with_defaults();

        // CannedBackend serves the SAME reply to every call. To have
        // mixed outcomes per-lens we need different backends per
        // fire. Easiest: a backend that errors regardless (the second
        // and third lenses fire too because the scheduler doesn't
        // abort on a single Err — they'll all error). We assert the
        // SCHEDULER kept evaluating, not that mixed outcomes coexist
        // (mixed outcomes require per-lens backends which the v1
        // interface doesn't expose).
        let bad_backend = CannedBackend::new("not json");
        let lens_a = test_lens(
            "first",
            TriggerPolicy::Schedule {
                interval: Duration::from_secs(60),
            },
        );
        let lens_b = test_lens(
            "second",
            TriggerPolicy::Schedule {
                interval: Duration::from_secs(60),
            },
        );
        let lens_c = test_lens(
            "third",
            TriggerPolicy::Schedule {
                interval: Duration::from_secs(60),
            },
        );
        let mut scheduler = LensScheduler::new(vec![lens_a, lens_b, lens_c], resolver);

        let summary = scheduler
            .run_one_tick(&bad_backend, &embedder, &mut corpus)
            .await;
        // All three should be in `fired` (attempted).
        assert_eq!(summary.fired.len(), 3);
        assert_eq!(
            summary.fired,
            vec![
                "first".to_string(),
                "second".to_string(),
                "third".to_string()
            ],
            "tick must iterate all due lenses in registration order"
        );
        // All three errored (same garbage backend reply). The error
        // ORDER should also match registration order — pinned so an
        // off-by-one that mis-attributes a failure (B's error under
        // A's name) would surface.
        let error_names: Vec<String> = summary.errors.iter().map(|(n, _)| n.clone()).collect();
        assert_eq!(
            error_names,
            vec![
                "first".to_string(),
                "second".to_string(),
                "third".to_string()
            ],
            "errors must be recorded in registration order alongside fired"
        );
    }

    #[tokio::test]
    async fn schedule_with_zero_interval_fires_every_tick() {
        // Degenerate case: `interval = 0` means "fire every tick"
        // (no minimum gap). Pin so the duration_since(t) >= interval
        // comparator's edge behavior is locked in.
        let (_dir, mut corpus) = fresh_corpus().await;
        let backend = CannedBackend::new(finished_lens_output());
        let embedder = FakeEmbedder { dim: 4 };
        let resolver = ModelResolver::with_defaults();
        let lens = test_lens(
            "every-tick",
            TriggerPolicy::Schedule {
                interval: Duration::from_secs(0),
            },
        );
        let mut scheduler = LensScheduler::new(vec![lens], resolver);

        // Two back-to-back ticks should both fire.
        let first = scheduler
            .run_one_tick(&backend, &embedder, &mut corpus)
            .await;
        assert_eq!(first.fired.len(), 1);
        let second = scheduler
            .run_one_tick(&backend, &embedder, &mut corpus)
            .await;
        assert_eq!(
            second.fired.len(),
            1,
            "interval = 0 must fire every tick (no minimum gap)"
        );
    }

    #[tokio::test]
    async fn load_lenses_skips_symlinks_but_keeps_regular_files() {
        // Two files: one regular TOML (must still load), one symlink
        // to a TOML elsewhere (must be skipped). Pins both halves so
        // a regression that broadened the skip (e.g. accidentally
        // checking `!is_file()`) would surface here.
        let dir = TempDir::new().unwrap();
        let target_dir = TempDir::new().unwrap();
        let target = target_dir.path().join("target.toml");
        let body = |name: &str| {
            format!(
                r#"
name = "{name}"
prompt_text = "x"
retired = false

[source]
kind = "project_meta"
name = "m"

[model]
type = "default"

[trigger]
kind = "schedule"
interval = 60
"#
            )
        };
        tokio::fs::write(&target, body("from-symlink"))
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("regular.toml"), body("regular"))
            .await
            .unwrap();
        // Create a symlink in the lens dir pointing at the target.
        let symlink_path = dir.path().join("linked.toml");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&target, &symlink_path).unwrap();
        }
        #[cfg(not(unix))]
        {
            // On non-unix platforms (we don't ship for these today),
            // verify the regular file path still works and skip the
            // symlink half.
            let _ = (target, symlink_path);
            let lenses = load_lenses_from_dir(dir.path()).await;
            assert_eq!(lenses.len(), 1);
            assert_eq!(lenses[0].name, "regular");
            return;
        }
        let lenses = load_lenses_from_dir(dir.path()).await;
        assert_eq!(
            lenses.len(),
            1,
            "regular TOML loads; symlinked TOML skipped"
        );
        assert_eq!(lenses[0].name, "regular");
    }
}
