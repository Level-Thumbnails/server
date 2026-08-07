use crate::util::MessageResponse;
use crate::{db, util};
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::Response;
use image::ImageReader;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use axum::body::Bytes;
use dashmap::{Entry};
use futures::FutureExt;
use tracing::warn;
use webp::Encoder;
use crate::db::ResizeKey;

const CACHE_DIR: &str = "thumbnails/cache";

/// Thumbnail resolution options
#[derive(Deserialize, Serialize, Debug, Clone, Copy, utoipa::ToSchema, Eq, Hash, PartialEq)]
pub enum Res {
    /// High resolution thumbnail (1920x1080)
    #[serde(rename = "high")]
    High,
    /// Medium resolution thumbnail (1280x720)
    #[serde(rename = "medium")]
    Medium,
    /// Low resolution thumbnail (640x360)
    #[serde(rename = "small")]
    Small,
}

impl Res {
    fn dimensions(&self) -> (u32, u32) {
        match self {
            Res::High => (1920, 1080),
            Res::Medium => (1280, 720),
            Res::Small => (640, 360),
        }
    }
}

impl std::fmt::Display for Res {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Res::High => write!(f, "high"),
            Res::Medium => write!(f, "medium"),
            Res::Small => write!(f, "small"),
        }
    }
}

fn cache_path(id: u64, res: Res) -> PathBuf {
    PathBuf::from(format!("{}/{}_{}.webp", CACHE_DIR, id, res))
}

pub async fn delete_thumbnail(id: i64) {
    let image_path = PathBuf::from(format!("thumbnails/{}.webp", id));
    if let Err(e) = tokio::fs::remove_file(&image_path).await {
        if e.kind() != std::io::ErrorKind::NotFound {
            warn!("Failed to remove thumbnail {:?}: {}", image_path, e);
        }
    }
}

pub async fn purge_resize_cache(id: i64) {
    for res in [Res::Small, Res::Medium] {
        let path = cache_path(id as u64, res);
        if let Err(e) = tokio::fs::remove_file(&path).await {
            if e.kind() != std::io::ErrorKind::NotFound {
                warn!("Failed to remove resize cache {:?}: {}", path, e);
            }
        }
    }
}

fn image_response(image_data: impl Into<Bytes>, id: u64, upload_info: &db::UploadInfo) -> Response {
    let bytes: Bytes = image_data.into();

    Response::builder()
        .header(header::CONTENT_TYPE, "image/webp")
        .header(header::CONTENT_DISPOSITION, format!("inline; filename=\"{}.webp\"", id))
        .header(header::CACHE_CONTROL, "public, max-age=31536000, immutable")
        .header(header::CONTENT_LENGTH, bytes.len())
        .header("X-Level-ID", id.to_string())
        .header("X-Thumbnail-Author", &upload_info.username)
        .header("X-Thumbnail-User-ID", upload_info.account_id.to_string())
        .body(bytes.into())
        .unwrap()
}

async fn get_upload_info(db: &db::AppState, id: u64) -> Result<db::UploadInfo, Response> {
    match db.get_upload_info(id as i64).await {
        Some(upload) => Ok(upload),
        None => Err(util::str_response(StatusCode::NOT_FOUND, "Image not found")),
    }
}

async fn read_original_image(image_path: &PathBuf) -> Result<Vec<u8>, Response> {
    tokio::fs::read(image_path).await.map_err(|e| {
        util::str_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to read image file: {}", e),
        )
    })
}

