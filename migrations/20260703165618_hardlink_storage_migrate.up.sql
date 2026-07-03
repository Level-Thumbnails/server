CREATE TABLE IF NOT EXISTS migration_state
(
    hardlink_storage_migrated BOOLEAN NOT NULL DEFAULT FALSE
);