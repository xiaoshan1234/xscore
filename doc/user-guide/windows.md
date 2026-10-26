# xscore on Windows

This guide uses the native Windows `xscore.exe`. It can manage:

- a WSL shell through `shell:///C:/Windows/System32/wsl.exe`;
- Windows PowerShell 5.1 through the full `powershell.exe` path;
- PowerShell 7 through the full `pwsh.exe` path.

All three use the same Shell transport over Windows ConPTY. The resident daemon
runs as a Windows process and is reached through a per-user named pipe.

For Linux, WSL-local tmux, serial devices and the Unix socket daemon, read
[`linux.md`](linux.md).

For every command and flag, read [`commands.md`](commands.md).

## Requirements

- Windows 10 or Windows 11 with ConPTY.
- Rust 1.95 or newer to build from source.
- WSL is optional and is needed only for the `wsl.exe` launcher.
- `pwsh.exe` is optional.

Check the programs you intend to use:

```powershell
wsl.exe --status
powershell.exe -NoProfile -Command '$PSVersionTable.PSVersion'
pwsh.exe -NoProfile -Command '$PSVersionTable.PSVersion'
```

## Build

From the repository root:

```powershell
cargo build --release
.\target\release\xscore.exe --help
```

Optionally install it into a directory on `PATH`:

```powershell
New-Item -ItemType Directory -Force "$env:USERPROFILE\bin" | Out-Null
Copy-Item .\target\release\xscore.exe "$env:USERPROFILE\bin\xscore.exe"
```

The examples below use `.\target\release\xscore.exe`; replace it with
`xscore.exe` after installation.

## Model

```text
Session  a workspace
  Window a grouping container
  Pane   a UI viewport and geometry; it references a Buffer
  Buffer independently managed bytes and history
  Link   one connection to a shell
```

`session new` creates the Session and requires at least one raw Link. Its
backend SessionManager turns Link input into independent Buffers, and creates
the initial UI Pane.

## Create a Session

```powershell
# Raw Session with one Link
.\target\release\xscore.exe session new dev `
  --link 'shell:///C:/Windows/System32/wsl.exe'

# All child commands now resolve `dev`
.\target\release\xscore.exe session show dev
```

`window add`, `pane add` and `link add` always require an explicit `--session`.

## Link URL syntax

There is one public shell scheme:

```text
shell:///<executable-path>[?options]
```

`shell` is the protocol. Everything after `shell:///` is the executable path.
There are no launcher keywords hidden in the path.

```text
shell:///C:/Windows/System32/WindowsPowerShell/v1.0/powershell.exe
shell:///C:/Program%20Files/PowerShell/7/pwsh.exe
shell:///C:/Windows/System32/wsl.exe
```

Use three slashes. `shell://powershell` puts `powershell` in the URL authority
and is rejected.

### PowerShell link

```powershell
# Windows PowerShell 5.1
.\target\release\xscore.exe session new ps `
  --link 'shell:///C:/Windows/System32/WindowsPowerShell/v1.0/powershell.exe?arg=-NoLogo&cwd=C:/src&cols=120&rows=40'

# PowerShell 7 in ps7
.\target\release\xscore.exe session new ps7 `
  --link 'shell:///C:/Program%20Files/PowerShell/7/pwsh.exe?arg=-NoLogo&cwd=C:/src&cols=120&rows=40'
```

Use forward slashes in the URL. Single-quote the complete URL in PowerShell so
that `&` is not interpreted by the shell.

### WSL link

```powershell
# Default distribution
.\target\release\xscore.exe session new wsl `
  --link 'shell:///C:/Windows/System32/wsl.exe'

# Named distribution, directory and inner shell
.\target\release\xscore.exe session new ubuntu `
  --link 'shell:///C:/Windows/System32/wsl.exe?arg=--distribution&arg=Ubuntu&arg=--cd&arg=/home/me/src&arg=--exec&arg=/bin/bash'
```

Shell query parameters:

| Key | Meaning |
|---|---|
| `arg` | One child-process argument. Repeat it for every argument. |
| `cwd` | Host process working directory. |
| `cols`, `rows` | Initial terminal size. |
| `name` | Link display name. |

For WSL, change the Linux directory through an explicit `--cd` argument rather
than the host-process `cwd`. The named-distribution example expands to:

```text
wsl.exe --distribution Ubuntu --cd /home/me/src --exec /bin/bash
```

The raw SessionManager owns Link-to-Buffer routing. A Pane is only a UI
viewport that references the Buffer ID it displays.

## Create and find a session

```powershell
$out = .\target\release\xscore.exe session new dev `
  --link 'shell:///C:/Windows/System32/WindowsPowerShell/v1.0/powershell.exe?arg=-NoLogo'

