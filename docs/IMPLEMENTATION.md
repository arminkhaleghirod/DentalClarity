# Implementation plan

Milestones run from 2026-11-26 to the submission deadline on 2027-02-15. Dates are targets and some milestones overlap. The order puts a complete demo workflow ahead of polish, as decided in [DECISIONS.md](DECISIONS.md) (P3). ClarityEval has its own timeline in [CLARITYEVAL.md](CLARITYEVAL.md); its preliminary model check feeds M4 and its final runs land in M10.

| Milestone | Scope | Target |
|---|---|---|
| M0 | Foundations: workspace, planning docs, synthetic connector, dashboard | 2026-11-26 (done) |
| M1 | Staff session, demo visitor isolation, security headers and CSP | 2026-12-03 |
| M2 | Open Dental connector and schema-matched synthetic MySQL | 2026-12-10 |
| M3 | Knowledge base entries, patient directory, settings and setup wizard | 2026-12-14 |
| M4 | Generation pipeline, validators, review pass, persistent jobs | 2026-12-22 |
| M5 | Editor, Present mode, patient screens, narration | 2027-01-10 |
| M6 | Odontogram, procedure illustrations, journey and cost screens | 2027-01-10 |
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

## M1 Staff session and request security

Acceptance:
- Shared staff account with Argon2id password hash. Login, logout and session expiry work. Sessions are server-side rows with opaque cookie tokens.
- In demo mode each visitor gets an isolated session without logging in, and nothing one visitor creates is visible to another.
- CSRF token required on every state-changing request.
- Content Security Policy that works with the Trunk bundle (verify whether Trunk's inline loader needs a hash).
- Rate limiting on login and on all `/api` routes.

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

## M4 Generation pipeline

Acceptance:
- Pipeline stages from [AI_PIPELINE.md](AI_PIPELINE.md) implemented, with prompts versioned in the repository.
- Default demo model `deepseek/deepseek-v4.1-flash-1017` with high reasoning effort, using structured output.
- Jobs persist in SQLite, survive a server restart and report progress to the create screen.
- Content that fails validation is held as needs review and never shown to patients.
- Timing measured with the benchmark definition in [AI_PIPELINE.md](AI_PIPELINE.md).

## M5 Editor, Present mode and narration

Acceptance:
- Editor with provenance labels, staff edits, revert and the checks panel.
- Present button locks the staff session server-side. Exit needs the PIN in clinical mode and nothing in demo mode.
- Patient screens: at a glance, procedures, costs, next steps.
- Sentence-level narration with `heygen/heygen-voice-1-turbo` in demo mode, cached per sentence and invalidated by edits.

## M6 Chart, illustrations, journey and costs

Acceptance:
- Interactive odontogram following the rules in [UI_SPEC.md](UI_SPEC.md), including primary teeth and procedures with no tooth.
- Original SVG illustrations for the procedure categories in the synthetic fixture.
- Journey and cost screens rendered from snapshot data only.

## M7 Languages, reading levels and questions

Acceptance:
- Translation from approved English text, cached per language and reading level.
- Reading level control on every patient screen.
- Ask a question drawer with grounding, refusals, escalation wording and the notice that the practice can read the conversation.

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
