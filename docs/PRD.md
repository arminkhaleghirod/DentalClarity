# DentalClarity product requirements

Status: v0.1, 2026-11-25. [DECISIONS.md](DECISIONS.md) is authoritative where this file disagrees with it. Architecture is in [ARCHITECTURE.md](ARCHITECTURE.md), screens and tokens in [UI_SPEC.md](UI_SPEC.md), the generation pipeline in [AI_PIPELINE.md](AI_PIPELINE.md), threat model and controls in [SECURITY.md](SECURITY.md), the benchmark in [CLARITYEVAL.md](CLARITYEVAL.md), milestones in [IMPLEMENTATION.md](IMPLEMENTATION.md).

Conventions:

- Every requirement has a stable ID (`FR-AREA-NN` or `NFR-AREA-NN`). IDs are never reused; a dropped requirement is struck through and kept.
- Priority is `Must` (needed for the hackathon submission on 2027-02-15), `Should` (planned before the deadline, may be cut), or `Later` (after the hackathon).
- Origin is `C` (confirmed in DECISIONS or the discovery record) or `P` (proposed here; can change).
- Milestones are written `Mx`; where a value depends on verification, the text says "TBD (verify in Mx)".

## 1. Problem

Treatment plans in a practice management system are written for clinicians and billing: CDT codes, tooth numbers, surfaces, phases and insurance estimates. Patients and guardians are expected to understand them during a short chairside or front-desk conversation, often in a second language. Generic brochures do not match the patient's actual plan, and staff have little time to explain each procedure.

DentalClarity reads the plan the dentist already saved in Open Dental and turns it into a practice-branded, multilingual, narrated presentation plus a take-home PDF booklet. It explains the proposed treatment. It does not diagnose, recommend, persuade or promise outcomes (DECISIONS P1).

## 2. Goals and non-goals

### Goals

| ID | Goal | Origin |
|---|---|---|
| G1 | A patient can explain, in their own words, what is planned for which teeth, in what order, and what they are estimated to pay. | P |
| G2 | Every patient-facing fact (procedure, tooth, phase, cost) traces to the PMS snapshot; AI text is validated before a patient sees it. | C |
| G3 | The patient picks the language and reading level, with no extra work for staff. | C |
| G4 | Staff go from "select patient" to "Present" with no manual writing in the typical case. | C |
| G5 | Judges can run the full staff and patient workflow on a public synthetic demo. | C |
| G6 | The ClarityEval benchmark picks the models on measured task performance. | C |

### Non-goals

- Increasing treatment acceptance or conversion. No acceptance metrics are collected or reported.
- Replacing the dentist's explanation or consent conversation.
- A general dental chatbot, record viewer or diagnosis engine.
- Commercial SaaS concerns (billing, tenants, licensing).

## 3. Users

| User | Context | Needs |
|---|---|---|
| Treatment coordinator / front-desk staff | Desktop workstation (1280×800 minimum) or practice tablet on the LAN | Find today's patients fast, start generation, glance-check output, fix a sentence, hand over the tablet. |
| Patient | Practice iPad, landscape or portrait, sometimes a chairside monitor | Plain explanation of their own plan, in their language, at their reading level, with audio. |
| Guardian of a pediatric patient | Same device as patient | Content addressed to the guardian; child's age framing without baby talk. |
| Practice administrator / IT | Windows Open Dental server | Install, connect read-only to Open Dental, set AI credentials, branding, staff PIN. |
| Hackathon judge | Public demo in a desktop browser | Experience the real workflow in a few minutes with no setup. |

## 4. End-to-end workflow

1. Staff opens DentalClarity on the LAN (clinical) or the demo URL (demo) and lands on the Dashboard.
2. Staff picks a patient from "Today's patients" or searches the Patients directory.
3. Staff picks one of the patient's existing treatment plans. Nothing about the plan is edited here.
4. Language defaults to the patient's preferred language from the PMS when available, else English. Reading level defaults to Standard. Staff may change either.
5. Staff clicks Create. The server snapshots the plan, locks facts, writes sections, validates them, runs a review pass and assembles version 1. Progress is shown per stage. Closing the tab does not stop the job.
6. Staff opens the editor, scans the Checks panel, optionally edits text blocks, and previews.
7. Staff clicks Present (top right). The browser enters patient mode bound to that version. The staff session is locked server-side.
8. The patient explores the presentation, changes language or reading level, plays narration, and optionally asks questions.
9. Staff exits patient mode: staff PIN in clinical mode, no PIN in demo mode (DECISIONS P8).
10. Staff exports the PDF booklet and hands it over through the practice's own channel.
11. Later, if the plan changes in Open Dental, the poller creates a new version and flags it on the Dashboard.

## 5. Functional requirements

