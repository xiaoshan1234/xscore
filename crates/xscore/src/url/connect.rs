// url/connect.rs
use crate::core::{Error, LinkAddress, Result};
use crate::model::{ConnectSpec, SinkConfig, TransportConfig};
use std::collections::HashMap;

pub fn parse(s: &str) -> Result<(ConnectSpec, LinkAddress)> {
    let url = url::Url::parse(s).map_err(|e| Error::Url(e.to_string()))?;
    let scheme = url.scheme();
    let host = url.host_str().map(|s| s.to_string());
    let user = (!url.username().is_empty()).then(|| url.username().to_string());

    let mut pairs: HashMap<String, String> = url.query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
    let tmux = pairs.remove("tmux");
    let name = pairs.remove("name").unwrap_or_default();

    let (transport, address) = match scheme {
        "shell" | "sh" => {
            let command = url.path().to_string();
            if command.is_empty() { return Err(Error::Url("shell URL needs command".into())); }
            (TransportConfig::Shell { command: command.clone(), args: vec![], cwd: None },
             LinkAddress::Shell { command, args: vec![] })
        }
        "serial" => {
            let path = url.path().to_string();
            if path.is_empty() { return Err(Error::Url("serial URL needs path".into())); }
            let baud: u32 = pairs.remove("baud")
                .ok_or_else(|| Error::Url("serial needs baud".into()))?
                .parse().map_err(|e: std::num::ParseIntError| Error::Url(e.to_string()))?;
            let data_bits: u8 = pairs.remove("data_bits").unwrap_or_else(|| "8".into()).parse()
                .map_err(|e: std::num::ParseIntError| Error::Url(e.to_string()))?;
            let parity = pairs.remove("parity").unwrap_or_else(|| "n".into());
            let stop_bits: u8 = pairs.remove("stop_bits").unwrap_or_else(|| "1".into()).parse()
                .map_err(|e: std::num::ParseIntError| Error::Url(e.to_string()))?;
            (TransportConfig::Serial { path: path.clone(), baud, data_bits, parity, stop_bits },
             LinkAddress::Serial { path })
        }
        "ssh" => {
            let host = host.ok_or_else(|| Error::Url("ssh URL needs host".into()))?;
            let user = user.ok_or_else(|| Error::Url("ssh URL needs user".into()))?;
            let port = url.port().unwrap_or(22);
            let identity = pairs.remove("identity");
            let proxy = pairs.remove("proxy");
            (TransportConfig::Ssh { host: host.clone(), port, user: user.clone(),
                                   identity: identity.clone(), proxy: proxy.clone() },
             LinkAddress::Ssh { host, port, user, identity, proxy })
        }
        other => return Err(Error::Url(format!("unknown scheme: {other}"))),
    };

    // Sink default: direct transport I/O (Raw). Opt in to the tmux
    // server (TmuxSink) with `?tmux=on` (mode=auto) or `?tmux=auto|local|remote`.
    // The user requested: "我的本来的目的是支持 tmux 会话的管理，不是所有 link
    // 都要走 tmux" — tmux is one option, not the default.
    let session_name = (!name.is_empty()).then(|| name.clone());
    let sink = match tmux.as_deref() {
        Some("on") => SinkConfig::Tmux { mode: "auto".into(), session_name },
        Some("auto") | Some("local") | Some("remote") => {
            SinkConfig::Tmux { mode: tmux.unwrap(), session_name }
        }
        // "off" or absent → RawSink (direct transport I/O)
        _ => SinkConfig::Raw,
    };

    Ok((ConnectSpec { name, transport, sink, address: address.clone() }, address))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::TransportConfig;
    use crate::core::LinkAddress;

    #[test]
    fn parse_ssh_url_with_tmux() {
        let (spec, addr) = parse("ssh://root@10.0.0.1:22?identity=~/.ssh/id_ed25519").unwrap();
        assert!(matches!(spec.transport, TransportConfig::Ssh { .. }));
        assert_eq!(addr, LinkAddress::Ssh {
            host: "10.0.0.1".into(), port: 22, user: "root".into(),
            identity: Some("~/.ssh/id_ed25519".into()), proxy: None,
        });
    }

    #[test]
    fn parse_shell_url_yields_shell_address() {
        let (_, addr) = parse("shell:///bin/bash").unwrap();
        assert_eq!(addr, LinkAddress::Shell { command: "/bin/bash".into(), args: vec![] });
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

    // ----- 2026-10-19 design pivot: tmux is opt-in, not default -----

    #[test]
    fn parse_shell_url_default_is_raw_sink() {
        // No `?tmux=` query → SinkConfig::Raw (no tmux server).
        let (spec, _) = parse("shell:///bin/bash").unwrap();
        assert!(matches!(spec.sink, SinkConfig::Raw),
            "expected raw sink by default, got {:?}", spec.sink);
    }

    #[test]
    fn parse_shell_url_tmux_off_is_raw_sink() {
        let (spec, _) = parse("shell:///bin/bash?tmux=off").unwrap();
        assert!(matches!(spec.sink, SinkConfig::Raw));
    }

    #[test]
    fn parse_shell_url_tmux_on_is_tmux_sink_auto() {
        let (spec, _) = parse("shell:///bin/bash?tmux=on").unwrap();
        assert!(matches!(spec.sink,
            SinkConfig::Tmux { ref mode, .. } if mode == "auto"),
            "expected tmux/auto, got {:?}", spec.sink);
    }

    #[test]
    fn parse_shell_url_tmux_local_is_tmux_sink_local() {
        let (spec, _) = parse("shell:///bin/bash?tmux=local").unwrap();
        assert!(matches!(spec.sink,
            SinkConfig::Tmux { ref mode, .. } if mode == "local"),
            "expected tmux/local, got {:?}", spec.sink);
    }

    #[test]
    fn parse_shell_url_tmux_remote_is_tmux_sink_remote() {
        let (spec, _) = parse("shell:///bin/bash?tmux=remote").unwrap();
        assert!(matches!(spec.sink,
            SinkConfig::Tmux { ref mode, .. } if mode == "remote"),
            "expected tmux/remote, got {:?}", spec.sink);
    }

    #[test]
    fn parse_shell_url_with_name_sets_tmux_session_name() {
        // `?name=X` only meaningful with `?tmux=...`
        let (spec, _) = parse("shell:///bin/bash?tmux=on&name=build").unwrap();
        assert!(matches!(spec.sink,
            SinkConfig::Tmux { session_name: Some(ref n), .. } if n == "build"),
            "expected tmux w/ name=build, got {:?}", spec.sink);
    }
}
