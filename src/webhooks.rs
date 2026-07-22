use crate::db;
use crate::db::{Difficulty, Rating, Role};
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde_json::json;
use std::sync::LazyLock;
use std::time::Duration;

static WEBHOOK_CLIENT: LazyLock<WebhookClient> = LazyLock::new(WebhookClient::new);

pub(crate) struct WebhookClient {
    client: Client,
    base_url: String,

    thumb_webhook_url: String,
    enabled_thumbs: bool,

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

pub(crate) enum SystemNotification<'a> {
    RoleChanged {
        old_role: db::Role,
        new_role: db::Role,
        username: &'a str,
        discord_handle: Option<i64>,
        changed_by: &'a str,
        changed_by_role: db::Role,
        changed_by_discord: Option<i64>,
    },
    LevelLocked {
        level_id: i64,
        reason: Option<&'a str>,
        by_username: &'a str,
        by_role: db::Role,
        by_discord: Option<i64>,
    },
    LevelUnlocked {
        level_id: i64,
        by_username: &'a str,
        by_role: db::Role,
        by_discord: Option<i64>,
    },
    UserBanned {
        username: &'a str,
        role: db::Role,
        discord: Option<i64>,
        reason: &'a str,
        expires_at: Option<DateTime<Utc>>,
        by_username: &'a str,
        by_role: db::Role,
        by_discord: Option<i64>,
    },
    UserUnbanned {
        username: &'a str,
        role: db::Role,
        discord: Option<i64>,
        by_username: &'a str,
        by_role: db::Role,
        by_discord: Option<i64>,
    },
    ThumbnailDeleted {
        level_id: i64,
        upload_id: i64,
        by_username: &'a str,
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

pub(crate) enum ThumbnailNotification<'a> {
    NewUpload {
        level_name: &'a str,
        level_creator: &'a str,
        level_id: i64,
        difficulty: Difficulty,
        rating: Rating,
        upload_id: i64,
        created_by: &'a str,
        created_by_role: db::Role,
        created_by_discord: Option<i64>,
        accepted_by: &'a str,
        accepted_by_role: db::Role,
        accepted_by_discord: Option<i64>,
    },
    Replacement {
        level_name: &'a str,
        level_creator: &'a str,
        level_id: i64,
        difficulty: Difficulty,
        rating: Rating,
        upload_id: i64,
        old_upload_id: i64,
        created_by: &'a str,
        created_by_role: db::Role,
        created_by_discord: Option<i64>,
        accepted_by: &'a str,
        accepted_by_role: db::Role,
        accepted_by_discord: Option<i64>,
    },
}

const NEW_UPLOAD_ACCENT: i32 = 0x2ECC71;
const REPLACEMENT_ACCENT: i32 = 0x3498DB;

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
            thumb_webhook_url,
            enabled_thumbs,
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

        let has_ping = !self.notif_ping.trim().is_empty();

        let (accent_color, content) = match notification {
            GlobalNotification::SubmissionsOpen => (
                SUBMISSIONS_OPEN_ACCENT,
                format!(
                    "### ✅ Submissions are open!\n-# {}{}• {}",
                    if has_ping { &self.notif_ping } else { "" },
                    if has_ping { " " } else { "" },
                    gen_timestamp()
                ),
            ),
            GlobalNotification::SubmissionsClosed { pending } => (
                SUBMISSIONS_CLOSED_ACCENT,
                format!(
                    "### 🛑 Submissions are closed!\nPending thumbnails: **__{}__**\n\n-# {}{}• {}",
                    pending,
                    if has_ping { &self.notif_ping } else { "" },
                    if has_ping { " " } else { "" },
                    gen_timestamp()
                ),
            ),
        };

