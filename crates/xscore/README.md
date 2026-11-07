# xscore

`xscore` 是可复用的终端连接与传输库，提供统一的异步接口来管理：

- 本地 PTY / Windows ConPTY 子进程；
- 通过内嵌 `russh` 建立的 SSH shell；
- tmux 控制模式连接及其多个 pane；
- 每个 pane 独立的后台输出流、输入、resize 和关闭操作。

`xscore` 只负责连接、进程和原始字节 I/O，不负责 VT 屏幕解析、布局或图形渲染。
调用方可以把 [`TerminalHandle`] 的输出字节交给自己的终端模拟器。

本仓库中的 `xsc-client` 和 `xsc-server` 是 CLI 示例及集成测试前端，不是使用
`xscore` 的必选依赖。

## 在其他项目中导入

### 使用本地路径

同一个 workspace 内的其他 crate 可以直接使用相对路径：

```toml
[dependencies]
xscore = { path = "../xscore/crates/xscore" }
```

路径必须指向包含 `xscore` 的 `Cargo.toml` 的目录，而不是仓库根目录。
该 crate 的 `Cargo.toml` 不依赖根 workspace 的版本或依赖继承，因此也可以直接将
`crates/xscore` 复制或作为子模块放入其他项目。

也可以让 Cargo 修改依赖清单：

```bash
cargo add xscore --path <path-to-xscore>/crates/xscore
```

Windows PowerShell 示例：

```powershell
cargo add xscore --path C:\work\xscore\crates\xscore
```

### 使用 Git 或注册表

仓库托管后，可以固定到 tag 或 commit：

```toml
[dependencies]
xscore = { git = "https://github.com/xiaoshan1234/xscore.git", package = "xscore" }
```

发布到 crates.io 后可改为版本依赖：

```toml
[dependencies]
xscore = "0.6"
```

`xscore` 当前处于 `0.x` 阶段，建议使用 `0.6` 这样的兼容版本约束；如果需要严格
可复现构建，Git 依赖应追加 `tag = "<tag>"` 或 `rev = "<commit>"`。

## 运行环境

`xscore` 的公开操作都是异步的，并且会通过 `tokio::spawn` 启动 I/O 转发任务。
调用方必须提供一个 Tokio runtime。宿主机需要 Rust 支持的条件见下方平台限制。

```toml
[dependencies]
xscore = { path = "../xscore/crates/xscore" }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

顶层 `prelude` 汇集了常用类型：

```rust
use xscore::prelude::*;
```

需要 tmux 控制连接、协议事件或底层 PTY 时，可以从 crate 根模块导入
`TmuxControl`、`TmuxEvent`、`LocalPty`，或使用 `xscore::platform` 下的细分模块。

## 本地终端

下面的程序启动本地 shell，写入一条命令并读取输出：

```rust
use std::time::Duration;
use tokio::time::timeout;
use xscore::prelude::*;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let command = if cfg!(windows) { "cmd.exe" } else { "sh" };
    let config = TerminalConfig::local(command).with_size(Size::new(100, 30));
    let terminal = LocalDomain::new().spawn_terminal(&config).await?;
    let mut output = terminal.subscribe();

    // Windows ConPTY 的交互式回车使用 \r\n；Unix PTY 使用 \n。
    let input = if cfg!(windows) {
        b"echo xscore-ok\r\n".as_slice()
    } else {
        b"echo xscore-ok\n".as_slice()
    };
    terminal.send_input(input).await?;

    while let Ok(Ok(chunk)) = timeout(Duration::from_secs(5), output.recv()).await {
        print!("{}", String::from_utf8_lossy(&chunk));
        if String::from_utf8_lossy(&chunk).contains("xscore-ok") {
            break;
        }
    }

    terminal.close().await?;
    Ok(())
}
```

`TerminalConfig::local` 和 `TerminalConfig::local_command` 使用 `80x24` 默认尺寸。
需要工作目录或环境变量时，可以直接构造 `TerminalConfig::Local`：

```rust
let config = TerminalConfig::Local {
    command: "sh".into(),
    args: vec!["-lc".into(), "printf '%s\\n' \"$APP_ENV\"".into()],
    cwd: Some("/srv/app".into()),
    env: vec![("APP_ENV".into(), "development".into())],
    cols: 120,
    rows: 40,
};
```

## SSH shell

SSH shell 使用进程内 `russh`，不会启动系统 `ssh` 命令。密码认证：

```rust
use xscore::prelude::*;