### 5.1 Setup and configuration (SETUP)

| ID | Requirement | Acceptance criteria | Pri | Origin |
|---|---|---|---|---|
| FR-SETUP-01 | First launch in clinical mode opens a setup wizard; staff routes are unavailable until it completes. | With an empty data dir, every `/` route redirects to `/setup`; after completion the redirect stops and the wizard is unreachable except via Settings. | Should | C |
| FR-SETUP-02 | Wizard step: practice name, logo (PNG/SVG), optional brand colors. | Saved values appear in the patient header and booklet cover. Brand colors that fail the contrast check in UI_SPEC §2.4 are rejected with the failing pair named. | Must | C |
| FR-SETUP-03 | Wizard step: PMS selection (Open Dental only), read-only connection details, connection test. | Test reports success, auth failure, network failure, or "user has write privileges" as distinct messages. A user with write grants is a warning that must be acknowledged. Exact grant check TBD (verify in M2). | Should | C |
| FR-SETUP-04 | Wizard step: AI provider credentials. Clinical mode offers AWS only; demo mode reads OpenRouter key from environment. | Clinical mode UI shows no OpenRouter option, and the server refuses to construct an OpenRouter adapter in clinical mode even if config names one (test covers both). | Must | C |
| FR-SETUP-05 | Wizard step: shared staff account password and a 4 to 8 digit staff PIN. | Password and PIN are stored as Argon2 hashes; plaintext never appears in logs or DB (test greps both). | Should | C |
| FR-SETUP-06 | Wizard final step: acknowledgment of local-network deployment, provider data terms and the clinical-readiness notice (§9). | Completion is blocked until the checkbox is ticked; the acknowledgment is stored with a timestamp in `audit_events`. | Should | C |
| FR-SETUP-07 | Model choice is app-managed configuration, not an admin picker. | No UI control lists or selects models. Settings shows the active model IDs read-only. | Must | C |

### 5.2 Staff access and sessions (AUTH)

| ID | Requirement | Acceptance criteria | Pri | Origin |
|---|---|---|---|---|
| FR-AUTH-01 | Clinical mode: one shared staff account, password login. | Wrong password returns a generic error; 5 failures in 5 minutes trigger a backoff (P). Session cookie is `HttpOnly; Secure; SameSite=Strict`. | Should | C |
| FR-AUTH-02 | Demo mode: each visitor gets an isolated auto-session with no login. | Two browsers see independent presentations and edits; neither can read the other's by ID (API test). | Must | C |
| FR-AUTH-03 | Staff session expires after inactivity. | Default 30 minutes idle in clinical mode (P), configurable; expired session returns 401 and the UI routes to login. | Should | P |
| FR-AUTH-04 | State-changing requests require a CSRF token. | POST/PUT/DELETE without the header return 403 (test per route group). | Must | C |

### 5.3 Dashboard (DASH)

| ID | Requirement | Acceptance criteria | Pri | Origin |
|---|---|---|---|---|
| FR-DASH-01 | Four status cards: Ready to present, Generating, Plan changed, Needs review, each with count or first item and a link. | Counts equal the number of presentations in each state for today's patients; a card with zero items shows "None" and no link. | Must | C (design) |
| FR-DASH-02 | "Today's patients" table: time, patient, age, plan summary, procedure count, language, presentation status, action. | Rows come from the connector's recommended-patients call. Action label is Create, Open, Review or View according to status (UI_SPEC §5.1). | Must | C |
| FR-DASH-03 | Filter segmented control: All, Not started, Ready, Needs attention. | Needs attention = Plan changed or Needs review. Counts update without reload. | Must | C (design) |
| FR-DASH-04 | Recommended-patient rule: appointment today or in the next N days and an active plan with unscheduled planned procedures, ordered by appointment time. | Synthetic fixtures with known appointments produce the expected list and order. N default TBD (verify in M2). | Must | P |
| FR-DASH-05 | PMS sync indicator in the header ("Open Dental · synced 2 min ago"). | Shows the last successful poll; turns to the error style after 3 failed polls with "Open Dental unreachable". | Should | C (design) |

### 5.4 Patients directory (PAT)

| ID | Requirement | Acceptance criteria | Pri | Origin |
|---|---|---|---|---|
| FR-PAT-01 | Search by name, birth year or PMS patient number. | Typing 2+ characters returns results within 1 s on the synthetic set; recommended patients are pinned above results. | Must | C |
| FR-PAT-02 | Patient row expands to that patient's treatment plans (status, saved date, saved by, procedure count). | Plans list equals connector output. Plans with zero planned procedures are shown disabled with "No planned procedures". | Must | C |
| FR-PAT-03 | Patient names never appear in URLs. | Routes use opaque IDs; an automated crawl of all staff routes finds no name in any URL or query string. | Must | C |

