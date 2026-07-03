use sqlx::{Postgres, QueryBuilder};
use crate::db::{AdminUserQueryOptions, PendingQueryOptions, Role, SortDirection, UserListSortBy};

pub fn apply_pending_filters<'a>(
    builder: &mut QueryBuilder<'a, Postgres>,
    options: &PendingQueryOptions,
) {
    if let Some(level_id) = options.level_id {
        builder.push(" AND uploads.level_id = ").push_bind(level_id);
    }

    if let Some(user_id) = options.user_id {
        builder.push(" AND uploads.user_id = ").push_bind(user_id);
    }

    if let Some(ref username) = options.username {
        builder
            .push(" AND LOWER(username) LIKE LOWER(")
            .push_bind(format!("%{}%", username))
            .push(")");
    }
}

pub fn apply_user_filters<'a>(
    builder: &mut QueryBuilder<'a, Postgres>,
    options: &AdminUserQueryOptions,
) {
    if let Some(id) = options.id {
        builder.push(" AND id = ").push_bind(id);
    }

    if let Some(ref username) = options.username {
        let username = username.trim();
        if !username.is_empty() {
            builder
                .push(" AND LOWER(username) LIKE LOWER(")
                .push_bind(format!("%{}%", username))
                .push(")");
        }
    }

    if let Some(account_id) = options.account_id {
        builder.push(" AND account_id = ").push_bind(account_id);
    }

    if let Some(discord_id) = options.discord_id {
        builder.push(" AND discord_id = ").push_bind(discord_id);
    }

    if let Some(role) = options.role {
        builder.push(" AND role = ").push_bind(role);
    }

    if let Some(total_uploads) = options.total_uploads {
        builder.push(" AND total_uploads = ").push_bind(total_uploads);
    }

    if let Some(banned) = options.banned {
        builder.push(" AND banned = ").push_bind(banned);
    }
}

pub fn apply_user_sort(
    builder: &mut QueryBuilder<'_, Postgres>,
    sort_by: UserListSortBy,
    sort_direction: SortDirection,
) {
    let direction = match sort_direction {
        SortDirection::Asc => "ASC",
        SortDirection::Desc => "DESC",
    };

    builder.push(" ORDER BY ");
    match sort_by {
        UserListSortBy::Id => builder.push("id ").push(direction),
        UserListSortBy::Username => builder.push("LOWER(username) ").push(direction),
        UserListSortBy::AccountId => builder.push("account_id ").push(direction),
        UserListSortBy::DiscordId => {
            builder.push("discord_id ").push(direction).push(" NULLS LAST")
        }
        UserListSortBy::Role => {
            builder.push("CASE role ");
            for (rank, role) in Role::ORDERED.iter().copied().enumerate() {
                builder
                    .push("WHEN ")
                    .push_bind(role)
                    .push(" THEN ")
                    .push_bind(rank as i32)
                    .push(" ");
            }
            builder.push("END ").push(direction)
        }
        UserListSortBy::TotalUploads => builder.push("total_uploads ").push(direction),
        UserListSortBy::Accepted => builder.push("accepted ").push(direction),
        UserListSortBy::Pending => builder.push("pending ").push(direction),
        UserListSortBy::Rejected => builder.push("rejected ").push(direction),
        UserListSortBy::ActiveThumbnails => builder.push("active_thumbnails ").push(direction),
        UserListSortBy::Banned => builder.push("banned ").push(direction),
    };
}