async fn get_or_resize_image(
    image_path: PathBuf,
    key: ResizeKey,
    state: &db::AppState,
) -> Result<Bytes, Response> {
    let cache_file = cache_path(key.id, key.res);
    if let Ok(cached_data) = tokio::fs::read(&cache_file).await {
        return Ok(cached_data.into());
    }

    let shared_fut = match state.active_resizes.entry(key.clone()) {
        Entry::Occupied(entry) => entry.get().clone(),
        Entry::Vacant(entry) => {
            let semaphore = state.resize_semaphore.clone();
            let active_resizes = state.active_resizes.clone();
            let key_clone = key.clone();
            let cache_file_clone = cache_file.clone();

            let handle = tokio::spawn(async move {
                let _permit = semaphore.acquire_owned().await;
                let result = execute_resize(image_path, key_clone.res).await;

                if let Ok(ref bytes) = result {
                    let temp_path = cache_file_clone.with_extension("tmp");
                    if tokio::fs::write(&temp_path, bytes).await.is_ok() {
                        let _ = tokio::fs::rename(&temp_path, &cache_file_clone).await;
                    }
                }

                active_resizes.remove(&key_clone);
                result
            });

            let fut = async move {
                match handle.await {
                    Ok(res) => res,
                    Err(_) => Err("Task panicked or was aborted".to_string()),
                }
            }
                .boxed()
                .shared();

            entry.insert(fut.clone());
            fut
        }
    };

    match shared_fut.await {
        Ok(bytes) => Ok(bytes),
        Err(err_msg) => Err(util::str_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Image processing error: {}", err_msg),
        )),
    }
}

async fn execute_resize(image_path: PathBuf, res: Res) -> db::ResizeResult {
    let (width, height) = res.dimensions();

    tokio::task::spawn_blocking(move || -> db::ResizeResult {
        let image = ImageReader::open(&image_path)
            .map_err(|e| format!("Failed to open image: {}", e))?
            .decode()
            .map_err(|e| format!("Failed to decode image: {}", e))?;

        let resized_image =
            image.resize_exact(width, height, image::imageops::FilterType::Lanczos3).to_rgb8();

        Ok(Encoder::from_rgb(&resized_image, width, height).encode_lossless().to_vec().into())
    })
    .await
    .map_err(|e| format!("Task join error: {}", e))?
}

async fn handle_image(id: u64, res: Res, db: db::AppState) -> Response {
    // Check if image file exists
    let image_path = PathBuf::from(format!("thumbnails/{}.webp", id));
    if !tokio::fs::try_exists(&image_path).await.unwrap_or_default() {
        return util::str_response(StatusCode::NOT_FOUND, "Image not found");
    }

    // Verify image exists in database and get metadata
    let upload_info = match get_upload_info(&db, id).await {
        Ok(info) => info,
        Err(response) => return response,
    };

    match res {
        Res::High => {
            // For high resolution, serve the original image
            let image_data = match read_original_image(&image_path).await {
                Ok(data) => data,
                Err(response) => return response,
            };

            image_response(image_data, id, &upload_info)
        }

        Res::Medium | Res::Small => {
            // For lower resolutions, resize the image
            let key = ResizeKey { id, res };
            match get_or_resize_image(image_path, key, &db).await {
                Ok(resized_data) => image_response(resized_data, id, &upload_info),
                Err(response) => response,
            }
        }
    }
}

#[utoipa::path(
    get,
    path = "/thumbnail/{id}/{res}",
    description = "Returns the thumbnail image for the specified Geometry Dash level ID with requested resolution.",
    tag = "Thumbnails",
    params(
        ("id" = u64, Path, description = "Geometry Dash level ID"),
        ("res" = Res, Path, description = "Thumbnail resolution: high, medium, or small"),
    ),
    responses(
        (
            status = 200,
            description = "Thumbnail image in WebP format",
            content_type = "image/webp",
            body = Vec<u8>
        ),
        (
            status = 404,
            description = "Image not found",
            body = MessageResponse,
            example = json!({"status": 404, "message": "Image not found"})
        ),
        (
            status = 500,
            description = "Internal server error",
            body = MessageResponse,
            example = json!({"status": 500, "message": "Failed to read image file: <error details>"})
        )
    )
)]
pub async fn image_handler_with_res(
    Path((id, res)): Path<(u64, Res)>,
    State(db): State<db::AppState>,
) -> Response {
    handle_image(id, res, db).await
}

