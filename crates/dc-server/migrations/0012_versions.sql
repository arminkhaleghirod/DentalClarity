-- Version history and plan sync (milestone M8).

-- One row per version of a presentation. The current version's sections
-- live in `presentation_sections`, so `sections_json` is NULL for it; when
-- a version is superseded, its section rows are frozen here as a JSON array
-- (the columns of `presentation_sections` that the views read). Staff edits
-- and note confirmations are already stored per version. Times are Unix
-- seconds. `changes_json` lists staff-readable changes from the previous
-- version, e.g. "Cost of Crown on tooth 30 changed".
CREATE TABLE presentation_versions (
    presentation_id TEXT NOT NULL REFERENCES presentations (id) ON DELETE CASCADE,
    version         INTEGER NOT NULL,
    cause           TEXT NOT NULL CHECK (cause IN ('created', 'plan_changed', 'regenerated')),
    created_at      INTEGER NOT NULL,
    snapshot_json   TEXT NOT NULL,
    fingerprint     TEXT NOT NULL,
    changes_json    TEXT NOT NULL DEFAULT '[]',
    sections_json   TEXT,
    presented_at    INTEGER,
    exported_at     INTEGER,
    PRIMARY KEY (presentation_id, version)
);

INSERT INTO presentation_versions (presentation_id, version, cause, created_at, snapshot_json,
    fingerprint)
SELECT id, version, 'created', created_at, snapshot_json, fingerprint FROM presentations;

-- What the last plan poll found: `ok`, `plan_deleted` (the plan is gone
-- from the practice records) or `source_unavailable` (they could not be
-- read). `checked_at` is the poll time in Unix seconds.
ALTER TABLE presentations ADD COLUMN source_state TEXT NOT NULL DEFAULT 'ok'
    CHECK (source_state IN ('ok', 'plan_deleted', 'source_unavailable'));
ALTER TABLE presentations ADD COLUMN checked_at INTEGER;