        let payload = json!({
            "flags": 32768,
            "components": [{
                "type": 17,
                "accent_color": accent_color,
                "components": [{
                    "type": 10,
                    "content": content
                }]
            }]
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
        notification: SystemNotification<'_>
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
                    reason.unwrap_or_else(|| "(No reason provided)"),
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
                        format!("Expires: <t:{}:R>\n", time.timestamp())
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
            "allowed_mentions": { "parse": [] },
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

    pub async fn send_thumb_notification(
        &self,
        notification: ThumbnailNotification<'_>
    ) -> Result<(), reqwest::Error> {
        if !self.enabled_thumbs {
            return Ok(());
        }

        let components = match notification {
            ThumbnailNotification::NewUpload {
                level_name,
                level_creator,
                level_id,
                difficulty,
                rating,
                upload_id,
                created_by,
                created_by_role,
                created_by_discord,
                accepted_by,
                accepted_by_role,
                accepted_by_discord,
            } => {
                let footer = Self::create_upload_footer(created_by, accepted_by, accepted_by_role, accepted_by_discord);

                json!({
                    "type": 17,
                    "accent_color": NEW_UPLOAD_ACCENT,
                    "components": [{
                        "type": 10,
                        "content": "### 🖼️ New Thumbnail!"
                    },{
                        "type": 14,
                        "divider": true,
                        "spacing": 1
                    },{
                        "type": 9,
                        "accessory": {
                            "type": 11,
                            "media": { "url": build_difficulty_face(&self.base_url, difficulty, rating) }
                        },
                        "components": [{
                            "type": 10,
                            "content": format!(
                                "### **__{}__** by **{}**\n{} Thumbnail by **{}**{}\n\n-# *Level ID: `{}`*",
                                level_name,
                                level_creator,
                                role_badge_emoji(created_by_role),
                                created_by,
                                format_discord(created_by_discord),
                                level_id
                            )
                        }]
                    },{
                        "type": 12,
                        "items": [{
                            "media": { "url": format!("{}/pending/{}/image", &self.base_url, upload_id) }
                        }]
                    },{
                        "type": 10,
                        "content": footer
                    }]
                })
            },
            ThumbnailNotification::Replacement {
                level_name,
                level_creator,
                level_id,
                difficulty,
                rating,
                upload_id,
                old_upload_id,
                created_by,
                created_by_role,
                created_by_discord,
                accepted_by,
                accepted_by_role,
                accepted_by_discord
            } => {
                let footer = Self::create_upload_footer(created_by, accepted_by, accepted_by_role, accepted_by_discord);

                json!({
                    "type": 17,
                    "accent_color": REPLACEMENT_ACCENT,
                    "components": [{
                        "type": 10,
                        "content": "### 🖼️ Thumbnail Replaced!"
                    },{
                        "type": 14,
                        "divider": true,
                        "spacing": 1
                    },{
                        "type": 9,
                        "accessory": {
                            "type": 11,
                            "media": { "url": build_difficulty_face(&self.base_url, difficulty, rating) }
                        },
                        "components": [{
                            "type": 10,
                            "content": format!(
                                "### **__{}__** by **{}**\n{} Thumbnail by **{}**{}\n\n-# *Level ID: `{}`*",
                                level_name,
                                level_creator,
                                role_badge_emoji(created_by_role),
                                created_by,
                                format_discord(created_by_discord),
                                level_id
                            )
                        }]
                    },{
                        "type": 12,
                        "items": [{
                            "media": { "url": format!("{}/pending/{}/image", &self.base_url, upload_id) }
                        }]
                    },{
                        "type": 10,
                        "content": "Old thumbnail:"
                    },{
                        "type": 12,
                        "items": [{
                            "media": { "url": format!("{}/pending/{}/image", &self.base_url, old_upload_id) },
                            "spoiler": true
                        }]
                    },{
                        "type": 10,
                        "content": footer
                    }]
                })
            }
        };

        let payload = json!({
            "flags": 32768,
            "allowed_mentions": { "parse": [] },
            "components": [components]
        });

        self.client
            .post(&self.thumb_webhook_url)
            .json(&payload)
            .send()
            .await?
            .error_for_status()?;

        Ok(())
    }

    fn create_upload_footer(created_by: &str, accepted_by: &str, accepted_by_role: Role, accepted_by_discord: Option<i64>) -> String {
        if created_by == accepted_by {
            format!(
                "-# {} {} • {}",
                role_badge_emoji(accepted_by_role),
                format_role(accepted_by_role),
                gen_timestamp()
            )
        } else {
            format!(
                "-# Accepted by: {} **{}**{} • {}",
                role_badge_emoji(accepted_by_role),
                accepted_by,
                format_discord(accepted_by_discord),
                gen_timestamp()
            )
        }
    }
}

fn build_difficulty_face(base: &str, difficulty: Difficulty, rating: Rating) -> String {
    let diff = match difficulty {
        Difficulty::NA => "unrated",
        Difficulty::Auto => "auto",
        Difficulty::Easy => "easy",
        Difficulty::Normal => "normal",
        Difficulty::Hard => "hard",
        Difficulty::Harder => "harder",
        Difficulty::Insane => "insane",
        Difficulty::EasyDemon => "demon-easy",
        Difficulty::MediumDemon => "demon-medium",
        Difficulty::HardDemon => "demon-hard",
        Difficulty::InsaneDemon => "demon-insane",
        Difficulty::ExtremeDemon => "demon-extreme",
    };

    let rate = match rating {
        Rating::NA | Rating::Rated => "",
        Rating::Featured => "-featured",
        Rating::Epic => "-epic",
        Rating::Legendary => "-legendary",
        Rating::Mythic => "-mythic",
    };

    format!("{}/difficulties/{}{}.webp", base, diff, rate)
}
