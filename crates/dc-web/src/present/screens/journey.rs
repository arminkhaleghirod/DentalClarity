//! Treatment journey.

use super::common::*;
use crate::present::narration::NarratedBlock;
use crate::present::{block, phases, section, use_data, use_nav};
use dc_core::present::PatientSectionKind;
use leptos::prelude::*;

#[component]
pub fn Journey() -> impl IntoView {
    let data = use_data();
    let nav = use_nav();
    let view = data.get_value();
    let journey = section(&view, PatientSectionKind::Journey).cloned();
    let Some(journey) = journey else {
        return view! { <div class="p-page"><Held /></div> }.into_any();
    };
    let key = journey.key.clone();
    let dependencies = block(&journey, "dependencies").cloned();
    let columns = phases(&view)
        .into_iter()
        .map(|phase| {
            let label = phase.label();
            let class = phase.class();
            let summary = phase.summary.clone().map(|b| {
                view! { <NarratedBlock section_key=key.clone() block=b hide_heading=true /> }
            });
            let chips = phase
                .procedures
                .iter()
                .map(|p| {
                    let r = p.procedure_ref.clone();
                    view! {
                        <li>
                            <button type="button" class="p-chip" on:click=move |_| nav.open_procedure(r.clone())>
                                {procedure_line(p)}
                            </button>
                        </li>
                    }
                })
                .collect_view();
            view! {
                <li class=format!("p-journey-phase {class}")>
                    <div class="p-journey-node" aria-hidden="true">{phase.number.to_string()}</div>
                    <div class="p-journey-card">
                        <h2 class="p-phase-eyebrow">{label}</h2>
                        {summary}
                        <ul class="p-chip-list" aria-label=format!("Procedures in phase {}", phase.number)>{chips}</ul>
                    </div>
                </li>
            }
        })
        .collect_view();
    view! {
        <div class="p-page">
            <h1 class="p-page-title">{journey.title.clone()}</h1>
            <p class="p-page-sub">"Your dental team will schedule the visits."</p>
            <ol class="p-journey">{columns}</ol>
            {dependencies.map(|b| view! { <div class="p-journey-deps"><NarratedBlock section_key=key.clone() block=b /></div> })}
        </div>
    }
    .into_any()
}

// Procedures -------------------------------------------------------------------
