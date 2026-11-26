//! Staff edits of generated blocks, revert, and confirmation of
//! note-derived blocks (DECISIONS P11). Every route runs behind
//! `security::require_session`, so writes need the CSRF token.
//!
//! Edited text is checked by the same deterministic validators as model
//! output, but only as warnings: staff text is never blocked.

use super::present::actor;
use super::presentations::{
    MAX_ID_CHARS, StaffChanges, not_found, owner_scope, parse_level, section_rows, section_view,
};
use crate::http::{Failure, JsonBody};
use crate::security::CurrentSession;
use crate::sessions::Session;
use crate::{AppState, audit};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use dc_core::Mode;
use dc_core::content::ReadingLevel;
use dc_core::plan::PlanSnapshot;
use dc_core::present::{
    BlockChangeResponse, ConfirmNoteRequest, RevertBlockRequest, UpdateBlockRequest,
};
use dc_core::presentation::{BlockSource, SectionView, TextBlock};
use dc_generation::facts::FactSheet;
use dc_generation::schema::{NextStepsOut, Sentence};
use dc_generation::validate::{Context, check_next_steps};
use std::collections::HashSet;
use std::time::Duration;

pub const MAX_EDIT_SENTENCES: usize = 12;
pub const MAX_EDIT_SENTENCE_CHARS: usize = 400;
/// Per block (docs/SECURITY.md, demo abuse controls).
pub const MAX_EDIT_BLOCK_CHARS: usize = 2000;
const MAX_KEY_CHARS: usize = 64;
/// Saved edits per demo visitor scope per hour (docs/SECURITY.md, demo
/// abuse controls). Every edit is new text that Present mode may narrate.
pub const DEMO_EDITS_PER_HOUR: u32 = 60;

/// The presentation and generated block an edit request points at.
struct Target {
    version: i64,
    sheet: FactSheet,
    level: ReadingLevel,
    /// The section as generated, without staff changes.
    generated: SectionView,
}

impl Target {
    fn generated_block(&self, field: &str) -> &TextBlock {
        self.generated
            .blocks
            .iter()
            .find(|b| b.field == field)
            .expect("checked when the target was loaded")
    }
}

fn block_not_found() -> Failure {
    Failure::new(
        StatusCode::NOT_FOUND,
        "block_not_found",
        "That part of the presentation was not found.",
    )
}

async fn target(
    state: &AppState,
    session: &Session,
    id: &str,
    section_key: &str,
    field: &str,
) -> Result<Target, Failure> {
    if id.len() > MAX_ID_CHARS {
        return Err(not_found("presentation"));
    }
    let row: Option<(i64, String, String)> = sqlx::query_as(
        "SELECT version, reading_level, snapshot_json FROM presentations
         WHERE id = ? AND owner_scope = ?",
    )
    .bind(id)
    .bind(owner_scope(state, session))
    .fetch_optional(&state.db)
    .await?;
    let Some((version, level, snapshot)) = row else {
        return Err(not_found("presentation"));
    };
    if section_key.len() > MAX_KEY_CHARS || field.len() > MAX_KEY_CHARS {
        return Err(block_not_found());
    }
    let snapshot: PlanSnapshot = serde_json::from_str(&snapshot).map_err(Failure::internal)?;
    let sheet = FactSheet::build(&snapshot, &state.kb);
    let generated = section_rows(state, id)
        .await?
        .into_iter()
        .find(|r| r.0 == section_key)
        .map(|r| section_view(r, &sheet))
        .ok_or_else(block_not_found)?;
    if !generated.blocks.iter().any(|b| b.field == field) {
        return Err(block_not_found());
    }
    Ok(Target {
        version,
        sheet,
        level: parse_level(&level),
        generated,
    })
}

/// The section with every staff change of this version applied.
async fn changed_section(
    state: &AppState,
    id: &str,
    target: &Target,
) -> Result<SectionView, Failure> {
    let mut section = target.generated.clone();
    StaffChanges::load(state, id, target.version)
        .await?
        .apply(&mut section);
    Ok(section)
}

fn invalid_edit(message: String) -> Failure {
    Failure::new(StatusCode::UNPROCESSABLE_ENTITY, "invalid_edit", message)
}

