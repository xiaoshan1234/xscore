# 开发视图

本文描述当前源码的模块划分、依赖方向、构建与测试。逻辑职责见
[view-logic.md](./view-logic.md)，权威设计见 [terminal-emulator.md](./terminal-emulator.md)。

## 1. 构建单元

```text
crates/
  emulator/              后端库（L0–L2）
  emulator-frontend/     CLI 前端（L3–L5 模拟），含 askpass 角色
  xscore/                前身 CLI/daemon，已过时，保留待清理
```

## 2. 源码结构

```text
crates/emulator/src/
  platform/
    mod.rs                LocalPty（portable-pty）
    ssh.rs                SshConnection（russh）
    tmux/
      mod.rs
      model.rs            TmuxHostConfig / TmuxSocket
      protocol.rs         控制模式协议解析（纯函数）
      format.rs           -F 输出解析、send-keys、host_prefix
      control.rs          TmuxControl 进程 + reader/writer 线程
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

crates/emulator-frontend/src/
  main.rs                 REPL、命令解析、askpass 入口、日志初始化
  layout.rs               Node / TmuxContainer / Layout
```

## 3. 模块依赖

```text
emulator-frontend → emulator
emulator: platform ← domain ← term
```

- `platform` 只依赖 OS 原语与 tmux 协议，不依赖 domain/term。
- `domain` 依赖 `platform`；tmux 协议细节封装在 `platform/tmux`。
- `term` 是纯逻辑层，不依赖 platform/domain。
- 前端依赖 `emulator`，组合 Domain、TerminalCore 与布局树。

## 4. 关键接口

- `PaneHandle`：`id` / `kind` / `as_any` / `send_input` / `resize` / `subscribe` /
  `close`。`as_any` 供前端 downcast 到 `TmuxPane` 以发送 tmux 命令。
- `TerminalCore`：`feed` / `snapshot` / `resize`。
- `TmuxControl`：`spawn` / `send` / `subscribe` / `output` / `close`，命令按
  `%begin` 编号关联回复。

## 5. 构建与测试

```bash
cargo build --workspace
cargo test -p emulator -p emulator-frontend
cargo clippy -p emulator -p emulator-frontend --all-targets -- -D warnings
cargo fmt --all -- --check
```

测试：

| 测试 | 目的 |
|---|---|
| emulator unit | tmux 协议/格式、VT 核心、argv |
| `local_domain` | 本地 PTY 端到端 roundtrip |
| layout unit | 布局树拆分、删除、镜像焦点 |
| `tmux_ssh`（`#[ignore]`） | 真实 SSH tmux：attach/split/kill/枚举 |

## 6. 扩展点

- 新 Domain：实现 `Domain`/`PaneHandle`，在 `domain/` 增加模块。
- 新传输后端：在 `platform/` 增加适配器。
- 新渲染：替换 CLI 前端的文本快照，消费 `TerminalCore::snapshot` 与布局树。
- 真前端：保留 `emulator` 不变，替换 `emulator-frontend`。
