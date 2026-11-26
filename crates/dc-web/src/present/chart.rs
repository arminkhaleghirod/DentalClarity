//! A simple tooth chart: Universal numbering on two elliptical arches, the
//! patient's right on the viewer's left. Planned teeth are filled with the
//! practice highlight color and also carry a bold outline, a bold number and
//! a count badge, so color is never the only cue. The full odontogram is M6.

use dc_core::ToothRef;
use leptos::ev::KeyboardEvent;
use leptos::prelude::*;
use std::f64::consts::PI;

/// One tooth with planned work.
#[derive(Debug, Clone, PartialEq)]
pub struct Planned {
    /// "30" or "K".
    pub label: String,
    pub procedures: usize,
}

struct Shape {
    label: String,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    /// Direction away from the arch center, for the badge.
    upper: bool,
}

fn permanent_size(n: u8) -> (f64, f64) {
    match n {
        1..=3 | 14..=19 | 30..=32 => (30.0, 30.0),
        4 | 5 | 12 | 13 | 20 | 21 | 28 | 29 => (24.0, 26.0),
        6 | 11 | 22 | 27 => (21.0, 27.0),
        _ => (20.0, 25.0),
    }
}

fn primary_size(c: char) -> (f64, f64) {
    match c {
        'A' | 'B' | 'I' | 'J' | 'K' | 'L' | 'S' | 'T' => (26.0, 26.0),
        'C' | 'H' | 'M' | 'R' => (20.0, 24.0),
        _ => (18.0, 22.0),
    }
}

/// Permanent arch in a 400×410 box, geometry from the approved mockup.
fn permanent_arch() -> Vec<Shape> {
    let (cx, rx, ry, cy_upper, cy_lower) = (200.0, 150.0, 160.0, 180.0, 228.0);
    let mut out = Vec::with_capacity(32);
    for i in 0..16u8 {
        let th = PI - (f64::from(i) + 0.5) * PI / 16.0;
        let x = cx + rx * th.cos();
        for (n, y, upper) in [
            (i + 1, cy_upper - ry * th.sin(), true),
            (32 - i, cy_lower + ry * th.sin(), false),
        ] {
            let (w, h) = permanent_size(n);
            out.push(Shape {
                label: n.to_string(),
                x,
                y,
                w,
                h,
                upper,
            });
        }
    }
    out
}

/// Primary arch (A to T) in a 400×300 box, same layout rules.
fn primary_arch() -> Vec<Shape> {
    let (cx, rx, ry, cy_upper, cy_lower) = (200.0, 120.0, 110.0, 130.0, 170.0);
    let mut out = Vec::with_capacity(20);
    for i in 0..10u8 {
        let th = PI - (f64::from(i) + 0.5) * PI / 10.0;
        let x = cx + rx * th.cos();
        let upper = char::from(b'A' + i);
        let lower = char::from(b'T' - i);
        for (c, y, is_upper) in [
            (upper, cy_upper - ry * th.sin(), true),
            (lower, cy_lower + ry * th.sin(), false),
        ] {
            let (w, h) = primary_size(c);
            out.push(Shape {
                label: c.to_string(),
                x,
                y,
                w,
                h,
                upper: is_upper,
            });
        }
    }
    out
}

/// "lower right, back" for a single tooth.
pub fn tooth_location(tooth: ToothRef) -> Option<String> {
    let (upper, right, position) = match tooth {
        ToothRef::Permanent(n) => (
            n <= 16,
            n <= 8 || n >= 25,
            match n {
                1..=3 | 14..=19 | 30..=32 => "back",
                4 | 5 | 12 | 13 | 20 | 21 | 28 | 29 => "side",
                _ => "front",
            },
        ),
        ToothRef::Primary(c) => (
            c <= 'J',
            c <= 'E' || c >= 'P',
            match c {
                'A' | 'B' | 'I' | 'J' | 'K' | 'L' | 'S' | 'T' => "back",
                _ => "front",
            },
        ),
        _ => return None,
    };
    Some(format!(
        "{} {}, {position}",
        if upper { "upper" } else { "lower" },
        if right { "right" } else { "left" }
    ))
}

fn accessible_name(label: &str, count: usize) -> String {
    let location = label
        .parse::<ToothRef>()
        .ok()
        .and_then(tooth_location)
        .map(|l| format!(", {l}"))
        .unwrap_or_default();
    let plural = if count == 1 {
        "procedure"
    } else {
        "procedures"
    };
    format!("Tooth {label}{location}. {count} planned {plural}.")
}

