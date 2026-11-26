//! `clarityeval report <run dir>`: the comparison report (report.md) and the
//! machine-readable results (results.json).
//!
//! Scores are recomputed from the stored section results with the current
//! scorers when the case file is still present, so a scorer fix does not need
//! a new (paid) run. A case whose file changed since the run is flagged.
//! Every interval is a 95% cluster-bootstrap percentile interval over base
//! plans with a fixed seed (docs/CLARITYEVAL.md section 13).

use crate::case::LoadedCase;
use crate::runner::{CaseResult, Manifest};
use crate::score::visible::{self, ViolationKind};
use crate::score::{self, State};
use crate::stats::{self, Estimate, Obs};
use crate::systems::{SystemKind, reasoning_str};
use anyhow::{Context as _, Result};
use dc_knowledge::KnowledgeBase;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
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

/// Recomputes metrics and patient-visible scores in place; returns case ids
/// whose case file changed since the run.
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
        if c.sha256 != r.case_sha256 && !changed.contains(&r.case_id) {
            changed.push(r.case_id.clone());
        }
        if r.plan_ref.is_empty() {
            r.plan_ref = c.case.plan_ref.clone();
        }
        r.metrics = score::score(&c.case, kb, &r.sections);
        r.passed = score::case_passed(&r.metrics);
        r.critical_failures = score::critical_failures(&r.metrics);
        r.visible = Some(visible::score_visible(
            &c.case,
            kb,
            r.system_kind,
            &r.sections,
        ));
    }
    changed
}

/// Bootstrap settings, recorded in results.json and the report.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Bootstrap {
    pub resamples: usize,
    pub seed: u64,
}

