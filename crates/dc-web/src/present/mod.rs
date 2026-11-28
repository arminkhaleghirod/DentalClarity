//! Present mode: the patient-facing screens under `/present` (milestone M5).
//!
//! This tree is mounted on its own, without the staff shell or session, and
//! talks only to `/api/present/*`. Everything a patient sees is branded with
//! the practice's name and colors; DentalClarity is never named here.

mod api;
mod ask;
mod chart;
mod engagement;
mod exit;
mod narration;
mod screens;

use crate::motion::view_transition;
use dc_core::ToothRef;
use dc_core::present::{
    PatientBlock, PatientSection, PatientSectionKind, PatientView, PracticeBrand,
};
use dc_core::presentation::ProcedureFactView;
use leptos::html;
use leptos::prelude::*;
use narration::Narrator;

/// Whether the current page belongs to Present mode.
pub fn is_present_path() -> bool {
    let path = window().location().pathname().unwrap_or_default();
    path == "/present" || path.starts_with("/present/")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    AtAGlance,
    Journey,
    Procedures,
    Costs,
    NextSteps,
}

impl Tab {
    fn label(self) -> &'static str {
        match self {
            Tab::AtAGlance => "At a glance",
            Tab::Journey => "Journey",
            Tab::Procedures => "Procedures",
            Tab::Costs => "Costs",
            Tab::NextSteps => "Next steps",
        }
    }
}

/// Which screen and procedure the patient is looking at.
#[derive(Clone, Copy)]
pub struct Nav {
    pub tab: RwSignal<Tab>,
    pub procedure: RwSignal<Option<String>>,
    data: StoredValue<PatientView>,
    narrator: Narrator,
}

/// Root classes for a tab change: which region animates and which way it
/// slides, following the order of the tabs.
fn tab_scope(from: Tab, to: Tab) -> &'static str {
    if (to as u8) >= (from as u8) {
        "vt-tab vt-forward"
    } else {
        "vt-tab vt-back"
    }
}

impl Nav {
    pub fn go(&self, tab: Tab) {
        self.narrator.stop();
        let nav = *self;
        let from = self.tab.get_untracked();
        let update = move || {
            if tab == Tab::Procedures && nav.procedure.get_untracked().is_none() {
                let first = nav
                    .data
                    .with_value(|v| v.procedures.first().map(|p| p.procedure_ref.clone()));
                nav.procedure.set(first);
            }
            nav.tab.set(tab);
            focus_main();
        };
        if from == tab {
            update();
        } else {
            view_transition(tab_scope(from, tab), update);
        }
    }

    pub fn open_procedure(&self, procedure_ref: String) {
        self.narrator.stop();
        let nav = *self;
        let from = self.tab.get_untracked();
        if from != Tab::Procedures {
            view_transition(tab_scope(from, Tab::Procedures), move || {
                nav.procedure.set(Some(procedure_ref));
                nav.tab.set(Tab::Procedures);
                focus_main();
            });
            return;
        }
        let current = self.procedure.get_untracked();
        if current.as_deref() == Some(procedure_ref.as_str()) {
            return;
        }
        let position = |r: Option<&str>| {
            nav.data.with_value(|v| {
                v.procedures
                    .iter()
                    .position(|p| Some(p.procedure_ref.as_str()) == r)
            })
        };
        let scope = if position(Some(&procedure_ref)) >= position(current.as_deref()) {
            "vt-detail vt-forward"
        } else {
            "vt-detail vt-back"
        };
        view_transition(scope, move || {
            nav.procedure.set(Some(procedure_ref));
            if let Some(main) = document().get_element_by_id("p-main") {
                main.set_scroll_top(0);
            }
        });
    }

    /// Opens the first procedure, in plan order, on the given tooth.
    pub fn open_tooth(&self, label: String) {
        let found = self.data.with_value(|v| {
            v.procedures
                .iter()
                .find(|p| p.tooth.trim().eq_ignore_ascii_case(&label))
                .map(|p| p.procedure_ref.clone())
        });
        if let Some(procedure_ref) = found {
            self.open_procedure(procedure_ref);
        }
    }
}

fn focus_main() {
    if let Some(main) = document().get_element_by_id("p-main") {
        let _ = js_sys::Reflect::get(&main, &"focus".into())
            .ok()
            .and_then(|f| leptos::wasm_bindgen::JsCast::dyn_into::<js_sys::Function>(f).ok())
            .map(|f| f.call0(&main));
        main.set_scroll_top(0);
    }
}

