//! The practice logo: content checks and storage.
//!
//! The type of an upload is decided from its bytes. The file name and the
//! `Content-Type` the browser sent are never trusted. PNG and JPEG need a
//! well-formed header with sensible dimensions. SVG is never rewritten: it
//! is parsed and accepted only when everything in it is on an allowlist of
//! drawing elements, so a file that could run script or load anything is
//! refused as a whole (docs/SECURITY.md, DECISIONS E24).

use anyhow::Result;
use sha2::{Digest, Sha256};
use sqlx::{Executor, Sqlite};

/// Largest accepted upload, as UI_SPEC 5.7 states.
pub const MAX_BYTES: usize = 1024 * 1024;
/// Largest accepted width or height of a PNG or JPEG, in pixels.
pub const MAX_SIDE: u32 = 4096;

const SVG_NS: &str = "http://www.w3.org/2000/svg";
const XLINK_NS: &str = "http://www.w3.org/1999/xlink";

/// SVG elements a logo may use: shapes, text, paint servers, grouping and
/// clipping. No `script`, `style`, `foreignObject`, `image`, `a`, `iframe`,
/// animation or filter elements.
const SVG_ELEMENTS: &[&str] = &[
    "svg",
    "g",
    "defs",
    "title",
    "desc",
    "metadata",
    "symbol",
    "use",
    "path",
    "rect",
    "circle",
    "ellipse",
    "line",
    "polyline",
    "polygon",
    "text",
    "tspan",
    "linearGradient",
    "radialGradient",
    "stop",
    "pattern",
    "clipPath",
    "mask",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Png,
    Jpeg,
    Svg,
}

impl Kind {
    pub fn content_type(self) -> &'static str {
        match self {
            Kind::Png => "image/png",
            Kind::Jpeg => "image/jpeg",
            Kind::Svg => "image/svg+xml",
        }
    }
}

/// Decides what an upload is and whether it is acceptable. The error is a
/// sentence for staff.
pub fn check(bytes: &[u8]) -> Result<Kind, &'static str> {
    if bytes.is_empty() {
        return Err("Choose a logo file to upload.");
    }
    if bytes.len() > MAX_BYTES {
        return Err("The logo must be 1 MB or smaller.");
    }
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        let (w, h) = png_size(bytes).ok_or("This PNG file looks damaged.")?;
        return sides_ok(w, h).map(|()| Kind::Png);
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        let (w, h) = jpeg_size(bytes).ok_or("This JPEG file looks damaged.")?;
        return sides_ok(w, h).map(|()| Kind::Jpeg);
    }
    check_svg(bytes).map(|()| Kind::Svg)
}

fn sides_ok(width: u32, height: u32) -> Result<(), &'static str> {
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
        return Err("The logo can be at most 4096 pixels wide and 4096 pixels tall.");
    }
    Ok(())
}

/// Width and height from the IHDR chunk, which must come first.
fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let ihdr = bytes.get(8..24)?;
    if ihdr[0..4] != [0, 0, 0, 13] || &ihdr[4..8] != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes(ihdr[8..12].try_into().ok()?);
    let h = u32::from_be_bytes(ihdr[12..16].try_into().ok()?);
    Some((w, h))
}

/// Width and height from the first start-of-frame marker.
fn jpeg_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let mut i = 2;
    loop {
        if *bytes.get(i)? != 0xFF {
            return None;
        }
        // Markers may be padded with extra 0xFF bytes.
        while *bytes.get(i + 1)? == 0xFF {
            i += 1;
        }
        let marker = *bytes.get(i + 1)?;
        match marker {
            // Markers without a length.
            0x01 | 0xD0..=0xD7 => {
                i += 2;
                continue;
            }
            // End of image or start of scan before any frame header.
            0xD9 | 0xDA => return None,
            _ => {}
        }
        let len = usize::from(u16::from_be_bytes([*bytes.get(i + 2)?, *bytes.get(i + 3)?]));
        if len < 2 {
            return None;
        }
        let is_frame = matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC);
        if is_frame {
            let h = u16::from_be_bytes([*bytes.get(i + 5)?, *bytes.get(i + 6)?]);
            let w = u16::from_be_bytes([*bytes.get(i + 7)?, *bytes.get(i + 8)?]);
            return Some((u32::from(w), u32::from(h)));
        }
        i += 2 + len;
    }
}

const NOT_AN_IMAGE: &str = "Upload the logo as a PNG, JPEG or SVG file.";
const UNSAFE_SVG: &str = "This SVG contains scripts, links, embedded files or styling that patient screens cannot show safely. Export it as a plain SVG, or upload a PNG.";

