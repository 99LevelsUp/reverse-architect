//! U4 integration tests: taxonomy + lens + markdown renderer.
//!
//! Also stands up the determinism harness (advisor): golden snapshot plus
//! double-build and cross-prefix byte-equality over the rendered corpus.

mod common;

use common::init_repo_with_files;
use reverse_architect::cli::LinkStyle;
use reverse_architect::extract::{run_all, trust_ranks, Repo};
use reverse_architect::graph::{merge, Graph};
use reverse_architect::render::{build_corpus, Corpus};
use std::path::Path;

fn corpus_over(dir: &Path, links: LinkStyle) -> Corpus {
    let repo = Repo::open(dir).unwrap();
    let mut graph = Graph::new();
    run_all(&repo, &mut graph).unwrap();
    build_corpus(&graph, links, None).unwrap()
}

/// Like `corpus_over` but merges first — the real pipeline order (KTD-B5), so
/// additive facts (e.g. a dep in both dep tables) are resolved as a user sees.
fn merged_corpus_over(dir: &Path, links: LinkStyle) -> Corpus {
    let repo = Repo::open(dir).unwrap();
    let mut graph = Graph::new();
    run_all(&repo, &mut graph).unwrap();
    merge(&mut graph, &trust_ranks());
    build_corpus(&graph, links, None).unwrap()
}

fn file<'a>(corpus: &'a Corpus, path: &str) -> &'a str {
    &corpus
        .files
        .iter()
        .find(|f| f.path == path)
        .unwrap_or_else(|| panic!("missing corpus file: {path}"))
        .contents
}

fn standard_fixture(dir: &Path) {
    init_repo_with_files(
        dir,
        "Alice",
        &[
            ("src/lib.rs", "// lib"),
            ("src/extract/git.rs", "// git"),
            ("README.md", "# readme"),
        ],
    );
}

#[test]
fn map_links_to_all_leaves() {
    let tmp = tempfile::tempdir().unwrap();
    standard_fixture(tmp.path());
    let corpus = corpus_over(tmp.path(), LinkStyle::Portable);

    let index = file(&corpus, "index.md");
    assert!(index.contains("[Overview](overview.md)"));
    assert!(index.contains("[Module structure](module-structure.md)"));
}

#[test]
fn leaf_is_self_contained_with_prescribed_sections() {
    let tmp = tempfile::tempdir().unwrap();
    standard_fixture(tmp.path());
    let corpus = corpus_over(tmp.path(), LinkStyle::Portable);

    let overview = file(&corpus, "overview.md");
    // Section ordering: guiding question, Problem, Mechanism, In this repository.
    let gq = overview.find("**Guiding question:**").unwrap();
    let problem = overview.find("## Problem").unwrap();
    let mechanism = overview.find("## Mechanism").unwrap();
    let instances = overview.find("## In this repository").unwrap();
    assert!(gq < problem && problem < mechanism && mechanism < instances);
}

#[test]
fn problem_and_mechanism_are_identical_across_repos_but_instances_differ() {
    let big = tempfile::tempdir().unwrap();
    standard_fixture(big.path());
    let small = tempfile::tempdir().unwrap();
    init_repo_with_files(small.path(), "Bob", &[("only.rs", "// one")]);

    let big_overview =
        file(&corpus_over(big.path(), LinkStyle::Portable), "overview.md").to_string();
    let small_overview = file(
        &corpus_over(small.path(), LinkStyle::Portable),
        "overview.md",
    )
    .to_string();

    // Authored framing (KTD9) is byte-identical regardless of the repo.
    let problem_sentence = "A newcomer needs a truthful first picture";
    assert!(big_overview.contains(problem_sentence));
    assert!(small_overview.contains(problem_sentence));

    // Instances differ — different file counts.
    assert!(big_overview.contains("3 tracked files"));
    assert!(small_overview.contains("1 tracked file across"));
}

#[test]
fn flat_repo_module_structure_reports_no_facts() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(tmp.path(), "Alice", &[("a.rs", "// a"), ("b.rs", "// b")]);
    let corpus = corpus_over(tmp.path(), LinkStyle::Portable);

    let module_structure = file(&corpus, "module-structure.md");
    assert!(module_structure.contains("No facts were found for this view."));
}

