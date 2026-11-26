# DentalClarity decision log

This is the authoritative list of product and technical decisions. Every other planning doc must agree with it. Confirmed means the project owner decided it. Proposed means engineering chose it and it can change if evidence warrants. When a decision changes, edit it here first.

Last updated: 2026-11-26

## Product

| # | Decision | Status |
|---|---|---|
| P1 | DentalClarity is a patient education tool. It explains the treatment the dentist already proposed. It never diagnoses, recommends treatment, persuades, optimizes acceptance or guarantees outcomes. | Confirmed |
| P2 | Three deliverables: the DentalClarity app (self-hosted distribution plus a public synthetic demo), the ClarityEval benchmark (same repo), and a research-style paper. | Confirmed |
| P3 | When goals conflict, the hackathon submission wins: demo polish, the patient experience and ClarityEval come first. The clinical provider path, Windows installer and auto-sync get built and smoke-tested but may stay thinner. | Confirmed |
| P4 | Hackathon: Teens in AI Global Challenge 2027, track AI for Health, Safety & Inclusion. Deadline 2027-02-15. Required: project description, tech list, public GitHub repo, working demo link, 2 to 5 minute video. | Confirmed (rules read 2026-11-25) |
| P5 | Staff UI is English-only and DentalClarity-branded. The patient UI is fully practice-branded, with no DentalClarity branding. | Confirmed |
| P6 | Patients get a PDF booklet to take home. No patient portal, public links or tunnels. | Confirmed |
| P7 | One shared staff account for the prototype. Individual accounts and RBAC are out of scope. This is documented as a clinical-readiness gap. | Confirmed |
| P8 | Leaving Present mode needs the staff PIN in clinical mode and nothing in demo mode. In both modes, the patient session is server-scoped to one presentation version. | Confirmed |
| P9 | Reading levels: simple, standard, detailed. Broad language coverage via the model; untested languages are never described as validated. | Confirmed |
| P10 | Narration highlighting is sentence-level: audio is generated per sentence and the active sentence is highlighted. No claims of word-accurate timing. | Confirmed |
| P11 | A clinical note linked to a planned procedure is used to explain "why your dentist recommended it" (shown as "From Dr. X's notes"). Only notes linked to planned procedures are used, never the chart history, and identifiers are stripped before the note reaches a model. The note-derived block is held from patients until staff confirm it with one click in the editor; until then the patient sees the common-reasons wording. With no linked note, the content describes common reasons and never invents a patient-specific one. | Confirmed (2026-11-25) |

## Providers and data

| # | Decision | Status |
|---|---|---|
| D1 | Demo mode: every AI feature (text, translation, Q&A, TTS) uses OpenRouter. Synthetic data only. | Confirmed |
| D2 | Clinical mode: every AI feature uses the practice's own AWS: Bedrock for text, Polly for narration. Code refuses OpenRouter in clinical mode. | Confirmed |
| D3 | No real PHI is used before the deadline. The Bedrock/Polly path is smoke-tested against a dev AWS account only. | Confirmed |
| D4 | Models are app-managed defaults in configuration; there is no admin model picker. Demo defaults: text `deepseek/deepseek-v4.1-flash-1017` at high reasoning effort, speech `heygen/heygen-voice-1-turbo` (both listed on OpenRouter as of 2026-11-25). ClarityEval checks these choices and can replace them. Clinical (Bedrock) models are chosen in M9. | Confirmed |
| D5 | Public demo OpenRouter spend cap: $25/month, plus per-visitor rate limits. Pre-generated cases keep working when the cap is hit. | Confirmed |
| D6 | Demo hosting provider: undecided. The demo ships as a portable container. | Confirmed (open) |
| D7 | Open Dental integration uses direct read-only SQL with a least-privilege user. It never writes to the PMS. Development targets both a real Open Dental install with its demo database (to verify semantics) and a schema-matched synthetic MySQL database (for the demo and CI). | Confirmed |
| D8 | CDT: only bare codes (e.g. `D2740`) are stored as keys, with our own plain-language names and categories. ADA descriptor text is never committed. At runtime, the practice's own Open Dental descriptions may be displayed. | Confirmed |
| D9 | The knowledge base is drafted as original text citing reputable sources. Each entry carries `review_status` and stays `unreviewed` until a dentist signs off. | Confirmed |
| D10 | Prompts carry an allowlist of typed fields. Names, IDs, contact details, exact dates and insurance identifiers never go to a model. Personalization placeholders (e.g. `{{patient_first_name}}`) are substituted locally. Masking alone is not called de-identification. | Confirmed |
| D11 | Demo text calls route to PrimeIntellect first on OpenRouter, falling back to other fast providers on errors. Only providers that support every parameter we send, do not store or train on prompts, and run 8-bit or better weights are used. Chosen by benchmark on 2026-11-26. | Confirmed |
| D12 | A text call with hidden reasoning times out after 120 s, other calls after 45 s. A call that times out or returns an unreadable reply counts against the budget at its worst-case reservation, because the provider may have billed it, and is retried once. Connection errors, 429 and 5xx keep three attempts. | Confirmed |

