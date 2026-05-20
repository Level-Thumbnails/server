use crate::{cache_controller, database, util};
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use serde::Deserialize;
use crate::routes::thumbnail;
use crate::util::VersionInfo;

const DEFAULT_ADMIN_USER_PAGE_SIZE: u32 = 50;
const MAX_ADMIN_USER_PAGE_SIZE: u32 = 100;

pub async fn admin_middleware(
    headers: &HeaderMap,
    db: &database::AppState,
) -> Result<database::User, Response> {
    match util::auth_middleware(headers, db).await {
        Ok(user) => {
            if user.role.can_manage_settings() {
                Ok(user)
            } else {
                Err(util::str_response(StatusCode::FORBIDDEN, "Admin or Owner privileges required"))
            }
        }
        Err(resp) => Err(resp),
    }
}

pub async fn mod_middleware(
    headers: &HeaderMap,
    db: &database::AppState,
) -> Result<database::User, Response> {
    match util::auth_middleware(headers, db).await {
        Ok(user) => {
            if user.role.can_moderate_pending_uploads() {
                Ok(user)
            } else {
                Err(util::str_response(StatusCode::FORBIDDEN, "Moderator, Admin, or Owner privileges required"))
            }
        }
        Err(resp) => Err(resp),
    }
}

pub async fn get_settings(headers: HeaderMap, State(db): State<database::AppState>) -> Response {
    match admin_middleware(&headers, &db).await {
        Ok(_) => util::response(
            StatusCode::OK,
            serde_json::to_value(&*db.settings.read().await).unwrap(),
        ),
        Err(resp) => resp,
    }
}

#[derive(Deserialize, Debug)]
pub struct UpdateSettingsPayload {
    pub pause_submissions: bool,
    pub min_supported_client: String,
}

pub async fn update_settings(
    headers: HeaderMap,
    State(db): State<database::AppState>,
    Json(payload): Json<UpdateSettingsPayload>,
) -> Response {
    match admin_middleware(&headers, &db).await {
        Ok(_) => {
            {
                let mut settings = db.settings.write().await;
                settings.pause_submissions = payload.pause_submissions;
                settings.min_supported_client = match VersionInfo::from_str(&payload.min_supported_client) {
                    Some(version) => version,
                    None => {
                        return util::str_response(
                            StatusCode::BAD_REQUEST,
                            "Invalid version format for min_supported_client",
                        );
                    }
                };
            }

            match db.save_settings().await {
                Ok(_) => util::str_response(StatusCode::OK, "Settings updated successfully"),
                Err(e) => util::str_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &format!("Failed to save settings: {}", e),
                ),
            }
        }
        Err(resp) => resp,
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct AdminUsersQueryParams {
    pub page: Option<u32>,
    pub per_page: Option<u32>,
    pub id: Option<i64>,
    pub username: Option<String>,
    pub account_id: Option<i64>,
    pub discord_id: Option<i64>,
    pub role: Option<database::Role>,
    pub total_uploads: Option<i64>,
    pub banned: Option<bool>,
    pub sort_by: Option<database::UserListSortBy>,
    pub sort_dir: Option<database::SortDirection>,
}

pub async fn get_users(
    headers: HeaderMap,
    Query(params): Query<AdminUsersQueryParams>,
    State(db): State<database::AppState>,
) -> Response {
    match mod_middleware(&headers, &db).await {
        Ok(_) => {
            let page = params.page.unwrap_or(1).max(1);
            let per_page = params
                .per_page
                .unwrap_or(DEFAULT_ADMIN_USER_PAGE_SIZE)
                .clamp(1, MAX_ADMIN_USER_PAGE_SIZE);

            let options = database::AdminUserQueryOptions {
                page,
                per_page,
                id: params.id,
                username: params.username,
                account_id: params.account_id,
                discord_id: params.discord_id,
                role: params.role,
                total_uploads: params.total_uploads,
                banned: params.banned,
                sort_by: params.sort_by.unwrap_or(database::UserListSortBy::Id),
                sort_dir: params.sort_dir.unwrap_or(database::SortDirection::Asc),
            };

            match db.get_admin_users_paginated(options).await {
                Ok(page_data) => {
                    let total_pages = if page_data.total == 0 {
                        0
                    } else {
                        (page_data.total + per_page as i64 - 1) / per_page as i64
                    };

                    util::response(
                        StatusCode::OK,
                        serde_json::json!({
                            "status": StatusCode::OK.as_u16(),
                            "users": page_data.users,
                            "page": page,
                            "per_page": per_page,
                            "total": page_data.total,
                            "total_pages": total_pages,
                        }),
                    )
                }
                Err(e) => util::str_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &format!("Failed to fetch users: {}", e),
                ),
            }
        }
        Err(resp) => resp,
    }
}

#[derive(Deserialize, Debug)]
pub struct UpdateUserPayload {
    pub username: Option<String>,
    pub account_id: Option<i64>,
    pub discord_id: Option<Option<DiscordIdRaw>>,
    pub role: Option<database::Role>,
}

#[derive(Deserialize, Debug)]
#[serde(untagged)]
pub enum DiscordIdRaw {
    Str(String),
    Num(i64),
}

