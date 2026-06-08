# Minos Web UI — Developer Guide

This document is for developers extending or modifying the operator web
UI. For the operator-facing walkthrough, see `docs/operator/quick-start.md`.

## Crate layout

```
crates/minos-web/
  Cargo.toml
  src/lib.rs                       — `run`, `AppState`, `WebError` re-exports
  src/state.rs                     — `AppState` struct + `FromRef<AppState> for Key`
  src/error.rs                     — `WebError` + `IntoResponse`
  src/auth.rs                      — Argon2id hash/verify, `require_auth` middleware
  src/draft.rs                     — `DraftStore` (in-memory pipeline drafts)
  src/assets.rs                    — `rust-embed` handler for `/assets/*`
  src/routes/
    mod.rs                         — `router(AppState) -> Router`
    login.rs                       — `/login`, `/logout`
    dashboard.rs                   — `/`
    services.rs                    — `/services/{name}` + edit actions
    filters.rs                     — `/services/{name}/filters/...`
    history.rs                     — `/history`, `/history/diff`, rollback
    log.rs                         — `/log`, `/log/ws`
    save_progress.rs               — `/services/{name}/save-progress` (WS)
    settings.rs                    — `/settings`, password change, defaults
  src/auto_form.rs                 — JSON-schema → HTML form walker
  src/assets/
    style.css                      — UI stylesheet (embedded)
    htmx.min.js                    — vendored htmx 2.0.x
    codemirror/                    — vendored CodeMirror 5 (python editor)
    log_ws.js, dashboard_blocks.js, save_progress.js — WebSocket glue
  templates/                       — Askama templates
  tests/                           — integration tests (`oneshot`; WS tests use a real listener)
```

## State

Every handler extracts `State<AppState>`. `AppState` is `Clone`; every
field is reference-counted:

- `bus: minos_config::Bus` — `Arc<ArcSwap<RuleSet>>` + `mpsc::Sender<LogEntry>`. The web crate writes via `Bus::swap` (indirectly through `save_config`), never directly.
- `storage: Arc<dyn Storage>` — versioned config + settings + block log.
- `registry: Arc<FilterRegistry>` — used by `save_config` and the
  "kinds" dropdown on the new-filter page.
- `drafts: Arc<DraftStore>` — per-service in-memory drafts.
- `cookie_key: axum_extra::extract::cookie::Key` — signs session cookies.

`AppState` implements `FromRef<AppState> for Key` so axum's
`SignedCookieJar` extractor can pull the key out automatically.

## Auth model

Single-tenant. One shared password (Argon2id hash in
`storage.get_setting("password_hash")`). On first-run (no hash), the first
`/login` POST stores the submitted password and accepts. The session is a
signed cookie (`minos_session=ok`); the signature provides authenticity.

The `require_auth` middleware reads `SignedCookieJar`, checks the cookie
value, and 303-redirects to `/login` if absent or invalid. Apply it to
the protected sub-router via `route_layer`:

```rust
let protected = Router::new()
    .route("/", get(dashboard::get))
    // ... more routes ...
    .route_layer(from_fn_with_state(state.clone(), require_auth));
```

**Gotcha:** `route_layer` panics on an empty `Router` in axum 0.8 — add
at least one route before applying the layer.

## Templating

Askama 0.14, templates in `crates/minos-web/templates/`. `askama_axum`
0.4 is **incompatible** with Askama 0.14 (it pulls in the old 0.12). Each
handler renders manually:

```rust
fn render<T: Template>(t: T) -> Response {
    match t.render() {
        Ok(html) => Html(html).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}
```

(Returning `Response` rather than `Result<Response, WebError>` keeps
clippy's `unnecessary_wraps` happy when the function never errors.)

Templates use the standard `{% extends "base.html" %}` / `{% block ... %}`
inheritance. `_partial_filter_row.html` is included from
`service_detail.html`; Askama 0.14 includes inherit the parent's context,
so `name` and the loop variable `r` are visible inside the partial.

## Drafts

`DraftStore` is process-local in-memory state. Single-user model means
one draft per service across the whole process. Edits (toggle, reorder,
delete, kind-specific edits) load-or-create the draft, mutate it, and
write it back via `DraftStore::put`. Save consumes the draft, runs
`save_config`, and clears it.

Helper used by every mutator:

```rust
fn draft_for(state: &AppState, name: &str) -> Result<ServiceConfig, WebError> {
    if let Some(d) = state.drafts.get(name) { return Ok(d); }
    let guard = state.bus.rules.load();
    let svc = guard.services().iter().find(|s| s.name == name)
        .ok_or_else(|| WebError::NotFound(format!("service {name}")))?
        .clone();
    Ok(svc)
}
```

Restart loses all drafts. That's intentional — events are short, drafts
are scratch space.

## Adding a route

1. Add a handler module under `src/routes/` and declare it with
   `pub mod foo;` in `routes/mod.rs`.
2. Wire the route into either the `public` chain (no auth required) or
   the `protected` chain (auth required) in `routes/mod.rs::router`.
3. If it renders a template, create the `.html` under
   `crates/minos-web/templates/` and add a struct with
   `#[derive(Template)] #[template(path = "your.html")]`.
4. Add an integration test under `crates/minos-web/tests/` using
   `tower::ServiceExt::oneshot` and the existing `login_cookie` helper
   pattern (duplicate it per test file — refactoring across test files
   is over-reach).

## Adding a per-kind filter editor

Two paths:

1. **Built-in kind:** add a branch to the `match kind.as_str()` in both
   `filters::edit_get` and `filters::edit_post`. Add a per-kind template
   (`filter_edit_<kind>.html`). The plain-textarea generic fallback
   covers any kind you don't write a custom editor for.
2. **Third-party kind:** ship the `FilterKind` implementation in your
   own crate; register it with the binary's `FilterRegistry`. The web UI
   renders a **schema-driven auto-form** from the kind's
   `schemars`-derived `Config` schema (see `auto_form.rs`): booleans →
   checkboxes, integers/numbers → number inputs, strings → text inputs,
   nested objects → fieldsets, arrays → a JSON snippet textarea. Fields
   are named with dot-paths under `config` and coerced back to a typed
   JSON config on POST. Kinds with no registered schema fall back to the
   raw JSON textarea.

## Assets

`rust-embed` bundles all of `src/assets/` at compile time. The
`/assets/{*path}` route is **public** — the login page needs CSS too.

Vendored assets:

- `htmx.min.js` — htmx 2.0.x.
- `codemirror/` — CodeMirror 5.65.x (`codemirror.js`, `codemirror.css`,
  `python.js`); loaded only on the `python_sidecar` editor page. See
  `codemirror/README.md` for the refresh command and license.
- `log_ws.js`, `dashboard_blocks.js`, `save_progress.js` — small
  hand-written WebSocket glue (no framework). Each builds DOM with
  `textContent` rather than `innerHTML` because log reason/sample bytes
  are attacker-influenced.

To refresh a vendored library, re-download it from the URL noted in its
provenance comment/README.

## Testing patterns

All integration tests use `tower::ServiceExt::oneshot` instead of a real
HTTP listener. The pattern:

```rust
fn state() -> AppState {
    let (bus, _rx) = new_bus(RuleSet::empty_for(Config::default()));
    let storage: Arc<dyn Storage> = Arc::new(InMemoryStorage::new());
    let registry = Arc::new(FilterRegistry::new());
    let (broadcast_tx, _sub) = tokio::sync::broadcast::channel(16);
    AppState::new(bus, storage, registry, Key::generate(), Arc::new(broadcast_tx))
}

async fn login_cookie(app: &Router) -> String {
    // POST /login with any password — first-run accepts it.
    let res = app.clone().oneshot(/* POST /login */).await.unwrap();
    res.headers().get(SET_COOKIE).unwrap().to_str().unwrap()
        .split(';').next().unwrap().to_string()
}
```

To exercise post-conditions (draft state, bus state, storage state),
capture `AppState` before passing it to `router(state.clone())`, then
inspect the original after.

**WebSocket tests are the exception.** A long-lived upgrade can't go
through `oneshot`, so `tests/ws_log.rs` and `tests/save_progress.rs`
bind `axum::serve` on `127.0.0.1:0`, then drive it with a
`tokio-tungstenite` client (dev-dep). Login still happens via `oneshot`
on a clone of the same router — the signed session cookie validates
across both because they share one `AppState` (one cookie key).

## Phase 4b features (shipped)

The live log fan-out lives in the **log-writer task**: `spawn_log_writer`
takes a `tokio::sync::broadcast::Sender<LogEntry>` (aliased
`minos_proxy::LogBroadcast`) and broadcasts each entry after it is
persisted. `AppState` holds an `Arc<LogBroadcast>`; WebSocket handlers
`.subscribe()` to it. The data plane is unchanged — it still writes the
same mpsc the log writer drains.

- **WebSocket live log** — `GET /log/ws` subscribes to the broadcast and
  pushes already-filtered entries as JSON `LogRowDto` (sample bytes are
  pre-rendered to a printable preview, not shipped raw). Filtering is
  server-side, mirroring the `/log` query dimensions. `log_ws.js`
  prepends rows on the `/log` page; `dashboard_blocks.js` feeds the
  dashboard's recent-blocks panel from the same endpoint.
- **CodeMirror script editor** — loaded only on the `python_sidecar`
  editor via `CodeMirror.fromTextArea`, which keeps the underlying
  `<textarea>` in sync on submit.
- **uv install progress streaming** — `GET /services/{name}/save-progress`
  is a WebSocket that *is* the save trigger. It pre-installs the draft's
  python venvs with `venv::install_streaming` (streaming each line),
  exploiting the content-addressed, idempotent venv cache so the
  subsequent `save_config` hits a warm cache. This keeps the core
  `FilterKind` build path callback-free. A plain `POST .../save` remains
  for the no-JS path.
- **History diff view** — `GET /history/diff?from=A&to=B` renders both
  config blobs as pretty-printed JSON side by side.
- **"Create rule from this match"** — a log row's `+ rule` link hits
  `GET /services/{name}/filters/new?prefill_kind=regex&prefill_pattern=…`;
  a single submit seeds a dry-run regex rule and returns to the service.
- **Schema-driven auto-form** for third-party `FilterKind::Config` types
  (see `auto_form.rs`).
- **Recent-blocks panel on the dashboard** (live, WebSocket).

## Phase 5 outlook

Not yet built: the `minos` binary that wires `Bus` + `Storage` +
`FilterRegistry` + the web server + proxy listeners + the
log-writer-with-broadcast, plus the `Dockerfile`/`docker-compose.yml`
and operator install docs.
