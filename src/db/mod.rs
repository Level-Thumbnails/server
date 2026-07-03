pub mod constants;
pub mod types;
pub mod models;
pub mod filters;
pub mod impls;

pub use types::*;
pub use models::*;
pub use constants::*;

use std::sync::Arc;
use tokio::sync::RwLock;
use std::collections::{HashMap, HashSet};
use chrono::{DateTime, Utc};
use sqlx::{Pool, Postgres};

#[derive(Debug, Clone)]
pub struct AppState {
    pub pool: Arc<Pool<Postgres>>,
    pub settings: Arc<RwLock<Settings>>,
    pub online_moderators: Arc<RwLock<HashMap<i64, (String, DateTime<Utc>)>>>,
    pub registered_users: Arc<RwLock<HashSet<i64>>>
}

pub async fn get_db() -> AppState {
    AppState::new().await
}