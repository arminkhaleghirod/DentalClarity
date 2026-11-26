# Knowledge base

Plain-language educational content the generation pipeline draws from. The model may only state what these passages, the plan facts and a linked clinical note support, and every generated sentence cites its source.

## Files

- `codes.toml` maps procedure codes to our categories and patient-facing names.
- `entries/<category>.toml` holds one entry per procedure category.

## Entry format

```toml
id = "root_canal"                # equals the category id
version = 1                      # bump on any text change
title = "Root canal treatment"
review_status = "unreviewed"     # "unreviewed" or "dentist_reviewed"
reviewed_by = ""                 # reviewer code (e.g. "R1"), never a name without consent
updated = "2026-11-26"

[[sources]]
id = "aae_what_is_rct"           # [a-z0-9_]+, unique in the entry
publisher = "American Association of Endodontists"
title = "What is a root canal?"
url = "https://..."              # https only, checked when written
accessed = "2026-11-26"

[[passages]]
id = "what_it_is"                # [a-z0-9_]+, unique in the entry
topic = "what_it_is"
text = "..."
sources = ["aae_what_is_rct"]
```

Every entry has exactly one passage for each topic:

| Topic | Content |
|---|---|
| `what_it_is` | What the procedure is, in plain words |
| `common_reasons` | General reasons it is done. Never phrased as this patient's reason |
| `what_happens` | What typically happens during the visit |
| `recovery` | What recovery commonly looks like, framed as typical |
| `benefits` | What the procedure is meant to achieve |
| `if_delayed` | What can happen if it is put off, calm and balanced |
| `questions` | Questions a patient might ask their dental team |

## Writing rules

- Original wording. Do not copy or closely paraphrase a source; read it, then write it your own way.
- About an 8th-grade reading level, 2 to 4 sentences per passage, second person where natural.
- No prices, visit counts, durations, success percentages, brand names, medication names or doses.
- No guarantees, pressure or fear. Use "often", "usually" and "many people".
- Cite only pages you have opened and that support the passage. Prefer professional associations, government health sites and academic medical centers.
- New entries start as `unreviewed`. Only a dentist's review changes that, and the reviewer is recorded by code.
