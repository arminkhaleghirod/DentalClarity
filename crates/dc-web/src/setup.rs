//! The first-launch setup wizard (UI_SPEC 5.8, DECISIONS E27).
//!
//! Five steps collect everything in the browser: the setup code with a
//! connection check, practice name and colors, an optional logo, the staff
//! account, and a review with the three acknowledgments. Nothing is stored
//! until the last step, which sends one `POST /api/setup`; the logo is then
//! uploaded with the new staff session.

use crate::api;
use crate::icons::Logo;
use crate::logo::{ACCEPT, chosen_file, precheck, size_label};
use crate::session::use_session;
use crate::settings::{ColorField, Preview};
use dc_core::api::{DEFAULT_BRAND_HIGHLIGHT, DEFAULT_BRAND_PRIMARY, SessionInfo, SetupRequest};
use dc_core::color;
use dc_core::setup::{ConnectionCheck, SetupCheckResponse};
use leptos::prelude::*;
use leptos::task::spawn_local;
use web_sys::File;

/// The shortest staff password the server accepts.
const MIN_PASSWORD_LEN: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Step {
    Connect,
    Practice,
    Logo,
    Account,
    Review,
}

impl Step {
    const ALL: [Step; 5] = [
        Step::Connect,
        Step::Practice,
        Step::Logo,
        Step::Account,
        Step::Review,
    ];

    fn label(self) -> &'static str {
        match self {
            Step::Connect => "Setup code and connections",
            Step::Practice => "Practice",
            Step::Logo => "Logo",
            Step::Account => "Staff account",
            Step::Review => "Review and confirm",
        }
    }

    fn index(self) -> usize {
        Step::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }

    fn next(self) -> Option<Step> {
        Step::ALL.get(self.index() + 1).copied()
    }

    fn previous(self) -> Option<Step> {
        self.index().checked_sub(1).map(|i| Step::ALL[i])
    }
}

/// Whether a PIN has the 6 to 8 digits the server accepts.
fn pin_shape_ok(pin: &str) -> bool {
    (6..=8).contains(&pin.len()) && pin.bytes().all(|b| b.is_ascii_digit())
}

fn hex_ok(value: &str) -> bool {
    color::parse_hex(value.trim()).is_some()
}

fn contrast_of(primary: &str) -> Option<f64> {
    color::parse_hex(primary.trim()).map(color::contrast_with_white)
}

/// Everything the wizard collects. One copy, shared by every step.
#[derive(Clone, Copy)]
struct Form {
    code: RwSignal<String>,
    checked: RwSignal<Option<SetupCheckResponse>>,
    name: RwSignal<String>,
    primary: RwSignal<String>,
    highlight: RwSignal<String>,
    logo: RwSignal<Option<File>, LocalStorage>,
    password: RwSignal<String>,
    confirm: RwSignal<String>,
    pin: RwSignal<String>,
    pin_confirm: RwSignal<String>,
    ack_network: RwSignal<bool>,
    ack_terms: RwSignal<bool>,
    ack_readiness: RwSignal<bool>,
}

impl Form {
    fn new() -> Self {
        Self {
            code: RwSignal::new(String::new()),
            checked: RwSignal::new(None),
            name: RwSignal::new(String::new()),
            primary: RwSignal::new(DEFAULT_BRAND_PRIMARY.into()),
            highlight: RwSignal::new(DEFAULT_BRAND_HIGHLIGHT.into()),
            logo: RwSignal::new_local(None),
            password: RwSignal::new(String::new()),
            confirm: RwSignal::new(String::new()),
            pin: RwSignal::new(String::new()),
            pin_confirm: RwSignal::new(String::new()),
            ack_network: RwSignal::new(false),
            ack_terms: RwSignal::new(false),
            ack_readiness: RwSignal::new(false),
        }
    }

