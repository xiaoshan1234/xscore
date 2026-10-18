# Release process

This document describes the v0.1.0 release process for `xscore` and the
version-bump policy going forward.

## Versioning

`xscore` follows [Semantic Versioning 2.0.0](https://semver.org/):

- **MAJOR** (1.0, 2.0): breaking CLI flag changes, breaking `Transport` /
  `Sink` trait changes, breaking schema changes to `state.json` or
  template files (`link config`).
- **MINOR** (0.1, 0.2): new subcommands, new verb-group verbs, new
  optional features, additive schema changes (new fields with defaults).
- **PATCH** (0.1.0, 0.1.1): bug fixes, doc updates, internal refactors
  that don't change user-visible behavior.

We follow `0.y.z` pre-1.0. While in `0.y`, MINOR bumps may include
breaking changes documented in the [CHANGELOG](CHANGELOG.md) under
"Changed" / "Removed". After 1.0, breaking changes require a MAJOR bump.

## Pre-release checklist (v0.1.0)

This is the checklist followed for the v0.1.0 release. Future v0.y.z
patches skip steps that don't apply.

- [x] All 28 tests pass (`cargo test -p xscore`)
- [x] 0 warnings (`cargo check --workspace`)
- [x] No `#[ignore]`d tests in the test suite
- [x] `CHANGELOG.md` updated for the release (commit `0cc8ac5`)
- [x] `README.md` "5-minute demo" + "Status" + "Tests" sections
      reflect the actual state
- [x] No stale `Known limitations` from past slices that have been
      resolved
- [ ] `Cargo.toml` workspace version is `0.1.0`
- [ ] `cargo build --release` produces a working binary
- [ ] Manual smoke of the 5-minute demo (steps 1-9) on a clean
      machine (or fresh HOME) succeeds
- [ ] Git tag `v0.1.0` is created and pushed
- [ ] GitHub Release is published (release notes auto-generated
      from the CHANGELOG section)
- [ ] (If publishing to crates.io) `cargo publish -p xscore`
- [ ] (If publishing binaries) `cargo install --path .` from
      release tarball succeeds on a clean machine

## Cutting a release

### 1. Update the version in `Cargo.toml`

```bash
# Workspace version lives at [workspace.package] in /Cargo.toml.
# The crate inherits via version.workspace = true.
$EDITOR Cargo.toml   # bump version = "X.Y.Z"
cargo build --workspace
```

### 2. Update `CHANGELOG.md`

Move the `[Unreleased]` section's contents into a new dated section
matching the version. Use today's date (UTC).

```bash
$EDITOR CHANGELOG.md
# Replace the topmost "## [Unreleased]" with "## [X.Y.Z] - YYYY-MM-DD"
# Add a new "## [Unreleased]" section above it (empty for now)
# Update the link footers at the bottom of the file
```

### 3. Verify

```bash
cargo build --workspace
cargo check --workspace
cargo test -p xscore
```

All green. No warnings. All tests pass.

### 4. Commit, tag, push

```bash
git add Cargo.toml CHANGELOG.md
git commit -m "chore(release): vX.Y.Z"
git tag -a vX.Y.Z -m "xscore vX.Y.Z — <one-line summary>"
git push origin main
git push origin vX.Y.Z
```

### 5. GitHub Release

Either:
- Use the GitHub UI: `https://github.com/<owner>/xscore/releases/new`
  → choose the tag → copy the CHANGELOG section as the release notes
- Or: `gh release create vX.Y.Z --title "xscore vX.Y.Z" --notes-file <(sed -n '/^## \[X.Y.Z\]/,/^## /p' CHANGELOG.md)`

### 6. (Optional) Publish to crates.io

```bash
cargo login
cargo publish -p xscore
```

This requires a crates.io account. The first publish needs
`cargo publish --dry-run` first to verify the package metadata.

### 7. (Optional) Pre-built binaries

For the v0.1.0 release we don't ship pre-built binaries. When we do:

```bash
# On a Linux x86_64 machine:
cargo build --release --target x86_64-unknown-linux-gnu
strip target/x86-64-unknown-linux-gnu/release/xscore

# On macOS x86_64:
cargo build --release --target x86_64-apple-darwin
# On macOS arm64:
cargo build --release --target aarch64-apple-darwin
# On Windows:
cargo build --release --target x86_64-pc-windows-msvc
```

Tar up each binary, attach to the GitHub Release.

## Hotfix flow (v0.y.z → v0.y.z+1)

```
edit code
cargo test -p xscore                # green
$EDITOR Cargo.toml                   # version bump
$EDITOR CHANGELOG.md                 # new patch entry
git commit -am "fix: <one-line>"
cargo build --workspace
git tag -a v0.y.z+1 -m "..."
git push origin main && git push origin v0.y.z+1
```

For trivial docs-only changes (README typo, comment fix), skip the
version bump and CHANGELOG — commit directly to main.

## Post-1.0 policy (preview)

When we cut 1.0:
- A migration guide replaces the "Known limitations" section
- Pre-built binaries become part of every release
- The `tmux_window_index` persisted-state fix from v0.1.0 (commit
  `0cc8ac5`) is no longer relevant; users who upgraded from pre-0.1.0
  have already migrated.
- CHANGELOG keeps the same format; new "## [Unreleased]" section
  starts the cycle again.

## References

- [Keep a Changelog](https://keepachangelog.com/) — changelog format
- [Semantic Versioning](https://semver.org/) — version policy
- [Conventional Commits](https://www.conventionalcommits.org/) — commit
  message style (the existing commit history already uses this style:
  `feat:`, `fix:`, `docs:`, `test+docs:`, `chore:`)
- [cargo-release](https://github.com/crate-ci/cargo-release) — optional
  automation for the version-bump + tag step
