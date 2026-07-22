use crate::routes::thumbnail;
use crate::util::MessageResponse;
use crate::{cache_controller, db, util};
use crate::webhooks::{SystemNotification, WebhookClient};
use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::Response;
use db::NoteData;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::cmp::PartialEq;
use webp::Encoder;

const IMAGE_WIDTH: u32 = 1920;
const IMAGE_HEIGHT: u32 = 1080;
const DEFAULT_PENDING_PAGE_SIZE: u32 = 24;
const MAX_PENDING_PAGE_SIZE: u32 = 100;
const SUBMISSION_NOTE_HEADER: &str = "x-submission-note";

#[derive(Debug, Deserialize, Default, utoipa::ToSchema)]
pub struct LockLevelPayload {
    /// Optional reason for locking the level. Displayed to users who attempt to upload thumbnails for the locked level.
    pub reason: Option<String>,
}

/// Response type for the GET /thumbnail/{id}/lock endpoint
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct LevelLockResponse {
    /// HTTP status code of the response
    pub status: u16,
    /// Level lock information. Null if the level is not currently locked.
    pub data: Option<db::LevelLock>,
}

/// Helper function to authenticate moderator/admin
async fn authenticate_moderator(
    headers: &HeaderMap,
    db: &db::AppState,
) -> Result<db::User, Response> {
    let user = util::auth_middleware(headers, db).await?;

    if !user.role.can_moderate_pending_uploads() {
        return Err(util::str_response(
            StatusCode::FORBIDDEN,
            "Only moderators, admins, or owners can perform this action",
        ));
    }

    let _ = db.touch_moderator_seen(user.id, user.username.clone()).await;

    Ok(user)
}

async fn authenticate_admin(headers: &HeaderMap, db: &db::AppState) -> Result<db::User, Response> {
    let user = util::auth_middleware(headers, db).await?;

    if !user.role.can_manage_level_locks() {
        return Err(util::str_response(
            StatusCode::FORBIDDEN,
            "Admin or Owner privileges required",
        ));
    }

    let _ = db.touch_moderator_seen(user.id, user.username.clone()).await;

    Ok(user)
}

// Helper function to validate image dimensions and convert to WebP
fn process_image(data: &[u8]) -> Result<Vec<u8>, String> {
    let image = image::load_from_memory(data).map_err(|e| format!("Invalid image data: {}", e))?;

    if image.width() != IMAGE_WIDTH || image.height() != IMAGE_HEIGHT {
        return Err(format!("Image must be exactly {}x{}", IMAGE_WIDTH, IMAGE_HEIGHT));
    }

    let rgb_data = image.into_rgb8();
    let encoder = Encoder::from_rgb(&rgb_data, IMAGE_WIDTH, IMAGE_HEIGHT);
    Ok(encoder.encode_lossless().to_owned())
}

// Handler for uploading images for admins/moderators (and verified for new thumbnails)
async fn force_save(
    id: u64,
    image_data: &[u8],
    submission_note: NoteData,
    user: &db::User,
    db: &db::AppState,
) -> Result<(), String> {
    let upload_id = db
        .add_upload(id as i64, user.id, "", true, submission_note)
        .await
        .map_err(|e| format!("Failed to add upload entry: {}", e))?;

    let image_path = util::get_upload_path(upload_id);
    tokio::fs::write(&image_path, image_data)
        .await
        .map_err(|e| format!("Failed to save image: {}", e))?;

    let new_image_path = format!("thumbnails/{}.webp", id);
    let _ = tokio::fs::remove_file(&new_image_path).await;
    tokio::fs::hard_link(&image_path, &new_image_path)
        .await
        .map_err(|e| format!("Failed to move image to final location: {}", e))?;

    db.add_active_thumbnail(id).await;
    cache_controller::purge(id as i64);
    thumbnail::purge_resize_cache(id as i64).await;
    Ok(())
}

