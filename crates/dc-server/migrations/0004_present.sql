-- Present sessions, staff edits and note confirmations (milestone M5).

-- A present session lets the patient screens read one presentation version.
-- It is bound to the staff (or demo visitor) session that started it, which
-- stays locked until exit. Tokens are stored only as SHA-256 hashes. Times
-- are Unix seconds. `pin_attempts` counts PIN tries for this lock; after
-- five, only the staff password unlocks.
CREATE TABLE present_sessions (
    token_hash       TEXT PRIMARY KEY NOT NULL,
    presentation_id  TEXT NOT NULL REFERENCES presentations (id) ON DELETE CASCADE,
    version          INTEGER NOT NULL,
    staff_token_hash TEXT NOT NULL REFERENCES sessions (token_hash) ON DELETE CASCADE,
    pin_attempts     INTEGER NOT NULL DEFAULT 0,
    created_at       INTEGER NOT NULL,
    last_seen_at     INTEGER NOT NULL,
    expires_at       INTEGER NOT NULL
);

CREATE INDEX present_sessions_staff ON present_sessions (staff_token_hash);
CREATE INDEX present_sessions_expires_at ON present_sessions (expires_at);

-- Staff text that replaces one generated block. `original_json` keeps the
-- generated sentences as they were when the block was first edited, so the
-- edit can be reverted and compared.
CREATE TABLE staff_edits (
    presentation_id TEXT NOT NULL REFERENCES presentations (id) ON DELETE CASCADE,
    version         INTEGER NOT NULL,
    section_key     TEXT NOT NULL,
    field           TEXT NOT NULL,
    sentences_json  TEXT NOT NULL,
    original_json   TEXT NOT NULL,
    updated_at      INTEGER NOT NULL,
    PRIMARY KEY (presentation_id, version, section_key, field)
);

-- One row per note-derived block that staff confirmed for patients (P11).
CREATE TABLE note_confirmations (
    presentation_id TEXT NOT NULL REFERENCES presentations (id) ON DELETE CASCADE,
    version         INTEGER NOT NULL,
    section_key     TEXT NOT NULL,
    field           TEXT NOT NULL,
    confirmed_at    INTEGER NOT NULL,
    PRIMARY KEY (presentation_id, version, section_key, field)
);