#[test]
fn obsidian_links_use_wikilinks() {
    let tmp = tempfile::tempdir().unwrap();
    standard_fixture(tmp.path());
    let corpus = corpus_over(tmp.path(), LinkStyle::Obsidian);

    let index = file(&corpus, "index.md");
    assert!(index.contains("[[overview|Overview]]"));
    assert!(!index.contains("(overview.md)"));
}

#[test]
fn corpus_is_deterministic_across_builds() {
    let tmp = tempfile::tempdir().unwrap();
    standard_fixture(tmp.path());

    let a = corpus_over(tmp.path(), LinkStyle::Portable);
    let b = corpus_over(tmp.path(), LinkStyle::Portable);
    assert_eq!(a.files, b.files);
}

/// Cross-prefix: the same repo content at two different absolute paths must
/// produce a byte-identical corpus (repo-root-relative keys, KTD7). This catches
/// non-portability that a same-machine double run cannot.
#[test]
fn corpus_is_identical_across_absolute_prefixes() {
    let one = tempfile::tempdir().unwrap();
    standard_fixture(one.path());
    let two = tempfile::tempdir().unwrap();
    standard_fixture(two.path());

    let a = corpus_over(one.path(), LinkStyle::Portable);
    let b = corpus_over(two.path(), LinkStyle::Portable);
    assert_eq!(a.files, b.files);
}

// ---- U10: the new backbone views ----

/// A polyglot fixture with a manifest (deps + a bin entry) and a real `fn main`.
fn rich_fixture(dir: &Path) {
    init_repo_with_files(
        dir,
        "Alice",
        &[
            (
                "Cargo.toml",
                "[dependencies]\nserde = \"1\"\n\n[dev-dependencies]\ntempfile = \"3\"\n\n[[bin]]\nname = \"tool\"\npath = \"src/main.rs\"\n",
            ),
            ("src/main.rs", "fn main() {}\n"),
            ("src/lib.rs", "pub fn helper() {}\n"),
        ],
    );
}

#[test]
fn dependencies_view_distinguishes_dev_and_normal() {
    let tmp = tempfile::tempdir().unwrap();
    rich_fixture(tmp.path());
    let corpus = corpus_over(tmp.path(), LinkStyle::Portable);

    let deps = file(&corpus, "dependencies.md");
    assert!(
        deps.contains("`serde` (normal)"),
        "normal dep listed: {deps}"
    );
    assert!(deps.contains("`tempfile` (dev)"), "dev dep listed: {deps}");
    assert!(
        deps.contains("declared in `Cargo.toml`"),
        "declaring manifest named: {deps}"
    );
}

#[test]
fn dependency_in_both_tables_keeps_both_scopes_after_merge() {
    // The additive-scope case the merge layer must not collapse (E5). Asserted
    // over a MERGED graph — the real pipeline order — not just raw extraction.
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[(
            "Cargo.toml",
            "[dependencies]\nx = \"1\"\n\n[dev-dependencies]\nx = \"1\"\n",
        )],
    );
    let corpus = merged_corpus_over(tmp.path(), LinkStyle::Portable);

    let deps = file(&corpus, "dependencies.md");
    assert!(
        deps.contains("`x` (dev, normal)"),
        "both scopes survive merge: {deps}"
    );
}

#[test]
fn repo_without_manifests_shows_dependencies_no_facts_note() {
    // A backbone view with no facts is explicit, never omitted (R18).
    let tmp = tempfile::tempdir().unwrap();
    standard_fixture(tmp.path());
    let corpus = corpus_over(tmp.path(), LinkStyle::Portable);

    let deps = file(&corpus, "dependencies.md");
    assert!(deps.contains("No facts were found for this view."));
}

#[test]
fn entry_points_view_lists_language_and_manifest_entries() {
    let tmp = tempfile::tempdir().unwrap();
    rich_fixture(tmp.path());
    let corpus = corpus_over(tmp.path(), LinkStyle::Portable);

    let entries = file(&corpus, "entry-points.md");
    // Language-level `fn main` and the manifest `[[bin]]` are both real sources.
    assert!(
        entries.contains("`main` in `src/main.rs` — language-level entry point."),
        "language entry listed: {entries}"
    );
    assert!(
        entries.contains("`bin:tool` declared in `Cargo.toml` — manifest entry point."),
        "manifest entry listed: {entries}"
    );
}

