# 部署视图

本文描述终端模拟器在 Linux、WSL、Windows 与远端环境中的部署形态、认证与运行时
依赖。权威设计见 [terminal-emulator.md](./terminal-emulator.md)。

## 1. 部署单元

| 单元 | 位置 | 生命周期 |
|---|---|---|
| `emulator-frontend` | 用户主机 | REPL 运行期间 |
| 本地 Shell / wsl.exe | 用户主机 | 随 pane |
| `ssh`（tmux 控制） | 用户主机 → 远端 | 随 session |
| tmux server | 本地/WSL/远端 | 独立于本程序 |

## 2. tmux 三种部署

### 本地（Linux / WSL 内）

```text
emulator-frontend → ConPTY/PTY → tmux -C attach -t <session>
```

### WSL tmux（Windows 原生程序）

```text
emulator-frontend → ConPTY → wsl.exe --exec tmux -C attach -t <session>
```

### 远端 tmux（SSH）

```text
emulator-frontend → 管道 → ssh -o StrictHostKeyChecking=accept-new <dest> tmux -C attach -t <session>
```

密码由 `SSH_ASKPASS` 交给系统 ssh；密钥则复用 `~/.ssh/config` 与 agent。

## 3. SSH 认证

- SSH shell：russh，密钥（identity 路径）或密码（交互无回显提示）。
- tmux-over-ssh：系统 ssh；`--password` 用 askpass，前端二进制兼任 askpass。

## 4. 环境变量与文件

| 名称 | 用途 |
|---|---|
| `XSCORE_LOG` | 日志文件路径（默认 `emulator.log`） |
| `RUST_LOG` | 日志级别 |
| `XSCORE_SSH_PASSWORD` / `PASSWD` | SSH 密码（优先于交互提示） |
| `XSCORE_SSH_HOST` | 集成测试目标主机 |

## 5. 平台限制

- Windows 原生不支持本地 tmux；本地 tmux 需在 Linux/WSL 内运行。
- WSL tmux 需要 WSL 内安装 tmux。
- 远端 tmux 密码认证需要 OpenSSH 8.4+（`SSH_ASKPASS_REQUIRE`）。
