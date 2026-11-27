# ClarityEval results: final held-out run

This page summarizes the final run on the ClarityEval held-out split. The full report, with every table, interval and presentation, is `eval/runs/20261126T230607Z-final-heldout-v1-291b679/report.md`. Method and metric definitions are in [CLARITYEVAL.md](CLARITYEVAL.md).

## What was run

- **Cases.** The 36 held-out cases (12 treatment plans, 12 grounding traps, 12 safety traps) on 36 synthetic plans. No case was seen during prompt work. The runner checked every case file against the salted SHA-256 commitment published before the run (root `b4c88e53fb39df1fbdbccfb314125d51b9c4fe9ec527a24ae557aa043b182695`), and all 36 matched.
- **Systems.** 13 models, each run two ways: the DentalClarity pipeline, and a single-prompt baseline that carries the same content rules, facts and note. That is 26 systems and 936 presentations.
- **Code.** The run started at 2026-11-26T23:06Z from commit `291b679` with a clean tree. The rerun pass described below ran from commit `fe7504d`, also clean. The commits between them touch only the ClarityEval runner and report and one error message in the OpenRouter adapter; prompts, validators, knowledge and scoring are unchanged.
- **Judge.** Claude Opus 5.5, one call per patient-visible section.
- **Spend.** $5.36 billed by OpenRouter for the systems. The Claude systems and the judge ran on a subscription; their list-price estimates ($44.95 for the systems, $112.89 for the judge) are reported but were not charged.

## Headline

DeepSeek v4.1 Flash is the app's default model. Through the pipeline, its presentations showed patients **0.0** sentences with an unsafe or unsupported statement per 100 presentations (95% interval 0.0 to 8.3). The same model with the baseline prompt showed **36.1** (16.7 to 61.1). The paired difference on the same 36 cases is -36.1 (-61.1 to -16.7). The pipeline cost $0.0063 per presentation against $0.0005 for the baseline, and withheld 9% of requested sections.

Across all 13 models:

- The pipeline showed no violating sentence for 10 models. The other three had at most two such sentences each in 36 presentations: GLM 5.3 Flash 5.6 per 100, MiMo v2.6 Flash 5.6, and Claude Opus 5.5 2.8.
- The baselines ranged from 2.8 per 100 (Tencent hy4 Flash) to 105.6 (MiniMax m3.1 Flash).
- For 10 of the 13 models, the paired interval excludes zero. For GPT-6 Luna Pro, Tencent hy4 Flash and Claude Opus 5.5 the baselines were already low, and the interval reaches zero.
- On the violation kinds the pipeline's validators do not check (injected instructions followed, forbidden phrases, leaked identifiers, invented reasons), the pipeline showed 0.0 for 10 models and at most 5.6. The baselines ranged from 0.0 to 41.7.
- The judge rated pipeline sections more faithful to the plan than baseline sections for every model.

The pipeline held back between 4% and 31% of requested sections, depending on the model, while the baseline shows everything it writes. GPT-OSS 120b (31%) and Ripple 65b (27%) were the most often held back.

## Per model

Violating sentences are per 100 presentations, with 95% intervals; n = 36 presentations per system. "Unchecked kinds" counts only the violation kinds the pipeline's validators do not run. Faithfulness is the judge's mean on a 1 to 5 scale.

| Model | Pipeline | Baseline | Paired difference | Unchecked kinds, pipeline / baseline | Not shown, pipeline | $ per presentation, pipeline / baseline | Faithfulness, pipeline / baseline |
|---|---|---|---|---|---|---|---|
| DeepSeek v4.1 Flash | 0.0 [0.0, 8.3] | 36.1 [16.7, 61.1] | -36.1 [-61.1, -16.7] | 0.0 / 16.7 | 9% | 0.0063 / 0.0005 | 4.22 / 3.97 |
| GPT-OSS 120b | 0.0 [0.0, 8.3] | 25.0 [5.6, 52.8] | -25.0 [-52.8, -5.6] | 0.0 / 16.7 | 31% | 0.0131 / 0.0025 | 3.87 / 3.16 |
| GLM 5.3 | 0.0 [0.0, 8.3] | 27.8 [11.1, 47.2] | -27.8 [-47.2, -11.1] | 0.0 / 5.6 | 7% | 0.0183 / 0.0021 | 4.21 / 3.77 |
| GLM 5.3 Flash | 5.6 [0.0, 16.7] | 44.4 [11.1, 97.2] | -38.9 [-91.7, -2.8] | 5.6 / 19.4 | 5% | 0.0061 / 0.0001 | 4.01 / 3.73 |
| GPT-6 Luna Pro | 0.0 [0.0, 8.3] | 5.6 [0.0, 13.9] | -5.6 [-13.9, 0.0] | 0.0 / 0.0 | 16% | 0.0151 / 0.0041 | 4.32 / 4.04 |
| GPT-6 Luna | 0.0 [0.0, 8.3] | 11.1 [2.8, 22.2] | -11.1 [-22.2, -2.8] | 0.0 / 2.8 | 11% | 0.0047 / 0.0009 | 4.22 / 4.09 |
| Tencent hy4 Flash | 0.0 [0.0, 8.3] | 2.8 [0.0, 8.3] | -2.8 [-8.3, 0.0] | 0.0 / 2.8 | 4% | 0.0124 / 0.0026 | 4.14 / 3.51 |
| MiniMax m3.1 Flash | 0.0 [0.0, 8.3] | 105.6 [61.1, 158.3] | -105.6 [-158.3, -61.1] | 0.0 / 30.6 | 15% | 0.0052 / 0.0009 | 3.94 / 3.18 |
| MiMo v2.6 Flash | 5.6 [0.0, 13.9] | 69.4 [38.9, 102.8] | -63.9 [-97.2, -33.3] | 5.6 / 41.7 | 10% | 0.0067 / 0.0006 | 4.05 / 3.80 |
| Ripple 65b preview | 0.0 [0.0, 8.3] | 66.7 [27.8, 113.9] | -66.7 [-113.9, -27.8] | 0.0 / 27.8 | 27% | 0.0243 / 0.0030 | 4.17 / 3.62 |
| Claude Opus 5.5 | 2.8 [0.0, 8.3] | 5.6 [0.0, 13.9] | -2.8 [-8.3, 0.0] | 2.8 / 2.8 | 7% | 0.7033 / 0.0840 | 4.56 / 4.10 |
| Claude Sonnet 5.5 | 0.0 [0.0, 8.3] | 19.4 [5.6, 36.1] | -19.4 [-36.1, -5.6] | 0.0 / 5.6 | 5% | 0.3903 / 0.0454 | 4.29 / 4.12 |
| Claude Haiku 5.5 | 0.0 [0.0, 8.3] | 19.4 [5.6, 38.9] | -19.4 [-38.9, -5.6] | 0.0 / 11.1 | 18% | 0.0525 / 0.0050 | 4.41 / 4.07 |

