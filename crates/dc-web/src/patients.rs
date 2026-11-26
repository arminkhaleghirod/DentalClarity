//! Patient directory and the patient page with the plan table.

use crate::api;
use crate::format::{language_name, money_view};
use dc_core::api::{PatientDetail, PatientListItem, PlanDetail, ProcedureLine};
use leptos::html;
use leptos::leptos_dom::helpers::debounce;
use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::use_params_map;
use std::time::Duration;

/// Path of the patient page.
pub fn patient_href(key: &str) -> String {
    format!("/patients/{}", api::encode(key))
}

#[component]
pub fn PatientsPage() -> impl IntoView {
    let query = RwSignal::new(String::new());
    let on_input = debounce(Duration::from_millis(250), move |ev: leptos::ev::Event| {
        query.set(event_target_value(&ev));
    });
    let results = LocalResource::new(move || api::patients(query.get()));
    let input = NodeRef::<html::Input>::new();
    Effect::new(move || {
        if let Some(el) = input.get() {
            let _ = el.focus();
        }
    });

    view! {
        <header class="topbar"><h1 class="page-title">"Patients"</h1></header>
        <div class="search-field">
            <label for="patient-search" class="visually-hidden">"Search patients"</label>
            <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke-width="1.8"
                stroke-linecap="round" aria-hidden="true">
                <path d="M11 4a7 7 0 1 0 0 14a7 7 0 1 0 0-14M20 20l-4-4" />
            </svg>
            <input
                id="patient-search"
                type="search"
                node_ref=input
                maxlength="100"
                autocomplete="off"
                placeholder="Search by name or patient number"
                on:input=on_input
            />
        </div>
        <Suspense fallback=|| view! { <div class="notice">"Loading patients…"</div> }>
            {move || results.get().map(|result| match result {
                Ok(rows) if rows.is_empty() => view! {
                    <div class="notice">{format!("No patients match \"{}\".", query.get_untracked().trim())}</div>
                }.into_any(),
                Ok(rows) => view! { <DirectoryTable rows /> }.into_any(),
                Err(failure) => view! {
                    <div class="notice error" role="alert">"Could not load patients. " {failure.message}</div>
                }.into_any(),
            })}
        </Suspense>
    }
}

#[component]
fn DirectoryTable(rows: Vec<PatientListItem>) -> impl IntoView {
    view! {
        <table class="table directory">
            <thead>
                <tr>
                    <th scope="col">"Patient"</th>
                    <th scope="col">"Age"</th>
                    <th scope="col">"Patient #"</th>
                    <th scope="col">"Language"</th>
                    <th scope="col">"Appointment"</th>
                    <th scope="col">"Open plans"</th>
                    <th scope="col"><span class="visually-hidden">"Action"</span></th>
                </tr>
            </thead>
            <tbody>
                {rows.into_iter().map(|row| {
                    let href = patient_href(&row.patient_key);
                    let label = format!("Open {}", row.display_name);
                    view! {
                        <tr>
                            <td class="name"><A href=href.clone()>{row.display_name.clone()}</A></td>
                            <td class="num">{row.age_years.map(|a| a.to_string()).unwrap_or_default()}</td>
                            <td class="num">{row.patient_key.clone()}</td>
                            <td>{row.language.as_deref().map(language_name).unwrap_or_default()}</td>
                            <td class="num">{row.appointment_time.clone().unwrap_or_else(|| "None today".into())}</td>
                            <td class="num">{row.open_plans}</td>
                            <td class="action"><A href=href attr:aria-label=label>"Open"</A></td>
                        </tr>
                    }
                }).collect_view()}
            </tbody>
        </table>
    }
}

