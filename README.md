# DentalClarity

Understand your dental care.

DentalClarity turns the treatment plan a dentist has already written into a presentation the patient can follow: which teeth are involved, what each procedure is, what the visits look like, what it is estimated to cost, and what happens next. It reads the plan from the practice management system, explains it in the patient's language at the reading level they choose, narrates it aloud, and produces a booklet to take home.

It explains the plan. It does not diagnose, recommend treatment, or try to persuade anyone to accept it.

> **Status:** early development for the Teens in AI Global Challenge 2027. Not for use with real patient data. See [Clinical use](#clinical-use).

## How it works

- Procedures, teeth, phases and costs come from the practice's records and are never written by a language model.
- Explanations are generated from a curated knowledge base, checked by deterministic validators and a second review pass, and labeled with where each sentence came from.
- Anything that fails a check is held for staff review instead of being shown to the patient.
- Staff hand the tablet to the patient in Present mode, which locks the staff session on the server.

## Repository layout

| Path | Contents |
|---|---|
| `crates/dc-core` | Domain model and API types shared by server and web app |
| `crates/dc-pms` | Practice management system connectors (synthetic now, Open Dental next) |
| `crates/dc-knowledge` | Curated knowledge base and code lookup |
| `crates/dc-ai` | Model providers and the mode policy that decides which may run |
| `crates/dc-generation` | Generation pipeline and validators |
| `crates/dc-server` | Axum server and the `dentalclarity` binary |
| `crates/dc-web` | Leptos web app for staff and patients |
| `fixtures/synthetic` | A fictional practice used by the demo and tests |
| `knowledge` | Procedure code map and knowledge entries |
| `docs` | Product, architecture, AI pipeline, UI, security and evaluation plans |

## Running locally

Requirements: Rust 1.88 or newer, the `wasm32-unknown-unknown` target and [Trunk](https://trunkrs.dev).

```sh
rustup target add wasm32-unknown-unknown
cargo install trunk --locked

# Build the web app, then run the server in demo mode.
cd crates/dc-web && trunk build --release && cd ../..
cargo run -p dc-server
```

Open http://127.0.0.1:8080. Configuration comes from environment variables; see [`.env.example`](.env.example).

For frontend work, run `cargo run -p dc-server` and `trunk serve` in `crates/dc-web`, then open http://127.0.0.1:8081. Trunk rebuilds on save and forwards API calls to the server.

Run the tests with `cargo test`.

## Modes

| | Demo | Clinical |
|---|---|---|
| Data | Synthetic practice only | Open Dental, read-only |
| AI provider | OpenRouter | The practice's own AWS account (Bedrock and Polly) |
| Intended for | The public demo and development | A practice network, once the gaps below are closed |

## Clinical use

DentalClarity is a prototype. Open source and self-hosting do not make software HIPAA compliant, and neither does using a cloud service that is eligible under a business associate agreement. Before any practice uses it with real patient records, at least these gaps must be closed:

- Individual staff accounts and audit attribution. The prototype has one shared staff account.
- A completed security assessment by the practice.
- A business associate agreement and verified configuration for the AWS services used.
- Clinician review of every knowledge base entry and illustration.

The full list is in [docs/SECURITY.md](docs/SECURITY.md).

## Documentation

- [Decision log](docs/DECISIONS.md)
- [Product requirements](docs/PRD.md)
- [Architecture](docs/ARCHITECTURE.md)
- [AI pipeline](docs/AI_PIPELINE.md)
- [UI specification](docs/UI_SPEC.md)
- [Security](docs/SECURITY.md)
- [ClarityEval benchmark](docs/CLARITYEVAL.md)
- [Implementation plan](docs/IMPLEMENTATION.md)

## License

Apache License 2.0. See [LICENSE](LICENSE).

CDT procedure codes are referenced by number only. CDT descriptors are copyrighted by the American Dental Association and are not included in this repository.
