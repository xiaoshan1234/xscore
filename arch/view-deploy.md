# 部署视图

本文描述终端模拟器在 Linux、WSL、Windows 与远端环境中的部署形态、认证与运行时
依赖。权威设计见 [terminal-emulator.md](./terminal-emulator.md)。

## 1. 部署单元

| 单元 | 位置 | 生命周期 |
|---|---|---|
| `xs` | 用户主机 | 单条命令期间 |
| `xs-daemon` | 用户主机 | 常驻，直到 `xs daemon stop` |
| 本地 Shell / wsl.exe | 用户主机 | 随 pane |
| russh SSH channel（tmux 控制） | 用户主机 → 远端 | 随 session |
| tmux server | 本地/WSL/远端 | 独立于本程序 |

## 2. tmux 三种部署

### 本地（Linux / WSL 内）

```text
xs → xs-daemon → ConPTY/PTY → tmux -C attach -t <session>
```

### WSL tmux（Windows 原生程序）

```text
xs → xs-daemon → ConPTY → wsl.exe --exec tmux -C attach -t <session>
```

### 远端 tmux（SSH）

```text
xs → xs-daemon → russh channel → exec tmux -C attach -t <session>
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
| `XSCORE_DAEMON_EXE` | 可选的 `xs-daemon` 路径覆盖 |

## 5. 平台限制

- Windows 原生不支持本地 tmux；本地 tmux 需在 Linux/WSL 内运行。
- WSL tmux 需要 WSL 内安装 tmux。
- 远端 tmux 使用应用内 russh，不读取 `~/.ssh/config`、ssh-agent 或 ProxyJump。
