//! Sentence-level narration (DECISIONS P10).
//!
//! Each sentence has its own clip. Playing a block plays its clips one after
//! another on a single audio element, preloads the next clip on a second
//! element, and highlights the sentence whose clip is playing. Highlighting
//! changes only at sentence boundaries; nothing claims word timing.

use super::api;
use dc_core::present::{PatientBlock, PatientSentence};
use dc_core::presentation::BlockSource;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos::wasm_bindgen::JsCast;
use leptos::wasm_bindgen::JsValue;
use leptos::wasm_bindgen::closure::Closure;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Idle,
    /// Waiting for the current clip; the first play can take a few seconds.
    Loading,
    Playing,
    Paused,
}

/// Shared player state for every narrated block on the patient screens.
#[derive(Clone, Copy)]
pub struct Narrator {
    /// Id of the block being narrated.
    active: RwSignal<Option<String>>,
    sentence: RwSignal<Option<usize>>,
    status: RwSignal<Status>,
    /// Inline message for one block: (block id, text).
    message: RwSignal<Option<(String, String)>>,
    /// The server said narration is off for this presentation.
    unavailable: RwSignal<bool>,
    /// The present session expired while playing.
    pub session_ended: RwSignal<bool>,
    keys: StoredValue<Vec<String>>,
    /// Bumped on every start and stop, so events from an old clip are ignored.
    generation: StoredValue<u32>,
    audio: StoredValue<Option<JsValue>, LocalStorage>,
    preload: StoredValue<Option<JsValue>, LocalStorage>,
}

const UNAVAILABLE: &str =
    "Listening is not available for this presentation. You can read along on screen.";
const PLAY_FAILED: &str = "This part could not be played right now. You can read it on screen.";

fn audio_element() -> Option<JsValue> {
    let element = document().create_element("audio").ok()?;
    let _ = element.set_attribute("preload", "auto");
    Some(element.into())
}

fn call(target: &JsValue, method: &str) -> Option<JsValue> {
    let function = js_sys::Reflect::get(target, &method.into())
        .ok()?
        .dyn_into::<js_sys::Function>()
        .ok()?;
    function.call0(target).ok()
}

fn set(target: &JsValue, name: &str, value: &JsValue) {
    let _ = js_sys::Reflect::set(target, &name.into(), value);
}

fn handler(f: impl FnMut() + 'static) -> JsValue {
    Closure::<dyn FnMut()>::new(f).into_js_value()
}

/// Stops loading and playing without firing an error the app reacts to.
fn clear(element: &JsValue) {
    call(element, "pause");
    for name in ["onplaying", "onwaiting", "onended", "onerror"] {
        set(element, name, &JsValue::NULL);
    }
    if let Some(el) = element.dyn_ref::<leptos::web_sys::Element>() {
        let _ = el.remove_attribute("src");
    }
    call(element, "load");
}

impl Narrator {
    pub fn new() -> Self {
        Narrator {
            active: RwSignal::new(None),
            sentence: RwSignal::new(None),
            status: RwSignal::new(Status::Idle),
            message: RwSignal::new(None),
            unavailable: RwSignal::new(false),
            session_ended: RwSignal::new(false),
            keys: StoredValue::new(Vec::new()),
            generation: StoredValue::new(0),
            audio: StoredValue::new_local(None),
            preload: StoredValue::new_local(None),
        }
    }

    pub fn is_active(&self, id: &str) -> bool {
        self.active.with(|a| a.as_deref() == Some(id))
    }

    pub fn is_current(&self, id: &str, index: usize) -> bool {
        self.is_active(id) && self.sentence.get() == Some(index)
    }

    pub fn status_for(&self, id: &str) -> Status {
        if self.is_active(id) {
            self.status.get()
        } else {
            Status::Idle
        }
    }

    pub fn message_for(&self, id: &str) -> Option<String> {
        self.message.with(|m| {
            m.as_ref()
                .filter(|(block, _)| block == id)
                .map(|(_, text)| text.clone())
        })
    }

    pub fn unavailable(&self) -> bool {
        self.unavailable.get()
    }

    /// Text for the shared polite live region.
    pub fn announcement(&self) -> String {
        if self.active.with(Option::is_none) {
            return String::new();
        }
        let total = self.keys.with_value(Vec::len);
        let position = self
            .sentence
            .get()
            .map(|i| format!(", sentence {} of {total}", i + 1))
            .unwrap_or_default();
        match self.status.get() {
            Status::Idle => String::new(),
            Status::Loading => "Preparing audio".into(),
            Status::Playing => format!("Playing{position}"),
            Status::Paused => format!("Paused{position}"),
        }
    }

    /// Play, pause or resume the given block.
    pub fn toggle(&self, id: &str, sentences: &[PatientSentence]) {
        if self.unavailable.get_untracked() {
            self.message
                .set(Some((id.to_owned(), UNAVAILABLE.to_owned())));
            return;
        }
        let same = self.active.with_untracked(|a| a.as_deref() == Some(id));
        match (same, self.status.get_untracked()) {
            (true, Status::Playing | Status::Loading) => self.pause(),
            (true, Status::Paused) => self.resume(),
            _ => self.start(id, sentences),
        }
    }

