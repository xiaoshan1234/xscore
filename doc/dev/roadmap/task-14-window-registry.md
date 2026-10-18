# Task 14: Window — registry, sink ops, CLI verb group, pane `--window`

**Objective:** Introduce the Window tier between Link and Pane (ADR 0006). Add `WindowId` / `Window` / `WindowRegistry` to the model, `new_window` / `kill_window` to `Sink`, `add_window` / `del_window` to `SessionManager`, the `xscore window` subcommand, and rename `pane --link` → `pane --window`. Keep the common path (`xscore link add` → pane) one step via auto-created default window.

**Background:** ADR 0006 adds Window so xscore's addressable tiers (link → window → pane) match tmux's (session → window → pane); the server stays shared per ADR 0003. Window primarily enables tmux split-pane UI within one window. ADR 0005 already shaped the link verb tree; this task mirrors the same shape for window.

**Files:**
- Create: `crates/xscore/src/window.rs` (or `crates/xscore/src/cli/window.rs` + `crates/xscore/src/core/window.rs`)
- Modify: `crates/xscore/src/core/{id,mod,address}.rs` — add `WindowId` type
- Modify: `crates/xscore/src/model/{mod,session}.rs` — add `Window` / `WindowInfo` structs (`window.rs`)
- Modify: `crates/xscore/src/sink/{mod,sink,tmux,raw}.rs` — add `new_window` / `kill_window` ops; `TmuxSink::new_pane` becomes a `split-window` op (a pane is a split within a window, not a new window)
- Modify: `crates/xscore/src/core/{manager,mod,link_registry,address}.rs` — add `WindowRegistry`; new `add_window` / `del_window` methods; `PaneInfo.link_id` → `PaneInfo.window_id`
- Modify: `crates/xscore/src/cli/{mod,link,pane}.rs` — new `WindowCmd` 4-variant enum + handler; `PaneCmd::Add` switches to `--window <window-id>` + auto-create path
- Modify: `crates/xscore/src/main.rs` — register `Commands::Window`
- Modify: `crates/xscore/tests/cli_smoke.rs` — new window smoke tests; pane-add tests move to `--window`

---

## Command reference

```
xscore window add   --link <link-id> [--name=NAME]    # tmux new-window inside the link's session
xscore window list  [--link <link-id>]               # enumerate windows in a link
xscore window show  --id <window-id>
xscore window del   --id <window-id>                  # tmux kill-window (panes inside die)

xscore pane   add   --window <window-id> [--name=NAME]   # tmux split-window inside that window
xscore pane   list  [--window <window-id>]
xscore pane   show  --id <pane-id>
xscore pane   del   --id <pane-id>

# Ergonomic shortcut: `link add` auto-creates one default window + one pane
xscore link add shell:///bin/bash    # 1 link + 1 default window + 1 default pane
```

The `link add` shortcut is mandatory for the slice's common path to stay one step. Without it, the user's "I want one work context" path becomes `link add` + `window add` + `pane add` (three steps), which would push the user away from xscore.

---

## Step 1: `WindowId` in `core/id.rs`

```rust
// In core/id.rs

use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LinkId(Uuid);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowId(Uuid);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PaneId(Uuid);
```

`WindowId::new()` generates a uuid v7 (consistent with `LinkId` / `PaneId`).
`WindowId::parse(&str)` parses the canonical string form (hyphenated 36-char UUID).

**Step 1: Commit**
```bash
git add crates/xscore/src/core/id.rs
git commit -m "feat(core): add WindowId type (uuid v7)"
```

---

## Step 2: `Window` / `WindowInfo` in `model/window.rs`

