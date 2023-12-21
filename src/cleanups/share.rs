use chrono::{DateTime, Local};
use sea_orm::{DatabaseConnection, ModelTrait};

pub async fn cleanup_share(available_users: Vec<i64>, shares: Vec<crate::entity::share::Model>, user_picture_list: Vec<i64>, db: &DatabaseConnection, now: DateTime<Local>) {
    for share in shares {
        if !available_users.contains(&share.uid) {
            share.delete(db).await.unwrap();
            continue;
        }

        if now.timestamp_millis() > share.expiry {
            share.delete(db).await.unwrap();
            continue;
        }

        if !user_picture_list.contains(&share.id) {
            share.delete(db).await.unwrap();
            continue;
        }
    }
}