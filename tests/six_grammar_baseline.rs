//! U1: the six-grammar byte-identity baseline (R17, R18).
//!
//! This is characterization work. The snapshots here are correct **by
//! definition** — they record what this binary does today, whatever that is.
//! Nothing this fixture reveals gets fixed here; it gets recorded. A later unit
//! that changes extraction output is measured against these bytes.
//!
//! Why the existing goldens could not serve: `render.rs` and `manifest.rs`
//! build from a fixture whose `.rs` files are the single comment `// lib`, and
//! the card golden is C#-only. Between them they parse one real source file, so
//! five of six grammars had no coverage and a per-language regression could
//! land without turning anything red.
//!
//! **One snapshot per grammar, not one for the corpus.** U11 migrates all six
//! grammars in a single move — running the in-process extractor and a component
//! side by side under one extractor id would be exactly the dual-track period
//! KTD4 exists to avoid. Attribution is bought here instead: a whole-corpus
//! regression still resolves to one grammar because each grammar's slice is its
//! own snapshot. That is the property to preserve if this file is ever
//! reorganized.
//!
//! **What this baseline does not cover.** The fixture produces no ambiguous
//! relations and no merge dissent — deliberately, since language-prefixed
//! names are what keep each grammar's slice free of ambiguity that says
//! nothing about the grammar under it. Manufacturing collisions here would
//! fight the attribution this fixture exists for. Those paths are covered
//! directly instead, by `tests/merge.rs` and the `flush_usages` unit tests
//! in `src/extract/treesitter.rs`. A migration that changes ambiguity or
//! dissent handling will not move these snapshots — check those.
//!
//! Four more gaps, found by review and left open **deliberately** so a future
//! reader does not mistake absence for coverage:
//!
//! - **`Extends`, `Implements` and `EntryPoint` have zero coverage.** No
//!   fixture file declares a base type, interface, trait impl or entry point,
//!   so six separate inheritance parsers are unguarded. This is the largest
//!   known hole; closing it means adding a base/derived pair per grammar and
//!   re-capturing.
//! - **Only the uncached path is characterized.** These snapshots run
//!   `extract::run_all`, while `generate` runs `run_all_with_cached`
//!   (`src/lib.rs`). A change affecting only the cached path over these six
//!   grammars moves nothing here.
//! - **Rendering is not characterized.** No snapshot pins cards, corpus
//!   markdown or module structure over real multi-language source — the
//!   existing corpus goldens still use the near-empty three-file fixture.
//!   Only `two_runs_…` exercises the full pipeline, and it just compares two
//!   runs to each other.
//! - **[`GRAMMARS`] is hand-maintained.** `Language` in
//!   `src/extract/treesitter.rs` is private, so nothing couples the two. A
//!   seventh grammar would land with no characterization and nothing red;
//!   that check belongs to whichever unit adds one.

mod common;

use common::hash_dir;
use common::six_grammar::{self, GRAMMARS};
use reverse_architect::coverage::build_coverage;
use reverse_architect::extract::{run_all, trust_ranks, Repo};
use reverse_architect::graph::{
    merge, EdgeKind, FactKind, FactValue, Graph, NodeKind, Provenance, Span,
};
use reverse_architect::manifest::{build_manifest, to_json_pretty};
use std::path::Path;

/// Build the graph the way the real pipeline does — extract, then merge (KTD-B5)
/// — so every snapshot records what a user actually gets rather than a
/// pre-merge intermediate no artifact ever shows.
fn merged_graph_over(dir: &Path) -> Graph {
    let repo = Repo::open(dir).unwrap();
    let mut graph = Graph::new();
    run_all(&repo, &mut graph).unwrap();
    merge(&mut graph, &trust_ranks());
    graph
}

fn span_str(span: Span) -> String {
    match (span.start(), span.end()) {
        (None, _) => "whole".to_string(),
        (Some(start), None) => format!("{start}"),
        (Some(start), Some(end)) => format!("{start}-{end}"),
    }
}

