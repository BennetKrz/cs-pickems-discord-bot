-- T05: admin void of a match. Conventions as in 0001 and
-- docs/adr/0003-database-schema.md.

-- An admin voided the match: no points for anyone, picks are kept for
-- history. Only admin actions change it; the reconciler carries it over.
ALTER TABLE matches ADD COLUMN voided INTEGER NOT NULL DEFAULT 0 CHECK (voided IN (0, 1));
