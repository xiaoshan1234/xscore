# ADR 0002: Split Transport and Sink (vs flat SessionBackend)

- **Status:** Accepted (Revised 2026-10-20 — see [ADR 0007 v2](./0007-raw-default-tmux-opt-in.md); Sink is still optional; `RawSink` is the no-sink marker for raw links, `TmuxSink` for tmux-backed links. The `Box<dyn Sink>` on Link is unchanged.)
- **Date:** 2026-10-16 (revised 2026-10-19, again 2026-10-20)
- **Deciders:** dev
- **Related:** [ADR 0001](./0001-link-pane-model.md), [ADR 0007](./0007-raw-default-tmux-opt-in.md) (the 2026-10-19 design pivot that made the Sink layer optional), [overview.md](../roadmap/overview.md)

## Context

The original design had one `SessionBackend` trait per `SessionKind` (Ssh/Shell/Serial/Tmux). Every backend owned its own PTY, byte mux, and protocol logic. To support "ssh + tmux running inside the SSH channel" we either:

1. Hand-roll tmux protocol inside SshBackend (huge).
2. Compose: a `SshTmuxBackend` that wraps a `russh` channel and a tmux process spawned over that channel.

The second is cleaner — but it meant the `SessionBackend` trait must describe **two orthogonal layers** that any combination should satisfy. Naming it `SessionBackend` would have hidden the two-layer seam.

## Decision

Split the seam into **two orthogonal traits**:

```rust
#[async_trait]
pub trait Transport: Send + Sync {
    fn kind(&self) -> &'static str;
    fn address(&self) -> LinkAddress;
    async fn open(&mut self) -> Result<()>;
    async fn write(&self, data: &[u8]) -> Result<usize>;
    async fn read(&self, buf: &mut [u8]) -> Result<usize>;
    async fn resize(&self, cols: u16, rows: u16) -> Result<()>;
    async fn close(&self) -> Result<()>;
}

#[async_trait]
pub trait Sink: Send + Sync {
    fn kind(&self) -> &'static str;
    async fn attach(&mut self, transport: &dyn Transport) -> Result<()>;
    async fn new_pane(&self) -> Result<String>;
    async fn close_pane(&self, pane_handle: &str) -> Result<()>;
    async fn list_panes(&self) -> Result<Vec<String>>;
    async fn snapshot(&self, pane_handle: &str, buf: &mut [u8]) -> Result<usize>;
    async fn write(&self, pane_handle: &str, data: &[u8]) -> Result<usize>;
    async fn resize(&self, pane_handle: &str, cols: u16, rows: u16) -> Result<()>;
    async fn close(&self) -> Result<()>;
}
```

`Transport` is **bytes only** — open/write/read/resize/close over a byte stream.
`Sink` is **PTY/terminal semantics** — owns the protocol (tmux CLI for now; passthrough stub for raw mode).

Composition is a separate trait:

```rust
#[async_trait]
pub trait PaneBackend: Send + Sync {
    async fn snapshot(&self, buf: &mut [u8]) -> Result<usize>;
    async fn write(&self, data: &[u8]) -> Result<usize>;
    async fn write_raw(&self, data: &[u8]) -> Result<usize>;
    async fn resize(&self, cols: u16, rows: u16) -> Result<()>;
    async fn close(&self) -> Result<()>;
}
```

`PaneBackend` is what `SessionManager` sees. It is implemented by `CompositeBackend { pane_id, link: ArcLink, sink_handle: String }` — one per pane.

## Consequences

**Positive:**
- The same `TmuxSink` works on `Shell`/`Serial`/`Ssh` transports without per-transport glue. The factory decides where the tmux server runs (`local` / `remote` / `off`), not the transport.
- Adding a future `WebTransport` is just one new file in `transport/`. Adding a future `ScriptSink` is one new file in `sink/`. Cross-product grows linearly.
- `write_raw` on `PaneBackend` exists for forward-compat — escape sink semantics and hit the transport directly. Used by debug tools; rarely.

**Negative:**
- More trait boilerplate. Each new backend impl is 60-100 lines.
- The `attach()` method on `Sink` is a forward-compat hook (currently no-op for TmuxSink). Defer real implementation until SSH transport has real IO (then `attach()` will spawn tmux over the SSH channel).

**Risks:**
- The `address()` method on `Transport` adds a redundant field (`LinkAddress`) — easy to forget to keep updated when adding fields. Mitigated by tests on `LinkAddress::from(transport_address)` in the factory path.
- `PaneBackend::close()` semantics ambiguous: should it close the pane only, or the link? Decided: close **pane only**. Link close is `link close --id`.

## Alternatives considered

- **Single trait with extra methods.** Rejected: forces every backend to know about PTY/ANSI even if it doesn't care.
- **Three traits (Transport / Sink / Link).** Rejected: `Link` is a struct, not a trait; it composes the two.
- **Trait objects only on Transport, generics on Sink.** Rejected: complicates factory pattern; `Arc<dyn Sink>` is fine in Rust 2021.

## Revised 2026-10-19 (per ADR 0007): Sink is now optional (RawSink is the no-sink marker)

This ADR's split between Transport (bytes) and Sink (PTY/ANSI/tmux)
stands, but the Sink layer is no longer universal. The 2026-10-19
design pivot ([ADR 0007](./0007-raw-default-tmux-opt-in.md)) made
`Sink` optional:

- For a **raw link** (default, no `?tmux=` query), the link's sink is
  a `RawSink` — a unit struct that implements the `Sink` trait with
  every method as a no-op (or `unreachable!()` for ops that don't make
  sense without a sink, like `new_window` / `new_pane`). The transport
  is the I/O source directly; data ops would call the transport's
  `write` / `read` / `resize` (this is wired through
  `backends/composite.rs` and `RawSink` is just a marker that the link
  has no tmux session).
- For a **tmux-backed link** (`?tmux=...` query), the link's sink is
  the existing `TmuxSink` (per ADR 0003 + ADR 0006). The split between
  Transport and Sink is exactly as documented above.

The factory in `sink/factory.rs` now dispatches on `SinkConfig`:
`Raw` → `Box::new(RawSink)`; `Tmux { ... }` → `Box::new(TmuxSink::create(...))`.

The two-layer trait design is now a **two-layer optional stack**:
Transport is always present, Sink is present only when the user opts
in to tmux via the URL. This is a cleaner separation than v0.1.0's
"every link has a TmuxSink" because the trait shape itself now matches
the actual usage (most raw users never touch the Sink methods).

Read [ADR 0007](./0007-raw-default-tmux-opt-in.md) for the full
decision context and the migration notes from v0.1.0.