let config = TerminalConfig::ssh_with_password("alice", "example.com", "secret")
    .with_size(Size::new(120, 40));
let terminal = RemoteSshDomain::new().spawn_terminal(&config).await?;
let mut output = terminal.subscribe();
terminal.send_input(b"uname -a\n").await?;
```

私钥认证：

```rust
let config = TerminalConfig::ssh_with_identity(
    "alice",
    "example.com",
    "/home/alice/.ssh/id_ed25519",
);
```

需要自定义端口、环境变量或认证组合时，可构造 `TerminalConfig::Ssh`。`identity`
存在时优先于 `password`。

> 安全提示：当前 SSH 客户端接受服务端提供的任意 host key。对 host key 校验有严格
> 要求的应用，需要在上层补充 known-hosts 校验，或等待库提供对应配置接口。

## tmux

tmux 一个控制连接可以提供多个 pane。`attach` 返回该 session 下所有现有 pane：

```rust
use xscore::prelude::*;

let host = TmuxHostConfig::Ssh {
    user: "alice".into(),
    host: "example.com".into(),
    port: 22,
    identity: None,
    password: Some("secret".into()),
};
let panes = TmuxDomain::new()
    .attach(&host, &TmuxSocket::Default, "work")
    .await?;

for pane in panes {
    let mut output = pane.subscribe();
    pane.send_input(b"printf 'hello from tmux\\n'\n").await?;
}
```

Windows 上如需连接 WSL 内的 tmux：

```rust
let host = TmuxHostConfig::Wsl { args: Vec::new() };
```

Linux/macOS 本地 tmux 使用 `TmuxHostConfig::Local`。Windows 原生环境不提供本地
tmux，需要使用 WSL 或 SSH 主机。

## 输出、尺寸与生命周期

`TerminalHandle::subscribe()` 返回 Tokio broadcast receiver。需要注意：

- 订阅只接收订阅之后产生的新输出，历史输出不会回放；
- receiver 跟不上时可能收到 `Lagged`，高吞吐调用方应及时消费或增加自己的缓冲；
- `send_input` 接收原始字节，不替你转换行尾；
- `resize` 使用 `Size { cols, rows }`，单位是字符行列；
- `close` 会关闭 pane 对应的 PTY、SSH channel 或 tmux pane；
- 一个 `TerminalHandle` 对应一个逻辑 pane，不同 pane 的状态由调用方分别保存。

## 可复用的 API 边界

| 类型 / trait | 用途 |
|---|---|
| `LocalDomain` | 创建本地 PTY/ConPTY 终端 |
| `RemoteSshDomain` | 创建独立 SSH shell 终端 |
| `TmuxDomain` | attach tmux session 并创建多个 pane 句柄 |
| `Domain` | 后端无关的终端工厂接口 |
| `TerminalConfig` | 本地或 SSH 终端创建参数 |
| `TerminalHandle` | 输入、输出、resize 和关闭的统一异步句柄 |
| `TerminalKind` | 标识 `Local`、`Ssh` 或 `Tmux` |
| `Size` | 字符网格尺寸 |
| `TmuxHostConfig` / `TmuxSocket` | 指定 tmux 宿主机和 socket |

`TerminalHandle::as_any()` 可用于访问具体实现；tmux 调用方通常用它取得
`TmuxTerminal` 并执行 `send_command` 等 tmux 专有操作。

## 平台限制

- Windows 本地 shell 使用 ConPTY；
- Windows 原生不支持本地 tmux，本地 tmux 必须运行在 Linux/WSL 内；
- 远端 tmux 使用库内 `russh` 连接，不读取系统 OpenSSH 配置、agent 或 ProxyJump；
- 本地终端和 SSH 服务端的 PTY 都必须支持目标 shell 所需的能力。

## 验证

在仓库根目录运行核心库测试：

```bash
cargo test -p xscore
cargo clippy -p xscore --all-targets -- -D warnings
cargo fmt --all -- --check
```

构建仓库内已有示例前端：

```bash
cargo build -p xsc-client -p xsc-server
```

需要真实 SSH 主机的测试默认标记为 `#[ignore]`，连接配置见仓库根目录的
`test-example.config` 和集成测试脚本。

## 许可

MIT。