#[test]
fn entry_points_view_is_honest_when_none_detected() {
    // No `fn main`, no manifest → honest E9 wording, never empty-silent and
    // never a bare "no facts" note.
    let tmp = tempfile::tempdir().unwrap();
    standard_fixture(tmp.path());
    let corpus = corpus_over(tmp.path(), LinkStyle::Portable);

    let entries = file(&corpus, "entry-points.md");
    assert!(
        entries.contains("No language-level or manifest-declared entry point was detected"),
        "honest scoped-absence wording: {entries}"
    );
    assert!(
        entries.contains("Framework-level entry points"),
        "framework caveat present: {entries}"
    );
    assert!(
        !entries.contains("No facts were found for this view."),
        "never the bare R18 note: {entries}"
    );
    // Never falsely enumerates an entry that was not detected.
    assert!(!entries.contains("language-level entry point."));
    assert!(!entries.contains("manifest entry point."));
}

#[test]
fn test_coverage_view_uses_exact_wording_never_untested() {
    let tmp = tempfile::tempdir().unwrap();
    standard_fixture(tmp.path());
    let corpus = corpus_over(tmp.path(), LinkStyle::Portable);

    let cov = file(&corpus, "test-coverage.md");
    assert!(
        cov.contains("test by convention not found"),
        "exact pairing wording: {cov}"
    );
    // Scope the "untested" guard to the repo-derived instances — the authored
    // Mechanism prose legitimately uses the word to explain why verdicts avoid it.
    let instances = cov
        .split("## In this repository")
        .nth(1)
        .expect("instances section");
    assert!(
        !instances.contains("untested"),
        "verdicts never claim absence: {instances}"
    );
    // The extraction-coverage line is present in the dedicated view too, with the
    // same `:` separator the overview uses (unified — no punctuation drift).
    assert!(cov.contains("Extraction coverage:"), "coverage line: {cov}");
}

#[test]
fn overview_shows_the_extraction_coverage_level() {
    let tmp = tempfile::tempdir().unwrap();
    standard_fixture(tmp.path());
    let corpus = corpus_over(tmp.path(), LinkStyle::Portable);

    let overview = file(&corpus, "overview.md");
    assert!(
        overview.contains("Extraction coverage:"),
        "overview carries the E9 coverage level: {overview}"
    );
    assert!(overview.contains("carry tree-sitter structural facts"));
}

#[test]
fn every_backbone_view_is_present_on_a_rich_repo() {
    // E11: the full backbone renders, each wired to a real extractor source.
    let tmp = tempfile::tempdir().unwrap();
    rich_fixture(tmp.path());
    let corpus = corpus_over(tmp.path(), LinkStyle::Portable);

    for leaf in [
        "overview.md",
        "module-structure.md",
        "dependencies.md",
        "entry-points.md",
        "test-coverage.md",
    ] {
        assert!(
            corpus.files.iter().any(|f| f.path == leaf),
            "missing backbone leaf: {leaf}"
        );
    }
}

#[test]
fn rich_corpus_is_deterministic_across_builds() {
    let tmp = tempfile::tempdir().unwrap();
    rich_fixture(tmp.path());

    let a = corpus_over(tmp.path(), LinkStyle::Portable);
    let b = corpus_over(tmp.path(), LinkStyle::Portable);
    assert_eq!(a.files, b.files);
}

/// Cross-prefix over the rich fixture: exercises the new data paths (dep nodes,
/// symbol/entry provenance, manifest decls) for an absolute-path leak a same-
/// prefix double-run cannot catch (KTD7).
#[test]
fn rich_corpus_is_identical_across_absolute_prefixes() {
    let one = tempfile::tempdir().unwrap();
    rich_fixture(one.path());
    let two = tempfile::tempdir().unwrap();
    rich_fixture(two.path());

    let a = corpus_over(one.path(), LinkStyle::Portable);
    let b = corpus_over(two.path(), LinkStyle::Portable);
    assert_eq!(a.files, b.files);
}

