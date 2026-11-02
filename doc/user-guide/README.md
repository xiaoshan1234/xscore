# xscore 使用说明

xscore 是一个终端模拟器。当前交付的是后端（L0–L2：平台抽象、域/连接、VT 核心）
加一个 CLI 模式的模拟前端（L3–L5：布局树、渲染、协调），用于在图形界面完成之前
驱动和验证后端。

目标能力：原生支持 `tmux -CC` 控制模式、普通 SSH、本地/远端 Shell，以及多 pane
分屏布局。现在还没有图形渲染，前端是一个交互式 REPL，也支持在终端里直接带命令
一次性调用。

架构说明见 [terminal-emulator.md](../arch/terminal-emulator.md)，实现计划见
[roadmap](../roadmap/README.md)。

## 构建与运行

```bash
cargo build --workspace
cargo run -p emulator-frontend
```

不带参数进入交互式命令行，提示符为 `xscore>`；输入 `help` 可随时查看命令。二进制
名称为 `xs`，也可以直接跟命令在终端里跑一次（不进入 REPL）：

```bash
./target/debug/xs shell powershell       # Linux / WSL
.\target\debug\xs.exe shell powershell   # Windows
xs list
xs tmux work --wsl
```

`xs --help` 列出全部子命令；带命令时执行一次后即退出，适合脚本化驱动。

### 常驻 daemon

一次性命令与 REPL 都不直接持有连接：命令通过本地 IPC（Windows 命名管道 / Unix
socket）发给一个常驻 daemon，由它持有所有 live 连接与 pane 状态。这样
`xs shell powershell` 退出后，pane 仍在 daemon 里，下一条 `xs list` 还能看到。
daemon 在第一条命令时自动拉起，也可以手动管理：

```text
> daemon serve      # 前台运行 daemon（自动拉起用的也是这个）
> daemon status     # 查看 daemon 当前持有的 pane / tmux session
> daemon stop       # 让 daemon 退出
```

daemon 持有的是进程内存态：`daemon stop` 会释放全部 live 连接；跨 daemon 重启的
磁盘持久化尚未实现。

## 命令参考

| 命令 | 说明 |
|---|---|
| `shell <program> [args...]` | 起一个本地 shell pane |
| `ssh <user@host[:port]> [identity]` | 起一个远端 SSH shell pane；无 identity 时无回显提示密码 |
| `wsl [args...]` | 起一个 WSL shell pane（Windows） |
| `tmux <session> [-L <socket> \| -S <path>] [--ssh <dest> \| --wsl] [--password]` | attach 一个 tmux session |
| `split h\|v <program> [args...]` | 在聚焦 pane 处拆分出新 pane |
| `write <pane-id> <text>` | 给 pane 发一行输入（自动补换行） |
| `read <pane-id>` | 打印该 pane 当前渲染出的屏幕 |
| `resize <pane-id> <cols> <rows>` | 调整 pane 尺寸 |
| `focus <pane-id>` | 切换焦点（tmux pane 走容器内焦点） |
| `pane del <pane-id>` | 删除 pane（tmux 发 kill-pane，本地/SSH 关闭连接） |
| `window del <handle>` | 删除 tmux window（kill-window） |
| `daemon serve \| status \| stop` | 运行 / 查看 / 停止常驻 daemon |
| `sync` | 按 tmux 服务端现状同步：删除已消失的 pane/window，纳入新增的 pane/window |
| `layout` | 打印窗格树 |
| `list` | 列出所有 pane |
| `quit` / `exit` | 退出 |

tmux 的 `--ssh-arg <arg>` 和 `--wsl-arg <arg>` 可重复，透传给 `ssh` 或 `wsl.exe`。
`--password` 触发密码认证：优先读 `XSCORE_SSH_PASSWORD` / `PASSWD` 环境变量，否则
无回显交互提示。

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
> ssh dev@example.com                             # 无 identity 时无回显提示密码
> ssh dev@example.com C:/Users/me/.ssh/id_ed25519 # 密钥
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

密码认证（提示读 `XSCORE_SSH_PASSWORD` / `PASSWD`，没有则无回显交互提示）：

```text
> tmux work --ssh dev@example.com --password
```

同步与删除：

```text
> sync                 # 移除服务端已删除的 pane/window
> pane del <pane-id>
> window del @0
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

## 真实环境测试

仓库带一个针对真实 SSH tmux 的集成测试（默认 `#[ignore]`，未设环境变量时自动跳
过）：

```powershell
$env:XSCORE_SSH_HOST = 'loner@shanclaw'
$env:XSCORE_SSH_PASSWORD = '...'   # 或 $env:PASSWD
cargo test -p emulator-frontend --test tmux_ssh -- --ignored --nocapture
```

该测试在远端新建一个专用 session，覆盖 attach、split-window、结构枚举、kill-pane
与清理，结束后自动 `kill-session`。

## 平台支持

| 平台 | 支持 |
|---|---|
| Linux / WSL（在 WSL 内运行） | 本地 shell、本地 tmux、远端 tmux over ssh |
| Windows 原生 | 本地 ConPTY shell、WSL shell、WSL tmux；不支持本地 tmux |
| 远端 SSH | shell 会话（russh，密钥或密码）、tmux（系统 ssh） |

## 已知限制

- `tmux` 只 attach 已有 session，不创建；创建请用外部 `tmux new-session`。
- SSH 密码认证支持密码和密钥，暂不支持 agent；tmux-over-ssh 的密码走系统 `ssh` 的
  `SSH_ASKPASS`，不复用交互 shell pane 里输入的密码。
- `read` 在脚本化输入下有一个 60ms 的静默窗口，极端时序可能读到旧屏；交互使用
  无影响。
- tmux 结构同步目前是 `sync` 命令的按需重枚举，双向覆盖“服务端删除→本地删”和
  “服务端新增→本地加”；事件驱动的实时增删尚未接入。
- SSH tmux 端到端已在 `loner@shanclaw` 验证；WSL tmux 仍需真实 WSL 环境验证。
