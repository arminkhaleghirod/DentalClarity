//! Knowledge entries: one per procedure category, each with one passage per
//! topic and the sources those passages are based on.

use crate::{CodeMap, KnowledgeError};
use dc_core::ProcedureCode;
use dc_core::procedure::ProcedureCategory;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Topic {
    WhatItIs,
    CommonReasons,
    WhatHappens,
    Recovery,
    Benefits,
    IfDelayed,
    Questions,
}

impl Topic {
    pub const ALL: [Topic; 7] = [
        Topic::WhatItIs,
        Topic::CommonReasons,
        Topic::WhatHappens,
        Topic::Recovery,
        Topic::Benefits,
        Topic::IfDelayed,
        Topic::Questions,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewStatus {
    Unreviewed,
    DentistReviewed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub id: String,
    pub publisher: String,
    pub title: String,
    pub url: String,
    pub accessed: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Passage {
    pub id: String,
    pub topic: Topic,
    pub text: String,
    pub sources: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub id: String,
    pub version: u32,
    pub title: String,
    pub review_status: ReviewStatus,
    #[serde(default)]
    pub reviewed_by: String,
    pub updated: String,
    pub sources: Vec<Source>,
    pub passages: Vec<Passage>,
}

impl Entry {
    pub fn passage(&self, topic: Topic) -> Option<&Passage> {
        self.passages.iter().find(|p| p.topic == topic)
    }

    /// Citation id for a passage, as the pipeline uses it: `kb:<entry>#<passage>`.
    pub fn source_id(&self, passage: &Passage) -> String {
        format!("kb:{}#{}", self.id, passage.id)
    }

    /// Checks the rules in `knowledge/README.md` that can be checked mechanically.
    pub fn validate(&self) -> Result<(), String> {
        let id_ok = |s: &str| {
            !s.is_empty()
                && s.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        };
        if !id_ok(&self.id) {
            return Err(format!("entry id {:?} must match [a-z0-9_]+", self.id));
        }
        if self.version == 0 {
            return Err("version starts at 1".into());
        }
        if self.review_status == ReviewStatus::DentistReviewed && self.reviewed_by.is_empty() {
            return Err("a reviewed entry needs reviewed_by".into());
        }
        let mut source_ids = HashSet::new();
        for source in &self.sources {
            if !id_ok(&source.id) || !source_ids.insert(source.id.as_str()) {
                return Err(format!("source id {:?} is invalid or repeated", source.id));
            }
            if !source.url.starts_with("https://") {
                return Err(format!("source {} must use an https URL", source.id));
            }
        }
        let mut passage_ids = HashSet::new();
        for passage in &self.passages {
            if !id_ok(&passage.id) || !passage_ids.insert(passage.id.as_str()) {
                return Err(format!(
                    "passage id {:?} is invalid or repeated",
                    passage.id
                ));
            }
            if passage.text.trim().is_empty() {
                return Err(format!("passage {} is empty", passage.id));
            }
            if passage.sources.is_empty() {
                return Err(format!("passage {} cites no source", passage.id));
            }
            if let Some(missing) = passage
                .sources
                .iter()
                .find(|s| !source_ids.contains(s.as_str()))
            {
                return Err(format!(
                    "passage {} cites unknown source {missing}",
                    passage.id
                ));
            }
            if passage.text.contains('$') || passage.text.contains("http") {
                return Err(format!("passage {} contains a price or URL", passage.id));
            }
        }
        for topic in Topic::ALL {
            let count = self.passages.iter().filter(|p| p.topic == topic).count();
            if count != 1 {
                return Err(format!(
                    "needs exactly one {topic:?} passage, found {count}"
                ));
            }
        }
        Ok(())
    }
}

/// Code map plus entries, loaded once at startup.
#[derive(Debug, Clone, Default)]
pub struct KnowledgeBase {
    pub codes: CodeMap,
    entries: BTreeMap<String, Entry>,
}

impl KnowledgeBase {
    /// Loads `codes.toml` and every `entries/*.toml` under `dir`, validating each entry.
    pub fn load(dir: &Path) -> Result<Self, KnowledgeError> {
        let codes = CodeMap::from_path(&dir.join("codes.toml"))?;
        let mut entries = BTreeMap::new();
        let entries_dir = dir.join("entries");
        let listing = match std::fs::read_dir(&entries_dir) {
            Ok(listing) => listing,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self { codes, entries });
            }
            Err(source) => {
                return Err(KnowledgeError::Io {
                    path: entries_dir.display().to_string(),
                    source,
                });
            }
        };
        let mut paths: Vec<_> = listing
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "toml"))
            .collect();
        paths.sort();
        for path in paths {
            let display = path.display().to_string();
            let source = std::fs::read_to_string(&path).map_err(|source| KnowledgeError::Io {
                path: display.clone(),
                source,
            })?;
            let entry: Entry = toml::from_str(&source).map_err(|e| KnowledgeError::Parse {
                path: display.clone(),
                message: e.to_string(),
            })?;
            entry.validate().map_err(|message| KnowledgeError::Parse {
                path: display.clone(),
                message,
            })?;
            entries.insert(entry.id.clone(), entry);
        }
        Ok(Self { codes, entries })
    }

    /// Builds a knowledge base from already-loaded parts. Entries are validated.
    pub fn from_parts(codes: CodeMap, entries: Vec<Entry>) -> Result<Self, String> {
        let mut map = BTreeMap::new();
        for entry in entries {
            entry.validate()?;
            map.insert(entry.id.clone(), entry);
        }
        Ok(Self {
            codes,
            entries: map,
        })
    }

    /// The entry for a category, if one has been written.
    pub fn entry(&self, category: ProcedureCategory) -> Option<&Entry> {
        self.entries.get(&category_id(category))
    }

    /// The entry for a procedure code, through the code map.
    pub fn entry_for_code(&self, code: &ProcedureCode) -> Option<&Entry> {
        self.entry(self.codes.category(code))
    }

    pub fn entries(&self) -> impl Iterator<Item = &Entry> {
        self.entries.values()
    }
}

