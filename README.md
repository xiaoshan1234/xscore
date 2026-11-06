# xscore

终端模拟器：原生支持 `tmux -CC` 控制模式，把 tmux 的 window/pane 渲染成原生
UI（标签页、分屏），并与普通 SSH、本地 Shell 会话在同一窗口并存。

- 后端 L0–L2（平台抽象 / 域与连接 / 终端模拟核心）——本项目实现。
- `xs-daemon` 承载 L3–L5（组合窗格树 / 渲染 / 应用协调）。
- `xs` 是轻量命令客户端，通过本地协议驱动 daemon。

> 状态：后端 L0–L2 与 CLI 模式前端已经落地；前端由 `xs` client 和 `xs-daemon`
> server 两个进程组成。

## 文档

- [终端模拟器架构设计](arch/terminal-emulator.md) —— 权威核心文档（L0–L5）
- [逻辑视图](arch/view-logic.md)
- [开发视图](arch/view-dev.md)
- [运行视图](arch/view-process.md)
- [部署视图](arch/view-deploy.md)
- [架构视图索引](arch/README.md)

## 构建与测试

```bash
cargo build --workspace
cargo test --workspace --all-targets -- --test-threads=1
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

## 许可证

MIT（见 `LICENSE`）。
