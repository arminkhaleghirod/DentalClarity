//! Practice and branding settings.

use crate::api;
use crate::motion::SkeletonCard;
use crate::session::use_session;
use dc_core::Mode;
use dc_core::api::{DEFAULT_BRAND_HIGHLIGHT, DEFAULT_BRAND_PRIMARY, PracticeSettings};
use dc_core::color;
use leptos::ev::SubmitEvent;
use leptos::prelude::*;
use leptos::task::spawn_local;

#[component]
pub fn SettingsPage() -> impl IntoView {
    let session = use_session();
    let info = session.info.get_untracked().and_then(Result::ok);
    let is_demo = info.as_ref().is_some_and(|i| i.mode == Mode::Demo);
    let header = view! { <header class="topbar"><h1 class="page-title">"Settings"</h1></header> };

    if is_demo {
        // Demo visitors cannot read settings from the server; the demo always
        // runs with the default branding.
        let values = PracticeSettings {
            practice_name: info.map(|i| i.practice_name).unwrap_or_default(),
            brand_primary: DEFAULT_BRAND_PRIMARY.into(),
            brand_highlight: DEFAULT_BRAND_HIGHLIGHT.into(),
        };
        return view! {
            {header}
            <section class="card settings-card reveal" aria-labelledby="branding-title">
                <h2 id="branding-title" class="card-title">"Practice and branding"</h2>
                <p class="notice">"Settings are read-only in the demo."</p>
                <dl class="settings-values">
                    <dt>"Practice name"</dt><dd>{values.practice_name.clone()}</dd>
                    <dt>"Brand color"</dt><dd class="mono">{values.brand_primary.clone()}</dd>
                    <dt>"Highlight color"</dt><dd class="mono">{values.brand_highlight.clone()}</dd>
                </dl>
                <Preview
                    name=Signal::stored(values.practice_name)
                    primary=Signal::stored(values.brand_primary)
                    highlight=Signal::stored(values.brand_highlight)
                />
            </section>
        }
        .into_any();
    }

    let loaded = LocalResource::new(api::settings);
    view! {
        {header}
        <Suspense fallback=|| view! { <SkeletonCard label="Loading settings" lines=7 class="settings-card" /> }>
            {move || loaded.get().map(|result| match result {
                Ok(values) => view! { <BrandingForm values /> }.into_any(),
                Err(failure) => view! {
                    <div class="notice error reveal" role="alert">"Could not load settings. " {failure.message}</div>
                }.into_any(),
            })}
        </Suspense>
    }
    .into_any()
}

/// Lower-case `#rrggbb` for `<input type="color">`, which accepts nothing else.
fn picker_value(hex: &str) -> String {
    match color::parse_hex(hex.trim()) {
        Some(_) => hex.trim().to_ascii_lowercase(),
        None => "#000000".into(),
    }
}

