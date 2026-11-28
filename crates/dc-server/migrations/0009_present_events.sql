-- In-office engagement analytics (milestone M8). One row per report from
-- the patient screens while Present mode runs, plus one `open` row when
-- staff start Present mode. Rows hold a screen name, a duration and a
-- narration count only: never patient text.
--
-- `session` is the present session's token hash, so rows of one showing can
-- be grouped after the present session itself has ended and been deleted.
-- Rows go with their presentation, so the 24-hour demo purge removes them.
-- `at` is in milliseconds, like `jobs.created_at`.
CREATE TABLE present_events (
    presentation_id TEXT NOT NULL REFERENCES presentations (id) ON DELETE CASCADE,
    version         INTEGER NOT NULL,
    session         TEXT NOT NULL,
    kind            TEXT NOT NULL CHECK (kind IN ('open', 'view', 'narration')),
    screen          TEXT CHECK (screen IN ('at_a_glance', 'journey', 'procedures', 'costs', 'next_steps')),
    seconds         INTEGER NOT NULL DEFAULT 0 CHECK (seconds >= 0),
    at              INTEGER NOT NULL,
    CHECK ((kind = 'open') = (screen IS NULL))
);

CREATE INDEX present_events_presentation ON present_events (presentation_id, at);
