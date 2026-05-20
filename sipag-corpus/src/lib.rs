//! Local vector store for sipag — the storage substrate underneath
//! lens-workers.
//!
//! Per `docs/modules.md` §3 (Phase 1 #3) and `docs/extraction-plan.md`
//! §3: append-only forever, tagged + timestamped, embedded via
//! `ollama-bridge-client` → bridge → `/api/embed`. One giant store;
//! tags + timestamps + cosine-similarity search are the slicing.
//!
//! ## What this crate owns
//!
//! - [`CorpusItem`] — the value object. Content + embedding + tags
//!   + timestamp + source-refs + generation. Append-only forever.
//! - [`Corpus`] — the storage. JSONL-on-disk at
//!   `~/.sipag/corpus/items.jsonl` (or a caller-supplied path);
//!   full corpus loaded into memory on `open` because vector search
//!   is linear-scan in v1.
//! - [`Embedder`] — async trait. One method: turn text into a
//!   `Vec<f32>`. Optional concrete impl `BridgeEmbedder` wraps
//!   `ollama-bridge-client::OllamaBridgeClient` (gated behind the
//!   default `bridge` cargo feature so tests + alternate impls can
//!   drop the dep).
//! - [`SearchFilter`] — tag any-of + timestamp range. Composes with
//!   [`Corpus::search`].
//!
//! ## What this crate does NOT own
//!
//! - **Model selection.** Whether to embed with
//!   `nomic-embed-text` vs `mxbai-embed-large` etc. lives in
//!   `sipag-lens`'s lens definition + `~/.sipag/models.toml`
//!   profile resolution.
//! - **Prompt composition.** Lens-workers compose prompts before
//!   calling the corpus + bridge.
//! - **MCP tool surface** (`corpus.search` / `corpus.expand`).
//!   Those wrappers live in `sipag-lens`, called by gemma
//!   mid-prompt. This crate provides the underlying primitives;
//!   the tool wrapper translates between the lens's view and
//!   these calls.
//! - **Compaction / pruning.** Append-only forever per the design.
//!   Cross-corpus retrieval (sipag ↔ diwa) is open — see modules.md
//!   §10.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use thiserror::Error;
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::warn;

// ── value object ────────────────────────────────────────────────────

/// One row in the corpus. The unit of storage and search.
///
/// **Append-only forever.** No update API. Edits = new items with
/// `source_refs` pointing at the prior version.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorpusItem {
    /// Monotonically increasing per-corpus id. First item is `1`.
    pub id: u64,
    /// Free-form text. The thing being remembered.
    pub content: String,
    /// Vector embedding produced by an [`Embedder`] at write time.
    /// Length depends on the model used; comparisons assume same
    /// length (see [`Corpus::search`]).
    pub embedding: Vec<f32>,
    /// Free-form tags. Sliced via [`SearchFilter::tags`] at search
    /// time. Typical: `lens=<name>`, `kr_ref=<id>`,
    /// `session=<sid>`, `valence=<pos|neg|neutral>`, etc.
    pub tags: Vec<String>,
    /// When this was added. UTC.
    pub timestamp: DateTime<Utc>,
    /// Source items this was derived from. Empty for
    /// generation-0 (raw observations). Lets `expand`-style
    /// traversal walk back through chains of derivations (the
    /// `corpus.expand` MCP tool in sipag-lens will use this).
    pub source_refs: Vec<u64>,
    /// 0 = raw observation. N = derived from generation N-1 items.
    /// Used by sipag-lens's meta-cognitive guardrails to flag deep
    /// derivation chains with thin source support.
    pub generation: u8,
}

// ── filter ──────────────────────────────────────────────────────────

/// Slicing predicate applied at search time. All present fields must
/// match (AND); within `tags`, ANY tag in the list must match (OR).
#[derive(Debug, Clone, Default)]
pub struct SearchFilter {
    /// Any-of tag match. `None` means no tag filter. `Some([])` is
    /// a degenerate "no possible match" (intentional; matches the
    /// "empty whitelist" intuition).
    pub tags: Option<Vec<String>>,
    /// Items with `timestamp >= after`. `None` = no lower bound.
    pub timestamp_after: Option<DateTime<Utc>>,
    /// Items with `timestamp <= before`. `None` = no upper bound.
    pub timestamp_before: Option<DateTime<Utc>>,
    /// Items with `generation <= generation_at_most`. Used by
    /// lens-workers that want to exclude high-generation derived
    /// items (e.g. "search raw observations only" sets this to 0).
    pub generation_at_most: Option<u8>,
}