// ---- U11: mermaid diagrams at the leaves ----

/// The number of `["` node declarations inside each ```mermaid block.
fn mermaid_block_node_counts(leaf: &str) -> Vec<usize> {
    leaf.split("```mermaid")
        .skip(1)
        .map(|rest| {
            // Count node-declaration lines up to the block's closing fence.
            let block = rest.split("```").next().unwrap_or("");
            block.lines().filter(|l| l.contains("[\"")).count()
        })
        .collect()
}

#[test]
fn module_structure_leaf_carries_a_diagram_above_its_instances() {
    let tmp = tempfile::tempdir().unwrap();
    standard_fixture(tmp.path());
    let corpus = corpus_over(tmp.path(), LinkStyle::Portable);

    let leaf = file(&corpus, "module-structure.md");
    let diagram = leaf.find("## Diagram").expect("diagram section present");
    let mechanism = leaf.find("## Mechanism").unwrap();
    let instances = leaf.find("## In this repository").unwrap();
    // Slotted after Mechanism, above the authoritative instances (KTD-C2/R7).
    assert!(mechanism < diagram && diagram < instances);
    assert!(
        leaf.contains("```mermaid"),
        "a fenced mermaid block: {leaf}"
    );
    assert!(leaf.contains("flowchart TD"), "a flowchart header: {leaf}");
}

#[test]
fn diagram_is_accompanied_by_the_instances_prose_so_the_leaf_stands_without_it() {
    // R7: the leaf is retrievable without parsing any mermaid — the same facts
    // are asserted in words in the instances section that follows the diagram.
    let tmp = tempfile::tempdir().unwrap();
    standard_fixture(tmp.path());
    let corpus = corpus_over(tmp.path(), LinkStyle::Portable);

    let leaf = file(&corpus, "module-structure.md");
    let after_diagram = leaf.split("## Diagram").nth(1).expect("diagram section");
    assert!(
        after_diagram.contains("## In this repository"),
        "instances follow the diagram"
    );
    assert!(
        after_diagram.contains("`src` — 2 entries, 2 files"),
        "the same structural facts are stated in words: {after_diagram}"
    );
}

#[test]
fn a_view_without_a_diagram_fn_emits_no_diagram_section() {
    let tmp = tempfile::tempdir().unwrap();
    standard_fixture(tmp.path());
    let corpus = corpus_over(tmp.path(), LinkStyle::Portable);

    // Overview has no diagram fn; entry-points/test-coverage likewise.
    let overview = file(&corpus, "overview.md");
    assert!(
        !overview.contains("## Diagram"),
        "no empty diagram: {overview}"
    );
}

#[test]
fn dependencies_leaf_diagrams_declared_dependencies() {
    let tmp = tempfile::tempdir().unwrap();
    rich_fixture(tmp.path());
    let corpus = corpus_over(tmp.path(), LinkStyle::Portable);

    let deps = file(&corpus, "dependencies.md");
    assert!(
        deps.contains("## Diagram"),
        "diagram present with deps: {deps}"
    );
    // Labels are the raw dep node keys (`dep:<eco>:<name>`), decoupled from the
    // human name the prose uses.
    assert!(
        deps.contains("dep:cargo:serde"),
        "dep node labelled: {deps}"
    );
}

#[test]
fn a_repo_without_dependencies_has_no_diagram_on_that_leaf() {
    let tmp = tempfile::tempdir().unwrap();
    standard_fixture(tmp.path());
    let corpus = corpus_over(tmp.path(), LinkStyle::Portable);

    let deps = file(&corpus, "dependencies.md");
    assert!(
        !deps.contains("## Diagram"),
        "no diagram when no deps: {deps}"
    );
}

