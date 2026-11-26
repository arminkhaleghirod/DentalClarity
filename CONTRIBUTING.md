# Contributing to DentalClarity

Thank you for helping. DentalClarity is an early prototype, so small, focused changes are the easiest to review. For anything larger, open an issue first so we can agree on the approach.

## Build and run

Requirements: Rust 1.88 or newer, the `wasm32-unknown-unknown` target and [Trunk](https://trunkrs.dev).

```sh
rustup target add wasm32-unknown-unknown
cargo install trunk --locked

cd crates/dc-web && trunk build --release && cd ../..
cargo run -p dc-server
```

The server embeds the web app at compile time, so restart it after each `trunk build`. For frontend work, run `trunk serve` in `crates/dc-web` next to the server and open http://127.0.0.1:8081. Configuration is described in [`.env.example`](.env.example).

## Test and lint

CI runs these on every pull request. Run them before you push:

```sh
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test

cargo clippy -p dc-web --target wasm32-unknown-unknown -- -D warnings
cargo test -p dc-web
```

Tests need no network and no API key. ClarityEval runs call paid models and never run in CI.

## Rules for patient data and content

- Never add real patient data, in code, fixtures, tests, issues, logs or screenshots. Use the synthetic practice in `fixtures/synthetic` or write new fictional cases.
- Refer to CDT procedure codes by number only (for example `D2740`) with our own plain-language names. CDT descriptor text is copyrighted by the American Dental Association and must not be committed.
- Knowledge base entries cite their published sources and stay `unreviewed` until a dentist signs off. Do not mark an entry as reviewed yourself.
- Illustrations must be original work. Do not trace or adapt existing drawings.
- Never commit secrets. `.env` is ignored by git; keep it that way.

## Style

- Code: `rustfmt` defaults, no Clippy warnings, and tests for new behavior. Changes to prompts need a new prompt version file.
- Decisions: [docs/DECISIONS.md](docs/DECISIONS.md) is authoritative. If a change alters a decision, update it there in the same pull request.
- Writing in docs, comments and UI text: plain and specific, sentence-case headings, no em dashes, no marketing words. Do not claim more than the evidence supports; the app is not clinically validated.

## Pull requests

Keep each pull request to one change, describe what it does and how you tested it, and fill in the checklist in the template. By contributing you agree that your work is licensed under the Apache License 2.0.