pub async fn update_user(
    headers: HeaderMap,
    State(db): State<database::AppState>,
    Path(id): Path<i64>,
    Json(payload): Json<UpdateUserPayload>,
) -> Response {
    match mod_middleware(&headers, &db).await {
        Ok(current_user) => {
            match db.get_user_by_id(id).await {
                Some(target_user) => {
                    if !current_user.role.can_manage_user(target_user.role) {
                        return util::str_response(StatusCode::FORBIDDEN, "Insufficient privileges to modify this user");
                    }

                    if let Some(ref new_role) = payload.role {
                        if !current_user.role.can_assign_role(*new_role) {
                            return util::str_response(StatusCode::FORBIDDEN, "Insufficient privileges to assign the requested role");
                        }
                    }

                    let discord_db: Option<Option<i64>> = match payload.discord_id {
                        None => None,
                        Some(None) => Some(None),
                        Some(Some(DiscordIdRaw::Num(n))) => Some(Some(n)),
                        Some(Some(DiscordIdRaw::Str(s))) => match s.parse::<i64>() {
                            Ok(n) => Some(Some(n)),
                            Err(_) => return util::str_response(StatusCode::BAD_REQUEST, "Invalid discord_id format; must be numeric or stringified number"),
                        },
                    };

                    let options = database::UpdateUserOptions {
                        username: payload.username,
                        account_id: payload.account_id,
                        discord_id: discord_db,
                        role: payload.role,
                    };

                    match db.update_user(id, options).await {
                        Ok(_) => {
                            let query_opts = database::AdminUserQueryOptions {
                                page: 1,
                                per_page: 1,
                                id: Some(id),
                                username: None,
                                account_id: None,
                                discord_id: None,
                                role: None,
                                total_uploads: None,
                                banned: None,
                                sort_by: database::UserListSortBy::Id,
                                sort_dir: database::SortDirection::Asc,
                            };

                            match db.get_admin_users_paginated(query_opts).await {
                                Ok(page_data) => {
                                    util::response(
                                        StatusCode::OK,
                                        serde_json::json!({
                                            "status": StatusCode::OK.as_u16(),
                                            "data": page_data.users.into_iter().next(),
                                        }),
                                    )
                                }
                                Err(e) => util::str_response(
                                    StatusCode::INTERNAL_SERVER_ERROR,
                                    &format!("Failed to fetch updated user: {}", e),
                                ),
                            }
                        }
                        Err(e) => util::str_response(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            &format!("Failed to update user: {}", e),
                        ),
                    }
                }
                None => util::str_response(StatusCode::NOT_FOUND, "User not found"),
            }
        }
        Err(resp) => resp,
    }
}

pub async fn delete_thumbnail(
    headers: HeaderMap,
    State(db): State<database::AppState>,
    Path(id): Path<i64>,
) -> Response {
    match mod_middleware(&headers, &db).await {
        Ok(_) => {
            match db.delete_thumbnail_by_id(id).await {
                Ok(deleted) => {
                    if deleted {
                        thumbnail::delete_thumbnail(id).await;
                        thumbnail::purge_resize_cache(id).await;
                        cache_controller::purge(id);
                        util::str_response(StatusCode::OK, "Thumbnail deleted successfully")
                    } else {
                        util::str_response(StatusCode::NOT_FOUND, "Thumbnail not found")
                    }
                }
                Err(e) => util::str_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &format!("Failed to delete thumbnail: {}", e),
                ),
            }
        }
        Err(resp) => resp,
    }
}

#[derive(Deserialize, Debug)]
pub struct BanUserPayload {
    pub reason: String,
    pub expires_by: Option<chrono::DateTime<chrono::Utc>>,
}

pub async fn ban_user(
    headers: HeaderMap,
    State(db): State<database::AppState>,
    Path(id): Path<i64>,
    Json(payload): Json<BanUserPayload>,
) -> Response {
    match mod_middleware(&headers, &db).await {
        Ok(current_user) => {
            match db.get_user_by_id(id).await {
                Some(target_user) => {
                    if !current_user.role.can_manage_user(target_user.role) {
                        return util::str_response(StatusCode::FORBIDDEN, "Insufficient privileges to ban this user");
                    }

                    match db.ban_user(
                        id, payload.reason,
                        current_user.id,
                        payload.expires_by.map(|dt| dt.naive_utc())
                    ).await {
                        Ok(_) => util::str_response(StatusCode::OK, "User banned successfully"),
                        Err(e) => util::str_response(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            &format!("Failed to ban user: {}", e),
                        ),
                    }
                }
                None => util::str_response(StatusCode::NOT_FOUND, "User not found"),
            }
        }
        Err(resp) => resp,
    }
}

pub async fn unban_user(
    headers: HeaderMap,
    State(db): State<database::AppState>,
    Path(id): Path<i64>,
) -> Response {
    match mod_middleware(&headers, &db).await {
        Ok(current_user) => {
            match db.get_user_by_id(id).await {
                Some(target_user) => {
                    if !current_user.role.can_manage_user(target_user.role) {
                        return util::str_response(StatusCode::FORBIDDEN, "Insufficient privileges to unban this user");
                    }

                    match db.unban_user(id).await {
                        Ok(changed) => {
                            if changed {
                                util::str_response(StatusCode::OK, "User unbanned successfully")
                            } else {
                                util::str_response(StatusCode::NOT_FOUND, "No active ban found for this user")
                            }
                        }
                        Err(e) => util::str_response(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            &format!("Failed to unban user: {}", e),
                        ),
                    }
                }
                None => util::str_response(StatusCode::NOT_FOUND, "User not found"),
            }
        }
        Err(resp) => resp,
    }
}

