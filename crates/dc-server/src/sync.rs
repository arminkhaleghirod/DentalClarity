//! Plan sync (docs/AI_PIPELINE.md, Sync; DECISIONS E6 and E33).
//!
//! A background poll reads the plan behind each presentation from the
//! practice records and compares its fingerprint with the presentation's
//! snapshot. An unchanged fingerprint does nothing else. A changed one makes
//! a new version: the plan diff, matched by PMS source id, decides which
//! sections carry over unchanged (cost-only and display changes) and which
//! are held for staff to write again (added procedures, clinical changes,
//! and the summary sections they affect). The poll never calls a model.
//! Older versions stay readable, so a present session bound to one is not
//! affected.

use crate::jobs::{job_state_str, section_label, stage_for_state};
use crate::versions::{self, SectionRow};
use crate::{AppState, audit};
use anyhow::{Context, Result};
use dc_core::PlanSnapshot;
use dc_core::content::AgeBand;
use dc_core::plan::{PlanKey, PlannedProcedure};
use dc_core::presentation::{JobState, SectionProgress};
use dc_core::versions::VersionCause;
use dc_core::{ProcedureCode, ToothRef};
use dc_generation::facts::FactSheet;
use dc_generation::pipeline::sections_for;
use dc_generation::prompt::SectionKind;
use dc_knowledge::KnowledgeBase;
use dc_pms::PmsError;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

/// Default poll interval (PRD FR-VER-03). `DC_SYNC_INTERVAL_SECS` overrides it.
pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(5 * 60);
/// Shortest interval accepted from the environment.
const MIN_INTERVAL_SECS: u64 = 30;

pub const REASON_NEW_PROCEDURE: &str =
    "New procedure in the plan. Write this section to show it to patients.";
pub const REASON_CLINICAL_CHANGE: &str =
    "Clinical change in plan. Write this section again before patients see it.";
pub const REASON_PROCEDURES_CHANGED: &str =
    "The procedures in the plan changed. Write this section again so it matches.";
pub const REASON_ORDER_CHANGED: &str =
    "The order of treatment changed. Write this section again so it matches.";
pub const REASON_AGE_CHANGED: &str =
    "The patient's age group changed. Write this section again so it fits.";
const REASON_EDIT_DROPPED: &str = "Staff edits from the previous version were not carried over, because the plan changed for this section.";

/// The poll interval: `DC_SYNC_INTERVAL_SECS`, at least 30 seconds.
pub fn interval_from_env() -> Duration {
    std::env::var("DC_SYNC_INTERVAL_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map_or(DEFAULT_INTERVAL, |secs| {
            Duration::from_secs(secs.max(MIN_INTERVAL_SECS))
        })
}

/// What happens to one section of the new version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fate {
    /// Same inputs: the section of the old version, under this old key,
    /// carries over with its staff edits.
    Carry(String),
    /// Inputs changed: the section is held, with no text, until staff
    /// write it again.
    Rewrite(&'static str),
}

/// The difference between two snapshots of one plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanDiff {
    /// Staff-readable changes, e.g. "Cost of Crown on tooth 30 changed".
    pub changes: Vec<String>,
    /// Every section of the new plan in order, with its fate.
    pub sections: Vec<(String, Fate)>,
    /// Old procedure ref to new ref for procedures in both plans.
    pub refs: HashMap<String, String>,
}

fn label(p: &PlannedProcedure, kb: &KnowledgeBase) -> String {
    let name = kb
        .codes
        .get(&p.code)
        .map_or_else(|| p.code.to_string(), |i| i.name.clone());
    match p.tooth {
        ToothRef::None => name,
        ToothRef::Permanent(_) | ToothRef::Primary(_) => format!("{name} on tooth {}", p.tooth),
        other => format!("{name}, {other}"),
    }
}

fn clinical(p: &PlannedProcedure) -> (&ProcedureCode, ToothRef, &str, Option<&str>) {
    (
        &p.code,
        p.tooth,
        p.surfaces.as_str(),
        p.note.as_ref().map(|n| n.text.as_str()),
    )
}

