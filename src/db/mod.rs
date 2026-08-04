pub mod constants;
pub mod filters;
pub mod impls;
pub mod models;
pub mod types;

pub use constants::*;
pub use models::*;
pub use types::*;

use chrono::{DateTime, Utc};
use sqlx::{Pool, Postgres};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use axum::body::Bytes;
use dashmap::DashMap;
use futures::future::{BoxFuture, Shared};
use tokio::sync::{RwLock, Semaphore};
use crate::routes::thumbnail;

#[derive(Hash, Eq, PartialEq, Clone)]
pub struct ResizeKey {
    pub id: u64,
    pub res: thumbnail::Res,
}

pub(crate) type ResizeResult = Result<Bytes, String>;

#[derive(Clone)]
pub struct AppState {
    pub pool: Arc<Pool<Postgres>>,
    pub settings: Arc<RwLock<Settings>>,
    pub online_moderators: Arc<RwLock<HashMap<i64, (String, DateTime<Utc>)>>>,
    pub registered_users: Arc<RwLock<HashSet<i64>>>,
    pub active_thumbnails: Arc<RwLock<HashSet<u64>>>,
    pub resize_semaphore: Arc<Semaphore>,
    pub active_resizes: Arc<DashMap<ResizeKey, Shared<BoxFuture<'static, ResizeResult>>>>,
}

pub async fn get_db() -> AppState {
    AppState::new().await
}