impl SearchFilter {
    fn matches(&self, item: &CorpusItem) -> bool {
        if let Some(tag_set) = &self.tags {
            // `Some([])` intentionally matches nothing — empty
            // whitelist semantics.
            if tag_set.is_empty() {
                return false;
            }
            if !tag_set.iter().any(|t| item.tags.contains(t)) {
                return false;
            }
        }
        if let Some(after) = self.timestamp_after {
            if item.timestamp < after {
                return false;
            }
        }
        if let Some(before) = self.timestamp_before {
            if item.timestamp > before {
                return false;
            }
        }
        if let Some(max) = self.generation_at_most {
            if item.generation > max {
                return false;
            }
        }
        true
    }
}

// ── embedder ────────────────────────────────────────────────────────

/// Async embedder. One method: text in, `Vec<f32>` out.
///
/// Sipag's production impl is [`BridgeEmbedder`] (under the
/// `bridge` cargo feature, default-enabled), which routes through
/// `ollama-bridge-client::OllamaBridgeClient::submit_and_wait`.
/// Tests use a deterministic fake embedder (hash-derived fixed-
/// dimension vectors); see this crate's test module.
#[async_trait]
pub trait Embedder: Send + Sync {
    /// Embed a single text. Return value length should be stable
    /// per-embedder-impl — [`Corpus::search`] assumes query +
    /// stored embeddings have matching dimensions.
    async fn embed(&self, text: &str) -> CorpusResult<Vec<f32>>;
}

/// Production embedder — wraps `OllamaBridgeClient::submit_and_wait`
/// against the bridge's `/api/embed` endpoint. Caller picks the
/// model (typically from `~/.sipag/models.toml` profile
/// resolution, which lives in sipag-lens).
///
/// Gated behind the default `bridge` cargo feature. Disable
/// (`default-features = false`) when consuming sipag-corpus with
/// an alternate embedder.
#[cfg(feature = "bridge")]
pub struct BridgeEmbedder {
    client: ollama_bridge_client::OllamaBridgeClient,
    model: String,
    timeout: std::time::Duration,
}

#[cfg(feature = "bridge")]
impl BridgeEmbedder {
    /// Build with a pre-constructed client + model name (e.g.
    /// `"nomic-embed-text"`). Default per-call timeout: 30s.
    pub fn new(client: ollama_bridge_client::OllamaBridgeClient, model: String) -> Self {
        Self {
            client,
            model,
            timeout: std::time::Duration::from_secs(30),
        }
    }

    /// Override the per-call timeout (default 30s).
    pub fn with_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

#[cfg(feature = "bridge")]
#[async_trait]
impl Embedder for BridgeEmbedder {
    async fn embed(&self, text: &str) -> CorpusResult<Vec<f32>> {
        let body = serde_json::json!({
            "model": self.model,
            "input": text,
        });
        let result = self
            .client
            .submit_and_wait(ollama_bridge_client::JobEndpoint::Embed, body, self.timeout)
            .await
            .map_err(|e| CorpusError::Embed(e.to_string()))?;
        // Ollama's /api/embed response shape:
        //   { "embedding": [...] }  (older single-input)
        // or
        //   { "embeddings": [[...]] } (newer batch form)
        // The bridge passes through; handle both.
        if let Some(arr) = result.get("embedding").and_then(|v| v.as_array()) {
            return parse_f32_array(arr);
        }
        if let Some(arr) = result
            .get("embeddings")
            .and_then(|v| v.as_array())
            .and_then(|outer| outer.first())
            .and_then(|v| v.as_array())
        {
            return parse_f32_array(arr);
        }
        Err(CorpusError::Embed(format!(
            "neither 'embedding' nor 'embeddings' field in bridge response: {result}"
        )))
    }
}

#[cfg(feature = "bridge")]
fn parse_f32_array(arr: &[serde_json::Value]) -> CorpusResult<Vec<f32>> {
    arr.iter()
        .map(|v| {
            v.as_f64()
                .map(|x| x as f32)
                .ok_or_else(|| CorpusError::Embed(format!("non-numeric embedding entry: {v}")))
        })
        .collect()
}

// ── errors ──────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum CorpusError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("embedder error: {0}")]
    Embed(String),

