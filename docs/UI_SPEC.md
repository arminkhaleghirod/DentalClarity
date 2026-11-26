# DentalClarity UI specification

Status: v0.1, 2026-11-25. Source of visual truth is mockup round 2, "clean clinical" (`.local/design/DentalClarity Mockups.dc.html`, section `t2`, screens 2a to 2f). Round 1 (`t1`) is superseded (DECISIONS U2); only its floating "Ask a question" button placement is reused. Requirements referenced as `FR-*` are in [PRD.md](PRD.md). [DECISIONS.md](DECISIONS.md) wins any conflict.

Each item is tagged `C` (confirmed: DECISIONS or the t2 mockup) or `P` (proposed here). Values copied from the mockup are `C` unless a contrast fix changes them, which is called out.

## 1. Principles

- Staff UI is dense and quiet: small type, gray sidebar, gray-header tables, one blue. It is DentalClarity-branded and English-only (C).
- Patient UI uses the same system at larger sizes and is fully practice-branded through `--brand-*` tokens. No DentalClarity name or logo appears (C).
- Coral is reserved for "your teeth" on the patient side. It is never used for buttons, warnings or phases (C).
- Every content block says where it came from (provenance label, §4.4) (C).
- Costs always say "estimate" (C).
- Color never carries meaning alone: status has text, highlighted teeth have a shape cue, phases have a "Phase N" label (P, from PRD NFR-A11Y).

## 2. Design tokens

All tokens are CSS custom properties on `:root`. The patient route tree (`/present`) sets `--brand-*` from practice settings on its root element; staff routes never read `--brand-*` (C, ARCHITECTURE §7).

### 2.1 Staff color tokens

| Token | Value | Use | Origin |
|---|---|---|---|
| `--dc-bg` | `#FFFFFF` | Page and card background | C |
| `--dc-surface-subtle` | `#F5F7FA` | Sidebar, table header, create/editor canvas, chips container | C |
| `--dc-surface-muted` | `#EEF1F5` | Progress track | C |
| `--dc-surface-neutral` | `#F2F4F7` | Neutral badge ("Not started"), number bubbles | C |
| `--dc-border` | `#E4E7EC` | Card and table outer borders, header dividers | C |
| `--dc-border-subtle` | `#F0F2F5` | Table row dividers, in-panel separators | C |
| `--dc-border-control` | `#868E9C` | Input, select and segmented-control outline | P (replaces `#D0D5DD` on controls; see §2.4) |
| `--dc-border-decorative` | `#D0D5DD` | Secondary button outline, pending step ring, dashed placeholder chips | C |
| `--dc-border-dashed` | `#C9D2DE` | Demo-data notice | C |
| `--dc-text` | `#101828` | Primary text | C |
| `--dc-text-2` | `#344054` | Body text, nav labels | C |
| `--dc-text-3` | `#475467` | Secondary text, table cells, icons | C |
| `--dc-text-muted` | `#667085` | Meta text on white or `--dc-surface-subtle` only | C |
| `--dc-text-disabled` | `#98A2B3` | Disabled text, decorative chevrons, separators. Never for readable content or placeholders | C (restricted, §2.4) |
| `--dc-primary` | `#2A7DD1` | Non-text accents: progress fill, check marks, selected outline, focus ring, active step | C |
| `--dc-primary-fill` | `#1F72C4` | Filled buttons and active nav pill that carry white text | P (contrast fix, §2.4) |
| `--dc-primary-ink` | `#1C5FA8` | Text links, text on `--dc-primary-soft`, outline-button text | C |
| `--dc-primary-soft` | `#EEF5FD` | Ready badges, info callouts, done section chips | C |
| `--dc-primary-soft-border` | `#DCE9F8` | Border of soft blue cards | C |
| `--dc-primary-avatar` | `#D5E7FA` | Initials avatar background | C |
| `--dc-primary-pending` | `#9EC3EB` | Dashed outline of the in-progress section chip | C |

Status families (background / border / ink / accent):

| Status | Bg | Border | Ink | Accent | Used for | Origin |
|---|---|---|---|---|---|---|
| `ready` | `#EEF5FD` | `#DCE9F8` | `#1C5FA8` | `#2A7DD1` | Ready to present, AI-written · checked | C |
| `generating` | `#F3F1FC` | `#E5E1F7` | `#5B4BB7` | `#7A6AD8` | Generating, queued | C |
| `changed` | `#FDF3EC` | `#F7E3D5` | `#B4532A` | `#B4532A` | Plan changed, edited by staff, warnings, outdated narration | C |
| `review` | `#FDEFEF` | `#F8DCDC` | `#B42318` | `#B42318` | Needs review, held section, failed job | C |
| `neutral` | `#F2F4F7` | `#E4E7EC` | `#475467` | `#667085` | Not started, From clinical note, Locked | C |

Inside any tinted status surface, secondary text uses `--dc-text-3` (`#475467`), not `--dc-text-muted` (P, §2.4).

### 2.2 Patient brand tokens

Practices override these in Settings. Defaults are the fictional Maple Grove practice in the mockup.

| Token | Default | Use | Override | Origin |
|---|---|---|---|---|
| `--brand-primary` | `#1B3A5C` | Logo tile, active nav tab, primary buttons, user chat bubbles, player button | Yes | C |
| `--brand-on-primary` | `#FFFFFF` | Text and icons on `--brand-primary` | Computed | C |
| `--brand-soft` | `#EEF4FA` | Note callout, narration bar, info icon bubble | Yes (derived if unset) | C |
| `--brand-soft-track` | `#D3E0EE` | Narration progress track | Derived | C |
| `--brand-insurance` | `#B7D0EA` | Insurance segment of the cost bar | Derived | C |
| `--brand-accent` | `#E8704A` | "Your teeth" fill on the odontogram | Yes | C |
| `--brand-accent-border` | `#C9562F` | Outline of highlighted teeth | Derived | C |
| `--brand-on-accent` | `#101828` | Tooth number on `--brand-accent` | Computed | P (mockup used white; fails, §2.4) |
| `--brand-highlight` | `#FBE3D8` | Active narration sentence background | Derived from accent | C |
| `--brand-logo` | URL | Header logo; falls back to an initial on `--brand-primary` | Yes | C |
| `--brand-name` | text | Practice name in header and booklet | Yes | C |

Derivation (P): when a practice sets only `--brand-primary` and `--brand-accent`, the app derives `soft` (primary at 8% over white), `soft-track` (18%), `insurance` (32%), `accent-border` (accent darkened 12% in OKLCH lightness) and `highlight` (accent at 22% over white), then runs the §2.4 checks. `--brand-on-*` is whichever of `#FFFFFF` or `#101828` has the higher contrast.

