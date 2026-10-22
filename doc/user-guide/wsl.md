# WSL

xscore is a Linux program. On Windows it runs **inside WSL**, against the WSL
side of the machine — and that is what this page is about: which boundaries
work, which are translated, and which cannot work at all.

Measured on WSL2 (kernel `6.18.x-microsoft-standard`) with Debian.

## The short version

| You want | Does it work |
|---|---|
| native sessions (shell / ssh / serial) inside WSL | yes |
| tmux sessions on the **WSL** tmux server | yes |
| `cwd` given as a Windows path (`C:\Users`) | translated to `/mnt/c/Users` |
| `cwd` given as a WSL path (`/mnt/c/Users`, `~/src`) | yes |
| a Windows program (`.exe`) as a link | connects, produces no output — warned at creation |
| tmux on the **Windows** side | no: Windows has no tmux |
| running xscore as a Windows executable | no: not built for it |

## What gets translated

`cwd` is the only place a Windows path is accepted, because it is the only
place a Windows path is what a user would naturally type:

```bash
xscore session new dev 'shell:///bin/bash?cwd=C:\Users\loner'
# the shell really starts in /mnt/c/Users/loner

xscore session new work --tmux 'shell://?cwd=C:\src'
# tmux new-session -c /mnt/c/src
```

The rule is deliberately narrow:

- only `X:\…`, `X:/…` or a bare `X:` — a drive letter plus a separator;
- only when `/mnt/<letter>` is actually mounted, otherwise the value is left
  exactly as typed and fails as a missing directory;
- nothing else is guessed. `\\wsl$\…` UNC paths and `C:foo` drive-relative
  paths are *not* rewritten.

Anything that does not exist is now refused instead of quietly ignored:

```
$ xscore session new dev 'shell:///bin/bash?cwd=/nope'
Error: invalid input: cwd `/nope` is not a directory
```

(Portable-pty used to fall back to `$HOME` when the child could not `chdir`,
so a typo produced a session in the wrong directory with no error at all.)

## What does not work

**A Windows program as a link.** `shell:///mnt/c/Windows/System32/cmd.exe`
spawns, `link show` reports `connected: true`, and then nothing ever arrives in
the pane's buffer: WSL interop does not give the program a console behind a
Linux pty. xscore warns when the command ends in `.exe`, `.bat` or `.cmd`:

```
$ xscore session new win 'shell:///mnt/c/Windows/System32/cmd.exe'
warning: `/mnt/c/Windows/System32/cmd.exe` is a Windows program. WSL interop
gives it no console behind a pty, so it typically produces no output; run it
from inside a WSL shell pane instead.
```

Do that instead:

```bash
xscore session new dev                       # a WSL shell
printf 'cmd.exe /c dir C:\\\n' | xscore data write --pane <pid>
```

A program started from a real WSL shell inherits that shell's terminal, so it
behaves the way it does in your terminal.

**tmux on Windows.** There is no tmux for Windows, so `--tmux` always means a
server on the WSL side (or one reached over `ssh://`). A tmux session whose
panes run Windows programs is not a supported shape: the first window's command
goes through `/bin/sh -c` inside WSL, and if it exits, the session ends.

**A native Windows build.** Out of scope: no tmux, and the daemon, the socket
and the pty layer are all POSIX.

## WSL-specific gotchas

**SSH keys living on `/mnt/c`.** Windows-side key files are `0777` on 9p, and
`ssh` refuses those ("bad permissions"). Copy the key into WSL once:

```bash
install -m600 /mnt/c/Users/you/.ssh/id_ed25519 ~/.ssh/id_ed25519
ssh-add ~/.ssh/id_ed25519        # or just let the agent hold it
```

`ssh://` links and `--tmux 'ssh://host'` both use the system `ssh`, so
`~/.ssh/config`, `ProxyJump` and the agent all apply as usual.

**`/mnt/c` is 9p, not a Linux filesystem.** It is slow for many small files, it
has no Unix permission bits worth trusting, and advisory locks on it are
advisory in the weakest sense. Keep xscore's own data off it: `HOME`,
`XDG_CONFIG_HOME` and `XDG_RUNTIME_DIR` should point into the WSL filesystem.
The defaults do — `$XDG_RUNTIME_DIR` is `/mnt/wslg/runtime-dir`, which is a
**tmpfs** (verified: `none on /mnt/wslg type tmpfs`), so the daemon socket and
its lock are fine there.

If you do set `HOME=/mnt/c/...`, expect `state.json` and the daemon lock to be
slow and to survive restarts in a half-written state; nothing in xscore guards
against that.

**A WSL restart drops the daemon.** Its socket disappears with the tmpfs, and
buffered pane bytes go with the process — that is the normal `daemon stop`
semantics, just triggered by `wsl --shutdown`. Metadata (`state.json`) and the
tmux server survive if tmux itself survives; tmux dies with the WSL instance, so
`session ls` may list a session whose server is gone:

```
$ xscore data read --pane <pid>
Error: daemon error: tmux session `work` is gone: its control client exited
          (the tmux session ended, or the server went away)
```

`session kill <name>` clears the rows.

**Interop is per-process.** The daemon inherits the environment of whatever
started it (the first command that needed a live connection), including
`WSLENV`, `PATH` and proxy variables. If you change your Windows-side
environment, `xscore daemon stop` and let it restart from a fresh shell.

## A sane WSL setup

```bash
# 1. everything on the Linux side
cd ~ && cargo build --release
install -m755 target/release/xscore ~/.local/bin/xscore

# 2. tmux is the WSL one; a private socket keeps experiments apart
export XSCORE_SOCKET=xscore

# 3. Windows paths are accepted where they make sense
xscore session new c 'shell:///bin/bash?cwd=C:\src\myproject&cols=120&rows=40'

# 4. if a remote tmux server is the point, the key must be in WSL with 0600
xscore session new build --tmux 'ssh://dev@box?identity=~/.ssh/id_ed25519'
```
