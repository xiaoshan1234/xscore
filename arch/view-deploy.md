# 部署视图

本文描述终端模拟器在 Linux、WSL、Windows 与远端环境中的部署形态、认证与运行时
依赖。权威设计见 [terminal-emulator.md](./terminal-emulator.md)。

## 1. 部署单元

| 单元 | 位置 | 生命周期 |
|---|---|---|
| `xsc` | 用户主机 | 单条命令期间 |
| `xss` | 用户主机 | 常驻，直到 `xsc daemon stop` |
| 本地 Shell / wsl.exe | 用户主机 | 随 pane |
| russh SSH channel（tmux 控制） | 用户主机 → 远端 | 随 session |
| tmux server | 本地/WSL/远端 | 独立于本程序 |

## 2. tmux 三种部署

### 本地（Linux / WSL 内）

```text
xsc → xss → ConPTY/PTY → tmux -C attach -t <session>
```

### WSL tmux（Windows 原生程序）

```text
xsc → xss → ConPTY → wsl.exe --exec tmux -C attach -t <session>
```

### 远端 tmux（SSH）

```text
xsc → xss → russh channel → exec tmux -C attach -t <session>
```

连接在进程内由 russh 建立，密码或私钥直接参与 SSH 认证，不启动系统 ssh，
也不使用 askpass。

## 3. SSH 认证

- SSH shell：russh，密钥（identity 路径）或密码（交互无回显提示）。
- tmux-over-ssh：russh exec channel；密钥（identity 路径）或密码直接认证。

## 4. 环境变量与文件

| 名称 | 用途 |
|---|---|
| `XSCORE_LOG` | 日志文件路径（默认 `xscore.log`） |
| `RUST_LOG` | 日志级别 |
| `XSCORE_DAEMON_EXE` | 可选的 `xss` 路径覆盖 |

## 5. 平台限制

- Windows 原生不支持本地 tmux；本地 tmux 需在 Linux/WSL 内运行。
- WSL tmux 需要 WSL 内安装 tmux。
- 远端 tmux 使用应用内 russh，不读取 `~/.ssh/config`、ssh-agent 或 ProxyJump。

## 6. 构建、安装与运行

前置条件：Rust 工具链（`cargo`）；集成测试额外需要 Python 3。

### 构建

```bash
cargo build --workspace
```

### 测试与检查

```bash
cargo test -p xscore -p xsc-protocol -p xsc-client -p xsc-server
cargo clippy -p xscore -p xsc-protocol -p xsc-client -p xsc-server --all-targets -- -D warnings
cargo fmt --all -- --check
```

### 安装

```bash
cargo install --path crates/xsc-server   # 安装 `xss`
cargo install --path crates/xsc-client   # 安装 `xsc`
```

两个可执行文件装到 cargo 的 bin 目录（Linux/macOS 为 `~/.cargo/bin`，Windows 为
`%USERPROFILE%\.cargo\bin`），需确保该目录在 `PATH` 中。

### 运行

`xsc` 是一次性 CLI，命令通过本地回环 TCP 发给常驻的 `xss`；daemon 不存在时
会自动拉起。不带参数运行 `xsc` 可查看帮助。

```bash
xsc ls pane                    # 列出当前 pane
xsc ls window [<group>]        # 列出窗口，可限定 group
xsc ls group                   # 列出窗口组
xsc tml add --url shell://powershell
xsc tml read <pane-id>         # 读 pane 渲染快照
xsc tml write <pane-id> text   # 向 pane 发送文本
xsc tmx attach --url tmux://<socket>/<session>
xsc layout [<window-id>]       # 打印布局树
```

daemon 生命周期：

```bash
xsc daemon serve               # 前台运行 daemon
xsc daemon status              # 查看 daemon 当前状态
xsc daemon stop                # 停止 daemon
```

### 集成测试

需要真实 SSH 主机时，从根目录 `test.config` 读取连接配置（格式见
`test-example.config`）：

```powershell
python scripts/run_integration_tests.py
```

结果写入 `test-results/`。
