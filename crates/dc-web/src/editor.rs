//! Presentation editor (mockup 2c): section rail, the selected section with
//! editable text blocks, the checks panel, and the Present button.

use crate::api;
use crate::format::{
    CONTENT_LANGUAGES, content_language, language_name, money_view, timestamp, tooth_label,
};
use crate::motion::{SkeletonCard, restore_focus, view_transition};
use crate::patients::patient_href;
use crate::progress::{GenerationProgress, stage_text};
use crate::session::use_session;
use dc_core::Mode;
use dc_core::content::ReadingLevel;
use dc_core::present::{ConfirmNoteRequest, RevertBlockRequest, UpdateBlockRequest};
use dc_core::presentation::{
    BlockSource, JobState, JobStatus, PresentationView, ProcedureFactView, SectionStage,
    SectionView, SectionVisibility, TextBlock,
};
use leptos::ev::KeyboardEvent;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos::wasm_bindgen::JsCast;
use leptos_router::components::A;
use leptos_router::hooks::use_params_map;
use std::time::Duration;

/// Key of the costs entry, which is built from the plan rather than written.
const COSTS_KEY: &str = "costs";

const COST_CAVEAT: &str = "These are estimates from your plan. Your insurance decides the final amount, which can change.";

const HELD_NOTE: &str = "Held from patients until staff release it or it is written again.";
const RELEASE_CONFIRM: &str = "You've checked this section. It will be shown to the patient.";

/// How often a section rewrite is polled.
const REWRITE_POLL_EVERY: Duration = Duration::from_secs(1);
/// The wait after a failed poll, which is usually a dropped request.
const REWRITE_RETRY_EVERY: Duration = Duration::from_secs(3);
const DEMO_NOTE: &str = "This is a public demo. Don't enter real personal or health information.";

#[component]
pub fn PresentationPage() -> impl IntoView {
    let params = use_params_map();
    let id = move || params.read().get("id").unwrap_or_default();
    let view_data = LocalResource::new(move || api::presentation(id()));
    let refetch = Callback::new(move |_| view_data.refetch());

    view! {
        <div id="main-content" class="subpage" tabindex="-1">
            <Suspense fallback=|| view! {
                <header class="subbar"><BackLink href="/presentations".into() label="Back to presentations" /></header>
                <div class="editor-grid editor-skeleton">
                    <div class="section-rail"></div>
                    <div class="editor-canvas"><SkeletonCard label="Loading presentation" lines=5 /></div>
                    <div class="checks-rail"></div>
                </div>
            }>
                {move || view_data.get().map(|result| match result {
                    // A section rewrite leaves the other sections readable,
                    // so only a whole-presentation job shows the progress card.
                    Ok(p) if !p.job_state.is_finished() && p.sections.is_empty() => {
                        let job_id = p.job_id.clone();
                        let presentation_id = p.id.clone();
                        view! {
                            <EditorBar data=RwSignal::new(p) />
                            <div class="subpage-body running-body">
                                <GenerationProgress job_id presentation_id on_finished=refetch />
                            </div>
                        }.into_any()
                    }
                    Ok(p) => {
                        let data = RwSignal::new(p);
                        view! { <EditorBar data /> <Editor data /> }.into_any()
                    }
                    Err(failure) => view! {
                        <header class="subbar"><BackLink href="/presentations".into() label="Back to presentations" /></header>
                        <div class="subpage-body">
                            <div class="notice error" role="alert">{failure.message}</div>
                        </div>
                    }.into_any(),
                })}
            </Suspense>
        </div>
    }
}

#[component]
fn BackLink(href: String, label: &'static str) -> impl IntoView {
    view! {
        <A href=href attr:class="back-button" attr:aria-label=label>
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke-width="2"
                stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                <path d="M15 5l-7 7 7 7" />
            </svg>
        </A>
    }
}

fn procedure_count(n: usize) -> String {
    if n == 1 {
        "1 procedure".into()
    } else {
        format!("{n} procedures")
    }
}

/// Present needs a finished job and at least one section patients may see.
fn can_present(p: &PresentationView) -> bool {
    p.job_state.is_finished()
        && p.sections
            .iter()
            .any(|s| s.visibility == SectionVisibility::Ready)
}

/// Sends the browser to Present mode with a full page load, so no staff
/// state stays in memory. The editor's history entry is replaced.
fn load_page(path: &str) -> Result<(), String> {
    // Replace, not assign: while Present runs, this page shows the staff
    // lock screen, so Back from Present must not lead here.
    window()
        .location()
        .replace(path)
        .map_err(|_| "The browser could not open Present mode.".to_owned())
}

#[component]
fn EditorBar(data: RwSignal<PresentationView>) -> impl IntoView {
    let (id, name, meta, version, created, patient) = data.with_untracked(|p| {
        (
            p.id.clone(),
            p.patient_display_name.clone(),
            format!("{} · {}", p.plan_name, procedure_count(p.procedures.len())),
            format!("v{}", p.version),
            timestamp(&p.created_at),
            patient_href(&p.patient_key),
        )
    });
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let ready = move || data.with(can_present);
    let present = move |_| {
        if !ready() || busy.get_untracked() {
            return;
        }
        busy.set(true);
        error.set(None);
        let id = id.clone();
        spawn_local(async move {
            match api::start_present(id).await {
                Ok(started) => {
                    if let Err(message) = load_page(&started.present_path) {
                        error.set(Some(message));
                        busy.set(false);
                    }
                }
                Err(failure) => {
                    error.set(Some(failure.message));
                    busy.set(false);
                }
            }
        });
    };
    view! {
        <header class="subbar editor-bar">
            <BackLink href=patient label="Back to patient" />
            <div class="editor-who">
                <h1 class="editor-name">{name}</h1>
                <p class="editor-meta">{meta}</p>
            </div>
            <span class="version-chip">
                <b>{version}</b>
                <span class="muted">{created}</span>
            </span>
            <div class="editor-actions">
                <button type="button" class="button text" aria-disabled="true"
                    title="Version history arrives in a later update">"History"</button>
                <button type="button" class="button text" aria-disabled="true"
                    title="Writing the whole presentation again arrives in a later update">"Regenerate"</button>
                <button type="button" class="button" aria-disabled="true"
                    title="The booklet arrives in a later update">"Export booklet"</button>
                <button type="button" class="button primary present-button"
                    aria-disabled=move || (!ready() || busy.get()).then_some("true")
                    aria-busy=move || busy.get().then_some("true")
                    title=move || (!ready()).then_some("No section is ready to show patients yet.")
                    on:click=present
                >
                    <span class="play" aria-hidden="true"></span>
                    {move || if busy.get() { "Starting…" } else { "Present" }}
                </button>
            </div>
            {move || error.get().map(|message| view! {
                <p class="present-error" role="alert">{message}</p>
            })}
        </header>
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Mark {
    Ready,
    Review,
    Failed,
    Auto,
}

#[derive(Clone)]
struct NavEntry {
    key: String,
    label: String,
    mark: Mark,
    procedure: bool,
}

fn mark_for(visibility: SectionVisibility) -> Mark {
    match visibility {
        SectionVisibility::Ready => Mark::Ready,
        SectionVisibility::NeedsReview => Mark::Review,
        SectionVisibility::Failed => Mark::Failed,
    }
}

fn is_procedure(section: &SectionView) -> bool {
    section.procedure_ref.is_some() || section.key.starts_with("procedure:")
}

fn fact_for<'a>(p: &'a PresentationView, section: &SectionView) -> Option<&'a ProcedureFactView> {
    let wanted = section
        .procedure_ref
        .as_deref()
        .or_else(|| section.key.strip_prefix("procedure:"))?;
    p.procedures.iter().find(|f| f.procedure_ref == wanted)
}

