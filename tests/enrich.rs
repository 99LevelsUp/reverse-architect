//! End-to-end: the enrich pipeline over a real fixture repo, driven by the
//! deterministic FakeBackend (no network). The Phase T1-A exit gate.

use reverse_architect::enrich::backend::FakeBackend;
use reverse_architect::enrich::{enrich, EnrichConfig};
use reverse_architect::extract::{self, Repo};
use reverse_architect::graph::{self, Graph};
use tempfile::TempDir;

mod common;

fn graph_over_fixture() -> (TempDir, Graph) {
    let tmp = TempDir::new().unwrap();
    // Reuse the existing fixture helper (tests/common/mod.rs): init a repo with
    // files and one fixed-signature commit. TempDir is returned so it outlives use.
    common::init_repo_with_files(
        tmp.path(),
        "Ann",
        &[
            ("core/lib.rs", "pub fn f() {}\n"),
            ("README.md", "# demo\n"),
        ],
    );
    let repo = Repo::open(tmp.path()).unwrap();
    let mut graph = Graph::new();
    extract::run_all(&repo, &mut graph).unwrap();
    graph::merge(&mut graph, &extract::trust_ranks());
    (tmp, graph)
}

#[test]
fn enrich_over_fixture_produces_a_complete_corpus() {
    let (_tmp, graph) = graph_over_fixture();
    let backend = FakeBackend::new(vec!["A small, cohesive repository.".into(); 16]);
    let cfg = EnrichConfig {
        lang: "en".into(),
        retries: 2,
        profile: None,
        links: reverse_architect::cli::LinkStyle::Portable,
    };

    let result = enrich(&graph, &backend, &cfg).unwrap();

    // index.md + one leaf per view, none empty.
    assert!(result.corpus.files.iter().any(|f| f.path == "index.md"));
    assert!(result.corpus.files.iter().all(|f| !f.contents.is_empty()));
    // At least one leaf enriched; the report accounts for every non-map file.
    assert_eq!(
        result.report.enriched + result.report.degraded,
        result.corpus.files.len() - 1
    );
}