async fn add_to_pending(
    id: u64,
    image_data: &[u8],
    submission_note: NoteData,
    user: &db::User,
    db: &db::AppState,
) -> Response {
    if db.settings.read().await.pause_submissions {
        return util::str_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "Thumbnail submissions are currently closed because there are too many submissions. \
            Please wait for thumbnail moderators to clear the queue, and then submissions will reopen.",
        );
    }

    match db.add_upload(id as i64, user.id, "", false, submission_note).await {
        Ok(upload_id) => {
            let image_path = util::get_upload_path(upload_id);
            match tokio::fs::write(&image_path, image_data).await {
                Ok(_) => {}
                Err(e) => {
                    // TODO: should probably roll back the upload entry here
                    return util::str_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        &format!("Failed to save pending image: {}", e),
                    );
                }
            }

            util::str_response(
                StatusCode::ACCEPTED,
                &format!("Image for level ID {} is now pending", id),
            )
        }
        Err(e) => util::str_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to add pending upload entry: {}", e),
        ),
    }
}

pub async fn is_image_uploaded(id: u64) -> bool {
    let image_path = format!("thumbnails/{}.webp", id);
    tokio::fs::try_exists(&image_path).await.unwrap_or(false)
}

pub async fn upload(
    State(db): State<db::AppState>,
    headers: HeaderMap,
    Path(id): Path<u64>,
    data: Bytes,
) -> Response {
    let user = match util::auth_middleware(&headers, &db).await {
        Ok(user) => user,
        Err(response) => return response,
    };

    match db.get_user_ban(user.id).await {
        Ok(Some(ban)) => {
            return if let Some(expires_at) = ban.expires_at {
                util::response(
                    StatusCode::FORBIDDEN,
                    json!({
                        "status": StatusCode::FORBIDDEN.as_u16(),
                        // TODO: remove the reason from message once the mod actually reads the "reason" field
                        "message": format!(
                            "You are banned from uploading thumbnails until {}. Reason: {}",
                            expires_at.format("%Y-%m-%d %H:%M:%S").to_string(),
                            ban.reason
                        ),
                        "reason": ban.reason,
                        "expires_at": expires_at,
                    }),
                )
            } else {
                util::response(
                    StatusCode::FORBIDDEN,
                    json!({
                        "status": StatusCode::FORBIDDEN.as_u16(),
                        // TODO: remove the reason from message once the mod actually reads the "reason" field
                        "message": format!("You are banned from uploading thumbnails. Reason: {}", ban.reason),
                        "reason": ban.reason,
                    }),
                )
            };
        }
        Ok(None) => (),
        Err(e) => {
            return util::str_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Failed to check user ban status: {}", e),
            );
        }
    };

    let ua = match util::parse_useragent(&headers) {
        Some(ua) => {
            if db.settings.read().await.min_supported_client.is_newer_than(&ua.version) {
                return util::str_response(
                    StatusCode::UPGRADE_REQUIRED,
                    &format!(
                        "Your Level Thumbnails version ({}) is outdated. Please update to the latest version to upload thumbnails.",
                        ua.version
                    ),
                );
            }
            ua
        }
        None => {
            return util::str_response(
                StatusCode::UPGRADE_REQUIRED,
                "Your game version is not supported. Please update Geometry Dash and install the latest version of Level Thumbnails mod.",
            );
        }
    };

    let Some(value) = headers.get(SUBMISSION_NOTE_HEADER) else {
        return util::str_response(StatusCode::BAD_REQUEST, "Missing submission note header");
    };

    let note = match util::parse_submission_note(value.to_str().unwrap_or_default()) {
        Ok(note) => NoteData::from_parsed(note, ua),
        Err(response) => return util::str_response(StatusCode::BAD_REQUEST, &response),
    };

    // allow admins and owners to bypass locks
    if !user.role.can_bypass_level_locks() {
        match db.get_level_lock(id as i64).await {
            Ok(Some(lock)) => {
                return util::response(
                    StatusCode::LOCKED,
                    json!({
                        "status": 423,
                        "message": "Thumbnail submissions are locked for this level",
                        "reason": lock.reason
                    }),
                );
            }
            Ok(None) => {}
            Err(e) => {
                return util::str_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &format!("Failed to check level lock: {}", e),
                );
            }
        }
    }

    // Process and validate the image
    let webp_data = match process_image(&data) {
        Ok(data) => data,
        Err(e) => return util::str_response(StatusCode::BAD_REQUEST, &e),
    };

    if user.role.can_upload_replacement_directly() {
        match force_save(id, &webp_data, note, &user, &db).await {
            Ok(_) => util::str_response(
                StatusCode::CREATED,
                &format!("Image for level ID {} uploaded", id),
            ),
            Err(e) => util::str_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Error saving image: {}", e),
            ),
        }
    } else if user.role.can_upload_new_thumbnail_directly() {
        if !is_image_uploaded(id).await {
            match force_save(id, &webp_data, note, &user, &db).await {
                Ok(_) => util::str_response(
                    StatusCode::CREATED,
                    &format!("Image for level ID {} uploaded", id),
                ),
                Err(e) => util::str_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &format!("Error saving image: {}", e),
                ),
            }
        } else {
            // Image exists, add to pending for approval
            add_to_pending(id, &webp_data, note, &user, &db).await
        }
    } else {
        // Regular users must go through approval process
        add_to_pending(id, &webp_data, note, &user, &db).await
    }
}

