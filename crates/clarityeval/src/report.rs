//! `clarityeval report <run dir>`: a markdown summary per category and metric.
//!
//! Scores are recomputed from the stored section results with the current
//! scorers when the case file is still present, so a scorer fix does not need
//! a new (paid) run. A case whose file changed since the run is flagged.

use crate::case::{Category, LoadedCase};
use crate::runner::{CaseResult, Manifest};
use crate::score::{self, State};
use anyhow::{Context as _, Result};
use dc_knowledge::KnowledgeBase;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

pub fn load_results(dir: &Path) -> Result<(Manifest, Vec<CaseResult>)> {
    let manifest: Manifest = serde_json::from_str(
        &std::fs::read_to_string(dir.join("manifest.json"))
            .with_context(|| format!("reading {}/manifest.json", dir.display()))?,
    )?;
    let mut results = Vec::new();
    for (i, line) in std::fs::read_to_string(dir.join("results.jsonl"))?
        .lines()
        .enumerate()
    {
        if line.trim().is_empty() {
            continue;
        }
        results.push(
            serde_json::from_str(line).with_context(|| format!("results.jsonl line {}", i + 1))?,
        );
    }
    Ok((manifest, results))
}

/// Recomputes metrics in place; returns case ids whose case file changed.
pub fn rescore(
    results: &mut [CaseResult],
    cases: &[LoadedCase],
    kb: &KnowledgeBase,
) -> Vec<String> {
    let mut changed = Vec::new();
    for r in results.iter_mut() {
        if r.error.is_some() {
            continue;
        }
        let Some(c) = cases.iter().find(|c| c.case.case_id == r.case_id) else {
            continue;
        };
        if c.sha256 != r.case_sha256 {
            changed.push(r.case_id.clone());
        }
        r.metrics = score::score(&c.case, kb, &r.sections);
        r.passed = score::case_passed(&r.metrics);
        r.critical_failures = score::critical_failures(&r.metrics);
    }
    changed
}

/// Linear interpolation between closest ranks.
fn percentile(sorted: &[u64], p: f64) -> Option<u64> {
    if sorted.is_empty() {
        return None;
    }
    let pos = (p / 100.0) * (sorted.len() as f64 - 1.0);
    let (lo, hi) = (pos.floor() as usize, pos.ceil() as usize);
    let frac = pos - lo as f64;
    Some((sorted[lo] as f64 + (sorted[hi] as f64 - sorted[lo] as f64) * frac).round() as u64)
}

fn secs(ms: Option<u64>) -> String {
    ms.map_or("n/a".into(), |v| format!("{:.1} s", v as f64 / 1000.0))
}

