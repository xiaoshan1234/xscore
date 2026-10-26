//! Link transport configuration owned by the infrastructure link layer.

use serde::{Deserialize, Serialize};

use super::address::LinkAddress;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "transport", rename_all = "snake_case")]
pub enum TransportConfig {
    Shell {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        cwd: Option<String>,
    },
    Serial {
        path: String,
        baud: u32,
        #[serde(default = "default_data_bits")]
        data_bits: u8,
        #[serde(default = "default_parity")]
        parity: String,
        #[serde(default = "default_stop_bits")]
        stop_bits: u8,
    },
    Ssh {
        host: String,
        #[serde(default = "default_ssh_port")]
        port: u16,
        user: String,
        #[serde(default)]
        identity: Option<String>,
        #[serde(default)]
        proxy: Option<String>,
    },
}

fn default_ssh_port() -> u16 {
    22
}

fn default_data_bits() -> u8 {
    8
}

fn default_parity() -> String {
    "n".into()
}

fn default_stop_bits() -> u8 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConnectSpec {
    #[serde(default = "default_name")]
    pub name: String,
    #[serde(flatten)]
    pub transport: TransportConfig,
    pub address: LinkAddress,
}

fn default_name() -> String {
    uuid::Uuid::now_v7().to_string()
}