fn value_str(value: &FactValue) -> String {
    match value {
        FactValue::Text(t) => format!("{t:?}"),
        FactValue::Int(i) => i.to_string(),
        FactValue::Bool(b) => b.to_string(),
    }
}

/// The bracketed provenance tail every rendered line carries.
///
/// One helper rather than three copies — and deliberately used for dissent
/// too. The first draft rendered a dissent claim *without* its
/// `ambiguous_candidates`, which would have let a later migration change a
/// losing claim's candidate count invisibly. `ambiguous_candidates` is the
/// lowest-priority `Ord` field in the merge tie-break, so a claim that lost
/// on it lands in dissent carrying exactly that number. This fixture
/// produces no dissent today, so unifying costs no snapshot bytes; it closes
/// the gap before there is one to miss.
fn provenance_str(p: &Provenance) -> String {
    let ambiguous = match p.ambiguous_candidates {
        Some(n) => format!(" ambiguous:{n}"),
        None => String::new(),
    };
    format!(
        "[{} {:?} {:?}{} @ {}:{}]",
        p.extractor,
        p.tier,
        p.resolution,
        ambiguous,
        p.file,
        span_str(p.span)
    )
}

/// Render every fact and edge whose provenance points inside `prefix`.
///
/// Slicing on the provenance rather than the node key is deliberate: a Symbol
/// node's key is a hash, and a File node can carry facts from several
/// extractors. What a regression changes is the set of assertions made *about
/// a given set of files*, which is exactly this.
///
/// A predicate rather than a path prefix so the three manifests can be sliced
/// too. Everything `manifest_deps` produces anchors to `Cargo.toml`, `go.mod`
/// or `package.json`, so none of it falls inside any grammar prefix — without
/// its own slice its only guard is the 4882-line manifest snapshot, whose
/// positional string interning makes one added symbol reindex every later
/// reference.
fn slice_where(graph: &Graph, keep: impl Fn(&Provenance) -> bool) -> String {
    let mut out = String::new();

    out.push_str("--- facts ---\n");
    for node in graph.nodes_sorted() {
        let facts: Vec<String> = node
            .facts
            .iter()
            .filter(|f| keep(&f.provenance))
            .map(|f| {
                let mut line = format!(
                    "  {:?} = {}  {}\n",
                    f.kind,
                    value_str(&f.value),
                    provenance_str(&f.provenance)
                );
                // A fact carries dissent exactly as an edge does. Rendering only
                // the edges' would leave a scalar-conflict change invisible in
                // every per-grammar slice.
                for claim in &f.dissent {
                    line.push_str(&format!(
                        "    dissent {}\n",
                        provenance_str(&claim.provenance)
                    ));
                }
                line
            })
            .collect();
        if facts.is_empty() {
            continue;
        }
        out.push_str(&format!("{:?} {}\n", node.kind, node.key.as_str()));
        for f in facts {
            out.push_str(&f);
        }
    }

    out.push_str("--- edges ---\n");
    for view in graph.edges_sorted() {
        let p = &view.edge.provenance;
        if !keep(p) {
            continue;
        }
        out.push_str(&format!(
            "{:?} {} -> {}  {}\n",
            view.edge.kind,
            view.from.as_str(),
            view.to.as_str(),
            provenance_str(p)
        ));
        for claim in &view.edge.dissent {
            out.push_str(&format!(
                "  dissent {}\n",
                provenance_str(&claim.provenance)
            ));
        }
    }

    out
}

/// Capture one grammar's slice.
///
/// One `#[test]` per grammar rather than a loop over [`GRAMMARS`]. A loop
/// aborts on the first `assert_snapshot!` that fails, so a regression in the
/// first grammar would hide the other five — which is precisely the
/// per-grammar attribution this file exists to provide. Six tests also put
/// the language in the test *name*.
fn capture_grammar(name: &str, prefix: &str) {
    let tmp = tempfile::tempdir().unwrap();
    six_grammar::init(tmp.path());
    let graph = merged_graph_over(tmp.path());
    insta::assert_snapshot!(
        format!("u1_{name}"),
        slice_where(&graph, |p| p.file.starts_with(prefix))
    );
}