### 5.5 Create and generation (GEN)

| ID | Requirement | Acceptance criteria | Pri | Origin |
|---|---|---|---|---|
| FR-GEN-01 | Create screen shows the selected plan read-only: phase, tooth, code, PMS description, fee, patient estimate, totals. | Values match the snapshot exactly, money formatted from integer cents. The screen states the values are read-only. | Must | C |
| FR-GEN-02 | Language select defaults to the PMS preferred language if supported, else English; reading level defaults to Standard. | Fixture with Spanish preference preselects Spanish; unknown language code preselects English and shows "Preferred language not set". | Must | C |
| FR-GEN-03 | If a clinical note linked to a planned procedure is used, the create screen shows which note excerpt will be used and says other notes stay on the server. | Only notes linked to planned procedures appear (P11). With no note, the screen says "No clinical note linked; content will describe common reasons". | Should | P |
| FR-GEN-04 | Generation runs as a server job with stages: import snapshot, match references, lock facts, write sections, review pass, assemble version. | `GET /api/staff/jobs/:id` returns current stage, per-section status and elapsed time. Stage names match UI_SPEC §4.9. | Must | C |
| FR-GEN-05 | Jobs survive browser close and server restart. | Killing the server mid-"write sections" and restarting resumes from the last completed stage; already-written sections are not re-requested (call count assertion with FakeTextModel). | Must | C |
| FR-GEN-06 | Patient-facing content never shows an unvalidated section. | A section with `ValidationState::Failed` or `NeedsReview` is excluded from Present and the booklet, and its slot shows the held-section state (UI_SPEC §6). | Must | C |
| FR-GEN-07 | Procedures with no knowledge-base entry (`Unsupported`) route to review instead of improvised text. | Fixture with an unmapped code yields that section in Needs review with reason "No reference · Dxxxx" and no AI-written body. | Must | C |
| FR-GEN-08 | Failed stages retry with a bound; unresolved failures end in `needs_review` or `failed` with a staff-readable reason. | Retry count bound TBD (AI_PIPELINE.md). A failed job shows Retry and the reason; it never shows a stack trace. | Must | C |
| FR-GEN-09 | Concurrent jobs are limited. | Worker pool size configurable; extra jobs show "Queued" with position. | Should | P |

### 5.6 Editor (EDIT)

| ID | Requirement | Acceptance criteria | Pri | Origin |
|---|---|---|---|---|
| FR-EDIT-01 | Editor is a navigable preview of the presentation with a section list (At a glance, Treatment journey, Procedures..., Costs, What happens next) and per-section state. | Section states shown: checked, edited, needs review, auto (Costs). Clicking a section scrolls the canvas to it. | Must | C |
| FR-EDIT-02 | Top bar: back, patient name, plan summary, version select, History, Regenerate, Export booklet, Present (primary, rightmost). | Present is the only filled primary button and sits at the top-right (DECISIONS U4). | Must | C |
| FR-EDIT-03 | Each content block shows one provenance label: Locked · from Open Dental, From clinical note, AI-written · checked, Edited by staff (with time). | Every rendered block has exactly one label (DOM test over all fixtures). | Must | C |
| FR-EDIT-04 | Locked facts (tooth, code, phase, fees, estimates) are not editable. | No input exists for locked fields; API rejects edits to Source blocks with 403. | Must | C |
| FR-EDIT-05 | Text blocks are editable inline. An edit records the previous text and offers Revert to AI version and Show diff. | Revert restores the exact prior text and label. Diff shows inserted and removed words. | Must | C |
| FR-EDIT-06 | Editing a block re-runs deterministic checks on that block and shows warnings in the Checks panel; warnings do not block Present unless they are blocking checks. | Adding an unsupported duration (e.g. "about 90 minutes") yields a warning. Adding diagnosis language or a cost figure that differs from the snapshot is blocking. Rule list in AI_PIPELINE.md. | Must | C |
| FR-EDIT-07 | An edit invalidates dependent artifacts: narration for that block's sentences in every variant, translations derived from it, and booklets. | Narration panel shows "Outdated after edit"; booklet export regenerates; translated variants of that block are marked stale. | Must | C |
| FR-EDIT-08 | Checks panel lists results for the current section: facts match PMS, no diagnosis or outcome promises, review-pass agreement with N references, and warnings. | Panel content matches stored validation results for the section. | Must | C (design) |
| FR-EDIT-09 | References panel lists the knowledge-base sources for the current section with review status. | Each reference shows title and source; entries with `review_status = unreviewed` show "Unreviewed" (D9). | Must | C |
| FR-EDIT-10 | Regenerate a single section or the whole version. | Regenerating one section keeps staff edits elsewhere; regenerating an edited block asks for confirmation first. | Should | C |

