use chrono::{NaiveDate, NaiveDateTime};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use sqlx::types::Json;
use crate::db::{Difficulty, Length, Rating, Role, SortDirection, UserListSortBy};
use crate::util::{ModUserAgent, ParsedSubmissionNote};

fn serialize_discord_snowflake<S>(value: &Option<i64>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    match value {
        Some(id) => serializer.serialize_some(&id.to_string()),
        None => serializer.serialize_none(),
    }
}


#[derive(Debug, FromRow, Serialize)]
pub struct User {
    pub id: i64,
    pub account_id: i64,
    pub username: String,
    pub role: Role,
    #[serde(serialize_with = "serialize_discord_snowflake")]
    pub discord_id: Option<i64>,
}

#[derive(FromRow)]
pub struct UploadInfo {
    pub account_id: i64,
    pub username: String,
}

/// Detailed information about a thumbnail upload
#[derive(FromRow, Serialize, Deserialize, utoipa::ToSchema)]
pub struct UploadExtended {
    /// ID of the level associated with this upload
    pub level_id: i64,
    /// Geometry Dash account ID of the user who made the upload
    pub account_id: i64,
    /// Username of the user who made the upload
    pub username: String,
    /// Timestamp when the upload was made
    #[schema(value_type = String, format = DateTime)]
    pub upload_time: NaiveDateTime,
    /// Timestamp of the first accepted upload for this level (may be the same as upload_time if this is the first accepted upload)
    #[schema(value_type = String, format = DateTime)]
    pub first_upload_time: NaiveDateTime,
    /// Timestamp when this upload was accepted
    #[schema(value_type = String, format = DateTime)]
    pub accepted_time: Option<NaiveDateTime>,
    /// Geometry Dash account ID of the admin who accepted this upload, if applicable
    pub accepted_by: Option<i64>,
    /// Username of the admin who accepted this upload, if applicable
    pub accepted_by_username: Option<String>,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize, utoipa::ToSchema)]
pub struct NoteData {
    pub level_name: String,
    pub creator_id: i64,
    pub creator_name: String,
    pub downloads: i64,
    pub likes: i64,
    pub stars: i64,
    pub length: Length,
    pub rating: Rating,
    pub difficulty: Difficulty,
    pub percentage: f32,
    pub attempt_time: f64,
    pub message: Option<String>,
    pub mod_version: String,
    pub mod_platform: String,
}

impl NoteData {
    pub fn from_parsed(note: ParsedSubmissionNote, ua: ModUserAgent) -> Self {
        Self {
            level_name: note.level_name,
            creator_id: note.creator_id,
            creator_name: note.creator_name,
            downloads: note.downloads,
            likes: note.likes,
            stars: note.stars,
            length: match note.length {
                0 => Length::Tiny,
                1 => Length::Short,
                2 => Length::Medium,
                3 => Length::Long,
                4 => Length::XL,
                5 => Length::Plat,
                _ => Length::Tiny
            },
            rating: match note.rating {
                0 => Rating::NA,
                1 => Rating::Rated,
                2 => Rating::Featured,
                3 => Rating::Epic,
                4 => Rating::Legendary,
                5 => Rating::Mythic,
                _ => Rating::NA
            },
            difficulty: match note.difficulty {
                0 => Difficulty::NA,
                1 => Difficulty::Auto,
                2 => Difficulty::Easy,
                3 => Difficulty::Normal,
                4 => Difficulty::Hard,
                5 => Difficulty::Harder,
                6 => Difficulty::Insane,
                7 => Difficulty::EasyDemon,
                8 => Difficulty::MediumDemon,
                9 => Difficulty::HardDemon,
                10 => Difficulty::InsaneDemon,
                11 => Difficulty::ExtremeDemon,
                _ => Difficulty::NA
            },
            percentage: note.percentage,
            attempt_time: note.attempt_time,
            message: note.message,
            mod_version: ua.version.to_string(),
            mod_platform: ua.platform.to_string(),
        }
    }
}

