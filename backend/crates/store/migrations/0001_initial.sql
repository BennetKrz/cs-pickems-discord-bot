-- T02: canonical schema. Conventions are in docs/adr/0003-database-schema.md.
--
-- * All tables are STRICT. Internal IDs are INTEGER PRIMARY KEY; Discord
--   snowflakes are TEXT of digits.
-- * Timestamps are UTC TEXT in one fixed-width form, 'YYYY-MM-DDTHH:MM:SS.sssZ',
--   so string comparison equals time comparison. Every timestamp column checks
--   this with GLOB.
-- * Enum columns are checked against the string forms in `domain::model`.
-- * Scores are relative to the match's team A/B, fixed at creation.
-- * Never edit this file once released; add a new migration instead.

-- Canonical tournament data --------------------------------------------------

CREATE TABLE teams (
    id        INTEGER PRIMARY KEY,
    name      TEXT NOT NULL CHECK (name <> ''),
    acronym   TEXT,
    logo_url  TEXT,
    country   TEXT
) STRICT;

CREATE TABLE events (
    id        INTEGER PRIMARY KEY,
    name      TEXT NOT NULL CHECK (name <> ''),
    tier      TEXT NOT NULL DEFAULT 'unknown'
              CHECK (tier IN ('s', 'a', 'b', 'c', 'd', 'unknown')),
    begin_at  TEXT CHECK (begin_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
    end_at    TEXT CHECK (end_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
    status    TEXT NOT NULL CHECK (status IN ('upcoming', 'running', 'finished', 'canceled'))
) STRICT;

CREATE TABLE stages (
    id        INTEGER PRIMARY KEY,
    event_id  INTEGER NOT NULL REFERENCES events (id),
    name      TEXT NOT NULL CHECK (name <> ''),
    kind      TEXT NOT NULL
              CHECK (kind IN ('swiss', 'groups', 'single_elim', 'double_elim', 'other')),
    -- Sort position within the event. Named `position` because `order` is
    -- an SQL keyword.
    position  INTEGER NOT NULL DEFAULT 0
) STRICT;

CREATE INDEX stages_event ON stages (event_id, position);

CREATE TABLE matches (
    id              INTEGER PRIMARY KEY,
    event_id        INTEGER NOT NULL REFERENCES events (id),
    stage_id        INTEGER REFERENCES stages (id),
    -- NULL while the team is TBD.
    team_a_id       INTEGER REFERENCES teams (id),
    team_b_id       INTEGER REFERENCES teams (id),
    format          TEXT NOT NULL CHECK (format IN ('bo1', 'bo3', 'bo5')),
    status          TEXT NOT NULL DEFAULT 'scheduled'
                    CHECK (status IN ('scheduled', 'live', 'finished', 'canceled', 'postponed')),
    scheduled_at    TEXT CHECK (scheduled_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
    started_at      TEXT CHECK (started_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
    ended_at        TEXT CHECK (ended_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
    lock_at         TEXT CHECK (lock_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
    locked          INTEGER NOT NULL DEFAULT 0 CHECK (locked IN (0, 1)),
    maps_won_a      INTEGER NOT NULL DEFAULT 0,
    maps_won_b      INTEGER NOT NULL DEFAULT 0,
    winner_team_id  INTEGER REFERENCES teams (id),
    bracket_side    TEXT CHECK (bracket_side IN ('upper', 'lower', 'final')),
    round_index     INTEGER CHECK (round_index >= 0),
    slot_index      INTEGER CHECK (slot_index >= 0),
    feeds_a_from_match_id  INTEGER REFERENCES matches (id),
    feeds_a_from_outcome   TEXT CHECK (feeds_a_from_outcome IN ('winner', 'loser')),
    feeds_b_from_match_id  INTEGER REFERENCES matches (id),
    feeds_b_from_outcome   TEXT CHECK (feeds_b_from_outcome IN ('winner', 'loser')),

    CHECK (team_a_id IS NULL OR team_b_id IS NULL OR team_a_id <> team_b_id),
    -- `IS` instead of `=`, so a NULL (TBD) team never lets a winner through.
    CHECK (winner_team_id IS NULL
           OR winner_team_id IS team_a_id
           OR winner_team_id IS team_b_id),
    CHECK (maps_won_a BETWEEN 0 AND CASE format WHEN 'bo1' THEN 1 WHEN 'bo3' THEN 2 ELSE 3 END),
    CHECK (maps_won_b BETWEEN 0 AND CASE format WHEN 'bo1' THEN 1 WHEN 'bo3' THEN 2 ELSE 3 END),
    CHECK ((feeds_a_from_match_id IS NULL) = (feeds_a_from_outcome IS NULL)),
    CHECK ((feeds_b_from_match_id IS NULL) = (feeds_b_from_outcome IS NULL))
) STRICT;

CREATE INDEX matches_event ON matches (event_id, scheduled_at);
CREATE INDEX matches_stage ON matches (stage_id);
CREATE INDEX matches_scheduled ON matches (scheduled_at);
-- The lock ticker (T06) looks for unlocked matches whose lock_at has passed.
CREATE INDEX matches_lock_due ON matches (lock_at) WHERE locked = 0;

CREATE TABLE maps (
    match_id        INTEGER NOT NULL REFERENCES matches (id) ON DELETE CASCADE,
    number          INTEGER NOT NULL CHECK (number BETWEEN 1 AND 5),
    -- NULL until the veto is known.
    map_name        TEXT,
    status          TEXT NOT NULL DEFAULT 'upcoming'
                    CHECK (status IN ('upcoming', 'live', 'finished', 'not_played')),
    rounds_a        INTEGER CHECK (rounds_a >= 0),
    rounds_b        INTEGER CHECK (rounds_b >= 0),
    winner_team_id  INTEGER REFERENCES teams (id),
    PRIMARY KEY (match_id, number),
    CHECK ((rounds_a IS NULL) = (rounds_b IS NULL))
) STRICT;

-- Source data -----------------------------------------------------------------

CREATE TABLE external_refs (
    entity_type  TEXT NOT NULL CHECK (entity_type IN ('team', 'event', 'stage', 'match')),
    entity_id    INTEGER NOT NULL,
    source       TEXT NOT NULL CHECK (source <> ''),
    external_id  TEXT NOT NULL CHECK (external_id <> ''),
    PRIMARY KEY (source, entity_type, external_id)
) STRICT;

CREATE INDEX external_refs_entity ON external_refs (entity_type, entity_id);

-- Latest raw payload per external entity, for replay without API calls.
CREATE TABLE source_records (
    source        TEXT NOT NULL CHECK (source <> ''),
    entity_type   TEXT NOT NULL CHECK (entity_type IN ('team', 'event', 'stage', 'match')),
    external_id   TEXT NOT NULL CHECK (external_id <> ''),
    payload       TEXT NOT NULL CHECK (json_valid(payload)),
    payload_hash  TEXT NOT NULL CHECK (payload_hash <> ''),
    fetched_at    TEXT NOT NULL CHECK (fetched_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
    PRIMARY KEY (source, entity_type, external_id)
) STRICT;

-- Discord users ---------------------------------------------------------------

CREATE TABLE users (
    discord_user_id  TEXT PRIMARY KEY
                     CHECK (discord_user_id <> '' AND discord_user_id NOT GLOB '*[^0-9]*'),
    username         TEXT NOT NULL CHECK (username <> ''),
    global_name      TEXT,
    avatar_hash      TEXT,
    updated_at       TEXT NOT NULL CHECK (updated_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z')
) STRICT;

CREATE TABLE guild_members (
    guild_id      TEXT NOT NULL CHECK (guild_id <> '' AND guild_id NOT GLOB '*[^0-9]*'),
    user_id       TEXT NOT NULL REFERENCES users (discord_user_id) ON DELETE CASCADE,
    nick          TEXT,
    last_seen_at  TEXT NOT NULL CHECK (last_seen_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
    PRIMARY KEY (guild_id, user_id)
) STRICT;

-- Picks -----------------------------------------------------------------------

CREATE TABLE picks (
    id              INTEGER PRIMARY KEY,
    guild_id        TEXT NOT NULL CHECK (guild_id <> '' AND guild_id NOT GLOB '*[^0-9]*'),
    user_id         TEXT NOT NULL REFERENCES users (discord_user_id),
    match_id        INTEGER NOT NULL REFERENCES matches (id),
    winner_team_id  INTEGER REFERENCES teams (id),
    series_a        INTEGER CHECK (series_a BETWEEN 0 AND 3),
    series_b        INTEGER CHECK (series_b BETWEEN 0 AND 3),
    created_at      TEXT NOT NULL CHECK (created_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
    updated_at      TEXT NOT NULL CHECK (updated_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
    UNIQUE (guild_id, user_id, match_id),
    CHECK ((series_a IS NULL) = (series_b IS NULL))
) STRICT;

CREATE INDEX picks_match ON picks (match_id);

CREATE TABLE pick_maps (
    pick_id         INTEGER NOT NULL REFERENCES picks (id) ON DELETE CASCADE,
    map_number      INTEGER NOT NULL CHECK (map_number BETWEEN 1 AND 5),
    winner_team_id  INTEGER NOT NULL REFERENCES teams (id),
    rounds_a        INTEGER CHECK (rounds_a >= 0),
    rounds_b        INTEGER CHECK (rounds_b >= 0),
    overtime        INTEGER NOT NULL DEFAULT 0 CHECK (overtime IN (0, 1)),
    PRIMARY KEY (pick_id, map_number),
    CHECK ((rounds_a IS NULL) = (rounds_b IS NULL)),
    CHECK (overtime = 0 OR rounds_a IS NULL)
) STRICT;

-- One row per save, for the edit history.
CREATE TABLE pick_revisions (
    id         INTEGER PRIMARY KEY,
    pick_id    INTEGER NOT NULL REFERENCES picks (id) ON DELETE CASCADE,
    snapshot   TEXT NOT NULL CHECK (json_valid(snapshot)),
    edited_at  TEXT NOT NULL CHECK (edited_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z')
) STRICT;

CREATE INDEX pick_revisions_pick ON pick_revisions (pick_id, id);

CREATE TABLE pick_results (
    pick_id        INTEGER PRIMARY KEY REFERENCES picks (id) ON DELETE CASCADE,
    points         INTEGER NOT NULL,
    breakdown      TEXT NOT NULL CHECK (json_valid(breakdown)),
    rules_version  INTEGER NOT NULL CHECK (rules_version >= 1),
    settled_at     TEXT NOT NULL CHECK (settled_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z')
) STRICT;

-- Guild configuration ---------------------------------------------------------

CREATE TABLE guild_settings (
    guild_id               TEXT PRIMARY KEY
                           CHECK (guild_id <> '' AND guild_id NOT GLOB '*[^0-9]*'),
    announce_channel_id    TEXT
                           CHECK (announce_channel_id <> '' AND announce_channel_id NOT GLOB '*[^0-9]*'),
    admin_role_id          TEXT
                           CHECK (admin_role_id <> '' AND admin_role_id NOT GLOB '*[^0-9]*'),
    -- NULL means the default rules (T11).
    scoring_rules          TEXT CHECK (json_valid(scoring_rules)),
    scoring_rules_version  INTEGER NOT NULL DEFAULT 1 CHECK (scoring_rules_version >= 1),
    updated_at             TEXT NOT NULL CHECK (updated_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z')
) STRICT;

CREATE TABLE followed_events (
    guild_id     TEXT NOT NULL CHECK (guild_id <> '' AND guild_id NOT GLOB '*[^0-9]*'),
    event_id     INTEGER NOT NULL REFERENCES events (id),
    followed_by  TEXT NOT NULL CHECK (followed_by <> '' AND followed_by NOT GLOB '*[^0-9]*'),
    followed_at  TEXT NOT NULL CHECK (followed_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
    PRIMARY KEY (guild_id, event_id)
) STRICT;

CREATE INDEX followed_events_event ON followed_events (event_id);

-- Operations ------------------------------------------------------------------

CREATE TABLE audit_log (
    id           INTEGER PRIMARY KEY,
    -- A Discord user ID, or a component name such as `sync`.
    actor        TEXT NOT NULL CHECK (actor <> ''),
    action       TEXT NOT NULL CHECK (action <> ''),
    entity_type  TEXT NOT NULL CHECK (entity_type <> ''),
    entity_id    TEXT,
    before       TEXT CHECK (json_valid(before)),
    after        TEXT CHECK (json_valid(after)),
    at           TEXT NOT NULL CHECK (at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z')
) STRICT;

CREATE INDEX audit_log_entity ON audit_log (entity_type, entity_id, id);

-- Per job and source; the sync scheduler (T06) defines the jobs.
CREATE TABLE sync_state (
    job                TEXT NOT NULL CHECK (job <> ''),
    source             TEXT NOT NULL CHECK (source <> ''),
    cursor             TEXT,
    last_run_at        TEXT CHECK (last_run_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
    last_success_at    TEXT CHECK (last_success_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
    last_error         TEXT,
    request_count      INTEGER NOT NULL DEFAULT 0 CHECK (request_count >= 0),
    window_started_at  TEXT CHECK (window_started_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
    remaining_quota    INTEGER,
    updated_at         TEXT NOT NULL CHECK (updated_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
    PRIMARY KEY (job, source)
) STRICT;

-- Cross-table rules (CHECK cannot read other tables) --------------------------
--
-- Max map number per format: bo1 → 1, bo3 → 3, bo5 → 5.

CREATE TRIGGER maps_check_insert BEFORE INSERT ON maps
BEGIN
    SELECT RAISE(ABORT, 'map number exceeds match format')
    WHERE NEW.number > (SELECT CASE format WHEN 'bo1' THEN 1 WHEN 'bo3' THEN 3 ELSE 5 END
                        FROM matches WHERE id = NEW.match_id);
    SELECT RAISE(ABORT, 'map winner is not a team of the match')
    WHERE NEW.winner_team_id IS NOT NULL
      AND EXISTS (SELECT 1 FROM matches WHERE id = NEW.match_id
                  AND NOT (team_a_id IS NEW.winner_team_id OR team_b_id IS NEW.winner_team_id));
END;

CREATE TRIGGER maps_check_update BEFORE UPDATE OF match_id, number, winner_team_id ON maps
BEGIN
    SELECT RAISE(ABORT, 'map number exceeds match format')
    WHERE NEW.number > (SELECT CASE format WHEN 'bo1' THEN 1 WHEN 'bo3' THEN 3 ELSE 5 END
                        FROM matches WHERE id = NEW.match_id);
    SELECT RAISE(ABORT, 'map winner is not a team of the match')
    WHERE NEW.winner_team_id IS NOT NULL
      AND EXISTS (SELECT 1 FROM matches WHERE id = NEW.match_id
                  AND NOT (team_a_id IS NEW.winner_team_id OR team_b_id IS NEW.winner_team_id));
END;

CREATE TRIGGER picks_check_insert BEFORE INSERT ON picks
BEGIN
    SELECT RAISE(ABORT, 'pick winner is not a team of the match')
    WHERE NEW.winner_team_id IS NOT NULL
      AND EXISTS (SELECT 1 FROM matches WHERE id = NEW.match_id
                  AND NOT (team_a_id IS NEW.winner_team_id OR team_b_id IS NEW.winner_team_id));
END;

CREATE TRIGGER picks_check_update BEFORE UPDATE OF match_id, winner_team_id ON picks
BEGIN
    SELECT RAISE(ABORT, 'pick winner is not a team of the match')
    WHERE NEW.winner_team_id IS NOT NULL
      AND EXISTS (SELECT 1 FROM matches WHERE id = NEW.match_id
                  AND NOT (team_a_id IS NEW.winner_team_id OR team_b_id IS NEW.winner_team_id));
END;

CREATE TRIGGER pick_maps_check_insert BEFORE INSERT ON pick_maps
BEGIN
    SELECT RAISE(ABORT, 'map number exceeds match format')
    WHERE NEW.map_number > (SELECT CASE m.format WHEN 'bo1' THEN 1 WHEN 'bo3' THEN 3 ELSE 5 END
                            FROM picks p JOIN matches m ON m.id = p.match_id
                            WHERE p.id = NEW.pick_id);
    SELECT RAISE(ABORT, 'map pick winner is not a team of the match')
    WHERE EXISTS (SELECT 1 FROM picks p JOIN matches m ON m.id = p.match_id
                  WHERE p.id = NEW.pick_id
                  AND NOT (m.team_a_id IS NEW.winner_team_id OR m.team_b_id IS NEW.winner_team_id));
END;

CREATE TRIGGER pick_maps_check_update
BEFORE UPDATE OF pick_id, map_number, winner_team_id ON pick_maps
BEGIN
    SELECT RAISE(ABORT, 'map number exceeds match format')
    WHERE NEW.map_number > (SELECT CASE m.format WHEN 'bo1' THEN 1 WHEN 'bo3' THEN 3 ELSE 5 END
                            FROM picks p JOIN matches m ON m.id = p.match_id
                            WHERE p.id = NEW.pick_id);
    SELECT RAISE(ABORT, 'map pick winner is not a team of the match')
    WHERE EXISTS (SELECT 1 FROM picks p JOIN matches m ON m.id = p.match_id
                  WHERE p.id = NEW.pick_id
                  AND NOT (m.team_a_id IS NEW.winner_team_id OR m.team_b_id IS NEW.winner_team_id));
END;

-- A match can't shrink below maps or map picks it already has, and can't
-- swap out a team that existing results or picks name as winner.
CREATE TRIGGER matches_check_update
BEFORE UPDATE OF format, team_a_id, team_b_id ON matches
BEGIN
    SELECT RAISE(ABORT, 'match format is shorter than its existing maps')
    WHERE EXISTS (SELECT 1 FROM maps
                  WHERE match_id = NEW.id
                  AND number > CASE NEW.format WHEN 'bo1' THEN 1 WHEN 'bo3' THEN 3 ELSE 5 END)
       OR EXISTS (SELECT 1 FROM pick_maps pm JOIN picks p ON p.id = pm.pick_id
                  WHERE p.match_id = NEW.id
                  AND pm.map_number > CASE NEW.format WHEN 'bo1' THEN 1 WHEN 'bo3' THEN 3 ELSE 5 END);
    SELECT RAISE(ABORT, 'a team named as winner by a map or pick cannot be removed from the match')
    WHERE EXISTS (SELECT 1 FROM maps
                  WHERE match_id = NEW.id AND winner_team_id IS NOT NULL
                  AND NOT (winner_team_id IS NEW.team_a_id OR winner_team_id IS NEW.team_b_id))
       OR EXISTS (SELECT 1 FROM picks
                  WHERE match_id = NEW.id AND winner_team_id IS NOT NULL
                  AND NOT (winner_team_id IS NEW.team_a_id OR winner_team_id IS NEW.team_b_id))
       OR EXISTS (SELECT 1 FROM pick_maps pm JOIN picks p ON p.id = pm.pick_id
                  WHERE p.match_id = NEW.id
                  AND NOT (pm.winner_team_id IS NEW.team_a_id OR pm.winner_team_id IS NEW.team_b_id));
END;
