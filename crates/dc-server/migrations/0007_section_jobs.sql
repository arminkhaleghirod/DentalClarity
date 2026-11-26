-- Single-section regeneration and per-section job results.

-- `section_key` is NULL for a job that writes the whole presentation and
-- names one section (`procedure:p3`) for a regeneration job. `client_key`
-- is the same address hash as `presentations.client_key`, taken from the
-- request that started the job, for the demo per-address regeneration limit.
ALTER TABLE jobs ADD COLUMN section_key TEXT;
ALTER TABLE jobs ADD COLUMN client_key TEXT NOT NULL DEFAULT '';

CREATE INDEX jobs_client ON jobs (client_key, created_at);

-- Each section result is saved here as soon as it finishes, so a job that
-- restarts skips the sections it already paid for. When the job finishes,
-- its rows move to `presentation_sections` in one transaction and are
-- deleted here. Columns match `presentation_sections`.
CREATE TABLE job_section_results (
    job_id                  TEXT NOT NULL REFERENCES jobs (id) ON DELETE CASCADE,
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
    PRIMARY KEY (job_id, key)
);
