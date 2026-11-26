# AI for Medicine submission

Draft of the written description for the AI for Medicine hackathon. Figures marked `TBD` are filled in from the final held-out ClarityEval run ([RESULTS.md](RESULTS.md)) and are not estimates.

- Live demo: https://dentalclarity-demo.fly.dev (no sign-in, synthetic data)
- Code: https://github.com/arminkhaleghirod/DentalClarity
- Video: `TBD`

## Problem

A dental treatment plan is written for the dental team: procedure codes, tooth numbers, phases and fees. Patients are usually asked to accept it after a short chairside conversation. Many leave without knowing what a build-up is, why a crown follows a root canal, or which part of the cost is theirs. Explaining each plan carefully, in the patient's language and at a reading level that suits them, takes time that most practices do not have.

General-purpose chatbots can write a friendly explanation, but they also change facts. In our own baseline runs a plain prompt with the same plan produced explanations that stated reasons the dentist never recorded, and in other cases added tooth numbers, visit counts or prices. In a clinical setting those errors are the main risk, so the system has to be built around preventing them, and the prevention has to be measured.

## What DentalClarity does

DentalClarity reads the plan the dentist already saved in the practice management system and turns it into a presentation the patient can browse on a tablet:

- At a glance: a tooth chart with the planned teeth highlighted, the phases in one paragraph, and the estimated patient share.
- Journey: the phases in treatment order and how the steps depend on each other.
- One screen per procedure: where in the mouth, an illustration, what it is, why it was recommended (only after staff confirm it), what happens during the visit, recovery, and questions to ask.
- Costs: fees, insurance estimates and patient share per phase, built from the plan with no model involved.
- Next steps, and narration for every block of text with the transcript on screen and the current sentence highlighted.

It explains the plan the dentist made. It does not diagnose, recommend treatment, or try to persuade anyone to accept it.

## Method

The pipeline treats the model as a writer of plain-language sentences around facts it cannot change.

1. **Locked facts.** The server takes a read-only snapshot of the plan and builds a FactSheet. Procedures, teeth, surfaces, phases and fees become facts with opaque ids (`p1`, `ph1`). Names, patient numbers, dates, contact details and money never reach a model; they are filled in on the server after validation.
2. **Curated knowledge.** Each procedure code maps deterministically to one of 22 knowledge base entries, written for this project with cited sources. A code without an entry gets no generated text at all.
3. **Sanitized note.** The linked clinical note is stripped of identifiers. If too much has to be removed, it is dropped instead of passed on.
4. **Section writer.** One model call per section returns strict JSON in which every sentence cites the facts, passages or note it relies on.
5. **Deterministic validators.** Seventeen checks run on every section: schema, reference ids, citations, tooth numbers, codes, timing, costs, and wording that would diagnose, promise an outcome, pressure the patient or give medication advice. Any blocking failure sends the section back for repair.
6. **Review pass.** A second model call checks faithfulness, scope, balance, certainty and clarity against the same facts. Only safety findings can hold a section; a reviewer can never override a validator failure. Each section gets at most two repairs and is then held for staff.
7. **Staff confirmation.** A "why" or "objective" taken from the clinical note stays hidden until a staff member confirms it. Until then the patient sees the knowledge base's general wording. Held sections show a neutral note to the patient.

All sections run in parallel. A full presentation takes about 35 to 110 seconds and costs about $0.015 with `deepseek/deepseek-v4.1-flash-1017`.

The app is written in Rust: an Axum server with SQLite, and a Leptos web app compiled to WebAssembly. Demo mode uses OpenRouter with synthetic data. Clinical mode is designed for the practice's own AWS account (Bedrock and Polly) and refuses to construct the OpenRouter client in code.

## Evaluation

ClarityEval measures what a patient would actually read.

- **Data.** 36 held-out cases on 36 synthetic plans that share no patient, plan or procedure mix with the 16 public dev cases. Twelve cases each cover treatment plans, grounding traps (codes without knowledge entries, notes about a different tooth, unplanned treatment) and safety traps (notes that contain instructions to the model, pressure, medication instructions, planted identifiers). Prompts were tuned on dev only.
- **Commitment.** Before the final run we published a salted SHA-256 commitment to every held-out case (root `b4c88e53fb39df1fbdbccfb314125d51b9c4fe9ec527a24ae557aa043b182695`). The runner checks each case file against it. Cases and salts are published after the run so anyone can verify them.
- **Primary metric.** Sentences with an unsafe or unsupported statement shown to patients, per 100 presentations: an unsupported tooth or code, an invented cost or timing, guarantee, prognosis, pressure or medication wording, an invented reason, a followed injected instruction, a forbidden phrase or a leaked identifier. Note-derived content is reported as a second figure, because the pipeline holds it for a staff confirmation the baseline does not have.
- **Baseline.** The same model with one prompt that carries the same content rules as the pipeline's writer (no prices, durations, guarantees, medication names or diagnosis) and the same facts and note, but no knowledge passages, validators, review or repair. This isolates what the pipeline's checks add beyond telling the model the rules.
- **Circularity, stated up front.** Several violation kinds are detected with the same checks the pipeline runs before showing a section, so the pipeline scores zero on those by construction. The report therefore shows the kinds the pipeline does not check (injected instructions followed, forbidden phrases, identifier leaks, invented reasons) on their own line, and the LLM judge rates every visible section independently.
- **Secondary metrics.** Share of requested sections not shown to the patient (the cost of safety), violations per 100 visible sentences, citation validity, schema validity, identifier leakage, US dollars per presentation, and latency p50 and p90.
- **Statistics.** 95% cluster-bootstrap intervals over base plans (10,000 resamples), Wilson intervals for proportions, a rule-of-three upper bound when no events occur, and paired bootstrap differences between pipeline and baseline on the same cases. A presentation with any transport failure is left out of both systems' figures and counted.
- **Judge.** An LLM judge rates faithfulness and balance on anchored 1 to 5 scales. It is reported separately and labelled as automated, not clinician review.

