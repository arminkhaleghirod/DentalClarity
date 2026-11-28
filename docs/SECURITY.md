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
| Staff auth | visitor session created by `POST /api/visit`, no password | shared staff account with password |
| Leave Present | no PIN [C, P8] | staff PIN [C, P8] |
| TLS | terminated by the hosting platform | rustls in-process (M9); until then a TLS proxy in front, see the startup check |
| Secrets | env vars only | encrypted `secrets` table, DPAPI-wrapped key |
| Limits | per-visitor rate limits, $25/month cap [C, D5] | practice-configured, login and PIN limits always on |
| Setup wizard | disabled; config from env | first launch only, with the one-time setup code printed to the server console |
| Startup check | none | refuses to start unless `DC_PUBLIC_ORIGIN` is `https://` or the bind address is loopback |
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
| Cookie | one session cookie shared with demo visitors (see below) | `__Host-dc_present` over HTTPS, `dc_present` over plain HTTP, same flags |
| Idle timeout | 30 min | 60 min |
| Absolute lifetime | 12 h | 4 h |
| Rotation | new token at login and at unlock; the unlocked session keeps the absolute expiry of the locked one | new token per present session |
| Bound to | the shared account | `(presentation_id, version)` and the staff or visitor session that created it |

Implemented in M1: staff and demo visitor sessions use one cookie, named `__Host-dc_session` over HTTPS and `dc_session` over plain HTTP (local development only). Flags are `HttpOnly; SameSite=Strict; Path=/`, plus `Secure` over HTTPS. There is no `Max-Age`, because the server enforces lifetimes. A demo visitor session has a 24 h idle timeout and a 24 h absolute lifetime. Implemented in M5: present sessions live in `present_sessions` and use their own cookie. `/api/staff/*` reads only the session cookie and `/api/present/*` reads only the present cookie, so neither opens the other. Ending or signing out of the locked session also ends its present session. While a patient is viewing, each present request keeps the locked session from idling out, so staff can still unlock afterwards. This is deliberate: a present request checks the locked session's 12 h absolute expiry but not its 30 min idle timeout, because the present session's own 60 min idle window governs while the screen is handed to a patient. Checking both would end a patient's screen after 30 quiet minutes. The locked session opens only through the exit (PIN or password in clinical mode), and once the present session ends its idle timeout runs again from the last present request.

- The cookie name and the `Secure` flag follow the scheme of `DC_PUBLIC_ORIGIN`. The app does not read `X-Forwarded-Proto`.
- The session kind must match the mode: demo accepts only visitor sessions, clinical accepts only staff sessions.
- The mode is stored in settings. When the server starts in a different mode than last time, all sessions are deleted.
- `GET /api/session` is read-only. Demo visitor sessions are created by `POST /api/visit`, which is Origin-checked and limited to 30 new visitor sessions per minute per client address.
- A locked session whose present session has ended reports `present_ended: true` in `GET /api/session`, and staff routes answer `423 locked_present_ended` instead of `423 locked`. `POST /api/session/unlock` lifts that lock with a rotated token and CSRF token, like an exit (DECISIONS E13).

API responses carry `Cache-Control: no-store`. The `__Host-` prefix needs TLS. In demo mode the platform terminates TLS and the public origin is `https://`.

### CSRF

- Session-bound synchronizer token. The token is issued with the session, stored with the session row, and returned by `GET /api/session`, which only same-origin pages can read. There is no CSRF cookie. The client must send it in the `X-DC-CSRF` header on every state-changing staff request and on logout; the server compares it in constant time [ARCH 5].
- `SameSite=Strict` on the session cookie.
- `Origin` (or `Referer` as a fallback) must match the configured origin on every non-GET `/api` request, including login, setup and visit.
- No state change on GET.
- The present session has no CSRF token, because the patient screens never see staff state. `POST /api/present/exit` relies on the `SameSite=Strict` present cookie, the Origin check, and a JSON body (`Content-Type: application/json`), which a cross-site form cannot send.

### Rate limits and lockout