Patient UI reuses staff neutrals (`--dc-text*`, `--dc-border*`, `--dc-surface-subtle`) and phase tokens:

| Token | Bg / border / ink | Origin |
|---|---|---|
| `--phase-1-*` | `#FDF3EC` / `#F7E3D5` / `#B4532A` | C |
| `--phase-2-*` | `#EEF5FD` / `#DCE9F8` / `#1C5FA8` | C |
| `--phase-3-*` | `#F3F1FC` / `#E5E1F7` / `#5B4BB7` | P |
| `--phase-4-*` and later | `#F2F4F7` / `#E4E7EC` / `#475467` | P |

Open question: phase 1 uses a warm orange close to the coral "your teeth" color. It is distinct (`#B4532A` ink on a pale tint vs. a saturated coral fill) and always labelled "Phase 1", so it is kept as in the mockup.

### 2.3 Typography, spacing, radii, shadows

Fonts (C, DECISIONS E8): Figtree 400/500/600/700 for all UI text; IBM Plex Mono 400/500 for CDT codes, version hashes and the section eyebrow in illustrations. Both are self-hosted WOFF2 files embedded in the binary and declared with `@font-face` and `font-display: swap`. No Google Fonts or other CDN request is allowed; the mockup's Google Fonts link is prototype-only. Fallback stack: `Figtree, system-ui, -apple-system, "Segoe UI", sans-serif`. For scripts Figtree lacks (CJK, Arabic, Devanagari, Thai and others), the stack continues with bundled Noto Sans subsets; exact set TBD (verify in the booklet milestone) (P).

Type scale. Size / line height / weight. Headings 19 px and up use `letter-spacing: -0.02em` (staff) or `-0.025em` (patient display sizes) (C).

| Token | Staff use | Patient use | Size / LH / weight |
|---|---|---|---|
| `--fs-11` | Provenance badges, locked-strip labels | | 11 / 1.3 / 600 |
| `--fs-12` | Table header, meta, chips, status badges | Eyebrows, chart labels, disclaimers | 12 / 1.4 / 400 to 700 |
| `--fs-13` | Table cells, panel body, segmented controls | Small meta, legend | 13 / 1.45 / 400 to 600 |
| `--fs-14` | Nav items, step list | Card body, phase sub-line | 14 / 1.45 / 400 to 600 |
| `--fs-15` | Editor block body, Present label, entity title | Nav tabs, language select, chat text, step list | 15 / 1.5 to 1.6 / 400 to 700 |
| `--fs-16` | Panel heading (Generating presentation) | Accordion headers, cost table rows, buttons | 16 / 1.4 / 600 to 700 |
| `--fs-17` | | Note callout body, phase table headers, drawer title | 17 / 1.5 / 400 to 700 |
| `--fs-18` | | Narrated reading text | 18 / 1.55 / 400 |
| `--fs-19` | Status card title, section heading | | 19 / 1.2 / 600 to 700 |
| `--fs-20` | Patient name on create screen | Phase card title | 20 / 1.25 / 600 to 700 |
| `--fs-21` | Page title ("Good morning, ...") | | 21 / 1.2 / 700 |
| `--fs-24` | Editor block-set title | | 24 / 1.2 / 700 |
| `--fs-30` | | Estimated share on At a glance | 30 / 1.1 / 700 |
| `--fs-32` | | Procedure title | 32 / 1.15 / 700 |
| `--fs-34` | | Costs headline | 34 / 1.15 / 700 |
| `--fs-38` | | At a glance headline | 38 / 1.12 / 700 |
| `--fs-42` | | Costs estimated share figure | 42 / 1 / 700 |

Spacing (P, normalized from the mockup): `--space-1` 4, `-2` 8, `-3` 12, `-4` 16, `-5` 20, `-6` 24, `-7` 28, `-8` 32, `-10` 40 px. Dense components keep the mockup's 2, 6, 10, 14, 18, 22 px values as component-local constants.

Radii (C, values from the mockup):

| Token | Value | Use |
|---|---|---|
| `--radius-xs` | 4 px | Narration sentence highlight, tiny inline badges |
| `--radius-sm` | 6 px | Section chips, mini buttons in cards, small-arch teeth |
| `--radius-seg-inner` | 7 px | Selected segment inside segmented control |
| `--radius-md` | 8 px | Selects (staff), editor icon buttons |
| `--radius-ctl` | 9 px | Sidebar pill, staff buttons, checks callout |
| `--radius-lg` | 10 px | Staff segmented outer, locked strip, patient inputs and list buttons |
| `--radius-xl` | 12 px | Staff cards and tables, patient segmented outer, patient buttons |
| `--radius-2xl` | 14 px | Patient cards, chat bubbles |
| `--radius-3xl` | 16 px | Patient chart panel |
| `--radius-pill` | 999 px | Status and provenance badges, sync indicator, avatars |

Shadows. The t2 system is flat; borders do the work (C). Allowed shadows (P except where noted):

| Token | Value | Use |
|---|---|---|
| `--shadow-focus` | `0 0 0 2px #FFFFFF, 0 0 0 4px var(--dc-primary)` | Keyboard focus, staff |
| `--shadow-focus-brand` | `0 0 0 2px #FFFFFF, 0 0 0 4px var(--brand-primary)` | Keyboard focus, patient |
| `--shadow-active-block` | `0 0 0 4px #EEF5FD` plus 2 px `--dc-primary` border | Block being edited (C) |
| `--shadow-tooth-selected` | `0 0 0 3px #F5F7FA, 0 0 0 5px var(--brand-primary)` | Selected tooth (C) |
| `--shadow-float` | `0 6px 20px rgba(16, 24, 40, 0.18)` | Floating Ask button, toasts |
| `--shadow-overlay` | `0 12px 32px rgba(16, 24, 40, 0.16)` | Drawer in overlay mode, modals |

Motion (P): `--dur-fast` 120 ms, `--dur-base` 200 ms, easing `cubic-bezier(.2,0,0,1)`. With `prefers-reduced-motion: reduce`, all durations become 0 except opacity fades of at most 150 ms, progress bars jump instead of animating, and the drawer appears without sliding.

### 2.4 Contrast checks (WCAG 2.2 AA)

Text needs 4.5:1 (3:1 for 24 px regular or 18.66 px bold and up). Control boundaries and meaningful graphics need 3:1. Ratios computed from sRGB relative luminance.

