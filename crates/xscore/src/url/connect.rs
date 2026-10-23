// url/connect.rs
use crate::core::{Error, LinkAddress, Result};
use crate::model::{ConnectSpec, TmuxHostConfig, TransportConfig};
use crate::transport::launcher::ShellLaunch;
use percent_encoding::percent_decode_str;
use std::collections::HashMap;

/// Settings that describe the *pane* the link is first shown in, rather than
/// the connection itself. They live in the URL's query so that a session can be
/// created from one string (`session new dev 'shell:///bin/bash?cols=120'`),
/// and so that `link add` accepts the same grammar.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PaneParams {
    pub cwd: Option<String>,
    pub cols: Option<u16>,
    pub rows: Option<u16>,
}

/// What a URL may say about a tmux session: where the server is, and how the
/// first window/pane starts.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TmuxTarget {
    pub host: TmuxHostConfig,
    pub cmd: Option<String>,
    pub cwd: Option<String>,
    pub cols: Option<u16>,
    pub rows: Option<u16>,
}

pub fn parse(s: &str) -> Result<(ConnectSpec, LinkAddress)> {
    let (spec, address, _) = parse_with_pane(s)?;
    Ok((spec, address))
}

/// Parse a pipe-link URL. Accepts `cwd`, `cols` and `rows` as pane settings and
/// rejects anything it does not understand: a silently ignored `cols=12O` is a
/// bug report waiting to happen.
pub fn parse_with_pane(s: &str) -> Result<(ConnectSpec, LinkAddress, PaneParams)> {
    let url = url::Url::parse(s).map_err(|e| Error::Url(e.to_string()))?;
    let scheme = url.scheme();
    let host = url.host_str().map(|s| s.to_string());
    let user = (!url.username().is_empty()).then(|| url.username().to_string());

    let args: Vec<String> = url
        .query_pairs()
        .filter(|(key, _)| key == "arg")
        .map(|(_, value)| value.into_owned())
        .collect();
    if scheme != "shell" && !args.is_empty() {
        return Err(Error::Url(
            "the `arg` query parameter applies only to shell:// URLs".into(),
        ));
    }
    let mut pairs: HashMap<String, String> = url
        .query_pairs()
        .filter(|(key, _)| key != "arg")
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    let name = pairs.remove("name").unwrap_or_default();
    let mut pane = take_pane_params(&mut pairs)?;

    let (transport, address) = match scheme {
        "shell" => {
            if host.is_some() || user.is_some() || url.port().is_some() {
                return Err(Error::Url(
                    "shell URL takes its program in the path; use \
                     shell:///<executable-path>, not shell://executable"
                        .into(),
                ));
            }
            let command = decode_shell_path(url.path());
            if command.is_empty() || command == "/" {
                return Err(Error::Url("shell URL needs command".into()));
            }
            let launch = ShellLaunch::new(command, args.clone(), pane.cwd.clone());
            (launch.transport(), launch.address())
        }
        "serial" => {
            let path = url.path().to_string();
            if path.is_empty() {
                return Err(Error::Url("serial URL needs path".into()));
            }
            if pane.cwd.is_some() {
                return Err(Error::Url(
                    "cwd only applies to shell:// (or --tmux); a serial link has no working directory"
                        .into(),
                ));
            }
            let baud: u32 = pairs
                .remove("baud")
                .ok_or_else(|| Error::Url("serial needs baud".into()))?
                .parse()
                .map_err(|e: std::num::ParseIntError| Error::Url(e.to_string()))?;
            let data_bits: u8 = pairs
                .remove("data_bits")
                .unwrap_or_else(|| "8".into())
                .parse()
                .map_err(|e: std::num::ParseIntError| Error::Url(e.to_string()))?;
            let parity = pairs.remove("parity").unwrap_or_else(|| "n".into());
            let stop_bits: u8 = pairs
                .remove("stop_bits")
                .unwrap_or_else(|| "1".into())
                .parse()
                .map_err(|e: std::num::ParseIntError| Error::Url(e.to_string()))?;
            (
                TransportConfig::Serial { path: path.clone(), baud, data_bits, parity, stop_bits },
                LinkAddress::Serial { path },
            )
        }
        "ssh" => {
            let host = host.ok_or_else(|| Error::Url("ssh URL needs host".into()))?;
            let user = user.ok_or_else(|| Error::Url("ssh URL needs user".into()))?;
            if pane.cwd.is_some() {
                return Err(Error::Url(
                    "cwd only applies to shell:// (or --tmux); an ssh link has no working directory"
                        .into(),
                ));
            }
            let port = url.port().unwrap_or(22);
            let identity = pairs.remove("identity");
            let proxy = pairs.remove("proxy");
            (
                TransportConfig::Ssh {
                    host: host.clone(),
                    port,
                    user: user.clone(),
                    identity: identity.clone(),
                    proxy: proxy.clone(),
                },
                LinkAddress::Ssh { host, port, user, identity, proxy },
            )
        }
        other => return Err(Error::Url(format!("unknown scheme: {other}"))),
    };

    // The transport may have consumed `cwd` itself; keep the pane view in sync
    // with what the transport will actually do.
    if matches!(transport, TransportConfig::Shell { cwd: Some(_), .. }) {
        pane.cwd = None;
    }
    reject_unknown(&pairs)?;

    // A URL only says how to connect. "Does this go through tmux?" is a
    // property of the session (`SessionBackend`), so there is nothing to opt
    // into here — the old tmux query flag is gone (ADR 0010 §D7).
    Ok((ConnectSpec { name, transport, address: address.clone() }, address, pane))
}

