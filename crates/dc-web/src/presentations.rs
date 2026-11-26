//! Presentations list (`GET /api/staff/presentations`).

use crate::api;
use crate::format::timestamp;
use crate::motion::SkeletonTable;
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
    let rows = LocalResource::new(api::presentations);
    view! {
        <header class="topbar"><h1 class="page-title">"Presentations"</h1></header>
        <Suspense fallback=|| view! { <SkeletonTable label="Loading presentations" rows=6 cols=7 /> }>
            {move || rows.get().map(|result| match result {
                Ok(rows) if rows.is_empty() => view! {
                    <div class="notice empty-state reveal">
                        <p>"No presentations yet. Create one from a patient's treatment plan."</p>
                        <A href="/patients" attr:class="button">"Find a patient"</A>
                    </div>
                }.into_any(),
                Ok(rows) => view! { <PresentationTable rows /> }.into_any(),
                Err(failure) => view! {
                    <div class="notice error reveal" role="alert">"Could not load presentations. " {failure.message}</div>
                }.into_any(),
            })}
        </Suspense>
    }
}

#[component]
fn PresentationTable(rows: Vec<PresentationSummary>) -> impl IntoView {
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
                {rows.into_iter().map(|row| {
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
                }).collect_view()}
            </tbody>
        </table>
        </div>
    }
}
