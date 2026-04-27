//! `/login` HTML page.
//!
//! Branches on three signals from the server:
//!   1. `access_method` — localhost or remote
//!   2. `has_credentials` — whether any passkeys are registered
//!   3. `?setup_token=…` query param — a paste-from-token pair flow
//!
//! Single SPA-ish page that talks to `/api/auth/*` from JS.

use crate::serve::access::AccessMethod;
use crate::serve::state::AppState;
use axum::{
    extract::{ConnectInfo, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use serde::Deserialize;
use std::net::SocketAddr;

pub fn routes() -> Router<AppState> {
    Router::new().route("/login", get(login_get))
}

#[derive(Deserialize)]
struct LoginQuery {
    #[serde(default)]
    setup_token: Option<String>,
}

async fn login_get(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(q): Query<LoginQuery>,
) -> Response {
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok());
    let access = AccessMethod::classify(peer, host);
    let snap = state.auth_store.snapshot().await;
    let has_credentials = !snap.credentials.is_empty();
    let setup_token = q.setup_token.filter(|t| !t.is_empty());

    let html = render_page(access, has_credentials, setup_token.as_deref());

    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        html,
    )
        .into_response()
}

fn render_page(access: AccessMethod, has_credentials: bool, setup_token: Option<&str>) -> String {
    let intent = pick_intent(access, has_credentials, setup_token);
    let body = render_body(&intent);
    let setup_token_json = setup_token
        .map(|t| serde_json::to_string(t).unwrap_or_else(|_| "null".into()))
        .unwrap_or_else(|| "null".into());

    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Sign in · sipag</title>
<style>
  body {{ font: 15px/1.5 ui-sans-serif, system-ui, -apple-system, sans-serif;
         background: #0e1013; color: #e6e8eb;
         max-width: 460px; margin: 80px auto; padding: 24px; }}
  h1 {{ font-size: 18px; font-weight: 600; margin: 0 0 12px; }}
  p  {{ color: #9aa3ae; margin: 0 0 16px; }}
  pre {{ background: #14181d; padding: 12px; border-radius: 6px;
        font-family: ui-monospace, monospace; font-size: 13px;
        overflow-x: auto; margin: 0 0 16px; }}
  code {{ color: #7aa2f7; }}
  button {{ background: #7aa2f7; color: #0e1013; border: none;
           padding: 10px 18px; border-radius: 6px; font: inherit;
           font-size: 14px; cursor: pointer; }}
  button:hover {{ filter: brightness(1.1); }}
  button:disabled {{ opacity: 0.5; cursor: progress; }}
  .err {{ color: #f7768e; margin-top: 12px;
         font-family: ui-monospace, monospace; font-size: 13px;
         word-break: break-word; }}
</style>
</head>
<body>
{body}
<script>
  const SETUP_TOKEN = {setup_token_json};
  const errEl = document.getElementById('err');
  const btn = document.getElementById('action');

  function b64urlToBytes(s) {{
    s = s.replace(/-/g, '+').replace(/_/g, '/');
    while (s.length % 4) s += '=';
    const bin = atob(s);
    const bytes = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
    return bytes;
  }}
  function bytesToB64url(buf) {{
    const bytes = new Uint8Array(buf);
    let s = '';
    for (const b of bytes) s += String.fromCharCode(b);
    return btoa(s).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
  }}

  async function jpost(url, body) {{
    const r = await fetch(url, {{
      method: 'POST',
      headers: body ? {{ 'Content-Type': 'application/json' }} : {{}},
      body: body ? JSON.stringify(body) : undefined,
    }});
    if (!r.ok) {{
      const t = await r.text();
      throw new Error(url + ': HTTP ' + r.status + ' ' + t);
    }}
    return r.status === 204 ? null : r.json();
  }}

  function attestation(cred) {{
    return {{
      id: cred.id,
      rawId: bytesToB64url(cred.rawId),
      type: cred.type,
      response: {{
        attestationObject: bytesToB64url(cred.response.attestationObject),
        clientDataJSON: bytesToB64url(cred.response.clientDataJSON),
      }},
      extensions: cred.getClientExtensionResults ? cred.getClientExtensionResults() : {{}},
    }};
  }}

  function assertion(cred) {{
    return {{
      id: cred.id,
      rawId: bytesToB64url(cred.rawId),
      type: cred.type,
      response: {{
        authenticatorData: bytesToB64url(cred.response.authenticatorData),
        clientDataJSON: bytesToB64url(cred.response.clientDataJSON),
        signature: bytesToB64url(cred.response.signature),
        userHandle: cred.response.userHandle ? bytesToB64url(cred.response.userHandle) : null,
      }},
      extensions: cred.getClientExtensionResults ? cred.getClientExtensionResults() : {{}},
    }};
  }}

  function buildPublicKeyOptions(challenge, isAuth) {{
    const opts = challenge.publicKey;
    opts.challenge = b64urlToBytes(opts.challenge);
    if (isAuth) {{
      if (Array.isArray(opts.allowCredentials)) {{
        for (const c of opts.allowCredentials) c.id = b64urlToBytes(c.id);
      }}
    }} else {{
      opts.user.id = b64urlToBytes(opts.user.id);
      if (Array.isArray(opts.excludeCredentials)) {{
        for (const c of opts.excludeCredentials) c.id = b64urlToBytes(c.id);
      }}
    }}
    return opts;
  }}

  async function doRegister() {{
    const begin = await jpost('/api/auth/register/start');
    const opts = buildPublicKeyOptions(begin.options, false);
    const cred = await navigator.credentials.create({{ publicKey: opts }});
    await jpost('/api/auth/register/finish', {{
      challenge_id: begin.challenge_id,
      response: attestation(cred),
    }});
  }}

  async function doLogin() {{
    const begin = await jpost('/api/auth/login/start');
    const opts = buildPublicKeyOptions(begin.options, true);
    const cred = await navigator.credentials.get({{ publicKey: opts }});
    await jpost('/api/auth/login/finish', {{
      challenge_id: begin.challenge_id,
      response: assertion(cred),
    }});
  }}

  async function doPair() {{
    const begin = await jpost('/api/auth/pair/start', {{ setup_token: SETUP_TOKEN }});
    const opts = buildPublicKeyOptions(begin.options, false);
    const cred = await navigator.credentials.create({{ publicKey: opts }});
    await jpost('/api/auth/pair/finish', {{
      challenge_id: begin.challenge_id,
      setup_token_id: begin.setup_token_id,
      response: attestation(cred),
    }});
  }}

  if (btn) {{
    btn.addEventListener('click', async () => {{
      btn.disabled = true;
      errEl.textContent = '';
      try {{
        const action = btn.dataset.action;
        if (action === 'register') await doRegister();
        else if (action === 'login') await doLogin();
        else if (action === 'pair') await doPair();
        window.location = '/';
      }} catch (e) {{
        btn.disabled = false;
        errEl.textContent = String(e && e.message || e);
      }}
    }});
  }}
</script>
</body>
</html>"#,
        body = body,
        setup_token_json = setup_token_json,
    )
}

enum Intent<'a> {
    Register,
    Login,
    Pair(&'a str),
    Bootstrap,
}

fn pick_intent<'a>(
    access: AccessMethod,
    has_credentials: bool,
    setup_token: Option<&'a str>,
) -> Intent<'a> {
    if let Some(tok) = setup_token {
        return Intent::Pair(tok);
    }
    if has_credentials {
        return Intent::Login;
    }
    if matches!(access, AccessMethod::Localhost) {
        Intent::Register
    } else {
        Intent::Bootstrap
    }
}

fn render_body(intent: &Intent<'_>) -> String {
    match intent {
        Intent::Register => r#"
<h1>Register first device</h1>
<p>This sipag has no passkeys yet. Register one on this device — you're on localhost, so you have authority.</p>
<button id="action" data-action="register">Register passkey</button>
<div class="err" id="err"></div>
"#
        .to_string(),

        Intent::Login => r#"
<h1>Sign in</h1>
<p>Use the passkey enrolled with this sipag.</p>
<button id="action" data-action="login">Sign in with passkey</button>
<div class="err" id="err"></div>
"#
        .to_string(),

        Intent::Pair(_token) => r#"
<h1>Pair this device</h1>
<p>Add a passkey to this sipag using the setup token from the URL.</p>
<button id="action" data-action="pair">Pair passkey</button>
<div class="err" id="err"></div>
"#
        .to_string(),

        Intent::Bootstrap => r#"
<h1>No passkeys yet</h1>
<p>This sipag isn't bootstrapped. On the host running sipag, open it on localhost to register the first passkey, or paste a setup token URL minted from another device.</p>
<div class="err" id="err"></div>
"#
        .to_string(),
    }
}
