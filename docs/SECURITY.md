# DentalClarity security

Status: v0.1, written at project start. [DECISIONS.md](DECISIONS.md) wins where this doc disagrees. Related: [ARCHITECTURE.md](ARCHITECTURE.md), [AI_PIPELINE.md](AI_PIPELINE.md), [IMPLEMENTATION.md](IMPLEMENTATION.md), [PRD.md](PRD.md).

Tags: [C] is confirmed by the owner (DECISIONS id given). [P] is proposed by engineering and can change. TBD must be verified in the named milestone.

DentalClarity is a prototype. It must not be used with real patient data until every item in the [clinical-readiness gap checklist](#clinical-readiness-gap-checklist) is closed [C, D3].

## Threat model

### Assets

| Asset | Where | Why it matters |
|---|---|---|
| PHI: snapshots, generated text, linked notes, Q&A, audio, PDFs | SQLite, `artifacts/`, backups | confidentiality |
| Educational content integrity | versions, blocks, KB | wrong or pressuring content can harm a patient's decision |
| Open Dental read credentials | `secrets` table | read access to the whole practice database |
| AWS credentials (clinical) | `secrets` table | Bedrock/Polly spend; access to whatever the IAM policy allows |
| OpenRouter key (demo) | container env var | spend, abuse under the project's name |
| Staff password, staff PIN | Argon2id hashes in `staff_account` | access to every patient in the directory |
| Session tokens | cookies, hashed in `sessions` | session hijack |
| Audit and job logs | `audit_events`, `logs/` | accountability; also may leak data if logging is careless |

### Actors

| Actor | Capability | Main controls |
|---|---|---|
| Curious patient holding the tablet | full UI access to the Present view; can tap Back, edit the URL, open dev tools if the browser allows | server-side staff lock with `423`, present session bound to one version, PIN to exit (clinical), no staff data in the Present bundle state |
| Other people in the office | see an unattended staff screen | idle timeout, lock on Present, no patient names in URLs or tab titles |
| LAN attacker (compromised workstation, misconfigured guest Wi-Fi) | sniff, scan, replay, brute-force login | TLS, `Secure`/`HttpOnly` cookies, CSRF tokens, rate limits, backoff, bind to LAN interface only |
| Malicious demo visitor | scripted requests, cost abuse, prompt injection, XSS attempts, trying to reach secrets | per-visitor and global limits, $25 cap, input caps, text-only rendering, no uploads, secrets in env only |
| Prompt injection via PMS free text | text in clinical notes, procedure descriptions, phase labels | allowlist (descriptions and labels never sent), notes framed as data, no tools for any model, schema and validators, fail closed |
| Prompt injection via patient questions | arbitrary text into Q&A | per-session context holds one presentation only, no tools, output validators, fixed fallbacks |
| Insider using the shared account | everything staff can do | audit events; attribution is impossible with one account, a known gap [C, P7] |
| Thief with disk or backup | offline access to files | full-disk encryption (practice responsibility), DPAPI-wrapped secrets, backup encryption (TBD) |
| Supply-chain attacker | malicious crate or CI action | lockfile, cargo-deny, cargo-audit, pinned actions |

Prompt injection cannot trigger actions because no model has tools, network access or write access. The worst case is wrong or harmful text, which the deterministic validators and the reviewer pass in [AI_PIPELINE.md](AI_PIPELINE.md) are there to catch. A cross-patient leak through Q&A cannot happen by construction, because the context is built from the present session's single version.

## Mode differences

Mode is fixed at startup and cannot change at runtime [C, ARCH 1].

| Control | `demo` | `clinical` |
|---|---|---|
| Data | synthetic only [C, D1] | real PMS via read-only SQL (no real PHI before the deadline [C, D3]) |
| AI providers | OpenRouter for all [C, D1] | Bedrock and Polly; OpenRouter adapter cannot be constructed [C, D2] |
| Staff auth | auto-created visitor session, no password | shared staff account with password |
| Leave Present | no PIN [C, P8] | staff PIN [C, P8] |
| TLS | terminated by the hosting platform | rustls in-process |
| Secrets | env vars only | encrypted `secrets` table, DPAPI-wrapped key |
| Limits | per-visitor rate limits, $25/month cap [C, D5] | practice-configured, login and PIN limits always on |
| Setup wizard | disabled; config from env | first launch only, local or with a setup token |
| Logging of prompt text | never in production builds | never |

## Authentication

[P unless marked]

### Staff account (clinical)

- One shared account [C, P7]. Password at least 12 characters, checked against a bundled list of common passwords. No composition rules.
- Hash: Argon2id via the `argon2` crate, starting at m = 19 MiB, t = 2, p = 1. Tune at install so one hash takes 200 to 500 ms on the target machine, and store the parameters with the hash so they can be raised later.
- Staff PIN: 6 to 8 digits, hashed separately with the same Argon2id settings. The PIN only unlocks a staff session that Present mode locked. It never logs anyone in.
- Password change invalidates every staff session.

### Sessions and cookies

| Item | Staff session | Present session |
|---|---|---|
| Token | 32 random bytes from the OS CSPRNG, base64url | same |
| Stored as | SHA-256 of the token in `sessions` | SHA-256 in `present_sessions` |
| Cookie | `__Host-dc_staff`; `Secure; HttpOnly; SameSite=Strict; Path=/` | `__Host-dc_present`; same flags |
| Idle timeout | 30 min | 60 min |
| Absolute lifetime | 12 h | 4 h |
| Rotation | new token at login and at unlock | new token per present session |
| Bound to | the shared account | `(presentation_id, version_id)` and the staff session that created it |

API responses carry `Cache-Control: no-store`. The `__Host-` prefix needs TLS. In demo mode the platform terminates TLS, and the app trusts `X-Forwarded-Proto` only from the configured proxy address.

### CSRF

- Double-submit token: a non-HttpOnly `dc_csrf` cookie, echoed in an `X-DC-CSRF` header on every state-changing request; compared in constant time [ARCH 5].
- `SameSite=Strict` on session cookies.
- `Origin` (or `Referer`) must match the configured host on every non-GET request.
- No state change on GET.

### Rate limits and lockout

| Endpoint | Limit | On exceeding |
|---|---|---|
| `POST /api/auth/login` | 5 per minute per IP; 20 per minute global | `429` with `Retry-After` |
| Login failures | after 5 failures from an IP, delay doubles from 1 s up to 5 min | per-IP backoff, not an account lockout (one shared account means a hard lockout would let any LAN device lock out the whole practice) |
| `POST /api/present/exit` (PIN) | 5 wrong PINs per present session | the PIN is disabled for that lock; unlocking then needs the full staff password |
| Global failed login count | over 50 in 15 min | audit event and a banner on next staff login |
| Other staff API | 300 per minute per session | `429` |

All limits are in-memory token buckets keyed by IP and session, reset on restart. A restart-persisted counter is not needed for the prototype.

## Present-mode isolation

Confirmed flow from ARCHITECTURE section 5 [C, P8], with details [P]:

1. Staff click Present. `POST /api/staff/presentations/:id/present` creates a `present_session` bound to `(presentation_id, version_id)`, sets `staff_session.locked = true` in SQLite, and sets the `__Host-dc_present` cookie.
2. The frontend does a full page load of `/present`, which drops all staff state held in WASM memory. The Present bundle never fetches staff data.
3. While locked, every `/api/staff/*` call with that staff session returns `423 Locked` and no body beyond an error code. This covers other tabs, the back button and direct API calls. The staff SPA shows a lock screen on `423`.
4. `/api/present/*` handlers take the presentation and version only from the present session. Any id in the path or body is ignored or must match, otherwise `403`.
5. Exit, clinical: `POST /api/present/exit` with the PIN (or the password after 5 wrong PINs). On success the present session ends, the staff session unlocks with a new token, and an audit event is written.
6. Exit, demo: `POST /api/present/exit` without a PIN [C, P8]. Accepted because demo data is synthetic.
7. If the present session expires, the staff session stays locked. Unlocking needs the PIN or the password.
8. Other workstations log in with their own staff session, which is not locked. Locking applies per session.

Fullscreen is cosmetic and is never treated as an access control [C, brainstorm 3]. For a stronger lock on a shared tablet, practices can use the OS kiosk or guided-access feature. That is optional and documented in the install guide.

## Authorization matrix

Rows are endpoint groups. A means allowed; dash means rejected with the code shown.

| Endpoint group | Anonymous | Staff (unlocked) | Staff (locked) | Present session | Demo visitor |
|---|---|---|---|---|---|
| `GET /` static bundle, fonts, illustrations | A | A | A | A | A |
| `GET /healthz` (no details) | A | A | A | A | A |
| `/api/setup/*` | first launch only, and only from localhost or with the one-time setup token | 403 after setup | 403 | 403 | disabled (404) |
| `POST /api/auth/login`, `logout` | A (login) | A (logout) | logout only | 403 | 404 |
| `/api/staff/patients`, `plans` (read PMS) | 401 | A | 423 | 401 | A (synthetic, read-only) |
| `/api/staff/presentations` create, edit, regenerate, history | 401 | A | 423 | 401 | A, own visitor scope only |
| `/api/staff/jobs/:id` | 401 | A | 423 | 401 | A, own jobs only |
| `/api/staff/presentations/:id/export` (PDF) | 401 | A | 423 | 401 | A, own scope |
| `/api/staff/presentations/:id/present` | 401 | A | 423 | 401 | A |
| `/api/staff/qa` (read conversations) | 401 | A | 423 | 401 | own scope |
| `/api/staff/analytics` | 401 | A | 423 | 401 | aggregate only |
| `/api/staff/settings` (branding, sync) | 401 | A | 423 | 401 | 403 |
| `/api/staff/secrets` (write-only) | 401 | A, write only, never returns values | 423 | 401 | 403 |
| `/api/present/content`, `variant`, `audio/:key` | 401 | 401 | 401 | A, bound version only | A, bound version only |
| `/api/present/qa` | 401 | 401 | 401 | A, caps apply | A, caps and budget apply |
| `/api/present/analytics` (engagement events) | 401 | 401 | 401 | A, write only | A, write only |
| `POST /api/present/exit` | 401 | 401 | 401 | A (PIN in clinical) | A |

Secrets are write-only through the API. The settings page shows "set" and the time it was last changed, never the value.

## Transport

- Clinical mode serves HTTPS only, with rustls (TLS 1.2 and 1.3). Port 80, if enabled, only redirects. [P]
- The service binds to a configured LAN interface address. The installer's firewall rule allows the LAN subnet only (M9).
- HSTS is sent only when the certificate is trusted by the clients (option 1 or 2 below), so a self-signed setup does not lock clients out.

### Certificate options

| Option | Setup | Trade-off |
|---|---|---|
| 1. Practice internal CA or IT-provided cert | import PEM in Settings | preferred; needs IT support |
| 2. App-generated local CA plus server cert (`rcgen`) | installer creates a CA, exports its cert, staff install it on each workstation and tablet | works offline; each device needs a one-time trust step; the CA private key must be protected like a secret |
| 3. Self-signed with a browser warning | none | discouraged; trains staff to click through warnings. Allowed only for evaluation |

- The certificate needs a SAN for the stable hostname or IP the tablets use. Renewal reminder 30 days before expiry. [P]
- The Open Dental MySQL connection is local when DentalClarity runs on the OD server. If it runs on a separate host, MySQL TLS is required; whether the target OD MySQL/MariaDB setup supports it is TBD (verify in M2).
- Demo mode: TLS terminates at the hosting platform (D6 is still open).

## Secrets at rest

| Secret | Clinical (Windows) | Demo (container) |
|---|---|---|
| PMS DB password | `secrets` row, AES-256-GCM with a data key | not used |
| AWS credentials | same | not used |
| OpenRouter key | refused in clinical mode | env var `DC_OPENROUTER_API_KEY` |
| Data key | wrapped with DPAPI, machine scope [ARCH 5] | not used |
| TLS private key, local CA key | file in data dir, ACL restricted | platform-managed |

- DPAPI machine scope lets any process on the machine unwrap the key. The data directory ACL is therefore limited to the service account and Administrators, set by the installer (M9). A dedicated service account with user-scope DPAPI is the stronger alternative and is TBD (verify in M9).
- The SQLite database itself is not encrypted by the app. Practices are expected to run BitLocker or equivalent full-disk encryption. Encrypting the database (for example SQLCipher) is a listed gap.
- AWS credentials: an IAM principal limited to `bedrock:InvokeModel` (and the streaming variant if used) on the selected model ARNs, plus `polly:SynthesizeSpeech`, in the practice's chosen region. Long-lived access keys are the simple path and need rotation. Short-lived alternatives such as IAM Roles Anywhere are TBD (verify in M9).
- Secrets are never logged, never returned by the API, never in fixtures, and never in source control. CI runs a secret scanner (gitleaks, [P]).

## Open Dental least-privilege user

[C, D7] DentalClarity only reads. The example below is a starting point. Verify the table list against the target OD version (M2), and add column-level grants on `patient` to leave out fields not needed (for example SSN and address) once the column names are confirmed.

```sql
-- Example only. Verify table list against OD version (M2).
CREATE USER 'dentalclarity_ro'@'<dentalclarity-host>' IDENTIFIED BY '<generated, 24+ chars>';
GRANT SELECT ON opendental.patient       TO 'dentalclarity_ro'@'<dentalclarity-host>';
GRANT SELECT ON opendental.treatplan     TO 'dentalclarity_ro'@'<dentalclarity-host>';
GRANT SELECT ON opendental.proctp        TO 'dentalclarity_ro'@'<dentalclarity-host>';
GRANT SELECT ON opendental.procedurelog  TO 'dentalclarity_ro'@'<dentalclarity-host>';
GRANT SELECT ON opendental.procedurecode TO 'dentalclarity_ro'@'<dentalclarity-host>';
GRANT SELECT ON opendental.claimproc     TO 'dentalclarity_ro'@'<dentalclarity-host>';
GRANT SELECT ON opendental.appointment   TO 'dentalclarity_ro'@'<dentalclarity-host>';
-- Tables for linked procedure notes, provider display names and phase/priority
-- definitions: TBD (verify in M2).
```

- Host-restricted (`'@<host>'`, never `'%'`). No `INSERT`, `UPDATE`, `DELETE`, `CREATE`, `FILE`, `PROCESS` or `SUPER`.
- The connector also opens its connection with `SET SESSION TRANSACTION READ ONLY` as a second guard.
- Query load limits (polling interval, statement timeout) are set in M2 after measuring on a real OD install.

## Data minimization and retention

All defaults are [P] and need owner confirmation. Clinical values apply per practice and can be changed in Settings.

| Data | Contents | Clinical default | Demo default | Deletion |
|---|---|---|---|---|
| Snapshots | normalized plan, display context (first name, age band, language) | kept while the presentation exists | 24 h, visitor scope | deleted with the presentation |
| Versions, blocks, variants | generated and edited text | kept while the presentation exists; presentations with no activity for 12 months are flagged for deletion | 24 h | hard delete rows, then GC artifacts |
| Q&A conversations | questions and answers | 30 days | 24 h | scheduled hard delete |
| Analytics events | sections visited, plays, language, active time; no free text | raw 90 days, then aggregated without presentation linkage | 24 h | scheduled delete |
| `ai_calls` | metadata only, no prompt or response text | 12 months | 30 days | scheduled delete |
| Audio clips | per-sentence audio | while referenced by a variant | while referenced | daily GC of unreferenced files |
| PDF booklets | rendered booklet | 7 days (re-exportable) | 24 h | scheduled delete |
| Audit events | logins, unlocks, exports, deletions, review actions | 12 months | 7 days | scheduled delete |
| Logs | redacted operational logs | 30 days, daily rotation | platform default, max 7 days | rotation |
| Backups | DB and artifacts | practice-managed | none | deleted data stays in backups until those backups age out; documented |

- SQLite runs with `PRAGMA secure_delete = ON`, and a weekly `VACUUM` reclaims freed pages.
- Deleting a presentation removes all versions, variants, Q&A, analytics links and artifacts not referenced elsewhere, and writes an audit event without patient details.
- Provider-side retention: Bedrock, Polly and OpenRouter data handling must be checked against each provider's current terms. TBD (OpenRouter verified in M5; Bedrock and Polly verified in M9). Provider zero-retention settings are not a substitute for a BAA or de-identification.
- No patient names or PMS ids in URLs. Routes use DentalClarity's own random ids. Page titles in the Present view show the practice name only.

## Logging redaction

- PHI-bearing values are wrapped in `Sensitive<T>`, whose `Debug` and `Display` print `[redacted]` [C, E9]. Snapshot, note, question, answer and generated-text types hold their strings as `Sensitive<String>`.
- `tracing` spans log ids, stage names, durations, counts and error codes. Prompt and response text is never logged in any mode in release builds.
- Provider error bodies can echo input, so they are parsed into an error code and a `Sensitive` message.
- HTTP access logs record method, route template (`/api/staff/presentations/:id`), status and latency. No query strings, no bodies.
- A unit test serializes every API and domain type with sample PHI through `Debug` and the log formatter, and fails if the sample strings appear.
- Panics use a custom hook that logs location only.

## Output sanitization of AI text

- Generated, translated and Q&A text is rendered as text nodes only. Leptos `inner_html` is banned in `dc-web` by a clippy `disallowed_methods` rule, with one reviewed exception for our own static SVG assets.
- Validator V15 rejects HTML, Markdown and URLs in model output. Reference links come only from KB entries, which allow `https` URLs only.
- PDF: text goes to Typst through the JSON data model and is placed as content strings, never evaluated as Typst markup. Test fixtures include Typst control characters (`#`, `$`, `[`, `]`).
- Response headers on every page: `Content-Security-Policy: default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self'; img-src 'self' data:; media-src 'self'; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'` (exact Leptos/Trunk requirements TBD, verify in M1), plus `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer`, and a `Permissions-Policy` that disables camera, microphone and geolocation.

## Demo abuse controls

[C, D5 for the cap; the numbers are P]

| Control | Value |
|---|---|
| Visitor identity | random visitor id in a `__Host-` cookie on first visit, plus client IP from the trusted proxy |
| Visitor isolation | presentations, edits, jobs and Q&A are scoped to the visitor id; the synthetic dataset is read-only and shared; visitor data is purged after 24 h |
| New generations | 3 per hour per visitor, 10 per day per IP |
| On-demand variants | 10 per hour per visitor |
| Q&A | 20 questions per hour per visitor, 500 characters each |
| Staff edits | 2,000 characters per block |
| Concurrency | 2 generation jobs globally, 1 per visitor; later jobs queue with a visible position |
| Budget | $25 per month, reservation-based guard, 80% soft stop on new generations (see [AI_PIPELINE.md](AI_PIPELINE.md)) |
| Uploads | none; no endpoint accepts files |
| PMS | synthetic connector only; no DB credentials exist in the container |
| PII warning | shown next to every free-text input (Q&A, edits): "This is a public demo with made-up patients. Don't enter real personal or health information." |
| Secrets | OpenRouter key in env only; never sent to the browser; settings page disabled |
| Pre-generated cases | always available, including after the cap is hit |
| Bot traffic | none at launch beyond the limits; a hosting-platform challenge is added only if abuse appears |

## Dependency and supply-chain hygiene

[P]

- `Cargo.lock` committed. Toolchain pinned in `rust-toolchain.toml`. Trunk and wasm-bindgen versions pinned.
- `cargo-deny` in CI: advisories, licenses (Apache-2.0-compatible allowlist), banned crates, crates.io as the only source.
- `cargo-audit` in CI on every PR and on a weekly schedule.
- Dependabot or Renovate for crates and GitHub Actions. Actions pinned by commit SHA, with `permissions: read-all` by default.
- New dependencies are reviewed for maintenance status and need a reason in the PR.
- Demo container: distroless runtime image pinned by digest, non-root user, read-only root filesystem with a writable `/data`.
- SBOM per release (`cargo-cyclonedx`). Windows code signing is TBD (M9).

## HIPAA posture

- DentalClarity is open-source software. Being open source or self-hosted does not make it, or any deployment of it, HIPAA compliant, and the project maintainers are not a business associate of any practice that installs it.
- A practice using real PHI is responsible for its own risk analysis, policies, access controls, BAAs and configuration.
- A BAA with AWS and the use of HIPAA-eligible services are necessary for the clinical provider path, but they do not make a deployment compliant end to end. Current eligibility of Bedrock and Polly, region and data handling must be confirmed by the practice (TBD, verify in M9).
- OpenRouter is for synthetic data only. Clinical mode refuses it in code [C, D2].
- The shared staff account [C, P7] conflicts with the HIPAA Security Rule's unique user identification requirement and prevents per-person audit attribution.
- Placeholder masking and note stripping are minimization, not de-identification [C, D10].
- This software is not for use with real PHI until the gaps below are closed and the practice has done its own security assessment.

## Clinical-readiness gap checklist

| # | Gap | Needed before real PHI |
|---|---|---|
| G1 | Shared staff account | individual accounts, unique user identification, per-user audit trail |
| G2 | No RBAC | at least admin and staff roles; settings and secrets restricted to admin |
| G3 | Database not encrypted by the app | encrypted DB or a documented, verified full-disk encryption requirement |
| G4 | Backup encryption and restore | encrypted backups, tested restore procedure |
| G5 | Provider agreements | signed AWS BAA, confirmed service eligibility and region, documented data flows |
| G6 | AWS credential handling | least-privilege IAM, rotation or short-lived credentials |
| G7 | TLS on all clients | trusted certificate on every workstation and tablet; no click-through warnings |
| G8 | Audit log integrity | append-only or hash-chained audit events, retention set by the practice |
| G9 | Clinical content review | KB entries reviewed by a dentist (`review_status` not `unreviewed`) [C, D9]; validator and reviewer measured in ClarityEval |
| G10 | Open Dental semantics | M2 verification complete for the deployed OD version, including note linkage |
| G11 | Note handling | staff confirmation per note-derived block (P11); clinician review of note-derived output |
| G12 | Retention | practice-approved retention values configured |
| G13 | Incident response | documented contact, breach-assessment procedure, log access |
| G14 | Independent review | external security review or penetration test of the clinical build |
| G15 | Windows hardening | service account, data-dir ACLs, firewall rule verified (M9) |

## Reporting a vulnerability

- Use GitHub private vulnerability reporting on the DentalClarity repository (Security tab, "Report a vulnerability"). Do not open a public issue.
- Include affected version or commit, steps to reproduce, and impact.
- This is a volunteer project: acknowledgement within 7 days and a fix or mitigation plan within 30 days for confirmed issues, as a goal and not a guarantee.
- Testing the public demo is welcome within limits: synthetic data only, no denial-of-service or high-volume testing, no attempts to spend the AI budget, and no social engineering.
- Accepted reports are credited in the release notes unless the reporter prefers otherwise.
- A `SECURITY.md` at the repository root (or a link to this file) is needed for GitHub to show the policy, and private reporting has to be enabled in the repository settings.
