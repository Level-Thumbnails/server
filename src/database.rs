use sqlx::postgres::PgPoolOptions;
use sqlx::{FromRow, Postgres, QueryBuilder};
use std::collections::HashMap;
use std::path::Path;

use crate::util::VersionInfo;
use chrono::{Datelike, NaiveDate, NaiveDateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

const PENDING_UPLOAD_SELECT: &str = "SELECT uploads.id, user_id, users.username AS username, level_id, accepted, upload_time, \
     submission_note, users.account_id AS account_id, users.role AS user_role \
     FROM uploads \
     LEFT JOIN users ON users.id = user_id \
     WHERE accepted = FALSE AND accepted_time IS NULL";

const MAX_MY_UPLOADS_PAGE_SIZE: u32 = 100;

const USER_STATS_CTE: &str = r#"WITH upload_counts AS (
    SELECT
        user_id,
        COUNT(*)::BIGINT AS total_uploads,
        COUNT(*) FILTER (WHERE accepted = TRUE)::BIGINT AS accepted,
        COUNT(*) FILTER (WHERE accepted = FALSE AND accepted_time IS NULL)::BIGINT AS pending,
        COUNT(*) FILTER (WHERE accepted = FALSE AND accepted_time IS NOT NULL)::BIGINT AS rejected
    FROM uploads
    GROUP BY user_id
), latest_accepted_uploads AS (
    SELECT DISTINCT ON (level_id)
        level_id,
        user_id
    FROM uploads
    WHERE accepted = TRUE AND deleted_at IS NULL
    ORDER BY level_id, upload_time DESC, id DESC
), active_counts AS (
    SELECT
        user_id,
        COUNT(*)::BIGINT AS active_thumbnails
    FROM latest_accepted_uploads
    GROUP BY user_id
), active_bans AS (
    SELECT DISTINCT ON (bans.user_id)
        bans.user_id,
        bans.ban_time,
        bans.reason,
        bans.expires_at,
        banned_by.username AS banned_by_username
    FROM bans
    JOIN users AS banned_by ON banned_by.id = bans.banned_by
    WHERE bans.expires_at IS NULL OR bans.expires_at > NOW()
    ORDER BY bans.user_id, bans.ban_time DESC, bans.id DESC
), user_stats AS (
    SELECT
        users.id,
        users.username,
        users.account_id,
        users.discord_id,
        users.role,
        COALESCE(upload_counts.total_uploads, 0) AS total_uploads,
        COALESCE(upload_counts.accepted, 0) AS accepted,
        COALESCE(upload_counts.pending, 0) AS pending,
        COALESCE(upload_counts.rejected, 0) AS rejected,
        COALESCE(active_counts.active_thumbnails, 0) AS active_thumbnails,
        active_bans.ban_time AS ban_time,
        active_bans.reason AS ban_reason,
        active_bans.expires_at AS ban_expires_at,
        active_bans.banned_by_username AS banned_by_username,
        active_bans.user_id IS NOT NULL AS banned
    FROM users
    LEFT JOIN upload_counts ON upload_counts.user_id = users.id
    LEFT JOIN active_counts ON active_counts.user_id = users.id
    LEFT JOIN active_bans ON active_bans.user_id = users.id
)"#;

fn serialize_discord_snowflake<S>(value: &Option<i64>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    match value {
        Some(id) => serializer.serialize_some(&id.to_string()),
        None => serializer.serialize_none(),
    }
}

fn default_min_supported_client() -> VersionInfo {
    VersionInfo::from_str("v2.1.0").expect("Invalid default version")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub pause_submissions: bool,
    #[serde(default = "default_min_supported_client")]
    pub min_supported_client: VersionInfo,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            pause_submissions: false,
            min_supported_client: default_min_supported_client(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AppState {
    pub pool: Arc<sqlx::Pool<Postgres>>,
    pub settings: Arc<tokio::sync::RwLock<Settings>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Permission {
    DirectUploadNewThumbnail,
    DirectUploadReplacement,
    ModeratePendingUploads,
    ManageUserProfiles,
    ViewOtherUserHistory,
    ManageLevelLocks,
    ManageSettings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, sqlx::Type)]
#[serde(rename_all = "lowercase")]
#[sqlx(type_name = "TEXT", rename_all = "lowercase")]
pub enum Role {
    User,      // regular user
    Verified,  // verified users can upload thumbnails without approval
    Moderator, // moderators can approve or reject uploads
    Admin,     // admins can manage users and uploads
    Owner,     // superadmin with unrestricted access
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Role::User => write!(f, "user"),
            Role::Verified => write!(f, "verified"),
            Role::Moderator => write!(f, "moderator"),
            Role::Admin => write!(f, "admin"),
            Role::Owner => write!(f, "owner"),
        }
    }
}

impl Role {
    pub const ORDERED: [Role; 5] =
        [Role::User, Role::Verified, Role::Moderator, Role::Admin, Role::Owner];

    pub fn rank(self) -> u8 {
        match self {
            Role::User => 0,
            Role::Verified => 1,
            Role::Moderator => 2,
            Role::Admin => 3,
            Role::Owner => 4,
        }
    }

