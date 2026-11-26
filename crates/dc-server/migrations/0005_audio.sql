-- Narration clips, one per sentence, addressed by `dc_core::present::audio_key`
-- (SHA-256 over the speech model and voice, the language and the exact text).
-- The audio itself is stored at `<data_dir>/artifacts/audio/<key>.<ext>`.
-- Clips hold only approved, placeholder-substituted sentence text read aloud,
-- and are shared across presentations that use the same sentence.
-- Times are Unix seconds.
CREATE TABLE audio_clips (
    key          TEXT PRIMARY KEY NOT NULL,
    language     TEXT NOT NULL,
    model        TEXT NOT NULL,
    bytes        INTEGER NOT NULL,
    mime         TEXT NOT NULL,
    created_at   INTEGER NOT NULL,
    last_used_at INTEGER NOT NULL
);

CREATE INDEX audio_clips_last_used ON audio_clips (last_used_at);
