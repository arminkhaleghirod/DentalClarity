//! Presentations list (`GET /api/staff/presentations`).

use crate::api;
use crate::format::timestamp;
use crate::motion::SkeletonTable;
use crate::poll::{Load, PolledRows};
use crate::progress::presentation_href;
use dc_core::presentation::{JobState, PresentationSummary};
use leptos::prelude::*;
use leptos_router::components::A;

pub fn state_pill(state: JobState) -> (&'static str, &'static str) {
    match state {
        JobState::Queued => ("pill not-started", "Queued"),
        JobState::Running => ("pill generating", "Generating"),
        JobState::Succeeded => ("pill ready", "Ready"),
        JobState::NeedsReview => ("pill review", "Needs review"),
        JobState::Failed => ("pill review", "Failed"),
    }
}

#[component]
pub fn PresentationsPage() -> impl IntoView {
    let PolledRows { rows, load, .. } =
        PolledRows::start(api::presentations, |row: &PresentationSummary| {
            !row.job_state.is_finished()
        });
    let empty = move || rows.with(Vec::is_empty);
    view! {
        <header class="topbar"><h1 class="page-title">"Presentations"</h1></header>
        {move || match load.get() {
            Load::Loading => view! { <SkeletonTable label="Loading presentations" rows=6 cols=7 /> }.into_any(),
            Load::Failed(message) => view! {
                <div class="notice error reveal" role="alert">"Could not load presentations. " {message}</div>
            }.into_any(),
            Load::Ready => view! {
                <Show when=move || !empty() fallback=|| view! {
                    <div class="notice empty-state reveal">
                        <p>"No presentations yet. Create one from a patient's treatment plan."</p>
                        <A href="/patients" attr:class="button">"Find a patient"</A>
                    </div>
                }>
                    <PresentationTable rows />
                </Show>
            }.into_any(),
        }}
    }
}

#[component]
fn PresentationTable(rows: RwSignal<Vec<PresentationSummary>>) -> impl IntoView {
    view! {
        <div class="table-wrap reveal">
        <table class="table">
            <thead>
                <tr>
                    <th scope="col">"Patient"</th>
                    <th scope="col">"Plan"</th>
                    <th scope="col">"Version"</th>
                    <th scope="col">"Created"</th>
                    <th scope="col">"Status"</th>
                    <th scope="col">"Sections ready"</th>
                    <th scope="col"><span class="visually-hidden">"Action"</span></th>
                </tr>
            </thead>
            <tbody>
                // Keyed on the whole row, so only rows whose job moved on are
                // rebuilt when a poll returns.
                <For
                    each=move || rows.get()
                    key=|row| format!("{row:?}")
                    children=|row| view! { <PresentationRow row /> }
                />
            </tbody>
        </table>
        </div>
    }
}

#[component]
fn PresentationRow(row: PresentationSummary) -> impl IntoView {
    let href = presentation_href(&row.id);
    let (pill_class, pill_label) = state_pill(row.job_state);
    let label = format!("Open presentation for {}", row.patient_display_name);
    view! {
        <tr>
            <td class="name"><A href=href.clone()>{row.patient_display_name.clone()}</A></td>
            <td>{row.plan_name}</td>
            <td class="num">{format!("v{}", row.version)}</td>
            <td class="num">{timestamp(&row.created_at)}</td>
            <td><span class=pill_class>{pill_label}</span></td>
            <td class="num">{format!("{} of {}", row.sections_ready, row.sections_total)}</td>
            <td class="action"><A href=href attr:aria-label=label>"Open"</A></td>
        </tr>
    }
}
