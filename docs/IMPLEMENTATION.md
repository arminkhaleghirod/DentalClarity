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

## M3 Knowledge base and staff screens

Acceptance:
- 25 to 40 knowledge entries in `knowledge/entries/`, one per category, written as original text with verified source links. Each entry has `review_status = "unreviewed"` until a dentist signs it off.
- Patient directory with search, and plan selection.
- Settings: practice name, logo and brand colors. Setup wizard for first launch.

Progress:
- Done: patient directory (`GET /api/staff/patients`), patient page with the read-only plan table and totals (`GET /api/staff/patients/{key}`), and practice name and brand colors in Settings (`GET` and `PUT /api/staff/settings`, with a 4.5:1 contrast check on the brand color; both answer `403 demo_read_only` in the demo).
- Done: 22 knowledge entries in `knowledge/entries/`, all `unreviewed`.
- Open: more entries toward the 25 to 40 target, logo upload, and the multi-step setup wizard.

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
- Open: resume within a section (per stage), a visible queue position, the `ai_calls` table, the dated price table, and the timing benchmark.

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
- Ask a question is built (DECISIONS E23): `GET` and `POST /api/present/qa` answer from the bound version's patient-visible text, the fact sheet without money and the knowledge base, with citations checked and V1, V3 and V5 to V16 on every answer. Declined answers carry a question for the dental team; failing answers show the fixed fallback.
- Urgent questions get the fixed escalation text before any model call, also when the budget is used up or no provider exists, and flag the presentation for staff.
- The drawer in Present mode has the notice that the practice can read the conversation, focus trapping, Escape to close and polite announcements. Staff read the conversation on the presentation page (`GET /api/staff/presentations/{id}/questions`).
- Conversations are stored in `qa_conversations` (migration 0011), purged with demo presentations and after 30 days. Demo limits and the budget apply.
- Not done yet: translated answers and reading-level variants (with translation), and opening the drawer prefilled from the "Questions you might ask" cards.

## M8 Booklet, sync and analytics

Acceptance:
- Typst booklet in the chosen language with bundled fonts.
- Plan polling with fingerprint comparison and the diff rules in [AI_PIPELINE.md](AI_PIPELINE.md#sync).
- Version history and in-office engagement analytics.

## M9 Clinical path and Windows

Acceptance:
- Bedrock and Polly adapters behind the `aws` feature, smoke-tested against a development AWS account with synthetic data only.
- `dentalclarity.exe service install | uninstall | run`.
- Installer technology chosen and built. TLS on the LAN, secrets protected with DPAPI.
- The clinical-readiness gaps in [SECURITY.md](SECURITY.md) are listed in the README.

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
