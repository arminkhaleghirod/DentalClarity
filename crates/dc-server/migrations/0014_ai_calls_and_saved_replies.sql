-- One row per model call, and the replies a running section has received.

-- Every text and speech call the server sends (docs/DECISIONS.md E23). No
-- prompt, reply or patient text is stored. `presentation_id`, `job_id` and
-- `section_key` are set for generation calls and are plain text, not foreign
-- keys, so the rows outlive purged demo presentations as spend records.
-- `stage` is `write`, `repair`, `review` or `speech`. `round` is the
-- section's attempt and `attempt` the transport attempt inside one call.
-- `cost_source` says where `cost_usd` came from: the provider's reported
-- cost, the dated price table, the placeholder rates for a model the table
-- does not list, or the worst-case reservation for a call that timed out or
-- came back unusable. Times are Unix milliseconds; money is US dollars.
CREATE TABLE ai_calls (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    created_at        INTEGER NOT NULL,
    presentation_id   TEXT,
    job_id            TEXT,
    section_key       TEXT,
    stage             TEXT NOT NULL CHECK (stage IN ('write', 'repair', 'review', 'speech', 'other')),
    round             INTEGER NOT NULL DEFAULT 0,
    attempt           INTEGER NOT NULL DEFAULT 1,
    provider          TEXT NOT NULL,
    model_requested   TEXT NOT NULL,
    model_returned    TEXT,
    provider_route    TEXT,
    prompt_version    TEXT,
    input_tokens      INTEGER NOT NULL DEFAULT 0,
    output_tokens     INTEGER NOT NULL DEFAULT 0,
    audio_chars       INTEGER,
    latency_ms        INTEGER NOT NULL,
    outcome           TEXT NOT NULL CHECK (outcome IN ('ok', 'unusable', 'failed')),
    cost_usd          REAL NOT NULL DEFAULT 0,
    cost_source       TEXT NOT NULL CHECK (cost_source IN ('reported', 'price_table', 'fallback_rate', 'reservation', 'none')),
    reported_cost_usd REAL,
    est_cost_usd      REAL
);

CREATE INDEX ai_calls_job ON ai_calls (job_id);
CREATE INDEX ai_calls_created ON ai_calls (created_at);

-- Replies a section of a running job has received, in order (`seq` from 0),
-- so a restart replays them instead of paying for them again (E5). `content`
-- is the model's output, the same kind of text as
-- `presentation_sections.output_json`; prompts are never stored. Rows are
-- deleted when the job finishes or fails.
CREATE TABLE job_saved_replies (
    job_id         TEXT NOT NULL REFERENCES jobs (id) ON DELETE CASCADE,
    section_key    TEXT NOT NULL,
    seq            INTEGER NOT NULL,
    stage          TEXT NOT NULL CHECK (stage IN ('write', 'repair', 'review')),
    round          INTEGER NOT NULL,
    prompt_version TEXT NOT NULL,
    model          TEXT NOT NULL,
    content        TEXT NOT NULL,
    input_tokens   INTEGER NOT NULL DEFAULT 0,
    output_tokens  INTEGER NOT NULL DEFAULT 0,
    cost_usd       REAL,
    PRIMARY KEY (job_id, section_key, seq)
);
