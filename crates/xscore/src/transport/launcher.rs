//! Normalize shell-family URL schemes into one `Shell` transport.
//!
//! Bash, Windows PowerShell, `pwsh`, and a WSL shell all ultimately start a
//! child process behind ConPTY/PTY. The differences belong in the program and
//! argv, not in the byte-transport abstraction.

use crate::core::{Error, LinkAddress, Result};
use crate::model::TransportConfig;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellLaunch {
    command: String,
    args: Vec<String>,
    cwd: Option<String>,
}

impl ShellLaunch {
    pub fn local(command: String, args: Vec<String>, cwd: Option<String>) -> Self {
        Self { command, args, cwd }
    }

    /// `wsl.exe` is the launcher; the Linux shell and directory are arguments
    /// interpreted by WSL rather than by the Windows child process.
    pub fn wsl(distribution: Option<String>, shell: Option<String>, cwd: Option<String>) -> Self {
        let distribution = clean(distribution);
        let shell = clean(shell);
        let cwd = clean(cwd);
        let mut args = Vec::new();
        if let Some(distribution) = distribution.as_deref() {
            args.extend(["--distribution".into(), distribution.into()]);
        }
        if let Some(cwd) = cwd.as_deref() {
            args.extend(["--cd".into(), cwd.into()]);
        }
        if let Some(shell) = shell.as_deref() {
            args.extend(["--exec".into(), shell.into()]);
        }
        Self {
            command: "wsl.exe".into(),
            args,
            // `--cd` changes the Linux child's directory. Changing the host
            // process cwd here would instead start wsl.exe from that location.
            cwd: None,
        }
    }

    /// The URL host selects the PowerShell generation:
    ///
    /// * omitted, `windows`, `5`, `5.1` -> `powershell.exe`
    /// * `pwsh`, `7`, `core` -> `pwsh.exe`
    pub fn powershell(edition: Option<&str>, cwd: Option<String>) -> Result<Self> {
        let edition = edition
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_ascii_lowercase);
        let command = match edition.as_deref() {
            None | Some("") | Some("windows") | Some("5") | Some("5.1") => "powershell.exe",
            Some("pwsh") | Some("7") | Some("core") => "pwsh.exe",
            Some(other) => {
                return Err(Error::Url(format!(
                    "unknown PowerShell edition `{other}`; use shell:///powershell \
                     or shell:///pwsh"
                )))
            }
        };
        Ok(Self {
            command: command.into(),
            args: vec!["-NoLogo".into()],
            cwd: clean(cwd),
        })
    }

    pub fn transport(&self) -> TransportConfig {
        TransportConfig::Shell {
            command: self.command.clone(),
            args: self.args.clone(),
            cwd: self.cwd.clone(),
        }
    }

    pub fn address(&self) -> LinkAddress {
        LinkAddress::Shell {
            command: self.command.clone(),
            args: self.args.clone(),
        }
    }

    pub fn command(&self) -> &str {
        &self.command
    }

    pub fn args(&self) -> &[String] {
        &self.args
    }
}

fn clean(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wsl_arguments_precede_the_linux_shell() {
        let launch = ShellLaunch::wsl(
            Some("Ubuntu".into()),
            Some("/bin/bash".into()),
            Some(r"C:\src".into()),
        );
        assert_eq!(launch.command(), "wsl.exe");
        assert_eq!(
            launch.args(),
            [
                "--distribution",
                "Ubuntu",
                "--cd",
                r"C:\src",
                "--exec",
                "/bin/bash",
            ]
        );
        assert!(matches!(
            launch.transport(),
            TransportConfig::Shell { cwd: None, .. }
        ));
    }

    #[test]
    fn powershell_editions_resolve_to_one_shell_transport() {
        let legacy = ShellLaunch::powershell(None, Some(r"C:\src".into())).unwrap();
        assert_eq!(legacy.command(), "powershell.exe");
        assert_eq!(legacy.args(), ["-NoLogo"]);

        let modern = ShellLaunch::powershell(Some("pwsh"), None).unwrap();
        assert_eq!(modern.command(), "pwsh.exe");
        assert!(matches!(modern.transport(), TransportConfig::Shell { .. }));
        assert!(ShellLaunch::powershell(Some("unknown"), None).is_err());
    }

    #[test]
    fn empty_shell_options_are_ignored() {
        let launch = ShellLaunch::wsl(Some("  ".into()), None, None);
        assert!(launch.args().is_empty());
    }
}
