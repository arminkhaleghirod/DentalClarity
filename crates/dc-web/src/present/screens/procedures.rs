//! Procedures and procedure detail with narration (mockup 2e).

use super::common::*;
use crate::present::chart::{ToothChart, tooth_location};
use crate::present::narration::NarratedBlock;
use crate::present::{planned_teeth, procedure_section, single_tooth, use_data, use_nav};
use dc_core::ToothRef;
use dc_core::presentation::ProcedureFactView;
use leptos::prelude::*;

#[component]
pub fn Procedures() -> impl IntoView {
    let data = use_data();
    let nav = use_nav();
    let procedures = data.with_value(|v| v.procedures.clone());
    let planned = data.with_value(planned_teeth);
    if procedures.is_empty() {
        return view! { <div class="p-page"><Held /></div> }.into_any();
    }
    let current = {
        let procedures = procedures.clone();
        Memo::new(move |_| {
            let selected = nav.procedure.get();
            procedures
                .iter()
                .find(|p| Some(&p.procedure_ref) == selected.as_ref())
                .or_else(|| procedures.first())
                .cloned()
        })
    };
    let selected_tooth = Signal::derive(move || current.get().and_then(|p| single_tooth(&p)));

    let list = procedures
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let r = p.procedure_ref.clone();
            let is_current = {
                let r = r.clone();
                move || current.with(|c| c.as_ref().map(|c| &c.procedure_ref) == Some(&r))
            };
            let line = procedure_line(p);
            view! {
                <li>
                    <button type="button" class="p-step" class:is-current=is_current.clone()
                        aria-current=move || is_current().then_some("true")
                        on:click=move |_| nav.open_procedure(r.clone())>
                        <span class="p-step-num" aria-hidden="true">{(i + 1).to_string()}</span>
                        <span class="p-step-text">{line}</span>
                    </button>
                </li>
            }
        })
        .collect_view();

    view! {
        <div class="p-procedures">
            <aside class="p-rail" aria-label="Your procedures">
                <ToothChart planned selected=selected_tooth compact=true
                    on_select=Callback::new(move |label| nav.open_tooth(label)) />
                <h2 class="p-eyebrow-muted">{format!("Your plan · {}", steps_text(procedures.len()))}</h2>
                <ol class="p-step-list">{list}</ol>
            </aside>
            <div class="p-detail">
                {move || current.get().map(|p| view! { <ProcedureDetail procedure=p /> })}
            </div>
        </div>
    }
    .into_any()
}

#[component]
fn ReadingLevel(current: dc_core::content::ReadingLevel) -> impl IntoView {
    use dc_core::content::ReadingLevel as Level;
    let options = [
        (Level::Simple, "Simple"),
        (Level::Standard, "Standard"),
        (Level::Detailed, "Detailed"),
    ];
    // Only the level the presentation was written at exists for now. It
    // shows as selected; the others show as unavailable and say why.
    view! {
        <div class="p-segmented" role="radiogroup" aria-label="Reading level"
            aria-describedby="p-level-later">
            {options.into_iter().map(|(level, label)| {
                let on = level == current;
                view! {
                    <span role="radio" class="p-segment" class:is-on=on class:is-unavailable=!on
                        aria-checked=if on { "true" } else { "false" }
                        aria-disabled=(!on).then_some("true")
                        title=(!on).then_some("Not available yet. More reading levels arrive in a later update.")>
                        {label}
                    </span>
                }
            }).collect_view()}
            <span id="p-level-later" class="visually-hidden">"More reading levels arrive in a later update"</span>
        </div>
    }
}

#[component]
fn ProcedureDetail(procedure: ProcedureFactView) -> impl IntoView {
    let data = use_data();
    let (section, provider, level) = data.with_value(|v| {
        (
            procedure_section(v, &procedure.procedure_ref).cloned(),
            v.provider_display_name.clone(),
            v.reading_level,
        )
    });
    let tooth = single_tooth(&procedure);
    let location = match &tooth {
        Some(t) => {
            let place = t
                .parse::<ToothRef>()
                .ok()
                .and_then(tooth_location)
                .map(|l| format!(", {l}"))
                .unwrap_or_default();
            format!("Tooth {t}{place} · phase {}", procedure.phase)
        }
        None => format!("{} · phase {}", not_tied_label(&procedure), procedure.phase),
    };
    let title = section
        .as_ref()
        .map(|s| s.title.clone())
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| procedure.name.clone());

    // On wide screens the questions join the drawing in a side column, so
    // the narrated text keeps a reading width.
    let mut questions = Vec::new();
    let body = match section {
        None => view! { <Held /> }.into_any(),
        Some(section) => {
            let key = section.key.clone();
            let blocks = section
                .blocks
                .iter()
                .filter(|b| !b.sentences.is_empty())
                .cloned()
                .map(|b| {
                    view! { <NarratedBlock section_key=key.clone() block=b provider=provider.clone() /> }
                })
                .collect_view();
            questions = section.questions.clone();
            blocks.into_any()
        }
    };
    let questions = (!questions.is_empty()).then(|| {
        view! {
            <section class="p-block p-questions">
                <h2 class="p-block-title">"Questions you might ask your dental team"</h2>
                <ul class="p-question-list">
                    {questions.into_iter().map(|q| view! { <li>{q}</li> }).collect_view()}
                </ul>
            </section>
        }
    });

    view! {
        <article class="p-detail-inner">
            <div class="p-detail-main">
                <div class="p-detail-head">
                    <div>
                        <div class="p-eyebrow">{location}</div>
                        <h1 class="p-detail-title">{title}</h1>
                    </div>
                    <ReadingLevel current=level />
                </div>
                <div class="p-detail-body">{body}</div>
            </div>
            <div class="p-detail-side">
                <Illustration category=procedure.category.clone() />
                {questions}
            </div>
        </article>
    }
}

