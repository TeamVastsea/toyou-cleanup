use std::fs::OpenOptions;
use std::io::{Read, Write};

use serde::{Deserialize, Serialize};
use serde_inline_default::serde_inline_default;
use tracing::error;

#[serde_inline_default]
#[derive(Serialize, Deserialize, Debug)]
pub struct ServerConfig {
    #[serde_inline_default(String::from("mysql://toyou:tuyou123@localhost/tuyou"))]
    pub url: String,
    #[serde_inline_default(String::from("info"))]
    pub trace_level: String,
    #[serde_inline_default(false)]
    pub sqlx_debug: bool,
    #[serde_inline_default(String::from("127.0.0.1:8102/admin/cleanup"))]
    pub mark_url: String,
    #[serde_inline_default(false)]
    pub ignore_mark_fail: bool,
}

pub fn get_config() -> ServerConfig {
    let mut raw_config = String::new();
    let mut file = OpenOptions::new().read(true).write(true).create(true).open("config.toml").expect("Cannot open 'config.toml'");
    file.read_to_string(&mut raw_config).unwrap();

    let config: ServerConfig = toml::from_str(&raw_config).unwrap();

    if toml::to_string_pretty(&config).unwrap() != raw_config {
        save(&config)
    }

    config
}

pub fn save(config: &ServerConfig) {
    error!("Config changed, please edit and restart");
    let config_str = toml::to_string_pretty(config).unwrap();

    let mut file = OpenOptions::new().write(true).truncate(true).open("config.toml").expect("Cannot open 'config.toml'");
    file.write(config_str.as_bytes()).unwrap();

    panic!("config changed");
}