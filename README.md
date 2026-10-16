# xscore

Rust session manager for SSH / shell / serial. Sessions are opened from URLs and routed
through a `Transport × Sink` composition so the same CLI works for local tmux, remote
tmux-in-ssh, and plain shells.

## Quick start

```
xscore connect shell:///bin/bash
xscore connect ssh://user@host:22?tmux=on
xscore connect serial:///dev/ttyUSB0?baud=115200
```

## Commands

```
xscore connect <url> [--save]
xscore session list
xscore session show --id <uuid>
xscore session close --id <uuid>
xscore session save --id <uuid>
xscore session load --path <file>

xscore data read --id <uuid> [--bytes N]
xscore data write --id <uuid> [--from-file FILE]
xscore data resize --id <uuid> --cols C --rows R

xscore config show
xscore config path
xscore config set <key> <value>
xscore config unset <key>
```

## Architecture

```
CLI  →  SessionManager  →  CompositeBackend(Transport, Sink)
                               │
            ┌──────────────────┼──────────────────┐
        Transport             Sink              Result
     shell/serial/ssh     tmux/raw            bytes
            │                  │
            ▼                  ▼
     local / ssh channel   external tmux server
                           on -L xscore socket
```

Every session pairs one Transport with one Sink. The same `TmuxSink` works on shell
(local tmux), ssh (remote tmux running inside the channel), and serial (passthrough
wrapper). A future `RawSink` (or `?tmux=off`) supports passthrough.