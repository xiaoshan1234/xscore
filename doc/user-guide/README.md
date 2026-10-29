# xscore 使用说明

xscore 是一个终端模拟器。当前交付的是后端（L0–L2：平台抽象、域/连接、VT 核心）
加一个 CLI 模式的模拟前端（L3–L5：布局树、渲染、协调），用于在图形界面完成之前
驱动和验证后端。

目标能力：原生支持 `tmux -CC` 控制模式、普通 SSH、本地/远端 Shell，以及多 pane
分屏布局。现在还没有图形渲染，前端是一个交互式 REPL。

架构说明见 [terminal-emulator.md](../arch/terminal-emulator.md)，实现计划见
[roadmap](../roadmap/README.md)。

## 构建与运行

```bash
cargo build --workspace
cargo run -p emulator-frontend
```

进入后是交互式命令行，提示符为 `xscore>`。输入 `help` 可随时查看命令。

## 命令参考

| 命令 | 说明 |
|---|---|
| `shell <program> [args...]` | 起一个本地 shell pane |
| `ssh <user@host[:port]> <identity>` | 起一个远端 SSH shell pane（仅密钥，identity 是私钥路径） |
| `wsl [args...]` | 起一个 WSL shell pane（Windows） |
| `tmux <session> [-L <socket> \| -S <path>] [--ssh <dest> \| --wsl]` | attach 一个 tmux session |
| `split h\|v <program> [args...]` | 在聚焦 pane 处拆分出新 pane |
| `write <pane-id> <text>` | 给 pane 发一行输入（自动补换行） |
| `read <pane-id>` | 打印该 pane 当前渲染出的屏幕 |
| `resize <pane-id> <cols> <rows>` | 调整 pane 尺寸 |
| `focus <pane-id>` | 切换焦点（tmux pane 走容器内焦点） |
| `layout` | 打印窗格树 |
| `list` | 列出所有 pane |
| `quit` / `exit` | 退出 |

tmux 的 `--ssh-arg <arg>` 和 `--wsl-arg <arg>` 可重复，透传给 `ssh` 或 `wsl.exe`。

## 使用示例

### 本地 shell

```text
> shell bash
> write 1 echo hello
> read 1
```

Windows 原生：

```text
> shell cmd.exe
> shell powershell.exe -NoLogo
```

### SSH shell

```text
> ssh dev@example.com C:/Users/me/.ssh/id_ed25519
> write 1 ls -la
> read 1
```

### WSL shell（Windows）

```text
> wsl
> wsl --distribution Ubuntu --exec /bin/bash
```

### tmux

先在外部建好 tmux session（当前 `tmux` 命令只 attach，不负责创建）：

```bash
tmux new-session -d -s work
```

本地：

```text
> tmux work
> list
> read <pane-id>
> write <pane-id> echo hi
```

WSL 里的 tmux（Windows 原生程序）：

```text
> tmux work --wsl
> tmux work --wsl --wsl-arg --distribution --wsl-arg Ubuntu
```

远端主机上的 tmux（走系统 `ssh`，复用 `~/.ssh/config`、agent）：

```text
> tmux work --ssh dev@example.com
> tmux work --ssh dev@example.com --ssh-arg -p --ssh-arg 2222
```

### 布局

```text
> shell bash
> split h bash
> layout
[P1 | P2]
focused: 2
```

`split` 在本地 pane 上拆出新 pane；在 tmux 镜像容器上拆分时，容器保持完整，新
pane 成为它的平级兄弟，tmux 内部 pane 不会出现在本地树里。

## 平台支持

| 平台 | 支持 |
|---|---|
| Linux / WSL（在 WSL 内运行） | 本地 shell、本地 tmux、远端 tmux over ssh |
| Windows 原生 | 本地 ConPTY shell、WSL shell、WSL tmux；不支持本地 tmux |
| 远端 SSH | shell 会话（russh，仅密钥）、tmux（系统 ssh） |

## 已知限制

- `tmux` 只 attach 已有 session，不创建；创建请用外部 `tmux new-session`。
- SSH shell 只支持密钥认证，暂不支持密码/agent。
- `read` 在脚本化输入下有一个 60ms 的静默窗口，极端时序可能读到旧屏；交互使用
  无影响。
- tmux 的 `%layout-change` 实时对账（容器内 pane 动态增删）尚未接入，当前是
  attach 时枚举一次。
- WSL tmux、远端 tmux 的端到端路径需要真实 WSL/SSH 环境验证。
