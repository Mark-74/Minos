# Minos — Design Spec

**Date:** 2026-05-10
**Status:** Approved (brainstorming complete, awaiting implementation plan)

## 1. Purpose and scope

Minos is a modular firewall for Attack/Defense CTFs, written in Rust. It sits in front of a team's vulnerable services as a reverse proxy, inspecting traffic with a configurable pipeline of filters and either forwarding or blocking each request. A web UI lets the operator edit rules during a round, watch blocked traffic live, and roll back to previous configurations.

**Primary goals:**

- Inline blocking of exploit traffic with sub-millisecond per-filter latency for regex/HTTP stages.
- Modular filter pipeline: new filter types added without touching core code.
- Operator UX optimized for editing rules under stress during a CTF round.
- Coexists with retrospective traffic-analysis tools (Tulip, Caronte) without fighting for packets.
- Dockerized, event-scoped lifecycle (hours per CTF event, not a long-running service).

**Non-goals:**

- General-purpose WAF for production environments.
- Long-term operation, multi-month log retention, or multi-tenant operation.
- Replacing traffic capture/analysis tools.

## 2. High-level architecture

A single Rust binary contains both planes:

```
                 ┌─────────────────── single binary ───────────────────┐
                 │                                                     │
  team VPN  ───► │  ┌──────────── data plane (tokio) ───────────┐      │
                 │  │  listener:80  ─┐                          │      │
                 │  │  listener:443 ─┼─► pipeline executor ─►   │      │
                 │  │  listener:9999 ┘     ↑                    │      │
                 │  │                      │ reads RuleSet      │      │
                 │  └──────────────────────┼────────────────────┘      │
                 │                         │                           │
                 │                  ArcSwap<RuleSet>  ◄── log channel  │
                 │                         ▲                  │        │
                 │  ┌──────────── control plane (axum) ───────┴───┐   │
                 │  │  /services  /rules  /log  /script-editor    │   │
                 │  │  htmx + templates  ◄──►  SQLite  ◄──►       │   │
                 │  └──────────────────────────────────────────────┘   │
                 │                                                     │
                 │       Unix socket ─────► python-sidecar (separate process)
                 └─────────────────────────────────────────────────────┘
```

**Data plane** — tokio listeners per service, filter pipeline executor, sidecar supervisors. Reads `RuleSet` lock-free; writes to a bounded mpsc log channel. Never touches SQLite.

**Control plane** — axum web UI, SQLite config store, version history. On save, builds a new `RuleSet`, atomically swaps it into the `ArcSwap`, writes the version row. Never opens a TCP listener for service traffic.

**Bus** — exactly two seams between planes: `ArcSwap<RuleSet>` and an mpsc `LogEntry` channel.

**Python sidecar** — one separate process per service that uses a Python filter; isolation is per-service so a misbehaving script for service A cannot stall service B.

## 3. Workspace layout

Cargo workspace with crate-level enforced boundaries. Repo root is `Minos/`:

```
Minos/
  Cargo.toml             # workspace manifest
  Dockerfile
  docker-compose.yml
  README.md
  .gitignore
  docs/
    operator/            # end-user docs
    developer/           # extension authors
    reference/           # config + sidecar protocol reference
    design/              # this file
  crates/
    minos-core/          # Filter trait, Verdict, Packet, FilterRegistry contract — zero deps
    minos-storage/       # Storage trait + SQLite impl + in-memory impl; depends on -core
    minos-filters/       # built-in filters (regex, http, python_sidecar); depends on -core
    minos-proxy/         # listeners + pipeline executor; depends on -core
    minos-config/        # config types + versioning + hot-reload bus; depends on -core + -storage
    minos-web/           # axum + templates + htmx; depends on -config + -core
    minos/               # main binary; wires everything
```

Same single binary at deploy. Compiler-enforced modularity: `minos-filters` literally cannot import from `minos-web`.

(Crate names use the `minos-` prefix throughout the spec from this point on; earlier sections that say `firewall-core`, `firewall-filters`, etc. refer to the same crates.)

## 4. Data flow through the filter pipeline

### 4.1 Per-service runtime structure

Each configured service spawns one tokio listener configured by:

