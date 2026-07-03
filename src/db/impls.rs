use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use chrono::{Datelike, NaiveDate, NaiveDateTime, Utc};
use sqlx::{FromRow, Postgres, QueryBuilder};
use sqlx::postgres::PgPoolOptions;
use tokio::sync::RwLock;
use tracing::{error, warn};
use crate::db::{ActiveUpload, ActiveUploadsPage, AdminUserQueryOptions, AdminUserRow, AdminUsersPage, AppState, BadgeLists, LevelLock, MyUploadsSummary, NoteData, PendingQueryOptions, PendingUpload, PendingUploadsPage, RejectedUpload, RejectedUploadsPage, Settings, StatsSnapshot, UpdateUserOptions, UploadExtended, UploadInfo, User, UserBan, UserHistoryPoint, UserStats, MAX_MY_UPLOADS_PAGE_SIZE, PENDING_UPLOAD_SELECT, USER_STATS_CTE};
use crate::db::filters::{apply_pending_filters, apply_user_filters, apply_user_sort};
use crate::util;
use crate::util::ModUserAgent;

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

async fn migrate_submission_notes_perform(pool: &sqlx::Pool<Postgres>) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;

    #[derive(Debug, Clone, FromRow)]
    struct UploadRow {
        id: i64,
        submission_note: String,
    }

    let rows = sqlx::query_as::<_, UploadRow>(
        "SELECT id, submission_note FROM uploads WHERE submission_note IS NOT NULL"
    )
        .fetch_all(&mut *tx)
        .await?;

    for row in rows {
        let upload_id: i64 = row.id;
        let legacy_note: String = row.submission_note;

        if let Ok(parsed_note) = util::parse_submission_note(&legacy_note) {
            let note_data = NoteData::from_parsed(parsed_note, ModUserAgent::default());
            sqlx::query(
                "INSERT INTO notes (upload_id, level_name, creator_id, creator_name, downloads, likes, stars, length, rating, difficulty, percentage, attempt_time, message, mod_version, mod_platform)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8::length_enum, $9::rating_enum, $10::difficulty_enum, $11, $12, $13, $14, $15)",
            )
                .bind(upload_id)
                .bind(note_data.level_name)
                .bind(note_data.creator_id)
                .bind(note_data.creator_name)
                .bind(note_data.downloads)
                .bind(note_data.likes)
                .bind(note_data.stars)
                .bind(note_data.length)
                .bind(note_data.rating)
                .bind(note_data.difficulty)
                .bind(note_data.percentage)
                .bind(note_data.attempt_time)
                .bind(note_data.message)
                .bind(note_data.mod_version)
                .bind(note_data.mod_platform)
                .execute(&mut *tx)
                .await?;
        } else {
            error!("Failed to parse submission_note for upload id {}: {}", upload_id, legacy_note);
            return Err(sqlx::Error::Protocol(format!("Failed to parse submission_note for upload id {}", upload_id).into()));
        }
    }

    tx.commit().await?;
    warn!("Migration completed successfully!");
    Ok(())
}