/// Compares two snapshots of a plan (docs/AI_PIPELINE.md, Sync). Procedures
/// are matched by their PMS source id.
pub fn diff(old: &PlanSnapshot, new: &PlanSnapshot, kb: &KnowledgeBase) -> PlanDiff {
    let old_by_id: HashMap<&str, (usize, &PlannedProcedure)> = old
        .procedures
        .iter()
        .enumerate()
        .map(|(i, p)| (p.source_id.as_str(), (i, p)))
        .collect();
    let new_ids: Vec<&str> = new
        .procedures
        .iter()
        .map(|p| p.source_id.as_str())
        .collect();

    let mut changes = Vec::new();
    let mut refs = HashMap::new();
    let mut added = false;
    let mut clinical_change = false;
    let mut order_change = false;
    // New ref to its fate, for procedure sections.
    let mut procedure_fates: HashMap<String, Fate> = HashMap::new();

    for (j, p) in new.procedures.iter().enumerate() {
        let new_ref = format!("p{}", j + 1);
        let Some((i, before)) = old_by_id.get(p.source_id.as_str()) else {
            added = true;
            changes.push(format!("Added: {}", label(p, kb)));
            procedure_fates.insert(new_ref, Fate::Rewrite(REASON_NEW_PROCEDURE));
            continue;
        };
        let old_ref = format!("p{}", i + 1);
        refs.insert(old_ref.clone(), new_ref.clone());
        if clinical(before) != clinical(p) {
            clinical_change = true;
            changes.push(format!("Clinical details changed: {}", label(p, kb)));
            procedure_fates.insert(new_ref, Fate::Rewrite(REASON_CLINICAL_CHANGE));
        } else {
            procedure_fates.insert(new_ref, Fate::Carry(format!("procedure:{old_ref}")));
        }
        if before.phase != p.phase {
            order_change = true;
        }
        if (
            before.fee,
            before.insurance_estimate,
            before.patient_estimate,
        ) != (p.fee, p.insurance_estimate, p.patient_estimate)
        {
            changes.push(format!("Cost changed: {}", label(p, kb)));
        }
        if before.status != p.status {
            changes.push(format!("Status changed: {}", label(p, kb)));
        }
    }
    let mut removed = false;
    for p in &old.procedures {
        if !new_ids.contains(&p.source_id.as_str()) {
            removed = true;
            changes.push(format!("Removed: {}", label(p, kb)));
        }
    }
    // The relative order of matched procedures counts as sequence too.
    let matched_old: Vec<&str> = old
        .procedures
        .iter()
        .map(|p| p.source_id.as_str())
        .filter(|id| new_ids.contains(id))
        .collect();
    let matched_new: Vec<&str> = new_ids
        .iter()
        .copied()
        .filter(|id| old_by_id.contains_key(id))
        .collect();
    if matched_old != matched_new {
        order_change = true;
    }
    if order_change {
        changes.push("Treatment order changed".into());
    }
    let age_change =
        AgeBand::from_age(old.patient.age_years) != AgeBand::from_age(new.patient.age_years);
    if age_change {
        changes.push("Patient age group changed".into());
    }
    if old.patient.first_name != new.patient.first_name
        || old.patient.preferred_language != new.patient.preferred_language
        || old.plan_name != new.plan_name
        || old.provider_display_name != new.provider_display_name
    {
        changes.push("Patient or plan details changed".into());
    }
    if changes.is_empty() {
        changes.push("Plan details changed".into());
    }

    let procedures_changed = added || removed || clinical_change;
    let summary_fate = |key: &str, also: bool, reason_also: &'static str| {
        if age_change {
            Fate::Rewrite(REASON_AGE_CHANGED)
        } else if procedures_changed {
            Fate::Rewrite(REASON_PROCEDURES_CHANGED)
        } else if also {
            Fate::Rewrite(reason_also)
        } else {
            Fate::Carry(key.to_owned())
        }
    };
    let sheet = FactSheet::build(new, kb);
    let sections = sections_for(&sheet)
        .into_iter()
        .map(|kind| {
            let key = kind.key();
            let fate = match &kind {
                SectionKind::AtAGlance | SectionKind::NextSteps => {
                    summary_fate(&key, false, REASON_ORDER_CHANGED)
                }
                SectionKind::Journey => summary_fate(&key, order_change, REASON_ORDER_CHANGED),
                SectionKind::Procedure(r) => {
                    if age_change {
                        Fate::Rewrite(REASON_AGE_CHANGED)
                    } else {
                        procedure_fates
                            .get(r)
                            .cloned()
                            .unwrap_or(Fate::Rewrite(REASON_NEW_PROCEDURE))
                    }
                }
            };
            (key, fate)
        })
        .collect();
    PlanDiff {
        changes,
        sections,
        refs,
    }
}

