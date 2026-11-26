//! Line icons drawn on a 24×24 grid. Stroke color comes from CSS.

use leptos::prelude::*;

#[component]
pub fn Icon(path: &'static str) -> impl IntoView {
    view! {
        <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke-width="1.8"
            stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d=path />
        </svg>
    }
}

pub const HOME: &str = "M3 11l9-7 9 7v9a1 1 0 0 1-1 1h-5v-6H9v6H4a1 1 0 0 1-1-1z";
pub const PATIENTS: &str = "M5.5 8a3.5 3.5 0 1 0 7 0a3.5 3.5 0 1 0-7 0M2.5 20c.8-3.5 3.4-5.5 6.5-5.5s5.7 2 6.5 5.5M16 4.5a3.5 3.5 0 0 1 0 7M18.5 14.8c1.6.8 2.6 2.6 3 5.2";
pub const PRESENTATIONS: &str = "M5 4h14a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2zM12 17v3M8 21h8M10 8.5v5l4-2.5z";
pub const ANALYTICS: &str = "M4 20V10M10 20V4M16 20v-7M22 20H2";
pub const SETTINGS: &str = "M12 9a3 3 0 1 0 0 6a3 3 0 1 0 0-6M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-2.9 1.2V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-2.9-1.2l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1A1.7 1.7 0 0 0 3 14H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.2-2.9l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1A1.7 1.7 0 0 0 10 3V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 2.9 1.2l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1A1.7 1.7 0 0 0 21 10h.1a2 2 0 1 1 0 4H21a1.7 1.7 0 0 0-1.6 1z";
pub const SIGN_OUT: &str = "M15 4h3a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2h-3M10 17l-5-5 5-5M5 12h11";

/// The DentalClarity mark from `assets/brand/logo-mark.svg`: a molar whose
/// root becomes a speech bubble tail. Fill color comes from CSS.
const LOGO_MARK: &str = "M21 7C13.5 7 8 12.5 8 20.5C8 27 10 32 12 36.5C12.5 44 10.5 51 7 57.5C17 54.5 22.8 49.5 25.6 42.2C26.8 39.8 29 38.5 32 38.5C35 38.5 37.5 40 38.5 43L41.5 53.5C42.4 56.6 46.6 56.6 47.3 53.4L50.5 39.5C53.5 33 56 27.5 56 20.5C56 12.5 50.5 7 43 7C38.5 7 36 9.5 32 9.5C28 9.5 25.5 7 21 7Z M16 23a4 4 0 1 0 8 0a4 4 0 1 0-8 0ZM28 23a4 4 0 1 0 8 0a4 4 0 1 0-8 0ZM40 23a4 4 0 1 0 8 0a4 4 0 1 0-8 0Z";

/// Mark and wordmark as in `assets/brand/logo.svg`: "dental" in navy,
/// "clarity" and the mark in dental blue. Used on every staff screen.
#[component]
pub fn Logo() -> impl IntoView {
    view! {
        <svg class="logo-mark" viewBox="0 0 64 64" aria-hidden="true">
            <path fill-rule="evenodd" d=LOGO_MARK />
        </svg>
        <span class="wordmark-text">"dental"<span class="wordmark-accent">"clarity"</span></span>
    }
}
