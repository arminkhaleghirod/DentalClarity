//! Patient questions on the presentation page: the read-only list of what
//! patients asked in Present mode and what they were shown (PRD FR-QA-06).

use crate::api;
use crate::format::timestamp;
use dc_core::qa::{AnswerKind, StaffQaTurn};
use leptos::prelude::*;

fn kind_label(kind: AnswerKind) -> &'static str {
    match kind {
        AnswerKind::Answered => "Answered",
        AnswerKind::Partial => "Partly answered",
        AnswerKind::OutOfScope => "Declined, sent to the team",
        AnswerKind::Urgent => "Urgent",
        AnswerKind::Fallback => "Fixed reply shown",
    }
}

#[component]
pub fn PatientQuestions(id: String) -> impl IntoView {
    let questions = LocalResource::new(move || api::presentation_questions(id.clone()));
    view! {
        <div class="rail-divider"></div>
        <h2 class="rail-title" id="qa-title">"Patient questions"</h2>
        <section class="qa-review" aria-labelledby="qa-title">
            {move || questions.get().map(|result| match result {
                Err(failure) => view! { <p class="qa-empty" role="alert">{failure.message}</p> }.into_any(),
                Ok(list) if list.turns.is_empty() => view! {
                    <p class="qa-empty">"No questions yet. Patients are told the practice can read them."</p>
                }.into_any(),
                Ok(list) => view! {
                    {list.urgent.then(|| view! {
                        <div class="check-callout review">
                            <span class="callout-mark" aria-hidden="true">"!"</span>
                            <p class="callout-lead">"A patient question mentioned possibly urgent symptoms. Follow up with the patient."</p>
                        </div>
                    })}
                    <ol class="qa-list">
                        {list.turns.into_iter().map(|t| view! { <Turn turn=t /> }).collect_view()}
                    </ol>
                }.into_any(),
            })}
        </section>
    }
}

#[component]
fn Turn(turn: StaffQaTurn) -> impl IntoView {
    let answer = turn.answer;
    let text = answer.sentences.join(" ");
    let meta = format!(
        "{} · Session {} · v{}",
        timestamp(&turn.asked_at),
        turn.session,
        turn.version
    );
    view! {
        <li class="qa-turn" class:is-urgent=turn.urgent>
            <p class="qa-meta">{meta}</p>
            <p class="qa-question">{turn.question}</p>
            {answer.urgent_notice.map(|n| view! { <p class="qa-urgent">{n}</p> })}
            {(!text.is_empty()).then(|| view! { <p class="qa-answer">{text}</p> })}
            {answer.suggested_question.map(|q| view! { <p class="qa-suggest">"Suggested: "<i>{q}</i></p> })}
            <p class="qa-outcome">{kind_label(answer.kind)}" · checks: "{turn.outcome}</p>
        </li>
    }
}
