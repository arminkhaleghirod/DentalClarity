//! At a glance (mockup 2d).

use super::common::*;
use crate::format::money_view;
use crate::present::chart::ToothChart;
use crate::present::narration::NarratedBlock;
use crate::present::{
    Phase, Tab, block, phases, planned_teeth, section, single_tooth, use_data, use_nav,
};
use dc_core::present::PatientSectionKind;
use dc_core::presentation::ProcedureFactView;
use leptos::prelude::*;

#[component]
pub fn AtAGlance() -> impl IntoView {
    let data = use_data();
    let nav = use_nav();
    let view = data.get_value();
    let all_phases = phases(&view);
    let glance = section(&view, PatientSectionKind::AtAGlance).cloned();
    let has_journey = section(&view, PatientSectionKind::Journey).is_some();
    let planned = planned_teeth(&view);
    let non_tooth: Vec<ProcedureFactView> = view
        .procedures
        .iter()
        .filter(|p| single_tooth(p).is_none())
        .cloned()
        .collect();
    let provider = view.provider_display_name.clone();
    // The chart lists each tooth's procedures in a panel; choosing one opens
    // it on the Procedures tab.
    let chart_procedures = view.procedures.clone();

    let prepared = if view.guardian_audience {
        format!("Prepared for {}'s family", view.patient_first_name)
    } else {
        format!("Prepared for {}", view.patient_first_name)
    };

    let summary = glance.as_ref().and_then(|s| block(s, "summary").cloned());
    let objective = glance.as_ref().and_then(|s| block(s, "objective").cloned());
    let glance_key = glance.as_ref().map(|s| s.key.clone()).unwrap_or_default();
    // A short first sentence becomes the headline. A long one would not
    // read as a headline, so the plan counts are used and the whole summary
    // follows as text.
    let short_lead = summary
        .as_ref()
        .and_then(|b| b.sentences.first())
        .is_some_and(|s| s.text.chars().count() <= 80);
    let headline = match summary {
        Some(b) if short_lead => view! {
            <NarratedBlock section_key=glance_key.clone() block=b headline=true label="Your plan in brief" />
        }
        .into_any(),
        Some(b) => view! {
            <h1 class="p-headline">{local_headline(&view, all_phases.len())}</h1>
            <NarratedBlock section_key=glance_key.clone() block=b hide_heading=true label="Your plan in brief" />
        }
        .into_any(),
        None => view! { <h1 class="p-headline">{local_headline(&view, all_phases.len())}</h1> }
            .into_any(),
    };
    let objective = objective.map(|b| {
        view! { <NarratedBlock section_key=glance_key.clone() block=b provider=provider.clone() /> }
    });

    let shown = if all_phases.len() > 4 {
        3
    } else {
        all_phases.len()
    };
    let more = all_phases.len() - shown;
    let cards = all_phases
        .into_iter()
        .take(shown)
        .map(|phase| view! { <PhaseCard phase /> })
        .collect_view();

    let share = view.total_patient_estimate;
    let (next_label, next_tab) = if has_journey {
        ("See your journey ›", Tab::Journey)
    } else {
        ("See your costs ›", Tab::Costs)
    };

    view! {
        <div class="p-glance">
            <div class="p-chart-panel">
                <ToothChart planned selected=Signal::derive(|| None::<String>)
                    procedures=chart_procedures
                    on_select=Callback::new(|_: String| {})
                    on_open=Callback::new(move |r| nav.open_procedure(r)) />
                <NonToothList procedures=non_tooth />
            </div>
            <div class="p-glance-main">
                <div class="p-eyebrow">{prepared}</div>
                {headline}
                {objective}
                <div class="p-phase-grid">
                    {cards}
                    {(more > 0).then(|| view! {
                        <button type="button" class="p-phase-card phase-4 p-more" on:click=move |_| nav.go(Tab::Journey)>
                            {format!("+{more} more phases")}
                        </button>
                    })}
                </div>
                <div class="p-share-card">
                    <div>
                        <div class="p-share-label">"Your estimated share"</div>
                        <div class="p-share-figure">{money_view(share)}</div>
                        <div class="p-share-note">"Estimate after insurance. Not a guarantee."</div>
                    </div>
                    <button type="button" class="p-button primary" on:click=move |_| nav.go(next_tab)>
                        {next_label}
                    </button>
                </div>
            </div>
        </div>
    }
}

#[component]
fn PhaseCard(phase: Phase) -> impl IntoView {
    let nav = use_nav();
    let label = phase.label();
    let title = grouped_names(phase.procedures.iter().map(|p| p.name.as_str()));
    let first = phase.procedures.first().map(|p| p.procedure_ref.clone());
    let count = steps_text(phase.procedures.len());
    let aria = format!("Learn more about phase {}", phase.number);
    view! {
        <div class=format!("p-phase-card {}", phase.class())>
            <div class="p-phase-eyebrow">{label}</div>
            <div class="p-phase-title">{title}</div>
            <div class="p-phase-foot">
                {first.map(|r| view! {
                    <button type="button" class="p-mini-button" aria-label=aria.clone()
                        on:click=move |_| nav.open_procedure(r.clone())>
                        "Learn more ›"
                    </button>
                })}
                <span class="p-phase-count">{count}</span>
            </div>
        </div>
    }
}

/// "Tooth-colored filling, Tooth-colored filling, Cleaning" reads as
/// "2× Tooth-colored filling, Cleaning", as on the staff dashboard.
fn grouped_names<'a>(names: impl Iterator<Item = &'a str>) -> String {
    let mut groups: Vec<(&str, usize)> = Vec::new();
    for name in names {
        match groups.iter_mut().find(|(n, _)| *n == name) {
            Some((_, count)) => *count += 1,
            None => groups.push((name, 1)),
        }
    }
    groups
        .into_iter()
        .map(|(name, n)| {
            if n == 1 {
                name.to_owned()
            } else {
                format!("{n}× {name}")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::grouped_names;

    #[test]
    fn repeated_procedures_are_grouped() {
        let names = ["Tooth-colored filling", "Tooth-colored filling", "Cleaning"];
        assert_eq!(
            grouped_names(names.into_iter()),
            "2× Tooth-colored filling, Cleaning"
        );
        assert_eq!(grouped_names(["Crown"].into_iter()), "Crown");
    }
}

// Journey ----------------------------------------------------------------------
