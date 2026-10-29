# 终端模拟器后端实现计划

权威架构见 [terminal-emulator.md](../arch/terminal-emulator.md) 与 4 个视图。范围：
后端 L0–L2 为实施对象，前端 L3–L5 以 CLI 模式模拟（操作 + 数据读取）。

## 里程碑

| # | 里程碑 | 内容 | 验收 |
|---|---|---|---|
| M1 | 本地 Shell 通路 | L0 PTY/ConPTY、L1 Domain/PaneHandle + LocalDomain、L2 原始字节核心、CLI REPL | `shell` 后能 `write`/`read`/`resize`/`list` |
| M2 | VT 核心 | xterm 子集状态机 + 网格 + 脏区，`RawTerm` 替换为 `GridTerm` | 光标/清屏/换行等基本序列正确 |
| M3 | SSH 域 | L0 SSH 传输（russh）+ RemoteSshDomain + CLI `ssh` | 远端 PTY 可交互读写 |
| M4 | tmux 域 | 复用控制模式协议解析，TmuxDomain（1:N）+ CLI `tmux` | 多 pane 按 `%id` 路由 |
| M5 | 布局树（L3） | Node/Branch/LocalLeaf/TmuxMirror + 焦点，CLI 分屏 | 多 pane 同屏、焦点切换 |
| M6 | tmux 镜像容器 | TmuxContainer、两级布局、`%layout-change` 驱动 | tmux window 作为不可拆单元渲染 |

## 目录

```text
crates/emulator/            后端库（L0–L2）
  platform/                 L0 平台抽象
  domain/                   L1 域/连接抽象
  term/                     L2 终端核心
crates/emulator-frontend/  CLI 模式前端（L3–L5 模拟）
```

## 构建与测试

```bash
cargo build --workspace
cargo test -p emulator
cargo run -p emulator-frontend
```
