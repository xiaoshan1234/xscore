# xscore

终端模拟器：原生支持 tmux 控制模式，把 tmux 的 window/pane 渲染成原生 UI
（标签页、分屏），并与普通 SSH、本地 Shell 会话在同一窗口并存。

- `crates/xscore` 是核心可复用库，提供 L0–L1（平台抽象 / 域与连接）。
- `xss` 是 CLI 示例前端，承载 L2–L5（终端模拟核心 / 组合窗格树 / 渲染 / 应用协调）。
- `xsc` 是轻量命令客户端示例，通过本地协议驱动 daemon。

默认 workspace 成员只有 `xscore`，因此根目录执行 `cargo build` 时只构建核心库。
需要构建示例前端时显式使用 `--workspace`。

## 文档

- [在其他项目中导入 xscore](crates/xscore/README.md)
- [终端模拟器架构设计](arch/terminal-emulator.md) —— 权威核心文档（L0–L5）
- [逻辑视图](arch/view-logic.md)
- [开发视图](arch/view-dev.md)
- [运行视图](arch/view-process.md)
- [部署视图](arch/view-deploy.md)
- [架构视图索引](arch/README.md)

## 构建与测试

```bash
cargo build
cargo build --workspace
cargo test --workspace --all-targets -- --test-threads=1
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

## 许可证

MIT（见 `LICENSE`）。
