# 运行视图

本文描述进程、线程/异步任务、数据流、并发与故障。部署拓扑见
[view-deploy.md](./view-deploy.md)。

## 1. 运行单元

单个宿主进程（CLI 前端）内运行后端库与若干子进程：

```text
宿主进程
  REPL（阻塞读 stdin）
  per-pane TerminalCore 泵任务
  └─ 子进程：
     本地 Shell / wsl.exe（ConPTY）
     ssh（tmux 控制客户端，管道）
```

## 2. 线程模型

- `LocalPty`：reader / writer 两个 OS 线程，异步侧经无界 channel。
- `SshConnection`：reader 任务用 10ms 超时轮询释放 channel 锁，读写不互饿死。
- `TmuxControl`：writer 线程写 stdin，reader 线程解析 stdout；SSH 场景用管道，
  Local/WSL 场景用 ConPTY。

## 3. 数据流

### 输出

```text
PTY/SSH/tmux → Domain 路由 → PaneHandle 输出流 → TerminalCore.feed → snapshot
```

### 输入

```text
REPL → PaneHandle.send_input → Domain 封装 → PTY/SSH/send-keys
```

### tmux 控制

```text
send(command) → writer 线程 → 子进程 stdin
reader 线程 → ProtocolParser → %begin/%end 关联回复 / %output 按 %N 路由
```

## 4. 并发与同步

| 同步点 | 保护对象 |
|---|---|
| `TmuxControl.shared` | waiter 队列、事件订阅、`%N` 输出订阅、closed |
| `SshConnection.channel` | russh channel（读用超时轮询） |
| `App.panes` | pane 句柄与 TerminalCore |
| `App.layout` | 本地布局树与焦点 |

## 5. 日志

- 默认写 `emulator.log`（`XSCORE_LOG` 改路径），追加写入。
- 控制台仅 WARN 以上；debug/trace 进文件。
- 级别默认 `emulator=debug,emulator_frontend=debug`，`RUST_LOG` 覆盖。

## 6. 故障与恢复

| 故障 | 行为 |
|---|---|
| 本地 Shell 退出 | PaneHandle 输出流关闭，网格历史保留 |
| SSH 断开 | `SshConnection` 报错，pane 标记断开 |
| tmux `%exit` | reader 标记 closed，控制连接结束 |
| tmux server 删除 pane/window | 用 `tmx sync <group-id>` 按需重枚举指定 group 并收敛 |
| 密码错误 | ssh 认证失败报错（stderr 继承到控制台） |