#[derive(PartialEq)]
enum PendingFilter {
    All,
    ByUser(i64),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct PendingQueryParams {
    page: u32,
    per_page: u32,
    replacement_only: bool,
    new_only: bool,
    level_id: Option<i64>,
    user_id: Option<i64>,
    username: Option<String>,
    search: Option<String>,
    rated_only: bool,
    from_creator_only: bool,
    sort_by: Option<db::PendingUploadSortBy>,
    sort_dir: Option<db::SortDirection>,
}

impl Default for PendingQueryParams {
    fn default() -> Self {
        Self {
            page: 1,
            per_page: DEFAULT_PENDING_PAGE_SIZE,
            replacement_only: false,
            new_only: false,
            level_id: None,
            user_id: None,
            username: None,
            search: None,
            rated_only: false,
            from_creator_only: false,
            sort_by: None,
            sort_dir: None,
        }
    }
}

impl PendingQueryParams {
    fn sanitized(mut self) -> Self {
        if self.page == 0 {
            self.page = 1;
        }

        if self.per_page == 0 {
            self.per_page = DEFAULT_PENDING_PAGE_SIZE;
        }

        self.per_page = self.per_page.min(MAX_PENDING_PAGE_SIZE);
        self
    }
}

#[derive(Serialize)]
struct PendingUploadsResponse {
    uploads: Vec<db::PendingUpload>,
    page: u32,
    per_page: u32,
    total: i64,
}

async fn get_pending_uploads(
    headers: HeaderMap,
    db: &db::AppState,
    filter: PendingFilter,
    query: PendingQueryParams,
) -> Response {
    let user = match filter {
        PendingFilter::ByUser(_) => match util::auth_middleware(&headers, db).await {
            Ok(user) => user,
            Err(response) => return response,
        },
        _ => match authenticate_moderator(&headers, db).await {
            Ok(user) => user,
            Err(response) => return response,
        },
    };

    let mut sanitized_query = query.sanitized();

    match filter {
        PendingFilter::ByUser(user_id) => {
            if user.id != user_id && !user.role.can_moderate_pending_uploads() {
                return util::str_response(
                    StatusCode::FORBIDDEN,
                    "You can only view your own pending uploads",
                );
            }
            sanitized_query.user_id = Some(user_id);
        }
        PendingFilter::All => {}
    }

    let options = db::PendingQueryOptions {
        page: sanitized_query.page,
        per_page: sanitized_query.per_page,
        level_id: sanitized_query.level_id,
        user_id: sanitized_query.user_id,
        username: sanitized_query.username.clone(),
        search: sanitized_query.search.clone(),
        rated_only: sanitized_query.rated_only,
        from_creator_only: sanitized_query.from_creator_only,
        replacement_only: sanitized_query.replacement_only,
        new_only: sanitized_query.new_only,
        sort_by: sanitized_query.sort_by.unwrap_or(db::PendingUploadSortBy::UploadTime),
        sort_dir: sanitized_query.sort_dir.unwrap_or(db::SortDirection::Asc),
    };

    match db.get_pending_uploads_paginated(options).await {
        Ok(mut page) => {
            for upload in &mut page.uploads {
                upload.replacement = is_image_uploaded(upload.level_id as u64).await;
            }

            let response = PendingUploadsResponse {
                uploads: page.uploads,
                page: sanitized_query.page,
                per_page: sanitized_query.per_page,
                total: page.total,
            };

            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "application/json")
                .body(serde_json::to_string(&response).unwrap().into())
                .unwrap()
        }
        Err(e) => util::str_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Error fetching pending uploads: {}", e),
        ),
    }
}

