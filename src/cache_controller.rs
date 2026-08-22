use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{error, info, warn};

const CACHE_TTL: Duration = Duration::from_secs(3600);

struct CloudflareClient {
    api_token: String,
    zone_id: String,
    root_url: String,
    client: reqwest::Client,
    user_stats_cache: RwLock<Option<(u64, Instant)>>,
}

#[derive(Debug)]
pub struct PurgeError {
    pub status: reqwest::StatusCode,
    pub body: String,
}

static CLOUDFLARE_CLIENT: std::sync::LazyLock<CloudflareClient> =
    std::sync::LazyLock::new(CloudflareClient::new);

impl CloudflareClient {
    pub fn get() -> &'static Self {
        &CLOUDFLARE_CLIENT
    }

    fn new() -> Self {
        let api_token = dotenv::var("CLOUDFLARE_API_KEY")
            .expect("CLOUDFLARE_API_KEY must be set in the environment");

        let zone_id = dotenv::var("CLOUDFLARE_ZONE_ID")
            .expect("CLOUDFLARE_ZONE_ID must be set in the environment");

        let root_url = dotenv::var("HOME_URL").expect("HOME_URL must be set in the environment");

        let client = reqwest::ClientBuilder::new()
            .user_agent(format!("level-thumbnails-server/{}", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(10))
            .build()
            .expect("Failed to create HTTP client");

        Self {
            api_token,
            zone_id,
            root_url,
            client,
            user_stats_cache: RwLock::new(None),
        }
    }

    fn cached_user_stats_result(cached_value: Option<u64>, message: String) -> Result<u64, String> {
        match cached_value {
            Some(value) => {
                warn!(
                    "Failed to refresh Cloudflare user stats, using stale cached value: {}",
                    message
                );
                Ok(value)
            }
            None => Err(message),
        }
    }

    pub async fn purge_thumbnail(&self, level_id: i64) -> Result<(), PurgeError> {
        let urls = [
            format!("{}/thumbnail/{}", self.root_url, level_id),
            format!("{}/thumbnail/{}/small", self.root_url, level_id),
            format!("{}/thumbnail/{}/medium", self.root_url, level_id),
            format!("{}/thumbnail/{}/high", self.root_url, level_id),
            format!("{}/thumbnail/{}/info", self.root_url, level_id),
        ];

        let endpoint =
            format!("https://api.cloudflare.com/client/v4/zones/{}/purge_cache", self.zone_id);

        let payload = serde_json::json!({ "files": urls });
        let response =
            self.client.post(&endpoint).bearer_auth(&self.api_token).json(&payload).send().await;

        let response = match response {
            Ok(resp) => resp,
            Err(e) => {
                return Err(PurgeError {
                    status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                    body: e.to_string(),
                });
            }
        };

        if response.status().is_success() {
            Ok(())
        } else {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            Err(PurgeError { status, body: text })
        }
    }

    pub async fn purge_pending(&self, upload_id: i64) -> Result<(), PurgeError> {
        let urls = [
            format!("{}/pending/{}/image", self.root_url, upload_id),
        ];

        let endpoint =
            format!("https://api.cloudflare.com/client/v4/zones/{}/purge_cache", self.zone_id);

        let payload = serde_json::json!({ "files": urls });
        let response =
            self.client.post(&endpoint).bearer_auth(&self.api_token).json(&payload).send().await;

        let response = match response {
            Ok(resp) => resp,
            Err(e) => {
                return Err(PurgeError {
                    status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                    body: e.to_string(),
                });
            }
        };

        if response.status().is_success() {
            Ok(())
        } else {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            Err(PurgeError { status, body: text })
        }
    }

    pub async fn get_user_stats(&self) -> Result<u64, String> {
        let stale_cached_value = {
            let cache = self.user_stats_cache.read().await;
            match *cache {
                Some((value, timestamp)) => {
                    if timestamp.elapsed() < CACHE_TTL {
                        return Ok(value);
                    }

                    Some(value)
                }
                None => None,
            }
        };

        let since =
            (chrono::Utc::now() - chrono::Duration::days(30)).format("%Y-%m-%d").to_string();
        let until = chrono::Utc::now().format("%Y-%m-%d").to_string();

        let query = "query ($zoneTag: String!, $since: Date!, $until: Date!) { \
            viewer { \
                zones(filter: { zoneTag: $zoneTag }) { \
                    httpRequests1dGroups( \
                        limit: 31, \
                        filter: { \
                            date_geq: $since, \
                            date_lt: $until \
                        } \
                    ) { \
                        uniq { uniques } \
                    } \
                } \
            } \
        }";

        let payload = serde_json::json!({
            "query": query,
            "variables": {
                "zoneTag": self.zone_id,
                "since": since,
                "until": until,
            }
        });

        let response = self
            .client
            .post("https://api.cloudflare.com/client/v4/graphql")
            .bearer_auth(&self.api_token)
            .json(&payload)
            .send()
            .await;

        let response = match response {
            Ok(response) => response,
            Err(e) => return Self::cached_user_stats_result(stale_cached_value, e.to_string()),
        };

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Self::cached_user_stats_result(
                stale_cached_value,
                format!("Cloudflare API error {}: {}", status, text),
            );
        }

        let data: serde_json::Value = match response.json().await {
            Ok(data) => data,
            Err(e) => return Self::cached_user_stats_result(stale_cached_value, e.to_string()),
        };

        if let Some(errors) = data["errors"].as_array() {
            if !errors.is_empty() {
                let message = errors
                    .iter()
                    .filter_map(|error| error["message"].as_str())
                    .collect::<Vec<_>>()
                    .join("; ");

                return Self::cached_user_stats_result(
                    stale_cached_value,
                    format!("Cloudflare GraphQL error: {}", message),
                );
            }
        }

        let groups = data["data"]["viewer"]["zones"][0]["httpRequests1dGroups"]
            .as_array()
            .ok_or_else(|| format!("Unexpected response structure: {}", data));

        let groups = match groups {
            Ok(groups) => groups,
            Err(message) => return Self::cached_user_stats_result(stale_cached_value, message),
        };

        let total: u64 = groups.iter().filter_map(|group| group["uniq"]["uniques"].as_u64()).sum();

        *self.user_stats_cache.write().await = Some((total, Instant::now()));

        Ok(total)
    }
}

