# xs 命令手册

`xs` 是 xscore 终端模拟器的命令行前端。当前交付的是「一次性命令 + 常驻 daemon」模式：
每条命令执行一次后进程即退出，但 pane 与 live 连接由后台 daemon 持有，不会随 CLI 进程退出
而丢失。没有交互式 REPL。

## 基本用法

```text
xs                         # 不带参数：打印 clap 帮助
xs --help                  # 列出全部子命令（clap 帮助）
xs help                    # 打印简洁命令清单
xs <command> [args...]     # 执行一条一次性命令
```

安装：`cargo install --path crates/emulator-frontend`，得到 `~/.cargo/bin/xs`。

## 命令总览

| 命令 | 说明 |
|---|---|
| `tml add [...]` | 添加本地 shell/ssh/wsl 对话或 tmux window/pane |
| `tml del <pane-id>` | 删除 pane |
| `tmx attach --url <URL> [--ssh <ssh-url> \| --wsl <wsl-url>]` | 用 URL 接入 tmux 会话 |
| `split h\|v <program> [args...]` | 在聚焦 pane 上拆分出新 pane |
| `write <pane-id> <text>...` | 向 pane 发送一行输入 |
| `read <pane-id>` | 打印 pane 当前渲染出的屏幕 |
| `resize <pane-id> <cols> <rows>` | 调整 pane 尺寸 |
| `focus <pane-id>` | 切换焦点 |
| `layout` | 打印窗口/布局树 |
| `pane del <pane-id>` | 删除 pane |
| `pane fetch <pane-id> [--line <N>]` | 拉取 tmux pane 历史 |
| `window create <group> <name>` | 在指定 group 里创建 tmux window |
| `window del <group> <window>` | 删除指定 group 里的 tmux window |
| `window list [<group>]` | 列出 tmux window（可限定 group） |
| `window select <group> <window>` | 切换指定 group 里的 tmux window |
| `group list` | 列出 window group |
| `ls pane` / `ls window [<group>]` / `ls group` | 列出 pane / window / group |
| `sync` | 按 tmux 服务端现状同步本地结构 |
| `list` | 列出所有 pane |
| `help` | 打印简洁命令清单 |
| `daemon serve\|status\|stop` | 管理常驻 daemon |

## 会话与 pane（tml）

### tml add

`tml` 是 `terminal` 的简写。`add` 按目标 group 分两种行为：

```text
xs tml add [--url <URL>] [--group <GROUP>] [--new-window | --sph <ID> | --spv <ID>]
```

- `--group <GROUP>`：目标 window group（group 名或数字 GroupId），缺省为默认 group。

默认 group（放本地 shell/ssh/wsl 会话）：

- `--url <URL>` 必填，描述 shell/ssh/wsl 对话。
- `--new-window`：在新窗口创建（默认）。
- `--sph <ID>` / `--spv <ID>`：拆分指定本地 pane 后创建。

tmux session group：

- `--url` 忽略（新 window/pane 使用 tmux 默认 shell）。
- `--new-window`：在会话内 `new-window`（默认）。
- `--sph <ID>` / `--spv <ID>`：`split-window` 拆分指定 tmux pane（`<ID>` 为该 pane 的本地 PaneId）。

URL scheme：

| scheme | 形式 | 查询参数 |
|---|---|---|
| `shell` | `shell://<program>?cwd=<path>&cols=<n>&rows=<n>&args=a,b` | `cwd`、`cols`、`rows`、`args`（逗号分隔） |
| `ssh` | `ssh://<user@host[:port]>?identity=<path>&password=<pwd>&cols=<n>&rows=<n>` | `identity`、`password`、`cols`、`rows` |
| `wsl` | `wsl://?args=a,b&cols=<n>&rows=<n>` | `args`、`cols`、`rows` |

- `cols` / `rows` 缺省 80x24；`args` 缺省为空；`cwd` 缺省为当前目录。
- `ssh` 认证：优先用 URL 里的 `password`；其次用 `identity` 密钥；两者都没有时在客户端边缘提示密码。
- 注意：`password` 写在 URL 里会出现在进程参数和 shell 历史中，生产环境建议改用 `XSCORE_SSH_PASSWORD` / `PASSWD` 环境变量或交互提示。

示例：

```text
xs tml add --url 'shell://powershell?cwd=/home/me/src&cols=120&rows=40' --new-window
xs tml add --url 'shell://bash?cols=100&rows=30' --spv 3
xs tml add --url 'ssh://dev@box?identity=~/.ssh/id_ed25519'
xs tml add --url 'ssh://dev@box?password=secret'
xs tml add --url 'wsl://?args=--distribution,Ubuntu'
xs tml add --group work --new-window
xs tml add --group work --sph 3
```

### tml del

```text
xs tml del <pane-id>
```

删除指定 pane（tmux pane 走 `kill-pane`；本地/SSH pane 关闭连接）。

```text
xs tml del 3
```

## tmux 会话

### tmx

