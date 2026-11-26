//! Helpers shared by the patient screens.

use crate::format::tooth_label;
use crate::present::{planned_teeth, use_nav};
use dc_core::Money;
use dc_core::present::PatientView;
use dc_core::presentation::ProcedureFactView;
use leptos::prelude::*;

pub(super) const HELD: &str = "Your dental team will go over this part with you.";

pub(super) fn number_word(n: usize) -> String {
    const WORDS: [&str; 11] = [
        "no", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
    ];
    WORDS
        .get(n)
        .map_or_else(|| n.to_string(), |w| (*w).to_owned())
}

/// Headline from the plan facts, used when the summary is not available.
pub(super) fn local_headline(view: &PatientView, phase_count: usize) -> String {
    let teeth = planned_teeth(view).len();
    let phases = if phase_count == 1 {
        "one phase".to_owned()
    } else {
        format!("{} phases", number_word(phase_count))
    };
    if teeth > 0 {
        let noun = if teeth == 1 { "tooth" } else { "teeth" };
        format!(
            "Your plan treats {} {noun} in {phases}.",
            number_word(teeth)
        )
    } else {
        let n = view.procedures.len();
        let noun = if n == 1 { "procedure" } else { "procedures" };
        format!("Your plan has {} {noun} in {phases}.", number_word(n))
    }
}

pub(super) fn steps_text(n: usize) -> String {
    if n == 1 {
        "1 step".into()
    } else {
        format!("{n} steps")
    }
}

/// "Root canal · #30", or the name alone without a tooth.
pub(super) fn procedure_line(p: &ProcedureFactView) -> String {
    let tooth = tooth_label(&p.tooth);
    if tooth.is_empty() {
        p.name.clone()
    } else {
        format!("{} · {tooth}", p.name)
    }
}

pub(super) fn not_tied_label(p: &ProcedureFactView) -> String {
    let area = p.tooth.trim();
    if area.is_empty() {
        "Not tied to one tooth".into()
    } else {
        area.to_owned()
    }
}

#[component]
pub(super) fn Held() -> impl IntoView {
    view! { <div class="p-held">{HELD}</div> }
}

/// Procedures that cannot be shown on the chart, as links to their detail.
#[component]
pub(super) fn NonToothList(procedures: Vec<ProcedureFactView>) -> impl IntoView {
    let nav = use_nav();
    (!procedures.is_empty()).then(|| {
        view! {
            <div class="p-nontooth">
                <h2 class="p-eyebrow-muted">"Not tied to one tooth"</h2>
                <ul class="p-plain-list">
                    {procedures.into_iter().map(|p| {
                        let r = p.procedure_ref.clone();
                        let meta = match p.tooth.trim() {
                            "" => format!("Phase {}", p.phase),
                            area => area.to_owned(),
                        };
                        view! {
                            <li>
                                <button type="button" class="p-list-button" on:click=move |_| nav.open_procedure(r.clone())>
                                    <span class="p-list-name">{p.name}</span>
                                    <span class="p-list-meta">{meta}</span>
                                </button>
                            </li>
                        }
                    }).collect_view()}
                </ul>
            </div>
        }
    })
}

// At a glance ------------------------------------------------------------------

pub(super) fn sum(values: impl Iterator<Item = Option<Money>>) -> Option<Money> {
    let mut total = Money::ZERO;
    for v in values {
        total = total + v?;
    }
    Some(total)
}
