//! Display helpers shared by staff pages.

use dc_core::Money;
use leptos::prelude::*;
use leptos::wasm_bindgen::JsValue;

/// English name for the language tags the app currently recognizes.
/// Unknown tags are shown as-is rather than guessed.
pub fn language_name(tag: &str) -> String {
    let name = match tag.split('-').next().unwrap_or(tag) {
        "en" => "English",
        "es" => "Spanish",
        "vi" => "Vietnamese",
        "zh" => "Chinese",
        "tl" => "Tagalog",
        "ar" => "Arabic",
        "fr" => "French",
        "ko" => "Korean",
        "ru" => "Russian",
        "pt" => "Portuguese",
        _ => return tag.to_owned(),
    };
    name.to_owned()
}

/// Languages offered when creating a presentation, as (tag, label).
pub const PRESENTATION_LANGUAGES: [(&str, &str); 10] = [
    ("en", "English"),
    ("es", "Español · Spanish"),
    ("vi", "Tiếng Việt · Vietnamese"),
    ("zh", "中文 · Chinese"),
    ("tl", "Tagalog"),
    ("ar", "العربية · Arabic"),
    ("fr", "Français · French"),
    ("ko", "한국어 · Korean"),
    ("ru", "Русский · Russian"),
    ("pt", "Português · Portuguese"),
];

/// The offered language matching a patient's recorded preference, else English.
pub fn default_presentation_language(preferred: Option<&str>) -> &'static str {
    let base = preferred
        .and_then(|tag| tag.split('-').next())
        .unwrap_or("en")
        .to_ascii_lowercase();
    PRESENTATION_LANGUAGES
        .iter()
        .map(|(tag, _)| *tag)
        .find(|tag| *tag == base)
        .unwrap_or("en")
}

/// A money cell; missing amounts say so instead of showing a number.
pub fn money_view(value: Option<Money>) -> AnyView {
    match value {
        Some(m) => m.to_string().into_any(),
        None => view! { <span class="unknown">"Not available"</span> }.into_any(),
    }
}

/// Tooth label for staff screens: "#30" for a tooth number or letter, the
/// area as written for anything else, empty when there is no tooth.
pub fn tooth_label(tooth: &str) -> String {
    let t = tooth.trim();
    if t.is_empty() {
        String::new()
    } else if t.len() <= 2 && t.chars().all(|c| c.is_ascii_alphanumeric()) {
        format!("#{t}")
    } else {
        t.to_owned()
    }
}

/// Elapsed time as "0:48" or "12:05".
pub fn elapsed(ms: u64) -> String {
    let secs = ms / 1000;
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// A server timestamp as "Today 8:21 AM" or "Nov 18 8:21 AM". Values the
/// browser cannot parse are shown as-is.
pub fn timestamp(value: &str) -> String {
    let millis = js_sys::Date::parse(value);
    if millis.is_nan() {
        return value.to_owned();
    }
    let date = js_sys::Date::new(&JsValue::from_f64(millis));
    let time_options = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&time_options, &"hour".into(), &"numeric".into());
    let _ = js_sys::Reflect::set(&time_options, &"minute".into(), &"2-digit".into());
    let time = String::from(date.to_locale_time_string_with_options("en-US", &time_options));
    let today = js_sys::Date::new_0();
    let same_day = date.get_full_year() == today.get_full_year()
        && date.get_month() == today.get_month()
        && date.get_date() == today.get_date();
    if same_day {
        return format!("Today {time}");
    }
    let day_options = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&day_options, &"month".into(), &"short".into());
    let _ = js_sys::Reflect::set(&day_options, &"day".into(), &"numeric".into());
    if date.get_full_year() != today.get_full_year() {
        let _ = js_sys::Reflect::set(&day_options, &"year".into(), &"numeric".into());
    }
    let day = String::from(date.to_locale_date_string("en-US", &day_options));
    format!("{day} {time}")
}
