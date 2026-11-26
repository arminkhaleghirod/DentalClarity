//! The tooth chart (odontogram) for the patient screens.
//!
//! Universal numbering: permanent teeth 1 to 32, primary teeth A to T. The
//! chart is drawn from the patient's point of view, so the patient's right
//! is on the viewer's left. Two arches (upper and lower) are laid out on
//! ellipses, and each tooth is an occlusal outline sized and shaped by its
//! type, turned to follow the arch.
//!
//! A plan with only permanent teeth shows the permanent dentition, a plan
//! with only primary teeth shows the primary dentition, and a plan with both
//! shows the primary teeth on an inner ring inside the permanent ones.
//!
//! Planned teeth use the practice highlight color, and color is never the
//! only cue: they also get a thicker outline, a bold label and a badge with
//! the number of planned procedures. Planned teeth are buttons in a roving
//! tabindex group (arrow keys move along the arch, Enter or Space selects);
//! the rest of the chart is decorative and hidden from assistive technology.
//!
//! The geometry and grouping logic at the top of this file is plain Rust
//! with no browser calls, so it is unit tested on the host.

use dc_core::ToothRef;
use dc_core::presentation::ProcedureFactView;
use leptos::ev::KeyboardEvent;
use leptos::prelude::*;
use std::collections::HashMap;
use std::f64::consts::PI;
use std::sync::atomic::{AtomicUsize, Ordering};

/// One tooth with planned work.
#[derive(Debug, Clone, PartialEq)]
pub struct Planned {
    /// "30" or "K".
    pub label: String,
    pub procedures: usize,
}

// Geometry --------------------------------------------------------------------

/// The SVG viewBox (x, y, width, height). Every layout, including selection
/// rings, badges and touch areas, fits inside it.
const VIEW_BOX: (f64, f64, f64, f64) = (6.0, -10.0, 388.0, 428.0);
const CX: f64 = 200.0;
const CY_UPPER: f64 = 180.0;
const CY_LOWER: f64 = 228.0;
/// Radius of the invisible touch area around a planned tooth, in chart
/// units. At the full chart width (about 400 px) this is a 48 px circle.
const HIT_RADIUS: f64 = 24.0;
const BADGE_RADIUS: f64 = 7.5;
/// How much wider the selection ring is than the crown, on each side.
const RING_GAP: f64 = 4.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dentition {
    Permanent,
    Primary,
    /// Both kinds planned: primary teeth on an inner ring.
    Mixed,
}

