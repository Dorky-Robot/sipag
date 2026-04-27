//! Expand worker.
//!
//! Trigger label: `expand`. Reads the item's discourse history,
//! continues the conversation with ollama, and publishes the response
//! as `assistant.message` on the discourse topic.

use crate::serve::workers::{publish_progress, ItemKind, Worker, WorkerCtx, WorkerItem};
use anyhow::Result;
use sipag_core::llm::{chat, env_host, env_model, ChatMessage, ChatOptions};
use sipag_core::pubsub::Envelope;

const WORKER_NAME: &str = "expand";

pub struct ExpandWorker;

#[async_trait::async_trait]
impl Worker for ExpandWorker {
    fn label(&self) -> &'static str {
        "expand"
    }

    fn name(&self) -> &'static str {
        WORKER_NAME
    }

    async fn run(&self, ctx: &WorkerCtx, item: WorkerItem) -> Result<()> {
        publish_progress(
            &ctx.broker,
            &item,
            WORKER_NAME,
            "reading discourse history…",
        );
        let topic = item.discourse_topic();
        let history = ctx.broker.read(&topic, 0).unwrap_or_default();
        let messages = build_messages(&item, &history);

        publish_progress(&ctx.broker, &item, WORKER_NAME, "generating response…");
        let opts = ChatOptions {
            model: env_model(),
            temperature: 0.6,
            num_predict: Some(800),
        };
        let host = env_host();
        let response = match chat(&ctx.http, &host, messages, opts).await {
            Ok(s) => s,
            Err(e) => {
                let payload = serde_json::json!({
                    "worker": WORKER_NAME,
                    "error": e.to_string(),
                });
                let _ = ctx.broker.publish(&topic, "worker.error", payload);
                let _ = strip_label(ctx, &item, "expand", Some("error"));
                return Err(e.into());
            }
        };

        let payload = serde_json::json!({
            "worker": WORKER_NAME,
            "text": response,
        });
        let _ = ctx.broker.publish(&topic, "assistant.message", payload);
        let _ = strip_label(ctx, &item, "expand", None);
        Ok(())
    }
}

/// Translate the discourse log into chat-style messages for ollama.
fn build_messages(item: &WorkerItem, history: &[Envelope]) -> Vec<ChatMessage> {
    let kind_label = match item.kind {
        ItemKind::KeyResult => "Key Result",
        ItemKind::Task => "Task",
    };
    let mut msgs = vec![ChatMessage::system(format!(
        "You are continuing an ongoing R&D conversation about an OKR. The user is treating you as a thoughtful colleague — be concrete, ask sharp questions, build on what has already been said. The current focus:\n\n{kind_label}: {title}\nProject: {project}",
        title = item.title,
        project = item.project,
    ))];

    for env in history {
        let role = env.kind.as_str();
        let text = extract_text(env);
        if text.is_empty() {
            continue;
        }
        match role {
            "human.message" => msgs.push(ChatMessage::user(text)),
            "assistant.message" | "worker.complete" => msgs.push(ChatMessage::assistant(text)),
            // Worker progress / errors aren't conversation turns; skip.
            _ => continue,
        }
    }

    if msgs.len() == 1 {
        // No prior conversation — seed the model with the title so it
        // doesn't ask "what would you like to discuss?".
        msgs.push(ChatMessage::user(format!(
            "Help me think about: {}",
            item.title
        )));
    }
    msgs
}

fn extract_text(env: &Envelope) -> String {
    if let Some(text) = env.payload.get("text").and_then(|v| v.as_str()) {
        return text.to_string();
    }
    if let Some(message) = env.payload.get("message").and_then(|v| v.as_str()) {
        return message.to_string();
    }
    String::new()
}

fn strip_label(
    ctx: &WorkerCtx,
    item: &WorkerItem,
    remove: &str,
    add: Option<&str>,
) -> anyhow::Result<()> {
    use sipag_core::board::{KeyResult, Task};
    let add_v: Vec<String> = add.into_iter().map(|s| s.to_string()).collect();
    match item.kind {
        ItemKind::KeyResult => {
            let mut kr = KeyResult::load(&ctx.sipag_dir, &item.project, item.id)?;
            kr.labels.retain(|l| l != remove);
            for a in &add_v {
                if !kr.labels.iter().any(|l| l == a) {
                    kr.labels.push(a.clone());
                }
            }
            kr.save(&ctx.sipag_dir, &item.project)?;
        }
        ItemKind::Task => {
            let mut t = Task::load(&ctx.sipag_dir, &item.project, item.id)?;
            t.labels.retain(|l| l != remove);
            for a in &add_v {
                if !t.labels.iter().any(|l| l == a) {
                    t.labels.push(a.clone());
                }
            }
            t.updated = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
            t.save(&ctx.sipag_dir, &item.project)?;
        }
    }
    Ok(())
}