#[component]
pub fn PatientPage() -> impl IntoView {
    let params = use_params_map();
    let key = move || params.read().get("key").unwrap_or_default();
    let detail = LocalResource::new(move || api::patient(key()));

    view! {
        <Suspense fallback=|| view! { <div class="notice page-gap">"Loading patient…"</div> }>
            {move || detail.get().map(|result| match result {
                Ok(patient) => view! { <PatientView patient /> }.into_any(),
                Err(failure) => view! {
                    <header class="topbar"><Breadcrumb name=None /></header>
                    <div class="notice error" role="alert">{failure.message}</div>
                }.into_any(),
            })}
        </Suspense>
    }
}

#[component]
pub fn Breadcrumb(name: Option<String>) -> impl IntoView {
    view! {
        <nav class="breadcrumb" aria-label="Breadcrumb">
            <A href="/patients">"Patients"</A>
            {name.map(|n| view! {
                <span class="crumb-sep" aria-hidden="true">"›"</span>
                <span aria-current="page">{n}</span>
            })}
        </nav>
    }
}

fn initials(first: &str, last: &str) -> String {
    first
        .chars()
        .next()
        .into_iter()
        .chain(last.chars().next())
        .flat_map(char::to_uppercase)
        .collect()
}

/// Avatar, name and meta line shared by the patient page and the create
/// screen. `children` go on the right, for the plan picker or chip.
#[component]
pub fn PatientHead(patient: PatientDetail, children: Children) -> impl IntoView {
    let name = format!("{} {}", patient.first_name, patient.last_name);
    let mut meta = Vec::new();
    if let Some(age) = patient.age_years {
        meta.push(age.to_string());
    }
    meta.push(format!("Patient #{}", patient.patient_key));
    if let Some(lang) = &patient.language {
        meta.push(format!("Prefers {}", language_name(lang)));
    }
    view! {
        <div class="patient-head">
            <div class="patient-id">
                <div class="avatar" aria-hidden="true">{initials(&patient.first_name, &patient.last_name)}</div>
                <div>
                    <h1 id="patient-name" class="patient-name">{name}</h1>
                    <p class="patient-meta">{meta.join(" · ")}</p>
                </div>
            </div>
            {children()}
        </div>
    }
}

#[component]
fn PatientView(patient: PatientDetail) -> impl IntoView {
    let name = format!("{} {}", patient.first_name, patient.last_name);
    let selected = RwSignal::new(0usize);
    let plans = StoredValue::new(patient.plans.clone());
    let plan_count = patient.plans.len();
    let patient_key = patient.patient_key.clone();

    view! {
        <header class="topbar"><Breadcrumb name=Some(name) /></header>
        <section class="card patient-card" aria-labelledby="patient-name">
            <PatientHead patient>
                <PlanPicker plans selected count=plan_count />
            </PatientHead>
            {move || {
                let all = plans.get_value();
                match all.get(selected.get()).cloned() {
                    Some(plan) => {
                        let create_href = format!(
                            "/patients/{}/plans/{}/new",
                            api::encode(&patient_key),
                            api::encode(&plan.plan_key)
                        );
                        let has_unknown = plan_has_unknown(&plan);
                        view! {
                            <PlanTable plan />
                            <div class="card-foot">
                                <PlanNotes has_unknown />
                                <div class="card-actions">
                                    <A href=create_href attr:class="button primary">"Create presentation"</A>
                                </div>
                            </div>
                        }.into_any()
                    }
                    None => view! {
                        <div class="card-foot">
                            <p class="notice">"This patient has no open treatment plan."</p>
                        </div>
                    }.into_any(),
                }
            }}
        </section>
    }
}

pub fn plan_label(plan: &PlanDetail) -> String {
    match &plan.provider_display_name {
        Some(provider) => format!("{} · {provider}", plan.name),
        None => plan.name.clone(),
    }
}

/// The plan name on its own, for screens that show a single plan.
#[component]
pub fn PlanChip(plan: PlanDetail) -> impl IntoView {
    view! {
        <div class="plan-chip"><span class="muted">"Plan"</span><b>{plan_label(&plan)}</b></div>
    }
}

