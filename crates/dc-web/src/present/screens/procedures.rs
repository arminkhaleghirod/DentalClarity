//! Procedures and procedure detail with narration (mockup 2e).

use super::common::*;
use crate::present::chart::{ToothChart, tooth_location};
use crate::present::narration::NarratedBlock;
use crate::present::{planned_teeth, procedure_section, single_tooth, use_data, use_nav};
use dc_core::ToothRef;
use dc_core::presentation::ProcedureFactView;
use leptos::prelude::*;

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
