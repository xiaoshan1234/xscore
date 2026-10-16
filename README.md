# xscore

Rust session manager for SSH / shell / serial, modeled as **Link x Pane**.

- **Link** = one physical connection (SSH channel, serial port, local child, or tmux server).
- **Pane** = one working view into a link. N panes per link; each pane has its own id, snapshot, write state.

## Quick start

```
xscore link add shell:///bin/bash
xscore link add ssh://user@host:22
xscore link add serial:///dev/ttyUSB0?baud=115200

# Persistent template (one-time):
xscore link config add web ssh://u@h:22?identity=~/.ssh/id_ed25519

# Reuse template + attach multiple panes on the same link:
xscore link add ssh://u@h:22 -c web           # 1st pane on the link

# Open / detach / destroy — verb split (ADR 0005):
xscore link open 'tmux://my-session'           # attach existing tmux session
xscore link close --id <link-id>               # tmux-detach only (session keeps running)
xscore link del --id <link-id>                 # full teardown (kills tmux server)
```

## Commands

```
xscore link add <url?> [-c CONFIG]            # always create new (url OR -c CONFIG required)
xscore link del --id <link-id>                # full teardown (kills transport + sink + tmux server)
xscore link open <url?> [-c CONFIG]           # attach existing tmux session (tmux only)
xscore link close --id <link-id>              # tmux-detach only (session keeps running; non-tmux errs)
xscore link show --id <link-id>
xscore link list

xscore link config add <name> <url>
xscore link config remove <name>
xscore link config show <name>
xscore link config list

xscore pane add --link <link-id> [--name=NAME]
xscore pane open --id <pane-id> [--link <link-id>]
xscore pane close --id <pane-id>
xscore pane remove --id <pane-id>
xscore pane show --id <pane-id>
xscore pane list [--link <link-id>]

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
       +-- LinkRegistry     (by_id, by_address)
       |    +-- Link { transport + sink + Vec<PaneId> }
       +-- PaneRegistry     (pane_id -> PaneInfo)
       +-- BackendRegistry  (pane_id -> Arc<dyn PaneBackend>)
                                  ^ wraps (Link, sink_handle)
```

Each Link owns one Transport (raw IO) + one Sink (PTY/ANSI/tmux). Each Pane lives inside
one link and addresses the link's sink via a sink_handle (e.g. tmux `%42`).

**`add` vs `del` vs `open` vs `close`** (ADR 0005, four-verb `link` tree):
- `add` creates a brand-new external resource (transport + sink + 1st pane).
- `del` is the universal full teardown: kill transport + sink + every pane, plus (for tmux) the per-link tmux server.
- `open` attaches to an existing tmux session (tmux only — shell/serial/ssh return `OpenOnlyForTmux`).
- `close` is the in-memory detach: unregister the link and its panes, but the tmux session keeps running (tmux only — shell/serial/ssh return `CloseOnlyForTmux`).

`link open` and `link close` are tmux-only because shell/serial/ssh processes don't have cross-invocation identity to attach to or detach from. `link add <same url>` always creates a brand-new link; to re-attach to an existing resource, the verb is `link open`. See [ADR 0005](doc/dev/adr/0005-add-open-close-del-verbs.md) for the rationale.