//! What happens next.

use super::common::*;
use crate::present::narration::NarratedBlock;
use crate::present::{block, section, use_data};
use dc_core::present::PatientSectionKind;
use leptos::prelude::*;

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