### 5.7 Versions and sync (VER)

| ID | Requirement | Acceptance criteria | Pri | Origin |
|---|---|---|---|---|
| FR-VER-01 | Every presentation version is bound to one snapshot fingerprint and is immutable once presented or exported. | Editing a presented version creates a new draft version (P). | Must | C |
| FR-VER-02 | Version history lists versions with time, cause (created, staff edit, plan changed, regenerated), status, and allows opening any version read-only. | List matches `versions` table; older versions cannot be presented unless restored as a new version. | Should | C |
| FR-VER-03 | Poller checks plans behind active presentations on an interval (default 5 minutes) and creates a new version on fingerprint change. | Unchanged fingerprint makes zero inference calls (counter test). | Should | C |
| FR-VER-04 | Diff handling: cost-only change refreshes costs deterministically; added procedure generates its section; removed procedure is dropped; sequence change refreshes journey; clinical change marks affected sections Needs review. | One fixture per case; each produces the expected section states. | Should | C |
| FR-VER-05 | Staff edits carry forward when their block's source facts are unchanged; otherwise the block is flagged as an edit conflict. | Conflict appears as "Plan changed · Edit conflict" on the Dashboard and in the editor. | Should | C |
| FR-VER-06 | A patient currently presenting stays on their version until exit. | Sync during an active present session does not change what `/api/present/*` returns. | Must | C |
| FR-VER-07 | PMS unavailable, plan deleted, or incomplete data produce visible states, never substitute values. | Each case has a fixture and a UI state (UI_SPEC §6). | Must | C |

### 5.8 Present mode and patient experience (PRES)

| ID | Requirement | Acceptance criteria | Pri | Origin |
|---|---|---|---|---|
| FR-PRES-01 | Present creates a server-side present session bound to (presentation, version) and locks the staff session. | While presenting, every `/api/staff/*` call with that staff session returns 423; browser back shows the present view, not staff data. | Must | C |
| FR-PRES-02 | Exiting Present requires the staff PIN in clinical mode and nothing in demo mode. | Clinical: wrong PIN 5 times locks exit for 60 s (P). Demo: an "Exit presentation" control returns to the editor in one tap. | Must | C |
| FR-PRES-03 | Patient UI is fully practice-branded with no DentalClarity name or logo. | Text search of the rendered patient DOM and booklet finds no "DentalClarity". | Must | C |
| FR-PRES-04 | Section nav: At a glance, Journey, Procedures, Costs, Next steps, reachable in any order. | Each tab is a button with `aria-current` on the active one; works with touch and keyboard. | Must | C |
| FR-PRES-05 | At a glance: odontogram with planned teeth highlighted, one-sentence plan summary, documented objective (from note if present), phase cards, estimated share. | Summary counts (teeth, phases) equal the snapshot. Without a note, the note callout is replaced by general text labelled as general. | Must | C |
| FR-PRES-06 | Treatment journey shows phases in order with the procedures in each, using only documented phases and dependencies. | No appointment counts, durations or dates appear unless present in the snapshot (validator test). | Must | C |
| FR-PRES-07 | Procedure detail: tooth and location, title, illustration, narration player, "What it is", collapsible "What happens during the visit", "Recovery", "If you wait", "Questions to ask", references. | Every procedure section in fixtures renders all headings or an explicit "Not available" state. | Must | C |
| FR-PRES-08 | "Why your dentist recommended it" appears only when a linked note supports it; otherwise "Common reasons for this procedure". | Fixture without a note never renders patient-specific reason text (validator + DOM test). | Must | C |
| FR-PRES-09 | What happens next: balanced outlook, benefits, possible consequences of waiting, practical next steps, suggested questions. | Validator rejects coercive phrasing from the AI_PIPELINE list; content contains no acceptance call-to-action. | Must | C |
| FR-PRES-10 | The patient session cannot reach other patients, plans, settings or credentials. | Fuzzed requests with other IDs on `/api/present/*` return 404; staff endpoints return 401/423. | Must | C |
| FR-PRES-11 | Present session expires. | Default 2 hours or on exit (P); after expiry the patient view shows "This presentation has ended. Please hand the device back to the front desk." | Should | P |

### 5.9 Odontogram (CHART)

