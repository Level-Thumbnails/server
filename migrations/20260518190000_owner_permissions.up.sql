DO $$
DECLARE
    constraint_name text;
BEGIN
    SELECT c.conname
    INTO constraint_name
    FROM pg_constraint c
    JOIN pg_class t ON t.oid = c.conrelid
    JOIN pg_attribute a ON a.attrelid = t.oid AND a.attnum = ANY (c.conkey)
    WHERE t.relname = 'users'
      AND c.contype = 'c'
      AND a.attname = 'role'
    ORDER BY c.oid DESC
    LIMIT 1;

    IF constraint_name IS NOT NULL THEN
        EXECUTE format('ALTER TABLE users DROP CONSTRAINT %I', constraint_name);
    END IF;
END
$$;

ALTER TABLE users
    ADD CONSTRAINT users_role_check
        CHECK (role IN ('user', 'verified', 'moderator', 'admin', 'owner'));

CREATE OR REPLACE FUNCTION role_rank(role TEXT)
RETURNS SMALLINT
LANGUAGE SQL
IMMUTABLE
STRICT
AS $$
    SELECT CASE role
        WHEN 'user' THEN 0
        WHEN 'verified' THEN 1
        WHEN 'moderator' THEN 2
        WHEN 'admin' THEN 3
        WHEN 'owner' THEN 4
        ELSE 0
    END;
$$;

CREATE OR REPLACE FUNCTION role_from_rank(rank SMALLINT)
RETURNS TEXT
LANGUAGE SQL
IMMUTABLE
STRICT
AS $$
    SELECT CASE rank
        WHEN 0 THEN 'user'
        WHEN 1 THEN 'verified'
        WHEN 2 THEN 'moderator'
        WHEN 3 THEN 'admin'
        ELSE 'owner'
    END;
$$;

CREATE OR REPLACE PROCEDURE migrate(disc_id bigint, geometry_id bigint)
LANGUAGE plpgsql
AS $$
BEGIN
    -- Step 1: Move uploads
    UPDATE uploads
    SET user_id = disc_id
    WHERE user_id = geometry_id;

    -- Step 2: Migrate data to discord user
    UPDATE users AS u0
    SET
        account_id = u1.account_id,
        username = u1.username,
        role = role_from_rank(GREATEST(role_rank(u0.role), role_rank(u1.role)))
    FROM users AS u1
    WHERE u0.id = disc_id
      AND u1.id = geometry_id;

    -- Step 3: Delete Geometry Dash user
    DELETE FROM users
    WHERE id = geometry_id;
END;
$$;

