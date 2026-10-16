# xscore

Rust session manager for SSH / shell / serial, modeled as **Link x Pane**.

- **Link** = one physical connection (SSH channel, serial port, local child, or tmux server).
- **Pane** = one working view into a link. N panes per link; each pane has its own id, snapshot, write state.

## Quick start

```
xscore connect shell:///bin/bash
xscore connect ssh://user@host:22
xscore connect serial:///dev/ttyUSB0?baud=115200

# Multiple panes on the same link:
xscore connect ssh://u@h:22
xscore connect ssh://u@h:22 --link=existing   # opens a 2nd pane in the same tmux server
```

## Commands

```
xscore connect <url> [--save] [--link=existing]
xscore link list
xscore link show --id <link-id>
xscore link close --id <link-id>
xscore link save --id <link-id>
xscore link load --path <file>

xscore pane list [--link <link-id>]
xscore pane show --id <pane-id>
xscore pane close --id <pane-id>

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
       +-- LinkRegistry     (address-keyed dedup)
       |    +-- Link { transport + sink + Vec<PaneId> }
       +-- PaneRegistry     (pane_id -> PaneInfo)
       +-- BackendRegistry  (pane_id -> Arc<dyn PaneBackend>)
                                  ^ wraps (Link, sink_handle)
```

Each Link owns one Transport (raw IO) + one Sink (PTY/ANSI/tmux). Each Pane lives inside
one link and addresses the link's sink via a sink_handle (e.g. tmux `%42`). Closing a
pane does not close the link; closing a link drops every pane on it.

`--link=existing` opts into **address-keyed dedup**: search LinkRegistry for an existing
link matching the URL's transport address; if found, open a new pane on it; if not found,
error. Without `--link=existing`, every `xscore connect` creates a brand-new link.