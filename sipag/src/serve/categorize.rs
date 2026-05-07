//! Gemma4-driven KR proposals for misc observations.
//!
//! When a katulong session shows up in sipag's misc tray (uncategorized),
//! we want to suggest which KR it belongs to. The user accepts (one
//! click → kr_id set) or overrides (pick another).
//!
//! This module is deliberately a plain function plus an in-memory cache
//! rather than a `Worker` trait implementation — proposals are
//! best-effort, render-time hints, not pubsub-choreographed jobs.

use serde::Deserialize;
use sipag_core::llm::{chat, env_auth, env_host, env_model, ChatMessage, ChatOptions};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// One candidate KR a session could be categorized into. Objective-scoped:
/// gemma4 sees the objective's aspiration sentence as context for *why*
/// the KR exists, which produces better matches than a bare KR title.
#[derive(Debug, Clone)]
pub struct KrChoice {
    pub objective: String,
    pub objective_aspiration: String,
    pub kr: u64,
    pub kr_title: String,
}

/// gemma4's recommendation for a misc observation.
#[derive(Debug, Clone)]
pub struct KrProposal {
    pub objective: String,
    pub objective_aspiration: String,
    pub kr: u64,
    pub kr_title: String,
    pub confidence: u8,
    pub reason: String,
}

/// Cache state for an observation's proposal. The hashed summary lets
/// us re-run gemma4 when the underlying summary changes (katulong's
/// summarizer rewrites it as the session evolves) without re-running
/// on every render.
#[derive(Debug, Clone)]
pub enum ProposalState {
    /// Background task in flight; render skips the chip this cycle.
    Pending,
    /// gemma4 had a suggestion. `hash` keys it to a specific summary.
    Some { proposal: KrProposal, hash: u64 },
    /// gemma4 said "no fit". Same hash semantics.
    NoFit { hash: u64 },
    /// User rejected the previous proposal at this summary hash. Don't
    /// re-propose until the summary changes.
    Rejected { hash: u64 },
}

/// Hash a summary so we can decide whether a cached proposal is stale.
pub fn summary_hash(s: &str) -> u64 {
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

/// Ask gemma4 which KR (if any) a session described by `summary`
/// belongs to, given the available `krs`. Returns `None` on transport
/// errors, on parse failures, or when gemma4 picks "0 = no fit".
pub async fn propose_kr(
    http: &reqwest::Client,
    summary: &str,
    krs: &[KrChoice],
) -> Option<KrProposal> {
    if krs.is_empty() || summary.trim().is_empty() {
        return None;
    }

    // Group choices by objective so gemma4 sees the aspiration as
    // context. The numbering is global across the whole list (1, 2, 3…)
    // so the model returns a single index.
    let mut current_obj = String::new();
    let mut lines = Vec::new();
    for (i, c) in krs.iter().enumerate() {
        if c.objective != current_obj {
            current_obj = c.objective.clone();
            lines.push(format!(
                "\n[objective: {}]\n  aspiration: {}",
                c.objective, c.objective_aspiration
            ));
        }
        lines.push(format!("{}. {}", i + 1, c.kr_title));
    }
    let krs_text = lines.join("\n");

    let system = "You categorize a Claude Code session into the Key Result that best \
        captures what direction the session is moving us toward.\n\
        \n\
        Each KR lives under an Objective (an asymptotic aspiration — a direction \
        we're approaching but never finish reaching). The Objective gives you the \
        context for *why* a KR exists; pick the KR that most directly advances its \
        Objective given what the session is actually doing.\n\
        \n\
        Reply with a single JSON object on one line. No markdown, no commentary.\n\
        Schema: {\"choice\": <int>, \"confidence\": <int 0-100>, \"reason\": \"<6-word phrase>\"}\n\
        \n\
        Use choice=0 when no KR is a real fit — better to leave the session \
        uncategorized than to muddy the data. Use confidence<50 only when you're \
        guessing.";

    let user =
        format!("Session summary: {summary}\n\nAvailable KRs (across objectives):\n{krs_text}");

    // gemma4:31b is a reasoning model — it spends ~200 tokens "thinking"
    // before emitting the actual JSON content. A tight num_predict (e.g.
    // 120) kills generation mid-thought and the stream finishes with
    // empty content. 2048 leaves plenty of headroom for the chain of
    // thought plus the small JSON answer.
    let opts = ChatOptions {
        model: env_model(),
        temperature: 0.2,
        num_predict: Some(2048),
        auth_bearer: env_auth(),
    };
    let messages = vec![ChatMessage::system(system), ChatMessage::user(user)];

    let raw = match chat(http, &env_host(), messages, opts).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("categorize: gemma4 call failed: {e}");
            return None;
        }
    };

    let json_str = extract_json_object(&raw)?;
    let parsed: ProposalReply = match serde_json::from_str(json_str) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("categorize: gemma4 reply not JSON ({e}): {raw}");
            return None;
        }
    };

    if parsed.choice == 0 || parsed.choice > krs.len() {
        return None;
    }
    let chosen = &krs[parsed.choice - 1];
    Some(KrProposal {
        objective: chosen.objective.clone(),
        objective_aspiration: chosen.objective_aspiration.clone(),
        kr: chosen.kr,
        kr_title: chosen.kr_title.clone(),
        confidence: parsed.confidence.clamp(0, 100),
        reason: parsed.reason.trim().to_string(),
    })
}

/// Find the first {...} block in a free-text gemma4 reply. Models
/// occasionally wrap JSON in prose ("Here's my answer: {...}") even
/// when told not to; this is forgiving about that.
fn extract_json_object(s: &str) -> Option<&str> {
    let start = s.find('{')?;
    let end = s.rfind('}')?;
    if end > start {
        Some(&s[start..=end])
    } else {
        None
    }
}

#[derive(Deserialize)]
struct ProposalReply {
    choice: usize,
    #[serde(default)]
    confidence: u8,
    #[serde(default)]
    reason: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_json_handles_prose_wrapping() {
        let s = "Sure, here's my answer: {\"choice\": 2, \"confidence\": 80, \"reason\": \"matches goal\"} hope that helps";
        let extracted = extract_json_object(s).unwrap();
        assert!(extracted.starts_with('{') && extracted.ends_with('}'));
        let parsed: ProposalReply = serde_json::from_str(extracted).unwrap();
        assert_eq!(parsed.choice, 2);
        assert_eq!(parsed.confidence, 80);
    }

    #[test]
    fn extract_json_returns_none_when_no_braces() {
        assert!(extract_json_object("nothing here").is_none());
    }

    #[test]
    fn summary_hash_stable() {
        assert_eq!(summary_hash("hello"), summary_hash("hello"));
        assert_ne!(summary_hash("hello"), summary_hash("world"));
    }
}
