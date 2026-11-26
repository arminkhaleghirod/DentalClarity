//! Create presentation screen (mockup 2b): plan on the left, settings and
//! generation progress on the right.

use crate::api;
use crate::format::{PRESENTATION_LANGUAGES, default_presentation_language};
use crate::patients::{
    PatientHead, PlanChip, PlanNotes, PlanTable, patient_href, plan_has_unknown,
};
use crate::progress::GenerationProgress;
use dc_core::api::{PatientDetail, PlanDetail};
use dc_core::content::ReadingLevel;
use dc_core::presentation::{CreatePresentationRequest, CreatePresentationResponse};
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::A;
use leptos_router::hooks::use_params_map;

/// Error codes after which retrying from this screen cannot help.
const BLOCKING_CODES: [&str; 2] = ["budget_reached", "provider_not_configured"];

#[component]
pub fn CreatePresentationPage() -> impl IntoView {
    let params = use_params_map();
    let key = move || params.read().get("key").unwrap_or_default();
    let plan_key = move || params.read().get("plan").unwrap_or_default();
    let detail = LocalResource::new(move || api::patient(key()));

    view! {
        <div class="subpage">
            <Suspense fallback=|| view! {
                <SubpageBar name=None patient_key=String::new() />
                <div class="subpage-body"><div class="notice">"Loading plan…"</div></div>
            }>
                {move || detail.get().map(|result| match result {
                    Ok(patient) => {
                        let wanted = plan_key();
                        match patient.plans.iter().find(|p| p.plan_key == wanted).cloned() {
                            Some(plan) => view! { <CreateView patient plan /> }.into_any(),
                            None => {
                                let name = format!("{} {}", patient.first_name, patient.last_name);
                                view! {
                                    <SubpageBar name=Some(name) patient_key=patient.patient_key.clone() />
                                    <div class="subpage-body">
                                        <div class="notice error" role="alert">
                                            "This treatment plan is not open in the practice's records."
                                        </div>
                                    </div>
                                }.into_any()
                            }
                        }
                    }
                    Err(failure) => view! {
                        <SubpageBar name=None patient_key=String::new() />
                        <div class="subpage-body">
                            <div class="notice error" role="alert">{failure.message}</div>
                        </div>
                    }.into_any(),
                })}
            </Suspense>
        </div>
    }
}

/// Sub-page top bar: wordmark and breadcrumb ending in "New presentation".
#[component]
fn SubpageBar(name: Option<String>, patient_key: String) -> impl IntoView {
    view! {
        <header class="subbar">
            <A href="/" attr:class="wordmark subbar-wordmark" attr:aria-label="DentalClarity dashboard">
                "dental"<span>"clarity"</span>
            </A>
            <nav class="breadcrumb" aria-label="Breadcrumb">
                <A href="/patients">"Patients"</A>
                {name.map(|n| view! {
                    <span class="crumb-sep" aria-hidden="true">"›"</span>
                    <A href=patient_href(&patient_key)>{n}</A>
                })}
                <span class="crumb-sep" aria-hidden="true">"›"</span>
                <span aria-current="page">"New presentation"</span>
            </nav>
        </header>
    }
}

fn level_label(level: ReadingLevel) -> &'static str {
    match level {
        ReadingLevel::Simple => "Simple",
        ReadingLevel::Standard => "Standard",
        ReadingLevel::Detailed => "Detailed",
    }
}

