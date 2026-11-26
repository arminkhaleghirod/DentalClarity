//! Treatment journey: a vertical timeline of the phases in treatment order.
//!
//! Each phase shows its procedures as chips that open the procedure detail,
//! and the journey section's summary for that phase with its own narration
//! control. Only documented order and dependencies appear: no dates, visit
//! counts or durations. When the journey section is held or empty, the
//! phases still come from the plan facts, with a neutral note.

use super::common::*;
use crate::present::narration::NarratedBlock;
use crate::present::{Phase, block, phases, section, use_data, use_nav};
use dc_core::present::PatientSectionKind;
use leptos::prelude::*;

const NO_SUMMARY_NOTE: &str =
    "Your dental team will explain each step with you. The order below comes from your plan.";

#[component]
pub fn Journey() -> impl IntoView {
    let data = use_data();
    let view = data.get_value();
    let journey = section(&view, PatientSectionKind::Journey).cloned();
    let key = journey.as_ref().map(|j| j.key.clone()).unwrap_or_default();
    let title = journey
        .as_ref()
        .map(|j| j.title.trim().to_owned())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "Your treatment journey".to_owned());
    let dependencies = journey
        .as_ref()
        .and_then(|j| block(j, "dependencies"))
        .cloned();
    let phases = phases(&view);
    let any_summary = phases.iter().any(|p| p.summary.is_some());
    let count = phases.len();

    let intro = if count == 1 {
        "Your plan has one phase.".to_owned()
    } else {
        format!(
            "Your plan has {} phases, shown in treatment order.",
            number_word(count)
        )
    };

    let items = phases
        .into_iter()
        .map(|phase| view! { <PhaseStep phase section_key=key.clone() /> })
        .collect_view();

    view! {
        <div class="p-page p-journey-page">
            <div class="p-journey-intro">
                <h1 class="p-page-title">{title}</h1>
                <p class="p-page-sub">{intro} " Your dental team will schedule the visits."</p>
            </div>
            {(!any_summary).then(|| view! { <p class="p-journey-note">{NO_SUMMARY_NOTE}</p> })}
            {(count == 0).then(|| view! { <Held /> })}
            <ol class="p-journey" aria-label="Treatment phases in order">{items}</ol>
            {dependencies.map(|b| view! {
                <div class="p-journey-deps">
                    <NarratedBlock section_key=key.clone() block=b />
                </div>
            })}
        </div>
    }
}

#[component]
fn PhaseStep(phase: Phase, section_key: String) -> impl IntoView {
    let nav = use_nav();
    let class = format!("p-journey-phase {}", phase.class());
    let heading_id = format!("p-journey-phase-{}", phase.number);
    let item_label = heading_id.clone();
    let teeth = crate::present::teeth_phrase(phase.procedures.iter());
    let summary = phase.summary.clone().map(|b| {
        view! { <NarratedBlock section_key=section_key.clone() block=b hide_heading=true /> }
    });
    let chips = phase
        .procedures
        .iter()
        .map(|p| {
            let r = p.procedure_ref.clone();
            let line = procedure_line(p);
            view! {
                <li>
                    <button type="button" class="p-chip p-journey-chip"
                        aria-label=format!("{line}, open details")
                        on:click=move |_| nav.open_procedure(r.clone())>
                        <span>{line.clone()}</span>
                        <span class="p-chip-arrow" aria-hidden="true">"›"</span>
                    </button>
                </li>
            }
        })
        .collect_view();
    view! {
        <li class=class aria-labelledby=item_label>
            <div class="p-journey-marker" aria-hidden="true">{phase.number.to_string()}</div>
            <div class="p-journey-card">
                <h2 class="p-journey-title" id=heading_id>
                    {format!("Phase {}", phase.number)}
                    {teeth.map(|t| view! { <span class="p-journey-teeth">{format!(" · {t}")}</span> })}
                </h2>
                {summary}
                <ul class="p-chip-list" aria-label=format!("Procedures in phase {}", phase.number)>{chips}</ul>
            </div>
        </li>
    }
}
