//! In-office engagement analytics (milestone M8).
//!
//! `POST /api/present/analytics` takes screen-time and narration reports from
//! the patient screens; `GET /api/staff/analytics` summarizes them with the
//! generation history of the caller's presentations. Only screen names,
//! durations and counts are stored (migration 0009).

use super::presentations::{owner_scope, parse_level};
use crate::http::{Failure, JsonBody};
use crate::security::{CurrentPresent, CurrentSession};
use crate::{AppState, jobs};
use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use dc_core::analytics::{
    ANALYTICS_PERIODS, AnalyticsResponse, GenerationStats, LanguageStats, MAX_VIEW_SECONDS,
    PresentEvent, PresentScreen, PresentStats, PresentedRow, ScreenStats,
};
use serde::Deserialize;
use std::time::Duration;

/// Reports one present session may send per minute. The screens send about
/// four view reports a minute plus one per screen change and narration play.
const EVENTS_PER_MINUTE: u32 = 30;

/// Raw engagement rows are kept this long (docs/SECURITY.md, retention), the
/// longest period the analytics screen offers.
const RETENTION_DAYS: i64 = 90;

/// `POST /api/present/analytics`: one engagement report for the bound
/// presentation. View time is capped so a session never reports more time
/// than has passed since staff started it.
pub async fn record(
    State(state): State<AppState>,
    CurrentPresent(present): CurrentPresent,
    JsonBody(event): JsonBody<PresentEvent>,
) -> Result<StatusCode, Failure> {
    state
        .limits
        .present_events
        .check(
            &present.token_hash,
            EVENTS_PER_MINUTE,
            Duration::from_secs(60),
        )
        .map_err(Failure::too_many)?;
    let now = jobs::now_ms();
    let (kind, screen, seconds) = match event {
        PresentEvent::View { screen, seconds } => {
            let (opened, reported): (Option<i64>, i64) = sqlx::query_as(
                "SELECT MIN(CASE WHEN kind = 'open' THEN at END), COALESCE(SUM(seconds), 0)
                 FROM present_events WHERE session = ?",
            )
            .bind(&present.token_hash)
            .fetch_one(&state.db)
            .await?;
            let elapsed = opened.map_or(0, |at| (now - at).max(0) / 1000);
            let room = (elapsed + 5 - reported).max(0);
            let seconds = i64::from(seconds.min(MAX_VIEW_SECONDS)).min(room);
            if seconds == 0 {
                return Ok(StatusCode::NO_CONTENT);
            }
            ("view", screen, seconds)
        }
        PresentEvent::Narration { screen } => ("narration", screen, 0),
    };
    sqlx::query(
        "INSERT INTO present_events (presentation_id, version, session, kind, screen, seconds, at)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&present.presentation_id)
    .bind(present.version)
    .bind(&present.token_hash)
    .bind(kind)
    .bind(screen.as_str())
    .bind(seconds)
    .bind(now)
    .execute(&state.db)
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
pub struct Period {
    days: Option<u32>,
}

/// `GET /api/staff/analytics?days=N`: generation and Present-mode activity
/// for the caller's presentations over the last 7, 30 or 90 days (30 when
/// absent).
pub async fn summary(
    State(state): State<AppState>,
    CurrentSession(session): CurrentSession,
    Query(period): Query<Period>,
) -> Result<Json<AnalyticsResponse>, Failure> {
    let days = period.days.unwrap_or(30);
    if !ANALYTICS_PERIODS.contains(&days) {
        return Err(Failure::new(
            StatusCode::BAD_REQUEST,
            "invalid_period",
            "The period must be 7, 30 or 90 days.",
        ));
    }
    let owner = owner_scope(&state, &session);
    let since_ms = jobs::now_ms() - i64::from(days) * 86_400_000;
    Ok(Json(AnalyticsResponse {
        days,
        generation: generation(&state, &owner, since_ms / 1000).await?,
        present: present_totals(&state, &owner, since_ms).await?,
        screens: screens(&state, &owner, since_ms).await?,
        languages: languages(&state, &owner, since_ms).await?,
        presented: presented(&state, &owner, since_ms).await?,
    }))
}

fn count(n: i64) -> u32 {
    u32::try_from(n.max(0)).unwrap_or(u32::MAX)
}

/// Generation outcomes of the presentations created since `since_s`
/// (presentations.created_at is in seconds).
async fn generation(
    state: &AppState,
    owner: &str,
    since_s: i64,
) -> Result<GenerationStats, Failure> {
    // The full-presentation job of each presentation; single-section
    // regenerations carry a section key.
    let jobs: Vec<(String, Option<i64>, Option<i64>)> = sqlx::query_as(
        "SELECT j.state, j.started_at, j.finished_at
         FROM jobs j JOIN presentations p ON p.id = j.presentation_id
         WHERE p.owner_scope = ? AND p.created_at >= ? AND j.section_key IS NULL",
    )
    .bind(owner)
    .bind(since_s)
    .fetch_all(&state.db)
    .await?;
    let mut stats = GenerationStats {
        presentations: count(jobs.len() as i64),
        ..GenerationStats::default()
    };
    let mut durations = Vec::new();
    for (job_state, started, finished) in &jobs {
        match job_state.as_str() {
            "succeeded" => stats.succeeded += 1,
            "needs_review" => stats.needs_review += 1,
            "failed" => stats.failed += 1,
            _ => stats.in_progress += 1,
        }
        if let (Some(s), Some(f)) = (started, finished) {
            durations.push((f - s).max(0) as f64 / 1000.0);
        }
    }
    stats.p50_seconds = percentile(&mut durations, 50);
    stats.p90_seconds = percentile(&mut durations, 90);

    let sections: Vec<(String, i64, f64)> = sqlx::query_as(
        "SELECT s.state, COUNT(*), COALESCE(SUM(s.cost_usd), 0)
         FROM presentation_sections s JOIN presentations p ON p.id = s.presentation_id
         WHERE p.owner_scope = ? AND p.created_at >= ?
         GROUP BY s.state",
    )
    .bind(owner)
    .bind(since_s)
    .fetch_all(&state.db)
    .await?;
    for (section_state, n, cost) in sections {
        if section_state == "passed" {
            stats.sections_passed += count(n);
        } else {
            stats.sections_held += count(n);
        }
        stats.cost_usd += cost;
    }

    let (edits, notes): (i64, i64) = sqlx::query_as(
        "SELECT
             (SELECT COUNT(*) FROM staff_edits e JOIN presentations p ON p.id = e.presentation_id
              WHERE p.owner_scope = ?1 AND p.created_at >= ?2),
             (SELECT COUNT(*) FROM note_confirmations n JOIN presentations p ON p.id = n.presentation_id
              WHERE p.owner_scope = ?1 AND p.created_at >= ?2)",
    )
    .bind(owner)
    .bind(since_s)
    .fetch_one(&state.db)
    .await?;
    stats.staff_edits = count(edits);
    stats.notes_confirmed = count(notes);
    Ok(stats)
}

/// The `p`th percentile by the nearest-rank method; the median of an even
/// count is the mean of the middle two.
fn percentile(values: &mut [f64], p: usize) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let n = values.len();
    if p == 50 && n.is_multiple_of(2) {
        return Some((values[n / 2 - 1] + values[n / 2]) / 2.0);
    }
    let rank = (p * n).div_ceil(100).max(1);
    Some(values[rank - 1])
}

/// Deletes engagement rows older than the retention period. Returns how
/// many went.
pub async fn purge_old(db: &sqlx::SqlitePool) -> anyhow::Result<u64> {
    let cutoff = jobs::now_ms() - RETENTION_DAYS * 86_400_000;
    let done = sqlx::query("DELETE FROM present_events WHERE at < ?")
        .bind(cutoff)
        .execute(db)
        .await?;
    Ok(done.rows_affected())
}

/// Present-mode events in the caller's scope since `since_ms`. A macro so
/// the queries below stay static strings.
macro_rules! scoped_events {
    () => {
        " FROM present_events e JOIN presentations p ON p.id = e.presentation_id
         WHERE p.owner_scope = ?1 AND e.at >= ?2"
    };
}

async fn present_totals(
    state: &AppState,
    owner: &str,
    since_ms: i64,
) -> Result<PresentStats, Failure> {
    let (sessions, presentations, seconds, plays): (i64, i64, i64, i64) = sqlx::query_as(concat!(
        "SELECT COUNT(DISTINCT CASE WHEN e.kind = 'open' THEN e.session END),
                COUNT(DISTINCT CASE WHEN e.kind = 'open' THEN e.presentation_id END),
                COALESCE(SUM(e.seconds), 0),
                COALESCE(SUM(e.kind = 'narration'), 0)",
        scoped_events!()
    ))
    .bind(owner)
    .bind(since_ms)
    .fetch_one(&state.db)
    .await?;
    let per_session: Vec<(i64,)> = sqlx::query_as(concat!(
        "SELECT COALESCE(SUM(e.seconds), 0)",
        scoped_events!(),
        " GROUP BY e.session HAVING SUM(e.kind = 'open') > 0"
    ))
    .bind(owner)
    .bind(since_ms)
    .fetch_all(&state.db)
    .await?;
    let mut showing: Vec<f64> = per_session.iter().map(|(s,)| *s as f64).collect();
    Ok(PresentStats {
        sessions: count(sessions),
        presentations: count(presentations),
        seconds: seconds.max(0) as u64,
        median_seconds: percentile(&mut showing, 50).map(|s| s.round() as u64),
        narration_plays: count(plays),
    })
}

/// Presentations created since `since_ms`, and their showings, by language
/// and reading level.
async fn languages(
    state: &AppState,
    owner: &str,
    since_ms: i64,
) -> Result<Vec<LanguageStats>, Failure> {
    let rows: Vec<(String, String, i64, i64)> = sqlx::query_as(
        "SELECT p.language, p.reading_level, COUNT(*),
                COALESCE(SUM((SELECT COUNT(*) FROM present_events e
                              WHERE e.presentation_id = p.id AND e.kind = 'open' AND e.at >= ?2)), 0)
         FROM presentations p
         WHERE p.owner_scope = ?1 AND p.created_at >= ?2 / 1000
         GROUP BY p.language, p.reading_level
         ORDER BY COUNT(*) DESC, p.language, p.reading_level",
    )
    .bind(owner)
    .bind(since_ms)
    .fetch_all(&state.db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(language, level, presentations, showings)| LanguageStats {
            language,
            reading_level: parse_level(&level),
            presentations: count(presentations),
            showings: count(showings),
        })
        .collect())
}