/// Scrolls the tab strip, and only as far as needed, so the current tab is
/// fully visible. Does nothing when every tab already fits.
fn scroll_current_tab_into_view() {
    let Ok(Some(tab)) = document().query_selector(".p-tab.is-current") else {
        return;
    };
    let options = js_sys::Object::new();
    for (key, value) in [("block", "nearest"), ("inline", "nearest")] {
        let _ = js_sys::Reflect::set(&options, &key.into(), &value.into());
    }
    if let Some(scroll) = js_sys::Reflect::get(&tab, &"scrollIntoView".into())
        .ok()
        .and_then(|f| leptos::wasm_bindgen::JsCast::dyn_into::<js_sys::Function>(f).ok())
    {
        let _ = scroll.call1(&tab, &options);
    }
}

pub fn use_data() -> StoredValue<PatientView> {
    expect_context::<StoredValue<PatientView>>()
}

pub fn use_nav() -> Nav {
    expect_context::<Nav>()
}

// Data helpers ---------------------------------------------------------------

pub fn section(view: &PatientView, kind: PatientSectionKind) -> Option<&PatientSection> {
    view.sections.iter().find(|s| s.kind == kind)
}

pub fn procedure_section<'a>(
    view: &'a PatientView,
    procedure_ref: &str,
) -> Option<&'a PatientSection> {
    view.sections.iter().find(|s| {
        s.kind == PatientSectionKind::Procedure && s.procedure_ref.as_deref() == Some(procedure_ref)
    })
}

pub fn block<'a>(section: &'a PatientSection, field: &str) -> Option<&'a PatientBlock> {
    section
        .blocks
        .iter()
        .find(|b| b.field == field && !b.sentences.is_empty())
}

/// The tooth label ("30", "K") when the procedure is on one tooth.
pub fn single_tooth(p: &ProcedureFactView) -> Option<String> {
    match p.tooth.trim().parse::<ToothRef>() {
        Ok(t @ (ToothRef::Permanent(_) | ToothRef::Primary(_))) => Some(t.to_string()),
        _ => None,
    }
}

/// "tooth 30" or "teeth 14, 19"; None when no procedure is on one tooth.
pub fn teeth_phrase<'a>(procedures: impl Iterator<Item = &'a ProcedureFactView>) -> Option<String> {
    let mut teeth: Vec<String> = Vec::new();
    for p in procedures {
        if let Some(t) = single_tooth(p)
            && !teeth.contains(&t)
        {
            teeth.push(t);
        }
    }
    match teeth.len() {
        0 => None,
        1 => Some(format!("tooth {}", teeth[0])),
        _ => Some(format!("teeth {}", teeth.join(", "))),
    }
}

pub struct Phase {
    pub number: u32,
    pub procedures: Vec<ProcedureFactView>,
    /// The journey section's summary for this phase, when written.
    pub summary: Option<PatientBlock>,
}

impl Phase {
    /// "Phase 1 · tooth 30".
    pub fn label(&self) -> String {
        match teeth_phrase(self.procedures.iter()) {
            Some(teeth) => format!("Phase {} · {teeth}", self.number),
            None => format!("Phase {}", self.number),
        }
    }

    /// CSS class for the phase color family.
    pub fn class(&self) -> String {
        format!("phase-{}", self.number.clamp(1, 4))
    }
}

/// Phases in treatment order, with their procedures in plan order. The
/// journey's `phases/{i}/summary` block belongs to the i-th phase.
pub fn phases(view: &PatientView) -> Vec<Phase> {
    let mut numbers: Vec<u32> = view.procedures.iter().map(|p| p.phase).collect();
    numbers.sort_unstable();
    numbers.dedup();
    let journey = section(view, PatientSectionKind::Journey);
    numbers
        .into_iter()
        .enumerate()
        .map(|(i, number)| Phase {
            number,
            procedures: view
                .procedures
                .iter()
                .filter(|p| p.phase == number)
                .cloned()
                .collect(),
            summary: journey
                .and_then(|s| block(s, &format!("phases/{i}/summary")))
                .cloned(),
        })
        .collect()
}

