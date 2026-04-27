//! HTMX + maud spike route.
//!
//! `GET /spike` serves a tiny page with one input + a hint pane. As
//! the user types, HTMX fires a debounced GET to `/spike/hint?q=…`
//! and swaps the returned HTML fragment into the pane. No JS framework,
//! no client-side state, no build step — just maud rendering on the
//! server and HTMX swapping the result in.
//!
//! What this validates: the ambient knowledge surface — drafting
//! input enriched with prior decisions / patterns / scars / learnings
//! pulled from the indexed knowledge base, surfaced contextually as
//! the user types. If this feels right, we drop the cljs SPA and
//! rebuild the whole UI in this shape.

use crate::serve::insights::{self, Insight};
use crate::serve::state::AppState;
use axum::{
    extract::{Query, State},
    response::{Html, IntoResponse, Response},
    routing::get,
    Router,
};
use maud::{html, Markup, PreEscaped, DOCTYPE};
use serde::Deserialize;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/spike", get(spike_page))
        .route("/spike/hint", get(spike_hint))
}

// ---------- /spike ----------

async fn spike_page() -> Response {
    let body = html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width,initial-scale=1";
                title { "spike · sipag" }
                style {
                    (PreEscaped(SPIKE_CSS))
                }
                script src="/js/htmx.min.js" defer {}
            }
            body {
                main.spike {
                    h1 { "Drafting surface · spike" }
                    p.subtle {
                        "As you type, sipag asks the knowledge base what we already know about this. "
                        "Decisions, patterns, scars, and learnings show up below the input — "
                        "operationalised serendipity at the moment of forming intent."
                    }

                    div.field {
                        label for="draft" { "What are you drafting?" }
                        input #draft
                            type="text"
                            name="q"
                            placeholder="objective, key result, or task title"
                            autocomplete="off"
                            "hx-get"="/spike/hint"
                            "hx-trigger"="keyup changed delay:300ms"
                            "hx-target"="#hint-pane"
                            "hx-swap"="innerHTML";
                        div #hint-pane.hint-pane {}
                    }

                    p.subtle.foot {
                        "Cross-repo scope. Try "
                        em { "auth" } ", " em { "passkey" } ", " em { "yaml" }
                        ", or " em { "scar" } "."
                    }
                }
            }
        }
    };
    Html(body.into_string()).into_response()
}

// ---------- /spike/hint ----------

#[derive(Debug, Deserialize)]
struct HintQuery {
    q: Option<String>,
    #[serde(default)]
    repo: Option<String>,
}

async fn spike_hint(
    State(_state): State<AppState>,
    Query(q): Query<HintQuery>,
) -> Response {
    let query = q.q.unwrap_or_default();
    let insights = insights::search(&query, q.repo.as_deref(), 3)
        .await
        .unwrap_or_default();

    if insights.is_empty() {
        return Html(String::new()).into_response();
    }
    Html(render_hint_rows(&insights).into_string()).into_response()
}

fn render_hint_rows(insights: &[Insight]) -> Markup {
    html! {
        ul.hint-rows {
            @for ins in insights {
                li.hint-row .{ "hint-" (cat_class(&ins.category)) } {
                    span.hint-cat title=(ins.category) { (cat_symbol(&ins.category)) }
                    span.hint-title { (ins.title) }
                    span.hint-meta {
                        @if !ins.repo.is_empty() {
                            (ins.repo) " · "
                        }
                        (relative_time(&ins.commit_date))
                    }
                }
            }
        }
    }
}

fn cat_symbol(c: &str) -> &'static str {
    match c.to_ascii_lowercase().as_str() {
        "decision" => "●",
        "pattern" => "◐",
        "scar" => "○",
        "learning" => "✓",
        _ => "·",
    }
}

fn cat_class(c: &str) -> String {
    c.to_ascii_lowercase()
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '-')
        .collect()
}

fn relative_time(iso: &str) -> String {
    use chrono::{DateTime, Utc};
    let Ok(parsed) = DateTime::parse_from_rfc3339(iso) else {
        return String::new();
    };
    let then: DateTime<Utc> = parsed.with_timezone(&Utc);
    let diff = Utc::now().signed_duration_since(then);
    let mins = diff.num_minutes();
    let hrs = diff.num_hours();
    let days = diff.num_days();
    if mins < 1 {
        "just now".into()
    } else if mins < 60 {
        format!("{}m ago", mins)
    } else if hrs < 24 {
        format!("{}h ago", hrs)
    } else if days < 7 {
        format!("{}d ago", days)
    } else if days < 30 {
        format!("{}w ago", days / 7)
    } else {
        format!("{}mo ago", days / 30)
    }
}

// Inline styles — keeps the spike self-contained, no extra static
// asset to ship.
const SPIKE_CSS: &str = r#"
:root {
  --bg: #0e1013;
  --fg: #e6e8eb;
  --muted: #9aa3ae;
  --border: #262c34;
  --raised: #14181d;
  --accent: #7aa2f7;
  --decision: #7aa2f7;
  --pattern: #bb9af7;
  --scar: #f7768e;
  --learning: #9ece6a;
}
* { box-sizing: border-box; }
body {
  font: 15px/1.5 ui-sans-serif, system-ui, -apple-system, sans-serif;
  background: var(--bg);
  color: var(--fg);
  margin: 0;
}
main.spike { max-width: 640px; margin: 80px auto; padding: 24px; }
h1 { font-size: 22px; font-weight: 600; margin: 0 0 12px; }
.subtle { color: var(--muted); }
.field { margin-top: 24px; }
.field label {
  display: block; color: var(--muted); font-size: 13px; margin-bottom: 6px;
}
.field input {
  width: 100%; background: var(--raised); border: 1px solid var(--border);
  color: var(--fg); padding: 10px 12px; border-radius: 6px;
  font: inherit; font-size: 15px;
}
.field input:focus { border-color: var(--accent); outline: none; }
.hint-pane { margin-top: 8px; min-height: 1px; }
.hint-rows {
  list-style: none; margin: 0; padding: 0;
  border: 1px solid var(--border); border-radius: 6px;
  background: var(--raised); overflow: hidden;
}
.hint-row {
  display: grid;
  grid-template-columns: 24px 1fr auto;
  align-items: baseline;
  gap: 10px;
  padding: 10px 14px;
  border-bottom: 1px solid var(--border);
}
.hint-row:last-child { border-bottom: none; }
.hint-cat { font-size: 14px; text-align: center; }
.hint-decision .hint-cat { color: var(--decision); }
.hint-pattern  .hint-cat { color: var(--pattern); }
.hint-scar     .hint-cat { color: var(--scar); }
.hint-learning .hint-cat { color: var(--learning); }
.hint-title { color: var(--fg); font-size: 14px; line-height: 1.35; }
.hint-meta { font-size: 12px; color: var(--muted); white-space: nowrap; }
.foot { margin-top: 32px; font-size: 13px; }
"#;
