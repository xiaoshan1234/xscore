# 终端模拟器架构设计（完整提案）

> 状态：设计提案（待评审）。
>
> 实现范围：本项目先实现后端层 L0–L2；前端层 L3–L5 以模拟/桩方式运行，
> 用于验证后端。非目标不在本文档讨论范围内。

## 0. 设计目标与硬约束

**功能目标**

1. 作为终端模拟器，原生支持 `tmux -CC` 控制模式：tmux 的 window/pane 由本
   终端以原生 UI（标签页、分屏）渲染，而非在终端内绘制字符画。
2. 支持普通 SSH 会话与本地 Shell 会话。
3. 支持将多个独立连接（如两个 SSH，或 SSH + 本地 Shell）拼接进同一个
   window 的多个分屏。
4. 支持 tmux 会话与普通连接在同一应用内并存。

**硬约束**

- **布局单元不可混排**：tmux 的一个 window 内的多个 pane，必须作为一个不可
  拆分的布局单元整体呈现。该单元的内部 pane 不得与本地 SSH/Shell 窗格成为
  本地布局树中的兄弟节点，二者不得并排。
- tmux 单元内部的分隔条与本地布局的分隔条互不越界；两套布局的边界即单元
  矩形。

**非目标**：不在本文档讨论具体渲染后端（Metal/Vulkan 等）的选型、配置格式、
插件系统。

---

## 1. 分层总览

自底向上共六层，依赖严格单向（上层依赖下层，下层不感知上层）：

| 层 | 名称 | 职责一句话 | 平台相关性 |
|---|---|---|---|
| L0 | 平台抽象层 | PTY、进程、SSH 传输、窗口尺寸等 OS 原语 | 高 |
| L1 | 域 / 连接抽象层 | 将“连接”抽象为可插拔 Domain，产出统一 PaneHandle | 中 |
| L2 | 终端模拟核心层 | 字节流 → 屏幕网格（VT 状态机） | 无 |
| L3 | 组合窗格树层 | 布局模型：本地分屏树 + tmux 镜像容器 | 无 |
| L4 | 渲染层 | 网格与布局 → 像素 | 高 |
| L5 | 应用 / 协调层 | 窗口管理、焦点、快捷键、配置、全局调度 | 无 |

**横切设施**：事件总线（Domain → 树 → 渲染的单向通知）、焦点管理器、生命周
期管理器。它们由 L5 持有并注入，不构成独立层。

**核心原则**：L3 只认识 `PaneHandle`，不认识“SSH”或“tmux”；L1 只负责 I/O
与协议，不负责布局；L2 每窗格独立实例，互不共享状态。

---

## 2. L0 平台抽象层

**职责**

- 创建/销毁 PTY（Unix `/dev/ptmx`，Windows ConPTY）。
- 启动子进程并绑定 PTY；处理 `TIOCSWINSZ` 尺寸调整与 `SIGWINCH`。
- SSH 传输：TCP 连接、认证、channel 管理、`pty-req`/`window-change` 请求。
- 提供统一的异步字节流读写接口（读线程/协程模型由本层封装）。

**接口（示意）**

```rust
trait PtyTransport {
    fn spawn(program, args, env, size) -> PtyHandle
    fn read() -> AsyncStream<Bytes>
    fn write(Bytes)
    fn resize(cols, rows)
    fn wait_exit() -> ExitStatus
}

trait SshTransport {
    fn connect(host, auth, size) -> SshChannel
    // SshChannel 提供与 PtyHandle 同形的 read/write/resize/exit
}
```

**关键点**：L0 不理解 tmux。`tmux -CC attach` 在本层看来就是“在某个 PTY 上启
动一个程序”，其控制模式协议完全由 L1 处理。

---

## 3. L1 域 / 连接抽象层

这是整个架构的枢纽。它把“连接”与“窗格”拆成两个概念，并用统一接口屏蔽三种
差异极大的后端。

### 3.1 两个核心概念