| Endpoint | Limit | On exceeding |
|---|---|---|
| `POST /api/auth/login` | 5 per minute per client address | `429` with `Retry-After` |
| Login failures | after 5 failures from an address, delay doubles from 1 s up to 5 min; failures are forgotten after 15 min | per-address backoff, not an account lockout (one shared account means a hard lockout would let any LAN device lock out the whole practice). Wrong setup codes count toward the same backoff |
| `POST /api/visit` (demo) | 30 new visitor sessions per minute per client address | `429` |
| `POST /api/present/exit` (PIN) | 5 PIN tries per present session, counted before each check so parallel requests cannot exceed it; 10 exit requests per minute per present session and 30 per client address | after 5 wrong PINs the PIN is refused (`403 pin_disabled`) for that lock and only the staff password unlocks; the password is accepted at any time and its failures count toward the login backoff |
| Global failed login count | over 50 failures in 15 min | one `login_failure_surge` audit event per 15 min; the banner on next staff login is Later (not implemented in M1) |
| Other staff API | 300 per minute per session | `429` |
| Present API (`/api/present/*`) | 300 per minute per present session | `429` |
| `PUT /api/staff/presentations/{id}/blocks` (demo) | 60 saved edits per hour per visitor scope | `429` with `Retry-After` |
| `POST /api/session/unlock` (clinical) | the sign-in limit and backoff: 5 per minute per client address, failures count toward the login backoff | `429` with `Retry-After`; a wrong password is `401 invalid_credentials` and is audited as `session_unlock_failed` |
| `POST /api/staff/presentations/{id}/sections/{key}/regenerate` (demo) | 6 per hour per visitor scope and 20 per day per client address, counted from `regeneration_events` by each regeneration's own start time, so they survive a restart and the 24 h purge of the presentation; the rows move with the visitor scope when the token rotates and are deleted a day after they were written; one running job per visitor, shared with new presentations | `429 rate_limited` or `429 generation_in_progress`, with `Retry-After` |
| Narration synthesis (demo) | 20,000 synthesized characters per hour per visitor scope and 60,000 per day per client address, with each synthesized sentence charged at least 50 characters; cached clips are free | `429 narration_quota` with `Retry-After` |
| Upstream speech calls | 120 per minute for the whole server, of which one client address may use 30; every attempt counts, retries included | `429 narration_busy` with `Retry-After`; pre-warming waits it out up to five times per block |

A global login limit was dropped because a few devices could use it to block everyone. All limits are in memory, keyed by client address and session, and reset on restart. A restart-persisted counter is not needed for the prototype.

- IPv6 clients are keyed by their /64 prefix.
- Each purpose has its own limiter (sign-in, visitor creation, staff API, present API, present exit per session and per address, edits, narration per scope and per address, upstream speech calls overall and per address), so filling one cannot block another.
- Limiter maps are capped at 10,000 keys. When a map is full, expired entries are pruned first. Maps keyed by client address then refuse new keys until entries expire. Maps keyed by a session or visitor scope, whose creation is already limited per address, drop their oldest entry instead, so new sessions keep working.
- A demo visitor's edit and narration counts move to the new token when leaving Present mode or unlocking rotates the session. The move is remembered for an hour, and a narration charge resolves its scope at the time it is made, so the pre-warm task started under the old token keeps charging the visitor's new token.
- Client address: forwarding headers are read only when the direct peer is inside `DC_TRUSTED_PROXY` (one address or a comma-separated list of CIDR ranges). The header named by `DC_CLIENT_IP_HEADER` (for example `fly-client-ip`) is used when set and present; otherwise only the rightmost `X-Forwarded-For` entry, which is the one the proxy appended. `DC_CLIENT_IP_HEADER` without `DC_TRUSTED_PROXY` is refused at startup. An unknown peer is treated as `0.0.0.0`, never as local.
- Request bodies over 64 KiB are rejected. Malformed JSON gets an `ApiError` JSON body with code `bad_request`, without parser details.

### Audit events written

`setup_completed`, `setup_code_rejected`, `login`, `login_failed`, `logout` and `login_failure_surge`. From M3: `settings_changed`. From M5: `present_started`, `present_exited` (with how it unlocked), `present_exit_pin_failed`, `present_pin_disabled`, `present_exit_password_failed`, `block_edited`, `block_reverted` and `note_confirmed`. Also `session_unlocked` (with how), `session_unlock_failed`, `section_regeneration_started` and `section_released`. `present_started`, `present_exited`, `session_unlocked` and `section_released` are written in the same transaction as the change they record. For an exit or unlock this means nothing that can fail runs after the token rotation commits, so the browser always receives the new session cookie for a rotation that happened. The actor is the client address, `staff`, `visitor` or `server`. Details name only DentalClarity ids, section keys and block fields. Events hold no patient data and no edited text.

## Present-mode isolation

Confirmed flow from ARCHITECTURE section 5 [C, P8], with details [P]:

