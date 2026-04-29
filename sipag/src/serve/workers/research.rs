//! Research worker.
//!
//! Trigger label: `research`. The worker:
//!   1. Cross-repo `insights::search` with the item title (n=10).
//!   2. Extracts tags from results and runs follow-up searches.
//!   3. Calls ollama to synthesize themes / tensions / open questions.
//!   4. Posts progress events to the item's discourse topic.
//!   5. On completion, posts a `worker.complete` event with the synthesis
//!      and citations (commit SHAs).
//!   6. Removes the `research` label. Adds `attention` if the synthesis
//!      flags something the user should review.

use crate::serve::insights::{self, Insight};
use crate::serve::workers::{publish_progress, ItemKind, Worker, WorkerCtx, WorkerItem};
use anyhow::Result;
use sipag_core::llm::{chat, env_host, ChatMessage, ChatOptions};
use std::collections::BTreeSet;

/// Worker name surfaced in events.
const WORKER_NAME: &str = "research";

pub struct ResearchWorker;

#[async_trait::async_trait]
impl Worker for ResearchWorker {
    fn label(&self) -> &'static str {
        "research"
    }

    fn name(&self) -> &'static str {
        WORKER_NAME
    }

    async fn run(&self, ctx: &WorkerCtx, item: WorkerItem) -> Result<()> {
        publish_progress(&ctx.broker, &item, WORKER_NAME, "starting research");

        // Stage 1 — pull broad context from the indexed history.
        publish_progress(
            &ctx.broker,
            &item,
            WORKER_NAME,
            "exploring indexed insights…",
        );
        let primary = insights::search(&item.title, None, 10)
            .await
            .unwrap_or_default();
        publish_progress(
            &ctx.broker,
            &item,
            WORKER_NAME,
            &format!("found {} primary insights", primary.len()),
        );

        // Stage 2 — pull tags out of those, search a second time.
        let tag_set = collect_tags(&primary);
        let mut secondary = Vec::new();
        for tag in tag_set.iter().take(3) {
            publish_progress(
                &ctx.broker,
                &item,
                WORKER_NAME,
                &format!("expanding context via tag '{tag}'"),
            );
            let extra = insights::search(tag, None, 5).await.unwrap_or_default();
            secondary.extend(extra);
        }

        // Stage 3 — synthesize via ollama.
        publish_progress(&ctx.broker, &item, WORKER_NAME, "synthesizing themes…");
        let synthesis = match synthesize(&ctx.http, &item, &primary, &secondary).await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("ollama synthesis failed: {e}");
                let payload = serde_json::json!({
                    "worker": WORKER_NAME,
                    "error": e.to_string(),
                });
                let _ = ctx
                    .broker
                    .publish(&item.discourse_topic(), "worker.error", payload);
                // Strip the trigger label so we don't loop on a permanent
                // failure; surface via `error` label.
                let _ = mark_label_change_via_state(
                    ctx,
                    &item,
                    &["error".into()],
                    &["research".into()],
                );
                return Err(e);
            }
        };

        let citations: Vec<String> = primary
            .iter()
            .chain(secondary.iter())
            .map(|i| i.commit_sha.clone())
            .collect();
        let payload = serde_json::json!({
            "worker": WORKER_NAME,
            "text": synthesis,
            "citations": citations,
            "primary_count": primary.len(),
            "secondary_count": secondary.len(),
        });
        let _ = ctx
            .broker
            .publish(&item.discourse_topic(), "worker.complete", payload);

        // Heuristic: if the synthesis raises a flag, route to the human.
        let needs_attention = synthesis.to_lowercase().contains("contentious")
            || synthesis.to_lowercase().contains("tension")
            || synthesis.to_lowercase().contains("uncertain");

        let mut add: Vec<String> = Vec::new();
        if needs_attention {
            add.push("attention".to_string());
        }
        let _ = mark_label_change_via_state(ctx, &item, &add, &["research".into()]);
        Ok(())
    }
}

