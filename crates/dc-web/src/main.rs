//! DentalClarity web app.
//!
//! Client-side rendered with Leptos and served by `dc-server` from the same
//! origin. Patient-facing screens are added under `/present` in milestone M5.

mod api;
mod format;
mod icons;
mod pages;
mod session;
mod shell;

use dc_core::api::SessionState;
use leptos::prelude::*;
use leptos_router::components::{Route, Router, Routes};
use leptos_router::path;
use session::SessionContext;

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(App);
}

#[component]
fn App() -> impl IntoView {
    let session = SessionContext {
        info: RwSignal::new(None),
    };
    provide_context(session);
    api::on_session_lost(move || session.reload());
    session.reload();

    view! {
        {move || match session.info.get() {
            None => view! { <div class="auth-page"><p class="auth-text">"Loading…"</p></div> }.into_any(),
            Some(Err(failure)) => view! {
                <div class="auth-page">
                    <div class="notice error" role="alert">"DentalClarity is not reachable. " {failure.message}</div>
                </div>
            }.into_any(),
            Some(Ok(info)) => match info.state {
                SessionState::SetupRequired => view! { <session::Setup /> }.into_any(),
                SessionState::SignedOut => view! { <session::SignIn practice_name=info.practice_name /> }.into_any(),
                SessionState::Locked => view! { <session::Locked practice_name=info.practice_name /> }.into_any(),
                SessionState::Staff | SessionState::Visitor => view! { <Workspace /> }.into_any(),
                // Handled by `SessionContext::reload`, which starts the visit.
                SessionState::NewVisitor => view! { <div class="auth-page"><p class="auth-text">"Loading…"</p></div> }.into_any(),
            },
        }}
    }
}

#[component]
fn Workspace() -> impl IntoView {
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