- `mode: ProxyMode` — see §4.5; either `Reverse { bind, upstream }` or `Transparent { bind }`.
- `protocol: ProtocolKind` — `Http` or `Tcp`.
- `pipeline: Vec<FilterInstance>` — ordered, runs cheapest-first.
- Per-filter direction toggles (`on_inbound`, `on_outbound`).
- `max_body_bytes` — cap on inspected payload size (default 65536).
- `block_response_override: Option<BlockResponse>` — what to send on block.

### 4.2 Request lifecycle (HTTP)

1. **Accept** — listener accepts a TCP connection.
2. **Read one logical unit** — `ProtocolHandler::read_one(stream)`:
   - HTTP: parse with `httparse`, read body up to `Content-Length` or `max_body_bytes`.
   - TCP: read until N bytes or M ms inactivity (per-service tunable).
3. **Wrap in `Packet`**:
   ```rust
   pub enum Packet<'a> {
       Raw  { bytes: &'a [u8],       direction: Direction },
       Http { req:   &'a ParsedHttp, direction: Direction },
   }
   ```
4. **Run pipeline** — `PipelineExecutor` walks instances in order. For each:
   - Skip if `!enabled`.
   - Skip if direction doesn't match (`on_inbound` / `on_outbound`).
   - Skip if `!filter.accepts(packet)` (e.g. HTTP filter on a Raw packet).
   - Call `filter.inspect(packet) -> Verdict`.
   - On `Block` with `dry_run=true`: emit `LogEntry { kind: dry_run_match }`, continue.
   - On `Block` with `dry_run=false`: emit `LogEntry { kind: block }`, short-circuit.
   - On `Pass`: continue.
5. **Verdict action**:
   - All passed → forward bytes to upstream; proxy response back. If outbound filtering is enabled for any filter, response goes through the same pipeline before being sent to the client.
   - Blocked → for HTTP, send the configured block response (default 403 empty body); for TCP, close connection.

### 4.3 Where modularity lives in the data plane

- **`ProtocolHandler` trait** — `Http`, `Tcp` are the v1 impls. New protocols (TLS termination, etc.) added by writing one impl + a config-side enum variant. The pipeline executor doesn't change.
- **`Filter` trait** (Section 5) — filters declare `accepts(&Packet) -> bool`; mismatched packets are no-ops, so the same pipeline definition applies uniformly across HTTP and raw TCP services.
- **`Verdict`** — enum, future variants like `Modify { bytes }` can be added without breaking existing filters.
- **`PipelineExecutor`** — pure function over `(&Packet, &[FilterInstance])`. Knows nothing about TCP, HTTP, SQLite, or the web UI. Trivially unit-testable.

### 4.4 Pipeline runs per logical request

Pipeline runs on a buffered logical unit (one HTTP request, or one TCP "burst"), not per TCP segment. This is required because exploit payloads are commonly split across segments. Memory cost is bounded by `max_body_bytes` per active connection.

### 4.5 Deployment modes

Each service independently selects a `ProxyMode`. Both modes feed the same `ProtocolHandler` → `PipelineExecutor` → forwarder downstream of `accept`; only the accept path differs.

```rust
pub enum ProxyMode {
    Reverse {
        bind:     SocketAddr,    // Minos listens here
        upstream: SocketAddr,    // forward to here
    },
    Transparent {
        bind:     SocketAddr,    // iptables REDIRECT --to-port target
        // upstream is read per-connection from SO_ORIGINAL_DST
    },
}
```

**Reverse mode** — the default. Minos listens on the public port, the real service is moved to an internal port. Works in any environment, no privileges required. Documented happy path.

**Transparent mode** — Minos listens on a chosen port (e.g. 9999), and the operator sets up iptables on the host:
```
iptables -t nat -A PREROUTING -p tcp --dport 80 -j REDIRECT --to-port 9999
```
The connection arrives on Minos's bind port; Minos calls `getsockopt(SO_ORIGINAL_DST)` to discover the original destination (`80`), then forwards there. The real service keeps its original port binding — no service-side reconfiguration needed. Requires `CAP_NET_ADMIN` in the container and iptables on the host.

Per-service mixing is supported: one Minos instance can run `web-api` in reverse mode and `legacy-binary-svc` in transparent mode side-by-side. Filter pipeline, logging, UI, sidecar supervision, and config storage are mode-agnostic.

