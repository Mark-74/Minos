# Data-plane internals

## What the data plane is

The data plane lives in the `minos-proxy` crate. Its job is to accept TCP
connections on behalf of each defended service, inspect every inbound (and
optionally outbound) packet against the live filter pipeline, and either
forward the traffic to the upstream service or send the configured block
response back to the attacker.

`minos-proxy` does not own any persistent state. Configuration and reload
mechanics come from `minos-config`; the log of blocked traffic flows into
a `minos-storage::Storage` implementation owned by the control plane. The
crate is pure data-plane logic: IO, filtering, and channel handoff.

---

## The two seams between planes

The control plane and the data plane share exactly two concurrency
primitives, both defined in `minos_config::Bus`
(`crates/minos-config/src/bus.rs`):

### 1. `ArcSwap<RuleSet>` — live config

```
Bus::rules: Arc<ArcSwap<RuleSet>>
```

`ArcSwap` is a lock-free atomic pointer. The data plane calls
`bus.rules.load()` once per accepted connection to obtain a guard that
holds a strong reference to the current `RuleSet`. Reads never block and
never contend with each other.

When an operator saves a new config, the control plane calls `Bus::swap`,
which stores a new `Arc<RuleSet>` into the `ArcSwap`. In-flight guards
continue to hold the old value; the next `load()` call anywhere in the
process returns the new one. There is no window where a connection sees a
partially updated ruleset.

The concrete helper `handler::pipeline_snapshot` (in
`crates/minos-proxy/src/handler.rs`) loads the guard, locates the
pipeline for the requested service by name, clones the
`Vec<FilterInstance>`, and releases the guard — so the guard is held for
only a few microseconds per connection.

### 2. Unbounded mpsc channel — log entries

```
Bus::log: LogSink   (= tokio::sync::mpsc::UnboundedSender<LogEntry>)
```

Every time the pipeline executor emits a `Block` verdict — dry-run or real
— it constructs a `LogEntry` and sends it on `bus.log`. The send cannot
block: the channel is unbounded, and send failures (receiver dropped during
shutdown) are intentionally ignored.

The receiver half is not part of `Bus`. It is returned by `new_bus` and
passed to `spawn_log_writer` (`crates/minos-proxy/src/log_writer.rs`),
which drains the channel in a dedicated Tokio task and appends each entry
to the `Storage` impl via `spawn_blocking`. The task exits cleanly when
the last `Bus` clone (and therefore the last `LogSink`) is dropped.

---

## Adding a new `ProtocolHandler`

`ProtocolHandler` is an async trait defined in
`crates/minos-proxy/src/handler.rs`:

```rust
#[async_trait]
pub trait ProtocolHandler: Send + Sync {
    async fn handle(
        &self,
        client: TcpStream,
        ctx: &ServiceContext,
        bus: &Bus,
    ) -> Result<(), ProxyError>;
}
```

A handler owns exactly one accepted connection from accept to close.
Per-connection state (the pipeline snapshot, the upstream socket) should
be local variables inside `handle`, not fields on the implementor. Fields
on the implementor are for handler-level configuration that is the same
across all connections (for example, `TcpHandler` stores its idle timeout
there).

### Step-by-step

1. **Create `crates/minos-proxy/src/your_handler.rs`** and implement
   `ProtocolHandler`.

2. **Read the packet.** Parse or read whatever the protocol delivers from
   `client`. Wrap it as a `minos_core::Packet` (either `Packet::Raw` or
   `Packet::Http`) with the appropriate `Direction`.

3. **Snapshot the pipeline.** Call `handler::pipeline_snapshot(&bus,
   &ctx.service_name)`. If the service is unknown (returns `None`) either
   skip filtering and forward, or return an error — your choice.

4. **Run the executor.**

   ```rust
   let verdict = crate::execute(&packet, &ctx.service_name, &pipeline, &bus.log);
   ```

   `execute` is pure (no IO) and returns a `Verdict` immediately. A
   dry-run block produces a `LogEntry` and returns `Verdict::Pass`; a
   real block produces a `LogEntry` and returns `Verdict::Block`.

5. **Dispatch on verdict.**

   ```rust
   match verdict {
       Verdict::Pass => forward_to_upstream(/* ... */).await?,
       Verdict::Block { .. } => write_block_response(&ctx.block_response, /* ... */).await?,
   }
   ```

6. **Add a `ProtocolKind` variant** in `minos-core`
   (`crates/minos-core/src/`) if the new handler introduces a genuinely
   new wire protocol that operators choose at config time. If the handler
   is a specialisation of HTTP or raw TCP (TLS termination, gRPC, etc.),
   reuse the existing `ProtocolKind::Http` or `ProtocolKind::Tcp` variant
   and dispatch on a richer service-config field instead.