/// Rail label, e.g. "Root canal · #30".
fn procedure_label(p: &PresentationView, section: &SectionView) -> String {
    match fact_for(p, section) {
        Some(fact) => {
            let tooth = tooth_label(&fact.tooth);
            if tooth.is_empty() {
                fact.name.clone()
            } else {
                format!("{} · {tooth}", fact.name)
            }
        }
        None => section.title.clone(),
    }
}

fn nav_label(section: &SectionView) -> String {
    match section.key.as_str() {
        "at_a_glance" => "At a glance".into(),
        "journey" => "Treatment journey".into(),
        "next_steps" => "What happens next".into(),
        _ => section.title.clone(),
    }
}

/// Rail order: overview sections, procedures, costs, then what happens next.
fn nav_entries(p: &PresentationView) -> Vec<NavEntry> {
    let entry = |s: &SectionView, procedure: bool| NavEntry {
        key: s.key.clone(),
        label: if procedure {
            procedure_label(p, s)
        } else {
            nav_label(s)
        },
        mark: mark_for(s.visibility),
        procedure,
    };
    let mut entries: Vec<NavEntry> = p
        .sections
        .iter()
        .filter(|s| !is_procedure(s) && s.key != "next_steps")
        .map(|s| entry(s, false))
        .collect();
    entries.extend(
        p.sections
            .iter()
            .filter(|s| is_procedure(s))
            .map(|s| entry(s, true)),
    );
    entries.push(NavEntry {
        key: COSTS_KEY.into(),
        label: "Costs".into(),
        mark: Mark::Auto,
        procedure: false,
    });
    entries.extend(
        p.sections
            .iter()
            .filter(|s| s.key == "next_steps")
            .map(|s| entry(s, false)),
    );
    entries
}

/// What the checks panel says about the last change to a block.
#[derive(Clone, PartialEq)]
enum ChangeKind {
    Saved,
    Reverted,
    Confirmed,
    /// Staff released a held section to patients.
    Released,
    /// The section was written again.
    Rewritten,
}

#[derive(Clone, PartialEq)]
struct ChangeNote {
    section_key: String,
    heading: String,
    kind: ChangeKind,
    warnings: Vec<String>,
    /// Whether patients can see the section after the change.
    shown: bool,
}

/// Identifies one block: section key and block field.
type BlockKey = (String, String);

/// A section being written again, and the stage its job reports.
#[derive(Clone, PartialEq)]
struct Rewrite {
    key: String,
    stage: SectionStage,
}

/// Shared editor state. Only one block is edited at a time.
#[derive(Clone, Copy)]
struct EditCtx {
    id: StoredValue<String>,
    data: RwSignal<PresentationView>,
    editing: RwSignal<Option<BlockKey>>,
    draft: RwSignal<String>,
    busy: RwSignal<Option<BlockKey>>,
    error: RwSignal<Option<(BlockKey, String)>>,
    note: RwSignal<Option<ChangeNote>>,
    demo: Signal<bool>,
    /// The section being written again. One at a time per presentation.
    rewrite: RwSignal<Option<Rewrite>>,
    /// A release request is in flight.
    releasing: RwSignal<bool>,
    /// The section whose release confirmation is open.
    release_confirm: RwSignal<Option<String>>,
    /// A refused rewrite or release, by section key.
    section_error: RwSignal<Option<(String, String)>>,
}

fn dom_id(prefix: &str, key: &BlockKey) -> String {
    let clean = |s: &str| {
        s.chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect::<String>()
    };
    format!("{prefix}-{}-{}", clean(&key.0), clean(&key.1))
}

/// Moves focus to an element once the next frame has rendered it.
fn focus_later(id: String) {
    request_animation_frame(move || {
        if let Some(el) = document()
            .get_element_by_id(&id)
            .and_then(|e| e.dyn_into::<leptos::web_sys::HtmlElement>().ok())
        {
            let _ = el.focus();
        }
    });
}

/// The section heading in the editor pane. It takes focus when the control
/// that held it went away with nothing to replace it, for example after a
/// release moves the section out of review.
const PANE_TITLE_ID: &str = "pane-title";

/// Element ids for one held section's actions.
struct HeldIds {
    confirm: String,
    cancel: String,
    release: String,
    regenerate: String,
    progress: String,
}

impl HeldIds {
    fn new(key: &str) -> Self {
        let base = dom_id("s", &(key.to_owned(), String::new()));
        Self {
            confirm: format!("release-confirm-{base}"),
            cancel: format!("release-cancel-{base}"),
            release: format!("release-{base}"),
            regenerate: format!("regenerate-{base}"),
            progress: format!("rewrite-{base}"),
        }
    }

    /// Where focus goes when a confirmation or rewrite ends: back to the
    /// button that started it when it is still there, else the pane heading.
    fn after_action(&self, started_from: &str) -> Vec<String> {
        let mut order = vec![started_from.to_owned()];
        for id in [&self.release, &self.regenerate] {
            if *id != started_from {
                order.push(id.clone());
            }
        }
        order.push(PANE_TITLE_ID.into());
        order
    }
}

/// One sentence per line; blank lines are dropped.
fn sentences_from(draft: &str) -> Vec<String> {
    draft
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect()
}