pub fn purge(level_id: i64) {
    if dotenv::var("CLOUDFLARE_API_KEY").is_err() {
        warn!("CLOUDFLARE_API_KEY is not set, not purging level {}", level_id);
        return;
    }

    tokio::spawn(async move {
        let max_retries = 5;

        for attempt in 1..=max_retries {
            match CloudflareClient::get().purge_thumbnail(level_id).await {
                Ok(_) => {
                    if attempt > 1 {
                        info!("Purge for id {} succeeded after {} attempt(s)", level_id, attempt);
                    }
                    return;
                }
                Err(e) => {
                    if e.status.as_u16() == 429 || e.status.is_server_error() {
                        let delay = 30 * attempt;
                        error!(
                            "Purge failed for id {}: {}. Retrying in {} seconds (attempt {}/{})",
                            level_id, e.body, delay, attempt, max_retries
                        );
                        tokio::time::sleep(Duration::from_secs(delay)).await;
                    } else {
                        error!("Purge failed for id {}: {}", level_id, e.body);
                        break;
                    }
                }
            }
        }
    });
}

pub fn purge_pending(upload_id: i64) {
    if dotenv::var("CLOUDFLARE_API_KEY").is_err() {
        return;
    }

    tokio::spawn(CloudflareClient::get().purge_pending(upload_id));
}

pub async fn get_user_stats() -> Result<u64, String> {
    if dotenv::var("CLOUDFLARE_API_KEY").is_err() {
        return Err("CLOUDFLARE_API_KEY is not set".to_string());
    }

    if dotenv::var("CLOUDFLARE_ZONE_ID").is_err() {
        return Err("CLOUDFLARE_ZONE_ID is not set".to_string());
    }

    CloudflareClient::get().get_user_stats().await
}
