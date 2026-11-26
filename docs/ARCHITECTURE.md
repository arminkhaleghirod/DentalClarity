# DentalClarity architecture

Status: v0.1, written at project start. [DECISIONS.md](DECISIONS.md) is authoritative wherever the two disagree.

## 1. System context

```
          Practice LAN (clinical mode)                        Internet
 ┌──────────────────────────────────────────────┐
 │  Staff browser / tablet ──HTTPS──┐            │
 │                                  ▼            │
 │                     ┌────────────────────┐    │     ┌─────────────────────┐
 │                     │  dentalclarity.exe │────┼────▶│ AWS Bedrock / Polly │
 │                     │  (Windows service) │    │     │ (practice's account)│
 │                     └───┬────────────┬───┘    │     └─────────────────────┘
 │        read-only SQL    │            │ files  │
 │                         ▼            ▼        │
 │              Open Dental MySQL   SQLite + artifacts (data dir)
 └──────────────────────────────────────────────┘

   Public demo (demo mode): same binary in a container, synthetic connector,
   OpenRouter for all AI, rate limits plus a monthly budget cap.
```

One binary serves the API and the embedded Leptos WASM frontend. Mode (`demo` | `clinical`) is set at startup and cannot change at runtime. It picks the allowed PMS connectors, AI providers and security defaults:

| Concern | `demo` | `clinical` |
|---|---|---|
| PMS connector | `synthetic` only | `opendental` (or `synthetic` for training) |
| AI provider | OpenRouter (text and TTS) | Bedrock (text) and Polly (TTS); OpenRouter refused |
| Leave Present mode | no PIN | staff PIN |
| Staff login | demo auto-session per visitor, isolated | shared staff account |
| Inference limits | per-visitor rate limit, $25/mo budget cap | practice-configured |
| TLS | terminated by the hosting platform | rustls with a practice-provided or generated cert |

## 2. Workspace layout

```
DentalClarity/
├─ Cargo.toml                 workspace
├─ crates/
│  ├─ dc-core/                domain model + API types (no I/O; builds for native and wasm32)
│  ├─ dc-pms/                 PmsConnector trait; synthetic + opendental (feature) connectors
│  ├─ dc-knowledge/           curated knowledge base loader + deterministic lookup
│  ├─ dc-ai/                  provider traits; openrouter, bedrock/polly (feature) adapters
│  ├─ dc-generation/          multistage pipeline, validators, prompts
│  ├─ dc-booklet/             (planned, M8) Typst PDF booklet rendering
│  ├─ dc-server/              Axum binary: API, auth, sessions, jobs, sync, storage, service
│  ├─ dc-web/                 Leptos CSR app (staff + patient), built by Trunk
│  └─ clarityeval/            (planned) benchmark runner and scoring CLI
├─ knowledge/                 curated entries (TOML), original text, with sources (exists)
├─ fixtures/synthetic/        synthetic practice: patients, plans, notes, no real data (exists)
├─ eval/                      (planned) ClarityEval dataset (dev split; the held-out split stays out of git until final runs)
├─ assets/                    (planned) fonts, original SVG illustrations, booklet templates
├─ docs/                      planning docs
└─ deploy/                    (planned) container, Windows installer, service scripts
```

Dependency direction: `dc-core` ← everything. `dc-generation` depends on `dc-ai`, `dc-knowledge` and `dc-core`. `dc-server` depends on all of the backend crates. `dc-web` depends only on `dc-core`.

## 3. Domain model (dc-core)

The model starts with what the PMS says, so authoritative facts are separate from generated text.

- **`PlanSnapshot`** is immutable and normalized from the PMS at a point in time. It holds the patient's display context (first name, age band, preferred language; all local-only), the plan's identity and status, an ordered list of `PlannedProcedure`, and cost totals. `fingerprint = sha256(canonical_json(snapshot minus volatile fields))`.
- **`PlannedProcedure`**: source id, `ProcedureCode` (e.g. `D2740`), `ToothRef` (`Permanent(1..=32)` in Universal numbering, `Primary('A'..='T')`, `Quadrant`, `Arch`, `None`), surfaces, phase/priority, fee, insurance estimate and patient estimate (as `Money` in integer cents, or `Unknown`), status, and an optional linked `ClinicalNoteRef`.
- **`ProcedureCategory`**: our own taxonomy (e.g. `RootCanal`, `CrownSingle`, `FillingPosteriorComposite`, …), mapped from codes by `dc-knowledge`. Unmapped codes map to `Unsupported`, which routes the procedure to review instead of improvised content.
- **`Presentation`** → **`PresentationVersion`** (bound to one snapshot fingerprint) → **`Section`** (at-a-glance, journey, procedure×N, costs, next steps) → **`Block`**. Each block has a `Provenance`: `Source` (locked PMS fact), `ClinicalNote`, `Generated { model, prompt_version, checked }` or `StaffEdit { at, previous }`. A block also has a `ValidationState`: `Passed`, `NeedsReview(reasons)` or `Failed`.
- **`Variant`**: a (language, reading level) rendering of a version's text blocks. Narration `AudioClip`s attach to sentences of a specific variant and are keyed by the hash of the text, voice and provider, so any edit invalidates them automatically.

Money is never a float, and the AI never computes costs. Cost blocks are rendered deterministically from the snapshot.

## 4. PMS connectors (dc-pms)

```rust
#[async_trait]
pub trait PmsConnector: Send + Sync {
    async fn health(&self) -> Result<PmsHealth>;
    async fn search_patients(&self, q: &PatientQuery) -> Result<Vec<PatientSummary>>;
    async fn recommended_patients(&self, day: Date) -> Result<Vec<PatientSummary>>;
    async fn treatment_plans(&self, patient: &PatientKey) -> Result<Vec<PlanSummary>>;
    async fn plan_snapshot(&self, plan: &PlanKey) -> Result<PlanSnapshot>;
}
```

