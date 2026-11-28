-- The practice logo shown in the staff shell and the patient header. One row
-- at most. The bytes passed the content checks in `logo.rs` before they were
-- stored, and `content_type` is the type those checks detected, never the
-- type the browser sent. `sha256` is the hex digest of `bytes` and versions
-- the logo URLs, so a new upload is fetched again.
CREATE TABLE practice_logo (
    id           INTEGER PRIMARY KEY CHECK (id = 1),
    content_type TEXT NOT NULL
                 CHECK (content_type IN ('image/png', 'image/jpeg', 'image/svg+xml')),
    bytes        BLOB NOT NULL,
    sha256       TEXT NOT NULL,
    updated_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
