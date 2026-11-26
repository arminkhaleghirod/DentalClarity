//! Display helpers shared by staff pages.

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
