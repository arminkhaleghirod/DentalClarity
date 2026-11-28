//! Read-only connector for an Open Dental MySQL or MariaDB database.
//!
//! The connector only reads. At connect time it refuses a database user that
//! holds any privilege besides `SELECT` and `USAGE`, and every pooled
//! connection runs `SET SESSION TRANSACTION READ ONLY` as a second guard.
//! The SQL lives in [`queries`]. How each Open Dental column is read, and
//! which parts are not yet verified against a live install, is written up in
//! docs/OPENDENTAL.md and DECISIONS D13.

pub mod queries;

use crate::{PatientSummary, PlanSummary, PmsConnector, PmsError, Result};
use async_trait::async_trait;
use dc_core::money::{Amount, Money};
use dc_core::plan::{ClinicalNote, PatientContext, PatientKey, PlanKey, PlannedProcedure};
use dc_core::procedure::{ProcedureCode, ProcedureStatus};
use dc_core::tooth::{Arch, Quadrant};
use dc_core::{PlanSnapshot, ToothRef};
use sqlx::mysql::{MySqlPool, MySqlPoolOptions, MySqlRow};
use sqlx::{Executor, Row};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

pub use sqlx::mysql::{MySqlConnectOptions, MySqlSslMode};

/// Age below which a presentation is given to a parent or guardian.
const GUARDIAN_AGE: u8 = 18;

/// Connector over a connection pool to the Open Dental database.
#[derive(Debug, Clone)]
pub struct OpenDentalConnector {
    pool: MySqlPool,
}

impl OpenDentalConnector {
    /// Connects, checks that the user can only read, and keeps a small pool.
    pub async fn connect(options: MySqlConnectOptions) -> Result<Self> {
        let connector = Self::connect_unchecked(options).await?;
        let grants: Vec<String> = sqlx::query(queries::SHOW_GRANTS)
            .fetch_all(&connector.pool)
            .await
            .map_err(db_error)?
            .iter()
            // Some servers send the grant text with a binary collation.
            .map(|row| {
                row.try_get::<String, _>(0).or_else(|_| {
                    row.try_get::<Vec<u8>, _>(0)
                        .map(|b| String::from_utf8_lossy(&b).into_owned())
                })
            })
            .collect::<std::result::Result<_, _>>()
            .map_err(db_error)?;
        if let Err(reason) = check_grants(&grants) {
            connector.pool.close().await;
            return Err(PmsError::Unavailable(format!(
                "the Open Dental user must be read-only: {reason}"
            )));
        }
        Ok(connector)
    }

    /// Connects without the grant check. The session guard still applies.
    async fn connect_unchecked(options: MySqlConnectOptions) -> Result<Self> {
        let pool = MySqlPoolOptions::new()
            .max_connections(4)
            .acquire_timeout(Duration::from_secs(10))
            .after_connect(|conn, _meta| {
                Box::pin(async move {
                    conn.execute(queries::READ_ONLY_SESSION).await?;
                    Ok(())
                })
            })
            .connect_with(options)
            .await
            .map_err(db_error)?;
        Ok(Self { pool })
    }

    async fn fetch_all(&self, sql: &'static str, key: i64) -> Result<Vec<MySqlRow>> {
        sqlx::query(sql)
            .bind(key)
            .fetch_all(&self.pool)
            .await
            .map_err(db_error)
    }

    async fn provider_name(&self, prov_num: i64) -> Result<Option<String>> {
        let row = sqlx::query(queries::PROVIDER)
            .bind(prov_num)
            .fetch_optional(&self.pool)
            .await
            .map_err(db_error)?;
        let Some(row) = row else { return Ok(None) };
        Ok(provider_display_name(
            &get::<String>(&row, "LName")?,
            &get::<String>(&row, "PreferredName")?,
            get::<i64>(&row, "IsSecondary")? != 0,
            get::<i64>(&row, "IsNotPerson")? != 0,
        ))
    }
}

fn db_error(e: sqlx::Error) -> PmsError {
    match e {
        sqlx::Error::Io(_)
        | sqlx::Error::Tls(_)
        | sqlx::Error::PoolTimedOut
        | sqlx::Error::PoolClosed
        | sqlx::Error::WorkerCrashed => PmsError::Unavailable(e.to_string()),
        other => PmsError::Load(other.to_string()),
    }
}