fn collect_tags(insights: &[Insight]) -> BTreeSet<String> {
    let mut set = BTreeSet::new();
    for i in insights {
        for raw in i.tags.split([',', ' ']) {
            let t = raw.trim();
            if t.len() > 2 && t.len() < 40 {
                set.insert(t.to_lowercase());
            }
        }
    }
    set
}

async fn synthesize(
    http: &reqwest::Client,
    item: &WorkerItem,
    primary: &[Insight],
    secondary: &[Insight],
) -> Result<String> {
    // Cap context size aggressively. gemma4:31b's prefill on long prompts
    // can exceed Cloudflare's 100s edge timeout before any response byte
    // ships, and the bridge keepalive only kicks in after upstream sends
    // its response headers — too late. 8 insights × 160 chars keeps the
    // prefill comfortably under that budget.
    let mut bullets = String::new();
    for ins in primary.iter().chain(secondary.iter()).take(8) {
        bullets.push_str(&format!(
            "- [{cat}] {title} ({sha})\n  {body}\n",
            cat = ins.category,
            title = ins.title,
            sha = ins.commit_sha.chars().take(7).collect::<String>(),
            body = first_n_chars(&ins.body, 160),
        ));
    }

    let kind_label = match item.kind {
        ItemKind::KeyResult => "Key Result",
        ItemKind::Task => "Task",
    };

    let user_prompt = format!(
        "{kind_label}: {title}\n\nProject: {project}\n\nRelated insights from the codebase:\n{bullets}\n\nSynthesize, in 4–8 short bullet points: themes, tensions, prior decisions that bear on this, and 1–2 questions worth surfacing to the human.",
        title = item.title,
        project = item.project,
    );

    let messages = vec![
        ChatMessage::system(
            "You are a research assistant for an OKR system. \
             Be concise. Surface tensions, name decisions clearly, \
             and call out anything contentious so a human can weigh in.",
        ),
        ChatMessage::user(user_prompt),
    ];

    // gemma4:31b is a thinking model — its `thinking` tokens count
    // against `num_predict`. With ~1500 tokens of prompt context (15
    // insights of up to 240 chars each plus system prompt), thinking
    // can run 1000-3000 tokens before any visible content emerges.
    // 4000 leaves comfortable room for thinking + the 4-8 short bullet
    // points requested.
    let opts = ChatOptions {
        temperature: 0.4,
        num_predict: Some(4000),
        ..ChatOptions::default()
    };
    let host = env_host();
    let answer = chat(http, &host, messages, opts).await?;
    Ok(answer)
}

fn first_n_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// Helper that wires the worker context's broker/disk back into the
/// label-change function used by the scheduler. Avoids a circular
/// dep by re-loading the state via the WorkerCtx fields.
fn mark_label_change_via_state(
    ctx: &WorkerCtx,
    item: &WorkerItem,
    add_labels: &[String],
    remove_labels: &[String],
) -> anyhow::Result<()> {
    use sipag_core::board::{KeyResult, Task};
    match item.kind {
        ItemKind::KeyResult => {
            let mut kr = KeyResult::load(&ctx.sipag_dir, &item.project, item.id)?;
            apply(&mut kr.labels, add_labels, remove_labels);
            kr.save(&ctx.sipag_dir, &item.project)?;
        }
        ItemKind::Task => {
            let mut t = Task::load(&ctx.sipag_dir, &item.project, item.id)?;
            apply(&mut t.labels, add_labels, remove_labels);
            t.updated = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
            t.save(&ctx.sipag_dir, &item.project)?;
        }
    }
    Ok(())
}

fn apply(labels: &mut Vec<String>, add: &[String], remove: &[String]) {
    labels.retain(|l| !remove.contains(l));
    for a in add {
        if !labels.iter().any(|l| l == a) {
            labels.push(a.clone());
        }
    }
}