pub async fn get_pending_uploads_for_level(
    State(db): State<db::AppState>,
    Path(id): Path<i64>,
) -> Response {
    match db.get_pending_count_for_level(id).await {
        Ok(count) => util::response(
            StatusCode::OK,
            json!({
                "status": 200,
                "count": count
            }),
        ),
        Err(e) => util::str_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Error fetching pending upload count for level {}: {}", id, e),
        ),
    }
}

pub async fn get_all_pending_uploads(
    headers: HeaderMap,
    State(db): State<db::AppState>,
    Query(params): Query<PendingQueryParams>,
) -> Response {
    get_pending_uploads(headers, &db, PendingFilter::All, params).await
}

pub async fn get_pending_uploads_for_user(
    headers: HeaderMap,
    State(db): State<db::AppState>,
    Path(id): Path<i64>,
    Query(params): Query<PendingQueryParams>,
) -> Response {
    get_pending_uploads(headers, &db, PendingFilter::ByUser(id), params).await
}

pub async fn get_pending_info(
    headers: HeaderMap,
    State(db): State<db::AppState>,
    Path(id): Path<i64>,
) -> Response {
    let _user = match authenticate_moderator(&headers, &db).await {
        Ok(user) => user,
        Err(response) => return response,
    };

    match db.get_pending_upload(id).await {
        Ok(mut upload) => {
            upload.replacement = is_image_uploaded(upload.level_id as u64).await;

            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "application/json")
                .body(serde_json::to_string(&upload).unwrap().into())
                .unwrap()
        }
        Err(e) => util::str_response(
            StatusCode::NOT_FOUND,
            &format!("No pending upload found with ID {}: {}", id, e),
        ),
    }
}

#[derive(Deserialize, Serialize)]
pub struct PendingUploadAction {
    pub accepted: bool,
    pub reason: Option<String>,
}

pub async fn pending_action(
    headers: HeaderMap,
    State(db): State<db::AppState>,
    Path(id): Path<i64>,
    Json(action): Json<PendingUploadAction>,
) -> Response {
    let user = match authenticate_moderator(&headers, &db).await {
        Ok(user) => user,
        Err(response) => return response,
    };

    let old_image_path = util::get_upload_path(id);
    if !tokio::fs::try_exists(&old_image_path).await.unwrap_or(false) {
        return util::str_response(
            StatusCode::NOT_FOUND,
            &format!("No pending image found for upload ID {}", id),
        );
    }

    let upload = match db.get_pending_upload(id).await {
        Ok(upload) => upload,
        Err(e) => {
            return util::str_response(
                StatusCode::NOT_FOUND,
                &format!("No pending upload found with ID {}: {}", id, e),
            );
        }
    };

    if upload.accepted_time.is_some() {
        return util::str_response(StatusCode::CONFLICT, "This upload has already been reviewed");
    }

    if action.accepted {
        // Accept: hard link the image to the new location
        let new_image_path = format!("thumbnails/{}.webp", upload.level_id);

        let _ = tokio::fs::remove_file(&new_image_path).await;
        if let Err(e) = tokio::fs::hard_link(&old_image_path, &new_image_path).await {
            return util::str_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Error moving image: {}", e),
            );
        }

        if let Err(e) = db.accept_upload(upload.id, user.id, action.reason, true).await {
            return util::str_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Error accepting upload: {}", e),
            );
        }

        db.add_active_thumbnail(upload.level_id as u64).await;
        cache_controller::purge(upload.level_id);
        thumbnail::purge_resize_cache(upload.level_id).await;
        util::str_response(StatusCode::OK, &format!("Upload {} accepted", id))
    } else {
        // Reject: don't touch the image, just mark it as rejected
        if let Err(e) = db.accept_upload(upload.id, user.id, action.reason, false).await {
            return util::str_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Error rejecting upload: {}", e),
            );
        }

        util::str_response(StatusCode::OK, &format!("Upload {} rejected", id))
    }
}