#[test]
fn golden_rust_slice() {
    capture_grammar("rust", "rust/");
}

#[test]
fn golden_typescript_slice() {
    capture_grammar("typescript", "ts/");
}

#[test]
fn golden_tsx_slice() {
    capture_grammar("tsx", "tsx/");
}

#[test]
fn golden_python_slice() {
    capture_grammar("python", "py/");
}

#[test]
fn golden_csharp_slice() {
    capture_grammar("csharp", "cs/");
}

#[test]
fn golden_go_slice() {
    capture_grammar("go", "go/");
}

#[test]
fn golden_javascript_slice() {
    capture_grammar("javascript", "js/");
}

#[test]
fn golden_java_slice() {
    capture_grammar("java", "java/");
}

/// The manifests get their own slice: nothing they produce lands under a
/// grammar prefix, so without this the only attributed check is a three-name
/// substring assertion.
#[test]
fn golden_manifest_slice() {
    let tmp = tempfile::tempdir().unwrap();
    six_grammar::init(tmp.path());
    let graph = merged_graph_over(tmp.path());
    insta::assert_snapshot!(
        "u1_manifests",
        slice_where(&graph, |p| six_grammar::MANIFESTS
            .contains(&p.file.as_str()))
    );
}

#[test]
fn golden_six_grammar_manifest() {
    let tmp = tempfile::tempdir().unwrap();
    six_grammar::init(tmp.path());
    let graph = merged_graph_over(tmp.path());
    let manifest = build_manifest(&graph, None).unwrap();

    // Pretty here, compact in the artifact: a one-line snapshot is a snapshot
    // nobody can approve (the reason `manifest.rs`'s golden does the same).
    insta::assert_snapshot!("u1_manifest", to_json_pretty(&manifest));
}

#[test]
fn golden_six_grammar_coverage() {
    let tmp = tempfile::tempdir().unwrap();
    six_grammar::init(tmp.path());
    let repo = Repo::open(tmp.path()).unwrap();
    let graph = merged_graph_over(tmp.path());
    // The card noise as the pipeline counts it, so the golden carries the
    // whole `noise` section rather than only its graph half.
    let (_, _, card_rows) = reverse_architect::card::build_cards_with_insight(&graph);
    // The registry as the pipeline builds it, so the golden carries the
    // contract each language was parsed under (plan A, U5) rather than a shape
    // `generate` never writes.
    let mut languages = reverse_architect::extract::languages::Languages::bundled();
    reverse_architect::plugin::PluginSet::bundled().register_languages(&mut languages);
    let coverage = build_coverage(&graph, repo.head_revision(), &languages, Some(card_rows));

    // The revision is asserted, not snapshotted. Pinning the fixture's commit
    // oid would redden this snapshot on every fixture edit for a reason that
    // has nothing to do with extraction — and it carries the only per-language
    // coverage counts, so training people to accept its diffs is expensive.
    let revision = repo.head_revision().expect("the fixture has one commit");
    assert_eq!(
        coverage.repo_revision.as_deref(),
        Some(revision.as_str()),
        "coverage must carry the revision it was built from"
    );
    let json = reverse_architect::coverage::to_json(&coverage).replace(&revision, "[oid]");
    insta::assert_snapshot!("u1_coverage", json);
}

