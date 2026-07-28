//! Webhook-driven sync server.
//!
//! Todoist delivers events to POST /todoist-hook, authenticated with an
//! HMAC-SHA256 signature over the raw body (X-Todoist-Hmac-SHA256,
//! base64, keyed by the app's client secret). Events are coalesced for a
//! short window, then one reconcile pass runs. There is no polling: if
//! Todoist drops a delivery, `tache sync` reconciles by hand.

use anyhow::{Context, Result};
use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use base64::Engine;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

use crate::sync;
use crate::todoist::Client;

const DEBOUNCE: Duration = Duration::from_secs(2);

struct AppState {
    client_secret: String,
    kick: mpsc::Sender<()>,
}

pub async fn serve(bind: &str, client: Client, client_secret: String) -> Result<()> {
    if client_secret.is_empty() {
        tracing::warn!(
            "TODOIST_CLIENT_SECRET is empty — all webhooks will be rejected until it is set"
        );
    }
    let (kick, mut kicked) = mpsc::channel::<()>(16);

    // Single worker owns the Todoist client; coalesces bursts of events
    // (Todoist sends one per item change) into one reconcile pass.
    tokio::spawn(async move {
        while kicked.recv().await.is_some() {
            tokio::time::sleep(DEBOUNCE).await;
            while kicked.try_recv().is_ok() {}
            match sync::reconcile(&client).await {
                Ok(r) => tracing::info!(
                    total = r.total,
                    next = r.next,
                    blocked = r.blocked,
                    relabeled = r.relabeled,
                    "reconciled"
                ),
                Err(e) => tracing::error!(error = %e, "reconcile failed"),
            }
        }
    });

    let state = Arc::new(AppState {
        client_secret,
        kick,
    });
    let app = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/todoist-hook", post(hook))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .with_context(|| format!("binding {bind}"))?;
    tracing::info!(%bind, "listening");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn hook(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let signature = headers
        .get("x-todoist-hmac-sha256")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    if !verify(&state.client_secret, &body, signature) {
        tracing::warn!("rejected webhook with bad signature");
        return StatusCode::FORBIDDEN;
    }
    let _ = state.kick.try_send(());
    StatusCode::OK
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
    }
}
