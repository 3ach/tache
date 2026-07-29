//! Webhook-driven sync server, plus the one-time OAuth activation flow.
//!
//! Todoist delivers events to POST /todoist-hook, authenticated with an
//! HMAC-SHA256 signature over the raw body (X-Todoist-Hmac-SHA256,
//! base64, keyed by the app's client secret). Events are coalesced for a
//! short window, then one reconcile pass runs. A periodic timer
//! (TACHE_SYNC_MINUTES, default 15, 0 disables) kicks the same loop as a
//! safety net for dropped deliveries; `tache sync` reconciles by hand.
//!
//! Todoist only activates webhooks for a user after they complete the
//! OAuth flow for the app, so the server also hosts it: GET /oauth/start
//! redirects to the consent page, and /oauth/callback (the app's redirect
//! URL) exchanges the code, persists the token, and swaps it in live.

use anyhow::{Context, Result};
use axum::{
    Router,
    body::Bytes,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect},
    routing::{get, post},
};
use base64::Engine;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;

use crate::dag::Dag;
use crate::graph;
use crate::sync;
use crate::todoist::Client;

const DEBOUNCE: Duration = Duration::from_secs(2);

pub struct Oauth {
    pub client_id: String,
    pub client_secret: String,
    pub token_file: PathBuf,
}

struct AppState {
    client: Arc<Client>,
    oauth: Oauth,
    /// Shared secret for the read-only /graph view; None disables it.
    graph_key: Option<String>,
    /// CSRF state for the in-flight OAuth attempt, if any.
    pending_state: Mutex<Option<String>>,
    kick: mpsc::Sender<()>,
}

pub async fn serve(
    bind: &str,
    client: Arc<Client>,
    oauth: Oauth,
    graph_key: Option<String>,
    sync_minutes: u64,
) -> Result<()> {
    if oauth.client_secret.is_empty() {
        tracing::warn!(
            "TODOIST_CLIENT_SECRET is empty — all webhooks will be rejected until it is set"
        );
    }
    if !client.has_token() {
        tracing::warn!("no API token yet — visit /oauth/start to authorize");
    }

    let (kick, mut kicked) = mpsc::channel::<()>(16);

    // Single worker owns the reconcile loop; coalesces bursts of events
    // (Todoist sends one per item change) into one pass.
    let worker_client = client.clone();
    tokio::spawn(async move {
        while kicked.recv().await.is_some() {
            tokio::time::sleep(DEBOUNCE).await;
            while kicked.try_recv().is_ok() {}
            match sync::reconcile(&worker_client).await {
                Ok(r) => tracing::info!(
                    scoped = r.scoped,
                    projects = r.projects,
                    next = r.next,
                    blocked = r.blocked,
                    relabeled = r.relabeled,
                    stripped = r.stripped,
                    "reconciled"
                ),
                Err(e) => tracing::error!(error = %e, "reconcile failed"),
            }
        }
    });

    // Safety net for dropped webhook deliveries: kick the reconcile loop
    // on an interval.
    if sync_minutes == 0 {
        tracing::info!("TACHE_SYNC_MINUTES=0 — periodic reconcile disabled");
    } else {
        let timer_kick = kick.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(sync_minutes * 60));
            interval.tick().await; // consume the immediate first tick
            loop {
                interval.tick().await;
                let _ = timer_kick.try_send(());
            }
        });
        tracing::info!("periodic reconcile every {sync_minutes}m");
    }

    if graph_key.is_none() {
        tracing::info!("TACHE_GRAPH_KEY is not set — /graph is disabled");
    }

    let state = Arc::new(AppState {
        client,
        oauth,
        graph_key,
        pending_state: Mutex::new(None),
        kick,
    });
    let app = Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/graph", get(graph_view))
        .route("/todoist-hook", post(hook))
        .route("/oauth/start", get(oauth_start))
        .route("/oauth/callback", get(oauth_callback))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .with_context(|| format!("binding {bind}"))?;
    tracing::info!(%bind, "listening");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn hook(State(state): State<Arc<AppState>>, headers: HeaderMap, body: Bytes) -> StatusCode {
    let signature = headers
        .get("x-todoist-hmac-sha256")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    if !verify(&state.oauth.client_secret, &body, signature) {
        tracing::warn!("rejected webhook with bad signature");
        return StatusCode::FORBIDDEN;
    }
    let _ = state.kick.try_send(());
    StatusCode::OK
}

