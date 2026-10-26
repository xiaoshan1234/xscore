//! Normalize shell-family URL schemes into one `Shell` transport.
//!
//! Bash, Windows PowerShell, `pwsh`, and a WSL shell all ultimately start a
//! child process behind ConPTY/PTY. The differences belong in the program and
//! argv, not in the byte-transport abstraction.

use crate::core::LinkAddress;
use crate::model::TransportConfig;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellLaunch {
    command: String,
    args: Vec<String>,
    cwd: Option<String>,
}

impl ShellLaunch {
    pub fn new(command: String, args: Vec<String>, cwd: Option<String>) -> Self {
        Self { command, args, cwd }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_keeps_program_arguments_and_cwd_separate() {
        let launch = ShellLaunch::new(
            r"C:\Windows\System32\wsl.exe".into(),
            vec!["--distribution".into(), "Ubuntu".into()],
            Some(r"C:\src".into()),
        );
        assert_eq!(launch.command(), r"C:\Windows\System32\wsl.exe");
        assert_eq!(launch.args(), ["--distribution", "Ubuntu"]);
        assert!(matches!(
            launch.transport(),
            TransportConfig::Shell {
                cwd: Some(ref cwd),
                ..
            } if cwd == r"C:\src"
        ));
    }

    #[test]
    fn address_matches_transport_fields() {
        let launch = ShellLaunch::new("bash".into(), vec!["-l".into()], None);
        assert_eq!(
            launch.address(),
            LinkAddress::Shell {
                command: "bash".into(),
                args: vec!["-l".into()],
            }
        );
    }
}