$pane = ($out -match '^pane:')[0] -replace '^pane:\s*', ''
$link = ($out -match '^link:')[0] -replace '^link:\s*', ''
```

The same ids can be recovered later:

```powershell
.\target\release\xscore.exe session show dev
.\target\release\xscore.exe pane list --session dev
.\target\release\xscore.exe buffer list --session dev
```

`pane list` prints `PANE ID` and `BUFFER ID`. Pane IDs address UI; data commands
resolve the attached Buffer. `buffer show/del` manage bytes independently from
the UI pane.

## Read from a pane

```powershell
.\target\release\xscore.exe data read --pane $pane --bytes 4096
```

Reading is non-destructive. It returns a snapshot of the newest requested bytes
from the pane ring buffer; it does not drain or acknowledge the stream.

The output is terminal bytes, not clean text. It can contain:

- echoed input;
- ANSI escape sequences;
- `\r` line endings;
- a PowerShell prompt.

If no bytes have arrived, the command prints nothing.

## Write to a pane

In PowerShell, piping a string adds a line ending and executes it:

```powershell
'Get-Date' | .\target\release\xscore.exe data write --pane $pane
.\target\release\xscore.exe data read --pane $pane --bytes 8192
```

Send multiple commands:

```powershell
'$PSVersionTable.PSVersion'
'Get-Location'
'Get-ChildItem' | .\target\release\xscore.exe data write --pane $pane
```

PowerShell turns the pipeline into newline-separated text before it reaches
`xscore`.

For exact bytes, use a file:

```powershell
Set-Content -Encoding ascii `
  .\commands.txt `
  -Value 'Get-Date', 'Get-Location'

.\target\release\xscore.exe data write `
  --pane $pane `
  --from-file .\commands.txt
```

Without a line ending, a console shell receives the text but does not execute
it as a command.

### Control keys

Control characters are ordinary bytes:

```powershell
[IO.File]::WriteAllBytes("$env:TEMP\ctrl-c.bin", [byte[]](3))
.\target\release\xscore.exe data write `
  --pane $pane `
  --from-file "$env:TEMP\ctrl-c.bin"
```

This writes `Ctrl-C` (`0x03`). Use `0x04` for Ctrl-D.

## Resize

```powershell
.\target\release\xscore.exe data resize --pane $pane --cols 120 --rows 40
```

The resize reaches the ConPTY and the child shell.

## Daemon

The daemon starts on demand:

```powershell
.\target\release\xscore.exe daemon status
.\target\release\xscore.exe daemon stop
```

`daemon status` prints JSON to stdout. A stopped daemon:

- loses all in-memory Buffers;
- closes live shell processes;
- keeps session metadata in `state.json`.

Reconnect a pipe pane after a daemon restart:

```powershell
.\target\release\xscore.exe pane attach --pane $pane --link $link
```

`pane attach` is also the reconnect operation. It recreates the saved shell
transport when the daemon no longer holds it.

## State locations

| Item | Location |
|---|---|
| Session metadata | `%USERPROFILE%\.config\xscore\state\state.json` |
| Link templates | `%USERPROFILE%\.config\xscore\links\` |
| Daemon IPC | Per-user Windows named pipe derived from the config path |

If `HOME` is set, it takes precedence over `USERPROFILE` for compatibility with
tests and isolated environments.

## Windows limitations

- Local Windows tmux is not supported. Run xscore inside WSL for WSL-local tmux.
- The native runner manages shell byte streams; it does not launch an extra
  Windows Terminal window.
- A pipe Link has one Buffer and one SessionManager route. Multiple UI panes
  may reference that Buffer without detaching one another.
- `cmd.exe` is not a documented launcher. Use PowerShell, `pwsh` or WSL.
- `daemon stop` loses buffered output.

## Troubleshooting

### WSL reports `E_ACCESSDENIED`

Run the command from a normal Windows Terminal or PowerShell session. Restricted
execution sandboxes can prevent the WSL service from creating a Linux instance.

### `pwsh.exe` is not found

Install PowerShell 7 or use the full Windows PowerShell executable path.

### The pane is empty

Check that the child is still alive:

```powershell
.\target\release\xscore.exe daemon status
.\target\release\xscore.exe pane show --id $pane
```

For a shell command, include a line ending or pipe a string through PowerShell.

### The link reports that its transport died

The daemon restarted or was stopped. Reconnect:

```powershell
.\target\release\xscore.exe pane attach --pane $pane --link $link
```

### Need exact command flags

```powershell
.\target\release\xscore.exe --help
.\target\release\xscore.exe session new --help
.\target\release\xscore.exe data write --help
```
