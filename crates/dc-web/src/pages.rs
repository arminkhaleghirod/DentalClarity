//! Dashboard and placeholder pages.

use crate::api;
use crate::format::language_name;
use crate::motion::{SkeletonCards, SkeletonTable};
use crate::progress::presentation_href;
use crate::session::use_session;
use dc_core::api::{PatientRow, PresentationStatus};
use leptos::leptos_dom::helpers::{IntervalHandle, set_interval_with_handle};
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::A;
use std::time::Duration;

/// How often the dashboard re-reads rows while a presentation is generating.
const POLL_EVERY: Duration = Duration::from_secs(3);

/// What the dashboard body shows. Only a change of state rebuilds it; new
/// rows update in place.
#[derive(Clone, PartialEq)]
enum Load {
    Loading,
    Failed(String),
    Ready,
}

/// Dashboard data and the poller that refreshes it while any row is generating.
#[derive(Clone, Copy)]
struct DashState {
    rows: RwSignal<Vec<PatientRow>>,
    load: RwSignal<Load>,
    in_flight: StoredValue<bool>,
    poller: StoredValue<Option<IntervalHandle>>,
}

impl DashState {
    fn stop_polling(self) {
        if let Some(handle) = self.poller.try_update_value(Option::take).flatten() {
            handle.clear();
        }
    }

    fn fetch(self) {
        if self.in_flight.try_get_value() != Some(false) {
            return;
        }
        self.in_flight.set_value(true);
        spawn_local(async move {
            let result = api::dashboard().await;
            // The page may have been left while the request ran.
            if self.in_flight.try_set_value(false).is_some() {
                return;
            }
            match result {
                Ok(fresh) => {
                    let generating = fresh
                        .iter()
                        .any(|r| r.status == PresentationStatus::Generating);
                    if self.rows.with_untracked(|old| *old != fresh) {
                        self.rows.set(fresh);
                    }
                    if self.load.get_untracked() != Load::Ready {
                        self.load.set(Load::Ready);
                    }
                    if !generating {
                        self.stop_polling();
                    } else if self.poller.with_value(Option::is_none)
                        && let Ok(handle) =
                            set_interval_with_handle(move || self.fetch(), POLL_EVERY)
                    {
                        self.poller.set_value(Some(handle));
                    }
                }
                // A failed poll keeps the rows already shown.
                Err(failure) => {
                    if self.load.get_untracked() != Load::Ready {
                        self.load.set(Load::Failed(failure.message));
                    }
                }
            }
        });
    }
}

#[component]
pub fn Dashboard() -> impl IntoView {
    let state = DashState {
        rows: RwSignal::new(Vec::new()),
        load: RwSignal::new(Load::Loading),
        in_flight: StoredValue::new(false),
        poller: StoredValue::new(None),
    };
    state.fetch();
    on_cleanup(move || state.stop_polling());
    let DashState { rows, load, .. } = state;

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
    let card = move |class: &'static str, title: &'static str, status: PresentationStatus| {
        let text = move || {
            let n = rows.with(|r| r.iter().filter(|r| r.status == status).count());
            let noun = if n == 1 { "patient" } else { "patients" };
            format!("{n} {noun} today")
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
