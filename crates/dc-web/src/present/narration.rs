//! Block narration (DECISIONS P10).
//!
//! A block plays as one clip that the server joins from its sentence clips
//! (`GET /api/present/narration`), with the start of each sentence computed
//! from sample counts. The highlight follows the audio element's position
//! against those offsets, so it changes only at sentence boundaries and
//! never claims word timing. A block longer than `MAX_NARRATION_SENTENCES`
//! plays as consecutive parts; the next part is fetched while one plays.

use super::api;
use dc_core::present::{MAX_NARRATION_SENTENCES, PatientBlock, PatientSentence};
use dc_core::presentation::BlockSource;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos::wasm_bindgen::JsCast;
use leptos::wasm_bindgen::JsValue;
use leptos::wasm_bindgen::closure::Closure;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Idle,
    /// Waiting for the clip; a cold cache can take a few seconds.
    Loading,
    Playing,
    Paused,
}

/// Shared player state for every narrated block on the patient screens.
#[derive(Clone, Copy)]
pub struct Narrator {
    /// Id of the block being narrated.
    active: RwSignal<Option<String>>,
    /// Index of the highlighted sentence in the whole block.
    sentence: RwSignal<Option<usize>>,
    status: RwSignal<Status>,
    /// Inline message for one block: (block id, text).
    message: RwSignal<Option<(String, String)>>,
    /// The server said narration is off for this presentation.
    unavailable: RwSignal<bool>,
    /// The present session expired while playing.
    pub session_ended: RwSignal<bool>,
    /// Audio keys of every sentence of the active block.
    keys: StoredValue<Vec<String>>,
    /// The part loaded on the audio element.
    part: StoredValue<usize>,
    /// Sentence starts of the loaded part, in milliseconds.
    offsets: StoredValue<Vec<f64>>,
    /// Object URL of the loaded part.
    current_url: StoredValue<Option<String>>,
    /// A part fetched ahead of time: (part, clip).
    ready: StoredValue<Option<(usize, api::BlockAudio)>>,
    /// The part being fetched.
    fetching: StoredValue<Option<usize>>,
    /// The part that should play as soon as it arrives.
    awaiting: StoredValue<Option<usize>>,
    /// A sentence to start from once its part is loaded.
    pending_seek: StoredValue<Option<usize>>,
    /// Generation of the running animation-frame loop, if any.
    ticking: StoredValue<Option<u32>>,
    /// Bumped on every start and stop, so events from an old clip are ignored.
    generation: StoredValue<u32>,
    audio: StoredValue<Option<JsValue>, LocalStorage>,
}

const UNAVAILABLE: &str =
    "Listening is not available for this presentation. You can read along on screen.";
const PLAY_FAILED: &str = "This part could not be played right now. You can read it on screen.";
const QUOTA: &str = "The listening limit for this demo is used up for now. You can read along on screen and try again later.";
const BUSY: &str = "Listening is busy right now. Try again in a moment.";