enum BlockAction {
    Save(Vec<String>),
    Revert,
    Confirm,
}

/// Staff wording for a rewrite or release refused while a job still runs.
const STILL_WRITING: &str =
    "This presentation is still being written. Wait for it to finish, then try again.";

/// Staff wording for a refused rewrite.
fn regenerate_message(failure: &api::Failure) -> String {
    let wait = failure
        .retry_after
        .map(|secs| match secs {
            0..=89 => format!(" Try again in {secs} seconds."),
            _ => format!(" Try again in about {} minutes.", secs.div_ceil(60)),
        })
        .unwrap_or_default();
    match failure.code.as_str() {
        "no_knowledge_entry" => "There's no reviewed content for this procedure yet, so it can't be written. It stays held from patients.".into(),
        "generation_in_progress" if failure.status == 429 => {
            format!("Another presentation is still being written. Wait for it to finish.{wait}")
        }
        "generation_in_progress" => STILL_WRITING.into(),
        "rate_limited" if !wait.is_empty() => {
            format!("The demo limits how often sections can be written again.{wait}")
        }
        "budget_reached" => {
            "The AI budget for this month is used up, so the section can't be written again. It stays held from patients.".into()
        }
        "provider_not_configured" => {
            "No AI writer is connected, so the section can't be written again. It stays held from patients.".into()
        }
        _ => failure.message.clone(),
    }
}

/// Staff wording for a refused release.
fn release_message(failure: &api::Failure) -> String {
    match failure.code.as_str() {
        // The message lists the findings.
        "release_blocked" => failure.message.clone(),
        "nothing_to_release" => {
            "This section has no text that passed the checks. Write it again instead.".into()
        }
        "already_ready" => "This section is already shown to patients.".into(),
        "generation_in_progress" => STILL_WRITING.into(),
        _ => failure.message.clone(),
    }
}

impl EditCtx {
    /// Whether a block edit, release or rewrite is running.
    fn any_busy(self) -> bool {
        self.busy.with(Option::is_some)
            || self.releasing.get()
            || self.rewrite.with(Option::is_some)
    }

    /// Whether the editor was left, so its signals are gone. Checked when a
    /// request returns.
    fn gone(self) -> bool {
        self.rewrite.try_with_untracked(|_| ()).is_none()
    }

    fn any_busy_untracked(self) -> bool {
        self.busy.with_untracked(Option::is_some)
            || self.releasing.get_untracked()
            || self.rewrite.with_untracked(Option::is_some)
    }

    /// Replaces one section with the server's copy, cross-fading the canvas.
    fn apply_section(self, section: SectionView) {
        view_transition("vt-state", move || {
            self.data.update(|p| {
                if let Some(slot) = p.sections.iter_mut().find(|s| s.key == section.key) {
                    *slot = section;
                }
            });
        });
    }

    fn release(self, key: String, heading: String) {
        if self.any_busy_untracked() {
            return;
        }
        self.releasing.set(true);
        self.section_error.set(None);
        let id = self.id.get_value();
        spawn_local(async move {
            let result = api::release_section(id, key.clone()).await;
            if self.releasing.try_set(false).is_some() {
                // The editor was left while the request ran.
                return;
            }
            self.release_confirm.set(None);
            let ids = HeldIds::new(&key);
            match result {
                Ok(change) => {
                    let shown = change.section.visibility == SectionVisibility::Ready;
                    self.apply_section(change.section);
                    self.note.set(Some(ChangeNote {
                        section_key: key,
                        heading,
                        kind: ChangeKind::Released,
                        warnings: change.warnings,
                        shown,
                    }));
                }
                Err(failure) => {
                    if failure.code == "already_ready" {
                        self.reload(key.clone(), None);
                    }
                    self.section_error
                        .set(Some((key, release_message(&failure))));
                }
            }
            // After the match, so a successful release has started the
            // transition that rebuilds the pane and focus waits for it.
            restore_focus(ids.after_action(&ids.release));
        });
    }

    fn regenerate(self, key: String, heading: String) {
        if self.any_busy_untracked() {
            return;
        }
        self.section_error.set(None);
        self.release_confirm.set(None);
        self.rewrite.set(Some(Rewrite {
            key: key.clone(),
            stage: SectionStage::Queued,
        }));
        restore_focus(vec![HeldIds::new(&key).progress]);
        let id = self.id.get_value();
        spawn_local(async move {
            let result = api::regenerate_section(id, key.clone()).await;
            if self.gone() {
                return;
            }
            match result {
                Ok(started) => {
                    // Present waits for the job, as after creating a presentation.
                    self.data.update(|p| p.job_state = JobState::Queued);
                    self.watch(started.job_id, key, heading);
                }
                Err(failure) => {
                    self.rewrite.set(None);
                    self.focus_after_rewrite(&key);
                    self.section_error
                        .set(Some((key, regenerate_message(&failure))));
                }
            }
        });
    }

    /// Picks up a rewrite that was already running when the editor opened.
    fn resume(self, job_id: String) {
        spawn_local(async move {
            let Ok(job) = api::job(job_id.clone()).await else {
                return;
            };
            let Some(key) = job.section_key.clone() else {
                return;
            };
            if self.gone() {
                return;
            }
            let heading = self
                .data
                .with_untracked(|p| {
                    p.sections
                        .iter()
                        .find(|s| s.key == key)
                        .map(|s| s.title.clone())
                })
                .unwrap_or_default();
            self.rewrite.set(Some(Rewrite {
                key: key.clone(),
                stage: stage_of(&job, &key),
            }));
            self.on_job(job, job_id, key, heading);
        });
    }

    /// Polls a rewrite job until it finishes, then reloads the presentation.
    /// Polling stops when the editor unmounts, since its signals are gone.
    fn watch(self, job_id: String, key: String, heading: String) {
        spawn_local(async move {
            let result = api::job(job_id.clone()).await;
            if self.gone() {
                return;
            }
            match result {
                Ok(job) => self.on_job(job, job_id, key, heading),
                Err(failure) if failure.status == 404 => {
                    self.rewrite.set(None);
                    self.focus_after_rewrite(&key);
                    self.section_error.set(Some((key, failure.message)));
                }
                // Other failures are retried; a lost session is handled by
                // the session gate.
                Err(_) => set_timeout(
                    move || self.watch(job_id, key, heading),
                    REWRITE_RETRY_EVERY,
                ),
            }
        });
    }