#[component]
fn CreateView(patient: PatientDetail, plan: PlanDetail) -> impl IntoView {
    let name = format!("{} {}", patient.first_name, patient.last_name);
    let patient_key = patient.patient_key.clone();
    let plan_key = plan.plan_key.clone();
    let has_unknown = plan_has_unknown(&plan);
    let has_procedures = !plan.procedures.is_empty();
    let chip_plan = plan.clone();

    let language =
        RwSignal::new(default_presentation_language(patient.language.as_deref()).to_owned());
    let level = RwSignal::new(ReadingLevel::Standard);
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let blocked = RwSignal::new(false);
    let started = RwSignal::new(None::<CreatePresentationResponse>);

    let create = {
        let patient_key = patient_key.clone();
        move |_| {
            if busy.get_untracked() || blocked.get_untracked() {
                return;
            }
            let request = CreatePresentationRequest {
                patient_key: patient_key.clone(),
                plan_key: plan_key.clone(),
                language: language.get_untracked(),
                reading_level: level.get_untracked(),
            };
            busy.set(true);
            error.set(None);
            spawn_local(async move {
                match api::create_presentation(&request).await {
                    Ok(response) => {
                        started.try_set(Some(response));
                    }
                    Err(failure) => {
                        if BLOCKING_CODES.contains(&failure.code.as_str()) {
                            blocked.try_set(true);
                        }
                        error.try_set(Some(failure.message));
                    }
                }
                busy.try_set(false);
            });
        }
    };
    let retry = Callback::new(move |()| started.set(None));
    let locked = move || started.with(Option::is_some) || busy.get();

    view! {
        <SubpageBar name=Some(name) patient_key=patient_key.clone() />
        <div class="subpage-body create-grid">
            <section class="card patient-card create-plan" aria-labelledby="patient-name">
                <PatientHead patient>
                    <PlanChip plan=chip_plan />
                </PatientHead>
                <PlanTable plan />
                <div class="card-foot">
                    <PlanNotes has_unknown />
                </div>
            </section>
            <div class="create-side">
                <section class="card settings-panel" aria-label="Presentation settings">
                    <div class="setting-row">
                        <label for="presentation-language" class="setting-label">"Language"</label>
                        <select
                            id="presentation-language"
                            class="select"
                            aria-describedby="language-help"
                            disabled=locked
                            on:change=move |ev| language.set(event_target_value(&ev))
                        >
                            {PRESENTATION_LANGUAGES.iter().map(|(tag, label)| {
                                let tag = *tag;
                                view! {
                                    <option value=tag selected=move || language.get() == tag>{*label}</option>
                                }
                            }).collect_view()}
                        </select>
                    </div>
                    <p id="language-help" class="setting-help">
                        "Content is written in English first; other languages arrive in a later update."
                    </p>
                    <div class="setting-row">
                        <span id="level-label" class="setting-label">"Reading level"</span>
                        <div class="segmented" role="radiogroup" aria-labelledby="level-label">
                            {[ReadingLevel::Simple, ReadingLevel::Standard, ReadingLevel::Detailed].into_iter().map(|option| view! {
                                <label class="segment" class:selected=move || level.get() == option>
                                    <input
                                        type="radio"
                                        name="reading-level"
                                        class="visually-hidden"
                                        value=option.to_string()
                                        prop:checked=move || level.get() == option
                                        disabled=locked
                                        on:change=move |_| level.set(option)
                                    />
                                    {level_label(option)}
                                </label>
                            }).collect_view()}
                        </div>
                    </div>
                </section>
                {move || match started.get() {
                    Some(job) => view! {
                        <GenerationProgress
                            job_id=job.job_id
                            presentation_id=job.presentation_id
                            show_open=true
                            on_retry=retry
                        />
                    }.into_any(),
                    None => {
                        let create = create.clone();
                        view! {
                            <div class="create-actions">
                                {move || error.get().map(|message| view! {
                                    <p class="form-error" role="alert">{message}</p>
                                })}
                                {(!has_procedures).then(|| view! {
                                    <p class="setting-help">"This plan has no planned procedures."</p>
                                })}
                                <button
                                    type="button"
                                    class="button primary create-button"
                                    disabled=move || busy.get() || blocked.get() || !has_procedures
                                    on:click=create
                                >
                                    {move || if busy.get() { "Starting…" } else { "Create presentation" }}
                                </button>
                            </div>
                        }.into_any()
                    }
                }}
            </div>
        </div>
    }
}
