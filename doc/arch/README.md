# xscore 架构视图

本文档集按 4+1 视图组织，权威模型见 [terminal-emulator.md](./terminal-emulator.md)。

| 视图 | 文档 | 关注点 |
|---|---|---|
| 核心模型 | [terminal-emulator.md](./terminal-emulator.md) | L0–L5 分层、Domain/PaneHandle、TmuxContainer、硬约束与权衡 |
| 逻辑视图 | [view-logic.md](./view-logic.md) | 实体、职责、协作与不变量 |
| 开发视图 | [view-dev.md](./view-dev.md) | 模块、目录、依赖、构建与测试 |
| 运行视图 | [view-process.md](./view-process.md) | 进程、任务、数据流、并发与故障 |
| 部署视图 | [view-deploy.md](./view-deploy.md) | Linux/WSL/Windows、远端 tmux/SSH |

推荐阅读顺序：

1. 先读核心模型理解 L0–L5 分层与硬约束。
2. 再读逻辑视图理解实体、职责与不变量。
3. 开发视图理解源码边界、依赖与扩展点。
4. 运行视图理解数据流、并发与生命周期。
5. 部署视图理解平台形态与限制。