/// Trims each sentence and enforces the edit limits.
pub fn clean_sentences(sentences: &[String]) -> Result<Vec<String>, Failure> {
    if sentences.is_empty() || sentences.len() > MAX_EDIT_SENTENCES {
        return Err(invalid_edit(format!(
            "Write 1 to {MAX_EDIT_SENTENCES} sentences."
        )));
    }
    let cleaned: Vec<String> = sentences.iter().map(|s| s.trim().to_owned()).collect();
    for (i, s) in cleaned.iter().enumerate() {
        let chars = s.chars().count();
        if chars == 0 || chars > MAX_EDIT_SENTENCE_CHARS {
            return Err(invalid_edit(format!(
                "Sentence {} must be 1 to {MAX_EDIT_SENTENCE_CHARS} characters.",
                i + 1
            )));
        }
        if s.chars().any(char::is_control) {
            return Err(invalid_edit(format!(
                "Sentence {} has a line break or other control character.",
                i + 1
            )));
        }
        // Placeholders are filled in after these length checks, so staff
        // text must not contain them.
        if s.contains("{{") || s.contains("}}") {
            return Err(invalid_edit(format!(
                "Sentence {} contains {{{{ or }}}}. Write names out instead of using placeholders.",
                i + 1
            )));
        }
    }
    let total: usize = cleaned.iter().map(|s| s.chars().count()).sum();
    if total > MAX_EDIT_BLOCK_CHARS {
        return Err(invalid_edit(format!(
            "A block can hold up to {MAX_EDIT_BLOCK_CHARS} characters."
        )));
    }
    Ok(cleaned)
}

/// Runs the deterministic validators over staff sentences as warnings.
/// Staff text has no citations, so the schema and citation checks (V1, V3,
/// V4) are skipped; everything else is reported per sentence.
pub fn edit_warnings(sentences: &[String], sheet: &FactSheet, level: ReadingLevel) -> Vec<String> {
    let out = NextStepsOut {
        outlook: sentences
            .iter()
            .map(|text| Sentence {
                text: text.clone(),
                cites: Vec::new(),
            })
            .collect(),
        next_steps: Vec::new(),
        questions: Vec::new(),
    };
    let cx = Context {
        sheet,
        kb_ids: HashSet::new(),
        note_ids: HashSet::new(),
        level,
    };
    check_next_steps(&out, &cx)
        .into_iter()
        .filter(|i| !matches!(i.code, "V1" | "V3" | "V4"))
        .filter_map(|i| {
            let n: usize = i.path.strip_prefix("/outlook/")?.parse().ok()?;
            Some(format!("Sentence {}: {} ({})", n + 1, i.detail, i.code))
        })
        .collect()
}

fn audit_detail(id: &str, section_key: &str, field: &str) -> String {
    format!("presentation {id} section {section_key} field {field}")
}

/// `PUT /api/staff/presentations/{id}/blocks`
pub async fn update(
    State(state): State<AppState>,
    CurrentSession(session): CurrentSession,
    Path(id): Path<String>,
    JsonBody(request): JsonBody<UpdateBlockRequest>,
) -> Result<Json<BlockChangeResponse>, Failure> {
    let target = target(&state, &session, &id, &request.section_key, &request.field).await?;
    let sentences = clean_sentences(&request.sentences)?;
    if state.mode == Mode::Demo {
        state
            .limits
            .edits
            .check(
                &owner_scope(&state, &session),
                DEMO_EDITS_PER_HOUR,
                Duration::from_secs(60 * 60),
            )
            .map_err(|wait| Failure {
                retry_after: Some(wait),
                ..Failure::new(
                    StatusCode::TOO_MANY_REQUESTS,
                    "rate_limited",
                    format!(
                        "The demo allows {DEMO_EDITS_PER_HOUR} edits per hour. Try again later."
                    ),
                )
            })?;
    }
    let warnings = edit_warnings(&sentences, &target.sheet, target.level);
    let original = &target.generated_block(&request.field).sentences;
    sqlx::query(
        "INSERT INTO staff_edits (presentation_id, version, section_key, field, sentences_json,
             original_json, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (presentation_id, version, section_key, field) DO UPDATE SET
             sentences_json = excluded.sentences_json, updated_at = excluded.updated_at",
    )
    .bind(&id)
    .bind(target.version)
    .bind(&request.section_key)
    .bind(&request.field)
    .bind(serde_json::to_string(&sentences).map_err(Failure::internal)?)
    .bind(serde_json::to_string(original).map_err(Failure::internal)?)
    .bind(crate::sessions::now())
    .execute(&state.db)
    .await?;
    audit::record(
        &state.db,
        actor(session.kind),
        "block_edited",
        &audit_detail(&id, &request.section_key, &request.field),
    )
    .await?;
    let section = changed_section(&state, &id, &target).await?;
    Ok(Json(BlockChangeResponse { warnings, section }))
}