- **synthetic**: loads `fixtures/synthetic/*.toml` into memory. It is the only connector in demo mode and is also used by tests and the eval.
- **opendental** (cargo feature): sqlx MySQL over a read-only user. Queries live in one module with tests run against the synthetic MySQL schema. Table semantics (`treatplan`, `proctp`, `procedurelog`, `procedurecode`, `claimproc`, `patient`, `appointment`) must be verified against the target OD version before being relied on; see the M2 checklist in [IMPLEMENTATION.md](IMPLEMENTATION.md).
- Recommended patients (proposed default): patients with an appointment today or in the next N days and an active treatment plan with unscheduled planned procedures, ordered by appointment time.

## 5. Server (dc-server)

- **HTTP**: Axum 0.8. `/api/staff/*` needs a staff session. `/api/present/*` needs a presentation-scoped session. Static assets (the Leptos bundle, fonts, illustrations) are embedded with `rust-embed`.
- **Sessions**: opaque random tokens in `HttpOnly; Secure; SameSite=Strict` cookies, with server-side rows in SQLite. CSRF is handled with a double-submit token header on state-changing requests.
- **Present mode** (security-relevant flow):
  1. Staff clicks Present. The server creates a `present_session` bound to `(presentation_id, version_id)` and locks the staff session server-side.
  2. The browser navigates to `/present`. Every staff endpoint now returns `423 Locked` for that staff session, so back navigation and direct API calls expose nothing.
  3. Exit: in clinical mode `POST /api/present/exit` with the PIN (rate-limited, Argon2-hashed) unlocks the staff session and ends the present session. Demo mode needs no PIN.
- **Storage**: SQLite with WAL, migrations in `crates/dc-server/migrations`. Tables (initial): `settings`, `secrets`, `staff_account`, `sessions`, `present_sessions`, `snapshots`, `presentations`, `versions`, `sections`, `blocks`, `variants`, `audio_clips`, `jobs`, `job_events`, `qa_conversations`, `analytics_events`, `audit_events`, `sync_state`.
- **Secrets** (AWS credentials, PMS password, OpenRouter key): encrypted at rest. On Windows the data key is protected with DPAPI (machine scope). In the demo container, secrets come from environment variables only.
- **Jobs**: a `jobs` row records the state (`queued → running(stage) → succeeded | needs_review | failed`) plus a progress event log. A Tokio worker pool claims jobs; on startup `queued` and `running` jobs run again, skipping the sections they already saved to `job_section_results` (per-section resume, DECISIONS E5; per-stage resume is not built). The browser polls `GET /api/staff/jobs/:id` (SSE can come later).
- **Sync**: every 5 minutes (configurable) a poller takes snapshots of the plans behind active presentations and compares fingerprints. A change creates a new version through the diff rules in [AI_PIPELINE.md](AI_PIPELINE.md#sync). Cost-only diffs regenerate nothing. Clinical diffs mark the affected sections `NeedsReview`.
- **Data dir**: `%ProgramData%\DentalClarity` on Windows and `/data` in the container. It holds `dentalclarity.db`, `artifacts/` (content-addressed `sha256.ext`) and `logs/`.

## 6. AI layer (dc-ai)

Traits are split by capability, with no single universal endpoint:

```rust
trait TextModel   { async fn generate(&self, req: TextRequest) -> Result<TextResponse>; } // JSON-schema structured output
trait SpeechModel { async fn synthesize(&self, req: SpeechRequest) -> Result<SpeechClip>; } // one sentence → audio
```

Adapters: `openrouter` (text, plus speech with `heygen/heygen-voice-1-turbo`, whose endpoint is verified in M5) and `aws` (Bedrock Converse plus Polly, feature-gated). The `ProviderPolicy` built from the mode decides which adapters can be constructed, and clinical mode cannot construct OpenRouter. Every call records the model id, latency, token counts and estimated cost for analytics and the demo budget meter.

## 7. Frontend (dc-web)

Leptos 0.8 CSR with leptos_router. Two route trees:

- `/` (staff): Dashboard, Patients, Presentations (editor, history, export), Analytics, Settings, Setup wizard.
- `/present` (patient): at a glance, journey, procedures (with the odontogram), costs, next steps, plus the Ask a question drawer, language and reading-level controls, and narration.

The odontogram and illustrations are inline SVG components. Design tokens are CSS custom properties, and practice branding overrides the `--brand-*` tokens for the patient tree only. See [UI_SPEC.md](UI_SPEC.md).

## 8. Booklet (dc-booklet)

Typst is compiled in-process from a template in `assets/booklet/`, fed with a JSON data model of one version and variant. Fonts are bundled. Output goes to `artifacts/` keyed by (version, variant, template hash).

## 9. Testing strategy (summary)

- Unit tests in each crate. Snapshot tests (`insta`) for prompt construction and the canonical fingerprint.
- Connector contract tests run against both synthetic and synthetic-MySQL.
- Pipeline tests use a deterministic `FakeTextModel`. No network in CI.
- Validators are tested with adversarial fixtures: invented costs, wrong tooth, diagnosis language.
- End-to-end: a browser-driven smoke test of create → generate → present → exit on the demo build.
- ClarityEval runs separately and is never part of CI (it costs money and needs network).

## 10. Deployment

- **Demo**: a multi-stage Dockerfile (build Trunk bundle → build server → distroless runtime). Host chosen later (DECISIONS D6).
- **Clinical (Windows)**: `dentalclarity.exe service install|uninstall|run`, a firewall rule for the LAN port, data in ProgramData, and the installer chosen in M9. It is installed on the OD server by default; a separate host is documented as the safer alternative.