#[utoipa::path(
    get,
    path = "/thumbnail/{id}",
    description = "Returns the thumbnail image for the specified Geometry Dash level ID in high resolution (1920x1080).",
    tag = "Thumbnails",
    params(
        ("id" = u64, Path, description = "Geometry Dash level ID")
    ),
    responses(
        (
            status = 200,
            description = "Thumbnail image in WebP format",
            content_type = "image/webp",
            body = Vec<u8>
        ),
        (
            status = 404,
            description = "Image not found",
            body = MessageResponse,
            example = json!({"status": 404, "message": "Image not found"})
        ),
        (
            status = 500,
            description = "Internal server error",
            body = MessageResponse,
            example = json!({"status": 500, "message": "Failed to read image file: <error details>"})
        )
    )
)]
pub async fn image_handler_default(
    Path(id): Path<u64>,
    State(db): State<db::AppState>,
) -> Response {
    handle_image(id, Res::High, db).await
}

#[utoipa::path(
    get,
    path = "/thumbnail/{id}/info",
    description = "Returns metadata about the thumbnail image for the specified Geometry Dash level ID",
    tag = "Thumbnails",
    params(
        ("id" = u64, Path, description = "Geometry Dash level ID")
    ),
    responses(
        (
            status = 200,
            description = "Thumbnail metadata in JSON format",
            body = db::UploadExtended,
            example = json!({
                "level_id": 1,
                "account_id": 9598348,
                "username": "Aardvark04",
                "upload_time": "2026-03-11T22:00:18.225970",
                "first_upload_time": "2024-03-24T22:59:29",
                "accepted_time": "2026-03-11T22:00:18.225970",
                "accepted_by": 9598348,
                "accepted_by_username": "Aardvark04"
            }),
        ),
        (
            status = 404,
            description = "Image not found",
            body = MessageResponse,
            example = json!({"status": 404, "message": "Image not found"})
        )
    )
)]
pub async fn thumbnail_info_handler(
    Path(id): Path<u64>,
    State(db): State<db::AppState>,
) -> Response {
    match db.get_upload_extended(id as i64).await {
        Some(upload) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::CACHE_CONTROL, "no-store")
            .body(serde_json::to_string(&upload).unwrap().into())
            .unwrap(),
        None => util::str_response(StatusCode::NOT_FOUND, "Image not found"),
    }
}

pub async fn handle_random(res: Res, db: db::AppState) -> Response {
    match db.random_active_thumbnail().await {
        Some(random_id) => {
            let url = format!("/thumbnail/{}/{}", random_id, res.to_string());
            Response::builder()
                .status(StatusCode::FOUND)
                .header(header::LOCATION, url)
                .body("".into())
                .unwrap()
        }
        None => util::str_response(StatusCode::NOT_FOUND, "No images found"),
    }
}

#[utoipa::path(
    get,
    path = "/thumbnail/random",
    description = "Redirects to a random thumbnail image in high resolution (1920x1080).",
    tag = "Thumbnails",
    responses(
        (
            status = 302,
            description = "Redirect to a random thumbnail image",
            headers(
                ("Location" = String, description = "URL of the random thumbnail image")
            )
        ),
        (
            status = 404,
            description = "No images found",
            body = MessageResponse,
            example = json!({"status": 404, "message": "No images found"})
        ),
        (
            status = 500,
            description = "Internal server error",
            body = MessageResponse,
            example = json!({"status": 500, "message": "Failed to get thumbnails: <error details>"})
        )
    )
)]
pub async fn random_handler(State(db): State<db::AppState>) -> Response {
    handle_random(Res::High, db).await
}

#[utoipa::path(
    get,
    path = "/thumbnail/random/{res}",
    description = "Redirects to a random thumbnail image in the specified resolution.",
    tag = "Thumbnails",
    params(
        ("res" = Res, Path, description = "Thumbnail resolution: high, medium, or small"),
    ),
    responses(
        (
            status = 302,
            description = "Redirect to a random thumbnail image",
            headers(
                ("Location" = String, description = "URL of the random thumbnail image")
            )
        ),
        (
            status = 404,
            description = "No images found",
            body = MessageResponse,
            example = json!({"status": 404, "message": "No images found"})
        ),
        (
            status = 500,
            description = "Internal server error",
            body = MessageResponse,
            example = json!({"status": 500, "message": "Failed to get thumbnails: <error details>"})
        )
    )
)]
pub async fn random_res_handler(Path(res): Path<Res>, State(db): State<db::AppState>) -> Response {
    handle_random(res, db).await
}
