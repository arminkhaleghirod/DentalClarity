//! The five patient screens: At a glance, Journey, Procedures, Costs and
//! Next steps (mockups 2d, 2e, 2f; the rest follow docs/UI_SPEC.md §5.11).

use super::chart::{ToothChart, tooth_location};
use super::narration::NarratedBlock;
use super::{
    Phase, Tab, block, phases, planned_teeth, procedure_section, section, single_tooth, use_data,
    use_nav,
};
use crate::format::{money_view, tooth_label};
use dc_core::Money;
use dc_core::ToothRef;
use dc_core::present::{PatientSectionKind, PatientView};
use dc_core::presentation::ProcedureFactView;
use leptos::prelude::*;

const HELD: &str = "Your dental team will go over this part with you.";

fn number_word(n: usize) -> String {
    const WORDS: [&str; 11] = [
        "no", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
    ];
    WORDS
        .get(n)
        .map_or_else(|| n.to_string(), |w| (*w).to_owned())
}

/// Headline from the plan facts, used when the summary is not available.
fn local_headline(view: &PatientView, phase_count: usize) -> String {
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

fn steps_text(n: usize) -> String {
    if n == 1 {
        "1 step".into()
    } else {
        format!("{n} steps")
    }
}

/// "Root canal · #30", or the name alone without a tooth.
fn procedure_line(p: &ProcedureFactView) -> String {
    let tooth = tooth_label(&p.tooth);
    if tooth.is_empty() {
        p.name.clone()
    } else {
        format!("{} · {tooth}", p.name)
    }
}

fn not_tied_label(p: &ProcedureFactView) -> String {
    let area = p.tooth.trim();
    if area.is_empty() {
        "Not tied to one tooth".into()
    } else {
        area.to_owned()
    }
}

#[component]
fn Held() -> impl IntoView {
    view! { <div class="p-held">{HELD}</div> }
}

/// Procedures that cannot be shown on the chart, as links to their detail.
#[component]
fn NonToothList(procedures: Vec<ProcedureFactView>) -> impl IntoView {
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
            <NarratedBlock section_key=glance_key.clone() block=b headline=true />
        }
        .into_any(),
        Some(b) => view! {
            <h1 class="p-headline">{local_headline(&view, all_phases.len())}</h1>
            <NarratedBlock section_key=glance_key.clone() block=b hide_heading=true />
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
                    on_select=Callback::new(move |label| nav.open_tooth(label)) />
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
    let names: Vec<&str> = phase.procedures.iter().map(|p| p.name.as_str()).collect();
    let title = names.join(", ");
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

// Journey ----------------------------------------------------------------------

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

#[component]
pub fn Procedures() -> impl IntoView {
    let data = use_data();
    let nav = use_nav();
    let procedures = data.with_value(|v| v.procedures.clone());
    let planned = data.with_value(planned_teeth);
    if procedures.is_empty() {
        return view! { <div class="p-page"><Held /></div> }.into_any();
    }
    let current = {
        let procedures = procedures.clone();
        Memo::new(move |_| {
            let selected = nav.procedure.get();
            procedures
                .iter()
                .find(|p| Some(&p.procedure_ref) == selected.as_ref())
                .or_else(|| procedures.first())
                .cloned()
        })
    };
    let selected_tooth = Signal::derive(move || current.get().and_then(|p| single_tooth(&p)));

    let list = procedures
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let r = p.procedure_ref.clone();
            let is_current = {
                let r = r.clone();
                move || current.with(|c| c.as_ref().map(|c| &c.procedure_ref) == Some(&r))
            };
            let line = procedure_line(p);
            view! {
                <li>
                    <button type="button" class="p-step" class:is-current=is_current.clone()
                        aria-current=move || is_current().then_some("true")
                        on:click=move |_| nav.open_procedure(r.clone())>
                        <span class="p-step-num" aria-hidden="true">{(i + 1).to_string()}</span>
                        <span class="p-step-text">{line}</span>
                    </button>
                </li>
            }
        })
        .collect_view();

    view! {
        <div class="p-procedures">
            <aside class="p-rail" aria-label="Your procedures">
                <ToothChart planned selected=selected_tooth compact=true
                    on_select=Callback::new(move |label| nav.open_tooth(label)) />
                <h2 class="p-eyebrow-muted">{format!("Your plan · {}", steps_text(procedures.len()))}</h2>
                <ol class="p-step-list">{list}</ol>
            </aside>
            <div class="p-detail">
                {move || current.get().map(|p| view! { <ProcedureDetail procedure=p /> })}
            </div>
        </div>
    }
    .into_any()
}