/// Alt text for each illustration in `assets/illustrations`, by category id.
const ILLUSTRATIONS: [(&str, &str); 22] = [
    (
        "exam",
        "Drawing of two back teeth in the gum with a small dental mirror next to them",
    ),
    (
        "radiographs",
        "Drawing of an X-ray machine aimed at a tooth with a small sensor behind it",
    ),
    (
        "cleaning",
        "Drawing of a cleaning tool removing hard buildup from a tooth at the gum line",
    ),
    (
        "periodontal_scaling",
        "Drawing of a tooth cut in half where the gum has pulled away, with a tool cleaning buildup off the root below the gum line",
    ),
    (
        "sealant",
        "Drawing of a back tooth cut in half with a thin coating filling the grooves on its chewing surface",
    ),
    (
        "fluoride",
        "Drawing of a tooth with a thin protective varnish painted over its outer surface",
    ),
    (
        "filling_composite",
        "Drawing of a back tooth cut in half with a tooth-colored filling in a cavity on its chewing surface",
    ),
    (
        "filling_amalgam",
        "Drawing of a back tooth cut in half with a silver filling in a cavity on its chewing surface",
    ),
    (
        "core_buildup",
        "Drawing of a broken back tooth rebuilt with filling material, with a dotted outline where a crown will go",
    ),
    (
        "post_and_core",
        "Drawing of a tooth cut in half with a small post in the root canal holding a build-up, with a dotted outline where a crown will go",
    ),
    (
        "crown",
        "Drawing of a crown being lowered over a tooth that has been shaped to fit it",
    ),
    (
        "root_canal",
        "Drawing of a tooth cut in half showing the cleaned and filled root canals and a sealed top",
    ),
    (
        "pulpotomy",
        "Drawing of a baby back tooth cut in half with healing material in the top of the pulp and the root pulp left in place",
    ),
    (
        "extraction",
        "Drawing of a tooth lifted out of its socket in the gum and bone",
    ),
    (
        "surgical_extraction",
        "Drawing of a tilted tooth under the gum and bone, with an opening made above it to remove it",
    ),
    (
        "bone_graft",
        "Drawing of an empty tooth socket filled with bone graft granules and covered by a thin membrane",
    ),
    (
        "implant",
        "Drawing of a screw-shaped implant post set in the jawbone with a small cap above the gum",
    ),
    (
        "implant_crown",
        "Drawing of a crown attached on top of an implant post in the jawbone",
    ),
    (
        "bridge",
        "Drawing of a bridge with crowns on two teeth holding a replacement tooth over the gap between them",
    ),
    (
        "denture_complete",
        "Drawing of a full upper denture seen from below, a gum-colored base holding a full row of teeth",
    ),
    (
        "denture_partial",
        "Drawing of a partial denture seen from above, replacement teeth on gum-colored bases joined by a metal bar with clasps",
    ),
    (
        "night_guard",
        "Drawing of a clear guard fitted over the upper teeth, resting between the upper and lower teeth",
    ),
];

const FALLBACK_ALT: &str = "Drawing of a tooth in the gum and jawbone";

/// The illustration file and alt text for a category, or the neutral fallback.
fn illustration_for(category: Option<&str>) -> (String, &'static str) {
    category
        .and_then(|c| ILLUSTRATIONS.iter().find(|(id, _)| *id == c))
        .map_or_else(
            || ("/illustrations/fallback.svg".to_owned(), FALLBACK_ALT),
            |(id, alt)| (format!("/illustrations/{id}.svg"), *alt),
        )
}

#[component]
fn Illustration(category: Option<String>) -> impl IntoView {
    let (src, alt) = illustration_for(category.as_deref());
    view! {
        <figure class="p-illustration">
            <img class="p-illustration-img" src=src alt=alt width="480" height="300" />
            <figcaption class="p-illustration-caption">"Educational drawing, not your X-ray."</figcaption>
        </figure>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_category_uses_its_drawing() {
        let (src, alt) = illustration_for(Some("root_canal"));
        assert_eq!(src, "/illustrations/root_canal.svg");
        assert!(alt.contains("root canals"));
    }

    #[test]
    fn missing_or_unknown_category_falls_back() {
        assert_eq!(illustration_for(None).0, "/illustrations/fallback.svg");
        assert_eq!(
            illustration_for(Some("unsupported")).0,
            "/illustrations/fallback.svg"
        );
    }

    #[test]
    fn every_category_has_a_drawing_file() {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/illustrations");
        for (id, alt) in ILLUSTRATIONS {
            assert!(dir.join(format!("{id}.svg")).is_file(), "missing {id}.svg");
            assert!(!alt.is_empty());
        }
        assert!(dir.join("fallback.svg").is_file());
    }

    /// Categories that show the neutral fallback drawing until one is drawn
    /// for them. A new category must be added either here or to
    /// `ILLUSTRATIONS`, so the choice is made on purpose.
    const USES_FALLBACK: &[&str] = &[
        "inlay_onlay",
        "veneer",
        "space_maintainer",
        "stainless_steel_crown",
        "crown_lengthening",
        "apicoectomy",
        "gum_graft",
        "whitening",
    ];

    #[test]
    fn every_mapped_category_has_alt_text() {
        let codes =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../knowledge/codes.toml");
        let text = std::fs::read_to_string(codes).expect("codes.toml");
        for part in text.split("category = \"").skip(1) {
            let id = part.split('"').next().unwrap_or_default();
            assert!(
                ILLUSTRATIONS.iter().any(|(known, _)| *known == id) || USES_FALLBACK.contains(&id),
                "no illustration for {id}"
            );
        }
    }
}

// Costs ------------------------------------------------------------------------