| Pair (fg on bg) | Ratio | Result | Where | Fix |
|---|---|---|---|---|
| `#101828` on `#FFFFFF` | 17.75 | Pass | Primary text | |
| `#344054` on `#FFFFFF` | 10.46 | Pass | Body | |
| `#475467` on `#FFFFFF` / `#F5F7FA` | 7.69 / 7.16 | Pass | Secondary | |
| `#667085` on `#FFFFFF` / `#F5F7FA` | 4.97 / 4.64 | Pass | Meta | |
| `#667085` on `#EEF5FD` | 4.53 | Pass (marginal) | Meta in blue cards | Prefer `#475467` |
| `#667085` on `#F3F1FC` | 4.45 | **Fail** | "~0:40 left" in Generating card | Use `#475467` (P) |
| `#667085` on `#FDF3EC` | 4.55 | Pass (marginal) | Meta in warm cards | Prefer `#475467` |
| `#98A2B3` on `#FFFFFF` | 2.58 | **Fail** | Placeholder "Type a question...", em-dash tooth cell, pending steps | Placeholders and readable text use `#667085`; pending step labels use `#667085`; `#98A2B3` only for disabled and decorative glyphs (P) |
| `#FFFFFF` on `#2A7DD1` | 4.24 | **Fail** at 13 to 15 px | Active sidebar pill, filled segment, Present button | Fill with `--dc-primary-fill` `#1F72C4` (4.93). `#2A7DD1` stays the brand hue for non-text (P; owner to confirm, since U1 names `#2A7DD1`) |
| `#2A7DD1` on `#FFFFFF` | 4.24 | **Fail** as text | Row actions "Create", "Open", reference links | Text links use `--dc-primary-ink` `#1C5FA8` (6.46) (P) |
| `#2A7DD1` on `#FFFFFF` (graphic) | 4.24 | Pass (3:1) | Check marks, progress, focus ring | |
| `#D0D5DD` on `#FFFFFF` | 1.47 | **Fail** as control boundary | Select and segmented outlines | `--dc-border-control` `#868E9C` (3.30) for inputs and selects; `#D0D5DD` only where the control also has a fill or text that identifies it (P) |
| `#1C5FA8` on `#EEF5FD` | 5.88 | Pass | Ready badge, chips | |
| `#5B4BB7` on `#F3F1FC` | 6.00 | Pass | Generating badge | |
| `#B4532A` on `#FDF3EC` | 4.56 | Pass (marginal) | Changed badge, phase 1 label | Keep weight 600 or more |
| `#B42318` on `#FDEFEF` | 5.88 | Pass | Review badge | |
| `#FFFFFF` on `#1B3A5C` | 11.63 | Pass | Patient primary | |
| `#1B3A5C` on `#EEF4FA` | 10.49 | Pass | Note callout title | |
| `#FFFFFF` on `#E8704A` | 3.07 | **Fail** at 12 px bold | Tooth numbers on highlighted teeth | `--brand-on-accent` `#101828` (5.79) (P) |
| `#E8704A` vs `#F5F7FA` panel | 2.86 | Fail as graphic alone | Highlighted tooth fill | Tooth outline `#C9562F` (4.32 on white) plus the non-color badge (§4.15) carry the state |
| `#101828` on `#FBE3D8` | 14.43 | Pass | Active narration sentence | |
| `#B7D0EA` on `#F5F7FA` | 1.48 | Fail as graphic | Insurance cost segment | Values are in the legend as text, and segments are separated by a 3 px gap; acceptable. Add a 1 px `#1B3A5C` outline on the insurance segment (P) |

Brand override validation (P, FR-SETUP-02): on save, the server checks `--brand-on-primary` on `--brand-primary` (4.5), `--brand-primary` on `#FFFFFF` as text (4.5, used for eyebrows), `--brand-on-accent` on `--brand-accent` (4.5), `--brand-accent-border` on `#FFFFFF` (3.0), and `#101828` on `--brand-highlight` (4.5). Any failure blocks the save and names the pair and ratio.

## 3. Layout and responsive targets

| Surface | Target | Notes | Origin |
|---|---|---|---|
| Staff | 1280×800 desktop, minimum width 1180 | Sidebar 210 px; below 1180 the sidebar collapses to 64 px icons with tooltips | C / P |
| Patient | iPad landscape 1180×820 | Header 76 px, page padding 28 px | C |
| Patient | iPad portrait 820×1180 | Rules in §5.11 | P |
| Patient | Desktop or chairside monitor up to 1920 | Content max-width 1180 px, centered | P |

Touch targets in patient UI are at least 44×44 px (nav tabs 40 px tall inside a 4 px padded group, giving a 48 px hit area; buttons 44 to 52 px). Staff targets are at least 32 px tall with 8 px spacing; Present is 42 px (C/P).

Keyboard and focus (P):

- Every interactive element is a native `button`, `a`, `input` or `select`, or has the matching ARIA role and key handling.
- Focus is always visible using `--shadow-focus` / `--shadow-focus-brand`; never `outline: none` without a replacement.
- Tab order follows visual order. Skip link "Skip to content" on every page.
- Segmented controls are radio groups (arrow keys move, Space selects). Pill and section navs use `aria-current="page"`.
- Odontogram teeth are buttons in a roving-tabindex group: arrows move along the arch, Enter selects.
- Drawer and modals trap focus, close on Escape (except the PIN modal in clinical mode, which only closes by Cancel or correct PIN), and return focus to the opener.
- Global staff shortcuts: `/` focuses patient search. No other single-key shortcuts.

## 4. Components

### 4.1 Sidebar pill nav (staff)

- 210 px wide, `--dc-surface-subtle`, padding 22/14/18 px, items gap 4 px (C).
- Wordmark: "dental" `#344054` + "clarity" `--dc-primary`, 19 px 600, plus a collapse icon button (C).
- Items: Dashboard, Patients, Presentations, Analytics (top); Settings, Help (bottom) (C). Height 40, radius 9, padding 0 12, icon 17 px stroke 1.8, label 14 px.
- Active: `--dc-primary-fill` background, white 600 label and icon. Inactive: `#344054` label, `#475467` icon; hover `#EAEEF3` background (P).
- Demo mode only: dashed notice at the bottom, "Synthetic demo data. No real patient records." (C, copy de-dashed).

### 4.2 Top bar (staff)

- Dashboard style: 72 px, page title left; right side sync indicator, notifications button (38 px circle), divider, initials avatar plus "Front desk" (shared account) (C).
- Sub-page style (create, editor): 68 px, bottom border `--dc-border`, wordmark or back button, breadcrumb `Patients › Name › New presentation` with `#98A2B3` separators (C).
- Sync indicator: pill 32 px, `--dc-surface-subtle`, 7 px dot. Dot `--dc-primary` when synced; `#B4532A` when the last poll is older than 3 intervals; `#B42318` with text "Open Dental unreachable" after 3 failures (P).

### 4.3 Status cards (dashboard)