pub async fn get_pending_image(
    State(db): State<db::AppState>,
    Path(id): Path<i64>,
) -> Response {
    match db.get_pending_upload(id).await {
        Ok(upload) => upload,
        Err(e) => {
            return util::str_response(
                StatusCode::NOT_FOUND,
                &format!("No pending upload found with ID {}: {}", id, e),
            );
        }
    };

    let image_path = util::get_upload_path(id);
    let image_data = match tokio::fs::read(&image_path).await {
        Ok(data) => data,
        Err(e) => {
            return util::str_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Error reading image file: {}", e),
            );
        }
    };

    Response::builder()
        .header(header::CONTENT_TYPE, "image/webp")
        .header(
            header::CONTENT_DISPOSITION,
            format!("inline; filename=\"upload_{}.webp\"", id),
        )
        .header(header::CACHE_CONTROL, "public, max-age=31536000, immutable")
        .header(header::CONTENT_LENGTH, image_data.len())
        .body(image_data.into())
        .unwrap()
}

#[utoipa::path(
    get,
    path = "/thumbnail/{id}/lock",
    description = "Get the lock status of a level.",
    tag = "Level Locking",
    params(
        ("id" = i64, Path, description = "Geometry Dash Level ID")
    ),
    responses(
        (
            status = 200,
            description = "Successful response with list of locked levels",
            body = LevelLockResponse,
            example = json!({
                "data": {
                    "level_id": 2,
                    "locked_at": "2026-03-12T20:25:43.350067",
                    "locked_by": 4393,
                    "locked_by_username": "prevter",
                    "reason": "preventing vandalism of main levels"
                },
                "status": 200
            })
        ),
        (
            status = 500,
            description = "Internal server error",
            body = MessageResponse,
            example = json!({"status": 500, "message": "Failed to fetch level lock: database error"})
        ),
    )
)]
pub async fn get_level_lock(State(db): State<db::AppState>, Path(id): Path<i64>) -> Response {
    match db.get_level_lock(id).await {
        Ok(lock) => util::response(
            StatusCode::OK,
            serde_json::to_value(LevelLockResponse { status: 200, data: lock }).unwrap(),
        ),
        Err(e) => util::str_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to fetch level lock: {}", e),
        ),
    }
}

#[utoipa::path(
    post,
    path = "/thumbnail/{id}/lock",
    description = "Locks submissions indefinitely for a specified level. Requires admin or higher permissions.",
    tag = "Level Locking",
    security(("bearerAuth" = []), ("cookieAuth" = [])),
    params(
        ("id" = i64, Path, description = "Geometry Dash Level ID")
    ),
    request_body = LockLevelPayload,
    responses(
        (
            status = 200,
            description = "Level successfully locked",
            body = MessageResponse,
            example = json!({"status": 200, "message": "Level 12345 is now locked for submissions"})
        ),
        (
            status = 401,
            description = "Missing or invalid authentication",
            body = MessageResponse,
            example = json!({"status": 401, "message": "Missing Authorization header"})
        ),
        (
            status = 403,
            description = "Admin or Owner privileges required",
            body = MessageResponse,
            example = json!({"status": 403, "message": "Admin or Owner privileges required"})
        ),
        (
            status = 498,
            description = "Invalid token (user not found)",
            body = MessageResponse,
            example = json!({"status": 498, "message": "User not found"})
        ),
        (
            status = 500,
            description = "Internal server error",
            body = MessageResponse,
            example = json!({"status": 500, "message": "Failed to lock level 12345: database error"})
        ),
    )
)]
pub async fn lock_level(
    headers: HeaderMap,
    State(db): State<db::AppState>,
    Path(id): Path<i64>,
    Json(payload): Json<LockLevelPayload>,
) -> Response {
    let user = match authenticate_admin(&headers, &db).await {
        Ok(user) => user,
        Err(response) => return response,
    };

    match db.lock_level(id, user.id, payload.reason.as_deref()).await {
        Ok(_) => {
            let _ = WebhookClient::get().send_system_notification(
                SystemNotification::LevelLocked {
                    level_id: id,
                    reason: payload.reason.clone(),
                    by_username: user.username.clone(),
                    by_role: user.role,
                    by_discord: user.discord_id,
                },
            ).await;

            util::str_response(
                StatusCode::OK,
                &format!("Level {} is now locked for submissions", id),
            )
        },
        Err(e) => util::str_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to lock level {}: {}", id, e),
        ),
    }
}