/// Parse the positional argument of `session new --tmux`.
///
/// Four accepted shapes. The scheme is what says how the tmux protocol is
/// carried — that is the link:
///
///   * omitted                → the local default server
///   * `shell://[?cmd=…]`     → the local server, explicitly
///   * `ssh://[user@]host[:port][?identity=…&cmd=…&cwd=…&cols=…&rows=…]`
///   * `?cmd=…&cols=…`        → local server, options only (shorthand)
///
/// `--socket` stays a flag (it also has an environment fallback), so a socket
/// name in the query is refused rather than silently competing with it.
pub fn parse_tmux_target(s: &str) -> Result<TmuxTarget> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(TmuxTarget::default());
    }

    // `shell://` — the local server, named explicitly. No path: a command for
    // the first window is `cmd=…`, never a path, so `shell:///bin/sh` is a
    // mistake worth reporting.
    if let Some(rest) = s.strip_prefix("shell://") {
        let (path, query) = match rest.split_once('?') {
            Some((path, query)) => (path, Some(query)),
            None => (rest, None),
        };
        if !path.is_empty() && path != "/" {
            return Err(Error::Url(
                "shell:// takes no path for a tmux session; the first window's command is `cmd=…`"
                    .into(),
            ));
        }
        let mut pairs = query.map(query_map).unwrap_or_default();
        let mut target = TmuxTarget::default();
        take_tmux_params(&mut pairs, &mut target)?;
        reject_unknown(&pairs)?;
        return Ok(target);
    }

    // Options without a host: `?cmd=cat`.
    if let Some(query) = s.strip_prefix('?') {
        let mut pairs = query_map(query);
        let mut target = TmuxTarget::default();
        take_tmux_params(&mut pairs, &mut target)?;
        reject_unknown(&pairs)?;
        return Ok(target);
    }

    let url = url::Url::parse(s).map_err(|e| Error::Url(e.to_string()))?;
    if url.scheme() != "ssh" {
        return Err(Error::Url(format!(
            "with --tmux the URL names the tmux host; use ssh://… or ?cmd=… (got {}://)",
            url.scheme()
        )));
    }
    let host = url
        .host_str()
        .ok_or_else(|| Error::Url("ssh URL needs host".into()))?
        .to_string();
    let destination = if url.username().is_empty() {
        host
    } else {
        format!("{}@{}", url.username(), host)
    };

    let mut args: Vec<String> = Vec::new();
    if let Some(port) = url.port() {
        args.push("-p".into());
        args.push(port.to_string());
    }

    let mut pairs = query_map(url.query().unwrap_or(""));
    if let Some(identity) = pairs.remove("identity") {
        args.push("-i".into());
        args.push(identity);
    }

    let mut target = TmuxTarget {
        host: TmuxHostConfig::Ssh { destination, args },
        ..TmuxTarget::default()
    };
    take_tmux_params(&mut pairs, &mut target)?;
    reject_unknown(&pairs)?;
    Ok(target)
}

