//! Fixtures shared by this crate's tests.

use dc_core::PlanSnapshot;
use dc_core::plan::PlanKey;
use dc_knowledge::{CodeMap, Entry, KnowledgeBase};
use dc_pms::PmsConnector;
use dc_pms::synthetic::SyntheticConnector;
use std::path::Path;

fn root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Daniel Okafor's plan from the synthetic practice.
pub fn daniel() -> PlanSnapshot {
    let connector =
        SyntheticConnector::from_path(&root().join("fixtures/synthetic/practice.toml")).unwrap();
    futures::executor::block_on(connector.plan_snapshot(&PlanKey("tp-10482-1".into()))).unwrap()
}

/// The repository's code map with no entries.
pub fn kb() -> KnowledgeBase {
    let codes = CodeMap::from_path(&root().join("knowledge/codes.toml")).unwrap();
    KnowledgeBase::from_parts(codes, vec![]).unwrap()
}

/// The code map plus a small root canal entry written for tests.
pub fn kb_with_entry() -> KnowledgeBase {
    let codes = CodeMap::from_path(&root().join("knowledge/codes.toml")).unwrap();
    KnowledgeBase::from_parts(codes, vec![root_canal_entry()]).unwrap()
}

pub fn root_canal_entry() -> Entry {
    let passage = |id: &str, text: &str| {
        format!(
            "[[passages]]\nid = \"{id}\"\ntopic = \"{id}\"\ntext = \"{text}\"\nsources = [\"test\"]\n"
        )
    };
    let source = format!(
        "id = \"root_canal\"\nversion = 1\ntitle = \"Root canal\"\nreview_status = \"unreviewed\"\nupdated = \"2026-11-26\"\n\
         [[sources]]\nid = \"test\"\npublisher = \"Test\"\ntitle = \"Test\"\nurl = \"https://example.org\"\naccessed = \"2026-11-26\"\n{}{}{}{}{}{}{}",
        passage(
            "what_it_is",
            "A root canal treats the soft tissue inside a tooth."
        ),
        passage(
            "common_reasons",
            "Common reasons include deep decay or a crack that reaches the nerve."
        ),
        passage(
            "what_happens",
            "The area is numbed, the inside is cleaned and the tooth is sealed."
        ),
        passage(
            "recovery",
            "Many people feel tender for a short while afterwards."
        ),
        passage("benefits", "It can let you keep your natural tooth."),
        passage(
            "if_delayed",
            "An infection inside a tooth can spread over time."
        ),
        passage("questions", "Will I need a crown afterwards?"),
    );
    toml::from_str(&source).unwrap()
}
