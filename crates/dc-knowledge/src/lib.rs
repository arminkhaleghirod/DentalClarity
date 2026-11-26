//! The curated knowledge base.
//!
//! Lookup is deterministic: a procedure code maps to one category through
//! `knowledge/codes.toml`, and each category has a reviewed entry. There is no
//! embedding search. Codes that are not mapped return
//! [`ProcedureCategory::Unsupported`] so the pipeline can send them to review.

use dc_core::ProcedureCode;
use dc_core::procedure::ProcedureCategory;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum KnowledgeError {
    #[error("could not read {path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    #[error("invalid knowledge file {path}: {message}")]
    Parse { path: String, message: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CodeInfo {
    pub category: ProcedureCategory,
    /// Our own patient-facing name for the procedure.
    pub name: String,
}

#[derive(Debug, Deserialize)]
struct CodesFile {
    codes: HashMap<String, CodeInfo>,
}

/// Maps procedure codes to categories and patient-facing names.
#[derive(Debug, Clone, Default)]
pub struct CodeMap {
    codes: HashMap<ProcedureCode, CodeInfo>,
}

impl CodeMap {
    pub fn from_toml(source: &str, path_for_errors: &str) -> Result<Self, KnowledgeError> {
        let file: CodesFile = toml::from_str(source).map_err(|e| KnowledgeError::Parse {
            path: path_for_errors.to_owned(),
            message: e.to_string(),
        })?;
        let codes = file
            .codes
            .into_iter()
            .map(|(code, info)| (ProcedureCode::new(code), info))
            .collect();
        Ok(Self { codes })
    }

    pub fn from_path(path: &Path) -> Result<Self, KnowledgeError> {
        let display = path.display().to_string();
        let source = std::fs::read_to_string(path).map_err(|source| KnowledgeError::Io {
            path: display.clone(),
            source,
        })?;
        Self::from_toml(&source, &display)
    }

    pub fn get(&self, code: &ProcedureCode) -> Option<&CodeInfo> {
        self.codes.get(code)
    }

    pub fn category(&self, code: &ProcedureCode) -> ProcedureCategory {
        self.get(code)
            .map_or(ProcedureCategory::Unsupported, |info| info.category)
    }

    pub fn len(&self) -> usize {
        self.codes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.codes.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo_codes() -> CodeMap {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../knowledge/codes.toml");
        CodeMap::from_path(&path).expect("codes.toml parses")
    }

    #[test]
    fn maps_known_codes() {
        let map = repo_codes();
        assert_eq!(
            map.category(&ProcedureCode::new("D3330")),
            ProcedureCategory::RootCanal
        );
        assert_eq!(map.get(&ProcedureCode::new("d2740")).unwrap().name, "Crown");
    }

    #[test]
    fn unknown_codes_are_unsupported() {
        assert_eq!(
            repo_codes().category(&ProcedureCode::new("D9999")),
            ProcedureCategory::Unsupported
        );
    }

    #[test]
    fn every_mapped_code_is_cdt_shaped() {
        let map = repo_codes();
        assert!(map.codes.keys().all(ProcedureCode::is_cdt_shaped));
    }

    #[test]
    fn every_synthetic_fixture_code_is_mapped() {
        let fixture = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/synthetic/practice.toml"),
        )
        .unwrap();
        let map = repo_codes();
        for line in fixture
            .lines()
            .filter(|l| l.trim_start().starts_with("code = "))
        {
            let code = line.split('"').nth(1).unwrap();
            assert!(
                map.get(&ProcedureCode::new(code)).is_some(),
                "fixture code {code} is not mapped"
            );
        }
    }
}