    #[error("dimension mismatch: query={query_dim}, stored={stored_dim}")]
    DimensionMismatch { query_dim: usize, stored_dim: usize },

    #[error("corpus not opened (call Corpus::open first)")]
    NotOpened,
}

pub type CorpusResult<T> = std::result::Result<T, CorpusError>;

// ── corpus ──────────────────────────────────────────────────────────

/// The vector store itself. Append-only on disk as JSONL.
///
/// Cheap to clone? No — holds an owned `Vec<CorpusItem>`. Build
/// once at process start, share via `Arc<Mutex<Corpus>>` or
/// similar at the caller side.
pub struct Corpus {
    /// Directory holding `items.jsonl` (and any future index
    /// files). The `open` constructor creates the dir if absent.
    dir: PathBuf,
    /// In-memory copy of all items. Linear-scan search is fine for
    /// thousands of items; replace with HNSW or similar when this
    /// becomes the bottleneck (won't be in v1).
    items: Vec<CorpusItem>,
    /// Next id to assign. `add` returns this then increments.
    next_id: u64,
}

impl Corpus {
    /// Open (or create) a corpus rooted at `dir`. Reads
    /// `<dir>/items.jsonl` if present and pre-loads all items into
    /// memory. Creates `<dir>` if it doesn't exist.
    pub async fn open(dir: impl AsRef<Path>) -> CorpusResult<Self> {
        let dir = dir.as_ref().to_path_buf();
        tokio::fs::create_dir_all(&dir).await?;
        let path = dir.join("items.jsonl");
        let mut items = Vec::new();
        let mut next_id: u64 = 1;
        if path.exists() {
            let file = File::open(&path).await?;
            let mut lines = BufReader::new(file).lines();
            while let Some(line) = lines.next_line().await? {
                if line.trim().is_empty() {
                    continue;
                }
                match serde_json::from_str::<CorpusItem>(&line) {
                    Ok(item) => {
                        if item.id >= next_id {
                            next_id = item.id + 1;
                        }
                        items.push(item);
                    }
                    Err(e) => {
                        // Malformed line: log and skip. Append-only
                        // forever doesn't preclude tolerating a
                        // corrupted line — the alternative (refuse
                        // to open) is operationally worse.
                        warn!(error = %e, "corpus: skipping malformed JSONL line");
                    }
                }
            }
        }
        Ok(Self {
            dir,
            items,
            next_id,
        })
    }

    /// Number of items currently in the corpus.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// True if no items.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Add an item using a pre-computed embedding. Returns the
    /// assigned id. Appends to the on-disk JSONL log AND the
    /// in-memory vector.
    ///
    /// For the convenience "embed-then-add" flow that uses an
    /// [`Embedder`], see [`Self::add_text`].
    pub async fn add(
        &mut self,
        content: String,
        embedding: Vec<f32>,
        tags: Vec<String>,
        source_refs: Vec<u64>,
        generation: u8,
    ) -> CorpusResult<u64> {
        let id = self.next_id;
        self.next_id += 1;
        let item = CorpusItem {
            id,
            content,
            embedding,
            tags,
            timestamp: Utc::now(),
            source_refs,
            generation,
        };
        self.append_to_disk(&item).await?;
        self.items.push(item);
        Ok(id)
    }

    /// Convenience: embed `content` via the provided embedder, then
    /// [`Self::add`]. Most callers (lens-workers) use this; the
    /// split exists so tests + alternate-embedder integrations can
    /// pass a known embedding without invoking an Embedder trait.
    pub async fn add_text<E: Embedder>(
        &mut self,
        embedder: &E,
        content: String,
        tags: Vec<String>,
        source_refs: Vec<u64>,
        generation: u8,
    ) -> CorpusResult<u64> {
        let embedding = embedder.embed(&content).await?;
        self.add(content, embedding, tags, source_refs, generation)
            .await
    }

