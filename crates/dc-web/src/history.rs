//! Version history drawer and the version banners of the editor
//! (UI_SPEC 5.5 and section 6: read-only version, plan changed, plan
//! deleted, practice records unreachable).

use crate::api;
use crate::format::timestamp;
use dc_core::presentation::PresentationView;
use dc_core::versions::{SourceState, VersionCause, VersionSummary};
use leptos::prelude::*;
use leptos_router::components::A;

fn cause_label(cause: VersionCause) -> &'static str {
    match cause {
        VersionCause::Created => "Created",
        VersionCause::PlanChanged => "Plan changed",
        VersionCause::Regenerated => "Regenerated",
    }
}

/// Banners above the editor for the version on screen.
#[component]
pub fn VersionBanner(data: RwSignal<PresentationView>) -> impl IntoView {
    move || {
        data.with(|p| {
            let current_href = format!("/presentations/{}", api::encode(&p.id));
            let read_only = p.read_only.then(|| {
                view! {
                    <div class="version-banner changed" role="status">
                        <p>
                            {format!(
                                "You are viewing v{} (read-only). Current version is v{}.",
                                p.version, p.latest_version
                            )}
                        </p>
                        <A href=current_href.clone() attr:class="button small">"Open current version"</A>
                    </div>
                }
            });
            let plan_changed = (!p.read_only && p.version_cause == VersionCause::PlanChanged).then(|| {
                view! {
                    <div class="version-banner changed" role="status">
                        <p>
                            {format!(
                                "Open Dental plan changed on {}. v{} is ready to review.",
                                timestamp(&p.version_created_at),
                                p.version
                            )}
                        </p>
                        {(!p.changes.is_empty()).then(|| view! {
                            <ul class="version-changes">
                                {p.changes.iter().map(|c| view! { <li>{c.clone()}</li> }).collect_view()}
                            </ul>
                        })}
                    </div>
                }
            });
            let source = match p.source_state {
                SourceState::Ok => None,
                SourceState::PlanDeleted => Some(
                    "This plan no longer exists in Open Dental. The last version can still be presented.",
                ),
                SourceState::SourceUnavailable => Some(
                    "Open Dental could not be reached at the last check. Existing presentations still work.",
                ),
            }
            .map(|text| view! { <div class="version-banner review" role="status"><p>{text}</p></div> });
            view! { {read_only} {plan_changed} {source} }
        })
    }
}

/// History drawer, newest version first, with links to open each one.
#[component]
pub fn HistoryDrawer(id: String, on_close: Callback<()>) -> impl IntoView {
    let list = {
        let id = id.clone();
        LocalResource::new(move || api::versions(id.clone()))
    };
    let entry = move |v: VersionSummary| {
        let href = if v.current {
            format!("/presentations/{}", api::encode(&id))
        } else {
            format!("/presentations/{}?version={}", api::encode(&id), v.version)
        };
        let booklet = api::booklet_href(&id, (!v.current).then_some(v.version));
        view! {
            <li class="history-entry">
                <div class="history-head">
                    <b>{format!("v{} · {}", v.version, cause_label(v.cause))}</b>
                    <span class="muted">{timestamp(&v.created_at)}</span>
                </div>
                <div class="history-marks">
                    {v.current.then(|| view! { <span class="pill ready">"Current"</span> })}
                    {v.presented.then(|| view! { <span class="pill not-started">"Presented"</span> })}
                    {v.exported.then(|| view! { <span class="pill not-started">"Exported"</span> })}
                    <span class="mono muted">{v.short_hash.clone()}</span>
                </div>
                {(!v.changes.is_empty()).then(|| view! {
                    <ul class="version-changes">
                        {v.changes.iter().map(|c| view! { <li>{c.clone()}</li> }).collect_view()}
                    </ul>
                })}
                <div class="history-actions">
                    <A href=href attr:class="button small"
                        on:click=move |_| on_close.run(())>
                        {if v.current { "Open" } else { "Open read-only" }}
                    </A>
                    <a class="link-button" href=booklet download>"Booklet"</a>
                </div>
            </li>
        }
    };
    view! {
        <aside class="history-drawer" aria-label="Version history">
            <div class="history-top">
                <h2>"Version history"</h2>
                <button type="button" class="button text" on:click=move |_| on_close.run(())>"Close"</button>
            </div>
            <Suspense fallback=|| view! { <p class="muted">"Loading versions…"</p> }>
                {move || list.get().map(|result| match result {
                    Ok(versions) => view! {
                        <ol class="history-list">
                            {versions.into_iter().map(&entry).collect_view()}
                        </ol>
                    }.into_any(),
                    Err(failure) => view! {
                        <div class="notice error" role="alert">{failure.message}</div>
                    }.into_any(),
                })}
            </Suspense>
        </aside>
    }
}
