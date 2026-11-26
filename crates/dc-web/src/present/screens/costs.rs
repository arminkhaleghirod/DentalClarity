//! Costs (mockup 2f).

use super::common::*;
use crate::format::money_view;
use crate::present::{phases, use_data};
use leptos::prelude::*;

#[component]
pub fn Costs() -> impl IntoView {
    let data = use_data();
    let view = data.get_value();
    let share = view.total_patient_estimate;
    let insurance = view.total_insurance_estimate;
    let fee = view.total_fee;

    let bar = match (insurance, share) {
        (Some(ins), Some(you))
            if ins.cents() >= 0 && you.cents() >= 0 && ins.cents() + you.cents() > 0 =>
        {
            let total = (ins.cents() + you.cents()) as f64;
            let ins_pct = ins.cents() as f64 / total * 100.0;
            let label = format!(
                "Insurance estimate {ins}; your estimated share {you}{}",
                fee.map(|f| format!("; total fees {f}")).unwrap_or_default()
            );
            Some(view! {
                <div class="p-cost-summary">
                    <div class="p-bar" role="img" aria-label=label>
                        <div class="p-bar-ins" style:width=format!("{ins_pct:.1}%")></div>
                        <div class="p-bar-you"></div>
                    </div>
                    <div class="p-legend">
                        <span class="p-legend-item"><span class="p-swatch ins" aria-hidden="true"></span>{format!("Insurance estimate · {ins}")}</span>
                        <span class="p-legend-item"><span class="p-swatch you" aria-hidden="true"></span>{format!("You · {you}")}</span>
                        {fee.map(|f| view! { <span class="p-legend-total">{format!("Total fees · {f}")}</span> })}
                    </div>
                </div>
            })
        }
        _ => None,
    };

    let tables = phases(&view)
        .into_iter()
        .map(|phase| {
            let subtotal = sum(phase.procedures.iter().map(|p| p.patient_estimate));
            let any_unknown = phase
                .procedures
                .iter()
                .any(|p| p.fee.is_none() || p.patient_estimate.is_none());
            let label = phase.label();
            let class = phase.class();
            let rows = phase
                .procedures
                .iter()
                .map(|p| {
                    view! {
                        <tr>
                            <th scope="row">{procedure_line(p)}</th>
                            <td class="fee">{money_view(p.fee)}</td>
                            <td class="you">{money_view(p.patient_estimate)}</td>
                        </tr>
                    }
                })
                .collect_view();
            view! {
                <section class=format!("p-cost-table {class}")>
                    <div class="p-cost-head">
                        <h2 class="p-cost-title">{label}</h2>
                        <span class="p-cost-subtotal">
                            <span class="visually-hidden">"You pay in this phase: "</span>
                            {money_view(subtotal)}
                        </span>
                    </div>
                    <table>
                        <thead><tr><th scope="col">"Procedure"</th><th scope="col" class="fee">"Fee"</th><th scope="col" class="you">"You pay"</th></tr></thead>
                        <tbody>{rows}</tbody>
                    </table>
                    {any_unknown.then(|| view! {
                        <p class="p-cost-note">"Some amounts were not recorded in your plan. The front desk can tell you more."</p>
                    })}
                </section>
            }
        })
        .collect_view();

    view! {
        <div class="p-costs">
            <div class="p-costs-head">
                <h1 class="p-costs-title">"What your plan is estimated to cost"</h1>
                <div class="p-costs-share">
                    <div class="p-share-label">"Your estimated share"</div>
                    <div class="p-costs-figure">{money_view(share)}</div>
                </div>
            </div>
            {bar}
            {insurance.is_none().then(|| view! {
                <p class="p-cost-note">"No insurance estimate was recorded for this plan."</p>
            })}
            <div class="p-cost-grid">{tables}</div>
            <div class="p-disclaimer">
                <span class="p-info" aria-hidden="true">"i"</span>
                <p>"These are " <b>"estimates"</b> " from your plan. Your insurance decides the final amount, which can change. Ask the front desk about payment options."</p>
            </div>
        </div>
    }
}

// Next steps -------------------------------------------------------------------
