use crate::{database, util};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use serde::Deserialize;

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

