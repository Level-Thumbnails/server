use crate::{db, util};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde::{Serialize, Deserialize};
use util::MessageResponse;

const ONLINE_MODERATOR_WINDOW_MINUTES: i64 = 5;

/// Represents a snapshot of server statistics at a specific point in time.
#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ServerStats {
    /// Total number of accepted uploads (including ones that got replaced later)
    accepted_uploads_total: i64,
    /// Number of uploads currently pending review
    current_pending_uploads: i64,
    /// Deprecated: Number of uploads in pending review in the latest snapshot. Use `current_pending_uploads` for real-time count instead.
    pending_uploads_total: i64,
    /// Total storage used by all thumbnails in bytes
    storage: i64,
    /// Total number of unique thumbnails stored on the server
    thumbnails: i64,
    /// How many unique levels has the server seen (includes rejected ones)
    total_levels: i64,
    /// Total number of uploads (including accepted, rejected, and pending) that the server has processed
    uploads_total: i64,
    /// Number of unique users (IPs) that have accessed any endpoint on the server in the past 30 days. (Uses Cloudflare data, may not be perfectly accurate)
    users_per_month: i64,
    /// Total number of registered users
    users_total: i64,
    /// Usernames of moderators seen online in the last 5 minutes.
    online_moderators: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct StatsResponse {
    /// HTTP status code
    pub status: u16,
    /// Payload data
    pub data: ServerStats,
}

#[utoipa::path(
    get,
    path = "/stats",
    description = "Get overall server statistics, including total storage used, total levels, total uploads, and user counts. \
    This endpoint is useful for monitoring the health and usage of the server. \
    Data is fetched once per hour and cached, so it may not reflect real-time values.",
    tag = "Server Status",
    responses(
        (
            status = 200,
            description = "Successful response with server statistics",
            body = StatsResponse,
            example = json!({
              "data": {
                "accepted_uploads_total": 122400,
                "current_pending_uploads": 171,
                "pending_uploads_total": 174,
                "storage": 45032154480i64,
                "thumbnails": 114091,
                "total_levels": 123965,
                "uploads_total": 142414,
                "users_per_month": 6122359,
                "users_total": 21215,
                "online_moderators": ["prevter"]
              },
              "status": 200
            })
        ),
        (
            status = 500,
            description = "Internal server error",
            body = MessageResponse,
            example = json!({"status": 500, "message": "Failed to fetch stats snapshot: <error details>"})
        ),
    )
)]
pub async fn get_stats(State(db): State<db::AppState>) -> Response {
    let latest_snapshot = match db.get_recent_stats_snapshots(1).await {
        Ok(mut snapshots) => snapshots.pop(),
        Err(e) => {
            return util::str_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Failed to fetch stats snapshot: {}", e),
            )
        }
    };

    let total_levels = match db.get_total_level_count().await {
        Ok(value) => value,
        Err(e) => {
            return util::str_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Failed to fetch total level count: {}", e),
            )
        }
    };

    let current_pending_uploads = match db.get_current_pending_upload_count().await {
        Ok(value) => value,
        Err(e) => {
            return util::str_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Failed to fetch pending upload count: {}", e),
            )
        }
    };

    let snapshot = latest_snapshot;
    let online_moderators = db
        .get_online_moderators(ONLINE_MODERATOR_WINDOW_MINUTES)
        .await
        .into_iter()
        .map(|(_, username)| username)
        .collect();

    util::response(
        StatusCode::OK,
        serde_json::to_value(StatsResponse {
            status: StatusCode::OK.as_u16(),
            data: ServerStats {
                storage: snapshot.as_ref().map(|s| s.storage_bytes).unwrap_or(0),
                thumbnails: snapshot.as_ref().map(|s| s.thumbnails_count).unwrap_or(0),
                users_per_month: snapshot.as_ref().and_then(|s| s.users_per_month).unwrap_or(0),
                users_total: snapshot.as_ref().map(|s| s.users_total).unwrap_or(0),
                uploads_total: snapshot.as_ref().map(|s| s.uploads_total).unwrap_or(0),
                pending_uploads_total: snapshot.as_ref().map(|s| s.pending_uploads_total).unwrap_or(0),
                accepted_uploads_total: snapshot.as_ref().map(|s| s.accepted_uploads_total).unwrap_or(0),
                total_levels,
                current_pending_uploads,
                online_moderators,
            }
        }).unwrap()
    )
}

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
pub struct StatsHistoryQueryParams {
    /// Optional limit on how many historical snapshots to return. Defaults to 72 (3 days). Max is 720 (30 days).
    pub limit: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct StatsHistoryResponse {
    /// HTTP status code
    pub status: u16,
    /// Payload data
    pub data: Vec<db::StatsSnapshot>,
}

#[utoipa::path(
    get,
    path = "/stats/history",
    description = "Get historical server statistics snapshots for up to past 30 days.",
    tag = "Server Status",
    params(
        StatsHistoryQueryParams,
    ),
    responses(
        (
            status = 200,
            description = "Successful response with historical server statistics snapshots",
            body = StatsHistoryResponse,
            example = json!({
                "status": 200,
                "data": [{
                  "accepted_uploads_total": 120379,
                  "captured_at": "2026-05-23T23:55:04.306971",
                  "id": 1775,
                  "pending_uploads_total": 378,
                  "storage_bytes": 44422887552i64,
                  "thumbnails_count": 112260,
                  "uploads_total": 139827,
                  "users_per_month": 6058329,
                  "users_total": 20822
                }]
            })
        ),
        (
            status = 500,
            description = "Internal server error",
            body = MessageResponse,
            example = json!({"status": 500, "message": "Failed to fetch stats history: <error details>"})
        ),
    )
)]
pub async fn get_stats_history(
    Query(params): Query<StatsHistoryQueryParams>,
    State(db): State<db::AppState>,
) -> Response {
    let limit = params.limit.unwrap_or(72).clamp(1, 720);

    match db.get_recent_stats_snapshots_ascending(limit).await {
        Ok(history) => util::response(
            StatusCode::OK,
            serde_json::json!({
                "status": StatusCode::OK.as_u16(),
                "data": history,
            }),
        ),
        Err(e) => util::str_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to fetch stats history: {}", e),
        ),
    }
}


