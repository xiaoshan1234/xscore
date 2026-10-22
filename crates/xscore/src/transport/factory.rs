// transport/factory.rs
//
// Dispatches a `TransportConfig` to the corresponding concrete `Transport`
// impl. The match arms are exhaustive over `TransportConfig` (Shell, Serial,
// Ssh). Shell-family launchers such as WSL and PowerShell are normalized into
// the Shell variant by `launcher`.

use crate::core::Result;
use crate::model::TransportConfig;
use super::{serial::SerialTransport, shell::ShellTransport, ssh::SshTransport, Transport};

pub async fn transport_for(config: TransportConfig) -> Result<Box<dyn Transport>> {
    match config {
        TransportConfig::Shell { command, args, cwd } => {
            Ok(Box::new(ShellTransport::new(command, args, cwd).await?))
        }
        TransportConfig::Serial { path, baud, data_bits, parity, stop_bits } => {
            Ok(Box::new(SerialTransport::new(path, baud, data_bits, parity, stop_bits).await?))
        }
        TransportConfig::Ssh { host, port, user, identity, proxy } => {
            Ok(Box::new(SshTransport::connect(host, port, user, identity, proxy).await?))
        }
    }
}