| ID | Requirement | Acceptance criteria | Pri | Origin |
|---|---|---|---|---|
| FR-CHART-01 | SVG chart in Universal numbering: permanent 1 to 32, primary A to T; the chart shows the dentition the plan uses. | A plan with only primary teeth renders the primary arch; a mixed plan renders both. | Must | C |
| FR-CHART-02 | Teeth with planned procedures are highlighted with color and a non-color indicator, and are selectable. | Highlighted teeth have bold numbers, a thicker outline and a count badge; a grayscale screenshot still distinguishes them (visual test). | Must | C |
| FR-CHART-03 | Selecting a tooth lists every planned procedure on it and links to each detail. | Tooth 30 in the reference fixture lists Root canal, Build-up, Crown in plan order. | Must | C |
| FR-CHART-04 | Procedures with no tooth (quadrant, arch, mouth, none) are listed beside the chart, never pinned to an invented tooth. | Fixture with D1110 shows it under "Not tied to one tooth"; no tooth is highlighted for it. | Must | C |
| FR-CHART-05 | No historical restorations, implants or missing-tooth charting in v1. | Chart draws only planned-treatment highlights. | Must | C |

### 5.10 Language and reading level (LANG)

| ID | Requirement | Acceptance criteria | Pri | Origin |
|---|---|---|---|---|
| FR-LANG-01 | Patient can switch language from any page. | Switch to an uncached language shows a loading state on text blocks and never mixes languages within a section. | Must | C |
| FR-LANG-02 | Patient can switch reading level (Simple, Standard, Detailed) from any page. | Switching changes text and invalidates the playing narration for that block. | Must | C |
| FR-LANG-03 | Variants are produced on demand, validated like the original, and cached. | Second request for the same (version, language, level) makes no inference call. | Must | C |
| FR-LANG-04 | Locked facts are rendered by the app in every language, not translated by the model. | Numbers, tooth IDs and money in a translated variant equal the snapshot (validator). | Must | C |
| FR-LANG-05 | Untested languages are never described as validated. | Language list shows a "Tested" marker only for languages with ClarityEval results; others say "Machine-translated, not reviewed". | Must | C |
| FR-LANG-06 | Pediatric plans address the guardian. | Fixture with a patient under 13 (P threshold) produces guardian-directed copy; Dashboard shows "· guardian". | Should | C |

### 5.11 Narration (NARR)

| ID | Requirement | Acceptance criteria | Pri | Origin |
|---|---|---|---|---|
| FR-NARR-01 | Each section and procedure has a play control; nothing autoplays. | Loading any page produces no audio request until play is pressed. | Must | C |
| FR-NARR-02 | Audio is synthesized per sentence and cached by hash of text, voice and provider. | Replaying makes no TTS call; editing one sentence regenerates only that sentence. | Must | C |
| FR-NARR-03 | The active sentence is highlighted during playback. No word-level timing claims. | Highlight moves at sentence boundaries; copy and docs never say "word-by-word". | Must | C |
| FR-NARR-04 | Default-language narration for a new version is synthesized in the background after the text job completes. | Editor Narration panel shows per-language status (Ready, Generating, Outdated after edit, Not generated). | Should | P |
| FR-NARR-05 | TTS requests carry only the approved sentence text; no names or identifiers. | Personalization placeholders are rendered as neutral text before TTS (e.g. "you"); request log test. | Must | C |
| FR-NARR-06 | A transcript is always visible; audio is never the only channel. | Every narrated block has its text on screen. | Must | C |

### 5.12 Ask a question (QA)

| ID | Requirement | Acceptance criteria | Pri | Origin |
|---|---|---|---|---|
| FR-QA-01 | A text Q&A drawer is available on every patient page and is optional. | The floating button is present on all five sections; the drawer opens and closes without losing scroll position. | Must | C |
| FR-QA-02 | Before the first question the drawer states that answers come from the plan and trusted sources, that the dental team can read the conversation, and asks the patient not to share personal details. | Notice is visible above the input before and after the first question. | Must | C |
| FR-QA-03 | Answers are grounded in the presentation, snapshot facts and knowledge base and cite a source. | Each answer shows "Source: ..." or "From your treatment plan". | Must | C |
| FR-QA-04 | Requests for diagnosis, new recommendations, prognosis, medication advice or plan changes are declined and redirected to the dental team with a suggested question. | ClarityEval safety cases pass at the threshold set in CLARITYEVAL.md. | Must | C |
| FR-QA-05 | Urgent symptoms get escalation language. | Fixture questions mentioning swelling, fever or uncontrolled bleeding return the escalation template. Exact template TBD (AI_PIPELINE.md). | Must | C |
| FR-QA-06 | Staff can review conversations per presentation. | Conversation view in Presentations shows questions and answers with time; retention period configurable (default TBD, SECURITY.md). | Should | C |
| FR-QA-07 | Demo mode free text goes to OpenRouter with a visible "Demo: don't enter personal information" warning. | Warning shows in demo mode only. | Must | C |
| FR-QA-08 | Per-session question rate limit. | Default 20 questions per present session (P); limit message explains how to ask the front desk. | Must | P |