/// Renames procedure references in a carried section's stored output, so
/// its citations point at the same procedure under its new ref.
pub fn remap_refs(value: &mut Value, refs: &HashMap<String, String>) {
    match value {
        Value::String(s) => {
            let (prefix, reference) = match s.split_once(':') {
                Some((prefix, reference)) => (Some(prefix), reference),
                None => (None, s.as_str()),
            };
            if let Some(new_ref) = refs.get(reference) {
                *s = match prefix {
                    Some(prefix) => format!("{prefix}:{new_ref}"),
                    None => new_ref.clone(),
                };
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|v| remap_refs(v, refs)),
        Value::Object(map) => map.values_mut().for_each(|v| remap_refs(v, refs)),
        _ => {}
    }
}

/// What one sync of a presentation did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Unchanged,
    /// A new version with this number.
    NewVersion(i64),
    /// A generation job is running; the next poll tries again.
    Busy,
    Missing,
}

/// Compares presentation `id` with `live`, the plan as the practice records
/// have it now, and makes a new version when it changed.
pub async fn apply(state: &AppState, id: &str, live: &PlanSnapshot) -> Result<Outcome> {
    let now = crate::sessions::now();
    let fingerprint = live.fingerprint();
    let mut tx = state.db.begin().await?;
    let row: Option<(i64, String, String)> = sqlx::query_as(
        "SELECT version, snapshot_json, fingerprint FROM presentations WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((version, old_json, old_fingerprint)) = row else {
        return Ok(Outcome::Missing);
    };
    if old_fingerprint == fingerprint {
        set_source_state(&mut tx, id, "ok", now).await?;
        tx.commit().await?;
        return Ok(Outcome::Unchanged);
    }
    let (running,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM jobs WHERE presentation_id = ? AND state IN ('queued', 'running')",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await?;
    if running > 0 {
        return Ok(Outcome::Busy);
    }

    let old: PlanSnapshot =
        serde_json::from_str(&old_json).context("reading the stored snapshot")?;
    let plan = diff(&old, live, &state.kb);
    let old_rows: Vec<SectionRow> = sqlx::query_as(
        "SELECT key, state, output_json, reasons_json, needs_note_confirmation,
             writer_model, prompt_version
         FROM presentation_sections WHERE presentation_id = ? ORDER BY position",
    )
    .bind(id)
    .fetch_all(&mut *tx)
    .await?;
    let edited: Vec<(String,)> = sqlx::query_as(
        "SELECT DISTINCT section_key FROM staff_edits WHERE presentation_id = ? AND version = ?",
    )
    .bind(id)
    .bind(version)
    .fetch_all(&mut *tx)
    .await?;

    let live_json = serde_json::to_string(live)?;
    let next = versions::begin_next(
        &mut tx,
        id,
        version,
        versions::NextVersion {
            cause: VersionCause::PlanChanged,
            snapshot_json: &live_json,
            fingerprint: &fingerprint,
            changes: &plan.changes,
            now,
        },
    )
    .await?;
    sqlx::query("DELETE FROM presentation_sections WHERE presentation_id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;

    // New key for each carried old key.
    let mut carried: BTreeMap<String, String> = BTreeMap::new();
    let mut new_rows: Vec<SectionRow> = Vec::new();
    for (key, fate) in &plan.sections {
        let row = match fate {
            Fate::Carry(old_key) => old_rows.iter().find(|r| &r.0 == old_key).map(|r| {
                carried.insert(old_key.clone(), key.clone());
                let output = r.2.as_deref().map(|o| {
                    let mut value: Value = serde_json::from_str(o).unwrap_or(Value::Null);
                    remap_refs(&mut value, &plan.refs);
                    value.to_string()
                });
                (
                    key.clone(),
                    r.1.clone(),
                    output,
                    r.3.clone(),
                    r.4,
                    r.5.clone(),
                    r.6.clone(),
                )
            }),
            Fate::Rewrite(_) => None,
        };
        let row = row.unwrap_or_else(|| {
            let reason = match fate {
                Fate::Rewrite(reason) => *reason,
                Fate::Carry(_) => REASON_NEW_PROCEDURE,
            };
            let mut reasons = vec![reason.to_owned()];
            // The old section this one replaces, if any, by procedure source id.
            let old_key = old_key_for(key, &plan.refs);
            if old_key
                .as_deref()
                .is_some_and(|k| edited.iter().any(|(e,)| e == k))
            {
                reasons.push(REASON_EDIT_DROPPED.to_owned());
            }
            (
                key.clone(),
                "needs_review".to_owned(),
                None,
                serde_json::to_string(&reasons).unwrap_or_else(|_| "[]".into()),
                false,
                String::new(),
                String::new(),
            )
        });
        new_rows.push(row);
    }
    for (position, row) in new_rows.iter().enumerate() {
        sqlx::query(
            "INSERT INTO presentation_sections (presentation_id, key, position, state, output_json,
                 reasons_json, needs_note_confirmation, writer_model, prompt_version)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(&row.0)
        .bind(position as i64)
        .bind(&row.1)
        .bind(&row.2)
        .bind(&row.3)
        .bind(row.4)
        .bind(&row.5)
        .bind(&row.6)
        .execute(&mut *tx)
        .await?;
    }
    versions::copy_staff_changes(&mut tx, id, version, next, |key| carried.get(key).cloned())
        .await?;
    refresh_job(&mut tx, id, &new_rows, live, &state.kb).await?;
    set_source_state(&mut tx, id, "ok", now).await?;
    audit::record(
        &mut *tx,
        "sync",
        "plan_changed",
        &format!("presentation {id} version {next}"),
    )
    .await?;
    tx.commit().await?;
    Ok(Outcome::NewVersion(next))
}

/// The key a new-version section had in the old version: the same key for
/// summary sections, the old ref for a procedure in both plans.
fn old_key_for(new_key: &str, refs: &HashMap<String, String>) -> Option<String> {
    match new_key.strip_prefix("procedure:") {
        None => Some(new_key.to_owned()),
        Some(new_ref) => refs
            .iter()
            .find(|(_, n)| n.as_str() == new_ref)
            .map(|(old, _)| format!("procedure:{old}")),
    }
}

async fn set_source_state(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    id: &str,
    source_state: &str,
    now: i64,
) -> Result<()> {
    sqlx::query("UPDATE presentations SET source_state = ?, checked_at = ? WHERE id = ?")
        .bind(source_state)
        .bind(now)
        .bind(id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Lists and the editor read the newest job as the presentation's state,
/// so it is brought in line with the new version's sections.
async fn refresh_job(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    id: &str,
    rows: &[SectionRow],
    snapshot: &PlanSnapshot,
    kb: &KnowledgeBase,
) -> Result<()> {
    let job: Option<(String,)> = sqlx::query_as(
        "SELECT id FROM jobs WHERE presentation_id = ? ORDER BY created_at DESC LIMIT 1",
    )
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some((job_id,)) = job else {
        return Ok(());
    };
    let sheet = FactSheet::build(snapshot, kb);
    let progress: Vec<SectionProgress> = rows
        .iter()
        .map(|row| SectionProgress {
            key: row.0.clone(),
            label: SectionKind::from_key(&row.0)
                .map_or_else(|| row.0.clone(), |kind| section_label(&kind, &sheet)),
            stage: stage_for_state(&row.1),
        })
        .collect();
    let passed = rows.iter().filter(|r| r.1 == "passed").count();
    let state = if passed == rows.len() {
        JobState::Succeeded
    } else if passed > 0 {
        JobState::NeedsReview
    } else {
        JobState::Failed
    };
    sqlx::query("UPDATE jobs SET state = ?, progress_json = ? WHERE id = ?")
        .bind(job_state_str(state))
        .bind(serde_json::to_string(&progress)?)
        .bind(&job_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Counts from one poll.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PollReport {
    pub checked: usize,
    pub new_versions: usize,
    pub unavailable: usize,
}

/// Polls the plan behind every presentation without a running job once.
/// Each plan is read once, however many presentations it has.
pub async fn poll_once(state: &AppState) -> Result<PollReport> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT p.id, p.plan_key FROM presentations p
         WHERE NOT EXISTS (SELECT 1 FROM jobs j WHERE j.presentation_id = p.id
                           AND j.state IN ('queued', 'running'))
         ORDER BY p.plan_key",
    )
    .fetch_all(&state.db)
    .await?;
    let mut by_plan: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (id, plan) in rows {
        by_plan.entry(plan).or_default().push(id);
    }
    let mut report = PollReport::default();
    let now = crate::sessions::now();
    for (plan, ids) in by_plan {
        let live = state.pms.plan_snapshot(&PlanKey(plan)).await;
        for id in ids {
            report.checked += 1;
            match &live {
                Ok(snapshot) => {
                    if let Outcome::NewVersion(_) = apply(state, &id, snapshot).await? {
                        report.new_versions += 1;
                    }
                }
                Err(err) => {
                    let source_state = match err {
                        PmsError::PlanNotFound(_) | PmsError::PatientNotFound(_) => "plan_deleted",
                        _ => {
                            report.unavailable += 1;
                            "source_unavailable"
                        }
                    };
                    let mut tx = state.db.begin().await?;
                    set_source_state(&mut tx, &id, source_state, now).await?;
                    tx.commit().await?;
                }
            }
        }
    }
    Ok(report)
}

/// Runs [`poll_once`] every `interval` until the server stops.
pub fn spawn(state: AppState, interval: Duration) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(interval);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            match poll_once(&state).await {
                Ok(report) if report.new_versions > 0 || report.unavailable > 0 => {
                    tracing::info!(
                        checked = report.checked,
                        new_versions = report.new_versions,
                        unavailable = report.unavailable,
                        "plan sync"
                    );
                }
                Ok(_) => {}
                Err(err) => tracing::warn!(%err, "plan sync failed"),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use dc_core::Money;
    use dc_core::money::Amount;

    fn kb() -> KnowledgeBase {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../knowledge");
        KnowledgeBase::load(&root).unwrap()
    }

    fn daniel() -> PlanSnapshot {
        let root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/synthetic");
        let pms =
            dc_pms::synthetic::SyntheticConnector::from_path(&root.join("practice.toml")).unwrap();
        futures::executor::block_on(dc_pms::PmsConnector::plan_snapshot(
            &pms,
            &PlanKey("tp-10482-1".into()),
        ))
        .unwrap()
    }

    fn fates(diff: &PlanDiff) -> Vec<(&str, bool)> {
        diff.sections
            .iter()
            .map(|(k, f)| (k.as_str(), matches!(f, Fate::Carry(_))))
            .collect()
    }

    #[test]
    fn cost_only_change_carries_every_section() {
        let old = daniel();
        let mut new = old.clone();
        new.procedures[1].fee = Amount::Known(Money(99_900));
        let diff = diff(&old, &new, &kb());
        assert!(
            diff.sections
                .iter()
                .all(|(_, f)| matches!(f, Fate::Carry(_)))
        );
        assert_eq!(diff.changes.len(), 1);
        assert!(diff.changes[0].starts_with("Cost changed"));
    }

    #[test]
    fn added_procedure_holds_its_section_and_the_summaries() {
        let old = daniel();
        let mut new = old.clone();
        let mut extra = new.procedures[1].clone();
        extra.source_id = "new-1".into();
        extra.tooth = ToothRef::Permanent(3);
        new.procedures.insert(0, extra);
        let diff = diff(&old, &new, &kb());
        let f = fates(&diff);
        assert_eq!(f[0], ("at_a_glance", false));
        assert_eq!(f[1], ("journey", false));
        assert_eq!(f[2], ("procedure:p1", false));
        // The old p1 is now p2 and carries over.
        assert_eq!(diff.sections[3].1, Fate::Carry("procedure:p1".into()));
        assert_eq!(diff.refs["p1"], "p2");
        assert_eq!(f.last().unwrap(), &("next_steps", false));
    }

    #[test]
    fn removed_procedure_drops_its_section() {
        let old = daniel();
        let mut new = old.clone();
        new.procedures.remove(1);
        let diff = diff(&old, &new, &kb());
        assert_eq!(diff.sections.len(), 2 + 5 + 1);
        assert!(diff.changes.iter().any(|c| c.starts_with("Removed")));
        assert_eq!(diff.sections[2].1, Fate::Carry("procedure:p1".into()));
        assert_eq!(diff.sections[3].1, Fate::Carry("procedure:p3".into()));
    }

    #[test]
    fn clinical_change_holds_that_procedure_and_summaries() {
        let old = daniel();
        let mut new = old.clone();
        new.procedures[2].tooth = ToothRef::Permanent(2);
        let diff = diff(&old, &new, &kb());
        assert_eq!(diff.sections[4].1, Fate::Rewrite(REASON_CLINICAL_CHANGE));
        assert_eq!(diff.sections[2].1, Fate::Carry("procedure:p1".into()));
        assert_eq!(diff.sections[0].1, Fate::Rewrite(REASON_PROCEDURES_CHANGED));
    }

    #[test]
    fn phase_change_holds_only_the_journey() {
        let old = daniel();
        let mut new = old.clone();
        new.procedures[0].phase = Some(2);
        let diff = diff(&old, &new, &kb());
        for (key, fate) in &diff.sections {
            assert_eq!(matches!(fate, Fate::Carry(_)), key != "journey", "{key}");
        }
    }

    #[test]
    fn refs_are_renamed_in_citations() {
        let refs = HashMap::from([("p1".to_owned(), "p2".to_owned())]);
        let mut value = serde_json::json!({
            "procedure_ref": "p1",
            "what_it_is": [{ "text": "p1 stays in text", "cites": ["fact:p1", "fact:ph1", "note:p1"] }]
        });
        remap_refs(&mut value, &refs);
        assert_eq!(value["procedure_ref"], "p2");
        assert_eq!(value["what_it_is"][0]["text"], "p1 stays in text");
        assert_eq!(
            value["what_it_is"][0]["cites"],
            serde_json::json!(["fact:p2", "fact:ph1", "note:p2"])
        );
    }
}