/// Planned teeth with their procedure counts, in chart order of appearance.
pub fn planned_teeth(view: &PatientView) -> Vec<chart::Planned> {
    let mut out: Vec<chart::Planned> = Vec::new();
    let labels = view.procedures.iter().filter_map(single_tooth).chain(
        view.affected_teeth
            .iter()
            .filter_map(|t| match t.trim().parse::<ToothRef>() {
                Ok(t @ (ToothRef::Permanent(_) | ToothRef::Primary(_))) => Some(t.to_string()),
                _ => None,
            }),
    );
    for label in labels {
        if !out.iter().any(|p| p.label == label) {
            let procedures = view
                .procedures
                .iter()
                .filter(|p| single_tooth(p).as_deref() == Some(label.as_str()))
                .count()
                .max(1);
            out.push(chart::Planned { label, procedures });
        }
    }
    out
}

fn valid_hex(color: &str) -> bool {
    color.len() == 7 && color.starts_with('#') && color[1..].chars().all(|c| c.is_ascii_hexdigit())
}

fn initial(name: &str) -> String {
    name.chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_default()
}

// Components -----------------------------------------------------------------

#[component]
pub fn PresentApp() -> impl IntoView {
    document().set_title("Your treatment plan");
    let content = LocalResource::new(api::content);
    view! {
        {move || match content.get() {
            None => view! { <LoadingScreen /> }.into_any(),
            Some(Err(failure)) if failure.session_ended() => view! { <Ended /> }.into_any(),
            Some(Err(_)) => view! {
                <div class="present p-center">
                    <div class="p-center-card" role="alert">
                        <p class="p-center-text">"This presentation could not be loaded."</p>
                        <button type="button" class="p-button primary" on:click=move |_| content.refetch()>
                            "Try again"
                        </button>
                    </div>
                </div>
            }.into_any(),
            Some(Ok(view)) => view! { <PresentRoot view /> }.into_any(),
        }}
    }
}

/// Placeholder in the shape of the header and At a glance, so the first
/// screen fades in without the layout jumping.
#[component]
fn LoadingScreen() -> impl IntoView {
    view! {
        <div class="present p-loading">
            <p class="visually-hidden" role="status">"Loading your treatment plan"</p>
            <div class="p-loading-header skeleton" aria-hidden="true">
                <span class="sk-bar sk-logo"></span>
                <span class="sk-bar sk-name"></span>
                <span class="sk-bar sk-tabs"></span>
                <span class="sk-bar sk-tool"></span>
            </div>
            <div class="p-glance p-loading-body skeleton" aria-hidden="true">
                <div class="p-chart-panel sk-panel"></div>
                <div class="p-glance-main">
                    <span class="sk-bar sk-eyebrow"></span>
                    <span class="sk-bar sk-headline"></span>
                    <span class="sk-bar sk-headline sk-short"></span>
                    <span class="sk-bar sk-block"></span>
                    <span class="sk-bar sk-block"></span>
                </div>
            </div>
        </div>
    }
}

#[component]
fn Ended() -> impl IntoView {
    view! {
        <div class="present p-center">
            <p class="p-center-text p-ended" role="alert">
                "This presentation has ended. Please hand the device back to the front desk."
            </p>
        </div>
    }
}