7. **Wire the dispatch** in `listen_service`
   (`crates/minos-proxy/src/listener.rs`). Add a match arm:

   ```rust
   let handler: Arc<dyn ProtocolHandler> = match cfg.protocol {
       ProtocolKind::Http => Arc::new(HttpHandler),
       ProtocolKind::Tcp  => Arc::new(TcpHandler::new(DEFAULT_TCP_IDLE_MS)),
       ProtocolKind::Your => Arc::new(YourHandler::new(/* cfg */)),
   };
   ```

   Everything downstream — the accept loop, per-connection task spawning,
   error logging — is already handled by `listen_service` and does not
   need to change.

---

## Dry-run vs real semantics

Both behaviours are implemented in `execute`
(`crates/minos-proxy/src/executor.rs`):

- **Dry-run (`instance.dry_run == true`):** When a filter returns
  `Verdict::Block`, `execute` constructs a `LogEntry` (with
  `dry_run: true`) and sends it on the log channel, then **continues**
  walking the remaining pipeline and returns `Verdict::Pass` at the end.
  The attacker's traffic flows through; the operator sees the match in the
  live log.

- **Real block (`instance.dry_run == false`):** Same `LogEntry` is
  emitted (with `dry_run: false`), but `execute` **short-circuits**,
  returning `Verdict::Block` immediately. The handler writes the
  configured block response and closes the connection.

Filters that are disabled (`instance.enabled == false`) or that do not
apply to the packet's direction (`instance.applies_to(direction) == false`)
are skipped entirely with no log entry.

---

## Transparent mode caveats

`ProxyMode::Transparent` lets Minos intercept traffic without changing the
service's listen port. An iptables `REDIRECT` rule directs packets to the
Minos bind address; Minos then reads the original destination via the
`SO_ORIGINAL_DST` socket option and opens a new connection to the real
upstream.

### Requirements

- **Linux only.** `SO_ORIGINAL_DST` is a Linux kernel extension. On
  non-Linux platforms `original_dst` (`crates/minos-proxy/src/transparent.rs`)
  returns `ProxyError::TransparentLookup` unconditionally at the first
  accepted connection; the listener logs the error and continues (but every
  connection will fail).

- **`CAP_NET_ADMIN` capability.** The process (or the Docker container)
  must hold `CAP_NET_ADMIN` so that iptables rules can be added at deploy
  time. The rules themselves must be installed out-of-band (see the
  operator guide).

- **Per-connection lookup.** `resolve_upstream` in `listener.rs` calls
  `original_dst` once per accepted connection, immediately after `accept`.
  If the lookup fails (e.g., the connection arrived without a REDIRECT
  rule), the connection is dropped with a `tracing::warn` log line; the
  accept loop continues.

### iptables setup sketch

```
# Redirect port 8080 to the Minos listener on 9090:
iptables -t nat -A PREROUTING -p tcp --dport 8080 -j REDIRECT --to-port 9090
```

Minos resolves the original `:8080` destination and opens a new connection
to it, so the upstream service does not need to move or reconfigure.

---

## Testing patterns

Integration tests live in `crates/minos-proxy/tests/`. Each file is a
separate test binary compiled with `[[test]]` in `Cargo.toml`.

### Fixture upstream + `pick_free_port`

The standard pattern (used in `listener.rs` unit tests and the integration
suite) is:

```rust
fn pick_free_port() -> SocketAddr {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    drop(l);
    addr
}
```

Bind a listener on port 0, record the OS-assigned address, drop the
listener, then hand the address to the service under test. The window
between drop and re-bind is negligible in a loopback test.

### Concurrent handler + upstream

Because handlers and the upstream echo server must run concurrently,
tests that exercise a full request/response round-trip annotate the test
with `flavor = "multi_thread"`:

```rust
#[tokio::test(flavor = "multi_thread")]
async fn listen_service_serves_one_request_end_to_end() {
    // Spawn a fixture upstream on a free port.
    let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_addr = upstream.local_addr().unwrap();
    tokio::spawn(async move { /* accept + echo */ });

    // Pick a bind address and start the service under test.
    let bind = pick_free_port();
    let _handle = listen_service(cfg, bus);

    // Give the listener a moment to bind before connecting.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Connect, write a request, read the response, assert.
    let mut client = tokio::net::TcpStream::connect(bind).await.unwrap();
    /* ... */
}
```

The `multi_thread` flavor is required because a single-threaded runtime
would stall waiting for the upstream to accept while the accept loop itself
is also waiting on the same thread.

### Executor unit tests

Tests that exercise only `execute` (no IO) live in
`crates/minos-proxy/tests/executor_unit.rs`. They construct `Filter`
implementations inline, build `FilterInstance` values, and assert on the
returned `Verdict` and on log entries received from the mpsc channel. No
Tokio runtime is needed for these; they run as plain `#[test]` functions
or within a minimal `#[tokio::test]` if the channel is involved.
