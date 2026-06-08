//! `GET /log` — server-rendered readback, plus `GET /log/ws` live feed.

use std::sync::Arc;

use askama::Template;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{RawQuery, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use minos_core::{Direction, LogEntry, LogFilter};
use serde::Serialize;

use crate::{AppState, WebError};

const DEFAULT_LIMIT: usize = 100;

/// Cap on bytes of `sample` rendered as a short preview.
const SAMPLE_PREVIEW_BYTES: usize = 80;

/// Render a packet sample as a short, printable preview: ASCII graphic bytes
/// pass through; everything else becomes `\xNN`. Capped at
/// [`SAMPLE_PREVIEW_BYTES`].
pub(crate) fn sample_short(sample: &[u8]) -> String {
    sample
        .iter()
        .take(SAMPLE_PREVIEW_BYTES)
        .map(|b| {
            if b.is_ascii_graphic() || *b == b' ' {
                char::from(*b).to_string()
            } else {
                format!("\\x{b:02x}")
            }
        })
        .collect()
}

/// JSON payload pushed to live subscribers and rendered client-side. A
/// trimmed view of [`LogEntry`] — notably `sample` is pre-rendered to a
/// printable preview rather than shipped as a raw byte array.
#[derive(Serialize)]
struct LogRowDto {
    ts: i64,
    service: String,
    direction: String,
    kind: String,
    dry_run: bool,
    reason: String,
    sample_short: String,
}

impl From<&LogEntry> for LogRowDto {
    fn from(e: &LogEntry) -> Self {
        Self {
            ts: e.ts,
            service: e.service.clone(),
            direction: format!("{:?}", e.direction),
            kind: e.rule_kind.clone(),
            dry_run: e.dry_run,
            reason: e.reason.clone(),
            sample_short: sample_short(&e.sample),
        }
    }
}

#[derive(Default)]
struct LogFilterEcho {
    service: String,
    kind: String,
    search: String,
    dry_run: String,
}

struct Row {
    ts: i64,
    service: String,
    direction: String,
    kind: String,
    dry_run: bool,
    reason: String,
    sample_short: String,
}

#[derive(Template)]
#[template(path = "log.html")]
struct LogPage {
    entries: Vec<Row>,
    /// Current filter query string, echoed into the WS URL so the live feed
    /// uses the same filters as the rendered snapshot.
    ws_qs: String,
    service_filter: String,
    kind_filter: String,
    search_filter: String,
    dry_run_filter: String,
}

/// `GET /log`.
///
/// # Errors
///
/// Returns [`WebError::Storage`] if log readback fails.
pub async fn get(
    State(state): State<AppState>,
    RawQuery(qs): RawQuery,
) -> Result<Response, WebError> {
    let (filter, echo) = parse_log_query(qs.as_deref());
    let entries = state.storage.query_log(&filter, DEFAULT_LIMIT)?;
    let rows: Vec<Row> = entries
        .into_iter()
        .map(|e| Row {
            ts: e.ts,
            sample_short: sample_short(&e.sample),
            service: e.service,
            direction: format!("{:?}", e.direction),
            kind: e.rule_kind,
            dry_run: e.dry_run,
            reason: e.reason,
        })
        .collect();
    Ok(render(LogPage {
        entries: rows,
        ws_qs: qs.unwrap_or_default(),
        service_filter: echo.service,
        kind_filter: echo.kind,
        search_filter: echo.search,
        dry_run_filter: echo.dry_run,
    }))
}

/// Server-side filter for the live WS feed. Mirrors the `/log` query
/// dimensions; an empty list/`None` field means "no constraint".
#[derive(Clone, Default)]
struct WsFilter {
    services: Vec<String>,
    directions: Vec<Direction>,
    kinds: Vec<String>,
    filter_id: Option<uuid::Uuid>,
    dry_run: Option<bool>,
    search: Option<String>,
}

impl WsFilter {
    fn matches(&self, e: &LogEntry) -> bool {
        if !self.services.is_empty() && !self.services.iter().any(|s| s == &e.service) {
            return false;
        }
        if !self.directions.is_empty() && !self.directions.contains(&e.direction) {
            return false;
        }
        if !self.kinds.is_empty() && !self.kinds.iter().any(|k| k == &e.rule_kind) {
            return false;
        }
        if let Some(fid) = self.filter_id {
            if fid != e.filter_id {
                return false;
            }
        }
        if let Some(want_dry) = self.dry_run {
            if want_dry != e.dry_run {
                return false;
            }
        }
        if let Some(needle) = &self.search {
            let n = needle.to_lowercase();
            let reason = e.reason.to_lowercase();
            let sample = String::from_utf8_lossy(&e.sample).to_lowercase();
            if !reason.contains(&n) && !sample.contains(&n) {
                return false;
            }
        }
        true
    }
}

fn parse_ws_filter(qs: Option<&str>) -> WsFilter {
    let mut f = WsFilter::default();
    let Some(qs) = qs else {
        return f;
    };
    for (k, v) in form_urlencoded::parse(qs.as_bytes()) {
        match k.as_ref() {
            "service" => f.services.push(v.into_owned()),
            "direction" => match v.as_ref() {
                "inbound" => f.directions.push(Direction::Inbound),
                "outbound" => f.directions.push(Direction::Outbound),
                _ => {}
            },
            "kind" => f.kinds.push(v.into_owned()),
            "filter_id" => f.filter_id = uuid::Uuid::parse_str(&v).ok(),
            "dry_run" => match v.as_ref() {
                "true" => f.dry_run = Some(true),
                "false" => f.dry_run = Some(false),
                _ => {}
            },
            "search" if !v.is_empty() => f.search = Some(v.into_owned()),
            _ => {}
        }
    }
    f
}

/// `GET /log/ws` — upgrade to a WebSocket that streams matching log entries
/// as JSON `LogRowDto` frames. Filtering happens server-side using the
/// same query dimensions as `GET /log`.
pub async fn ws_get(
    State(state): State<AppState>,
    RawQuery(qs): RawQuery,
    ws: WebSocketUpgrade,
) -> Response {
    let filter = parse_ws_filter(qs.as_deref());
    let log_broadcast = state.log_broadcast.clone();
    ws.on_upgrade(move |socket| handle_ws(socket, log_broadcast, filter))
}

async fn handle_ws(
    mut socket: WebSocket,
    log_broadcast: Arc<minos_proxy::LogBroadcast>,
    filter: WsFilter,
) {
    let mut sub = log_broadcast.subscribe();
    loop {
        tokio::select! {
            biased;
            // Detect client close / error to end the task promptly.
            msg = socket.recv() => {
                if matches!(msg, None | Some(Err(_) | Ok(Message::Close(_)))) {
                    return;
                }
            }
            entry = sub.recv() => {
                let Ok(entry) = entry else { return }; // sender dropped or lagged
                if !filter.matches(&entry) {
                    continue;
                }
                let dto = LogRowDto::from(&entry);
                let Ok(payload) = serde_json::to_string(&dto) else { continue };
                if socket.send(Message::Text(payload.into())).await.is_err() {
                    return;
                }
            }
        }
    }
}

fn parse_log_query(qs: Option<&str>) -> (LogFilter, LogFilterEcho) {
    let mut f = LogFilter::default();
    let mut echo = LogFilterEcho::default();
    let Some(qs) = qs else {
        return (f, echo);
    };
    for (k, v) in form_urlencoded::parse(qs.as_bytes()) {
        match k.as_ref() {
            "service" => {
                echo.service = v.to_string();
                f.services.push(v.into());
            }
            "direction" => match v.as_ref() {
                "inbound" => f.directions.push(Direction::Inbound),
                "outbound" => f.directions.push(Direction::Outbound),
                _ => {}
            },
            "kind" => {
                echo.kind = v.to_string();
                f.kinds.push(v.into());
            }
            "filter_id" => f.filter_id = uuid::Uuid::parse_str(&v).ok(),
            "dry_run" => match v.as_ref() {
                "true" => {
                    echo.dry_run = "true".into();
                    f.dry_run_only = Some(true);
                }
                "false" => {
                    echo.dry_run = "false".into();
                    f.dry_run_only = Some(false);
                }
                _ => {}
            },
            "search" if !v.is_empty() => {
                echo.search = v.to_string();
                f.search = Some(v.into());
            }
            "since_ms" => f.since_ms = v.parse().ok(),
            "until_ms" => f.until_ms = v.parse().ok(),
            _ => {}
        }
    }
    (f, echo)
}

fn render<T: Template>(t: T) -> Response {
    match t.render() {
        Ok(html) => Html(html).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}