## Engineering

| # | Decision | Status |
|---|---|---|
| E1 | Rust-first: Axum + Tokio server; frontend in Leptos, compiled to WebAssembly. JS only where justified. | Confirmed |
| E2 | Frontend is Leptos 0.8 client-side rendered, built with Trunk and embedded in the server binary. One binary, no Node/Python/Docker at runtime. | Proposed |
| E3 | Server ↔ frontend use a JSON REST API. Request and response types live in `dc-core` and compile for both native and wasm. | Proposed |
| E4 | App storage: SQLite (sqlx, migrations). Large artifacts (audio, PDF) are stored as content-addressed files in the data directory. | Proposed |
| E5 | Generation jobs live in SQLite, run on a Tokio worker pool and resume after restart. Each section result is saved as it finishes, so a resumed job skips finished sections and does not pay for them twice; a section that was in flight starts over. | Proposed |
| E6 | Source sync: periodic read-only polling with a normalized snapshot fingerprint (SHA-256 of canonical JSON). An unchanged fingerprint means no inference. | Proposed |
| E7 | PDF booklet: Typst compiled in-process, with bundled fonts. | Proposed |
| E8 | Fonts (Figtree, IBM Plex Mono; both OFL) are self-hosted. No external CDN, since practice LANs may block them. | Proposed |
| E9 | Logging uses `tracing`. PHI-bearing values are wrapped in a `Sensitive<T>` type whose `Debug` and `Display` are redacted. | Proposed |
| E10 | Windows: runs as a Windows service (`windows-service` crate); the installer technology is chosen in milestone M9. | Proposed |
| E11 | License: Apache-2.0. | Confirmed |
| E12 | Staff resolve a held section in two ways. Regenerate writes that one section again from the same facts as a job, under the same gates as a new presentation (provider, one running job per demo visitor, budget soft stop) and its own demo limits of 6 per visitor per hour and 20 per network per day. A procedure with no knowledge entry cannot be regenerated and stays held. Release marks a `NeedsReview` section ready for patients; it needs stored output, and the deterministic validators run again on the current text, staff edits included, so a release never skips a blocking finding. Each release writes an `audit_events` row. Regenerating keeps staff edits (same facts) and drops that section's note confirmations. | Proposed |
| E13 | A session stays locked when its present session expires. The lock is lifted with `POST /api/session/unlock`: in demo mode without a password, rotating the token and moving the visitor's presentations as a normal exit does; in clinical mode only with the staff password. The PIN is refused there, because its five-try allowance belonged to the present session that ended. | Proposed |

## Design

| # | Decision | Status |
|---|---|---|
| U1 | Visual direction is mockup round 2 ("clean clinical"): staff primary `#2A7DD1`, gray sidebar with pill nav, gray-header tables. The default patient brand is navy `#1B3A5C` with coral reserved for "your teeth". Figtree plus IBM Plex Mono. | Confirmed |
| U2 | Mockup round 1 (teal/cream/serif) is superseded. | Confirmed |
| U3 | A provenance label on every content block: *Locked · from Open Dental*, *From clinical note*, *AI-written · checked*, *Edited by staff*. | Confirmed (from design) |
| U4 | A Present button at the top right of the editor. | Confirmed |
