//! RemoteSshDomain 端到端：russh 认证、PTY shell 输入输出与 resize。

mod common;

use common::{receive_until, TestConfig};
use emulator::domain::{Domain, PaneConfig, PaneKind, RemoteSshDomain};
use emulator::platform::Size;
use std::time::Duration;

const SSH_OK: &[u8] = b"XSCORE_SSH_DOMAIN_OK";

#[tokio::test]
#[ignore = "requires test.config with a reachable SSH host"]
async fn ssh_shell_roundtrip_and_resize() {
    let config = TestConfig::load();
    let domain = RemoteSshDomain::new();

    let pane = domain
        .spawn_pane(&PaneConfig::Ssh {
            host: config.ssh_host,
            port: config.ssh_port,
            user: config.ssh_user,
            identity: None,
            password: Some(config.ssh_password),
            cols: 80,
            rows: 24,
        })
        .await
        .expect("connect and start remote SSH shell");

    assert_eq!(pane.kind(), PaneKind::Ssh);
    pane.resize(Size::new(100, 30))
        .await
        .expect("resize remote SSH PTY");

    let mut output = pane.subscribe();
    pane.send_input(b"printf 'XSCORE_%s\\n' 'SSH_DOMAIN_OK'\n")
        .await
        .expect("write command to remote SSH shell");

    let received = receive_until(&mut output, SSH_OK, Duration::from_secs(20)).await;
    let text = String::from_utf8_lossy(&received);
    assert!(
        received
            .windows(SSH_OK.len())
            .any(|window| window == SSH_OK),
        "expected SSH output marker, got: {text:?}"
    );

    pane.send_input(b"exit\n")
        .await
        .expect("close remote SSH shell");
    pane.close().await.expect("close SSH pane");
}
