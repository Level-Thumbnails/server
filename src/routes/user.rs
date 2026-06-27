use crate::{database, util};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use serde::{Deserialize, Serialize};
use std::future::Future;

trait MyUploadsPageLike<T> {
    fn uploads(self) -> Vec<T>;
    fn total(&self) -> i64;
}

impl MyUploadsPageLike<database::ActiveUpload> for database::ActiveUploadsPage {
    fn uploads(self) -> Vec<database::ActiveUpload> { self.uploads }
    fn total(&self) -> i64 { self.total }
}

impl MyUploadsPageLike<database::PendingUpload> for database::PendingUploadsPage {
    fn uploads(self) -> Vec<database::PendingUpload> { self.uploads }
    fn total(&self) -> i64 { self.total }
}

impl MyUploadsPageLike<database::RejectedUpload> for database::RejectedUploadsPage {
    fn uploads(self) -> Vec<database::RejectedUpload> { self.uploads }
    fn total(&self) -> i64 { self.total }
}

pub async fn get_user_info(id: i64, db: &database::AppState) -> Response {
    match db.get_user_stats(id).await {
        Some(user) => util::response(
            StatusCode::OK,
            serde_json::json!({
                "status": StatusCode::OK.as_u16(),
                "data": user,
            }),
        ),
        None => util::str_response(StatusCode::NOT_FOUND, "User not found"),
    }
}

pub async fn get_me(headers: HeaderMap, State(db): State<database::AppState>) -> Response {
    match util::auth_middleware(&headers, &db).await {
        Ok(user) => get_user_info(user.id, &db).await,
        Err(response) => response,
    }
}

pub async fn get_user_by_id(Path(id): Path<i64>, State(db): State<database::AppState>) -> Response {
    get_user_info(id, &db).await
}

pub async fn get_user_by_gd_id(Path(id): Path<i64>, State(db): State<database::AppState>) -> Response {
    if id <= 0 {
        return util::str_response(StatusCode::BAD_REQUEST, "Invalid GD user ID");
    }

    {
        let set = db.registered_users.read().await;
        if !set.contains(&id) {
            return util::str_response(StatusCode::NOT_FOUND, "User not found");
        }
    }

    match db.get_user_by_gd_id(id).await {
        Some(user) => util::response(
            StatusCode::OK,
            serde_json::json!({
                "status": StatusCode::OK.as_u16(),
                "data": user,
            }),
        ),
        None => util::str_response(StatusCode::NOT_FOUND, "User not found"),
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct HistoryQueryParams {
    pub months: Option<i64>,
}

async fn get_history_response(
    user_id: i64,
    months: i64,
    db: &database::AppState,
) -> Response {
    match db.get_user_history(user_id, months).await {
        Ok(history) => util::response(
            StatusCode::OK,
            serde_json::json!({
                "status": StatusCode::OK.as_u16(),
                "data": history,
            }),
        ),
        Err(e) => util::str_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to fetch user history: {}", e),
        ),
    }
}

pub async fn get_me_history(
    headers: HeaderMap,
    Query(params): Query<HistoryQueryParams>,
    State(db): State<database::AppState>,
) -> Response {
    match util::auth_middleware(&headers, &db).await {
        Ok(user) => get_history_response(user.id, params.months.unwrap_or(12), &db).await,
        Err(response) => response,
    }
}

pub async fn get_user_history(
    headers: HeaderMap,
    Path(id): Path<i64>,
    Query(params): Query<HistoryQueryParams>,
    State(db): State<database::AppState>,
) -> Response {
    match util::auth_middleware(&headers, &db).await {
        Ok(user) => {
            if user.id == id || user.role.can_view_other_user_history() {
                get_history_response(id, params.months.unwrap_or(12), &db).await
            } else {
                util::str_response(StatusCode::FORBIDDEN, "Insufficient permissions")
            }
        }
        Err(response) => response,
    }
}

const DEFAULT_MY_UPLOADS_PAGE_SIZE: u32 = 12;
const MAX_MY_UPLOADS_PAGE_SIZE: u32 = 100;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct MyUploadsQueryParams {
    pub page: u32,
    pub per_page: u32,
    pub level_id_search: Option<String>,
}

impl Default for MyUploadsQueryParams {
    fn default() -> Self {
        Self {
            page: 1,
            per_page: DEFAULT_MY_UPLOADS_PAGE_SIZE,
            level_id_search: None,
        }
    }
}

impl MyUploadsQueryParams {
    fn sanitized(mut self) -> Self {
        self.page = self.page.max(1);
        self.per_page = self.per_page.max(1).min(MAX_MY_UPLOADS_PAGE_SIZE);

        self.level_id_search = self.level_id_search.as_ref().and_then(|s| {
            if s.trim().is_empty() { None } else { Some(s.trim().to_string()) }
        });

        self
    }
}

pub async fn get_my_active_uploads(
    headers: HeaderMap,
    Query(params): Query<MyUploadsQueryParams>,
    State(db): State<database::AppState>,
) -> Response {
    get_my_uploads_page_response(
        headers,
        Query(params),
        State(db),
        "Failed to fetch active uploads",
        |user_id, params, db| async move {
            db.get_user_active_uploads_paginated(
                user_id,
                params.page,
                params.per_page,
                params.level_id_search,
                None,
            ).await
        },
    ).await
}

#[derive(Debug, Serialize)]
pub struct MyUploadsSummaryResponse {
    pub active: i64,
    pub pending: i64,
    pub rejected: i64,
}

pub async fn get_my_upload_summary(
    headers: HeaderMap,
    Query(params): Query<MyUploadsQueryParams>,
    State(db): State<database::AppState>,
) -> Response {
    let user = match util::auth_middleware(&headers, &db).await {
        Ok(user) => user,
        Err(response) => return response,
    };

    let params = params.sanitized();

    match db.get_my_upload_summary(user.id, params.level_id_search.clone()).await {
        Ok(summary) => util::response(
            StatusCode::OK,
            serde_json::json!({
                "status": StatusCode::OK.as_u16(),
                "data": MyUploadsSummaryResponse {
                    active: summary.active,
                    pending: summary.pending,
                    rejected: summary.rejected,
                },
            }),
        ),
        Err(e) => util::str_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to fetch upload summary: {}", e),
        ),
    }
}