1. Staff click Present. `POST /api/staff/presentations/:id/present` creates a `present_session` bound to `(presentation_id, version)`, sets `staff_session.locked = true` and writes `present_started` in one SQLite transaction, and sets the `__Host-dc_present` cookie. Nothing after the commit can fail the request, so a locked session always gets its present cookie. It is refused with `409` until the job has finished and at least one section is ready. Demo visitor sessions lock the same way.
2. The frontend does a full page load of `/present`, which drops all staff state held in WASM memory. The Present bundle never fetches staff data.
3. While locked, every `/api/staff/*` call with that staff session returns `423 Locked` and no body beyond an error code. This covers other tabs, the back button and direct API calls. The staff SPA shows a lock screen on `423`.
4. `/api/present/*` handlers take the presentation and version only from the present session. Any id in the path or body is ignored or must match, otherwise `403`.
5. Exit, clinical: `POST /api/present/exit` with the PIN (or the password after 5 wrong PINs). On success the present session ends, the locked session is replaced by a new session token and a new CSRF token (the old token stops working at once), the present cookie is cleared, and an audit event is written.
6. Exit, demo: `POST /api/present/exit` without a PIN [C, P8]. Accepted because demo data is synthetic.
7. If the present session expires (60 min idle or 4 h), the staff session stays locked. Without a present session there is nothing to exit, so `GET /api/session` reports `locked` with `present_ended: true`, and staff routes answer `423 locked_present_ended`. The lock screen then offers `POST /api/session/unlock` (CSRF token required). Demo mode unlocks without a password. Clinical mode needs the staff password; the PIN is refused, because its five-try allowance belonged to the present session that ended. Unlocking rotates the token as an exit does, moves demo presentations to the new visitor scope, clears the present cookie and writes `session_unlocked`. While a present session is still live it answers `409 present_active`. Sign-out also still works.
8. Other workstations log in with their own staff session, which is not locked. Locking applies per session.

Fullscreen is cosmetic and is never treated as an access control [C, brainstorm 3]. For a stronger lock on a shared tablet, practices can use the OS kiosk or guided-access feature. That is optional and documented in the install guide.

## Authorization matrix

Rows are endpoint groups. A means allowed; dash means rejected with the code shown.

| Endpoint group | Anonymous | Staff (unlocked) | Staff (locked) | Present session | Demo visitor |
|---|---|---|---|---|---|
| `GET /` static bundle, fonts, illustrations | A | A | A | A | A |
| `GET /api/health` (status, version and mode only) | A | A | A | A | A |
| `POST /api/setup` | needs the one-time setup code; 403 after setup | 403 | 403 | 403 | disabled (404) |
| `GET /api/session` (read-only) | A | A | A | A | A |
| `POST /api/session/unlock` | 401 | 409 `not_locked` | A only when its present session ended; password in clinical | 401 | A when locked and its present session ended; no password |
| `POST /api/visit` | 404 in clinical | 404 | 404 | 404 | A (demo only, Origin-checked, rate-limited) |
| `POST /api/auth/login`, `logout` | A (login) | A (logout) | logout only | 403 | 404 |
| `/api/staff/dashboard`, `patients`, `patients/:key` (read PMS) | 401 | A | 423 | 401 | A (synthetic, read-only) |
| `/api/staff/presentations` list, view, create, edit (`blocks`, `blocks/revert`, `confirm-note`), regenerate (`sections/:key/regenerate`), release (`sections/:key/release`), history (planned) | 401 | A | 423 | 401 | A, own visitor scope only |
| `/api/staff/jobs/:id` | 401 | A | 423 | 401 | A, own jobs only |
| `/api/staff/presentations/:id/export` (PDF, planned for M8) | 401 | A | 423 | 401 | A, own scope |
| `/api/staff/presentations/:id/present` | 401 | A | 423 | 401 | A |
| `GET /api/staff/presentations/:id/questions` (read conversations) | 401 | A | 423 | 401 | A, own visitor scope only (404 otherwise) |
| `/api/staff/analytics` | 401 | A | 423 | 401 | A, own visitor scope only |
| `/api/staff/settings` (branding, sync) | 401 | A | 423 | 401 | 403 `demo_read_only` |
| `/api/staff/secrets` (write-only, planned for M9) | 401 | A, write only, never returns values | 423 | 401 | 403 |
| `/api/present/content`, `audio/:key`, `narration?keys=`, `variant` (planned for M7) | 401 | 401 | 401 | A, bound version only | A, bound version only |
| `GET`, `POST /api/present/qa` | 401 | 401 | 401 | A, bound version only, caps apply | A, bound version only, caps and budget apply |
| `POST /api/present/analytics` (engagement reports) | 401 | 401 | 401 | A, write only, 30 per minute per present session | A, write only, same cap |
| `POST /api/present/exit` | 401 | 401 | 401 | A (PIN in clinical) | A |

First-launch setup works as follows. When no staff account exists, the server prints a one-time setup code to its console at startup and notes in the log that a code was issued. The code is 12 characters from an unambiguous alphabet (about 60 bits). Wrong codes count toward the same per-address backoff as logins and are audited as `setup_code_rejected`. The earlier restriction to localhost was removed, because a local TLS proxy would make every client look local; the code is the gate.

Secrets are write-only through the API. The settings page shows "set" and the time it was last changed, never the value.