    /// The first problem that keeps `step` from being complete, if any.
    fn blocker(self, step: Step) -> Option<&'static str> {
        match step {
            Step::Connect => self
                .checked
                .with(Option::is_none)
                .then_some("Check the setup code to continue."),
            Step::Practice => {
                let name = self.name.get();
                let name = name.trim();
                if name.is_empty() || name.chars().count() > 100 {
                    Some("Enter the practice name (up to 100 characters).")
                } else if !hex_ok(&self.primary.get()) || !hex_ok(&self.highlight.get()) {
                    Some("Enter both colors as #RRGGBB.")
                } else if contrast_of(&self.primary.get())
                    .is_none_or(|r| r < color::MIN_CONTRAST_ON_PRIMARY)
                {
                    Some("Choose a darker brand color so white text stays readable.")
                } else {
                    None
                }
            }
            Step::Logo => None,
            Step::Account => {
                let password = self.password.get();
                if password.chars().count() < MIN_PASSWORD_LEN {
                    Some("The staff password needs at least 12 characters.")
                } else if password != self.confirm.get() {
                    Some("The two passwords don't match.")
                } else if !pin_shape_ok(&self.pin.get()) {
                    Some("The staff PIN needs 6 to 8 digits.")
                } else if self.pin.get() != self.pin_confirm.get() {
                    Some("The two PINs don't match.")
                } else {
                    None
                }
            }
            Step::Review => {
                (!(self.ack_network.get() && self.ack_terms.get() && self.ack_readiness.get()))
                    .then_some("Tick all three boxes to finish setup.")
            }
        }
    }

    fn request(self) -> SetupRequest {
        SetupRequest {
            setup_code: self.code.get_untracked(),
            practice_name: self.name.get_untracked(),
            password: self.password.get_untracked(),
            pin: self.pin.get_untracked(),
            brand_primary: Some(self.primary.get_untracked()),
            brand_highlight: Some(self.highlight.get_untracked()),
            acknowledged: true,
        }
    }
}

#[component]
pub fn SetupWizard() -> impl IntoView {
    let session = use_session();
    let form = Form::new();
    let step = RwSignal::new(Step::Connect);
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    // Set when the account exists but the logo upload failed.
    let finished = RwSignal::new(None::<(SessionInfo, String)>);

    let go = move |to: Step| {
        error.set(None);
        step.set(to);
        crate::motion::focus_page_content("setup-step");
    };

    let finish = move || {
        busy.set(true);
        error.set(None);
        let request = form.request();
        let logo = form.logo.get_untracked();
        spawn_local(async move {
            match api::setup(&request).await {
                Ok(info) => {
                    let upload = match logo {
                        Some(file) => api::upload_logo(file).await.err(),
                        None => None,
                    };
                    match upload {
                        None => session.set(info),
                        Some(failure) => finished.set(Some((info, failure.message))),
                    }
                }
                Err(failure) if failure.code == "bad_setup_code" => {
                    form.checked.set(None);
                    go(Step::Connect);
                    error.set(Some(failure.message));
                }
                Err(failure) => error.set(Some(failure.message)),
            }
            busy.set(false);
        });
    };

    let on_continue = move |_| {
        let current = step.get_untracked();
        if let Some(problem) = form.blocker(current) {
            error.set(Some(problem.into()));
            return;
        }
        match current.next() {
            Some(next) => go(next),
            None => finish(),
        }
    };

    let stepper = Step::ALL
        .into_iter()
        .map(|s| {
            let class = move || {
                let current = step.get();
                if s == current {
                    "stage-row active"
                } else if s < current {
                    "stage-row passed"
                } else {
                    "stage-row queued"
                }
            };
            view! {
                <li class=class aria-current=move || (step.get() == s).then_some("step")>
                    <span class="stage-icon" aria-hidden="true">
                        {move || match step.get() {
                            c if c == s => view! { <span class="stage-dot"></span> }.into_any(),
                            c if s < c => "✓".into_any(),
                            _ => "".into_any(),
                        }}
                    </span>
                    <span class="stage-label">{format!("{}. {}", s.index() + 1, s.label())}</span>
                </li>
            }
        })
        .collect_view();

    let body = move || match step.get() {
        Step::Connect => view! { <ConnectStep form error /> }.into_any(),
        Step::Practice => view! { <PracticeStep form /> }.into_any(),
        Step::Logo => view! { <LogoStep form /> }.into_any(),
        Step::Account => view! { <AccountStep form /> }.into_any(),
        Step::Review => view! { <ReviewStep form /> }.into_any(),
    };

    let continue_label = move || match (step.get(), busy.get()) {
        (Step::Review, true) => "Setting up…",
        (Step::Review, false) => "Finish setup",
        (Step::Logo, _) if form.logo.with(Option::is_none) => "Skip for now",
        _ => "Continue",
    };

    view! {
            <div class="wizard" hidden=move || finished.with(Option::is_some)>
                <aside class="wizard-steps">
                    <div class="wordmark"><Logo /></div>
                    <h1 class="wizard-title">"Set up DentalClarity"</h1>
                    <ol class="stage-list" aria-label="Setup steps">{stepper}</ol>
                    <p class="auth-note">
                        "One shared account is a prototype limit. Real clinical use needs individual staff accounts; see the README."
                    </p>
                </aside>
                <main class="wizard-main">
                    <form class="wizard-form" novalidate on:submit=move |ev| { ev.prevent_default(); on_continue(()); }>
                        <section id="setup-step" class="wizard-step" tabindex="-1" aria-labelledby="setup-step-title">
                            <h2 id="setup-step-title" class="card-title">{move || step.get().label()}</h2>
                            {move || error.get().map(|m| view! { <p class="form-error" role="alert">{m}</p> })}
                            {body}
                        </section>
                        <div class="wizard-footer">
                            <button type="button" class="button"
                                disabled=move || busy.get() || step.get().previous().is_none()
                                on:click=move |_| if let Some(p) = step.get_untracked().previous() { go(p) }>
                                "Back"
                            </button>
                            <button type="submit" class="button primary"
                                disabled=move || busy.get() || form.blocker(step.get()).is_some()>
                                {continue_label}
                            </button>
                        </div>
                    </form>
                </main>
            </div>
        {move || finished.with(Option::is_some).then(|| view! { <LogoFailed finished /> })}
    }
}

