//! The session gate: setup, sign-in and lock screens shown before the workspace.

use crate::api;
use dc_core::api::{SessionInfo, SessionState, SetupRequest};
use leptos::ev::SubmitEvent;
use leptos::prelude::*;
use leptos::task::spawn_local;

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

#[component]
pub fn Locked(practice_name: String) -> impl IntoView {
    let session = use_session();
    let sign_out = move |_| {
        spawn_local(async move {
            let _ = api::logout().await;
            session.reload();
        });
    };
    view! {
        <AuthCard title="Presenting to a patient" subtitle=practice_name>
            <p class="auth-text">
                "This browser is showing a presentation to a patient, so the staff workspace is locked."
            </p>
            <button class="button" on:click=sign_out>"Sign out"</button>
        </AuthCard>
    }
}