async fn migrate_submission_notes(pool: &sqlx::Pool<Postgres>) {
    let row = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (
            SELECT 1
            FROM information_schema.columns
            WHERE table_name='uploads' AND column_name='submission_note'
        )",
    )
        .fetch_one(pool)
        .await
        .expect("Failed to check for submission_note column");

    if row {
        warn!("Migrating old 'submission_note' column");

        if let Err(e) = migrate_submission_notes_perform(pool).await {
            error!("Failed to migrate submission notes: {}", e);
            return; // don't drop the column if migration failed
        }

        sqlx::query("ALTER TABLE uploads DROP COLUMN submission_note")
            .execute(pool)
            .await
            .expect("Failed to drop submission_note column");

        warn!("Dropped old 'submission_note' column after migration");
    }
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

        migrate_submission_notes(&pool).await;

        // load settings from state.json or create default
        let settings = if let Ok(settings_data) = tokio::fs::read_to_string("state.json").await {
            serde_json::from_str(&settings_data).unwrap_or_default()
        } else {
            Settings::default()
        };

        // preload registered users
        let mut set: HashSet<i64>;
        {
            #[derive(Debug, FromRow)]
            struct Row { account_id: i64 }

            let rows: Vec<Row> = sqlx::query_as("SELECT account_id FROM users WHERE account_id != -1")
                .fetch_all(&pool)
                .await
                .expect("Failed to fetch registered users");

            set = HashSet::with_capacity(rows.len());
            for row in rows {
                set.insert(row.account_id);
            }
        }

        AppState {
            pool: Arc::new(pool),
            settings: Arc::new(RwLock::new(settings)),
            online_moderators: Arc::new(RwLock::new(HashMap::new())),
            registered_users: Arc::new(RwLock::new(set)),
        }
    }

    pub async fn touch_moderator_seen(&self, moderator_id: i64, username: String) {
        let mut map = self.online_moderators.write().await;
        map.insert(moderator_id, (username, chrono::Utc::now()));
    }

    pub async fn get_online_moderators(&self, window_minutes: i64) -> Vec<(i64, String)> {
        let map = self.online_moderators.read().await;
        let threshold = Utc::now() - chrono::Duration::minutes(window_minutes);
        map.iter()
            .filter_map(|(id, (username, last_seen))| {
                if *last_seen >= threshold {
                    Some((*id, username.clone()))
                } else {
                    None
                }
            })
            .collect()
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
            let user = sqlx::query_as::<_, User>(
                "INSERT INTO users (account_id, username, role) VALUES ($1, $2, 'user') RETURNING *",
            )
                .bind(account_id)
                .bind(username)
                .fetch_one(&*self.pool)
                .await?;

            self.registered_users.write().await.insert(account_id);
            Ok(user)
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
        submission_note: NoteData,
    ) -> Result<(), sqlx::Error> {
        let upload_id: i64 = if accepted {
            sqlx::query_scalar(
                "INSERT INTO uploads (level_id, user_id, image_path, accepted, accepted_time, accepted_by)
                 VALUES ($1, $2, $3, TRUE, NOW(), $2) RETURNING id",
            )
                .bind(level_id)
                .bind(user_id)
                .bind(image_path)
                .fetch_one(&*self.pool)
                .await?
        } else {
            sqlx::query_scalar(
                "INSERT INTO uploads (level_id, user_id, image_path, accepted)
                 VALUES ($1, $2, $3, FALSE)
                 ON CONFLICT (user_id, level_id)
                 WHERE accepted = FALSE AND accepted_time IS NULL
                 DO UPDATE SET
                     image_path = EXCLUDED.image_path,
                     upload_time = NOW()
                 RETURNING id",
            )
                .bind(level_id)
                .bind(user_id)
                .bind(image_path)
                .fetch_one(&*self.pool)
                .await?
        };

        sqlx::query(
            "INSERT INTO notes (upload_id, level_name, creator_id, creator_name, downloads, likes, stars, length, rating, difficulty, percentage, attempt_time, message, mod_version, mod_platform)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8::length_enum, $9::rating_enum, $10::difficulty_enum, $11, $12, $13, $14, $15)
             ON CONFLICT (upload_id)
             DO UPDATE SET
                level_name = EXCLUDED.level_name,
                creator_id = EXCLUDED.creator_id,
                creator_name = EXCLUDED.creator_name,
                downloads = EXCLUDED.downloads,
                likes = EXCLUDED.likes,
                stars = EXCLUDED.stars,
                length = EXCLUDED.length,
                rating = EXCLUDED.rating,
                difficulty = EXCLUDED.difficulty,
                percentage = EXCLUDED.percentage,
                attempt_time = EXCLUDED.attempt_time,
                message = EXCLUDED.message,
                mod_version = EXCLUDED.mod_version,
                mod_platform = EXCLUDED.mod_platform",
        )
            .bind(upload_id)
            .bind(submission_note.level_name)
            .bind(submission_note.creator_id)
            .bind(submission_note.creator_name)
            .bind(submission_note.downloads)
            .bind(submission_note.likes)
            .bind(submission_note.stars)
            .bind(submission_note.length)
            .bind(submission_note.rating)
            .bind(submission_note.difficulty)
            .bind(submission_note.percentage)
            .bind(submission_note.attempt_time)
            .bind(submission_note.message)
            .bind(submission_note.mod_version)
            .bind(submission_note.mod_platform)
            .execute(&*self.pool)
            .await?;

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

    pub async fn get_pending_uploads_paginated(
        &self,
        options: PendingQueryOptions,
    ) -> Result<PendingUploadsPage, sqlx::Error> {
        if options.replacement_only || options.new_only {
            let mut builder = QueryBuilder::new(PENDING_UPLOAD_SELECT);
            apply_pending_filters(&mut builder, &options);
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
            apply_pending_filters(&mut data_builder, &options);
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
            apply_pending_filters(&mut count_builder, &options);
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
                    row_to_json(notes) AS note_data
                FROM uploads
                LEFT JOIN notes ON uploads.id = notes.upload_id
                WHERE uploads.accepted = TRUE AND uploads.deleted_at IS NULL
                ORDER BY uploads.level_id, uploads.upload_time DESC, uploads.id DESC
            )
            SELECT id, level_id, upload_time, accepted_time, note_data
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
                uploads.reason,
                row_to_json(notes) AS note_data,
                accepted_by.username AS accepted_by_username
            FROM uploads
            LEFT JOIN users AS accepted_by ON accepted_by.id = uploads.accepted_by
            LEFT JOIN notes ON notes.upload_id = uploads.id
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
        apply_user_filters(&mut data_builder, &options);
        apply_user_sort(&mut data_builder, options.sort_by, options.sort_dir);
        data_builder.push(" LIMIT ").push_bind(per_page).push(" OFFSET ").push_bind(offset);

        let users = data_builder.build_query_as::<AdminUserRow>().fetch_all(&*self.pool).await?;

        let mut count_builder = QueryBuilder::new(count_query);
        apply_user_filters(&mut count_builder, &options);
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

    pub async fn get_user_by_gd_id(&self, account_id: i64) -> Option<User> {
        sqlx::query_as::<_, User>("SELECT * FROM users WHERE account_id = $1")
            .bind(account_id)
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

    pub async fn get_all_badges(&self) -> Result<BadgeLists, sqlx::Error> {
        sqlx::query_as::<_, BadgeLists>(
            r#"
            SELECT
                COALESCE(array_agg(account_id) FILTER (WHERE role = 'verified'), '{}') AS verified,
                COALESCE(array_agg(account_id) FILTER (WHERE role = 'moderator'), '{}') AS moderator,
                COALESCE(array_agg(account_id) FILTER (WHERE role = 'admin'), '{}') AS admin,
                COALESCE(array_agg(account_id) FILTER (WHERE role = 'owner'), '{}') AS owner
            FROM users
            WHERE account_id > 0
            "#
        )
            .fetch_one(&*self.pool)
            .await
    }
}
