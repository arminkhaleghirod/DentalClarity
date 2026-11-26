//! The whole run flow offline: pipeline, baseline, judge, scoring and report,
//! answered by the scripted dry-run model. No network and no key.

use clarityeval::case::{self, Split};
use clarityeval::report::{self, Bootstrap};
use clarityeval::runner::{self, ModelChoice, RunOptions};
use clarityeval::systems::{SeedMode, SystemKind};
use dc_ai::ReasoningEffort;
use dc_knowledge::KnowledgeBase;

fn opts(out: std::path::PathBuf, split: Split, final_run: bool) -> RunOptions {
    let root = clarityeval::repo_root();
    RunOptions {
        name: Some("dry-test".into()),
        split,
        cases_dir: case::split_dir(&root, Split::Dev),
        final_run,
        allow_missing_commitment: false,
        models: vec![ModelChoice {
            model: "deepseek/deepseek-v4.1-flash-1017".into(),
            reviewer: None,
            reasoning: Some(ReasoningEffort::High),
        }],
        baseline: true,
        judge: Some("judge/model".into()),
        judge_reasoning: None,
        seed: 7,
        seed_mode: SeedMode::Auto,
        concurrency: 4,
        patterns: vec![],
        out: Some(out),
        limit: None,
        max_cost_usd: 1.0,
        dry_run: true,
        bootstrap: 300,
        command: vec!["run".into(), "--dry-run".into()],
        config_file: None,
    }
}

#[tokio::test]
async fn dry_run_exercises_the_whole_flow() {
    let root = clarityeval::repo_root();
    let kb = KnowledgeBase::load(&root.join("knowledge")).unwrap();
    let cases: Vec<_> = case::load_dir(&case::split_dir(&root, Split::Dev))
        .unwrap()
        .into_iter()
        .map(Result::unwrap)
        .collect();
    let out = std::env::temp_dir().join(format!("clarityeval-dry-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);

    let dir = runner::run(&root, &cases, &kb, &opts(out.clone(), Split::Dev, false))
        .await
        .unwrap();
    let (manifest, mut results) = report::load_results(&dir).unwrap();
    assert_eq!(manifest.systems.len(), 2);
    assert!(manifest.dry_run);
    assert_eq!(results.len(), cases.len() * 2);
    assert!(results.iter().all(|r| r.error.is_none()));
    // Every call records the returned model and route.
    assert!(results.iter().flat_map(|r| &r.calls).all(|c| {
        c.model_returned
            .as_deref()
            .is_some_and(|m| m.ends_with("-dryrun"))
            && c.provider_route.as_deref() == Some("dry-run")
    }));
    // The scripted writer plants a cost on the first procedure attempt; the
    // pipeline repairs it, so nothing unsafe is visible.
    let pipe: Vec<_> = results
        .iter()
        .filter(|r| r.system_kind == SystemKind::Pipeline)
        .collect();
    assert!(
        pipe.iter()
            .any(|r| r.sections.iter().any(|s| s.attempts == 2))
    );
    assert!(
        pipe.iter()
            .all(|r| r.visible.as_ref().unwrap().violation_count() == 0),
        "{:#?}",
        pipe.iter()
            .flat_map(|r| &r.visible.as_ref().unwrap().violations)
            .collect::<Vec<_>>()
    );
    // The baseline's planted cost and timing reach the patient.
    let base_violations: usize = results
        .iter()
        .filter(|r| r.system_kind == SystemKind::Baseline)
        .map(|r| r.visible.as_ref().unwrap().violation_count())
        .sum();
    assert!(base_violations > 0);
    assert!(results.iter().any(|r| !r.judgments.is_empty()));

    // Re-scoring offline reproduces the stored scores exactly.
    let stored: Vec<_> = results.iter().map(|r| r.visible.clone()).collect();
    let changed = report::rescore(&mut results, &cases, &kb);
    assert!(changed.is_empty());
    assert_eq!(
        stored,
        results
            .iter()
            .map(|r| r.visible.clone())
            .collect::<Vec<_>>()
    );

    let b = Bootstrap {
        resamples: 300,
        seed: 1,
    };
    let summary = report::summarize(&manifest, &results, &changed, b);
    let again = report::summarize(&manifest, &results, &changed, b);
    assert_eq!(summary.systems, again.systems, "bootstrap must be seeded");
    let p = &summary.paired[0];
    assert!(p.violations_per_presentation.value.unwrap() < 0.0);
    let md = report::write(&dir, &summary, &results).unwrap();
    assert!(md.contains("Automated judge, not clinician review"));
    assert!(md.contains("**Dry run.**"));
    assert!(dir.join("results.json").is_file());
    std::fs::remove_dir_all(&out).unwrap();
}

#[tokio::test]
async fn heldout_needs_final_and_a_commitment() {
    let root = clarityeval::repo_root();
    let kb = KnowledgeBase::load(&root.join("knowledge")).unwrap();
    let out = std::env::temp_dir().join("clarityeval-never-written");
    let err = runner::run(&root, &[], &kb, &opts(out.clone(), Split::Heldout, false))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("--final"), "{err}");
    if runner::read_commitment(&root).is_none() {
        let err = runner::run(&root, &[], &kb, &opts(out, Split::Heldout, true))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("commitment"), "{err}");
    }
}
