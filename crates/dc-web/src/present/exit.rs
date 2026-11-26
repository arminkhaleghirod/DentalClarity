//! Leaving Present mode (DECISIONS P8). Demo mode leaves directly; clinical
//! mode asks for the staff PIN, or the staff password once the PIN is locked.
//! Success does a full page load so no patient state stays in memory.

use super::api::{self, ExitBody};
use super::narration::use_narrator;
use leptos::ev::{KeyboardEvent, SubmitEvent};
use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;

/// Only same-origin paths are followed.
fn go_to_staff(path: &str) -> Result<(), String> {
    if !path.starts_with('/') || path.starts_with("//") || path.starts_with("/\\") {
        return Err("The server sent an unexpected address.".into());
    }
    window()
        .location()
        .assign(path)
        .map_err(|_| "The browser could not open the staff view.".to_owned())
}

#[component]
pub fn ExitControl(demo: bool) -> impl IntoView {
    let narrator = use_narrator();
    let open = RwSignal::new(false);
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let done_button = NodeRef::<html::Button>::new();

    let leave_demo = move || {
        busy.set(true);
        error.set(None);
        spawn_local(async move {
            match api::exit(&ExitBody::default()).await {
                Ok(response) => {
                    if let Err(message) = go_to_staff(&response.staff_path) {
                        error.set(Some(message));
                        busy.set(false);
                    }
                }
                Err(failure) if failure.session_ended() => narrator.session_ended.set(true),
                Err(failure) => {
                    error.set(Some(failure.message));
                    busy.set(false);
                }
            }
        });
    };

    let on_done = move |_| {
        narrator.stop();
        if demo {
            leave_demo();
        } else {
            open.set(true);
        }
    };
    let close = Callback::new(move |()| {
        open.set(false);
        if let Some(button) = done_button.get_untracked() {
            let _ = button.focus();
        }
    });

    view! {
        <div class="p-exit">
            <button type="button" class="p-done" node_ref=done_button on:click=on_done
                disabled=move || busy.get()
                title=(!demo).then_some("For staff: leave the presentation")>
                {move || if busy.get() { "Leaving…" } else { "Done" }}
            </button>
            <Show when=move || error.get().is_some()>
                <p class="p-exit-error" role="alert">{move || error.get().unwrap_or_default()}</p>
            </Show>
            <Show when=move || open.get()>
                <PinDialog close />
            </Show>
        </div>
    }
}

#[component]
fn PinDialog(close: Callback<()>) -> impl IntoView {
    let narrator = use_narrator();
    let secret = RwSignal::new(String::new());
    let locked = RwSignal::new(false);
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let input = NodeRef::<html::Input>::new();
    let submit_button = NodeRef::<html::Button>::new();

    let focus_input = move || {
        if let Some(el) = input.get_untracked() {
            let _ = el.focus();
        }
    };
    Effect::new(move |_| {
        if let Some(el) = input.get() {
            let _ = el.focus();
        }
    });

    let on_submit = move |ev: SubmitEvent| {
        ev.prevent_default();
        if busy.get_untracked() {
            return;
        }
        let value = secret.get_untracked();
        let is_locked = locked.get_untracked();
        let body = if is_locked {
            if value.is_empty() {
                error.set(Some("Enter the staff password.".into()));
                return;
            }
            ExitBody {
                pin: None,
                password: Some(value),
            }
        } else {
            let digits = value.chars().all(|c| c.is_ascii_digit());
            if !digits || !(6..=8).contains(&value.len()) {
                error.set(Some("The staff PIN has 6 to 8 digits.".into()));
                return;
            }
            ExitBody {
                pin: Some(value),
                password: None,
            }
        };
        busy.set(true);
        error.set(None);
        spawn_local(async move {
            match api::exit(&body).await {
                Ok(response) => {
                    if let Err(message) = go_to_staff(&response.staff_path) {
                        error.set(Some(message));
                        busy.set(false);
                    }
                    return;
                }
                Err(failure) if failure.session_ended() => {
                    narrator.session_ended.set(true);
                    return;
                }
                Err(failure) => {
                    if failure.code == "pin_locked" {
                        locked.set(true);
                    }
                    error.set(Some(failure.message));
                }
            }
            secret.set(String::new());
            busy.set(false);
            focus_input();
        });
    };

    // Focus stays inside the dialog: Tab from the last control wraps to the
    // field, Shift+Tab from the field wraps to the last control.
    let wrap_back = move |ev: KeyboardEvent| {
        if ev.key() == "Tab" && ev.shift_key() {
            ev.prevent_default();
            if let Some(el) = submit_button.get_untracked() {
                let _ = el.focus();
            }
        }
    };
    let wrap_forward = move |ev: KeyboardEvent| {
        if ev.key() == "Tab" && !ev.shift_key() {
            ev.prevent_default();
            focus_input();
        }
    };

    view! {
        <div class="p-modal-backdrop">
            <div class="p-modal" role="dialog" aria-modal="true" aria-labelledby="p-exit-title"
                aria-describedby="p-exit-desc">
                <div class="p-modal-eyebrow">"For staff"</div>
                <h2 id="p-exit-title" class="p-modal-title">
                    {move || if locked.get() { "Enter the staff password" } else { "Enter the staff PIN" }}
                </h2>
                <p id="p-exit-desc" class="p-modal-text">
                    "If you are a patient, please hand the device back to your dental team."
                </p>
                <form class="p-modal-form" on:submit=on_submit novalidate>
                    <label class="p-field-label" for="p-exit-secret">
                        {move || if locked.get() { "Staff password" } else { "Staff PIN (6 to 8 digits)" }}
                    </label>
                    <input id="p-exit-secret" class="p-input" type="password" autocomplete="off"
                        node_ref=input
                        inputmode=move || if locked.get() { "text" } else { "numeric" }
                        maxlength=move || (!locked.get()).then_some("8")
                        prop:value=move || secret.get()
                        on:input=move |ev| secret.set(event_target_value(&ev))
                        on:keydown=wrap_back
                        aria-invalid=move || error.get().is_some().then_some("true")
                        aria-describedby="p-exit-error" />
                    <p id="p-exit-error" class="p-modal-error" role="alert">
                        {move || error.get().unwrap_or_default()}
                    </p>
                    <div class="p-modal-actions">
                        <button type="button" class="p-button secondary" on:click=move |_| close.run(())>
                            "Cancel"
                        </button>
                        <button type="submit" class="p-button primary" node_ref=submit_button
                            aria-disabled=move || busy.get().then_some("true")
                            on:keydown=wrap_forward>
                            {move || if busy.get() { "Checking…" } else { "Unlock" }}
                        </button>
                    </div>
                </form>
            </div>
        </div>
    }
}