fn get<'r, T>(row: &'r MySqlRow, column: &str) -> Result<T>
where
    T: sqlx::Decode<'r, sqlx::MySql> + sqlx::Type<sqlx::MySql>,
{
    row.try_get(column)
        .map_err(|e| PmsError::Load(format!("column {column}: {e}")))
}

fn text(row: &MySqlRow, column: &str) -> Result<String> {
    Ok(get::<Option<String>>(row, column)?.unwrap_or_default())
}

/// Parses a plan or patient key. Keys are Open Dental primary keys.
fn parse_key(key: &str) -> Option<i64> {
    key.trim().parse::<i64>().ok().filter(|n| *n > 0)
}

fn summary(row: &MySqlRow) -> Result<PatientSummary> {
    let age = age(get::<Option<i64>>(row, "Age")?);
    Ok(PatientSummary {
        key: PatientKey(get::<i64>(row, "PatNum")?.to_string()),
        first_name: text(row, "FName")?,
        last_name: text(row, "LName")?,
        age_years: age,
        preferred_language: language_tag(&text(row, "Language")?),
        appointment_time: get::<Option<String>>(row, "AptTime")?,
        is_guardian_presentation: age.is_some_and(|a| a < GUARDIAN_AGE),
    })
}

fn age(years: Option<i64>) -> Option<u8> {
    years
        .and_then(|y| u8::try_from(y).ok())
        .filter(|y| *y <= 130)
}

/// Escapes a search fragment for `LIKE ... ESCAPE '!'`.
fn like_pattern(query: &str) -> String {
    let mut out = String::with_capacity(query.len() + 2);
    out.push('%');
    for c in query.chars() {
        if matches!(c, '!' | '%' | '_') {
            out.push('!');
        }
        out.push(c);
    }
    out.push('%');
    out
}

/// BCP 47 tag for the language stored in `patient.Language`.
///
/// Open Dental stores ISO 639-2 codes such as `eng` and `spa`, or free text
/// for a custom language. Codes with an ISO 639-1 form are shortened to it.
/// Other three-letter codes are valid BCP 47 tags as they are. Free text is
/// dropped, since the app cannot tell what language it names.
pub fn language_tag(raw: &str) -> Option<String> {
    const TWO_LETTER: &[(&str, &str)] = &[
        ("amh", "am"),
        ("ara", "ar"),
        ("arm", "hy"),
        ("ben", "bn"),
        ("bur", "my"),
        ("chi", "zh"),
        ("deu", "de"),
        ("dut", "nl"),
        ("ell", "el"),
        ("eng", "en"),
        ("fas", "fa"),
        ("fil", "tl"),
        ("fra", "fr"),
        ("fre", "fr"),
        ("ger", "de"),
        ("gre", "el"),
        ("guj", "gu"),
        ("hat", "ht"),
        ("heb", "he"),
        ("hin", "hi"),
        ("hye", "hy"),
        ("ita", "it"),
        ("jpn", "ja"),
        ("khm", "km"),
        ("kor", "ko"),
        ("lao", "lo"),
        ("mar", "mr"),
        ("mya", "my"),
        ("nep", "ne"),
        ("nld", "nl"),
        ("pan", "pa"),
        ("per", "fa"),
        ("pol", "pl"),
        ("por", "pt"),
        ("ron", "ro"),
        ("rum", "ro"),
        ("rus", "ru"),
        ("som", "so"),
        ("spa", "es"),
        ("swa", "sw"),
        ("tam", "ta"),
        ("tel", "te"),
        ("tgl", "tl"),
        ("tha", "th"),
        ("tur", "tr"),
        ("ukr", "uk"),
        ("urd", "ur"),
        ("vie", "vi"),
        ("zho", "zh"),
    ];
    let code = raw.trim().to_ascii_lowercase();
    if !(2..=3).contains(&code.len()) || !code.bytes().all(|b| b.is_ascii_lowercase()) {
        return None;
    }
    if code.len() == 2 {
        return Some(code);
    }
    Some(
        TWO_LETTER
            .iter()
            .find(|(three, _)| *three == code)
            .map_or(code.clone(), |(_, two)| (*two).to_owned()),
    )
}