### 5.13 Costs (COST)

| ID | Requirement | Acceptance criteria | Pri | Origin |
|---|---|---|---|---|
| FR-COST-01 | Costs page shows estimated share, a bar split of insurance estimate and patient share, total fees, and per-phase tables. | All numbers come from the snapshot in integer cents; phase subtotals sum to the total (property test). | Must | C |
| FR-COST-02 | Every cost figure is labelled an estimate, with the plan date and "Your insurance decides the final amount". | Text present on Costs, At a glance and booklet cost page. | Must | C |
| FR-COST-03 | Unknown estimates show "Not available" and are excluded from sums with a note. | Fixture with one missing estimate shows the note and no fabricated total. | Must | C |
| FR-COST-04 | The model never sees or computes costs. | Prompt snapshot tests contain no money fields. | Must | C |

### 5.14 Booklet (BOOK)

| ID | Requirement | Acceptance criteria | Pri | Origin |
|---|---|---|---|---|
| FR-BOOK-01 | Export a practice-branded PDF for one version and one (language, reading level) variant. | PDF contains cover, overview with chart, journey, one page or more per procedure, costs, next steps, questions, references (UI_SPEC §7). | Must | C |
| FR-BOOK-02 | Booklet carries version and source identification. | Footer shows practice name, plan date, version number and a short version hash. | Must | C |
| FR-BOOK-03 | Booklet excludes held sections and says so. | A version with a Needs review section prints a placeholder "Your dental team will go over this part with you". | Must | C |
| FR-BOOK-04 | Fonts are embedded; non-Latin scripts render. | Booklet in Vietnamese, Arabic and Chinese renders without missing glyphs. Font coverage TBD (verify in the booklet milestone). | Should | C |
| FR-BOOK-05 | Export is cached by (version, variant, template hash). | Second export returns the same file without recompiling. | Should | P |

### 5.15 Analytics (ANLY)

| ID | Requirement | Acceptance criteria | Pri | Origin |
|---|---|---|---|---|
| FR-ANLY-01 | Engagement per present session: duration, sections visited, active time (approximate), narration plays, language and level used, reached end, questions asked. | Events stored locally; labelled "interaction, not comprehension or consent". | Should | C |
| FR-ANLY-02 | Generation metrics: duration per stage, provider, model ID, failures, estimated cost. | Analytics page shows p50/p90 generation time over the last 30 days. | Should | P |
| FR-ANLY-03 | No acceptance or conversion metrics. | No field or chart relates presentations to scheduled or accepted treatment. | Must | C |
| FR-ANLY-04 | No patient names or free text in analytics events. | Schema test: analytics table has no text columns beyond enums and IDs. | Must | C |

### 5.16 Settings (SET)

| ID | Requirement | Acceptance criteria | Pri | Origin |
|---|---|---|---|---|
| FR-SET-01 | Sections: Practice and branding, Open Dental connection, AI provider, Staff account and PIN, Data and retention, About. | Each section saves independently and shows last-changed time. | Should | C |
| FR-SET-02 | Secrets are write-only in the UI. | After saving, the field shows "Set · last changed ..." and never the value; API never returns it. | Must | C |
| FR-SET-03 | Branding preview shows the patient header, a phase card and a tooth with the chosen colors before saving. | Preview updates live; contrast failures are listed. | Should | P |
| FR-SET-04 | Mode (demo/clinical) is shown read-only. | No control changes mode at runtime. | Must | C |

### 5.17 Public demo (DEMO)

| ID | Requirement | Acceptance criteria | Pri | Origin |
|---|---|---|---|---|
| FR-DEMO-01 | Demo uses only the synthetic connector and synthetic data. | Demo build has no Open Dental connector compiled or configured (startup check). | Must | C |
| FR-DEMO-02 | Several pre-generated cases work with no inference. | With the OpenRouter key removed, pre-generated cases still open, present and export. | Must | C |
| FR-DEMO-03 | Live generation is rate-limited per visitor and capped at $25/month spend. | When the cap is reached, Create shows "Live generation is paused for this month. Try a ready-made case." | Must | C |
| FR-DEMO-04 | A persistent "Synthetic demo data, no real patient records" notice in the staff sidebar. | Visible on every staff page in demo mode. | Must | C |
| FR-DEMO-05 | Visitor data is reset after a period. | Visitor edits and generated versions expire after 24 hours (P). | Should | P |

## 6. Non-functional requirements

### 6.1 Performance

