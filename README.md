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
xscore link add ssh://u@h:22 -c web
xscore link add ssh://u@h:22 --link=existing
```

## Commands

```
xscore link add <url> [-c CONFIG] [--link=existing]
xscore link open --id <link-id>
xscore link close --id <link-id>
xscore link remove --id <link-id>
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

**`add` vs `open`:** `add` creates a new external resource; `open` re-attaches an
existing one. **`close` vs `remove`:** `close` kills the underlying resource (transport
+ sink + tmux window); `remove` only unregisters from in-memory state and leaves the
external resources alive.

`--link=existing` opts into **address-keyed dedup**: search LinkRegistry for an existing
link matching the URL's transport address; if found, open a new pane on it; if not found,
error. Without `--link=existing`, every `xscore link add` creates a brand-new link.