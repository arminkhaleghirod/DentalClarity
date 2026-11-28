//! Connector contract tests.
//!
//! Every connector must pass [`check_contract`]. The synthetic TOML connector
//! always runs. The Open Dental connector runs against the synthetic MySQL
//! database in `fixtures/opendental` when `DC_TEST_MYSQL_URL` is set, for
//! example `mysql://dentalclarity_ro:synthetic-test-only@127.0.0.1:3306/opendental`,
//! and is skipped otherwise. Against that database it must also return the
//! same patients and plans as the TOML fixture, apart from source ids.

use dc_core::PlanSnapshot;
use dc_core::plan::{PatientKey, PlanKey};
use dc_core::procedure::ProcedureStatus;
use dc_pms::synthetic::SyntheticConnector;
use dc_pms::{PatientSummary, PmsConnector, PmsError};
use std::collections::HashSet;
use std::path::Path;

fn synthetic() -> SyntheticConnector {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/synthetic/practice.toml");
    SyntheticConnector::from_path(&path).expect("fixture loads")
}

async fn check_contract(c: &dyn PmsConnector) {
    let everyone = c.search_patients("").await.unwrap();
    assert!(
        !everyone.is_empty(),
        "{}: empty search lists patients",
        c.name()
    );
    let keys: HashSet<_> = everyone.iter().map(|p| p.key.clone()).collect();
    assert_eq!(keys.len(), everyone.len(), "{}: keys are unique", c.name());

    for p in &everyone {
        let by_key = c.search_patients(&p.key.0).await.unwrap();
        assert!(
            by_key.contains(p),
            "{}: search by key finds {}",
            c.name(),
            p.key.0
        );
        let name = format!("{} {}", p.first_name, p.last_name).to_uppercase();
        let by_name = c.search_patients(&name).await.unwrap();
        assert!(
            by_name.contains(p),
            "{}: name search ignores case",
            c.name()
        );
        assert_eq!(
            p.is_guardian_presentation,
            p.age_years.is_some_and(|a| a < 18),
            "{}: guardian presentations are for minors",
            c.name()
        );
        if let Some(time) = &p.appointment_time {
            assert_eq!(time.len(), 5, "{}: appointment time is HH:MM", c.name());
        }

        for plan in c.treatment_plans(&p.key).await.unwrap() {
            let snap = c.plan_snapshot(&plan.key).await.unwrap();
            assert_eq!(snap.plan, plan.key);
            assert_eq!(snap.patient.key, p.key);
            assert_eq!(snap.procedures.len() as u32, plan.procedure_count);
            assert!(
                plan.procedure_count > 0,
                "{}: plans are not empty",
                c.name()
            );
            let ids: HashSet<_> = snap.procedures.iter().map(|x| &x.source_id).collect();
            assert_eq!(
                ids.len(),
                snap.procedures.len(),
                "{}: procedure ids unique",
                c.name()
            );
            for proc in &snap.procedures {
                assert_ne!(proc.status, ProcedureStatus::Completed);
                for amount in [proc.fee, proc.insurance_estimate, proc.patient_estimate] {
                    assert!(
                        amount.known().is_none_or(|m| m.0 >= 0),
                        "{}: amounts",
                        c.name()
                    );
                }
            }
            let phases: Vec<_> = snap.procedures.iter().filter_map(|x| x.phase).collect();
            assert!(
                phases.is_sorted(),
                "{}: procedures in phase order",
                c.name()
            );
        }
    }

    let recommended = c.recommended_patients().await.unwrap();
    assert!(
        !recommended.is_empty(),
        "{}: someone is recommended",
        c.name()
    );
    let times: Vec<_> = recommended
        .iter()
        .map(|r| {
            r.appointment_time
                .clone()
                .expect("recommended patients have a time")
        })
        .collect();
    assert!(
        times.is_sorted(),
        "{}: recommended in appointment order",
        c.name()
    );
    for r in &recommended {
        assert!(
            everyone.contains(r),
            "{}: recommended patients are searchable",
            c.name()
        );
        assert!(!c.treatment_plans(&r.key).await.unwrap().is_empty());
    }

    // Wildcards in a search are text, not patterns.
    assert!(c.search_patients("%").await.unwrap().is_empty());
    assert!(c.search_patients("_").await.unwrap().is_empty());

    let missing = PatientKey("987654321".into());
    assert!(matches!(
        c.treatment_plans(&missing).await,
        Err(PmsError::PatientNotFound(_))
    ));
    for plan in ["987654321", "not-a-plan"] {
        assert!(matches!(
            c.plan_snapshot(&PlanKey(plan.into())).await,
            Err(PmsError::PlanNotFound(_))
        ));
    }
}