- Four-column grid, gap 14. Card padding 16/16/14, radius 12, status family bg and border (C).
- Title 19 px 600 in status ink; sub-line 13 px `#475467`; footer row 28 px tall: white mini button (radius 6, 12 px 600, status ink, "View ›" or "Review ›") and a 12 px meta at right (C).
- Generating card replaces the button with a 5 px progress bar (white track, `#7A6AD8` fill) and elapsed or remaining time. Show remaining time only after the benchmark supports it; until then show elapsed ("0:48") (P).
- Zero items: title stays, sub-line "None", no button (P).

### 4.4 Badges and provenance labels

Status badge: 12 px 600, padding 3/9, radius pill, status family bg and ink. Labels: "Not started", "Ready · v2", "Generating", "Queued", "Plan changed", "Needs review", "Failed" (C/P).

Provenance label (C, DECISIONS U3): 11 px 600, padding 3/8, radius pill, top-right of each content block, exactly one per block.

| Label | Family | Icon | Meaning |
|---|---|---|---|
| Locked · from Open Dental | neutral, shown as a strip segment (§4.6) | lock 13 px | Snapshot fact; not editable |
| From clinical note | neutral | none | Text derived from a linked clinical note (P11). Shows a "Looks right" confirm button until staff confirm it; held from patients until then |
| AI-written · checked | ready | none | Model text that passed validation and review |
| Edited by staff · 8:34 AM | changed | none | Staff changed it; time of last edit |

Demo mode uses "Locked · from plan" since the source is the synthetic connector (P). In the patient UI, provenance is shown only where meaningful to the patient: "From Dr. X's notes" on note-derived callouts, and "Educational drawing, not your X-ray" on illustrations. Staff-only labels are hidden (C from mockup).

### 4.5 Data table (staff)

- Outer border `--dc-border`, radius 12, overflow hidden (C).
- Header row 38 px, `--dc-surface-subtle`, 12 px 600 `#475467`; sortable headers show "⌄" and are buttons with `aria-sort` (C/P).
- Body rows 46 px, top border `--dc-border-subtle`, 13 px. Name cell 600 `#101828`; others `#475467`. Row action at right, 600 `--dc-primary-ink` (P fix).
- Numeric columns right-aligned with tabular figures (`font-variant-numeric: tabular-nums`) (P). Codes in IBM Plex Mono `#475467` (C). Missing tooth shows "None" for screen readers with a visual hyphen in `#667085` (P).
- Totals row 48 px, top border `--dc-border`, 700 (C).
- Whole row is clickable to the row action; the action text is the accessible name (P).

### 4.6 Locked facts strip

Bordered strip (radius 10): cells for Tooth, Code, Phase, Patient est., each with an 11 px `#667085` label above a 13 px value, separated by `--dc-border-subtle`. Right end: `--dc-surface-subtle` segment with lock icon and "Locked · from Open Dental" (C). No cell is focusable or editable (FR-EDIT-04).

### 4.7 Content block (editor)

- Border `--dc-border`, radius 12, padding 16/18, gap 7. Title 13 px 700; body 15 px / 1.6 `#344054`; provenance label at right (C).
- Editing state: 2 px `--dc-primary` border, `--shadow-active-block`, caret visible; footer links "Revert to AI version" and "Show diff", 12 px 600 `--dc-primary-ink` (C/P fix).
- Diff view: inline, removed words struck through on `#FDEFEF`, inserted on `#EEF5FD`; both also marked with `del`/`ins` semantics (P).
- Held block (Needs review): `review` family left border 3 px, body replaced with reason and actions "Regenerate section" and "Write it myself" (P).

### 4.8 Checks panel (editor right rail)

- 300 px rail, left border, padding 22/20, groups Checks, Narration, References separated by 1 px `--dc-border-subtle` (C).
- Check row: 13 px / 1.45; passed rows start with a `--dc-primary` "✓"; warnings are a `changed` callout (radius 9, padding 10/12) with "!" (C); blocking failures are a `review` callout with "Blocks presenting" in 600 (P).
- Copy for passed checks (C): "Tooth, code, phase and cost match Open Dental", "No diagnosis or outcome promises", "Review pass agreed with N references".
- Narration group: one row per language with state "Ready", "Generating", "Outdated after edit" (changed ink), "Not generated" (muted); note "Audio regenerates when you finish editing." (C).
- References group: link list in `--dc-primary-ink`, each with "Unreviewed" neutral badge while `review_status = unreviewed` (P, D9).

### 4.9 Generation progress list

- Card radius 12, padding 20. Header "Generating presentation" 16 px 700 with elapsed time at right; 6 px bar on `--dc-surface-muted` with `--dc-primary` fill (C).
- Steps (C): Imported plan snapshot (with "6 procedures · 3 teeth · 2 phases"), Matched N references, Locked teeth, phases and costs, Writing sections · k of n, Independent review pass, Assemble version 1.
- Step icon 20 px circle: done = filled `--dc-primary` with white check; current = 2 px ring with 7 px dot; pending = 2 px `#D0D5DD` ring with label in `#667085` (P fix from `#98A2B3`).
- Section chips under the current step (12 px, radius 6): done `ready` family; writing = dashed `#9EC3EB` with ellipsis; pending = dashed `#D0D5DD`, text `#667085`; held = `review` family with "!" (P).
- Footer: "You can close this tab. Generation keeps running on the practice server." In demo mode: "...keeps running on the demo server." (C, copy de-dashed).
- Live region: step changes are announced politely; section chips are not announced individually (P).
- Copy conflict: the mockup says "Matched 6 reviewed references". Until a dentist signs off entries (D9), the copy is "Matched 6 references" plus "(unreviewed)" when any matched entry is unreviewed (P).

### 4.10 Segmented control (reading level, filters)

- Outer: 3 px padding, border, gap 2. Staff: radius 10 outer / 7 inner, segment height 30 (filters) or 28 (create), 13 px (C). Patient: radius 10 outer / 7 inner, height 34, 14 px (C).
- Selected: staff `--dc-primary-fill` + white 600; patient `--brand-primary` + `--brand-on-primary` 600. Unselected `#475467` 400 (C/P).
- Reading level options: Simple, Standard, Detailed (C, P9). Role `radiogroup`, label "Reading level".

### 4.11 Language select

- Staff: 34 px, radius 8, 13 px 600 value with chevron (C). Patient: 44 px, radius 10, 15 px (C). Border `--dc-border-control` (P).
- Opens a listbox. Each language shows its own name and English name ("Español · Spanish"). Tested languages have no marker; untested languages show "Not reviewed" in `#667085` (P, FR-LANG-05).
- Selecting an uncached language closes the list and shows the loading state on text blocks (§6) (C).

### 4.12 Narration player