/// Open Dental's `TreatmentArea` enum on `procedurecode.TreatArea`.
mod treat_area {
    pub const SURF: i64 = 1;
    pub const QUAD: i64 = 4;
    pub const ARCH: i64 = 6;
}

/// Where a procedure is and which surfaces it covers.
///
/// `ToothNum` holds a Universal tooth number for every area type. For
/// quadrant and arch codes, `Surf` holds `UR`, `UL`, `LR`, `LL` or `U`, `L`
/// instead of surfaces. Sextant, tooth range and whole-mouth codes have no
/// location the app can show, so they are [`ToothRef::None`] unless a tooth
/// number is set.
pub fn location(
    area: i64,
    tooth_num: &str,
    surf: &str,
) -> std::result::Result<(ToothRef, String), String> {
    let surf = surf.trim();
    match area {
        treat_area::QUAD => {
            let quadrant = match surf.to_ascii_uppercase().as_str() {
                "UR" => Quadrant::UpperRight,
                "UL" => Quadrant::UpperLeft,
                "LL" => Quadrant::LowerLeft,
                "LR" => Quadrant::LowerRight,
                _ => return Err(format!("quadrant code with area {surf:?}")),
            };
            Ok((ToothRef::Quadrant(quadrant), String::new()))
        }
        treat_area::ARCH => {
            let arch = match surf.to_ascii_uppercase().as_str() {
                "U" => Arch::Upper,
                "L" => Arch::Lower,
                _ => return Err(format!("arch code with area {surf:?}")),
            };
            Ok((ToothRef::Arch(arch), String::new()))
        }
        _ => {
            let tooth = tooth_num
                .parse::<ToothRef>()
                .map_err(|_| format!("tooth number {:?} is not supported", tooth_num.trim()))?;
            let surfaces = if area == treat_area::SURF {
                surf.to_owned()
            } else {
                String::new()
            };
            Ok((tooth, surfaces))
        }
    }
}

/// Phase from the priority definition: its name when the name is a number,
/// otherwise its 1-based position in the priority list.
pub fn phase(name: Option<&str>, order: Option<i64>) -> Option<u8> {
    let name = name?.trim();
    if let Ok(n) = name.parse::<u8>()
        && n > 0
    {
        return Some(n);
    }
    order
        .and_then(|o| u8::try_from(o.saturating_add(1)).ok())
        .filter(|p| *p > 0)
}

/// `ProcStatus` 1 and 8 are both planned; attached to a scheduled
/// appointment (`AptStatus` 1) the procedure counts as scheduled.
fn status(appointment_status: Option<i64>) -> ProcedureStatus {
    if appointment_status == Some(1) {
        ProcedureStatus::Scheduled
    } else {
        ProcedureStatus::Planned
    }
}

/// Dollars stored as a double, in whole cents.
fn cents(dollars: f64) -> Option<i64> {
    let c = (dollars * 100.0).round();
    (c.is_finite() && c.abs() < 1e13).then_some(c as i64)
}

/// The insurance rows for one procedure, summed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EstimateRows {
    pub insurance: f64,
    pub write_off: f64,
    pub capitation_rows: i64,
}

/// Fee, insurance estimate and patient estimate in cents.
///
/// The fee is `ProcFee` times the units (`UnitQty + BaseUnits`, at least 1).
/// The patient estimate is the fee less the discount, the insurance estimate
/// and the estimated write-off. With no estimate rows, with a capitation
/// estimate, or when the result would be negative, both estimates are
/// unknown so staff check them in Open Dental.
pub fn amounts(
    proc_fee: f64,
    units: i64,
    discount: f64,
    rows: Option<EstimateRows>,
) -> (Amount, Amount, Amount) {
    let units = units.max(1) as f64;
    let Some(fee) = cents(proc_fee * units).filter(|f| *f >= 0) else {
        return (Amount::Unknown, Amount::Unknown, Amount::Unknown);
    };
    let unknown = (Amount::Known(Money(fee)), Amount::Unknown, Amount::Unknown);
    let Some(rows) = rows.filter(|r| r.capitation_rows == 0) else {
        return unknown;
    };
    let (Some(insurance), Some(write_off), Some(discount)) = (
        cents(rows.insurance),
        cents(rows.write_off),
        cents(discount),
    ) else {
        return unknown;
    };
    let patient = fee - discount - insurance - write_off;
    if insurance < 0 || patient < 0 {
        return unknown;
    }
    (
        Amount::Known(Money(fee)),
        Amount::Known(Money(insurance)),
        Amount::Known(Money(patient)),
    )
}

