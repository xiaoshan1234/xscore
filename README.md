# xscore

终端模拟器：原生支持 `tmux -CC` 控制模式，把 tmux 的 window/pane 渲染成原生
UI（标签页、分屏），并与普通 SSH、本地 Shell 会话在同一窗口并存。

- 后端 L0–L2（平台抽象 / 域与连接 / 终端模拟核心）——本项目实现。
- 前端 L3–L5（组合窗格树 / 渲染 / 应用协调）——当前以模拟/桩方式验证后端。

> 状态：架构设计已完成，后端按 L0–L2 逐步落地。当前仓库代码为前身 CLI 实现，
> 文档以终端模拟器架构为准。

## 文档

- [终端模拟器架构设计](doc/arch/terminal-emulator.md) —— 权威核心文档（L0–L5）
- [逻辑视图](doc/arch/view-logic.md)
- [开发视图](doc/arch/view-dev.md)
- [运行视图](doc/arch/view-process.md)
- [部署视图](doc/arch/view-deploy.md)
- [架构视图索引](doc/arch/README.md)

## 构建与测试

```bash
cargo build --release
cargo test --workspace --all-targets -- --test-threads=1
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

## 许可证

MIT（见 `LICENSE`）。
