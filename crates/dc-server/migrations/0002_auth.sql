-- Shared staff account (one row) and server-side sessions.

CREATE TABLE staff_account (
    id                  INTEGER PRIMARY KEY CHECK (id = 1),
    password_hash       TEXT NOT NULL,
    pin_hash            TEXT NOT NULL,
    password_changed_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

-- `kind` is 'staff' (clinical login) or 'visitor' (demo mode, no login).
-- Session tokens are stored only as SHA-256 hashes. The CSRF token is useless
-- without the session token, so it is stored as issued.
CREATE TABLE sessions (
    token_hash   TEXT PRIMARY KEY NOT NULL,
    kind         TEXT NOT NULL CHECK (kind IN ('staff', 'visitor')),
    csrf_token   TEXT NOT NULL,
    locked       INTEGER NOT NULL DEFAULT 0,
    created_at   INTEGER NOT NULL,
    last_seen_at INTEGER NOT NULL,
    expires_at   INTEGER NOT NULL
);

CREATE INDEX sessions_expires_at ON sessions (expires_at);