    fn start(&self, id: &str, sentences: &[PatientSentence]) {
        self.stop();
        if sentences.is_empty() {
            return;
        }
        self.keys
            .set_value(sentences.iter().map(|s| s.audio_key.clone()).collect());
        self.active.set(Some(id.to_owned()));
        self.play_index(0);
    }

    fn main_element(&self) -> Option<JsValue> {
        if let Some(el) = self.audio.get_value() {
            return Some(el);
        }
        let el = audio_element()?;
        self.audio.set_value(Some(el.clone()));
        Some(el)
    }

    fn play_index(&self, index: usize) {
        let generation = self.generation.get_value();
        let Some(key) = self.keys.with_value(|k| k.get(index).cloned()) else {
            self.finish();
            return;
        };
        let Some(audio) = self.main_element() else {
            self.fail(key);
            return;
        };
        self.sentence.set(Some(index));
        self.status.set(Status::Loading);

        let this = *self;
        let current = move || this.generation.get_value() == generation;
        set(
            &audio,
            "onplaying",
            &handler(move || {
                if current() {
                    this.status.set(Status::Playing);
                }
            }),
        );
        set(
            &audio,
            "onwaiting",
            &handler(move || {
                if current() && this.status.get_untracked() == Status::Playing {
                    this.status.set(Status::Loading);
                }
            }),
        );
        set(
            &audio,
            "onended",
            &handler(move || {
                if current() {
                    this.play_index(index + 1);
                }
            }),
        );
        let failed_key = key.clone();
        set(
            &audio,
            "onerror",
            &handler(move || {
                if current() {
                    this.fail(failed_key.clone());
                }
            }),
        );
        set(&audio, "src", &api::audio_url(&key).into());
        self.play_element(&audio, generation);

        // Warm the next clip so the gap between sentences stays short.
        if let Some(next) = self.keys.with_value(|k| k.get(index + 1).cloned())
            && let Some(preload) = self.preload_element()
        {
            set(&preload, "src", &api::audio_url(&next).into());
            call(&preload, "load");
        }
    }

    fn preload_element(&self) -> Option<JsValue> {
        if let Some(el) = self.preload.get_value() {
            return Some(el);
        }
        let el = audio_element()?;
        set(&el, "muted", &JsValue::TRUE);
        self.preload.set_value(Some(el.clone()));
        Some(el)
    }

    fn play_element(&self, audio: &JsValue, generation: u32) {
        let Some(promise) = call(audio, "play").and_then(|p| p.dyn_into::<js_sys::Promise>().ok())
        else {
            return;
        };
        let this = *self;
        let rejected = Closure::<dyn FnMut(JsValue)>::new(move |error: JsValue| {
            // The browser refused to start without a tap; let the patient resume.
            let name = js_sys::Reflect::get(&error, &"name".into())
                .ok()
                .and_then(|n| n.as_string())
                .unwrap_or_default();
            if this.generation.get_value() == generation && name == "NotAllowedError" {
                this.status.set(Status::Paused);
            }
        });
        let _ = promise.catch(&rejected);
        rejected.forget();
    }

    fn pause(&self) {
        if let Some(audio) = self.audio.get_value() {
            call(&audio, "pause");
        }
        self.status.set(Status::Paused);
    }

    fn resume(&self) {
        let generation = self.generation.get_value();
        if let Some(audio) = self.audio.get_value() {
            self.status.set(Status::Loading);
            self.play_element(&audio, generation);
        }
    }

    fn finish(&self) {
        self.stop();
    }

    fn fail(&self, key: String) {
        let id = self.active.get_untracked().unwrap_or_default();
        self.stop();
        let this = *self;
        spawn_local(async move {
            let failure = api::audio_failure(&key).await;
            let text = if failure.status == 503 || failure.code == "narration_unavailable" {
                this.unavailable.set(true);
                UNAVAILABLE
            } else if failure.session_ended() {
                this.session_ended.set(true);
                return;
            } else {
                PLAY_FAILED
            };
            this.message.set(Some((id, text.to_owned())));
        });
    }

    /// Stops playback and resets the player, e.g. when the patient moves to
    /// another screen.
    pub fn stop(&self) {
        self.generation.update_value(|g| *g = g.wrapping_add(1));
        if let Some(audio) = self.audio.get_value() {
            clear(&audio);
        }
        if let Some(preload) = self.preload.get_value() {
            clear(&preload);
        }
        if self.active.get_untracked().is_some() {
            self.active.set(None);
        }
        if self.sentence.get_untracked().is_some() {
            self.sentence.set(None);
        }
        if self.status.get_untracked() != Status::Idle {
            self.status.set(Status::Idle);
        }
        if self.message.with_untracked(Option::is_some) {
            self.message.set(None);
        }
    }
}

pub fn use_narrator() -> Narrator {
    expect_context::<Narrator>()
}

