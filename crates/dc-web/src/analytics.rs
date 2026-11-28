//! Analytics (`GET /api/staff/analytics`, milestone M8): how presentations
//! are used in the office and how generation went, over 7, 30 or 90 days.

use crate::api;
use crate::format::language_name;
use crate::format::timestamp;
use crate::motion::{SkeletonCards, SkeletonTable};
use crate::progress::presentation_href;
use dc_core::analytics::{
    ANALYTICS_PERIODS, AnalyticsResponse, LanguageStats, PresentedRow, ScreenStats,
};
use dc_core::content::ReadingLevel;
use leptos::prelude::*;
use leptos_router::components::A;

/// "45 s", "12 min", "2 h 05 min".
pub fn duration(seconds: u64) -> String {
    match seconds {
        0..60 => format!("{seconds} s"),
        60..3600 => format!("{} min", (seconds + 30) / 60),
        _ => {
            let minutes = (seconds + 30) / 60;
            format!("{} h {:02} min", minutes / 60, minutes % 60)
        }
    }
}

fn plural(n: u32, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

#[component]
pub fn AnalyticsPage() -> impl IntoView {
    let days = RwSignal::new(30_u32);
    let data = LocalResource::new(move || api::analytics(days.get()));
    view! {
        <header class="topbar">
            <h1 class="page-title">"Analytics"</h1>
            <div class="segmented" role="radiogroup" aria-label="Period">
                {ANALYTICS_PERIODS.into_iter().map(|option| view! {
                    <label class="segment" class:selected=move || days.get() == option>
                        <input
                            type="radio"
                            name="analytics-period"
                            class="visually-hidden"
                            value=option.to_string()
                            prop:checked=move || days.get() == option
                            on:change=move |_| days.set(option)
                        />
                        {format!("Last {option} days")}
                    </label>
                }).collect_view()}
            </div>
        </header>
        {move || match data.get() {
            None => view! {
                <SkeletonCards />
                <SkeletonTable label="Loading analytics" rows=5 cols=6 />
            }.into_any(),
            Some(Err(failure)) => view! {
                <div class="notice error reveal" role="alert">"Could not load analytics. " {failure.message}</div>
            }.into_any(),
            Some(Ok(report)) => view! { <Report report /> }.into_any(),
        }}
    }
}

#[component]
fn Report(report: AnalyticsResponse) -> impl IntoView {
    let p = report.present.clone();
    let g = report.generation.clone();
    let median_showing = p
        .median_seconds
        .map_or_else(|| "None yet".to_owned(), duration);
    let finished = g.succeeded + g.needs_review + g.failed;
    let first_pass = (g.succeeded * 100 + finished / 2)
        .checked_div(finished)
        .map_or_else(|| "None yet".to_owned(), |pct| format!("{pct}%"));
    view! {
        <section class="stat-cards reveal" aria-label="Summary">
            <div class="stat-card">
                <h2>"Shown to patients"</h2>
                <p class="stat-value">{plural(p.presentations, "presentation", "presentations")}</p>
                <p class="stat-note">{plural(p.sessions, "showing", "showings")}</p>
            </div>
            <div class="stat-card">
                <h2>"Median time in presentation"</h2>
                <p class="stat-value">{median_showing}</p>
                <p class="stat-note">{format!("{} in total", duration(p.seconds))}</p>
            </div>
            <div class="stat-card">
                <h2>"Narration plays"</h2>
                <p class="stat-value">{p.narration_plays}</p>
                <p class="stat-note">"Times a patient pressed Listen"</p>
            </div>
            <div class="stat-card">
                <h2>"Questions asked"</h2>
                <p class="stat-value">{p.questions}</p>
                <p class="stat-note">"In the Ask a question drawer"</p>
            </div>
        </section>

        <div class="analytics-grid">
            <section class="card analytics-panel reveal" aria-labelledby="screens-head">
                <div class="panel-head">
                    <h2 id="screens-head">"Where patients spend time"</h2>
                    <p>"Time on each patient screen during Present mode"</p>
                </div>
                <ScreenBars screens=report.screens.clone() total=p.seconds />
            </section>
            <section class="card analytics-panel reveal" aria-labelledby="generation-head">
                <div class="panel-head">
                    <h2 id="generation-head">"Generation"</h2>
                    <p>{format!("Presentations created in the last {} days", report.days)}</p>
                </div>
                <dl class="facts">
                    <div><dt>"Created"</dt><dd>{g.presentations}</dd></div>
                    <div><dt>"Ready"</dt><dd>{g.succeeded}</dd></div>
                    <div><dt>"Needs review"</dt><dd>{g.needs_review}</dd></div>
                    <div><dt>"Failed"</dt><dd>{g.failed}</dd></div>
                    <div><dt>"Ready without review"</dt><dd>{first_pass}</dd></div>
                    {(g.in_progress > 0).then(|| view! { <div><dt>"In progress"</dt><dd>{g.in_progress}</dd></div> })}
                    <div><dt>"Sections passed"</dt><dd>{g.sections_passed}</dd></div>
                    <div><dt>"Sections held"</dt><dd>{g.sections_held}</dd></div>
                    <div><dt>"Time to write, median"</dt><dd>{seconds_or_none(g.p50_seconds)}</dd></div>
                    <div><dt>"Time to write, 90th percentile"</dt><dd>{seconds_or_none(g.p90_seconds)}</dd></div>
                    <div><dt>"Model spend"</dt><dd>{format!("${:.2}", g.cost_usd)}</dd></div>
                    <div><dt>"Staff edits"</dt><dd>{g.staff_edits}</dd></div>
                    <div><dt>"Notes confirmed"</dt><dd>{g.notes_confirmed}</dd></div>
                </dl>
            </section>
        </div>

        <div class="section-head">
            <div>
                <h2>"Languages and reading levels"</h2>
                <p>"Presentations created in this period"</p>
            </div>
        </div>
        <LanguageTable rows=report.languages />

        <div class="section-head">
            <div>
                <h2>"Shown to patients"</h2>
                <p>"Presentations opened in Present mode, most recent first"</p>
            </div>
        </div>
        <PresentedTable rows=report.presented />
        <p class="analytics-footnote">
            "Engagement shows interaction in the office. It does not measure understanding, consent or treatment decisions."
        </p>
    }
}

fn seconds_or_none(value: Option<f64>) -> String {
    value.map_or_else(|| "None yet".to_owned(), |s| duration(s.round() as u64))
}

fn level_name(level: ReadingLevel) -> &'static str {
    match level {
        ReadingLevel::Simple => "Simple",
        ReadingLevel::Standard => "Standard",
        ReadingLevel::Detailed => "Detailed",
    }
}

