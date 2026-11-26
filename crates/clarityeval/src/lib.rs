//! ClarityEval: the benchmark for DentalClarity's model tasks.
//!
//! The plan is `docs/CLARITYEVAL.md`. This crate holds the case format and
//! its validator ([`case`], [`lint`]), the deterministic scorers ([`score`]),
//! the runner that drives the real `dc-generation` pipeline ([`runner`]) and
//! the report writer ([`report`]). Scorers are pure and unit-tested; only
//! `run` touches the network.

pub mod case;
pub mod lint;
pub mod meta;
pub mod report;
pub mod runner;
pub mod score;

use std::path::{Path, PathBuf};

/// The repository root: the nearest ancestor of the working directory that
/// holds `eval/canary.txt`, else the root this crate was built from.
pub fn repo_root() -> PathBuf {
    if let Ok(cwd) = std::env::current_dir() {
        for dir in cwd.ancestors() {
            if dir.join("eval/canary.txt").is_file() {
                return dir.to_path_buf();
            }
        }
    }
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap_or_else(|_| Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
}

pub fn read_canary(root: &Path) -> anyhow::Result<String> {
    Ok(std::fs::read_to_string(root.join("eval/canary.txt"))?
        .trim()
        .to_owned())
}