#[component]
fn ConnectStep(form: Form, error: RwSignal<Option<String>>) -> impl IntoView {
    let checking = RwSignal::new(false);
    let run_check = move |_| {
        let code = form.code.get_untracked();
        if code.trim().is_empty() {
            error.set(Some("Enter the setup code first.".into()));
            return;
        }
        checking.set(true);
        error.set(None);
        spawn_local(async move {
            match api::setup_check(code).await {
                Ok(result) => form.checked.set(Some(result)),
                Err(failure) => {
                    form.checked.set(None);
                    error.set(Some(failure.message));
                }
            }
            checking.set(false);
        });
    };
    view! {
        <p class="auth-text">
            "The setup code is printed in the server console when DentalClarity first starts. Checking it also tests the connections the server was started with."
        </p>
        <label class="field">
            <span>"Setup code"</span>
            <input type="text" autocomplete="off" autocapitalize="characters" spellcheck="false"
                prop:value=move || form.code.get()
                on:input=move |ev| {
                    form.code.set(event_target_value(&ev));
                    // A changed code has not been checked.
                    form.checked.set(None);
                } />
        </label>
        <div class="card-actions wizard-inline-actions">
            <button type="button" class="button" disabled=move || checking.get() on:click=run_check>
                {move || if checking.get() { "Checking…" } else { "Check code and connections" }}
            </button>
        </div>
        {move || form.checked.get().map(|result| view! {
            <ul class="check-list" aria-label="Connection check">
                <CheckRow title="Practice records" check=result.practice_records />
                <CheckRow title="AI provider" check=result.ai_provider />
            </ul>
            <p class="auth-note">"Connection details are set in the server configuration. Settings will show them once they can be edited there."</p>
        })}
    }
}

#[component]
fn CheckRow(title: &'static str, check: ConnectionCheck) -> impl IntoView {
    let class = if check.ok {
        "check-row ok"
    } else {
        "check-row warn"
    };
    view! {
        <li class=class>
            <span class="check-title">{title}" · "{check.label}</span>
            <span class="check-message">{check.message}</span>
        </li>
    }
}