/// The snake_case id serde uses for a category, which is also its entry id.
pub fn category_id(category: ProcedureCategory) -> String {
    serde_json::to_value(category)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Entry {
        toml::from_str(
            r#"
            id = "crown"
            version = 1
            title = "Crown"
            review_status = "unreviewed"
            updated = "2026-11-26"
            [[sources]]
            id = "src"
            publisher = "P"
            title = "T"
            url = "https://example.org/crowns"
            accessed = "2026-11-26"
            [[passages]]
            id = "what_it_is"
            topic = "what_it_is"
            text = "A crown is a cap."
            sources = ["src"]
            [[passages]]
            id = "common_reasons"
            topic = "common_reasons"
            text = "Crowns are often used after a large filling."
            sources = ["src"]
            [[passages]]
            id = "what_happens"
            topic = "what_happens"
            text = "The tooth is shaped."
            sources = ["src"]
            [[passages]]
            id = "recovery"
            topic = "recovery"
            text = "Many people feel some sensitivity."
            sources = ["src"]
            [[passages]]
            id = "benefits"
            topic = "benefits"
            text = "A crown protects the tooth."
            sources = ["src"]
            [[passages]]
            id = "if_delayed"
            topic = "if_delayed"
            text = "A weak tooth may crack."
            sources = ["src"]
            [[passages]]
            id = "questions"
            topic = "questions"
            text = "What material will my crown be made of?"
            sources = ["src"]
            "#,
        )
        .unwrap()
    }

    #[test]
    fn sample_entry_is_valid() {
        let entry = sample();
        entry.validate().unwrap();
        let passage = entry.passage(Topic::Benefits).unwrap();
        assert_eq!(entry.source_id(passage), "kb:crown#benefits");
    }

    #[test]
    fn rejects_missing_topics_and_unknown_sources() {
        let mut entry = sample();
        entry.passages.retain(|p| p.topic != Topic::Recovery);
        assert!(entry.validate().is_err());

        let mut entry = sample();
        entry.passages[0].sources = vec!["nope".into()];
        assert!(entry.validate().is_err());

        let mut entry = sample();
        entry.sources[0].url = "http://example.org".into();
        assert!(entry.validate().is_err());
    }

    #[test]
    fn category_ids_match_serde_names() {
        assert_eq!(category_id(ProcedureCategory::RootCanal), "root_canal");
        assert_eq!(
            category_id(ProcedureCategory::FillingComposite),
            "filling_composite"
        );
    }

    #[test]
    fn repository_entries_load() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../knowledge");
        let kb = KnowledgeBase::load(&dir).expect("knowledge base loads");
        for entry in kb.entries() {
            entry.validate().unwrap();
        }
    }
}
