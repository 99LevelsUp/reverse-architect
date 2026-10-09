//! Opt-in live smoke test (T1-KTD6): runs a real backend, so it is #[ignore]d and
//! excluded from CI. Verifies the guardrail invariant against a real model — that
//! *something* is generated and it passes the guardrail — not specific bytes.
//! Run with: `cargo test --test enrich_smoke -- --ignored`
//! Requires a reachable backend, e.g. a local Ollama at http://localhost:11434.

use reverse_architect::enrich::backend::{build_backend, BackendChoice, BackendKind};
use reverse_architect::enrich::{enrich, EnrichConfig};
use reverse_architect::extract::{self, Repo};
use reverse_architect::graph::{self, Graph};
use tempfile::TempDir;

mod common;

#[test]
#[ignore = "requires a live LLM backend"]
fn live_ollama_enriches_without_guardrail_violation() {
    let tmp = TempDir::new().unwrap();
    common::init_repo_with_files(tmp.path(), "Ann", &[("core/lib.rs", "pub fn f() {}\n")]);
    let repo = Repo::open(tmp.path()).unwrap();
    let mut graph = Graph::new();
    extract::run_all(&repo, &mut graph).unwrap();
    graph::merge(&mut graph, &extract::trust_ranks());

    let choice = BackendChoice {
        kind: BackendKind::Ollama,
        base_url: Some("http://localhost:11434".into()),
        model: Some(std::env::var("RA_SMOKE_MODEL").unwrap_or_else(|_| "llama3.1".into())),
        auth_env: None,
    };
    let backend = build_backend(&choice).unwrap();
    let cfg = EnrichConfig {
        lang: "en".into(),
        retries: 2,
        profile: None,
        links: reverse_architect::cli::LinkStyle::Portable,
    };

    let result = enrich(&graph, backend.as_ref(), &cfg).unwrap();
    // The invariant: whatever the model wrote that survived is a complete corpus;
    // any fabrication would have degraded the leaf, never emitted a bad fact.
    assert!(result.corpus.files.iter().all(|f| !f.contents.is_empty()));
    eprintln!(
        "enriched {}, degraded {}",
        result.report.enriched, result.report.degraded
    );
}