`tmx` 是 `tmux` 的简写，用 URL 统一描述 tmux 目标与远端主机：

```text
xs tmx attach --url <tmux-url> [--ssh <ssh-url> | --wsl <wsl-url>]
```

- `--url tmux://<socket>/<session>`：
  - `tmux://<session>`：默认 socket。
  - `tmux://<name>/<session>`：命名 socket（`-L <name>`）。
  - `tmux:///<path>/<session>`：socket 路径（`-S <path>`，path 含 `/`）。
- `--ssh <ssh-url>`：`ssh://user@host[:port]?identity=<path>&password=<pwd>`，透传为
  `ssh -i <identity> -p <port> user@host`。
- `--wsl <wsl-url>`：`wsl://?args=--distribution,Ubuntu`，透传为 `wsl.exe` 参数。

示例：

```text
xs tmx attach --url tmux://SOCKET/work --ssh ssh://dev@box?identity=~/.ssh/id_ed25519
xs tmx attach --url tmux://work
xs tmx attach --url tmux://x/work --wsl 'wsl://?args=--distribution,Ubuntu'
xs tmx attach --url tmux:///tmp/tmux-0/default/work
```

## 布局与焦点

### split

```text
xs split h|v <program> [args...]
```

- `h` = 水平拆分，`v` = 垂直拆分；在聚焦 pane 上拆出新本地 pane。

### focus

```text
xs focus <pane-id>
```

本地 pane 切换焦点；tmux pane 走容器内焦点。

### layout

```text
xs layout
```

打印布局树与当前焦点。

## pane 操作

### write

```text
xs write <pane-id> <text>...
```

把 `text` 用空格拼接后，补一个换行发给 pane。Windows 本地 shell 使用 `\r\n`，SSH/tmux 使用
`\n`。

### read

```text
xs read <pane-id>
```

打印该 pane 当前渲染出的屏幕内容。脚本化输入下有一个 60ms 静默窗口，极端时序可能读到旧屏。

### resize

```text
xs resize <pane-id> <cols> <rows>
```

### list

```text
xs list
```

列出所有 pane（PANE / KIND / GROUP / WINDOW / COLS / ROWS）。

### pane del

```text
xs pane del <pane-id>
```

删除 pane：tmux pane 走 `kill-pane`；本地/SSH pane 关闭连接。

### pane fetch

```text
xs pane fetch <pane-id> [--line <N>]
```

拉取 tmux pane 的滚动历史（`capture-pane`）：不带 `--line` 拉全部，`--line <N>` 拉最近 N 行。
仅对 tmux pane 有效。

## window 操作

window 现在按 group 作用域：一个 tmux session 对应一个 window group，默认 group 放本地
shell/ssh/wsl 会话。`<group>` 可以用 group 名（session 名或 `default`）或数字 GroupId。

window 标识统一编码为 `Gxx-Tyy` / `Gxx-Myy`：`xx` 是 group id，`yy` 是该 group 内的
window 序号，`T` 表示 tmux window，`M` 表示 shell/ssh/wsl window。

### window create

```text
xs window create <group> <name>
```

在指定 group（tmux session）里创建新 window（`new-window`）。

### window del

```text
xs window del <group> <window>
```

删除指定 group 里的 tmux window（`kill-window`）。

### window list

```text
xs window list [<group>]
```

列出 tmux window（WINDOW / GROUP / ACTIVE / PANES）；带 `<group>` 时只列该 group。

### window select

```text
xs window select <group> <window>
```

切换到指定 group 里的 tmux window（`select-window`）。

### group list

```text
xs group list
```

列出所有 window group（GROUP / NAME / KIND / PANES）。

## 同步

### sync

```text
xs sync
```

按 tmux 服务端现状 reconcile 本地结构：删除服务端已消失的 pane/window，纳入服务端新增的
pane/window。

## daemon 生命周期

```text
xs daemon serve    # 前台运行 daemon（一次性命令会自动拉起）
xs daemon status   # 查看 daemon 当前持有的 pane / tmux session
xs daemon stop     # 让 daemon 退出
```

## help

```text
xs help
```

打印简洁命令清单（与 `xs --help` 的 clap 帮助不同）。

## 环境变量

| 变量 | 用途 |
|---|---|
| `XSCORE_LOG` | 日志文件路径，默认 `emulator.log` |
| `RUST_LOG` | 日志级别，默认 `emulator=debug,emulator_frontend=debug` |
| `XSCORE_SSH_PASSWORD` / `PASSWD` | SSH 密码（优先于交互提示） |
| `XSCORE_SSH_HOST` | 集成测试目标主机 |
| `XSCORE_ASKPASS` | 内部：前端二进制作为 askpass 时输出密码 |

## 平台限制

- Windows 原生不支持本地 tmux；本地 tmux 需在 Linux/WSL 内运行。
- WSL tmux 需要 WSL 内安装 tmux。
- 远端 tmux 密码认证需要 OpenSSH 8.4+（`SSH_ASKPASS_REQUIRE`）。
