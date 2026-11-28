-- "Ask a question" conversations (milestone M7). One row per question asked
-- in Present mode. Staff can read them per presentation; the patient is told
-- so before asking.
--
-- `present_token_hash` is the SHA-256 of the present session token that
-- asked. It is not a foreign key, because present sessions are deleted when
-- they end and the conversation must stay readable after that.
-- `answer_json` is the `QaAnswer` as shown, with `{{practice_name}}` and
-- `{{provider_display_name}}` still in place; they are filled in on read.
-- `outcome` is what the checks decided ("passed", "blocked: V8, V14",
-- "unreadable", "model_error", "no_model"). Raw provider requests and
-- replies are never stored. Rows go with their presentation (the demo purge
-- after 24 hours) and are otherwise deleted 30 days after `asked_at`
-- (Unix seconds).
CREATE TABLE qa_conversations (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    presentation_id    TEXT NOT NULL REFERENCES presentations (id) ON DELETE CASCADE,
    version            INTEGER NOT NULL,
    present_token_hash TEXT NOT NULL,
    asked_at           INTEGER NOT NULL,
    question           TEXT NOT NULL,
    answer_json        TEXT NOT NULL,
    kind               TEXT NOT NULL CHECK (kind IN ('answered', 'partial', 'out_of_scope', 'urgent', 'fallback')),
    urgent             INTEGER NOT NULL DEFAULT 0,
    outcome            TEXT NOT NULL,
    prompt_version     TEXT NOT NULL,
    model_id           TEXT NOT NULL,
    input_tokens       INTEGER NOT NULL DEFAULT 0,
    output_tokens      INTEGER NOT NULL DEFAULT 0,
    cost_usd           REAL NOT NULL DEFAULT 0
);

CREATE INDEX qa_conversations_presentation ON qa_conversations (presentation_id, asked_at);
CREATE INDEX qa_conversations_session ON qa_conversations (present_token_hash, asked_at);
CREATE INDEX qa_conversations_asked ON qa_conversations (asked_at);