#[component]
fn PracticeStep(form: Form) -> impl IntoView {
    let contrast_text = move || match contrast_of(&form.primary.get()) {
        None => "Enter the brand color as #RRGGBB.".to_owned(),
        Some(r) if r >= color::MIN_CONTRAST_ON_PRIMARY => {
            format!("White text contrast {r:.1}:1. Passes the 4.5:1 minimum.")
        }
        Some(r) => {
            format!("White text contrast {r:.1}:1. Needs at least 4.5:1; choose a darker color.")
        }
    };
    let contrast_ok = move || {
        contrast_of(&form.primary.get()).is_some_and(|r| r >= color::MIN_CONTRAST_ON_PRIMARY)
    };
    view! {
        <label class="field">
            <span>"Practice name"</span>
            <input type="text" autocomplete="organization" maxlength="100"
                prop:value=move || form.name.get()
                on:input=move |ev| form.name.set(event_target_value(&ev)) />
            <small>"Shown on patient screens and the take-home booklet."</small>
        </label>
        <ColorField label="Brand color" id="setup-primary" value=form.primary
            hint="Used behind white text on patient screens." />
        <p class=move || if contrast_ok() { "contrast pass" } else { "contrast fail" } aria-live="polite">
            {contrast_text}
        </p>
        <ColorField label="Highlight color" id="setup-highlight" value=form.highlight
            hint="Marks the patient's own teeth." />
        <Preview name=form.name.into() primary=form.primary.into()
            highlight=form.highlight.into() logo=Signal::stored(None) />
    }
}

#[component]
fn LogoStep(form: Form) -> impl IntoView {
    let problem = RwSignal::new(None::<String>);
    let on_change = move |ev: leptos::ev::Event| {
        let Some(file) = chosen_file(&ev) else { return };
        match precheck(&file) {
            Ok(()) => {
                problem.set(None);
                form.logo.set(Some(file));
            }
            Err(message) => problem.set(Some(message)),
        }
    };
    let chosen = move || {
        form.logo.with(|f| {
            f.as_ref()
                .map(|f| format!("{} ({})", f.name(), size_label(f)))
        })
    };
    view! {
        <p class="auth-text">
            "Optional. The logo appears in the patient header and the staff sidebar. Without one, patient screens show the first letter of the practice name. You can change it later in Settings."
        </p>
        <div class="field">
            <label for="setup-logo">"Logo file"</label>
            <input id="setup-logo" type="file" accept=ACCEPT aria-describedby="setup-logo-hint"
                on:change=on_change />
            <small id="setup-logo-hint">"PNG, JPEG or SVG, up to 1 MB. SVG files with scripts, links or embedded images are refused."</small>
        </div>
        {move || chosen().map(|label| view! {
            <div class="logo-chosen">
                <span>"Chosen: "{label}</span>
                <button type="button" class="button small" on:click=move |_| form.logo.set(None)>"Remove"</button>
            </div>
        })}
        {move || problem.get().map(|m| view! { <p class="form-error" role="alert">{m}</p> })}
    }
}

#[component]
fn AccountStep(form: Form) -> impl IntoView {
    view! {
        <p class="auth-text">"One shared account signs staff in on every workstation."</p>
        <label class="field">
            <span>"Staff password"</span>
            <input type="password" autocomplete="new-password" minlength="12"
                prop:value=move || form.password.get()
                on:input=move |ev| form.password.set(event_target_value(&ev)) />
            <small>"At least 12 characters. A short phrase works well."</small>
        </label>
        <label class="field">
            <span>"Confirm password"</span>
            <input type="password" autocomplete="new-password"
                prop:value=move || form.confirm.get()
                on:input=move |ev| form.confirm.set(event_target_value(&ev)) />
        </label>
        <label class="field">
            <span>"Staff PIN"</span>
            <input type="password" inputmode="numeric" autocomplete="off" maxlength="8"
                prop:value=move || form.pin.get()
                on:input=move |ev| form.pin.set(event_target_value(&ev)) />
            <small>"6 to 8 digits. Used to leave Present mode after handing a patient the tablet."</small>
        </label>
        <label class="field">
            <span>"Confirm PIN"</span>
            <input type="password" inputmode="numeric" autocomplete="off" maxlength="8"
                prop:value=move || form.pin_confirm.get()
                on:input=move |ev| form.pin_confirm.set(event_target_value(&ev)) />
        </label>
    }
}