/// The name shown for the plan's provider, or `None` to leave it out.
///
/// `PreferredName` is what Open Dental shows patients. Without it, a dentist
/// is shown as "Dr." and the last name. Hygienists (`IsSecondary`) and
/// placeholder providers (`IsNotPerson`) are not shown as doctors.
pub fn provider_display_name(
    last_name: &str,
    preferred: &str,
    is_secondary: bool,
    is_not_person: bool,
) -> Option<String> {
    if is_not_person {
        return None;
    }
    let preferred = preferred.trim();
    if !preferred.is_empty() {
        return Some(preferred.to_owned());
    }
    let last = last_name.trim();
    (!is_secondary && !last.is_empty()).then(|| format!("Dr. {last}"))
}

/// Accepts grants made only of `USAGE` and `SELECT`, including column-level
/// `SELECT (...)` grants. Anything else, a role grant included, is refused,
/// since the connector cannot see what a role holds.
pub fn check_grants(grants: &[String]) -> std::result::Result<(), String> {
    for grant in grants {
        let line = grant.trim();
        let upper = line.to_ascii_uppercase();
        let Some(rest) = upper.strip_prefix("GRANT ") else {
            return Err(format!("unrecognized grant {line:?}"));
        };
        let Some(on) = find_outside_parens(rest, " ON ") else {
            return Err(format!("role or proxy grant {line:?}"));
        };
        if upper.contains(" WITH GRANT OPTION") {
            return Err(format!("grant option in {line:?}"));
        }
        for privilege in split_outside_parens(&rest[..on]) {
            let name = privilege.split('(').next().unwrap_or("").trim();
            if name != "SELECT" && name != "USAGE" {
                return Err(format!("{name} privilege in {line:?}"));
            }
        }
    }
    Ok(())
}

fn find_outside_parens(s: &str, needle: &str) -> Option<usize> {
    let mut depth = 0i32;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            _ if depth == 0 && s[i..].starts_with(needle) => return Some(i),
            _ => {}
        }
    }
    None
}

fn split_outside_parens(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&s[start..]);
    parts
}

fn plan_name(heading: &str, tp_status: i64) -> String {
    let heading = heading.trim();
    if !heading.is_empty() {
        heading.to_owned()
    } else if tp_status == 1 {
        "Active treatment plan".into()
    } else {
        "Inactive treatment plan".into()
    }
}

