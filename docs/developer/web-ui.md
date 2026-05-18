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
    history.rs                     — `/history`, rollback
    log.rs                         — `/log`
    settings.rs                    — `/settings`, password change, defaults
  src/assets/
    style.css                      — UI stylesheet (embedded)
    htmx.min.js                    — vendored htmx 2.x (placeholder in 4a)
  templates/                       — Askama templates
  tests/                           — integration tests using `tower::ServiceExt::oneshot`
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
   falls back to the generic JSON-textarea editor automatically. A
   schema-driven auto-form is **Phase 4b**.

## Assets

`rust-embed` bundles CSS and htmx at compile time. The `/assets/{*path}`
route is **public** — the login page needs CSS too.

**htmx vendoring:** Phase 4a ships a placeholder. To install real htmx:

```bash
curl -L https://unpkg.com/htmx.org@2/dist/htmx.min.js \
  -o crates/minos-web/src/assets/htmx.min.js
```

This is documented inside the placeholder file itself.

## Testing patterns

All integration tests use `tower::ServiceExt::oneshot` instead of a real
HTTP listener. The pattern:

```rust
fn state() -> AppState {
    let (bus, _rx) = new_bus(RuleSet::empty_for(Config::default()));
    let storage: Arc<dyn Storage> = Arc::new(InMemoryStorage::new());
    let registry = Arc::new(FilterRegistry::new());
    AppState::new(bus, storage, registry, Key::generate())
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

## Phase 4b features (deferred)

- **WebSocket live log** — server-pushed log entries. Subscriber reads
  the same mpsc receiver the log-writer task drains.
- **CodeMirror script editor** — replaces the plain textarea on the
  python_sidecar editor.
- **uv install progress streaming** — `save_config` integrates with a
  WebSocket that pipes `uv pip install` stdout/stderr live.
- **History diff view** — side-by-side JSON diff of two versions.
- **"Create rule from this match"** — one-click navigation from a log
  entry to a pre-populated new-filter form.
- **Schema-driven auto-form** for arbitrary `FilterKind::Config` types
  (currently: generic JSON textarea).
- **Recent-blocks panel on the dashboard** (live, WebSocket).