| 概念 | 含义 | 基数 |
|---|---|---|
| **Connection** | 实际 I/O 通道（一个 PTY、一个 SSH channel、一个 tmux 控制模式 PTY） | 每个 Domain 实例至少 1 个 |
| **PaneHandle** | 逻辑窗格句柄：绑定一个模拟核心 + 输入路由 + 输出事件流 | SSH/本地域为 1；tmux 域为 N |

`Domain` 的职责定义为：**PaneHandle 的工厂 + 本域内所有 PaneHandle 的注册表 +
输出路由器**。

### 3.2 统一接口

```rust
trait Domain {
    fn spawn_pane(config) -> PaneHandle       // 1:1 域：新建连接；tmux 域：复用连接并新开/附加 pane
    fn attach_pane(pane_id) -> PaneHandle     // tmux 域：将远端已存在的 pane 注册进来
    fn detach_all()                           // 断开整个域
}

trait PaneHandle {
    fn send_input(Bytes)                      // 域内负责封装
    fn resize(cols, rows)                     // 域内负责封装
    fn output_stream() -> AsyncStream<Bytes>  // 该 pane 专属字节流
    fn on_closed(callback)                    // 双向生命周期事件
    fn pane_kind() -> PaneKind                // Local | Ssh | Tmux
}
```

### 3.3 三种 Domain 实现

**LocalDomain**

- `spawn_pane` 调 L0 的 `PtyTransport::spawn` 启动 Shell，返回唯一 PaneHandle。
- `send_input` 直写 PTY；`output_stream` 即 PTY 读流；`resize` 直调 PTY 尺寸。
- 生命周期由本地驱动。

**RemoteSshDomain**

- `spawn_pane` 建立 SSH 连接并申请 PTY channel，返回唯一 PaneHandle。
- 输入输出直通 SSH channel；`resize` 转成 `window-change` 请求。
- 生命周期由本地驱动（连接断开或本地关闭）。

**TmuxDomain（1:N 域）**

- **Connection**：唯一一个运行 `tmux -CC attach` 的本地 PTY。
- **内部常驻 reader**：解析控制模式线路协议：
  - `%output %<pane-id> <escaped-bytes>` → 解码后**原样**推入
    `pane_registry[pane-id]` 的 `output_stream`。注意这些字节是终端原始字节，
    必须绕过 tmux 的字符渲染，直接送 L2 解析（Sixel、OSC 8 等序列才能保真）。
  - `%layout-change`、`%window-add`、`%window-close`、
    `%window-pane-changed`、`%session-changed` → 转为结构化事件，一路更新内部
    `pane_registry`，一路向 L3 广播（用于容器增删与镜像布局更新）。
  - `%begin`/`%end` 块按命令编号路由回复。
- **输入路由**：每个 PaneHandle 知道自己的 `%pane-id`：
  - `send_input` → `send-keys -t %<id> -H <hex>` 写入控制 PTY。
  - `resize` 由容器整体发起，不按 pane 单独发（见 L3）。
- **生命周期双向**：本地关闭 → 发 `kill-pane`/`kill-window`；远端
  `kill-pane` → `%window-close`/`%layout-change` → 触发对应 PaneHandle 的
  `on_closed`。
- **布局解析**：`%layout-change` 携带 tmux layout string（含 rotate-add 校验
  和），在域内解析为 tmux 布局树，交给 L3 的容器持有。

### 3.4 域注册表

L5 持有一个 `DomainRegistry`，按 `domain_id` 索引 Domain 实例。L3 的叶子只保
存 `PaneHandle` 引用，不直接持有 Domain。域的销毁由 L5 在“其下所有 PaneHandle
均关闭”后触发。

---

## 4. L2 终端模拟核心层

**职责**

- 每 PaneHandle 一个独立实例：VT100/xterm 状态机 + 网格缓冲区（光标、字符属
  性、颜色、滚动区、滚动回看）。
- 消费 `output_stream` 的字节，产出网格状态变更事件。
- 不感知连接类型：无论字节来自 SSH、本地 PTY 还是 tmux 的 `%output`，处理完
  全相同。

