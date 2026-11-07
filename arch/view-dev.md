# 开发视图

本文描述当前源码的模块划分、依赖方向、构建与测试。逻辑职责见
[view-logic.md](./view-logic.md)，权威设计见 [terminal-emulator.md](./terminal-emulator.md)。

## 1. 构建单元

```text
crates/
  xscore/              后端库（L0–L2）
  xsc-protocol/       client/server 共享协议
  xsc-client/         `xs` 命令客户端
  xsc-server/         `xs-daemon` 服务端（L3–L5 模拟）
```

## 2. 源码结构

```text
crates/xscore/src/
  platform/
    mod.rs                LocalPty（portable-pty）
    ssh.rs                SshConnection（russh）
    tmux/
      mod.rs
      model.rs            TmuxHostConfig / TmuxSocket
      protocol.rs         控制模式协议解析（纯函数）
      format.rs           -F 输出解析、send-keys、本地/WSL 进程前缀
      control.rs          TmuxControl + TmuxTransport（PTY 或 russh channel）
  domain/
    mod.rs                Domain / PaneHandle / PaneId / PaneConfig
    local.rs              LocalDomain / LocalPane
    ssh.rs                RemoteSshDomain / SshPane
    tmux.rs               TmuxDomain / TmuxPane / enumerate / make_pane
  term/
    mod.rs                TerminalCore trait
    grid.rs               网格、光标、滚动
    vte.rs                xterm 子集状态机
  error.rs
  lib.rs

crates/xsc-protocol/src/
  lib.rs                  Request/Response、IPC 编解码、端口
crates/xsc-client/src/
  main.rs                 `xs` 入口：转发 argv、打印输出
  secrets.rs              边缘密码提示（daemon 脱离终端无法自行 prompt）
  daemon.rs               daemon 自动拉起、协议协商、RPC
crates/xsc-server/src/
  main.rs                 `xs-daemon` 入口与日志初始化
  server/                 回环 TCP：accept、握手、请求分发
  cli/                    命令输入：args（clap）/ model / url / map
  app/                    应用状态：App / PaneRegistry / window/group
  commands/               命令分发与处理器
  layout/                 布局树
  render/                 文本渲染
  logging/                进程日志初始化与 pane logger
  tmux/                   tmux 会话协调
```

## 3. 模块依赖

```text
xsc-client → xsc-protocol
xsc-server → xsc-protocol
xsc-server → xscore
xscore: platform ← domain ← term
```

- `platform` 只依赖 OS 原语与 tmux 协议，不依赖 domain/term。
- `domain` 依赖 `platform`；tmux 协议细节封装在 `platform/tmux`。
- `term` 是纯逻辑层，不依赖 platform/domain。
- server 依赖 `xscore`，组合 Domain、TerminalCore 与布局树；client 不依赖二者。

## 4. 关键接口

- `PaneHandle`：`id` / `kind` / `as_any` / `send_input` / `resize` / `subscribe` /
  `close`。`as_any` 供前端 downcast 到 `TmuxPane` 以发送 tmux 命令。
- `TerminalCore`：`feed` / `snapshot` / `resize`。
- `TmuxControl`：`spawn` / `send` / `subscribe` / `output` / `close`，命令按
  `%begin` 编号关联回复。

## 5. 构建与测试

```bash
cargo build --workspace
cargo test -p xscore -p xsc-protocol -p xsc-client -p xsc-server
cargo clippy -p xscore -p xsc-protocol -p xsc-client -p xsc-server --all-targets -- -D warnings
cargo fmt --all -- --check
```

测试：

| 测试 | 目的 |
|---|---|
| xscore unit | tmux 协议/格式、VT 核心、argv |
| `local_domain` | 本地 PTY 端到端 roundtrip |
| `ssh_shell`（`#[ignore]`） | 真实 SSH shell：PTY 输入输出与 resize |
| `tmux_ssh` xscore（`#[ignore]`） | russh tmux：I/O、resize、split、kill |
| `tmux_ssh` server（`#[ignore]`） | 服务端远端 tmux 路径 |
| layout unit | 布局树拆分、删除、镜像焦点 |

集成测试统一通过 `python scripts/run_integration_tests.py` 运行，结果写入
`test-results/`。

## 6. 扩展点

- 新 Domain：实现 `Domain`/`PaneHandle`，在 `domain/` 增加模块。
- 新传输后端：在 `platform/` 增加适配器。
- 新渲染：替换 CLI 前端的文本快照，消费 `TerminalCore::snapshot` 与布局树。
- GUI 前端：保留 `xscore` 与 `xsc-server` 不变，将协议响应扩展为结构化视图 DTO。
