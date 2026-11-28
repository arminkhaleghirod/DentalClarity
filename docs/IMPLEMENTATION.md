# Implementation plan

Milestones run from 2026-11-26 to the submission deadline on 2027-02-15. Dates are targets and some milestones overlap. The order puts a complete demo workflow ahead of polish, as decided in [DECISIONS.md](DECISIONS.md) (P3). ClarityEval has its own timeline in [CLARITYEVAL.md](CLARITYEVAL.md); its preliminary model check feeds M4 and its final runs land in M10.

| Milestone | Scope | Target |
|---|---|---|
| M0 | Foundations: workspace, planning docs, synthetic connector, dashboard | 2026-11-26 (done) |
| M1 | Staff session, demo visitor isolation, security headers and CSP | 2026-11-26 (done) |
| M2 | Open Dental connector and schema-matched synthetic MySQL | 2026-12-10 |
| M3 | Knowledge base entries, patient directory, settings and setup wizard | 2026-12-14 |
| M4 | Generation pipeline, validators, review pass, persistent jobs | 2026-12-22 |
| M5 | Editor, Present mode, patient screens, narration | 2026-11-26 (done) |
| M6 | Odontogram, procedure illustrations, journey and cost screens | 2026-11-26 (done) |
| M7 | Languages, reading levels, Ask a question | 2027-01-17 |
| M8 | PDF booklet, plan sync and versions, analytics | 2027-01-26 |
| M9 | Bedrock and Polly path, Windows service and installer, hardening | 2027-02-02 |
| M10 | Public demo, final ClarityEval and dentist review, paper, video, submission | 2027-02-14 |

## M0 Foundations (done)

- Rust workspace with `dc-core`, `dc-pms`, `dc-knowledge`, `dc-ai`, `dc-generation`, `dc-server` and `dc-web`.
- Planning docs in `docs/`.
- Synthetic practice fixture with eight patients, including a pediatric guardian case, a plan with an unknown insurance estimate, and a patient with no appointment.
- Code map for 36 procedure codes with our own names.
- `dc-ai`: provider traits, mode policy (clinical mode cannot build the OpenRouter adapter), OpenRouter text adapter, scripted fake model.
- `dc-generation`: deterministic validators for money in text, unplanned tooth numbers and blocked phrases.
- Server: health and dashboard endpoints, SQLite with migrations, embedded web app.
- Web app: staff shell and dashboard matching the approved mockup.

## M1 Staff session and request security (done)

- Shared staff account with Argon2id password and PIN hashes. First-launch setup needs a one-time code printed in the server console.
- Server-side sessions with opaque HttpOnly cookies; only SHA-256 hashes of tokens are stored. Staff sessions expire after 30 minutes idle or 12 hours; demo visitor sessions after 24 hours.
- Demo visitors get an isolated session from `POST /api/visit` without signing in. A visitor session never opens the clinical workspace, and changing the mode ends all sessions.
- Session-bound CSRF token in `X-DC-CSRF`, plus an Origin check on every state-changing request.
- Per-address login limits and backoff, a capped limiter, IPv6 grouped by /64, and an audit event for failure surges.
- Content Security Policy that allows Trunk's loader script by hash, with no `unsafe-inline`, plus the other security headers.
- Clinical mode refuses to run on the network without HTTPS.
- A security review of this milestone found nine issues; all are fixed and covered by tests.

## M2 Open Dental connector

Acceptance:
- `opendental` cargo feature on `dc-pms` with sqlx MySQL over a read-only user.
- The table semantics checklist below is verified against a real Open Dental install with the demo database, and the supported version range is written down.
- A synthetic MySQL database with the same schema subset seeds CI. Connector contract tests pass against both the synthetic TOML connector and the synthetic MySQL database.

Checklist to verify before relying on any query:
- How active and saved treatment plans are stored (`treatplan`, `proctp`) and how they link to `procedurelog`.
- Procedure status values that mean planned, scheduled and complete.
- Tooth number, surface and area fields (`ToothNum`, `Surf`, `ToothRange`) and how quadrant and arch procedures are stored.
- Priority or phase representation.
- Where insurance estimates live (`claimproc`) and how write-offs and patient portion are computed.
- Patient language field format.
- Procedure notes and how a note links to a planned procedure.
- Appointment tables for the recommended-patient list.

