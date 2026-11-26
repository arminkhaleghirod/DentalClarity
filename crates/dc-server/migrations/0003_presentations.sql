-- Presentations, their generated sections, generation jobs, and monthly AI
-- spend for the demo budget guard.

-- `owner_scope` is the visitor session's token hash in demo mode and the
-- literal 'staff' in clinical mode (one shared account). Demo rows are purged
-- 24 hours after `created_at`.
-- `client_key` is a SHA-256 of the client address group, used only for the
-- demo per-address limit; empty in clinical mode.
CREATE TABLE presentations (
    id                   TEXT PRIMARY KEY NOT NULL,
    owner_scope          TEXT NOT NULL,
    client_key           TEXT NOT NULL DEFAULT '',
    patient_key          TEXT NOT NULL,
    patient_display_name TEXT NOT NULL,
    plan_key             TEXT NOT NULL,
    language             TEXT NOT NULL,
    reading_level        TEXT NOT NULL CHECK (reading_level IN ('simple', 'standard', 'detailed')),
    version              INTEGER NOT NULL DEFAULT 1,
    -- Unix seconds.
    created_at           INTEGER NOT NULL,
    snapshot_json        TEXT NOT NULL,
    fingerprint          TEXT NOT NULL
);

CREATE INDEX presentations_owner ON presentations (owner_scope, created_at);
CREATE INDEX presentations_client ON presentations (client_key, created_at);
CREATE INDEX presentations_created ON presentations (created_at);

-- One row per generated section of a presentation. `output_json` holds the
-- accepted model output, citations included; it is NULL for failed sections.
CREATE TABLE presentation_sections (
    presentation_id         TEXT NOT NULL REFERENCES presentations (id) ON DELETE CASCADE,
    key                     TEXT NOT NULL,
    position                INTEGER NOT NULL,
    state                   TEXT NOT NULL CHECK (state IN ('passed', 'needs_review', 'failed')),
    output_json             TEXT,
    reasons_json            TEXT NOT NULL DEFAULT '[]',
    needs_note_confirmation INTEGER NOT NULL DEFAULT 0,
    writer_model            TEXT NOT NULL,
    prompt_version          TEXT NOT NULL,
    attempts                INTEGER NOT NULL DEFAULT 0,
    attempt_log_json        TEXT NOT NULL DEFAULT '[]',
    input_tokens            INTEGER NOT NULL DEFAULT 0,
    output_tokens           INTEGER NOT NULL DEFAULT 0,
    cost_usd                REAL NOT NULL DEFAULT 0,
    elapsed_ms              INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (presentation_id, key)
);

-- Generation jobs. Times are Unix milliseconds. `progress_json` is the list
-- of `SectionProgress` shown on the create screen, updated while the job runs.
CREATE TABLE jobs (
    id              TEXT PRIMARY KEY NOT NULL,
    presentation_id TEXT NOT NULL REFERENCES presentations (id) ON DELETE CASCADE,
    state           TEXT NOT NULL CHECK (state IN ('queued', 'running', 'succeeded', 'needs_review', 'failed')),
    error           TEXT,
    created_at      INTEGER NOT NULL,
    started_at      INTEGER,
    finished_at     INTEGER,
    elapsed_ms      INTEGER NOT NULL DEFAULT 0,
    progress_json   TEXT NOT NULL DEFAULT '[]'
);

CREATE INDEX jobs_presentation ON jobs (presentation_id);
CREATE INDEX jobs_state ON jobs (state);

-- Model spend per UTC month (`YYYY-MM`), for the demo budget guard. Never
-- purged with visitor data.
CREATE TABLE ai_usage (
    month    TEXT PRIMARY KEY NOT NULL,
    cost_usd REAL NOT NULL DEFAULT 0,
    calls    INTEGER NOT NULL DEFAULT 0
);
