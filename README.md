# Minos

**A modular firewall for Attack/Defense CTFs.**

Minos sits in front of your vulnerable services and inspects traffic inline. A
web UI lets your team edit blocking rules, watch blocked traffic live, and roll
back to a known-good config in one click — all without restarting or dropping
connections.

It ships three filter types out of the box:

- **Regex** — raw-byte pattern matching on the wire.
- **HTTP** — match on method, path, body, and headers.
- **Python sidecar** — your own `filter(packet)` script, with its own pip
  dependencies, isolated per service.

Minos is *inline blocking* and complements retrospective tools like Tulip and
Caronte — keep capturing pcaps as usual.

---

## Table of contents

- [Quick start (Docker)](#quick-start-docker)
- [Other ways to run it](#other-ways-to-run-it)
- [Your first five minutes](#your-first-five-minutes)
- [Configuration](#configuration)
- [Declaring services](#declaring-services)
- [How traffic flows through Minos](#how-traffic-flows-through-minos)
- [Filter types](#filter-types)
- [Building from source](#building-from-source)
- [Troubleshooting](#troubleshooting)
- [Project layout](#project-layout)
- [Deeper documentation](#deeper-documentation)

---

## Quick start (Docker)

The fastest path. You need Docker with Compose.

```bash
git clone https://github.com/Mark-74/Minos.git minos
cd minos
docker compose up --build
```

Then open **http://localhost:8080** in a browser. The first password you type
on the login page becomes the shared team password.

Minos starts with an empty config so you can explore the UI immediately. To
actually defend something, declare your services in a config file and restart —
see [Declaring services](#declaring-services). State (your config history and
the block log) persists in a Docker volume across restarts.

To stop it:

```bash
docker compose down          # keep your data
docker compose down -v       # also wipe the volume (fresh start)
```

---

## Other ways to run it

### Docker without Compose

```bash
docker build -t minos .
docker run -d --name minos -p 8080:8080 -v minos-data:/data --cap-add NET_ADMIN minos
```

`--cap-add NET_ADMIN` is only needed for transparent mode (see below); it's
harmless otherwise.

### The binary directly

You need Rust (stable) to build, plus **Python 3** and **[uv](https://docs.astral.sh/uv/)**
on `PATH` *only* if you use Python-sidecar filters. Regex and HTTP filters
need neither.

```bash
cargo build --release --bin minos
./target/release/minos
```

By default it opens the UI on `0.0.0.0:8080` and stores everything in
`minos.db` in the current directory. Override with environment variables — see
[Configuration](#configuration).

> Full deployment details, including **transparent mode** (keep the service on
> its original port via iptables) and reverse-proxy wiring, live in
> [docs/operator/install.md](docs/operator/install.md).

---

## Your first five minutes

Once the UI is up at http://localhost:8080:

1. **Log in.** Type any password on `/login` — the first one sets the shared
   password. Everyone on the team uses it after that. (Change it later under
   **Settings → Change password**.)

2. **Declare your services.** Services are defined in a small JSON config file
   that Minos seeds from on first start (the web UI manages *filters within*
   services, not the service list itself). Copy the example and edit it:

   ```bash
   cp examples/config.example.json config.json
   # edit config.json: set each service's bind + upstream
   ```

   Each service has a **bind** (where Minos listens, e.g. `0.0.0.0:9000`), an
   **upstream** (your real service, e.g. `127.0.0.1:5000`), and a **protocol**
   (`http` or `tcp`). Point players/checkers at the bind port. Then start
   Minos with `MINOS_CONFIG` pointing at the file:

   ```bash
   MINOS_CONFIG=config.json ./minos
   # or with Docker, mount it and set MINOS_CONFIG (see install.md)
   ```

   See [Declaring services](#declaring-services) for the full file format.

   > Listeners bind when Minos starts, so **adding or removing a service needs
   > an edit to the file + a restart with a fresh database**. Editing a
   > service's filters and rules afterwards is fully live in the UI — no
   > restart, no dropped connections.

3. **Add a filter.** Open the service, click **+ Add filter**, pick a kind
   (regex / http / python_sidecar), and fill it in. New filters start in
   **dry-run**: they log what they *would* block without actually blocking, so
   you can test safely.

4. **Save.** Edits accumulate in a draft. Hit **Save** to validate and make
   them live. If a Python filter has new requirements, you'll watch the
   `uv pip install` output stream in live.

5. **Watch the log.** Open **`/log`**. Blocked (and dry-run "would-block")
   events stream in live over a WebSocket — no refresh needed. Filter by
   service, kind, dry-run, or text. Each row has a **+ rule** link that
   scaffolds a new regex rule straight from that match.

6. **Go live.** Happy with a dry-run filter? Open it, turn **dry-run** off,
   and Save. It now blocks for real.

7. **Made a mistake?** Open **`/history`**, find a known-good version, and hit
   **Rollback** — one click back to safety. **Diff vs active** shows you
   exactly what changed first.

---

## Configuration

All configuration is via environment variables. Defaults are sensible; nothing
is required.

| Variable           | Default                | Purpose                                         |
|--------------------|------------------------|-------------------------------------------------|
| `MINOS_DB`         | `minos.db`             | SQLite file: config history + block log.        |
| `MINOS_WEB_BIND`   | `0.0.0.0:8080`         | Web UI listen address.                          |
| `MINOS_CONFIG`     | *(unset)*              | JSON config to seed services from on first run. |
| `MINOS_VENV_ROOT`  | `/var/lib/minos/venvs` | Where per-requirements sidecar venvs are built. |
| `MINOS_SOCKET_DIR` | `/run/minos/sidecars`  | Unix sockets + staged scripts for sidecars.     |
| `RUST_LOG`         | `info`                 | Log verbosity, e.g. `minos=debug,info`.         |

Example:

```bash
MINOS_DB=/data/minos.db MINOS_WEB_BIND=0.0.0.0:80 MINOS_CONFIG=config.json RUST_LOG=info ./minos
```

Press **Ctrl-C** for a clean shutdown.

---

## Declaring services

Services live in a JSON file referenced by `MINOS_CONFIG`. It seeds the
database **on first run only** (when no config exists yet); afterwards the
database — and everything you do in the UI, including rollback history — is
authoritative. To change the service list later, edit the file and start with
a fresh database (`docker compose down -v`, or delete `minos.db`). This suits
Minos's event-scoped lifecycle: declare services once at setup, then manage
rules live in the UI.

A minimal config (see [`examples/config.example.json`](examples/config.example.json)):

```json
{
  "services": [
    {
      "name": "web",
      "mode": { "kind": "reverse", "bind": "0.0.0.0:9000", "upstream": "127.0.0.1:5000" },
      "protocol": "http",
      "max_body_bytes": 65536,
      "pipeline": []
    }
  ]
}
```

Field reference:

| Field            | Meaning                                                                          |
|------------------|----------------------------------------------------------------------------------|
| `name`           | Unique service name (shown in the UI and log).                                    |
| `mode`           | `{"kind":"reverse","bind":…,"upstream":…}` or `{"kind":"transparent","bind":…}`.  |
| `protocol`       | `"http"` or `"tcp"`.                                                              |
| `max_body_bytes` | Cap on inspected payload size per request.                                        |
| `pipeline`       | Filters — usually start empty `[]` and add them in the UI.                        |

You *can* pre-declare filters in `pipeline` too (each needs `id`,
`display_name`, `kind`, `config`, `enabled`, `dry_run`, `on_inbound`,
`on_outbound`), but it's easier to add them through the UI after first start.
The shipped example includes one dry-run regex filter to show the shape.

---

## How traffic flows through Minos

```
            ┌─────────────────────────── Minos ───────────────────────────┐
attacker ─▶ │  listener ─▶ filter pipeline ─▶ (Pass) ─▶ upstream service   │ ─▶ your service
            │                       │                                       │
            │                    (Block)                                    │
            │                       └─▶ block response back to client       │
            │                       └─▶ block log ─▶ live UI feed           │
            └───────────────────────────────────────────────────────────────┘
                         ▲                                   ▲
                         │ hot-reload (atomic, lock-free)    │ web UI (rules, log, history)
                         └──────────── operator ─────────────┘
```

Rule edits swap the live ruleset atomically — in-flight inspections finish
under the old rules, the next ones use the new rules. No mid-round latency
spikes.

---

## Filter types

| Kind             | Matches on                                   | Needs Python/uv |
|------------------|----------------------------------------------|-----------------|
| `regex`          | Raw bytes (and HTTP method+path+body)        | No              |
| `http`           | HTTP method, path regex, body regex, headers | No              |
| `python_sidecar` | Anything — your `filter(packet)` script      | Yes             |

Each filter instance has independent toggles: **enabled**, **dry-run**,
**inspect inbound**, **inspect outbound**. Reorder them with the up/down
buttons; the pipeline runs top to bottom and the first Block wins.

Writing a Python sidecar? The packet schema and protocol are documented in
[docs/reference/sidecar-protocol.md](docs/reference/sidecar-protocol.md).

---

## Building from source

```bash
# Build everything
cargo build --release --workspace

# Run the test suite
cargo test --workspace

# The binary lands at:
./target/release/minos
```

SQLite is statically bundled, so there's no system library to install for a
build.

---

## Troubleshooting

**The dashboard is empty after first start.**
That's expected — Minos bootstraps an empty config. Add your first service.

**My new service isn't intercepting traffic.**
Listeners bind at startup. Restart Minos after adding or removing a service.
(Editing filters on an existing service is live and needs no restart.)

**A filter isn't blocking.**
New filters default to **dry-run** (log only). Open it, turn dry-run off, and
Save. Check `/log` — dry-run matches are labeled `dry`.

**Saving a Python filter fails.**
The `uv pip install` output streams into the save panel; the error is usually
there (a bad requirement or a script `SyntaxError`). The save is rejected
*before* anything goes live, so your running config is never left half-updated.
Make sure Python 3 and `uv` are on `PATH` (the Docker image includes both).

**I locked myself out / forgot the password.**
The password hash lives in the database. For an event-scoped tool the simplest
reset is a fresh database (`docker compose down -v`, or delete `minos.db`),
then set a new password on next login.

**Transparent mode isn't seeing real destinations.**
It needs `NET_ADMIN` and usually `network_mode: host`. See the transparent-mode
section in [docs/operator/install.md](docs/operator/install.md).

---

## Project layout

Minos is a Rust workspace; the compiler enforces the modular boundaries.

```
crates/
  minos-core      Filter trait, core types, registry — zero deps
  minos-storage   Storage trait + SQLite and in-memory backends
  minos-config    Config, RuleSet, the hot-reload bus, validate/save
  minos-proxy     Listeners, protocol handlers, pipeline executor, log writer
  minos-filters   regex, http, python_sidecar + the uv-backed venv machinery
  minos-web       axum + Askama web UI (auth, dashboard, editors, live log)
  minos           The binary that wires it all together
```

---

## Deeper documentation

- **Operators**
  - [Install & deploy](docs/operator/install.md) — Docker, the binary,
    transparent mode, env reference.
  - [Quick-start walkthrough](docs/operator/quick-start.md) — first-run UI tour.
- **Developers / extension authors**
  - [Data plane](docs/developer/data-plane.md)
  - [Writing filters](docs/developer/filters.md)
  - [Web UI internals](docs/developer/web-ui.md)
  - [Sidecar protocol](docs/reference/sidecar-protocol.md)
- **Design**
  - [Architecture & decisions](docs/design/2026-05-10-minos-firewall-design.md)

---

## License

MIT.
