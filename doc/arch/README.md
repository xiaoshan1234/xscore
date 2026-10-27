# xscore Architecture Views

本文档集按 4+1 视图组织：

| 视图 | 文档 | 关注点 |
|---|---|---|
| 逻辑视图 | [view-logic.md](./view-logic.md) | 实体、职责、约束、关键协作 |
| 开发视图 | [view-dev.md](./view-dev.md) | 模块、目录、依赖、构建与测试 |
| 运行视图 | [view-process.md](./view-process.md) | 进程、任务、数据流、并发与故障 |
| 部署视图 | [view-deploy.md](./view-deploy.md) | Linux/WSL/Windows、远端 tmux、文件与 IPC |
| 场景与详细设计 | [arch.md](./arch.md) | 详细模型、命令面、历史演进和决策链接 |

推荐阅读顺序：

1. 先读逻辑视图理解 Session/Window/Pane/Buffer/Link。
2. 再读开发视图理解源码边界。
3. 运行视图解释 daemon、PTY、tmux 和数据流。
4. 部署视图解释实际安装形态和平台限制。