## Transport

- Clinical mode serves HTTPS only, with rustls (TLS 1.2 and 1.3). Port 80, if enabled, only redirects. [P]
- The service binds to a configured LAN interface address. The installer's firewall rule allows the LAN subnet only (M9).
- Clinical mode refuses to start unless `DC_PUBLIC_ORIGIN` is `https://` or the bind address is loopback.
- HSTS is sent only when `DC_HSTS` is enabled and the origin is `https://`. Enable it only with a certificate that all clients trust (option 1 or 2 below), so a self-signed setup does not lock clients out.

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
| OpenRouter key | refused in clinical mode | env var `OPENROUTER_API_KEY` |
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
| Analytics events (`present_events`) | screen visited, seconds on it, narration plays; no free text | 90 days, then deleted by the 10-minute maintenance task | 24 h, with the presentation | scheduled delete |
| `ai_calls` | metadata only, no prompt or response text | 12 months | 30 days | scheduled delete |
| Audio clips | per-sentence audio and merged block clips | 30 days after last play | 24 h after last play | the 10-minute maintenance task deletes the row and file of stale clips, then the least recently played clips while the cache is over 2 GB, then clip files with no row older than the retention; only names of 64 hex characters with a known extension inside the audio directory are deleted |
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
- Response headers on every page: `Content-Security-Policy: default-src 'self'; script-src 'self' 'wasm-unsafe-eval' 'sha256-<hash of each inline script in the embedded index.html>'; style-src 'self'; img-src 'self' data:; media-src 'self' blob:; connect-src 'self'; font-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'`. Trunk loads the WASM through one inline module script, so the server computes its hash at startup from the embedded `index.html` and no `'unsafe-inline'` is needed. `media-src` allows `blob:` because the patient screens fetch a block's narration (to read its sentence offsets) and play the bytes from an object URL; a page can only make object URLs for data it already holds.
- Also sent: `X-Content-Type-Options: nosniff`, `X-Frame-Options: DENY`, `Referrer-Policy: no-referrer`, `Permissions-Policy: camera=(), microphone=(), geolocation=(), payment=()`, `Cross-Origin-Opener-Policy: same-origin`, and `Cache-Control: no-store` on `/api`.

## Demo abuse controls

[C, D5 for the cap; the numbers are P]

| Control | Value |
|---|---|
| Visitor identity | visitor session created by `POST /api/visit` (session cookie described under sessions and cookies), plus client address from the trusted proxy |
| Session creation | 30 new visitor sessions per minute per client address |
| Visitor isolation | presentations, edits, jobs and Q&A are scoped to the visitor id; the synthetic dataset is read-only and shared; visitor data is purged after 24 h |
| New generations | 3 per hour per visitor, 10 per day per IP |
| Section rewrites | 6 single-section regenerations per hour per visitor, 20 per day per IP; they share the one-running-job limit and the budget soft stop with new generations |
| On-demand variants | 10 per hour per visitor (planned for M7) |
| Q&A | 500 characters per question, 20 questions per present session, 20 per hour per visitor and 60 per day per network (IPv6 by /64); `429 question_limit`. Questions the server refuses (length, personal details, budget) are not counted. The budget guard applies; an urgent question still gets the fixed escalation text when the budget is used up |
| Staff edits | 2,000 characters per block, 400 per sentence; 60 saved edits per hour per visitor; text containing `{{` or `}}` is refused (`422 invalid_edit`), because placeholders are filled in after the length checks |
| Narration | 20,000 synthesized characters per hour per visitor and 60,000 per day per network (IPv6 by /64), counted per synthesized sentence on cache misses only (at least 50 characters each), never for merging a block or replaying a cached clip; `429 narration_quota`. One network may make 30 of the server's 120 upstream speech calls per minute (`429 narration_busy`). The block route `/api/present/narration` serves only keys that form one aligned part of one block of the present session's bound version: the block cut into parts of 12 sentences from its start, the last part shorter. That is how pre-warming and the patient screens ask, and it keeps each block to a few cached merged clips. Keys from two blocks, a run that starts or ends inside a part, or keys outside that version answer `404 audio_not_found`. A speech call that times out or returns an unusable reply counts against the budget at its full price and is retried once (as D12 does for text). Pre-warming on entering Present mode covers generated blocks only; staff-edited text is narrated only when played |
| Concurrency | 2 generation jobs globally, 1 per visitor; later jobs queue (a visible queue position is not built yet) |
| Budget | $25 per month, reservation-based guard, 80% soft stop on new generations (see [AI_PIPELINE.md](AI_PIPELINE.md)). Narration may spend at most 20% of the cap, tracked in `ai_usage.speech_cost_usd`; past it narration answers `503 budget_reached` while generation keeps working |
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