- Bar: radius 12, `--brand-soft` background, padding 8/16/8/8, gap 14 (C).
- Play/pause button 44×44, radius 10, `--brand-primary` with white glyph; accessible name "Play narration" / "Pause narration" (C/P).
- Status label 14 px 600 `--brand-primary`: "Listen", "Listening", "Paused", "Preparing audio..." (P from mockup "Listening").
- Progress track 4 px `--brand-soft-track`, fill `--brand-primary`; time "0:21 / 0:56" 13 px `#475467` (C). Track is a slider with sentence-sized steps (P).
- Sentence highlight (C, P10): the active sentence span gets `--brand-highlight` background, ink `#101828`, radius 4, padding 1/2, `box-decoration-break: clone`. Other sentences `#344054`. Highlight changes only at sentence boundaries; it never claims word timing.
- Tapping a sentence while playing seeks to it (P).
- Changing language or reading level stops playback and resets the player (P, FR-LANG-02).
- Reduced motion: no auto-scroll animation; the active sentence scrolls into view instantly only if off screen (P).

### 4.13 Ask a question drawer and button

- The patient-facing label is "Ask a question" (mockup copy) (C).
- Floating button (from t1 placement): bottom-right, 28 px from edges, 52 px tall, radius pill, `--brand-primary`, white "?" bubble 26 px plus label, `--shadow-float`. Hidden while the drawer is open. The Costs page also has the inline version in its footer (C, 2f).
- Drawer (C, 2e): 340 px right column on landscape, left border. Header 17 px 700 "Ask a question" with 34 px close button (44 px hit area). Notice box `--dc-surface-subtle`, 12.5 px `#475467`: "Answers come from your treatment plan and trusted dental sources. Your dental team can read this conversation. Please don't share personal details." Demo adds "This is a demo. Don't enter personal information." (C).
- Messages: patient bubble right, `--brand-primary`, white, radius 14/14/4/14, max 85%; answer bubble left, `--dc-surface-subtle`, `#344054`, radius 14/14/14/4, max 92%, with "Source: ..." 12 px `#667085` (C).
- Declined answers end with a suggested question in italics for the dental team (C).
- Input 46 px, radius 10, placeholder "Type a question..." in `#667085` (P fix); send button 46×46 `--brand-primary` (C). Enter sends, Shift+Enter newline.
- Pending answer: three-dot typing indicator (static dots under reduced motion) and the input disabled (P).
- Answers are announced via `aria-live="polite"` (P).

### 4.14 Phase cards

- Grid 2 columns, gap 14. Card padding 16, radius 14, phase family bg and border (C).
- Eyebrow 12 px 700 phase ink: "Phase 1 · tooth 30" / "Phase 2 · teeth 14, 19". Title 20 px 600 `#101828` (AI-written, short: "Save the tooth"). Sub-line 14 px `#475467` (procedure names). Footer: white 34 px "Learn more ›" button in phase ink, and "3 steps" 13 px `#475467` (C, `#667085` changed to `#475467` per §2.1).
- More than 2 phases: 2-column grid wraps; more than 4 phases: the At a glance shows the first 3 and a "+N more phases" card linking to Journey (P).

### 4.15 Odontogram

Geometry (C, mockup `arch()`): two elliptical arches, upper teeth 1 to 16 right-to-left from the patient's right, lower 32 to 17. Center x 200, radius x 150, radius y 160 in a 400×410 box at scale 1; procedure page uses scale 0.62. Tooth sizes: molars 30×30, premolars 24×26, canines 21×27, incisors 20×25; radius 9 (large) or 6 (small). Labels "Your right", "Upper", "Your left" above, "Lower" below, 12 px 600 `#667085`. Center hint "Tap a highlighted tooth".

Rules:

| Rule | Detail | Origin |
|---|---|---|
| Numbering | Universal: permanent 1 to 32, primary A to T. Labels show the number or letter; accessible name "Tooth 30, lower right, back. 3 planned procedures." | C |
| Dentition | Render the permanent arch, the primary arch (20 teeth, same layout rules), or both stacked for mixed plans, based on the `ToothRef` values in the snapshot | C |
| Orientation | Patient's right on the viewer's left, labelled; never mirrored | C |
| Highlight | Planned teeth: `--brand-accent` fill, `--brand-accent-border` 1.5 px outline (2.5 px, P), number in `--brand-on-accent` 700 at 12 px (P fix) | C/P |
| Non-color cue | Highlighted teeth also get a count badge (white circle, 1.5 px `--brand-accent-border`, 14 px, digit 10 px 700) at the outer edge, and the bold number. Unplanned teeth are white with `#D0D5DD` outline and `#98A2B3` 500 numbers (decorative, `aria-hidden` group with one summary label) | P |
| Selection | Selected tooth: `--shadow-tooth-selected` ring and `aria-pressed="true"`. Selecting opens or updates the step list (§4.16) | C |
| Multiple procedures | One tooth, many procedures: badge shows the count; the list shows all in plan order | C |
| Surfaces | Surfaces are shown in text ("MO, two surfaces"), not drawn on the tooth in v1 | P |
| Non-tooth procedures | Procedures with `Quadrant`, `Arch` or `None` are listed under the chart as "Not tied to one tooth" (quadrant and arch named, e.g. "Upper right area"). No tooth is highlighted for them | C |
| No invented locations | If the snapshot tooth value is missing or invalid, the procedure goes in the non-tooth list with "Location not recorded" and the validator flags it | C |
| No history | Existing restorations, implants and missing teeth are not drawn in v1 | C |
| Text equivalent | A visually hidden list ("Planned teeth: 14, 19, 30") follows the SVG; the at-a-glance summary sentence repeats the counts | P |

### 4.16 Procedure step list

Under the small chart (2e): eyebrow "Tooth 30 · 3 steps" 12 px 700 `#667085`; buttons 48 px, radius 10, 15 px, gap 6, each with a 22 px number bubble. Active: `--brand-primary` fill, white text, bubble `rgba(255,255,255,.18)`. Inactive: white, border, `#344054`, bubble `#F2F4F7` (C).

### 4.17 Cost bar and phase cost tables

- Container `--dc-surface-subtle`, radius 14, padding 18/20, gap 12 (C).
- Bar 18 px, radius pill, two segments with 3 px gap: insurance `--brand-insurance` (width = insurance / total), patient `--brand-primary`. Role `img` with label "Insurance estimate $2,081 of $3,450 total; your estimated share $1,369" (C/P).
- Legend 14 px `#475467`: 12 px swatches, "Insurance estimate · $2,081", "You · $1,369", and "Total fees · $3,450" right-aligned 600 `#101828` (C).
- Phase tables: 2-column grid, gap 18; header 14/18 padding in phase family bg with title 17 px 700 phase ink and subtotal 16 px 700; column header row 12 px 600 `#667085` on `--dc-surface-subtle` (Procedure, Fee, You); rows 16 px, padding 13/18, fee `#667085`, share 600 (C).
- More than 2 phases: tables stack in a scroll area with the bar fixed above (P).
- Unknown values: "Not available" in `#475467` and a note under the table (P, FR-COST-03).
- Disclaimer row: bordered box with "i" bubble: "These are **estimates** from your plan on November 18. Your insurance decides the final amount, which can change. Ask the front desk about payment options." (C).

