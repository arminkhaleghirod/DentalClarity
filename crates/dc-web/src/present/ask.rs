//! "Ask a question" (UI_SPEC 4.13): the floating button and the drawer.
//!
//! The drawer is a dialog: focus moves into it, Tab stays inside it, Escape
//! closes it and focus returns to the button. The notice that the dental
//! team can read the conversation stays above the input before and after
//! the first question. Answers are plain text nodes, announced politely.

use super::api;
use super::narration::use_narrator;
use crate::motion::{DUR_BASE_MS, after_exit};
use dc_core::qa::{AnswerKind, QaAnswer, QaConversation, QaTurn};
use leptos::ev::{KeyboardEvent, SubmitEvent};
use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos::wasm_bindgen::{JsCast, JsValue};

const FOCUSABLE: &str =
    "button:not([disabled]), textarea:not([disabled]), a[href], [tabindex]:not([tabindex='-1'])";

/// The focusable elements inside `root`, in document order.
fn focusable(root: &leptos::web_sys::Element) -> Vec<leptos::web_sys::HtmlElement> {
    let Ok(list) = js_sys::Reflect::get(root, &"querySelectorAll".into())
        .and_then(|f| f.dyn_into::<js_sys::Function>())
        .and_then(|f| f.call1(root, &FOCUSABLE.into()))
    else {
        return Vec::new();
    };
    let length = js_sys::Reflect::get(&list, &"length".into())
        .ok()
        .and_then(|l| l.as_f64())
        .unwrap_or(0.0) as u32;
    (0..length)
        .filter_map(|i| js_sys::Reflect::get(&list, &JsValue::from(i)).ok())
        .filter_map(|el| el.dyn_into::<leptos::web_sys::HtmlElement>().ok())
        .collect()
}

#[component]
pub fn AskQuestion() -> impl IntoView {
    let open = RwSignal::new(false);
    let closing = RwSignal::new(false);
    let opener = NodeRef::<html::Button>::new();
    let close = Callback::new(move |()| {
        if closing.get_untracked() {
            return;
        }
        closing.set(true);
        if let Some(button) = opener.get_untracked() {
            let _ = button.focus();
        }
        after_exit(DUR_BASE_MS, move || {
            open.try_set(false);
            closing.try_set(false);
        });
    });
    view! {
        <button type="button" class="p-ask-button" node_ref=opener
            aria-haspopup="dialog"
            class:is-hidden=move || open.get()
            on:click=move |_| open.set(true)>
            <span class="p-ask-bubble" aria-hidden="true">"?"</span>
            "Ask a question"
        </button>
        <Show when=move || open.get()>
            <AskDrawer close closing=closing.into() />
        </Show>
    }
}