impl Default for Bootstrap {
    fn default() -> Self {
        Bootstrap {
            resamples: stats::BOOTSTRAP_RESAMPLES,
            seed: stats::BOOTSTRAP_SEED,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Metrics {
    /// Patient-visible unsafe or unsupported statements per presentation.
    pub violations_per_presentation: Option<Estimate>,
    /// The same, leaving out unconfirmed clinical-note content, so a reader can
    /// see how much of a gap comes from the app's confirmation hold alone.
    #[serde(default)]
    pub violations_excl_note_per_presentation: Option<Estimate>,
    /// Share of presentations with zero visible violations.
    pub zero_violation_rate: Option<Estimate>,
    /// Share of visible citations that were ids the system was given.
    pub citation_validity: Option<Estimate>,
    /// Share of unsupported procedures with no patient-visible text.
    pub unsupported_held_rate: Option<Estimate>,
    /// Share of presentations (with planted identifiers) leaking any of them.
    pub identifier_leak_rate: Option<Estimate>,
    /// Share of requested sections withheld from patients.
    pub withheld_section_rate: Option<Estimate>,
    /// Share of written sections that parse as the section schema.
    pub schema_valid_rate: Option<Estimate>,
    /// US dollars per presentation (provider-reported).
    pub cost_usd_per_presentation: Option<Estimate>,
    pub latency_p50_s: Option<f64>,
    pub latency_p90_s: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct JudgeSummary {
    pub sections_judged: usize,
    pub judge_failures: usize,
    pub faithfulness: Option<Estimate>,
    pub balance: Option<Estimate>,
    /// Sections flagged critical per presentation.
    pub critical_per_presentation: Option<Estimate>,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SystemSummary {
    pub label: String,
    pub kind: SystemKind,
    pub writer_model: String,
    /// Presentations scored (infrastructure failures excluded).
    pub presentations: usize,
    pub infra_failures: usize,
    pub metrics: Metrics,
    pub by_category: BTreeMap<String, Metrics>,
    /// Visible violations by kind, summed over presentations.
    pub violations_by_kind: BTreeMap<String, usize>,
    pub cost_usd_total: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub calls: usize,
    pub call_errors: usize,
    pub call_latency_p50_s: Option<f64>,
    pub judge: Option<JudgeSummary>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Paired {
    pub model: String,
    /// Pipeline minus baseline, per presentation, on the same cases.
    pub violations_per_presentation: Estimate,
    #[serde(default = "Estimate::empty")]
    pub violations_excl_note_per_presentation: Estimate,
    pub withheld_section_rate: Estimate,
    pub cost_usd_per_presentation: Estimate,
}

/// results.json.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    pub run_id: String,
    pub generated_at: String,
    pub scorer_version: String,
    pub visible_scorer_version: String,
    pub bootstrap: Bootstrap,
    pub systems: Vec<SystemSummary>,
    pub paired: Vec<Paired>,
    pub changed_cases: Vec<String>,
    pub manifest: Manifest,
}

fn cluster(r: &CaseResult) -> &str {
    if r.plan_ref.is_empty() {
        &r.case_id
    } else {
        &r.plan_ref
    }
}

/// Whole-presentation infrastructure failures: a run error, or every section
/// lost to transport errors. They are counted, not scored.
pub fn infra_failure(r: &CaseResult) -> bool {
    r.error.is_some()
        || r.visible.is_none()
        || (!r.sections.is_empty() && r.sections.iter().all(|s| s.transport_error()))
}

fn est(obs: &[Obs], b: Bootstrap) -> Option<Estimate> {
    (!obs.is_empty()).then(|| stats::cluster_bootstrap(obs, b.resamples, b.seed))
}

fn excl_note(v: &visible::VisibleScore) -> usize {
    v.violations
        .iter()
        .filter(|x| x.kind != ViolationKind::UnconfirmedNote)
        .count()
}

/// Numerator and denominator of a per-presentation ratio, if it applies.
type RatioFn = dyn Fn(&CaseResult, &visible::VisibleScore) -> Option<(f64, f64)>;

pub fn metrics(rs: &[&CaseResult], b: Bootstrap) -> Metrics {
    let scored: Vec<(&CaseResult, &visible::VisibleScore)> = rs
        .iter()
        .filter(|r| !infra_failure(r))
        .filter_map(|r| r.visible.as_ref().map(|v| (*r, v)))
        .collect();
    let obs = |f: &RatioFn| {
        scored
            .iter()
            .filter_map(|(r, v)| f(r, v).map(|(n, d)| Obs::ratio(cluster(r), n, d)))
            .collect::<Vec<_>>()
    };
    let mut lat: Vec<f64> = scored
        .iter()
        .map(|(r, _)| r.elapsed_ms as f64 / 1000.0)
        .collect();
    lat.sort_by(f64::total_cmp);
    Metrics {
        violations_per_presentation: est(&obs(&|_, v| Some((v.violation_count() as f64, 1.0))), b),
        violations_excl_note_per_presentation: est(
            &obs(&|_, v| Some((excl_note(v) as f64, 1.0))),
            b,
        ),
        zero_violation_rate: est(
            &obs(&|_, v| Some((f64::from(u8::from(v.violation_count() == 0)), 1.0))),
            b,
        ),
        citation_validity: est(
            &obs(&|_, v| {
                (v.cites_total > 0).then_some((v.cites_valid as f64, v.cites_total as f64))
            }),
            b,
        ),
        unsupported_held_rate: est(
            &obs(&|_, v| {
                (v.unsupported_total > 0)
                    .then_some((v.unsupported_held as f64, v.unsupported_total as f64))
            }),
            b,
        ),
        identifier_leak_rate: est(
            &obs(&|_, v| {
                (v.identifiers_checked > 0)
                    .then_some((f64::from(u8::from(v.identifiers_leaked > 0)), 1.0))
            }),
            b,
        ),
        withheld_section_rate: est(
            &obs(&|_, v| {
                (v.sections_requested > 0)
                    .then_some((v.sections_withheld as f64, v.sections_requested as f64))
            }),
            b,
        ),
        schema_valid_rate: est(
            &obs(&|_, v| {
                (v.sections_written > 0).then_some((
                    (v.sections_written - v.schema_failures) as f64,
                    v.sections_written as f64,
                ))
            }),
            b,
        ),
        cost_usd_per_presentation: est(&obs(&|r, _| r.cost_usd.map(|c| (c, 1.0))), b),
        latency_p50_s: stats::percentile(&lat, 50.0),
        latency_p90_s: stats::percentile(&lat, 90.0),
    }
}

fn judge_summary(rs: &[&CaseResult], b: Bootstrap) -> Option<JudgeSummary> {
    if rs.iter().all(|r| r.judgments.is_empty()) {
        return None;
    }
    let scored: Vec<&&CaseResult> = rs.iter().filter(|r| !infra_failure(r)).collect();
    let mut faith = Vec::new();
    let mut bal = Vec::new();
    let mut crit = Vec::new();
    let mut s = JudgeSummary::default();
    for r in &scored {
        let ok: Vec<&crate::judge::JudgeOut> = r
            .judgments
            .iter()
            .filter_map(|j| j.result.as_ref())
            .collect();
        s.sections_judged += ok.len();
        s.judge_failures += r.judgments.len() - ok.len();
        s.cost_usd += r.judgments.iter().filter_map(|j| j.cost_usd).sum::<f64>();
        if !ok.is_empty() {
            let n = ok.len() as f64;
            faith.push(Obs::ratio(
                cluster(r),
                ok.iter().map(|o| f64::from(o.faithfulness.score)).sum(),
                n,
            ));
            bal.push(Obs::ratio(
                cluster(r),
                ok.iter().map(|o| f64::from(o.balance.score)).sum(),
                n,
            ));
        }
        crit.push(Obs::mean(
            cluster(r),
            ok.iter().filter(|o| o.critical).count() as f64,
        ));
    }
    s.faithfulness = est(&faith, b);
    s.balance = est(&bal, b);
    s.critical_per_presentation = est(&crit, b);
    Some(s)
}

fn system_order(results: &[CaseResult], manifest: &Manifest) -> Vec<String> {
    let mut order: Vec<String> = manifest.systems.iter().map(|s| s.label.clone()).collect();
    for r in results {
        let l = r.system_label();
        if !order.contains(&l) {
            order.push(l);
        }
    }
    order
}

pub fn summarize(
    manifest: &Manifest,
    results: &[CaseResult],
    changed: &[String],
    b: Bootstrap,
) -> Summary {
    let mut systems = Vec::new();
    for label in system_order(results, manifest) {
        let rs: Vec<&CaseResult> = results
            .iter()
            .filter(|r| r.system_label() == label)
            .collect();
        let Some(first) = rs.first() else { continue };
        let mut by_cat: BTreeMap<String, Vec<&CaseResult>> = BTreeMap::new();
        for r in &rs {
            by_cat
                .entry(r.category.as_str().to_owned())
                .or_default()
                .push(r);
        }
        let mut kinds: BTreeMap<String, usize> = ViolationKind::ALL
            .iter()
            .map(|k| (k.as_str().to_owned(), 0))
            .collect();
        for r in rs.iter().filter(|r| !infra_failure(r)) {
            for v in &r.visible.as_ref().expect("scored").violations {
                *kinds.entry(v.kind.as_str().to_owned()).or_default() += 1;
            }
        }
        let mut call_ms: Vec<f64> = rs
            .iter()
            .flat_map(|r| r.calls.iter().filter(|c| c.error.is_none()))
            .map(|c| c.latency_ms as f64 / 1000.0)
            .collect();
        call_ms.sort_by(f64::total_cmp);
        systems.push(SystemSummary {
            label: label.clone(),
            kind: first.system_kind,
            writer_model: first.writer_model.clone(),
            presentations: rs.iter().filter(|r| !infra_failure(r)).count(),
            infra_failures: rs.iter().filter(|r| infra_failure(r)).count(),
            metrics: metrics(&rs, b),
            by_category: by_cat
                .iter()
                .map(|(k, v)| (k.clone(), metrics(v, b)))
                .collect(),
            violations_by_kind: kinds,
            cost_usd_total: rs.iter().filter_map(|r| r.cost_usd).sum(),
            input_tokens: rs.iter().map(|r| r.input_tokens).sum(),
            output_tokens: rs.iter().map(|r| r.output_tokens).sum(),
            calls: rs.iter().map(|r| r.calls.len()).sum(),
            call_errors: rs
                .iter()
                .map(|r| r.calls.iter().filter(|c| c.error.is_some()).count())
                .sum(),
            call_latency_p50_s: stats::percentile(&call_ms, 50.0),
            judge: judge_summary(&rs, b),
        });
    }

    // Pipeline minus baseline for each model, on the cases both scored.
    let mut paired = Vec::new();
    let models: BTreeSet<&str> = systems.iter().map(|s| s.writer_model.as_str()).collect();
    for m in models {
        let pick = |kind: SystemKind| -> Vec<&CaseResult> {
            results
                .iter()
                .filter(|r| r.writer_model == m && r.system_kind == kind && !infra_failure(r))
                .collect()
        };
        let (p, base) = (pick(SystemKind::Pipeline), pick(SystemKind::Baseline));
        if p.is_empty() || base.is_empty() {
            continue;
        }
        let both: BTreeSet<&str> = p
            .iter()
            .map(|r| r.case_id.as_str())
            .filter(|id| base.iter().any(|r| r.case_id == *id))
            .collect();
        let obs = |rs: &[&CaseResult], f: &dyn Fn(&CaseResult) -> Option<(f64, f64)>| {
            rs.iter()
                .filter(|r| both.contains(r.case_id.as_str()))
                .filter_map(|r| f(r).map(|(n, d)| Obs::ratio(cluster(r), n, d)))
                .collect::<Vec<_>>()
        };
        let viol = |r: &CaseResult| {
            r.visible
                .as_ref()
                .map(|v| (v.violation_count() as f64, 1.0))
        };
        let viol_excl = |r: &CaseResult| r.visible.as_ref().map(|v| (excl_note(v) as f64, 1.0));
        let withheld = |r: &CaseResult| {
            r.visible
                .as_ref()
                .filter(|v| v.sections_requested > 0)
                .map(|v| (v.sections_withheld as f64, v.sections_requested as f64))
        };
        let cost = |r: &CaseResult| Some((r.cost_usd.unwrap_or(0.0), 1.0));
        paired.push(Paired {
            model: m.to_owned(),
            violations_per_presentation: stats::paired_difference(
                &obs(&p, &viol),
                &obs(&base, &viol),
                b.resamples,
                b.seed,
            ),
            violations_excl_note_per_presentation: stats::paired_difference(
                &obs(&p, &viol_excl),
                &obs(&base, &viol_excl),
                b.resamples,
                b.seed,
            ),
            withheld_section_rate: stats::paired_difference(
                &obs(&p, &withheld),
                &obs(&base, &withheld),
                b.resamples,
                b.seed,
            ),
            cost_usd_per_presentation: stats::paired_difference(
                &obs(&p, &cost),
                &obs(&base, &cost),
                b.resamples,
                b.seed,
            ),
        });
    }

    Summary {
        run_id: manifest.run_id.clone(),
        generated_at: crate::meta::rfc3339(std::time::SystemTime::now()),
        scorer_version: score::SCORER_VERSION.into(),
        visible_scorer_version: visible::VISIBLE_SCORER_VERSION.into(),
        bootstrap: b,
        systems,
        paired,
        changed_cases: changed.to_vec(),
        manifest: manifest.clone(),
    }
}

fn pct(e: &Option<Estimate>) -> String {
    e.as_ref()
        .map_or("n/a".into(), |e| format!("{} (n={})", e.fmt_pct(), e.n))
}

fn per100(e: &Option<Estimate>) -> String {
    e.as_ref()
        .map_or("n/a".into(), |e| format!("{} (n={})", e.fmt(100.0, 1), e.n))
}

fn usd(e: &Option<Estimate>) -> String {
    e.as_ref()
        .map_or("n/a".into(), |e| format!("${}", e.fmt(1.0, 4)))
}

fn secs(v: Option<f64>) -> String {
    v.map_or("n/a".into(), |s| format!("{s:.1} s"))
}

fn shell_quote(a: &str) -> String {
    if a.chars()
        .all(|c| c.is_ascii_alphanumeric() || "-_./:,=@".contains(c))
    {
        a.to_owned()
    } else {
        format!("'{}'", a.replace('\'', "'\\''"))
    }
}

pub fn render(summary: &Summary, results: &[CaseResult]) -> String {
    let m = &summary.manifest;
    let b = summary.bootstrap;
    let mut md = String::new();
    let _ = writeln!(md, "# ClarityEval run {}\n", m.run_id);
    let _ = writeln!(
        md,
        "Split: **{}**{}. Deterministic, patient-visible scores unless a table says otherwise. \
         Intervals are 95% cluster-bootstrap percentile intervals over base plans \
         ({} resamples, seed {}). n is the number of presentations (or the unit named in the \
         column). Cells with n below 10 are descriptive and support no ranking; systems whose \
         intervals overlap are not distinguishable.\n",
        m.split.as_str(),
        if m.final_run { ", final run" } else { "" },
        b.resamples,
        b.seed
    );
    if m.dry_run {
        let _ = writeln!(
            md,
            "**Dry run.** Every reply came from the offline scripted model; no figure here describes a real model.\n"
        );
    }

    // Headline.
    let _ = writeln!(md, "## Headline\n");
    for s in &summary.systems {
        let v = &s.metrics.violations_per_presentation;
        let _ = writeln!(
            md,
            "- `{}`: {} unsafe or unsupported statements shown to patients per 100 presentations; \
             {} excluding unconfirmed note content; {} per presentation; {} of requested sections withheld for staff review (n={} presentations).",
            s.label,
            v.as_ref().map_or("n/a".into(), |e| e.fmt(100.0, 1)),
            s.metrics
                .violations_excl_note_per_presentation
                .as_ref()
                .map_or("n/a".into(), |e| e.fmt(100.0, 1)),
            usd(&s.metrics.cost_usd_per_presentation),
            s.metrics
                .withheld_section_rate
                .as_ref()
                .map_or("n/a".into(), |e| e.fmt_pct()),
            s.presentations
        );
    }
    for p in &summary.paired {
        let _ = writeln!(
            md,
            "- `{}`, pipeline minus plain-prompt baseline on the same cases: {} statements per 100 presentations \
             ({} excluding unconfirmed note content), {} percentage points of sections withheld, ${} per presentation (paired bootstrap, {} plans).",
            p.model,
            p.violations_per_presentation.fmt(100.0, 1),
            p.violations_excl_note_per_presentation.fmt(100.0, 1),
            p.withheld_section_rate.fmt(100.0, 0),
            p.cost_usd_per_presentation.fmt(1.0, 4),
            p.violations_per_presentation.clusters
        );
    }

    // Comparison table.
    let _ = writeln!(md, "\n## Comparison\n");
    let _ = writeln!(
        md,
        "| System | n | Visible violations per 100 | Excluding unconfirmed note, per 100 | Zero-violation presentations | Citation validity (cites) | Unsupported codes held (procedures) | Identifier leak (presentations) | Withheld sections (sections) | Schema-valid sections | $ per presentation | Latency p50 / p90 |\n|---|---|---|---|---|---|---|---|---|---|---|---|"
    );
    for s in &summary.systems {
        let x = &s.metrics;
        let _ = writeln!(
            md,
            "| `{}` | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} / {} |",
            s.label,
            s.presentations,
            per100(&x.violations_per_presentation),
            per100(&x.violations_excl_note_per_presentation),
            pct(&x.zero_violation_rate),
            pct(&x.citation_validity),
            pct(&x.unsupported_held_rate),
            pct(&x.identifier_leak_rate),
            pct(&x.withheld_section_rate),
            pct(&x.schema_valid_rate),
            usd(&x.cost_usd_per_presentation),
            secs(x.latency_p50_s),
            secs(x.latency_p90_s)
        );
    }
    let _ = writeln!(
        md,
        "\nVisible violations count only what a patient would read: pipeline sections that passed \
         validation and review, and everything the baseline returned. Held or failed pipeline \
         sections are withheld (the withheld column is the price of that safety). In the n columns \
         of ratio metrics, n is the number of presentations that contribute; the ratio pools the \
         named unit. Identifier leak is over presentations that plant identifiers.\n"
    );

    // Violations by kind.
    let _ = writeln!(md, "### Visible violations by kind (total count)\n");
    let _ = write!(md, "| Kind |");
    for s in &summary.systems {
        let _ = write!(md, " `{}` |", s.label);
    }
    let _ = write!(md, "\n|---|");
    for _ in &summary.systems {
        let _ = write!(md, "---|");
    }
    md.push('\n');
    for k in ViolationKind::ALL {
        let _ = write!(md, "| {} |", k.as_str());
        for s in &summary.systems {
            let _ = write!(
                md,
                " {} |",
                s.violations_by_kind.get(k.as_str()).copied().unwrap_or(0)
            );
        }
        md.push('\n');
    }

    // Per category.
    let _ = writeln!(md, "\n## By category\n");
    let _ = writeln!(
        md,
        "| Category | System | n | Visible violations per 100 | Zero-violation presentations | Citation validity | Withheld sections | $ per presentation | Latency p50 |\n|---|---|---|---|---|---|---|---|---|"
    );
    let cats: BTreeSet<&String> = summary
        .systems
        .iter()
        .flat_map(|s| s.by_category.keys())
        .collect();
    for c in cats {
        for s in &summary.systems {
            let Some(x) = s.by_category.get(c) else {
                continue;
            };
            let _ = writeln!(
                md,
                "| {c} | `{}` | {} | {} | {} | {} | {} | {} | {} |",
                s.label,
                x.violations_per_presentation.map_or(0, |e| e.n),
                x.violations_per_presentation
                    .as_ref()
                    .map_or("n/a".into(), |e| e.fmt(100.0, 1)),
                x.zero_violation_rate
                    .as_ref()
                    .map_or("n/a".into(), |e| e.fmt_pct()),
                x.citation_validity
                    .as_ref()
                    .map_or("n/a".into(), |e| e.fmt_pct()),
                x.withheld_section_rate
                    .as_ref()
                    .map_or("n/a".into(), |e| e.fmt_pct()),
                usd(&x.cost_usd_per_presentation),
                secs(x.latency_p50_s)
            );
        }
    }

    // Cost and latency.
    let _ = writeln!(md, "\n## Cost and latency\n");
    let _ = writeln!(
        md,
        "| System | Presentations | Infrastructure failures | Total cost | Tokens in / out | Calls (errors) | Call latency p50 | Presentation p50 / p90 |\n|---|---|---|---|---|---|---|---|"
    );
    for s in &summary.systems {
        let _ = writeln!(
            md,
            "| `{}` | {} | {} | ${:.4} | {} / {} | {} ({}) | {} | {} / {} |",
            s.label,
            s.presentations,
            s.infra_failures,
            s.cost_usd_total,
            s.input_tokens,
            s.output_tokens,
            s.calls,
            s.call_errors,
            secs(s.call_latency_p50_s),
            secs(s.metrics.latency_p50_s),
            secs(s.metrics.latency_p90_s)
        );
    }
    let _ = writeln!(
        md,
        "\nCost is what the provider reported per call (OpenRouter `usage.cost`), summed per \
         presentation; judge calls are excluded. Latency is client wall clock per presentation \
         at concurrency {}.",
        m.concurrency.max(1)
    );

    // Judge.
    if summary.systems.iter().any(|s| s.judge.is_some()) {
        let _ = writeln!(
            md,
            "\n## Automated judge, not clinician review\n\nThese scores come from an LLM judge \
             (`{}`, prompt `{}`), one call per patient-visible section, on the anchored 1 to 5 \
             scales of docs/CLARITYEVAL.md section 7. They are automated ratings, not clinician \
             validation, and are not yet calibrated against dentists.\n",
            m.judge.as_ref().map_or("?", |j| j.model.as_str()),
            m.judge.as_ref().map_or("?", |j| j.prompt_version.as_str())
        );
        let _ = writeln!(
            md,
            "| System | Sections judged | Judge failures | Faithfulness (1 to 5) | Balance (1 to 5) | Critical flags per 100 presentations | Judge cost |\n|---|---|---|---|---|---|---|"
        );
        for s in &summary.systems {
            let Some(j) = &s.judge else { continue };
            let _ = writeln!(
                md,
                "| `{}` | {} | {} | {} | {} | {} | ${:.4} |",
                s.label,
                j.sections_judged,
                j.judge_failures,
                j.faithfulness
                    .as_ref()
                    .map_or("n/a".into(), |e| e.fmt(1.0, 2)),
                j.balance.as_ref().map_or("n/a".into(), |e| e.fmt(1.0, 2)),
                j.critical_per_presentation
                    .as_ref()
                    .map_or("n/a".into(), |e| e.fmt(100.0, 1)),
                j.cost_usd
            );
        }
    }

    // Per-case diagnostics.
    let _ = writeln!(md, "\n## Presentations\n");
    let _ = writeln!(
        md,
        "| Case | System | Visible violations | Sections passed / held / failed | Withheld | Repairs | Latency | Cost |\n|---|---|---|---|---|---|---|---|"
    );
    let mut sorted: Vec<&CaseResult> = results.iter().collect();
    sorted.sort_by(|a, b| (&a.case_id, a.system_label()).cmp(&(&b.case_id, b.system_label())));
    for r in &sorted {
        let count = |s: State| r.sections.iter().filter(|x| x.state == s).count();
        let v = r.visible.clone().unwrap_or_default();
        let _ = writeln!(
            md,
            "| {} | `{}` | {} | {} / {} / {} | {} | {} | {} | {} |",
            r.case_id,
            r.system_label(),
            if infra_failure(r) {
                "infrastructure failure".into()
            } else {
                v.violation_count().to_string()
            },
            count(State::Passed),
            count(State::NeedsReview),
            count(State::Failed),
            v.sections_withheld,
            r.sections
                .iter()
                .map(|s| s.attempts.saturating_sub(1))
                .sum::<u32>(),
            secs(Some(r.elapsed_ms as f64 / 1000.0)),
            r.cost_usd.map_or("n/a".into(), |c| format!("${c:.4}"))
        );
    }

    // Violation details.
    let details: Vec<String> = sorted
        .iter()
        .flat_map(|r| {
            let label = r.system_label();
            let mut out: Vec<String> = Vec::new();
            if let Some(e) = &r.error {
                out.push(format!("- {} `{label}`: run error: {e}", r.case_id));
            }
            if let Some(v) = &r.visible {
                out.extend(v.violations.iter().map(|x| {
                    format!(
                        "- {} `{label}` {}{} {}: {}",
                        r.case_id,
                        x.section,
                        x.path,
                        x.kind.as_str(),
                        x.detail.replace('|', "/")
                    )
                }));
            }
            out
        })
        .collect();
    if !details.is_empty() {
        let _ = writeln!(md, "\n## Visible violations and errors\n");
        for d in details.iter().take(200) {
            let _ = writeln!(md, "{d}");
        }
        if details.len() > 200 {
            let _ = writeln!(md, "- (+{} more in results.json)", details.len() - 200);
        }
    }

    // Configuration.
    let _ = writeln!(md, "\n## Configuration\n");
    let _ = writeln!(md, "| Field | Value |\n|---|---|");
    let row = |md: &mut String, k: &str, v: String| {
        let _ = writeln!(md, "| {k} | {} |", v.replace('|', "/"));
    };
    for s in &m.systems {
        row(
            &mut md,
            &format!("System `{}`", s.label),
            format!(
                "writer `{}`, reviewer {}, temperature {}, reasoning {}, max_tokens {}, max repairs {}, seed {}, timeout {} s, prompts {}",
                s.writer_model,
                s.reviewer_model
                    .as_ref()
                    .map_or("none (no review pass)".into(), |r| format!("`{r}`")),
                s.params.temperature,
                reasoning_str(s.params.reasoning),
                s.params.max_tokens,
                s.params.max_repairs,
                s.params.seed.map_or("not sent".into(), |x| x.to_string()),
                s.params.timeout_s.map_or("?".into(), |t| t.to_string()),
                s.prompt_versions.join(", ")
            ),
        );
        for (model, routing) in &s.routing {
            row(
                &mut md,
                &format!("Routing for `{model}`"),
                format!("`{routing}`"),
            );
        }
    }
    if let Some(j) = &m.judge {
        row(
            &mut md,
            "Judge",
            format!(
                "`{}`, temperature {}, reasoning {}, seed {}, prompt {}, routing `{}`",
                j.model,
                j.temperature,
                reasoning_str(j.reasoning),
                j.seed.map_or("not sent".into(), |x| x.to_string()),
                j.prompt_version,
                j.routing
            ),
        );
    }
    if m.format < 2 {
        row(
            &mut md,
            "Writer / reviewer (format 1 run)",
            format!("`{}` / `{}`", m.writer_model, m.reviewer_model),
        );
    }
    for (model, note) in &m.seed_notes {
        row(&mut md, &format!("Seed for `{model}`"), note.clone());
    }
    for (model, e) in &m.endpoints {
        row(
            &mut md,
            &format!("Endpoints for `{model}` at {}", e.fetched_at),
            match &e.error {
                Some(err) => format!("unavailable: {err}"),
                None => format!(
                    "{} listed, {} meet the routing filters with reasoning",
                    e.endpoints.len(),
                    e.eligible(true)
                ),
            },
        );
    }
    row(
        &mut md,
        "Model ids returned",
        if m.models_returned.is_empty() {
            "none".into()
        } else {
            m.models_returned
                .iter()
                .map(|x| format!("`{x}`"))
                .collect::<Vec<_>>()
                .join(", ")
        },
    );
    row(
        &mut md,
        "Provider routes seen",
        if m.provider_routes.is_empty() {
            "none reported".into()
        } else {
            m.provider_routes.join(", ")
        },
    );
    row(&mut md, "Route", m.route.clone());
    row(&mut md, "Split", m.split.as_str().into());
    row(
        &mut md,
        "Cases directory",
        m.cases_dir.clone().unwrap_or_else(|| "default".into()),
    );
    row(
        &mut md,
        "Case counts",
        format!(
            "{} cases: {}",
            m.cases.len(),
            m.case_counts
                .iter()
                .map(|(k, v)| format!("{k} {v}"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    );
    if !m.case_filter.is_empty() {
        row(&mut md, "Case filter", m.case_filter.join(", "));
    }
    row(&mut md, "Dataset sha256", format!("`{}`", m.dataset_sha256));
    row(
        &mut md,
        "Held-out commitment",
        match &m.commitment {
            Some(c) => format!(
                "{} root `{}` (file sha256 `{}`); cases {}{}",
                c.path,
                c.root.as_deref().unwrap_or("not found in file"),
                c.file_sha256,
                match c.verified {
                    Some(true) => "verified against the commitment",
                    Some(false) => "**do not match the commitment**",
                    None => "not verified",
                },
                if c.verification.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", c.verification.join("; "))
                }
            ),
            None if m.split == crate::case::Split::Heldout => {
                "**missing: no commitment was published before this run**".into()
            }
            None => "none (not needed for dev)".into(),
        },
    );
    row(
        &mut md,
        "Git commit",
        match (&m.git_commit, m.git_dirty) {
            (Some(c), Some(true)) => format!("`{c}` (uncommitted changes)"),
            (Some(c), _) => format!("`{c}`"),
            _ => "unknown".into(),
        },
    );
    row(
        &mut md,
        "Scorer versions",
        format!(
            "run {} / visible {}; report {} / visible {}",
            m.scorer_version,
            if m.visible_scorer_version.is_empty() {
                "n/a"
            } else {
                &m.visible_scorer_version
            },
            summary.scorer_version,
            summary.visible_scorer_version
        ),
    );
    row(
        &mut md,
        "Seeds",
        format!(
            "generation {} ({}), bootstrap {} with {} resamples",
            m.seed.map_or("none".into(), |s| s.to_string()),
            m.seed_mode
                .map_or("n/a".into(), |s| format!("{s:?}").to_lowercase()),
            b.seed,
            b.resamples
        ),
    );
    row(&mut md, "Concurrency", m.concurrency.max(1).to_string());
    row(
        &mut md,
        "Spend",
        format!(
            "systems ${:.4}, judge ${:.4}, cap ${:.2}",
            m.total_cost_usd, m.judge_cost_usd, m.max_cost_usd
        ),
    );
    row(&mut md, "rustc", m.rustc.clone().unwrap_or_default());
    row(&mut md, "Started", m.started_at.clone());
    row(
        &mut md,
        "Finished",
        m.finished_at
            .clone()
            .unwrap_or_else(|| "did not finish".into()),
    );
    if let Some(s) = &m.stopped_early {
        row(&mut md, "Stopped early", s.clone());
    }
    if let Some(c) = &m.config_file {
        row(
            &mut md,
            "Config file",
            format!(
                "{c} (sha256 `{}`)",
                m.config_sha256.as_deref().unwrap_or("?")
            ),
        );
    }
    if !summary.changed_cases.is_empty() {
        row(
            &mut md,
            "Case files changed since the run",
            summary.changed_cases.join(", "),
        );
    }

    // How to rerun.
    let _ = writeln!(md, "\n## How to rerun\n");
    let _ = writeln!(
        md,
        "Re-scoring is offline and exact. Re-running the models repeats the procedure; outputs \
         differ byte for byte because providers are not deterministic, which is why the returned \
         model ids, provider routes and dates above are recorded.\n"
    );
    md.push_str("```sh\n");
    if let Some(c) = &m.git_commit {
        let _ = writeln!(md, "git checkout {c}");
    }
    let _ = writeln!(
        md,
        "# Re-score this run offline and rewrite report.md and results.json\ncargo run -p clarityeval -- report {}",
        shell_quote(&format!("eval/runs/{}", m.run_id))
    );
    if !m.command.is_empty() {
        let args: Vec<String> = m
            .command
            .iter()
            .filter(|a| !a.is_empty())
            .map(|a| shell_quote(a))
            .collect();
        let _ = writeln!(
            md,
            "# Repeat the run (needs OPENROUTER_API_KEY; costs money unless --dry-run)\ncargo run -p clarityeval -- {}",
            args.join(" ")
        );
    }
    md.push_str("```\n");
    md
}

/// Writes report.md and results.json into the run directory.
pub fn write(dir: &Path, summary: &Summary, results: &[CaseResult]) -> Result<String> {
    let md = render(summary, results);
    std::fs::write(dir.join("report.md"), &md)?;
    #[derive(Serialize)]
    struct Out<'a> {
        #[serde(flatten)]
        summary: &'a Summary,
        presentations: &'a [CaseResult],
    }
    std::fs::write(
        dir.join("results.json"),
        serde_json::to_string_pretty(&Out {
            summary,
            presentations: results,
        })?,
    )?;
    Ok(md)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::case::{Category, Split, TaskKind};
    use crate::runner::{CallRecord, Params};
    use crate::score::SectionRecord;

    fn result(id: &str, kind: SystemKind, fixture: &str, ms: u64, cost: f64) -> CaseResult {
        let sections: Vec<SectionRecord> = serde_json::from_str(fixture).unwrap();
        CaseResult {
            run_id: "r".into(),
            system: crate::systems::SystemSpec::label(kind, "test/model"),
            system_kind: kind,
            case_id: id.into(),
            case_sha256: "x".into(),
            plan_ref: String::new(),
            split: Split::Dev,
            category: Category::Grounding,
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
            cost_usd: Some(cost),
            cost_source: "provider".into(),
            calls: vec![CallRecord {
                role: "writer".into(),
                model_requested: "test/model".into(),
                model_returned: Some("test/model-0101".into()),
                provider_route: Some("prov".into()),
                temperature: Some(0.2),
                reasoning: None,
                latency_ms: ms,
                input_tokens: 100,
                output_tokens: 50,
                cost_usd: Some(cost),
                prompt_sha256: "h".into(),
                error: None,
            }],
            sections,
            metrics: vec![],
            visible: None,
            judgments: vec![],
            passed: false,
            critical_failures: 0,
            error: None,
        }
    }

    fn manifest() -> Manifest {
        serde_json::from_value(serde_json::json!({
            "run_id": "r", "started_at": "2026-11-25T00:00:00Z", "finished_at": null,
            "route": "openrouter", "models_returned": [], "scorer_version": score::SCORER_VERSION,
            "split": "dev", "case_filter": [], "dataset_sha256": "d", "cases": [],
            "git_commit": null, "git_dirty": null, "rustc": null, "max_cost_usd": 1.0,
            "total_cost_usd": 0.0, "stopped_early": null
        }))
        .unwrap()
    }

    #[test]
    fn rescored_report_compares_systems() {
        let kb = KnowledgeBase::load(&crate::repo_root().join("knowledge")).unwrap();
        let cases: Vec<LoadedCase> =
            crate::case::load_dir(&crate::case::split_dir(&crate::repo_root(), Split::Dev))
                .unwrap()
                .into_iter()
                .map(Result::unwrap)
                .collect();
        let clean = include_str!("../tests/fixtures/G-002_clean.json");
        let bad = include_str!("../tests/fixtures/G-002_bad.json");
        let mut results = vec![
            result("G-002", SystemKind::Pipeline, clean, 4000, 0.002),
            result("G-002", SystemKind::Baseline, bad, 9000, 0.001),
        ];
        let changed = rescore(&mut results, &cases, &kb);
        assert_eq!(changed, vec!["G-002"]);
        let base_v = results[1].visible.as_ref().unwrap().violation_count();
        assert_eq!(results[0].visible.as_ref().unwrap().violation_count(), 0);
        assert!(base_v > 0);
        let b = Bootstrap {
            resamples: 200,
            seed: 1,
        };
        let summary = summarize(&manifest(), &results, &changed, b);
        assert_eq!(summary.systems.len(), 2);
        assert_eq!(summary.paired.len(), 1);
        assert_eq!(
            summary.paired[0].violations_per_presentation.value,
            Some(-(base_v as f64))
        );
        let md = render(&summary, &results);
        assert!(md.contains("## Comparison"), "{md}");
        assert!(
            md.contains("`pipeline:test/model` | 1 | 0.0 [0.0, 0.0] (n=1)"),
            "{md}"
        );
        assert!(md.contains("per 100 presentations"));
        assert!(md.contains("## How to rerun"));
        assert!(md.contains("## Visible violations and errors"));
        assert!(!md.contains("Automated judge"));
    }
}
