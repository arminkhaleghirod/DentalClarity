//! DentalClarity web app.
//!
//! Client-side rendered with Leptos and served by `dc-server` from the same
//! origin. Patient-facing screens are added under `/present` in milestone M5.

mod api;
mod format;
mod icons;
mod pages;
mod shell;

use leptos::prelude::*;
use leptos_router::components::{Route, Router, Routes};
use leptos_router::path;

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(App);
}

#[component]
fn App() -> impl IntoView {
    view! {
        <Router>
            <shell::StaffShell>
                <Routes fallback=|| view! { <pages::NotFound /> }>
                    <Route path=path!("/") view=pages::Dashboard />
                    <Route path=path!("/patients") view=|| view! { <pages::Upcoming title="Patients" milestone="M3" /> } />
                    <Route path=path!("/presentations") view=|| view! { <pages::Upcoming title="Presentations" milestone="M5" /> } />
                    <Route path=path!("/analytics") view=|| view! { <pages::Upcoming title="Analytics" milestone="M8" /> } />
                    <Route path=path!("/settings") view=|| view! { <pages::Upcoming title="Settings" milestone="M3" /> } />
                </Routes>
            </shell::StaffShell>
        </Router>
    }
}