#[utoipa::path(
    delete,
    path = "/thumbnail/{id}/lock",
    description = "Unlocks submissions for a specified level if they were locked. Requires admin or higher permissions.",
    tag = "Level Locking",
    security(("bearerAuth" = []), ("cookieAuth" = [])),
    params(
        ("id" = i64, Path, description = "Geometry Dash Level ID")
    ),
    responses(
        (
            status = 200,
            description = "Level successfully locked",
            body = MessageResponse,
            example = json!({"status": 200, "message": "Level 12345 is now unlocked for submissions"})
        ),
        (
            status = 401,
            description = "Missing or invalid authentication",
            body = MessageResponse,
            example = json!({"status": 401, "message": "Missing Authorization header"})
        ),
        (
            status = 403,
            description = "Admin or Owner privileges required",
            body = MessageResponse,
            example = json!({"status": 403, "message": "Admin or Owner privileges required"})
        ),
        (
            status = 404,
            description = "Level lock not found",
            body = MessageResponse,
            example = json!({"status": 404, "message": "Level lock not found"})
        ),
        (
            status = 498,
            description = "Invalid token (user not found)",
            body = MessageResponse,
            example = json!({"status": 498, "message": "User not found"})
        ),
        (
            status = 500,
            description = "Internal server error",
            body = MessageResponse,
            example = json!({"status": 500, "message": "Failed to unlock level 12345: database error"})
        ),
    )
)]
pub async fn unlock_level(
    headers: HeaderMap,
    State(db): State<db::AppState>,
    Path(id): Path<i64>,
) -> Response {
    let user = match authenticate_admin(&headers, &db).await {
        Ok(user) => user,
        Err(response) => return response,
    };

    match db.unlock_level(id).await {
        Ok(true) => {
            let _ = WebhookClient::get().send_system_notification(
                SystemNotification::LevelUnlocked {
                    level_id: id,
                    by_username: user.username.clone(),
                    by_role: user.role,
                    by_discord: user.discord_id,
                },
            ).await;

            util::str_response(
                StatusCode::OK,
                &format!("Level {} is now unlocked for submissions", id),
            )
        },
        Ok(false) => util::str_response(StatusCode::NOT_FOUND, "Level lock not found"),
        Err(e) => util::str_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to unlock level {}: {}", id, e),
        ),
    }
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct AllLevelLocksResponse {
    /// HTTP status code of the response
    pub status: u16,
    /// A list of all currently locked levels, including their lock reason and timestamp
    pub locks: Vec<db::LevelLock>,
}

#[utoipa::path(
    get,
    path = "/thumbnail/locks",
    description = "Get all currently locked levels with their lock reason and timestamp. Requires admin or higher permissions.",
    tag = "Level Locking",
    security(("bearerAuth" = []), ("cookieAuth" = [])),
    responses(
        (
            status = 200,
            description = "Successful response with list of locked levels",
            body = AllLevelLocksResponse,
            example = json!({
                "locks": [{
                    "level_id": 2,
                    "locked_at": "2026-03-12T20:25:43.350067",
                    "locked_by": 4393,
                    "locked_by_username": "prevter",
                    "reason": "preventing vandalism of main levels"
                }],
                "status": 200
            })
        ),
        (
            status = 401,
            description = "Missing or invalid authentication",
            body = MessageResponse,
            example = json!({"status": 401, "message": "Missing Authorization header"})
        ),
        (
            status = 403,
            description = "Admin or Owner privileges required",
            body = MessageResponse,
            example = json!({"status": 403, "message": "Admin or Owner privileges required"})
        ),
        (
            status = 498,
            description = "Invalid token (user not found)",
            body = MessageResponse,
            example = json!({"status": 498, "message": "User not found"})
        ),
        (
            status = 500,
            description = "Internal server error",
            body = MessageResponse,
            example = json!({"status": 500, "message": "Failed to fetch locked levels: database error"})
        ),
    )
)]
pub async fn get_all_level_locks(headers: HeaderMap, State(db): State<db::AppState>) -> Response {
    if let Err(response) = authenticate_admin(&headers, &db).await {
        return response;
    }

    match db.get_all_level_locks().await {
        Ok(locks) => util::response(
            StatusCode::OK,
            json!({
                "status": StatusCode::OK.as_u16(),
                "locks": locks,
            }),
        ),
        Err(e) => util::str_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to fetch locked levels: {}", e),
        ),
    }
}
