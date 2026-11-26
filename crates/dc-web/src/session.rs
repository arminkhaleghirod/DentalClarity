//! The session gate: setup, sign-in and lock screens shown before the workspace.

use crate::api;
use dc_core::Mode;
use dc_core::api::{SessionInfo, SessionState, SetupRequest};
use leptos::ev::SubmitEvent;
use leptos::leptos_dom::helpers::set_interval_with_handle;
use leptos::prelude::*;
use leptos::task::spawn_local;
use std::time::Duration;

/// The current session, shared with every component below the gate.
#[derive(Clone, Copy)]
pub struct SessionContext {
    pub info: RwSignal<Option<Result<SessionInfo, api::Failure>>>,
}

impl SessionContext {
    /// Re-reads the session. In demo mode a browser without a visitor
    /// session starts one automatically.
    pub fn reload(self) {
        spawn_local(async move {
            let result = match api::session().await {
                Ok(info) if info.state == SessionState::NewVisitor => api::visit().await,
                other => other,
            };
            self.info.set(Some(result));
        });
    }

    pub fn set(self, info: SessionInfo) {
        self.info.set(Some(Ok(info)));
    }
}

pub fn use_session() -> SessionContext {
    expect_context::<SessionContext>()
}

/// Wraps a form submission: shows a busy state and the server's message on failure.
fn submit<F, Fut>(busy: RwSignal<bool>, error: RwSignal<Option<String>>, action: F)
where
    F: FnOnce() -> Fut + 'static,
    Fut: std::future::Future<Output = Result<SessionInfo, api::Failure>> + 'static,
{
    let session = use_session();
    busy.set(true);
    error.set(None);
    spawn_local(async move {
        match action().await {
            Ok(info) => session.set(info),
            Err(failure) => error.set(Some(failure.message)),
        }
        busy.set(false);
    });
}

#[component]
fn AuthCard(title: &'static str, subtitle: String, children: Children) -> impl IntoView {
    view! {
        <div class="auth-page">
            <div class="auth-card">
                <div class="wordmark">"dental"<span>"clarity"</span></div>
                <h1>{title}</h1>
                <p class="auth-subtitle">{subtitle}</p>
                {children()}
            </div>
        </div>
    }
}

#[component]
fn FormError(error: RwSignal<Option<String>>) -> impl IntoView {
    view! {
        <Show when=move || error.get().is_some()>
            <p class="form-error" role="alert">{move || error.get().unwrap_or_default()}</p>
        </Show>
    }
}

#[component]
pub fn SignIn(practice_name: String) -> impl IntoView {
    let password = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let on_submit = move |ev: SubmitEvent| {
        ev.prevent_default();
        let value = password.get_untracked();
        submit(busy, error, move || api::login(value));
    };
    view! {
        <AuthCard title="Sign in" subtitle=practice_name>
            <form class="form" on:submit=on_submit>
                <label class="field">
                    <span>"Staff password"</span>
                    <input type="password" autocomplete="current-password" required
                        bind:value=password />
                </label>
                <FormError error />
                <button class="button primary" type="submit" disabled=move || busy.get()>
                    {move || if busy.get() { "Signing in…" } else { "Sign in" }}
                </button>
            </form>
        </AuthCard>
    }
}

