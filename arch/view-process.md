# 运行视图

本文描述进程、线程/异步任务、数据流、并发与故障。部署拓扑见
[view-deploy.md](./view-deploy.md)。

## 1. 运行单元

用户命令由轻量 client 进程发送给常驻 daemon；daemon 内运行后端库与若干子进程：

```text
xs client
  clap 命令解析
  TCP 请求/响应

xs-daemon
  per-pane TerminalCore 泵任务
  per-logger 文件写入任务
  └─ 子进程：
     本地 Shell / wsl.exe（ConPTY）
   tmux-over-SSH 直接使用进程内 russh channel
```

## 2. 线程模型

- `LocalPty`：reader / writer 两个 OS 线程，异步侧经无界 channel。
- `SshConnection`：单任务持有 SSH channel，通过命令队列串行处理 read/write/resize/
  close，避免超时取消读取导致后续消息丢失。
- `TmuxControl`：通过 `TmuxTransport` 写入和关闭连接；Local/WSL 使用
  PTY reader/writer 任务，SSH 使用 russh exec channel 的异步读写任务。
- `LoggerRegistry`：每个 logger 独立订阅 `PaneHandle` 输出，raw 写原始字节，
  char 过滤 ANSI/控制序列后按带时间戳的文本行写入本地文件。

## 3. 数据流

### 输出

```text
PTY/SSH/tmux → Domain 路由 → PaneHandle 输出流 → TerminalCore.feed → snapshot
```

### 输入

```text
xs client → xs-daemon → PaneHandle.send_input → Domain 封装 → PTY/SSH/send-keys
```

### tmux 控制

```text
send(command) → TmuxTransport → PTY stdin / SSH channel
reader 任务 → ProtocolParser → %begin/%end 关联回复 / %output 按 %N 路由
```

## 4. 并发与同步

| 同步点 | 保护对象 |
|---|---|
| `TmuxControl.shared` | waiter 队列、事件订阅、`%N` 输出订阅、closed |
| `SshConnection.commands` | 单 channel 任务串行处理 read/write/resize/close |
| `App.panes` | pane 句柄与 TerminalCore |
| `App.layout` | 本地布局树与焦点 |

## 5. 日志

- 默认写 `emulator.log`（`XSCORE_LOG` 改路径），追加写入。
- 控制台仅 WARN 以上；debug/trace 进文件。
- 级别默认 `emulator=debug,xscore_server=debug`，`RUST_LOG` 覆盖。

## 6. 故障与恢复

| 故障 | 行为 |
|---|---|
| 本地 Shell 退出 | PaneHandle 输出流关闭，网格历史保留 |
| SSH 断开 | `SshConnection` 报错，pane 标记断开 |
| tmux `%exit` | reader 标记 closed，控制连接结束 |
| tmux server 删除 pane/window | 用 `tmx sync <group-id>` 按需重枚举指定 group 并收敛 |
| 密码错误 | ssh 认证失败报错（stderr 继承到控制台） |