```rust
// model/window.rs
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use crate::core::{LinkId, WindowId};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WindowInfo {
    pub id: WindowId,
    pub link_id: LinkId,
    pub name: String,                    // user-friendly name; not unique across links
    pub created_at: DateTime<Utc>,
    /// Index of this window inside its link's tmux session (tmux assigns
    /// these incrementally; used to address the window via tmux wire
    /// commands like `tmux -L xscore kill-window -t <session>:<idx>`).
    pub tmux_window_index: u32,
}

#[derive(Debug)]
pub struct Window {
    pub info: WindowInfo,
    pub panes: tokio::sync::Mutex<Vec<crate::core::PaneId>>,
}

impl Window {
    pub fn new(info: WindowInfo) -> Self {
        Self { info, panes: tokio::sync::Mutex::new(Vec::new()) }
    }
    pub async fn add_pane(&self, pane_id: crate::core::PaneId) {
        self.panes.lock().await.push(pane_id);
    }
    pub async fn remove_pane(&self, pane_id: crate::core::PaneId) {
        self.panes.lock().await.retain(|p| *p != pane_id);
    }
}
```

**Step 2: Commit**
```bash
git add crates/xscore/src/model/window.rs
git commit -m "feat(model): Window + WindowInfo structs (id, link_id, name, tmux_window_index)"
```

---

## Step 3: `WindowRegistry` in `core/window_registry.rs`

```rust
// core/window_registry.rs
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use crate::core::{LinkId, PaneId, WindowId};
use crate::model::Window;

type ArcWindow = Arc<Window>;

#[derive(Debug, Default)]
pub struct WindowRegistryInner {
    by_id: HashMap<WindowId, ArcWindow>,
    by_link: HashMap<LinkId, Vec<WindowId>>,
}

#[derive(Debug, Default)]
pub struct WindowRegistry {
    inner: RwLock<WindowRegistryInner>,
}

impl WindowRegistry {
    pub async fn insert(&self, window: Window) -> ArcWindow {
        let mut g = self.inner.write().await;
        let link_id = window.info.link_id;
        let id = window.info.id;
        let entry = g.by_link.entry(link_id).or_default();
        if !entry.contains(&id) { entry.push(id); }
        let arc = Arc::new(window);
        g.by_id.insert(id, arc.clone());
        arc
    }

    pub async fn get(&self, id: WindowId) -> Option<ArcWindow> {
        self.inner.read().await.by_id.get(&id).cloned()
    }

    pub async fn list_for_link(&self, link_id: LinkId) -> Vec<ArcWindow> {
        let g = self.inner.read().await;
        let ids = g.by_link.get(&link_id).cloned().unwrap_or_default();
        ids.iter().filter_map(|wid| g.by_id.get(wid).cloned()).collect()
    }

    pub async fn remove(&self, id: WindowId) -> Option<ArcWindow> {
        let mut g = self.inner.write().await;
        let arc = g.by_id.remove(&id)?;
        if let Some(ids) = g.by_link.get_mut(&arc.info.link_id) {
            ids.retain(|w| *w != id);
        }
        Some(arc)
    }
}
```

**Step 3: Commit**
```bash
git add crates/xscore/src/core/window_registry.rs crates/xscore/src/core/mod.rs
git commit -m "feat(core): WindowRegistry (by_id + by_link secondary index)"
```

---

## Step 4: `Sink` gets window lifecycle (`new_window` / `kill_window`)

```rust
// In sink/mod.rs — extend the existing Sink trait

use crate::model::{ConnectSpec, PaneInfo, WindowInfo};

#[async_trait]
pub trait Sink: Send + Sync {
    fn kind(&self) -> &'static str;

    /// Wire this sink to a transport. Called once per link lifetime.
    async fn attach(&mut self, transport: &dyn Transport) -> Result<()>;

    /// List panes — used by LinkRegistry recovery / `pane list`.
    async fn list_panes(&self) -> Result<Vec<String>>;
    async fn new_pane(&self, pane_handle: &str) -> Result<()>;
    async fn close_pane(&self, pane_handle: &str) -> Result<()>;
    async fn snapshot(&self, pane_handle: &str, buf: &mut [u8]) -> Result<usize>;
    async fn write(&self, pane_handle: &str, data: &[u8]) -> Result<usize>;
    async fn resize(&self, pane_handle: &str, cols: u16, rows: u16) -> Result<()>;

    /// NEW (ADR 0006): create a new tmux window inside the sink's session.
    /// Returns the WindowInfo for bookkeeping; the sink retains the address.
    async fn new_window(&mut self, name: &str) -> Result<WindowInfo>;

    /// NEW (ADR 0006): kill a window. The sink tears down the tmux window;
    /// panes inside die with it.
    async fn kill_window(&mut self, tmux_window_index: u32) -> Result<()>;

    async fn close(&mut self) -> Result<()>;
}
```

