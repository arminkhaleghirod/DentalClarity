//! Presentation preview (mockup 2c, read-only for now): section rail,
//! selected section, and the checks panel.

use crate::api;
use crate::format::{language_name, money_view, timestamp, tooth_label};
use crate::patients::patient_href;
use crate::progress::GenerationProgress;
use dc_core::content::ReadingLevel;
use dc_core::presentation::{
    BlockSource, PresentationView, ProcedureFactView, SectionView, SectionVisibility, TextBlock,
};
use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::use_params_map;

/// Key of the costs entry, which is built from the plan rather than written.
const COSTS_KEY: &str = "costs";

const COST_CAVEAT: &str = "These are estimates from your plan. Your insurance decides the final amount, which can change.";

#[component]
pub fn PresentationPage() -> impl IntoView {
    let params = use_params_map();
    let id = move || params.read().get("id").unwrap_or_default();
    let view_data = LocalResource::new(move || api::presentation(id()));
    let refetch = Callback::new(move |_| view_data.refetch());

    view! {
        <div class="subpage">
            <Suspense fallback=|| view! {
                <header class="subbar"><BackLink href="/presentations".into() label="Back to presentations" /></header>
                <div class="subpage-body"><div class="notice">"Loading presentation…"</div></div>
            }>
                {move || view_data.get().map(|result| match result {
                    Ok(p) if !p.job_state.is_finished() => {
                        let job_id = p.job_id.clone();
                        let presentation_id = p.id.clone();
                        view! {
                            <EditorBar p />
                            <div class="subpage-body running-body">
                                <GenerationProgress job_id presentation_id on_finished=refetch />
                            </div>
                        }.into_any()
                    }
                    Ok(p) => view! { <EditorBar p=p.clone() /> <Editor p /> }.into_any(),
                    Err(failure) => view! {
                        <header class="subbar"><BackLink href="/presentations".into() label="Back to presentations" /></header>
                        <div class="subpage-body">
                            <div class="notice error" role="alert">{failure.message}</div>
                        </div>
                    }.into_any(),
                })}
            </Suspense>
        </div>
    }
}

#[component]
fn BackLink(href: String, label: &'static str) -> impl IntoView {
    view! {
        <A href=href attr:class="back-button" attr:aria-label=label>
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke-width="2"
                stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                <path d="M15 5l-7 7 7 7" />
            </svg>
        </A>
    }
}

fn procedure_count(n: usize) -> String {
    if n == 1 {
        "1 procedure".into()
    } else {
        format!("{n} procedures")
    }
}

#[component]
fn EditorBar(p: PresentationView) -> impl IntoView {
    let meta = format!("{} · {}", p.plan_name, procedure_count(p.procedures.len()));
    view! {
        <header class="subbar editor-bar">
            <BackLink href=patient_href(&p.patient_key) label="Back to patient" />
            <div class="editor-who">
                <h1 class="editor-name">{p.patient_display_name.clone()}</h1>
                <p class="editor-meta">{meta}</p>
            </div>
            <span class="version-chip">
                <b>{format!("v{}", p.version)}</b>
                <span class="muted">{timestamp(&p.created_at)}</span>
            </span>
            <div class="editor-actions">
                <button type="button" class="button text" aria-disabled="true"
                    title="Version history arrives in a later update">"History"</button>
                <button type="button" class="button text" aria-disabled="true"
                    title="Regenerating arrives in a later update">"Regenerate"</button>
                <button type="button" class="button" aria-disabled="true"
                    title="The booklet arrives in a later update">"Export booklet"</button>
                <button type="button" class="button primary present-button" aria-disabled="true"
                    title="Present mode arrives in M5">
                    <span class="play" aria-hidden="true"></span>
                    "Present"
                </button>
            </div>
        </header>
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Mark {
    Ready,
    Review,
    Failed,
    Auto,
}

#[derive(Clone)]
struct NavEntry {
    key: String,
    label: String,
    mark: Mark,
    procedure: bool,
}

fn mark_for(visibility: SectionVisibility) -> Mark {
    match visibility {
        SectionVisibility::Ready => Mark::Ready,
        SectionVisibility::NeedsReview => Mark::Review,
        SectionVisibility::Failed => Mark::Failed,
    }
}

fn is_procedure(section: &SectionView) -> bool {
    section.procedure_ref.is_some() || section.key.starts_with("procedure:")
}

fn fact_for<'a>(p: &'a PresentationView, section: &SectionView) -> Option<&'a ProcedureFactView> {
    let wanted = section
        .procedure_ref
        .as_deref()
        .or_else(|| section.key.strip_prefix("procedure:"))?;
    p.procedures.iter().find(|f| f.procedure_ref == wanted)
}