### 4.18 Buttons

| Variant | Staff | Patient |
|---|---|---|
| Primary | `--dc-primary-fill`, white 600, 38 px, radius 9 | `--brand-primary`, white 600 16 px, 52 px, radius 12 |
| Present (special primary) | 42 px, padding 0 20, radius 10, 15 px 700, play triangle | n/a |
| Secondary | white, `--dc-border-decorative` border, `#344054` 600 | white, border, `#344054` |
| Outline accent | `--dc-primary` border, `--dc-primary-ink` text (All patients) | n/a |
| Text | `#344054` 600, no border (History, Regenerate) | n/a |
| Destructive | `#B42318` text or fill with white (P) | n/a |

Disabled: 40% opacity plus `aria-disabled`, and a tooltip or helper text with the reason (P).

## 5. Screen specs

### 5.1 Dashboard (2a)

- Layout: sidebar + main (padding 0 28 20). Top bar per §4.2 with "Good morning, {practice}" (time-of-day greeting) (C).
- Status cards row (§4.3) (C).
- "Today's patients" heading 19 px 700 with sub-line "Appointments today with an open treatment plan"; right: filter segmented control "All · 7 / Not started / Ready / Needs attention" and outline button "All patients" (C).
- Table columns and widths (C): Time 60, Patient 1.3fr, Age 54, Treatment plan 1.5fr, Procedures 96, Language 104, Presentation 150, action 64. Pediatric rows append "· guardian" in `#667085` (C).
- Row action by status (C): Not started: Create. Ready: Open. Generating: View. Plan changed: Review. Needs review: Review.
- Empty: "No appointments with open treatment plans today." with a button "Search all patients" (P).

### 5.2 Patients directory (unmocked, P)

- Main area: title "Patients", search field (44 px, radius 10, magnifier, placeholder "Search by name, birth year or patient number"), focused on load.
- "Recommended" block first: same table style as the dashboard, max 10 rows.
- Results table columns: Patient, Age, Patient #, Language, Plans (count), Last presentation (badge), action.
- Expanding a row reveals a nested plan list (indent 24, `--dc-surface-subtle` background): Status, Saved (date, by), Procedures, and a "Create presentation" or "Open" action. Plans with zero planned procedures are disabled with "No planned procedures".
- Empty search: "No patients match "{query}"." Under 2 characters: recommended block only.
- Pagination: 50 rows, "Show more" button.

### 5.3 Create presentation and generating (2b)

- Sub-page top bar with breadcrumb (C). Canvas `--dc-surface-subtle`, grid `1fr 400px`, gap 24, padding 24/28 (C).
- Left card: avatar 46 px, name 20 px 700, meta "42 · Patient #10482 · Prefers English"; plan select at right ("Plan · Active · saved Nov 18 by Dr. Patel ⌄") (C).
- Plan table (§4.5): Phase 56, Tooth 54, Code 70, Description (Open Dental) 1fr, Fee 84, Pt. est. 84, totals row (C).
- Note callout (`--dc-primary-soft`, radius 10): "Clinical note used: "..." Other notes stay on the server." Without a note: neutral callout "No clinical note linked. Content will describe common reasons for each procedure." (C / P).
- Footer note 12 px: "Read-only from Open Dental. DentalClarity never changes these values." (C, de-dashed).
- Right column top card: Language select and Reading level segmented control (C). Below: primary "Create presentation" button (P, not in mockup since it shows the running state).
- Right column generating card (§4.9) replaces the button once started. On success: "Version 1 ready" with buttons "Open editor" (primary) and "Present now" (secondary) (P). On `needs_review`: "Version 1 ready with 1 section held" and "Review" (P).
- Demo mode with budget cap reached: Create button disabled with "Live generation is paused for this month. Try a ready-made case." and a link to pre-generated cases (C, FR-DEMO-03).

### 5.4 Editor (2c)

- Top bar 68 px (C): back button 34 px, name 15 px 700 + "Active plan · 6 procedures" 12 px, version select ("v1 Today 8:21 AM ⌄"), spacer, text buttons History and Regenerate, secondary "Export booklet", primary Present.
- Grid `240px 1fr 300px` (C).
- Left rail `--dc-surface-subtle`, padding 18/12: "Sections" label; items 38 px radius 8: At a glance, Treatment journey, "Procedures" group label, procedure items indented 18 px ("Root canal · #30", "Fillings · #14, #19", "Cleaning"), Costs (with "auto" in `#667085`), What happens next. Status at right: "✓" in `--dc-primary` for checked, "Edited" chip, "!" review chip, spinner for regenerating. Active item `--dc-primary-fill` white 600 (C/P fix).
- Procedure grouping (P, inferred from mockup): one procedure section per distinct procedure category per phase; same-category procedures on several teeth (two fillings) share a section listing each tooth. Mockup has 6 procedures in 5 sections.
- Canvas padding 26/32, gap 16: eyebrow "Procedure 1 of 5", title 24 px 700, locked strip (§4.6), content blocks (§4.7) in order: Why your dentist recommended it (or Common reasons), What it is, What happens during the visit, Recovery, If you wait, Questions to ask (C/P).
- Right rail: Checks panel (§4.8).
- Present is disabled with a tooltip "1 section is held. Resolve or hide it to present." only when a blocking check exists; held sections otherwise are hidden from the patient and Present stays enabled with a confirmation listing what is hidden (P).
- Staff text edits autosave 800 ms after typing stops; "Saved" indicator in the top bar (P).

### 5.5 Presentations list and version history (unmocked, P)

- Presentations page: title, filter segmented control (All, Ready, Generating, Needs attention, Archived), search. Table: Patient, Plan, Version, Language(s), Updated, Status, actions (Open, Export).
- History drawer (opened from editor "History"): 400 px right overlay, `--shadow-overlay`. Timeline list, newest first. Each entry: "v3 · Plan changed" title, time, cause chip (Created, Staff edit, Plan changed, Regenerated, Restored), short diff summary ("Cost of D2740 changed", "1 procedure added"), status badge, "Presented" / "Exported" markers. Actions: "Open read-only", "Restore as new version".
- Read-only version view: editor layout with a top `changed` banner "You are viewing v1 (read-only). Current version is v3." and Present disabled.
- Edit conflict: banner in `changed` family on the affected block: "The plan changed for this procedure. Your edit from Nov 20 may no longer fit." Actions: "Keep my edit", "Use new AI text", "Show diff".
- Q&A conversations tab inside a presentation: list of sessions, each expanding to the transcript.

