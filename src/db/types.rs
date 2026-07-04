use crate::util::VersionInfo;
use serde::{Deserialize, Serialize};

pub fn default_min_supported_client() -> VersionInfo {
    VersionInfo::from_str("v2.1.0").expect("Invalid default version")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub pause_submissions: bool,
    #[serde(default = "default_min_supported_client")]
    pub min_supported_client: VersionInfo,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            pause_submissions: false,
            min_supported_client: default_min_supported_client(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Permission {
    DirectUploadNewThumbnail,
    DirectUploadReplacement,
    ModeratePendingUploads,
    ManageUserProfiles,
    ViewOtherUserHistory,
    ManageLevelLocks,
    ManageSettings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, sqlx::Type)]
#[serde(rename_all = "lowercase")]
#[sqlx(type_name = "TEXT", rename_all = "lowercase")]
pub enum Role {
    User,      // regular user
    Verified,  // verified users can upload thumbnails without approval
    Moderator, // moderators can approve or reject uploads
    Admin,     // admins can manage users and uploads
    Owner,     // superadmin with unrestricted access
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Role::User => write!(f, "user"),
            Role::Verified => write!(f, "verified"),
            Role::Moderator => write!(f, "moderator"),
            Role::Admin => write!(f, "admin"),
            Role::Owner => write!(f, "owner"),
        }
    }
}

impl Role {
    pub const ORDERED: [Role; 5] =
        [Role::User, Role::Verified, Role::Moderator, Role::Admin, Role::Owner];

    pub fn rank(self) -> u8 {
        match self {
            Role::User => 0,
            Role::Verified => 1,
            Role::Moderator => 2,
            Role::Admin => 3,
            Role::Owner => 4,
        }
    }

    pub fn has_permission(self, permission: Permission) -> bool {
        match permission {
            Permission::DirectUploadNewThumbnail => {
                matches!(self, Role::Verified | Role::Moderator | Role::Admin | Role::Owner)
            }
            Permission::DirectUploadReplacement => {
                matches!(self, Role::Moderator | Role::Admin | Role::Owner)
            }
            Permission::ModeratePendingUploads => {
                matches!(self, Role::Moderator | Role::Admin | Role::Owner)
            }
            Permission::ManageUserProfiles => {
                matches!(self, Role::Moderator | Role::Admin | Role::Owner)
            }
            Permission::ViewOtherUserHistory => {
                matches!(self, Role::Moderator | Role::Admin | Role::Owner)
            }
            Permission::ManageLevelLocks => matches!(self, Role::Admin | Role::Owner),
            Permission::ManageSettings => matches!(self, Role::Admin | Role::Owner),
        }
    }

    pub fn can_manage_user(self, target: Role) -> bool {
        self == Role::Owner
            || (self.has_permission(Permission::ManageUserProfiles) && self.rank() > target.rank())
    }

    pub fn can_assign_role(self, target: Role) -> bool {
        self == Role::Owner
            || (self.has_permission(Permission::ManageUserProfiles) && self.rank() > target.rank())
    }

    pub fn can_bypass_level_locks(self) -> bool {
        self.has_permission(Permission::ManageLevelLocks)
    }

    pub fn can_manage_level_locks(self) -> bool {
        self.has_permission(Permission::ManageLevelLocks)
    }

    pub fn can_moderate_pending_uploads(self) -> bool {
        self.has_permission(Permission::ModeratePendingUploads)
    }

    pub fn can_view_other_user_history(self) -> bool {
        self.has_permission(Permission::ViewOtherUserHistory)
    }

    pub fn can_manage_settings(self) -> bool {
        self.has_permission(Permission::ManageSettings)
    }

    pub fn can_upload_new_thumbnail_directly(self) -> bool {
        self.has_permission(Permission::DirectUploadNewThumbnail)
    }

    pub fn can_upload_replacement_directly(self) -> bool {
        self.has_permission(Permission::DirectUploadReplacement)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, sqlx::Type, utoipa::ToSchema)]
#[sqlx(type_name = "length_enum")]
pub enum Length {
    Tiny,
    Short,
    Medium,
    Long,
    XL,
    Plat,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, sqlx::Type, utoipa::ToSchema)]
#[sqlx(type_name = "rating_enum")]
pub enum Rating {
    NA,
    Rated,
    Featured,
    Epic,
    Legendary,
    Mythic,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, sqlx::Type, utoipa::ToSchema)]
#[sqlx(type_name = "difficulty_enum")]
pub enum Difficulty {
    NA,
    Auto,
    Easy,
    Normal,
    Hard,
    Harder,
    Insane,
    EasyDemon,
    MediumDemon,
    HardDemon,
    InsaneDemon,
    ExtremeDemon,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UserListSortBy {
    Id,
    Username,
    AccountId,
    DiscordId,
    Role,
    TotalUploads,
    Accepted,
    Pending,
    Rejected,
    ActiveThumbnails,
    Banned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PendingUploadSortBy {
    UploadTime,
    LevelId,
    LevelName,
    CreatorName,
    Username,
    Stars,
    Rating,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SortDirection {
    Asc,
    Desc,
}

#[derive(Debug, Clone)]
pub struct UpdateUserOptions {
    pub username: Option<String>,
    pub account_id: Option<i64>,
    pub discord_id: Option<Option<i64>>,
    pub role: Option<Role>,
}