/// Rail label, e.g. "Root canal · #30".
fn procedure_label(p: &PresentationView, section: &SectionView) -> String {
    match fact_for(p, section) {
        Some(fact) => {
            let tooth = tooth_label(&fact.tooth);
            if tooth.is_empty() {
                fact.name.clone()
            } else {
                format!("{} · {tooth}", fact.name)
            }
        }
        None => section.title.clone(),
    }
}

fn nav_label(section: &SectionView) -> String {
    match section.key.as_str() {
        "at_a_glance" => "At a glance".into(),
        "journey" => "Treatment journey".into(),
        "next_steps" => "What happens next".into(),
        _ => section.title.clone(),
    }
}

/// Rail order: overview sections, procedures, costs, then what happens next.
fn nav_entries(p: &PresentationView) -> Vec<NavEntry> {
    let entry = |s: &SectionView, procedure: bool| NavEntry {
        key: s.key.clone(),
        label: if procedure {
            procedure_label(p, s)
        } else {
            nav_label(s)
        },
        mark: mark_for(s.visibility),
        procedure,
    };
    let mut entries: Vec<NavEntry> = p
        .sections
        .iter()
        .filter(|s| !is_procedure(s) && s.key != "next_steps")
        .map(|s| entry(s, false))
        .collect();
    entries.extend(
        p.sections
            .iter()
            .filter(|s| is_procedure(s))
            .map(|s| entry(s, true)),
    );
    entries.push(NavEntry {
        key: COSTS_KEY.into(),
        label: "Costs".into(),
        mark: Mark::Auto,
        procedure: false,
    });
    entries.extend(
        p.sections
            .iter()
            .filter(|s| s.key == "next_steps")
            .map(|s| entry(s, false)),
    );
    entries
}

#[component]
fn Editor(p: PresentationView) -> impl IntoView {
    let entries = nav_entries(&p);
    let first = entries
        .first()
        .map(|e| e.key.clone())
        .unwrap_or_else(|| COSTS_KEY.into());
    let selected = RwSignal::new(first);
    let data = StoredValue::new(p);
    let no_sections = data.with_value(|p| p.sections.is_empty());

    view! {
        <div class="editor-grid">
            <SectionRail entries selected />
            <main class="editor-canvas">
                {no_sections.then(|| view! {
                    <div class="notice error" role="alert">
                        "No sections were written for this version. Patients cannot be shown this presentation."
                    </div>
                })}
                {move || {
                    let key = selected.get();
                    data.with_value(|p| {
                        if key == COSTS_KEY {
                            view! { <CostsPane p=p.clone() /> }.into_any()
                        } else {
                            match p.sections.iter().find(|s| s.key == key) {
                                Some(section) => section_pane(p, section),
                                None => view! { <div class="notice">"Select a section."</div> }.into_any(),
                            }
                        }
                    })
                }}
            </main>
            <aside class="checks-rail" aria-labelledby="checks-title">
                <h2 id="checks-title" class="rail-title">"Checks"</h2>
                {move || {
                    let key = selected.get();
                    data.with_value(|p| checks_for(p, &key))
                }}
                <div class="rail-divider"></div>
                <h2 class="rail-title">"Details"</h2>
                {data.with_value(details)}
            </aside>
        </div>
    }
}

