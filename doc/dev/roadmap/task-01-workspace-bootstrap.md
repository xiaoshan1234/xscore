# Task 1: Workspace bootstrap

**Objective:** Create Rust workspace, base `xscore` crate skeleton, builds empty.

**Files:**
- Create: `Cargo.toml` (workspace)
- Create: `crates/xscore/Cargo.toml`
- Create: `crates/xscore/src/main.rs`
- Create: `crates/xscore/src/lib.rs`

**Step 1: Write workspace root**

```toml
# /home/loner/prj/xscore/Cargo.toml
[workspace]
resolver = "2"
members = ["crates/xscore"]

[workspace.package]
edition = "2021"
license = "MIT"
version = "0.1.0"

[workspace.dependencies]
tokio = { version = "1", features = ["full"] }
clap = { version = "4", features = ["derive"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "1"
anyhow = "1"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
uuid = { version = "1", features = ["v7", "serde"] }
url = "2"
russh = "0.43"            # task-08 SSH transport — needs a real release probe before use
russh-keys = "0.43"        # task-08 SSH transport
portable-pty = "0.8"       # task-07 shell transport — needs a real release probe before use
# tokio-serial = "?"        # task-08 serial transport — added when this task is implemented (current is 5.x)
async-trait = "0.1"
dirs = "5"
mockall = "0.13"
tempfile = "3"
chrono = { version = "0.4", features = ["serde"] }
```

**Step 2: Write crate manifest**

```toml
# crates/xscore/Cargo.toml
[package]
name = "xscore"
edition.workspace = true
version.workspace = true
license.workspace = true

[dependencies]
tokio.workspace = true
clap.workspace = true
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
anyhow.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
uuid.workspace = true
url.workspace = true
async-trait.workspace = true
dirs.workspace = true
russh.workspace = true
russh-keys.workspace = true
portable-pty.workspace = true
tokio-serial.workspace = true

[dev-dependencies]
mockall.workspace = true
tempfile.workspace = true
```

**Step 3: Empty main.rs and lib.rs**

```rust
// crates/xscore/src/main.rs
fn main() {}
```

```rust
// crates/xscore/src/lib.rs
```

**Step 4: Build**

Run: `cargo build`
Expected: `Finished 'dev' profile`, no errors.

**Step 5: Commit**

```bash
git add Cargo.toml crates/xscore/
git commit -m "feat: scaffold xscore workspace and empty binary"
```