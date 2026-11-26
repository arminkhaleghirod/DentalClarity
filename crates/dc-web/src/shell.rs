//! Staff layout: sidebar navigation and the main content area.

use crate::api;
use crate::icons::{self, Icon};
use dc_core::Mode;
use leptos::prelude::*;
use leptos_router::components::A;

#[component]
pub fn StaffShell(children: Children) -> impl IntoView {
    let health = LocalResource::new(api::health);
    let is_demo = move || matches!(health.get(), Some(Ok(h)) if h.mode == Mode::Demo);

    view! {
        <div class="shell">
            <nav class="sidebar" aria-label="Main">
                <div class="wordmark">"dental"<span>"clarity"</span></div>
                <NavLink href="/" icon=icons::HOME label="Dashboard" exact=true />
                <NavLink href="/patients" icon=icons::PATIENTS label="Patients" />
                <NavLink href="/presentations" icon=icons::PRESENTATIONS label="Presentations" />
                <NavLink href="/analytics" icon=icons::ANALYTICS label="Analytics" />
                <div class="sidebar-spacer"></div>
                <NavLink href="/settings" icon=icons::SETTINGS label="Settings" />
                <Show when=is_demo>
                    <div class="demo-badge">
                        <strong>"Synthetic demo data"</strong>". No real patient records."
                    </div>
                </Show>
            </nav>
            <main class="main">{children()}</main>
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
        <A href=href exact=exact attr:class="nav-link">
            <Icon path=icon />
            {label}
        </A>
    }
}
