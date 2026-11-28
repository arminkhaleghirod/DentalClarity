//! Dashboard and the not-found page.

use crate::api;
use crate::format::language_name;
use crate::motion::{SkeletonCards, SkeletonTable};
use crate::poll::{Load, PolledRows};
use crate::progress::presentation_href;
use crate::session::use_session;
use dc_core::api::{PatientRow, PresentationStatus};
use leptos::prelude::*;
use leptos_router::components::A;

#[component]
pub fn Dashboard() -> impl IntoView {
    let PolledRows { rows, load, .. } = PolledRows::start(api::dashboard, |row: &PatientRow| {
        row.status == PresentationStatus::Generating
    });

    let session = use_session();
    let practice = move || {
        session
            .info
            .get()
            .and_then(Result::ok)
            .map(|i| i.practice_name)
            .unwrap_or_default()
    };
    let greeting = match js_sys::Date::new_0().get_hours() {
        0..=11 => "Good morning",
        12..=17 => "Good afternoon",
        _ => "Good evening",
    };

    view! {
        <header class="topbar">
            <h1 class="page-title">{greeting}", "{practice}</h1>
            <span class="sync-pill">"Practice records connected"</span>
        </header>
        {move || match load.get() {
            Load::Loading => view! {
                <SkeletonCards />
                <div class="section-head"><div><h2>"Today's patients"</h2><p>"Appointments today with an open treatment plan"</p></div></div>
                <SkeletonTable label="Loading today's patients" rows=6 cols=8 />
            }.into_any(),
            Load::Failed(message) => view! {
                <div class="notice error reveal" role="alert">
                    "Could not load today's patients. " {message}
                </div>
            }.into_any(),
            Load::Ready => view! { <StatusCards rows /> <PatientTable rows /> }.into_any(),
        }}
    }
}

#[component]
fn StatusCards(rows: RwSignal<Vec<PatientRow>>) -> impl IntoView {
    // Each card counts the rows of today's table whose presentation is in
    // that state, so "0 of 7" reads against the seven appointments listed.
    let card = move |class: &'static str, title: &'static str, status: PresentationStatus| {
        let text = move || {
            rows.with(|r| {
                let n = r.iter().filter(|r| r.status == status).count();
                status_count_text(n, r.len())
            })
        };
        view! {
            <div class=format!("status-card {class}")>
                <h2>{title}</h2>
                <p class="status-count">{text}</p>
            </div>
        }
    };
    view! {
        <section class="status-cards reveal" aria-label="Presentation status">
            {card("ready", "Ready to present", PresentationStatus::Ready)}
            {card("generating", "Generating", PresentationStatus::Generating)}
            {card("changed", "Plan changed", PresentationStatus::PlanChanged)}
            {card("review", "Needs review", PresentationStatus::NeedsReview)}
        </section>
    }
}

/// "2 of 7 patients today": a status count against today's appointments.
fn status_count_text(n: usize, total: usize) -> String {
    let noun = if total == 1 { "patient" } else { "patients" };
    format!("{n} of {total} {noun} today")
}

#[component]
fn PatientTable(rows: RwSignal<Vec<PatientRow>>) -> impl IntoView {
    let empty = move || rows.with(Vec::is_empty);
    view! {
        <div class="section-head">
            <div>
                <h2>"Today's patients"</h2>
                <p>"Appointments today with an open treatment plan"</p>
            </div>
        </div>
        <Show when=move || !empty() fallback=|| view! {
            <div class="notice reveal">"No appointments with open treatment plans today."</div>
        }>
            <div class="table-wrap reveal">
                <table class="table dashboard-table">
                    <thead>
                        <tr>
                            <th scope="col">"Time"</th>
                            <th scope="col">"Patient"</th>
                            <th scope="col">"Age"</th>
                            <th scope="col">"Treatment plan"</th>
                            <th scope="col">"Procedures"</th>
                            <th scope="col">"Language"</th>
                            <th scope="col">"Presentation"</th>
                            <th scope="col"><span class="visually-hidden">"Action"</span></th>
                        </tr>
                    </thead>
                    <tbody>
                        <For
                            each=move || rows.get()
                            key=|row| format!("{row:?}")
                            children=|row| view! { <PatientRowView row /> }
                        />
                    </tbody>
                </table>
            </div>
        </Show>
    }
}

/// Where a row's action leads: the latest presentation when there is one,
/// otherwise the patient page to create one.
fn row_href(row: &PatientRow) -> String {
    match &row.presentation_id {
        Some(id) => presentation_href(id),
        None => crate::patients::patient_href(&row.patient_key),
    }
}

#[component]
fn PatientRowView(row: PatientRow) -> impl IntoView {
    let (pill_class, pill_label, action) = match row.status {
        PresentationStatus::NotStarted => ("not-started", "Not started", "Create"),
        PresentationStatus::Generating => ("generating", "Generating", "View"),
        PresentationStatus::Ready => ("ready", "Ready", "Open"),
        PresentationStatus::PlanChanged => ("changed", "Plan changed", "Review"),
        PresentationStatus::NeedsReview => ("review", "Needs review", "Review"),
    };
    let href = row_href(&row);
    let label = format!("{action}: {}", row.display_name);
    view! {
        <tr>
            <td class="num">{row.appointment_time.clone().unwrap_or_default()}</td>
            <td class="name">{row.display_name.clone()}</td>
            <td class="num">{row.age_years.map(|a| a.to_string()).unwrap_or_default()}</td>
            <td>{row.plan_summary.clone()}</td>
            <td class="num">{row.procedure_count}</td>
            <td>{language_name(&row.language)}</td>
            <td><span class=format!("pill {pill_class}")>{pill_label}</span></td>
            <td class="action"><A href=href attr:aria-label=label>{action}</A></td>
        </tr>
    }
}

#[component]
pub fn NotFound() -> impl IntoView {
    view! {
        <header class="topbar"><h1 class="page-title">"Page not found"</h1></header>
        <div class="notice">"That page does not exist."</div>
    }
}

#[cfg(test)]
mod tests {
    use super::status_count_text;

    #[test]
    fn status_counts_read_against_todays_list() {
        assert_eq!(status_count_text(0, 7), "0 of 7 patients today");
        assert_eq!(status_count_text(1, 1), "1 of 1 patient today");
    }
}