/// `POST /api/staff/presentations/{id}/blocks/revert`: restores the generated text.
pub async fn revert(
    State(state): State<AppState>,
    CurrentSession(session): CurrentSession,
    Path(id): Path<String>,
    JsonBody(request): JsonBody<RevertBlockRequest>,
) -> Result<Json<BlockChangeResponse>, Failure> {
    let target = target(&state, &session, &id, &request.section_key, &request.field).await?;
    let removed = sqlx::query(
        "DELETE FROM staff_edits
         WHERE presentation_id = ? AND version = ? AND section_key = ? AND field = ?",
    )
    .bind(&id)
    .bind(target.version)
    .bind(&request.section_key)
    .bind(&request.field)
    .execute(&state.db)
    .await?
    .rows_affected();
    if removed > 0 {
        audit::record(
            &state.db,
            actor(session.kind),
            "block_reverted",
            &audit_detail(&id, &request.section_key, &request.field),
        )
        .await?;
    }
    let section = changed_section(&state, &id, &target).await?;
    Ok(Json(BlockChangeResponse {
        warnings: Vec::new(),
        section,
    }))
}

/// `POST /api/staff/presentations/{id}/confirm-note`: staff confirm that a
/// note-derived block reads correctly, so patients may see it.
pub async fn confirm_note(
    State(state): State<AppState>,
    CurrentSession(session): CurrentSession,
    Path(id): Path<String>,
    JsonBody(request): JsonBody<ConfirmNoteRequest>,
) -> Result<Json<BlockChangeResponse>, Failure> {
    let target = target(&state, &session, &id, &request.section_key, &request.field).await?;
    let generated = target.generated_block(&request.field);
    let note_derived = matches!(request.field.as_str(), "why" | "objective")
        && generated.source == BlockSource::ClinicalNote
        && generated.needs_confirmation;
    let mut section = changed_section(&state, &id, &target).await?;
    let edited = section
        .blocks
        .iter()
        .any(|b| b.field == request.field && b.source == BlockSource::StaffEdit);
    if !note_derived || edited {
        return Err(Failure::new(
            StatusCode::CONFLICT,
            "not_confirmable",
            "Only an unedited explanation from a clinical note needs confirmation.",
        ));
    }
    let inserted = sqlx::query(
        "INSERT INTO note_confirmations (presentation_id, version, section_key, field, confirmed_at)
         VALUES (?, ?, ?, ?, ?)
         ON CONFLICT (presentation_id, version, section_key, field) DO NOTHING",
    )
    .bind(&id)
    .bind(target.version)
    .bind(&request.section_key)
    .bind(&request.field)
    .bind(crate::sessions::now())
    .execute(&state.db)
    .await?
    .rows_affected();
    if inserted > 0 {
        audit::record(
            &state.db,
            actor(session.kind),
            "note_confirmed",
            &audit_detail(&id, &request.section_key, &request.field),
        )
        .await?;
    }
    if let Some(block) = section.blocks.iter_mut().find(|b| b.field == request.field) {
        block.needs_confirmation = false;
    }
    Ok(Json(BlockChangeResponse {
        warnings: Vec::new(),
        section,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_limits() {
        let ok = clean_sentences(&["  A short sentence.  ".into()]).unwrap();
        assert_eq!(ok, ["A short sentence."]);
        assert!(clean_sentences(&[]).is_err());
        assert!(clean_sentences(&vec!["Fine.".to_string(); 13]).is_err());
        assert!(clean_sentences(&["   ".into()]).is_err());
        assert!(clean_sentences(&["x".repeat(401)]).is_err());
        assert!(clean_sentences(&["x".repeat(400)]).is_ok());
        assert!(clean_sentences(&vec!["x".repeat(400); 6]).is_err());
        assert!(clean_sentences(&vec!["x".repeat(400); 5]).is_ok());
        assert!(clean_sentences(&["Line one.\nLine two.".into()]).is_err());
        for placeholder in [
            "Ask {{provider_display_name}} about it.",
            "Call us at {{practice_name}}.",
            "Half {{ open.",
            "Half }} closed.",
        ] {
            let err = clean_sentences(&[placeholder.into()]).unwrap_err();
            assert_eq!(err.code, "invalid_edit");
            assert!(err.message.contains("placeholders"));
        }
        assert!(clean_sentences(&["A { single } brace is fine.".into()]).is_ok());
    }
}
