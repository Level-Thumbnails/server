use axum::extract::DefaultBodyLimit;
use axum::{Router, routing::delete, routing::get, routing::patch, routing::post};
use std::path::Path;
use tokio::net::TcpListener;
use tokio::time::{Duration, sleep};
use tower_http::cors;
use tower_http::services::{ServeDir, ServeFile};
use tracing::{info, warn};
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::filter::EnvFilter;
use utoipa::openapi::security::{ApiKey, ApiKeyValue, HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi};
use utoipa_swagger_ui::SwaggerUi;

mod auth;
mod cache_controller;
pub mod db;
mod routes;
mod util;

use routes::{admin, login, stats, thumbnail, upload, user};

struct SecurityAddon;

impl Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.as_mut().unwrap();

        components.add_security_scheme(
            "bearerAuth",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .bearer_format("JWT")
                    .description(Some("User token passed via Authorization header."))
                    .build(),
            ),
        );

        components.add_security_scheme(
            "cookieAuth",
            SecurityScheme::ApiKey(ApiKey::Cookie(ApiKeyValue::with_description(
                "auth_token",
                "User token passed via auth_token cookie.",
            ))),
        );
    }
}

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Level Thumbnails API",
        version = "1.0.0",
        description = "API / Dashboard for Level Thumbnails, a Geometry Dash mod that adds image previews to levels in-game.",
        license(name = "Apache-2.0", url = "https://www.apache.org/licenses/LICENSE-2.0.html")
    ),
    paths(
        stats::get_stats,
        stats::get_stats_history,
        thumbnail::image_handler_default,
        thumbnail::image_handler_with_res,
        thumbnail::thumbnail_info_handler,
        thumbnail::random_handler,
        thumbnail::random_res_handler,
        upload::get_all_level_locks,
        upload::get_level_lock,
        upload::lock_level,
        upload::unlock_level,
        admin::delete_thumbnail
    ),
    components(
        schemas(
            util::MessageResponse,

            // stats
            stats::ServerStats,
            stats::StatsResponse,
            db::StatsSnapshot,
            stats::StatsHistoryResponse,

            // thumbnails
            thumbnail::Res,
            db::UploadExtended,

            // upload
            db::LevelLock,
            upload::AllLevelLocksResponse,
            upload::LockLevelPayload,
            upload::LevelLockResponse
        )
    ),
    modifiers(&SecurityAddon),
    tags(
        (name = "Server Status", description = "Fetching overall server statistics."),
        (name = "Thumbnails", description = "Fetching thumbnail images and metadata."),
        (name = "Level Locking", description = "Locking and unlocking levels for thumbnail uploads."),
        (name = "Admin", description = "Miscellaneous moderation endpoints.")
    )
)]
struct ApiDoc;

