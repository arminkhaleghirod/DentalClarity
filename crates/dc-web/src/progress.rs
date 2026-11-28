//! The "Generating presentation" card, polled from `GET /api/staff/jobs/:id`.

use crate::api;
use crate::format::elapsed;
use crate::session::use_session;
use dc_core::Mode;
use dc_core::presentation::{JobState, JobStatus, SectionProgress, SectionStage};
use leptos::leptos_dom::helpers::{IntervalHandle, set_interval_with_handle};
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::A;
use std::cell::Cell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Path of the presentation preview.
pub fn presentation_href(id: &str) -> String {
    format!("/presentations/{}", api::encode(id))
}

pub(crate) fn stage_text(stage: SectionStage) -> &'static str {
    match stage {
        SectionStage::Queued => "Queued",
        SectionStage::Writing => "Writing",
        SectionStage::Reviewing => "Reviewing",
        SectionStage::Repairing => "Repairing",
        SectionStage::Passed => "Passed",
        SectionStage::NeedsReview => "Needs review",
        SectionStage::Failed => "Failed",
    }
}

fn stage_class(stage: SectionStage) -> &'static str {
    match stage {
        SectionStage::Queued => "queued",
        SectionStage::Writing | SectionStage::Reviewing | SectionStage::Repairing => "active",
        SectionStage::Passed => "passed",
        SectionStage::NeedsReview => "held",
        SectionStage::Failed => "failed",
    }
}

fn is_settled(stage: SectionStage) -> bool {
    matches!(
        stage,
        SectionStage::Passed | SectionStage::NeedsReview | SectionStage::Failed
    )
}

/// Share of the work done, 0 to 100. Sections in progress count as half done.
fn percent(status: &JobStatus) -> u32 {
    if status.state.is_finished() {
        return 100;
    }
    let total = status.sections.len();
    if total == 0 {
        return 0;
    }
    let units: usize = status
        .sections
        .iter()
        .map(|s| match stage_class(s.stage) {
            "queued" => 0,
            "active" => 1,
            _ => 2,
        })
        .sum();
    (units * 100 / (total * 2)) as u32
}

/// The line shown while a job waits for a slot (DECISIONS E37).
fn queue_note(ahead: Option<u32>) -> String {
    match ahead {
        None | Some(0) => "Queued. Writing starts in a moment.".into(),
        Some(n) => format!("Queued, {n} ahead. Writing starts when the server is free."),
    }
}

fn held_count(status: &JobStatus) -> usize {
    status
        .sections
        .iter()
        .filter(|s| matches!(s.stage, SectionStage::NeedsReview | SectionStage::Failed))
        .count()
}

fn title(status: Option<&JobStatus>) -> String {
    let Some(status) = status else {
        return "Generating presentation".into();
    };
    match status.state {
        JobState::Queued | JobState::Running => "Generating presentation".into(),
        JobState::Succeeded => "Presentation ready".into(),
        JobState::NeedsReview => match held_count(status) {
            1 => "Ready with 1 section held".into(),
            n => format!("Ready with {n} sections held"),
        },
        JobState::Failed => "Generation failed".into(),
    }
}

