use super::*;
use dc_core::content::ReadingLevel;
use dc_core::present::{PatientBlock, PatientSection, PatientSentence, PracticeBrand};
use dc_core::presentation::{BlockSource, ProcedureFactView};

fn sentence(text: &str) -> PatientSentence {
    PatientSentence {
        text: text.into(),
        audio_key: "0".repeat(64),
    }
}

fn view() -> PatientView {
    let procedure = |reference: &str, name: &str, tooth: &str, category: &str| ProcedureFactView {
        procedure_ref: reference.into(),
        name: name.into(),
        code: "D0000".into(),
        tooth: tooth.into(),
        surfaces: String::new(),
        phase: 1,
        fee: Some(Money(120_000)),
        insurance_estimate: Some(Money(60_000)),
        patient_estimate: None,
        category: Some(category.into()),
    };
    PatientView {
        brand: PracticeBrand {
            name: "Maple Grove Family Dental".into(),
            primary: "#1B3A5C".into(),
            highlight: "#E0735A".into(),
        },
        patient_first_name: "Daniel".into(),
        guardian_audience: false,
        provider_display_name: Some("Dr. Patel".into()),
        language: "en".into(),
        reading_level: ReadingLevel::Standard,
        available_languages: vec!["en".into()],
        sections: vec![
            PatientSection {
                key: "at_a_glance".into(),
                kind: PatientSectionKind::AtAGlance,
                title: "At a glance".into(),
                procedure_ref: None,
                blocks: vec![PatientBlock {
                    field: "summary".into(),
                    heading: "Your plan at a glance".into(),
                    // Typst control characters must print as written.
                    sentences: vec![sentence(
                        "Tooth #30 needs care. $5 [x] *bold* _under_ = #let a = 1",
                    )],
                    source: BlockSource::Generated,
                }],
                questions: Vec::new(),
            },
            PatientSection {
                key: "procedure:p1".into(),
                kind: PatientSectionKind::Procedure,
                title: "Root canal · #30".into(),
                procedure_ref: Some("p1".into()),
                blocks: vec![PatientBlock {
                    field: "what_it_is".into(),
                    heading: "What it is".into(),
                    sentences: vec![sentence("A root canal cleans the inside of a tooth.")],
                    source: BlockSource::Generated,
                }],
                questions: vec!["How long will it take?".into()],
            },
            PatientSection {
                key: "next_steps".into(),
                kind: PatientSectionKind::NextSteps,
                title: "What happens next".into(),
                procedure_ref: None,
                blocks: Vec::new(),
                questions: Vec::new(),
            },
        ],
        procedures: vec![
            procedure("p1", "Root canal", "30", "root_canal"),
            procedure("p2", "Crown", "30", "crown"),
        ],
        affected_teeth: vec!["30".into()],
        total_fee: Some(Money(240_000)),
        total_insurance_estimate: Some(Money(120_000)),
        total_patient_estimate: None,
        demo: true,
    }
}

fn outline() -> Vec<OutlineEntry> {
    ["at_a_glance", "procedure:p1", "procedure:p2", "next_steps"]
        .iter()
        .zip([
            "At a glance",
            "Root canal · #30",
            "Crown · #30",
            "What happens next",
        ])
        .map(|(key, title)| OutlineEntry {
            key: (*key).into(),
            title: title.into(),
        })
        .collect()
}

fn info() -> VersionInfo {
    VersionInfo {
        version: 2,
        fingerprint: "abcdef0123456789".into(),
        plan_as_of: "Nov 28, 2026".into(),
        prepared_on: "Nov 28, 2026".into(),
        paper: Paper::UsLetter,
    }
}

#[test]
fn held_sections_print_only_the_held_card() {
    let booklet = Booklet::build(&view(), &outline(), &info());
    let crown = booklet
        .sections
        .iter()
        .find(|s| s.key == "procedure:p2")
        .unwrap();
    assert!(crown.held);
    assert!(crown.blocks.is_empty() && crown.questions.is_empty());
    let text = booklet.plain_text().unwrap();
    assert!(text.contains("Your dental team will go over this part with you."));
    assert!(text.contains("Crown · #30"));
}

#[test]
fn markup_characters_print_as_written() {
    let text = Booklet::build(&view(), &outline(), &info())
        .plain_text()
        .unwrap()
        .replace('\n', " ");
    for piece in ["#30", "$5", "[x]", "*bold*", "_under_", "#let"] {
        assert!(text.contains(piece), "missing {piece} in {text}");
    }
}

#[test]
fn footer_and_costs_carry_version_and_estimates() {
    let text = Booklet::build(&view(), &outline(), &info())
        .plain_text()
        .unwrap()
        .replace('\n', " ");
    assert!(text.contains("Version 2 · abcdef01"), "{text}");
    assert!(text.contains("$2,400.00"));
    assert!(text.contains("Not available"));
    assert!(!text.contains("DentalClarity"));
}

#[test]
fn renders_a_pdf() {
    let pdf = Booklet::build(&view(), &outline(), &info())
        .render()
        .unwrap();
    assert!(pdf.starts_with(b"%PDF"));
}

#[test]
fn arabic_text_renders() {
    let mut view = view();
    view.language = "ar".into();
    view.sections[1].blocks[0].sentences = vec![sentence("علاج قناة الجذر ينظف داخل السن.")];
    let booklet = Booklet::build(&view, &outline(), &info());
    assert_eq!(booklet.lang, "ar");
    assert!(booklet.render().unwrap().starts_with(b"%PDF"));
}

#[test]
fn bad_brand_colors_fall_back() {
    let mut view = view();
    view.brand.primary = "red); evil".into();
    let booklet = Booklet::build(&view, &outline(), &info());
    assert_eq!(booklet.primary, "#1B3A5C");
}

#[test]
#[ignore = "writes a sample booklet to DC_BOOKLET_SAMPLE for a visual check"]
fn write_sample() {
    let path = std::env::var("DC_BOOKLET_SAMPLE").unwrap();
    let pdf = Booklet::build(&view(), &outline(), &info())
        .render()
        .unwrap();
    std::fs::write(path, pdf).unwrap();
}

#[test]
fn every_category_drawing_resolves() {
    assert_eq!(
        world::illustration_path("root_canal"),
        "/illustrations/root_canal.svg"
    );
    assert_eq!(
        world::illustration_path("no_such"),
        "/illustrations/fallback.svg"
    );
}