#[component]
fn SectionRail(entries: Vec<NavEntry>, selected: RwSignal<String>) -> impl IntoView {
    let first_procedure = entries.iter().position(|e| e.procedure);
    let costs_at = entries.iter().position(|e| e.key == COSTS_KEY);
    view! {
        <nav class="section-rail" aria-label="Sections">
            <p class="rail-label">"Sections"</p>
            <ul class="rail-list">
                {entries.into_iter().enumerate().map(|(i, entry)| {
                    let group = (Some(i) == first_procedure).then(|| view! {
                        <li class="rail-group" aria-hidden="true">"Procedures"</li>
                    });
                    let key = entry.key.clone();
                    let key_for_click = key.clone();
                    let is_current = move || selected.get() == key;
                    let (mark, mark_label) = match entry.mark {
                        Mark::Ready => ("✓", "Ready"),
                        Mark::Review => ("!", "Needs review"),
                        Mark::Failed => ("×", "Failed"),
                        Mark::Auto => ("auto", "Built from the plan"),
                    };
                    let mark_class = match entry.mark {
                        Mark::Ready => "rail-mark ready",
                        Mark::Review => "rail-mark review",
                        Mark::Failed => "rail-mark failed",
                        Mark::Auto => "rail-mark auto",
                    };
                    let spaced = Some(i) == costs_at && i > 0;
                    view! {
                        {group}
                        <li>
                            <button
                                type="button"
                                class="rail-item"
                                class:indent=entry.procedure
                                class:spaced=spaced
                                aria-current=move || if is_current() { Some("true") } else { None }
                                on:click=move |_| selected.set(key_for_click.clone())
                            >
                                <span class="rail-text">{entry.label}</span>
                                <span class=mark_class aria-hidden="true">{mark}</span>
                                <span class="visually-hidden">{format!(", {mark_label}")}</span>
                            </button>
                        </li>
                    }
                }).collect_view()}
            </ul>
        </nav>
    }
}

fn section_pane(p: &PresentationView, section: &SectionView) -> AnyView {
    let procedures: Vec<&SectionView> = p.sections.iter().filter(|s| is_procedure(s)).collect();
    let eyebrow = procedures
        .iter()
        .position(|s| s.key == section.key)
        .map(|i| format!("Procedure {} of {}", i + 1, procedures.len()));
    let fact = fact_for(p, section).cloned();
    let held = match section.visibility {
        SectionVisibility::Ready => None,
        SectionVisibility::NeedsReview => Some((
            "held-note review",
            "Held for staff review. Patients see nothing from this section until it is resolved.",
        )),
        SectionVisibility::Failed => Some((
            "held-note failed",
            "This section failed its checks and is never shown to patients.",
        )),
    };
    let blocks = section.blocks.clone();
    let questions = section.questions.clone();
    let empty = blocks.is_empty() && questions.is_empty();
    view! {
        <div class="pane-head">
            {eyebrow.map(|e| view! { <p class="eyebrow">{e}</p> })}
            <h2 class="pane-title">{section.title.clone()}</h2>
        </div>
        {fact.map(|fact| view! { <LockedStrip fact /> })}
        {held.map(|(class, text)| view! { <p class=class>{text}</p> })}
        {blocks.into_iter().map(|block| view! { <BlockCard block /> }).collect_view()}
        {(!questions.is_empty()).then(|| view! {
            <article class="block-card">
                <div class="block-head"><h3 class="block-title">"Questions to ask"</h3></div>
                <ul class="question-list">
                    {questions.into_iter().map(|q| view! { <li>{q}</li> }).collect_view()}
                </ul>
            </article>
        })}
        {(empty && section.visibility == SectionVisibility::Ready).then(|| view! {
            <p class="notice">"This section has no text."</p>
        })}
    }
    .into_any()
}

