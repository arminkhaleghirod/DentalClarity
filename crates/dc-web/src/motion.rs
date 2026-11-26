//! Motion helpers: view transitions, reduced motion, delayed unmounts and
//! loading skeletons. Durations match the `--dur-*` tokens in tokens.css.

use leptos::prelude::*;
use leptos::wasm_bindgen::closure::Closure;
use leptos::wasm_bindgen::{JsCast, JsValue};
use std::time::Duration;

/// `--dur-base`: the length of most exit animations.
pub const DUR_BASE_MS: u64 = 200;

fn js_get(target: &JsValue, name: &str) -> Option<JsValue> {
    js_sys::Reflect::get(target, &JsValue::from_str(name)).ok()
}

fn js_fn(target: &JsValue, name: &str) -> Option<js_sys::Function> {
    js_get(target, name).and_then(|f| f.dyn_into::<js_sys::Function>().ok())
}

/// Whether the user asked the system for less motion.
pub fn prefers_reduced_motion() -> bool {
    let win: JsValue = window().into();
    js_fn(&win, "matchMedia")
        .and_then(|f| {
            f.call1(&win, &JsValue::from_str("(prefers-reduced-motion: reduce)"))
                .ok()
        })
        .and_then(|query| js_get(&query, "matches"))
        .and_then(|m| m.as_bool())
        .unwrap_or(false)
}

/// Runs `update` inside a view transition when the browser supports one, so
/// the old and new states cross-fade. `scope` is a space-separated list of
/// classes put on the root element while the transition runs, which tells
/// the stylesheet which region animates and in which direction. Without the
/// API, `update` runs directly and the CSS enter animations take over.
pub fn view_transition(scope: &'static str, update: impl FnOnce() + 'static) {
    let doc: JsValue = document().into();
    let Some(start) = js_fn(&doc, "startViewTransition") else {
        update();
        return;
    };
    let root = document().document_element();
    let classes: Vec<&'static str> = scope.split_whitespace().collect();
    if let Some(root) = &root {
        for class in &classes {
            let _ = root.class_list().add_1(class);
        }
    }

    let callback = Closure::once_into_js(move || -> js_sys::Promise {
        update();
        // Leptos applies signal changes in queued tasks. Resolving after a
        // zero timeout lets them run, so the new state is what gets captured.
        js_sys::Promise::new(&mut |resolve, _reject| {
            set_timeout(
                move || {
                    let _ = resolve.call0(&JsValue::NULL);
                },
                Duration::ZERO,
            );
        })
    });

    let cleanup = move || {
        if let Some(root) = &root {
            for class in &classes {
                let _ = root.class_list().remove_1(class);
            }
        }
    };

    let Ok(transition) = start.call1(&doc, &callback) else {
        cleanup();
        return;
    };
    // A skipped transition rejects `ready`; that is expected, not an error.
    if let Some(ready) =
        js_get(&transition, "ready").and_then(|p| p.dyn_into::<js_sys::Promise>().ok())
    {
        let ignore = Closure::new(Box::new(|_: JsValue| {}) as Box<dyn FnMut(JsValue)>);
        let _ = ready.catch(&ignore);
        ignore.forget();
    }
    match js_get(&transition, "finished").and_then(|p| p.dyn_into::<js_sys::Promise>().ok()) {
        Some(finished) => {
            let mut cleanup = Some(cleanup);
            let done = Closure::new(Box::new(move |_: JsValue| {
                if let Some(cleanup) = cleanup.take() {
                    cleanup();
                }
            }) as Box<dyn FnMut(JsValue)>);
            let _ = finished.then(&done);
            done.forget();
        }
        None => cleanup(),
    }
}

/// Calls `f` once an exit animation of `ms` has played, or right away when
/// the user prefers reduced motion (exit animations are off then).
pub fn after_exit(ms: u64, f: impl FnOnce() + 'static) {
    if prefers_reduced_motion() {
        f();
    } else {
        set_timeout(f, Duration::from_millis(ms));
    }
}

/// Moves focus to the page content after a route change, unless something
/// on the new page (a search field, say) already took it. The element gets
/// focus without scrolling, so the router's own scroll handling stands. It
/// waits a moment because a view transition swaps the page a frame later.
pub fn focus_page_content(id: &'static str) {
    set_timeout(
        move || {
            let doc = document();
            let Some(target) = doc.get_element_by_id(id) else {
                return;
            };
            if let Some(active) = doc.active_element() {
                let in_page = target.contains(Some(&active)) && active != target;
                if in_page && active.is_connected() {
                    return;
                }
            }
            if let Some(el) = target.dyn_ref::<leptos::web_sys::HtmlElement>() {
                let options: JsValue = js_sys::Object::new().into();
                let _ = js_sys::Reflect::set(&options, &"preventScroll".into(), &JsValue::TRUE);
                if let Some(focus) = js_fn(el.as_ref(), "focus") {
                    let _ = focus.call1(el.as_ref(), &options);
                }
            }
        },
        Duration::from_millis(80),
    );
}

// Skeletons ------------------------------------------------------------------

/// A placeholder bar. Widths and heights come from the stylesheet.
fn bars(n: usize, class: &'static str) -> impl IntoView {
    (0..n)
        .map(|_| view! { <span class=class></span> })
        .collect_view()
}

/// A table-shaped placeholder: a header row and `rows` rows of the same
/// heights as `.table`, so nothing moves when the data arrives.
#[component]
pub fn SkeletonTable(
    label: &'static str,
    #[prop(default = 5)] rows: usize,
    #[prop(default = 6)] cols: usize,
) -> impl IntoView {
    view! {
        <div class="skeleton skeleton-table" role="status">
            <span class="visually-hidden">{label}</span>
            <div class="sk-row sk-head" aria-hidden="true">{bars(cols, "sk-bar")}</div>
            {(0..rows).map(|_| view! {
                <div class="sk-row" aria-hidden="true">{bars(cols, "sk-bar")}</div>
            }).collect_view()}
        </div>
    }
}

/// The four dashboard status cards.
#[component]
pub fn SkeletonCards() -> impl IntoView {
    view! {
        <div class="status-cards skeleton" aria-hidden="true">
            {(0..4).map(|_| view! {
                <div class="sk-card"><span class="sk-bar sk-title"></span><span class="sk-bar sk-line"></span></div>
            }).collect_view()}
        </div>
    }
}

/// A card with a heading bar and `lines` text lines.
#[component]
pub fn SkeletonCard(
    label: &'static str,
    #[prop(default = 4)] lines: usize,
    #[prop(optional)] class: &'static str,
) -> impl IntoView {
    view! {
        <div class=format!("skeleton skeleton-card {class}") role="status">
            <span class="visually-hidden">{label}</span>
            <div class="sk-card-head" aria-hidden="true">
                <span class="sk-avatar"></span>
                <span class="sk-stack"><span class="sk-bar sk-title"></span><span class="sk-bar sk-line"></span></span>
            </div>
            <div class="sk-lines" aria-hidden="true">{bars(lines, "sk-bar sk-text")}</div>
        </div>
    }
}