#[component]
fn LanguageTable(rows: Vec<LanguageStats>) -> impl IntoView {
    if rows.is_empty() {
        return view! {
            <div class="notice reveal">"No presentations yet in this period."</div>
        }
        .into_any();
    }
    view! {
        <div class="table-wrap reveal">
            <table class="table">
                <thead>
                    <tr>
                        <th scope="col">"Language"</th>
                        <th scope="col">"Reading level"</th>
                        <th scope="col">"Presentations"</th>
                        <th scope="col">"Showings"</th>
                    </tr>
                </thead>
                <tbody>
                    {rows.into_iter().map(|row| view! {
                        <tr>
                            <td class="name">{language_name(&row.language)}</td>
                            <td>{level_name(row.reading_level)}</td>
                            <td class="num">{row.presentations}</td>
                            <td class="num">{row.showings}</td>
                        </tr>
                    }).collect_view()}
                </tbody>
            </table>
        </div>
    }
    .into_any()
}

#[component]
fn ScreenBars(screens: Vec<ScreenStats>, total: u64) -> impl IntoView {
    if total == 0 {
        return view! {
            <p class="panel-empty">"No time recorded yet. Screen time appears here after a patient views a presentation in Present mode."</p>
        }
        .into_any();
    }
    let max = screens.iter().map(|s| s.seconds).max().unwrap_or(1).max(1);
    view! {
        <ul class="screen-bars">
            {screens.into_iter().map(|s| {
                let width = format!("{:.1}%", s.seconds as f64 / max as f64 * 100.0);
                let share = (s.seconds * 100 + total / 2) / total;
                let detail = format!(
                    "{} · {}{}",
                    duration(s.seconds),
                    plural(s.sessions, "showing", "showings"),
                    if s.narration_plays > 0 { format!(" · {}", plural(s.narration_plays, "play", "plays")) } else { String::new() },
                );
                view! {
                    <li>
                        <div class="screen-bar-label">
                            <span>{s.screen.label()}</span>
                            <span class="num">{format!("{share}%")}</span>
                        </div>
                        <div class="screen-bar-track" aria-hidden="true"><div class="screen-bar-fill" style:width=width></div></div>
                        <p class="screen-bar-detail">{detail}</p>
                    </li>
                }
            }).collect_view()}
        </ul>
    }
    .into_any()
}

#[component]
fn PresentedTable(rows: Vec<PresentedRow>) -> impl IntoView {
    if rows.is_empty() {
        return view! {
            <div class="notice empty-state reveal">
                <p>"No presentation has been shown in this period. Open a ready presentation and choose Present."</p>
                <A href="/presentations" attr:class="button">"Open presentations"</A>
            </div>
        }
        .into_any();
    }
    view! {
        <div class="table-wrap reveal">
            <table class="table">
                <thead>
                    <tr>
                        <th scope="col">"Patient"</th>
                        <th scope="col">"Last shown"</th>
                        <th scope="col">"Showings"</th>
                        <th scope="col">"Time"</th>
                        <th scope="col">"Screens viewed"</th>
                        <th scope="col">"Narration plays"</th>
                        <th scope="col"><span class="visually-hidden">"Action"</span></th>
                    </tr>
                </thead>
                <tbody>
                    {rows.into_iter().map(|row| {
                        let href = presentation_href(&row.presentation_id);
                        let label = format!("Open presentation for {}", row.patient_display_name);
                        view! {
                            <tr>
                                <td class="name"><A href=href.clone()>{row.patient_display_name.clone()}</A></td>
                                <td class="num">{timestamp(&row.last_presented_at)}</td>
                                <td class="num">{row.sessions}</td>
                                <td class="num">{duration(row.seconds)}</td>
                                <td class="num">{format!("{} of 5", row.screens_viewed)}</td>
                                <td class="num">{row.narration_plays}</td>
                                <td class="action"><A href=href attr:aria-label=label>"Open"</A></td>
                            </tr>
                        }
                    }).collect_view()}
                </tbody>
            </table>
        </div>
    }
    .into_any()
}

#[cfg(test)]
mod tests {
    use super::duration;

    #[test]
    fn durations_read_naturally() {
        assert_eq!(duration(0), "0 s");
        assert_eq!(duration(59), "59 s");
        assert_eq!(duration(90), "2 min");
        assert_eq!(duration(3600), "1 h 00 min");
        assert_eq!(duration(7500), "2 h 05 min");
    }
}
