//! A list that re-reads itself while any row is still changing, such as a
//! presentation that is generating. Rows update in place; only a change of
//! load state rebuilds the page around them.

use crate::api::Failure;
use leptos::leptos_dom::helpers::{IntervalHandle, set_interval_with_handle};
use leptos::prelude::*;
use leptos::task::spawn_local;
use std::future::Future;
use std::time::Duration;

/// How often a list is re-read while a row is unfinished.
const POLL_EVERY: Duration = Duration::from_secs(3);

/// What a polled page shows around its rows.
#[derive(Clone, PartialEq)]
pub enum Load {
    Loading,
    Failed(String),
    Ready,
}

/// Rows from `fetch`, re-read every few seconds while `unfinished` holds for
/// any of them. Polling stops once every row is finished, and on unmount.
pub struct PolledRows<T: Send + Sync + 'static, F> {
    pub rows: RwSignal<Vec<T>>,
    pub load: RwSignal<Load>,
    in_flight: StoredValue<bool>,
    poller: StoredValue<Option<IntervalHandle>>,
    fetch: F,
    unfinished: fn(&T) -> bool,
}

impl<T: Send + Sync + 'static, F: Copy> Clone for PolledRows<T, F> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: Send + Sync + 'static, F: Copy> Copy for PolledRows<T, F> {}

impl<T, F, Fut> PolledRows<T, F>
where
    T: Clone + PartialEq + Send + Sync + 'static,
    F: Fn() -> Fut + Copy + Send + Sync + 'static,
    Fut: Future<Output = Result<Vec<T>, Failure>> + 'static,
{
    /// Starts the first read and ties the poller to the current owner, so it
    /// is cleared when the component unmounts.
    pub fn start(fetch: F, unfinished: fn(&T) -> bool) -> Self {
        let state = PolledRows {
            rows: RwSignal::new(Vec::new()),
            load: RwSignal::new(Load::Loading),
            in_flight: StoredValue::new(false),
            poller: StoredValue::new(None),
            fetch,
            unfinished,
        };
        state.read();
        on_cleanup(move || state.stop_polling());
        state
    }

    fn stop_polling(self) {
        if let Some(handle) = self.poller.try_update_value(Option::take).flatten() {
            handle.clear();
        }
    }

    fn read(self) {
        if self.in_flight.try_get_value() != Some(false) {
            return;
        }
        self.in_flight.set_value(true);
        spawn_local(async move {
            let result = (self.fetch)().await;
            // The page may have been left while the request ran.
            if self.in_flight.try_set_value(false).is_some() {
                return;
            }
            match result {
                Ok(fresh) => {
                    let changing = fresh.iter().any(self.unfinished);
                    if self.rows.with_untracked(|old| *old != fresh) {
                        self.rows.set(fresh);
                    }
                    if self.load.get_untracked() != Load::Ready {
                        self.load.set(Load::Ready);
                    }
                    if !changing {
                        self.stop_polling();
                    } else if self.poller.with_value(Option::is_none)
                        && let Ok(handle) =
                            set_interval_with_handle(move || self.read(), POLL_EVERY)
                    {
                        self.poller.set_value(Some(handle));
                    }
                }
                // A failed poll keeps the rows already shown.
                Err(failure) => {
                    if self.load.get_untracked() != Load::Ready {
                        self.load.set(Load::Failed(failure.message));
                    }
                }
            }
        });
    }
}