#[component]
fn Arch(
    shapes: Vec<Shape>,
    height: f64,
    planned: Vec<Planned>,
    selected: Signal<Option<String>>,
    on_select: Callback<String>,
    compact: bool,
) -> impl IntoView {
    let count_of = |label: &str| {
        planned
            .iter()
            .find(|p| p.label == label)
            .map(|p| p.procedures)
    };
    let (marked, plain): (Vec<_>, Vec<_>) = shapes
        .into_iter()
        .map(|s| {
            let count = count_of(&s.label);
            (s, count)
        })
        .partition(|(_, count)| count.is_some());
    let radius = if compact { 6.0 } else { 9.0 };

    let plain_view = plain
        .into_iter()
        .map(|(s, _)| {
            view! {
                <g class="tooth">
                    <rect x=s.x - s.w / 2.0 y=s.y - s.h / 2.0 width=s.w height=s.h rx=radius />
                    <text x=s.x y=s.y + 0.5 text-anchor="middle" dominant-baseline="middle">{s.label}</text>
                </g>
            }
        })
        .collect_view();

    let marked_view = marked
        .into_iter()
        .map(|(s, count)| {
            let count = count.unwrap_or(1);
            let label = s.label.clone();
            let name = accessible_name(&s.label, count);
            let is_selected = {
                let label = label.clone();
                move || selected.with(|sel| sel.as_deref() == Some(label.as_str()))
            };
            let pressed = {
                let is_selected = is_selected.clone();
                move || if is_selected() { "true" } else { "false" }
            };
            let choose = {
                let label = label.clone();
                move || on_select.run(label.clone())
            };
            let on_key = {
                let choose = choose.clone();
                move |ev: KeyboardEvent| {
                    if ev.key() == "Enter" || ev.key() == " " {
                        ev.prevent_default();
                        choose();
                    }
                }
            };
            let (bx, by) = (
                s.x + s.w / 2.0 - 1.0,
                if s.upper { s.y - s.h / 2.0 + 1.0 } else { s.y + s.h / 2.0 - 1.0 },
            );
            view! {
                <g class="tooth is-planned" class:is-selected=is_selected role="button" tabindex="0"
                    aria-label=name aria-pressed=pressed
                    on:click=move |_| choose() on:keydown=on_key>
                    <rect class="ring" x=s.x - s.w / 2.0 - 4.0 y=s.y - s.h / 2.0 - 4.0
                        width=s.w + 8.0 height=s.h + 8.0 rx=radius + 3.0 />
                    <rect class="body" x=s.x - s.w / 2.0 y=s.y - s.h / 2.0 width=s.w height=s.h rx=radius />
                    <text x=s.x y=s.y + 0.5 text-anchor="middle" dominant-baseline="middle">{s.label.clone()}</text>
                    <circle class="badge" cx=bx cy=by r="7.5" />
                    <text class="badge-text" x=bx y=by + 0.5 text-anchor="middle" dominant-baseline="middle">{count.to_string()}</text>
                </g>
            }
        })
        .collect_view();

    view! {
        <svg class="p-arch" viewBox=format!("-12 -12 424 {}", height + 24.0) role="group"
            aria-label="Tooth chart. Highlighted teeth have planned work.">
            <g aria-hidden="true">{plain_view}</g>
            <g>{marked_view}</g>
        </svg>
    }
}

/// The chart, with the permanent arch always and the primary arch when any
/// planned tooth is a letter.
#[component]
pub fn ToothChart(
    planned: Vec<Planned>,
    #[prop(into)] selected: Signal<Option<String>>,
    on_select: Callback<String>,
    #[prop(optional)] compact: bool,
) -> impl IntoView {
    let has_primary = planned
        .iter()
        .any(|p| matches!(p.label.parse::<ToothRef>(), Ok(ToothRef::Primary(_))));
    let summary = if planned.is_empty() {
        "No single teeth have planned work.".to_owned()
    } else {
        let list: Vec<&str> = planned.iter().map(|p| p.label.as_str()).collect();
        format!("Planned teeth: {}.", list.join(", "))
    };
    view! {
        <div class="p-chart" class:is-compact=compact>
            {(!compact).then(|| view! {
                <div class="p-chart-labels" aria-hidden="true"><span>"Your right"</span><span>"Upper"</span><span>"Your left"</span></div>
            })}
            <div class="p-chart-arch">
                <Arch shapes=permanent_arch() height=410.0 planned=planned.clone() selected on_select compact />
                {(!compact && !planned.is_empty()).then(|| view! {
                    <div class="p-chart-hint" aria-hidden="true">"Tap a highlighted tooth"</div>
                })}
            </div>
            {(!compact).then(|| view! { <div class="p-chart-lower" aria-hidden="true">"Lower"</div> })}
            {has_primary.then(|| view! {
                <div class="p-chart-primary">
                    <div class="p-chart-caption">"Baby teeth"</div>
                    <Arch shapes=primary_arch() height=300.0 planned=planned.clone() selected on_select compact />
                </div>
            })}
            <p class="visually-hidden">{summary}</p>
        </div>
    }
}
