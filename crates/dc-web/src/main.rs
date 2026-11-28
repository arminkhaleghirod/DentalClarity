//! DentalClarity web app.
//!
//! Client-side rendered with Leptos and served by `dc-server` from the same
//! origin. Patient-facing screens are added under `/present` in milestone M5.

mod api;
mod create;
mod editor;
mod format;
mod history;
mod icons;
mod motion;
mod pages;
mod patients;
mod poll;
mod present;
mod presentations;
mod progress;
mod session;
mod settings;
mod shell;

use dc_core::api::SessionState;
use leptos::prelude::*;
use leptos_router::components::{Outlet, ParentRoute, Route, Router, Routes};
use leptos_router::path;
use session::SessionContext;

fn main() {
    console_error_panic_hook::set_once();
    // `/present` is the patient tree: no staff session, shell or router.
    if present::is_present_path() {
        leptos::mount::mount_to_body(present::PresentApp);
    } else {
        leptos::mount::mount_to_body(App);
    }
}

#[component]
fn App() -> impl IntoView {
    let session = SessionContext::new();
    provide_context(session);
    api::on_session_lost(move || session.reload());
    session.reload();

    // Only a change of screen re-renders the tree. A practice name edited in
    // Settings updates the session without remounting the workspace.
    let gate = Memo::new(move |_| match session.info.get() {
        None => Gate::Loading,
        Some(Err(failure)) => Gate::Unreachable(failure.message),
        Some(Ok(info)) => match info.state {
            SessionState::SetupRequired => Gate::Setup,
            SessionState::SignedOut => Gate::SignIn(info.practice_name),
            SessionState::Locked => Gate::Locked(info.practice_name),
            SessionState::Staff | SessionState::Visitor => Gate::Workspace,
            // Handled by `SessionContext::reload`, which starts the visit.
            SessionState::NewVisitor => Gate::Loading,
        },
    });

    view! {
        {move || match gate.get() {
            Gate::Loading => view! {
                <div class="auth-page">
                    <motion::SkeletonCard label="Loading DentalClarity" lines=3 class="auth-skeleton" />
                </div>
            }.into_any(),
            Gate::Unreachable(message) => view! {
                <div class="auth-page">
                    <div class="notice error" role="alert">"DentalClarity is not reachable. " {message}</div>
                </div>
            }.into_any(),
            Gate::Setup => view! { <session::Setup /> }.into_any(),
            Gate::SignIn(practice_name) => view! { <session::SignIn practice_name /> }.into_any(),
            Gate::Locked(practice_name) => view! { <session::Locked practice_name /> }.into_any(),
            Gate::Workspace => view! { <Workspace /> }.into_any(),
        }}
    }
}

#[derive(Clone, PartialEq)]
enum Gate {
    Loading,
    Unreachable(String),
    Setup,
    SignIn(String),
    Locked(String),
    Workspace,
}

/// Staff routes. Most pages sit inside the sidebar shell; the create screen
/// and the presentation preview use the full width (mockups 2b and 2c).
#[component]
fn Workspace() -> impl IntoView {
    view! {
        <Router>
            <RouteFocus />
            <Routes transition=true fallback=|| view! { <shell::StaffShell><pages::NotFound /></shell::StaffShell> }>
                <Route path=path!("/patients/:key/plans/:plan/new") view=create::CreatePresentationPage />
                <Route path=path!("/presentations/:id") view=editor::PresentationPage />
                <ParentRoute path=path!("") view=ShellLayout>
                    <Route path=path!("") view=pages::Dashboard />
                    <Route path=path!("patients") view=patients::PatientsPage />
                    <Route path=path!("patients/:key") view=patients::PatientPage />
                    <Route path=path!("presentations") view=presentations::PresentationsPage />
                    <Route path=path!("analytics") view=|| view! { <pages::Upcoming title="Analytics" milestone="M8" /> } />
                    <Route path=path!("settings") view=settings::SettingsPage />
                </ParentRoute>
            </Routes>
        </Router>
    }
}

/// After a route change, moves focus to the new page's content so screen
/// reader and keyboard users start there instead of on the old link.
#[component]
fn RouteFocus() -> impl IntoView {
    let location = leptos_router::hooks::use_location();
    Effect::new(move |previous: Option<String>| {
        let path = location.pathname.get();
        if previous.is_some_and(|p| p != path) {
            motion::focus_page_content("main-content");
        }
        path
    });
}

#[component]
fn ShellLayout() -> impl IntoView {
    view! { <shell::StaffShell><Outlet /></shell::StaffShell> }
}