The five pipeline sentences that reached a patient were forbidden phrases (GLM 5.3 Flash, MiMo v2.6 Flash), one leaked identifier (MiMo v2.6 Flash) and one invented reason (Claude Opus 5.5). None of these kinds is checked by a pipeline validator.

When notes are counted, the baselines show patients 75 to 289 sentences per 100 presentations drawn from clinical notes that no staff member confirmed. The pipeline holds that content until staff confirm it, so it shows none. This figure is reported separately because it measures a design choice the baseline does not have.

## What the judge found

The judge flagged at least one critical problem in 2.8 to 13.9 pipeline presentations per 100, depending on the model, against 5.6 to 80.6 for the baselines. For GPT-6 Luna and Claude Opus 5.5 the pipeline was flagged slightly more often than its baseline, and for Claude Sonnet 5.5 equally often; those intervals overlap widely. These flags have not been reviewed by a dentist, and the judge is not calibrated against clinicians.

## How to read these numbers

- **Circularity.** Several violation kinds (unsupported tooth or code, invented cost or timing, guarantee, prognosis, pressure, medication, a documented reason without a note) are detected with the same checks the pipeline runs before it shows a section. The pipeline scores zero on those by construction. The unchecked-kinds column and the judge are the fairer comparisons.
- **Small samples.** Each system has 36 presentations. A rate of 0.0 has an upper bound of 8.3 per 100 (rule of three), and systems whose intervals overlap are not distinguishable.
- **Temperature.** GPT-6 and the Claude models ran at the provider's default temperature, because their endpoints do not accept the parameter. All other models ran at the configured temperature.
- **Judge family.** The judge is Claude Opus 5.5, which shares a model family with three of the evaluated systems.
- **Claude route.** The Claude systems ran through the Claude Code CLI on a subscription. The CLI adds about 900 tokens of client context to every call, which cannot be turned off, and reports list-price estimates rather than charges.
- **Cost basis.** Batch and flex models were billed at their discounted rate, live models at the live rate. Costs are comparable within a route, not across routes.
- **Latency.** Latency is reported only for models on the live route (Tencent hy4 Flash, MiniMax m3.1 Flash, MiMo v2.6 Flash, Ripple 65b). For batch, flex and subscription models, time spent in a queue would swamp the model's own time.

## Reruns and infrastructure failures

The final report has no infrastructure failures. Getting there took one rerun pass, under the rule recorded in [DECISIONS.md](DECISIONS.md) E21.

- The first pass left 64 presentations with a transport error: rate limits, timeouts or empty replies, mostly from Ripple 65b and GPT-OSS 120b. `clarityeval retry` reran exactly those presentations with the run's own settings and judge. Both attempts are kept in `results.jsonl`, and the report scores the latest one.
- A reply that stopped at the output limit before writing any answer (GPT-OSS 120b spent its whole budget on hidden reasoning in some calls) counts as the model's own failure, not a transport error. Those presentations were scored as unusable output, not rerun until they passed.
- The batch provider served DeepSeek v4.1 Flash, GLM 5.3 and GLM 5.3 Flash slowly. GLM 5.3 Flash's last batch had returned 48 of 83 requests after almost 10 hours. The run process was stopped at 2026-11-27T10:06Z, after 893 of 936 presentations, and `retry --stopped` ran the remaining 43 on the live route for those three models. The model weights are the same, but the provider route differs from the rest of their presentations. The report's "Reruns" section names the moved models.
- The rerun pass reran 107 presentations, and all 107 were scored. It cost $1.83.

## Verifying the run

The held-out cases, their salts and the run directory are not in the repository yet. Once they are published, `eval/commitments/heldout-v1.txt` lists the steps to recompute every per-case hash and the root with only a shell and `sha256sum`. To re-score the stored run offline:

```sh
cargo run -p clarityeval -- report eval/runs/20261126T230607Z-final-heldout-v1-291b679
```

Re-scoring is exact. Re-running the models repeats the procedure, but provider outputs vary from run to run.
