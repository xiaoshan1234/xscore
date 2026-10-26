# Rust 命名规范

适用：`src-tauri/src/` 下所有 Rust 代码（Tauri 2 命令 + `portable-pty` 后端）。

## 核心原则

- 遵循 [Rust API Guidelines — Naming](https://rust-lang.github.io/api-guidelines/naming.html)
- 类型 `UpperCamelCase`、函数 / 变量 / 模块 `snake_case`、常量 `SCREAMING_SNAKE_CASE`
- 纯英文；禁止拼音 / 中文 / 无意义通用名（`tmp`、`obj`、`data`、`val`、`res`）
- 行业缩写可用：`id` / `pty` / `ssh` / `ipc` / `cmd` / `pid` / `buf` / `err` / `cwd` / `ttl`
- 禁止 `_xxx` / `xxx_` 变量（编译器约定的 `let _ = ...` 占位除外）
- 禁止把 TS 习惯带过来：`camelCase` 函数、`IUser` trait 前缀、enum 成员全大写

## 文件 / 模块

| 类型 | 格式 | 示例 |
|---|---|---|
| 文件 | `snake_case.rs` | `lib.rs`、`pty_state.rs` |
| 模块声明 | `snake_case`，与文件名一致 | `mod pty_state;` |
| Crate 名 | `snake_case`（`Cargo.toml` `[lib]` / `[package]`） | `xsterm_lib`（`_lib` 后缀见下） |

> **本项目 lib name 固定 `xsterm_lib`**：Windows 编译时 bin 与 lib 同名会冲突，故加 `_lib` 后缀（见 `src-tauri/Cargo.toml` + rust-lang/cargo#8519）。**不要改** `lib.rs` 与 `main.rs` 中的 `xsterm_lib::run()` 引用。

## 变量 / 字段

| 类型 | 格式 | 示例 |
|---|---|---|
| 普通变量 / 字段 | `snake_case` | `writer`、`state_guard` |
| 布尔 | `snake_case` + `is_` / `has_` / `should_` / `can_` 前缀 | `is_attached`、`has_pane` |
| 集合 | 复数名词 | `pane_ids`、`active_sessions` |
| 常量 / `static` | `SCREAMING_SNAKE_CASE` | `const MAX_PTY_BYTES: usize = 8192` |
| 生命周期 | 短小语义小写 | `'a`、`'b`；多场景用 `'src` / `'dst` |

## 函数 / 方法

- `snake_case`，**动词 + 名词**：`get_` / `set_` / `update_` / `create_` / `build_` / `init_` / `send_` / `handle_` / `try_` / `parse_` / `is_` / `has_`
- 返回 `Option` / `Result` 且可能失败的函数前缀 `try_`：`try_resize`
- 转换函数：
  - `as_` 无损借用转换（`&str -> &str`）
  - `to_` 可能分配 / 转换所有权（`&str -> String`）
  - `into_` 消耗 `self` 的转换

## 类型

| 类型 | 格式 | 示例 |
|---|---|---|
| 结构体 | `UpperCamelCase`，无 `S_` 前缀 | `PtyState`、`SessionInfo` |
| 枚举名 | `UpperCamelCase` | `Status`、`SplitDirection` |
| **枚举变体** | `UpperCamelCase`（**不是** `SCREAMING_SNAKE_CASE`） | `enum Status { Ok, Failed }` |
| Trait | `UpperCamelCase`，能力 trait 用 `-able` / `-ing` 后缀 | `SessionBackend`、`Resizable` |
| 类型别名 | `UpperCamelCase` | `pub type Result<T> = ...;` |
| 泛型 | 大写单字母 / 业务语义 `UpperCamelCase` | `<T>`、`<TPane>` |
| 新类型 ID | `<Thing>Id` newtype | `struct SessionId(pub u32);` |
| 错误类型 | `<Scope>Error` | `enum PtyError` |

> **关键差异（TS ↔ Rust）**：Rust enum **变体** 是 `UpperCamelCase`，**不是** TS 那种 `UPPER_SNAKE_CASE`。`enum Status { Ok, Failed }` ✅；`enum Status { OK, FAILED }` ❌。

## Tauri Command（IPC 边界）

Command 名是 **前后端契约**：字符串字面量在 TS 的 `invoke(...)` 与 Rust 的 `#[tauri::command]` 函数名之间**精确匹配**。改一个必须同步改另一个。

- 格式：`snake_case`，**动词开头**，不带参数后缀
- 当前命令清单（见 `src-tauri/src/lib.rs`）：`spawn_terminal` / `write_terminal` / `resize_terminal`
- 内部纯逻辑函数后缀 `_impl`：command 入口只做参数提取，调用 `<command>_impl`

```rust
#[tauri::command]
fn spawn_terminal(rows: u16, cols: u16, ...) -> Result<(), String> {
    spawn_terminal_impl(rows, cols).map_err(|e| e.to_string())
}

fn spawn_terminal_impl(rows: u16, cols: u16) -> Result<(), PtyError> {
    // 纯逻辑，可被单元测试直接调用
}
```

后端 `app.emit("event-name", payload)` 与前端 `listen<T>("event-name", cb)` 同样契约。当前事件：
- `terminal-data` → 载荷 `Vec<u8>`（TS 侧接收为 `number[]`，必须 `new Uint8Array(...)` 再解码）

## 禁止清单

```rust
// 全部禁止
let tmp, obj, data, val, res        // 无意义通用名
let aa, bb, cc                       // 随机命名
let 用户名, 状态                      // 中文 / 拼音
let user_str, user_arr               // 类型后缀
fn handleSomeThing() { }             // camelCase 函数
struct S_SessionInfo { }             // S_ 前缀
trait ISessionBackend { }            // I 前缀（TS 习惯带过来）
enum AppErr { }                      // 缩写
const maxPaneBytes: usize = ...;     // camelCase 常量
mod SessionManager;                  // PascalCase 模块
type SessionIdType = u32;            // _Type 后缀
struct SessionIdWrapper(pub u32);    // _Wrapper 后缀
```

## TS ↔ Rust 差异速查

| 维度 | TypeScript | Rust |
|---|---|---|
| 函数 | `getUserInfo` | `get_user_info` |
| 变量 | `userName` | `user_name` |
| 常量 | `MAX_PAGE_SIZE` | `MAX_PAGE_SIZE`（一致） |
| 类型 | `UserInfo` | `UserInfo`（一致） |
| 接口 | `interface UserInfo`（无 I 前缀） | `trait UserInfo`（无 I 前缀） |
| **枚举变体** | `enum Status { OK = "ok" }` | `enum Status { Ok }`（**不是全大写**） |
| 文件 | `userInfo.ts` | `user_info.rs` |
| 泛型 | `<T>` / `<ItemType>` | `<T>` / `<TPane>` |
| 布尔 | `isHydrated` | `is_hydrated` |

## 速查

| 代码类型 | 格式 | 示例 |
|---|---|---|
| 普通变量 / 函数 / 模块 | `snake_case` | `pane_count`、`spawn_terminal`、`pty_state` |
| 布尔 | `is_` / `has_` / `should_` / `can_` | `is_attached`、`has_pane` |
| 常量 / `static` | `SCREAMING_SNAKE_CASE` | `MAX_PTY_BYTES` |
| 结构体 / 枚举 / Trait / 类型别名 | `UpperCamelCase` | `PtyState`、`AppError` |
| **枚举变体** | `UpperCamelCase`（**不是**全大写） | `Status::Ok` |
| 文件 | `snake_case.rs` | `pty_state.rs` |
| Tauri command | `snake_case` + 动词 | `spawn_terminal` |
| 实现函数（command 内部） | `<command>_impl` | `spawn_terminal_impl` |
| 新类型 ID | `<Thing>Id` | `SessionId(u32)` |
| 错误类型 | `<Scope>Error` | `PtyError` |
