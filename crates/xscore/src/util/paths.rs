// util/paths.rs
//
// WSL (and "a path someone copied out of Explorer") translation. The rule is
// deliberately narrow: only a drive letter plus a separator is rewritten, only
// when that drive is actually mounted, and only in one direction. Everything
// else is taken as a Linux path and left alone — guessing more than that turns
// a typo into a silently different directory, which is what this is meant to
// stop.
use std::path::Path;

/// `C:\Users\x`, `C:/Users/x`, `C:` → `/mnt/c/Users/x`, `/mnt/c`. `None` when
/// the input is not a Windows drive path.
///
/// Pure: it does not check that the drive is mounted, so it can be tested
/// anywhere. `to_linux_path` adds that check.
pub fn windows_to_wsl(raw: &str) -> Option<String> {
    let bytes = raw.as_bytes();
    if bytes.len() < 2 || !bytes[0].is_ascii_alphabetic() || bytes[1] != b':' {
        return None;
    }
    let rest = &raw[2..];
    // `C:` is the drive root; anything else must be followed by a separator,
    // so that a bare `C:foo` (a drive-relative path) is not silently guessed.
    if !rest.is_empty() && !rest.starts_with('\\') && !rest.starts_with('/') {
        return None;
    }
    let letter = (bytes[0] as char).to_ascii_lowercase();
    let tail = rest.trim_start_matches(['\\', '/']).replace('\\', "/");
    Some(if tail.is_empty() {
        format!("/mnt/{letter}")
    } else {
        format!("/mnt/{letter}/{tail}")
    })
}

/// Translate a Windows path to its WSL mount, but only when that mount exists.
/// A path that is not Windows-style comes back unchanged.
pub fn to_linux_path(raw: &str) -> String {
    match windows_to_wsl(raw) {
        Some(translated) => {
            let mount = translated
                .strip_prefix("/mnt/")
                .and_then(|rest| rest.split('/').next())
                .map(|letter| format!("/mnt/{letter}"))
                .unwrap_or_else(|| "/mnt".to_string());
            if Path::new(&mount).is_dir() {
                translated
            } else {
                raw.to_string()
            }
        }
        None => raw.to_string(),
    }
}

/// A `.exe`/`.bat`/`.cmd` target: a Windows program spawned from WSL. Measured
/// behaviour is that it connects and then produces nothing — WSL interop gives
/// it no console behind a Linux pty — so the CLI warns rather than letting the
/// user stare at an empty buffer.
pub fn looks_like_windows_program(command: &str) -> bool {
    let lower = command.trim().to_ascii_lowercase();
    lower.ends_with(".exe") || lower.ends_with(".bat") || lower.ends_with(".cmd")
}

/// A `cwd=` value that must exist before anything is spawned in it.
///
/// portable-pty falls back to `$HOME` when the child cannot chdir, so an
/// unchecked typo used to produce a session in the wrong directory with no
/// error at all (measured: `cwd=/nonexistent-dir` started in `$HOME`).
pub fn require_dir(raw: &str) -> crate::core::Result<String> {
    let path = to_linux_path(raw);
    if Path::new(&path).is_dir() {
        return Ok(path);
    }
    let hint = if path != raw {
        format!(" (looking for `{path}`)")
    } else {
        String::new()
    };
    Err(crate::core::Error::Invalid(format!(
        "cwd `{raw}` is not a directory{hint}"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_program_names_are_recognised() {
        assert!(looks_like_windows_program(
            "/mnt/c/Windows/System32/cmd.exe"
        ));
        assert!(looks_like_windows_program("CMD.EXE"));
        assert!(looks_like_windows_program(r"C:\tools\x.bat"));
        assert!(!looks_like_windows_program("/bin/bash"));
        assert!(!looks_like_windows_program("python3"));
    }

    #[test]
    fn windows_paths_map_to_mnt() {
        assert_eq!(
            windows_to_wsl(r"C:\Users\loner").as_deref(),
            Some("/mnt/c/Users/loner")
        );
        assert_eq!(
            windows_to_wsl("C:/Users/loner").as_deref(),
            Some("/mnt/c/Users/loner")
        );
        assert_eq!(windows_to_wsl(r"d:\work").as_deref(), Some("/mnt/d/work"));
        assert_eq!(windows_to_wsl("C:").as_deref(), Some("/mnt/c"));
        assert_eq!(windows_to_wsl(r"C:\").as_deref(), Some("/mnt/c"));
    }

    #[test]
    fn linux_paths_are_left_alone() {
        for p in [
            "/home/loner",
            "relative/dir",
            "~/x",
            r"\\wsl$\debian\home",
            "C:foo",
        ] {
            assert_eq!(windows_to_wsl(p), None, "{p} must not be rewritten");
        }
    }

    #[test]
    fn a_windows_path_survives_when_the_drive_is_not_mounted() {
        // `/mnt/zz` does not exist anywhere; the value must come back as typed
        // rather than turning into a path that never existed.
        let raw = r"zz:\nothing";
        assert_eq!(to_linux_path(raw), raw);
    }

    #[test]
    fn require_dir_rejects_a_missing_directory() {
        let err = require_dir("/definitely-not-here-xyz")
            .unwrap_err()
            .to_string();
        assert!(err.contains("is not a directory"), "{err}");
        assert!(err.contains("/definitely-not-here-xyz"), "{err}");
    }

    #[test]
    fn require_dir_accepts_tmp() {
        assert_eq!(require_dir("/tmp").unwrap(), "/tmp");
    }
}
