//! Display helpers shared by staff pages.

use dc_core::Money;
use dc_core::language;
use leptos::prelude::*;
use leptos::wasm_bindgen::JsValue;

/// English name for a language tag. Unknown tags are shown as-is rather
/// than guessed.
pub fn language_name(tag: &str) -> String {
    language::find(tag).map_or_else(|| tag.to_owned(), |l| l.english.to_owned())
}

/// The language to preselect on the create screen: the patient's recorded
/// preference when it is offered, else English. Content in every offered
/// language is derived from the approved English text (DECISIONS E32).
pub fn default_presentation_language(preferred: Option<&str>) -> &'static str {
    preferred
        .and_then(language::find)
        .map_or(language::SOURCE_LANGUAGE, |l| l.tag)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_preselects_an_offered_preference() {
        assert_eq!(default_presentation_language(Some("vi")), "vi");
        assert_eq!(default_presentation_language(Some("es-MX")), "es");
        assert_eq!(default_presentation_language(Some("xx")), "en");
        assert_eq!(default_presentation_language(None), "en");
        assert_eq!(language_name("es"), "Spanish");
        assert_eq!(language_name("xx"), "xx");
    }
}