| ID | Requirement | Measurement | Pri |
|---|---|---|---|
| NFR-PERF-01 | Text generation for the reference plan completes in under 90 seconds at p50. | Benchmark below. The claim is made only after a recorded run. | Must |
| NFR-PERF-02 | Staff page first render under 2 s on a LAN workstation; patient section switch under 300 ms for cached content. | Browser-driven test on the demo build, cold and warm. | Should |
| NFR-PERF-03 | First narration sentence starts within 1.5 s of pressing play when cached, 4 s when not. | Timed in the e2e test with the fake TTS plus one live sample run. | Should |
| NFR-PERF-04 | Poller load on Open Dental stays bounded. | Queries per poll and duration logged; target TBD (verify in M2). | Should |

Generation benchmark definition (Proposed):

- Workload: the reference synthetic plan (6 procedures, 3 teeth, 2 phases, one linked note; the Okafor fixture) plus one larger plan (12 or more procedures) reported separately.
- Settings: English, Standard reading level, demo provider path, app-managed default models, cold cache (no cached variants), default worker pool.
- Start: the Create request is accepted by the server. Stop: job reaches `succeeded` or `needs_review`.
- Included: snapshot, reference matching, fact locking, section writing, deterministic validation, review pass, assembly. Excluded: narration audio, other languages, booklet. Narration is timed and reported as a separate number.
- Runs: 10 sequential runs, spread over at least 2 different days; report p50, p90, max, failure count, model IDs, provider and date.
- Pass: p50 under 90 s and no run in `failed`. If p90 exceeds 120 s, the UI copy must not promise a time.
- Run the same benchmark on the clinical path against the dev AWS account and report it separately (Should).

### 6.2 Accessibility

| ID | Requirement | Measurement | Pri |
|---|---|---|---|
| NFR-A11Y-01 | Staff and patient UI target WCAG 2.2 AA. | Automated axe scan with zero serious violations on every screen in UI_SPEC; manual keyboard pass. No conformance claim beyond what was tested. | Must |
| NFR-A11Y-02 | All text and UI pairs meet AA contrast, including practice-overridden brand colors. | Token pairs in UI_SPEC §2.4; brand override validation (FR-SETUP-02). | Must |
| NFR-A11Y-03 | Touch targets in patient UI are at least 44×44 px. | Layout test at 1180×820 and 820×1180. | Must |
| NFR-A11Y-04 | Reduced motion is respected. | With `prefers-reduced-motion: reduce`, no transitions over 0 ms except opacity fades of 150 ms or less. | Must |
| NFR-A11Y-05 | Text alternatives for illustrations, chart and narration. | Every SVG illustration has a title and description; chart has a text list equivalent. | Must |
| NFR-A11Y-06 | Patient text is resizable to 200% without loss of content. | Manual check at 200% zoom on iPad landscape. | Should |

### 6.3 Privacy

| ID | Requirement | Measurement | Pri |
|---|---|---|---|
| NFR-PRIV-01 | Prompts use an allowlist of typed fields. Names, IDs, contact details, exact dates and insurance identifiers never go to a model. | Prompt snapshot tests; a test fails if any snapshot field outside the allowlist appears. | Must |
| NFR-PRIV-02 | Personalization placeholders are substituted locally after generation. | Test asserts placeholders in model output and real values only in rendered output. | Must |
| NFR-PRIV-03 | Logs redact PHI via `Sensitive<T>`. | Log capture test over a full workflow finds no fixture name, DOB or patient number. | Must |
| NFR-PRIV-04 | No real PHI before the deadline. | Process control; release checklist item. | Must |
| NFR-PRIV-05 | Docs never call placeholder masking de-identification. | Docs review item. | Must |

### 6.4 Reliability and security

| ID | Requirement | Measurement | Pri |
|---|---|---|---|
| NFR-REL-01 | Job restart recovery. | FR-GEN-05 test. | Must |
| NFR-REL-02 | Pre-generated demo cases work with providers down. | FR-DEMO-02 test. | Must |
| NFR-REL-03 | PMS failure never corrupts or replaces existing versions. | Fault-injection test on the connector. | Should |
| NFR-REL-04 | SQLite backup and restore are documented and tested. | Restore of a backup reproduces presentations and audio links. | Later |
| NFR-SEC-01 | Controls in SECURITY.md (TLS in clinical mode, encrypted secrets, CSRF, rate limits, session scoping) are implemented and tested. | SECURITY.md test list. | Must |

## 7. Demo mode and clinical mode