#[component]
fn BrandingForm(values: PracticeSettings) -> impl IntoView {
    let session = use_session();
    let name = RwSignal::new(values.practice_name);
    let primary = RwSignal::new(values.brand_primary);
    let highlight = RwSignal::new(values.brand_highlight);
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let saved = RwSignal::new(false);

    let contrast = move || color::parse_hex(primary.get().trim()).map(color::contrast_with_white);
    let contrast_text = move || match contrast() {
        None => "Enter the brand color as #RRGGBB.".to_owned(),
        Some(r) if r >= color::MIN_CONTRAST_ON_PRIMARY => {
            format!("White text contrast {r:.1}:1. Passes the 4.5:1 minimum.")
        }
        Some(r) => {
            format!("White text contrast {r:.1}:1. Needs at least 4.5:1; choose a darker color.")
        }
    };
    let contrast_ok = move || contrast().is_some_and(|r| r >= color::MIN_CONTRAST_ON_PRIMARY);

    let on_submit = move |ev: SubmitEvent| {
        ev.prevent_default();
        let request = PracticeSettings {
            practice_name: name.get_untracked(),
            brand_primary: primary.get_untracked(),
            brand_highlight: highlight.get_untracked(),
        };
        busy.set(true);
        error.set(None);
        saved.set(false);
        spawn_local(async move {
            match api::save_settings(&request).await {
                Ok(stored) => {
                    name.set(stored.practice_name.clone());
                    primary.set(stored.brand_primary);
                    highlight.set(stored.brand_highlight);
                    session.info.update(|info| {
                        if let Some(Ok(info)) = info {
                            info.practice_name = stored.practice_name;
                        }
                    });
                    saved.set(true);
                }
                Err(failure) => error.set(Some(failure.message)),
            }
            busy.set(false);
        });
    };

    view! {
        <section class="card settings-card reveal" aria-labelledby="branding-title">
            <h2 id="branding-title" class="card-title">"Practice and branding"</h2>
            <form class="form settings-form" on:submit=on_submit>
                <label class="field">
                    "Practice name"
                    <input
                        type="text"
                        required
                        maxlength="100"
                        prop:value=move || name.get()
                        on:input=move |ev| name.set(event_target_value(&ev))
                    />
                </label>
                <ColorField label="Brand color" id="brand-primary" value=primary
                    hint="Used behind white text on patient screens." />
                <p class=move || if contrast_ok() { "contrast pass" } else { "contrast fail" } aria-live="polite">
                    {contrast_text}
                </p>
                <ColorField label="Highlight color" id="brand-highlight" value=highlight
                    hint="Marks the patient's own teeth." />
                <Preview name=name.into() primary=primary.into() highlight=highlight.into() />
                <Show when=move || error.get().is_some()>
                    <p class="form-error" role="alert">{move || error.get().unwrap_or_default()}</p>
                </Show>
                <div class="card-actions">
                    <Show when=move || saved.get()>
                        <span class="saved" role="status">"Saved."</span>
                    </Show>
                    <button class="button primary" type="submit" disabled=move || busy.get()>
                        {move || if busy.get() { "Saving…" } else { "Save" }}
                    </button>
                </div>
            </form>
        </section>
    }
}

#[component]
fn ColorField(
    label: &'static str,
    id: &'static str,
    value: RwSignal<String>,
    hint: &'static str,
) -> impl IntoView {
    let hint_id = format!("{id}-hint");
    view! {
        <div class="field">
            <label for=id>{label}</label>
            <div class="color-row">
                <input
                    type="color"
                    class="color-picker"
                    aria-label=format!("{label} picker")
                    prop:value=move || picker_value(&value.get())
                    on:input=move |ev| value.set(event_target_value(&ev).to_ascii_uppercase())
                />
                <input
                    id=id
                    type="text"
                    class="mono hex-input"
                    maxlength="7"
                    spellcheck="false"
                    autocomplete="off"
                    aria-describedby=hint_id.clone()
                    prop:value=move || value.get()
                    on:input=move |ev| value.set(event_target_value(&ev))
                />
            </div>
            <small id=hint_id>{hint}</small>
        </div>
    }
}

/// The patient header as it will look with these settings.
#[component]
fn Preview(
    name: Signal<String>,
    primary: Signal<String>,
    highlight: Signal<String>,
) -> impl IntoView {
    let background = move || {
        let p = primary.get();
        color::parse_hex(p.trim())
            .map(|_| p.trim().to_owned())
            .unwrap_or_else(|| DEFAULT_BRAND_PRIMARY.into())
    };
    let accent = move || {
        let h = highlight.get();
        color::parse_hex(h.trim())
            .map(|_| h.trim().to_owned())
            .unwrap_or_else(|| DEFAULT_BRAND_HIGHLIGHT.into())
    };
    view! {
        <figure class="brand-preview">
            <figcaption>"Preview of the patient header"</figcaption>
            <div class="preview-bar" style:background-color=background>
                <span class="preview-logo" aria-hidden="true">
                    {move || name.get().chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default()}
                </span>
                <span class="preview-name">{move || name.get()}</span>
            </div>
            <div class="preview-body">
                <span class="preview-tooth" style:background-color=accent aria-hidden="true"></span>
                <span>"Your teeth are shown in the highlight color."</span>
            </div>
        </figure>
    }
}