#[tokio::main]
async fn main() {
    // parse .env file
    dotenv::dotenv().ok();

    // setup logging
    let log_level = std::env::var("RUST_LOG").unwrap_or_else(|_| "warn".to_string());
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(log_level));

    let file_appender = RollingFileAppender::new(Rotation::DAILY, "logs", "server.log");
    let (non_blocking_logger, _guard) = tracing_appender::non_blocking(file_appender);

    tracing_subscriber::fmt()
        .with_writer(non_blocking_logger)
        .with_ansi(false)
        .with_env_filter(filter)
        .init();

    // setup directories
    tokio::fs::create_dir_all("thumbnails").await.unwrap();
    tokio::fs::create_dir_all("thumbnails/cache").await.unwrap();
    tokio::fs::create_dir_all("uploads").await.unwrap();

    let cors = cors::CorsLayer::new()
        .allow_origin(cors::Any)
        .allow_methods(cors::Any)
        .allow_headers(cors::Any);

    let db = db::get_db().await;

    let snapshot_db = db.clone();
    tokio::spawn(async move {
        stats_snapshot_loop(snapshot_db).await;
    });

    let app = Router::new()
        .route("/stats", get(stats::get_stats))
        .route("/stats/history", get(stats::get_stats_history))
        // /thumbnail
        .route("/thumbnail/locks", get(upload::get_all_level_locks))
        .route("/thumbnail/{id}/lock", get(upload::get_level_lock))
        .route("/thumbnail/{id}/lock", post(upload::lock_level))
        .route("/thumbnail/{id}/lock", delete(upload::unlock_level))
        .route("/thumbnail/{id}", get(thumbnail::image_handler_default))
        .route("/thumbnail/{id}/{res}", get(thumbnail::image_handler_with_res))
        .route("/thumbnail/{id}/info", get(thumbnail::thumbnail_info_handler))
        .route("/thumbnail/random", get(thumbnail::random_handler))
        .route("/thumbnail/random/{res}", get(thumbnail::random_res_handler))
        // /auth
        .route("/auth/login", post(login::login))
        .route("/auth/discord", get(login::discord_oauth_handler))
        .route("/auth/session", get(login::get_session))
        .route("/auth/link", get(login::get_link_token))
        .route("/auth/link", post(login::link_account))
        .route("/auth/logout", get(login::logout))
        // /user
        .route("/user/me", get(user::get_me))
        .route("/user/me/history", get(user::get_me_history))
        .route("/user/me/uploads/summary", get(user::get_my_upload_summary))
        .route("/user/me/uploads/active", get(user::get_my_active_uploads))
        .route("/user/me/uploads/pending", get(user::get_my_pending_uploads))
        .route("/user/me/uploads/rejected", get(user::get_my_rejected_uploads))
        // .route("/user/me", delete(user::delete_me))
        .route("/user/{id}", get(user::get_user_by_id))
        .route("/user/gd/{id}", get(user::get_user_by_gd_id))
        .route("/user/badges", get(user::get_user_badges))
        .route("/user/{id}/history", get(user::get_user_history))
        // .route("/user/{id}/uploads", get(routes::user::get_user_uploads))
        // /upload
        .route("/upload/{id}", post(upload::upload))
        // /pending
        .route("/pending/{id}/image", get(upload::get_pending_image))
        .route("/pending", get(upload::get_all_pending_uploads))
        .route("/pending/{id}", get(upload::get_pending_info))
        .route("/pending/{id}", post(upload::pending_action))
        .route("/pending/level/{id}", get(upload::get_pending_uploads_for_level))
        .route("/pending/user/{id}", get(upload::get_pending_uploads_for_user))
        // /admin
        .route("/admin/settings", get(admin::get_settings))
        .route("/admin/settings", post(admin::update_settings))
        .route("/admin/users", get(admin::get_users))
        .route("/admin/user/{id}", patch(admin::update_user))
        .route("/admin/ban/{id}", post(admin::ban_user))
        .route("/admin/ban/{id}", delete(admin::unban_user))
        // .route("/admin/user/{id}", get(admin::get_user_by_id))
        .route("/admin/thumbnail/{id}", delete(admin::delete_thumbnail))
        .merge(SwaggerUi::new("/swagger").url("/api-docs/openapi.json", ApiDoc::openapi()))
        .with_state(db)
        .layer(cors)
        .layer(DefaultBodyLimit::disable())
        .fallback_service(ServeDir::new("dist").fallback(ServeFile::new("dist/index.html")));

    let bind_address = dotenv::var("BIND_ADDRESS").unwrap_or_else(|_| "0.0.0.0:3000".to_string());
    let listener = TcpListener::bind(bind_address).await.unwrap();

    info!("Started server!");
    axum::serve(listener, app).await.unwrap();
}

async fn get_dir_stats(path: &Path) -> Result<(u64, usize), std::io::Error> {
    let mut entries = tokio::fs::read_dir(path).await?;
    let mut total_size = 0;
    let mut file_count = 0;

    while let Some(entry) = entries.next_entry().await? {
        let metadata = entry.metadata().await?;
        if metadata.is_file() {
            file_count += 1;
            total_size += metadata.len();
        }
    }

    Ok((total_size, file_count))
}

async fn get_dir_total_size_recursive(path: &Path) -> Result<u64, std::io::Error> {
    let mut total_size = 0;
    let mut dirs = vec![path.to_path_buf()];

    while let Some(dir) = dirs.pop() {
        let mut entries = tokio::fs::read_dir(&dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let metadata = entry.metadata().await?;
            if metadata.is_file() {
                total_size += metadata.len();
            } else if metadata.is_dir() {
                dirs.push(entry.path());
            }
        }
    }

    Ok(total_size)
}

async fn stats_snapshot_loop(db: db::AppState) {
    let interval_minutes = dotenv::var("STATS_SNAPSHOT_INTERVAL_MINUTES")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(60);

    let interval = Duration::from_secs(interval_minutes * 60);

    loop {
        if let Err(e) = create_stats_snapshot(&db).await {
            warn!("Failed to create stats snapshot: {}", e);
        }

        sleep(interval).await;
    }
}

async fn create_stats_snapshot(db: &db::AppState) -> Result<(), String> {
    let storage_size = get_dir_total_size_recursive(Path::new("uploads"))
        .await
        .map_err(|e| format!("Failed to collect upload storage stats: {}", e))?;

    let (_, thumbnails_count) = get_dir_stats(Path::new("thumbnails"))
        .await
        .map_err(|e| format!("Failed to collect thumbnail count stats: {}", e))?;

    let users_per_month = cache_controller::get_user_stats().await.ok().map(|value| value as i64);

    db.create_stats_snapshot(storage_size as i64, thumbnails_count as i64, users_per_month)
        .await
        .map_err(|e| format!("Failed to write stats snapshot: {}", e))
}