/// Polls the job every second until it finishes, then calls `on_finished`.
///
/// With `show_open`, a finished job shows an "Open presentation" button. A
/// failed job shows "Try again" when `on_retry` is given.
#[component]
pub fn GenerationProgress(
    job_id: String,
    presentation_id: String,
    #[prop(optional)] show_open: bool,
    #[prop(optional)] on_finished: Option<Callback<JobStatus>>,
    #[prop(optional)] on_retry: Option<Callback<()>>,
) -> impl IntoView {
    let session = use_session();
    let demo = move || session.info.get().and_then(Result::ok).map(|i| i.mode) == Some(Mode::Demo);
    let status = RwSignal::new(None::<JobStatus>);
    let poll_error = RwSignal::new(None::<String>);

    let alive = Arc::new(AtomicBool::new(true));
    let handle: Arc<Mutex<Option<IntervalHandle>>> = Arc::new(Mutex::new(None));
    let in_flight = Rc::new(Cell::new(false));

    let stop = {
        let handle = handle.clone();
        move || {
            if let Some(h) = handle.lock().ok().and_then(|mut h| h.take()) {
                h.clear();
            }
        }
    };

    let poll = {
        let alive = alive.clone();
        let stop = stop.clone();
        move || {
            if in_flight.get() || !alive.load(Ordering::Relaxed) {
                return;
            }
            in_flight.set(true);
            let in_flight = in_flight.clone();
            let alive = alive.clone();
            let stop = stop.clone();
            let id = job_id.clone();
            spawn_local(async move {
                let result = api::job(id).await;
                in_flight.set(false);
                if !alive.load(Ordering::Relaxed) {
                    return;
                }
                match result {
                    Ok(job) => {
                        let finished = job.state.is_finished();
                        poll_error.try_set(None);
                        status.try_set(Some(job.clone()));
                        if finished {
                            stop();
                            alive.store(false, Ordering::Relaxed);
                            if let Some(cb) = on_finished {
                                cb.run(job);
                            }
                        }
                    }
                    Err(failure) if failure.status == 404 => {
                        stop();
                        poll_error.try_set(Some("This generation job no longer exists.".into()));
                    }
                    Err(failure) => {
                        poll_error.try_set(Some(format!(
                            "Lost contact with the server. Retrying. {}",
                            failure.message
                        )));
                    }
                }
            });
        }
    };

    poll();
    if let Ok(h) = set_interval_with_handle(poll, Duration::from_secs(1))
        && let Ok(mut slot) = handle.lock()
    {
        *slot = Some(h);
    }
    {
        let alive = alive.clone();
        on_cleanup(move || {
            alive.store(false, Ordering::Relaxed);
            stop();
        });
    }

    let state = move || status.with(|s| s.as_ref().map(|s| s.state));
    let finished = move || state().is_some_and(JobState::is_finished);
    let pct = move || status.with(|s| s.as_ref().map(percent).unwrap_or(0));
    let announce = move || match state() {
        Some(JobState::Succeeded) => "Presentation ready.".to_owned(),
        Some(JobState::NeedsReview) => format!("{}.", status.with(|s| title(s.as_ref()))),
        Some(JobState::Failed) => "Generation failed.".to_owned(),
        _ => String::new(),
    };
    let open_href = presentation_href(&presentation_id);

    view! {
        <section class="card progress-card" aria-labelledby="progress-title">
            <div class="progress-head">
                <h2 id="progress-title" class="progress-title">{move || status.with(|s| title(s.as_ref()))}</h2>
                <span class="progress-time" aria-label="Elapsed time">
                    {move || status.with(|s| elapsed(s.as_ref().map(|s| s.elapsed_ms).unwrap_or(0)))}
                </span>
            </div>
            <div
                class="progress-bar"
                role="progressbar"
                aria-label="Generation progress"
                aria-valuemin="0"
                aria-valuemax="100"
                aria-valuenow=move || pct().to_string()
            >
                <div class="progress-fill" class:failed=move || state() == Some(JobState::Failed)
                    style:width=move || format!("{}%", pct())></div>
            </div>
            <p class="visually-hidden" role="status">{announce}</p>
            {move || match state() {
                None => Some(view! { <p class="progress-note">"Starting…"</p> }.into_any()),
                Some(JobState::Queued) => Some(view! {
                    <p class="progress-note">
                        {move || queue_note(status.with(|s| s.as_ref().and_then(|s| s.queue_ahead)))}
                    </p>
                }.into_any()),
                _ => None,
            }}
            <ol class="stage-list" aria-live="polite" aria-label="Sections">
                <For
                    each=move || status.with(|s| s.as_ref().map(|s| s.sections.clone()).unwrap_or_default())
                    key=|s| (s.key.clone(), stage_text(s.stage))
                    children=|section| view! { <StageRow section /> }
                />
            </ol>
            {move || status.with(|s| s.as_ref().and_then(|s| s.error.clone())).map(|message| view! {
                <p class="form-error" role="alert">{message}</p>
            })}
            {move || poll_error.get().map(|message| view! { <p class="form-error">{message}</p> })}
            {
                let open_href = open_href.clone();
                move || {
                    let open = (show_open && finished() && state() != Some(JobState::Failed)).then(|| view! {
                        <A href=open_href.clone() attr:class="button primary">"Open presentation"</A>
                    });
                    let retry = on_retry.filter(|_| state() == Some(JobState::Failed)).map(|cb| view! {
                        <button type="button" class="button primary" on:click=move |_| cb.run(())>"Try again"</button>
                    });
                    (open.is_some() || retry.is_some()).then(|| view! {
                        <div class="card-actions">{open}{retry}</div>
                    })
                }
            }
            <p class="progress-foot">
                {move || if demo() {
                    "You can close this tab. Generation keeps running on the demo server."
                } else {
                    "You can close this tab. Generation keeps running on the practice server."
                }}
            </p>
        </section>
    }
}

#[component]
fn StageRow(section: SectionProgress) -> impl IntoView {
    let class = stage_class(section.stage);
    let mark = match section.stage {
        SectionStage::Passed => "✓",
        SectionStage::NeedsReview => "!",
        SectionStage::Failed => "×",
        _ => "",
    };
    let settled = is_settled(section.stage);
    let text = if settled {
        stage_text(section.stage).to_owned()
    } else if section.stage == SectionStage::Queued {
        "Queued".to_owned()
    } else {
        format!("{}…", stage_text(section.stage))
    };
    view! {
        <li class=format!("stage-row {class}")>
            <span class="stage-icon" aria-hidden="true">
                {(class == "active").then(|| view! { <span class="stage-dot"></span> })}
                {mark}
            </span>
            <span class="stage-label">{section.label}</span>
            <span class="stage-state">{text}</span>
        </li>
    }
}
