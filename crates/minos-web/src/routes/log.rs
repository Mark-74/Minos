//! `GET /log` — server-rendered readback.

use askama::Template;
use axum::extract::{RawQuery, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use minos_core::{Direction, LogFilter};

use crate::{AppState, WebError};

const DEFAULT_LIMIT: usize = 100;

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
        .map(|e| {
            let sample_short: String = e
                .sample
                .iter()
                .take(80)
                .map(|b| {
                    if b.is_ascii_graphic() || *b == b' ' {
                        char::from(*b).to_string()
                    } else {
                        let hex = format!("\\x{b:02x}");
                        hex
                    }
                })
                .collect();
            Row {
                ts: e.ts,
                service: e.service,
                direction: format!("{:?}", e.direction),
                kind: e.rule_kind,
                dry_run: e.dry_run,
                reason: e.reason,
                sample_short,
            }
        })
        .collect();
    Ok(render(LogPage {
        entries: rows,
        service_filter: echo.service,
        kind_filter: echo.kind,
        search_filter: echo.search,
        dry_run_filter: echo.dry_run,
    }))
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
