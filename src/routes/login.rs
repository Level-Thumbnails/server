use crate::{auth, db, util};
use auth::UserSession;
use axum::Json;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::env;
use tracing::{error, warn};

#[derive(Deserialize, Debug)]
pub struct LoginPayload {
    account_id: i64,
    user_id: i64,
    username: String,
    argon_token: String,
    // discord_token: Option<String>,
}

fn handle_verdict_error(verdict: auth::Verdict) -> Response {
    let details = match verdict {
        auth::Verdict::Invalid(cause) => format!("Invalid token: {}", cause),
        auth::Verdict::Weak(username) => format!("Weak token for user: {}", username),
        _ => "Authentication failed".to_string(),
    };

    util::response(
        StatusCode::UNAUTHORIZED,
        json!({
            "status": StatusCode::UNAUTHORIZED.as_u16(),
            "error": "Authentication failed",
            "details": details,
        }),
    )
}

pub async fn login(State(db): State<db::AppState>, Json(payload): Json<LoginPayload>) -> Response {
    // Validate argon token
    let verdict = match auth::ArgonClient::get()
        .verify(payload.account_id, payload.user_id, &payload.username, &payload.argon_token)
        .await
    {
        Ok(verdict) => verdict,
        Err(e) => {
            error!("Argon verification failed for user {}: {}", payload.username, e);
            return util::response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({
                    "status": StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
                    "error": "Argon verification failed",
                    "details": e.to_string(),
                }),
            );
        }
    };

    // Find or create entry in the database
    match verdict {
        auth::Verdict::Strong => {
            match db.find_or_create_user(payload.account_id, &payload.username).await {
                Ok(user) => util::response(
                    StatusCode::OK,
                    json!({
                        "status": StatusCode::OK.as_u16(),
                        "message": "User authenticated successfully",
                        "user": user,
                        "token": UserSession::new(user.id, payload.username).to_jwt(),
                    }),
                ),
                Err(e) => {
                    error!("Database error during login for user {}: {}", payload.username, e);
                    util::response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        json!({
                            "status": StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
                            "error": "Database error",
                            "details": e.to_string(),
                        }),
                    )
                }
            }
        }
        verdict => {
            warn!("Failed login attempt for user {}", payload.username);
            handle_verdict_error(verdict)
        }
    }
}

#[derive(Deserialize, Debug)]
pub struct DiscordOAuthPayload {
    code: String,
}

pub async fn discord_oauth_handler(
    Query(query): Query<DiscordOAuthPayload>,
    State(db): State<db::AppState>,
) -> Response {
    if query.code.is_empty() {
        return util::str_response(StatusCode::BAD_REQUEST, "Missing code parameter");
    }

    let client = reqwest::Client::new();

    // Use the code to fetch user info from Discord
    let res = match client
        .post("https://discord.com/api/oauth2/token")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .form(&[
            ("client_id", env::var("DISCORD_CLIENT_ID").expect("DISCORD_CLIENT_ID must be set")),
            (
                "client_secret",
                env::var("DISCORD_CLIENT_SECRET").expect("DISCORD_CLIENT_SECRET must be set"),
            ),
            ("code", query.code),
            ("grant_type", "authorization_code".to_string()),
            (
                "redirect_uri",
                format!("{}/auth/discord", env::var("HOME_URL").expect("HOME_URL must be set")),
            ),
        ])
        .send()
        .await
    {
        Ok(response) => match response.json::<Value>().await {
            Ok(json) => json,
            Err(e) => {
                error!("Failed to parse Discord token response: {}", e);
                return util::str_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Failed to parse Discord response",
                );
            }
        },
        Err(e) => {
            error!("Failed to fetch Discord token: {}", e);
            return util::str_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to fetch Discord token",
            );
        }
    };

    if !res.get("access_token").is_some() {
        return util::str_response(StatusCode::UNAUTHORIZED, "Invalid Discord code");
    }

    let access_token = res["access_token"].as_str().unwrap();
    let user_info = match client
        .get("https://discord.com/api/users/@me")
        .bearer_auth(access_token)
        .send()
        .await
    {
        Ok(response) => match response.json::<Value>().await {
            Ok(json) => json,
            Err(e) => {
                error!("Failed to parse Discord user info: {}", e);
                return util::str_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Failed to parse Discord user info",
                );
            }
        },
        Err(e) => {
            error!("Failed to fetch Discord user info: {}", e);
            return util::str_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to fetch Discord user info",
            );
        }
    };

    let discord_id: i64 = match user_info["id"].as_str() {
        Some(id) => match id.parse::<i64>() {
            Ok(id) => id,
            Err(_) => {
                return util::str_response(StatusCode::UNAUTHORIZED, "Invalid Discord user ID");
            }
        },
        None => {
            return util::str_response(StatusCode::UNAUTHORIZED, "Discord user ID not found");
        }
    };

    let username = user_info["username"].as_str().unwrap_or("");
    match db.find_or_create_user_discord(discord_id, username).await {
        Ok(user) => {
            let token = UserSession::new(user.id, user.username.clone()).to_jwt();
            Response::builder()
                .status(StatusCode::FOUND)
                .header("Set-Cookie", format!("auth_token={}; HttpOnly; Path=/; SameSite=Lax; Expires=Fri, 31 Dec 9999 23:59:59 GMT", token))
                .header("Set-Cookie", format!("auth_role={}; Path=/; SameSite=Lax; Expires=Fri, 31 Dec 9999 23:59:59 GMT", user.role.to_string()))
                .header("Location", "/dashboard")
                .body("Redirecting to dashboard...".into())
                .unwrap()
        }
        Err(e) => {
            error!("Database error during Discord OAuth for user {}: {}", username, e);
            util::str_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string())
        }
    }
}

