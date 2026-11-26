//! Brand color parsing and WCAG contrast checks.
//!
//! Shared by the server, which enforces the rules, and the web app, which
//! shows the result while staff pick a color.

/// Minimum contrast between white text and `brand_primary` (WCAG AA, normal text).
pub const MIN_CONTRAST_ON_PRIMARY: f64 = 4.5;

/// Parses `#RRGGBB` (either case) into its three channels.
pub fn parse_hex(value: &str) -> Option<[u8; 3]> {
    let hex = value.strip_prefix('#')?;
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let channel = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some([channel(0)?, channel(2)?, channel(4)?])
}

/// WCAG 2 relative luminance of an sRGB color.
pub fn relative_luminance([r, g, b]: [u8; 3]) -> f64 {
    let linear = |c: u8| {
        let c = f64::from(c) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
}

/// WCAG 2 contrast ratio between two colors, from 1.0 to 21.0.
pub fn contrast_ratio(a: [u8; 3], b: [u8; 3]) -> f64 {
    let (la, lb) = (relative_luminance(a), relative_luminance(b));
    let (light, dark) = if la > lb { (la, lb) } else { (lb, la) };
    (light + 0.05) / (dark + 0.05)
}

/// Contrast of white text on the given background.
pub fn contrast_with_white(background: [u8; 3]) -> f64 {
    contrast_ratio(background, [255, 255, 255])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_colors() {
        assert_eq!(parse_hex("#1B3A5C"), Some([0x1b, 0x3a, 0x5c]));
        assert_eq!(parse_hex("#e8693f"), Some([0xe8, 0x69, 0x3f]));
        assert_eq!(parse_hex("1B3A5C"), None);
        assert_eq!(parse_hex("#1B3A5"), None);
        assert_eq!(parse_hex("#1B3A5G"), None);
        assert_eq!(parse_hex("#1B3A5C0"), None);
    }

    #[test]
    fn contrast_matches_known_values() {
        assert!((contrast_with_white([0, 0, 0]) - 21.0).abs() < 1e-9);
        assert!((contrast_with_white([255, 255, 255]) - 1.0).abs() < 1e-9);
        // The default brand navy passes; a light yellow does not.
        assert!(contrast_with_white(parse_hex("#1B3A5C").unwrap()) > 11.0);
        assert!(contrast_with_white(parse_hex("#FFD966").unwrap()) < MIN_CONTRAST_ON_PRIMARY);
        // #767676 is the lightest gray that passes 4.5:1 on white.
        assert!(contrast_with_white(parse_hex("#767676").unwrap()) >= 4.5);
        assert!(contrast_with_white(parse_hex("#777777").unwrap()) < 4.5);
    }
}