/// The fixture's own contract: if this ever goes red, the fixture stopped being
/// the instrument the snapshots assume — which is a different failure from a
/// snapshot diff and deserves to be told apart.
#[test]
fn every_grammar_contributes_a_declaration_and_a_usage_edge() {
    let tmp = tempfile::tempdir().unwrap();
    six_grammar::init(tmp.path());
    let graph = merged_graph_over(tmp.path());

    for (name, prefix) in GRAMMARS {
        let declares = graph.nodes_sorted().iter().any(|n| {
            n.facts
                .iter()
                .any(|f| f.kind == FactKind::SymbolName && f.provenance.file.starts_with(prefix))
        });
        // Must name the edge KINDS that mean "one symbol used another". Without
        // that filter this is satisfied by filetree's `Contains` edge, which
        // every file gets whether or not the grammar produced anything at all —
        // so the assertion could not fail on total loss of tree-sitter
        // relations, which is the regression it exists to catch.
        let uses = graph.edges_sorted().iter().any(|v| {
            v.edge.provenance.file.starts_with(prefix)
                && matches!(
                    v.edge.kind,
                    EdgeKind::Calls | EdgeKind::References | EdgeKind::Instantiates
                )
        });
        assert!(declares, "{name}: no declaration fact under {prefix}");
        assert!(
            uses,
            "{name}: no Calls/References/Instantiates edge under {prefix}"
        );
    }
}

/// Internal imports resolve in every language whose resolver can see them.
///
/// This is not decoration. `crate_root_dir` fails closed without a `src/`
/// component and `resolve_python_from` bails on a non-relative import, so an
/// earlier draft of this fixture produced no `Imports` edge for Rust or
/// Python — and the snapshots pinned that absence as the correct answer. Go
/// and C# are excluded because they resolve through package and namespace,
/// not through a file-level import the extractor models as an edge.
#[test]
fn internal_imports_resolve_for_every_language_that_models_them() {
    let tmp = tempfile::tempdir().unwrap();
    six_grammar::init(tmp.path());
    let graph = merged_graph_over(tmp.path());

    for (name, prefix) in [
        ("rust", "rust/"),
        ("typescript", "ts/"),
        ("tsx", "tsx/"),
        ("python", "py/"),
        ("javascript", "js/"),
    ] {
        let imports = graph.edges_sorted().iter().any(|v| {
            v.edge.provenance.file.starts_with(prefix) && v.edge.kind == EdgeKind::Imports
        });
        assert!(
            imports,
            "{name}: consumer's import of model did not resolve"
        );
    }
}

/// Every grammar the extractor models doc comments for contributes one
/// carrying diacritics and a line break — and Go, which it does not, carries
/// none.
///
/// `DocComment` is the one fact holding verbatim prose R10 promises is never
/// reflowed or stripped of diacritics, and U11 moves its producer across a
/// WASM boundary where string marshaling is the likeliest defect class there
/// is. If this goes red the fixture stopped being able to see that corruption.
///
/// **Go is excluded because the extractor does not read its doc comments, not
/// because the fixture omits them.** `preceding_doc_comment` handles Rust
/// `///`, C# `///` and TS/JS `/** … */` (`src/extract/treesitter.rs:1650-1652`);
/// Go's `//` convention is not among them. The fixture's Go files carry
/// idiomatic doc comments anyway, and the absence is *asserted* rather than
/// ignored — so if support is ever added this test goes red and someone
/// records the change deliberately, instead of a snapshot quietly gaining
/// facts nobody decided on.
#[test]
fn doc_comments_carry_diacritics_and_line_breaks_where_the_extractor_reads_them() {
    let tmp = tempfile::tempdir().unwrap();
    six_grammar::init(tmp.path());
    let graph = merged_graph_over(tmp.path());

    let docs_under = |prefix: &str| -> Vec<String> {
        graph
            .nodes_sorted()
            .into_iter()
            .flat_map(|n| n.facts.iter())
            .filter(|f| f.kind == FactKind::DocComment && f.provenance.file.starts_with(prefix))
            .filter_map(|f| match &f.value {
                FactValue::Text(t) => Some(t.clone()),
                _ => None,
            })
            .collect()
    };

    let go_docs = docs_under("go/");
    assert!(
        go_docs.is_empty(),
        "the extractor grew Go doc-comment support; that is a real behavior \
         change to record deliberately, not to absorb: {go_docs:?}"
    );

    for (name, prefix) in GRAMMARS.iter().filter(|(n, _)| *n != "go") {
        let docs = docs_under(prefix);
        assert!(
            !docs.is_empty(),
            "{name}: no DocComment fact under {prefix}"
        );
        assert!(
            docs.iter().any(|d| !d.is_ascii()),
            "{name}: no doc comment carries a non-ASCII character; a marshaling \
             bug at the WASM boundary would be invisible here: {docs:?}"
        );
        assert!(
            docs.iter().any(|d| d.contains('\n')),
            "{name}: no doc comment spans two lines; reflowing would be invisible: {docs:?}"
        );
    }
}