/// Which teeth to draw for a set of planned tooth labels.
pub fn dentition<'a>(labels: impl IntoIterator<Item = &'a str>) -> Dentition {
    let (mut permanent, mut primary) = (false, false);
    for label in labels {
        match label.parse::<ToothRef>() {
            Ok(ToothRef::Permanent(_)) => permanent = true,
            Ok(ToothRef::Primary(_)) => primary = true,
            _ => {}
        }
    }
    match (permanent, primary) {
        (true, true) => Dentition::Mixed,
        (false, true) => Dentition::Primary,
        _ => Dentition::Permanent,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToothKind {
    Molar,
    Premolar,
    Canine,
    Incisor,
}

pub fn tooth_kind(tooth: ToothRef) -> Option<ToothKind> {
    match tooth {
        ToothRef::Permanent(n) => Some(match n {
            1..=3 | 14..=19 | 30..=32 => ToothKind::Molar,
            4 | 5 | 12 | 13 | 20 | 21 | 28 | 29 => ToothKind::Premolar,
            6 | 11 | 22 | 27 => ToothKind::Canine,
            _ => ToothKind::Incisor,
        }),
        ToothRef::Primary(c) => Some(match c {
            'A' | 'B' | 'I' | 'J' | 'K' | 'L' | 'S' | 'T' => ToothKind::Molar,
            'C' | 'H' | 'M' | 'R' => ToothKind::Canine,
            _ => ToothKind::Incisor,
        }),
        _ => None,
    }
}

/// Crown size at scale 1: (along the arch, across the arch). Molars are the
/// widest, lower incisors the narrowest, as in a real mouth.
fn tooth_size(tooth: ToothRef) -> (f64, f64) {
    match tooth {
        ToothRef::Permanent(n) => match n {
            1..=3 | 14..=19 | 30..=32 => (28.0, 30.0),
            4 | 5 | 12 | 13 | 20 | 21 | 28 | 29 => (23.0, 26.0),
            6 | 11 | 22 | 27 => (21.0, 27.0),
            8 | 9 => (22.0, 25.0),
            7 | 10 => (19.0, 24.0),
            _ => (17.0, 23.0),
        },
        ToothRef::Primary(c) => match c {
            'A' | 'B' | 'I' | 'J' | 'K' | 'L' | 'S' | 'T' => (22.0, 22.0),
            'C' | 'H' | 'M' | 'R' => (17.0, 21.0),
            _ => (16.0, 19.0),
        },
        _ => (0.0, 0.0),
    }
}

/// One ellipse the teeth of a dentition sit on.
#[derive(Debug, Clone, Copy)]
struct Ring {
    rx: f64,
    ry: f64,
    scale: f64,
    primary: bool,
}

/// From the approved mockup: radius 150 by 160 in a 400 by 410 box.
const PERMANENT_RING: Ring = Ring {
    rx: 150.0,
    ry: 160.0,
    scale: 1.0,
    primary: false,
};
/// Primary teeth inside the permanent ring, for mixed dentition.
const PRIMARY_INNER_RING: Ring = Ring {
    rx: 92.0,
    ry: 98.0,
    scale: 1.0,
    primary: true,
};
/// Primary teeth alone use most of the chart, drawn larger.
const PRIMARY_ONLY_RING: Ring = Ring {
    rx: 138.0,
    ry: 148.0,
    scale: 1.3,
    primary: true,
};

fn rings(dentition: Dentition) -> Vec<Ring> {
    match dentition {
        Dentition::Permanent => vec![PERMANENT_RING],
        Dentition::Primary => vec![PRIMARY_ONLY_RING],
        Dentition::Mixed => vec![PERMANENT_RING, PRIMARY_INNER_RING],
    }
}

fn outer_ring(dentition: Dentition) -> Ring {
    rings(dentition)[0]
}

/// A tooth's place on the chart.
#[derive(Debug, Clone, PartialEq)]
pub struct ToothGeom {
    pub tooth: ToothRef,
    pub label: String,
    pub kind: ToothKind,
    /// Center, in chart units.
    pub x: f64,
    pub y: f64,
    /// Width along the arch and height across it.
    pub w: f64,
    pub h: f64,
    /// Rotation in degrees (SVG, clockwise) that turns the crown outline,
    /// drawn with its outer (cheek or lip) side up, to follow the arch.
    pub angle: f64,
    pub upper: bool,
}

impl ToothGeom {
    /// Unit vector from the tooth toward the cheek or lip side.
    pub fn outward(&self) -> (f64, f64) {
        let a = self.angle.to_radians();
        (a.sin(), -a.cos())
    }

    /// Center of the count badge, just outside the crown's outer edge.
    pub fn badge_center(&self) -> (f64, f64) {
        let (dx, dy) = self.outward();
        let d = self.h / 2.0 + 3.0;
        (self.x + dx * d, self.y + dy * d)
    }

    /// Where the tooth number goes. An incisor narrows toward the tongue,
    /// so its number moves a little toward the wide biting edge; centered,
    /// it would touch the slanted sides of the narrow lower incisors.
    pub fn label_center(&self) -> (f64, f64) {
        if self.kind != ToothKind::Incisor {
            return (self.x, self.y);
        }
        let (dx, dy) = self.outward();
        let d = self.h * 0.12;
        (self.x + dx * d, self.y + dy * d)
    }

    /// Lower incisors and other crowns under 18 units wide, whose numbers
    /// are set a size smaller to stay clear of the outline.
    pub fn is_narrow(&self) -> bool {
        self.w < 18.0
    }

    fn transform(&self) -> String {
        format!(
            "translate({:.2} {:.2}) rotate({:.2})",
            self.x, self.y, self.angle
        )
    }
}

/// Point on a ring at parameter `th` (π is the viewer's left, 0 the right)
/// and the tangent rotation for a tooth there.
fn ring_point(ring: Ring, th: f64, upper: bool) -> (f64, f64, f64) {
    let x = CX + ring.rx * th.cos();
    if upper {
        let y = CY_UPPER - ring.ry * th.sin();
        let angle = (ring.ry * th.cos()).atan2(ring.rx * th.sin()).to_degrees();
        (x, y, angle)
    } else {
        let y = CY_LOWER + ring.ry * th.sin();
        let angle = (-ring.ry * th.cos()).atan2(ring.rx * th.sin()).to_degrees() + 180.0;
        (x, y, angle)
    }
}

/// Every tooth of a dentition with its position, in Universal order per
/// ring: upper teeth from the patient's right to left, then lower teeth.
pub fn layout(dentition: Dentition) -> Vec<ToothGeom> {
    let mut out = Vec::new();
    for ring in rings(dentition) {
        let n: u8 = if ring.primary { 10 } else { 16 };
        for i in 0..n {
            let th = PI - (f64::from(i) + 0.5) * PI / f64::from(n);
            let (upper_tooth, lower_tooth) = if ring.primary {
                (
                    ToothRef::Primary(char::from(b'A' + i)),
                    ToothRef::Primary(char::from(b'T' - i)),
                )
            } else {
                (ToothRef::Permanent(i + 1), ToothRef::Permanent(32 - i))
            };
            for (tooth, upper) in [(upper_tooth, true), (lower_tooth, false)] {
                let (x, y, angle) = ring_point(ring, th, upper);
                let (w, h) = tooth_size(tooth);
                out.push(ToothGeom {
                    tooth,
                    label: tooth.to_string(),
                    kind: tooth_kind(tooth).unwrap_or(ToothKind::Incisor),
                    x,
                    y,
                    w: w * ring.scale,
                    h: h * ring.scale,
                    angle,
                    upper,
                });
            }
        }
    }
    out
}

/// Planned teeth in the order arrow keys visit them: around the mouth the
/// way Universal numbers run, upper arch from the viewer's left to right,
/// then lower arch from right to left. Mixed rings interleave by position.
pub fn focus_order(geoms: &[ToothGeom], planned: &[&str]) -> Vec<String> {
    let mut teeth: Vec<&ToothGeom> = geoms
        .iter()
        .filter(|g| planned.contains(&g.label.as_str()))
        .collect();
    teeth.sort_by(|a, b| {
        let key = |g: &ToothGeom| if g.upper { g.x } else { -g.x };
        b.upper.cmp(&a.upper).then(key(a).total_cmp(&key(b)))
    });
    teeth.into_iter().map(|g| g.label.clone()).collect()
}

/// The index arrow-key `key` moves to from `current`, wrapping at the ends.
pub fn next_index(len: usize, current: usize, key: &str) -> Option<usize> {
    if len == 0 {
        return None;
    }
    match key {
        "ArrowRight" | "ArrowDown" => Some((current + 1) % len),
        "ArrowLeft" | "ArrowUp" => Some((current + len - 1) % len),
        "Home" => Some(0),
        "End" => Some(len - 1),
        _ => None,
    }
}

/// Occlusal crown outline centered on (0, 0), outer side up (negative y).
pub fn crown_path(kind: ToothKind, w: f64, h: f64) -> String {
    let (a, b) = (w / 2.0, h / 2.0);
    let m = w.min(h);
    let p = |x: f64, y: f64| format!("{x:.2} {y:.2}");
    match kind {
        ToothKind::Molar => {
            // Rounded square with a groove between the cusps on both sides.
            let r = 0.32 * m;
            let dip = 0.07 * h;
            format!(
                "M{} Q{} {} Q{} {} Q{} {} L{} Q{} {} Q{} {} Q{} {} Q{} {} L{} Q{} {}Z",
                p(-a + r, -b),
                p(-a * 0.5, -b - dip),
                p(0.0, -b + dip),
                p(a * 0.5, -b - dip),
                p(a - r, -b),
                p(a, -b),
                p(a, -b + r),
                p(a, b - r),
                p(a, b),
                p(a - r, b),
                p(a * 0.5, b + dip),
                p(0.0, b - dip),
                p(-a * 0.5, b + dip),
                p(-a + r, b),
                p(-a, b),
                p(-a, b - r),
                p(-a, -b + r),
                p(-a, -b),
                p(-a + r, -b),
            )
        }
        ToothKind::Premolar => {
            // Rounded oval, slightly fuller on the cheek and tongue sides.
            let r = 0.45 * m;
            let bulge = 0.06 * h;
            format!(
                "M{} Q{} {} Q{} {} L{} Q{} {} Q{} {} Q{} {} L{} Q{} {}Z",
                p(-a + r, -b),
                p(0.0, -b - bulge),
                p(a - r, -b),
                p(a, -b),
                p(a, -b + r),
                p(a, b - r),
                p(a, b),
                p(a - r, b),
                p(0.0, b + bulge),
                p(-a + r, b),
                p(-a, b),
                p(-a, b - r),
                p(-a, -b + r),
                p(-a, -b),
                p(-a + r, -b),
            )
        }
        ToothKind::Canine => {
            // Teardrop with the cusp tip toward the lip.
            format!(
                "M{} C{} {} {} C{} {} {} C{} {} {} C{} {} {}Z",
                p(0.0, -b),
                p(a * 0.7, -b * 0.84),
                p(a, -b * 0.3),
                p(a, b * 0.2),
                p(a, b * 0.76),
                p(a * 0.5, b),
                p(0.0, b),
                p(-a * 0.5, b),
                p(-a, b * 0.76),
                p(-a, b * 0.2),
                p(-a, -b * 0.3),
                p(-a * 0.7, -b * 0.84),
                p(0.0, -b),
            )
        }
        ToothKind::Incisor => {
            // Wide biting edge on the lip side, narrowing toward the tongue.
            let r = 0.18 * m;
            format!(
                "M{} L{} Q{} {} L{} Q{} {} Q{} {} L{} Q{} {}Z",
                p(-a + r, -b),
                p(a - r, -b),
                p(a, -b),
                p(a, -b + r),
                p(a * 0.62, b - r),
                p(a * 0.5, b),
                p(0.0, b),
                p(-a * 0.5, b),
                p(-a * 0.62, b - r),
                p(-a, -b + r),
                p(-a, -b),
                p(-a + r, -b),
            )
        }
    }
}

// Areas -----------------------------------------------------------------------

/// A procedure on an area rather than one tooth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub upper: bool,
    /// Patient's right (`Some(true)`), left, or the whole arch (`None`).
    pub right: Option<bool>,
}

/// Reads the area names the server writes for quadrant and arch procedures
/// ("upper right", "lower arch") and common short forms. Anything else,
/// including an empty value, is not an area and is never guessed.
pub fn parse_region(value: &str) -> Option<Region> {
    let v = value.trim().to_ascii_lowercase();
    let region = |upper, right| Some(Region { upper, right });
    match v.as_str() {
        "upper right" | "ur" => region(true, Some(true)),
        "upper left" | "ul" => region(true, Some(false)),
        "lower left" | "ll" => region(false, Some(false)),
        "lower right" | "lr" => region(false, Some(true)),
        "upper arch" | "upper" | "maxillary" => region(true, None),
        "lower arch" | "lower" | "mandibular" => region(false, None),
        _ => None,
    }
}

/// A line along the outer ring under a region's teeth, drawn as a wide,
/// light band behind them.
pub fn region_path(region: Region, dentition: Dentition) -> String {
    let ring = outer_ring(dentition);
    // Patient's right is the viewer's left, which is th in (π/2, π).
    let inset = 0.4 * PI / 16.0;
    let (from, to) = match region.right {
        Some(true) => (PI - inset, PI / 2.0 + inset / 4.0),
        Some(false) => (PI / 2.0 - inset / 4.0, inset),
        None => (PI - inset, inset),
    };
    let steps = 24;
    let mut d = String::new();
    for i in 0..=steps {
        let th = from + (to - from) * f64::from(i) / f64::from(steps);
        let (x, y, _) = ring_point(ring, th, region.upper);
        d.push_str(if i == 0 { "M" } else { " L" });
        d.push_str(&format!("{x:.1} {y:.1}"));
    }
    d
}

fn region_name(region: Region) -> &'static str {
    match (region.upper, region.right) {
        (true, Some(true)) => "upper right",
        (true, Some(false)) => "upper left",
        (false, Some(false)) => "lower left",
        (false, Some(true)) => "lower right",
        (true, None) => "upper arch",
        (false, None) => "lower arch",
    }
}

