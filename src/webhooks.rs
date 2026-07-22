use crate::db;
use reqwest::Client;
use serde_json::{Value, json};
use std::sync::LazyLock;
use std::time::Duration;
use chrono::{DateTime, Utc};

static WEBHOOK_CLIENT: LazyLock<WebhookClient> = LazyLock::new(WebhookClient::new);

pub(crate) struct WebhookClient {
    client: Client,
    base_url: String,

    _thumb_webhook_url: String,
    _enabled_thumbs: bool,

    system_webhook_url: String,
    enabled_system: bool,

    global_webhook_url: String,
    enabled_global: bool,
    notif_ping: String,
}

pub(crate) enum GlobalNotification {
    SubmissionsOpen,
    SubmissionsClosed { pending: i64 },
}

const SUBMISSIONS_OPEN_ACCENT: i32 = 0x57F287;
const SUBMISSIONS_CLOSED_ACCENT: i32 = 0xED4245;

pub(crate) enum SystemNotification {
    RoleChanged {
        old_role: db::Role,
        new_role: db::Role,
        username: String,
        discord_handle: Option<i64>,
        changed_by: String,
        changed_by_role: db::Role,
        changed_by_discord: Option<i64>,
    },
    LevelLocked {
        level_id: i64,
        reason: Option<String>,
        by_username: String,
        by_role: db::Role,
        by_discord: Option<i64>,
    },
    LevelUnlocked {
        level_id: i64,
        by_username: String,
        by_role: db::Role,
        by_discord: Option<i64>,
    },
    UserBanned {
        username: String,
        role: db::Role,
        discord: Option<i64>,
        reason: String,
        expires_at: Option<DateTime<Utc>>,
        by_username: String,
        by_role: db::Role,
        by_discord: Option<i64>,
    },
    UserUnbanned {
        username: String,
        role: db::Role,
        discord: Option<i64>,
        by_username: String,
        by_role: db::Role,
        by_discord: Option<i64>,
    },
    ThumbnailDeleted {
        level_id: i64,
        upload_id: i64,
        by_username: String,
        by_role: db::Role,
        by_discord: Option<i64>,
    },
    ServerRestart
}

const ROLE_CHANGED_ACCENT: i32 = 0xE67E22;
const LEVEL_LOCKED_ACCENT: i32 = 0xAD1457;
const LEVEL_UNLOCKED_ACCENT: i32 = 0x1F8B4C;
const USER_BANNED_ACCENT: i32 = 0xE74C3C;
const USER_UNBANNED_ACCENT: i32 = 0x2ECC71;
const THUMBNAIL_DELETED_ACCENT: i32 = 0x71368A;
const SERVER_RESTART_ACCENT: i32 = 0xF1C40F;

// pub(crate) enum ThumbnailNotification {
//     NewUpload {},
//     Replacement {},
// }

fn gen_timestamp() -> String {
    format!("<t:{}:f>", Utc::now().timestamp())
}

fn role_badge_emoji(role: db::Role) -> &'static str {
    match role {
        db::Role::User => "<:LT_Badge_User:1529232286086598847>",
        db::Role::Verified => "<:LT_Badge_VerThumbnailer:1529232288200527892>",
        db::Role::Moderator => "<:LT_Badge_ThumbnailMod:1529232290134360165>",
        db::Role::Admin => "<:LT_Badge_ThumbnailAdmin:1529232292281847999>",
        db::Role::Owner => "<:LT_Badge_Owner:1529232294274142261>",
    }
}

fn format_role(role: db::Role) -> &'static str {
    match role {
        db::Role::User => "User",
        db::Role::Verified => "Verified",
        db::Role::Moderator => "Moderator",
        db::Role::Admin => "Admin",
        db::Role::Owner => "Owner",
    }
}

fn format_discord(handle: Option<i64>) -> String {
    if let Some(id) = handle {
        format!(" (<@{}>)", id)
    } else {
        "".to_string()
    }
}