fn query_map(query: &str) -> HashMap<String, String> {
    url::form_urlencoded::parse(query.as_bytes())
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}

/// A URL path is percent-encoded. Windows drive paths appear as `/C:/...`
/// inside `shell:///`; remove only that transport slash before handing the
/// program to the Windows process spawner.
fn decode_shell_path(path: &str) -> String {
    let decoded = percent_decode_str(path).decode_utf8_lossy().into_owned();
    let bytes = decoded.as_bytes();
    if bytes.len() >= 3
        && bytes[0] == b'/'
        && bytes[1].is_ascii_alphabetic()
        && bytes[2] == b':'
    {
        decoded[1..].to_string()
    } else {
        decoded
    }
}

fn take_pane_params(pairs: &mut HashMap<String, String>) -> Result<PaneParams> {
    let mut pane = PaneParams {
        // `C:\Users` is what a WSL user copies out of Explorer; translate it
        // before anything tries to chdir there.
        cwd: pairs
            .remove("cwd")
            .filter(|s| !s.is_empty())
            .map(|s| crate::util::to_linux_path(&s)),
        cols: None,
        rows: None,
    };
    pane.cols = take_dim(pairs, "cols")?;
    pane.rows = take_dim(pairs, "rows")?;
    Ok(pane)
}

fn take_tmux_params(pairs: &mut HashMap<String, String>, target: &mut TmuxTarget) -> Result<()> {
    target.cmd = pairs.remove("cmd").filter(|s| !s.trim().is_empty());
    target.cwd = pairs
        .remove("cwd")
        .filter(|s| !s.is_empty())
        .map(|s| crate::util::to_linux_path(&s));
    target.cols = take_dim(pairs, "cols")?;
    target.rows = take_dim(pairs, "rows")?;
    if pairs.remove("socket").is_some() {
        return Err(Error::Url(
            "put the tmux socket on --socket, not in the URL".into(),
        ));
    }
    Ok(())
}

fn take_dim(pairs: &mut HashMap<String, String>, key: &str) -> Result<Option<u16>> {
    match pairs.remove(key) {
        None => Ok(None),
        Some(raw) => {
            let v: u16 = raw
                .parse()
                .map_err(|_| Error::Url(format!("{key} must be a number from 1 to 65535, got `{raw}`")))?;
            if v == 0 {
                return Err(Error::Url(format!("{key} must be greater than 0")));
            }
            Ok(Some(v))
        }
    }
}

