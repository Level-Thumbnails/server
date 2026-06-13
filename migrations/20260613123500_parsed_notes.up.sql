CREATE TYPE length_enum AS ENUM
(
    'Tiny',
    'Short',
    'Medium',
    'Long',
    'XL',
    'Plat'
);

CREATE TYPE rating_enum AS ENUM
(
    'NA',
    'Rated',
    'Featured',
    'Epic',
    'Legendary',
    'Mythic'
);

CREATE TYPE difficulty_enum AS ENUM
(
    'NA',
    'Auto',
    'Easy',
    'Normal',
    'Hard',
    'Harder',
    'Insane',
    'EasyDemon',
    'MediumDemon',
    'HardDemon',
    'InsaneDemon',
    'ExtremeDemon'
);

CREATE TABLE IF NOT EXISTS notes
(
    upload_id BIGINT PRIMARY KEY REFERENCES uploads(id) ON DELETE CASCADE,
    level_name TEXT NOT NULL,
    creator_id BIGINT NOT NULL,
    creator_name TEXT NOT NULL,
    downloads BIGINT NOT NULL,
    likes BIGINT NOT NULL,
    stars BIGINT NOT NULL,
    length length_enum NOT NULL,
    rating rating_enum NOT NULL,
    difficulty difficulty_enum NOT NULL,
    percentage DECIMAL(5, 2) NOT NULL,
    attempt_time DECIMAL NOT NULL,
    message TEXT,
    mod_version TEXT NOT NULL,
    mod_platform TEXT NOT NULL
);