    fn on_job(self, job: JobStatus, job_id: String, key: String, heading: String) {
        if job.state.is_finished() {
            self.reload(key, Some((job, heading)));
            return;
        }
        let stage = stage_of(&job, &key);
        if self.rewrite.with_untracked(|r| r.as_ref().map(|r| r.stage)) != Some(stage) {
            self.rewrite.set(Some(Rewrite {
                key: key.clone(),
                stage,
            }));
        }
        set_timeout(move || self.watch(job_id, key, heading), REWRITE_POLL_EVERY);
    }

    /// Reloads the presentation in place, so the selected section stays
    /// selected. With a finished rewrite job, also reports how it went.
    fn reload(self, key: String, finished: Option<(JobStatus, String)>) {
        let id = self.id.get_value();
        // A finished rewrite started from Regenerate; any other reload
        // follows a refused release.
        let ids = HeldIds::new(&key);
        let focus_order = if finished.is_some() {
            ids.after_action(&ids.regenerate)
        } else {
            ids.after_action(&ids.release)
        };
        spawn_local(async move {
            let fresh = api::presentation(id).await;
            if self.gone() {
                return;
            }
            match fresh {
                Ok(p) => {
                    let shown = p
                        .sections
                        .iter()
                        .any(|s| s.key == key && s.visibility == SectionVisibility::Ready);
                    view_transition("vt-state", move || {
                        self.data.set(p);
                        self.rewrite.set(None);
                    });
                    restore_focus(focus_order.clone());
                    if let Some((job, heading)) = finished {
                        if let Some(error) = job.error {
                            self.section_error.set(Some((key.clone(), error)));
                        }
                        self.note.set(Some(ChangeNote {
                            section_key: key,
                            heading,
                            kind: ChangeKind::Rewritten,
                            warnings: Vec::new(),
                            shown,
                        }));
                    }
                }
                Err(failure) => {
                    self.rewrite.set(None);
                    restore_focus(focus_order);
                    self.section_error.set(Some((
                        key,
                        format!(
                            "The presentation could not be reloaded. {}",
                            failure.message
                        ),
                    )));
                }
            }
        });
    }

    /// A rewrite replaces its progress line and, when it finishes, the
    /// whole pane. Focus that was on either goes back to the section's
    /// actions, or to the heading once the section has none.
    fn focus_after_rewrite(self, key: &str) {
        let ids = HeldIds::new(key);
        restore_focus(ids.after_action(&ids.regenerate));
    }

    fn run(self, key: BlockKey, heading: String, action: BlockAction) {
        if self.any_busy_untracked() {
            return;
        }
        self.busy.set(Some(key.clone()));
        self.error.set(None);
        let id = self.id.get_value();
        spawn_local(async move {
            let (section_key, field) = key.clone();
            let (kind, result) = match action {
                BlockAction::Save(sentences) => (
                    ChangeKind::Saved,
                    api::update_block(
                        id,
                        &UpdateBlockRequest {
                            section_key,
                            field,
                            sentences,
                        },
                    )
                    .await,
                ),
                BlockAction::Revert => (
                    ChangeKind::Reverted,
                    api::revert_block(id, &RevertBlockRequest { section_key, field }).await,
                ),
                BlockAction::Confirm => (
                    ChangeKind::Confirmed,
                    api::confirm_note(id, &ConfirmNoteRequest { section_key, field }).await,
                ),
            };
            self.busy.set(None);
            match result {
                Ok(change) => {
                    let shown = change.section.visibility == SectionVisibility::Ready;
                    let section_key = change.section.key.clone();
                    self.data.update(|p| {
                        if let Some(slot) = p.sections.iter_mut().find(|s| s.key == section_key) {
                            *slot = change.section;
                        }
                    });
                    if kind == ChangeKind::Saved {
                        self.editing.set(None);
                    }
                    self.note.set(Some(ChangeNote {
                        section_key,
                        heading,
                        kind,
                        warnings: change.warnings,
                        shown,
                    }));
                    focus_later(dom_id("block", &key));
                }
                Err(failure) => self.error.set(Some((key, failure.message))),
            }
        });
    }
}

#[component]
fn Editor(data: RwSignal<PresentationView>) -> impl IntoView {
    let session = use_session();
    let demo = Signal::derive(move || {
        session.info.get().and_then(Result::ok).map(|i| i.mode) == Some(Mode::Demo)
    });
    let ctx = EditCtx {
        id: StoredValue::new(data.with_untracked(|p| p.id.clone())),
        data,
        editing: RwSignal::new(None),
        draft: RwSignal::new(String::new()),
        busy: RwSignal::new(None),
        error: RwSignal::new(None),
        note: RwSignal::new(None),
        demo,
        rewrite: RwSignal::new(None),
        releasing: RwSignal::new(false),
        release_confirm: RwSignal::new(None),
        section_error: RwSignal::new(None),
    };
    let (job_id, running) = data.with_untracked(|p| (p.job_id.clone(), !p.job_state.is_finished()));
    if running {
        ctx.resume(job_id);
    }
    let first = data.with_untracked(|p| {
        nav_entries(p)
            .first()
            .map(|e| e.key.clone())
            .unwrap_or_else(|| COSTS_KEY.into())
    });
    let selected = RwSignal::new(first);
    let no_sections = move || data.with(|p| p.sections.is_empty());

    view! {
        <div class="editor-grid reveal">
            {move || {
                let entries = data.with(nav_entries);
                view! { <SectionRail entries selected rewrite=ctx.rewrite /> }
            }}
            <main class="editor-canvas">
                {move || no_sections().then(|| view! {
                    <div class="notice error" role="alert">
                        "No sections were written for this version. Patients cannot be shown this presentation."
                    </div>
                })}
                {move || {
                    let key = selected.get();
                    data.with(|p| {
                        if key == COSTS_KEY {
                            view! { <CostsPane p=p.clone() /> }.into_any()
                        } else {
                            match p.sections.iter().find(|s| s.key == key) {
                                Some(section) => section_pane(p, section, ctx),
                                None => view! { <div class="notice">"Select a section."</div> }.into_any(),
                            }
                        }
                    })
                }}
            </main>
            <aside class="checks-rail" aria-labelledby="checks-title">
                <h2 id="checks-title" class="rail-title">"Checks"</h2>
                <div role="status" class="change-status">
                    {move || {
                        let key = selected.get();
                        ctx.note
                            .get()
                            .filter(|n| n.section_key == key)
                            .map(change_note_view)
                    }}
                </div>
                {move || {
                    let key = selected.get();
                    data.with(|p| checks_for(p, &key))
                }}
                <div class="rail-divider"></div>
                <h2 class="rail-title">"Details"</h2>
                {move || data.with(details)}
                <crate::questions::PatientQuestions id=ctx.id.get_value() />
            </aside>
        </div>
    }
}