fn reject_unknown(pairs: &HashMap<String, String>) -> Result<()> {
    if pairs.is_empty() {
        return Ok(());
    }
    let mut keys: Vec<&str> = pairs.keys().map(|k| k.as_str()).collect();
    keys.sort_unstable();
    Err(Error::Url(format!(
        "unknown URL parameter(s): {}",
        keys.join(", ")
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::LinkAddress;
    use crate::model::TmuxHostConfig;
    use crate::model::TransportConfig;

    #[test]
    fn parse_ssh_url_with_identity() {
        let (spec, addr) = parse("ssh://root@10.0.0.1:22?identity=~/.ssh/id_ed25519").unwrap();
        assert!(matches!(spec.transport, TransportConfig::Ssh { .. }));
        assert_eq!(
            addr,
            LinkAddress::Ssh {
                host: "10.0.0.1".into(),
                port: 22,
                user: "root".into(),
                identity: Some("~/.ssh/id_ed25519".into()),
                proxy: None,
            }
        );
    }

    #[test]
    fn parse_shell_url_yields_shell_address() {
        let (_, addr) = parse("shell:///bin/bash").unwrap();
        assert_eq!(addr, LinkAddress::Shell { command: "/bin/bash".into(), args: vec![] });
    }

    #[test]
    fn parse_wsl_executable_with_repeated_args() {
        let (spec, addr, pane) = parse_with_pane(
            "shell:///C:/Windows/System32/wsl.exe?arg=--distribution&arg=Ubuntu\
             &arg=--cd&arg=/home/me/src&arg=--exec&arg=/bin/bash&cols=120",
        )
        .unwrap();
        assert_eq!(
            spec.transport,
            TransportConfig::Shell {
                command: "C:/Windows/System32/wsl.exe".into(),
                args: vec![
                    "--distribution".into(),
                    "Ubuntu".into(),
                    "--cd".into(),
                    "/home/me/src".into(),
                    "--exec".into(),
                    "/bin/bash".into(),
                ],
                cwd: None,
            }
        );
        assert_eq!(
            addr,
            LinkAddress::Shell {
                command: "C:/Windows/System32/wsl.exe".into(),
                args: vec![
                    "--distribution".into(),
                    "Ubuntu".into(),
                    "--cd".into(),
                    "/home/me/src".into(),
                    "--exec".into(),
                    "/bin/bash".into(),
                ],
            }
        );
        assert_eq!(pane.cols, Some(120));
    }

    #[test]
    fn parse_wsl_executable_without_args() {
        let (spec, addr, _) =
            parse_with_pane("shell:///C:/Windows/System32/wsl.exe").unwrap();
        assert_eq!(
            spec.transport,
            TransportConfig::Shell {
                command: "C:/Windows/System32/wsl.exe".into(),
                args: vec![],
                cwd: None,
            }
        );
        assert_eq!(
            addr,
            LinkAddress::Shell {
                command: "C:/Windows/System32/wsl.exe".into(),
                args: vec![],
            }
        );
    }

    #[test]
    fn shell_url_refuses_authority_and_unknown_options() {
        let err = parse("shell://bash").unwrap_err().to_string();
        assert!(err.contains("shell:///"), "{err}");
        assert!(parse("shell:///bin/bash?frontend=foo").is_err());
    }

    #[test]
    fn shell_path_is_percent_decoded_and_args_are_preserved() {
        let (spec, address, pane) = parse_with_pane(
            "shell:///C:/Program%20Files/PowerShell/7/pwsh.exe\
             ?arg=-NoLogo&arg=-NoProfile&cwd=/tmp&cols=120",
        )
        .unwrap();
        assert_eq!(
            spec.transport,
            TransportConfig::Shell {
                command: "C:/Program Files/PowerShell/7/pwsh.exe".into(),
                args: vec!["-NoLogo".into(), "-NoProfile".into()],
                cwd: Some("/tmp".into()),
            }
        );
        assert_eq!(address.kind(), "shell");
        assert_eq!(pane.cwd, None);
    }

    #[test]
    fn argument_query_is_shell_only() {
        assert!(parse("serial:///dev/ttyUSB0?baud=9600&arg=x").is_err());
    }

    #[test]
    fn parse_serial_url_yields_serial_address() {
        let (_, addr) = parse("serial:///dev/ttyUSB0?baud=115200").unwrap();
        assert_eq!(addr, LinkAddress::Serial { path: "/dev/ttyUSB0".into() });
    }

    #[test]
    fn parse_rejects_unknown_scheme() {
        assert!(parse("telnet://host").is_err());
    }

    #[test]
    fn ssh_with_different_identity_different_address() {
        let (_, a1) = parse("ssh://u@h:22?identity=k1").unwrap();
        let (_, a2) = parse("ssh://u@h:22?identity=k2").unwrap();
        assert_ne!(a1, a2);
    }

    #[test]
    fn pane_geometry_comes_from_the_query() {
        let (_, _, pane) = parse_with_pane("shell:///bin/bash?cols=120&rows=40").unwrap();
        assert_eq!(pane.cols, Some(120));
        assert_eq!(pane.rows, Some(40));
        assert_eq!(pane.cwd, None);
    }

    #[test]
    fn cwd_belongs_to_the_shell_transport() {
        let (spec, _, pane) = parse_with_pane("shell:///bin/bash?cwd=/var/log").unwrap();
        match spec.transport {
            TransportConfig::Shell { cwd, .. } => assert_eq!(cwd.as_deref(), Some("/var/log")),
            other => panic!("expected a shell transport, got {other:?}"),
        }
        assert_eq!(pane.cwd, None, "the pane view must not claim it twice");
    }

    #[test]
    fn cwd_on_a_serial_url_is_refused() {
        let err = parse_with_pane("serial:///dev/ttyUSB0?baud=9600&cwd=/tmp")
            .unwrap_err()
            .to_string();
        assert!(err.contains("cwd only applies to shell"), "{err}");
    }

    #[test]
    fn unknown_parameter_is_refused() {
        let err = parse_with_pane("shell:///bin/bash?col=120").unwrap_err().to_string();
        assert!(err.contains("unknown URL parameter"), "{err}");
        assert!(err.contains("col"), "{err}");
    }

    #[test]
    fn a_bad_dimension_is_refused() {
        let err = parse_with_pane("shell:///bin/bash?cols=12O").unwrap_err().to_string();
        assert!(err.contains("cols must be a number"), "{err}");
        assert!(parse_with_pane("shell:///bin/bash?cols=0").is_err());
    }

    #[test]
    fn tmux_target_absent_is_local() {
        let t = parse_tmux_target("").unwrap();
        assert_eq!(t.host, TmuxHostConfig::Local);
        assert_eq!(t.cmd, None);
    }

    #[test]
    fn tmux_target_ssh_host_with_options() {
        let t = parse_tmux_target("ssh://dev@box:2222?cmd=htop&cols=100&rows=30").unwrap();
        assert_eq!(
            t.host,
            TmuxHostConfig::Ssh { destination: "dev@box".into(), args: vec!["-p".into(), "2222".into()] }
        );
        assert_eq!(t.cmd.as_deref(), Some("htop"));
        assert_eq!(t.cols, Some(100));
        assert_eq!(t.rows, Some(30));
    }

    #[test]
    fn tmux_target_without_user() {
        let t = parse_tmux_target("ssh://box").unwrap();
        assert_eq!(t.host, TmuxHostConfig::Ssh { destination: "box".into(), args: vec![] });
    }

    #[test]
    fn tmux_target_query_only_is_local() {
        let t = parse_tmux_target("?cmd=cat").unwrap();
        assert_eq!(t.host, TmuxHostConfig::Local);
        assert_eq!(t.cmd.as_deref(), Some("cat"));
    }

    #[test]
    fn tmux_target_shell_scheme_is_local() {
        let t = parse_tmux_target("shell://").unwrap();
        assert_eq!(t.host, TmuxHostConfig::Local);
        let t = parse_tmux_target("shell://?cmd=cat&cols=90").unwrap();
        assert_eq!(t.host, TmuxHostConfig::Local);
        assert_eq!(t.cmd.as_deref(), Some("cat"));
        assert_eq!(t.cols, Some(90));
    }

    #[test]
    fn tmux_target_shell_scheme_takes_no_path() {
        let err = parse_tmux_target("shell:///bin/bash").unwrap_err().to_string();
        assert!(err.contains("takes no path"), "{err}");
    }

    #[test]
    fn tmux_target_refuses_a_pipe_scheme() {
        let err = parse_tmux_target("serial:///dev/ttyS0?baud=9600").unwrap_err().to_string();
        assert!(err.contains("names the tmux host"), "{err}");
    }

    #[test]
    fn tmux_target_refuses_socket_in_the_query() {
        let err = parse_tmux_target("?socket=x").unwrap_err().to_string();
        assert!(err.contains("--socket"), "{err}");
    }
}
