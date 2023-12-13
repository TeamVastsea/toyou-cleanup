use std::time::Instant;

use sea_orm::{DatabaseConnection, ModelTrait};
use tracing::{debug, info};

pub async fn cleanup_user(users: Vec<crate::entity::user::Model>, db: &DatabaseConnection, instant: Instant) -> Vec<i64> {
    let mut available_user: Vec<i64> = Vec::new();

    for user in users {
        if user.available == 0 {
            debug!("removing user: {}", user.username);
            user.delete(db).await.unwrap();
        } else if !available_user.contains(&user.uid) {
            available_user.push(user.uid);
        }
    }

    let time_description = format!("{:?}", instant.elapsed());
    info!("user cleanup finished in {time_description}.");

    available_user
}

pub fn collect_user(users: Vec<crate::entity::user::Model>) -> Vec<i64> {
    let mut available_user: Vec<i64> = Vec::new();

    for user in users {
        if !available_user.contains(&user.uid) {
            available_user.push(user.uid);
        }
    }

    available_user
}