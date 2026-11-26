-- One row per single-section regeneration started in demo mode, for the
-- demo regeneration limits. The limits used to count `jobs` joined to
-- `presentations`, but those rows are purged with the presentation, 24 hours
-- after it was created, so a regeneration started late in that window
-- stopped counting soon after. These rows carry their own time and are
-- purged by it.
--
-- `owner_scope` is the visitor scope, moved with the presentations when the
-- visitor's token rotates. `client_key` is the address hash, as in
-- `jobs.client_key`. `created_at` is in milliseconds, like `jobs.created_at`.
CREATE TABLE regeneration_events (
    owner_scope TEXT NOT NULL,
    client_key  TEXT NOT NULL,
    created_at  INTEGER NOT NULL
);

CREATE INDEX regeneration_events_scope ON regeneration_events (owner_scope, created_at);
CREATE INDEX regeneration_events_client ON regeneration_events (client_key, created_at);

-- Regenerations that are still counted when this migration runs.
INSERT INTO regeneration_events (owner_scope, client_key, created_at)
SELECT p.owner_scope, j.client_key, j.created_at
FROM jobs j JOIN presentations p ON p.id = j.presentation_id
WHERE j.section_key IS NOT NULL AND p.owner_scope <> 'staff';
