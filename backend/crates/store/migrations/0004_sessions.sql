-- T07: server-side sessions for the Activity. Conventions as in 0001 and
-- docs/adr/0003-database-schema.md; design in docs/adr/0007-discord-auth-sessions.md.

-- One row per sign-in. Only the SHA-256 of the opaque bearer token is
-- stored (lowercase hex), so a copy of the database grants no access.
-- The guild is the one verified at sign-in; every request is scoped to it.
CREATE TABLE sessions (
    token_hash  TEXT PRIMARY KEY
                CHECK (length(token_hash) = 64 AND token_hash NOT GLOB '*[^0-9a-f]*'),
    user_id     TEXT NOT NULL REFERENCES users (discord_user_id) ON DELETE CASCADE,
    guild_id    TEXT NOT NULL CHECK (guild_id <> '' AND guild_id NOT GLOB '*[^0-9]*'),
    created_at  TEXT NOT NULL CHECK (created_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
    expires_at  TEXT NOT NULL CHECK (expires_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
    CHECK (expires_at > created_at)
) STRICT;

CREATE INDEX sessions_expires_at ON sessions (expires_at);
CREATE INDEX sessions_user ON sessions (user_id);