/// The stage a job reports for one section; queued until it is listed.
fn stage_of(job: &JobStatus, key: &str) -> SectionStage {
    job.sections
        .iter()
        .find(|s| s.key == key)
        .map_or(SectionStage::Queued, |s| s.stage)
}

fn change_note_view(note: ChangeNote) -> AnyView {
    let outcome = match (&note.kind, note.shown) {
        (ChangeKind::Saved, true) => "Saved. Patients will see your text.",
        (ChangeKind::Saved, false) => "Saved. This section is still held from patients.",
        (ChangeKind::Reverted, true) => "Restored the generated text. Patients will see it.",
        (ChangeKind::Reverted, false) => {
            "Restored the generated text. This section is still held from patients."
        }
        (ChangeKind::Confirmed, true) => "Confirmed. Patients will see this text.",
        (ChangeKind::Confirmed, false) => "Confirmed. This section is still held from patients.",
        (ChangeKind::Released, _) => "Released. Patients will see this section.",
        (ChangeKind::Rewritten, true) => {
            "Written again and passed the checks. Patients will see it."
        }
        (ChangeKind::Rewritten, false) => {
            "Written again. This section is still held from patients."
        }
    };
    if note.warnings.is_empty() {
        return view! {
            <ul class="check-list">
                <li class="check-row">
                    <span class="check-mark" aria-hidden="true">"✓"</span>
                    <span><b>{note.heading}</b>": "{outcome}</span>
                </li>
            </ul>
        }
        .into_any();
    }
    let lead = match note.kind {
        ChangeKind::Saved => "Check this edit:",
        _ => "Check this text:",
    };
    view! {
        <div class="check-callout changed">
            <span class="callout-mark" aria-hidden="true">"!"</span>
            <div>
                <p class="callout-lead">{note.heading}" · "{lead}</p>
                <ul class="reason-list">
                    {note.warnings.into_iter().map(|w| view! { <li>{w}</li> }).collect_view()}
                </ul>
                <p class="callout-outcome">{outcome}</p>
            </div>
        </div>
    }
    .into_any()
}

#[component]
fn SectionRail(
    entries: Vec<NavEntry>,
    selected: RwSignal<String>,
    rewrite: RwSignal<Option<Rewrite>>,
) -> impl IntoView {
    let first_procedure = entries.iter().position(|e| e.procedure);
    let costs_at = entries.iter().position(|e| e.key == COSTS_KEY);
    view! {
        <nav class="section-rail" aria-label="Sections">
            <p class="rail-label">"Sections"</p>
            <ul class="rail-list">
                {entries.into_iter().enumerate().map(|(i, entry)| {
                    let group = (Some(i) == first_procedure).then(|| view! {
                        <li class="rail-group" aria-hidden="true">"Procedures"</li>
                    });
                    let key = entry.key.clone();
                    let rewriting = {
                        let key = key.clone();
                        Memo::new(move |_| rewrite.with(|r| r.as_ref().is_some_and(|r| r.key == key)))
                    };
                    let key_for_click = key.clone();
                    let is_current = move || selected.get() == key;
                    let (mark, mark_label) = match entry.mark {
                        Mark::Ready => ("✓", "Ready"),
                        Mark::Review => ("!", "Needs review"),
                        Mark::Failed => ("×", "Failed"),
                        Mark::Auto => ("auto", "Built from the plan"),
                    };
                    let mark_class = match entry.mark {
                        Mark::Ready => "rail-mark ready",
                        Mark::Review => "rail-mark review",
                        Mark::Failed => "rail-mark failed",
                        Mark::Auto => "rail-mark auto",
                    };
                    let spaced = Some(i) == costs_at && i > 0;
                    view! {
                        {group}
                        <li>
                            <button
                                type="button"
                                class="rail-item"
                                class:indent=entry.procedure
                                class:spaced=spaced
                                aria-current=move || if is_current() { Some("true") } else { None }
                                on:click=move |_| {
                                    let key = key_for_click.clone();
                                    if selected.get_untracked() != key {
                                        view_transition("vt-section", move || {
                                            selected.set(key);
                                            if let Ok(Some(canvas)) = document().query_selector(".editor-canvas") {
                                                canvas.set_scroll_top(0);
                                            }
                                        });
                                    }
                                }
                            >
                                <span class="rail-text">{entry.label}</span>
                                <span class=mark_class class:rewriting=rewriting aria-hidden="true">{mark}</span>
                                <span class="visually-hidden">
                                    {move || if rewriting.get() { ", Writing again".to_owned() } else { format!(", {mark_label}") }}
                                </span>
                            </button>
                        </li>
                    }
                }).collect_view()}
            </ul>
        </nav>
    }
}

fn section_pane(p: &PresentationView, section: &SectionView, ctx: EditCtx) -> AnyView {
    let procedures: Vec<&SectionView> = p.sections.iter().filter(|s| is_procedure(s)).collect();
    let eyebrow = procedures
        .iter()
        .position(|s| s.key == section.key)
        .map(|i| format!("Procedure {} of {}", i + 1, procedures.len()));
    let fact = fact_for(p, section).cloned();
    let held = match section.visibility {
        SectionVisibility::Ready => None,
        SectionVisibility::NeedsReview => Some(("held-note review", HELD_NOTE)),
        SectionVisibility::Failed => Some((
            "held-note failed",
            "This section failed its checks and is never shown to patients.",
        )),
    };
    let reasons = (section.visibility != SectionVisibility::Ready && !section.reasons.is_empty())
        .then(|| section.reasons.clone());
    let editable = section.visibility != SectionVisibility::Failed;
    let shown = section.visibility == SectionVisibility::Ready;
    let section_key = section.key.clone();
    let blocks = section.blocks.clone();
    let questions = section.questions.clone();
    let empty = blocks.is_empty() && questions.is_empty();
    view! {
        <div class="pane-head">
            {eyebrow.map(|e| view! { <p class="eyebrow">{e}</p> })}
            <h2 class="pane-title" id=PANE_TITLE_ID tabindex="-1">{section.title.clone()}</h2>
        </div>
        {fact.map(|fact| view! { <LockedStrip fact /> })}
        {held.map(|(class, text)| view! {
            <div class=class>
                <p class="held-lead">{text}</p>
                {reasons.map(|reasons| view! {
                    <ul class="reason-list">
                        {reasons.into_iter().map(|r| view! { <li>{r}</li> }).collect_view()}
                    </ul>
                })}
                <HeldActions section=section.clone() ctx />
            </div>
        })}
        {blocks.into_iter().map(|block| view! {
            <BlockCard block section_key=section_key.clone() editable shown ctx />
        }).collect_view()}
        {(!questions.is_empty()).then(|| view! {
            <article class="block-card">
                <div class="block-head"><h3 class="block-title">"Questions to ask"</h3></div>
                <ul class="question-list">
                    {questions.into_iter().map(|q| view! { <li>{q}</li> }).collect_view()}
                </ul>
            </article>
        })}
        {(empty && section.visibility == SectionVisibility::Ready).then(|| view! {
            <p class="notice">"This section has no text."</p>
        })}
    }
    .into_any()
}