async fn graph_view(
    State(state): State<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>,
) -> axum::response::Response {
    let Some(key) = &state.graph_key else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "TACHE_GRAPH_KEY is not set — graph view disabled",
        )
            .into_response();
    };
    if params.get("key") != Some(key) {
        return (StatusCode::FORBIDDEN, "missing or bad ?key=").into_response();
    }
    match render_graph(&state.client).await {
        Ok(html) => axum::response::Html(html).into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, format!("todoist fetch failed: {e}")).into_response(),
    }
}

async fn render_graph(client: &Client) -> Result<String> {
    let projects = client.active_projects().await?;
    let enabled = sync::enabled_ids(&projects);
    let tasks: Vec<_> = client
        .active_tasks()
        .await?
        .into_iter()
        .filter(|t| enabled.contains(&t.project_id))
        .collect();
    let dag = Dag::build(&tasks);
    let classes = dag.classify(&tasks);
    let scoped: Vec<_> = projects
        .into_iter()
        .filter(|p| enabled.contains(&p.id))
        .collect();
    Ok(graph::page(&scoped, &tasks, &dag, &classes))
}

async fn oauth_start(State(state): State<Arc<AppState>>) -> axum::response::Response {
    if state.oauth.client_id.is_empty() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "TODOIST_CLIENT_ID is not set",
        )
            .into_response();
    }
    let mut buf = [0u8; 16];
    if getrandom::getrandom(&mut buf).is_err() {
        return (StatusCode::INTERNAL_SERVER_ERROR, "no entropy").into_response();
    }
    let csrf = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf);
    *state.pending_state.lock().unwrap() = Some(csrf.clone());
    Redirect::temporary(&format!(
        "https://todoist.com/oauth/authorize?client_id={}&scope=data:read_write&state={csrf}",
        state.oauth.client_id
    ))
    .into_response()
}

async fn oauth_callback(
    State(state): State<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>,
) -> axum::response::Response {
    let (Some(code), Some(csrf)) = (params.get("code"), params.get("state")) else {
        return (StatusCode::BAD_REQUEST, "missing code or state").into_response();
    };
    if state.pending_state.lock().unwrap().take().as_deref() != Some(csrf.as_str()) {
        return (StatusCode::FORBIDDEN, "state mismatch — retry /oauth/start").into_response();
    }
    let token = match state
        .client
        .exchange_code(&state.oauth.client_id, &state.oauth.client_secret, code)
        .await
    {
        Ok(t) => t,
        Err(e) => {
            tracing::error!(error = %e, "oauth exchange failed");
            return (StatusCode::BAD_GATEWAY, "token exchange failed").into_response();
        }
    };
    if let Err(e) = std::fs::write(&state.oauth.token_file, &token) {
        tracing::error!(error = %e, file = %state.oauth.token_file.display(), "persisting token failed");
        return (StatusCode::INTERNAL_SERVER_ERROR, "could not persist token").into_response();
    }
    state.client.set_token(token);
    let _ = state.kick.try_send(());
    tracing::info!("oauth complete — token stored, webhooks active");
    "tache: authorized. webhooks are active and a first sync is running.".into_response()
}

fn verify(secret: &str, body: &[u8], signature_b64: &str) -> bool {
    if secret.is_empty() {
        return false;
    }
    let Ok(signature) = base64::engine::general_purpose::STANDARD.decode(signature_b64) else {
        return false;
    };
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac accepts any key");
    mac.update(body);
    mac.verify_slice(&signature).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifies_todoist_signature() {
        let secret = "shhh";
        let body = br#"{"event_name":"item:completed"}"#;
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        let sig = base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());
        assert!(verify(secret, body, &sig));
        assert!(!verify(secret, body, "AAAA"));
        assert!(!verify("wrong", body, &sig));
        assert!(!verify("", body, &sig));
    }
}
