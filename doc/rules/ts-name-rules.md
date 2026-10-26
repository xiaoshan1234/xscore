# TypeScript / TSX 命名规范

适用：`src/` 下所有 TypeScript / TSX 代码（前端 Vite + React 19）。

## 核心原则

- 语义化、见名知意、严格区分大小写
- 纯英文；禁止拼音 / 中文 / 无意义通用名（`tmp`、`obj`、`data`、`val`）
- 行业缩写可用：`ID` / `URL` / `API` / `HTTP` / `IPC` / `DOM` / `CSS` / `PTY` / `IPC`
- 禁止：类型后缀（`userInfoDTO`、`userVO`）、I 前缀接口（`IUser`）

## 文件命名

| 类型        | 格式              | 示例                                         |
| ----------- | ----------------- | -------------------------------------------- |
| React 组件  | `PascalCase.tsx`  | `src/ui/TabBar.tsx`、`src/ui/PaneChrome.tsx` |
| 模块 / 工具 | `camelCase.ts`    | `src/service/pty.ts`                         |
| Vite / 配置 | `kebab-case.ts`   | `vite.config.ts`、`tsconfig.node.json`       |
| 类型声明    | `kebab-case.d.ts` | `vite-env.d.ts`                              |

## 变量命名

| 类型                                | 格式                                               | 示例                            |
| ----------------------------------- | -------------------------------------------------- | ------------------------------- |
| 普通变量                            | `camelCase`                                        | `terminalRef`、`xtermRows`      |
| 布尔                                | `camelCase` + `is` / `has` / `should` / `can` 前缀 | `isMounted`、`hasError`         |
| 数组 / 集合                         | 复数名词                                           | `terminals`、`userList`         |
| 局部常量（config 对象、一次性魔数） | `camelCase`                                        | `const defaultRows = 24`        |
| 全局 / 跨模块常量                   | `UPPER_SNAKE_CASE`                                 | `const MAX_TERMINAL_ROWS = 200` |

## 函数 / 方法

- `camelCase`，**动词开头**：`get` / `set` / `handle` / `init` / `update` / `create` / `check` / `format`
- React 事件处理函数统一 `handleX` 前缀（与 `onX` prop 对应）：`handleResize` ↔ `<Component onResize={handleResize} />`
- 异步初始化函数惯用 `init`（如 `Terminal.tsx::init`）

## 类型 / 接口 / 泛型

- 全部 `PascalCase`，**禁止** `I` 前缀（摒弃老式规范）
- 对象结构用 `interface`；联合 / 交叉 / 别名用 `type`
- 简单泛型用 `T`；业务泛型用 `PascalCase`：`<T>`、`<ItemType>`
- 数组 / 数据后缀约定：`UserInfo`、`UserItem`、`UserList`、`UserConfig`、`LoadStatus`

## 枚举

- 枚举名 `PascalCase`
- **成员 `UPPER_SNAKE_CASE` 字符串值**（TS 习惯；与 Rust 不同）

```typescript
enum Status {
  Ok = "ok",
  Failed = "failed",
  Pending = "pending",
}
```

## React 组件

- 组件函数名 `PascalCase`，**default export**
- 一个文件一个组件，文件名与组件名一致
- props 类型在同文件 `type XxxProps = {...}` 定义，不导出除非外部使用

## Tauri IPC 调用侧

- `invoke<T>("command_name", { arg1, arg2 })` 字符串字面量必须与 `src-tauri/src/lib.rs` 中的 `#[tauri::command]` 函数名**精确匹配**（`snake_case`，见 `rust-name-rules.md`）
- `listen<PayloadT>("event-name", cb)` 同样必须匹配 Rust `app.emit(...)` 的字符串
- 接收 `Vec<u8>` 载荷时，必须 `new Uint8Array(payload)` 再 `TextDecoder` 解码（见 `Terminal.tsx`）

## 禁止清单

```typescript
// 全部禁止
let tmp, obj, data, val        // 无意义通用名
let aa, bb, cc                 // 随机命名
let 用户名, 状态                // 中文 / 拼音
let user_str, user_arr         // 类型后缀
interface IUserInfo            // I 前缀
type UserInfoDTO               // _DTO / _VO / _Bean 后缀
const MAX_PANE_BYTES = ...     // 非 const 模块允许 SCREAMING
```

## 速查

| 代码类型             | 格式                                          | 示例                                      |
| -------------------- | --------------------------------------------- | ----------------------------------------- |
| 组件 / 类 / 类型     | `PascalCase`                                  | `Terminal`、`UserInfo`                    |
| 变量 / 函数 / 数组   | `camelCase`                                   | `terminalRef`、`handleResize`、`userList` |
| 布尔                 | `is` / `has` / `should` / `can` + `camelCase` | `isMounted`、`hasError`                   |
| 全局常量             | `UPPER_SNAKE_CASE`                            | `MAX_TERMINAL_ROWS`                       |
| TSX 文件             | `PascalCase.tsx`                              | `Terminal.tsx`                            |
| TS 模块              | `camelCase.ts`                                | `sessionService.ts`                       |
| 枚举成员             | `UPPER_SNAKE_CASE` 字符串                     | `Status.Ok = "ok"`                        |
| 泛型                 | `T` / 业务语义 `PascalCase`                   | `<T>`、`<ItemType>`                       |
| Tauri command 字符串 | `snake_case`（与 Rust 对齐）                  | `"spawn_terminal"`                        |