`TmuxSink` implements `new_window` as `tmux -L xscore new-window -t <session_name> -n <window_name>` and parses the output for the new window's index. `RawSink` adds no-op stubs. The existing `new_pane` becomes `tmux -L xscore split-window -t <session>:<window_index>` (a pane is now a split inside a window, not a new window — per ADR 0006's mapping).

**Step 4: Commit**
```bash
git add crates/xscore/src/sink/
git commit -m "feat(sink): Sink::new_window + Sink::kill_window; TmuxSink now uses split-window for panes"
```

---

## Step 5: `PaneInfo` field rename + new manager methods

```rust
// In model/session.rs — rename PaneInfo.link_id → PaneInfo.window_id

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PaneInfo {
    pub id: PaneId,
    pub window_id: WindowId,             // was: link_id
    pub name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub sink_handle: String,             // tmux window %id; pane's underlying tmux window
    pub pane_index: u32,                 // index inside the window (split index)
}
```

```rust
// In core/manager.rs — new methods + updated add_pane signature

impl SessionManager {
    /// Add a new window to an existing link. Returns the new WindowId.
    pub async fn add_window(&self, link_id: LinkId, name: Option<String>) -> Result<WindowId> {
        let arc_link = self.links.get(link_id).await.ok_or(Error::LinkNotFound(link_id))?;
        let resolved_name = name.unwrap_or_else(|| format!("w-{}", arc_link.info.tmux_session_id()));
        // Reconfigure-style: assign WindowId, call sink.new_window, register.
        let window_id = WindowId::new();
        let window_info = WindowInfo {
            id: window_id,
            link_id,
            name: resolved_name.clone(),
            created_at: chrono::Utc::now(),
            tmux_window_index: 0,    // sink returns the real one; for code clarity the field is set post-call.
        };
        let mut window = Window::new(window_info);
        // Real implementation calls `sink.new_window(&name)` and uses the
        // returned tmux_window_index.
        self.windows.insert(window).await;
        Ok(window_id)
    }

    /// Add a new pane to an existing window. Returns the new PaneId.
    /// (Was: `add_pane(link_id, name)` — signature change per ADR 0006.)
    pub async fn add_pane(&self, window_id: WindowId, name: Option<String>) -> Result<PaneId> {
        // ... same body as old add_pane, but parent is window instead of link
    }

    /// Tear down a window + every pane inside.
    pub async fn del_window(&self, id: WindowId) -> Result<()> {
        // drop the sink's tmux window (panes inside die with it), unregister
    }
}
```

**Step 5: Commit**
```bash
git add crates/xscore/src/core/manager.rs crates/xscore/src/model/
git commit -m "feat(core): rename PaneInfo.link_id → window_id; add_window/del_window methods"
```

---

## Step 6: `xscore window` subcommand (cli/window.rs)

