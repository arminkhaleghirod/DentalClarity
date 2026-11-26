//! Motion helpers: view transitions, reduced motion, delayed unmounts and
//! loading skeletons. Durations match the `--dur-*` tokens in tokens.css.

use leptos::prelude::*;
use leptos::wasm_bindgen::closure::Closure;
use leptos::wasm_bindgen::{JsCast, JsValue};
use std::cell::{Cell, RefCell};
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

/// The view transition this module started last, while it runs. Only its
/// own `finished` handler may take its scope classes off the root, so an
/// older transition that ends late cannot strip a newer one's classes.
struct ActiveTransition {
    id: u64,
    transition: JsValue,
    classes: Vec<&'static str>,
}

thread_local! {
    static NEXT_TRANSITION: Cell<u64> = const { Cell::new(0) };
    static ACTIVE: RefCell<Option<ActiveTransition>> = const { RefCell::new(None) };
}

fn remove_classes(root: Option<&leptos::web_sys::Element>, classes: &[&'static str]) {
    if let Some(root) = root {
        for class in classes {
            let _ = root.class_list().remove_1(class);
        }
    }
}

/// Runs `update` inside a view transition when the browser supports one, so
/// the old and new states cross-fade. `scope` is a space-separated list of
/// classes put on the root element while the transition runs, which tells
/// the stylesheet which region animates and in which direction. Without the
/// API, `update` runs directly and the CSS enter animations take over.
///
/// Starting a transition while another runs makes the browser skip the
/// older one. Its classes come off first, so the root never holds both
/// directions at once.
pub fn view_transition(scope: &'static str, update: impl FnOnce() + 'static) {
    let doc: JsValue = document().into();
    let Some(start) = js_fn(&doc, "startViewTransition") else {
        update();
        return;
    };
    let root = document().document_element();
    if let Some(previous) = ACTIVE.with(|a| a.borrow_mut().take()) {
        remove_classes(root.as_ref(), &previous.classes);
    }
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

    let Ok(transition) = start.call1(&doc, &callback) else {
        remove_classes(root.as_ref(), &classes);
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
    let Some(finished) =
        js_get(&transition, "finished").and_then(|p| p.dyn_into::<js_sys::Promise>().ok())
    else {
        remove_classes(root.as_ref(), &classes);
        return;
    };
    let id = NEXT_TRANSITION.with(|n| {
        n.set(n.get() + 1);
        n.get()
    });
    ACTIVE.with(|a| {
        *a.borrow_mut() = Some(ActiveTransition {
            id,
            transition,
            classes,
        });
    });
    let done = Closure::new(Box::new(move |_: JsValue| {
        let ours = ACTIVE.with(|a| {
            let mut active = a.borrow_mut();
            if active.as_ref().is_some_and(|t| t.id == id) {
                active.take()
            } else {
                None
            }
        });
        if let Some(ours) = ours {
            remove_classes(root.as_ref(), &ours.classes);
        }
    }) as Box<dyn FnMut(JsValue)>);
    let _ = finished.then(&done);
    done.forget();
}

/// Calls `f` once the running view transition, if any, has applied its
/// update to the page, or right away when none is running.
pub fn after_update(f: impl FnOnce() + 'static) {
    let pending = ACTIVE.with(|a| {
        a.borrow()
            .as_ref()
            .and_then(|t| js_get(&t.transition, "updateCallbackDone"))
            .and_then(|p| p.dyn_into::<js_sys::Promise>().ok())
    });
    let Some(pending) = pending else {
        f();
        return;
    };
    let mut f = Some(f);
    // `updateCallbackDone` rejects only when the update threw; focus is
    // still worth placing then.
    let done = Closure::new(Box::new(move |_: JsValue| {
        if let Some(f) = f.take() {
            f();
        }
    }) as Box<dyn FnMut(JsValue)>);
    let _ = pending.then2(&done, &done);
    done.forget();
}

/// Whether focus has nowhere useful to be: on the body, or on an element
/// that was taken out of the page.
fn focus_lost() -> bool {
    let doc = document();
    match doc.active_element() {
        None => true,
        Some(active) => {
            !active.is_connected()
                || doc
                    .body()
                    .is_some_and(|body| active == *AsRef::<leptos::web_sys::Element>::as_ref(&body))
        }
    }
}

/// Focuses `el` without scrolling it into view, and reports whether it
/// took focus. A disabled or hidden element does not.
fn focus_quietly(el: &leptos::web_sys::Element) -> bool {
    let options: JsValue = js_sys::Object::new().into();
    let _ = js_sys::Reflect::set(&options, &"preventScroll".into(), &JsValue::TRUE);
    if let Some(focus) = js_fn(el.as_ref(), "focus") {
        let _ = focus.call1(el.as_ref(), &options);
    }
    document().active_element().as_ref() == Some(el)
}

/// Puts focus back after a control that held it was replaced, for example
/// when an inline confirmation opens or closes. Once the page has updated,
/// and only if focus fell to the body, the first of `ids` that can take
/// focus gets it. Focus the user moved elsewhere in the meantime stays.
pub fn restore_focus(ids: Vec<String>) {
    after_update(move || {
        request_animation_frame(move || {
            if !focus_lost() {
                return;
            }
            let doc = document();
            for id in &ids {
                if let Some(el) = doc.get_element_by_id(id)
                    && focus_quietly(&el)
                {
                    return;
                }
            }
        });
    });
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

/// Frames to wait for a route's view transition to swap the page, about a
/// second at 60 Hz. Past that, focus moves regardless.
const ROUTE_SWAP_FRAMES: u32 = 60;

/// Frames to wait when no route transition has shown up. The router starts
/// one from a queued task, so the first frames may not see it yet; a route
/// change that needs no transition is already in the page by then.
const ROUTE_IDLE_FRAMES: u32 = 6;

/// Whether the router's own view transition is running. `leptos_router`
/// marks the root with `router-outlet-<level>` until it finishes.
fn route_transition_running() -> bool {
    document().document_element().is_some_and(|root| {
        let list = root.class_list();
        (0..list.length())
            .filter_map(|i| list.item(i))
            .any(|class| class.starts_with("router-outlet-"))
    })
}

/// Moves focus to the page content after a route change, unless something
/// on the new page (a search field, say) already took it. The element gets
/// focus without scrolling, so the router's own scroll handling stands.
///
/// A route view transition swaps the page a few frames after the location
/// changes. So this waits, a frame at a time, until the page that was
/// showing is gone: `#id` itself was replaced, or its first child was.
/// Browsers without view transitions get the next frame.
pub fn focus_page_content(id: &'static str) {
    let doc = document();
    let old = doc.get_element_by_id(id);
    let old_child = old.as_ref().and_then(|el| el.first_element_child());
    let doc_js: JsValue = doc.into();
    if js_fn(&doc_js, "startViewTransition").is_none() {
        request_animation_frame(move || focus_page_now(id));
        return;
    }
    wait_for_page(id, old, old_child, 0);
}

fn wait_for_page(
    id: &'static str,
    old: Option<leptos::web_sys::Element>,
    old_child: Option<leptos::web_sys::Element>,
    frames: u32,
) {
    request_animation_frame(move || {
        let swapped = match &old {
            Some(el) => !el.is_connected() || el.first_element_child() != old_child,
            None => document().get_element_by_id(id).is_some(),
        };
        let limit = if route_transition_running() {
            ROUTE_SWAP_FRAMES
        } else {
            ROUTE_IDLE_FRAMES
        };
        if swapped || frames >= limit {
            focus_page_now(id);
        } else {
            wait_for_page(id, old, old_child, frames + 1);
        }
    });
}

fn focus_page_now(id: &str) {
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
    focus_quietly(&target);
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
