use crate::auth::UserSession;
use crate::database;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::Response;
use serde::{Deserialize, Serialize};
use serde_json::json;

const MAX_SUBMISSION_NOTE_LENGTH: usize = 500;

pub fn response(status: StatusCode, body: serde_json::Value) -> Response {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CACHE_CONTROL, "no-store")
        .body(body.to_string().into())
        .unwrap()
}

/// A simple response type for returning a status code and message in the response body. Useful for error responses.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct MessageResponse {
    /// HTTP status code of the response
    pub status: u16,
    /// A human-readable message describing the result of the request
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedSubmissionNote {
    pub level_name: String,
    pub creator_id: i64,
    pub creator_name: String,
    pub downloads: i64,
    pub likes: i64,
    pub stars: i64,
    pub length: i8,
    pub rating: i8,
    pub difficulty: i8,
    pub percentage: f32,
    pub attempt_time: f64,
    pub message: Option<String>,
}

fn decode_submission_value(value: &str) -> Result<String, String> {
    urlencoding::decode(value)
        .map(|value| value.into_owned())
        .map_err(|e| format!("Invalid URL-encoded string: {}", e))
}

pub fn parse_submission_note(raw: &str) -> Result<ParsedSubmissionNote, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("Submission note cannot be empty".to_string());
    }

    if trimmed.chars().count() > MAX_SUBMISSION_NOTE_LENGTH {
        return Err(format!("Submission note is too long (max {} characters)", MAX_SUBMISSION_NOTE_LENGTH));
    }

    let mut level_name = None;
    let mut creator_id = None;
    let mut creator_name = None;
    let mut downloads = None;
    let mut likes = None;
    let mut stars = None;
    let mut length = None;
    let mut rating = None;
    let mut difficulty = None;
    let mut percentage = None;
    let mut attempt_time = None;
    let mut message = None;

    for part in trimmed.split(';') {
        let mut kv = part.splitn(2, '=');
        let key = kv.next().unwrap_or("");
        let value = kv.next().unwrap_or("");

        match key {
            "v" => {
                if value != "1" {
                    return Err("Unsupported submission note version".to_string());
                }
            }
            "ln" if level_name.is_none() => {
                level_name = Some(decode_submission_value(value)?);
            }
            "ci" if creator_id.is_none() => creator_id = value.parse().ok(),
            "cn" if creator_name.is_none() => {
                creator_name = Some(decode_submission_value(value)?);
            }
            "dw" if downloads.is_none() => downloads = value.parse().ok(),
            "lk" if likes.is_none() => likes = value.parse().ok(),
            "ls" if stars.is_none() => stars = value.parse().ok(),
            "ll" if length.is_none() => length = value.parse().ok(),
            "lr" if rating.is_none() => rating = value.parse().ok(),
            "ld" if difficulty.is_none() => difficulty = value.parse().ok(),
            "pr" if percentage.is_none() => percentage = value.parse().ok(),
            "tm" if attempt_time.is_none() => attempt_time = value.parse().ok(),
            "m" if message.is_none() => message = Some(decode_submission_value(value)?),
            _ => {}
        }
    }

    if level_name.is_none()
        || creator_id.is_none()
        || creator_name.is_none()
        || downloads.is_none()
        || likes.is_none()
        || stars.is_none()
        // || length.is_none() // length can be sometimes missing
        || rating.is_none()
        || difficulty.is_none()
        || percentage.is_none()
        || attempt_time.is_none()
    {
        return Err("Missing required fields in submission note".to_string());
    }

    Ok(ParsedSubmissionNote {
        level_name: level_name.unwrap_or_default(),
        creator_id: creator_id.unwrap_or_default(),
        creator_name: creator_name.unwrap_or_default(),
        downloads: downloads.unwrap_or_default(),
        likes: likes.unwrap_or_default(),
        stars: stars.unwrap_or_default(),
        length: length.unwrap_or_default(),
        rating: rating.unwrap_or_default(),
        difficulty: difficulty.unwrap_or_default(),
        percentage: percentage.unwrap_or_default(),
        attempt_time: attempt_time.unwrap_or_default(),
        message,
    })
}

pub fn str_response(status: StatusCode, message: &str) -> Response {
    response(
        status,
        json!({
            "status": status.as_u16(),
            "message": message,
        }),
    )
}

fn try_read_cookie(headers: &HeaderMap, cookie_name: &str) -> Option<String> {
    headers.get("Cookie").and_then(|cookie| {
        cookie.to_str().ok().and_then(|cookie_str| {
            cookie_str.split(';').find_map(|part| {
                let trimmed = part.trim();
                if trimmed.starts_with(cookie_name) {
                    Some(trimmed.trim_start_matches(cookie_name).to_string())
                } else {
                    None
                }
            })
        })
    })
}

async fn session_response(
    token: &str,
    db: &database::AppState,
) -> Result<database::User, Response> {
    match UserSession::from_jwt(token) {
        Ok(session) => match db.get_user_by_id(session.id).await {
            Some(user) => Ok(user),
            None => Err(str_response(
                StatusCode::from_u16(498).unwrap(), // "Invalid Token"
                "User not found"
            )),
        },
        Err(e) => Err(str_response(StatusCode::UNAUTHORIZED, &e.to_string())),
    }
}

