# 运行视图

本文描述进程、异步任务、数据流、并发控制与故障恢复。部署拓扑见
[部署视图](./view-deploy.md)，权威模型见 [terminal-emulator.md](./terminal-emulator.md)。

## 1. 运行单元

后端（L0–L2）以库形式运行在宿主进程中；前端（L3–L5，当前模拟）在同一进程内作
为驱动，负责注入事件与拉取网格快照。

```mermaid
flowchart LR
    subgraph APP["宿主进程（前端模拟 + 后端库）"]
        L5["L5 协调 / DomainRegistry / 焦点"]
        L3["L3 布局树"]
        L2["L2 每 pane TerminalCore"]
        L1["L1 Domain / PaneHandle 路由"]
        L0["L0 platform"]
    end

    subgraph EXT["外部运行时"]
        PTY["本地 Shell PTY/ConPTY"]
        SSH["远端 SSH channel"]
        TMUXPTY["tmux -CC attach 控制 PTY"]
        TMUX["tmux server"]
    end

    L5 --> L3 --> L2
    L3 --> L1 --> L0
    L0 <--> PTY
    L0 <--> SSH
    L0 <--> TMUXPTY
    TMUXPTY <--> TMUX
```

### 后端

- 每个 `Connection` 有一个 reader（PTY reader、SSH channel reader，或 tmux 控制
  流 reader），把字节交给 L1 路由。
- 1:1 域直通；TmuxDomain 的常驻 reader 按 `%pane-id` 查表路由到对应
  `output_stream`。
- 每个 PaneHandle 的 `output_stream` 喂给该 pane 独立的 L2 `TerminalCore.feed`。

### 前端模拟

- 单进程持有 L3 布局树与 L5 协调逻辑。
- 模拟驱动把脚本化的输入写进焦点 PaneHandle，并周期拉取 `GridSnapshot` 输出。
- 不启动真实窗口系统或 GPU 渲染。

## 2. 主要数据流

### 输出路径

```mermaid
sequenceDiagram
    participant IO as L0 传输
    participant D as L1 Domain
    participant PH as PaneHandle
    participant TC as L2 TerminalCore
    participant L3 as L3 布局树
    participant L4 as L4 渲染

    IO->>D: 字节（PTY / SSH / tmux 控制流）
    D->>D: tmux 域按 %pane-id 查表，其余直通
    D->>PH: output_stream
    PH->>TC: feed(bytes)
    TC->>L3: 网格脏区事件
    L3->>L4: 叶子矩形 + 脏区
    L4-->>L4: 合并重绘
```

### 输入路径

```mermaid
sequenceDiagram
    participant L5 as L5 焦点解析
    participant L3 as L3 布局树
    participant PH as PaneHandle
    participant D as L1 Domain
    participant IO as L0 传输

    L5->>L3: 当前聚焦 Node
    alt 本地叶子
        L3->>PH: send_input
    else tmux 镜像
        L3->>L3: 取容器 active_pane
        L3->>PH: send_input
    end
    PH->>D: 域内封装
    D->>IO: PTY 直写 / SSH channel / send-keys -t %id -H <hex>
```

## 3. 并发模型

- 每个 `Connection` 一个 reader 任务；读写独立，避免阻塞读饿死写。
- TmuxDomain 常驻 reader 解析协议，结构通知经事件总线单向流向 L3。
- 每个 PaneHandle 一个 L2 实例，各自维护脏区，互不共享状态。
- L3 的布局变更与渲染读取通过快照/事件同步，渲染不直接持锁长跑。

### 主要同步点

| 同步点 | 保护对象 |
|---|---|
| `DomainRegistry` | domain_id → Domain 实例 |
| TmuxDomain `pane_registry` | `%pane-id` → PaneHandle / output 路由 |
| `TmuxContainer.panes` | 容器内 pane 映射（私有，不暴露给本地树） |
| `TmuxContainer.layout_tree` | `%layout-change` 驱动的内层布局 |
| `TmuxContainer.active_pane` | `%window-pane-changed` 驱动的容器内焦点 |
| L3 本地树 | 节点增删、ratio 更新与收缩 |

## 4. 生命周期

```mermaid
stateDiagram-v2
    [*] --> Connecting: spawn_pane / attach_pane
    Connecting --> Live: 连接建立
    Live --> Live: 输入/输出/resize
    Live --> Closing: 本地关闭
    Live --> Closing: 远端断开
    Closing --> Closed: on_closed
    Closed --> [*]
```

- 本地关闭叶子：L1 发 `kill-pane`/`kill-window`（tmux）或关闭连接（本地/SSH）。
- 远端关闭 pane：`%window-close`/`%layout-change` 触发对应 PaneHandle 的
  `on_closed`，L3 收缩布局或整体移除容器。
- 域在其下所有 PaneHandle 均关闭后由 L5 回收。

## 5. 故障与恢复

| 故障 | 行为 |
|---|---|
| 本地 Shell 子进程退出 | 对应 PaneHandle `on_closed`；已收到的网格历史保留 |
| SSH 连接断开 | 对应 PaneHandle `on_closed`，连接状态标记为断开 |
| tmux control client `%exit` | 容器整体失效；tmux server 本身可继续运行，重 attach 恢复 |
| tmux server 消失 | 容器内 pane 全部 `on_closed`，本地树移除容器 |
| 尺寸竞争 | 本地只对容器整体发 resize，避免对同一 pane 的竞争写 |

## 6. 数据时效性

- tmux `%output` 是实时字节流，不重放历史；订阅前的输出不可恢复。
- L2 网格是内存状态，不承诺持久化。
- `%output` 解码后的原始字节直达 L2，保证 Sixel/OSC 8 等序列保真。
- L4 通过 `GridSnapshot` 拉取或订阅脏区，不直接触碰 I/O。