### Results

| System | Violating sentences per 100 presentations | Sections not shown | $ per presentation | Latency p50 |
|---|---|---|---|---|
| Pipeline, `TBD` | `TBD` | `TBD` | `TBD` | `TBD` |
| Baseline, `TBD` | `TBD` | `TBD` | `TBD` | `TBD` |

Full tables, intervals and per-category results: [RESULTS.md](RESULTS.md).

### Reproducing it

```sh
cargo run -p clarityeval -- validate
cargo run -p clarityeval -- run --dry-run --models deepseek/deepseek-v4.1-flash-1017 --baseline --judge any/judge
cargo run -p clarityeval -- run --split dev --models deepseek/deepseek-v4.1-flash-1017 --baseline --max-cost-usd 2
cargo run -p clarityeval -- report eval/runs/<run_id>
```

The dry run needs no key and checks the whole flow offline. Every run records model ids, the upstream provider, seeds, prompt versions, case hashes and the git commit. Re-scoring a stored run is exact; re-running the models repeats the procedure, but provider outputs vary between runs.

## Who benefits, and who could be harmed

**Who benefits.** Patients who leave the chair unsure what they agreed to, especially those who read English as a second language, have lower health literacy, or are anxious in the dental chair and remember little of what was said. Guardians deciding for a child. Dental teams who explain the same procedures many times a day and want a consistent explanation that stays within what the dentist recorded.

**Who could be harmed, and how we limit it.**

- A patient given a wrong fact (tooth, procedure, cost) could agree to treatment they did not understand. Facts are never written by the model, and every sentence is checked against them.
- A patient could read an explanation as a diagnosis or a promise. Validators block diagnostic, prognostic and guarantee wording, and the app always says the dentist is the one to ask.
- A patient could feel pushed toward treatment. Pressure wording is blocked, the costs screen labels everything as an estimate, and no screen asks for acceptance.
- A clinical note may contain identifiers or text that tries to steer the model. Notes are sanitized, note-derived reasons are hidden until staff confirm them, and the safety split tests injected instructions directly.
- Staff could trust the output too much. The editor shows where each block came from and which checks passed, and an automated review is never presented as clinician approval.
- Patients who do not read English are only partly served today; translation is planned and not built.

## Limitations

- The knowledge base entries and illustrations have not yet been reviewed by a dentist. Every entry is marked unreviewed.
- All data is synthetic. There has been no study with patients or practices, so we make no claim about comprehension or outcomes.
- The automated judge is not calibrated against dentists yet, and the held-out set is small (36 cases), so intervals are wide.
- English only for now. Reading levels, translation, patient questions and the take-home booklet are planned.
- The prototype has one shared staff account. Individual accounts, audit attribution and a security assessment are required before any real patient data is used. It is not HIPAA compliant as it stands.
- The Open Dental connector reads a synthetic practice; the live connector is not finished.

## With another month

- Dentist review of all 22 knowledge entries and the illustrations, and a blinded review of 20 to 30 outputs to calibrate the automated judge.
- Translation into Spanish and Vietnamese with back-translation checks, and three reading levels, each measured in ClarityEval.
- A grounded "Ask a question" feature that answers only from the plan and the knowledge base, and refers everything else to the dentist.
- The take-home PDF booklet.
- The Open Dental connector against a real test install, and the clinical AWS path smoke-tested end to end.
- A small usability session with volunteers reading synthetic plans, measuring recall of what each procedure is and what it costs.

## Built with

Rust, Axum, Tokio, SQLx with SQLite, Leptos (WebAssembly), Trunk, OpenRouter (`deepseek/deepseek-v4.1-flash-1017` for text, `heygen/heygen-voice-1-turbo` for narration), Fly.io. Full attribution is in the [README](../README.md#credits-and-attribution).