Progress:
- Done: the `opendental` feature on `dc-pms` (sqlx MySQL with rustls). The connector refuses a user with any privilege besides `SELECT`, and every connection runs `SET SESSION TRANSACTION READ ONLY` (DECISIONS D14). All SQL is in `crates/dc-pms/src/opendental/queries.rs`.
- Done: every checklist item answered from the published Open Dental 24.1 schema, with sources, in [OPENDENTAL.md](OPENDENTAL.md), and the reading rules in DECISIONS D13. The written-down target range is 24.1 and later 24.x on MySQL 5.7 or 8.0 or MariaDB 10.x; it is untested.
- Done: the synthetic Open Dental database in `fixtures/opendental` (schema subset, seed mirroring the TOML fixture plus rows that must be left out, and the read-only user with column-level grants). The connector contract tests in `crates/dc-pms/tests/contract.rs` run against the TOML connector always and against the synthetic database when `DC_TEST_MYSQL_URL` is set; a CI job loads the seed into MySQL 8.0 and MariaDB 10.11 and runs them.
- Done: server selection with `DC_PMS=opendental` in clinical mode only, credentials from `DC_OPENDENTAL_*`, and TLS required for a database on another host.
- Open: a first green run of the MySQL and MariaDB CI job. The MySQL contract tests have not run yet; they were written without a local database.
- Open: verify every checklist answer against a real Open Dental install with the demo database, then confirm the version range. Until then the D13 rules stay Proposed.
- Open: saved plans (`proctp`), the patient's preferred name, the statement timeout and polling load limits, and moving the password into the encrypted `secrets` table.

## M3 Knowledge base and staff screens

Acceptance:
- 25 to 40 knowledge entries in `knowledge/entries/`, one per category, written as original text with verified source links. Each entry has `review_status = "unreviewed"` until a dentist signs it off.
- Patient directory with search, and plan selection.
- Settings: practice name, logo and brand colors. Setup wizard for first launch.

Progress:
- Done: patient directory (`GET /api/staff/patients`), patient page with the read-only plan table and totals (`GET /api/staff/patients/{key}`), and practice name and brand colors in Settings (`GET` and `PUT /api/staff/settings`, with a 4.5:1 contrast check on the brand color; both answer `403 demo_read_only` in the demo).
- Done: 30 knowledge entries in `knowledge/entries/`, all `unreviewed`. The eight added in this milestone (DECISIONS E25) use the fallback illustration.
- Done: logo upload in Settings (`PUT` and `DELETE /api/staff/settings/logo`, migration 0013), checked by content with SVG accepted only from an allowlist, and shown in the staff sidebar and the patient header (DECISIONS E26).
- Done: the five-step setup wizard with a read-only connection check (`POST /api/setup/check`), brand colors and logo at setup, and the three acknowledgments (DECISIONS E27).
- Open: editable Open Dental and AI provider settings, which wait for M9; drawings for the eight new categories; dentist review of every entry.

## M4 Generation pipeline

Acceptance:
- Pipeline stages from [AI_PIPELINE.md](AI_PIPELINE.md) implemented, with prompts versioned in the repository.
- Default demo model `deepseek/deepseek-v4.1-flash-1017` with high reasoning effort, using structured output.
- Jobs persist in SQLite, survive a server restart and report progress to the create screen.
- Content that fails validation is held as needs review and never shown to patients.
- Timing measured with the benchmark definition in [AI_PIPELINE.md](AI_PIPELINE.md).