#[component]
fn ReviewStep(form: Form) -> impl IntoView {
    let logo = move || {
        form.logo
            .with(|f| f.as_ref().map_or_else(|| "None".to_owned(), |f| f.name()))
    };
    let provider = move || {
        form.checked.with(|c| {
            c.as_ref()
                .map(|c| c.ai_provider.label.clone())
                .unwrap_or_default()
        })
    };
    let records = move || {
        form.checked.with(|c| {
            c.as_ref()
                .map(|c| c.practice_records.label.clone())
                .unwrap_or_default()
        })
    };
    view! {
        <dl class="settings-values">
            <dt>"Practice name"</dt><dd>{move || form.name.get().trim().to_owned()}</dd>
            <dt>"Brand color"</dt><dd class="mono">{move || form.primary.get().to_ascii_uppercase()}</dd>
            <dt>"Highlight color"</dt><dd class="mono">{move || form.highlight.get().to_ascii_uppercase()}</dd>
            <dt>"Logo"</dt><dd>{logo}</dd>
            <dt>"Practice records"</dt><dd>{records}</dd>
            <dt>"AI provider"</dt><dd>{provider}</dd>
            <dt>"Staff account"</dt><dd>"Password and PIN set"</dd>
        </dl>
        <fieldset class="ack-list">
            <legend>"Before you finish"</legend>
            <label class="ack">
                <input type="checkbox" prop:checked=move || form.ack_network.get()
                    on:change=move |ev| form.ack_network.set(event_target_checked(&ev)) />
                <span>"DentalClarity runs only on the practice's local network and is not exposed to the internet."</span>
            </label>
            <label class="ack">
                <input type="checkbox" prop:checked=move || form.ack_terms.get()
                    on:change=move |ev| form.ack_terms.set(event_target_checked(&ev)) />
                <span>"The practice is responsible for its agreements and terms with the AI provider it configures."</span>
            </label>
            <label class="ack">
                <input type="checkbox" prop:checked=move || form.ack_readiness.get()
                    on:change=move |ev| form.ack_readiness.set(event_target_checked(&ev)) />
                <span>"I have read the clinical-readiness notice in the README. This prototype is not ready for real patient data."</span>
            </label>
        </fieldset>
    }
}

/// Shown when the account was created but the logo upload failed.
#[component]
fn LogoFailed(finished: RwSignal<Option<(SessionInfo, String)>>) -> impl IntoView {
    let session = use_session();
    let message = move || finished.with(|f| f.as_ref().map(|(_, m)| m.clone()).unwrap_or_default());
    let go_on = move |_| {
        if let Some((info, _)) = finished.get_untracked() {
            session.set(info);
        }
    };
    view! {
        <div class="auth-page">
            <div class="auth-card">
                <div class="wordmark"><Logo /></div>
                <h1>"Setup is complete"</h1>
                <p class="auth-subtitle">"The staff account is ready, but the logo was not saved."</p>
                <p class="form-error" role="alert">{message}</p>
                <p class="auth-text">"You can upload the logo later under Settings, Practice and branding."</p>
                <button type="button" class="button primary" on:click=go_on>"Continue to DentalClarity"</button>
            </div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_run_in_order() {
        assert_eq!(Step::Connect.next(), Some(Step::Practice));
        assert_eq!(Step::Review.next(), None);
        assert_eq!(Step::Connect.previous(), None);
        assert_eq!(Step::Account.previous(), Some(Step::Logo));
    }

    #[test]
    fn pin_shape_matches_the_server_rule() {
        assert!(pin_shape_ok("480913"));
        assert!(pin_shape_ok("48091312"));
        assert!(!pin_shape_ok("4809"));
        assert!(!pin_shape_ok("48091a"));
        assert!(!pin_shape_ok("480913120"));
    }
}