**NFQUEUE** is deliberately not supported in v1. Rationale: the two reasons people choose NFQUEUE — keeping the service's port binding, or doing L3/L4 filtering — are respectively covered by transparent mode (which avoids TCP reassembly) and out-of-scope for an application-layer firewall. Adding NFQUEUE would mean writing TCP reassembly in userspace (security-critical, easy to get wrong) for no feature unlock. Revisit only if a concrete use case appears that the other two modes can't serve.

## 5. Filter trait and registry

### 5.1 Trait (in `firewall-core`)

```rust
pub enum Direction { Inbound, Outbound }

pub enum Packet<'a> {
    Raw  { bytes: &'a [u8],       direction: Direction },
    Http { req:   &'a ParsedHttp, direction: Direction },
}

pub enum Verdict {
    Pass,
    Block { reason: String },
}

pub trait Filter: Send + Sync {
    fn kind(&self) -> &'static str;
    fn accepts(&self, p: &Packet) -> bool;
    fn inspect(&self, p: &Packet) -> Verdict;
}
```

The trait is sync. The Python sidecar filter does its blocking IO inside `inspect` via `tokio::task::block_in_place`. This is faster than `async-trait` for this workload and keeps the API trivial.

### 5.2 FilterInstance

```rust
pub struct FilterInstance {
    pub id: Uuid,
    pub display_name: String,
    pub enabled: bool,
    pub dry_run: bool,
    pub on_inbound: bool,
    pub on_outbound: bool,
    pub filter: Arc<dyn Filter>,
}
```

Per-instance toggles live on the instance, not the filter. The executor handles them; filters stay tiny.

### 5.3 Registry — extension model

```rust
pub trait FilterKind: 'static {
    const NAME: &'static str;
    type Config: Serialize + DeserializeOwned + JsonSchema;
    fn build(cfg: Self::Config) -> Result<Arc<dyn Filter>, BuildError>;
}
```

`FilterRegistry` maps `kind name -> deserializer + builder + JSON schema`. The web UI auto-renders an edit form from the JSON schema (via `schemars` + a generic-form template). A filter type that wants a custom editor (e.g. `python_sidecar` with CodeMirror) registers a template override.

### 5.4 Adding a new filter type

A third-party crate `firewall-filter-X` depends only on `firewall-core` + `serde` + `schemars`. It defines a `Filter` impl and a `FilterKind` impl. The main binary calls `registry.register::<XKind>()` in `main.rs`. The UI immediately offers it in the "Add filter" dropdown, generates a form for its config, persists the config in SQLite, and the data plane runs it. **No core code is changed.**

### 5.5 Schema versioning

Out of scope for v1 (event-scoped lifecycle; no need for cross-version migration). If needed later, add a `schema_version` column to the rule rows and a `migrate(old_version, value)` hook on `FilterKind`. Adding it later is purely additive.

## 6. Configuration, persistence, versioning, and hot reload

### 6.0 Storage abstraction

All persistence goes through a `Storage` trait defined in the `minos-storage` crate. The rest of the codebase calls the trait, never SQL directly. v1 ships two impls: a SQLite production backend and an in-memory backend used by tests. Future backends (Postgres, etcd, file-based) are added as new crates implementing the same trait, gated behind Cargo feature flags in the binary — no consumer code changes.

```rust
pub trait Storage: Send + Sync {
    // Versioned config
    fn save_version(&self, blob: &[u8], note: Option<&str>) -> Result<u64, StorageError>;
    fn load_version(&self, version: u64) -> Result<Vec<u8>, StorageError>;
    fn list_versions(&self) -> Result<Vec<VersionMeta>, StorageError>;
    fn active_version(&self) -> Result<u64, StorageError>;
    fn set_active_version(&self, version: u64) -> Result<(), StorageError>;

    // Block log
    fn append_log(&self, entry: &LogEntry) -> Result<(), StorageError>;
    fn query_log(&self, filter: &LogFilter, limit: usize) -> Result<Vec<LogEntry>, StorageError>;

    // Settings (key-value)
    fn get_setting(&self, key: &str) -> Result<Option<String>, StorageError>;
    fn set_setting(&self, key: &str, value: &str) -> Result<(), StorageError>;
}
```

The trait is sync; backends that need async IO (e.g. a future Postgres impl) wrap blocking calls inside `block_in_place`, same pattern used for the Python sidecar in §5.1. This keeps the trait simple and reusable.