#[component]
fn PresentRoot(view: PatientView) -> impl IntoView {
    let brand = view.brand.clone();
    let demo = view.demo;
    let languages = view.available_languages.clone();
    // `lang` must name the language the text is in, so screen readers and
    // hyphenation follow the content rather than the requested language.
    let language = crate::format::content_language(&view.language, &languages);

    document().set_title(&brand.name);
    if let Some(root) = document().document_element() {
        let _ = root.set_attribute("lang", &language);
    }

    // The journey can always be drawn from the plan facts, even when its
    // section is held, so the tab shows whenever there is something to order.
    let journey_exists =
        section(&view, PatientSectionKind::Journey).is_some() || !view.procedures.is_empty();
    let data = StoredValue::new(view);
    let narrator = Narrator::new();
    let nav = Nav {
        tab: RwSignal::new(Tab::AtAGlance),
        procedure: RwSignal::new(None),
        data,
        narrator,
    };
    provide_context(data);
    provide_context(narrator);
    provide_context(nav);
    on_cleanup(move || narrator.stop());
    let tracker = engagement::Tracker::start(nav.tab);
    provide_context(tracker);

    // Audio keeps playing in a hidden tab; pause it there.
    let doc: leptos::wasm_bindgen::JsValue = document().into();
    let on_visibility = leptos::wasm_bindgen::closure::Closure::<dyn FnMut()>::new(move || {
        if document().hidden() {
            narrator.pause_when_hidden();
            tracker.pause();
        } else {
            tracker.resume();
        }
    })
    .into_js_value();
    let _ = js_sys::Reflect::set(&doc, &"onvisibilitychange".into(), &on_visibility);
    on_cleanup(move || {
        let doc: leptos::wasm_bindgen::JsValue = document().into();
        let _ = js_sys::Reflect::set(
            &doc,
            &"onvisibilitychange".into(),
            &leptos::wasm_bindgen::JsValue::NULL,
        );
    });

    let root = NodeRef::<html::Div>::new();
    let PracticeBrand {
        name,
        primary,
        highlight,
        logo_url,
    } = brand;
    Effect::new(move |_| {
        if let Some(el) = root.get() {
            let style = leptos::web_sys::HtmlElement::style(&el);
            if valid_hex(&primary) {
                let _ = style.set_property("--brand-primary", &primary);
            }
            if valid_hex(&highlight) {
                let _ = style.set_property("--brand-highlight", &highlight);
            }
        }
    });

    let tabs: Vec<Tab> = [
        Tab::AtAGlance,
        Tab::Journey,
        Tab::Procedures,
        Tab::Costs,
        Tab::NextSteps,
    ]
    .into_iter()
    .filter(|t| *t != Tab::Journey || journey_exists)
    .collect();

    // On a phone the tabs scroll sideways. Keep the current one in view
    // when a button elsewhere on the page changes the tab.
    Effect::new(move |previous: Option<Tab>| {
        let tab = nav.tab.get();
        if previous.is_some_and(|p| p != tab) {
            crate::motion::after_update(|| {
                request_animation_frame(scroll_current_tab_into_view);
            });
        }
        tab
    });

    let tab_buttons = tabs
        .into_iter()
        .map(|tab| {
            let current = move || nav.tab.get() == tab;
            view! {
                <button type="button" class="p-tab" class:is-current=current
                    aria-current=move || current().then_some("page")
                    on:click=move |_| nav.go(tab)>
                    {tab.label()}
                </button>
            }
        })
        .collect_view();

    let language_options = languages
        .iter()
        .map(|tag| {
            let selected = *tag == language;
            view! { <option value=tag.clone() selected=selected>{crate::format::language_name(tag)}</option> }
        })
        .collect_view();
    let single_language = languages.len() <= 1;

    let logo = initial(&name);
    let page = move || match nav.tab.get() {
        Tab::AtAGlance => view! { <screens::AtAGlance /> }.into_any(),
        Tab::Journey => view! { <screens::Journey /> }.into_any(),
        Tab::Procedures => view! { <screens::Procedures /> }.into_any(),
        Tab::Costs => view! { <screens::Costs /> }.into_any(),
        Tab::NextSteps => view! { <screens::NextSteps /> }.into_any(),
    };

    let ended = move || narrator.session_ended.get();
    view! {
        <div class="present" node_ref=root>
            <div class="p-frame" inert=move || ended().then_some("")>
                <a class="p-skip" href="#p-main">"Skip to content"</a>
                <header class="p-header">
                    <div class="p-brand">
                        // The practice name follows, so the image is decorative.
                        {match logo_url.filter(|u| u.starts_with("/api/present/logo")) {
                            Some(src) => view! { <img class="p-logo p-logo-image" src=src alt="" /> }.into_any(),
                            None => view! { <div class="p-logo" aria-hidden="true">{logo}</div> }.into_any(),
                        }}
                        <div class="p-brand-text">
                            <div class="p-brand-name">{name}</div>
                            {demo.then(|| view! { <div class="p-demo-note">"Demo with made-up patients"</div> })}
                        </div>
                    </div>
                    <nav class="p-tabs" aria-label="Sections">{tab_buttons}</nav>
                    <div class="p-tools">
                        <label class="visually-hidden" for="p-language">"Language"</label>
                        <select id="p-language" class="p-select"
                            title=single_language.then_some("More languages arrive in a later update")>
                            {language_options}
                        </select>
                        <exit::ExitControl demo />
                    </div>
                </header>
                <main id="p-main" class="p-main" tabindex="-1">{page}</main>
                <ask::AskQuestion />
                <div class="visually-hidden" aria-live="polite" aria-atomic="true">
                    {move || narrator.announcement()}
                </div>
            </div>
            {move || ended().then(|| view! {
                <div class="p-center p-overlay">
                    <p class="p-center-text p-ended" role="alert">
                        "This presentation has ended. Please hand the device back to the front desk."
                    </p>
                </div>
            })}
        </div>
    }
}
