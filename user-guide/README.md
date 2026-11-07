# xscore 使用说明

xscore 是一个终端模拟器。当前交付的是后端（L0–L1：平台抽象、域/连接）
加一个 CLI 模式的模拟前端（L2–L5：VT 核心、布局树、渲染、协调），用于在图形界面完成之前
驱动和验证后端。

目标能力：原生支持 `tmux -CC` 控制模式、普通 SSH、本地/远端 Shell，以及多 pane
分屏布局。现在还没有图形渲染，前端是一个纯命令行工具：一次性命令 + 常驻 daemon，
没有交互式 REPL。

架构说明见 [terminal-emulator.md](../arch/terminal-emulator.md)，实现计划见
[roadmap](../roadmap/README.md)。

## 安装

```bash
cargo install --path crates/xsc-server
cargo install --path crates/xsc-client
```

安装后得到两个二进制：`xsc`（命令客户端）和 `xss`（常驻服务端），都放在
`~/.cargo/bin`（确保目录在 `PATH` 里）。也可以不安装，先运行
`cargo build --workspace`，再直接使用 `./target/debug/xsc[.exe] <命令>`；client 会在
同目录寻找 `xss[.exe]`。

## 构建与运行

```bash
cargo build --workspace
```

不带参数时打印帮助（已无交互式 REPL）：

```bash
./target/debug/xsc tml add --url shell://powershell
.\target\debug\xsc.exe tml add --url shell://powershell   # Windows
xsc ls pane
xsc tmx attach --url tmux://work --wsl 'wsl://?args=--distribution,Ubuntu'
xsc
```

`xsc --help` 列出全部子命令；每条命令执行一次后即退出，适合脚本化驱动。

### 常驻 daemon

一次性命令不直接持有连接：`xsc` 通过本地回环 TCP 把命令发给独立的 `xss`，
由它持有所有 live 连接与 pane 状态。这样 `xsc tml add --url shell://powershell`
退出后，pane 仍在 daemon 里，下一条 `xsc ls pane` 还能看到。daemon 在第一条命令时
自动拉起，也可以手动管理：

```text
xsc daemon serve    # 前台运行 xss（自动拉起的也是这个）
xsc daemon status   # 查看 daemon 当前持有的 pane / tmux session
xsc daemon stop     # 让 daemon 退出
```

daemon 持有的是进程内存态：`daemon stop` 会释放全部 live 连接；跨 daemon 重启的
磁盘持久化尚未实现。

## 命令参考

| 命令 | 说明 |
|---|---|
| `tml add --url <URL> [...]` | 添加本地 shell/ssh/wsl 对话，或 tmux window/pane |
| `tml del <pane-id>` | 删除 pane |
| `tmx attach --url <URL> [--ssh <ssh-url> \| --wsl <wsl-url>]` | 接入 tmux 会话 |
| `tml write <pane-id> <text>` | 给 pane 发一行输入（自动补换行） |
| `tml read <pane-id>` | 打印该 pane 当前渲染出的屏幕 |
| `tml resize <pane-id> <cols> <rows>` | 调整 pane 尺寸 |
| `tml focus <pane-id>` | 切换焦点（tmux pane 走容器内焦点） |
| `tml fetch <pane-id> [--line <N>]` | 拉取 tmux pane 历史 |
| `tml log <pane-id> --format <raw\|char> --max-size <N>` | 将 pane 输出记录到本地文件 |
| `ls pane` / `ls window [<group>]` / `ls group` | 列出 pane / window / group |
| `daemon serve \| status \| stop` | 运行 / 查看 / 停止常驻 daemon |
| `tmx sync <group-id>` | 按 tmux 服务端现状同步指定 group：删除已消失的 pane/window，纳入新增的 pane/window |
| `layout [<window-id>]` | 打印布局树或指定 window 的布局 |

## 使用示例

### 本地 / SSH / WSL 对话

```text
xsc tml add --url 'shell://bash'
xsc tml add --url 'shell://powershell.exe?cwd=/home/me/src&cols=120&rows=40'
xsc tml add --url 'ssh://dev@example.com'
xsc tml add --url 'wsl://?args=--distribution,Ubuntu'
xsc tml write 1 echo hello
xsc tml read 1
```

### tmux

先在外部建好 tmux session，再用 `tmx attach` 接入：

```bash
tmux new-session -d -s work
```

```text
xsc tmx attach --url tmux://work
xsc tmx attach --url tmux://work --wsl 'wsl://?args=--distribution,Ubuntu'
xsc tmx attach --url tmux://work --ssh 'ssh://dev@example.com?identity=~/.ssh/id_ed25519'
xsc ls pane
xsc tmx sync work
xsc tml read <pane-id>
xsc tml write <pane-id> echo hi
```

### tmux 里的 window / pane

```text
xsc tml add --group work --new-window
xsc tml add --group work --sph 3
xsc ls window work
```

window 标识统一编码为 `Gxx-Tyy` / `Gxx-Myy`：`xx` 是 group id，`yy` 是 group 内
window 序号，`T` 表示 tmux window，`M` 表示 shell/ssh/wsl window。

### 布局

```text
xsc tml add --url 'shell://bash'
xsc tml add --url 'shell://bash' --sph 1
xsc layout
```

`tml add --sph/--spv` 在指定 pane 上拆出新 pane；在 tmux 镜像容器上拆分时，容器保持
完整，新 pane 成为它的平级兄弟，tmux 内部 pane 不会出现在本地树里。

## 真实环境测试

仓库带有 SSH shell、远端 tmux 和服务端 tmux 集成测试。测试从工作区根目录的
`test.config` 读取 SSH 连接信息，格式见
[`test-example.config`](../test-example.config)：

```text
ssh_host=user@host
ssh_password=...
ssh_port=22
```

```powershell
python scripts/run_integration_tests.py
```

脚本会运行本地 PTY、SSH shell 往返/resize、tmux attach/输入/输出/resize/split/
kill，以及服务端 tmux 路径。每次运行会在 `test-results/<UTC 时间>/` 生成
`summary.md`、`results.json`、`junit.xml` 和逐套件日志；日志中的 SSH 密码会被
脱敏。

## 平台支持

| 平台 | 支持 |
|---|---|
| Linux / WSL（在 WSL 内运行） | 本地 shell、本地 tmux、远端 tmux over ssh |
| Windows 原生 | 本地 ConPTY shell、WSL shell、WSL tmux；不支持本地 tmux |
| 远端 SSH | shell 会话与 tmux（均为 russh，密钥或密码） |

## 已知限制

- tmux 结构同步目前是 `tmx sync <group-id>` 命令的按需重枚举，双向覆盖“服务端删除→
  本地删”和“服务端新增→本地加”；事件驱动的实时增删尚未接入。
- `read` 在脚本化输入下有一个 60ms 的静默窗口，极端时序可能读到旧屏；交互使用
  无影响。
- SSH tmux 端到端已在 `loner@shanclaw` 验证；WSL tmux 仍需真实 WSL 环境验证。