/// What staff can do with a held section: write it again, or release it to
/// patients after checking it. When release is refused, the findings that
/// block it are listed so staff know what to fix.
#[component]
fn HeldActions(section: SectionView, ctx: EditCtx) -> impl IntoView {
    let key = section.key.clone();
    let heading = section.title.clone();
    let can_regenerate = section.can_regenerate;
    let can_release = section.can_release;
    let blockers = (!can_release && section.visibility == SectionVisibility::NeedsReview)
        .then_some(section.release_blockers)
        .filter(|b| !b.is_empty());

    let rewriting = {
        let key = key.clone();
        Memo::new(move |_| {
            ctx.rewrite
                .with(|r| r.as_ref().filter(|r| r.key == key).map(|r| r.stage))
        })
    };
    let confirming = {
        let key = key.clone();
        Memo::new(move |_| {
            ctx.release_confirm
                .with(|c| c.as_deref() == Some(key.as_str()))
        })
    };
    let error = {
        let key = key.clone();
        move || {
            ctx.section_error.with(|e| {
                e.as_ref()
                    .filter(|(k, _)| *k == key)
                    .map(|(_, m)| m.clone())
            })
        }
    };
    let ids = HeldIds::new(&key);
    let after_cancel = ids.after_action(&ids.release);

    let regenerate = {
        let key = key.clone();
        let heading = heading.clone();
        move |_| ctx.regenerate(key.clone(), heading.clone())
    };
    let ask_release = {
        let key = key.clone();
        let cancel_id = ids.cancel.clone();
        move |_| {
            if ctx.any_busy_untracked() {
                return;
            }
            ctx.section_error.set(None);
            ctx.release_confirm.set(Some(key.clone()));
            // Cancel is the safe choice, so focus starts there.
            restore_focus(vec![cancel_id.clone()]);
        }
    };
    let release = {
        let key = key.clone();
        let heading = heading.clone();
        move |_| ctx.release(key.clone(), heading.clone())
    };
    let cancel_release = move || {
        if ctx.releasing.get_untracked() {
            return;
        }
        ctx.release_confirm.set(None);
        restore_focus(after_cancel.clone());
    };
    let confirm_keydown = {
        let cancel_release = cancel_release.clone();
        move |ev: KeyboardEvent| {
            if ev.key() == "Escape" {
                ev.prevent_default();
                cancel_release();
            }
        }
    };

    let actions = move || {
        if let Some(stage) = rewriting.get() {
            // Rebuilt on each stage change, so the new stage settles in.
            return view! {
                <div class="rewrite-progress" id=ids.progress.clone() tabindex="-1" role="status">
                    <span class="rewrite-dot" aria-hidden="true"></span>
                    <span class="rewrite-stage">{format!("Writing this section again · {}", stage_text(stage))}</span>
                </div>
            }
            .into_any();
        }
        if confirming.get() {
            let release = release.clone();
            let cancel_release = cancel_release.clone();
            return view! {
                <div class="release-confirm" id=ids.confirm.clone() role="group"
                    aria-label="Confirm release" on:keydown=confirm_keydown.clone()>
                    <p class="release-confirm-text">{RELEASE_CONFIRM}</p>
                    <div class="held-action-row">
                        <button type="button" class="button primary small"
                            disabled=move || ctx.releasing.get()
                            on:click=release>
                            {move || if ctx.releasing.get() { "Releasing…" } else { "Release to patient" }}
                        </button>
                        <button type="button" class="button small" id=ids.cancel.clone()
                            disabled=move || ctx.releasing.get()
                            on:click=move |_| cancel_release()>"Cancel"</button>
                    </div>
                </div>
            }
            .into_any();
        }
        let regenerate = regenerate.clone();
        let ask_release = ask_release.clone();
        (can_regenerate || can_release)
            .then(|| {
                view! {
                    <div class="held-action-row">
                        {can_release.then(|| view! {
                            <button type="button" class="button primary small" id=ids.release.clone()
                                disabled=move || ctx.any_busy()
                                on:click=ask_release>"Release to patient"</button>
                        })}
                        {can_regenerate.then(|| view! {
                            <button type="button" class="button small" id=ids.regenerate.clone()
                                disabled=move || ctx.any_busy()
                                on:click=regenerate>"Regenerate section"</button>
                        })}
                    </div>
                }
            })
            .into_any()
    };

    view! {
        <div class="held-actions">
            {blockers.map(|blockers| view! {
                <div class="release-blockers">
                    <p class="held-lead">"Fix these before releasing:"</p>
                    <ul class="reason-list">
                        {blockers.into_iter().map(|b| view! { <li>{b}</li> }).collect_view()}
                    </ul>
                </div>
            })}
            {(!can_regenerate).then(|| view! {
                <p class="held-hint">"There's no reviewed content for this procedure yet, so it can't be written again."</p>
            })}
            {actions}
            {move || error().map(|message| view! {
                <p class="form-error" role="alert">{message}</p>
            })}
        </div>
    }
}

#[component]
fn LockIcon() -> impl IntoView {
    view! {
        <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke-width="2" aria-hidden="true">
            <rect x="5" y="11" width="14" height="10" rx="2"></rect>
            <path d="M8 11V7a4 4 0 0 1 8 0v4"></path>
        </svg>
    }
}