### 5.6 Analytics (unmocked, P)

- Header row of 4 metric tiles (status-card geometry, neutral family): Presentations shown (30 days), Median time in presentation, Narration plays, Questions asked.
- "Engagement by section" horizontal bar list: sections with visit rate as a percentage and a text value.
- "Languages and reading levels" table.
- "Generation" table: p50 and p90 time, failures, held sections, estimated model cost (from call records), provider and model IDs.
- Footnote on page: "Engagement shows interaction in the office. It does not measure understanding, consent or treatment decisions." No acceptance or conversion figures appear (C, FR-ANLY-03).
- Empty: "No presentations yet in this period."

### 5.7 Settings (unmocked, P)

- Left sub-nav inside main area (200 px): Practice and branding, Open Dental, AI provider, Staff account and PIN, Data and retention, About.
- Practice and branding: name, logo upload (PNG or SVG, max 1 MB), brand primary and accent color inputs (hex plus picker), live preview card (patient header, a phase card, three teeth with one highlighted), contrast result list with pass/fail per pair (§2.4).
- Open Dental: host, port, database, user, password (write-only), "Test connection" with result line; poll interval (minutes); last sync status.
- AI provider: clinical shows AWS region, credential mode, "Test" button; active model IDs listed read-only; demo shows "OpenRouter (demo)" read-only and the month's spend against the $25 cap.
- Staff account and PIN: change password, change PIN (both require current value).
- Data and retention: retention for Q&A, analytics, old versions (values TBD, see SECURITY.md); "Export backup" (Later).
- About: version, mode (read-only), license, clinical-readiness notice.
- Each section has its own Save button and "Last changed" meta.

### 5.8 First-launch setup wizard (unmocked, P)

- Full-screen, no sidebar. Left: 260 px stepper on `--dc-surface-subtle` with steps 1 Practice, 2 Open Dental, 3 AI provider, 4 Staff account, 5 Review and confirm; done steps use the generation-list icons (§4.9). Right: form column, max width 560.
- Footer bar: Back (secondary), Continue (primary). Continue is disabled until required fields pass; errors are inline under fields and summarized at the top on submit.
- Step 2 test result states: success (ready family), auth failed, unreachable, write privileges detected (changed family, requires "I understand" checkbox).
- Step 5 lists entered settings and three required checkboxes: local practice network only; provider terms and agreements are the practice's responsibility; clinical-readiness notice (PRD §9) read.

### 5.9 Present-mode exit (unmocked, P)

- Patient header right end, after the language select: a 44 px icon button with a lock glyph, neutral border, accessible name "Staff: exit presentation". It has no text label, so it stays unobtrusive for patients.
- Clinical mode: opens a modal (radius 14, 360 px, `--shadow-overlay`) "Enter staff PIN" with 4 to 8 dot indicators and a 3×4 numeric keypad (64 px keys) plus hardware keyboard input. Wrong PIN: dots shake (no shake under reduced motion), "Incorrect PIN" in `#B42318`. After 5 failures: "Try again in 60 seconds" with a countdown. Cancel returns to the presentation. Success returns to the editor.
- Demo mode: the same button opens no modal; it returns to the editor directly. Its accessible name is "Back to staff view".
- Expired present session: full-screen message "This presentation has ended. Please hand the device back to the front desk." with no navigation.

### 5.10 Patient header (all patient screens, C)

76 px, bottom border, padding 0 28, gap 20: logo tile 36 px radius 10 (`--brand-primary` with initial, or the uploaded logo) + practice name 16 px 700; centered section nav (segmented, radius 12, tabs 40 px, 15 px): At a glance, Journey, Procedures, Costs, Next steps; right: language select (44 px), reading-level button (P, see below), exit button (§5.9).

Reading level placement (P): the mockup shows the Simple/Standard/Detailed control only on procedure detail. PRD FR-LANG-02 needs it on every page, so the header has an "Aa" 44 px button that opens a popover with the same segmented control; on procedure detail the inline control stays as well.

### 5.11 Patient screens

At a glance (2d, C):

- Grid `460px 1fr`, gap 32, padding 28.
- Left: chart panel `--dc-surface-subtle` radius 16, full-size odontogram (§4.15), non-tooth list under it (P).
- Right, gap 18: eyebrow "Prepared for {{patient_first_name}} · {date}" 14 px 600 `--brand-primary`; headline 38 px 700 ("Your plan treats three teeth in two phases."; counts filled locally from the snapshot); note callout `--brand-soft` radius 14 with "From Dr. {name}'s notes" 12 px 700 and 17 px body, or "Common reasons" title without a note; phase cards (§4.14); bottom summary box (border, radius 14): "Your estimated share" 13 px, figure 30 px 700, "After insurance estimate, not a guarantee" 12 px, and primary button "See your journey ›" (C, de-dashed).

Treatment journey (unmocked, P):

- Horizontal phase timeline across the content width: one column per phase, connected by a 2 px `--dc-border` line with 28 px numbered phase nodes in phase ink.
- Each phase column: phase card header (phase family), procedure list in plan order with tooth labels and a "Learn more" link to the detail.
- Dependencies shown only when documented (e.g. "Crown follows the root canal" only if the phase order or a note states it). Text "Your dental team will schedule the visits" replaces any visit count or date not in the snapshot.
- More than 4 phases: timeline becomes vertical.
- Narration player above the timeline for the journey summary.

Procedure detail (2e, C):

- Grid `284px 1fr 340px` with drawer open, `284px 1fr` closed.
- Left rail `--dc-surface-subtle`, padding 22/18: small odontogram (scale 0.62, selected tooth ringed), step list (§4.16). For non-tooth procedures the chart shows no selection and the eyebrow reads "Not tied to one tooth" (P).
- Center, padding 22/30, gap 14: eyebrow "Tooth 30 · lower right, back" 14 px 600 `--brand-primary`; title 32 px 700; reading-level segmented control at right; illustration slot (140 px tall, radius 14, caption "Educational drawing, not your X-ray"); narration player (§4.12); "What it is" (13 px 700 `#667085` label, 18 px / 1.55 narrated text); accordions 48 px radius 10, 16 px 600: What happens during the visit, Recovery, If you wait, Questions to ask, Sources (P adds the last two).
- Right: Ask drawer (§4.13).

Costs (2f, C):

- Padding 30/40/32, gap 22. Headline row: "What your plan is estimated to cost" 34 px 700 (max 560 px) and right-aligned "Your estimated share" 14 px + 42 px figure.
- Cost bar (§4.17), phase tables, footer row with the disclaimer box and the inline "Ask a question" button.
- When insurance data is absent: bar hidden, figure labelled "Total estimated fees", note "No insurance estimate was recorded for this plan." (P).