#[tokio::test]
async fn synthetic_connector_meets_the_contract() {
    check_contract(&synthetic()).await;
}

/// Comparable form of a patient summary without its key.
#[cfg_attr(not(feature = "opendental"), allow(dead_code))]
fn person(p: &PatientSummary) -> impl PartialEq + std::fmt::Debug + use<> {
    (
        p.first_name.clone(),
        p.last_name.clone(),
        p.age_years,
        p.preferred_language.clone(),
        p.appointment_time.clone(),
        p.is_guardian_presentation,
    )
}

/// Comparable form of a snapshot without the source ids.
#[cfg_attr(not(feature = "opendental"), allow(dead_code))]
fn plan_facts(s: &PlanSnapshot) -> impl PartialEq + std::fmt::Debug + use<> {
    let procedures: Vec<_> = s
        .procedures
        .iter()
        .map(|p| {
            (
                p.code.clone(),
                p.pms_description.clone(),
                p.tooth,
                p.surfaces.clone(),
                p.phase,
                p.status,
                p.fee,
                p.insurance_estimate,
                p.patient_estimate,
                p.note.as_ref().map(|n| n.text.clone()),
            )
        })
        .collect();
    (
        s.patient.first_name.clone(),
        s.patient.last_name.clone(),
        s.patient.age_years,
        s.patient.preferred_language.clone(),
        s.plan_name.clone(),
        s.provider_display_name.clone(),
        procedures,
    )
}

/// Both connectors must tell the same story about the same practice.
#[cfg_attr(not(feature = "opendental"), allow(dead_code))]
async fn check_same_practice(expected: &dyn PmsConnector, actual: &dyn PmsConnector) {
    let want = expected.search_patients("").await.unwrap();
    let got = actual.search_patients("").await.unwrap();
    assert_eq!(want.len(), got.len(), "same number of patients");
    for w in &want {
        let g = got
            .iter()
            .find(|g| g.first_name == w.first_name && g.last_name == w.last_name)
            .unwrap_or_else(|| panic!("{} {} is missing", w.first_name, w.last_name));
        assert_eq!(person(w), person(g));
        let want_plans = expected.treatment_plans(&w.key).await.unwrap();
        let got_plans = actual.treatment_plans(&g.key).await.unwrap();
        assert_eq!(
            want_plans.len(),
            got_plans.len(),
            "plans of {}",
            w.last_name
        );
        for (wp, gp) in want_plans.iter().zip(&got_plans) {
            assert_eq!(wp.name, gp.name);
            assert_eq!(wp.procedure_count, gp.procedure_count);
            let ws = expected.plan_snapshot(&wp.key).await.unwrap();
            let gs = actual.plan_snapshot(&gp.key).await.unwrap();
            assert_eq!(plan_facts(&ws), plan_facts(&gs), "plan of {}", w.last_name);
        }
    }
    let order = |rows: Vec<PatientSummary>| -> Vec<_> { rows.iter().map(person).collect() };
    assert_eq!(
        order(expected.recommended_patients().await.unwrap()),
        order(actual.recommended_patients().await.unwrap()),
        "same recommended list"
    );
}

#[cfg(feature = "opendental")]
#[tokio::test]
async fn opendental_connector_meets_the_contract_on_synthetic_mysql() {
    use dc_pms::opendental::{MySqlConnectOptions, OpenDentalConnector};

    let Ok(url) = std::env::var("DC_TEST_MYSQL_URL") else {
        eprintln!("DC_TEST_MYSQL_URL not set; skipping the MySQL contract tests");
        return;
    };
    let options: MySqlConnectOptions = url.parse().expect("DC_TEST_MYSQL_URL parses");
    let od = OpenDentalConnector::connect(options)
        .await
        .expect("connects as the read-only user");
    check_contract(&od).await;
    check_same_practice(&synthetic(), &od).await;
}