**为什么必须每窗格独立**

- 两个独立连接的光标、滚动区、字符集状态互不干扰，共享状态机必然串扰。
- tmux 的每个 pane 在 tmux 服务器侧本就是独立终端，其 `%output` 是独立字节流。
- 独立实例使渲染层可以按叶子矩形裁剪重绘，天然支持脏区优化。

**接口（示意）**

```rust
trait TerminalCore {
    fn feed(Bytes)                            // 增量解析，产出网格脏区
    fn resize(cols, rows)
    fn snapshot() -> GridSnapshot             // 供渲染层读取
    fn on_dirty(callback)
}
```

---

## 5. L3 组合窗格树层

这是布局的唯一权威。它通过**节点类型**在结构上保证“tmux pane 不与本地窗格并
排”的硬约束。

### 5.1 节点类型

```rust
enum Node {
    Branch { direction: H|V, ratio: f32, left: Box<Node>, right: Box<Node> },
    LocalLeaf { handle: PaneHandle, core: TerminalCore },
    TmuxMirror { container: TmuxContainer },   // 不可拆分的布局原子
}

struct TmuxContainer {
    domain: TmuxDomainRef,
    window_id: TmuxWindowId,
    layout_tree: TmuxLayoutTree,               // 由 %layout-change 驱动
    panes: Map<TmuxPaneId, (PaneHandle, TerminalCore)>,  // 私有，不暴露给本地树
    active_pane: TmuxPaneId,
}
```

**结构性保证**：`TmuxContainer` 的内部 pane 只存在于其私有 `panes` 映射中，**不
会作为 `Node` 出现在本地树里**。本地树中 `TmuxMirror` 是叶节点。因此“本地窗格
与 tmux 内 pane 成为兄弟”在类型上不可表达——无需任何运行时检查。

### 5.2 两级布局计算

- **外层（本地树）**：从根递归分配矩形。`Branch` 按 `ratio` 切割；
  `LocalLeaf` 得到最终矩形；`TmuxMirror` 只得到**容器整体矩形**。
- **内层（tmux 布局）**：`TmuxContainer` 在自己的矩形内，用 `layout_tree` 再
  计算每个 pane 的子矩形，交给 L4 绘制。
- **分隔条归属**：本地树只绘制 `Branch` 的分隔条；容器内只绘制 tmux 布局的
  分隔条。边界即容器矩形，互不越界。

**尺寸同步**：容器矩形变化 → `TmuxDomain` 发
`refresh-client -C <cols>x<rows>` 或 `resize-window` → tmux 重新分配内部 pane
→ `%layout-change` 回来更新 `layout_tree`。本地**不直接**对容器内单个 pane 发
resize。

### 5.3 焦点模型（两级）

- **本地焦点**：当前聚焦的 `Node`。若为 `LocalLeaf`，输入直送其 PaneHandle。
- **容器内焦点**：本地焦点为 `TmuxMirror` 时，本地只记录“焦点在容器内”。具体
  pane 由 tmux 的 `%window-pane-changed` 驱动 `active_pane`，本地按键经
  `TmuxDomain` 路由到该 pane 的 PaneHandle。
- 本地快捷键的窗格导航只遍历本地树节点；容器内部的 pane 切换通过 tmux 的
  `select-pane` 命令，由远端通知回流。

### 5.4 树操作

- **拆分**：在 `LocalLeaf` 上拆出新的 `LocalLeaf` 或 `TmuxMirror`；在
  `TmuxMirror` 上拆分时，新节点与容器**平级**成为 `Branch` 的子节点，容器本
  身不被破坏。
- **关闭**：关闭 `LocalLeaf` → 销毁其 PaneHandle 与连接。关闭 `TmuxMirror` →
  `TmuxDomain` 发 `kill-window` 或 `detach`，收到 `%window-close` 后整体移除
  容器。本地 UI **不提供**单独关闭容器内某个 pane 的入口——那属于 tmux 命令空
  间。
