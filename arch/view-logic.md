# 逻辑视图

本文描述终端模拟器的核心实体、职责边界、协作关系与不变量。权威设计见
[terminal-emulator.md](./terminal-emulator.md)，本文反映当前实现状态。

## 1. 目标

后端实现 L0–L2（平台抽象、域/连接、终端核心），前端以 CLI 模式模拟 L3–L5（布局
树、渲染、协调）。核心是把“连接”与“窗格”拆开：上层只消费 `PaneHandle`，连接基数
差异封装在域内。

## 2. 角色与外部系统

| 参与者 | 作用 |
|---|---|
| 用户 | 通过 `xs` 一次性命令操作窗格、读取屏幕 |
| `xs` client | argv 转发、边缘密码提示、daemon 生命周期与 IPC |
| `xs-daemon`（L3–L5，CLI 模拟） | 布局树、焦点、命令分发、结构同步 |
| 后端（L0–L2） | 平台 I/O、Domain 路由、VT 状态机 |
| PTY 子进程 | 本地 Shell、`tmux -C attach`、`wsl.exe` |
| SSH server | RemoteSshDomain 与远端 tmux 的通道 |
| tmux server | TmuxDomain 的结构与字节事实来源 |

## 3. 核心实体

| 实体 | 所有者 | 说明 |
|---|---|---|
| `Connection` | L1 Domain | 一个实际 I/O 通道（PTY / SSH channel / tmux 控制连接） |
| `PaneHandle` | L1 Domain | 逻辑窗格句柄：输入路由 + 输出流 + 生命周期 |
| `Domain` | L5 前端 | PaneHandle 工厂；Local/Ssh/Tmux 三种实现 |
| `TerminalCore` | L3 叶子 | 每 pane 独立的 VT 状态机 + 网格 |
| `Node` | L3 布局 | Leaf / Branch / TmuxMirror |
| `TmuxContainer` | L3 布局 | tmux window 的不可拆分布局原子，内部 pane 私有 |
| `Layout` | L3 布局 | 本地分屏树 + 焦点 |

## 4. 层与职责

| 层 | 负责 | 不负责 |
|---|---|---|
| L0 平台 | PTY/ConPTY、SSH 传输、tmux 控制进程与协议解析 | 业务语义、布局 |
| L1 域/连接 | Connection 抽象、PaneHandle 工厂/路由、tmux 枚举 | VT 解析、布局几何 |
| L2 终端核心 | 字节 → 网格、VT 状态机、脏区 | 连接类型、布局 |
| L3 布局 | 本地分屏树 + tmux 镜像容器、两级焦点 | I/O、协议、渲染 |
| L4 渲染（模拟） | 文本网格快照 | 持有 PaneHandle、协议 |
| L5 协调（模拟） | 命令分发、DomainRegistry、结构同步 | 协议/VT/几何计算 |

## 5. 三种 Domain 语义

### LocalDomain（1:1）

一个 pane 对应一个本地 PTY/ConPTY 子进程；输入直写 PTY，输出即 PTY 读流，resize
直调 PTY 尺寸。

### RemoteSshDomain（1:1）

一个 pane 对应一条 russh SSH channel；支持密钥或密码认证，`request_pty` +
`request_shell`，resize 转 `window-change`。

### TmuxDomain（1:N）

一个 session 对应一个 `tmux -C attach` 控制连接；枚举 window/pane，按 `%N` 路由
`%output`，输入走 `send-keys`，resize 走 `resize-pane`。

## 6. 必须保持的不变量

1. 依赖严格单向：前端 → 后端 → 平台，下层不感知上层。
2. L3 只认识 `PaneHandle`，不出现 SSH/tmux 分支。
3. tmux 内部 pane 只存在于 `TmuxContainer.panes`，不出现在本地 `Node` 枚举。
4. 每个 PaneHandle 绑定独立 `TerminalCore`，状态不共享。
5. `%output` 解码后原样直送 L2，不经字符渲染。
6. tmux 结构以“按需重枚举”为准：`tmx sync <group-id>` 对指定 group 双向收敛
   （删已消失、加新出现）。

## 7. 关键场景

### 输出路径

```text
PTY / SSH channel / tmux 控制流
  → L1 路由（tmux 按 %N 查表，其余直通）
  → PaneHandle 输出流
  → L2 该 pane 独立 TerminalCore
  → 网格快照
```

### 输入路径

```text
xs 命令
  → 目标 pane 的 PaneHandle.send_input
  → L1 封装（PTY 直写 / SSH channel / send-keys -t %N）
  → L0 传输
```

### tmux 结构同步

```text
tmx sync <group-id> 命令
  → 解析目标 group 对应的 tmux session
  → 枚举该 session 的 window/pane
  → 与本地 %N 差集
  → 删除已消失的 pane，新建新出现的 pane，更新 TmuxContainer
```

## 8. 非目标

- 不讨论渲染后端选型、配置格式、插件系统。
- 不在运行时做“tmux pane 不与本地窗格并排”的校验——由节点类型保证。
