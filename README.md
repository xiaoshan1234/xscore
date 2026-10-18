# xscore

Rust session manager for SSH / shell / serial, modeled as **Link × Window × Pane** (ADR 0006).

- **Link** = one physical connection (SSH channel, serial port, local child, or one tmux session inside the shared `tmux -L xscore` server).
- **Window** = one tmux window inside the link's session. A link can have N named windows.
- **Pane** = one working view — a tmux split-window inside a Window. A window can have M panes sharing its screen.

## Quick start

```
xscore link add shell:///bin/bash
xscore link add ssh://user@host:22
xscore link add serial:///dev/ttyUSB0?baud=115200

# Persistent template (one-time):
xscore link config add web ssh://u@h:22?identity=~/.ssh/id_ed25519

# Multiple windows inside one link (ADR 0006) — e.g. dev + ops + play sessions:
xscore window add   --link <link-id> --name dev      # new tmux window
xscore pane   add   --window <window-id> --name bash  # split-window inside that tmux window

# Open / detach / destroy — verb split (ADR 0005):
xscore link open 'tmux://my-session'           # attach existing tmux session
xscore link close --id <link-id>               # tmux-detach only (session keeps running)
xscore link del --id <link-id>                 # full teardown (kills the per-link tmux session)
```

## Commands

```
xscore link add <url?> [-c CONFIG]            # always create new (url OR -c CONFIG required)
xscore link del --id <link-id>                # full teardown (kills transport + sink + the per-link tmux session)
xscore link open <url?> [-c CONFIG]           # attach existing tmux session (tmux only)
xscore link close --id <link-id>              # tmux-detach only (session keeps running; non-tmux errs)
xscore link show --id <link-id>
xscore link list

xscore link config add <name> <url>
xscore link config remove <name>
xscore link config show <name>
xscore link config list

xscore window add   --link <link-id> [--name=NAME]
xscore window list  [--link <link-id>]
xscore window show  --id <window-id>
xscore window del   --id <window-id>

xscore pane add --window <window-id> [--name=NAME]
xscore pane del --id <pane-id>
xscore pane show --id <pane-id>
xscore pane list [--window <window-id>]

xscore data read --pane <pane-id> [--bytes N]
xscore data write --pane <pane-id> [--from-file FILE]
xscore data resize --pane <pane-id> --cols C --rows R

xscore config show
xscore config path
xscore config set <key> <value>
xscore config unset <key>
```

## Architecture

```
CLI -> SessionManager
       +-- LinkRegistry     (by_id, by_address)         -- one Link per physical connection
       +-- WindowRegistry   (by_id, by_link)           -- one Window per tmux window (ADR 0006)
       +-- PaneRegistry     (pane_id -> PaneInfo;     -- one Pane per tmux split-window
                              PaneInfo.window_id)       (ADR 0006: was PaneInfo.link_id)
       +-- BackendRegistry  (pane_id -> Arc<dyn PaneBackend>)
                                  ^ wraps (Link, sink_handle)
```

Each Link owns one Transport (raw IO) + one Sink (PTY/ANSI/tmux). Each Window inside a link is a tmux window inside the link's session. Each Pane inside a window is a tmux split-window sharing the window's screen. Killing a pane does not affect its siblings or its parent window; killing a window kills every pane inside it (via tmux kill-window); closing a link keeps the tmux session alive (re-attachable via `link open`); killing a link (`link del`) drops every window + pane and the tmux session.

**`add` vs `del` vs `open` vs `close`** (ADR 0005, four-verb `link` tree):
- `add` creates a brand-new external resource: transport + sink + a default window + a default pane.
- `del` is the universal full teardown: kill transport + sink + every window + every pane, plus (for tmux) the per-link tmux session. The shared `tmux -L xscore` server stays up.
- `open` attaches to an existing tmux session (tmux only — shell/serial/ssh return `OpenOnlyForTmux`).
- `close` is the in-memory detach: unregister the link + windows + panes, but the tmux session keeps running (tmux only — shell/serial/ssh return `CloseOnlyForTmux`).

`link open` and `link close` are tmux-only because shell/serial/ssh processes don't have cross-invocation identity to attach to or detach from. `link add <same url>` always creates a brand-new link; to re-attach to an existing resource, the verb is `link open`. See [ADR 0005](doc/dev/adr/0005-add-open-close-del-verbs.md) for the rationale.