    /// Cosine-similarity k-NN search with optional filters.
    /// Returns up to `top_k` items sorted by similarity score
    /// (highest first).
    ///
    /// **Linear scan over the full corpus.** Adequate for
    /// thousands of items; replace with an index when it isn't.
    /// `query` must match the dimension of stored embeddings, or
    /// the function returns [`CorpusError::DimensionMismatch`]
    /// (caller bug — using a different model than what populated
    /// the corpus).
    pub fn search(
        &self,
        query: &[f32],
        filter: &SearchFilter,
        top_k: usize,
    ) -> CorpusResult<Vec<(f32, &CorpusItem)>> {
        if self.items.is_empty() || top_k == 0 {
            return Ok(Vec::new());
        }
        // Dimension check against the first stored item. All items
        // in a single corpus must share dimension (caller invariant
        // — same model throughout).
        let stored_dim = self.items[0].embedding.len();
        if query.len() != stored_dim {
            return Err(CorpusError::DimensionMismatch {
                query_dim: query.len(),
                stored_dim,
            });
        }
        let mut scored: Vec<(f32, &CorpusItem)> = self
            .items
            .iter()
            .filter(|i| filter.matches(i))
            .filter(|i| !i.embedding.is_empty() && i.embedding.len() == stored_dim)
            .map(|i| (cosine_similarity(query, &i.embedding), i))
            .collect();
        // Sort descending by score. NaN gets pushed to the end via
        // unwrap_or(Less) — better than panicking.
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Less));
        scored.truncate(top_k);
        Ok(scored)
    }

    /// Read one item by id. Returns `None` if not present. O(N)
    /// today — replace with a HashMap when an id-index is worth it.
    pub fn get(&self, id: u64) -> Option<&CorpusItem> {
        self.items.iter().find(|i| i.id == id)
    }

    /// Iterate items in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = &CorpusItem> {
        self.items.iter()
    }

    async fn append_to_disk(&self, item: &CorpusItem) -> CorpusResult<()> {
        let path = self.dir.join("items.jsonl");
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await?;
        let mut line = serde_json::to_string(item)?;
        line.push('\n');
        file.write_all(line.as_bytes()).await?;
        file.flush().await?;
        Ok(())
    }
}