/// Represents a lock on a specific level, preventing new thumbnail uploads for that level until the lock is removed.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize, utoipa::ToSchema)]
pub struct LevelLock {
    /// ID of the level that is locked
    pub level_id: i64,
    /// ID of the admin who locked the level
    pub locked_by: i64,
    /// Username of the admin who locked the level
    pub locked_by_username: String,
    /// Timestamp when the level was locked
    #[schema(value_type = String, format = DateTime)]
    pub locked_at: NaiveDateTime,
    /// Optional reason provided by the admin for locking the level
    pub reason: Option<String>,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct PendingUpload {
    pub id: i64,
    pub user_id: i64,
    pub username: String,
    pub level_id: i64,
    pub accepted: bool,
    pub upload_time: NaiveDateTime,

    pub note_data: Option<Json<NoteData>>,
    pub account_id: Option<i64>,
    pub user_role: Role,

    #[sqlx(skip)]
    pub replacement: bool,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ActiveUpload {
    pub id: i64,
    pub level_id: i64,
    pub upload_time: NaiveDateTime,
    pub accepted_time: Option<NaiveDateTime>,
    pub note_data: Option<Json<NoteData>>,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct RejectedUpload {
    pub id: i64,
    pub level_id: i64,
    pub upload_time: NaiveDateTime,
    pub accepted_time: Option<NaiveDateTime>,
    pub note_data: Option<Json<NoteData>>,
    pub reason: Option<String>,
    pub accepted_by_username: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PendingQueryOptions {
    pub page: u32,
    pub per_page: u32,
    pub level_id: Option<i64>,
    pub user_id: Option<i64>,
    pub username: Option<String>,
    pub replacement_only: bool,
    pub new_only: bool,
}

#[derive(Debug, Clone)]
pub struct PendingUploadsPage {
    pub uploads: Vec<PendingUpload>,
    pub total: i64,
}

#[derive(Debug, Clone)]
pub struct ActiveUploadsPage {
    pub uploads: Vec<ActiveUpload>,
    pub total: i64,
}

#[derive(Debug, Clone)]
pub struct RejectedUploadsPage {
    pub uploads: Vec<RejectedUpload>,
    pub total: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct MyUploadsSummary {
    pub active: i64,
    pub pending: i64,
    pub rejected: i64,
}

#[derive(FromRow, Serialize, Deserialize)]
pub struct UserStats {
    pub id: i64,
    pub account_id: i64,
    pub username: String,
    pub role: Role,
    pub upload_count: i64,
    pub accepted_upload_count: i64,
    pub pending_upload_count: i64,
    pub level_count: i64,
    pub accepted_level_count: i64,
    pub active_thumbnail_count: i64,
}

#[derive(FromRow, Serialize, Deserialize)]
pub struct UserBan {
    pub id: i64,
    pub user_id: i64,
    pub ban_time: NaiveDateTime,
    pub reason: String,
    pub expires_at: Option<NaiveDateTime>,
}

#[derive(Debug, Clone)]
pub struct AdminUserQueryOptions {
    pub page: u32,
    pub per_page: u32,
    pub id: Option<i64>,
    pub username: Option<String>,
    pub account_id: Option<i64>,
    pub discord_id: Option<i64>,
    pub role: Option<Role>,
    pub total_uploads: Option<i64>,
    pub banned: Option<bool>,
    pub sort_by: UserListSortBy,
    pub sort_dir: SortDirection,
}

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct AdminUserRow {
    pub id: i64,
    pub username: String,
    pub account_id: i64,
    #[serde(serialize_with = "serialize_discord_snowflake")]
    pub discord_id: Option<i64>,
    pub role: Role,
    pub total_uploads: i64,
    pub accepted: i64,
    pub pending: i64,
    pub rejected: i64,
    pub active_thumbnails: i64,
    pub banned: bool,
    pub ban_time: Option<NaiveDateTime>,
    pub ban_reason: Option<String>,
    pub ban_expires_at: Option<NaiveDateTime>,
    pub banned_by_username: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AdminUsersPage {
    pub users: Vec<AdminUserRow>,
    pub total: i64,
}

/// Represents a snapshot of various statistics about the thumbnail repository at a specific point in time.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize, utoipa::ToSchema)]
pub struct StatsSnapshot {
    /// Unique identifier for this snapshot
    pub id: i64,
    /// Timestamp when the snapshot was taken
    #[schema(value_type = String, format = DateTime)]
    pub captured_at: NaiveDateTime,
    /// Total storage used by all thumbnails in bytes
    pub storage_bytes: i64,
    /// Total number of unique thumbnails stored on the server
    pub thumbnails_count: i64,
    /// Number of unique users (IPs) that have accessed any endpoint on the server in the past 30 days. (Uses Cloudflare data, may not be perfectly accurate)
    pub users_per_month: Option<i64>,
    /// Total number of registered users
    pub users_total: i64,
    /// Total number of uploads (including accepted, rejected, and pending) that the server has processed
    pub uploads_total: i64,
    /// Number of uploads in pending review
    pub pending_uploads_total: i64,
    /// Total number of accepted uploads (including ones that got replaced later)
    pub accepted_uploads_total: i64,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct UserHistoryPoint {
    pub period: NaiveDate,
    pub upload_count: i64,
    pub accepted_upload_count: i64,
    pub pending_upload_count: i64,
    pub level_count: i64,
    pub accepted_level_count: i64,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct BadgeLists {
    pub verified: Vec<i64>,
    pub moderator: Vec<i64>,
    pub admin: Vec<i64>,
    pub owner: Vec<i64>,
}