    pub fn has_permission(self, permission: Permission) -> bool {
        match permission {
            Permission::DirectUploadNewThumbnail => {
                matches!(self, Role::Verified | Role::Moderator | Role::Admin | Role::Owner)
            }
            Permission::DirectUploadReplacement => {
                matches!(self, Role::Moderator | Role::Admin | Role::Owner)
            }
            Permission::ModeratePendingUploads => {
                matches!(self, Role::Moderator | Role::Admin | Role::Owner)
            }
            Permission::ManageUserProfiles => {
                matches!(self, Role::Moderator | Role::Admin | Role::Owner)
            }
            Permission::ViewOtherUserHistory => {
                matches!(self, Role::Moderator | Role::Admin | Role::Owner)
            }
            Permission::ManageLevelLocks => matches!(self, Role::Admin | Role::Owner),
            Permission::ManageSettings => matches!(self, Role::Admin | Role::Owner),
        }
    }

    pub fn can_manage_user(self, target: Role) -> bool {
        self == Role::Owner
            || (self.has_permission(Permission::ManageUserProfiles) && self.rank() > target.rank())
    }

    pub fn can_assign_role(self, target: Role) -> bool {
        self == Role::Owner
            || (self.has_permission(Permission::ManageUserProfiles) && self.rank() > target.rank())
    }

    pub fn can_bypass_level_locks(self) -> bool {
        self.has_permission(Permission::ManageLevelLocks)
    }

    pub fn can_manage_level_locks(self) -> bool {
        self.has_permission(Permission::ManageLevelLocks)
    }

    pub fn can_moderate_pending_uploads(self) -> bool {
        self.has_permission(Permission::ModeratePendingUploads)
    }

    pub fn can_view_other_user_history(self) -> bool {
        self.has_permission(Permission::ViewOtherUserHistory)
    }

    pub fn can_manage_settings(self) -> bool {
        self.has_permission(Permission::ManageSettings)
    }

    pub fn can_upload_new_thumbnail_directly(self) -> bool {
        self.has_permission(Permission::DirectUploadNewThumbnail)
    }

