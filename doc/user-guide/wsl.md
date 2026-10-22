# Windows and WSL

xscore has two supported ways to work with WSL:

1. Run the native Windows `xscore.exe` from PowerShell or Windows Terminal.
   `shell:///wsl`, `shell:///powershell` and `shell:///pwsh` all start a
   process through Windows ConPTY, while the resident daemon runs on Windows
   and is reached through a per-user named pipe.
2. Run xscore itself inside WSL. Use this shape for WSL-local tmux, Linux serial
   devices and the normal Unix daemon socket.

## Native Windows quick start

```powershell
cargo build --release

# Active distribution and its configured default shell
.\target\release\xscore.exe session new dev

# Explicit distribution, shell and working directory
.\target\release\xscore.exe session new ubuntu `
  'shell:///wsl?distro=Ubuntu&shell=/bin/bash&cwd=/home/me/src&cols=120&rows=40'

# Native Windows PowerShell
.\target\release\xscore.exe session new ps 'shell:///powershell?cwd=C:/src'
.\target\release\xscore.exe session new ps7 'shell:///pwsh?cwd=C:/src'
```

PowerShell single quotes are recommended because `&` separates query
parameters. The pane id printed by `session new` is used by all data commands:

```powershell
'echo hi from WSL' | .\target\release\xscore.exe data write --pane <pid>
.\target\release\xscore.exe data read --pane <pid>
```

The daemon survives across PowerShell invocations. Stop it explicitly with:

```powershell
.\target\release\xscore.exe daemon stop
```

Stopping the daemon loses pane ring buffers. Session metadata remains in
`state.json`. `pane attach` is also the reconnect operation for a pipe link.

## `shell:///wsl` grammar

`wsl`, `powershell` and `pwsh` are program names in the URL path, just like
`/bin/bash`. They are not URL schemes or transport kinds.

| URL | Meaning |
|---|---|
| `shell:///wsl` | default WSL distribution and default shell |
| `shell:///wsl?distro=Ubuntu` | named distribution, default shell |
| `shell:///wsl?distro=Ubuntu&shell=/bin/bash` | named distribution and shell |
| `shell:///wsl?cwd=/home/me/src` | default distribution in a WSL directory |

Accepted query parameters are `cwd`, `shell`, `cols`, `rows` and `name`.
Options are passed as argv entries to `wsl.exe`; xscore does not construct a
second shell command string. The resulting arguments are:

```text
wsl.exe --distribution <distro> --cd <cwd> --exec <shell>
```

An omitted option is omitted from argv rather than replaced with a guessed
value. This lets WSL use the user's configured distribution and login shell.

## PowerShell

PowerShell is another launch profile for the same Shell transport, not a new
backend or transport variant:

| URL path | Program |
|---|---|
| `shell:///powershell` | `powershell.exe -NoLogo` |
| `shell:///powershell.exe` | `powershell.exe -NoLogo` |
| `shell:///pwsh` | `pwsh.exe -NoLogo` |
| `shell:///pwsh.exe` | `pwsh.exe -NoLogo` |

`cwd`, `cols`, `rows` and `name` are accepted as query parameters. On a Windows
host, `cwd` can be a Windows or WSL path; it is applied by the process spawner.

## Paths

`cwd` may be a WSL path or a Windows drive path. When xscore runs inside WSL,
an existing `C:\src` is translated to `/mnt/c/src`. In the native Windows
build, the path is left as typed and handed to `wsl.exe --cd`, which performs
the translation on the Windows side.

`\\wsl$\...` UNC paths and drive-relative paths such as `C:foo` are not
rewritten. Use an absolute WSL path when the result must be unambiguous.

## What is supported

| Shape | Native Windows `xscore.exe` | xscore inside WSL |
|---|---|---|
| WSL shell through `shell:///wsl` | yes | use `shell:///bin/bash` instead |
| Windows PowerShell through `shell:///powershell` | yes | use `shell:///pwsh` when installed |
| Linux pty and cross-command pane buffers | yes, ConPTY | yes |
| daemon | Windows named pipe | Unix socket |
| tmux on the WSL side | not through native local `--tmux` yet | yes |
| Linux serial device | not directly | yes |
| remote tmux over SSH | depends on Windows `ssh` setup | yes |

The native Windows build supports WSL, Windows PowerShell 5.1 and PowerShell 7.
Windows `cmd.exe` is not a promised URL backend, and local tmux on Windows is
not supported.

## Running xscore inside WSL

This remains the best choice for tmux and Linux-only device access:

```bash
cd ~ && cargo build --release
install -m755 target/release/xscore ~/.local/bin/xscore

xscore session new dev 'shell:///bin/bash?cwd=/home/me/src&cols=120&rows=40'
xscore session new work --tmux 'shell://?cwd=/home/me/src'
```

Inside WSL, a Windows program such as `cmd.exe` may spawn without producing
useful output because WSL interop does not give it a console behind the Linux
pty. Start the Windows program from a WSL shell pane instead:

```bash
printf 'cmd.exe /c dir C:\\\n' | xscore data write --pane <pid>
```

## WSL-specific gotchas

SSH keys on `/mnt/c` appear with Windows permissions, which the Linux `ssh`
client may reject. Copy private keys into the WSL filesystem with mode `0600`.

`/mnt/c` is a slow 9p mount and does not carry trustworthy Unix permissions.
Keep xscore state, config and daemon files on the Linux filesystem.

A WSL restart drops the in-WSL daemon and its memory buffers. A native Windows
daemon survives, but its PowerShell/WSL child processes may need to be
reconnected after WSL itself restarts.
