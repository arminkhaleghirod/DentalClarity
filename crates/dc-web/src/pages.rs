use crate::api;
use crate::format::language_name;
use dc_core::api::{PatientRow, PresentationStatus};
use leptos::prelude::*;

#[component]
pub fn Dashboard() -> impl IntoView {
    let rows = LocalResource::new(api::dashboard);

    view! {
        <header class="topbar">
            <h1 class="page-title">"Today"</h1>
            <span class="sync-pill">"Synthetic practice connected"</span>
        </header>
        <Suspense fallback=|| view! { <div class="notice">"Loading today's patients…"</div> }>
            {move || rows.get().map(|result| match result {
                Ok(rows) => view! { <StatusCards rows=rows.clone() /> <PatientTable rows=rows /> }.into_any(),
                Err(message) => view! {
                    <div class="notice error" role="alert">
                        "Could not load today's patients. " {message}
                    </div>
                }.into_any(),
            })}
        </Suspense>
    }
}

#[component]
fn StatusCards(rows: Vec<PatientRow>) -> impl IntoView {
    let count = |status: PresentationStatus| rows.iter().filter(|r| r.status == status).count();
    let card = |class: &'static str, title: &'static str, n: usize| {
        let noun = if n == 1 { "patient" } else { "patients" };
        view! {
            <div class=format!("status-card {class}")>
                <h2>{title}</h2>
                <p>{format!("{n} {noun} today")}</p>
            </div>
        }
    };
    view! {
        <section class="status-cards" aria-label="Presentation status">
            {card("ready", "Ready to present", count(PresentationStatus::Ready))}
            {card("generating", "Generating", count(PresentationStatus::Generating))}
            {card("changed", "Plan changed", count(PresentationStatus::PlanChanged))}
            {card("review", "Needs review", count(PresentationStatus::NeedsReview))}
        </section>
    }
}

#[component]
fn PatientTable(rows: Vec<PatientRow>) -> impl IntoView {
    let empty = rows.is_empty();
    view! {
        <div class="section-head">
            <div>
                <h2>"Today's patients"</h2>
                <p>"Appointments today with an open treatment plan"</p>
            </div>
        </div>
        <Show when=move || !empty fallback=|| view! {
            <div class="notice">"No appointments with open treatment plans today."</div>
        }>
            <table class="table">
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
                    {rows.clone().into_iter().map(|row| view! { <PatientRowView row /> }).collect_view()}
                </tbody>
            </table>
        </Show>
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
    view! {
        <tr>
            <td class="num">{row.appointment_time.clone().unwrap_or_default()}</td>
            <td class="name">{row.display_name.clone()}</td>
            <td class="num">{row.age_years.map(|a| a.to_string()).unwrap_or_default()}</td>
            <td>{row.plan_summary.clone()}</td>
            <td class="num">{row.procedure_count}</td>
            <td>{language_name(&row.language)}</td>
            <td><span class=format!("pill {pill_class}")>{pill_label}</span></td>
            <td class="action">{action}</td>
        </tr>
    }
}

#[component]
pub fn Upcoming(title: &'static str, milestone: &'static str) -> impl IntoView {
    view! {
        <header class="topbar"><h1 class="page-title">{title}</h1></header>
        <div class="notice">{format!("This screen is scheduled for milestone {milestone}. See docs/IMPLEMENTATION.md.")}</div>
    }
}

#[component]
pub fn NotFound() -> impl IntoView {
    view! {
        <header class="topbar"><h1 class="page-title">"Page not found"</h1></header>
        <div class="notice">"That page does not exist."</div>
    }
}
