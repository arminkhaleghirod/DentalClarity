//! Costs (mockup 2f): estimated share, insurance bar, per-phase tables and
//! plan totals. Unknown amounts read "Not available", and any total with an
//! unknown part is "Not available" too; unknowns are never summed as zero.

use super::common::*;
use crate::format::{money_view, tooth_label};
use crate::present::{Phase, phases, use_data};
use dc_core::Money;
use dc_core::presentation::ProcedureFactView;
use leptos::prelude::*;

const UNKNOWN_NOTE: &str = "Some amounts were not recorded in your plan, so they show as \
     \"Not available\". The front desk can tell you more.";

#[component]
pub fn Costs() -> impl IntoView {
    let data = use_data();
    let view = data.get_value();
    let share = view.total_patient_estimate;
    let insurance = view.total_insurance_estimate;
    let fee = view.total_fee;
    let no_insurance_at_all = view
        .procedures
        .iter()
        .all(|p| p.insurance_estimate.is_none());

    // Headline figure: the share, or the total fees when no insurance
    // estimate was recorded at all (UI_SPEC 5.11, Costs).
    let (figure_label, figure) = match (share, no_insurance_at_all, fee) {
        (None, true, Some(f)) => ("Total estimated fees", Some(f)),
        _ => ("Your estimated share", share),
    };

    let bar = match (insurance, share) {
        (Some(ins), Some(you))
            if ins.cents() >= 0 && you.cents() >= 0 && ins.cents() + you.cents() > 0 =>
        {
            Some(view! { <CostBar ins you fee /> })
        }
        _ => None,
    };
    let insurance_note = if no_insurance_at_all {
        Some("No insurance estimate was recorded for this plan.")
    } else if insurance.is_none() {
        Some(
            "An insurance estimate was not recorded for every procedure, so some totals are not available.",
        )
    } else {
        None
    };

    let phases = phases(&view);
    let many = phases.len() > 2;
    let tables = phases
        .into_iter()
        .map(|phase| view! { <PhaseTable phase /> })
        .collect_view();

    view! {
        <div class="p-costs">
            <div class="p-costs-head">
                <h1 class="p-costs-title">"What your plan is estimated to cost"</h1>
                <div class="p-costs-share">
                    <div class="p-share-label">{figure_label}</div>
                    <div class="p-costs-figure p-num">{money_view(figure)}</div>
                </div>
            </div>
            {bar}
            {insurance_note.map(|n| view! { <p class="p-cost-note p-cost-callout">{n}</p> })}
            <div class="p-cost-grid" class:is-stacked=many>{tables}</div>
            <PlanTotal fee insurance share />
            <div class="p-disclaimer">
                <span class="p-info" aria-hidden="true">"i"</span>
                <p>"These are " <b>"estimates"</b> " from your plan. Your insurance decides the final amount, which can change. Ask the front desk about payment options."</p>
            </div>
        </div>
    }
}

#[component]
fn CostBar(ins: Money, you: Money, fee: Option<Money>) -> impl IntoView {
    let total = (ins.cents() + you.cents()) as f64;
    let ins_pct = ins.cents() as f64 / total * 100.0;
    let you_pct = 100.0 - ins_pct;
    let label = match fee {
        Some(f) => {
            format!("Insurance estimate {ins} of {f} total fees; your estimated share {you}")
        }
        None => format!("Insurance estimate {ins}; your estimated share {you}"),
    };
    view! {
        <div class="p-cost-summary">
            <div class="p-bar" role="img" aria-label=label>
                {(ins.cents() > 0).then(|| view! {
                    <div class="p-bar-ins" style:width=format!("{ins_pct:.1}%")></div>
                })}
                {(you.cents() > 0).then(|| view! {
                    <div class="p-bar-you" style:width=format!("{you_pct:.1}%")></div>
                })}
            </div>
            <div class="p-legend" aria-hidden="true">
                <span class="p-legend-item"><span class="p-swatch ins"></span>{format!("Insurance estimate · {ins}")}</span>
                <span class="p-legend-item"><span class="p-swatch you"></span>{format!("You · {you}")}</span>
                {fee.map(|f| view! { <span class="p-legend-total">{format!("Total fees · {f}")}</span> })}
            </div>
        </div>
    }
}