#[test]
fn large_module_tree_auto_splits_each_block_within_threshold() {
    // A repo whose module tree exceeds the 30-node threshold — including one
    // top-level module (`src`) that alone holds far more than 30 nodes across two
    // sub-modules, so the split must recurse past a single seam level.
    let tmp = tempfile::tempdir().unwrap();
    let mut files: Vec<(String, String)> = Vec::new();
    for i in 0..20 {
        files.push((format!("src/a/f{i:02}.rs"), "// a".to_string()));
        files.push((format!("src/b/f{i:02}.rs"), "// b".to_string()));
    }
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(p, c)| (p.as_str(), c.as_str()))
        .collect();
    init_repo_with_files(tmp.path(), "Alice", &refs);
    let corpus = corpus_over(tmp.path(), LinkStyle::Portable);

    let leaf = file(&corpus, "module-structure.md");
    let counts = mermaid_block_node_counts(leaf);
    assert!(
        counts.len() > 1,
        "auto-split into multiple blocks: {counts:?}"
    );
    for c in &counts {
        assert!(
            *c <= reverse_architect::render::mermaid::SPLIT_THRESHOLD,
            "every block within threshold: {counts:?}"
        );
    }
}

#[test]
fn diagrams_are_deterministic_across_builds() {
    let tmp = tempfile::tempdir().unwrap();
    rich_fixture(tmp.path());

    let a = corpus_over(tmp.path(), LinkStyle::Portable);
    let b = corpus_over(tmp.path(), LinkStyle::Portable);
    let da = file(&a, "module-structure.md");
    let db = file(&b, "module-structure.md");
    assert_eq!(da, db, "byte-identical mermaid across runs");
}

// ---- U12: cohesion/coupling smell surfaced in failure-modes ----

#[test]
fn low_cohesion_module_surfaces_a_split_recommendation_end_to_end() {
    // A module above the size guard whose files never reference one another →
    // the smell fires and the module-structure leaf carries a Failure modes
    // section with a "Consider splitting" recommendation. Runs through the real
    // pipeline so the wiring in `lens::compose` and `render_leaf` is exercised.
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            ("src/widgets/a.rs", "// a"),
            ("src/widgets/b.rs", "// b"),
            ("src/widgets/c.rs", "// c"),
            // A resolvable import in a below-guard module gives the graph an
            // import signal, so the zero-signal guard (E9) doesn't suppress —
            // this is genuine low cohesion in widgets, not a no-import repo.
            ("src/sig/s0.rs", "use crate::sig::s1;\n"),
            ("src/sig/s1.rs", "// s1"),
        ],
    );
    let corpus = corpus_over(tmp.path(), LinkStyle::Portable);

    let leaf = file(&corpus, "module-structure.md");
    assert!(
        leaf.contains("## Failure modes"),
        "failure-modes section present: {leaf}"
    );
    assert!(
        leaf.contains("Consider splitting `src/widgets`"),
        "split recommendation surfaced: {leaf}"
    );
    // R5: a recommendation, never a verdict — scope the guard to the section.
    let failure = leaf
        .split("## Failure modes")
        .nth(1)
        .expect("failure-modes section");
    for verdict in ["wrong", "bad", "broken", "poor", "violation"] {
        assert!(
            !failure.to_lowercase().contains(verdict),
            "smell note is never a verdict ({verdict}): {failure}"
        );
    }
}

#[test]
fn healthy_small_repo_has_no_failure_modes_section() {
    // The standard fixture sits below the size guard (no module holds 3+ direct
    // files), so the smell stays silent and no Failure modes section renders —
    // the no-false-alarm path through the real pipeline (KTD-C4).
    let tmp = tempfile::tempdir().unwrap();
    standard_fixture(tmp.path());
    let corpus = corpus_over(tmp.path(), LinkStyle::Portable);

    let leaf = file(&corpus, "module-structure.md");
    assert!(
        !leaf.contains("## Failure modes"),
        "no smell on a below-guard repo: {leaf}"
    );
    // Only the module-structure view carries the smell; others never do.
    for other in [
        "overview.md",
        "dependencies.md",
        "entry-points.md",
        "test-coverage.md",
    ] {
        assert!(
            !file(&corpus, other).contains("## Failure modes"),
            "no failure-modes on {other}"
        );
    }
}

#[test]
fn golden_corpus_snapshot() {
    let tmp = tempfile::tempdir().unwrap();
    standard_fixture(tmp.path());
    let corpus = corpus_over(tmp.path(), LinkStyle::Portable);

    let mut combined = String::new();
    for f in &corpus.files {
        combined.push_str(&format!("===== {} =====\n{}\n", f.path, f.contents));
    }
    insta::assert_snapshot!("phase_a_corpus", combined);
}