#[component]
fn PlanPicker(
    plans: StoredValue<Vec<PlanDetail>>,
    selected: RwSignal<usize>,
    count: usize,
) -> impl IntoView {
    if count <= 1 {
        let label = plans
            .get_value()
            .first()
            .map(plan_label)
            .unwrap_or_default();
        return view! {
            <div class="plan-chip"><span class="muted">"Plan"</span><b>{label}</b></div>
        }
        .into_any();
    }
    view! {
        <label class="plan-select">
            <span class="muted">"Plan"</span>
            <select on:change=move |ev| {
                if let Ok(i) = event_target_value(&ev).parse() {
                    selected.set(i);
                }
            }>
                {plans.get_value().iter().enumerate().map(|(i, plan)| view! {
                    <option value=i.to_string() selected=move || selected.get() == i>{plan_label(plan)}</option>
                }).collect_view()}
            </select>
        </label>
    }
    .into_any()
}

/// True when some fee or patient estimate is missing, so totals are incomplete.
pub fn plan_has_unknown(plan: &PlanDetail) -> bool {
    plan.procedures
        .iter()
        .any(|p| p.fee.is_none() || p.patient_estimate.is_none())
}

/// The plan's procedures with totals, read-only from the practice's records.
#[component]
pub fn PlanTable(plan: PlanDetail) -> impl IntoView {
    view! {
        <table class="plan-table">
            <caption class="visually-hidden">{format!("Treatment plan: {}", plan_label(&plan))}</caption>
            <thead>
                <tr>
                    <th scope="col">"Phase"</th>
                    <th scope="col">"Tooth"</th>
                    <th scope="col">"Code"</th>
                    <th scope="col">"Description"</th>
                    <th scope="col" class="amount">"Fee"</th>
                    <th scope="col" class="amount">"Pt. est."</th>
                </tr>
            </thead>
            <tbody>
                {plan.procedures.into_iter().map(|p| view! { <ProcedureRowView p /> }).collect_view()}
            </tbody>
            <tfoot>
                <tr>
                    <td></td>
                    <td></td>
                    <td></td>
                    <th scope="row">"Total"</th>
                    <td class="amount">{money_view(plan.total_fee)}</td>
                    <td class="amount">{money_view(plan.total_patient_estimate)}</td>
                </tr>
            </tfoot>
        </table>
    }
}

/// Notes under the plan table.
#[component]
pub fn PlanNotes(has_unknown: bool) -> impl IntoView {
    view! {
        <Show when=move || has_unknown>
            <p class="foot-note">"Some amounts are not available in the practice's records yet, so the totals are incomplete."</p>
        </Show>
        <p class="foot-note">"Read-only from the practice's records. DentalClarity never changes these values."</p>
    }
}

#[component]
fn ProcedureRowView(p: ProcedureLine) -> impl IntoView {
    let tooth = if p.tooth.is_empty() {
        view! { <span class="none" aria-label="No tooth">"—"</span> }.into_any()
    } else {
        view! {
            <span class="tooth">{p.tooth.clone()}</span>
            {(!p.surfaces.is_empty()).then(|| view! { <span class="surfaces">{p.surfaces.clone()}</span> })}
        }
        .into_any()
    };
    let description = match p.name {
        Some(name) => view! {
            <span class="our-name">{name}</span>
            <span class="pms-desc">{p.pms_description}</span>
        }
        .into_any(),
        None => view! { <span class="our-name">{p.pms_description}</span> }.into_any(),
    };
    view! {
        <tr>
            <td class="num">{p.phase.map(|n| n.to_string()).unwrap_or_default()}</td>
            <td>{tooth}</td>
            <td class="mono code">{p.code}</td>
            <td class="desc">{description}</td>
            <td class="amount">{money_view(p.fee)}</td>
            <td class="amount estimate">{money_view(p.patient_estimate)}</td>
        </tr>
    }
}