pub async fn get_my_pending_uploads(
    headers: HeaderMap,
    Query(params): Query<MyUploadsQueryParams>,
    State(db): State<database::AppState>,
) -> Response {
    get_my_uploads_page_response(
        headers,
        Query(params),
        State(db),
        "Failed to fetch pending uploads",
        |user_id, params, db| async move {
            db.get_pending_uploads_paginated(database::PendingQueryOptions {
                page: params.page,
                per_page: params.per_page,
                level_id: params.level_id_search.and_then(|s| s.parse::<i64>().ok()),
                user_id: Some(user_id),
                username: None,
                replacement_only: false,
                new_only: false,
            }).await
        },
    ).await
}

pub async fn get_my_rejected_uploads(
    headers: HeaderMap,
    Query(params): Query<MyUploadsQueryParams>,
    State(db): State<database::AppState>,
) -> Response {
    get_my_uploads_page_response(
        headers,
        Query(params),
        State(db),
        "Failed to fetch rejected uploads",
        |user_id, params, db| async move {
            db.get_user_rejected_uploads_paginated(
                user_id,
                params.page,
                params.per_page,
                params.level_id_search,
                None,
            )
            .await
        },
    ).await
}

async fn get_my_uploads_page_response<T, P, F, Fut>(
    headers: HeaderMap,
    Query(params): Query<MyUploadsQueryParams>,
    State(db): State<database::AppState>,
    error_label: &'static str,
    fetcher: F,
) -> Response
where
    T: Serialize,
    P: MyUploadsPageLike<T>,
    F: FnOnce(i64, MyUploadsQueryParams, database::AppState) -> Fut,
    Fut: Future<Output = Result<P, sqlx::Error>>,
{
    let user = match util::auth_middleware(&headers, &db).await {
        Ok(user) => user,
        Err(response) => return response,
    };

    let params = params.sanitized();
    let page = match fetcher(user.id, params.clone(), db.clone()).await {
        Ok(page) => page,
        Err(e) => {
            return util::str_response(StatusCode::INTERNAL_SERVER_ERROR, &format!("{}: {}", error_label, e));
        }
    };

    let total_pages = if page.total() == 0 {
        0
    } else {
        (page.total() + params.per_page as i64 - 1) / params.per_page as i64
    };
    let total = page.total();
    let uploads = page.uploads();

    util::response(
        StatusCode::OK,
        serde_json::json!({
            "status": StatusCode::OK.as_u16(),
            "uploads": uploads,
            "page": params.page,
            "per_page": params.per_page,
            "total": total,
            "total_pages": total_pages,
        }),
    )
}

