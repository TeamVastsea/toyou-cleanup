use std::collections::HashMap;
use std::time::Instant;

use chrono::{DateTime, Days, Local};
use glob::glob;
use lazy_static::lazy_static;
use sea_orm::{ActiveModelBehavior, ActiveModelTrait, ConnectOptions, Database, DatabaseConnection, EntityTrait, IntoActiveModel, ModelTrait};
use tokio::{fs, spawn};
use tracing::{debug, error, info};
use tracing_appender::non_blocking;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{EnvFilter, fmt, Registry};
use tracing_subscriber::fmt::time::ChronoLocal;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::config::ServerConfig;
use crate::entity::{permission, picture, user_picture};
use crate::entity::prelude::{Permission, Picture, UserPicture};

mod entity;
mod config;

lazy_static! {
    static ref CONFIG: ServerConfig = config::get_config();
}

const DEFAULT_GROUP: Group = Group {
    priority: 0,
    storage: 2048.0,
    restrictions: 50.0,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    //time
    let start = Instant::now();
    let now = Local::now();
    let a_week_earlier = now.checked_sub_days(Days::new(7)).unwrap();

    //set up tracing
    fs::create_dir_all("logs").await?;
    let file_name = format!("logs/{}-least.cleanup.log", now.format("%Y-%m-%d"));
    if fs::try_exists(file_name.clone()).await? {
        let mut new_name = file_name.clone();
        let mut file_name_offset = 0;
        while fs::try_exists(new_name.clone()).await? {
            file_name_offset += 1;
            new_name = format!("logs/{}-{file_name_offset}.cleanup.log", now.format("%Y-%m-%d"));
        }

        fs::rename(file_name.clone(), new_name).await?;
    }

    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&CONFIG.trace_level));

    let file_appender = RollingFileAppender::builder()
        .rotation(Rotation::NEVER)
        .filename_suffix(file_name)
        .build("")?;
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

    /******************** CHECK TRASH DIR *****************************/

    let time_description = format!("{:?}", start.elapsed());
    info!("started in {time_description}.");

    //check dir
    if !std::path::Path::new("trash").exists() {
        std::fs::create_dir("trash")?;
    }

    //remove outdated
    for dir in glob("trash/*")? {
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
            fs::remove_dir_all(format!("trash/{}", name)).await?;
        }
    }
    let trash_name = format!("trash/{}", now.format("%Y-%m-%d"));
    fs::create_dir_all(&trash_name).await?;

    let time_description = format!("{:?}", start.elapsed());
    info!("trash dir ready in {time_description}.");

    /******************** CONNECT TO DATABASE *************************/

    let mut opt = ConnectOptions::new(&CONFIG.url);
    opt.sqlx_logging(CONFIG.sqlx_debug);
    let db = Database::connect(opt).await?;

    let time_description = format!("{:?}", start.elapsed());
    debug!("connected in {time_description}.");

    let client = reqwest::Client::new();
    let result = client.post(&CONFIG.mark_url).send().await;
    if result.is_err() {
        error!("send mark request failed: {}.", result.err().unwrap().to_string());
        if !CONFIG.ignore_mark_fail {
            panic!("Cannot send mark request");
        }
    }

    /******************** GET ALL PICTURES ****************************/
    let all_pictures = Picture::find().all(&db).await?;
    let all_user_pictures = UserPicture::find().all(&db).await?;
    let all_permissions = Permission::find().all(&db).await?;
    let time_description = format!("{:?}", start.elapsed());
    debug!("pictures query finished in {time_description}");

    /******************** CHECK USED AND UNUSED ***********************/
    let (unused, used, unesed_ref) = get_used_pictures(all_pictures, all_user_pictures, all_permissions).await;
    let time_description = format!("{:?}", start.elapsed());
    debug!("unused pictures calculated in {time_description}");

    /******************** DELETE DATABASE AND FILE ********************/
    let handle1 = spawn(delete_database(unused, db.clone(), start.clone(), "unused files removed from database in"));
    let handle2 = spawn(delete_database(unesed_ref, db.clone(), start.clone(), "wrong user pictures removed from database in"));
    let handle3 = spawn(delete_file(used, trash_name, start.clone()));
    handle1.await?;
    handle2.await?;
    handle3.await?;

    /******************** REMOVE EMPTY FOLDER *************************/
    remove_empty_folder().await?;
    let time_description = format!("{:?}", start.elapsed());
    info!("empty folder removed in {time_description}.");


    let result = client.delete(&CONFIG.mark_url).send().await;
    if result.is_err() {
        error!("send mark request failed: {}.", result.err().unwrap().to_string());
        if !CONFIG.ignore_mark_fail {
            panic!("Cannot send mark request");
        }
    }

    Ok(())
}

