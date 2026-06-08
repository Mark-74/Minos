# Minos Operator Quick-Start

This guide walks through the first-run experience: starting Minos, logging
in, adding a service, saving a filter, and watching the block log. It
assumes the Minos binary is running and the web UI is reachable at the
configured address (default `http://127.0.0.1:8080`).

To get Minos *running* in the first place (Docker, the binary, env vars,
reverse-proxy and transparent wiring), see [install.md](install.md).

## 1. First-run login

Open the UI in a browser. You'll land on `/login`. The first POST to
`/login` with any non-empty password **becomes the shared password** —
there's no separate setup flow. Pick a strong password; the team shares it.

The hint on the login page tells you when you're in first-run mode (no
hash stored yet). After the first successful login, subsequent logins
verify against the stored Argon2id hash.

If you ever need to change the password, do it from **Settings → Change
password** while logged in.

## 2. Dashboard

After login you land on `/`. The dashboard lists every configured service
(`name`, mode, bind, upstream, protocol, filter count, block count over
the last 5 minutes). Click a service name to open its detail page.

If you've just deployed Minos for the first time, this page will be empty
— Minos doesn't ship with services pre-configured. The binary bootstraps
an empty config on first run; add your first service here (see
[install.md](install.md) for how the bind/upstream map to your network).
Listeners bind at startup, so **adding or removing a service needs a
restart**; filter and rule edits hot-reload live.

## 3. Service detail

`/services/{name}` shows the per-service pipeline. The filter list shows:

- **Order** — pipeline runs top-to-bottom; cheapest filters first.
- **Toggles** — `on`, `dry-run`, `in`, `out` badges show the four
  per-instance flags.
- **↑ / ↓** — reorder buttons.
- **Edit** — open the per-kind filter editor.
- **Delete** — remove the filter (from the draft only — confirm with
  Save).

### Edits accumulate as a draft

Any change — toggling a flag, reordering, deleting, or saving an edit
from the filter editor — accumulates into an **in-memory draft**. The
banner at the top of the page shows when a draft exists. The live ruleset
is unchanged until you click **Save**.

**Save** runs the validate-then-swap flow (design §6.3):

1. Every filter rebuilds from its JSON config (regex must compile, Python
   script must `py_compile`, etc.).
2. A new version row is written to SQLite.
3. The new `RuleSet` is atomically swapped into the bus. The data plane
   picks it up on the next accepted connection.

If validation fails (bad regex, missing field), the save is rejected and
the active config is unchanged. The error appears at the top of the page.

**Discard** drops the draft.

## 4. Adding a filter

Click **+ Add filter** on a service detail page. Pick the kind from the
dropdown (`regex`, `http`, `python_sidecar`, or any third-party kind
registered at startup) and give it a display name. Save — you'll land on
the filter's editor with the form pre-populated.

Fill in the config. Per-kind editors:

- **regex** — single `pattern` field. Uses the `regex` crate's bytes
  engine. Matched against raw bytes for TCP packets and
  `method + " " + path + body` for HTTP packets.
- **http** — `methods`, `path_regex`, `body_regex`, and a `headers`
  textarea (one `Name: regex` pair per line). All non-empty conditions
  AND together.
- **python_sidecar** — Script editor (CodeMirror, Python syntax
  highlighting), Requirements textarea, per-call timeout in ms, and a
  fail-open/fail-closed select. The script must define a
  `filter(packet)` function — see `docs/reference/sidecar-protocol.md`
  for the packet schema. When you save a service whose Python filter has
  new requirements, the `uv pip install` output streams live into a
  panel so you can watch (or diagnose) the install.
- Anything else — a form generated automatically from the filter kind's
  config schema (checkboxes, number/text inputs, nested fieldsets). A
  kind with no registered schema falls back to a JSON textarea.

After editing, save the filter. The change is in your draft; flip back to
the service detail page and **Save** to make it live.

## 5. The block log

`/log` shows the last 100 block events with a filter form at the top.
Filter by service, kind, dry-run-only, real-only, or substring search;
the URL updates so you can share it with teammates.

New matches stream in live over a WebSocket — no refresh needed — and
respect whatever filters are in the URL. Each row has a **+ rule** link
that jumps to the new-filter form pre-seeded with that match as a regex
pattern (in dry-run), so you can turn an observed attack into a rule in
one click. The dashboard's **Recent blocks** panel uses the same live
feed.

## 6. History and rollback

`/history` lists every saved config version with the version number,
timestamp (Unix epoch seconds), and the note (`"from UI"`, `"rollback to
v3"`, etc.). The active version is highlighted.

Click **Rollback** on any version to re-save its blob as a new version
and make it active. History is a stack of intentional saves — rollback
doesn't truncate, it appends. To undo a rollback, rollback to the
version you started from.

**Diff vs active** on any non-active row opens a side-by-side
pretty-printed JSON comparison of that version against the active one.

## 7. Settings

`/settings` covers:

- **Change password** — current / new / confirm-new. The signed-cookie
  session continues to work after the change.
- **Default block response** — HTTP status, body, and extra headers.
  This is what Minos sends to a blocked client when the matched
  service doesn't override it. Default is `HTTP/1.1 403` empty body.

## 8. Logout

Click **Logout** in the header. The signed cookie is dropped; you're sent
back to `/login`.

## What's deferred to Phase 5

The operator UI is feature-complete after Phase 4b. What remains is
packaging:

- The `minos` binary that wires the data plane (proxy listeners) and the
  control plane (this web UI) together and starts everything.
- A `Dockerfile` and `docker-compose.yml` for one-command deployment.
- Operator install docs: Docker, transparent-mode iptables example, and
  reverse-proxy upstream wiring.

The data plane itself is fully feature-complete after Phases 1–3, so a
Minos instance with a saved config of regex / http / python_sidecar
filters blocks traffic in real time independently of the UI.