pub async fn get_session(headers: HeaderMap, State(db): State<db::AppState>) -> Response {
    match util::auth_middleware(&headers, &db).await {
        Ok(user) => util::response(
            StatusCode::OK,
            json!({
                "status": StatusCode::OK.as_u16(),
                "user": user,
            }),
        ),
        Err(response) => response,
    }
}

#[derive(Deserialize, Serialize, Debug)]
struct LinkToken {
    id: i64,
    exp: u64,
}

pub async fn get_link_token(headers: HeaderMap, State(db): State<db::AppState>) -> Response {
    match util::auth_middleware(&headers, &db).await {
        Ok(user) => {
            if user.account_id != -1 {
                return util::str_response(
                    StatusCode::BAD_REQUEST,
                    "You already have a Geometry Dash account linked",
                );
            }

            let link_token = LinkToken {
                id: user.id,
                exp: (chrono::Utc::now() + chrono::Duration::minutes(10)).timestamp() as u64,
            };

            let jwt_secret = dotenv::var("JWT_SECRET").expect("JWT_SECRET must be set");
            let token = jsonwebtoken::encode(
                &jsonwebtoken::Header::default(),
                &link_token,
                &jsonwebtoken::EncodingKey::from_secret(jwt_secret.as_bytes()),
            )
            .expect("Failed to encode JWT");

            util::response(
                StatusCode::OK,
                json!({
                    "status": StatusCode::OK.as_u16(),
                    "message": "Link token generated successfully",
                    "token": token,
                }),
            )
        }
        Err(response) => response,
    }
}

#[derive(Deserialize, Debug)]
pub struct LinkPayload {
    token: String,
}

async fn migrate_account(
    db: &db::AppState,
    user_id: i64,    // Geometry Dash user ID
    discord_id: i64, // Discord user ID
) -> Response {
    let pending = db.get_pending_uploads_for_user(user_id).await;

    match db.migrate_user_account(user_id, discord_id).await {
        Ok(user) => {
            match pending {
                Ok(uploads) => {
                    for upload in uploads {
                        tokio::fs::rename(
                            format!("uploads/{}_{}.webp", user_id, upload.level_id),
                            format!("uploads/{}_{}.webp", discord_id, upload.level_id),
                        )
                        .await
                        .unwrap_or(());
                    }
                }
                Err(_) => {}
            }

            util::response(
                StatusCode::OK,
                json!({
                    "status": StatusCode::OK.as_u16(),
                    "message": "Account linked successfully",
                    "user": user,
                    "token": UserSession::new(user.id, user.username.clone()).to_jwt(),
                }),
            )
        }
        Err(e) => {
            error!(
                "Account migration failed for user_id={}, discord_id={}: {}",
                user_id, discord_id, e
            );
            util::str_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string())
        }
    }
}

pub async fn link_account(
    headers: HeaderMap,
    State(db): State<db::AppState>,
    Json(payload): Json<LinkPayload>,
) -> Response {
    match util::auth_middleware(&headers, &db).await {
        Ok(user) => {
            if user.discord_id.is_some() {
                return util::str_response(
                    StatusCode::BAD_REQUEST,
                    "You already have a Discord account linked",
                );
            }

            let jwt_secret = dotenv::var("JWT_SECRET").expect("JWT_SECRET must be set");
            let validation = jsonwebtoken::Validation::default();
            match jsonwebtoken::decode::<LinkToken>(
                &payload.token,
                &jsonwebtoken::DecodingKey::from_secret(jwt_secret.as_bytes()),
                &validation,
            ) {
                Ok(decoded) => {
                    migrate_account(
                        &db,
                        user.id,           // Geometry Dash user ID
                        decoded.claims.id, // Discord user ID
                    )
                    .await
                }
                Err(_) => util::str_response(StatusCode::UNAUTHORIZED, "Invalid link token"),
            }
        }
        Err(response) => response,
    }
}

pub async fn logout() -> Response {
    Response::builder()
        .status(StatusCode::FOUND)
        .header(
            "Set-Cookie",
            "auth_token=; HttpOnly; Path=/; SameSite=Lax; Expires=Thu, 01 Jan 1970 00:00:00 GMT",
        )
        .header(
            "Set-Cookie",
            "auth_role=; Path=/; SameSite=Lax; Expires=Thu, 01 Jan 1970 00:00:00 GMT",
        )
        .header("Location", "/")
        .body("Redirecting to homepage...".into())
        .unwrap()
}
