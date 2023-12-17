use std::collections::HashMap;
use std::time::Instant;

use chrono::{Days, Local};
use glob::glob;
use sea_orm::{ActiveModelBehavior, ActiveModelTrait, DatabaseConnection, IntoActiveModel, ModelTrait};
use tokio::{fs, spawn};
use tracing::{debug, error, info};

use crate::DEFAULT_GROUP;
use crate::entity::{permission, picture, user_picture};

pub async fn cleanup_pictures(available_users: Vec<i64>, pictures: Vec<picture::Model>,
                              user_pictures: Vec<user_picture::Model>, permissions: Vec<permission::Model>,
                              db: &DatabaseConnection, start: Instant, trash_dir: String) -> Vec<i64> {
    //check
    let (unused, used, unused_ref) =
        get_used_pictures(available_users, pictures, user_pictures.clone(), permissions).await;

    //delete database and file
    let handle1 = spawn(delete_database(unused, db.clone(), start.clone(), "unused files removed from database in"));
    let handle2 = spawn(delete_database(unused_ref.clone(), db.clone(), start.clone(), "wrong user pictures removed from database in"));
    let handle3 = spawn(delete_file(used, trash_dir, start.clone()));
    //get used
    let handle4 = spawn(get_used_user_picture(unused_ref, user_pictures));
    handle1.await.unwrap();
    handle2.await.unwrap();
    handle3.await.unwrap();

    //remove empty folder
    remove_empty_folder().await.unwrap();
    let time_description = format!("{:?}", start.elapsed());
    info!("picture cleanup finished in {time_description}.");

    handle4.await.unwrap()
}

async fn get_used_pictures(available_users: Vec<i64>, pictures: Vec<picture::Model>,
                           user_pictures: Vec<user_picture::Model>, permissions: Vec<permission::Model>,
) -> (Vec<picture::Model>, Vec<picture::Model>, Vec<user_picture::Model>) {
    let mut picture_map: HashMap<String, picture::Model> = HashMap::new();//all pictures
    let mut space_map: HashMap<i64, i64> = HashMap::new();
    let permission_map: HashMap<i64, (crate::Group, i64)> = get_user_group(permissions).await;

    let mut used_vec: Vec<picture::Model> = Vec::new();
    let mut unused_vec: Vec<picture::Model> = Vec::new();
    let mut disable_vec: Vec<user_picture::Model> = Vec::new();

    for picture in pictures {
        picture_map.insert(picture.pid.clone(), picture);
    }

    for user_picture in user_pictures {
        if user_picture.available == 1 {
            let picture = picture_map.get(&user_picture.pid);

            if picture.is_none() {
                disable_vec.push(user_picture);
            } else if !available_users.contains(&user_picture.uid) {
                debug!("removing file as it has no available user: {}", user_picture.file_name);
                disable_vec.push(user_picture);
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
                    disable_vec.push(user_picture);
                    continue;
                }
                if picture.unwrap().size as f32 / 1024.0 / 1024.0 > group.restrictions {
                    debug!("removing file as size too big: {}", user_picture.file_name);
                    disable_vec.push(user_picture);
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
        } else {
            debug!("removing file as it is disabled: {}", user_picture.file_name);
            disable_vec.push(user_picture);
        }
    }

    for (_, picture) in picture_map {
        if picture.pid != "added" {
            unused_vec.push(picture);
        }
    }

    return (unused_vec, used_vec, disable_vec);
}

async fn get_user_group(permissions: Vec<permission::Model>) -> HashMap<i64, (crate::Group, i64)> {
    let mut permission_map: HashMap<i64, (crate::Group, i64)> = HashMap::new();

    for permission in permissions {
        if permission.available == 0 {
            continue;
        }
        if permission.expiry != 0 && permission.expiry < Local::now().checked_sub_days(Days::new(180)).unwrap().timestamp_millis() {
            continue;
        }

        let old = permission_map.get(&permission.uid);
        if old.is_none() {
            let group = crate::get_group(&permission.permission.to_ascii_lowercase());
            permission_map.insert(permission.uid, (group, permission.expiry));
            continue;
        }
        let (old, _) = old.unwrap();
        let group_new = crate::get_group(&permission.permission.to_ascii_lowercase());
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

async fn get_used_user_picture(unused_user_pictures: Vec<user_picture::Model>, user_pictures: Vec<user_picture::Model>) -> Vec<i64> {
    let mut used_vec: Vec<i64> = Vec::new();

    for user_picture in user_pictures {
        if !unused_user_pictures.contains(&user_picture) {
            used_vec.push(user_picture.id);
        }
    }

    used_vec
}