#[component]
fn LockIcon() -> impl IntoView {
    view! {
        <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke-width="2" aria-hidden="true">
            <rect x="5" y="11" width="14" height="10" rx="2"></rect>
            <path d="M8 11V7a4 4 0 0 1 8 0v4"></path>
        </svg>
    }
}

#[component]
fn LockedStrip(fact: ProcedureFactView) -> impl IntoView {
    let tooth = tooth_label(&fact.tooth);
    let tooth = if tooth.is_empty() {
        view! { <span class="none">"None"</span> }.into_any()
    } else if fact.surfaces.is_empty() {
        view! { <b>{tooth}</b> }.into_any()
    } else {
        view! { <b>{tooth}</b> <span class="surfaces">{fact.surfaces.clone()}</span> }.into_any()
    };
    view! {
        <dl class="locked-strip" aria-label="Locked facts from the practice's records">
            <div class="locked-cell"><dt>"Tooth"</dt><dd>{tooth}</dd></div>
            <div class="locked-cell"><dt>"Code"</dt><dd class="mono">{fact.code}</dd></div>
            <div class="locked-cell"><dt>"Phase"</dt><dd><b>{fact.phase}</b></dd></div>
            <div class="locked-cell"><dt>"Patient est."</dt><dd><b>{money_view(fact.patient_estimate)}</b></dd></div>
            <div class="locked-source"><LockIcon />"Locked · from Open Dental"</div>
        </dl>
    }
}

fn source_pill(source: BlockSource) -> (&'static str, &'static str) {
    match source {
        BlockSource::Source => ("prov-pill neutral", "Locked · from Open Dental"),
        BlockSource::ClinicalNote => ("prov-pill neutral", "From clinical note"),
        BlockSource::Generated => ("prov-pill ready", "AI-written · checked"),
        BlockSource::StaffEdit => ("prov-pill changed", "Edited by staff"),
    }
}

#[component]
fn BlockCard(block: TextBlock) -> impl IntoView {
    let (pill_class, pill_label) = source_pill(block.source);
    let confirm = block.source == BlockSource::ClinicalNote && block.needs_confirmation;
    let text = block.sentences.join(" ");
    view! {
        <article class="block-card" class:unconfirmed=confirm>
            <div class="block-head">
                <h3 class="block-title">{block.heading}</h3>
                <span class=pill_class>{pill_label}</span>
            </div>
            <p class="block-body">{text}</p>
            {confirm.then(|| view! {
                <div class="block-confirm">
                    <button type="button" class="button small" aria-disabled="true"
                        title="Confirming note-based text arrives in M5">"Looks right"</button>
                    <span class="block-hint">"Patients see general wording until this is confirmed."</span>
                </div>
            })}
        </article>
    }
}

