# AGENTS.md

本文件供在本仓库工作的 AI 编码代理阅读。先读它，再读 `arch/` 下的架构文档。

## 项目是什么

xscore 是一个终端模拟器：原生支持 `tmux -CC` 控制模式、普通 SSH、本地/远端 Shell，
多 pane 分屏。当前交付的是后端（L0–L2）加一个 CLI 模式前端（L3–L5 模拟），尚无
图形界面。

权威设计：[arch/terminal-emulator.md](arch/terminal-emulator.md) 与
[arch/README.md](arch/README.md) 索引的 4 个视图。

## 构建与测试

```bash
cargo build --workspace
cargo test -p emulator -p emulator-frontend
cargo clippy -p emulator -p emulator-frontend --all-targets -- -D warnings
cargo fmt --all -- --check
```

运行 CLI 前端：

```bash
cargo install --path crates/emulator-frontend   # 安装 xs 到 ~/.cargo/bin
xs ls pane                                       # 列出当前 pane
```

集成测试（本地 PTY、SSH shell、远端 tmux、前端 tmux）从根目录 `test.config`
读取连接配置，格式见 `test-example.config`：

```text
ssh_host=user@host
ssh_password=...
```

```powershell
python scripts/run_integration_tests.py
```

脚本会构建前端 askpass helper，运行全部集成测试，并在 `test-results/` 生成
Markdown、JSON、JUnit XML 和逐套件日志。

## 架构与模块

依赖单向：前端 → 后端库 → 平台。

```text
crates/emulator/            L0–L2 后端库
  platform/                 L0：PTY/ConPTY、SSH（russh）、tmux 控制客户端
  domain/                   L1：Domain / PaneHandle、Local/Ssh/Tmux 三种域
  term/                     L2：VT 状态机 + 网格（Grid/VtTerm）

crates/emulator-frontend/   L3–L5 CLI 前端
  src/main.rs               入口：askpass 角色、日志、模式路由（一次性 / daemon）
  src/cli/                  clap 命令定义（cmd）+ 参数/URL 解析（parse）+ 密码边缘解析（secrets）
  src/ui/                   应用核心（GUI 可复用）
    app.rs                  App 状态：pane 注册表、布局、window group、tmux 会话
    pane.rs                 PaneRegistry：PaneHandle ↔ TerminalCore 绑定 + 字节泵
    layout.rs               L3 布局树（Node / TmuxContainer / PaneRect）
    render.rs               L4 文本渲染（快照 / 表格 / 布局树 / 状态）
    window_group.rs         WindowGroup + GroupId（默认组 / tmux session 组）
    commands/               L5 命令分发与各命令处理器
    tmux/                   tmux 会话协调（events 差集 / session reconcile）
  src/daemon/               常驻 daemon：ipc / protocol / client / server
  src/logging.rs            日志初始化（文件 + stderr WARN）

crates/xscore/              前身 CLI/daemon，已废弃，待清理
```

命名规范见 [rules/rust-name-rules.md](rules/rust-name-rules.md)：函数/变量
`snake_case`、类型/枚举/枚举变体 `UpperCamelCase`、布尔 `is_/has_/should_/can_` 前缀。

## 必须知道的技术坑

- **一次性 CLI 走常驻 daemon**：`xs <command>` 通过本地回环 TCP 把命令发给
  `xs daemon serve`，live 连接与 pane 状态才不随 CLI 进程退出而丢失。daemon 按需
  自动拉起；密码在客户端解析后随命令传入。
- **tmux 用 `-C`，不是 `-CC`**：`-CC` 会 `tcgetattr`，在管道/部分 tty 场景下卡死。
- **tmux `-F` 分隔符用 `|`**：tab 会被展开成 `ESC[NC`，控制字符会被转义成 `\ooo`，
  可打印的 `|` 才能原样输出。
- **tmux 控制客户端两种 spawn**：
  - SSH：普通管道进程 + `SSH_ASKPASS`（前端二进制自己兼任 askpass，见
    `XSCORE_ASKPASS`）。
  - Local / WSL：portable-pty 的 ConPTY（`wsl.exe` 需要独立控制台，避免抢宿主输入）。
- **Windows ConPTY 输入回车用 `\r\n`**，Unix 用 `\n`。本地 shell 的 `write` 按
  `PaneKind::Local` 区分；SSH/tmux 仍是 `\n`（Unix 语义）。
- **一个 tmux window 独占全屏**：`layout` 只放当前（active）window 的镜像容器，
  其余 window 只登记数据不进布局；服务端 `%session-window-changed` 会触发
  reconcile 自动跟随。
- **`%output` 不回放历史**：attach 只接收订阅之后的新字节；历史用
  `tml fetch <id> [--line N]`（`capture-pane`）按需查询。
- **结构事件**：`%output %N` 和 `%exit` 已被消费；`%window-add`/`%window-close`/
  `%layout-change` 等走 `classify_tmux_event` → reconcile。

## SSH 认证

- SSH shell（russh）：密钥（`identity` 路径）或密码（无回显交互提示）。
- tmux-over-ssh（系统 `ssh`）：`--password` 用 askpass，密钥则复用
  `~/.ssh/config` 与 agent。

## 环境变量与文件

| 名称 | 用途 |
|---|---|
| `XSCORE_LOG` | 日志文件路径，默认 `emulator.log` |
| `RUST_LOG` | 日志级别，默认 `emulator=debug,emulator_frontend=debug` |
| `XSCORE_SSH_PASSWORD` / `PASSWD` | SSH 密码（优先于交互提示） |
| `XSCORE_ASKPASS` | 内部：前端二进制作为 askpass 时输出密码 |

## 平台限制

- Windows 原生不支持本地 tmux；本地 tmux 需在 Linux/WSL 内运行。
- WSL tmux 需要 WSL 内安装 tmux。
- 远端 tmux 密码认证需要 OpenSSH 8.4+（`SSH_ASKPASS_REQUIRE`）。