    pub fn can_upload_replacement_directly(self) -> bool {
        self.has_permission(Permission::DirectUploadReplacement)
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
    pub submission_note: Option<String>,
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
    pub submission_note: Option<String>,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct RejectedUpload {
    pub id: i64,
    pub level_id: i64,
    pub upload_time: NaiveDateTime,
    pub accepted_time: Option<NaiveDateTime>,
    pub submission_note: Option<String>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UserListSortBy {
    Id,
    Username,
    AccountId,
    DiscordId,
    Role,
    TotalUploads,
    Accepted,
    Pending,
    Rejected,
    ActiveThumbnails,
    Banned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SortDirection {
    Asc,
    Desc,
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

fn month_start(date: NaiveDate) -> NaiveDate {
    NaiveDate::from_ymd_opt(date.year(), date.month(), 1).expect("invalid month start")
}

fn add_months(date: NaiveDate, months: i32) -> NaiveDate {
    let total_months = date.year() * 12 + date.month0() as i32 + months;
    let year = total_months.div_euclid(12);
    let month0 = total_months.rem_euclid(12);

    NaiveDate::from_ymd_opt(year, (month0 + 1) as u32, 1).expect("invalid shifted month")
}

fn is_image_uploaded(level_id: i64) -> bool {
    Path::new(&format!("thumbnails/{}.webp", level_id)).exists()
}

#[derive(Debug, Clone)]
pub struct UpdateUserOptions {
    pub username: Option<String>,
    pub account_id: Option<i64>,
    pub discord_id: Option<Option<i64>>,
    pub role: Option<Role>,
}

impl AppState {
    pub async fn new() -> Self {
        let connection_string = dotenv::var("DATABASE_URL").expect("DATABASE_URL must be set");
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(&connection_string)
            .await
            .expect("Failed to connect to the database");

        // Run migrations if needed
        sqlx::migrate!("./migrations").run(&pool).await.expect("Failed to run migrations");

        // load settings from state.json or create default
        let settings = if let Ok(settings_data) = tokio::fs::read_to_string("state.json").await {
            serde_json::from_str(&settings_data).unwrap_or_default()
        } else {
            Settings::default()
        };

        AppState {
            pool: Arc::new(pool),
            settings: Arc::new(tokio::sync::RwLock::new(settings)),
        }
    }

    pub async fn get_upload_info(&self, id: i64) -> Option<UploadInfo> {
        sqlx::query_as::<_, UploadInfo>(
            "SELECT users.account_id, users.username
             FROM uploads
             JOIN users ON uploads.user_id = users.id
             WHERE uploads.level_id = $1 AND accepted = TRUE AND uploads.deleted_at IS NULL
             ORDER BY upload_time DESC LIMIT 1",
        )
        .bind(id)
        .fetch_optional(&*self.pool)
        .await
        .ok()?
    }

    pub async fn get_upload_extended(&self, id: i64) -> Option<UploadExtended> {
        sqlx::query_as::<_, UploadExtended>(
            "SELECT
                uploads.level_id,
                users.account_id,
                users.username,
                uploads.upload_time,
                (
                    SELECT MIN(upload_time) FROM uploads u2
                    WHERE u2.level_id = uploads.level_id AND u2.accepted = TRUE AND u2.deleted_at IS NULL
                ) AS first_upload_time,
                uploads.accepted_time,
                accepted_by.account_id AS accepted_by,
                accepted_by.username AS accepted_by_username
             FROM uploads
             JOIN users ON uploads.user_id = users.id
             LEFT JOIN users AS accepted_by ON uploads.accepted_by = accepted_by.id
             WHERE uploads.level_id = $1 AND accepted = TRUE AND uploads.deleted_at IS NULL
             ORDER BY upload_time DESC LIMIT 1",
        )
            .bind(id)
            .fetch_optional(&*self.pool)
            .await
            .ok()?
    }

    pub async fn find_or_create_user(
        &self,
        account_id: i64,
        username: &str,
    ) -> Result<User, sqlx::Error> {
        let user = sqlx::query_as::<_, User>("SELECT * FROM users WHERE account_id = $1")
            .bind(account_id)
            .fetch_optional(&*self.pool)
            .await?;

        if let Some(user) = user {
            Ok(user)
        } else {
            sqlx::query_as::<_, User>(
                "INSERT INTO users (account_id, username, role) VALUES ($1, $2, 'user') RETURNING *",
            )
                .bind(account_id)
                .bind(username)
                .fetch_one(&*self.pool)
                .await
        }
    }

    pub async fn find_or_create_user_discord(
        &self,
        discord_id: i64,
        username: &str,
    ) -> Result<User, sqlx::Error> {
        let user = sqlx::query_as::<_, User>("SELECT * FROM users WHERE discord_id = $1")
            .bind(discord_id)
            .fetch_optional(&*self.pool)
            .await?;

        if let Some(user) = user {
            return Ok(user);
        }

        // first check if we can link to existing legacy account
        let legacy_user = sqlx::query_as::<_, User>(
            "SELECT * FROM users WHERE account_id = -1 AND username = $1 AND discord_id IS NULL",
        )
        .bind(username)
        .fetch_optional(&*self.pool)
        .await?;

        if let Some(legacy_user) = legacy_user {
            // update the legacy user with the discord_id
            sqlx::query("UPDATE users SET discord_id = $1 WHERE id = $2")
                .bind(discord_id)
                .bind(legacy_user.id)
                .execute(&*self.pool)
                .await?;
            return Ok(legacy_user);
        }

        // if no legacy user found, create a new user
        sqlx::query_as::<_, User>(
            "INSERT INTO users (account_id, username, role, discord_id) VALUES (-1, $1, 'user', $2) RETURNING *",
        )
            .bind(username)
            .bind(discord_id)
            .fetch_one(&*self.pool)
            .await
    }

    pub async fn get_user_by_id(&self, id: i64) -> Option<User> {
        sqlx::query_as::<_, User>("SELECT * FROM users WHERE id = $1")
            .bind(id)
            .fetch_optional(&*self.pool)
            .await
            .ok()?
    }

    pub async fn update_user(
        &self,
        id: i64,
        options: UpdateUserOptions,
    ) -> Result<User, sqlx::Error> {
        use sqlx::QueryBuilder;

        let mut builder = QueryBuilder::new("UPDATE users SET ");
        let mut first = true;

        if let Some(username) = options.username {
            if !first {
                builder.push(", ");
            };
            builder.push("username = ").push_bind(username);
            first = false;
        }

        if let Some(account_id) = options.account_id {
            if !first {
                builder.push(", ");
            };
            builder.push("account_id = ").push_bind(account_id);
            first = false;
        }

        if let Some(discord_opt) = options.discord_id {
            if !first {
                builder.push(", ");
            };
            if let Some(discord_id) = discord_opt {
                builder.push("discord_id = ").push_bind(discord_id);
            } else {
                builder.push("discord_id = NULL");
            }
            first = false;
        }

        if let Some(role) = options.role {
            if !first {
                builder.push(", ");
            };
            builder.push("role = ").push_bind(role);
            first = false;
        }

        if first {
            return sqlx::query_as::<_, User>("SELECT * FROM users WHERE id = $1")
                .bind(id)
                .fetch_one(&*self.pool)
                .await;
        }

        builder.push(" WHERE id = ").push_bind(id).push(" RETURNING *");

        let query = builder.build_query_as::<User>();
        query.fetch_one(&*self.pool).await
    }

    pub async fn add_upload(
        &self,
        level_id: i64,
        user_id: i64,
        image_path: &str,
        accepted: bool,
        submission_note: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        if accepted {
            sqlx::query(
                "INSERT INTO uploads (level_id, user_id, image_path, accepted, accepted_time, accepted_by, submission_note)
                 VALUES ($1, $2, $3, TRUE, NOW(), $2, $4)",
            )
                .bind(level_id)
                .bind(user_id)
                .bind(image_path)
                .bind(submission_note)
                .execute(&*self.pool)
                .await?;
        } else {
            sqlx::query(
                "INSERT INTO uploads (level_id, user_id, image_path, accepted, submission_note)
                 VALUES ($1, $2, $3, FALSE, $4)
                 ON CONFLICT (user_id, level_id)
                 WHERE accepted = FALSE AND accepted_time IS NULL
                 DO UPDATE SET
                     image_path = EXCLUDED.image_path,
                     submission_note = EXCLUDED.submission_note,
                     upload_time = NOW()",
            )
            .bind(level_id)
            .bind(user_id)
            .bind(image_path)
            .bind(submission_note)
            .execute(&*self.pool)
            .await?;
        }

        Ok(())
    }

    pub async fn get_level_lock(&self, level_id: i64) -> Result<Option<LevelLock>, sqlx::Error> {
        sqlx::query_as::<_, LevelLock>(
            "SELECT
                level_locks.level_id,
                level_locks.locked_by,
                users.username AS locked_by_username,
                level_locks.locked_at,
                level_locks.reason
             FROM level_locks
             JOIN users ON users.id = level_locks.locked_by
             WHERE level_locks.level_id = $1",
        )
        .bind(level_id)
        .fetch_optional(&*self.pool)
        .await
    }

    pub async fn get_all_level_locks(&self) -> Result<Vec<LevelLock>, sqlx::Error> {
        sqlx::query_as::<_, LevelLock>(
            "SELECT
                level_locks.level_id,
                level_locks.locked_by,
                users.username AS locked_by_username,
                level_locks.locked_at,
                level_locks.reason
             FROM level_locks
             JOIN users ON users.id = level_locks.locked_by
             ORDER BY level_locks.locked_at DESC, level_locks.level_id DESC",
        )
        .fetch_all(&*self.pool)
        .await
    }

    pub async fn lock_level(
        &self,
        level_id: i64,
        locked_by: i64,
        reason: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO level_locks (level_id, locked_by, reason)
             VALUES ($1, $2, $3)
             ON CONFLICT (level_id)
             DO UPDATE SET
                 locked_by = EXCLUDED.locked_by,
                 reason = EXCLUDED.reason,
                 locked_at = NOW()",
        )
        .bind(level_id)
        .bind(locked_by)
        .bind(reason)
        .execute(&*self.pool)
        .await?;

        Ok(())
    }

    pub async fn unlock_level(&self, level_id: i64) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM level_locks WHERE level_id = $1")
            .bind(level_id)
            .execute(&*self.pool)
            .await?;

        Ok(result.rows_affected() > 0)
    }

    pub async fn delete_thumbnail_by_id(&self, level_id: i64) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE uploads SET deleted_at = NOW()
             WHERE level_id = $1 AND accepted = TRUE AND deleted_at IS NULL",
        )
        .bind(level_id)
        .execute(&*self.pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    fn apply_pending_filters<'a>(
        builder: &mut QueryBuilder<'a, Postgres>,
        options: &PendingQueryOptions,
    ) {
        if let Some(level_id) = options.level_id {
            builder.push(" AND uploads.level_id = ").push_bind(level_id);
        }

        if let Some(user_id) = options.user_id {
            builder.push(" AND uploads.user_id = ").push_bind(user_id);
        }

        if let Some(ref username) = options.username {
            builder
                .push(" AND LOWER(username) LIKE LOWER(")
                .push_bind(format!("%{}%", username))
                .push(")");
        }
    }

    fn apply_user_filters<'a>(
        builder: &mut QueryBuilder<'a, Postgres>,
        options: &AdminUserQueryOptions,
    ) {
        if let Some(id) = options.id {
            builder.push(" AND id = ").push_bind(id);
        }

        if let Some(ref username) = options.username {
            let username = username.trim();
            if !username.is_empty() {
                builder
                    .push(" AND LOWER(username) LIKE LOWER(")
                    .push_bind(format!("%{}%", username))
                    .push(")");
            }
        }

        if let Some(account_id) = options.account_id {
            builder.push(" AND account_id = ").push_bind(account_id);
        }

        if let Some(discord_id) = options.discord_id {
            builder.push(" AND discord_id = ").push_bind(discord_id);
        }

        if let Some(role) = options.role {
            builder.push(" AND role = ").push_bind(role);
        }

        if let Some(total_uploads) = options.total_uploads {
            builder.push(" AND total_uploads = ").push_bind(total_uploads);
        }

        if let Some(banned) = options.banned {
            builder.push(" AND banned = ").push_bind(banned);
        }
    }

    fn apply_user_sort(
        builder: &mut QueryBuilder<'_, Postgres>,
        sort_by: UserListSortBy,
        sort_direction: SortDirection,
    ) {
        let direction = match sort_direction {
            SortDirection::Asc => "ASC",
            SortDirection::Desc => "DESC",
        };

        builder.push(" ORDER BY ");
        match sort_by {
            UserListSortBy::Id => builder.push("id ").push(direction),
            UserListSortBy::Username => builder.push("LOWER(username) ").push(direction),
            UserListSortBy::AccountId => builder.push("account_id ").push(direction),
            UserListSortBy::DiscordId => {
                builder.push("discord_id ").push(direction).push(" NULLS LAST")
            }
            UserListSortBy::Role => {
                builder.push("CASE role ");
                for (rank, role) in Role::ORDERED.iter().copied().enumerate() {
                    builder
                        .push("WHEN ")
                        .push_bind(role)
                        .push(" THEN ")
                        .push_bind(rank as i32)
                        .push(" ");
                }
                builder.push("END ").push(direction)
            }
            UserListSortBy::TotalUploads => builder.push("total_uploads ").push(direction),
            UserListSortBy::Accepted => builder.push("accepted ").push(direction),
            UserListSortBy::Pending => builder.push("pending ").push(direction),
            UserListSortBy::Rejected => builder.push("rejected ").push(direction),
            UserListSortBy::ActiveThumbnails => builder.push("active_thumbnails ").push(direction),
            UserListSortBy::Banned => builder.push("banned ").push(direction),
        };
    }

    pub async fn get_pending_uploads_paginated(
        &self,
        options: PendingQueryOptions,
    ) -> Result<PendingUploadsPage, sqlx::Error> {
        if options.replacement_only || options.new_only {
            let mut builder = QueryBuilder::new(PENDING_UPLOAD_SELECT);
            Self::apply_pending_filters(&mut builder, &options);
            builder.push(" ORDER BY upload_time ASC, uploads.id ASC");

            let mut all_uploads =
                builder.build_query_as::<PendingUpload>().fetch_all(&*self.pool).await?;

            all_uploads.retain(|upload| {
                let uploaded = is_image_uploaded(upload.level_id);
                if options.replacement_only { uploaded } else { !uploaded }
            });

            let total = all_uploads.len() as i64;
            let per_page = options.per_page as usize;
            let offset = options.page.saturating_sub(1) as usize * per_page;
            let uploads = all_uploads.into_iter().skip(offset).take(per_page).collect();

            Ok(PendingUploadsPage { uploads, total })
        } else {
            let per_page = options.per_page as i64;
            let offset = options.page.saturating_sub(1) as i64 * per_page;

            let mut data_builder = QueryBuilder::new(PENDING_UPLOAD_SELECT);
            Self::apply_pending_filters(&mut data_builder, &options);
            data_builder
                .push(" ORDER BY upload_time ASC, uploads.id ASC LIMIT ")
                .push_bind(per_page)
                .push(" OFFSET ")
                .push_bind(offset);

            let uploads =
                data_builder.build_query_as::<PendingUpload>().fetch_all(&*self.pool).await?;

            let mut count_builder = QueryBuilder::new(
                "SELECT COUNT(*) FROM uploads
                 LEFT JOIN users ON users.id = user_id
                 WHERE accepted = FALSE AND accepted_time IS NULL",
            );
            Self::apply_pending_filters(&mut count_builder, &options);
            let total: i64 = count_builder.build_query_scalar().fetch_one(&*self.pool).await?;

            Ok(PendingUploadsPage { uploads, total })
        }
    }

    pub async fn get_pending_uploads_for_user(
        &self,
        user_id: i64,
    ) -> Result<Vec<PendingUpload>, sqlx::Error> {
        sqlx::query_as::<_, PendingUpload>(&format!(
            "{} AND user_id = $1 ORDER BY upload_time",
            PENDING_UPLOAD_SELECT
        ))
        .bind(user_id)
        .fetch_all(&*self.pool)
        .await
    }

    pub async fn get_user_active_uploads_paginated(
        &self,
        user_id: i64,
        page: u32,
        per_page: u32,
        level_id_search: Option<String>,
        _creator_search: Option<String>,
    ) -> Result<ActiveUploadsPage, sqlx::Error> {
        let per_page = per_page.min(MAX_MY_UPLOADS_PAGE_SIZE) as i64;
        let page = page.max(1) as i64;
        let offset = (page - 1) * per_page;
        let level_id_search = level_id_search.and_then(|s| s.parse::<i64>().ok());

        let uploads = sqlx::query_as::<_, ActiveUpload>(
            "WITH active_uploads AS (
                SELECT DISTINCT ON (uploads.level_id)
                    uploads.id,
                    uploads.user_id,
                    uploads.level_id,
                    uploads.upload_time,
                    uploads.accepted_time,
                    uploads.submission_note
                FROM uploads
                WHERE uploads.accepted = TRUE AND uploads.deleted_at IS NULL
                ORDER BY uploads.level_id, uploads.upload_time DESC, uploads.id DESC
            )
            SELECT id, level_id, upload_time, accepted_time, submission_note
            FROM active_uploads
            WHERE user_id = $1 AND ($4::BIGINT IS NULL OR level_id = $4)
            ORDER BY upload_time DESC, id DESC
            LIMIT $2 OFFSET $3",
        )
        .bind(user_id)
        .bind(per_page)
        .bind(offset)
        .bind(level_id_search)
        .fetch_all(&*self.pool)
        .await?;

        let total: i64 = sqlx::query_scalar(
            "WITH active_uploads AS (
                SELECT DISTINCT ON (uploads.level_id)
                    uploads.user_id,
                    uploads.level_id
                FROM uploads
                WHERE uploads.accepted = TRUE AND uploads.deleted_at IS NULL
                ORDER BY uploads.level_id, uploads.upload_time DESC, uploads.id DESC
            )
            SELECT COUNT(*) FROM active_uploads WHERE user_id = $1 AND ($2::BIGINT IS NULL OR level_id = $2)",
        )
        .bind(user_id)
        .bind(level_id_search)
        .fetch_one(&*self.pool)
        .await?;

