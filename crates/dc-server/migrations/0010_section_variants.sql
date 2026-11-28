-- Language and reading level variants of a presentation's sections
-- (docs/DECISIONS.md E32). Each row is one section of one presentation
-- version, derived from that section's approved English text for one
-- language and level. `source_hash` covers the English text (staff edits and
-- note confirmations included), the target, and the prompts and models, so a
-- change to any of them gives a new row and the old one is never shown.
--
-- `state` is 'pending' while a derivation runs, 'ready' when the text passed
-- the checks and the review, and 'failed' when it did not; a failed row is
-- final for its hash. A derivation that could not reach a model deletes its
-- row, so a later request tries again. `output_json` is the derived
-- `VariantText` with placeholders unfilled; NULL unless ready.
-- `updated_at` is in Unix milliseconds; a pending row older than ten minutes
-- belongs to a derivation that did not finish and may be taken over.
CREATE TABLE section_variants (
    presentation_id TEXT NOT NULL REFERENCES presentations (id) ON DELETE CASCADE,
    version         INTEGER NOT NULL,
    section_key     TEXT NOT NULL,
    language        TEXT NOT NULL,
    reading_level   TEXT NOT NULL CHECK (reading_level IN ('simple', 'standard', 'detailed')),
    source_hash     TEXT NOT NULL,
    state           TEXT NOT NULL CHECK (state IN ('pending', 'ready', 'failed')),
    output_json     TEXT,
    notes_json      TEXT NOT NULL DEFAULT '[]',
    translator_model TEXT NOT NULL DEFAULT '',
    prompt_version  TEXT NOT NULL DEFAULT '',
    attempts        INTEGER NOT NULL DEFAULT 0,
    input_tokens    INTEGER NOT NULL DEFAULT 0,
    output_tokens   INTEGER NOT NULL DEFAULT 0,
    cost_usd        REAL NOT NULL DEFAULT 0,
    updated_at      INTEGER NOT NULL,
    PRIMARY KEY (presentation_id, version, section_key, language, reading_level, source_hash)
);

-- One row per variant whose derivation started in demo mode, for the demo
-- variant limits. Like `regeneration_events`, rows carry their own time
-- (milliseconds) and are purged by it.
CREATE TABLE variant_events (
    owner_scope TEXT NOT NULL,
    client_key  TEXT NOT NULL,
    created_at  INTEGER NOT NULL
);

CREATE INDEX variant_events_scope ON variant_events (owner_scope, created_at);
CREATE INDEX variant_events_client ON variant_events (client_key, created_at);