#[async_trait]
impl PmsConnector for OpenDentalConnector {
    fn name(&self) -> &'static str {
        "opendental"
    }

    async fn search_patients(&self, query: &str) -> Result<Vec<PatientSummary>> {
        let q = query.trim().to_lowercase();
        let pat_num = parse_key(&q).unwrap_or(-1);
        let rows = sqlx::query(queries::SEARCH_PATIENTS)
            .bind(&q)
            .bind(like_pattern(&q))
            .bind(pat_num)
            .fetch_all(&self.pool)
            .await
            .map_err(db_error)?;
        rows.iter().map(summary).collect()
    }

    async fn recommended_patients(&self) -> Result<Vec<PatientSummary>> {
        let rows = sqlx::query(queries::RECOMMENDED_PATIENTS)
            .fetch_all(&self.pool)
            .await
            .map_err(db_error)?;
        rows.iter().map(summary).collect()
    }

    async fn treatment_plans(&self, patient: &PatientKey) -> Result<Vec<PlanSummary>> {
        let not_found = || PmsError::PatientNotFound(patient.0.clone());
        let pat_num = parse_key(&patient.0).ok_or_else(not_found)?;
        if self
            .fetch_all(queries::PATIENT_EXISTS, pat_num)
            .await?
            .is_empty()
        {
            return Err(not_found());
        }
        self.fetch_all(queries::TREATMENT_PLANS, pat_num)
            .await?
            .iter()
            .map(|row| {
                Ok(PlanSummary {
                    key: PlanKey(get::<i64>(row, "TreatPlanNum")?.to_string()),
                    name: plan_name(&text(row, "Heading")?, get::<i64>(row, "TPStatus")?),
                    procedure_count: u32::try_from(get::<i64>(row, "Procs")?).unwrap_or(0),
                })
            })
            .collect()
    }

    async fn plan_snapshot(&self, plan: &PlanKey) -> Result<PlanSnapshot> {
        let not_found = || PmsError::PlanNotFound(plan.0.clone());
        let plan_num = parse_key(&plan.0).ok_or_else(not_found)?;
        let header = self.fetch_all(queries::PLAN_HEADER, plan_num).await?;
        let header = header.first().ok_or_else(not_found)?;
        let last_name = text(header, "LName")?;
        let patient = PatientContext {
            key: PatientKey(get::<i64>(header, "PatNum")?.to_string()),
            first_name: text(header, "FName")?,
            last_name,
            age_years: age(get::<Option<i64>>(header, "Age")?),
            preferred_language: language_tag(&text(header, "Language")?),
        };
        let plan_name = plan_name(&text(header, "Heading")?, get::<i64>(header, "TPStatus")?);

        let mut estimates = HashMap::new();
        for row in self.fetch_all(queries::PLAN_ESTIMATES, plan_num).await? {
            estimates.insert(
                get::<i64>(&row, "ProcNum")?,
                EstimateRows {
                    insurance: get::<Option<f64>>(&row, "InsEst")?.unwrap_or(0.0),
                    write_off: get::<Option<f64>>(&row, "WriteOffEst")?.unwrap_or(0.0),
                    capitation_rows: get::<Option<i64>>(&row, "CapRows")?.unwrap_or(0),
                },
            );
        }
        let mut notes: HashMap<i64, ClinicalNote> = HashMap::new();
        for row in self.fetch_all(queries::PLAN_NOTES, plan_num).await? {
            let proc_num = get::<i64>(&row, "ProcNum")?;
            if notes.contains_key(&proc_num) {
                continue;
            }
            let text = text(&row, "Note")?;
            let text = text.trim();
            if !text.is_empty() {
                notes.insert(
                    proc_num,
                    ClinicalNote {
                        source_id: format!("procnote-{}", get::<i64>(&row, "ProcNoteNum")?),
                        text: text.to_owned(),
                    },
                );
            } else {
                // The newest note is blank: the note was cleared.
                notes.insert(
                    proc_num,
                    ClinicalNote {
                        source_id: String::new(),
                        text: String::new(),
                    },
                );
            }
        }

        let mut seen = HashSet::new();
        let mut providers: HashMap<i64, usize> = HashMap::new();
        let mut procedures = Vec::new();
        for row in self.fetch_all(queries::PLAN_PROCEDURES, plan_num).await? {
            let proc_num = get::<i64>(&row, "ProcNum")?;
            if !seen.insert(proc_num) {
                continue;
            }
            let (tooth, surfaces) = location(
                get::<i64>(&row, "TreatArea")?,
                &text(&row, "ToothNum")?,
                &text(&row, "Surf")?,
            )
            .map_err(|e| PmsError::Load(format!("plan {plan_num} procedure {proc_num}: {e}")))?;
            let units = get::<i64>(&row, "UnitQty")? + get::<i64>(&row, "BaseUnits")?;
            let (fee, insurance_estimate, patient_estimate) = amounts(
                get::<f64>(&row, "ProcFee")?,
                units,
                get::<f64>(&row, "Discount")?,
                estimates.get(&proc_num).copied(),
            );
            let layman = text(&row, "LaymanTerm")?;
            let description = if layman.trim().is_empty() {
                text(&row, "Descript")?
            } else {
                layman
            };
            *providers.entry(get::<i64>(&row, "ProvNum")?).or_default() += 1;
            procedures.push(PlannedProcedure {
                source_id: format!("procedurelog-{proc_num}"),
                code: ProcedureCode::new(&text(&row, "ProcCode")?),
                pms_description: description.trim().to_owned(),
                tooth,
                surfaces,
                phase: phase(
                    get::<Option<String>>(&row, "PriorityName")?.as_deref(),
                    get::<Option<i64>>(&row, "PriorityOrder")?,
                ),
                status: status(get::<Option<i64>>(&row, "AptStatus")?),
                fee,
                insurance_estimate,
                patient_estimate,
                note: notes.remove(&proc_num).filter(|n| !n.text.is_empty()),
            });
        }
        if procedures.is_empty() {
            return Err(not_found());
        }

        // The provider on most procedures; ties go to the lower number.
        let provider = providers
            .into_iter()
            .filter(|(num, _)| *num > 0)
            .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))
            .map(|(num, _)| num);
        let provider_display_name = match provider {
            Some(num) => self.provider_name(num).await?,
            None => None,
        };

        Ok(PlanSnapshot {
            plan: PlanKey(plan_num.to_string()),
            patient,
            plan_name,
            provider_display_name,
            procedures,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn languages_become_bcp47_tags() {
        assert_eq!(language_tag("eng").as_deref(), Some("en"));
        assert_eq!(language_tag(" SPA ").as_deref(), Some("es"));
        assert_eq!(language_tag("vie").as_deref(), Some("vi"));
        assert_eq!(language_tag("hmn").as_deref(), Some("hmn"));
        assert_eq!(language_tag("en").as_deref(), Some("en"));
        assert_eq!(language_tag(""), None);
        assert_eq!(language_tag("Tahitian"), None);
        assert_eq!(language_tag("e1g"), None);
    }

    #[test]
    fn reads_teeth_quadrants_and_arches() {
        assert_eq!(
            location(1, "14", "MO"),
            Ok((ToothRef::Permanent(14), "MO".into()))
        );
        assert_eq!(
            location(2, "K", "O"),
            Ok((ToothRef::Primary('K'), "".into()))
        );
        assert_eq!(
            location(4, "", "lr"),
            Ok((ToothRef::Quadrant(Quadrant::LowerRight), "".into()))
        );
        assert_eq!(
            location(6, "", "U"),
            Ok((ToothRef::Arch(Arch::Upper), "".into()))
        );
        assert_eq!(location(3, "", ""), Ok((ToothRef::None, "".into())));
        assert_eq!(location(5, "", "2"), Ok((ToothRef::None, "".into())));
        assert!(location(4, "", "").is_err());
        assert!(location(6, "", "UR").is_err());
        // Supernumerary teeth have no place on the chart, so the plan fails
        // rather than showing "no specific tooth".
        assert!(location(2, "51", "").is_err());
    }

    #[test]
    fn phases_come_from_priority_definitions() {
        assert_eq!(phase(Some("2"), Some(1)), Some(2));
        assert_eq!(phase(Some("Urgent"), Some(0)), Some(1));
        assert_eq!(phase(Some("Later"), Some(3)), Some(4));
        assert_eq!(phase(None, None), None);
    }

    #[test]
    fn scheduled_needs_a_scheduled_appointment() {
        assert_eq!(status(Some(1)), ProcedureStatus::Scheduled);
        assert_eq!(status(Some(5)), ProcedureStatus::Planned);
        assert_eq!(status(None), ProcedureStatus::Planned);
    }

    fn rows(insurance: f64, write_off: f64) -> Option<EstimateRows> {
        Some(EstimateRows {
            insurance,
            write_off,
            capitation_rows: 0,
        })
    }

    fn known(c: i64) -> Amount {
        Amount::Known(Money(c))
    }

    #[test]
    fn patient_estimate_subtracts_insurance_write_off_and_discount() {
        assert_eq!(
            amounts(1150.0, 1, 0.0, rows(690.0, 0.0)),
            (known(115_000), known(69_000), known(46_000))
        );
        assert_eq!(
            amounts(1000.0, 1, 50.0, rows(500.0, 150.0)),
            (known(100_000), known(50_000), known(30_000))
        );
        // Units multiply the fee; zero units still count once.
        assert_eq!(amounts(55.0, 2, 0.0, rows(110.0, 0.0)).0, known(11_000));
        assert_eq!(amounts(55.0, 0, 0.0, rows(55.0, 0.0)).0, known(5_500));
        // Doubles round to the nearest cent.
        assert_eq!(amounts(0.1 + 0.2, 1, 0.0, rows(0.0, 0.0)).0, known(30));
    }

    #[test]
    fn missing_or_odd_estimates_stay_unknown() {
        let unknown = (known(16_500), Amount::Unknown, Amount::Unknown);
        assert_eq!(amounts(165.0, 1, 0.0, None), unknown);
        assert_eq!(amounts(165.0, 1, 0.0, rows(200.0, 0.0)), unknown);
        let cap = Some(EstimateRows {
            insurance: 0.0,
            write_off: 0.0,
            capitation_rows: 1,
        });
        assert_eq!(amounts(165.0, 1, 0.0, cap), unknown);
    }

    #[test]
    fn provider_names() {
        assert_eq!(
            provider_display_name("Patel", "", false, false).as_deref(),
            Some("Dr. Patel")
        );
        assert_eq!(
            provider_display_name("Patel", "Dr. Anika Patel", false, false).as_deref(),
            Some("Dr. Anika Patel")
        );
        assert_eq!(provider_display_name("Lee", "", true, false), None);
        assert_eq!(provider_display_name("Billing", "", false, true), None);
    }

    #[test]
    fn grants_must_be_select_only() {
        let ok = [
            "GRANT USAGE ON *.* TO `dentalclarity_ro`@`localhost`".to_owned(),
            "GRANT SELECT ON `opendental`.`treatplan` TO `dentalclarity_ro`@`localhost`".to_owned(),
            "GRANT SELECT (`PatNum`, `FName`, `LName`) ON `opendental`.`patient` TO `dentalclarity_ro`@`localhost`".to_owned(),
        ];
        assert_eq!(check_grants(&ok), Ok(()));
        for bad in [
            "GRANT ALL PRIVILEGES ON *.* TO `root`@`%` WITH GRANT OPTION",
            "GRANT SELECT, INSERT ON `opendental`.* TO `x`@`%`",
            "GRANT SELECT ON `opendental`.* TO `x`@`%` WITH GRANT OPTION",
            "GRANT `od_reader`@`%` TO `x`@`%`",
            "GRANT SELECT (`a`), UPDATE (`b`) ON `opendental`.`patient` TO `x`@`%`",
            "REVOKE SELECT ON x FROM y",
        ] {
            assert!(check_grants(&[bad.to_owned()]).is_err(), "{bad}");
        }
    }

    #[test]
    fn search_patterns_escape_wildcards() {
        assert_eq!(like_pattern("ok"), "%ok%");
        assert_eq!(like_pattern("50%_!"), "%50!%!_!!%");
    }

    /// Needs an account with write privileges on the synthetic database,
    /// given in `DC_TEST_MYSQL_ADMIN_URL`. Skipped when it is not set.
    #[tokio::test]
    async fn refuses_writable_users_and_guards_the_session() {
        let Ok(url) = std::env::var("DC_TEST_MYSQL_ADMIN_URL") else {
            eprintln!("DC_TEST_MYSQL_ADMIN_URL not set; skipping");
            return;
        };
        let options: MySqlConnectOptions = url.parse().expect("admin URL parses");
        let err = OpenDentalConnector::connect(options.clone())
            .await
            .expect_err("an account that can write is refused");
        assert!(err.to_string().contains("read-only"), "{err}");

        let unchecked = OpenDentalConnector::connect_unchecked(options)
            .await
            .unwrap();
        let write = sqlx::query("UPDATE patient SET FName = FName WHERE PatNum = 0")
            .execute(&unchecked.pool)
            .await;
        assert!(write.is_err(), "the session guard blocks writes");
    }

    /// Runs against the seeded read-only user in `DC_TEST_MYSQL_URL`.
    #[tokio::test]
    async fn read_only_user_cannot_write_or_read_unneeded_columns() {
        let Ok(url) = std::env::var("DC_TEST_MYSQL_URL") else {
            eprintln!("DC_TEST_MYSQL_URL not set; skipping");
            return;
        };
        let c = OpenDentalConnector::connect(url.parse().unwrap())
            .await
            .unwrap();
        for sql in [
            "UPDATE patient SET FName = FName WHERE PatNum = 0",
            "DELETE FROM procedurelog WHERE ProcNum = 0",
            "SELECT SSN FROM patient LIMIT 1",
        ] {
            assert!(sqlx::query(sql).execute(&c.pool).await.is_err(), "{sql}");
        }
    }
}
