-- Speech spend per month, tracked apart from the total so narration can be
-- held to its own share of the demo budget. `cost_usd` and `calls` keep
-- counting every model call, speech included.
ALTER TABLE ai_usage ADD COLUMN speech_cost_usd REAL NOT NULL DEFAULT 0;
ALTER TABLE ai_usage ADD COLUMN speech_calls INTEGER NOT NULL DEFAULT 0;
