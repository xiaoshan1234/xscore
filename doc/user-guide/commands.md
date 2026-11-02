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
| `shell <program> [args...]` | 新建本地 shell pane |
| `ssh <user@host[:port]> [identity]` | 新建远程 SSH shell pane |
| `wsl [args...]` | 新建 WSL shell pane（Windows） |
| `tml create --url <URL> [...]` | 用 URL 统一创建 shell/ssh/wsl 对话 |
| `tmux <session> [...]` | attach 一个 tmux 会话 |
| `split h\|v <program> [args...]` | 在聚焦 pane 上拆分出新 pane |
| `write <pane-id> <text>...` | 向 pane 发送一行输入 |
| `read <pane-id>` | 打印 pane 当前渲染出的屏幕 |
| `resize <pane-id> <cols> <rows>` | 调整 pane 尺寸 |
| `focus <pane-id>` | 切换焦点 |
| `layout` | 打印窗口/布局树 |
| `pane del <pane-id>` | 删除 pane |
| `pane fetch <pane-id> [--line <N>]` | 拉取 tmux pane 历史 |
| `window del <handle>` | 删除 tmux window |
| `window list` | 列出 tmux window |
| `window select <handle>` | 切换 tmux window |
| `sync` | 按 tmux 服务端现状同步本地结构 |
| `list` | 列出所有 pane |
| `help` | 打印简洁命令清单 |
| `daemon serve\|status\|stop` | 管理常驻 daemon |

## 会话创建

### shell

```text
xs shell <program> [args...]
```

- `program`：可执行文件（如 `bash`、`powershell.exe`、`cmd.exe`）。
- `args...`：传给程序的参数，允许 `-` 开头的参数。
- 本地 PTY（Windows 为 ConPTY），默认尺寸 80x24。

示例：

```text
xs shell bash
xs shell powershell.exe -NoLogo
```

### ssh

```text
xs ssh <user@host[:port]> [identity]
```

- `user@host[:port]`：目标主机，端口缺省 22。
- `identity`：SSH 私钥路径（可选）。
- 未提供 `identity` 时，在客户端边缘无回显提示密码；优先读 `XSCORE_SSH_PASSWORD` / `PASSWD`。
- 默认尺寸 80x24。

示例：

```text
xs ssh dev@example.com
xs ssh dev@example.com C:/Users/me/.ssh/id_ed25519
```

### wsl

```text
xs wsl [args...]
```

- `args...` 原样传给 `wsl.exe`（Windows 专用）。

示例：

```text
xs wsl
xs wsl --distribution Ubuntu --exec /bin/bash
```

### tml create

`tml` 是 `terminal` 的简写，`create` 用 URL 统一描述 shell/ssh/wsl 对话：

```text
xs tml create --url <URL> [--new-window | --sph <ID> | --spv <ID>]
```

放置方式（三者互斥，省略时默认 `--new-window`）：

- `--new-window`：在新窗口创建（默认）。
- `--sph <ID>`：横向拆分指定 pane 后创建。
- `--spv <ID>`：纵向拆分指定 pane 后创建。

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
xs tml create --url 'shell://powershell?cwd=/home/me/src&cols=120&rows=40' --new-window
xs tml create --url 'shell://bash?cols=100&rows=30' --spv 3
xs tml create --url 'ssh://dev@box?identity=~/.ssh/id_ed25519'
xs tml create --url 'ssh://dev@box?password=secret'
xs tml create --url 'wsl://?args=--distribution,Ubuntu'
```

## tmux 会话

### tmux

```text
xs tmux <session> [options]
```

只 attach 已存在的 session，不负责创建（创建请用外部 `tmux new-session`）。

| 选项 | 说明 |
|---|---|
| `-L <NAME>` | tmux socket 名（`tmux -L <NAME>`） |
| `-S <PATH>` | tmux socket 路径（`tmux -S <PATH>`） |
| `--ssh <DEST>` | 在远端 SSH 主机上运行 tmux |
| `--wsl` | 在 WSL 内运行 tmux |
| `--password` | 密码认证（要求 `--ssh`） |
| `--ssh-arg <ARG>` | 透传给 `ssh` 的额外参数（可重复，要求 `--ssh`） |
| `--wsl-arg <ARG>` | 透传给 `wsl.exe` 的额外参数（可重复，要求 `--wsl`） |

- `-L` 与 `-S` 互斥；`--ssh` 与 `--wsl` 互斥。

示例：

```text
xs tmux work
xs tmux work --wsl
xs tmux work --ssh dev@example.com
xs tmux work --ssh dev@example.com --ssh-arg -p --ssh-arg 2222
xs tmux work --ssh dev@example.com --password
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

列出所有 pane（PANE / KIND / COLS / ROWS）。

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

### window del

```text
xs window del <handle>
```

删除 tmux window（`kill-window`）。

### window list

```text
xs window list
```

列出 tmux window（WINDOW / ACTIVE / PANES）。

### window select

```text
xs window select <handle>
```

切换到指定 tmux window（`select-window`）。

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
