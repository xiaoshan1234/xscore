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
    let tmux_mode = pairs.remove("tmux").unwrap_or_else(|| "auto".into());
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

    let sink = SinkConfig::Tmux {
        mode: tmux_mode,
        session_name: (!name.is_empty()).then(|| name.clone()),
    };

    Ok((ConnectSpec { name, transport, sink }, address))
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
}