Progress:
- Done: server wiring. `POST /api/staff/presentations` takes a plan snapshot, stores the presentation and a queued job, and starts it; `GET /api/staff/jobs/{id}` reports per-section progress while the job runs; `GET /api/staff/presentations` and `GET /api/staff/presentations/{id}` return the summary list and the full view with locked facts from the snapshot.
- Done: jobs in SQLite (migration 0003), at most 2 running at once, resumed after a restart without writing finished sections again (migration 0007). Final state is succeeded when every section passed, needs review when at least one passed, and failed when none did.
- Done: demo controls. Presentations are scoped to the visitor session and purged after 24 hours; 3 new generations per hour per visitor, 10 per day per client address, 1 running job per visitor (all `429` with `Retry-After`); the monthly budget guard with the 80% soft stop.
- Done: demo mode builds OpenRouter models from `OPENROUTER_API_KEY`, with `DC_TEXT_MODEL` as an override. Clinical mode answers `503 provider_not_configured` until M9.
- Done: call timeouts and billing (DECISIONS D12). A text call with hidden reasoning times out after 120 s, others after 45 s; a call that times out or returns an unreadable reply is charged at its worst-case reservation and retried once.
- Done: resume within a section (DECISIONS E5). Every model reply is saved to `job_saved_replies` (migration 0014) before the section moves on; a restarted section replays its saved replies through the same checks and calls the model only from the stage where it stopped, so no reply is sent or charged twice.
- Done: the `ai_calls` table (migration 0014, E35), one row per text or speech call with its job, section, stage, round, attempt, models, route, prompt version, tokens, latency, outcome, cost and cost source, and no prompt or reply text.
- Done: the dated price table in `crates/dc-server/prices.toml` (E36), used for reservations and for calls that report no cost.
- Done: the queue position. `GET /api/staff/jobs/{id}` returns `queue_ahead` and the progress card shows "Queued, N ahead" (E37).
- Done: the timing benchmark harness, `cargo run -p dc-server --bin dc-bench` (E38), with the `perf/typical` workload in `fixtures/perf/typical.toml`. Scripted results are in [AI_PIPELINE.md](AI_PIPELINE.md#measuring-the-90-second-target). The first run found that every job waited out the progress writer's 250 ms pause after its last section; the writer now wakes when the job is done.
- Open: the live timing runs (20 runs over at least 2 days on OpenRouter, `--live`) that the 90 s claim needs.

## M5 Editor, Present mode and narration

Acceptance:
- Editor with provenance labels, staff edits, revert and the checks panel. Note-derived explanations need one staff confirmation before patients see them (P11).
- Present button locks the staff session server-side. Exit needs the PIN in clinical mode and nothing in demo mode.
- Patient screens: at a glance, procedures, costs, next steps.
- Sentence-level narration with `heygen/heygen-voice-1-turbo` in demo mode, cached per sentence and invalidated by edits.

Progress:
- Done (server): Present mode. `POST /api/staff/presentations/{id}/present` locks the staff or visitor session and sets the present cookie; `GET /api/present/content` returns the patient view of the bound version; `GET /api/present/audio/{key}` serves narration for sentences of that version only; `POST /api/present/exit` unlocks with a rotated session token (PIN in clinical mode, password after 5 wrong PINs, nothing in demo mode). Migration 0004.
- Done (server): a lock whose present session expired is lifted with `POST /api/session/unlock` (no password in demo mode, the staff password in clinical mode); `GET /api/session` reports it with `present_ended`, and staff routes answer `423 locked_present_ended`. Held sections can be written again one at a time (`.../sections/{key}/regenerate`) or released after the deterministic checks pass on the current text (`.../sections/{key}/release`). Migration 0007. The demo regeneration limits count rows in `regeneration_events` (migration 0008), so they survive a restart and the 24 h purge.
- Done (server): the patient view shows ready sections only, staff text where it exists, and a note-derived block only after staff confirm it; until then the reason shows the knowledge base's common reasons and the objective is left out. Placeholders are filled locally and every sentence carries its audio key.
- Done (server): staff edits (`PUT .../blocks`), revert (`POST .../blocks/revert`) and note confirmation (`POST .../confirm-note`), with the original text kept, the deterministic validators run as warnings, the demo edit limits, and audit events.
- Done (web): the editor, the lock screen with Unlock, and the Regenerate section and Release to patient controls for held sections.
- Narration: OpenRouter `/audio/speech` adapter (PCM wrapped as WAV), `audio_clips` cache under `artifacts/audio`, one upstream call per key, 400-character limit, spend counted in the monthly budget with a 20% share for speech; clinical mode answers 503 until M9. Each block plays as one merged clip from `GET /api/present/narration` with sentence offsets for the highlight, requested in aligned parts of 12 sentences. Entering Present pre-warms generated blocks. Demo quotas charge at least 50 characters per synthesized sentence and give one client address at most 30 of the 120 upstream calls per minute (DECISIONS E19).

## M6 Chart, illustrations, journey and costs

Acceptance:
- Interactive odontogram following the rules in [UI_SPEC.md](UI_SPEC.md), including primary teeth and procedures with no tooth.
- Original SVG illustrations for the procedure categories in the synthetic fixture.
- Journey and cost screens rendered from snapshot data only.

Progress:
- Done: the interactive tooth chart, 22 original illustrations plus a fallback, and the journey and cost screens.
- Done: page transitions with motion tokens that reduced motion turns off, and a layout that scales from tablet portrait to 2560 px wide screens.

## M7 Languages, reading levels and questions

Acceptance:
- Translation from approved English text, cached per language and reading level.
- Reading level control on every patient screen.
- Ask a question drawer with grounding, refusals, escalation wording and the notice that the practice can read the conversation.

Progress:
- Ask a question is built (DECISIONS E24): `GET` and `POST /api/present/qa` answer from the bound version's patient-visible text, the fact sheet without money and the knowledge base, with citations checked and V1, V3 and V5 to V16 on every answer. Declined answers carry a question for the dental team; failing answers show the fixed fallback.
- Urgent questions get the fixed escalation text before any model call, also when the budget is used up or no provider exists, and flag the presentation for staff.
- The drawer in Present mode has the notice that the practice can read the conversation, focus trapping, Escape to close and polite announcements. Staff read the conversation on the presentation page (`GET /api/staff/presentations/{id}/questions`).
- Conversations are stored in `qa_conversations` (migration 0011), purged with demo presentations and after 30 days. Demo limits and the budget apply.
- Not done yet: translated answers and reading-level variants (with translation), and opening the drawer prefilled from the "Questions you might ask" cards.
- Done (server): language and reading level variants derived from the approved English text, one translator call per section with one repair round, the translation reviewer, and the checks V18 to V20 (DECISIONS E32). Cached in `section_variants` per version, section, language, level and source hash (migration 0010). `POST /api/present/variant` starts a variant and `GET /api/present/content?language=&level=` reads it; both take the presentation and version from the present session. Narration keys follow the derived text. Demo limits of 10 new variants per visitor per hour and 30 per network per day (`variant_events`); spend counts in the monthly budget; no provider means `503 provider_not_configured`.
- Done (web): a language select and an "Aa" reading level control in the patient header on every screen, plus the inline control on procedure detail. Switching keeps the screen, shows placeholder lines on sections being prepared, and labels sections that stay in English. Untested languages say "Not reviewed".
- Not done: translated screen labels, section titles and cost sentences, and a native-speaker review of any language.

## M8 Booklet, sync and analytics

Acceptance:
- Typst booklet in the chosen language with bundled fonts.
- Plan polling with fingerprint comparison and the diff rules in [AI_PIPELINE.md](AI_PIPELINE.md#sync).
- Version history and in-office engagement analytics.

Progress:
- Done: in-office engagement analytics (DECISIONS E23). The patient screens report time per screen every 15 seconds and on each screen change, and each narration start, to `POST /api/present/analytics`; starting Present mode records the showing (migration 0009). `GET /api/staff/analytics?days=7|30|90` and the Analytics screen show showings, median time in a presentation, narration plays, questions asked, time by screen, languages and reading levels, generation outcomes with p50 and p90 time and model spend, and the presentations shown. Rows hold no names or free text and are deleted after 90 days.
- Done: the booklet (DECISIONS E7). `GET /api/staff/presentations/{id}/export[?version=N]` returns the PDF of a version, typeset in process by the new `dc-booklet` crate with bundled Noto Sans and Noto Sans Arabic. It prints the cover, at a glance with the teeth in the plan, journey, one page per procedure with its drawing, costs and what happens next with lines for notes, from the patient view only. Held sections print their title and "Your dental team will go over this part with you." The footer carries practice name, plan date, version and short hash. Export writes a `booklet_exported` audit event and marks the version exported. At most 2 booklets render at once.
- Done: plan sync (E33). A poll every 5 minutes compares each presentation with its live plan; an unchanged fingerprint does nothing and makes no model call. A changed one makes a new version from the diff, carrying unchanged sections and holding changed ones for Regenerate. A deleted plan or unreachable records show as a banner. The dashboard shows "Plan changed" until the new version is presented or exported.
- Done: version history (E34, migration 0012). `GET .../versions` lists versions with cause, changes and presented and exported markers; `GET .../versions/{n}` opens one read-only. Present sessions stay on their version through a plan change. Regenerating a section of a presented or exported version makes a new version.
- Done (web): History drawer, read-only version view at `/presentations/{id}?version=N`, plan changed, plan deleted and records unreachable banners, and the Export booklet link.
- Open: translated booklet labels and language names (M7 branch), the booklet's references page and chart drawing, A4 paper in Settings, CJK fonts, a new draft version on staff edits of a presented version (FR-VER-01), Restore as new version, the edit-conflict banner with Keep my edit / Use new text, and the header sync indicator.

## M9 Clinical path and Windows

Acceptance:
- Bedrock and Polly adapters behind the `aws` feature, smoke-tested against a development AWS account with synthetic data only.
- `dentalclarity.exe service install | uninstall | run`.
- Installer technology chosen and built. TLS on the LAN, secrets protected with DPAPI.
- The clinical-readiness gaps in [SECURITY.md](SECURITY.md) are listed in the README.

Progress:
- Done: Bedrock (Converse, structured output through one `output` tool) and Polly (neural, 16 kHz PCM wrapped as WAV) adapters in `dc-ai` behind the `aws` feature, with unit tests for request building, reply parsing, voices and audio that need no network. Default and CI builds do not compile the AWS SDK (DECISIONS D15, D16).
- Done: clinical mode builds the AWS providers when the server is built with `--features aws` and `DC_AI_PROVIDER=aws` is set; otherwise it keeps the 503 answers. It never constructs OpenRouter (D17). Narration records Polly calls at the engine's price.
- Pending: the live smoke test (`cargo run -p dc-ai --features aws --example aws_smoke`, synthetic text, one Bedrock and one Polly call). On 2026-11-28 the development account's IAM Identity Center session had expired, so it was not run. Until it runs, the Bedrock model id and the tool-use reply shape are unverified against the live service.
- Done: `dentalclarity service install | uninstall | run` with the `windows-service` crate, under the virtual account `NT SERVICE\DentalClarity` (E28). The commands compile on Windows and other systems; installing and running the service on a clean Windows machine is still to be verified.
- Done: HTTPS in the server process with rustls, from practice PEM files or a generated local CA (E29), with a test that a client trusting only the exported CA connects and one without it is refused. Device trust steps are in [deploy/windows/README.md](../deploy/windows/README.md).
- Done: the DPAPI secret store with `dentalclarity secret set | delete` (E30), holding the AWS access key and the local CA key.
- Installer: Inno Setup chosen (E31); the script is `deploy/windows/dentalclarity.iss`. Not built yet, because Inno Setup is not installed on the development machine.
- Done: the README lists the clinical-readiness gaps from SECURITY.md.
- Open: the narration listening test for the Polly voice, Bedrock and Polly data-handling terms, the Polly price check, short-lived AWS credentials, code signing, and an expiry reminder for practice-provided certificates.

## M10 Submission

Acceptance:
- Public demo deployed with the $25 monthly budget cap and per-visitor limits, plus pre-generated cases.
- Final ClarityEval runs on the held-out split, dentist review completed, results published with limitations.
- Paper draft, 2 to 5 minute video, Devpost entry.

## Engineering conventions

- `cargo fmt`, `cargo clippy -- -D warnings` and `cargo test` pass before every push.
- No network access in tests. Model calls in tests use `FakeTextModel`.
- Never commit secrets, real patient data or CDT descriptor text.
- Commit messages are short and imperative ("Add synthetic connector").
