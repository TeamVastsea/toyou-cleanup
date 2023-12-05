use std::collections::HashMap;
use std::time::Instant;

use glob::glob;
use lazy_static::lazy_static;
use sea_orm::{ActiveModelTrait, ConnectOptions, Database, DatabaseConnection, DeleteResult, EntityTrait};
use sea_orm::ActiveValue::Set;
use tokio::fs;
use tracing::{debug, info};
use tracing_appender::non_blocking;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{EnvFilter, fmt, Registry};
use tracing_subscriber::fmt::time::ChronoLocal;
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

    let file_appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_suffix("cleanup.log")
        .build("logs")?;
    let (non_blocking_appender, _guard) = non_blocking(file_appender);

    let formatting_layer = fmt::layer()
        .with_writer(std::io::stderr)
        .with_timer(ChronoLocal::new("%Y-%m-%d %H:%M:%S%.f(%:z)".to_string()));
    let file_layer = fmt::layer()
        .with_timer(ChronoLocal::new("%Y-%m-%d %H:%M:%S%.f(%:z)".to_string()))
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

    let time_description = format!("{:?}", start.elapsed());
    debug!("connected in {time_description}");

    let all_pictures = Picture::find().all(&db).await?;
    let all_user_pictures = UserPicture::find().all(&db).await?;
    let unused = get_unused_pictures(all_pictures, all_user_pictures).await;
    deal_unused(unused, &db).await;

    let time_description = format!("{:?}", start.elapsed());
    info!("unused pictures removed in {time_description}.");

    let all_used_pictures = Picture::find().all(&db).await?;
    remove_unlinked_pictures(all_used_pictures).await?;

    let time_description = format!("{:?}", start.elapsed());
    info!("unlinked pictures removed in {time_description}.");

    remove_empty_folder().await?;
    let time_description = format!("{:?}", start.elapsed());
    info!("empty folder removed in {time_description}.");
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

async fn remove_unlinked_pictures(pictures: Vec<picture::Model>) -> Result<(), Box<dyn std::error::Error>> {
    let mut used_list: Vec<&str> = Vec::new();

    for picture in &pictures {
        used_list.push(&picture.original);
        used_list.push(&picture.thumbnail);
        used_list.push(&picture.watermark);
    }

    for entry in glob("pictures/**/*.*")? {
        let name = entry?.display().to_string();
        if !used_list.contains(&name.as_str()) {
            info!("removing unlinked file: {name}");
            fs::copy(&name, "trash/".to_string() + &name.split("/").last().unwrap()).await?;
            fs::remove_file(name).await?;
        }
    }

    Ok(())
}

async fn remove_empty_folder() -> Result<(), Box<dyn std::error::Error>> {
    for entry in glob("pictures/*")? {
        let entry = entry?;
        let inner = format!("{}/*.*", &entry.display().to_string());
        let mut inner_paths = glob(&inner)?;
        if inner_paths.next().is_none() {
            info!("removing empty folder: {}", entry.display());
            fs::remove_dir(entry.display().to_string()).await?;
        }
    }

    Ok(())
}