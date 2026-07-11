pub const PENDING_UPLOAD_SELECT: &str = "SELECT uploads.id, user_id, users.username AS username, level_id, accepted, upload_time, accepted_time, users.account_id AS account_id, users.role AS user_role, \
     row_to_json(notes) AS note_data
     FROM uploads \
     LEFT JOIN users ON users.id = user_id \
     LEFT JOIN notes ON notes.upload_id = uploads.id \
     WHERE accepted = FALSE AND accepted_time IS NULL";

pub const MAX_MY_UPLOADS_PAGE_SIZE: u32 = 100;

pub const USER_STATS_CTE: &str = r#"WITH upload_counts AS (
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