impl WebhookClient {
    pub fn get() -> &'static Self {
        &WEBHOOK_CLIENT
    }

    fn new() -> Self {
        let client = reqwest::ClientBuilder::new()
            .user_agent(format!("level-thumbnails-server/{}", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(10))
            .build()
            .expect("Failed to create HTTP client");

        let base_url = dotenv::var("HOME_URL").unwrap_or_default();

        let res = dotenv::var("DISCORD_WEBHOOK_URL_THUMBS");
        let enabled_thumbs = res.is_ok();
        let mut thumb_webhook_url = res.unwrap_or_default();
        if enabled_thumbs && !thumb_webhook_url.ends_with("?with_components=true") {
            thumb_webhook_url.push_str("?with_components=true");
        }

        let res = dotenv::var("DISCORD_WEBHOOK_URL_SYSTEM");
        let enabled_system = res.is_ok();
        let mut system_webhook_url = res.unwrap_or_default();
        if enabled_system && !system_webhook_url.ends_with("?with_components=true") {
            system_webhook_url.push_str("?with_components=true");
        }

        let res = dotenv::var("DISCORD_WEBHOOK_URL_GLOBAL");
        let enabled_global = res.is_ok();
        let mut global_webhook_url = res.unwrap_or_default();
        if enabled_global && !global_webhook_url.ends_with("?with_components=true") {
            global_webhook_url.push_str("?with_components=true");
        }

        let notif_ping = dotenv::var("DISCORD_WEBHOOK_PING").unwrap_or_default();

        Self {
            client,
            base_url,
            _thumb_webhook_url: thumb_webhook_url,
            _enabled_thumbs: enabled_thumbs,
            system_webhook_url,
            enabled_system,
            global_webhook_url,
            enabled_global,
            notif_ping,
        }
    }

    pub async fn send_global_notification(
        &self,
        notification: GlobalNotification,
    ) -> Result<(), reqwest::Error> {
        if !self.enabled_global {
            return Ok(());
        }

        let (accent_color, content) = match notification {
            GlobalNotification::SubmissionsOpen => (
                SUBMISSIONS_OPEN_ACCENT,
                format!("### ✅ Submissions are open!\n-# • {}", gen_timestamp()),
            ),
            GlobalNotification::SubmissionsClosed { pending } => (
                SUBMISSIONS_CLOSED_ACCENT,
                format!(
                    "### 🛑 Submissions are closed!\nPending thumbnails: **__{}__**\n-# • {}",
                    pending,
                    gen_timestamp()
                ),
            ),
        };

        let mut components: Vec<Value> = vec![json!({
            "type": 17,
            "accent_color": accent_color,
            "components": [{
                "type": 10,
                "content": content
            }]
        })];

        if !self.notif_ping.trim().is_empty() {
            components.push(json!({
                "type": 10,
                "content": format!("-# {}", self.notif_ping)
            }));
        }

        let payload = json!({
            "flags": 32768,
            "components": components,
        });

        self.client
            .post(&self.global_webhook_url)
            .json(&payload)
            .send()
            .await?
            .error_for_status()?;

        Ok(())
    }

    pub async fn send_system_notification(
        &self,
        notification: SystemNotification
    ) -> Result<(), reqwest::Error> {
        if !self.enabled_system {
            return Ok(());
        }

        let components = match notification {
            SystemNotification::RoleChanged {
                old_role,
                new_role,
                username,
                discord_handle,
                changed_by,
                changed_by_role,
                changed_by_discord,
            } => {
                let content = format!(
                    "### **{}**{}\n{} {} -> {} {}\n\n-# Changed by {} **{}**{} • {}",
                    username,
                    format_discord(discord_handle),
                    role_badge_emoji(old_role),
                    format_role(old_role),
                    role_badge_emoji(new_role),
                    format_role(new_role),
                    role_badge_emoji(changed_by_role),
                    changed_by,
                    format_discord(changed_by_discord),
                    gen_timestamp()
                );

                json!({
                    "type": 17,
                    "accent_color": ROLE_CHANGED_ACCENT,
                    "components": [{
                        "type": 10,
                        "content": "### 👤 Role Changed"
                    },{
                        "type": 14,
                        "divider": true,
                        "spacing": 1
                    },{
                        "type": 10,
                        "content": content
                    }]
                })
            },
            SystemNotification::LevelLocked {
                level_id,
                reason,
                by_username,
                by_role,
                by_discord,
            } => {
                let content = format!(
                    "### Level ID: `{}`\nReason: **__{}__**\n\n-# {} **{}**{} • {}",
                    level_id,
                    reason.unwrap_or_else(|| "(No reason provided)".to_string()),
                    role_badge_emoji(by_role),
                    by_username,
                    format_discord(by_discord),
                    gen_timestamp()
                );

                json!({
                    "type": 17,
                    "accent_color": LEVEL_LOCKED_ACCENT,
                    "components": [{
                        "type": 10,
                        "content": "### 🔒 Level Locked"
                    },{
                        "type": 14,
                        "divider": true,
                        "spacing": 1
                    },{
                        "type": 10,
                        "content": content
                    }]
                })
            },
            SystemNotification::LevelUnlocked {
                level_id,
                by_username,
                by_role,
                by_discord,
            } => {
                let content = format!(
                    "### Level ID: `{}`\n\n-# {} **{}**{} • {}",
                    level_id,
                    role_badge_emoji(by_role),
                    by_username,
                    format_discord(by_discord),
                    gen_timestamp()
                );

                json!({
                    "type": 17,
                    "accent_color": LEVEL_UNLOCKED_ACCENT,
                    "components": [{
                        "type": 10,
                        "content": "### 🔓 Level Unlocked"
                    },{
                        "type": 14,
                        "divider": true,
                        "spacing": 1
                    },{
                        "type": 10,
                        "content": content
                    }]
                })
            },
            SystemNotification::UserBanned {
                username,
                role,
                discord,
                reason,
                expires_at,
                by_username,
                by_role,
                by_discord
            } => {
                let content =format!(
                    "### {} **{}**{}\nReason: **__{}__**\n{}\n-# Banned by: {} **{}**{} • {}",
                    role_badge_emoji(role),
                    username,
                    format_discord(discord),
                    reason,
                    if let Some(time) = expires_at {
                        format!("Expires at: <t:{}:R>\n", time.timestamp())
                    } else {
                        "".to_string()
                    },
                    role_badge_emoji(by_role),
                    by_username,
                    format_discord(by_discord),
                    gen_timestamp()
                );

                json!({
                    "type": 17,
                    "accent_color": USER_BANNED_ACCENT,
                    "components": [{
                        "type": 10,
                        "content": "### ⛔ User Banned"
                    },{
                        "type": 14,
                        "divider": true,
                        "spacing": 1
                    },{
                        "type": 10,
                        "content": content
                    }]
                })
            },
            SystemNotification::UserUnbanned {
                username,
                role,
                discord,
                by_username,
                by_role,
                by_discord
            } => {
                let content =format!(
                    "### {} **{}**{}\n\n-# Unbanned by: {} **{}**{} • {}",
                    role_badge_emoji(role),
                    username,
                    format_discord(discord),
                    role_badge_emoji(by_role),
                    by_username,
                    format_discord(by_discord),
                    gen_timestamp()
                );

                json!({
                    "type": 17,
                    "accent_color": USER_UNBANNED_ACCENT,
                    "components": [{
                        "type": 10,
                        "content": "### ❎ User Unbanned"
                    },{
                        "type": 14,
                        "divider": true,
                        "spacing": 1
                    },{
                        "type": 10,
                        "content": content
                    }]
                })
            },
            SystemNotification::ThumbnailDeleted {
                level_id,
                upload_id,
                by_username,
                by_role,
                by_discord
            } => {
                json!({
                    "type": 17,
                    "accent_color": THUMBNAIL_DELETED_ACCENT,
                    "components": [{
                        "type": 10,
                        "content": "### 🗑️ Thumbnail Deleted"
                    },{
                        "type": 14,
                        "divider": true,
                        "spacing": 1
                    },{
                        "type": 10,
                        "content": format!("### Level ID: `{}`", level_id)
                    },{
                        "type": 12,
                        "items": [{
                            "media": { "url": format!("{}/pending/{}/image", self.base_url, upload_id) },
                            "description": null,
                            "spoiler": true
                        }]
                    },{
                        "type": 10,
                        "content": format!(
                            "-# Deleted by: {} **{}**{} • {}",
                            role_badge_emoji(by_role),
                            by_username,
                            format_discord(by_discord),
                            gen_timestamp()
                        )
                    }]
                })
            },
            SystemNotification::ServerRestart => {
                json!({
                    "type": 17,
                    "accent_color": SERVER_RESTART_ACCENT,
                    "components": [{
                        "type": 10,
                        "content": format!("### 🔃 Server Restarted\n-# • {}", gen_timestamp()),
                    }]
                })
            }
        };

        let payload = json!({
            "flags": 32768,
            "components": [components]
        });

        self.client
            .post(&self.system_webhook_url)
            .json(&payload)
            .send()
            .await?
            .error_for_status()?;

        Ok(())
    }
}
