# AGENTS.md

本文件供在本仓库工作的 AI 编码代理阅读。先读它，再读 `arch/` 下的架构文档。

## 项目是什么

xscore 是一个终端模拟器：原生支持 `tmux -CC` 控制模式、普通 SSH、本地/远端 Shell，
多 pane 分屏。当前交付的是后端（L0–L1）加一个 CLI 模式前端（L2–L5 模拟），尚无
图形界面。

权威设计：[arch/terminal-emulator.md](arch/terminal-emulator.md) 与
[arch/README.md](arch/README.md) 索引的 4 个视图。

## 构建与测试

```bash
cargo build --workspace
cargo test -p xscore -p xsc-protocol -p xsc-client -p xsc-server
cargo clippy -p xscore -p xsc-protocol -p xsc-client -p xsc-server --all-targets -- -D warnings
cargo fmt --all -- --check
```

运行 CLI 前端：

```bash
cargo install --path crates/xsc-server       # 安装 xs-daemon 到 ~/.cargo/bin
cargo install --path crates/xsc-client       # 安装 xs 到 ~/.cargo/bin
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

脚本会运行全部集成测试，并在 `test-results/` 生成
Markdown、JSON、JUnit XML 和逐套件日志。

## 架构与模块

依赖单向：client/server → 共享协议；server → 后端库 → 平台。client 不依赖后端库。

```text
crates/xscore/            L0–L1 连接/传输后端库
  platform/                 L0：PTY/ConPTY、SSH（russh）、tmux 控制客户端
  domain/                   L1：Domain / TerminalHandle、Local/Ssh/Tmux 三种域

crates/xsc-protocol/      client 与 server 共享的请求/响应、IPC 编解码和端口
crates/xsc-client/        `xs` 命令客户端：仅转发 argv、边缘密码提示、自动拉起 daemon、RPC
crates/xsc-server/        `xs-daemon` 常驻服务端：clap 解析、命令分发、App、pane、layout、term、logger、tmux
  src/main.rs                daemon 入口
  src/server/                回环 TCP：accept、协议握手与请求分发
  src/cli/                   命令输入：clap 解析、命令模型、URL 解析、命令映射
  src/app/                   应用状态：App、TerminalRegistry、window/group 标识
  src/commands/              命令分发与处理器
  src/layout/                布局树
  src/render/                文本渲染
  src/term/                  VT 状态机 + 网格（VtTerm/Grid/TerminalCore）
  src/logging/               进程日志初始化与 pane logger
  src/tmux/                  tmux 会话协调（events 差集 / session reconcile）

```

命名规范见 [rules/rust-name-rules.md](rules/rust-name-rules.md)：函数/变量
`snake_case`、类型/枚举/枚举变体 `UpperCamelCase`、布尔 `is_/has_/should_/can_` 前缀。

## 对外接口文档

- 跨文件可见的 `pub` / `pub(crate)` 类型、字段、枚举变体、常量和函数必须有
  `///` 注释。
- 函数和方法注释必须写明作用、参数方向（输入、输出或输入输出）和返回值/异步输出。
- `xscore` 库启用 `missing_docs`；新增 crate 外公共接口缺失注释会触发编译警告。

## 必须知道的技术坑

- **一次性 CLI 走常驻 daemon**：`xs <command>` 通过本地回环 TCP 把命令发给独立的
  `xs-daemon`，live 连接与 pane 状态才不随 CLI 进程退出而丢失。daemon 按需
  自动拉起；客户端只做边缘密码提示，命令解析与分发都在 daemon 内完成。
- **tmux 用 `-C`，不是 `-CC`**：`-CC` 会 `tcgetattr`，在管道/部分 tty 场景下卡死。
- **tmux `-F` 分隔符用 `|`**：tab 会被展开成 `ESC[NC`，控制字符会被转义成 `\ooo`，
  可打印的 `|` 才能原样输出。
- **tmux 控制客户端通过 `TmuxTransport` 统一传输**：
  - SSH：russh exec channel 直接运行 `tmux -C attach`，密钥或密码直接认证。
  - Local / WSL：portable-pty 的 ConPTY（`wsl.exe` 需要独立控制台，避免抢宿主输入）。
- **Windows ConPTY 输入回车用 `\r\n`**，Unix 用 `\n`。本地 shell 的 `write` 按
  `TerminalKind::Local` 区分；SSH/tmux 仍是 `\n`（Unix 语义）。
- **一个 tmux window 独占全屏**：`layout` 只放当前（active）window 的镜像容器，
  其余 window 只登记数据不进布局；服务端 `%session-window-changed` 会触发
  reconcile 自动跟随。
- **`%output` 不回放历史**：attach 只接收订阅之后的新字节；历史用
  `tml fetch <id> [--line N]`（`capture-pane`）按需查询。
- **Pane logger 独立订阅**：`tml log` 为每个 logger 建立独立 broadcast receiver；
  raw 按字节封顶，char 过滤 ANSI/控制序列后按带时间戳的行数封顶。
- **结构事件**：`%output %N` 和 `%exit` 已被消费；`%window-add`/`%window-close`/
  `%layout-change` 等走 `classify_tmux_event` → reconcile。

## SSH 认证

- SSH shell（russh）：密钥（`identity` 路径）或密码（无回显交互提示）。
- tmux-over-ssh（russh）：密钥（`identity` 路径）或密码直接认证，不读取
  `~/.ssh/config`、ssh-agent 或 ProxyJump。

## 环境变量与文件

| 名称 | 用途 |
|---|---|
| `XSCORE_LOG` | 日志文件路径，默认 `xscore.log` |
| `RUST_LOG` | 日志级别，默认 `xscore=debug,xsc_server=debug` |
| `XSCORE_DAEMON_EXE` | 可选的 `xs-daemon` 可执行文件路径覆盖 |

## 平台限制

- Windows 原生不支持本地 tmux；本地 tmux 需在 Linux/WSL 内运行。
- WSL tmux 需要 WSL 内安装 tmux。
- 远端 tmux 使用应用内 russh，不读取系统 OpenSSH 配置或 agent。