#[component]
fn LockedStrip(fact: ProcedureFactView) -> impl IntoView {
    let tooth = tooth_label(&fact.tooth);
    let tooth = if tooth.is_empty() {
        view! { <span class="none">"None"</span> }.into_any()
    } else if fact.surfaces.is_empty() {
        view! { <b>{tooth}</b> }.into_any()
    } else {
        view! { <b>{tooth}</b> <span class="surfaces">{fact.surfaces.clone()}</span> }.into_any()
    };
    view! {
        <dl class="locked-strip" aria-label="Locked facts from the practice's records">
            <div class="locked-cell"><dt>"Tooth"</dt><dd>{tooth}</dd></div>
            <div class="locked-cell"><dt>"Code"</dt><dd class="mono">{fact.code}</dd></div>
            <div class="locked-cell"><dt>"Phase"</dt><dd><b>{fact.phase}</b></dd></div>
            <div class="locked-cell"><dt>"Patient est."</dt><dd><b>{money_view(fact.patient_estimate)}</b></dd></div>
            <div class="locked-source"><LockIcon />"Locked · from Open Dental"</div>
        </dl>
    }
}

fn source_pill(source: BlockSource) -> (&'static str, &'static str) {
    match source {
        BlockSource::Source => ("prov-pill neutral", "Locked · from Open Dental"),
        BlockSource::ClinicalNote => ("prov-pill neutral", "From clinical note"),
        BlockSource::Generated => ("prov-pill ready", "AI-written · checked"),
        BlockSource::StaffEdit => ("prov-pill changed", "Edited by staff"),
    }
}

#[component]
fn BlockCard(
    block: TextBlock,
    section_key: String,
    editable: bool,
    shown: bool,
    ctx: EditCtx,
) -> impl IntoView {
    let (pill_class, pill_label) = source_pill(block.source);
    let note_block = block.source == BlockSource::ClinicalNote;
    let confirm = note_block && block.needs_confirmation;
    let confirmed = note_block && !block.needs_confirmation;
    let edited = block.source == BlockSource::StaffEdit;
    let editable = editable && block.source != BlockSource::Source;
    let key: BlockKey = (section_key, block.field.clone());
    let card_id = dom_id("block", &key);
    let edit_id = dom_id("edit", &key);
    let area_id = dom_id("text", &key);
    let hint_id = dom_id("hint", &key);
    let heading = block.heading.clone();
    let text = block.sentences.join(" ");
    let original = block.sentences.join("\n");

    let is_editing = {
        let key = key.clone();
        Memo::new(move |_| ctx.editing.with(|e| e.as_ref() == Some(&key)))
    };
    let is_busy = {
        let key = key.clone();
        Memo::new(move |_| ctx.busy.with(|b| b.as_ref() == Some(&key)))
    };
    let any_busy = move || ctx.any_busy();
    let error = {
        let key = key.clone();
        move || {
            ctx.error.with(|e| {
                e.as_ref()
                    .filter(|(k, _)| *k == key)
                    .map(|(_, m)| m.clone())
            })
        }
    };

    let start_edit = {
        let key = key.clone();
        let area_id = area_id.clone();
        move |_| {
            if any_busy() {
                return;
            }
            ctx.draft.set(original.clone());
            ctx.error.set(None);
            ctx.editing.set(Some(key.clone()));
            focus_later(area_id.clone());
        }
    };
    let cancel = {
        let edit_id = edit_id.clone();
        move || {
            ctx.editing.set(None);
            ctx.error.set(None);
            focus_later(edit_id.clone());
        }
    };
    let save = {
        let key = key.clone();
        let heading = heading.clone();
        move || {
            let sentences = sentences_from(&ctx.draft.get_untracked());
            if sentences.is_empty() {
                ctx.error
                    .set(Some((key.clone(), "Write at least one sentence.".into())));
                return;
            }
            ctx.run(key.clone(), heading.clone(), BlockAction::Save(sentences));
        }
    };
    let revert = {
        let key = key.clone();
        let heading = heading.clone();
        move |_| ctx.run(key.clone(), heading.clone(), BlockAction::Revert)
    };
    let confirm_note = {
        let key = key.clone();
        let heading = heading.clone();
        move |_| ctx.run(key.clone(), heading.clone(), BlockAction::Confirm)
    };

    let keydown = {
        let cancel = cancel.clone();
        let save = save.clone();
        move |ev: KeyboardEvent| {
            if ev.key() == "Escape" {
                ev.prevent_default();
                cancel();
            } else if ev.key() == "Enter" && (ev.ctrl_key() || ev.meta_key()) {
                ev.prevent_default();
                save();
            }
        }
    };
    let editor = {
        let area_id = area_id.clone();
        let hint_id = hint_id.clone();
        let heading = heading.clone();
        move || {
            let cancel = cancel.clone();
            let save = save.clone();
            let keydown = keydown.clone();
            view! {
                <label class="visually-hidden" for=area_id.clone()>{format!("Text for {heading}, one sentence per line")}</label>
                <textarea
                    id=area_id.clone()
                    class="block-textarea"
                    rows=6
                    aria-describedby=hint_id.clone()
                    prop:value=ctx.draft.get_untracked()
                    on:input=move |ev| ctx.draft.set(event_target_value(&ev))
                    on:keydown=keydown
                ></textarea>
                <div id=hint_id.clone() class="block-edit-hints">
                    <p class="block-hint">"One sentence per line. Don't add names or other personal details."</p>
                    {move || ctx.demo.get().then(|| view! {
                        <p class="block-hint demo-note">{DEMO_NOTE}</p>
                    })}
                </div>
                <div class="block-edit-actions">
                    <button type="button" class="button primary small"
                        disabled=any_busy
                        on:click=move |_| save()>
                        {move || if is_busy.get() { "Saving…" } else { "Save" }}
                    </button>
                    <button type="button" class="button small"
                        disabled=any_busy
                        on:click=move |_| cancel()>"Cancel"</button>
                </div>
            }
        }
    };

    let reader = move || {
        let start_edit = start_edit.clone();
        let revert = revert.clone();
        let confirm_note = confirm_note.clone();
        let text = text.clone();
        let edit_id = edit_id.clone();
        let heading = heading.clone();
        view! {
            <p class="block-body">{text}</p>
            {confirm.then(|| view! {
                <div class="block-confirm">
                    <button type="button" class="button small"
                        disabled=any_busy
                        on:click=confirm_note>
                        {move || if is_busy.get() { "Confirming…" } else { "Looks right" }}
                    </button>
                    <span class="block-hint">"Patients see general wording until this is confirmed."</span>
                </div>
            })}
            {confirmed.then(|| view! {
                <p class="block-hint">
                    {if shown { "Confirmed. Patients will see this text." } else { "Confirmed." }}
                </p>
            })}
            {(editable || edited).then(|| view! {
                <div class="block-links">
                    {editable.then(|| view! {
                        <button type="button" class="link-button" id=edit_id.clone()
                            aria-label=format!("Edit {heading}")
                            disabled=any_busy
                            on:click=start_edit>"Edit"</button>
                    })}
                    {edited.then(|| view! {
                        <button type="button" class="link-button"
                            disabled=any_busy
                            on:click=revert>
                            {move || if is_busy.get() { "Restoring…" } else { "Revert to generated text" }}
                        </button>
                    })}
                </div>
            })}
        }
    };

    view! {
        <article id=card_id class="block-card" class:unconfirmed=confirm class:editing=is_editing tabindex="-1">
            <div class="block-head">
                <h3 class="block-title">{block.heading}</h3>
                <span class=pill_class>{pill_label}</span>
            </div>
            {move || if is_editing.get() { editor().into_any() } else { reader().into_any() }}
            {move || error().map(|message| view! {
                <p class="form-error" role="alert">{message}</p>
            })}
        </article>
    }
}