async fn screens(
    state: &AppState,
    owner: &str,
    since_ms: i64,
) -> Result<Vec<ScreenStats>, Failure> {
    let rows: Vec<(String, i64, i64, i64)> = sqlx::query_as(concat!(
        "SELECT e.screen,
                COALESCE(SUM(e.seconds), 0),
                COUNT(DISTINCT CASE WHEN e.kind = 'view' THEN e.session END),
                COALESCE(SUM(e.kind = 'narration'), 0)",
        scoped_events!(),
        " AND e.screen IS NOT NULL GROUP BY e.screen"
    ))
    .bind(owner)
    .bind(since_ms)
    .fetch_all(&state.db)
    .await?;
    Ok(PresentScreen::ALL
        .into_iter()
        .map(|screen| {
            let row = rows.iter().find(|r| r.0 == screen.as_str());
            ScreenStats {
                screen,
                seconds: row.map_or(0, |r| r.1.max(0) as u64),
                sessions: row.map_or(0, |r| count(r.2)),
                narration_plays: row.map_or(0, |r| count(r.3)),
            }
        })
        .collect())
}

async fn presented(
    state: &AppState,
    owner: &str,
    since_ms: i64,
) -> Result<Vec<PresentedRow>, Failure> {
    let rows: Vec<(String, String, String, i64, i64, i64, i64)> = sqlx::query_as(concat!(
        "SELECT e.presentation_id, p.patient_display_name,
                strftime('%Y-%m-%dT%H:%M:%SZ', MAX(e.at) / 1000, 'unixepoch'),
                COUNT(DISTINCT CASE WHEN e.kind = 'open' THEN e.session END),
                COALESCE(SUM(e.seconds), 0),
                COUNT(DISTINCT CASE WHEN e.kind = 'view' THEN e.screen END),
                COALESCE(SUM(e.kind = 'narration'), 0)",
        scoped_events!(),
        // At most 50 rows: the most recently shown presentations.
        " GROUP BY e.presentation_id ORDER BY MAX(e.at) DESC LIMIT 50"
    ))
    .bind(owner)
    .bind(since_ms)
    .fetch_all(&state.db)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(presentation_id, name, last, sessions, seconds, screens, plays)| PresentedRow {
                presentation_id,
                patient_display_name: name,
                last_presented_at: last,
                sessions: count(sessions),
                seconds: seconds.max(0) as u64,
                screens_viewed: count(screens),
                narration_plays: count(plays),
            },
        )
        .collect())
}

#[cfg(test)]
mod tests {
    use super::percentile;

    #[test]
    fn percentiles_by_nearest_rank() {
        assert_eq!(percentile(&mut [], 50), None);
        assert_eq!(percentile(&mut [3.0, 1.0, 2.0], 50), Some(2.0));
        assert_eq!(percentile(&mut [4.0, 1.0, 3.0, 2.0], 50), Some(2.5));
        let mut ten: Vec<f64> = (1..=10).map(f64::from).collect();
        assert_eq!(percentile(&mut ten, 90), Some(9.0));
        assert_eq!(percentile(&mut [7.0], 90), Some(7.0));
    }
}