// Grouping and labels ---------------------------------------------------------

/// "30" or "K" when the value names one tooth.
pub fn single_label(value: &str) -> Option<String> {
    match value.trim().parse::<ToothRef>() {
        Ok(t @ (ToothRef::Permanent(_) | ToothRef::Primary(_))) => Some(t.to_string()),
        _ => None,
    }
}

/// Items grouped by the single tooth they are on, as indexes in plan
/// order. Teeth appear in the order of their first item; items without a
/// valid single tooth are left out.
pub fn group_by_tooth<T>(items: &[T], tooth_of: impl Fn(&T) -> &str) -> Vec<(String, Vec<usize>)> {
    let mut out: Vec<(String, Vec<usize>)> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let Some(label) = single_label(tooth_of(item)) else {
            continue;
        };
        match out.iter_mut().find(|(l, _)| *l == label) {
            Some((_, list)) => list.push(i),
            None => out.push((label, vec![i])),
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

/// "Tooth 30, lower right, 3 planned procedures: Root canal, Build-up, Crown".
pub fn accessible_name(label: &str, count: usize, names: &[&str]) -> String {
    let side = label
        .parse::<ToothRef>()
        .ok()
        .and_then(tooth_location)
        .and_then(|l| l.split(',').next().map(|s| format!(", {s}")))
        .unwrap_or_default();
    let plural = if count == 1 {
        "procedure"
    } else {
        "procedures"
    };
    let list = if names.is_empty() {
        String::new()
    } else {
        format!(": {}", names.join(", "))
    };
    format!("Tooth {label}{side}, {count} planned {plural}{list}")
}

/// Where the procedure panel sits over the chart, in percent of the chart
/// box: horizontally centered on the tooth (kept clear of the edges) and on
/// the inner side of the arch, so it never covers the tooth itself.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PanelAnchor {
    pub left: f64,
    /// Set for upper teeth (panel below the tooth).
    pub top: Option<f64>,
    /// Set for lower teeth (panel above the tooth).
    pub bottom: Option<f64>,
}

pub fn panel_anchor(g: &ToothGeom) -> PanelAnchor {
    let (vx, vy, vw, vh) = VIEW_BOX;
    let left = ((g.x - vx) / vw * 100.0).clamp(30.0, 70.0);
    let reach = g.w.max(g.h) / 2.0 + 8.0;
    if g.upper {
        PanelAnchor {
            left,
            top: Some(((g.y + reach - vy) / vh * 100.0).clamp(0.0, 100.0)),
            bottom: None,
        }
    } else {
        PanelAnchor {
            left,
            top: None,
            bottom: Some(((vy + vh - (g.y - reach)) / vh * 100.0).clamp(0.0, 100.0)),
        }
    }
}

// Components ------------------------------------------------------------------

static NEXT_CHART: AtomicUsize = AtomicUsize::new(0);

fn focus_by_id(id: &str) {
    if let Some(el) = document().get_element_by_id(id) {
        let _ = js_sys::Reflect::get(&el, &"focus".into())
            .ok()
            .and_then(|f| leptos::wasm_bindgen::JsCast::dyn_into::<js_sys::Function>(f).ok())
            .map(|f| f.call0(&el));
    }
}

fn capitalized(s: &str) -> String {
    let mut chars = s.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// The chart. `selected` rings a tooth (the procedures screen passes the
/// current procedure's tooth); `on_select` runs when a planned tooth is
/// chosen. With `procedures` and `on_open`, the full chart also opens a
/// panel listing the chosen tooth's procedures, each a link that calls
/// `on_open` with the procedure reference. `compact` is the small rail
/// chart: no labels, legend or panel.
#[component]
pub fn ToothChart(
    planned: Vec<Planned>,
    #[prop(into)] selected: Signal<Option<String>>,
    on_select: Callback<String>,
    #[prop(optional)] compact: bool,
    #[prop(optional)] procedures: Vec<ProcedureFactView>,
    #[prop(optional)] on_open: Option<Callback<String>>,
) -> impl IntoView {
    let chart = NEXT_CHART.fetch_add(1, Ordering::Relaxed);
    let tooth_id = move |label: &str| format!("dc-chart{chart}-tooth-{label}");

    let labels: Vec<&str> = planned.iter().map(|p| p.label.as_str()).collect();
    let dentition = dentition(labels.iter().copied());
    let geoms = layout(dentition);
    let order = focus_order(&geoms, &labels);
    let counts: HashMap<String, usize> = planned
        .iter()
        .map(|p| (p.label.clone(), p.procedures))
        .collect();
    let by_tooth: HashMap<String, Vec<ProcedureFactView>> =
        group_by_tooth(&procedures, |p| p.tooth.as_str())
            .into_iter()
            .map(|(label, idx)| {
                (
                    label,
                    idx.into_iter().map(|i| procedures[i].clone()).collect(),
                )
            })
            .collect();
    let mut regions: Vec<Region> = procedures
        .iter()
        .filter(|p| single_label(&p.tooth).is_none())
        .filter_map(|p| parse_region(&p.tooth))
        .collect();
    regions.dedup();
    let with_panel = !compact && on_open.is_some();

    // Which tooth has the panel open, and which one holds the roving tabindex.
    let open = RwSignal::new(None::<String>);
    let active = RwSignal::new(
        selected
            .get_untracked()
            .filter(|s| order.contains(s))
            .or_else(|| order.first().cloned()),
    );
    let order = StoredValue::new(order);
    Effect::new(move |_| {
        if let Some(s) = selected.get()
            && order.with_value(|o| o.contains(&s))
        {
            active.set(Some(s));
        }
    });

    let activate = move |label: String| {
        active.set(Some(label.clone()));
        if with_panel {
            open.update(|o| {
                *o = if o.as_deref() == Some(label.as_str()) {
                    None
                } else {
                    Some(label.clone())
                }
            });
        }
        on_select.run(label);
    };
    let close_panel = move || {
        if let Some(label) = open.get_untracked() {
            open.set(None);
            focus_by_id(&tooth_id(&label));
        }
    };

    let regions_view = regions
        .iter()
        .map(|r| view! { <path class="region" d=region_path(*r, dentition) /> })
        .collect_view();

    let (marked, plain): (Vec<&ToothGeom>, Vec<&ToothGeom>) =
        geoms.iter().partition(|g| counts.contains_key(&g.label));

    let plain_view = plain
        .into_iter()
        .map(|g| {
            view! {
                <g class="tooth" class:is-primary=matches!(g.tooth, ToothRef::Primary(_)) class:is-narrow=g.is_narrow()>
                    <path class="body" d=crown_path(g.kind, g.w, g.h) transform=g.transform() />
                    <text x=format!("{:.2}", g.label_center().0) y=format!("{:.2}", g.label_center().1 + 0.5)
                        text-anchor="middle" dominant-baseline="middle">{g.label.clone()}</text>
                </g>
            }
        })
        .collect_view();

    let marked_view = marked
        .into_iter()
        .map(|g| {
            let label = g.label.clone();
            let count = counts.get(&label).copied().unwrap_or(1);
            let names: Vec<&str> = by_tooth
                .get(&label)
                .map(|list| list.iter().map(|p| p.name.as_str()).collect())
                .unwrap_or_default();
            let name = accessible_name(&label, count, &names);
            let is_selected = {
                let label = label.clone();
                move || {
                    open.with(|o| o.as_deref() == Some(label.as_str()))
                        || selected.with(|s| s.as_deref() == Some(label.as_str()))
                }
            };
            let pressed = {
                let is_selected = is_selected.clone();
                move || if is_selected() { "true" } else { "false" }
            };
            let tabindex = {
                let label = label.clone();
                move || {
                    if active.with(|a| a.as_deref() == Some(label.as_str())) {
                        "0"
                    } else {
                        "-1"
                    }
                }
            };
            let on_click = {
                let label = label.clone();
                move |_| activate(label.clone())
            };
            let on_key = {
                let label = label.clone();
                move |ev: KeyboardEvent| {
                    let key = ev.key();
                    match key.as_str() {
                        "Enter" | " " => {
                            ev.prevent_default();
                            activate(label.clone());
                        }
                        "Escape" => close_panel(),
                        _ => {
                            let next = order.with_value(|o| {
                                let current = o.iter().position(|l| *l == label)?;
                                next_index(o.len(), current, &key).map(|i| o[i].clone())
                            });
                            if let Some(next) = next {
                                ev.prevent_default();
                                active.set(Some(next.clone()));
                                focus_by_id(&tooth_id(&next));
                            }
                        }
                    }
                }
            };
            let (bx, by) = g.badge_center();
            let transform = g.transform();
            view! {
                <g class="tooth is-planned" class:is-selected=is_selected class:is-narrow=g.is_narrow() id=tooth_id(&label)
                    role="button" tabindex=tabindex aria-label=name aria-pressed=pressed
                    on:click=on_click on:keydown=on_key>
                    <circle class="hit" cx=format!("{:.2}", g.x) cy=format!("{:.2}", g.y) r=HIT_RADIUS />
                    <path class="ring" d=crown_path(g.kind, g.w + 2.0 * RING_GAP, g.h + 2.0 * RING_GAP)
                        transform=transform.clone() />
                    <path class="body" d=crown_path(g.kind, g.w, g.h) transform=transform />
                    <text x=format!("{:.2}", g.label_center().0) y=format!("{:.2}", g.label_center().1 + 0.5)
                        text-anchor="middle" dominant-baseline="middle">{label.clone()}</text>
                    <circle class="badge" cx=format!("{bx:.2}") cy=format!("{by:.2}") r=BADGE_RADIUS />
                    <text class="badge-text" x=format!("{bx:.2}") y=format!("{:.2}", by + 0.5)
                        text-anchor="middle" dominant-baseline="middle">{count.to_string()}</text>
                </g>
            }
        })
        .collect_view();

    let show_hint = !compact && !planned.is_empty();
    let hint_y = (CY_UPPER + CY_LOWER) / 2.0;
    let (vx, vy, vw, vh) = VIEW_BOX;

    // Procedure panel for the open tooth.
    let geoms_sv = StoredValue::new(geoms.clone());
    let by_tooth = StoredValue::new(by_tooth);
    let panel_ref = NodeRef::<leptos::html::Div>::new();
    Effect::new(move |_| {
        let (Some(el), Some(label)) = (panel_ref.get(), open.get()) else {
            return;
        };
        let Some(anchor) =
            geoms_sv.with_value(|gs| gs.iter().find(|g| g.label == label).map(panel_anchor))
        else {
            return;
        };
        // Set through the style object: the page's CSP has no inline styles.
        let style = leptos::web_sys::HtmlElement::style(&el);
        let _ = style.set_property("left", &format!("{:.1}%", anchor.left));
        match (anchor.top, anchor.bottom) {
            (Some(top), _) => {
                let _ = style.set_property("top", &format!("{top:.1}%"));
                let _ = style.remove_property("bottom");
            }
            (_, Some(bottom)) => {
                let _ = style.set_property("bottom", &format!("{bottom:.1}%"));
                let _ = style.remove_property("top");
            }
            _ => {}
        }
    });
    let panel = move || {
        let label = open.get()?;
        let list = by_tooth.with_value(|m| m.get(&label).cloned().unwrap_or_default());
        let place = label
            .parse::<ToothRef>()
            .ok()
            .and_then(tooth_location)
            .map(|l| capitalized(&l))
            .unwrap_or_default();
        let heading_id = format!("dc-chart{chart}-panel-title");
        let labelled_by = heading_id.clone();
        let items = if list.is_empty() {
            view! { <p class="p-tooth-panel-empty">"Your dental team will go over the work on this tooth."</p> }
                .into_any()
        } else {
            view! {
                <ul class="p-tooth-panel-list">
                    {list.into_iter().map(|p| {
                        let r = p.procedure_ref.clone();
                        let surfaces = p.surfaces.trim().to_owned();
                        let meta = if surfaces.is_empty() {
                            format!("Phase {}", p.phase)
                        } else {
                            format!("Phase {} · surfaces {surfaces}", p.phase)
                        };
                        view! {
                            <li>
                                <button type="button" class="p-tooth-panel-item"
                                    on:click=move |_| {
                                        if let Some(cb) = on_open {
                                            cb.run(r.clone());
                                        }
                                    }>
                                    <span class="p-tooth-panel-name">{p.name}</span>
                                    <span class="p-tooth-panel-meta">{meta}</span>
                                    <span class="p-tooth-panel-go" aria-hidden="true">"›"</span>
                                </button>
                            </li>
                        }
                    }).collect_view()}
                </ul>
            }
            .into_any()
        };
        Some(view! {
            <div class="p-tooth-panel" node_ref=panel_ref role="dialog" aria-modal="false"
                aria-labelledby=labelled_by
                on:keydown=move |ev: KeyboardEvent| {
                    if ev.key() == "Escape" {
                        ev.prevent_default();
                        close_panel();
                    }
                }>
                <div class="p-tooth-panel-head">
                    <div>
                        <div class="p-tooth-panel-title" id=heading_id>{format!("Tooth {label}")}</div>
                        <div class="p-tooth-panel-place">{place}</div>
                    </div>
                    <button type="button" class="p-tooth-panel-close" aria-label="Close"
                        on:click=move |_| close_panel()>"×"</button>
                </div>
                {items}
            </div>
        })
    };

    let summary = {
        let mut parts = Vec::new();
        if planned.is_empty() {
            parts.push("No single teeth have planned work.".to_owned());
        } else {
            parts.push(format!(
                "Planned teeth: {}.",
                order.with_value(|o| o.join(", "))
            ));
        }
        if !regions.is_empty() {
            let names: Vec<&str> = regions.iter().map(|r| region_name(*r)).collect();
            parts.push(format!("Planned work on an area: {}.", names.join(", ")));
        }
        parts.join(" ")
    };
    let legend_rings = match dentition {
        Dentition::Mixed => {
            Some("Outer ring: adult teeth, numbered. Inner ring: baby teeth, lettered.")
        }
        Dentition::Primary => Some("Baby teeth, lettered A to T."),
        Dentition::Permanent => None,
    };

    view! {
        <div class="p-chart" class:is-compact=compact class:is-mixed=dentition == Dentition::Mixed>
            {(!compact).then(|| view! {
                <div class="p-chart-labels" aria-hidden="true"><span>"Your right"</span><span>"Upper"</span><span>"Your left"</span></div>
            })}
            <div class="p-chart-arch">
                <svg class="p-arch" viewBox=format!("{vx} {vy} {vw} {vh}") role="group"
                    aria-label="Tooth chart. Highlighted teeth have planned work.">
                    <g aria-hidden="true">
                        {regions_view}
                        {plain_view}
                        {show_hint.then(|| view! {
                            <text class="hint" class:is-hidden=move || open.with(Option::is_some)
                                x=CX y=hint_y text-anchor="middle" dominant-baseline="middle">
                                "Tap a highlighted tooth"
                            </text>
                        })}
                    </g>
                    <g>{marked_view}</g>
                </svg>
                {with_panel.then_some(panel)}
            </div>
            {(!compact).then(|| view! { <div class="p-chart-lower" aria-hidden="true">"Lower"</div> })}
            {(!compact && !planned.is_empty()).then(|| view! {
                <div class="p-chart-legend">
                    <span class="p-chart-key" aria-hidden="true"><span class="p-chart-key-badge">"2"</span></span>
                    <span>"Planned work. The number is how many steps on that tooth."</span>
                </div>
            })}
            {(!compact).then_some(legend_rings).flatten().map(|text| view! {
                <div class="p-chart-legend p-chart-legend-rings">{text}</div>
            })}
            <p class="visually-hidden">{summary}</p>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find<'a>(geoms: &'a [ToothGeom], label: &str) -> &'a ToothGeom {
        geoms.iter().find(|g| g.label == label).expect(label)
    }

    /// A two-digit number is about 11 by 7 chart units at 9.5 px and 10 by
    /// 6 at the 8.5 px of narrow crowns. Its box, turned into the crown's
    /// own frame, must stay inside the incisor outline with a unit to spare.
    #[test]
    fn incisor_numbers_stay_inside_the_crown() {
        let geoms = layout(Dentition::Permanent);
        for label in ["7", "8", "9", "10", "23", "24", "25", "26"] {
            let g = find(&geoms, label);
            let (hw, hh) = if g.is_narrow() {
                (5.0, 3.0)
            } else {
                (5.5, 3.5)
            };
            let (a, b) = (g.w / 2.0, g.h / 2.0);
            let r = 0.18 * g.w.min(g.h);
            // Half width of the outline at local y (outer side is -y).
            let half_width = |y: f64| {
                let t = ((y - (-b + r)) / (2.0 * (b - r))).clamp(0.0, 1.0);
                a + (0.62 * a - a) * t
            };
            let (cx, cy) = g.label_center();
            let rad = (-g.angle).to_radians();
            for (ox, oy) in [(-hw, -hh), (hw, -hh), (-hw, hh), (hw, hh)] {
                let (px, py) = (cx + ox - g.x, cy + oy - g.y);
                let lx = px * rad.cos() - py * rad.sin();
                let ly = px * rad.sin() + py * rad.cos();
                assert!(ly.abs() <= b - 1.0, "tooth {label}: {ly} outside {b}");
                assert!(
                    lx.abs() <= half_width(ly) - 1.0,
                    "tooth {label}: {lx} wider than {}",
                    half_width(ly)
                );
            }
        }
    }

    #[test]
    fn dentition_follows_planned_teeth() {
        assert_eq!(dentition(["30", "14"]), Dentition::Permanent);
        assert_eq!(dentition(["K", "t"]), Dentition::Primary);
        assert_eq!(dentition(["K", "3"]), Dentition::Mixed);
        assert_eq!(dentition(Vec::<&str>::new()), Dentition::Permanent);
        assert_eq!(dentition(["upper right", "x"]), Dentition::Permanent);
    }

    #[test]
    fn layouts_have_every_tooth_once() {
        assert_eq!(layout(Dentition::Permanent).len(), 32);
        assert_eq!(layout(Dentition::Primary).len(), 20);
        let mixed = layout(Dentition::Mixed);
        assert_eq!(mixed.len(), 52);
        let mut labels: Vec<&str> = mixed.iter().map(|g| g.label.as_str()).collect();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), 52);
    }

    #[test]
    fn patient_right_is_on_the_viewer_left() {
        let g = layout(Dentition::Mixed);
        // Upper: 1 (patient's right) left of 16. Lower: 32 (right) left of 17.
        assert!(find(&g, "1").x < find(&g, "16").x);
        assert!(find(&g, "32").x < find(&g, "17").x);
        assert!(find(&g, "A").x < find(&g, "J").x);
        assert!(find(&g, "T").x < find(&g, "K").x);
        assert!(find(&g, "8").y < find(&g, "25").y);
        assert!(find(&g, "1").upper && !find(&g, "30").upper);
    }

    #[test]
    fn molars_are_wider_than_incisors() {
        let g = layout(Dentition::Permanent);
        assert!(find(&g, "30").w > find(&g, "25").w);
        assert!(find(&g, "3").w > find(&g, "8").w);
        assert_eq!(find(&g, "19").kind, ToothKind::Molar);
        assert_eq!(find(&g, "6").kind, ToothKind::Canine);
    }

    #[test]
    fn primary_ring_sits_inside_permanent_ring() {
        let g = layout(Dentition::Mixed);
        let spread = |l: &str| {
            let t = find(&g, l);
            (t.x - CX).abs()
        };
        assert!(spread("A") < spread("1"));
        assert!(find(&g, "E").y > find(&g, "8").y);
    }

    #[test]
    fn crowns_turn_with_the_arch() {
        let g = layout(Dentition::Permanent);
        // Front teeth face the lip: up for upper, down for lower.
        let (_, up) = find(&g, "8").outward();
        let (_, down) = find(&g, "24").outward();
        assert!(up < -0.9 && down > 0.9);
        // Back teeth on the viewer's left face left.
        let (left, _) = find(&g, "1").outward();
        let (left_lower, _) = find(&g, "32").outward();
        assert!(left < -0.9 && left_lower < -0.9);
    }

    #[test]
    fn everything_fits_the_view_box() {
        let (vx, vy, vw, vh) = VIEW_BOX;
        for d in [Dentition::Permanent, Dentition::Primary, Dentition::Mixed] {
            for g in layout(d) {
                let (bx, by) = g.badge_center();
                let reach = g.w.max(g.h) / 2.0 + RING_GAP;
                for (x, y, r) in [
                    (g.x, g.y, reach),
                    (bx, by, BADGE_RADIUS),
                    (g.x, g.y, HIT_RADIUS),
                ] {
                    assert!(x - r >= vx && x + r <= vx + vw, "{} x in {d:?}", g.label);
                    assert!(y - r >= vy && y + r <= vy + vh, "{} y in {d:?}", g.label);
                }
            }
        }
    }

    #[test]
    fn neighbours_do_not_overlap() {
        for d in [Dentition::Permanent, Dentition::Primary, Dentition::Mixed] {
            let g = layout(d);
            for (i, a) in g.iter().enumerate() {
                for b in &g[i + 1..] {
                    let dist = ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt();
                    let min = a.w.min(a.h) / 2.0 + b.w.min(b.h) / 2.0;
                    assert!(dist >= min, "{} and {} overlap in {d:?}", a.label, b.label);
                }
            }
        }
    }

    #[test]
    fn focus_order_runs_around_the_mouth() {
        let g = layout(Dentition::Mixed);
        let order = focus_order(&g, &["19", "K", "3", "T", "14"]);
        assert_eq!(order, ["3", "14", "19", "K", "T"]);
        let g = layout(Dentition::Permanent);
        assert_eq!(focus_order(&g, &["30", "8", "14"]), ["8", "14", "30"]);
    }

    #[test]
    fn arrow_keys_wrap() {
        assert_eq!(next_index(3, 2, "ArrowRight"), Some(0));
        assert_eq!(next_index(3, 0, "ArrowLeft"), Some(2));
        assert_eq!(next_index(3, 1, "ArrowDown"), Some(2));
        assert_eq!(next_index(3, 1, "ArrowUp"), Some(0));
        assert_eq!(next_index(3, 1, "End"), Some(2));
        assert_eq!(next_index(3, 1, "Home"), Some(0));
        assert_eq!(next_index(3, 1, "a"), None);
        assert_eq!(next_index(0, 0, "ArrowRight"), None);
    }

    #[test]
    fn groups_procedures_by_tooth_in_plan_order() {
        let plan = [
            ("Root canal", "30"),
            ("Cleaning", ""),
            ("Filling", "k"),
            ("Build-up", " 30 "),
            ("Scaling", "upper right"),
            ("Crown", "30"),
            ("Odd", "33"),
        ];
        let groups = group_by_tooth(&plan, |p| p.1);
        assert_eq!(
            groups,
            vec![("30".to_owned(), vec![0, 3, 5]), ("K".to_owned(), vec![2])]
        );
    }

    #[test]
    fn regions_are_read_never_guessed() {
        assert_eq!(
            parse_region("upper right"),
            Some(Region {
                upper: true,
                right: Some(true)
            })
        );
        assert_eq!(
            parse_region("Lower arch"),
            Some(Region {
                upper: false,
                right: None
            })
        );
        assert_eq!(parse_region(""), None);
        assert_eq!(parse_region("30"), None);
        assert_eq!(parse_region("33"), None);
        let path = region_path(parse_region("UR").unwrap(), Dentition::Permanent);
        assert!(path.starts_with('M'));
        // The band for the patient's right stays on the viewer's left half.
        for point in path.trim_start_matches('M').split(" L") {
            let x: f64 = point.split(' ').next().unwrap().parse().unwrap();
            assert!(x <= CX + 1.0, "{point}");
        }
    }

    #[test]
    fn names_and_locations() {
        assert_eq!(
            accessible_name("30", 3, &["Root canal", "Build-up", "Crown"]),
            "Tooth 30, lower right, 3 planned procedures: Root canal, Build-up, Crown"
        );
        assert_eq!(
            accessible_name("K", 1, &[]),
            "Tooth K, lower left, 1 planned procedure"
        );
        assert_eq!(
            tooth_location(ToothRef::Permanent(14)).as_deref(),
            Some("upper left, back")
        );
        assert_eq!(
            tooth_location(ToothRef::Primary('T')).as_deref(),
            Some("lower right, back")
        );
        assert_eq!(tooth_location(ToothRef::None), None);
        assert_eq!(single_label(" k "), Some("K".to_owned()));
        assert_eq!(single_label("upper arch"), None);
    }

    #[test]
    fn panel_stays_inside_and_on_the_inner_side() {
        let g = layout(Dentition::Permanent);
        let upper = panel_anchor(find(&g, "1"));
        assert!(upper.top.is_some() && upper.bottom.is_none());
        assert!((30.0..=70.0).contains(&upper.left));
        let lower = panel_anchor(find(&g, "30"));
        assert!(lower.bottom.is_some() && lower.top.is_none());
    }

    #[test]
    fn crown_paths_are_closed() {
        for kind in [
            ToothKind::Molar,
            ToothKind::Premolar,
            ToothKind::Canine,
            ToothKind::Incisor,
        ] {
            let d = crown_path(kind, 28.0, 30.0);
            assert!(d.starts_with('M') && d.ends_with('Z'), "{kind:?}");
            assert!(!d.contains("NaN"));
        }
    }
}