async fn get_used_pictures(pictures: Vec<picture::Model>, user_pictures: Vec<user_picture::Model>, permissions: Vec<permission::Model>) -> (Vec<picture::Model>, Vec<picture::Model>, Vec<user_picture::Model>) {
    let mut picture_map: HashMap<String, picture::Model> = HashMap::new();//all pictures
    let mut space_map: HashMap<i64, i64> = HashMap::new();
    let permission_map: HashMap<i64, (Group, i64)> = get_user_group(permissions).await;

    let mut used_vec: Vec<picture::Model> = Vec::new();
    let mut unused_vec: Vec<picture::Model> = Vec::new();
    let mut disable_veg: Vec<user_picture::Model> = Vec::new();

    for picture in pictures {
        picture_map.insert(picture.pid.clone(), picture);
    }

    for user_picture in user_pictures {
        if user_picture.available == 1 {
            let picture = picture_map.get(&user_picture.pid);

            if picture.is_none() {
                disable_veg.push(user_picture);
            } else if picture.unwrap().pid != "added" {
                let used = match space_map.get(&user_picture.uid) {
                    None => {
                        0i64
                    }
                    Some(a) => {
                        *a
                    }
                };
                let used = used + picture.unwrap().size;
                let group = permission_map.get(&user_picture.uid);
                let (group, _expiry) = match group {
                    None => { &(DEFAULT_GROUP, 0) }
                    Some(g) => { g }
                };
                if used as f32 / 1024.0 / 1024.0 >= group.storage {
                    debug!("removing file as no enough space: {}", user_picture.file_name);
                    disable_veg.push(user_picture);
                    continue;
                }
                if picture.unwrap().size as f32 / 1024.0 / 1024.0 > group.restrictions {
                    debug!("removing file as size too big: {}", user_picture.file_name);
                    disable_veg.push(user_picture);
                    continue;
                }
                space_map.insert(user_picture.uid, used);

                let picture = picture.unwrap();
                used_vec.push(picture.clone());
                let picture_new = picture::Model {
                    pid: String::from("added"),
                    ..picture.clone()
                };
                picture_map.insert(user_picture.pid.clone(), picture_new);
            }
        }
    }

    for (_, picture) in picture_map {
        if picture.pid != "added" {
            unused_vec.push(picture);
        }
    }

    return (unused_vec, used_vec, disable_veg);
}

async fn get_user_group(permissions: Vec<permission::Model>) -> HashMap<i64, (Group, i64)> {
    let mut permission_map: HashMap<i64, (Group, i64)> = HashMap::new();

    for permission in permissions {
        if permission.available == 0 {
            continue;
        }
        if permission.expiry != 0 && permission.expiry < Local::now().checked_sub_days(Days::new(180)).unwrap().timestamp_millis() {
            continue;
        }

        let old = permission_map.get(&permission.uid);
        if old.is_none() {
            let group = get_group(&permission.permission.to_ascii_lowercase());
            permission_map.insert(permission.uid, (group, permission.expiry));
            continue;
        }
        let (old, _) = old.unwrap();
        let group_new = get_group(&permission.permission.to_ascii_lowercase());
        if group_new.priority > old.priority {
            permission_map.insert(permission.uid, (group_new, permission.expiry));
        }
    }

    return permission_map;
}

async fn delete_database<A, T>(pictures: Vec<T>, db: DatabaseConnection, instant: Instant, finish_message: &str)
    where A: ActiveModelTrait + ActiveModelBehavior + Send,
          T: ModelTrait + IntoActiveModel<A> {
    for picture in pictures {
        let picture = picture.into_active_model();
        let result = picture.delete(&db).await;
        match result {
            Ok(a) => { assert_eq!(a.rows_affected, 1); }
            Err(e) => { error!("cannot delete database: {e:?}"); }
        }
    }

    let time_description = format!("{:?}", instant.elapsed());
    info!("{finish_message} {time_description}");
}

async fn delete_file(pictures: Vec<picture::Model>, trash_dir: String, instant: Instant) {
    let mut used_list: Vec<&str> = Vec::new();

    for picture in &pictures {
        used_list.push(&picture.original);
        used_list.push(&picture.thumbnail);
        used_list.push(&picture.watermark);
    }

    for entry in glob("pictures/**/*.*").unwrap() {
        let name = entry.unwrap().display().to_string();
        if !used_list.contains(&name.as_str()) {
            debug!("removing file: {name}");
            fs::copy(&name, trash_dir.clone() + "/" + &name.split("/").last().unwrap()).await.unwrap();
            fs::remove_file(name).await.unwrap();
        }
    }

    let time_description = format!("{:?}", instant.elapsed());
    info!("unused files removed in {time_description}");
}

async fn remove_empty_folder() -> Result<(), Box<dyn std::error::Error>> {
    for entry in glob("pictures/*")? {
        let entry = entry?;
        let inner = format!("{}/*.*", &entry.display().to_string());
        let mut inner_paths = glob(&inner)?;
        if inner_paths.next().is_none() {
            debug!("removing empty folder: {}", entry.display());
            fs::remove_dir(entry.display().to_string()).await?;
        }
    }

    Ok(())
}

struct Group {
    priority: u16,
    storage: f32,
    restrictions: f32,
}

fn get_group(name: &str) -> Group {
    match name {
        "started" => {
            Group {
                priority: 1,
                storage: 10240.0,
                restrictions: 50.0,
            }
        }

        "advanced" => {
            Group {
                priority: 2,
                storage: 51200.0,
                restrictions: 100.0,
            }
        }

        "professional" => {
            Group {
                priority: 3,
                storage: 102400.0,
                restrictions: 999999.0,
            }
        }

        &_ => {
            Group {
                priority: 0,
                storage: 2048.0,
                restrictions: 50.0,
            }
        }
    }
}