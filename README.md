# xscore

Rust session manager for SSH / shell / tmux / serial.

## CLI

```
xscore session create --from spec.json [--save]
xscore session list
xscore session show --id <uuid>
xscore session close --id <uuid>
xscore session load --path spec.json

xscore data write --id <uuid> [--from-file FILE]
xscore data read  --id <uuid> [--bytes N]

xscore config show
xscore config path
xscore config set <key> <value>
xscore config unset <key>
```

Spec JSON:

```json
{
  "name": "web1",
  "type": "ssh",
  "host": "10.0.0.1",
  "port": 22,
  "user": "root"
}
```

## Architecture

```
CLI  →  SessionManager  →  [SessionBackend] → tmux / shell / ssh / serial
                ↓
            Registry  → JSON snapshots in ~/.config/xscore/sessions/
```

The same `SessionManager` will back the MCP server in the next slice.