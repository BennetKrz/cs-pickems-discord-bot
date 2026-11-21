-- T11: scoring rules history, rules pins and result states. Conventions as
-- in 0001 and docs/adr/0003-database-schema.md; design in
-- docs/adr/0011-scoring-and-settlement.md.

-- Every scoring rules version a guild has had. `guild_settings` keeps the
-- current one; this table lets settlement score a match with the version
-- that was active when it locked. NULL rules mean the default rules.
-- Append-only: a change writes a new version.
CREATE TABLE scoring_rule_versions (
    guild_id    TEXT NOT NULL CHECK (guild_id <> '' AND guild_id NOT GLOB '*[^0-9]*'),
    version     INTEGER NOT NULL CHECK (version >= 1),
    rules       TEXT CHECK (json_valid(rules)),
    valid_from  TEXT NOT NULL CHECK (valid_from GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
    PRIMARY KEY (guild_id, version)
) STRICT;

-- Which rules version settles a match in a guild, fixed at its first
-- settlement (`app rescore --all` moves it to the current version on
-- purpose). `finished_seen_at` is when settlement first saw the match
-- finished: the start of the 48 h data deadline if the match has no
-- `ended_at`.
CREATE TABLE match_scoring (
    guild_id          TEXT NOT NULL CHECK (guild_id <> '' AND guild_id NOT GLOB '*[^0-9]*'),
    match_id          INTEGER NOT NULL REFERENCES matches (id) ON DELETE CASCADE,
    rules_version     INTEGER NOT NULL CHECK (rules_version >= 1),
    finished_seen_at  TEXT CHECK (finished_seen_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
    PRIMARY KEY (guild_id, match_id)
) STRICT;

-- How settled a result is (`domain::ResultState`), and how many of its
-- parts still wait for a result or data (missing round scores).
ALTER TABLE pick_results ADD COLUMN state TEXT NOT NULL DEFAULT 'final'
    CHECK (state IN ('provisional', 'final', 'void'));
ALTER TABLE pick_results ADD COLUMN pending_parts INTEGER NOT NULL DEFAULT 0
    CHECK (pending_parts >= 0);
