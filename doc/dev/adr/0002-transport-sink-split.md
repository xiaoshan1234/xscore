# ADR 0002: Split Transport and Sink (vs flat SessionBackend)

- **Status:** Accepted
- **Date:** 2026-10-16
- **Deciders:** dev
- **Related:** [ADR 0001](./0001-link-pane-model.md), [overview.md](../roadmap/overview.md)

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