//! Staff layout: sidebar navigation and the main content area.
//!
//! Below 1180 px the sidebar collapses to icons; each link keeps its text
//! for screen readers and shows it as a tooltip.

use crate::api;
use crate::icons::{self, Icon, Logo};
use crate::session::use_session;
use dc_core::Mode;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::A;

#[component]
pub fn StaffShell(children: Children) -> impl IntoView {
    let session = use_session();
    let mode = move || session.info.get().and_then(Result::ok).map(|i| i.mode);
    let is_demo = move || mode() == Some(Mode::Demo);
    let signing_out = RwSignal::new(false);
    let sign_out_error = RwSignal::new(None::<String>);
    let sign_out = move |_| {
        if signing_out.get_untracked() {
            return;
        }
        signing_out.set(true);
        sign_out_error.set(None);
        spawn_local(async move {
            let result = api::logout().await;
            // The shell was unmounted while the request ran.
            if signing_out.try_set(false).is_some() {
                return;
            }
            match result {
                Ok(()) => session.reload(),
                // Staying signed in, so the workspace stays and says why.
                Err(failure) => sign_out_error.set(Some(failure.message)),
            }
        });
    };

    view! {
        <div class="shell">
            <a class="skip-link" href="#main-content">"Skip to content"</a>
            <nav class="sidebar" aria-label="Main">
                <div class="wordmark">
                    <Logo />
                </div>
                <NavLink href="/" icon=icons::HOME label="Dashboard" exact=true />
                <NavLink href="/patients" icon=icons::PATIENTS label="Patients" />
                <NavLink href="/presentations" icon=icons::PRESENTATIONS label="Presentations" />
                <NavLink href="/analytics" icon=icons::ANALYTICS label="Analytics" />
                <div class="sidebar-spacer"></div>
                <NavLink href="/settings" icon=icons::SETTINGS label="Settings" />
                <Show
                    when=is_demo
                    fallback=move || view! {
                        <button class="nav-link nav-button" title="Sign out"
                            disabled=move || signing_out.get() on:click=sign_out>
                            <Icon path=icons::SIGN_OUT />
                            <span class="nav-label">"Sign out"</span>
                        </button>
                    }
                >
                    <div class="demo-badge" title="Synthetic demo data. No real patient records.">
                        <span class="demo-full"><strong>"Synthetic demo data"</strong>". No real patient records."</span>
                        <span class="demo-short" aria-hidden="true">"Demo"</span>
                    </div>
                </Show>
            </nav>
            <main id="main-content" class="main" tabindex="-1">
                {move || sign_out_error.get().map(|message| view! {
                    <div class="notice error shell-error" role="alert">{message}</div>
                })}
                {children()}
            </main>
        </div>
    }
}

#[component]
fn NavLink(
    href: &'static str,
    icon: &'static str,
    label: &'static str,
    #[prop(optional)] exact: bool,
) -> impl IntoView {
    view! {
        <A href=href exact=exact attr:class="nav-link" attr:title=label>
            <Icon path=icon />
            <span class="nav-label">{label}</span>
        </A>
    }
}
