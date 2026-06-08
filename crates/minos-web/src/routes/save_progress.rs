//! `GET /services/{name}/save-progress` — WebSocket that runs the save flow
//! while streaming `uv pip install` output live, then reports a final status.
//!
//! ## Why pre-install
//!
//! The venv install is normally triggered deep inside
//! [`minos_config::save_config`] → `validate` → the `python_sidecar` filter
//! builder, where its output is not observable. Rather than thread a callback
//! through the core `FilterKind` build path (which the design keeps minimal),
//! this handler exploits the fact that venvs are content-addressed by the
//! hash of their `requirements` and installs are idempotent: it pre-installs
//! the draft's python venvs with [`venv::install_streaming`] (streaming each
//! line to the client), then calls `save_config`, whose own install hits the
//! now-warm cache.
//!
//! The WebSocket connection *is* the save trigger; the client opens it in
//! response to the operator clicking "Save". A plain `POST /services/{name}/save`
//! remains for the no-JS path.

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::response::Response;
use minos_filters::sidecar::venv;
use serde_json::json;

use crate::AppState;

/// Upgrade `GET /services/{name}/save-progress` to a WebSocket.
pub async fn ws(
    State(state): State<AppState>,
    Path(name): Path<String>,
    ws: WebSocketUpgrade,
) -> Response {
    ws.on_upgrade(move |socket| run(socket, state, name))
}

/// Send a JSON text frame; returns `false` if the socket is gone.
async fn send(socket: &mut WebSocket, value: &serde_json::Value) -> bool {
    socket
        .send(Message::Text(value.to_string().into()))
        .await
        .is_ok()
}

async fn fail(socket: &mut WebSocket, message: &str) {
    let _ = send(socket, &json!({ "status": "error", "message": message })).await;
    let _ = socket.send(Message::Close(None)).await;
}

async fn run(mut socket: WebSocket, state: AppState, name: String) {
    let Some(draft) = state.drafts.get(&name) else {
        fail(&mut socket, "no draft to save").await;
        return;
    };

    // Build the target Config: clone the live source and replace this
    // service's entry with the draft.
    let new_cfg = {
        let guard = state.bus.rules.load();
        let mut cfg = guard.source.clone();
        let Some(idx) = cfg.services.iter().position(|s| s.name == name) else {
            drop(guard);
            fail(&mut socket, "service not found").await;
            return;
        };
        cfg.services[idx] = draft.clone();
        cfg
    };

    // Requirements blobs for any python_sidecar filters in the draft.
    let requirements: Vec<String> = draft
        .pipeline
        .iter()
        .filter(|f| f.kind == "python_sidecar")
        .filter_map(|f| {
            f.config
                .get("requirements")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .collect();

    // Stream the install(s) from a blocking task through a channel.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let install = tokio::task::spawn_blocking(move || -> Result<(), String> {
        let root = venv::default_root();
        for req in requirements {
            venv::install_streaming(&root, req.as_bytes(), &mut |line| {
                let _ = tx.send(line.to_owned());
            })
            .map_err(|e| e.to_string())?;
        }
        Ok(())
    });

    while let Some(line) = rx.recv().await {
        if !send(&mut socket, &json!({ "line": line })).await {
            // Client vanished; the blocking install finishes in the
            // background (its sends become no-ops once rx is dropped).
            return;
        }
    }

    match install.await {
        Ok(Ok(())) => {}
        Ok(Err(msg)) => {
            fail(&mut socket, &format!("install failed: {msg}")).await;
            return;
        }
        Err(_) => {
            fail(&mut socket, "install task panicked").await;
            return;
        }
    }

    // Validate + persist + swap. save_config does its own (now-cached) install.
    let save_state = state.clone();
    let saved = tokio::task::spawn_blocking(move || {
        minos_config::save_config(
            save_state.storage.as_ref(),
            &save_state.registry,
            &new_cfg,
            Some("from UI"),
            Some(&save_state.bus),
        )
        .map_err(|e| e.to_string())
    })
    .await;

    match saved {
        Ok(Ok(version)) => {
            state.drafts.clear(&name);
            let _ = send(&mut socket, &json!({ "status": "ok", "version": version })).await;
        }
        Ok(Err(msg)) => {
            fail(&mut socket, &msg).await;
            return;
        }
        Err(_) => {
            fail(&mut socket, "save task panicked").await;
            return;
        }
    }
    let _ = socket.send(Message::Close(None)).await;
}