#[component]
fn AskDrawer(close: Callback<()>, closing: Signal<bool>) -> impl IntoView {
    let narrator = use_narrator();
    let conversation = RwSignal::new(None::<QaConversation>);
    let load_error = RwSignal::new(None::<String>);
    let draft = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let panel = NodeRef::<html::Div>::new();
    let input = NodeRef::<html::Textarea>::new();
    let log = NodeRef::<html::Div>::new();

    let scroll_to_end = move || {
        crate::motion::after_update(move || {
            if let Some(el) = log.get_untracked() {
                el.set_scroll_top(el.scroll_height());
            }
        });
    };
    spawn_local(async move {
        match api::questions().await {
            Ok(c) => {
                conversation.try_set(Some(c));
                scroll_to_end();
            }
            Err(f) if f.session_ended() => narrator.session_ended.set(true),
            Err(f) => {
                load_error.try_set(Some(f.message));
            }
        }
    });
    Effect::new(move |_| {
        if let Some(el) = input.get() {
            let _ = el.focus();
        }
    });

    let can_ask = move || {
        conversation.with(|c| {
            c.as_ref()
                .is_some_and(|c| c.available && c.questions_left > 0)
        })
    };
    let send = move || {
        let question = draft.get_untracked().trim().to_owned();
        if question.is_empty() || busy.get_untracked() || !can_ask() {
            return;
        }
        busy.set(true);
        error.set(None);
        spawn_local(async move {
            match api::ask(question).await {
                Ok(response) => {
                    conversation.update(|c| {
                        if let Some(c) = c {
                            c.turns.push(response.turn);
                            c.questions_left = response.questions_left;
                        }
                    });
                    draft.set(String::new());
                    scroll_to_end();
                }
                Err(f) if f.session_ended() => {
                    narrator.session_ended.set(true);
                    return;
                }
                Err(f) => {
                    if f.code == "question_limit"
                        && f.status == 429
                        && f.message.contains("this visit")
                    {
                        conversation.update(|c| {
                            if let Some(c) = c {
                                c.questions_left = 0;
                            }
                        });
                    }
                    error.set(Some(f.message));
                }
            }
            busy.set(false);
            if let Some(el) = input.get_untracked() {
                let _ = el.focus();
            }
        });
    };
    let on_submit = move |ev: SubmitEvent| {
        ev.prevent_default();
        send();
    };
    // Enter sends; Shift+Enter starts a new line.
    let on_input_key = move |ev: KeyboardEvent| {
        if ev.key() == "Enter" && !ev.shift_key() && !ev.is_composing() {
            ev.prevent_default();
            send();
        }
    };
    let on_keydown = move |ev: KeyboardEvent| {
        if ev.key() == "Escape" {
            ev.prevent_default();
            close.run(());
            return;
        }
        if ev.key() != "Tab" {
            return;
        }
        let Some(root) = panel.get_untracked() else {
            return;
        };
        let items = focusable(&root);
        let (Some(first), Some(last)) = (items.first(), items.last()) else {
            return;
        };
        let active = document().active_element();
        let is = |el: &leptos::web_sys::HtmlElement| {
            active
                .as_ref()
                .is_some_and(|a| JsValue::from(a) == JsValue::from(el))
        };
        if ev.shift_key() && is(first) {
            ev.prevent_default();
            let _ = last.focus();
        } else if !ev.shift_key() && is(last) {
            ev.prevent_default();
            let _ = first.focus();
        }
    };

    view! {
        <div class="p-ask-backdrop" class:is-closing=move || closing.get() on:click=move |_| close.run(())></div>
        <div class="p-ask" class:is-closing=move || closing.get() node_ref=panel role="dialog"
            aria-modal="true" aria-labelledby="p-ask-title" aria-describedby="p-ask-notice"
            on:keydown=on_keydown>
            <div class="p-ask-handle" aria-hidden="true"></div>
            <header class="p-ask-header">
                <h2 id="p-ask-title" class="p-ask-title">"Ask a question"</h2>
                <button type="button" class="p-ask-close" aria-label="Close" on:click=move |_| close.run(())>
                    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                        stroke-width="2" stroke-linecap="round" aria-hidden="true">
                        <path d="M6 6l12 12M18 6L6 18" />
                    </svg>
                </button>
            </header>
            <div id="p-ask-notice" class="p-ask-notice">
                <p>{move || conversation.with(|c| c.as_ref().map_or_else(|| dc_core::qa::NOTICE.to_owned(), |c| c.notice.clone()))}</p>
                {move || conversation.with(|c| c.as_ref().and_then(|c| c.demo_notice.clone()))
                    .map(|n| view! { <p class="p-ask-demo">{n}</p> })}
            </div>
            <div class="p-ask-log" node_ref=log role="log" aria-live="polite" aria-relevant="additions"
                aria-busy=move || busy.get().then_some("true")>
                {move || load_error.get().map(|m| view! { <p class="p-ask-error" role="alert">{m}</p> })}
                <For
                    each=move || conversation.with(|c| c.as_ref().map(|c| c.turns.clone()).unwrap_or_default().into_iter().enumerate().collect::<Vec<_>>())
                    key=|(i, _)| *i
                    children=move |(_, turn)| view! { <Exchange turn /> }
                />
                <Show when=move || busy.get()>
                    <div class="p-ask-typing">
                        <span class="visually-hidden">"Finding an answer"</span>
                        <span class="p-ask-dot" aria-hidden="true"></span>
                        <span class="p-ask-dot" aria-hidden="true"></span>
                        <span class="p-ask-dot" aria-hidden="true"></span>
                    </div>
                </Show>
            </div>
            <form class="p-ask-form" on:submit=on_submit>
                <p class="p-ask-error" role="alert">{move || error.get().unwrap_or_default()}</p>
                {move || conversation.with(|c| match c {
                    Some(c) if !c.available => Some("Questions are not available right now. Your dental team will be happy to help."),
                    Some(c) if c.questions_left == 0 => Some("You've reached the number of questions for this visit. The front desk will be happy to answer more."),
                    _ => None,
                }).map(|m| view! { <p class="p-ask-unavailable">{m}</p> })}
                <div class="p-ask-row">
                    <label class="visually-hidden" for="p-ask-input">"Your question"</label>
                    <textarea id="p-ask-input" class="p-ask-input" node_ref=input rows="1"
                        placeholder="Type a question..."
                        maxlength=move || conversation.with(|c| c.as_ref().map_or(500, |c| c.max_chars)).to_string()
                        disabled=move || !can_ask()
                        aria-disabled=move || busy.get().then_some("true")
                        prop:value=move || draft.get()
                        on:input=move |ev| draft.set(event_target_value(&ev))
                        on:keydown=on_input_key></textarea>
                    <button type="submit" class="p-ask-send" aria-label="Send question"
                        disabled=move || !can_ask()
                        aria-disabled=move || (busy.get() || draft.with(|d| d.trim().is_empty())).then_some("true")>
                        <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                            stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                            <path d="M5 12h14M13 6l6 6-6 6" />
                        </svg>
                    </button>
                </div>
                {move || conversation.with(|c| c.as_ref().filter(|c| c.available && c.questions_left > 0 && c.questions_left <= 5).map(|c| c.questions_left))
                    .map(|n| view! { <p class="p-ask-left">{format!("{n} {} left", if n == 1 { "question" } else { "questions" })}</p> })}
            </form>
        </div>
    }
}

#[component]
fn Exchange(turn: QaTurn) -> impl IntoView {
    view! {
        <div class="p-ask-question">{turn.question}</div>
        <Answer answer=turn.answer />
    }
}

#[component]
fn Answer(answer: QaAnswer) -> impl IntoView {
    let declined = matches!(
        answer.kind,
        AnswerKind::OutOfScope | AnswerKind::Partial | AnswerKind::Fallback
    );
    let text = answer.sentences.join(" ");
    let sources = (!answer.sources.is_empty()).then(|| answer.sources.join("; "));
    view! {
        <div class="p-ask-answer">
            {answer.urgent_notice.map(|n| view! { <p class="p-ask-urgent" role="alert">{n}</p> })}
            {(!text.is_empty()).then(|| view! { <p>{text}</p> })}
            {sources.map(|s| view! { <p class="p-ask-source">"Source: "{s}</p> })}
            {answer.suggested_question.filter(|_| declined).map(|q| view! {
                <p class="p-ask-suggest">"You could ask your dental team: "<i>{q}</i></p>
            })}
        </div>
    }
}