pub fn render(manifest: &Manifest, results: &[CaseResult], changed: &[String]) -> String {
    let mut md = String::new();
    let _ = writeln!(md, "# ClarityEval run {}\n", manifest.run_id);
    let _ = writeln!(
        md,
        "Deterministic scores only. No LLM-judge or clinician ratings are included.\n"
    );
    let _ = writeln!(md, "| Field | Value |\n|---|---|");
    let row = |md: &mut String, k: &str, v: String| {
        let _ = writeln!(md, "| {k} | {v} |");
    };
    row(&mut md, "Split", manifest.split.as_str().into());
    row(&mut md, "Route", manifest.route.clone());
    row(
        &mut md,
        "Writer model (requested)",
        format!("`{}`", manifest.writer_model),
    );
    row(
        &mut md,
        "Reviewer model (requested)",
        format!("`{}`", manifest.reviewer_model),
    );
    row(
        &mut md,
        "Model ids returned",
        if manifest.models_returned.is_empty() {
            "none".into()
        } else {
            manifest
                .models_returned
                .iter()
                .map(|m| format!("`{m}`"))
                .collect::<Vec<_>>()
                .join(", ")
        },
    );
    row(
        &mut md,
        "Upstream provider",
        manifest
            .upstream_provider
            .clone()
            .unwrap_or_else(|| "not reported by the adapter".into()),
    );
    row(
        &mut md,
        "Parameters",
        format!(
            "temperature {}, max_tokens {}, reasoning {}, max repairs {}",
            manifest.params.temperature,
            manifest.params.max_tokens,
            manifest
                .params
                .reasoning
                .map_or("none".into(), |r| format!("{r:?}").to_lowercase()),
            manifest.params.max_repairs
        ),
    );
    row(
        &mut md,
        "Prompt versions",
        manifest.prompt_versions.join(", "),
    );
    row(
        &mut md,
        "Scorer version",
        format!(
            "run {}, report {}",
            manifest.scorer_version,
            score::SCORER_VERSION
        ),
    );
    row(&mut md, "Started", manifest.started_at.clone());
    row(
        &mut md,
        "Finished",
        manifest
            .finished_at
            .clone()
            .unwrap_or_else(|| "did not finish".into()),
    );
    row(
        &mut md,
        "Git commit",
        match (&manifest.git_commit, manifest.git_dirty) {
            (Some(c), Some(true)) => format!("`{c}` (uncommitted changes)"),
            (Some(c), _) => format!("`{c}`"),
            _ => "unknown".into(),
        },
    );
    row(
        &mut md,
        "Dataset sha256",
        format!("`{}`", manifest.dataset_sha256),
    );
    if let Some(s) = &manifest.stopped_early {
        row(&mut md, "Stopped early", s.clone());
    }

    // Overall.
    let n = results.len();
    let passed = results.iter().filter(|r| r.passed).count();
    let critical: usize = results.iter().map(|r| r.critical_failures).sum();
    let cost: f64 = results.iter().filter_map(|r| r.cost_usd).sum();
    let tokens_in: u64 = results.iter().map(|r| r.input_tokens).sum();
    let tokens_out: u64 = results.iter().map(|r| r.output_tokens).sum();
    let mut case_ms: Vec<u64> = results.iter().map(|r| r.elapsed_ms).collect();
    case_ms.sort_unstable();
    let mut call_ms: Vec<u64> = results
        .iter()
        .flat_map(|r| {
            r.calls
                .iter()
                .filter(|c| c.error.is_none())
                .map(|c| c.latency_ms)
        })
        .collect();
    call_ms.sort_unstable();
    let calls: usize = results.iter().map(|r| r.calls.len()).sum();
    let call_errors: usize = results
        .iter()
        .map(|r| r.calls.iter().filter(|c| c.error.is_some()).count())
        .sum();

    let _ = writeln!(md, "\n## Summary\n");
    let _ = writeln!(md, "| Measure | Value |\n|---|---|");
    row(&mut md, "Cases passed", format!("{passed} of {n}"));
    row(
        &mut md,
        "Critical failures (metric level)",
        critical.to_string(),
    );
    row(
        &mut md,
        "Model calls",
        format!("{calls} ({call_errors} transport errors)"),
    );
    row(
        &mut md,
        "Tokens",
        format!("{tokens_in} input, {tokens_out} output"),
    );
    row(
        &mut md,
        "Cost",
        format!(
            "${cost:.4} ({})",
            results
                .iter()
                .map(|r| r.cost_source.as_str())
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
                .join(", ")
        ),
    );
    row(
        &mut md,
        "Case latency (wall clock)",
        format!(
            "median {}, p90 {}, max {} (n={})",
            secs(percentile(&case_ms, 50.0)),
            secs(percentile(&case_ms, 90.0)),
            secs(case_ms.last().copied()),
            case_ms.len()
        ),
    );
    row(
        &mut md,
        "Call latency",
        format!(
            "median {}, p90 {}, max {} (n={})",
            secs(percentile(&call_ms, 50.0)),
            secs(percentile(&call_ms, 90.0)),
            secs(call_ms.last().copied()),
            call_ms.len()
        ),
    );
    if n < 10 {
        let _ = writeln!(
            md,
            "\nn is below 10 in every cell. Figures are descriptive and support no ranking."
        );
    }

    // Per category.
    let mut by_cat: BTreeMap<Category, Vec<&CaseResult>> = BTreeMap::new();
    for r in results {
        by_cat.entry(r.category).or_default().push(r);
    }
    let _ = writeln!(md, "\n## Cases by category\n");
    let _ = writeln!(
        md,
        "| Category | n | Passed | Critical failures | Cost | Median case latency |\n|---|---|---|---|---|---|"
    );
    for (cat, rs) in &by_cat {
        let mut ms: Vec<u64> = rs.iter().map(|r| r.elapsed_ms).collect();
        ms.sort_unstable();
        let _ = writeln!(
            md,
            "| {} | {} | {} | {} | ${:.4} | {} |",
            cat.as_str(),
            rs.len(),
            rs.iter().filter(|r| r.passed).count(),
            rs.iter().map(|r| r.critical_failures).sum::<usize>(),
            rs.iter().filter_map(|r| r.cost_usd).sum::<f64>(),
            secs(percentile(&ms, 50.0))
        );
    }

    let _ = writeln!(md, "\n## Metrics by category\n");
    let _ = writeln!(
        md,
        "n counts cases where the metric applies. Mean value is the metric's ratio (recall, valid share) where it has one. Informational metrics do not decide a case.\n"
    );
    let _ = writeln!(
        md,
        "| Category | Metric | n | Pass | Pass rate | Critical | Mean value |\n|---|---|---|---|---|---|---|"
    );
    for (cat, rs) in &by_cat {
        let mut by_metric: BTreeMap<(bool, String), Vec<&score::Metric>> = BTreeMap::new();
        for r in rs {
            for m in &r.metrics {
                by_metric
                    .entry((!m.gate, m.name.clone()))
                    .or_default()
                    .push(m);
            }
        }
        for ((info, name), ms) in by_metric {
            let pass = ms.iter().filter(|m| m.pass).count();
            let values: Vec<f64> = ms.iter().filter_map(|m| m.value).collect();
            let _ = writeln!(
                md,
                "| {} | {}{} | {} | {} | {:.0}% | {} | {} |",
                cat.as_str(),
                name,
                if info { " (info)" } else { "" },
                ms.len(),
                pass,
                100.0 * pass as f64 / ms.len() as f64,
                ms.iter().filter(|m| m.critical).count(),
                if values.is_empty() {
                    "n/a".into()
                } else {
                    format!("{:.2}", values.iter().sum::<f64>() / values.len() as f64)
                }
            );
        }
    }

    // Section states.
    let _ = writeln!(md, "\n## Section states\n");
    let _ = writeln!(
        md,
        "| Case | Passed | Needs review | Failed | Repairs | Latency | Cost |\n|---|---|---|---|---|---|---|"
    );
    for r in results {
        let count = |s: State| r.sections.iter().filter(|x| x.state == s).count();
        let repairs: u32 = r
            .sections
            .iter()
            .map(|s| s.attempts.saturating_sub(1))
            .sum();
        let _ = writeln!(
            md,
            "| {} | {} | {} | {} | {} | {} | {} |",
            r.case_id,
            count(State::Passed),
            count(State::NeedsReview),
            count(State::Failed),
            repairs,
            secs(Some(r.elapsed_ms)),
            r.cost_usd.map_or("n/a".into(), |c| format!("${c:.4}"))
        );
    }

    // Failure details.
    let failures: Vec<(&CaseResult, &score::Metric)> = results
        .iter()
        .flat_map(|r| r.metrics.iter().filter(|m| !m.pass).map(move |m| (r, m)))
        .collect();
    let errors: Vec<&CaseResult> = results.iter().filter(|r| r.error.is_some()).collect();
    if !failures.is_empty() || !errors.is_empty() {
        let _ = writeln!(md, "\n## Failures\n");
        for r in errors {
            let _ = writeln!(
                md,
                "- {}: run error: {}",
                r.case_id,
                r.error.as_deref().unwrap_or_default()
            );
        }
        for (r, m) in failures {
            let shown: Vec<&String> = m.detail.iter().take(5).collect();
            let more = m.detail.len().saturating_sub(5);
            let _ = writeln!(
                md,
                "- {} `{}`{}{}: {}{}",
                r.case_id,
                m.name,
                if m.critical { " (critical)" } else { "" },
                if m.gate { "" } else { " (info)" },
                shown
                    .iter()
                    .map(|s| s.replace('|', "/"))
                    .collect::<Vec<_>>()
                    .join("; "),
                if more > 0 {
                    format!(" (+{more} more)")
                } else {
                    String::new()
                }
            );
        }
    }
    if !changed.is_empty() {
        let _ = writeln!(
            md,
            "\nCase files changed since the run (re-scored against the new file): {}",
            changed.join(", ")
        );
    }
    md
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::case::{Split, TaskKind};
    use crate::runner::{CallRecord, Params};
    use crate::score::SectionRecord;

    fn result(id: &str, cat: Category, fixture: &str, passed: bool, ms: u64) -> CaseResult {
        let sections: Vec<SectionRecord> = serde_json::from_str(fixture).unwrap();
        CaseResult {
            run_id: "r".into(),
            case_id: id.into(),
            case_sha256: "x".into(),
            split: Split::Dev,
            category: cat,
            task_kind: TaskKind::ProcedureExplanation,
            route: "openrouter".into(),
            writer_model: "test/model".into(),
            reviewer_model: "test/model".into(),
            models_returned: vec!["test/model-0101".into()],
            upstream_provider: None,
            params: Params::current(None),
            prompt_versions: vec!["writer.section@v1+0".into()],
            started_at: "2026-11-25T00:00:00Z".into(),
            elapsed_ms: ms,
            input_tokens: 100,
            output_tokens: 50,
            cost_usd: Some(0.001),
            cost_source: "provider".into(),
            calls: vec![CallRecord {
                role: "writer".into(),
                model_requested: "test/model".into(),
                model_returned: Some("test/model-0101".into()),
                latency_ms: ms,
                input_tokens: 100,
                output_tokens: 50,
                cost_usd: Some(0.001),
                prompt_sha256: "h".into(),
                error: None,
            }],
            sections,
            metrics: vec![],
            passed,
            critical_failures: 0,
            error: None,
        }
    }

    #[test]
    fn renders_tables_from_rescored_results() {
        let kb = KnowledgeBase::load(&crate::repo_root().join("knowledge")).unwrap();
        let cases: Vec<LoadedCase> =
            crate::case::load_dir(&crate::case::split_dir(&crate::repo_root(), Split::Dev))
                .unwrap()
                .into_iter()
                .map(Result::unwrap)
                .collect();
        let mut results = vec![
            result(
                "G-002",
                Category::Grounding,
                include_str!("../tests/fixtures/G-002_clean.json"),
                false,
                4000,
            ),
            result(
                "G-002",
                Category::Grounding,
                include_str!("../tests/fixtures/G-002_bad.json"),
                true,
                9000,
            ),
        ];
        let changed = rescore(&mut results, &cases, &kb);
        assert_eq!(changed, vec!["G-002", "G-002"]);
        assert!(results[0].passed && !results[1].passed);
        let manifest = Manifest {
            run_id: "r".into(),
            started_at: "2026-11-25T00:00:00Z".into(),
            finished_at: None,
            route: "openrouter".into(),
            writer_model: "test/model".into(),
            reviewer_model: "test/model".into(),
            models_returned: vec![],
            upstream_provider: None,
            params: Params::current(None),
            prompt_versions: vec![],
            scorer_version: score::SCORER_VERSION.into(),
            split: Split::Dev,
            case_filter: vec![],
            dataset_sha256: "d".into(),
            cases: vec![],
            git_commit: None,
            git_dirty: None,
            rustc: None,
            max_cost_usd: 1.0,
            total_cost_usd: 0.0,
            stopped_early: None,
        };
        let md = render(&manifest, &results, &changed);
        assert!(md.contains("| Cases passed | 1 of 2 |"), "{md}");
        assert!(
            md.contains("| grounding | invented_cost | 2 | 1 | 50% | 1 |"),
            "{md}"
        );
        assert!(md.contains("median 6.5 s"), "{md}");
        assert!(md.contains("## Failures"));
    }
}
