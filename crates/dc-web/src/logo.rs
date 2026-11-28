//! The practice logo in the staff app: the sidebar mark, the upload field
//! in Settings, and the file checks shared with the setup wizard.
//!
//! The server decides what a file is from its bytes (DECISIONS E24). The
//! checks here only save a round trip for files that are plainly wrong.

use crate::api;
use dc_core::Mode;
use leptos::prelude::*;
use leptos::task::spawn_local;
use web_sys::File;

/// The `accept` list for logo file inputs.
pub const ACCEPT: &str = "image/png,image/jpeg,image/svg+xml,.png,.jpg,.jpeg,.svg";
const MAX_BYTES: f64 = 1024.0 * 1024.0;

/// The current logo URL for staff screens, shared by the shell and Settings.
#[derive(Clone, Copy)]
pub struct ShellLogo(pub RwSignal<Option<String>>);

/// Provides [`ShellLogo`] and, outside the demo, loads it from the server.
/// Demo visitors cannot read settings, and the demo has no logo.
pub fn provide_shell_logo(mode: Option<Mode>) -> ShellLogo {
    let logo = ShellLogo(RwSignal::new(None));
    provide_context(logo);
    if mode == Some(Mode::Clinical) {
        spawn_local(async move {
            if let Ok(settings) = api::settings().await {
                let _ = logo.0.try_set(settings.logo_url);
            }
        });
    }
    logo
}

pub fn use_shell_logo() -> Option<ShellLogo> {
    use_context::<ShellLogo>()
}

/// The first file chosen in a file input.
pub fn chosen_file(ev: &leptos::ev::Event) -> Option<File> {
    let input = event_target::<web_sys::HtmlInputElement>(ev);
    input.files().and_then(|files| files.get(0))
}

/// A quick check before upload. The server repeats every check on the bytes.
pub fn precheck(file: &File) -> Result<(), String> {
    if file.size() > MAX_BYTES {
        return Err("The logo must be 1 MB or smaller.".into());
    }
    let kind = file.type_();
    if !kind.is_empty() && !matches!(kind.as_str(), "image/png" | "image/jpeg" | "image/svg+xml") {
        return Err("Upload the logo as a PNG, JPEG or SVG file.".into());
    }
    Ok(())
}

/// File size for display, such as "240 KB".
pub fn size_label(file: &File) -> String {
    let kb = (file.size() / 1024.0).ceil();
    if kb >= 1024.0 {
        format!("{:.1} MB", kb / 1024.0)
    } else {
        format!("{kb} KB")
    }
}

/// The practice's mark: its logo when there is one, else the name's first
/// letter. Decorative; the practice name is always shown beside it.
#[component]
pub fn PracticeMark(
    #[prop(into)] name: Signal<String>,
    #[prop(into)] logo: Signal<Option<String>>,
    class: &'static str,
) -> impl IntoView {
    let initial = move || {
        name.get()
            .chars()
            .next()
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_default()
    };
    move || match logo.get() {
        Some(src) => view! { <img class=format!("{class} has-image") src=src alt="" /> }.into_any(),
        None => view! { <span class=class aria-hidden="true">{initial}</span> }.into_any(),
    }
}

/// The logo field in Settings: shows the current logo, uploads a new one at
/// once and can remove it.
#[component]
pub fn LogoSettings(logo: RwSignal<Option<String>>) -> impl IntoView {
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let status = RwSignal::new(None::<&'static str>);
    let shell = use_shell_logo();
    let input = NodeRef::<leptos::html::Input>::new();

    let apply = move |url: Option<String>| {
        logo.set(url.clone());
        if let Some(shell) = shell {
            shell.0.set(url);
        }
    };

    let on_change = move |ev: leptos::ev::Event| {
        let Some(file) = chosen_file(&ev) else { return };
        if let Err(message) = precheck(&file) {
            error.set(Some(message));
            return;
        }
        busy.set(true);
        error.set(None);
        status.set(None);
        spawn_local(async move {
            match api::upload_logo(file).await {
                Ok(settings) => {
                    apply(settings.logo_url);
                    status.set(Some("Logo saved."));
                }
                Err(failure) => error.set(Some(failure.message)),
            }
            busy.set(false);
            // Choosing the same file again should upload it again.
            if let Some(el) = input.get_untracked() {
                el.set_value("");
            }
        });
    };

    let on_remove = move |_| {
        busy.set(true);
        error.set(None);
        status.set(None);
        spawn_local(async move {
            match api::remove_logo().await {
                Ok(settings) => {
                    apply(settings.logo_url);
                    status.set(Some("Logo removed."));
                }
                Err(failure) => error.set(Some(failure.message)),
            }
            busy.set(false);
        });
    };

    view! {
        <div class="field logo-field">
            <label for="logo-file">"Logo"</label>
            <div class="logo-row">
                <div class="logo-frame">
                    {move || match logo.get() {
                        Some(src) => view! { <img class="logo-current" src=src alt="Current practice logo" /> }.into_any(),
                        None => view! { <span class="logo-empty">"No logo"</span> }.into_any(),
                    }}
                </div>
                <div class="logo-actions">
                    <input id="logo-file" type="file" accept=ACCEPT node_ref=input
                        aria-describedby="logo-hint" disabled=move || busy.get() on:change=on_change />
                    <Show when=move || logo.get().is_some()>
                        <button type="button" class="button small" disabled=move || busy.get() on:click=on_remove>
                            "Remove logo"
                        </button>
                    </Show>
                </div>
            </div>
            <small id="logo-hint">"PNG, JPEG or SVG, up to 1 MB. Saved as soon as you choose a file. Patient screens show the first letter of the practice name when there is no logo."</small>
            <Show when=move || busy.get()>
                <span class="saved" role="status">"Uploading…"</span>
            </Show>
            {move || status.get().map(|s| view! { <span class="saved" role="status">{s}</span> })}
            {move || error.get().map(|m| view! { <p class="form-error" role="alert">{m}</p> })}
        </div>
    }
}
