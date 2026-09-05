CREATE TABLE IF NOT EXISTS level_daily_stats
(
    date     DATE   NOT NULL,
    level_id BIGINT NOT NULL,
    requests BIGINT NOT NULL,
    PRIMARY KEY (date, level_id)
);

CREATE INDEX IF NOT EXISTS level_daily_stats_level_id_idx
    ON level_daily_stats (level_id);

CREATE INDEX IF NOT EXISTS level_daily_stats_date_idx
    ON level_daily_stats (date);

CREATE TABLE IF NOT EXISTS level_stats_collection_log
(
    date         DATE PRIMARY KEY,
    collected_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);