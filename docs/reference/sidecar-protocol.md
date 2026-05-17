# Python Sidecar Wire Protocol

This is the on-the-wire spec between the Rust `SidecarSupervisor` (in
`crates/minos-filters/src/sidecar/supervisor.rs`) and the Python wrapper
script (`crates/minos-filters/src/sidecar/wrapper.py`). It describes the
frame format, the request and response schemas, the Python API the user
sees, and the failure semantics.

## Transport

Per-service Unix domain socket at `<MINOS_SOCKET_DIR>/<service_name>.sock`
(default `MINOS_SOCKET_DIR=/run/minos/sidecars`). The supervisor `listen()`s;
the wrapper `connect()`s.

## Frame format

Each message is a length-prefixed JSON object:

```
+---------------------+----------------------------+
|  4 bytes BE u32 N   |  N bytes UTF-8 JSON body   |
+---------------------+----------------------------+
```

- Length is big-endian unsigned 32-bit. The receiver reads exactly 4 bytes,
  then exactly N more.
- Max frame size: **8 MiB**. The Rust side returns `CodecError::Oversized`
  on a larger length prefix and refuses to allocate.
- Bodies are valid UTF-8 JSON objects.

The Rust types `Request` and `Response` and the framing helpers
`read_request`, `read_response`, `write_request`, `write_response` live in
`crates/minos-filters/src/sidecar/protocol.rs`.

## Request schema (supervisor → wrapper)

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

| Field        | Type            | Notes                                                         |
|--------------|-----------------|---------------------------------------------------------------|
| `id`         | u64             | Monotonic per-supervisor request id; echoed in the response.  |
| `direction`  | string          | `"inbound"` or `"outbound"`.                                  |
| `kind`       | string          | `"raw"` or `"http"`.                                          |
| `bytes_b64`  | string (base64) | Raw bytes. For `kind == "http"` this MAY be empty when the full payload is in `http`. |
| `http`       | object or null  | Present iff `kind == "http"`. Body fields are base64 strings. |

## Response schema (wrapper → supervisor)

The Rust `Response` enum uses the `verdict` field as a discriminator:

Pass:
```json
{ "verdict": "pass", "id": 12345 }
```

Block:
```json
{ "verdict": "block", "id": 12345, "reason": "matched DROP TABLE" }
```

| Field      | Type   | Notes                                                          |
|------------|--------|----------------------------------------------------------------|
| `verdict`  | string | `"pass"` or `"block"`. Discriminator.                          |
| `id`       | u64    | Echoes the request id.                                         |
| `reason`   | string | Block only. Human-readable; shown in the live log.             |

## The Python API the user writes

The user defines one function in their script. The wrapper handles base64
decoding, framing, and IO.

```python
def filter(packet):
    """
    packet: dict with keys
      direction: "inbound" | "outbound"
      kind:      "raw" | "http"
      bytes:     bytes               (already base64-decoded)
      http:      dict (only if kind == "http") with method, path, headers, body

    Returns:
      None or {"verdict": "pass"}                  → Pass
      {"verdict": "block", "reason": "..."}        → Block
      anything else                                → treated as Pass
    """
    if b"DROP TABLE" in packet["bytes"]:
        return {"verdict": "block", "reason": "sqli attempt"}
    return None
```

Any package declared in the per-service `requirements.txt` is available
via normal `import`. The full Python stdlib is available unconditionally.

## Failure modes

| What happens                                   | Wrapper response                                       | Supervisor reaction                |
|------------------------------------------------|--------------------------------------------------------|------------------------------------|
| User script raises an exception                | Logged to stderr; wrapper returns `{verdict: pass}`    | Pass verdict, sidecar keeps running |
| `filter()` returns a non-dict / unknown shape  | Wrapper returns `{verdict: pass}` (fail-open default)  | Pass                               |
| Sidecar process killed externally              | (no response — supervisor times out)                   | `Response::Pass` (fail-open), restart loop deferred to Phase 5 |
| Per-call timeout exceeded                      | Possibly mid-flight, response abandoned                | `Response::Pass` (fail-open by default; or Block if `fail_closed: true`) |
| Script has `SyntaxError`                       | Wrapper fails at startup, prints traceback, exits      | `save_config` rejects validation before any state change |
| Length prefix > 8 MiB                          | n/a                                                    | `CodecError::Oversized` — fail-open Pass |

The **invariant**: no Python failure mode stops traffic flowing. The Python
stage degrades to no-op (fail-open) or to deny-all (fail-closed if
configured); regex and HTTP stages keep running regardless.

## Resource limits (v1)

- **Per-call timeout** — default 10ms, configurable per filter
  (`timeout_ms` field on `PythonSidecarConfig`).
- **`RLIMIT_AS` memory cap** — **deferred to Phase 5**. The current
  supervisor does not set process memory limits. Operators relying on
  hostile-tenant isolation should run Minos in a memory-capped container.
- **Network sandbox** — not enforced. Per design §7.6, the operator wrote
  the script themselves.

## Versioning

The wire format has no version field. v1 is the only shape. If the protocol
changes incompatibly, future versions will add a `protocol: u32` field to
both Request and Response, and the supervisor will negotiate at startup.
For now, the supervisor and wrapper are shipped together in the same Minos
binary — they're always in lockstep.
