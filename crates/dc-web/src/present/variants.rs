//! Language and reading level switching on the patient screens
//! (PRD FR-LANG-01 and FR-LANG-02, UI_SPEC §4.10, §4.11 and §5.10).
//!
//! A switch asks the server to prepare the variant and shows what it has at
//! once. Sections still being prepared show placeholder lines; the view is
//! read again every couple of seconds until none is pending. The patient
//! stays on the same screen throughout.

use super::api;
use dc_core::content::ReadingLevel;
use dc_core::language::{self, LANGUAGES};
use dc_core::present::PatientView;
use leptos::prelude::*;
use leptos::task::spawn_local;
use std::time::Duration;

/// How often a variant being prepared is read again.
const POLL_EVERY: Duration = Duration::from_secs(2);
/// Reads before giving up on a variant, about three minutes.
const MAX_POLLS: u32 = 90;

/// The view on screen and the state of the latest switch.
#[derive(Clone, Copy)]
pub struct Variants {
    pub current: RwSignal<Option<PatientView>>,
    /// "Preparing Español…" while a variant is being prepared.
    pub preparing: RwSignal<Option<String>>,
    /// Why the last switch did not happen.
    pub notice: RwSignal<Option<String>>,
    /// The present session ended while switching.
    pub ended: RwSignal<bool>,
    /// Only the latest switch may change the view.
    latest: StoredValue<u32>,
}

fn native_name(tag: &str) -> &'static str {
    language::find(tag).map_or("this language", |l| l.native)
}

impl Variants {
    pub fn new() -> Self {
        Self {
            current: RwSignal::new(None),
            preparing: RwSignal::new(None),
            notice: RwSignal::new(None),
            ended: RwSignal::new(false),
            latest: StoredValue::new(0),
        }
    }

    fn next_request(self) -> u32 {
        self.latest.update_value(|n| *n += 1);
        self.latest.get_value()
    }

    fn is_latest(self, request: u32) -> bool {
        self.latest.try_get_value() == Some(request)
    }

    /// Shows `language` at `level`, preparing whatever the server has not
    /// cached yet.
    pub fn switch(self, language: String, level: ReadingLevel) {
        let request = self.next_request();
        let name = native_name(&language);
        self.notice.set(None);
        self.preparing.set(Some(format!("Preparing {name}…")));
        spawn_local(async move {
            let result = api::variant(&language, level).await;
            if !self.is_latest(request) {
                return;
            }
            match result {
                Ok(view) => {
                    let pending = view.pending();
                    self.current.try_set(Some(view));
                    if pending {
                        self.poll(request, language, level, 0);
                    } else {
                        self.preparing.try_set(None);
                    }
                }
                Err(failure) => self.failed(&failure, name),
            }
        });
    }

    fn failed(self, failure: &api::Failure, name: &str) {
        self.preparing.try_set(None);
        if failure.session_ended() {
            self.ended.try_set(true);
            return;
        }
        self.notice.try_set(Some(format!(
            "{name} is not available right now. {}",
            failure.message
        )));
    }

    fn poll(self, request: u32, language: String, level: ReadingLevel, count: u32) {
        set_timeout(
            move || {
                if !self.is_latest(request) {
                    return;
                }
                spawn_local(async move {
                    let result = api::content_in(&language, level).await;
                    if !self.is_latest(request) {
                        return;
                    }
                    match result {
                        Ok(view) if view.pending() && count + 1 < MAX_POLLS => {
                            self.poll(request, language, level, count + 1);
                        }
                        Ok(view) => {
                            self.current.try_set(Some(view));
                            self.preparing.try_set(None);
                        }
                        Err(failure) => self.failed(&failure, native_name(&language)),
                    }
                });
            },
            POLL_EVERY,
        );
    }
}

pub fn use_variants() -> Variants {
    expect_context::<Variants>()
}

fn level_label(level: ReadingLevel) -> &'static str {
    match level {
        ReadingLevel::Simple => "Simple",
        ReadingLevel::Standard => "Standard",
        ReadingLevel::Detailed => "Detailed",
    }
}

/// The Simple, Standard and Detailed control. Choosing a level keeps the
/// language and the screen.
#[component]
pub fn LevelControl(language: String, current: ReadingLevel) -> impl IntoView {
    let variants = use_variants();
    let options = [
        ReadingLevel::Simple,
        ReadingLevel::Standard,
        ReadingLevel::Detailed,
    ];
    view! {
        <div class="p-segmented" role="radiogroup" aria-label="Reading level">
            {options.into_iter().map(|level| {
                let on = level == current;
                let language = language.clone();
                view! {
                    <button type="button" role="radio" class="p-segment" class:is-on=on
                        aria-checked=if on { "true" } else { "false" }
                        on:click=move |_| {
                            if !on {
                                variants.switch(language.clone(), level);
                            }
                        }>
                        {level_label(level)}
                    </button>
                }
            }).collect_view()}
        </div>
    }
}

/// The header's language select. Languages without ClarityEval results say
/// they are not reviewed (PRD FR-LANG-05).
#[component]
pub fn LanguageSelect(current: String, level: ReadingLevel) -> impl IntoView {
    let variants = use_variants();
    let options = LANGUAGES
        .iter()
        .map(|l| {
            let label = if l.tested {
                l.label()
            } else {
                format!("{} · Not reviewed", l.label())
            };
            view! { <option value=l.tag selected=l.tag == current>{label}</option> }
        })
        .collect_view();
    view! {
        <label class="visually-hidden" for="p-language">"Language"</label>
        <select id="p-language" class="p-select"
            on:change=move |ev| variants.switch(event_target_value(&ev), level)>
            {options}
        </select>
    }
}

/// The header's "Aa" button, which opens the reading level control on every
/// screen (UI_SPEC §5.10).
#[component]
pub fn LevelButton(language: String, current: ReadingLevel) -> impl IntoView {
    let open = RwSignal::new(false);
    view! {
        <div class="p-level">
            <button type="button" class="p-icon-button p-level-button"
                aria-label=format!("Reading level: {}", level_label(current))
                aria-expanded=move || if open.get() { "true" } else { "false" }
                aria-controls="p-level-pop"
                on:click=move |_| open.update(|o| *o = !*o)>
                <span aria-hidden="true">"Aa"</span>
            </button>
            <div id="p-level-pop" class="p-level-pop" hidden=move || !open.get()>
                <div class="p-level-title">"Reading level"</div>
                <LevelControl language current />
            </div>
        </div>
    }
}
