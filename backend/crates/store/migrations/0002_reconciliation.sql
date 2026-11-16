-- T03: field provenance and the unlinked queue.
-- Conventions as in 0001 and docs/adr/0003-database-schema.md.

-- Which source set each current value of a canonical entity. `field` is a
-- field group such as `schedule` or `series`, or a per-map group such as
-- `map_rounds.2`. A missing row means no source has a value.
CREATE TABLE field_sources (
    entity_type  TEXT NOT NULL CHECK (entity_type IN ('team', 'event', 'stage', 'match')),
    entity_id    INTEGER NOT NULL,
    field        TEXT NOT NULL CHECK (field <> ''),
    source       TEXT NOT NULL CHECK (source <> ''),
    updated_at   TEXT NOT NULL CHECK (updated_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
    PRIMARY KEY (entity_type, entity_id, field)
) STRICT;

-- External entities a non-primary source reported that could not be linked
-- to a canonical entity. Admins resolve them (T08); the payload stays in
-- `source_records`.
CREATE TABLE unlinked_refs (
    source         TEXT NOT NULL CHECK (source <> ''),
    entity_type    TEXT NOT NULL CHECK (entity_type IN ('team', 'event', 'stage', 'match')),
    external_id    TEXT NOT NULL CHECK (external_id <> ''),
    reason         TEXT NOT NULL CHECK (reason IN ('no_match', 'ambiguous', 'dependency')),
    -- Human-readable context, e.g. candidate IDs or the missing dependency.
    detail         TEXT,
    first_seen_at  TEXT NOT NULL CHECK (first_seen_at GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9]Z'),
    PRIMARY KEY (source, entity_type, external_id)
) STRICT;