#[component]
pub fn Setup() -> impl IntoView {
    let code = RwSignal::new(String::new());
    let practice = RwSignal::new(String::new());
    let password = RwSignal::new(String::new());
    let confirm = RwSignal::new(String::new());
    let pin = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);

    let on_submit = move |ev: SubmitEvent| {
        ev.prevent_default();
        if password.get_untracked() != confirm.get_untracked() {
            error.set(Some("The two passwords don't match.".into()));
            return;
        }
        let request = SetupRequest {
            setup_code: code.get_untracked(),
            practice_name: practice.get_untracked(),
            password: password.get_untracked(),
            pin: pin.get_untracked(),
        };
        submit(
            busy,
            error,
            move || async move { api::setup(&request).await },
        );
    };

    view! {
        <AuthCard title="Set up DentalClarity"
            subtitle="Create the shared staff account for this practice. You can change branding and connections later in Settings.".to_owned()>
            <form class="form" on:submit=on_submit>
                <label class="field">
                    <span>"Setup code"</span>
                    <input type="text" autocomplete="off" autocapitalize="characters" required
                        spellcheck="false" bind:value=code />
                    <small>"Printed in the server console when DentalClarity first starts."</small>
                </label>
                <label class="field">
                    <span>"Practice name"</span>
                    <input type="text" autocomplete="organization" required maxlength="100"
                        bind:value=practice />
                </label>
                <label class="field">
                    <span>"Staff password"</span>
                    <input type="password" autocomplete="new-password" required minlength="12"
                        bind:value=password />
                    <small>"At least 12 characters. A short phrase works well."</small>
                </label>
                <label class="field">
                    <span>"Confirm password"</span>
                    <input type="password" autocomplete="new-password" required bind:value=confirm />
                </label>
                <label class="field">
                    <span>"Staff PIN"</span>
                    <input type="password" inputmode="numeric" autocomplete="off" required
                        pattern="[0-9]{6,8}" minlength="6" maxlength="8" bind:value=pin />
                    <small>"6 to 8 digits. Used to leave Present mode after handing a patient the tablet."</small>
                </label>
                <FormError error />
                <button class="button primary" type="submit" disabled=move || busy.get()>
                    {move || if busy.get() { "Setting up…" } else { "Create account" }}
                </button>
            </form>
            <p class="auth-note">
                "One shared account is a prototype limit. Real clinical use needs individual staff accounts; see the README."
            </p>
        </AuthCard>
    }
}

/// How often the lock screen re-reads the session.
const LOCK_CHECK_EVERY: Duration = Duration::from_secs(5);

/// The shortest staff password the server accepts at setup.
const MIN_PASSWORD_LEN: usize = 12;

/// Re-reads the session from the lock screen. A changed answer replaces the
/// current one, which moves the gate to the workspace once the lock lifts.
/// A failed read keeps the lock screen as it is rather than showing the
/// "not reachable" page for one dropped request.
fn recheck(session: SessionContext) {
    spawn_local(async move {
        let Ok(fresh) = api::session().await else {
            return;
        };
        if fresh.state == SessionState::NewVisitor {
            // The visitor session is gone; start a new one as on first load.
            session.reload();
            return;
        }
        let changed = session.info.with_untracked(|current| {
            current.as_ref().and_then(|r| r.as_ref().ok()) != Some(&fresh)
        });
        if changed {
            session.set(fresh);
        }
    });
}

/// A friendly message for a refused unlock.
fn unlock_message(failure: &api::Failure) -> String {
    match failure.code.as_str() {
        "present_active" => {
            "The patient screen is still open. Leave the presentation from there first.".into()
        }
        "password_required" => "Enter the staff password to unlock.".into(),
        "invalid_credentials" => "That password is not correct.".into(),
        "rate_limited" => match failure.retry_after {
            Some(secs) if secs >= 90 => {
                format!(
                    "Too many tries. Wait {} minutes and try again.",
                    secs.div_ceil(60)
                )
            }
            Some(secs) => format!("Too many tries. Wait {secs} seconds and try again."),
            None => "Too many tries. Wait a few minutes and try again.".into(),
        },
        "bad_csrf" => "This page is out of date. Reload it and try again.".into(),
        _ => failure.message.clone(),
    }
}

/// Sends the browser back to the patient screen. Replace, not assign, so
/// Back from Present does not land on this lock screen.
fn return_to_present() {
    let _ = window().location().replace("/present");
}