#[component]
fn ReadingLevel(current: dc_core::content::ReadingLevel) -> impl IntoView {
    use dc_core::content::ReadingLevel as Level;
    let options = [
        (Level::Simple, "Simple"),
        (Level::Standard, "Standard"),
        (Level::Detailed, "Detailed"),
    ];
    view! {
        <div class="p-segmented" role="radiogroup" aria-label="Reading level"
            aria-disabled="true" aria-describedby="p-level-later"
            title="More reading levels arrive in a later update">
            {options.into_iter().map(|(level, label)| {
                let on = level == current;
                view! {
                    <span role="radio" class="p-segment" class:is-on=on
                        aria-checked=if on { "true" } else { "false" } aria-disabled="true">
                        {label}
                    </span>
                }
            }).collect_view()}
            <span id="p-level-later" class="visually-hidden">"More reading levels arrive in a later update"</span>
        </div>
    }
}

#[component]
fn ProcedureDetail(procedure: ProcedureFactView) -> impl IntoView {
    let data = use_data();
    let (section, provider, level) = data.with_value(|v| {
        (
            procedure_section(v, &procedure.procedure_ref).cloned(),
            v.provider_display_name.clone(),
            v.reading_level,
        )
    });
    let tooth = single_tooth(&procedure);
    let location = match &tooth {
        Some(t) => {
            let place = t
                .parse::<ToothRef>()
                .ok()
                .and_then(tooth_location)
                .map(|l| format!(", {l}"))
                .unwrap_or_default();
            format!("Tooth {t}{place} · phase {}", procedure.phase)
        }
        None => format!("{} · phase {}", not_tied_label(&procedure), procedure.phase),
    };
    let title = section
        .as_ref()
        .map(|s| s.title.clone())
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| procedure.name.clone());

    let body = match section {
        None => view! { <Held /> }.into_any(),
        Some(section) => {
            let key = section.key.clone();
            let blocks = section
                .blocks
                .iter()
                .filter(|b| !b.sentences.is_empty())
                .cloned()
                .map(|b| {
                    view! { <NarratedBlock section_key=key.clone() block=b provider=provider.clone() /> }
                })
                .collect_view();
            let questions = section.questions.clone();
            view! {
                {blocks}
                {(!questions.is_empty()).then(|| view! {
                    <section class="p-block p-questions">
                        <h2 class="p-block-title">"Questions you might ask your dental team"</h2>
                        <ul class="p-question-list">
                            {questions.into_iter().map(|q| view! { <li>{q}</li> }).collect_view()}
                        </ul>
                    </section>
                })}
            }
            .into_any()
        }
    };

    view! {
        <article class="p-detail-inner">
            <div class="p-detail-head">
                <div>
                    <div class="p-eyebrow">{location}</div>
                    <h1 class="p-detail-title">{title}</h1>
                </div>
                <ReadingLevel current=level />
            </div>
            <div class="p-illustration" role="img" aria-label="Illustration coming soon">
                <span>"Illustration coming soon"</span>
            </div>
            {body}
        </article>
    }
}

// Costs ------------------------------------------------------------------------

fn sum(values: impl Iterator<Item = Option<Money>>) -> Option<Money> {
    let mut total = Money::ZERO;
    for v in values {
        total = total + v?;
    }
    Some(total)
}

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

#[component]
pub fn NextSteps() -> impl IntoView {
    let data = use_data();
    let next = data.with_value(|v| section(v, PatientSectionKind::NextSteps).cloned());
    let Some(next) = next else {
        return view! { <div class="p-page"><Held /></div> }.into_any();
    };
    let key = next.key.clone();
    let outlook = block(&next, "outlook").cloned();
    let steps = block(&next, "next_steps").cloned();
    let questions = next.questions.clone();
    view! {
        <div class="p-page">
            <h1 class="p-page-title">"What happens next"</h1>
            <div class="p-next">
                <div class="p-next-col">
                    {outlook.map(|b| view! { <NarratedBlock section_key=key.clone() block=b /> })}
                </div>
                <div class="p-next-col">
                    {steps.map(|b| view! { <NarratedBlock section_key=key.clone() block=b list=true /> })}
                    {(!questions.is_empty()).then(|| view! {
                        <section class="p-block">
                            <h2 class="p-block-title">"Questions you might ask"</h2>
                            <ul class="p-question-cards">
                                {questions.into_iter().map(|q| view! { <li>{q}</li> }).collect_view()}
                            </ul>
                        </section>
                    })}
                </div>
            </div>
            <p class="p-neutral">
                "This presentation explains the plan your dentist proposed. It is not medical advice. Your dental team can answer questions about your own situation."
            </p>
        </div>
    }
    .into_any()
}