#[component]
fn CostsPane(p: PresentationView) -> impl IntoView {
    let mut rows = p.procedures.clone();
    rows.sort_by_key(|f| f.phase);
    view! {
        <div class="pane-head">
            <p class="eyebrow">"Built from the plan"</p>
            <h2 class="pane-title">"Costs"</h2>
        </div>
        <div class="card">
            <table class="plan-table costs-table">
                <caption class="visually-hidden">"Estimated costs by procedure"</caption>
                <thead>
                    <tr>
                        <th scope="col">"Phase"</th>
                        <th scope="col">"Tooth"</th>
                        <th scope="col">"Code"</th>
                        <th scope="col">"Procedure"</th>
                        <th scope="col" class="amount">"Fee"</th>
                        <th scope="col" class="amount">"Insurance est."</th>
                        <th scope="col" class="amount">"Patient est."</th>
                    </tr>
                </thead>
                <tbody>
                    {rows.into_iter().map(|f| {
                        let tooth = tooth_label(&f.tooth);
                        view! {
                            <tr>
                                <td class="num">{f.phase}</td>
                                <td>
                                    {if tooth.is_empty() {
                                        view! { <span class="none" aria-label="No tooth">"—"</span> }.into_any()
                                    } else {
                                        view! { <span class="tooth">{tooth}</span> }.into_any()
                                    }}
                                </td>
                                <td class="mono code">{f.code}</td>
                                <td class="desc">{f.name}</td>
                                <td class="amount">{money_view(f.fee)}</td>
                                <td class="amount">{money_view(f.insurance_estimate)}</td>
                                <td class="amount estimate">{money_view(f.patient_estimate)}</td>
                            </tr>
                        }
                    }).collect_view()}
                </tbody>
                <tfoot>
                    <tr>
                        <td></td>
                        <td></td>
                        <td></td>
                        <th scope="row">"Total"</th>
                        <td class="amount">{money_view(p.total_fee)}</td>
                        <td></td>
                        <td class="amount">{money_view(p.total_patient_estimate)}</td>
                    </tr>
                </tfoot>
            </table>
        </div>
        <p class="cost-caveat">{COST_CAVEAT}</p>
    }
}

fn check_row(text: &'static str) -> impl IntoView {
    view! {
        <li class="check-row"><span class="check-mark" aria-hidden="true">"✓"</span>{text}</li>
    }
}

fn checks_for(p: &PresentationView, key: &str) -> AnyView {
    if key == COSTS_KEY {
        return view! {
            <ul class="check-list">
                {check_row("Amounts come straight from the practice's records")}
                {check_row("No amount is written or changed by the writer")}
            </ul>
        }
        .into_any();
    }
    let Some(section) = p.sections.iter().find(|s| s.key == key) else {
        return ().into_any();
    };
    match section.visibility {
        SectionVisibility::Ready => view! {
            <ul class="check-list">
                {check_row("Tooth, code, phase and cost match the practice's records")}
                {check_row("No diagnosis or outcome promises")}
                {check_row("Passed a separate review pass")}
            </ul>
        }
        .into_any(),
        visibility => {
            let (class, lead) = if visibility == SectionVisibility::Failed {
                ("check-callout failed", "Failed. Never shown to patients.")
            } else {
                ("check-callout review", "Held for staff review.")
            };
            let reasons = if section.reasons.is_empty() {
                vec!["No reason was recorded.".to_owned()]
            } else {
                section.reasons.clone()
            };
            view! {
                <div class=class>
                    <span class="callout-mark" aria-hidden="true">"!"</span>
                    <div>
                        <p class="callout-lead">{lead}</p>
                        <ul class="reason-list">
                            {reasons.into_iter().map(|r| view! { <li>{r}</li> }).collect_view()}
                        </ul>
                    </div>
                </div>
            }
            .into_any()
        }
    }
}

fn level_name(level: ReadingLevel) -> &'static str {
    match level {
        ReadingLevel::Simple => "Simple",
        ReadingLevel::Standard => "Standard",
        ReadingLevel::Detailed => "Detailed",
    }
}

fn details(p: &PresentationView) -> AnyView {
    let seconds = format!("{:.1} s", p.generation_ms as f64 / 1000.0);
    // Name the language the text is in, which is what the patient sees. A
    // different requested language is noted after it.
    let content = language_name(&content_language(&p.language, &CONTENT_LANGUAGES));
    let requested = language_name(&p.language);
    let language = if content == requested {
        content
    } else {
        format!("{content} ({requested} requested)")
    };
    view! {
        <dl class="detail-list">
            <dt>"Language"</dt><dd>{language}</dd>
            <dt>"Reading level"</dt><dd>{level_name(p.reading_level)}</dd>
            {p.provider_display_name.clone().map(|d| view! { <dt>"Provider"</dt><dd>{d}</dd> })}
            <dt class="stacked">"Writer"</dt><dd class="mono" title=p.writer_model.clone()>{p.writer_model.clone()}</dd>
            <dt class="stacked">"Prompt"</dt><dd class="mono" title=p.prompt_version.clone()>{p.prompt_version.clone()}</dd>
            <dt>"Generation time"</dt><dd>{seconds}</dd>
        </dl>
    }
    .into_any()
}
