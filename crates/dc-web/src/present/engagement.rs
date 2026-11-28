//! In-office engagement reports (milestone M8): how long each patient
//! screen is open and when narration starts, sent to
//! `POST /api/present/analytics`. Only the screen name and a duration leave
//! the page. A report that fails is dropped; the patient never sees it.

use super::{Tab, api};
use dc_core::analytics::{PresentEvent, PresentScreen};
use leptos::leptos_dom::helpers::{IntervalHandle, set_interval_with_handle};
use leptos::prelude::*;
use leptos::task::spawn_local;
use std::time::Duration;

/// How often the open screen's time is reported, so closing the browser
/// loses at most this much.
const REPORT_EVERY: Duration = Duration::from_secs(15);

pub fn screen_of(tab: Tab) -> PresentScreen {
    match tab {
        Tab::AtAGlance => PresentScreen::AtAGlance,
        Tab::Journey => PresentScreen::Journey,
        Tab::Procedures => PresentScreen::Procedures,
        Tab::Costs => PresentScreen::Costs,
        Tab::NextSteps => PresentScreen::NextSteps,
    }
}

fn send(event: PresentEvent) {
    spawn_local(async move {
        let _ = api::report(&event).await;
    });
}

/// Time on the current screen since the last report.
#[derive(Clone, Copy)]
pub struct Tracker {
    /// The screen being timed and when timing (re)started, in ms; `None`
    /// while the page is hidden.
    current: StoredValue<(Tab, Option<f64>)>,
}

impl Tracker {
    /// Starts timing `tab`, reports on every screen change and every
    /// [`REPORT_EVERY`], and stops when the owner is cleaned up.
    pub fn start(tab: RwSignal<Tab>) -> Self {
        let tracker = Tracker {
            current: StoredValue::new((tab.get_untracked(), Some(js_sys::Date::now()))),
        };
        Effect::new(move |_| {
            let next = tab.get();
            tracker.flush();
            tracker.current.update_value(|(t, _)| *t = next);
        });
        let handle: Option<IntervalHandle> =
            set_interval_with_handle(move || tracker.flush(), REPORT_EVERY).ok();
        on_cleanup(move || {
            if let Some(h) = handle {
                h.clear();
            }
        });
        tracker
    }

    /// Takes the time on the current screen as a report and restarts the
    /// clock. `None` when less than a second has passed.
    fn take(&self) -> Option<PresentEvent> {
        let now = js_sys::Date::now();
        let (tab, Some(since)) = self.current.try_get_value()? else {
            return None;
        };
        let seconds = ((now - since) / 1000.0).floor();
        if seconds < 1.0 {
            return None;
        }
        self.current
            .update_value(|(_, start)| *start = Some(since + seconds * 1000.0));
        Some(PresentEvent::View {
            screen: screen_of(tab),
            seconds: seconds.min(f64::from(u32::MAX)) as u32,
        })
    }

    /// Reports the time on the current screen and restarts the clock.
    pub fn flush(&self) {
        if let Some(event) = self.take() {
            send(event);
        }
    }

    /// Reports the last stretch and waits for it, so leaving Present mode
    /// does not end the session before the report arrives.
    pub async fn finish(&self) {
        if let Some(event) = self.take() {
            let _ = api::report(&event).await;
        }
    }

    /// The page went out of sight: report and stop the clock.
    pub fn pause(&self) {
        self.flush();
        self.current.update_value(|(_, start)| *start = None);
    }

    /// The page is visible again.
    pub fn resume(&self) {
        self.current
            .update_value(|(_, start)| *start = Some(js_sys::Date::now()));
    }
}

/// Reports that narration started on `tab`.
pub fn narration_started(tab: Tab) {
    send(PresentEvent::Narration {
        screen: screen_of(tab),
    });
}

pub fn use_tracker() -> Option<Tracker> {
    use_context::<Tracker>()
}