**Atomicity contract:** `save_version` is atomic (the row exists or it doesn't) and `set_active_version` is atomic. The save flow in §6.3 relies on these being the only two writes that can affect "what's running"; backends must guarantee both.

The binary picks the backend at startup based on a CLI flag / env var (default: SQLite at `/var/lib/minos/minos.db`). The in-memory backend is used in unit tests and end-to-end tests, never in production.

### 6.1 SQLite schema (default backend)

```sql
CREATE TABLE active_version (
    id      INTEGER PRIMARY KEY CHECK (id = 1),
    version INTEGER NOT NULL
);

CREATE TABLE config_versions (
    version    INTEGER PRIMARY KEY AUTOINCREMENT,
    created_at INTEGER NOT NULL,
    note       TEXT,
    blob       BLOB NOT NULL          -- serialized full Config
);

CREATE TABLE block_log (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    ts        INTEGER NOT NULL,
    service   TEXT NOT NULL,
    direction TEXT NOT NULL,
    filter_id TEXT NOT NULL,
    rule_kind TEXT NOT NULL,
    dry_run   INTEGER NOT NULL,
    reason    TEXT NOT NULL,
    sample    BLOB NOT NULL
);

CREATE TABLE settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
```

### 6.2 Config types

```rust
#[derive(Serialize, Deserialize, Clone)]
pub struct Config {
    pub services: Vec<ServiceConfig>,
    pub default_block_response: BlockResponse,
}

pub struct ServiceConfig {
    pub name: String,
    pub mode: ProxyMode,                   // Reverse { bind, upstream } | Transparent { bind }
    pub protocol: ProtocolKind,
    pub pipeline: Vec<FilterInstanceCfg>,
    pub block_response_override: Option<BlockResponse>,
    pub max_body_bytes: usize,
}

pub struct FilterInstanceCfg {
    pub id: Uuid,
    pub display_name: String,
    pub kind: String,
    pub config: serde_json::Value,
    pub enabled: bool,
    pub dry_run: bool,
    pub on_inbound: bool,
    pub on_outbound: bool,
}
```

`Config` is the on-disk source of truth (one blob per version). `RuleSet` is the in-memory built form with `Arc<dyn Filter>` instances ready to call.

### 6.3 Save flow (validate-then-swap)

1. **Validate** — deserialize new `Config`, build every filter via the registry. Any error rejects the save with a clear UI message; active config unchanged.
2. **Write version row** — `INSERT INTO config_versions`.
3. **Construct RuleSet** — reuse the validated filters.
4. **Atomic swap** — `RULES.store(Arc::new(new_ruleset))`. Lock-free for readers; in-flight inspections finish under the old ruleset.
5. **Update active_version** — `UPDATE active_version SET version = ?`.

If the process is killed between steps 2 and 5, the worst case is an orphaned version row that isn't pointed to by `active_version`. Restart boots into the previous version. No corruption.

### 6.4 Rollback flow

1. Operator clicks "Rollback to v37" in the UI.
2. Control plane loads `config_versions.blob` for v37, deserializes, runs the same build-and-swap flow.
3. `active_version` is updated to 37. (No new version row — history is a stack of intentional saves, not a churn log.)

### 6.5 Sidecar reload on save

When the new RuleSet contains a Python filter whose script *content hash* differs from the running sidecar's hash, the supervisor restarts that sidecar (~50ms). Other services' sidecars are unaffected. If only metadata changed (dry-run flipped, etc.), the existing sidecar is reused.

## 7. Python sidecar

### 7.1 Process model

- One sidecar per service that uses a Python filter.
- Spawned by the data plane on first need; supervised; restarted on crash with backoff (immediate → 1s → 5s → 30s).
- Communication via Unix domain socket at `/run/minos/sidecars/<service>.sock`.

### 7.2 Wire protocol

Length-prefixed JSON: `[4-byte big-endian length][JSON object]`.

**Request (data plane → sidecar):**
```json
{
  "id": 12345,
  "direction": "inbound",
  "kind": "http",
  "bytes_b64": "...",
  "http": {
    "method": "GET",
    "path": "/api/users",
    "headers": [["Host", "x"], ["User-Agent", "y"]],
    "body_b64": "..."
  }
}
```
The `http` block is omitted for raw TCP.

**Response (sidecar → data plane):**
```json
{ "id": 12345, "verdict": "pass" }
```
or
```json
{ "id": 12345, "verdict": "block", "reason": "..." }
```

### 7.3 The Python API the user writes

The sidecar binary (~50 lines, shipped with Minos) wraps a single user-defined function:

```python
def filter(packet):
    """
    packet: dict with keys
      direction: "inbound" | "outbound"
      kind:      "http" | "raw"
      bytes:     bytes
      http:      dict (only if kind == "http") with method, path, headers, body

    Returns:
      None or {"verdict": "pass"}
      or {"verdict": "block", "reason": "..."}
    """
    if b"DROP TABLE" in packet["bytes"]:
        return {"verdict": "block", "reason": "sqli attempt"}
    return None
```

Base64 decoding and JSON encoding handled by the wrapper. The user has full Python stdlib + any packages declared in their `requirements.txt`.

### 7.4 Failure modes

| Failure | Effect |
|---|---|
| `SyntaxError` in saved script | Save rejected at validate-time, no impact on running firewall |
| Missing import | New sidecar fails to start; UI shows banner; old sidecar keeps running |
| Per-packet exception | Packet gets fail-open verdict, exception logged, sidecar stays up |
| Script hangs | Per-call timeout fires (default 10ms, configurable), packet gets fail-open verdict, sidecar killed/restarted |
| Sidecar killed externally | Supervisor restarts with backoff; gap packets get fail-open |
| Disk full → can't spawn | Supervisor logs, packets get fail-open, alert in UI |

**Invariant:** no Python failure mode stops traffic flowing. The Python stage degrades to no-op (fail-open) or to deny-all (fail-closed if opted in); regex and HTTP stages keep running.

### 7.5 Fail-open vs fail-closed

Per-service toggle, surfaced in the UI near the Python filter config:

- **Fail-open (default)** — Python verdict is `pass` when the sidecar is unavailable.
- **Fail-closed** — Python verdict is `block` when the sidecar is unavailable.

### 7.6 Resource limits

- Per-process memory cap via `RLIMIT_AS` so a runaway script can't OOM the host.
- Per-call CPU-time guard via the per-call timeout above.
- **No** network sandboxing in v1 (operator wrote the script themselves).

### 7.7 Dependencies (per-service venv)

- Each service's Python filter has a Requirements editor in the UI (textarea with `requirements.txt` content), prefilled on creation with a default template (minimum: `requests>=2.31`).
- Venvs live at `/var/lib/minos/venvs/<sha256-of-requirements>/`. Identical requirements across services share one venv on disk.
- Built using **`uv`** (preferred) or **`pip`** (fallback) at save time. Install log streamed to UI via WebSocket.
- Install failure rejects the save; active config unchanged.
- Sidecar is launched with `PYTHONPATH` pointing at the venv. Global Python is never touched.
- **No venv GC in v1** — event-scoped lifecycle means the container is destroyed before disk pressure matters.

## 8. Coexistence with traffic-analysis tools

Minos is designed to coexist cleanly with Tulip, Caronte, and tcpdump. The reverse-proxy-per-port deployment means:

1. **Public ports stay public.** Tulip's pcap on the public interface sees full inbound exploits and outbound responses (real upstream replies or our 403). Internal forwarding is over loopback and is invisible to Tulip — exactly what's wanted.
2. **No iptables/NFQUEUE rewrites** that would hide destinations from pcap.
3. **Block decisions are wire-visible.** When we block, the client sees a real response that pcap captures.

**Two affordances:**

- **Configurable block response per service.** Default is HTTP 403 empty body; can be set to mimic the upstream (e.g. "behave like a 404") so attackers can't trivially fingerprint the firewall and so Tulip's flow grouping isn't disrupted.
- **"Create rule from this match" / "Import from flow"** route — an HTTP endpoint that accepts either a logged sample from the live log (intra-Minos use) or a pasted flow snippet (Tulip export use) and creates a draft regex rule. The Tulip → Minos workflow most teams use is "see exploit in Tulip → copy magic bytes → paste into Minos → save in dry-run → flip to enforce." This route shaves clicks; not architecture. **In scope for v1** (small).

**Non-goal:** Minos does not embed Tulip's flow-storage features.

## 9. Web UI

### 9.1 Site map

```
/login                           — password form
/                                — Dashboard: services overview
/services/:name                  — Service detail (pipeline editor)
/services/:name/filters/:id      — Filter instance editor
/log                             — Live blocked-traffic feed
/history                         — Config versions + rollback
/settings                        — Password, defaults
```

All routes except `/login` require an auth cookie (axum middleware redirects to `/login`).

### 9.2 Tech

- **axum** server, **Askama** templates (or **Maud**; pick during implementation).
- **htmx** for partial updates and form submits.
- **WebSocket** (axum native) for the live block log and the `uv pip install` progress stream.
- **CodeMirror** for the in-UI Python script editor (loaded only on the script-editor page).
- **rust-embed** to bundle CSS, htmx, CodeMirror, and templates into the binary. Single-file deployment.

### 9.3 Dashboard (`/`)

Shows services list with per-service status dot (green / yellow / red), block counters refreshed every 5s via `hx-get` + `hx-trigger="every 5s"`, and a "Recent blocks" panel updated by the same WebSocket as the full log via htmx OOB swap.

### 9.4 Service detail (`/services/:name`)

Pipeline editor: vertical ordered list of filter instances. Each row shows the four most-flipped controls (enabled, dry-run, on_inbound, on_outbound), edit/delete/reorder buttons, and (for python) the fail-open/fail-closed radio. Reorder via `↑↓` buttons in v1; drag-and-drop later.

Edits accumulate as draft state until **Save** is clicked, which triggers the validate-then-swap flow. **Discard** reverts to the active version.

### 9.5 Filter editor (`/services/:name/filters/:id`)

Three layouts (one per built-in kind):

- **regex** — pattern input + "test against sample" textarea.
- **http** — header rules table + body regex + method/path filters + "test against sample".
- **python_sidecar** — three tabs: **Script** (CodeMirror), **Requirements** (textarea, prefilled with default template), **Status** (current pid, restart count, last error, fail-open/closed toggle, per-call timeout).

For third-party filter kinds, the form is auto-generated from the JSON schema unless the kind registers a custom template.

### 9.6 Live log (`/log`)

Full-page real-time stream via WebSocket. Each row is one `<tr>` swapped via htmx WS extension.

**Filter dimensions** (server-side, parameterized into the WebSocket subscription):

| Dimension | Type |
|---|---|
| Service | multi-select |
| Direction | multi-select |
| Filter kind | multi-select |
| Specific rule | dropdown |
| Show | radio: blocks+dry-run / blocks only / dry-run only |
| Search | substring against `reason` and `sample` |
| Time | radio: live / last 1m / last 5m / last 30m / all (historical modes query SQLite) |

Filter selections are URL-encoded; bookmarkable / shareable across teammates. Click a row to expand the full sample (hex + ASCII, matched substring highlighted) and a **"Create rule from this match"** button.

### 9.7 History (`/history`)

Lists every config version with timestamp, note, **[diff vs prev]**, and **[↺ rollback]** buttons. Active version is highlighted. Diff view renders a side-by-side JSON diff of the two `Config` blobs.

### 9.8 Settings (`/settings`)

Change shared password (re-prompts current), default block response, log retention size, dashboard refresh interval. Saved to the `settings` table.

### 9.9 Auth

- `/login` shows a single password field. POST verifies against an Argon2 hash in `settings`.
- On success, sets a signed cookie (axum-extra `SignedCookieJar`) with session token + 24h expiry.
- All other routes go through middleware that reads the cookie.
- No CSRF token machinery in v1 (single-user, same-origin, shared password).

## 10. Build and deployment

- **Dockerfile** (multi-stage):
  - Builder: `cargo build --release` against a Rust image.
  - Runtime: slim image (debian-slim or distroless — pick during implementation based on `uv` + Python availability) with the binary, `uv`, `python3.12`, and required system libs (`libssl`, `libffi`).
- **docker-compose.yml** as the documented deployment, with example bindings, mounted config volume, and shared Docker network with upstream services. Documents two service-level patterns: reverse-proxy mode (no special privileges) and transparent mode (`cap_add: [NET_ADMIN]` plus iptables setup on the host or via an init container).
- **No "install on host" path documented.** Bare-metal install works (it's a Rust binary) but isn't the happy path.

## 11. Testing strategy

- **`firewall-core`** — unit tests; property tests for `PipelineExecutor` invariants.
- **`firewall-filters`** — per-filter unit tests with fixture corpora; integration tests for the Python sidecar (real subprocess against a fixture script).
- **`firewall-config`** — round-trip tests with in-memory SQLite; full save/rollback flow tested end-to-end.
- **`firewall-proxy`** — integration tests with a real localhost echo upstream.
- **`firewall-web`** — handler tests using `axum::TestServer`; template rendering against fixture data.
- **End-to-end smoke** — boot the full binary in-process against a fixture upstream, drive a request through, assert blocked and passed cases produce the right log entries.

**Not in v1 CI:** load testing, fuzzing the HTTP parser (rely on `httparse`'s existing fuzz corpus), property tests beyond the executor.

## 12. Documentation deliverables

Required as part of v1, organized under `docs/`:

1. **Operator guide** — install via Docker, first-run setup, configuring services, writing each filter type, reading the live log, version history and rollback, troubleshooting.
2. **Developer / extension guide** — `Filter` and `FilterKind` traits with examples, writing a third-party filter crate, registry mechanics, adding a new ProtocolHandler.
3. **Python sidecar protocol spec** — wire format, request/response schema, the user-facing `filter(packet)` API, dependency management, fail-open vs fail-closed semantics.
4. **Configuration reference** — SQLite schema, every config field, defaults.
5. **Architecture overview** — two-plane split, ArcSwap, sidecar supervision.
6. **Rustdoc on every public item in `firewall-core`** — examples on every public type, trait, method.
7. **README.md** at the repo root pointing to all of the above + quick-start.

Code examples in docs must compile / run.

## 13. v1 scope summary

### In scope

- Single Rust binary, workspace crates as in §3.
- Two deployment modes per service: **reverse proxy** and **transparent** (iptables REDIRECT, via `SO_ORIGINAL_DST`). Per-service config; both modes can run side-by-side in one Minos instance.
- HTTP and raw TCP listeners per service.
- Three filter kinds: `regex`, `http`, `python_sidecar`.
- Per-filter `enabled` / `dry_run` / `on_inbound` / `on_outbound` toggles.
- `FilterRegistry` with auto-form rendering from JSON Schema.
- Python sidecar: per-service process, JSON-over-Unix-socket, supervised with backoff, per-call timeout, fail-open default + per-service fail-closed toggle, in-UI script editor + Requirements editor with default template, `uv`-backed per-service venv (hash-deduped).
- **`Storage` trait abstraction** with two shipped impls: SQLite (production) and in-memory (tests). Future backends added as separate feature-gated crates with no consumer changes.
- Versioned config with atomic validate-then-swap reload via `ArcSwap<RuleSet>`; one-click rollback.
- Web UI: dashboard, service detail, filter editor, live log with multi-dimension filtering, history with diff + rollback, settings.
- Auth: shared password (Argon2), signed-cookie session, bind `0.0.0.0`.
- Block log persisted via Storage trait; streamed to UI via WebSocket.
- Tulip-friendly defaults (configurable block response per service); "create rule from match / import from flow" route.
- Dockerfile + docker-compose.yml documenting both reverse and transparent deployment patterns.
- Full documentation per §12.

### Explicitly NOT in scope

- Master kill switch (rollback covers it).
- Multi-user accounts / per-user audit log.
- `SCHEMA_VERSION` / migrations on filter configs.
- Venv garbage collection.
- Source-IP capture in log entries.
- Saved log-filter presets.
- Drag-and-drop pipeline reorder.
- Network/seccomp sandboxing of the Python sidecar.
- Load tests / benchmark gates in CI.
- A second shipped storage backend (Postgres, etcd, etc.) — trait is in v1, additional backends added when concrete need arises.
- NFQUEUE deployment mode (rationale documented in §4.5 — transparent mode covers the use cases that NFQUEUE would, without TCP reassembly).
- Two-binary IPC architecture.

### Open questions deferred to implementation

- Exact Docker base image (debian-slim vs distroless) — pick based on `uv` + `python3.12` availability and image size.
- Templating crate: **Askama** vs **Maud** — both are mature; pick whichever feels nicer once the first few templates are written. No design impact either way.
