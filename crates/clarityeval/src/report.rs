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
use crate::score::visible::{self, Group, ViolationKind};
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

/// Presentations `rescore` cannot re-score because their case file is not
/// among `cases`, as `<case id> <system>`. They keep the scores stored at run
/// time, from the scorer version recorded in the manifest.
pub fn not_rescored(results: &[CaseResult], cases: &[LoadedCase]) -> Vec<String> {
    results
        .iter()
        .filter(|r| r.error.is_none())
        .filter(|r| !cases.iter().any(|c| c.case.case_id == r.case_id))
        .map(|r| format!("{} {}", r.case_id, r.system_label()))
        .collect()
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

impl Bootstrap {
    /// The settings a run recorded, else the defaults.
    pub fn from_manifest(m: &Manifest) -> Self {
        let d = Bootstrap::default();
        Bootstrap {
            resamples: m.bootstrap_resamples.unwrap_or(d.resamples),
            seed: m.bootstrap_seed.unwrap_or(d.seed),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Metrics {
    /// Headline: distinct sentences with a counted violation, unconfirmed
    /// note content left out, per presentation.
    pub violating_sentences_per_presentation: Option<Estimate>,
    /// The same with unconfirmed note content counted.
    pub violating_sentences_with_note_per_presentation: Option<Estimate>,
    /// Distinct sentences with a violation of a kind no pipeline validator
    /// checks, per presentation.
    pub independent_sentences_per_presentation: Option<Estimate>,
    /// Headline sentences over visible sentences, pooled.
    pub violating_sentences_per_visible_sentence: Option<Estimate>,
    /// Counted violation instances (a sentence can carry several), note content included.
    pub violations_per_presentation: Option<Estimate>,
    /// Counted violation instances without unconfirmed note content.
    pub violations_excl_note_per_presentation: Option<Estimate>,
    /// Share of presentations with no headline violation.
    pub zero_violation_rate: Option<Estimate>,
    /// Share of visible citations that were ids the system was given.
    pub citation_validity: Option<Estimate>,
    /// Share of unsupported procedures with no patient-visible text.
    pub unsupported_held_rate: Option<Estimate>,
    /// Share of presentations (with planted identifiers) leaking any of them.
    pub identifier_leak_rate: Option<Estimate>,
    /// Share of requested sections held or failed by the system's gate.
    pub withheld_section_rate: Option<Estimate>,
    /// Share of requested sections a patient does not see: withheld plus missing.
    pub not_shown_rate: Option<Estimate>,
    /// Share of written sections that parse as the section schema.
    pub schema_valid_rate: Option<Estimate>,
    /// US dollars per presentation (provider-reported), over presentations with a cost.
    pub cost_usd_per_presentation: Option<Estimate>,
    pub latency_p50_s: Option<f64>,
    pub latency_p90_s: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct JudgeSummary {
    pub sections_judged: usize,
    pub judge_failures: usize,
    /// Presentations with visible text but no readable judgment at all; they
    /// are missing from the judge figures, not counted as clean.
    #[serde(default)]
    pub presentations_unjudged: usize,
    pub faithfulness: Option<Estimate>,
    pub balance: Option<Estimate>,
    /// Sections flagged critical per presentation, over judged presentations.
    pub critical_per_presentation: Option<Estimate>,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SystemSummary {
    pub label: String,
    pub kind: SystemKind,
    pub writer_model: String,
    /// Presentations scored: the system's own infrastructure failures, and
    /// cases where the paired system for the same model had one, excluded.
    pub presentations: usize,
    /// This system's own infrastructure failures.
    pub infra_failures: usize,
    /// Presentations left out only because the paired system failed on that case.
    #[serde(default)]
    pub excluded_with_pair: usize,
    /// Requested sections lost to transport errors, over every presentation.
    #[serde(default)]
    pub sections_transport_failed: usize,
    pub metrics: Metrics,
    pub by_category: BTreeMap<String, Metrics>,
    /// Violation instances by kind (flags included), over scored presentations.
    pub violations_by_kind: BTreeMap<String, usize>,
    /// Violation instances by group.
    #[serde(default)]
    pub violations_by_group: BTreeMap<String, usize>,
    /// Distinct violating sentences by group.
    #[serde(default)]
    pub sentences_by_group: BTreeMap<String, usize>,
    /// Presentations by `cost_source` (provider, provider_partial, none), over scored presentations.
    #[serde(default)]
    pub cost_sources: BTreeMap<String, usize>,
    /// Spend over every presentation, infrastructure failures included.
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
    /// Cases both systems scored.
    #[serde(default)]
    pub cases: usize,
    /// Pipeline minus baseline, per presentation, on the same cases.
    pub violating_sentences_per_presentation: Estimate,
    pub violating_sentences_with_note_per_presentation: Estimate,
    pub independent_sentences_per_presentation: Estimate,
    pub not_shown_rate: Estimate,
    pub withheld_section_rate: Estimate,
    /// Over the cases where both systems reported a cost.
    pub cost_usd_per_presentation: Estimate,
    #[serde(default)]
    pub cost_cases: usize,
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
    /// Presentations whose case file could not be loaded, so their stored
    /// scores were kept (`report --no-rescore`, or a case removed since).
    #[serde(default)]
    pub not_rescored: Vec<String>,
    /// `<model> <case id>` pairs excluded from both systems because one of
    /// them had an infrastructure failure.
    #[serde(default)]
    pub excluded_pairs: Vec<String>,
    /// The run directory, relative to the repository root where possible.
    #[serde(default)]
    pub run_dir: String,
    /// The `knowledge/` tree and validator lexicons when this report was made.
    #[serde(default)]
    pub knowledge_sha256: Option<String>,
    #[serde(default)]
    pub lexicon_sha256: String,
    pub manifest: Manifest,
}

fn cluster(r: &CaseResult) -> &str {
    if r.plan_ref.is_empty() {
        &r.case_id
    } else {
        &r.plan_ref
    }
}

/// Infrastructure failures: a run error, no score, or a transport error in
/// any requested section (the section's text then says nothing about the
/// model). They are counted, not scored, and the paired presentation of the
/// other system on the same case is left out with them.
pub fn infra_failure(r: &CaseResult) -> bool {
    r.error.is_some()
        || r.visible.is_none()
        || r.visible
            .as_ref()
            .is_some_and(|v| v.sections_transport_failed > 0)
        // The pipeline only produces requested sections, so this also covers
        // results stored before the visible score counted transport losses.
        || r.sections.iter().any(|s| s.transport_error())
}

/// `(model, case id)` pairs with an infrastructure failure in any system.
fn excluded(results: &[CaseResult]) -> BTreeSet<(String, String)> {
    results
        .iter()
        .filter(|r| infra_failure(r))
        .map(|r| (r.writer_model.clone(), r.case_id.clone()))
        .collect()
}

/// Numerator and denominator of a per-presentation ratio, if it applies.
type RatioFn = dyn Fn(&CaseResult, &visible::VisibleScore) -> Option<(f64, f64)>;

fn excl_note(v: &visible::VisibleScore) -> usize {
    v.violations
        .iter()
        .filter(|x| x.kind.counted() && x.kind != ViolationKind::UnconfirmedNote)
        .count()
}

fn independent(v: &visible::VisibleScore) -> usize {
    v.sentences_where(|k| k.group() == Group::Independent)
}

fn flag(b: bool) -> f64 {
    f64::from(u8::from(b))
}

/// Metrics over presentations that are already filtered to the scored ones.
pub fn metrics(scored: &[&CaseResult], b: Bootstrap) -> Metrics {
    let rows: Vec<(&CaseResult, &visible::VisibleScore)> = scored
        .iter()
        .filter_map(|r| r.visible.as_ref().map(|v| (*r, v)))
        .collect();
    let obs = |f: &RatioFn| {
        rows.iter()
            .filter_map(|(r, v)| f(r, v).map(|(n, d)| Obs::ratio(cluster(r), n, d)))
            .collect::<Vec<_>>()
    };
    let rate = |f: &RatioFn| {
        let o = obs(f);
        (!o.is_empty()).then(|| stats::count_rate(&o, b.resamples, b.seed))
    };
    let prop = |f: &RatioFn| {
        let o = obs(f);
        (!o.is_empty()).then(|| stats::proportion(&o, b.resamples, b.seed))
    };
    let boot = |f: &RatioFn| {
        let o = obs(f);
        (!o.is_empty()).then(|| stats::cluster_bootstrap(&o, b.resamples, b.seed))
    };
    let mut lat: Vec<f64> = rows
        .iter()
        .map(|(r, _)| r.elapsed_ms as f64 / 1000.0)
        .collect();
    lat.sort_by(f64::total_cmp);
    Metrics {
        violating_sentences_per_presentation: rate(&|_, v| {
            Some((v.violating_sentences() as f64, 1.0))
        }),
        violating_sentences_with_note_per_presentation: rate(&|_, v| {
            Some((v.violating_sentences_with_note() as f64, 1.0))
        }),
        independent_sentences_per_presentation: rate(&|_, v| Some((independent(v) as f64, 1.0))),
        violating_sentences_per_visible_sentence: rate(&|_, v| {
            (v.sentences_visible > 0)
                .then_some((v.violating_sentences() as f64, v.sentences_visible as f64))
        }),
        violations_per_presentation: rate(&|_, v| Some((v.violation_count() as f64, 1.0))),
        violations_excl_note_per_presentation: rate(&|_, v| Some((excl_note(v) as f64, 1.0))),
        zero_violation_rate: prop(&|_, v| Some((flag(v.violating_sentences() == 0), 1.0))),
        citation_validity: prop(&|_, v| {
            (v.cites_total > 0).then_some((v.cites_valid as f64, v.cites_total as f64))
        }),
        unsupported_held_rate: prop(&|_, v| {
            (v.unsupported_total > 0)
                .then_some((v.unsupported_held as f64, v.unsupported_total as f64))
        }),
        identifier_leak_rate: prop(&|_, v| {
            (v.identifiers_checked > 0).then_some((flag(v.identifiers_leaked > 0), 1.0))
        }),
        withheld_section_rate: prop(&|_, v| {
            (v.sections_requested > 0)
                .then_some((v.sections_withheld as f64, v.sections_requested as f64))
        }),
        not_shown_rate: prop(&|_, v| {
            (v.sections_requested > 0)
                .then_some((v.sections_not_shown() as f64, v.sections_requested as f64))
        }),
        schema_valid_rate: prop(&|_, v| {
            (v.sections_written > 0).then_some((
                (v.sections_written - v.schema_failures) as f64,
                v.sections_written as f64,
            ))
        }),
        cost_usd_per_presentation: boot(&|r, _| r.cost_usd.map(|c| (c, 1.0))),
        latency_p50_s: stats::percentile(&lat, 50.0),
        latency_p90_s: stats::percentile(&lat, 90.0),
    }
}

fn judge_summary(
    all: &[&CaseResult],
    scored: &[&CaseResult],
    b: Bootstrap,
) -> Option<JudgeSummary> {
    if all.iter().all(|r| r.judgments.is_empty()) {
        return None;
    }
    let mut faith = Vec::new();
    let mut bal = Vec::new();
    let mut crit = Vec::new();
    let mut s = JudgeSummary::default();
    for r in scored {
        let ok: Vec<&crate::judge::JudgeOut> = r
            .judgments
            .iter()
            .filter_map(|j| j.result.as_ref())
            .collect();
        s.sections_judged += ok.len();
        s.judge_failures += r.judgments.len() - ok.len();
        s.cost_usd += r.judgments.iter().filter_map(|j| j.cost_usd).sum::<f64>();
        let shown = r.visible.as_ref().is_some_and(|v| v.sentences_visible > 0);
        if ok.is_empty() {
            if shown {
                // Text reached the patient but no judgment came back: missing, not clean.
                s.presentations_unjudged += 1;
            } else {
                // Nothing shown, so nothing to flag.
                crit.push(Obs::mean(cluster(r), 0.0));
            }
            continue;
        }
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
        crit.push(Obs::mean(
            cluster(r),
            ok.iter().filter(|o| o.critical).count() as f64,
        ));
    }
    let est = |o: &[Obs]| (!o.is_empty()).then(|| stats::cluster_bootstrap(o, b.resamples, b.seed));
    s.faithfulness = est(&faith);
    s.balance = est(&bal);
    s.critical_per_presentation =
        (!crit.is_empty()).then(|| stats::count_rate(&crit, b.resamples, b.seed));
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
    let skip = excluded(results);
    let is_scored = |r: &CaseResult| !skip.contains(&(r.writer_model.clone(), r.case_id.clone()));
    let mut systems = Vec::new();
    for label in system_order(results, manifest) {
        let rs: Vec<&CaseResult> = results
            .iter()
            .filter(|r| r.system_label() == label)
            .collect();
        let Some(first) = rs.first() else { continue };
        let scored: Vec<&CaseResult> = rs.iter().copied().filter(|r| is_scored(r)).collect();
        let mut by_cat: BTreeMap<String, Vec<&CaseResult>> = BTreeMap::new();
        for r in &scored {
            by_cat
                .entry(r.category.as_str().to_owned())
                .or_default()
                .push(r);
        }
        let mut kinds: BTreeMap<String, usize> = ViolationKind::ALL
            .iter()
            .map(|k| (k.as_str().to_owned(), 0))
            .collect();
        let mut groups: BTreeMap<String, usize> = Group::ALL
            .iter()
            .map(|g| (group_key(*g).to_owned(), 0))
            .collect();
        let mut group_sentences = groups.clone();
        let mut cost_sources: BTreeMap<String, usize> = BTreeMap::new();
        for r in &scored {
            let v = r
                .visible
                .as_ref()
                .expect("scored presentations have a score");
            for x in &v.violations {
                *kinds.entry(x.kind.as_str().to_owned()).or_default() += 1;
                *groups
                    .entry(group_key(x.kind.group()).to_owned())
                    .or_default() += 1;
            }
            for g in Group::ALL {
                let distinct: BTreeSet<(&str, &str)> = v
                    .violations
                    .iter()
                    .filter(|x| x.kind.group() == g)
                    .map(|x| (x.section.as_str(), x.path.as_str()))
                    .collect();
                *group_sentences.entry(group_key(g).to_owned()).or_default() += distinct.len();
            }
            *cost_sources.entry(r.cost_source.clone()).or_default() += 1;
        }
        let mut call_ms: Vec<f64> = scored
            .iter()
            .flat_map(|r| r.calls.iter().filter(|c| c.error.is_none()))
            .map(|c| c.latency_ms as f64 / 1000.0)
            .collect();
        call_ms.sort_by(f64::total_cmp);
        let own = rs.iter().filter(|r| infra_failure(r)).count();
        systems.push(SystemSummary {
            label: label.clone(),
            kind: first.system_kind,
            writer_model: first.writer_model.clone(),
            presentations: scored.len(),
            infra_failures: own,
            excluded_with_pair: rs.len() - scored.len() - own,
            sections_transport_failed: rs
                .iter()
                .map(|r| {
                    r.visible.as_ref().map_or_else(
                        || r.sections.iter().filter(|s| s.transport_error()).count(),
                        |v| v.sections_transport_failed,
                    )
                })
                .sum(),
            metrics: metrics(&scored, b),
            by_category: by_cat
                .iter()
                .map(|(k, v)| (k.clone(), metrics(v, b)))
                .collect(),
            violations_by_kind: kinds,
            violations_by_group: groups,
            sentences_by_group: group_sentences,
            cost_sources,
            cost_usd_total: rs.iter().filter_map(|r| r.cost_usd).sum(),
            input_tokens: rs.iter().map(|r| r.input_tokens).sum(),
            output_tokens: rs.iter().map(|r| r.output_tokens).sum(),
            calls: rs.iter().map(|r| r.calls.len()).sum(),
            call_errors: rs
                .iter()
                .map(|r| r.calls.iter().filter(|c| c.error.is_some()).count())
                .sum(),
            call_latency_p50_s: stats::percentile(&call_ms, 50.0),
            judge: judge_summary(&rs, &scored, b),
        });
    }

    // Pipeline minus baseline for each model, on the cases both scored.
    let mut paired = Vec::new();
    let models: BTreeSet<&str> = systems.iter().map(|s| s.writer_model.as_str()).collect();
    for m in models {
        let pick = |kind: SystemKind| -> Vec<&CaseResult> {
            results
                .iter()
                .filter(|r| r.writer_model == m && r.system_kind == kind && is_scored(r))
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
        let costed: BTreeSet<&str> = both
            .iter()
            .copied()
            .filter(|id| {
                p.iter()
                    .chain(&base)
                    .filter(|r| r.case_id == *id)
                    .all(|r| r.cost_usd.is_some())
            })
            .collect();
        type PairFn = dyn Fn(&CaseResult) -> Option<(f64, f64)>;
        let obs = |rs: &[&CaseResult], ids: &BTreeSet<&str>, f: &PairFn| {
            rs.iter()
                .filter(|r| ids.contains(r.case_id.as_str()))
                .filter_map(|r| f(r).map(|(n, d)| Obs::ratio(cluster(r), n, d)))
                .collect::<Vec<_>>()
        };
        let diff = |ids: &BTreeSet<&str>, f: &PairFn| {
            stats::paired_difference(&obs(&p, ids, f), &obs(&base, ids, f), b.resamples, b.seed)
        };
        let per = |g: fn(&visible::VisibleScore) -> usize| {
            move |r: &CaseResult| r.visible.as_ref().map(|v| (g(v) as f64, 1.0))
        };
        paired.push(Paired {
            model: m.to_owned(),
            cases: both.len(),
            violating_sentences_per_presentation: diff(
                &both,
                &per(visible::VisibleScore::violating_sentences),
            ),
            violating_sentences_with_note_per_presentation: diff(
                &both,
                &per(visible::VisibleScore::violating_sentences_with_note),
            ),
            independent_sentences_per_presentation: diff(&both, &per(independent)),
            not_shown_rate: diff(&both, &|r: &CaseResult| {
                r.visible
                    .as_ref()
                    .filter(|v| v.sections_requested > 0)
                    .map(|v| (v.sections_not_shown() as f64, v.sections_requested as f64))
            }),
            withheld_section_rate: diff(&both, &|r: &CaseResult| {
                r.visible
                    .as_ref()
                    .filter(|v| v.sections_requested > 0)
                    .map(|v| (v.sections_withheld as f64, v.sections_requested as f64))
            }),
            cost_usd_per_presentation: diff(&costed, &|r: &CaseResult| {
                r.cost_usd.map(|c| (c, 1.0))
            }),
            cost_cases: costed.len(),
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
        not_rescored: Vec::new(),
        excluded_pairs: skip.iter().map(|(m, c)| format!("{m} {c}")).collect(),
        run_dir: format!("eval/runs/{}", manifest.run_id),
        knowledge_sha256: None,
        lexicon_sha256: crate::meta::lexicon_sha256(),
        manifest: manifest.clone(),
    }
}

/// Fills in what `summarize` cannot know: where the run lives, which
/// presentations kept stored scores, and the current `knowledge/` hash.
pub fn finish(summary: &mut Summary, root: &Path, run_dir: &Path, not_rescored: Vec<String>) {
    summary.run_dir = run_dir
        .strip_prefix(root)
        .unwrap_or(run_dir)
        .to_string_lossy()
        .replace('\\', "/");
    summary.not_rescored = not_rescored;
    summary.knowledge_sha256 = crate::meta::tree_sha256(&root.join("knowledge"));
}

fn group_key(g: Group) -> &'static str {
    match g {
        Group::Validator => "validator",
        Group::Independent => "independent",
        Group::NoteHold => "note_hold",
        Group::Flag => "flag",
    }
}

/// Things a reader must know before trusting the figures.
pub fn warnings(summary: &Summary) -> Vec<String> {
    let m = &summary.manifest;
    let mut out = Vec::new();
    match (&m.knowledge_sha256, &summary.knowledge_sha256) {
        (Some(a), Some(b)) if a != b => out.push(format!(
            "knowledge/ changed since the run (run `{}`, now `{}`); re-scored citations and \
             unsupported-code checks use today's knowledge base",
            short(a),
            short(b)
        )),
        (None, _) => out.push("the run did not record a knowledge/ hash".into()),
        _ => {}
    }
    match &m.lexicon_sha256 {
        Some(a) if *a != summary.lexicon_sha256 => out.push(format!(
            "the validator lexicons (dc-generation validate.rs) changed since the run (run `{}`, \
             now `{}`); the pipeline was gated by the old lexicons and is scored with the new ones",
            short(a),
            short(&summary.lexicon_sha256)
        )),
        None => out.push("the run did not record a lexicon hash".into()),
        _ => {}
    }
    if !summary.not_rescored.is_empty() {
        out.push(format!(
            "{} presentations were not re-scored and keep their stored scores (scorer {} / \
             visible {}): {}",
            summary.not_rescored.len(),
            m.scorer_version,
            if m.visible_scorer_version.is_empty() {
                "n/a"
            } else {
                &m.visible_scorer_version
            },
            summary.not_rescored.join(", ")
        ));
    }
    if !summary.changed_cases.is_empty() {
        out.push(format!(
            "case files changed since the run: {}",
            summary.changed_cases.join(", ")
        ));
    }
    if m.git_dirty == Some(true) {
        out.push("the run started from a tree with uncommitted changes".into());
    }
    if let Some(c) = &m.commitment {
        if c.verified == Some(false) {
            out.push("the held-out cases do not match the commitment".into());
        }
        if c.salts_present == Some(false) {
            out.push("the salts file was missing, so the commitment was not checked".into());
        }
        if c.git_tracked == Some(false) || c.git_clean == Some(false) {
            out.push("the commitment file was not committed or was modified".into());
        }
    } else if m.split == crate::case::Split::Heldout {
        out.push("no held-out commitment was published before this run".into());
    }
    if let Some(s) = &m.stopped_early {
        out.push(format!("stopped early: {s}"));
    }
    out
}

fn short(h: &str) -> &str {
    &h[..h.len().min(12)]
}

/// `†` after cells with n below 10, which support no ranking.
fn mark(e: &Estimate) -> &'static str {
    if e.small() { " †" } else { "" }
}

fn pct(e: &Option<Estimate>) -> String {
    e.as_ref().map_or("n/a".into(), |e| {
        format!("{} (n={}){}", e.fmt_pct(), e.n, mark(e))
    })
}

fn per100(e: &Option<Estimate>) -> String {
    e.as_ref().map_or("n/a".into(), |e| {
        format!("{} (n={}){}", e.fmt(100.0, 1), e.n, mark(e))
    })
}

fn per100_bare(e: &Option<Estimate>) -> String {
    e.as_ref()
        .map_or("n/a".into(), |e| format!("{}{}", e.fmt(100.0, 1), mark(e)))
}

fn pct_bare(e: &Option<Estimate>) -> String {
    e.as_ref()
        .map_or("n/a".into(), |e| format!("{}{}", e.fmt_pct(), mark(e)))
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
         Rates (per 100 presentations or sentences) carry 95% cluster-bootstrap percentile \
         intervals over base plans ({} resamples, seed {}); a rate with no events gets the \
         rule-of-three upper bound 3/n instead. Proportions carry Wilson intervals, widened to \
         the cluster-bootstrap interval where that is wider. n is the number of presentations \
         (or the unit named in the column). † marks cells with n below 10: they are descriptive \
         and support no ranking. Systems whose intervals overlap are not distinguishable.\n",
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
    let warn = warnings(summary);
    if !warn.is_empty() {
        let _ = writeln!(md, "## Warnings\n");
        for w in &warn {
            let _ = writeln!(md, "- {w}");
        }
        md.push('\n');
    }

    // Headline.
    let _ = writeln!(md, "## Headline\n");
    for s in &summary.systems {
        let x = &s.metrics;
        let _ = writeln!(
            md,
            "- `{}`: {} sentences with an unsafe or unsupported statement shown to patients per \
             100 presentations (n={}{}). Counting unconfirmed note content too: {}. {} of \
             requested sections not shown to the patient; {} per presentation.",
            s.label,
            x.violating_sentences_per_presentation
                .as_ref()
                .map_or("n/a".into(), |e| e.fmt(100.0, 1)),
            s.presentations,
            if s.presentations < stats::SMALL_N {
                ", descriptive only"
            } else {
                ""
            },
            x.violating_sentences_with_note_per_presentation
                .as_ref()
                .map_or("n/a".into(), |e| e.fmt(100.0, 1)),
            x.not_shown_rate
                .as_ref()
                .map_or("n/a".into(), |e| e.fmt_pct()),
            usd(&x.cost_usd_per_presentation),
        );
    }
    let _ = writeln!(
        md,
        "\nThe pipeline's note-derived reasons wait for a staff confirmation that the baseline \
         lacks, so the second figure counts every sentence the baseline draws from a note.\n"
    );
    let _ = writeln!(
        md,
        "- Checks the pipeline's validators do not run (invented reason, injection followed, \
         forbidden phrase, identifier leak), sentences per 100 presentations: {}.",
        summary
            .systems
            .iter()
            .map(|s| format!(
                "`{}` {}",
                s.label,
                per100_bare(&s.metrics.independent_sentences_per_presentation)
            ))
            .collect::<Vec<_>>()
            .join("; ")
    );
    for p in &summary.paired {
        let _ = writeln!(
            md,
            "- `{}`, pipeline minus baseline on the same {} cases: {} sentences per 100 \
             presentations ({} with unconfirmed note content, {} on the checks the validators do \
             not run), {} percentage points of sections not shown, ${} per presentation over the \
             {} cases where both reported a cost (paired bootstrap, {} plans).",
            p.model,
            p.cases,
            p.violating_sentences_per_presentation.fmt(100.0, 1),
            p.violating_sentences_with_note_per_presentation
                .fmt(100.0, 1),
            p.independent_sentences_per_presentation.fmt(100.0, 1),
            p.not_shown_rate.fmt(100.0, 0),
            p.cost_usd_per_presentation.fmt(1.0, 4),
            p.cost_cases,
            p.violating_sentences_per_presentation.clusters
        );
    }
    let failures: usize = summary.systems.iter().map(|s| s.infra_failures).sum();
    let _ = writeln!(
        md,
        "- Infrastructure failures: {failures} presentations (a run error or a transport error in \
         any requested section). For each, the other system's presentation of the same case is \
         left out too, so {} model and case pairs are excluded from every figure above and below.",
        summary.excluded_pairs.len()
    );

    let _ = writeln!(
        md,
        "\n### Threats to validity\n\nThe visible scorer reuses `dc-generation`'s lexicons and its \
         tooth, code, cost and timing checks. A pipeline section that fails them is repaired or \
         withheld, so the pipeline scores near zero on those kinds by construction. The baseline \
         prompt (`{}`) states the same content rules, but nothing checks its output. The kinds no \
         validator checks are the fairer comparison, and their total has its own line above and \
         in the breakdown below. Diagnosis wording is a flag for the judge, not a counted \
         violation (docs/CLARITYEVAL.md section 6).",
        m.baseline_variant.as_deref().unwrap_or("not run")
    );

    // Comparison table.
    let _ = writeln!(md, "\n## Comparison\n");
    let _ = writeln!(
        md,
        "| System | n | Violating sentences per 100 | With unconfirmed note, per 100 | Not checked by validators, per 100 | Per 100 visible sentences | Zero-violation presentations | Citation validity (cites) | Unsupported codes held (procedures) | Identifier leak (presentations) | Withheld (sections) | Not shown (sections) | Schema-valid sections | $ per presentation | Latency p50 / p90 |\n|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|"
    );
    for s in &summary.systems {
        let x = &s.metrics;
        let _ = writeln!(
            md,
            "| `{}` | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} / {} |",
            s.label,
            s.presentations,
            per100(&x.violating_sentences_per_presentation),
            per100(&x.violating_sentences_with_note_per_presentation),
            per100(&x.independent_sentences_per_presentation),
            per100(&x.violating_sentences_per_visible_sentence),
            pct(&x.zero_violation_rate),
            pct(&x.citation_validity),
            pct(&x.unsupported_held_rate),
            pct(&x.identifier_leak_rate),
            pct(&x.withheld_section_rate),
            pct(&x.not_shown_rate),
            pct(&x.schema_valid_rate),
            usd(&x.cost_usd_per_presentation),
            secs(x.latency_p50_s),
            secs(x.latency_p90_s)
        );
    }
    let _ = writeln!(
        md,
        "\nVisible violations count only what a patient would read: pipeline sections that passed \
         validation and review, and everything the baseline returned. The unit is the distinct \
         sentence: a sentence with two problems counts once here and twice in the breakdown by \
         kind. Held or failed pipeline sections are withheld; \"not shown\" adds requested \
         sections the system never produced, and is the price of the pipeline's safety. Sections \
         lost to transport errors are infrastructure failures and are counted in the cost and \
         latency table instead. In the n columns of ratio metrics, n is the number of \
         presentations that contribute; the ratio pools the named unit. Identifier leak is over \
         presentations that plant identifiers.\n"
    );

    // Violations by kind.
    let _ = writeln!(md, "### Visible violations by kind (instances)\n");
    let _ = write!(md, "| Group | Kind |");
    for s in &summary.systems {
        let _ = write!(md, " `{}` |", s.label);
    }
    let _ = write!(md, "\n|---|---|");
    for _ in &summary.systems {
        let _ = write!(md, "---|");
    }
    md.push('\n');
    for g in Group::ALL {
        for k in ViolationKind::ALL.iter().filter(|k| k.group() == g) {
            let _ = write!(md, "| {} | {} |", g.label(), k.as_str());
            for s in &summary.systems {
                let _ = write!(
                    md,
                    " {} |",
                    s.violations_by_kind.get(k.as_str()).copied().unwrap_or(0)
                );
            }
            md.push('\n');
        }
        let _ = write!(md, "| {} | **total (distinct sentences)** |", g.label());
        for s in &summary.systems {
            let _ = write!(
                md,
                " **{}** ({}) |",
                s.violations_by_group
                    .get(group_key(g))
                    .copied()
                    .unwrap_or(0),
                s.sentences_by_group.get(group_key(g)).copied().unwrap_or(0)
            );
        }
        md.push('\n');
    }
    let _ = writeln!(
        md,
        "\nNot checked by the pipeline's validators, total: {}.\n",
        summary
            .systems
            .iter()
            .map(|s| format!(
                "`{}` {} instances in {} distinct sentences",
                s.label,
                s.violations_by_group
                    .get(group_key(Group::Independent))
                    .copied()
                    .unwrap_or(0),
                s.sentences_by_group
                    .get(group_key(Group::Independent))
                    .copied()
                    .unwrap_or(0)
            ))
            .collect::<Vec<_>>()
            .join("; ")
    );
    let _ = writeln!(
        md,
        "The first group repeats checks that block a pipeline section (V4 to V8 and V11 to V14 in \
         `dc-generation`). Diagnosis hits match the app's V10 lexicon and go to the judge; they \
         are listed and never counted."
    );

    // Per category.
    let _ = writeln!(md, "\n## By category\n");
    let _ = writeln!(
        md,
        "| Category | System | n | Violating sentences per 100 | Not checked by validators, per 100 | Zero-violation presentations | Citation validity | Not shown | $ per presentation | Latency p50 |\n|---|---|---|---|---|---|---|---|---|---|"
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
            let n = x.violating_sentences_per_presentation.map_or(0, |e| e.n);
            let _ = writeln!(
                md,
                "| {c} | `{}` | {}{} | {} | {} | {} | {} | {} | {} | {} |",
                s.label,
                n,
                if n < stats::SMALL_N { " †" } else { "" },
                per100_bare(&x.violating_sentences_per_presentation),
                per100_bare(&x.independent_sentences_per_presentation),
                pct_bare(&x.zero_violation_rate),
                pct_bare(&x.citation_validity),
                pct_bare(&x.not_shown_rate),
                usd(&x.cost_usd_per_presentation),
                secs(x.latency_p50_s)
            );
        }
    }

    // Cost and latency.
    let _ = writeln!(md, "\n## Cost and latency\n");
    let _ = writeln!(
        md,
        "| System | Scored | Infrastructure failures | Left out with the pair | Sections lost to transport | Cost reported (full / partial / none) | Total spend | Tokens in / out | Calls (errors) | Call latency p50 | Presentation p50 / p90 |\n|---|---|---|---|---|---|---|---|---|---|---|"
    );
    for s in &summary.systems {
        let src = |k: &str| s.cost_sources.get(k).copied().unwrap_or(0);
        let _ = writeln!(
            md,
            "| `{}` | {} | {} | {} | {} | {} / {} / {} | ${:.4} | {} / {} | {} ({}) | {} | {} / {} |",
            s.label,
            s.presentations,
            s.infra_failures,
            s.excluded_with_pair,
            s.sections_transport_failed,
            src("provider"),
            src("provider_partial"),
            src("none"),
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
         presentation; judge calls are excluded. A `provider_partial` presentation had at least \
         one successful call without a reported cost, so its figure is a lower bound. Per \
         presentation cost and latency cover scored presentations only, so they leave out \
         infrastructure failures; total spend and tokens cover every presentation. Latency is \
         client wall clock per presentation at concurrency {}.",
        m.concurrency.max(1)
    );

    // Judge.
    if summary.systems.iter().any(|s| s.judge.is_some()) {
        let _ = writeln!(
            md,
            "\n## Automated judge, not clinician review\n\nThese scores come from an LLM judge \
             (`{}`, prompt `{}`), one call per patient-visible section, on the anchored 1 to 5 \
             scales of docs/CLARITYEVAL.md section 7. They are automated ratings, not clinician \
             validation, and are not yet calibrated against dentists. A presentation with visible \
             text whose judge calls all failed is missing from these figures, not counted as \
             clean.\n",
            m.judge.as_ref().map_or("?", |j| j.model.as_str()),
            m.judge.as_ref().map_or("?", |j| j.prompt_version.as_str())
        );
        let _ = writeln!(
            md,
            "| System | Sections judged | Judge failures | Presentations without a judgment | Faithfulness (1 to 5) | Balance (1 to 5) | Critical flags per 100 presentations | Judge cost |\n|---|---|---|---|---|---|---|---|"
        );
        for s in &summary.systems {
            let Some(j) = &s.judge else { continue };
            let _ = writeln!(
                md,
                "| `{}` | {} | {} | {} | {} | {} | {} | ${:.4} |",
                s.label,
                j.sections_judged,
                j.judge_failures,
                j.presentations_unjudged,
                j.faithfulness
                    .as_ref()
                    .map_or("n/a".into(), |e| e.fmt(1.0, 2)),
                j.balance.as_ref().map_or("n/a".into(), |e| e.fmt(1.0, 2)),
                per100(&j.critical_per_presentation),
                j.cost_usd
            );
        }
    }

    // Per-case diagnostics.
    let _ = writeln!(md, "\n## Presentations\n");
    let _ = writeln!(
        md,
        "| Case | System | Violating sentences (instances) | Sections passed / held / failed | Not shown | Repairs | Latency | Cost |\n|---|---|---|---|---|---|---|---|"
    );
    let skip: BTreeSet<&String> = summary.excluded_pairs.iter().collect();
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
            } else if skip.contains(&format!("{} {}", r.writer_model, r.case_id)) {
                "left out with its pair".into()
            } else {
                format!("{} ({})", v.violating_sentences(), v.violation_count())
            },
            count(State::Passed),
            count(State::NeedsReview),
            count(State::Failed),
            v.sections_not_shown(),
            r.sections
                .iter()
                .map(|s| s.attempts.saturating_sub(1))
                .sum::<u32>(),
            secs(Some(r.elapsed_ms as f64 / 1000.0)),
            r.cost_usd.map_or("n/a".into(), |c| {
                format!(
                    "${c:.4}{}",
                    if r.cost_source == "provider_partial" {
                        " (partial)"
                    } else {
                        ""
                    }
                )
            })
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
                        "- {} `{label}` {}{} {}{}: {}",
                        r.case_id,
                        x.section,
                        x.path,
                        x.kind.as_str(),
                        if x.kind.counted() { "" } else { " (flag)" },
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
    if let Some(v) = &m.baseline_variant {
        row(&mut md, "Baseline prompt variant", v.clone());
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
        "knowledge/ sha256",
        format!(
            "run `{}`, report `{}`",
            m.knowledge_sha256.as_deref().unwrap_or("not recorded"),
            summary
                .knowledge_sha256
                .as_deref()
                .unwrap_or("not computed")
        ),
    );
    row(
        &mut md,
        "Validator lexicons sha256 (dc-generation validate.rs)",
        format!(
            "run `{}`, report `{}`",
            m.lexicon_sha256.as_deref().unwrap_or("not recorded"),
            summary.lexicon_sha256
        ),
    );
    row(
        &mut md,
        "Held-out commitment",
        match &m.commitment {
            Some(c) => format!(
                "{} root `{}` (file sha256 `{}`, {}); cases {}{}",
                c.path,
                c.root.as_deref().unwrap_or("not found in file"),
                c.file_sha256,
                match (c.git_tracked, c.git_clean) {
                    (Some(true), Some(true)) => "committed and unmodified",
                    (Some(true), _) => "**modified since it was committed**",
                    (Some(false), _) => "**not committed**",
                    (None, _) => "git state not checked",
                },
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
    if !summary.not_rescored.is_empty() {
        row(
            &mut md,
            "Presentations not re-scored",
            summary.not_rescored.join(", "),
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
        "# Re-score this run offline and rewrite report.md and results.json\n\
         cargo run -p clarityeval -- report {} --bootstrap {} --bootstrap-seed {}",
        shell_quote(&summary.run_dir),
        b.resamples,
        b.seed
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
        let base_sentences = results[1].visible.as_ref().unwrap().violating_sentences();
        assert!(base_sentences <= base_v);
        assert_eq!(
            summary.paired[0].violating_sentences_per_presentation.value,
            Some(-(base_sentences as f64))
        );
        let md = render(&summary, &results);
        assert!(md.contains("## Comparison"), "{md}");
        // A zero count gets the rule-of-three bound, and n=1 is marked.
        assert!(
            md.contains("`pipeline:test/model` | 1 | 0.0 [0.0, 300.0] (n=1) †"),
            "{md}"
        );
        assert!(md.contains("per 100 presentations"));
        assert!(md.contains("## How to rerun"));
        assert!(md.contains("## Visible violations and errors"));
        assert!(!md.contains("Automated judge"));
    }

    fn transport_failed(mut r: CaseResult) -> CaseResult {
        r.sections[0].reasons = vec!["The writing model could not be reached (timeout).".into()];
        r
    }

    #[test]
    fn a_partial_outage_excludes_the_pair() {
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
            transport_failed(result("G-002", SystemKind::Pipeline, clean, 4000, 0.002)),
            result("G-002", SystemKind::Baseline, bad, 9000, 0.001),
        ];
        rescore(&mut results, &cases, &kb);
        assert!(infra_failure(&results[0]));
        assert!(!infra_failure(&results[1]));
        let b = Bootstrap {
            resamples: 100,
            seed: 1,
        };
        let summary = summarize(&manifest(), &results, &[], b);
        assert_eq!(summary.excluded_pairs, vec!["test/model G-002"]);
        let (p, base) = (&summary.systems[0], &summary.systems[1]);
        assert_eq!((p.presentations, p.infra_failures), (0, 1));
        assert_eq!(
            (
                base.presentations,
                base.infra_failures,
                base.excluded_with_pair
            ),
            (0, 0, 1)
        );
        assert!(base.metrics.violating_sentences_per_presentation.is_none());
        assert!(summary.paired.is_empty());
        assert!(p.sections_transport_failed >= 1);
    }

    #[test]
    fn paired_cost_skips_cases_without_a_reported_cost() {
        let kb = KnowledgeBase::load(&crate::repo_root().join("knowledge")).unwrap();
        let cases: Vec<LoadedCase> =
            crate::case::load_dir(&crate::case::split_dir(&crate::repo_root(), Split::Dev))
                .unwrap()
                .into_iter()
                .map(Result::unwrap)
                .collect();
        let clean = include_str!("../tests/fixtures/G-002_clean.json");
        let mut base = result("G-002", SystemKind::Baseline, clean, 9000, 0.001);
        base.cost_usd = None;
        base.cost_source = "none".into();
        let mut results = vec![
            result("G-002", SystemKind::Pipeline, clean, 4000, 0.002),
            base,
        ];
        rescore(&mut results, &cases, &kb);
        let summary = summarize(&manifest(), &results, &[], Bootstrap::default());
        let p = &summary.paired[0];
        assert_eq!((p.cases, p.cost_cases), (1, 0));
        assert!(p.cost_usd_per_presentation.value.is_none());
        assert_eq!(summary.systems[1].cost_sources.get("none"), Some(&1));
    }

    #[test]
    fn failed_judgments_are_missing_not_clean() {
        let kb = KnowledgeBase::load(&crate::repo_root().join("knowledge")).unwrap();
        let cases: Vec<LoadedCase> =
            crate::case::load_dir(&crate::case::split_dir(&crate::repo_root(), Split::Dev))
                .unwrap()
                .into_iter()
                .map(Result::unwrap)
                .collect();
        let bad = include_str!("../tests/fixtures/G-002_bad.json");
        let mut r = result("G-002", SystemKind::Baseline, bad, 9000, 0.001);
        r.judgments = vec![crate::judge::Judgment {
            section: "procedure:p3".into(),
            judge_model: "j".into(),
            model_returned: None,
            provider_route: None,
            prompt_version: "judge.section@v1".into(),
            result: None,
            error: Some("timeout".into()),
            latency_ms: 0,
            cost_usd: None,
        }];
        let mut results = vec![r];
        rescore(&mut results, &cases, &kb);
        let summary = summarize(&manifest(), &results, &[], Bootstrap::default());
        let j = summary.systems[0].judge.as_ref().unwrap();
        assert_eq!((j.presentations_unjudged, j.judge_failures), (1, 1));
        assert!(j.critical_per_presentation.is_none());
    }

    #[test]
    fn report_lists_what_it_could_not_rescore() {
        let clean = include_str!("../tests/fixtures/G-002_clean.json");
        let results = vec![result("G-002", SystemKind::Pipeline, clean, 4000, 0.002)];
        assert_eq!(
            not_rescored(&results, &[]),
            vec!["G-002 pipeline:test/model"]
        );
        let mut m = manifest();
        m.knowledge_sha256 = Some("a".repeat(64));
        m.lexicon_sha256 = Some("b".repeat(64));
        let mut summary = summarize(&m, &results, &[], Bootstrap::default());
        summary.not_rescored = not_rescored(&results, &[]);
        summary.knowledge_sha256 = Some("c".repeat(64));
        let w = warnings(&summary);
        assert!(w.iter().any(|x| x.contains("knowledge/ changed")), "{w:?}");
        assert!(w.iter().any(|x| x.contains("lexicons")), "{w:?}");
        assert!(w.iter().any(|x| x.contains("not re-scored")), "{w:?}");
        assert!(render(&summary, &results).contains("## Warnings"));
    }

    #[test]
    fn bootstrap_settings_come_from_the_manifest() {
        let mut m = manifest();
        assert_eq!(Bootstrap::from_manifest(&m), Bootstrap::default());
        m.bootstrap_resamples = Some(123);
        m.bootstrap_seed = Some(9);
        assert_eq!(
            Bootstrap::from_manifest(&m),
            Bootstrap {
                resamples: 123,
                seed: 9
            }
        );
    }
}
