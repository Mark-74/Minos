# Writing a Filter for Minos

This document is for developers extending Minos with new filter types. For
operator-facing documentation on configuring the built-in filters, see the
operator guide.

## The two traits

Filtering is built on two traits in `minos-core`:

- **`Filter`** — the runtime trait. One instance per `FilterInstance` in a
  service pipeline. The pipeline executor in `crates/minos-proxy/src/executor.rs`
  calls it once per inspected packet.
- **`FilterKind`** — the registry trait. Maps a kind name (e.g. `"regex"`)
  to a `Filter` builder driven by a JSON config. Used at config-save time to
  turn the on-disk config blob into runnable `Arc<dyn Filter>` instances.

The two are deliberately separate. `Filter` knows nothing about
configuration, JSON schemas, or how it was constructed — it only answers
"does this packet match?". `FilterKind` knows nothing about packets — it
only knows how to build a `Filter`.

## The `Filter` trait

```rust
pub trait Filter: Send + Sync {
    fn kind(&self) -> &'static str;
    fn accepts(&self, p: &Packet) -> bool;
    fn inspect(&self, p: &Packet) -> Verdict;
}
```

- `kind()` returns a static identifier (matches `FilterKind::NAME`). Used for
  log entries and UI display.
- `accepts(packet)` is a cheap pre-filter. The executor calls it before
  `inspect` and skips this filter if it returns `false`. Use this to declare
  "I only handle HTTP packets" or "I only handle inbound" — the latter is
  redundant since direction toggles live on `FilterInstance`, but the former
  is the main use.
- `inspect(packet) -> Verdict` is the real work. Return `Verdict::Pass` to
  let the packet continue down the pipeline, or `Verdict::Block { reason }`
  to short-circuit. The executor handles dry-run mode and direction toggles
  — your filter does not need to.

The trait is **sync** by design. Asynchronous filters (anything that needs
IO) use `tokio::task::block_in_place` + `Handle::current().block_on(...)` —
see `crates/minos-filters/src/sidecar/filter.rs` for the canonical example.

## The `FilterKind` trait

```rust
pub trait FilterKind: 'static {
    const NAME: &'static str;
    type Config: Serialize + DeserializeOwned + JsonSchema;
    fn build(cfg: Self::Config) -> Result<Arc<dyn Filter>, BuildError>;
}
```

- `NAME` is the string the JSON config uses (e.g. `"regex"`).
- `Config` is a Serde-deserializable struct that also derives `schemars::JsonSchema`. The schema is used to auto-render a config editor in the
  web UI without writing any UI code.
- `build(cfg)` validates the config and returns a working `Filter`. Return
  `BuildError::Invalid { kind, message }` for any user-visible problem
  (bad regex, unknown method, missing field). The config layer rejects the
  save and shows the message to the operator.

`build` is **synchronous**. Most filters can do all their setup inside it.
The `python_sidecar` kind installs a venv (sync) but defers spawning the
Python wrapper process until the binary starts the supervisor — see the
`PythonSidecarKind` rustdoc for why.

## Adding a new filter type as a third-party crate

A third-party filter crate depends only on `minos-core` (plus `serde` and
`schemars` for its config). It defines:

```rust
use std::sync::Arc;
use minos_core::{BuildError, Filter, FilterKind, Packet, Verdict};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub struct MyFilter { /* ... */ }
impl Filter for MyFilter {
    fn kind(&self) -> &'static str { "my_filter" }
    fn accepts(&self, _: &Packet) -> bool { true }
    fn inspect(&self, _: &Packet) -> Verdict { Verdict::Pass }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MyConfig { /* ... */ }

pub struct MyKind;
impl FilterKind for MyKind {
    const NAME: &'static str = "my_filter";
    type Config = MyConfig;
    fn build(_cfg: MyConfig) -> Result<Arc<dyn Filter>, BuildError> {
        Ok(Arc::new(MyFilter { /* ... */ }))
    }
}
```

To make Minos pick it up, the binary calls `registry.register::<MyKind>()`
once at startup, alongside `register_builtin_filters`. After that:

- The save flow accepts configs that reference `"my_filter"` as their kind.
- The web UI auto-generates a config form from your `MyConfig` JSON schema.
- The pipeline executor runs `MyFilter::inspect` like any built-in.

No core code is changed. No registration ceremony beyond one line.

## JSON schema and the UI

Every `FilterKind::Config` derives `schemars::JsonSchema`. The web UI reads
the registered schemas at runtime and renders edit forms automatically:
strings become text inputs, enums become dropdowns, nested structs become
sub-sections. This is why the `regex` kind's UI form was never written
explicitly — it falls out of the `pub struct RegexConfig { pub pattern: String }` definition.

If a filter wants a custom editor (the `python_sidecar` kind uses CodeMirror
for the script field), it registers a template override in the web crate.
The default auto-form remains as fallback.

## Fail-open vs fail-closed (python_sidecar)

The `python_sidecar` filter is the only built-in that can fail at runtime
(a sidecar process can crash, hang, or exceed the per-call timeout). The
default behavior is **fail-open**: on any sidecar failure, `inspect` returns
`Verdict::Pass` so traffic keeps flowing. Per design §7.5 this is the right
default for a CTF — losing visibility into one filter beats losing the whole
service.

Operators who prefer the opposite — block-on-failure — set `fail_closed: true`
in the filter's config. See `PythonSidecarConfig` in
`crates/minos-filters/src/sidecar/filter.rs`.

## Where the Python sidecar wire format lives

See `docs/reference/sidecar-protocol.md` for the on-the-wire spec between
the Rust supervisor and the Python wrapper script. The Python script itself
is `crates/minos-filters/src/sidecar/wrapper.py` — embedded at compile time
via `include_str!` into `wrapper.rs`.

## Testing patterns

- `Filter` impls are unit-testable in isolation — build directly, call
  `inspect` on a synthesized `Packet`. See
  `crates/minos-filters/src/regex_filter.rs` for examples.
- `FilterKind::build` is tested through the registry: register the kind,
  call `FilterRegistry::build("name", json_config)`, assert the result.
  See `crates/minos-filters/tests/regex_filter.rs`.
- Filters that need real IO (the Python sidecar) gate their integration
  tests behind a runtime check (`uv --version` succeeds) and soft-skip
  otherwise. CI without `uv` still passes.