fn check_svg(bytes: &[u8]) -> Result<(), &'static str> {
    let text = std::str::from_utf8(bytes).map_err(|_| NOT_AN_IMAGE)?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    if !text.trim_start().starts_with('<') {
        return Err(NOT_AN_IMAGE);
    }
    // A DTD is refused outright, which also rules out entity expansion.
    let options = roxmltree::ParsingOptions {
        allow_dtd: false,
        nodes_limit: 50_000,
    };
    let doc = roxmltree::Document::parse_with_options(text, options).map_err(|_| NOT_AN_IMAGE)?;
    let root = doc.root_element();
    if root.tag_name().name() != "svg" || root.tag_name().namespace() != Some(SVG_NS) {
        return Err(NOT_AN_IMAGE);
    }
    check_svg_node(root)
}

fn check_svg_node(node: roxmltree::Node<'_, '_>) -> Result<(), &'static str> {
    let name = node.tag_name();
    if name.namespace() != Some(SVG_NS) || !SVG_ELEMENTS.contains(&name.name()) {
        return Err(UNSAFE_SVG);
    }
    for attribute in node.attributes() {
        check_svg_attribute(&attribute)?;
    }
    // Editors put their own data in `metadata`, often in other namespaces.
    // It is never drawn, so its contents are not inspected, apart from
    // processing instructions, which are refused anywhere.
    if name.name() == "metadata" {
        return if node.descendants().any(|n| n.is_pi()) {
            Err(UNSAFE_SVG)
        } else {
            Ok(())
        };
    }
    for child in node.children() {
        if child.is_pi() {
            return Err(UNSAFE_SVG);
        }
        if child.is_element() {
            check_svg_node(child)?;
        }
    }
    Ok(())
}

fn check_svg_attribute(attribute: &roxmltree::Attribute<'_, '_>) -> Result<(), &'static str> {
    let name = attribute.name().to_ascii_lowercase();
    if name.starts_with("on") {
        return Err(UNSAFE_SVG);
    }
    let value: String = attribute
        .value()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_ascii_lowercase();
    // References may point only inside the document.
    if (name == "href" || attribute.namespace() == Some(XLINK_NS)) && !value.starts_with('#') {
        return Err(UNSAFE_SVG);
    }
    if value.contains("javascript:")
        || value.contains("data:")
        || value.contains("@import")
        || value.contains("expression(")
    {
        return Err(UNSAFE_SVG);
    }
    // `url(...)` is fine for a gradient or clip path in the same file.
    let mut rest = value.as_str();
    while let Some(at) = rest.find("url(") {
        rest = &rest[at + 4..];
        let target = rest.trim_start_matches(['"', '\'']);
        if !target.starts_with('#') {
            return Err(UNSAFE_SVG);
        }
    }
    Ok(())
}

/// A stored logo.
#[derive(Debug, Clone)]
pub struct Logo {
    pub content_type: String,
    pub bytes: Vec<u8>,
}

pub async fn load<'e>(db: impl Executor<'e, Database = Sqlite>) -> Result<Option<Logo>> {
    let row: Option<(String, Vec<u8>)> =
        sqlx::query_as("SELECT content_type, bytes FROM practice_logo WHERE id = 1")
            .fetch_optional(db)
            .await?;
    Ok(row.map(|(content_type, bytes)| Logo {
        content_type,
        bytes,
    }))
}

/// A short version of the stored logo for its URLs, without reading the bytes.
pub async fn version<'e>(db: impl Executor<'e, Database = Sqlite>) -> Result<Option<String>> {
    let row: Option<(String,)> = sqlx::query_as("SELECT sha256 FROM practice_logo WHERE id = 1")
        .fetch_optional(db)
        .await?;
    Ok(row.map(|(sha,)| sha.chars().take(16).collect()))
}

/// Stores bytes that passed [`check`], replacing any earlier logo.
pub async fn save<'e>(
    db: impl Executor<'e, Database = Sqlite>,
    kind: Kind,
    bytes: &[u8],
) -> Result<()> {
    let sha = hex::encode(Sha256::digest(bytes));
    sqlx::query(
        "INSERT INTO practice_logo (id, content_type, bytes, sha256) VALUES (1, ?, ?, ?)
         ON CONFLICT (id) DO UPDATE SET content_type = excluded.content_type,
             bytes = excluded.bytes, sha256 = excluded.sha256,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
    )
    .bind(kind.content_type())
    .bind(bytes)
    .bind(sha)
    .execute(db)
    .await?;
    Ok(())
}