/// Every *source* file is structural, and the only files that are not are the
/// three manifests.
///
/// The plan's scenario reads "every fixture file ... with no
/// `not_structural_because` reason". Taken literally that cannot hold and
/// should not: `Cargo.toml`, `go.mod` and `package.json` have no grammar, and
/// coverage saying so is the artifact working. Asserted at the honest scope —
/// twelve structural sources, three named exceptions — so the test fails if a
/// grammar stops producing structure *or* if the exception set grows.
#[test]
fn every_source_file_is_structural_and_only_the_manifests_are_not() {
    let tmp = tempfile::tempdir().unwrap();
    six_grammar::init(tmp.path());
    let repo = Repo::open(tmp.path()).unwrap();
    let graph = merged_graph_over(tmp.path());
    let coverage = build_coverage(
        &graph,
        repo.head_revision(),
        &reverse_architect::extract::languages::Languages::bundled(),
        None,
    );

    for file in &coverage.files {
        let expected_structural = !six_grammar::MANIFESTS.contains(&file.path.as_str());
        assert_eq!(
            file.structural, expected_structural,
            "{}: structural={} with reason {:?}",
            file.path, file.structural, file.not_structural_because
        );
        if expected_structural {
            assert!(
                file.not_structural_because.is_none(),
                "{}: carries a reason while structural",
                file.path
            );
        }
    }

    assert_eq!(
        coverage.unclassified_files,
        six_grammar::MANIFESTS.len(),
        "only the three manifests belong to no language"
    );
    // Pinned against the fixture's own list. The totals check below cannot do
    // this: both its sides derive from the same value, so dropping a fixture
    // file keeps `11 + 3 == 14` true.
    assert_eq!(
        coverage.files.len(),
        six_grammar::files().len(),
        "coverage must see every fixture file"
    );
    let total: usize = coverage.languages.iter().map(|l| l.files).sum();
    assert_eq!(
        total + coverage.unclassified_files,
        coverage.files.len(),
        "the coverage totals must close"
    );
}

/// R18's second half: the three manifest files bring `manifest_deps` and the
/// language tables under the same guard.
#[test]
fn all_three_manifest_files_produce_dependency_nodes() {
    let tmp = tempfile::tempdir().unwrap();
    six_grammar::init(tmp.path());
    let graph = merged_graph_over(tmp.path());

    let deps: Vec<&str> = graph
        .nodes_sorted()
        .into_iter()
        .filter(|n| n.kind == NodeKind::Dependency)
        .map(|n| n.key.as_str())
        .collect();

    for expected in ["serde", "github.com/google/uuid", "left-pad"] {
        assert!(
            deps.iter().any(|d| d.contains(expected)),
            "no dependency node from {expected}; saw: {deps:?}"
        );
    }
}

/// Determinism over the fixture, proven the way this repo already proves it.
#[test]
fn two_runs_over_the_fixture_produce_byte_identical_corpora() {
    let repo_dir = tempfile::tempdir().unwrap();
    six_grammar::init(repo_dir.path());

    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    for out in [first.path(), second.path()] {
        reverse_architect::generate(&common::config_for(repo_dir.path(), out)).unwrap();
    }

    // `hash_dir` folds an unreadable or missing tree into the same digest as
    // another one, so two empty output directories agree perfectly. Prove the
    // runs produced something before comparing them.
    let written = std::fs::read_dir(first.path()).unwrap().count();
    assert!(written > 0, "the first run wrote no corpus at all");
    assert_eq!(
        hash_dir(first.path()),
        hash_dir(second.path()),
        "two runs over the six-grammar fixture disagreed"
    );
}
