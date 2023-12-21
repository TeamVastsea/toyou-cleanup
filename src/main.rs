use std::env::args;
use std::time::Instant;

use chrono::{Days, Local};
use lazy_static::lazy_static;
use sea_orm::{ConnectOptions, Database, EntityTrait};
use tracing::{debug, error, info, warn};
use tracing_appender::non_blocking;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{EnvFilter, fmt, Registry};
use tracing_subscriber::fmt::time::ChronoLocal;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::cleanups::picture::cleanup_pictures;
use crate::cleanups::share::cleanup_share;
use crate::cleanups::user::{cleanup_user, collect_user};
use crate::config::{check_trash_dir, rename_log, ServerConfig};
use crate::entity::prelude::{Permission, Picture, Share, User, UserPicture};

mod entity;
mod config;
mod cleanups;

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

    let args: Vec<String> = args().collect();

    let remove_user = !args.contains(&"-no_user".to_string());
    let remove_picture = !args.contains(&"-no_picture".to_string());
    let remove_share = !args.contains(&"no_share".to_string());


    rename_log(now).await;
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&CONFIG.trace_level));

    let file_appender = RollingFileAppender::builder()
        .rotation(Rotation::NEVER)
        .filename_suffix(format!("logs/{}-least.cleanup.log", now.format("%Y-%m-%d")))
        .build("").unwrap();
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

    let time_description = format!("{:?}", start.elapsed());
    info!("started in {time_description}.");
    /******************** CHECK TRASH DIR *****************************/
    let trash_name = check_trash_dir(a_week_earlier, now).await;

    let time_description = format!("{:?}", start.elapsed());
    info!("trash dir ready in {time_description}.");

    /******************** CONNECT TO DATABASE *************************/

    let mut opt = ConnectOptions::new(&CONFIG.url);
    opt.sqlx_logging(CONFIG.sqlx_debug);
    let db = Database::connect(opt).await?;

    let time_description = format!("{:?}", start.elapsed());
    debug!("connected in {time_description}.");

    /******************** MARK START **********************************/

    let client = reqwest::Client::new();
    let result = client.post(&CONFIG.mark_url).send().await;
    if result.is_err() {
        error!("send mark request failed: {}.", result.err().unwrap().to_string());
        if !CONFIG.ignore_mark_fail {
            panic!("Cannot send mark request");
        }
    }

    /******************** CLEANUP USERS *******************************/
    let all_user = User::find().all(&db).await?;

    let time_description = format!("{:?}", start.elapsed());
    debug!("users query finished in {time_description}");

    let available_user = if remove_user {
        cleanup_user(all_user, &db, start).await
    } else {
        warn!("skipping cleanup users");
        collect_user(all_user)
    };

    /******************** GET ALL PICTURES ****************************/
    let all_pictures = Picture::find().all(&db).await?;
    let all_user_pictures = UserPicture::find().all(&db).await?;
    let all_permissions = Permission::find().all(&db).await?;

    let time_description = format!("{:?}", start.elapsed());
    debug!("pictures query finished in {time_description}");

    /******************** CLEANUP PICTURES ****************************/
    let used_user_pictures = if remove_picture {
        cleanup_pictures(available_user.clone(), all_pictures,
                         all_user_pictures, all_permissions,
                         &db, start, trash_name).await
    } else {
        warn!("skipping cleanup pictures");
        let mut all_used: Vec<i64> = Vec::new();
        for user_picture in all_user_pictures {
            all_used.push(user_picture.id);
        }

        all_used
    };

    /******************** CLEANUP SHARES ******************************/

    if remove_share {
        let all_shares = Share::find().all(&db).await?;
        cleanup_share(available_user, all_shares, used_user_pictures, &db, now.clone()).await;
    } else {
        warn!("skipping cleanup shares");
    }

    let time_description = format!("{:?}", start.elapsed());
    info!("share cleanup finished in {time_description}.");

    /******************** MARK END ************************************/
    let result = client.delete(&CONFIG.mark_url).send().await;
    if result.is_err() {
        error!("send mark request failed: {}.", result.err().unwrap().to_string());
        if !CONFIG.ignore_mark_fail {
            panic!("Cannot send mark request");
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