/// The lock screen. While a patient is viewing a presentation it offers the
/// way back to it; once that present session has ended it offers to unlock,
/// without a password in demo mode and with the staff password in clinical
/// mode. It re-reads the session every few seconds and on window focus, so
/// leaving Present in another tab unlocks this one too.
#[component]
pub fn Locked(practice_name: String) -> impl IntoView {
    let session = use_session();
    let status = Memo::new(move |_| {
        session.info.with(|i| {
            i.as_ref()
                .and_then(|r| r.as_ref().ok())
                .map(|i| (i.mode, i.present_ended))
        })
    });
    let demo = move || status.with(|s| s.is_some_and(|(mode, _)| mode == Mode::Demo));
    let ended = move || status.with(|s| s.is_some_and(|(_, ended)| ended));

    if let Ok(handle) = set_interval_with_handle(move || recheck(session), LOCK_CHECK_EVERY) {
        on_cleanup(move || handle.clear());
    }
    let focus = window_event_listener(leptos::ev::focus, move |_| recheck(session));
    on_cleanup(move || focus.remove());

    let password = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let confirming = RwSignal::new(false);

    let unlock = move || {
        if busy.get_untracked() {
            return;
        }
        let secret = (!demo()).then(|| password.get_untracked());
        if secret
            .as_ref()
            .is_some_and(|p| p.chars().count() < MIN_PASSWORD_LEN)
        {
            error.set(Some(format!(
                "The staff password has at least {MIN_PASSWORD_LEN} characters."
            )));
            return;
        }
        busy.set(true);
        error.set(None);
        spawn_local(async move {
            match api::unlock(secret).await {
                Ok(info) => {
                    // Cleared first: the new session unmounts this screen.
                    password.set(String::new());
                    session.set(info);
                }
                Err(failure) => {
                    busy.set(false);
                    if failure.code == "not_locked" {
                        // Unlocked elsewhere in the meantime.
                        recheck(session);
                    } else {
                        if failure.code == "present_active" {
                            recheck(session);
                        }
                        error.set(Some(unlock_message(&failure)));
                    }
                }
            }
        });
    };
    let on_submit = move |ev: SubmitEvent| {
        ev.prevent_default();
        unlock();
    };

    let sign_out = move |_| {
        busy.set(true);
        spawn_local(async move {
            let _ = api::logout().await;
            busy.set(false);
            session.reload();
        });
    };

    let sign_out_area = move || {
        if confirming.get() {
            let warning = if demo() {
                "Signing out ends this demo visit. Its presentations can't be opened again."
            } else {
                "Signing out ends the staff session on this browser. Sign in again with the staff password."
            };
            view! {
                <div class="lock-confirm" role="group" aria-labelledby="lock-confirm-text">
                    <p id="lock-confirm-text" class="lock-confirm-text">{warning}</p>
                    <div class="lock-confirm-actions">
                        <button type="button" class="button small" disabled=move || busy.get()
                            on:click=sign_out>"Sign out"</button>
                        <button type="button" class="button small text"
                            on:click=move |_| confirming.set(false)>"Cancel"</button>
                    </div>
                </div>
            }
            .into_any()
        } else {
            view! {
                <button type="button" class="button text lock-sign-out"
                    on:click=move |_| confirming.set(true)>"Sign out"</button>
            }
            .into_any()
        }
    };

    // Only a change between live and ended rebuilds the card, so its enter
    // animation marks the change and typing is never interrupted.
    move || {
        let practice_name = practice_name.clone();
        if ended() {
            let body = if demo() {
                view! {
                    <p class="auth-text">
                        "The patient screen was closed or timed out. Unlock to go back to your workspace."
                    </p>
                    <FormError error />
                    <button type="button" class="button primary" disabled=move || busy.get()
                        on:click=move |_| unlock()>
                        {move || if busy.get() { "Unlocking…" } else { "Unlock" }}
                    </button>
                }
                .into_any()
            } else {
                view! {
                    <p class="auth-text">
                        "The patient screen was closed or timed out. Enter the staff password to go back to your workspace."
                    </p>
                    <form class="form" on:submit=on_submit>
                        <label class="field">
                            <span>"Staff password"</span>
                            <input type="password" autocomplete="current-password" required
                                minlength=MIN_PASSWORD_LEN.to_string() bind:value=password />
                            <small>"The PIN only works from the patient screen."</small>
                        </label>
                        <FormError error />
                        <button class="button primary" type="submit" disabled=move || busy.get()>
                            {move || if busy.get() { "Unlocking…" } else { "Unlock" }}
                        </button>
                    </form>
                }
                .into_any()
            };
            view! {
                <AuthCard title="The presentation has ended" subtitle=practice_name>
                    <div class="lock-body">
                        {body}
                        {sign_out_area}
                    </div>
                </AuthCard>
            }
            .into_any()
        } else {
            view! {
                <AuthCard title="Presenting to a patient" subtitle=practice_name>
                    <div class="lock-body">
                        <p class="auth-text">
                            "This browser is showing a presentation to a patient, so the staff workspace is locked."
                        </p>
                        <a class="button primary" href="/present"
                            on:click=move |ev| {
                                ev.prevent_default();
                                return_to_present();
                            }>"Return to presentation"</a>
                        {sign_out_area}
                    </div>
                </AuthCard>
            }
            .into_any()
        }
    }
}