pub async fn auth_middleware(
    headers: &HeaderMap,
    db: &database::AppState,
) -> Result<database::User, Response> {
    match headers.get("Authorization").and_then(|h| h.to_str().ok()) {
        Some(token) => session_response(token, db).await,
        None => {
            if let Some(token) = try_read_cookie(headers, "auth_token=") {
                session_response(&token, db).await
            } else {
                Err(str_response(StatusCode::UNAUTHORIZED, "Missing Authorization header"))
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModPlatform {
    Windows,
    Android64,
    Android32,
    IntelMac,
    ArmMac,
    Ios,
}

impl std::fmt::Display for ModPlatform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let platform = match self {
            ModPlatform::Windows => "Windows",
            ModPlatform::Android64 => "Android64",
            ModPlatform::Android32 => "Android32",
            ModPlatform::IntelMac => "MacIntel",
            ModPlatform::ArmMac => "MacArm",
            ModPlatform::Ios => "iOS",
        };

        f.write_str(platform)
    }
}

#[derive(Debug, Clone)]
pub enum VersionPrefix {
    Release,
    Alpha(Option<u32>),
    Beta(Option<u32>),
    Prerelease(Option<u32>),
}

#[derive(Debug, Clone)]
pub struct VersionInfo {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
    pub prefix: VersionPrefix,
}

impl VersionInfo {
    pub fn from_str(str: &str) -> Option<VersionInfo> {
        let s = str.trim();

        let s = s.strip_prefix('v')?;

        let mut parts = s.splitn(2, '-');
        let nums_part = parts.next().unwrap_or("");
        let prerelease_part = parts.next();

        let nums: Vec<&str> = nums_part.split('.').collect();
        if nums.len() != 3 {
            return None;
        }

        let major = nums[0].parse::<u32>().ok()?;
        let minor = nums[1].parse::<u32>().ok()?;
        let patch = nums[2].parse::<u32>().ok()?;

        let prefix = if let Some(pr) = prerelease_part {
            let pr_parts: Vec<&str> = pr.split('.').collect();
            let ident = pr_parts.get(0).copied().unwrap_or("");
            let opt_num = pr_parts.get(1).and_then(|n| n.parse::<u32>().ok());

            if pr_parts.len() > 2 {
                return None;
            }

            match ident {
                "alpha" => VersionPrefix::Alpha(opt_num),
                "beta" => VersionPrefix::Beta(opt_num),
                "prerelease" | "pr" => VersionPrefix::Prerelease(opt_num),
                _ => return None,
            }
        } else {
            VersionPrefix::Release
        };

        Some(VersionInfo { major, minor, patch, prefix })
    }

    pub fn is_newer_than(&self, other: &VersionInfo) -> bool {
        if self.major != other.major {
            return self.major > other.major;
        }
        if self.minor != other.minor {
            return self.minor > other.minor;
        }
        if self.patch != other.patch {
            return self.patch > other.patch;
        }

        match (&self.prefix, &other.prefix) {
            (VersionPrefix::Release, VersionPrefix::Release) => false,
            (VersionPrefix::Release, _) => true,
            (_, VersionPrefix::Release) => false,
            (VersionPrefix::Alpha(n1), VersionPrefix::Alpha(n2))
            | (VersionPrefix::Beta(n1), VersionPrefix::Beta(n2))
            | (VersionPrefix::Prerelease(n1), VersionPrefix::Prerelease(n2)) => match (n1, n2) {
                (Some(num1), Some(num2)) => num1 > num2,
                (Some(_), None) => true,
                (None, Some(_)) => false,
                (None, None) => false,
            },
            (VersionPrefix::Alpha(_), _) => false,
            (_, VersionPrefix::Alpha(_)) => true,
            (VersionPrefix::Beta(_), _) => false,
            (_, VersionPrefix::Beta(_)) => true,
        }
    }
}

impl std::fmt::Display for VersionInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let prefix_str = match &self.prefix {
            VersionPrefix::Release => "".to_string(),
            VersionPrefix::Alpha(opt) => {
                format!("-alpha{}", opt.map_or("".to_string(), |n| format!(".{}", n)))
            }
            VersionPrefix::Beta(opt) => {
                format!("-beta{}", opt.map_or("".to_string(), |n| format!(".{}", n)))
            }
            VersionPrefix::Prerelease(opt) => {
                format!("-pr{}", opt.map_or("".to_string(), |n| format!(".{}", n)))
            }
        };
        write!(f, "v{}.{}.{}{}", self.major, self.minor, self.patch, prefix_str)
    }
}

impl serde::Serialize for VersionInfo {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> serde::Deserialize<'de> for VersionInfo {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        VersionInfo::from_str(&s).ok_or_else(|| serde::de::Error::custom("Invalid version format"))
    }
}

pub struct ModUserAgent {
    pub platform: ModPlatform,
    pub version: VersionInfo,
}

impl ModUserAgent {
    pub fn default() -> Self {
        Self {
            platform: ModPlatform::Windows,
            version: VersionInfo {
                major: 2,
                minor: 3,
                patch: 0,
                prefix: VersionPrefix::Release,
            },
        }
    }
}

pub fn parse_useragent(headers: &HeaderMap) -> Option<ModUserAgent> {
    headers.get(header::USER_AGENT).and_then(|ua| ua.to_str().ok()).and_then(|ua_str| {
        let parts: Vec<&str> = ua_str.split('/').collect();
        if parts.len() != 3 || parts[0] != "LevelThumbnails" {
            return None;
        }

        let platform = match parts[1] {
            "Windows" => ModPlatform::Windows,
            "Android64" => ModPlatform::Android64,
            "Android32" => ModPlatform::Android32,
            "MacIntel" => ModPlatform::IntelMac,
            "MacArm" => ModPlatform::ArmMac,
            "iOS" => ModPlatform::Ios,
            _ => return None,
        };

        let version = VersionInfo::from_str(parts[2])?;

        Some(ModUserAgent { platform, version })
    })
}
