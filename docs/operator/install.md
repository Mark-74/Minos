# Minos — Install & Deploy

Minos is a single binary (`minos`) that runs both planes: the reverse-proxy /
transparent listeners that inspect traffic, and the web UI you use to manage
rules. This guide covers getting it running. For day-to-day use once it's up,
see [quick-start.md](quick-start.md).

## Quickest path: Docker Compose

```bash
git clone <your-fork-url> minos && cd minos
docker compose up --build
```

The web UI is then on `http://<host>:8080`. The **first** login submits any
password and sets it; subsequent logins must match. State (config history +
sidecar venvs) persists in the `minos-data` volume across restarts.

## Docker (without compose)

```bash
docker build -t minos .
docker run -d --name minos \
  -p 8080:8080 \
  -v minos-data:/data \
  --cap-add NET_ADMIN \
  minos
```

`--cap-add NET_ADMIN` is only needed for transparent mode; it's harmless
otherwise.

## Running the binary directly

```bash
cargo build --release --bin minos
MINOS_WEB_BIND=0.0.0.0:8080 ./target/release/minos
```

The host needs **Python 3** and **`uv`** on `PATH` for `python_sidecar`
filters (the Docker image bundles both). Regex and HTTP filters need neither.

## Configuration (environment variables)

| Variable            | Default                  | Purpose                                            |
|---------------------|--------------------------|----------------------------------------------------|
| `MINOS_DB`          | `minos.db`               | SQLite path: config history + block log.           |
| `MINOS_WEB_BIND`    | `0.0.0.0:8080`           | Web UI listen address.                             |
| `MINOS_CONFIG`      | *(unset)*                | JSON config to seed services from on first run.    |
| `MINOS_VENV_ROOT`   | `/var/lib/minos/venvs`   | Where per-requirements sidecar venvs are built.    |
| `MINOS_SOCKET_DIR`  | `/run/minos/sidecars`    | Unix sockets + staged scripts for sidecars.        |
| `RUST_LOG`          | `info`                   | Log filter (e.g. `minos=debug,info`).              |

Listeners are bound at startup from the active config. **Adding or removing a
service requires a restart**; editing a service's filters/rules hot-reloads
live with no restart and no dropped connections.

## Declaring services

The web UI manages *filters within* services; the **service list itself comes
from a JSON config file** named by `MINOS_CONFIG`. It seeds the database on the
first run only — afterwards the database (and the UI's rollback history) is
authoritative. To change the service list, edit the file and start fresh
(`docker compose down -v`, or delete the db). Copy
[`examples/config.example.json`](../../examples/config.example.json) to start:

```bash
cp examples/config.example.json config.json   # then edit bind/upstream
MINOS_CONFIG=config.json ./target/release/minos
```

With Docker, mount the file in and point `MINOS_CONFIG` at it:

```bash
docker run -d --name minos -p 8080:8080 -p 9000:9000 \
  -v minos-data:/data \
  -v "$PWD/config.json:/config.json:ro" \
  -e MINOS_CONFIG=/config.json \
  minos
```

The full field reference is in the project [README](../../README.md#declaring-services).

## Wiring a service: reverse proxy (default)

In your `MINOS_CONFIG` file, give the service a `reverse` mode with:

- **bind** — where Minos listens, e.g. `0.0.0.0:9000`.
- **upstream** — your real service, e.g. `127.0.0.1:5000`.

```json
{ "kind": "reverse", "bind": "0.0.0.0:9000", "upstream": "127.0.0.1:5000" }
```

Point your players/scoring at the bind port instead of the real service. With
Docker, publish the bind port too (`-p 9000:9000`, or add it under `ports:`
in `docker-compose.yml`).

## Wiring a service: transparent mode

Transparent mode keeps the service on its original port — no client-side
reconfiguration. Set the service's mode to `transparent` in `MINOS_CONFIG`:

```json
{ "kind": "transparent", "bind": "0.0.0.0:15000" }
```

Minos recovers the real destination via `SO_ORIGINAL_DST`, so traffic must be
redirected to its bind port with iptables:

```bash
# Redirect inbound TCP for the protected port (e.g. 5000) to Minos (e.g. 5000
# service still listens internally; Minos binds, say, 127.0.0.1:15000).
iptables -t nat -A PREROUTING -p tcp --dport 5000 -j REDIRECT --to-ports 15000
```

Requirements:

- The container/host needs `NET_ADMIN` (`--cap-add NET_ADMIN`, already in
  `docker-compose.yml`).
- Transparent mode usually wants `network_mode: host` (so the REDIRECT and
  `SO_ORIGINAL_DST` see real addresses) instead of published `ports:`.

Reverse and transparent services can be mixed in one Minos instance.

## Coexisting with pcap tools (Tulip, Caronte)

Minos is inline blocking; Tulip/Caronte are retrospective analysis — they're
complementary. Keep capturing on the public interface as usual. To avoid
disrupting flow grouping (and fingerprinting), set a per-service block
response that mimics the real service rather than an obvious 403.

## Persistence & lifecycle

Minos is event-scoped: run it for the CTF, then stop it. Everything worth
keeping is in `MINOS_DB` (one row per saved config version, plus the block
log) and the venv cache under `MINOS_VENV_ROOT` — both on the `/data` volume
in Docker. There's no log rotation or venv GC to manage for a single event.
