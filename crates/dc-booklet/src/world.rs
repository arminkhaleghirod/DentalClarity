//! The Typst world of one booklet: the template, its JSON data, the bundled
//! fonts and the procedure drawings. Nothing is read from disk at render
//! time, and no system font is loaded.

use crate::{Booklet, BookletError, TEMPLATE};
use rust_embed::RustEmbed;
use std::sync::LazyLock;
use typst::diag::{FileError, FileResult};
use typst::foundations::{Bytes, Datetime, Duration};
use typst::layout::{Frame, FrameItem};
use typst::syntax::{FileId, RootedPath, Source, VirtualPath, VirtualRoot};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Library, LibraryExt, World};
use typst_layout::PagedDocument;

/// Bundled font files, in `fonts/`.
pub(crate) const FONT_FILES: [&str; 4] = [
    "NotoSans-Regular.ttf",
    "NotoSans-Bold.ttf",
    "NotoSansArabic-Regular.ttf",
    "NotoSansArabic-Bold.ttf",
];

const FONT_DATA: [&[u8]; 4] = [
    include_bytes!("../fonts/NotoSans-Regular.ttf"),
    include_bytes!("../fonts/NotoSans-Bold.ttf"),
    include_bytes!("../fonts/NotoSansArabic-Regular.ttf"),
    include_bytes!("../fonts/NotoSansArabic-Bold.ttf"),
];

/// The same drawings the patient screens show.
#[derive(RustEmbed)]
#[folder = "../../assets/illustrations"]
struct Illustrations;

const ILLUSTRATION_DIR: &str = "/illustrations/";

/// The virtual path of the drawing for a knowledge category id, or the
/// fallback drawing when there is none.
pub(crate) fn illustration_path(category: &str) -> String {
    let name = format!("{category}.svg");
    if !category.is_empty() && Illustrations::get(&name).is_some() {
        format!("{ILLUSTRATION_DIR}{name}")
    } else {
        format!("{ILLUSTRATION_DIR}fallback.svg")
    }
}

struct Fonts {
    book: LazyHash<FontBook>,
    fonts: Vec<Font>,
}

static FONTS: LazyLock<Fonts> = LazyLock::new(|| {
    let fonts: Vec<Font> = FONT_DATA
        .iter()
        .flat_map(|data| Font::iter(Bytes::new(*data)))
        .collect();
    Fonts {
        book: LazyHash::new(FontBook::from_fonts(&fonts)),
        fonts,
    }
});

static LIBRARY: LazyLock<LazyHash<Library>> = LazyLock::new(|| LazyHash::new(Library::default()));

fn file_id(path: &str) -> FileId {
    let vpath = VirtualPath::new(path).expect("static virtual path is valid");
    RootedPath::new(VirtualRoot::Project, vpath).intern()
}

struct BookletWorld {
    main: Source,
    data_id: FileId,
    data: Bytes,
}

impl BookletWorld {
    fn new(data: Vec<u8>) -> Self {
        Self {
            main: Source::new(file_id("/main.typ"), TEMPLATE.to_owned()),
            data_id: file_id("/data.json"),
            data: Bytes::new(data),
        }
    }
}

impl World for BookletWorld {
    fn library(&self) -> &LazyHash<Library> {
        &LIBRARY
    }

    fn book(&self) -> &LazyHash<FontBook> {
        &FONTS.book
    }

    fn main(&self) -> FileId {
        self.main.id()
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        if id == self.main.id() {
            Ok(self.main.clone())
        } else {
            Err(FileError::NotSource)
        }
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        if id == self.data_id {
            return Ok(self.data.clone());
        }
        let path = id.get().vpath().get_with_slash();
        let not_found = || FileError::NotFound(path.into());
        let name = path.strip_prefix(ILLUSTRATION_DIR).ok_or_else(not_found)?;
        let file = Illustrations::get(name).ok_or_else(not_found)?;
        Ok(Bytes::new(file.data.into_owned()))
    }

    fn font(&self, index: usize) -> Option<Font> {
        FONTS.fonts.get(index).cloned()
    }

    fn today(&self, _offset: Option<Duration>) -> Option<Datetime> {
        // The template prints dates it is given and never asks for today.
        None
    }
}

pub(crate) fn compile(booklet: &Booklet) -> Result<PagedDocument, BookletError> {
    let world = BookletWorld::new(serde_json::to_vec(booklet)?);
    typst::compile::<PagedDocument>(&world)
        .output
        .map_err(|errors| {
            BookletError::Compile(
                errors
                    .iter()
                    .map(|e| e.message.to_string())
                    .collect::<Vec<_>>()
                    .join("; "),
            )
        })
}

/// The text of a typeset document, one line per text run, pages separated
/// by a form feed.
pub fn plain_text_of(document: &PagedDocument) -> String {
    fn walk(frame: &Frame, out: &mut String) {
        for (_, item) in frame.items() {
            match item {
                FrameItem::Group(group) => walk(&group.frame, out),
                FrameItem::Text(text) => {
                    out.push_str(&text.text);
                    out.push('\n');
                }
                _ => {}
            }
        }
    }
    let mut out = String::new();
    for page in document.pages() {
        walk(&page.frame, &mut out);
        out.push('\u{c}');
    }
    out
}