What happens next (unmocked, P):

- Two columns (1fr 1fr, gap 24). Left: "What to expect" (balanced outlook, 18 px narrated text with player) and "If you wait" summary per phase, framed as possible consequences, never certainties. Right: "Next steps" numbered list (practical steps such as "Talk with the front desk about scheduling", without dates), and "Questions you might ask" cards (radius 14, `--brand-soft`) that open the Ask drawer prefilled with the question.
- Bottom: neutral box "This presentation explains the plan your dentist proposed. It is not medical advice. Your dental team can answer questions about your own situation." No acceptance button or call to action.

Portrait iPad 820×1180 (P):

- Header becomes two rows: row 1 logo, name, language, Aa, exit; row 2 full-width section nav (tabs flex equally, 44 px).
- At a glance: chart panel on top (full width, chart scaled to 0.85), content below.
- Procedure detail: left rail becomes a horizontal strip (small chart 160 px wide + step list as horizontal scroll chips 44 px); drawer becomes a bottom sheet at 70% height with a drag handle and close button.
- Costs: phase tables stack; headline and share stack.
- Journey: vertical timeline.

## 6. States

| State | Where | Treatment | Origin |
|---|---|---|---|
| Loading (page) | Staff tables | Skeleton rows: 46 px, `--dc-surface-muted` bars, no shimmer under reduced motion | P |
| Loading (variant) | Patient text blocks after language or level change | Text replaced by skeleton lines matching length; heading stays; status text "Preparing Español..." in a polite live region; locked facts remain visible. Never mix old and new language in one section | C/P |
| Loading (audio) | Player | Status "Preparing audio...", button shows spinner, still pausable/cancellable | P |
| Empty | Dashboard, Patients, Presentations, Analytics | One sentence, one action button; no illustration needed | P |
| Generating | Editor opened mid-job | Sections not yet written show the held-slot frame with "Writing..." and the generating family | P |
| Review needed | Editor section, Dashboard card | `review` family: section chip "!", block replaced by reason ("No reference · D5213", "Review pass disagreed with reference", "Clinical change in plan"), actions Regenerate / Write it myself / Hide from presentation | C/P |
| Held (patient) | Patient view and booklet | Neutral card "Your dental team will go over this part with you." No reason text, no AI content | P |
| Stale (plan changed) | Editor, Dashboard | `changed` banner "Open Dental plan changed on {date}. v3 is ready to review." with "Review changes"; patient sessions on the older version are unaffected | C |
| Edit conflict | Editor block | §5.5 banner | C |
| Narration outdated | Checks panel, player | "Outdated after edit"; patient player regenerates on demand or hides the play button until ready | C |
| PMS unreachable | Sync indicator, Create | `review` dot and text; Create disabled with "Open Dental is unreachable. Existing presentations still work." | P |
| Plan deleted in PMS | Editor, Presentations | `review` banner "This plan no longer exists in Open Dental." Present disabled; history viewable | P |
| Incomplete data | Create, patient view | Missing fee/estimate "Not available"; missing tooth goes to non-tooth list with "Location not recorded"; never substituted | C |
| Job failed | Create, Dashboard | `review` family card "Generation failed at {stage}." plain reason, "Retry" primary; no stack trace | P |
| Demo budget reached | Create | §5.3 | C |
| Offline (patient) | Patient view | Cached pages keep working; uncached actions show "Not available right now"; Ask input disabled with a note | P |
| Session expired | Staff | Redirect to login with "Your session ended. Sign in again." | P |

## 7. PDF booklet layouts

Rendered by Typst (DECISIONS E7) from the same version and variant data as the patient view. Fully practice-branded. All colors print-safe and understandable in grayscale (P).

Page setup (P): US Letter default, A4 option in Settings. Margins 18 mm, binding-neutral. Body Figtree 11 pt / 1.5, headings 18 / 14 pt 700, codes in IBM Plex Mono 9 pt. Fonts embedded (bundled; no system fonts). Language and reading level are those selected at export.

Running footer on every page except the cover: practice name, "Plan dated {date}", "Version {n} · {short hash}", page "n of N". Running header: section name.

| Page | Content |
|---|---|
| 1. Cover | Practice logo and name; title "Your treatment plan, explained"; "Prepared for {first name} on {date}"; language; practice contact block as configured in Settings. No DentalClarity branding |
| 2. At a glance | Odontogram (vector, grayscale-safe: highlighted teeth filled accent with count badge and bold number); summary sentence; note callout or "Common reasons"; phase summary boxes; estimated share with "estimate, not a guarantee" |
| 3. Your treatment journey | Phase timeline (vertical), procedures per phase with tooth labels; only documented order and dependencies |
| 4+. Procedures | One procedure section per page or more: tooth and location, illustration (same SVG as the app, with caption "Educational drawing, not your X-ray"), What it is, Why recommended / Common reasons, During the visit, Recovery, If you wait, Questions to ask. Section break before each procedure; avoid orphaned headings |
| Costs | Cost bar with values written on it, phase tables with fee and your share, totals, disclaimer "These are estimates from your plan on {date}. Your insurance decides the final amount, which can change." |
| What happens next | Outlook, next steps, questions list with blank lines for the patient's own notes |
| References | Numbered list of sources used (title, organization, URL), and the line "This booklet explains the plan your dentist proposed. It is not medical advice." |

Held sections print the held-patient card text (§6). Untranslated content never prints in a different language than selected; if a variant is incomplete, export is blocked with the reason (P).

## 8. Open questions and source conflicts

- `#2A7DD1` with white text fails AA at the sizes used. Proposed `--dc-primary-fill` `#1F72C4`; DECISIONS U1 names `#2A7DD1`, so the owner should confirm.
- White numerals on coral teeth fail AA; proposed dark ink. Alternative: darker coral fill `#C4532E` (4.54 with white) if the owner prefers white numerals.
- The mockup marks highlighted teeth by color only; this spec adds a count badge and bold number.
- "Matched 6 reviewed references" contradicts D9 (all entries unreviewed until a dentist signs off). Copy changed.
- Generation progress lists 7 sections (no Cleaning), while the editor shows a separate Cleaning section. This spec follows the editor; the progress chips list every section.
- The mockup has no Present exit control, no reading-level control outside procedure detail, and no floating Ask button in t2. This spec adds all three.
- The patient label is "Ask a question" everywhere in the UI.
- The mockup loads fonts from Google Fonts; production self-hosts them (E8).
- The at-a-glance note callout paraphrases the clinical note ("From Dr. Patel's notes"). Staff provenance labels it "From clinical note", and it needs the same one-click staff confirmation (P11).