```rust
// cli/window.rs
use crate::core::{Error, LinkId, SessionManager, WindowId};
use crate::url;
use clap::{Args, Subcommand};
use std::sync::Arc;

#[derive(Subcommand)]
pub enum WindowCmd {
    /// Create a new window inside a link's tmux session
    Add(WindowAddArgs),
    /// List windows (optionally filtered by --link)
    List { #[arg(long)] link: Option<String> },
    /// Print WindowInfo JSON
    Show { #[arg(long)] id: String },
    /// tmux kill-window (panes inside die with it; no in-memory detach semantic)
    Del { #[arg(long)] id: String },
}

#[derive(Args)]
pub struct WindowAddArgs {
    /// Link to add this window to
    #[arg(long)] link: String,
    /// Window name (becomes `tmux -L xscore new-window -t <session> -n <name>`)
    #[arg(long)] name: Option<String>,
}

pub async fn run(mgr: Arc<SessionManager>, cmd: WindowCmd) -> anyhow::Result<()> {
    match cmd {
        WindowCmd::Add(args) => add(mgr, args).await,
        WindowCmd::List { link } => list(mgr, link).await,
        WindowCmd::Show { id } => show(mgr, &id).await,
        WindowCmd::Del { id } => del(mgr, &id).await,
    }
}

async fn add(mgr: Arc<SessionManager>, args: WindowAddArgs) -> anyhow::Result<()> {
    let lid = LinkId::parse(&args.link)?;
    let wid = mgr.add_window(lid, args.name).await?;
    println!("window: {wid}");
    Ok(())
}

async fn list(mgr: Arc<SessionManager>, link: Option<String>) -> anyhow::Result<()> {
    let lid = link.as_deref().map(LinkId::parse).transpose()?;
    for w in mgr.windows.list_for_link_opt(lid).await {
        println!("{}\t{}\t{}\t{}", w.info.id, w.info.link_id, w.info.name, w.info.tmux_window_index);
    }
    Ok(())
}

async fn show(mgr: Arc<SessionManager>, id: &str) -> anyhow::Result<()> {
    let wid = WindowId::parse(id)?;
    let w = mgr.windows.get(wid).await.ok_or(Error::WindowNotFound(wid))?;
    println!("{}", serde_json::to_string_pretty(&w.info)?);
    Ok(())
}

async fn del(mgr: Arc<SessionManager>, id: &str) -> anyhow::Result<()> {
    let wid = WindowId::parse(id)?;
    mgr.del_window(wid).await?;
    println!("deleted: {wid}");
    Ok(())
}
```

`PaneInfo`: rename `link_id` field to `window_id`. The `pane add` CLI switches from `--link <link-id>` to `--window <window-id>` (mandatory).

`link add` shortcut: `xscore link add <url>` auto-creates one default window inside the link's session (named after the link-id-derived UUID), then auto-creates one default pane inside that window. This keeps the common path one step.

**Step 6: Commit**
```bash
git add crates/xscore/src/cli/
git commit -m "feat(cli): xscore window {add, del, show, list} + pane --window + link-add default window"
```

---

## Step 7: `main.rs` dispatch

```rust
// In main.rs

#[derive(Subcommand)]
pub enum Commands {
    Link(link::LinkCmd),
    Pane(pane::PaneCmd),
    Window(window::WindowCmd),     // NEW
    Data(data::DataCmd),
    Config(config::ConfigCmd),
}
```

---

## Step 8: Tests + smoke (lives in `task-13`, but referenced here)

- `xscore_window_add_creates_window_in_session`
- `xscore_window_list_filters_by_link`
- `xscore_window_del_kills_tmux_window`
- `xscore_link_add_auto_creates_default_window_and_pane`
- (existing) `xscore_pane_add_creates_pane_in_window` — uses `--window <wid>` instead of `--link <lid>`

---

## Step 9: Commit (final)

```bash
git add crates/xscore/src/cli/ crates/xscore/src/core/ crates/xscore/src/sink/ crates/xscore/src/model/ crates/xscore/src/main.rs
git commit -m "feat(cli,core,sink): Window — third tier (ADR 0006). New verb group + manager methods + tmux split-window for panes."
```

---

## Open questions for implementation (deferred until then)

1. **`window open <name>` for cross-invocation re-attach.** Per ADR 0006 Open Question 1.
2. **Default window naming.** Per ADR 0006 Open Question 2 — implementation picks one of: `<link_uuid>`, `default`, or let the user choose via a future flag.
3. **tmux address syntax for windows.** `<session>:<window_index>` vs `<session>:<window_name>`. Implementation picks one (ADR 0006 Open Question 3).

These are not blockers; pick at implementation time.