- **收缩**：叶子移除后，父 `Branch` 被其兄弟节点替换。

---

## 6. L4 渲染层

**职责**

- 按 L3 给出的矩形，绘制每个叶子的网格：字形缓存、字体度量、光标、选择高亮、
  滚动条。
- 绘制本地 `Branch` 分隔条与容器内 tmux 分隔条（样式可分属两套主题，但均基
  于各自布局树）。
- 只读 L2 的 `GridSnapshot` 与 L3 的布局树，不直接接触 L0/L1。
- 脏区驱动：L2 的 `on_dirty` → 标记对应叶子矩形 → 合并重绘。

**约束**：渲染层不得持有 PaneHandle；它通过 `snapshot()` 拉取或订阅网格变更，
保持与 I/O 解耦。

---

## 7. L5 应用 / 协调层

**职责**

- 窗口/Tab 管理：一个 Tab 持有一棵 L3 树。
- `DomainRegistry` 与 `Mux`：全局注册表，负责 Domain 创建/销毁、树结构变更的
  调度与广播。
- 焦点管理器：维护本地焦点与各容器的内部焦点。
- 快捷键、菜单、配置（Profile）、SSH 主机发现、主题。
- 生命周期编排：连接断开、进程退出、tmux 会话结束时的清理与用户提示。

**约束**：本层不实现协议解析、不实现 VT 解析、不做布局几何计算，只做编排。

---

## 8. 数据流

**输出路径**

```text
SSH channel / PTY / tmux 控制 PTY
  → L1 域路由（tmux 域按 %pane-id 查表；其余直通）
  → 对应 PaneHandle.output_stream
  → L2 该窗格独立 TerminalCore.feed
  → 网格脏区事件
  → L4 按 L3 矩形重绘
```

**输入路径**

```text
用户按键
  → L3 焦点解析（LocalLeaf 直取；TmuxMirror 取 active_pane）
  → PaneHandle.send_input
  → L1 域封装（SSH 直写 / tmux 包成 send-keys -t %id）
  → L0 传输
```

**布局变更路径**

```text
本地拖动分隔条 → L3 更新 ratio → 若涉及 TmuxMirror 矩形 → L1 发 resize-window
tmux 内操作     → %layout-change → L1 解析 → L3 更新容器 layout_tree → L4 重绘
```

**生命周期路径**

```text
本地关闭叶子 → L1 发 kill/detach → 远端确认 → on_closed → L3 移除节点 → L5 回收 Domain
远端关闭 pane → %window-close/%layout-change → L1 触发 on_closed → L3 收缩容器布局或移除容器
```

---

## 9. 硬约束的结构性保证汇总

| 约束 | 保证方式 |
|---|---|
| tmux pane 不与本地窗格并排 | `TmuxContainer` 内部 pane 不出现在本地 `Node` 枚举中，类型上不可表达 |
| 两套分隔条不越界 | 布局计算分外层/内层，几何边界为容器矩形 |
| tmux 1:N 与 SSH 1:1 对上层无差异 | 上层只消费 `PaneHandle` 与 `on_closed`，基数差异封装在 L1 路由表 |
| tmux 字节保真 | `%output` 解码后直送 L2，不经任何字符渲染 |
| 每窗格状态隔离 | L2 每 PaneHandle 独立 TerminalCore 实例 |

---

## 10. 关键设计权衡

- **Domain 层承担协议，而非独立 tmux 协议层**：tmux 只是三种后端之一，协议解
  析封装在 `TmuxDomain` 内部，避免全局 tmux 单例与本地/SSH 路径分叉。
- **容器而非“禁止操作”**：用节点类型隔离替代运行时校验，约束不可被后续功能
  误破。
- **尺寸双层同步**：本地只与容器整体对话，tmux 负责内部再分配，避免本地与远
  端对同一 pane 尺寸的竞争写。
- **焦点两级**：本地树导航与 tmux pane 导航分属两套语义，通过 `active_pane` 与
  `%window-pane-changed` 桥接，不强行统一。
