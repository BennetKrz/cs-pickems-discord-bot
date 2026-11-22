-- T08: bot posts (daily digest, match results), the Activity deep link,
-- and the digest schedule per guild. See docs/adr/0013-bot-announcements-launch.md.

-- The daily digest goes out at `digest_time` (HH:MM) local time in
-- `timezone` (an IANA name). This is the only local time the backend
-- keeps, and only to schedule the digest; posts show times as Discord
-- `<t:…>` markup, which each viewer sees in their own time zone.
ALTER TABLE guild_settings ADD COLUMN digest_time TEXT NOT NULL DEFAULT '10:00'
    CHECK (digest_time GLOB '[0-2][0-9]:[0-5][0-9]' AND digest_time < '24:00');
ALTER TABLE guild_settings ADD COLUMN timezone TEXT NOT NULL DEFAULT 'Europe/Berlin'
    CHECK (timezone <> '');
-- When the announce channel was set: results of matches that finished
-- before are never announced, so setting a channel doesn't flood it with
-- history.
ALTER TABLE guild_settings ADD COLUMN announce_since TEXT
    CHECK (announce_since GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z');

-- Every channel post of the bot, claimed before it is sent. A row with
-- `message_id` NULL is a send in flight (or interrupted by a restart); it
-- is resent with the same Discord nonce, so Discord returns the message
-- instead of posting twice. `posted_at` counts toward the post rate limit.
CREATE TABLE bot_posts (
    id            INTEGER PRIMARY KEY,
    guild_id      TEXT NOT NULL CHECK (guild_id <> '' AND guild_id NOT GLOB '*[^0-9]*'),
    kind          TEXT NOT NULL CHECK (kind IN ('digest', 'results')),
    -- `digest`: the local date (YYYY-MM-DD); `results`: `m<first match ID>`.
    post_key      TEXT NOT NULL CHECK (post_key <> ''),
    channel_id    TEXT NOT NULL CHECK (channel_id <> '' AND channel_id NOT GLOB '*[^0-9]*'),
    message_id    TEXT CHECK (message_id <> '' AND message_id NOT GLOB '*[^0-9]*'),
    -- Hash of the rendered content last sent, so edits happen only on change.
    content_hash  TEXT,
    posted_at     TEXT NOT NULL CHECK (posted_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
    edited_at     TEXT CHECK (edited_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
    UNIQUE (guild_id, kind, post_key)
) STRICT;

CREATE INDEX bot_posts_recent ON bot_posts (guild_id, posted_at);

-- Which result post announced a match in a guild: each match once.
CREATE TABLE announced_results (
    guild_id  TEXT NOT NULL CHECK (guild_id <> '' AND guild_id NOT GLOB '*[^0-9]*'),
    match_id  INTEGER NOT NULL REFERENCES matches (id) ON DELETE CASCADE,
    post_id   INTEGER NOT NULL REFERENCES bot_posts (id) ON DELETE CASCADE,
    PRIMARY KEY (guild_id, match_id)
) STRICT;

CREATE INDEX announced_results_post ON announced_results (post_id);

-- The Activity deep link: *Pick now* on a bot post stores the match here,
-- and the Activity takes it once on start (GET /api/me/pending-view).
-- Discord has no documented launch parameters. No foreign key to `users`:
-- the member may not have signed in yet.
CREATE TABLE pending_views (
    guild_id    TEXT NOT NULL CHECK (guild_id <> '' AND guild_id NOT GLOB '*[^0-9]*'),
    user_id     TEXT NOT NULL CHECK (user_id <> '' AND user_id NOT GLOB '*[^0-9]*'),
    match_id    INTEGER NOT NULL REFERENCES matches (id) ON DELETE CASCADE,
    expires_at  TEXT NOT NULL CHECK (expires_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
    PRIMARY KEY (guild_id, user_id)
) STRICT;