#[component]
fn CostsPane(p: PresentationView) -> impl IntoView {
    let mut rows = p.procedures.clone();
    rows.sort_by_key(|f| f.phase);
    view! {
        <div class="pane-head">
            <p class="eyebrow">"Built from the plan"</p>
            <h2 class="pane-title">"Costs"</h2>
        </div>
        <div class="card">
            <table class="plan-table costs-table">
                <caption class="visually-hidden">"Estimated costs by procedure"</caption>
                <thead>
                    <tr>
                        <th scope="col">"Phase"</th>
                        <th scope="col">"Tooth"</th>
                        <th scope="col">"Code"</th>
                        <th scope="col">"Procedure"</th>
                        <th scope="col" class="amount">"Fee"</th>
                        <th scope="col" class="amount">"Insurance est."</th>
                        <th scope="col" class="amount">"Patient est."</th>
                    </tr>
                </thead>
                <tbody>
                    {rows.into_iter().map(|f| {
                        let tooth = tooth_label(&f.tooth);
                        view! {
                            <tr>
                                <td class="num">{f.phase}</td>
                                <td>
                                    {if tooth.is_empty() {
                                        view! { <span class="none" aria-label="No tooth">"—"</span> }.into_any()
                                    } else {
                                        view! { <span class="tooth">{tooth}</span> }.into_any()
                                    }}
                                </td>
                                <td class="mono code">{f.code}</td>
                                <td class="desc">{f.name}</td>
                                <td class="amount">{money_view(f.fee)}</td>
                                <td class="amount">{money_view(f.insurance_estimate)}</td>
                                <td class="amount estimate">{money_view(f.patient_estimate)}</td>
                            </tr>
                        }
                    }).collect_view()}
                </tbody>
                <tfoot>
                    <tr>
                        <td></td>
                        <td></td>
                        <td></td>
                        <th scope="row">"Total"</th>
                        <td class="amount">{money_view(p.total_fee)}</td>
                        <td></td>
                        <td class="amount">{money_view(p.total_patient_estimate)}</td>
                    </tr>
                </tfoot>
            </table>
        </div>
        <p class="cost-caveat">{COST_CAVEAT}</p>
    }
}

fn check_row(text: &'static str) -> impl IntoView {
    view! {
        <li class="check-row"><span class="check-mark" aria-hidden="true">"✓"</span>{text}</li>
    }
}

fn checks_for(p: &PresentationView, key: &str) -> AnyView {
    if key == COSTS_KEY {
        return view! {
            <ul class="check-list">
                {check_row("Amounts come straight from the practice's records")}
                {check_row("No amount is written or changed by the writer")}
            </ul>
        }
        .into_any();
    }
    let Some(section) = p.sections.iter().find(|s| s.key == key) else {
        return ().into_any();
    };
    match section.visibility {
        SectionVisibility::Ready => view! {
            <ul class="check-list">
                {check_row("Tooth, code, phase and cost match the practice's records")}
                {check_row("No diagnosis or outcome promises")}
                {check_row("Passed independent review")}
            </ul>
        }
        .into_any(),
        visibility => {
            let (class, lead) = if visibility == SectionVisibility::Failed {
                ("check-callout failed", "Failed. Never shown to patients.")
            } else {
                ("check-callout review", "Held for staff review.")
            };
            let reasons = if section.reasons.is_empty() {
                vec!["No reason was recorded.".to_owned()]
            } else {
                section.reasons.clone()
            };
            view! {
                <div class=class>
                    <span class="callout-mark" aria-hidden="true">"!"</span>
                    <div>
                        <p class="callout-lead">{lead}</p>
                        <ul class="reason-list">
                            {reasons.into_iter().map(|r| view! { <li>{r}</li> }).collect_view()}
                        </ul>
                    </div>
                </div>
            }
            .into_any()
        }
    }
}

fn level_name(level: ReadingLevel) -> &'static str {
    match level {
        ReadingLevel::Simple => "Simple",
        ReadingLevel::Standard => "Standard",
        ReadingLevel::Detailed => "Detailed",
    }
}

fn details(p: &PresentationView) -> AnyView {
    let seconds = format!("{:.1} s", p.generation_ms as f64 / 1000.0);
    view! {
        <dl class="detail-list">
            <dt>"Language"</dt><dd>{language_name(&p.language)}</dd>
            <dt>"Reading level"</dt><dd>{level_name(p.reading_level)}</dd>
            {p.provider_display_name.clone().map(|d| view! { <dt>"Provider"</dt><dd>{d}</dd> })}
            <dt>"Writer"</dt><dd class="mono">{p.writer_model.clone()}</dd>
            <dt>"Prompt"</dt><dd class="mono">{p.prompt_version.clone()}</dd>
            <dt>"Generation time"</dt><dd>{seconds}</dd>
        </dl>
    }
    .into_any()
}