/// Events the player listens to on its audio element.
const EVENTS: [&str; 6] = [
    "onplaying",
    "onwaiting",
    "ontimeupdate",
    "onseeked",
    "onended",
    "onerror",
];

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
    for name in EVENTS {
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
            part: StoredValue::new(0),
            offsets: StoredValue::new(Vec::new()),
            current_url: StoredValue::new(None),
            ready: StoredValue::new(None),
            fetching: StoredValue::new(None),
            awaiting: StoredValue::new(None),
            pending_seek: StoredValue::new(None),
            ticking: StoredValue::new(None),
            generation: StoredValue::new(0),
            audio: StoredValue::new_local(None),
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

    /// Moves playback of the active block to the start of sentence `index`.
    /// Does nothing for a block that is not playing or paused.
    pub fn seek(&self, id: &str, index: usize) {
        if !self.active.with_untracked(|a| a.as_deref() == Some(id))
            || index >= self.keys.with_value(Vec::len)
        {
            return;
        }
        let part = index / MAX_NARRATION_SENTENCES;
        let loaded = self.current_url.with_value(Option::is_some)
            && self.awaiting.get_value().is_none()
            && self.part.get_value() == part;
        if loaded {
            let Some(audio) = self.audio.get_value() else {
                return;
            };
            let at = self
                .offsets
                .with_value(|o| o.get(index % MAX_NARRATION_SENTENCES).copied())
                .unwrap_or(0.0);
            set(&audio, "currentTime", &(at / 1000.0).into());
            self.update_sentence();
            if self.status.get_untracked() == Status::Paused {
                self.resume();
            }
        } else {
            // Marked as waiting for the target part before pausing: the
            // `timeupdate` that `pause()` queues on the old clip must not
            // move the highlight back to where that clip stopped.
            self.awaiting.set_value(Some(part));
            self.pending_seek.set_value(Some(index));
            self.sentence.set(Some(index));
            if let Some(audio) = self.audio.get_value() {
                call(&audio, "pause");
            }
            self.go_to(part);
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
        self.sentence.set(Some(0));
        self.go_to(0);
    }

    fn parts(&self) -> usize {
        self.keys
            .with_value(|k| k.len().div_ceil(MAX_NARRATION_SENTENCES))
    }

    fn part_keys(&self, part: usize) -> Vec<String> {
        self.keys.with_value(|k| {
            k.chunks(MAX_NARRATION_SENTENCES)
                .nth(part)
                .map(<[String]>::to_vec)
                .unwrap_or_default()
        })
    }

    /// Plays `part`, from the clip fetched ahead when it is that part, or
    /// after fetching it.
    fn go_to(&self, part: usize) {
        if let Some((ready_part, audio)) = self.ready.try_update_value(Option::take).flatten() {
            if ready_part == part {
                self.play_part(part, audio);
                return;
            }
            api::revoke(&audio.url);
        }
        self.awaiting.set_value(Some(part));
        if self.status.get_untracked() != Status::Paused {
            self.status.set(Status::Loading);
        }
        if self.fetching.get_value() != Some(part) {
            self.fetch(part);
        }
    }

    fn fetch(&self, part: usize) {
        let generation = self.generation.get_value();
        let keys = self.part_keys(part);
        if keys.is_empty() {
            return;
        }
        self.fetching.set_value(Some(part));
        let this = *self;
        spawn_local(async move {
            let result = api::narration(&keys).await;
            if this.generation.try_get_value() != Some(generation) {
                if let Ok(audio) = &result {
                    api::revoke(&audio.url);
                }
                return;
            }
            if this.fetching.get_value() == Some(part) {
                this.fetching.set_value(None);
            }
            let awaited = this.awaiting.get_value() == Some(part);
            match result {
                Ok(audio) if awaited => {
                    this.awaiting.set_value(None);
                    this.play_part(part, audio);
                }
                Ok(audio) => {
                    if let Some((_, old)) = this
                        .ready
                        .try_update_value(|r| r.replace((part, audio)))
                        .flatten()
                    {
                        api::revoke(&old.url);
                    }
                }
                Err(failure) if awaited => this.fail(&failure),
                // A part fetched ahead is fetched again when it is needed.
                Err(_) => {}
            }
        });
    }

    fn main_element(&self) -> Option<JsValue> {
        if let Some(el) = self.audio.get_value() {
            return Some(el);
        }
        let el = audio_element()?;
        self.audio.set_value(Some(el.clone()));
        Some(el)
    }

    fn play_part(&self, part: usize, clip: api::BlockAudio) {
        // This part is what plays now, whichever part was awaited before.
        self.awaiting.set_value(None);
        let Some(audio) = self.main_element() else {
            api::revoke(&clip.url);
            self.fail_playback();
            return;
        };
        let generation = self.generation.get_value();
        if let Some(old) = self
            .current_url
            .try_update_value(|u| u.replace(clip.url.clone()))
            .flatten()
        {
            api::revoke(&old);
        }
        self.offsets.set_value(clip.offsets_ms);
        self.part.set_value(part);

        let this = *self;
        let current = move || this.generation.try_get_value() == Some(generation);
        set(
            &audio,
            "onplaying",
            &handler(move || {
                if current() {
                    this.status.set(Status::Playing);
                    this.start_ticking();
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
        for event in ["ontimeupdate", "onseeked"] {
            set(
                &audio,
                event,
                &handler(move || {
                    if current() {
                        this.update_sentence();
                    }
                }),
            );
        }
        let parts = self.parts();
        set(
            &audio,
            "onended",
            &handler(move || {
                if current() {
                    if part + 1 < parts {
                        this.go_to(part + 1);
                    } else {
                        this.finish();
                    }
                }
            }),
        );
        set(
            &audio,
            "onerror",
            &handler(move || {
                if current() {
                    this.fail_playback();
                }
            }),
        );
        set(&audio, "src", &clip.url.into());
        let start = self
            .pending_seek
            .try_update_value(Option::take)
            .flatten()
            .filter(|i| i / MAX_NARRATION_SENTENCES == part)
            .and_then(|i| {
                self.offsets
                    .with_value(|o| o.get(i % MAX_NARRATION_SENTENCES).copied())
            });
        if let Some(at) = start {
            set(&audio, "currentTime", &(at / 1000.0).into());
        }
        self.update_sentence();
        // Paused while the clip was being prepared: stay paused.
        if self.status.get_untracked() != Status::Paused {
            self.status.set(Status::Loading);
            self.play_element(&audio, generation);
        }

        // Fetch the next part while this one plays.
        if part + 1 < parts
            && self.fetching.get_value().is_none()
            && self.ready.with_value(Option::is_none)
        {
            self.fetch(part + 1);
        }
    }

    /// Highlights the sentence whose range holds the current position.
    /// While another part is awaited, the element still holds the previous
    /// clip, whose position says nothing about the sentence being moved to.
    fn update_sentence(&self) {
        if self.awaiting.get_value().is_some() {
            return;
        }
        let Some(audio) = self.audio.get_value() else {
            return;
        };
        let Some(seconds) = js_sys::Reflect::get(&audio, &"currentTime".into())
            .ok()
            .and_then(|t| t.as_f64())
        else {
            return;
        };
        // Offsets are whole milliseconds; allow for rounding after a seek.
        let now = seconds * 1000.0 + 1.0;
        let index = self
            .offsets
            .with_value(|o| o.iter().rposition(|start| *start <= now).unwrap_or(0));
        let sentence = self.part.get_value() * MAX_NARRATION_SENTENCES + index;
        if self.sentence.get_untracked() != Some(sentence) {
            self.sentence.set(Some(sentence));
        }
    }

    /// Updates the highlight every frame while playing, so it moves on
    /// promptly at a sentence boundary (`timeupdate` fires only a few times
    /// a second).
    fn start_ticking(&self) {
        let generation = self.generation.get_value();
        if self.ticking.get_value() == Some(generation) {
            return;
        }
        self.ticking.set_value(Some(generation));
        self.tick(generation);
    }

    fn tick(&self, generation: u32) {
        let running = self.generation.try_get_value() == Some(generation)
            && self.status.try_get_untracked() == Some(Status::Playing);
        if !running {
            if self.ticking.try_get_value().flatten() == Some(generation) {
                self.ticking.set_value(None);
            }
            return;
        }
        self.update_sentence();
        let this = *self;
        request_animation_frame(move || this.tick(generation));
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
            if this.generation.try_get_value() == Some(generation) && name == "NotAllowedError" {
                this.status.set(Status::Paused);
            }
        });
        let _ = promise.catch(&rejected);
        rejected.forget();
    }

    /// Pauses when the page is hidden, for example when the browser tab
    /// changes, so narration never plays to an empty room. The patient
    /// resumes with the same button.
    pub fn pause_when_hidden(&self) {
        if matches!(
            self.status.get_untracked(),
            Status::Playing | Status::Loading
        ) {
            self.pause();
        }
    }

    fn pause(&self) {
        if let Some(audio) = self.audio.get_value() {
            call(&audio, "pause");
        }
        self.status.set(Status::Paused);
    }

    fn resume(&self) {
        let generation = self.generation.get_value();
        self.status.set(Status::Loading);
        // Still being prepared: it plays when it arrives.
        if self.awaiting.get_value().is_some() {
            return;
        }
        if let Some(audio) = self.audio.get_value() {
            self.play_element(&audio, generation);
        }
    }

    fn finish(&self) {
        self.stop();
    }

    fn fail(&self, failure: &api::Failure) {
        let id = self.active.get_untracked().unwrap_or_default();
        self.stop();
        if failure.session_ended() {
            self.session_ended.set(true);
            return;
        }
        let text = match (failure.status, failure.code.as_str()) {
            (429, "narration_quota") => QUOTA,
            (429, _) => BUSY,
            (503, _) | (_, "narration_unavailable") => {
                self.unavailable.set(true);
                UNAVAILABLE
            }
            _ => PLAY_FAILED,
        };
        self.message.set(Some((id, text.to_owned())));
    }

    /// The browser could not play a clip it was given.
    fn fail_playback(&self) {
        let id = self.active.get_untracked().unwrap_or_default();
        self.stop();
        self.message.set(Some((id, PLAY_FAILED.to_owned())));
    }

    /// Stops playback and resets the player, e.g. when the patient moves to
    /// another screen.
    pub fn stop(&self) {
        self.generation.update_value(|g| *g = g.wrapping_add(1));
        if let Some(audio) = self.audio.get_value() {
            clear(&audio);
        }
        if let Some(url) = self.current_url.try_update_value(Option::take).flatten() {
            api::revoke(&url);
        }
        if let Some((_, audio)) = self.ready.try_update_value(Option::take).flatten() {
            api::revoke(&audio.url);
        }
        self.fetching.set_value(None);
        self.awaiting.set_value(None);
        self.pending_seek.set_value(None);
        self.offsets.set_value(Vec::new());
        self.part.set_value(0);
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
/// `headline` renders the first sentence as the page headline (At a glance),
/// with the play control on its own row between the headline and the text.
/// `label` replaces the heading with a small label on that row, so the
/// control does not float alone.
#[component]
pub fn NarratedBlock(
    section_key: String,
    block: PatientBlock,
    #[prop(default = None)] provider: Option<String>,
    #[prop(optional)] headline: bool,
    #[prop(optional)] list: bool,
    #[prop(optional)] hide_heading: bool,
    #[prop(optional, into)] label: Option<String>,
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
            let seek_id = id.clone();
            let class = move || {
                if narrator.is_current(&id, index) {
                    "p-sentence is-current"
                } else {
                    "p-sentence"
                }
            };
            // While this block plays or is paused, a tap on a sentence
            // moves narration to it.
            let on_click = move |_| narrator.seek(&seek_id, index);
            view! { <span id=format!("{anchor}-{index}") class=class on:click=on_click>{text}</span> }
        }
    };

    let heading = block.heading.clone();
    let has_audio = block.sentences.iter().all(|s| !s.audio_key.is_empty());
    let compact = label.is_some();
    let title = match label {
        Some(l) => Some(l),
        None => (!hide_heading && !headline).then(|| heading.clone()),
    };
    let head = view! {
        <div class="p-block-head" class:is-compact=compact>
            {note_label.map(|l| view! { <div class="p-note-label">{l}</div> })}
            {title.map(|t| view! { <h2 class="p-block-title">{t}</h2> })}
            {has_audio.then(|| view! { <PlayControl id=id.clone() sentences=block.sentences.clone() label=heading.clone() /> })}
        </div>
    };

    let texts: Vec<String> = block.sentences.iter().map(|s| s.text.clone()).collect();
    let mut lead = None;
    let body = if headline {
        let mut rest = texts.into_iter().enumerate();
        lead = rest
            .next()
            .map(|(i, t)| view! { <h1 class="p-headline">{sentence_view(i, t)}</h1> });
        view! {
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
    view! {
        <section class="p-block" class:is-note=is_note>
            {lead}
            {head}
            {body}
            {move || message().map(|m| view! { <p class="p-inline-note">{m}</p> })}
        </section>
    }
}