/// Removes the logo. Reports whether there was one.
pub async fn delete<'e>(db: impl Executor<'e, Database = Sqlite>) -> Result<bool> {
    let removed = sqlx::query("DELETE FROM practice_logo WHERE id = 1")
        .execute(db)
        .await?
        .rows_affected();
    Ok(removed > 0)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A valid 1×1 PNG header with the given size. Only the header is checked.
    pub(crate) fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]);
        bytes
    }

    fn jpeg(width: u16, height: u16) -> Vec<u8> {
        let mut bytes = vec![0xFF, 0xD8];
        // An APP0 segment first, as most files have.
        bytes.extend_from_slice(&[0xFF, 0xE0, 0x00, 0x04, 0x4A, 0x46]);
        bytes.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x0B, 0x08]);
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&[0x01, 0x01, 0x11, 0x00, 0xFF, 0xD9]);
        bytes
    }

    fn svg(body: &str) -> Vec<u8> {
        format!(r#"<?xml version="1.0"?><svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" viewBox="0 0 10 10">{body}</svg>"#).into_bytes()
    }

    #[test]
    fn accepts_png_and_jpeg_by_content() {
        assert_eq!(check(&png(512, 256)), Ok(Kind::Png));
        assert_eq!(check(&jpeg(640, 480)), Ok(Kind::Jpeg));
    }

    #[test]
    fn rejects_damaged_or_huge_rasters() {
        assert!(check(&png(0, 10)).is_err());
        assert!(check(&png(5000, 10)).is_err());
        assert!(check(&png(10, 10)[..14]).is_err());
        assert!(check(&jpeg(10, 9000)).is_err());
        assert!(check(&[0xFF, 0xD8, 0xFF, 0xD9]).is_err());
    }

    #[test]
    fn rejects_other_files_whatever_their_name() {
        assert_eq!(check(b""), Err("Choose a logo file to upload."));
        assert_eq!(check(b"GIF89a....."), Err(NOT_AN_IMAGE));
        assert_eq!(check(b"<html><body>hi</body></html>"), Err(NOT_AN_IMAGE));
        assert_eq!(
            check(&vec![b' '; MAX_BYTES + 1]),
            Err("The logo must be 1 MB or smaller.")
        );
    }

    #[test]
    fn accepts_a_plain_svg_logo() {
        let logo = svg(concat!(
            r##"<title>Maple Grove</title><defs><linearGradient id="g"><stop offset="0" stop-color="#1B3A5C"/></linearGradient></defs>"##,
            r##"<g fill="url(#g)"><circle cx="5" cy="5" r="4"/><use xlink:href="#c"/></g><text x="1" y="9">MG</text>"##,
            r##"<metadata><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"/></metadata>"##,
        ));
        assert_eq!(check(&logo), Ok(Kind::Svg));
    }

    #[test]
    fn rejects_svg_that_could_run_or_load_anything() {
        let unsafe_bodies = [
            "<script>alert(1)</script>",
            r#"<circle r="1" onload="alert(1)"/>"#,
            r#"<a href="https://example.org"><circle r="1"/></a>"#,
            r#"<use href="https://example.org/x.svg#a"/>"#,
            r#"<use xlink:href="data:image/svg+xml;base64,AAAA"/>"#,
            r#"<image href="x.png"/>"#,
            r#"<foreignObject><div xmlns="http://www.w3.org/1999/xhtml">x</div></foreignObject>"#,
            "<style>@import url(https://example.org/a.css);</style>",
            r#"<rect style="fill: url( 'https://example.org/p' )"/>"#,
            r#"<rect fill="url(https://example.org/p)"/>"#,
            r#"<set attributeName="href" to="javascript:alert(1)"/>"#,
            r#"<animate attributeName="x"/>"#,
            r#"<rect style="background:java&#x0A;script:x"/>"#,
            "<?xml-stylesheet href=\"https://example.org/a.css\"?><rect/>",
        ];
        for body in unsafe_bodies {
            assert!(check(&svg(body)).is_err(), "accepted {body}");
        }
    }

    #[test]
    fn rejects_svg_with_a_dtd_or_the_wrong_root() {
        let dtd = br#"<?xml version="1.0"?><!DOCTYPE svg [<!ENTITY a "aaaa">]><svg xmlns="http://www.w3.org/2000/svg">&a;</svg>"#;
        assert!(check(dtd).is_err());
        let no_ns = br#"<svg viewBox="0 0 1 1"><rect/></svg>"#;
        assert!(check(no_ns).is_err());
        assert!(check(b"<svg xmlns=\"http://www.w3.org/2000/svg\"").is_err());
    }
}