/// An id usable in DOM ids, from a section key and field.
fn dom_id(id: &str) -> String {
    id.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// The play button and status label for one block.
#[component]
pub fn PlayControl(id: String, sentences: Vec<PatientSentence>, label: String) -> impl IntoView {
    let narrator = use_narrator();
    let status = {
        let id = id.clone();
        Memo::new(move |_| narrator.status_for(&id))
    };
    let on_click = {
        let id = id.clone();
        move |_| narrator.toggle(&id, &sentences)
    };
    let name = move || match status.get() {
        Status::Playing | Status::Loading => format!("Pause narration: {label}"),
        Status::Paused => format!("Resume narration: {label}"),
        Status::Idle => format!("Play narration: {label}"),
    };
    let text = move || match status.get() {
        Status::Idle => "Listen",
        Status::Loading => "Preparing audio…",
        Status::Playing => "Listening",
        Status::Paused => "Paused",
    };
    view! {
        <div class="p-play" class:is-on=move || status.get() != Status::Idle>
            <button type="button" class="p-play-button" aria-label=name
                aria-disabled=move || narrator.unavailable().then_some("true")
                on:click=on_click>
                {move || match status.get() {
                    Status::Loading => view! { <span class="p-spinner" aria-hidden="true"></span> }.into_any(),
                    Status::Playing => view! {
                        <span class="p-glyph-pause" aria-hidden="true"><span></span><span></span></span>
                    }.into_any(),
                    _ => view! { <span class="p-glyph-play" aria-hidden="true"></span> }.into_any(),
                }}
            </button>
            <span class="p-play-label" aria-hidden="true">{text}</span>
        </div>
    }
}

/// One narrated text block: heading, play control, sentences as text nodes.
/// `headline` renders the first sentence as the page headline (At a glance).
#[component]
pub fn NarratedBlock(
    section_key: String,
    block: PatientBlock,
    #[prop(default = None)] provider: Option<String>,
    #[prop(optional)] headline: bool,
    #[prop(optional)] list: bool,
    #[prop(optional)] hide_heading: bool,
) -> impl IntoView {
    let narrator = use_narrator();
    let id = format!("{section_key}/{}", block.field);
    let anchor = dom_id(&id);
    let is_note = block.source == BlockSource::ClinicalNote;
    let note_label = is_note.then(|| match &provider {
        Some(name) => format!("From {name}'s notes"),
        None => "From your dentist's notes".to_owned(),
    });

    // Keep the sentence being read in view, without animation.
    {
        let id = id.clone();
        let anchor = anchor.clone();
        Effect::new(move |_| {
            if !narrator.is_active(&id) {
                return;
            }
            let Some(index) = narrator.sentence.get() else {
                return;
            };
            if let Some(el) = document().get_element_by_id(&format!("{anchor}-{index}")) {
                let options = js_sys::Object::new();
                let _ = js_sys::Reflect::set(&options, &"block".into(), &"nearest".into());
                if let Ok(f) = js_sys::Reflect::get(&el, &"scrollIntoView".into())
                    && let Ok(f) = f.dyn_into::<js_sys::Function>()
                {
                    let _ = f.call1(&el, &options);
                }
            }
        });
    }

    let sentence_view = {
        let id = id.clone();
        let anchor = anchor.clone();
        move |index: usize, text: String| {
            let id = id.clone();
            let class = move || {
                if narrator.is_current(&id, index) {
                    "p-sentence is-current"
                } else {
                    "p-sentence"
                }
            };
            view! { <span id=format!("{anchor}-{index}") class=class>{text}</span> }
        }
    };

    let texts: Vec<String> = block.sentences.iter().map(|s| s.text.clone()).collect();
    let body = if headline {
        let mut rest = texts.into_iter().enumerate();
        let first = rest.next();
        view! {
            {first.map(|(i, t)| view! { <h1 class="p-headline">{sentence_view(i, t)}</h1> })}
            <p class="p-read">
                {rest.map(|(i, t)| view! { {sentence_view(i, t)} " " }).collect_view()}
            </p>
        }
        .into_any()
    } else if list {
        view! {
            <ol class="p-steps">
                {texts.into_iter().enumerate().map(|(i, t)| view! { <li>{sentence_view(i, t)}</li> }).collect_view()}
            </ol>
        }
        .into_any()
    } else {
        view! {
            <p class="p-read">
                {texts.into_iter().enumerate().map(|(i, t)| view! { {sentence_view(i, t)} " " }).collect_view()}
            </p>
        }
        .into_any()
    };

    let message = {
        let id = id.clone();
        move || narrator.message_for(&id)
    };
    let heading = block.heading.clone();
    let has_audio = block.sentences.iter().all(|s| !s.audio_key.is_empty());
    view! {
        <section class="p-block" class:is-note=is_note>
            <div class="p-block-head">
                {note_label.map(|l| view! { <div class="p-note-label">{l}</div> })}
                {(!hide_heading && !headline).then(|| view! { <h2 class="p-block-title">{heading.clone()}</h2> })}
                {has_audio.then(|| view! { <PlayControl id=id.clone() sentences=block.sentences.clone() label=heading.clone() /> })}
            </div>
            {body}
            {move || message().map(|m| view! { <p class="p-inline-note">{m}</p> })}
        </section>
    }
}