/// Cosine similarity. Both vectors must be same length (checked by
/// the caller). Returns 0 for any-zero-norm input (avoids NaN).
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len());
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        0.0
    } else {
        dot / (norm_a * norm_b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use tempfile::TempDir;

    // ── test embedder ────────────────────────────────────────────────
    //
    // Deterministic fake: hashes the text into a 4-dim fixed vector.
    // Same text → same vector; distinct text → distinct vector. Good
    // enough to test similarity ranking + persistence without spinning
    // up a real ollama-bridge.

    struct FakeEmbedder {
        dim: usize,
    }

    impl FakeEmbedder {
        fn new(dim: usize) -> Self {
            Self { dim }
        }
    }

    #[async_trait]
    impl Embedder for FakeEmbedder {
        async fn embed(&self, text: &str) -> CorpusResult<Vec<f32>> {
            // Hash text into a deterministic vector. Use the byte
            // values as floats and pad/truncate to `dim`.
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

    // ── feature-requirement tests ───────────────────────────────────
    //
    // Per the user direction, tests focus on what the crate
    // PROMISES to callers (lens-workers, eventually): items persist,
    // search ranks by similarity, filters narrow results, append-
    // only invariants hold, dimension mismatches surface as errors.

    #[tokio::test]
    async fn add_persists_to_disk_and_in_memory() {
        let dir = TempDir::new().unwrap();
        let mut c = Corpus::open(dir.path()).await.unwrap();
        let id = c
            .add(
                "hello".into(),
                vec![1.0, 0.0],
                vec!["lens=greeting".into()],
                vec![],
                0,
            )
            .await
            .unwrap();
        assert_eq!(id, 1);
        assert_eq!(c.len(), 1);
        // On-disk file exists.
        assert!(dir.path().join("items.jsonl").exists());
    }

    #[tokio::test]
    async fn reopen_recovers_items_and_continues_id_sequence() {
        let dir = TempDir::new().unwrap();
        {
            let mut c = Corpus::open(dir.path()).await.unwrap();
            c.add("first".into(), vec![1.0], vec![], vec![], 0)
                .await
                .unwrap();
            c.add("second".into(), vec![0.5], vec![], vec![], 0)
                .await
                .unwrap();
        }
        // Reopen — items survive, next id continues at 3.
        let mut c = Corpus::open(dir.path()).await.unwrap();
        assert_eq!(c.len(), 2);
        let id = c
            .add("third".into(), vec![0.0], vec![], vec![], 0)
            .await
            .unwrap();
        assert_eq!(id, 3);
    }

    #[tokio::test]
    async fn add_text_uses_embedder_and_returns_id() {
        let dir = TempDir::new().unwrap();
        let mut c = Corpus::open(dir.path()).await.unwrap();
        let embedder = FakeEmbedder::new(4);
        let id = c
            .add_text(&embedder, "the cat sat".into(), vec![], vec![], 0)
            .await
            .unwrap();
        assert_eq!(id, 1);
        let item = c.get(1).unwrap();
        assert_eq!(item.content, "the cat sat");
        assert_eq!(item.embedding.len(), 4);
    }

    #[tokio::test]
    async fn search_ranks_by_cosine_similarity() {
        let dir = TempDir::new().unwrap();
        let mut c = Corpus::open(dir.path()).await.unwrap();
        // Three items with distinct directions.
        c.add("a".into(), vec![1.0, 0.0, 0.0], vec![], vec![], 0)
            .await
            .unwrap();
        c.add("b".into(), vec![0.0, 1.0, 0.0], vec![], vec![], 0)
            .await
            .unwrap();
        c.add("c".into(), vec![0.0, 0.0, 1.0], vec![], vec![], 0)
            .await
            .unwrap();
        // Query aligned with "b".
        let results = c
            .search(&[0.0, 1.0, 0.0], &SearchFilter::default(), 3)
            .unwrap();
        assert_eq!(results.len(), 3);
        // Top result is "b" (perfect cosine 1.0).
        assert_eq!(results[0].1.content, "b");
        assert!((results[0].0 - 1.0).abs() < 1e-6, "got: {}", results[0].0);
        // The other two are orthogonal (cosine 0.0).
        assert!((results[1].0).abs() < 1e-6);
        assert!((results[2].0).abs() < 1e-6);
    }

    #[tokio::test]
    async fn search_respects_top_k() {
        let dir = TempDir::new().unwrap();
        let mut c = Corpus::open(dir.path()).await.unwrap();
        for i in 0..5 {
            c.add(format!("item-{i}"), vec![i as f32, 1.0], vec![], vec![], 0)
                .await
                .unwrap();
        }
        let r = c.search(&[1.0, 1.0], &SearchFilter::default(), 2).unwrap();
        assert_eq!(r.len(), 2);
    }

    #[tokio::test]
    async fn search_tag_filter_narrows_to_any_of_match() {
        let dir = TempDir::new().unwrap();
        let mut c = Corpus::open(dir.path()).await.unwrap();
        c.add("a".into(), vec![1.0], vec!["kr=auth".into()], vec![], 0)
            .await
            .unwrap();
        c.add("b".into(), vec![1.0], vec!["kr=infra".into()], vec![], 0)
            .await
            .unwrap();
        c.add(
            "c".into(),
            vec![1.0],
            vec!["kr=auth".into(), "session=x".into()],
            vec![],
            0,
        )
        .await
        .unwrap();
        // Filter to kr=auth.
        let filter = SearchFilter {
            tags: Some(vec!["kr=auth".into()]),
            ..Default::default()
        };
        let r = c.search(&[1.0], &filter, 10).unwrap();
        assert_eq!(r.len(), 2);
        let names: Vec<&str> = r.iter().map(|(_, i)| i.content.as_str()).collect();
        assert!(names.contains(&"a"));
        assert!(names.contains(&"c"));
        assert!(!names.contains(&"b"));
    }

    #[tokio::test]
    async fn search_empty_tag_filter_matches_nothing() {
        // `Some([])` is empty-whitelist semantics — intentionally
        // matches nothing. Pin this so a refactor that flips it to
        // "no filter" doesn't silently change behavior.
        let dir = TempDir::new().unwrap();
        let mut c = Corpus::open(dir.path()).await.unwrap();
        c.add("a".into(), vec![1.0], vec!["x".into()], vec![], 0)
            .await
            .unwrap();
        let filter = SearchFilter {
            tags: Some(vec![]),
            ..Default::default()
        };
        let r = c.search(&[1.0], &filter, 10).unwrap();
        assert!(r.is_empty());
    }

    #[tokio::test]
    async fn search_timestamp_range_filter() {
        let dir = TempDir::new().unwrap();
        let mut c = Corpus::open(dir.path()).await.unwrap();
        // Add three items, mutate their timestamps to known values
        // for the test (only way to test ranges without sleeping).
        for content in ["old", "mid", "new"] {
            c.add(content.into(), vec![1.0], vec![], vec![], 0)
                .await
                .unwrap();
        }
        // Override timestamps directly on in-memory items. The
        // append-only-on-disk invariant doesn't help us here; we
        // need known fixed timestamps.
        let day0 = Utc.with_ymd_and_hms(2026, 5, 18, 0, 0, 0).unwrap();
        let day1 = Utc.with_ymd_and_hms(2026, 5, 19, 0, 0, 0).unwrap();
        let day2 = Utc.with_ymd_and_hms(2026, 5, 20, 0, 0, 0).unwrap();
        c.items[0].timestamp = day0;
        c.items[1].timestamp = day1;
        c.items[2].timestamp = day2;
        // Filter to [day1, day2].
        let filter = SearchFilter {
            timestamp_after: Some(day1),
            timestamp_before: Some(day2),
            ..Default::default()
        };
        let r = c.search(&[1.0], &filter, 10).unwrap();
        assert_eq!(r.len(), 2);
        let names: Vec<&str> = r.iter().map(|(_, i)| i.content.as_str()).collect();
        assert!(names.contains(&"mid"));
        assert!(names.contains(&"new"));
        assert!(!names.contains(&"old"));
    }

    #[tokio::test]
    async fn search_generation_filter_excludes_high_generation() {
        let dir = TempDir::new().unwrap();
        let mut c = Corpus::open(dir.path()).await.unwrap();
        c.add("raw".into(), vec![1.0], vec![], vec![], 0)
            .await
            .unwrap();
        c.add("derived1".into(), vec![1.0], vec![], vec![1], 1)
            .await
            .unwrap();
        c.add("derived2".into(), vec![1.0], vec![], vec![2], 2)
            .await
            .unwrap();
        // "Raw observations only" — generation_at_most = 0.
        let filter = SearchFilter {
            generation_at_most: Some(0),
            ..Default::default()
        };
        let r = c.search(&[1.0], &filter, 10).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].1.content, "raw");
    }

    #[tokio::test]
    async fn search_returns_empty_when_corpus_empty() {
        let dir = TempDir::new().unwrap();
        let c = Corpus::open(dir.path()).await.unwrap();
        let r = c.search(&[1.0, 0.0], &SearchFilter::default(), 10).unwrap();
        assert!(r.is_empty());
    }

    #[tokio::test]
    async fn search_returns_empty_when_top_k_is_zero() {
        let dir = TempDir::new().unwrap();
        let mut c = Corpus::open(dir.path()).await.unwrap();
        c.add("x".into(), vec![1.0], vec![], vec![], 0)
            .await
            .unwrap();
        let r = c.search(&[1.0], &SearchFilter::default(), 0).unwrap();
        assert!(r.is_empty());
    }

    #[tokio::test]
    async fn search_dimension_mismatch_surfaces_as_error() {
        let dir = TempDir::new().unwrap();
        let mut c = Corpus::open(dir.path()).await.unwrap();
        c.add("a".into(), vec![1.0, 0.0, 0.0], vec![], vec![], 0)
            .await
            .unwrap();
        // Query with wrong dimension (2 vs stored 3).
        let err = c
            .search(&[1.0, 0.0], &SearchFilter::default(), 10)
            .unwrap_err();
        match err {
            CorpusError::DimensionMismatch {
                query_dim,
                stored_dim,
            } => {
                assert_eq!(query_dim, 2);
                assert_eq!(stored_dim, 3);
            }
            other => panic!("expected DimensionMismatch, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn source_refs_preserved_for_derivation_chain_traversal() {
        // Pin the chain-of-derivations contract: a generation-1
        // item's source_refs point at the generation-0 items it
        // was derived from. (corpus.expand in sipag-lens walks
        // this; the storage layer just has to preserve it.)
        let dir = TempDir::new().unwrap();
        let mut c = Corpus::open(dir.path()).await.unwrap();
        let raw1 = c
            .add("raw1".into(), vec![1.0], vec![], vec![], 0)
            .await
            .unwrap();
        let raw2 = c
            .add("raw2".into(), vec![1.0], vec![], vec![], 0)
            .await
            .unwrap();
        let derived = c
            .add("derived".into(), vec![1.0], vec![], vec![raw1, raw2], 1)
            .await
            .unwrap();
        let item = c.get(derived).unwrap();
        assert_eq!(item.source_refs, vec![raw1, raw2]);
        assert_eq!(item.generation, 1);
        // Reopen and confirm the source_refs survive serialization.
        drop(c);
        let c2 = Corpus::open(dir.path()).await.unwrap();
        let item2 = c2.get(derived).unwrap();
        assert_eq!(item2.source_refs, vec![raw1, raw2]);
    }

    #[tokio::test]
    async fn malformed_jsonl_line_skipped_not_fatal() {
        // A garbled line on disk (partial write from a crash; manual
        // edit gone wrong) shouldn't prevent the corpus from
        // opening. Skip + warn. The append-only-forever invariant
        // is about WRITES, not READS.
        let dir = TempDir::new().unwrap();
        // Write a legit item, a garbage line, then a second legit
        // item (different id).
        let path = dir.path().join("items.jsonl");
        tokio::fs::write(
            &path,
            r#"{"id":1,"content":"first","embedding":[1.0],"tags":[],"timestamp":"2026-05-19T00:00:00Z","source_refs":[],"generation":0}
this is not json
{"id":2,"content":"second","embedding":[1.0],"tags":[],"timestamp":"2026-05-19T00:00:00Z","source_refs":[],"generation":0}
"#,
        )
        .await
        .unwrap();
        let c = Corpus::open(dir.path()).await.unwrap();
        assert_eq!(c.len(), 2);
        assert_eq!(c.get(1).unwrap().content, "first");
        assert_eq!(c.get(2).unwrap().content, "second");
    }

    #[tokio::test]
    async fn add_text_propagates_embedder_error() {
        struct FailingEmbedder;
        #[async_trait]
        impl Embedder for FailingEmbedder {
            async fn embed(&self, _text: &str) -> CorpusResult<Vec<f32>> {
                Err(CorpusError::Embed("simulated failure".into()))
            }
        }
        let dir = TempDir::new().unwrap();
        let mut c = Corpus::open(dir.path()).await.unwrap();
        let err = c
            .add_text(&FailingEmbedder, "x".into(), vec![], vec![], 0)
            .await
            .unwrap_err();
        match err {
            CorpusError::Embed(msg) => assert!(msg.contains("simulated")),
            other => panic!("expected Embed, got {other:?}"),
        }
        // Corpus stayed empty — the failure short-circuited before add().
        assert_eq!(c.len(), 0);
    }

    #[test]
    fn cosine_similarity_handles_zero_vector() {
        // Avoid NaN — zero-norm inputs return 0.0, not NaN.
        let s = cosine_similarity(&[0.0, 0.0], &[1.0, 0.0]);
        assert_eq!(s, 0.0);
    }

    #[test]
    fn cosine_similarity_matches_known_values() {
        // Orthogonal → 0; aligned → 1; opposite → -1.
        assert!((cosine_similarity(&[1.0, 0.0], &[0.0, 1.0]) - 0.0).abs() < 1e-6);
        assert!((cosine_similarity(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-6);
        assert!((cosine_similarity(&[1.0, 0.0], &[-1.0, 0.0]) + 1.0).abs() < 1e-6);
    }
}
