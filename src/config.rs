use std::fs::OpenOptions;
use std::io::{Read, Write};

use chrono::{DateTime, Local};
use glob::glob;
use serde::{Deserialize, Serialize};
use serde_inline_default::serde_inline_default;
use tokio::fs;
use tracing::{error, info};

#[serde_inline_default]
#[derive(Serialize, Deserialize, Debug)]
pub struct ServerConfig {
    #[serde_inline_default(String::from("mysql://toyou:tuyou123@localhost/tuyou"))]
    pub url: String,
    #[serde_inline_default(String::from("info"))]
    pub trace_level: String,
    #[serde_inline_default(false)]
    pub sqlx_debug: bool,
    #[serde_inline_default(String::from("http://127.0.0.1:8102/admin/cleanup"))]
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

pub async fn rename_log(now: DateTime<Local>) {
    fs::create_dir_all("logs").await.unwrap();
    let file_name = format!("logs/{}-least.cleanup.log", now.format("%Y-%m-%d"));
    if fs::try_exists(file_name.clone()).await.unwrap() {
        let mut new_name = file_name.clone();
        let mut file_name_offset = 0;
        while fs::try_exists(new_name.clone()).await.unwrap() {
            file_name_offset += 1;
            new_name = format!("logs/{}-{file_name_offset}.cleanup.log", now.format("%Y-%m-%d"));
        }

        fs::rename(file_name.clone(), new_name).await.unwrap();
    }
}

pub async fn check_trash_dir(a_week_earlier: DateTime<Local>, now: DateTime<Local>) -> String {
    //check dir
    if !std::path::Path::new("trash").exists() {
        std::fs::create_dir("trash").unwrap();
    }

    //remove outdated
    for dir in glob("trash/*").unwrap() {
        let name = dir.unwrap().display().to_string();
        let name = name.split("/").last().unwrap();
        let date = DateTime::parse_from_str(&(name.to_string() + " 00:00:00 +0800"), "%Y-%m-%d %H:%M:%S %z");
        if date.is_err() {
            error!("{name} is not parseable");
            continue;
        }
        let date = date.unwrap();
        if date < a_week_earlier {
            info!("remove outdated trash: {}", name);
            fs::remove_dir_all(format!("trash/{}", name)).await.unwrap();
        }
    }
    let trash_name = format!("trash/{}", now.format("%Y-%m-%d"));
    fs::create_dir_all(&trash_name).await.unwrap();

    return trash_name;
}