/// Tooth cell: "#30", an area as written, or a visual dash read as "None".
fn tooth_cell(p: &ProcedureFactView) -> AnyView {
    let label = tooth_label(&p.tooth);
    if label.is_empty() {
        view! {
            <span class="p-cost-none" aria-hidden="true">"—"</span>
            <span class="visually-hidden">"None"</span>
        }
        .into_any()
    } else {
        label.into_any()
    }
}

#[component]
fn PhaseTable(phase: Phase) -> impl IntoView {
    let procs = &phase.procedures;
    let fee = sum(procs.iter().map(|p| p.fee));
    let ins = sum(procs.iter().map(|p| p.insurance_estimate));
    let you = sum(procs.iter().map(|p| p.patient_estimate));
    let any_unknown = procs
        .iter()
        .any(|p| p.fee.is_none() || p.insurance_estimate.is_none() || p.patient_estimate.is_none());
    let heading_id = format!("p-cost-phase-{}", phase.number);
    let section_label = heading_id.clone();
    let table_label = heading_id.clone();
    let label = phase.label();
    let rows = procs
        .iter()
        .map(|p| {
            view! {
                <tr>
                    <th scope="row" class="name">{p.name.clone()}</th>
                    <td class="tooth">{tooth_cell(p)}</td>
                    <td class="num fee" data-label="Fee">{money_view(p.fee)}</td>
                    <td class="num ins" data-label="Insurance estimate">{money_view(p.insurance_estimate)}</td>
                    <td class="num you" data-label="You pay">{money_view(p.patient_estimate)}</td>
                </tr>
            }
        })
        .collect_view();
    view! {
        <section class=format!("p-cost-table {}", phase.class()) aria-labelledby=section_label>
            <div class="p-cost-head">
                <h2 class="p-cost-title" id=heading_id>{label}</h2>
                <span class="p-cost-subtotal p-num">
                    <span class="visually-hidden">"You pay in this phase: "</span>
                    {money_view(you)}
                </span>
            </div>
            <table aria-labelledby=table_label>
                <thead>
                    <tr>
                        <th scope="col" class="name">"Procedure"</th>
                        <th scope="col" class="tooth">"Tooth"</th>
                        <th scope="col" class="num fee">"Fee"</th>
                        <th scope="col" class="num ins">"Insurance estimate"</th>
                        <th scope="col" class="num you">"You pay"</th>
                    </tr>
                </thead>
                <tbody>{rows}</tbody>
                <tfoot>
                    <tr>
                        <th scope="row" colspan="2" class="name">{format!("Phase {} subtotal", phase.number)}</th>
                        <td class="num fee" data-label="Fee">{money_view(fee)}</td>
                        <td class="num ins" data-label="Insurance estimate">{money_view(ins)}</td>
                        <td class="num you" data-label="You pay">{money_view(you)}</td>
                    </tr>
                </tfoot>
            </table>
            {any_unknown.then(|| view! { <p class="p-cost-note">{UNKNOWN_NOTE}</p> })}
        </section>
    }
}

#[component]
fn PlanTotal(fee: Option<Money>, insurance: Option<Money>, share: Option<Money>) -> impl IntoView {
    view! {
        <section class="p-cost-total" aria-labelledby="p-cost-total-title">
            <table aria-labelledby="p-cost-total-title">
                <thead>
                    <tr>
                        <th scope="col" class="name" id="p-cost-total-title">"Plan total"</th>
                        <th scope="col" class="num fee">"Fees"</th>
                        <th scope="col" class="num ins">"Insurance estimate"</th>
                        <th scope="col" class="num you">"You pay"</th>
                    </tr>
                </thead>
                <tbody>
                    <tr>
                        <th scope="row" class="name">"All phases"</th>
                        <td class="num fee" data-label="Fees">{money_view(fee)}</td>
                        <td class="num ins" data-label="Insurance estimate">{money_view(insurance)}</td>
                        <td class="num you" data-label="You pay">{money_view(share)}</td>
                    </tr>
                </tbody>
            </table>
        </section>
    }
}