| Aspect | Demo | Clinical |
|---|---|---|
| Data source | Synthetic connector only | Open Dental read-only SQL (synthetic allowed for training) |
| AI text, translation, Q&A | OpenRouter, `deepseek/deepseek-v4.1-flash-1017` at high reasoning effort (listed on OpenRouter as of 2026-11-25) | AWS Bedrock in the practice's account (models chosen and verified in M9) |
| Narration | OpenRouter, `heygen/heygen-voice-1-turbo` (listed on OpenRouter as of 2026-11-25); exact speech endpoint and audio format to verify in M5 | Amazon Polly (verified in M9) |
| OpenRouter allowed | Yes | No, refused in code |
| Staff login | Auto-session per visitor | Shared staff account, password |
| Exit Present | No PIN | Staff PIN |
| Setup wizard | Skipped; config from environment | Required on first launch |
| Spend control | $25/month cap, per-visitor rate limits | Practice-configured |
| Free-text warning | "Don't enter personal information" in Q&A | Practice-review notice only |
| Branding | Fictional practice ("Maple Grove Family Dental") | Practice's own |
| Hackathon status | Must, fully polished | Built and smoke-tested against a dev AWS account; may be thinner (P3) |

## 8. Out of scope

- Patient portal, public links, tunnels, email or SMS delivery of presentations.
- Individual staff accounts, roles, RBAC, multilingual staff UI.
- Other PMS integrations (the connector trait allows them later).
- Writing to Open Dental in any form.
- Voice conversation, speech-to-text, real-time voice AI.
- Quizzes, gamification, comprehension scoring.
- 3D models, animated procedure simulations, AI-generated anatomy images.
- Local LLM inference.
- Full historical charting (existing restorations, implants, missing teeth).
- Insurance or appointment education unrelated to the plan.
- Treatment acceptance tracking, conversion analytics, sales scripts.
- Admin model picker.
- Commercial SaaS: billing, tenants, license server, hosted clinical service.

## 9. Clinical-readiness gaps

These stay open at the hackathon release. The setup wizard and README state that the software is not ready for real patient data until they are resolved by the deploying practice.

| Gap | Why it matters | What closing it needs |
|---|---|---|
| Shared staff account (P7) | No individual attribution of edits, presents or Q&A review; may conflict with unique user identification requirements. | Individual accounts, per-user audit events, role separation. |
| Clinician review of the knowledge base (D9) | Entries stay `unreviewed` until a dentist signs off. | Dentist sign-off workflow and recorded review status per entry. |
| Provider agreements | Bedrock/Polly eligibility and a BAA are the practice's responsibility; eligibility alone is not compliance. | Practice-side BAA, region and logging configuration, verified per SECURITY.md. |
| Clinical note use (P11) | Notes may carry identifiers or unrelated findings in free text. Staff confirm each note-derived block before patients see it. | Redaction rules in AI_PIPELINE.md; ClarityEval cases on note handling; clinician review of note-derived output. |
| Retention and deletion | Q&A, analytics and audio retention defaults are TBD. | Documented defaults, deletion job, backup policy. |
| Open Dental semantics | Table meanings are unverified for target versions. | M2 verification against a real install. || Language quality | Most languages are untested. | ClarityEval multilingual results and native-speaker review per language. |
| Installer and service hardening | Windows service, firewall, TLS certificate handling are thin for the hackathon. | Security assessment by the practice; signed installer. |
| Illustration review | Original SVGs need clinician review. | Recorded review per illustration. |

## 10. Hackathon success metrics

Judging weights were reported as AI/ethics 25%, social impact 25%, coding/implementation 20%, UX/accessibility 15%, creativity 15%; recheck against the official rules before submission.

| Metric | Target | How measured |
|---|---|---|
| Demo workflow completes | Create, generate, edit, Present, exit, export on a fresh visitor session with no errors | Scripted e2e run against the public URL on submission day |
| Generation time | p50 under 90 s on the benchmark in §6.1 | Recorded benchmark run |
| Fact fidelity | 0 locked-fact mismatches across all demo fixtures | Validator report |
| Held sections | Every unsupported or failed section is held, none shown | Fixture suite |
| Accessibility | 0 serious axe violations on all patient screens; keyboard-only pass | Scan report and manual checklist |
| Languages demonstrated | At least 3 languages with ClarityEval results, including one non-Latin script | CLARITYEVAL.md results |
| ClarityEval | Benchmark published with dataset, runner and results for at least 3 model families | Repo contents |
| Clinician input | Feedback from at least 2 dentists on outputs, reported as given | Paper section; no claim if not obtained |
| Submission package | Description, tech list, public repo, working demo link, 2 to 5 minute video | Checklist (DECISIONS P4) |

## 11. Open questions

- Exact display and staff confirmation wording for note-derived explanations (P11).
- Pediatric threshold for guardian framing (proposed under 13).
- Default N days for recommended patients; retention defaults for Q&A and analytics.
- Speech model for demo narration: the owner confirmed `heygen/heygen-voice-1-turbo`, listed on OpenRouter as of 2026-11-25. Still to check in M5: the exact speech endpoint and audio format. Voice quality is judged in a listening test.
- Whether the staff primary fill color is darkened for contrast (UI_SPEC §2.4).
