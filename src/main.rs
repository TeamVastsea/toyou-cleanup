use std::collections::HashMap;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use lazy_static::lazy_static;
use sea_orm::{ActiveModelTrait, ConnectOptions, Database, DatabaseConnection, DeleteResult, EntityTrait};
use sea_orm::ActiveValue::Set;
use tokio::fs;
use toml::Value::String;
use tracing::info;

use tracing_appender::{non_blocking, rolling};
use tracing_subscriber::{EnvFilter, fmt, Registry};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::config::ServerConfig;
use crate::entity::picture;
use crate::entity::prelude::{Picture, UserPicture};

mod entity;
mod config;

lazy_static! {
    static ref CONFIG: ServerConfig = config::get_config();
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    //time
    let start = Instant::now();

    //set up tracing
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&CONFIG.trace_level));

    let formatting_layer = fmt::layer().with_writer(std::io::stderr);
    let file_appender = rolling::daily("log", "log");
    let (non_blocking_appender, _guard) = non_blocking(file_appender);
    let file_layer = fmt::layer()
        .with_ansi(false)
        .with_writer(non_blocking_appender);
    Registry::default()
        .with(env_filter)
        .with(formatting_layer)
        .with(file_layer)
        .init();
    //check dir
    if !std::path::Path::new("trash").exists() {
        std::fs::create_dir("trash")?;
    }

    let time_description = format!("{:?}", start.elapsed());

    info!("started in {time_description}.");

    let mut opt = ConnectOptions::new(&CONFIG.url);
    opt.sqlx_logging(CONFIG.sqlx_debug);
    let db = Database::connect(opt).await?;

    let all_pictures = Picture::find().all(&db).await?;
    let all_user_pictures = UserPicture::find().all(&db).await?;
    let unused = get_unused_pictures(all_pictures, all_user_pictures).await;
    deal_unused(unused, &db).await;

    let time_description = format!("{:?}", start.elapsed());

    info!("finished in {time_description}.");
    Ok(())
}

async fn get_unused_pictures(pictures: Vec<picture::Model>, user_picture: Vec<entity::user_picture::Model>) -> Vec<picture::Model> {
    let mut used_map: HashMap<&str, picture::Model> = HashMap::new();
    for picture in &pictures {
        used_map.insert(&picture.pid, picture.clone());
    }

    for used in &user_picture {
        if used_map.contains_key(used.pid.as_str()) {
            used_map.remove(used.pid.as_str());
        }
    }

    let mut unused_vec = Vec::new();
    for (_, picture) in used_map {
        unused_vec.push(picture);
    }

    return unused_vec;
}

async fn deal_unused(pictures: Vec<picture::Model>, db: &DatabaseConnection) {
    for picture in pictures {
        if picture.available == 1 {
            info!("disabling: {}", picture.original);
            let mut active_picture: picture::ActiveModel = picture.into();
            active_picture.available = Set(0);
            active_picture.save(db).await.unwrap();
        } else {
            info!("removing file: {}", picture.original);
            fs::copy(&picture.original, "trash/".to_string() + &picture.original.split("/").last().unwrap()).await.unwrap();

            fs::remove_file(&picture.original).await.unwrap();
            fs::remove_file(&picture.thumbnail).await.unwrap();
            fs::remove_file(&picture.watermark).await.unwrap();

            let active_picture: picture::ActiveModel = picture.into();
            let res: DeleteResult = active_picture.delete(db).await.unwrap();
            assert_eq!(res.rows_affected, 1);
        }
    }
}