        Ok(ActiveUploadsPage { uploads, total })
    }

    pub async fn get_user_rejected_uploads_paginated(
        &self,
        user_id: i64,
        page: u32,
        per_page: u32,
        level_id_search: Option<String>,
        _creator_search: Option<String>,
    ) -> Result<RejectedUploadsPage, sqlx::Error> {
        let per_page = per_page.min(MAX_MY_UPLOADS_PAGE_SIZE) as i64;
        let page = page.max(1) as i64;
        let offset = (page - 1) * per_page;
        let level_id_search = level_id_search.and_then(|s| s.parse::<i64>().ok());

        let uploads = sqlx::query_as::<_, RejectedUpload>(
            "SELECT
                uploads.id,
                uploads.level_id,
                uploads.upload_time,
                uploads.accepted_time,
                uploads.submission_note,
                uploads.reason,
                accepted_by.username AS accepted_by_username
            FROM uploads
            LEFT JOIN users AS accepted_by ON accepted_by.id = uploads.accepted_by
            WHERE uploads.user_id = $1
              AND uploads.accepted = FALSE
              AND uploads.accepted_time IS NOT NULL
              AND uploads.deleted_at IS NULL
              AND ($4::BIGINT IS NULL OR uploads.level_id = $4)
            ORDER BY uploads.accepted_time DESC, uploads.id DESC
            LIMIT $2 OFFSET $3",
        )
        .bind(user_id)
        .bind(per_page)
        .bind(offset)
        .bind(level_id_search)
        .fetch_all(&*self.pool)
        .await?;

        let total: i64 = sqlx::query_scalar(
            "SELECT COUNT(*)
             FROM uploads
             WHERE uploads.user_id = $1
               AND uploads.accepted = FALSE
               AND uploads.accepted_time IS NOT NULL
               AND uploads.deleted_at IS NULL
               AND ($2::BIGINT IS NULL OR uploads.level_id = $2)",
        )
        .bind(user_id)
        .bind(level_id_search)
        .fetch_one(&*self.pool)
        .await?;

        Ok(RejectedUploadsPage { uploads, total })
    }

    pub async fn get_my_upload_summary(
        &self,
        user_id: i64,
        level_id_search: Option<String>,
    ) -> Result<MyUploadsSummary, sqlx::Error> {
        let level_id_search = level_id_search.and_then(|s| s.parse::<i64>().ok());

        let active = if let Some(level_id) = level_id_search {
            sqlx::query_scalar(
                "WITH active_uploads AS (
                    SELECT DISTINCT ON (uploads.level_id)
                        uploads.user_id,
                        uploads.level_id
                    FROM uploads
                    WHERE uploads.accepted = TRUE AND uploads.deleted_at IS NULL
                    ORDER BY uploads.level_id, uploads.upload_time DESC, uploads.id DESC
                )
                SELECT COUNT(*) FROM active_uploads WHERE user_id = $1 AND level_id = $2",
            )
            .bind(user_id)
            .bind(level_id)
            .fetch_one(&*self.pool)
            .await?
        } else {
            sqlx::query_scalar(
                "WITH active_uploads AS (
                    SELECT DISTINCT ON (uploads.level_id)
                        uploads.user_id
                    FROM uploads
                    WHERE uploads.accepted = TRUE AND uploads.deleted_at IS NULL
                    ORDER BY uploads.level_id, uploads.upload_time DESC, uploads.id DESC
                )
                SELECT COUNT(*) FROM active_uploads WHERE user_id = $1",
            )
            .bind(user_id)
            .fetch_one(&*self.pool)
            .await?
        };

        let pending = if let Some(level_id) = level_id_search {
            sqlx::query_scalar(
                "SELECT COUNT(*)
                 FROM uploads
                 WHERE uploads.user_id = $1
                   AND uploads.accepted = FALSE
                   AND uploads.accepted_time IS NULL
                   AND uploads.deleted_at IS NULL
                   AND uploads.level_id = $2",
            )
            .bind(user_id)
            .bind(level_id)
            .fetch_one(&*self.pool)
            .await?
        } else {
            sqlx::query_scalar(
                "SELECT COUNT(*)
                 FROM uploads
                 WHERE uploads.user_id = $1
                   AND uploads.accepted = FALSE
                   AND uploads.accepted_time IS NULL
                   AND uploads.deleted_at IS NULL",
            )
            .bind(user_id)
            .fetch_one(&*self.pool)
            .await?
        };

        let rejected = if let Some(level_id) = level_id_search {
            sqlx::query_scalar(
                "SELECT COUNT(*)
                 FROM uploads
                 WHERE uploads.user_id = $1
                   AND uploads.accepted = FALSE
                   AND uploads.accepted_time IS NOT NULL
                   AND uploads.deleted_at IS NULL
                   AND uploads.level_id = $2",
            )
            .bind(user_id)
            .bind(level_id)
            .fetch_one(&*self.pool)
            .await?
        } else {
            sqlx::query_scalar(
                "SELECT COUNT(*)
                 FROM uploads
                 WHERE uploads.user_id = $1
                   AND uploads.accepted = FALSE
                   AND uploads.accepted_time IS NOT NULL
                   AND uploads.deleted_at IS NULL",
            )
            .bind(user_id)
            .fetch_one(&*self.pool)
            .await?
        };

        Ok(MyUploadsSummary { active, pending, rejected })
    }

    pub async fn get_pending_upload(&self, id: i64) -> Result<PendingUpload, sqlx::Error> {
        sqlx::query_as::<_, PendingUpload>(&format!(
            "{} AND uploads.id = $1",
            PENDING_UPLOAD_SELECT
        ))
        .bind(id)
        .fetch_one(&*self.pool)
        .await
    }

    pub async fn get_admin_users_paginated(
        &self,
        options: AdminUserQueryOptions,
    ) -> Result<AdminUsersPage, sqlx::Error> {
        let data_query = format!(
            "{} SELECT id, username, account_id, discord_id, role, total_uploads, accepted, pending, rejected, active_thumbnails, banned, ban_time, ban_reason, ban_expires_at, banned_by_username FROM user_stats WHERE TRUE",
            USER_STATS_CTE
        );
        let count_query = format!("{} SELECT COUNT(*) FROM user_stats WHERE TRUE", USER_STATS_CTE);

        let per_page = options.per_page as i64;
        let offset = options.page.saturating_sub(1) as i64 * per_page;

        let mut data_builder = QueryBuilder::new(data_query);
        Self::apply_user_filters(&mut data_builder, &options);
        Self::apply_user_sort(&mut data_builder, options.sort_by, options.sort_dir);
        data_builder.push(" LIMIT ").push_bind(per_page).push(" OFFSET ").push_bind(offset);

        let users = data_builder.build_query_as::<AdminUserRow>().fetch_all(&*self.pool).await?;

        let mut count_builder = QueryBuilder::new(count_query);
        Self::apply_user_filters(&mut count_builder, &options);
        let total: i64 = count_builder.build_query_scalar().fetch_one(&*self.pool).await?;

        Ok(AdminUsersPage { users, total })
    }

    pub async fn accept_upload(
        &self,
        id: i64,
        accepted_by: i64,
        reason: Option<String>,
        accept: bool,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE uploads SET accepted = $1, accepted_time = NOW(), accepted_by = $2, reason = $3 WHERE id = $4",
        )
            .bind(accept)
            .bind(accepted_by)
            .bind(reason)
            .bind(id)
            .execute(&*self.pool)
            .await?;

        Ok(())
    }

    pub async fn get_user_stats(&self, id: i64) -> Option<UserStats> {
        sqlx::query_as::<_, UserStats>(
            "SELECT
                users.id, users.account_id,
                users.username, users.role,
                COUNT(uploads.id) AS upload_count,
                COUNT(DISTINCT uploads.level_id) AS level_count,
                COUNT(uploads.id) FILTER (WHERE uploads.accepted = TRUE) AS accepted_upload_count,
                COUNT(uploads.id) FILTER (WHERE uploads.accepted = FALSE AND uploads.accepted_time IS NULL) AS pending_upload_count,
                COUNT(DISTINCT uploads.level_id) FILTER (WHERE uploads.accepted = TRUE) AS accepted_level_count,
                (
                    SELECT COUNT(*)
                    FROM (
                        SELECT u.level_id
                        FROM uploads u
                        WHERE u.accepted = TRUE
                          AND u.deleted_at IS NULL
                          AND u.user_id = users.id
                          AND u.upload_time = (
                            SELECT MAX(u2.upload_time)
                            FROM uploads u2
                            WHERE u2.level_id = u.level_id
                              AND u2.accepted = TRUE
                              AND u2.deleted_at IS NULL
                          )
                    ) active_levels
                ) AS active_thumbnail_count
             FROM users
             LEFT JOIN uploads ON users.id = uploads.user_id
             WHERE users.id = $1
             GROUP BY users.id, users.account_id, users.username, users.role",
        )
            .bind(id)
            .fetch_optional(&*self.pool)
            .await
            .ok()?
    }

    pub async fn get_user_history(
        &self,
        id: i64,
        months: i64,
    ) -> Result<Vec<UserHistoryPoint>, sqlx::Error> {
        let months = months.clamp(1, 24) as i32;
        let current_month = month_start(Utc::now().date_naive());
        let start_month = add_months(current_month, -(months - 1));
        let end_month = add_months(current_month, 1);

        let rows = sqlx::query_as::<_, UserHistoryPoint>(
            "SELECT
                date_trunc('month', upload_time)::date AS period,
                COUNT(*) AS upload_count,
                COUNT(*) FILTER (WHERE accepted = TRUE) AS accepted_upload_count,
                COUNT(*) FILTER (WHERE accepted = FALSE AND accepted_time IS NULL) AS pending_upload_count,
                COUNT(DISTINCT level_id) AS level_count,
                COUNT(DISTINCT level_id) FILTER (WHERE accepted = TRUE) AS accepted_level_count
             FROM uploads
             WHERE user_id = $1
               AND upload_time >= $2
               AND upload_time < $3
             GROUP BY 1
             ORDER BY 1",
        )
            .bind(id)
            .bind(start_month.and_hms_opt(0, 0, 0).expect("invalid start month time"))
            .bind(end_month.and_hms_opt(0, 0, 0).expect("invalid end month time"))
            .fetch_all(&*self.pool)
            .await?;

        let mut by_period = HashMap::with_capacity(rows.len());
        for row in rows {
            by_period.insert(row.period, row);
        }

        let mut history = Vec::with_capacity(months as usize);
        let mut period = start_month;

        for _ in 0..months {
            if let Some(row) = by_period.remove(&period) {
                history.push(row);
            } else {
                history.push(UserHistoryPoint {
                    period,
                    upload_count: 0,
                    accepted_upload_count: 0,
                    pending_upload_count: 0,
                    level_count: 0,
                    accepted_level_count: 0,
                });
            }
            period = add_months(period, 1);
        }

        Ok(history)
    }

    pub async fn migrate_user_account(
        &self,
        old_account_id: i64,
        new_account_id: i64,
    ) -> Result<User, sqlx::Error> {
        sqlx::query("CALL migrate($1, $2)")
            .bind(new_account_id)
            .bind(old_account_id)
            .execute(&*self.pool)
            .await?;

        sqlx::query_as::<_, User>("SELECT * FROM users WHERE id = $1")
            .bind(new_account_id)
            .fetch_one(&*self.pool)
            .await
    }

    pub async fn save_settings(&self) -> Result<(), std::io::Error> {
        let settings = self.settings.read().await;
        let settings_data = serde_json::to_string_pretty(&*settings)?;
        tokio::fs::write("state.json", settings_data).await
    }

    pub async fn create_stats_snapshot(
        &self,
        storage_bytes: i64,
        thumbnails_count: i64,
        users_per_month: Option<i64>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO stats_snapshots (
                storage_bytes,
                thumbnails_count,
                users_per_month,
                users_total,
                uploads_total,
                pending_uploads_total,
                accepted_uploads_total
            )
            VALUES (
                $1,
                $2,
                $3,
                (SELECT COUNT(*) FROM users),
                (SELECT COUNT(*) FROM uploads),
                (SELECT COUNT(*) FROM uploads WHERE accepted = FALSE AND accepted_time IS NULL),
                (SELECT COUNT(*) FROM uploads WHERE accepted = TRUE)
            )",
        )
        .bind(storage_bytes)
        .bind(thumbnails_count)
        .bind(users_per_month)
        .execute(&*self.pool)
        .await?;

        Ok(())
    }

    pub async fn get_recent_stats_snapshots(
        &self,
        limit: i64,
    ) -> Result<Vec<StatsSnapshot>, sqlx::Error> {
        sqlx::query_as::<_, StatsSnapshot>(
            "SELECT
                id,
                captured_at,
                storage_bytes,
                thumbnails_count,
                users_per_month,
                users_total,
                uploads_total,
                pending_uploads_total,
                accepted_uploads_total
             FROM stats_snapshots
             ORDER BY captured_at DESC, id DESC
             LIMIT $1",
        )
        .bind(limit)
        .fetch_all(&*self.pool)
        .await
    }

    pub async fn get_recent_stats_snapshots_ascending(
        &self,
        limit: i64,
    ) -> Result<Vec<StatsSnapshot>, sqlx::Error> {
        sqlx::query_as::<_, StatsSnapshot>(
            "SELECT id, captured_at, storage_bytes, thumbnails_count, users_per_month, users_total, uploads_total, pending_uploads_total, accepted_uploads_total
             FROM (
                 SELECT id, captured_at, storage_bytes, thumbnails_count, users_per_month, users_total, uploads_total, pending_uploads_total, accepted_uploads_total
                 FROM stats_snapshots
                 ORDER BY captured_at DESC, id DESC
                 LIMIT $1
             ) recent
             ORDER BY captured_at, id",
        )
            .bind(limit)
            .fetch_all(&*self.pool)
            .await
    }

    pub async fn get_total_level_count(&self) -> Result<i64, sqlx::Error> {
        sqlx::query_scalar("SELECT COUNT(DISTINCT level_id) FROM uploads")
            .fetch_one(&*self.pool)
            .await
    }

    pub async fn get_current_pending_upload_count(&self) -> Result<i64, sqlx::Error> {
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM uploads WHERE accepted = FALSE AND accepted_time IS NULL",
        )
        .fetch_one(&*self.pool)
        .await
    }

    pub async fn get_user_ban(&self, user_id: i64) -> Result<Option<UserBan>, sqlx::Error> {
        sqlx::query_as::<_, UserBan>(
            "SELECT id, user_id, ban_time, reason, expires_at
             FROM bans
             WHERE user_id = $1 AND (expires_at IS NULL OR expires_at > NOW())
             ORDER BY ban_time DESC
             LIMIT 1",
        )
        .bind(user_id)
        .fetch_optional(&*self.pool)
        .await
    }

    pub async fn ban_user(
        &self,
        user_id: i64,
        reason: String,
        banned_by: i64,
        expires_at: Option<NaiveDateTime>
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO bans (user_id, reason, banned_by, expires_at)
             VALUES ($1, $2, $3, $4)",
        )
        .bind(user_id)
        .bind(reason)
        .bind(banned_by)
        .bind(expires_at)
        .execute(&*self.pool)
        .await?;

        Ok(())
    }

    pub async fn unban_user(&self, user_id: i64) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE bans
             SET expires_at = NOW()
             FROM (
               SELECT id AS bid
               FROM bans
               WHERE user_id = $1 AND (expires_at IS NULL OR expires_at > NOW())
               ORDER BY ban_time DESC
               LIMIT 1
             ) sel
             WHERE bans.id = sel.bid",
        )
        .bind(user_id)
        .execute(&*self.pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }
}

pub async fn get_db() -> AppState {
    AppState::new().await
}
