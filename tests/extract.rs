//! U3 integration tests: filetree + git extractors over runtime git fixtures.

mod common;

use common::{init_repo_staged_no_commit, init_repo_with_files};
use reverse_architect::extract::test_pairing::{HAS_TEST, NO_TEST};
use reverse_architect::extract::{run_all, Repo};
use reverse_architect::graph::{
    FactKind, FactValue, Graph, NodeKey, NodeKind, Provenance, Resolution, Tier,
};

fn graph_over(dir: &std::path::Path) -> Graph {
    let repo = Repo::open(dir).unwrap();
    let mut graph = Graph::new();
    run_all(&repo, &mut graph).unwrap();
    graph
}

#[test]
fn filetree_builds_modules_files_and_contains_edges() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            ("src/lib.rs", "// lib"),
            ("src/extract/git.rs", "// git"),
            ("README.md", "# readme"),
        ],
    );

    let g = graph_over(tmp.path());

    assert_eq!(g.node(&NodeKey::new("src")).unwrap().kind, NodeKind::Module);
    assert_eq!(
        g.node(&NodeKey::new("src/extract")).unwrap().kind,
        NodeKind::Module
    );
    assert_eq!(
        g.node(&NodeKey::new("src/lib.rs")).unwrap().kind,
        NodeKind::File
    );
    assert_eq!(
        g.node(&NodeKey::new("README.md")).unwrap().kind,
        NodeKind::File
    );

    let has_edge = |from: &str, to: &str| {
        g.edges_sorted()
            .iter()
            .any(|e| e.from.as_str() == from && e.to.as_str() == to)
    };
    assert!(has_edge("src", "src/extract"));
    assert!(has_edge("src/extract", "src/extract/git.rs"));
    assert!(has_edge(".", "README.md"));
    assert!(has_edge(".", "src"));
}

#[test]
fn contains_edges_are_not_duplicated_across_sibling_files() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[("src/a.rs", "// a"), ("src/b.rs", "// b")],
    );

    let g = graph_over(tmp.path());

    // The ".-> src" edge must exist exactly once despite two files under src.
    let root_to_src = g
        .edges_sorted()
        .iter()
        .filter(|e| e.from.as_str() == "." && e.to.as_str() == "src")
        .count();
    assert_eq!(root_to_src, 1);
}

#[test]
fn a_repository_with_no_commit_at_all_still_enumerates_its_index() {
    // The corpus comes from what git TRACKS, and a freshly staged repository
    // tracks files before it has a single commit. Degradation, not an error
    // (E4): a run against one must still produce a corpus.
    let tmp = tempfile::tempdir().unwrap();
    init_repo_staged_no_commit(tmp.path(), &[("a.rs", "x")]);

    let g = graph_over(tmp.path());

    let node = g.node(&NodeKey::new("a.rs")).unwrap();
    assert_eq!(node.kind, NodeKind::File);
}

#[test]
fn gitignored_vendored_dirs_never_enter_the_graph() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[(".gitignore", "node_modules/\n"), ("src/app.rs", "// app")],
    );

    // A vendored dependency dir exists on disk but was never git-added.
    std::fs::create_dir_all(tmp.path().join("node_modules")).unwrap();
    std::fs::write(tmp.path().join("node_modules/pkg.js"), "x").unwrap();

    let g = graph_over(tmp.path());

    // KTD10: git-ignored paths are invisible to the tool entirely.
    assert!(g.node(&NodeKey::new("node_modules")).is_none());
    assert!(g.node(&NodeKey::new("node_modules/pkg.js")).is_none());
    // Tracked code is present.
    assert!(g.node(&NodeKey::new("src/app.rs")).is_some());
}

// ---- U7: metrics (tokei LOC) ----

/// The LOC fact on a file equals tokei's own total line count for that file.
#[test]
fn loc_fact_matches_tokei_line_count() {
    let tmp = tempfile::tempdir().unwrap();
    // Three code lines, one blank, one comment → tokei total lines = 5.
    let src = "fn main() {\n    let x = 1;\n\n    // note\n}\n";
    init_repo_with_files(tmp.path(), "Alice", &[("src/main.rs", src)]);

    // What tokei counts for this exact file, independently.
    let mut languages = tokei::Languages::new();
    languages.get_statistics(
        &[tmp.path().join("src/main.rs")],
        &[],
        &tokei::Config::default(),
    );
    let expected: usize = languages
        .values()
        .flat_map(|l| &l.reports)
        .map(|r| r.stats.lines())
        .sum();
    assert!(expected > 0, "fixture should have a non-zero line count");

    let g = graph_over(tmp.path());
    let node = g.node(&NodeKey::new("src/main.rs")).unwrap();
    assert!(
        node.facts
            .iter()
            .any(|f| f.kind == FactKind::Loc && f.value == FactValue::Int(expected as i64)),
        "file LOC fact must match tokei's own count ({expected})"
    );
}

/// Even a comment-only file carries a non-zero LOC fact — LOC is total physical
/// lines, not just code lines, so nothing that exists is silently zeroed.
#[test]
fn comment_only_file_still_has_a_nonzero_loc_fact() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(tmp.path(), "Alice", &[("src/lib.rs", "// just a comment")]);

    let g = graph_over(tmp.path());
    let node = g.node(&NodeKey::new("src/lib.rs")).unwrap();
    let loc = node
        .facts
        .iter()
        .find(|f| f.kind == FactKind::Loc)
        .expect("comment-only file still gets a LOC fact");
    assert_eq!(loc.value, FactValue::Int(1));
}

/// KTD10 regression: tokei is fed the tracked list, never a disk walk, so a
/// git-ignored `node_modules/` on disk yields no LOC facts for ignored paths.
#[test]
fn metrics_never_counts_gitignored_paths() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            (".gitignore", "node_modules/\n"),
            ("src/app.js", "const x = 1;\n"),
        ],
    );
    // A vendored dependency on disk, never git-added.
    std::fs::create_dir_all(tmp.path().join("node_modules")).unwrap();
    std::fs::write(
        tmp.path().join("node_modules/pkg.js"),
        "// lots\n// of\n// lines\n",
    )
    .unwrap();

    let g = graph_over(tmp.path());

    // The ignored file has no node at all, hence no LOC fact.
    assert!(g.node(&NodeKey::new("node_modules/pkg.js")).is_none());
    // No node anywhere carries a LOC fact whose provenance points into the
    // ignored tree.
    for node in g.nodes_sorted() {
        for fact in &node.facts {
            if fact.kind == FactKind::Loc {
                assert!(
                    !fact.provenance.file.starts_with("node_modules/"),
                    "LOC fact leaked from a git-ignored path: {}",
                    fact.provenance.file
                );
            }
        }
    }
    // The tracked source does carry one.
    assert!(g
        .node(&NodeKey::new("src/app.js"))
        .unwrap()
        .facts
        .iter()
        .any(|f| f.kind == FactKind::Loc));
}

/// A module's LOC is the derived sum of the files beneath it (recursive).
#[test]
fn module_loc_is_the_derived_sum_of_its_files() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            ("src/a.rs", "fn a() {}\n"),                  // 1 line
            ("src/inner/b.rs", "fn b() {}\nfn c() {}\n"), // 2 lines
        ],
    );

    let g = graph_over(tmp.path());
    let loc_of = |key: &str| {
        g.node(&NodeKey::new(key))
            .unwrap()
            .facts
            .iter()
            .find(|f| f.kind == FactKind::Loc)
            .map(|f| (f.value.clone(), f.provenance.tier))
    };

    let (a, _) = loc_of("src/a.rs").unwrap();
    let (b, _) = loc_of("src/inner/b.rs").unwrap();
    assert_eq!(a, FactValue::Int(1));
    assert_eq!(b, FactValue::Int(2));

    // src/inner = b (2); src = a + inner = 3; both are Derived facts.
    let (inner, inner_tier) = loc_of("src/inner").unwrap();
    let (src, src_tier) = loc_of("src").unwrap();
    assert_eq!(inner, FactValue::Int(2));
    assert_eq!(src, FactValue::Int(3));
    assert_eq!(inner_tier, reverse_architect::graph::Tier::Derived);
    assert_eq!(src_tier, reverse_architect::graph::Tier::Derived);
}

#[test]
fn a_path_that_became_a_directory_is_a_module_not_a_stale_file() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = init_repo_with_files(tmp.path(), "Alice", &[("x", "was a file")]);

    // "x" becomes a directory in the next commit.
    std::fs::remove_file(tmp.path().join("x")).unwrap();
    std::fs::create_dir(tmp.path().join("x")).unwrap();
    std::fs::write(tmp.path().join("x/y.rs"), "// y").unwrap();
    let mut index = repo.index().unwrap();
    index.remove_path(std::path::Path::new("x")).unwrap();
    index.add_path(std::path::Path::new("x/y.rs")).unwrap();
    index.write().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let sig = common::fixed_signature("Alice");
    let parent = repo.head().unwrap().peel_to_commit().unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "file to dir", &tree, &[&parent])
        .unwrap();

    let g = graph_over(tmp.path());
    let module = g.node(&NodeKey::new("x")).unwrap();

    // A path that WAS a file and is now a directory is a Module node, not a
    // File node carrying a stale kind from the shape it used to have.
    assert_eq!(module.kind, NodeKind::Module);
}

// ---- U7: test-source pairing + coverage ----

/// The `TestByConvention` fact text on a file node, if present.
fn pairing_of(g: &Graph, key: &str) -> Option<String> {
    g.node(&NodeKey::new(key))?.facts.iter().find_map(|f| {
        if f.kind == FactKind::TestByConvention {
            if let FactValue::Text(t) = &f.value {
                return Some(t.clone());
            }
        }
        None
    })
}

/// All `Coverage` fact texts on the root module node.
fn coverage_texts(g: &Graph) -> Vec<String> {
    g.node(&NodeKey::new("."))
        .unwrap()
        .facts
        .iter()
        .filter_map(|f| match (&f.kind, &f.value) {
            (FactKind::Coverage, FactValue::Text(t)) => Some(t.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn source_with_a_convention_test_is_paired() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            ("src/calc.rs", "pub fn calc() -> i32 { 1 }\n"),
            ("src/calc_test.rs", "fn t() { assert_eq!(1, 1); }\n"),
        ],
    );

    let g = graph_over(tmp.path());
    // The statement NAMES the test. "A test exists somewhere" would leave the
    // reader with the question they arrived with, and the pairing knows the
    // answer at the moment it decides.
    assert_eq!(
        pairing_of(&g, "src/calc.rs").as_deref(),
        Some(format!("{HAS_TEST}: `src/calc_test.rs`").as_str())
    );
    // The test file itself is not a "source" and carries no pairing fact.
    assert!(pairing_of(&g, "src/calc_test.rs").is_none());
}

#[test]
fn several_tests_sharing_one_stem_name_the_first_and_count_the_rest() {
    // `FooTests.cs` beside `FooTest.cs`, or the same name under two directories.
    // The statement must stay one line at any repository size (R17) and must not
    // silently drop the ones it did not name.
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            ("src/calc.rs", "pub fn calc() -> i32 { 1 }\n"),
            ("src/calc_test.rs", "fn a() {}\n"),
            ("tests/calc_test.rs", "fn b() {}\n"),
        ],
    );

    let g = graph_over(tmp.path());
    let statement = pairing_of(&g, "src/calc.rs").expect("a pairing");
    assert_eq!(
        statement,
        format!("{HAS_TEST}: `src/calc_test.rs`, and 1 more sharing its name"),
        "the first in SORTED order is named, so two runs agree (R18)"
    );
}

#[test]
fn source_without_a_test_says_not_found_never_untested() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[("src/lonely.rs", "pub fn lonely() {}\n")],
    );

    let g = graph_over(tmp.path());
    let text = pairing_of(&g, "src/lonely.rs").unwrap();
    // Exact wording is a contract (E9/R18).
    assert_eq!(text, "test by convention not found");
    assert!(!text.contains("untested"));
    assert!(!text.contains("no test"));
}

#[test]
fn test_in_a_nonconventional_location_is_not_recognized_and_coverage_admits_it() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            ("src/thing.rs", "pub fn thing() {}\n"),
            // A "test" under /spec with a name no convention matches: it is not
            // recognized as a test, so `thing.rs` shows "not found".
            ("spec/thing_check.rs", "pub fn check() {}\n"),
        ],
    );

    let g = graph_over(tmp.path());
    assert_eq!(pairing_of(&g, "src/thing.rs").as_deref(), Some(NO_TEST));
    // The /spec file is itself an (unrecognized) source, also not found.
    assert_eq!(
        pairing_of(&g, "spec/thing_check.rs").as_deref(),
        Some(NO_TEST)
    );

    // Coverage renders a real, honest number and never claims "untested".
    let cov = coverage_texts(&g);
    let rust = cov.iter().find(|c| c.starts_with("rust:")).unwrap();
    assert!(rust.contains("2 source files"));
    assert!(!rust.contains("untested"));
}

#[test]
fn zero_test_project_marks_every_source_not_found_with_a_real_coverage_number() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            ("src/a.rs", "pub fn a() {}\n"),     // has a symbol → structural
            ("src/b.rs", "// only a comment\n"), // no symbol → metrics+git only
        ],
    );

    let g = graph_over(tmp.path());
    assert_eq!(pairing_of(&g, "src/a.rs").as_deref(), Some(NO_TEST));
    assert_eq!(pairing_of(&g, "src/b.rs").as_deref(), Some(NO_TEST));

    // Coverage is a real number, and exercises both branches: 1 of 2 sources
    // structural, 1 with only metrics + file tree.
    let cov = coverage_texts(&g);
    let rust = cov.iter().find(|c| c.starts_with("rust:")).unwrap();
    assert!(
        rust.contains("1 of 2 source files carry tree-sitter structural facts"),
        "coverage was: {rust}"
    );
    assert!(
        rust.contains("1 has only metrics + file tree"),
        "coverage was: {rust}"
    );
}

#[test]
fn a_tests_dir_mod_rs_does_not_falsely_pair_source_modules() {
    // Regression: `tests/common/mod.rs` must not register test-stem `mod` and
    // make every `src/**/mod.rs` read "has a test by convention" (a false
    // positive, the inverse of the E9/R18 honesty rule).
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            ("src/engine/mod.rs", "pub fn run() {}\n"),
            ("tests/common/mod.rs", "pub fn helper() {}\n"),
        ],
    );

    let g = graph_over(tmp.path());
    // The source module is plumbing → no pairing fact at all (not a false
    // "has a test", and not a spurious "not found" either).
    assert!(pairing_of(&g, "src/engine/mod.rs").is_none());
}

#[test]
fn pairing_and_coverage_are_identical_across_two_runs() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            ("src/calc.rs", "pub fn calc() {}\n"),
            ("src/calc_test.rs", "fn t() {}\n"),
            ("pkg/mod.py", "def f():\n    pass\n"),
        ],
    );

    let facts = |g: &Graph| -> Vec<(String, FactKind, FactValue)> {
        let mut out = Vec::new();
        for node in g.nodes_sorted() {
            for f in &node.facts {
                if matches!(f.kind, FactKind::TestByConvention | FactKind::Coverage) {
                    out.push((node.key.as_str().to_string(), f.kind, f.value.clone()));
                }
            }
        }
        out
    };

    let a = facts(&graph_over(tmp.path()));
    let b = facts(&graph_over(tmp.path()));
    assert_eq!(a, b);
    assert!(!a.is_empty());
}

// U6: tree-sitter extractor end-to-end (tracked list → disk read → parse).

#[test]
fn treesitter_emits_symbols_and_entry_point_over_a_real_repo() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            ("src/main.rs", "fn main() {}\nstruct Cfg;\n"),
            ("README.md", "# readme"), // no grammar → skipped (E4)
        ],
    );

    let g = graph_over(tmp.path());

    // Symbols surfaced from the actual file content on disk.
    let main = g.node(&NodeKey::new("sym:rust:crate#main()")).unwrap();
    assert_eq!(main.kind, NodeKind::Symbol);
    assert!(main
        .facts
        .iter()
        .any(|f| f.kind == FactKind::EntryPoint && f.value == FactValue::Bool(true)));
    assert_eq!(
        g.node(&NodeKey::new("sym:rust:crate#Cfg")).unwrap().kind,
        NodeKind::Symbol
    );

    // file --Declares--> symbol edge exists (distinct from module Contains, so
    // module-level consumers never pull in symbol edges).
    assert!(g.edges_sorted().iter().any(|e| {
        e.from.as_str() == "src/main.rs"
            && e.to.as_str() == "sym:rust:crate#main()"
            && e.edge.kind == EdgeKind::Declares
    }));

    // The markdown file has no grammar → no symbol nodes derived from it. Checked
    // through provenance, not through the key: a symbol identity is rooted at its
    // module path (KTD3), so the source file no longer appears in the key at all.
    assert!(g
        .nodes_sorted()
        .iter()
        .filter(|n| n.kind == NodeKind::Symbol)
        .all(|n| n.facts.iter().all(|f| f.provenance.file != "README.md")));
}

#[test]
fn treesitter_degrades_when_no_grammar_matches() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[("notes.txt", "hello"), ("data.json", "{}")],
    );
    let g = graph_over(tmp.path());
    // No supported grammar → no Symbol nodes, but the run still succeeds and
    // filetree/git facts remain.
    assert!(g.nodes_sorted().iter().all(|n| n.kind != NodeKind::Symbol));
    assert!(g.node(&NodeKey::new("notes.txt")).is_some());
}

use reverse_architect::graph::EdgeKind;

/// A small polyglot repo exercising imports (TS + Python), entry points
/// (Go `main`, Python `__main__`, none for TS), and no-phantom discipline.
fn polyglot_fixture(dir: &std::path::Path) {
    init_repo_with_files(
        dir,
        "Alice",
        &[
            // TS: internal relative import + external npm package.
            (
                "web/a.ts",
                "import { b } from './b';\nimport x from 'lodash';\nexport function main() {}\n",
            ),
            ("web/b.ts", "export const b = 1;\n"),
            // Python: relative import + stdlib import + __main__ guard.
            (
                "app/a.py",
                "from .b import thing\nimport os\nif __name__ == '__main__':\n    thing()\n",
            ),
            ("app/b.py", "thing = 1\n"),
            // Go: top-level func main.
            ("cmd/main.go", "package main\nfunc main() {}\n"),
        ],
    );
}

fn has_import_edge(g: &Graph, from: &str, to: &str) -> bool {
    g.edges_sorted()
        .iter()
        .any(|e| e.from.as_str() == from && e.to.as_str() == to && e.edge.kind == EdgeKind::Imports)
}

#[test]
fn treesitter_imports_resolve_only_to_tracked_nodes() {
    let tmp = tempfile::tempdir().unwrap();
    polyglot_fixture(tmp.path());
    let g = graph_over(tmp.path());

    // Internal imports become edges between the two tracked file nodes.
    assert!(has_import_edge(&g, "web/a.ts", "web/b.ts"));
    assert!(has_import_edge(&g, "app/a.py", "app/b.py"));

    // External imports fabricate no phantom target node and no dangling edge.
    assert!(g.node(&NodeKey::new("lodash")).is_none());
    assert!(g.node(&NodeKey::new("os")).is_none());
    let import_targets: Vec<&str> = g
        .edges_sorted()
        .iter()
        .filter(|e| e.edge.kind == EdgeKind::Imports)
        .map(|e| e.to.as_str())
        .collect();
    assert_eq!(import_targets, vec!["app/b.py", "web/b.ts"]);
}

#[test]
fn treesitter_entry_points_are_honest_across_languages() {
    let tmp = tempfile::tempdir().unwrap();
    polyglot_fixture(tmp.path());
    let g = graph_over(tmp.path());

    let is_entry = |key: &str| {
        g.node(&NodeKey::new(key))
            .map(|n| {
                n.facts
                    .iter()
                    .any(|f| f.kind == FactKind::EntryPoint && f.value == FactValue::Bool(true))
            })
            .unwrap_or(false)
    };

    // Go `func main` and the Python `__main__` guard are entry points.
    assert!(is_entry("sym:go:cmd#main()"));
    assert!(is_entry("sym:python:app.a#__main__"));

    // TS has no language-level entry point — even a function named `main`
    // (its entry is the manifest main/bin, U8). This is correct, not a gap.
    assert!(g.node(&NodeKey::new("sym:ts:web/a.ts#main()")).is_some());
    assert!(!is_entry("sym:ts:web/a.ts#main()"));
}

/// U2 end-to-end, over a real repo, one type per supported language: every
/// grammar must produce the type, its members, and the `HasMember` edges joining
/// them (R1, KTD5).
#[test]
fn treesitter_members_are_extracted_for_every_supported_language() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            (
                "src/lib.rs",
                "/// Konfigurace.
pub struct Cfg {
    pub rate: u32,
}

impl Cfg {
    pub fn run(&self) -> bool {
        true
    }
}
",
            ),
            (
                "web/cfg.ts",
                "export class Cfg {
  private rate: number = 1;
  public run(a: number): void {}
}
",
            ),
            (
                "app/cfg.py",
                "class Cfg:
    \"\"\"Konfigurace.\"\"\"
    def run(self):
        pass
",
            ),
            (
                "Cfg.cs",
                "namespace N;
public class Cfg {
  public int Rate { get; set; }
  public bool Run() { return true; }
}
",
            ),
            (
                "cmd/cfg.go",
                "package cmd
type Cfg struct {
	Rate int
}
func (c Cfg) Run() bool { return true }
",
            ),
        ],
    );

    let g = graph_over(tmp.path());
    let members = |owner: &str| {
        let mut out: Vec<String> = g
            .edges_sorted()
            .iter()
            .filter(|e| e.edge.kind == EdgeKind::HasMember && e.from.as_str() == owner)
            .map(|e| e.to.as_str().to_string())
            .collect();
        out.sort();
        out
    };

    assert_eq!(
        members("sym:rust:crate#Cfg"),
        vec![
            "sym:rust:crate#Cfg.rate".to_string(),
            "sym:rust:crate#Cfg.run(&self)".to_string(),
        ]
    );
    assert_eq!(
        members("sym:ts:web/cfg.ts#Cfg"),
        vec![
            "sym:ts:web/cfg.ts#Cfg.rate".to_string(),
            "sym:ts:web/cfg.ts#Cfg.run(number)".to_string(),
        ]
    );
    assert_eq!(
        members("sym:python:app.cfg#Cfg"),
        vec!["sym:python:app.cfg#Cfg.run(self)".to_string()]
    );
    assert_eq!(
        members("sym:csharp:N#Cfg"),
        vec![
            "sym:csharp:N#Cfg.Rate".to_string(),
            "sym:csharp:N#Cfg.Run()".to_string(),
        ]
    );
    assert_eq!(
        members("sym:go:cmd#Cfg"),
        vec![
            "sym:go:cmd#Cfg.Rate".to_string(),
            "sym:go:cmd#Cfg.Run()".to_string(),
        ]
    );

    // Doc comments survive the round trip through the git blob verbatim.
    let doc = |key: &str| {
        g.node(&NodeKey::new(key)).and_then(|n| {
            n.facts
                .iter()
                .find(|f| f.kind == FactKind::DocComment)
                .and_then(|f| match &f.value {
                    FactValue::Text(t) => Some(t.clone()),
                    _ => None,
                })
        })
    };
    assert_eq!(doc("sym:rust:crate#Cfg").as_deref(), Some("Konfigurace."));
    assert_eq!(
        doc("sym:python:app.cfg#Cfg").as_deref(),
        Some("Konfigurace.")
    );
}

#[test]
fn treesitter_output_is_byte_identical_across_two_runs() {
    let tmp = tempfile::tempdir().unwrap();
    polyglot_fixture(tmp.path());

    let dump = |g: &Graph| {
        let nodes = serde_json::to_string(&g.nodes_sorted()).unwrap();
        let edges: Vec<_> = g
            .edges_sorted()
            .iter()
            .map(|e| (e.from.clone(), e.to.clone(), e.edge.kind))
            .collect();
        (nodes, serde_json::to_string(&edges).unwrap())
    };

    let g1 = graph_over(tmp.path());
    let g2 = graph_over(tmp.path());
    assert_eq!(dump(&g1), dump(&g2));
}

// ---- U3: type relations (extends / implements) ----

/// A polyglot repo whose inheritance is spread ACROSS files, so a base type is
/// routinely declared after (or before) the type deriving from it. Nothing here
/// may depend on the order the extractor happens to visit files in.
fn inheritance_fixture(dir: &std::path::Path) {
    init_repo_with_files(
        dir,
        "Alice",
        &[
            // C#: one namespace, one type per file. `Derived` names its base and
            // two interfaces in a base list the syntax does not disambiguate.
            (
                "Derived.cs",
                "namespace N;\npublic class Derived : Base, IFoo, IBar {}\n",
            ),
            ("Base.cs", "namespace N;\npublic class Base {}\n"),
            ("IFoo.cs", "namespace N;\npublic interface IFoo {}\n"),
            ("IBar.cs", "namespace N;\npublic interface IBar {}\n"),
            // An interface inheriting an interface EXTENDS it; it does not
            // implement it.
            ("IBaz.cs", "namespace N;\npublic interface IBaz : IFoo {}\n"),
            // A base outside the repo, and an enum's underlying type — which is
            // spelled like a base list but is not inheritance at all.
            ("Ext.cs", "namespace N;\npublic class Ext : System.Exception {}\n"),
            ("E.cs", "namespace N;\npublic enum E : byte { X }\n"),
            // Rust: `impl Trait for Type` implements, a bare `impl Type` does
            // not, a supertrait extends, and a std trait resolves to nothing.
            (
                "src/shapes.rs",
                "pub trait Shape {}\npub trait Solid: Shape {}\npub struct Sq;\nimpl Shape for Sq {}\nimpl Sq {\n    pub fn side(&self) -> u32 {\n        1\n    }\n}\nimpl std::fmt::Debug for Sq {\n    fn fmt(&self, _f: &mut std::fmt::Formatter) -> std::fmt::Result {\n        Ok(())\n    }\n}\n",
            ),
            // TS: a module IS its file, so heritage resolves within one file.
            (
                "web/shapes.ts",
                "export class Base {}\nexport interface IFoo {}\nexport interface IBar {}\nexport class Derived extends Base implements IFoo, IBar {}\nexport interface IBaz extends IFoo {}\n",
            ),
            // TS across files: the base is imported, so its identity lives in
            // another module. The module-local lookup cannot reach it; the
            // repository-wide fallback can, because `Base` is the only type of
            // that name the TypeScript/JavaScript family declares.
            (
                "web/other.ts",
                "import { Base } from './shapes';\nexport class Far extends Base {}\n",
            ),
            // Python: single and multiple inheritance, plus a builtin base.
            (
                "app/shapes.py",
                "class Base:\n    pass\n\n\nclass Mixin:\n    pass\n\n\nclass Derived(Base, Mixin):\n    pass\n\n\nclass Ext(Exception):\n    pass\n",
            ),
            // Go: struct embedding and interface embedding — neither is
            // inheritance, so this file must contribute no relation at all.
            (
                "cmd/shapes.go",
                "package cmd\n\ntype Base struct {\n\tX int\n}\n\ntype Emb struct {\n\tBase\n\tY int\n}\n\ntype I interface {\n\tM()\n}\n\ntype J interface {\n\tI\n\tN()\n}\n",
            ),
        ],
    );
}

/// Sorted `from -> to` pairs of one relation kind.
fn relations(g: &Graph, kind: EdgeKind) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = g
        .edges_sorted()
        .iter()
        .filter(|e| e.edge.kind == kind)
        .map(|e| (e.from.as_str().to_string(), e.to.as_str().to_string()))
        .collect();
    out.sort();
    out
}

/// The base class named in another FILE of the same C# namespace must still be
/// found: relations resolve after the whole repo is walked, never against the
/// files seen so far.
#[test]
fn csharp_base_class_in_another_file_is_an_extends_edge() {
    let tmp = tempfile::tempdir().unwrap();
    inheritance_fixture(tmp.path());
    let g = graph_over(tmp.path());

    assert!(relations(&g, EdgeKind::Extends).contains(&(
        "sym:csharp:N#Derived".to_string(),
        "sym:csharp:N#Base".to_string(),
    )));
}

/// A C# base list writes a base class and an interface identically. The kind of
/// each relation therefore comes from what the target turns out to BE, never from
/// an `I`-prefix naming convention.
#[test]
fn csharp_two_interfaces_yield_two_implements_edges() {
    let tmp = tempfile::tempdir().unwrap();
    inheritance_fixture(tmp.path());
    let g = graph_over(tmp.path());

    let from_derived: Vec<(String, String)> = relations(&g, EdgeKind::Implements)
        .into_iter()
        .filter(|(from, _)| from == "sym:csharp:N#Derived")
        .collect();
    assert_eq!(
        from_derived,
        vec![
            (
                "sym:csharp:N#Derived".to_string(),
                "sym:csharp:N#IBar".to_string()
            ),
            (
                "sym:csharp:N#Derived".to_string(),
                "sym:csharp:N#IFoo".to_string()
            ),
        ]
    );

    // An interface inheriting an interface extends it — it does not implement it.
    assert!(relations(&g, EdgeKind::Extends).contains(&(
        "sym:csharp:N#IBaz".to_string(),
        "sym:csharp:N#IFoo".to_string(),
    )));
    assert!(!relations(&g, EdgeKind::Implements)
        .iter()
        .any(|(from, _)| from == "sym:csharp:N#IBaz"));
}

/// `impl Trait for Type` is an implements relation; a bare inherent `impl Type`
/// is not a relation at all, and a supertrait bound extends.
#[test]
fn rust_trait_impl_implements_and_inherent_impl_does_not() {
    let tmp = tempfile::tempdir().unwrap();
    inheritance_fixture(tmp.path());
    let g = graph_over(tmp.path());

    assert_eq!(
        relations(&g, EdgeKind::Implements)
            .into_iter()
            .filter(|(from, _)| from.starts_with("sym:rust:"))
            .collect::<Vec<_>>(),
        vec![(
            "sym:rust:crate::shapes#Sq".to_string(),
            "sym:rust:crate::shapes#Shape".to_string()
        )]
    );

    // The inherent `impl Sq` block declares its method but no relation, so `Sq`
    // is neither side of an extends relation.
    assert!(relations(&g, EdgeKind::Extends).iter().all(|(from, to)| {
        from != "sym:rust:crate::shapes#Sq" && to != "sym:rust:crate::shapes#Sq"
    }));

    // A supertrait bound is inheritance between traits.
    assert!(relations(&g, EdgeKind::Extends).contains(&(
        "sym:rust:crate::shapes#Solid".to_string(),
        "sym:rust:crate::shapes#Shape".to_string(),
    )));
}

/// TS heritage within a module, and Python single + multiple inheritance.
#[test]
fn typescript_and_python_heritage_become_relations() {
    let tmp = tempfile::tempdir().unwrap();
    inheritance_fixture(tmp.path());
    let g = graph_over(tmp.path());

    let extends = relations(&g, EdgeKind::Extends);
    assert!(extends.contains(&(
        "sym:ts:web/shapes.ts#Derived".to_string(),
        "sym:ts:web/shapes.ts#Base".to_string(),
    )));
    assert!(extends.contains(&(
        "sym:ts:web/shapes.ts#IBaz".to_string(),
        "sym:ts:web/shapes.ts#IFoo".to_string(),
    )));
    assert_eq!(
        relations(&g, EdgeKind::Implements)
            .into_iter()
            .filter(|(from, _)| from.starts_with("sym:ts:"))
            .collect::<Vec<_>>(),
        vec![
            (
                "sym:ts:web/shapes.ts#Derived".to_string(),
                "sym:ts:web/shapes.ts#IBar".to_string()
            ),
            (
                "sym:ts:web/shapes.ts#Derived".to_string(),
                "sym:ts:web/shapes.ts#IFoo".to_string()
            ),
        ]
    );

    // Python multiple inheritance is two relations, one per base.
    assert_eq!(
        extends
            .iter()
            .filter(|(from, _)| from == "sym:python:app.shapes#Derived")
            .cloned()
            .collect::<Vec<_>>(),
        vec![
            (
                "sym:python:app.shapes#Derived".to_string(),
                "sym:python:app.shapes#Base".to_string()
            ),
            (
                "sym:python:app.shapes#Derived".to_string(),
                "sym:python:app.shapes#Mixin".to_string()
            ),
        ]
    );
}

/// A base type the repo does not declare — `System.Exception`, `std::fmt::Debug`,
/// Python's `Exception` — resolves to nothing, and no phantom target node is
/// invented for it (U5's job). An enum's underlying type is spelled like a base
/// list and is not inheritance either.
///
/// A base the repo DOES declare in another module resolves when it is the only
/// type of that name in its language family: `Far` extends the `Base` that
/// `web/shapes.ts` declares. (Until 29.09.2026 this test listed `Far` as
/// unresolved — the module-local lookup could not see across files, which left
/// most real-world inheritance unlinked.)
#[test]
fn base_type_outside_the_repo_emits_no_edge_and_no_phantom_node() {
    let tmp = tempfile::tempdir().unwrap();
    inheritance_fixture(tmp.path());
    let g = graph_over(tmp.path());

    let unresolved = [
        "sym:csharp:N#Ext",          // : System.Exception
        "sym:csharp:N#E",            // enum : byte
        "sym:python:app.shapes#Ext", // (Exception)
    ];
    for from in unresolved {
        // The deriving type itself exists — only the relation is missing.
        assert!(
            g.node(&NodeKey::new(from)).is_some(),
            "{from} not extracted"
        );
        for kind in [EdgeKind::Extends, EdgeKind::Implements] {
            assert!(
                !relations(&g, kind).iter().any(|(f, _)| f == from),
                "{from} must have no {kind:?} edge"
            );
        }
    }

    assert!(
        relations(&g, EdgeKind::Extends).contains(&(
            "sym:ts:web/other.ts#Far".to_string(),
            "sym:ts:web/shapes.ts#Base".to_string()
        )),
        "an imported base the repository declares once resolves across files"
    );

    // Nothing was fabricated for the unresolvable bases.
    for phantom in [
        "sym:csharp:System#Exception",
        "sym:csharp:N#System.Exception",
        "sym:rust:std::fmt#Debug",
        "sym:python:app.shapes#Exception",
        "sym:ts:web/other.ts#Base",
    ] {
        assert!(
            g.node(&NodeKey::new(phantom)).is_none(),
            "{phantom} was fabricated"
        );
    }

    // Every relation that IS emitted points at a node that exists.
    for kind in [EdgeKind::Extends, EdgeKind::Implements] {
        for (from, to) in relations(&g, kind) {
            assert!(g.node(&NodeKey::new(&from)).is_some(), "{from} missing");
            assert!(g.node(&NodeKey::new(&to)).is_some(), "{to} missing");
        }
    }
}

/// Go declares neither inheritance nor implementation: struct embedding is
/// composition, interface embedding is interface composition, and which types
/// satisfy an interface is decidable only with type information. So Go source
/// contributes no relation at all.
#[test]
fn go_embedding_is_not_inheritance() {
    let tmp = tempfile::tempdir().unwrap();
    inheritance_fixture(tmp.path());
    let g = graph_over(tmp.path());

    for kind in [EdgeKind::Extends, EdgeKind::Implements] {
        assert!(
            relations(&g, kind)
                .iter()
                .all(|(from, _)| !from.starts_with("sym:go:")),
            "Go produced a {kind:?} edge"
        );
    }
}

/// KD2/R20: a name-matched relation is `Heuristic` precision and never claims
/// more, and it is anchored to the line the base type is written on.
#[test]
fn type_relations_are_heuristic_and_line_anchored() {
    let tmp = tempfile::tempdir().unwrap();
    inheritance_fixture(tmp.path());
    let g = graph_over(tmp.path());

    let all: Vec<(String, Provenance)> = g
        .edges_sorted()
        .iter()
        .filter(|e| matches!(e.edge.kind, EdgeKind::Extends | EdgeKind::Implements))
        .map(|e| (e.from.as_str().to_string(), e.edge.provenance.clone()))
        .collect();
    assert!(!all.is_empty());
    for (from, prov) in &all {
        assert_eq!(
            prov.resolution,
            Resolution::Heuristic,
            "{from} claims more than a name match"
        );
        assert_eq!(prov.tier, Tier::Measured);
        assert!(prov.span.start().is_some(), "{from} has no source line");
    }

    // `public class Derived : Base, IFoo, IBar {}` is line 2 of Derived.cs, and
    // all three of its relations are written there.
    let derived: Vec<(String, Option<u32>)> = all
        .iter()
        .filter(|(from, _)| from == "sym:csharp:N#Derived")
        .map(|(_, prov)| (prov.file.clone(), prov.span.start()))
        .collect();
    assert_eq!(
        derived,
        vec![
            ("Derived.cs".to_string(), Some(2)),
            ("Derived.cs".to_string(), Some(2)),
            ("Derived.cs".to_string(), Some(2)),
        ]
    );
}

/// The same repo twice yields identical relations — the run-level deferral must
/// not leak the order files were visited in.
#[test]
fn type_relations_are_identical_across_two_runs() {
    let tmp = tempfile::tempdir().unwrap();
    inheritance_fixture(tmp.path());

    let dump = |g: &Graph| {
        serde_json::to_string(
            &g.edges_sorted()
                .iter()
                .filter(|e| matches!(e.edge.kind, EdgeKind::Extends | EdgeKind::Implements))
                .map(|e| {
                    (
                        e.from.clone(),
                        e.to.clone(),
                        e.edge.kind,
                        e.edge.provenance.clone(),
                    )
                })
                .collect::<Vec<_>>(),
        )
        .unwrap()
    };

    assert_eq!(dump(&graph_over(tmp.path())), dump(&graph_over(tmp.path())));
}

// ---- U8: manifest dependency parsers ----

/// True if a `DependsOn` edge `from → to` exists.
fn has_depends_on(g: &Graph, from: &str, to: &str) -> bool {
    g.edges_sorted().iter().any(|e| {
        e.from.as_str() == from && e.to.as_str() == to && e.edge.kind == EdgeKind::DependsOn
    })
}

/// The `DependencyScope` text values recorded on a dependency node, sorted.
fn dep_scopes(g: &Graph, key: &str) -> Vec<String> {
    let mut scopes: Vec<String> = g
        .node(&NodeKey::new(key))
        .map(|n| {
            n.facts
                .iter()
                .filter(|f| f.kind == FactKind::DependencyScope)
                .filter_map(|f| match &f.value {
                    FactValue::Text(s) => Some(s.clone()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    scopes.sort();
    scopes
}

/// The `EntryPointDecl` text values on a node, sorted.
fn entry_decls(g: &Graph, key: &str) -> Vec<String> {
    let mut decls: Vec<String> = g
        .node(&NodeKey::new(key))
        .map(|n| {
            n.facts
                .iter()
                .filter(|f| f.kind == FactKind::EntryPointDecl)
                .filter_map(|f| match &f.value {
                    FactValue::Text(s) => Some(s.clone()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    decls.sort();
    decls
}

#[test]
fn cargo_manifest_yields_dependency_edges_with_dev_flagged() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[(
            "Cargo.toml",
            "[package]\nname = \"demo\"\n\n[dependencies]\nserde = \"1\"\n\n[dev-dependencies]\ntempfile = \"3\"\n",
        )],
    );
    let g = graph_over(tmp.path());

    // A Dependency node + a DependsOn edge from the manifest per dependency.
    assert_eq!(
        g.node(&NodeKey::new("dep:cargo:serde")).unwrap().kind,
        NodeKind::Dependency
    );
    assert!(has_depends_on(&g, "Cargo.toml", "dep:cargo:serde"));
    assert!(has_depends_on(&g, "Cargo.toml", "dep:cargo:tempfile"));

    // Dev vs normal is distinguished on the scope fact.
    assert_eq!(dep_scopes(&g, "dep:cargo:serde"), vec!["normal"]);
    assert_eq!(dep_scopes(&g, "dep:cargo:tempfile"), vec!["dev"]);
}

#[test]
fn package_json_manifest_yields_deps_and_entry_points() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[(
            "package.json",
            r#"{"name":"web","dependencies":{"react":"^18"},"devDependencies":{"jest":"^29"},"main":"index.js","bin":{"cli":"bin/cli.js"}}"#,
        )],
    );
    let g = graph_over(tmp.path());

    assert!(has_depends_on(&g, "package.json", "dep:npm:react"));
    assert!(has_depends_on(&g, "package.json", "dep:npm:jest"));
    assert_eq!(dep_scopes(&g, "dep:npm:react"), vec!["normal"]);
    assert_eq!(dep_scopes(&g, "dep:npm:jest"), vec!["dev"]);

    // `main`/`bin` land as entry-point declarations on the manifest file node.
    let decls = entry_decls(&g, "package.json");
    assert!(decls.contains(&"main:index.js".to_string()));
    assert!(decls.contains(&"bin:cli=bin/cli.js".to_string()));
}

#[test]
fn go_mod_manifest_yields_module_dependency_edges() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[(
            "go.mod",
            "module github.com/me/proj\n\ngo 1.21\n\nrequire (\n\tgithub.com/pkg/errors v0.9.1\n)\n\nrequire github.com/spf13/cobra v1.8.0\n",
        )],
    );
    let g = graph_over(tmp.path());

    assert!(has_depends_on(&g, "go.mod", "dep:go:github.com/pkg/errors"));
    assert!(has_depends_on(
        &g,
        "go.mod",
        "dep:go:github.com/spf13/cobra"
    ));
    // The module's own path is never a dependency.
    assert!(g.node(&NodeKey::new("dep:go:github.com/me/proj")).is_none());
}

#[test]
fn csproj_manifest_output_type_is_an_entry_point_fact() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[(
            "App.csproj",
            "<Project Sdk=\"Microsoft.NET.Sdk\">\n  <PropertyGroup>\n    <OutputType>Exe</OutputType>\n  </PropertyGroup>\n  <ItemGroup>\n    <PackageReference Include=\"Newtonsoft.Json\" Version=\"13.0.3\" />\n  </ItemGroup>\n</Project>\n",
        )],
    );
    let g = graph_over(tmp.path());

    assert!(has_depends_on(
        &g,
        "App.csproj",
        "dep:nuget:Newtonsoft.Json"
    ));
    assert_eq!(entry_decls(&g, "App.csproj"), vec!["output_type:Exe"]);
}

#[test]
fn malformed_manifest_is_skipped_but_others_in_the_same_repo_still_produce_edges() {
    // E4: one broken package.json must not suppress a valid Cargo.toml / go.mod
    // in the SAME repo.
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            ("package.json", "{ this is not : valid json ,,,"),
            ("Cargo.toml", "[dependencies]\nserde = \"1\"\n"),
            ("go.mod", "module x\nrequire github.com/pkg/errors v0.9.1\n"),
        ],
    );
    let g = graph_over(tmp.path());

    // The broken manifest yielded no npm dependency node.
    assert!(g.node(&NodeKey::new("dep:npm:react")).is_none());
    assert_eq!(
        g.edges_sorted()
            .iter()
            .filter(|e| e.from.as_str() == "package.json" && e.edge.kind == EdgeKind::DependsOn)
            .count(),
        0
    );
    // The other ecosystems are unaffected.
    assert!(has_depends_on(&g, "Cargo.toml", "dep:cargo:serde"));
    assert!(has_depends_on(&g, "go.mod", "dep:go:github.com/pkg/errors"));
}

#[test]
fn dependency_node_does_not_collide_with_a_module_of_the_same_name() {
    // Key-space regression against the ensure_node kind-guard (KTD-B1): a source
    // module literally named `serde` and a `dep:cargo:serde` dependency must be
    // distinct nodes with their own kinds — no panic in this debug/test build.
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            ("serde/lib.rs", "// a module named serde"),
            ("Cargo.toml", "[dependencies]\nserde = \"1\"\n"),
        ],
    );
    let g = graph_over(tmp.path());

    assert_eq!(
        g.node(&NodeKey::new("serde")).unwrap().kind,
        NodeKind::Module
    );
    assert_eq!(
        g.node(&NodeKey::new("dep:cargo:serde")).unwrap().kind,
        NodeKind::Dependency
    );
}

#[test]
fn manifest_deps_output_is_byte_identical_across_two_runs() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            (
                "Cargo.toml",
                "[dependencies]\nserde = \"1\"\npetgraph = \"0.6\"\n[dev-dependencies]\ntempfile = \"3\"\n",
            ),
            (
                "package.json",
                r#"{"dependencies":{"react":"^18"},"main":"index.js"}"#,
            ),
        ],
    );

    let dump = |g: &Graph| {
        let nodes = serde_json::to_string(&g.nodes_sorted()).unwrap();
        let edges: Vec<_> = g
            .edges_sorted()
            .iter()
            .filter(|e| e.edge.kind == EdgeKind::DependsOn)
            .map(|e| (e.from.clone(), e.to.clone()))
            .collect();
        (nodes, serde_json::to_string(&edges).unwrap())
    };

    let g1 = graph_over(tmp.path());
    let g2 = graph_over(tmp.path());
    assert_eq!(dump(&g1), dump(&g2));
}

#[test]
fn a_gitignored_manifest_is_never_parsed() {
    // KTD10: a git-ignored `node_modules/package.json` on disk must never be
    // dispatched — manifest-deps reads only the tracked file list, never walks
    // the tree. Mirrors `metrics_never_counts_gitignored_paths`.
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            (".gitignore", "node_modules/\n"),
            ("Cargo.toml", "[dependencies]\nserde = \"1\"\n"),
        ],
    );
    // A vendored package manifest on disk, never git-added.
    std::fs::create_dir_all(tmp.path().join("node_modules/vendored")).unwrap();
    std::fs::write(
        tmp.path().join("node_modules/vendored/package.json"),
        r#"{"dependencies":{"leftpad":"1.0.0"}}"#,
    )
    .unwrap();

    let g = graph_over(tmp.path());

    // The ignored manifest's dependency never entered the graph.
    assert!(g.node(&NodeKey::new("dep:npm:leftpad")).is_none());
    // The tracked Cargo.toml still produced its edge.
    assert!(has_depends_on(&g, "Cargo.toml", "dep:cargo:serde"));
}

/// FIX 1 regression: a tracked file whose name contains a literal `%` must
/// still get its content facts. Its graph key is percent-escaped
/// (`a%main.rs` → `a%25main.rs`), but the content extractors must read the REAL
/// on-disk path — not the escaped key — or the File node and git facts survive
/// while LOC, symbols, and the entry point silently vanish (split-brain).
#[test]
fn percent_named_file_gets_content_facts_under_its_escaped_key() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(tmp.path(), "Alice", &[("a%main.rs", "fn main() {}\n")]);

    let g = graph_over(tmp.path());

    // filetree keys the File node under the byte-preserving (escaped) key.
    let file_key = "a%25main.rs";
    assert_eq!(
        g.node(&NodeKey::new(file_key)).unwrap().kind,
        NodeKind::File
    );

    // metrics: the LOC fact is present — absent if content were read from the
    // escaped key (a non-existent path).
    assert!(
        g.node(&NodeKey::new(file_key))
            .unwrap()
            .facts
            .iter()
            .any(|f| f.kind == FactKind::Loc),
        "metrics LOC fact missing — content read from the wrong (escaped) path"
    );

    // tree-sitter: the `main` symbol node and its entry-point fact. The file is
    // under no `src/`, so its module path is the byte-preserving (escaped) file
    // key without its extension — the escaping must survive into the identity.
    let sym = g
        .node(&NodeKey::new("sym:rust:a%25main#main()"))
        .expect("main symbol node missing — tree-sitter read the wrong path");
    assert_eq!(sym.kind, NodeKind::Symbol);
    assert!(
        sym.facts
            .iter()
            .any(|f| f.kind == FactKind::EntryPoint && f.value == FactValue::Bool(true)),
        "entry_point fact missing on the percent-named file's main symbol"
    );
}

// ---- U5: external symbols and resources ----

/// Every `NodeKind::External` key in the graph, sorted.
fn ext_keys(g: &Graph) -> Vec<String> {
    let mut keys: Vec<String> = g
        .nodes_sorted()
        .iter()
        .filter(|n| n.kind == NodeKind::External)
        .map(|n| n.key.as_str().to_string())
        .collect();
    keys.sort();
    keys
}

/// Every `Uses` edge as `(from, to)`, sorted.
fn ext_uses(g: &Graph) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = g
        .edges_sorted()
        .iter()
        .filter(|e| e.edge.kind == EdgeKind::Uses)
        .map(|e| (e.from.as_str().to_string(), e.to.as_str().to_string()))
        .collect();
    out.sort();
    out
}

/// The text values of one fact kind on one node, sorted.
fn ext_texts(g: &Graph, key: &str, kind: FactKind) -> Vec<String> {
    let mut out: Vec<String> = g
        .node(&NodeKey::new(key))
        .map(|n| {
            n.facts
                .iter()
                .filter(|f| f.kind == kind)
                .filter_map(|f| match &f.value {
                    FactValue::Text(t) => Some(t.clone()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

/// A polyglot repo where each language declares ONE dependency in its manifest
/// and then imports and uses a symbol from it inside a member. The manifest is
/// the authority on what leaves the repo, so every ecosystem needs its own.
fn foreign_type_fixture(dir: &std::path::Path) {
    init_repo_with_files(
        dir,
        "Alice",
        &[
            ("Cargo.toml", "[dependencies]\nserde = \"1\"\n"),
            (
                "src/lib.rs",
                "use serde::Serialize;\n\npub struct Cfg;\n\nimpl Cfg {\n    pub fn dump(&self) -> bool {\n        let _s: Option<Serialize> = None;\n        true\n    }\n}\n",
            ),
            ("package.json", "{\"dependencies\":{\"react\":\"^18\"}}"),
            (
                "web/app.ts",
                "import { useState } from \"react\";\n\nexport class Panel {\n  render(): void {\n    useState();\n  }\n}\n",
            ),
            (
                "go.mod",
                "module example.com/m\n\ngo 1.21\n\nrequire github.com/pkg/errors v0.9.1\n",
            ),
            (
                "cmd/main.go",
                "package cmd\n\nimport \"github.com/pkg/errors\"\n\ntype Runner struct{}\n\nfunc (r Runner) Run() error {\n\treturn errors.New(\"boom\")\n}\n",
            ),
            (
                "proj.csproj",
                "<Project>\n  <ItemGroup>\n    <PackageReference Include=\"Newtonsoft.Json\" Version=\"13.0.3\" />\n  </ItemGroup>\n</Project>\n",
            ),
            (
                "Store.cs",
                "using Newtonsoft.Json;\n\nnamespace N;\n\npublic class Store\n{\n    public string Dump()\n    {\n        return Newtonsoft.Json.JsonConvert.SerializeObject(this);\n    }\n}\n",
            ),
        ],
    );
}

/// Scenario 1: a type from a foreign library becomes an External node, and the
/// `Uses` edge starts at the MEMBER that uses it — which is exactly why members
/// are their own Symbol nodes (KTD5).
#[test]
fn a_foreign_type_is_used_by_the_member_that_names_it() {
    let tmp = tempfile::tempdir().unwrap();
    foreign_type_fixture(tmp.path());

    let g = graph_over(tmp.path());
    let uses = ext_uses(&g);

    for expected in [
        (
            "sym:rust:crate#Cfg.dump(&self)".to_string(),
            "ext:rust:serde::Serialize".to_string(),
        ),
        (
            "sym:ts:web/app.ts#Panel.render()".to_string(),
            "ext:ts:react.useState".to_string(),
        ),
        (
            "sym:go:cmd#Runner.Run()".to_string(),
            "ext:go:github.com/pkg/errors".to_string(),
        ),
        (
            "sym:csharp:N#Store.Dump()".to_string(),
            "ext:csharp:Newtonsoft.Json.JsonConvert.SerializeObject".to_string(),
        ),
    ] {
        assert!(
            uses.contains(&expected),
            "missing Uses edge {expected:?} in {uses:?}"
        );
    }

    // The node carries its readable name and its sort as facts, so no consumer
    // has to parse the opaque key apart (KTD-B1).
    assert_eq!(
        ext_texts(&g, "ext:rust:serde::Serialize", FactKind::ExternalName),
        vec!["serde::Serialize".to_string()]
    );
    assert_eq!(
        ext_texts(&g, "ext:rust:serde::Serialize", FactKind::ExternalKind),
        vec!["symbol".to_string()]
    );
    assert_eq!(
        g.node(&NodeKey::new("ext:rust:serde::Serialize"))
            .unwrap()
            .kind,
        NodeKind::External
    );
}

/// A C# `using N;` binds no name, so nothing inside a member can be attributed
/// to it — but the directive itself is a statement the FILE makes, and every
/// External node must be reachable (no orphans).
#[test]
fn a_namespace_import_is_used_by_the_file_that_imports_it() {
    let tmp = tempfile::tempdir().unwrap();
    foreign_type_fixture(tmp.path());

    let g = graph_over(tmp.path());
    assert!(
        ext_uses(&g).contains(&(
            "Store.cs".to_string(),
            "ext:csharp:Newtonsoft.Json".to_string()
        )),
        "the using directive's own file-level Uses edge is missing"
    );

    // No orphans: every External node is the target of at least one Uses edge.
    let targets: Vec<String> = ext_uses(&g).into_iter().map(|(_, to)| to).collect();
    for key in ext_keys(&g) {
        assert!(targets.contains(&key), "orphan External node {key}");
    }
}

/// KTD8's load-bearing half: an import this repo's manifests never declare is
/// NOT evidence of anything external. Without a declared dependency to match,
/// nothing is emitted — no guess, no phantom node.
#[test]
fn an_import_no_manifest_declares_yields_no_external_node() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            // No Cargo.toml at all: `mystery` could be a crate or a 2015-edition
            // local module, and the extractor may not decide which.
            (
                "src/lib.rs",
                "use mystery::Thing;\n\npub fn go() -> bool {\n    let _t: Option<Thing> = None;\n    true\n}\n",
            ),
        ],
    );

    let g = graph_over(tmp.path());
    assert_eq!(ext_keys(&g), Vec::<String>::new());
    assert_eq!(ext_uses(&g), Vec::<(String, String)>::new());
}

/// A C# verbatim SQL literal routinely spans many lines, so the provenance must
/// point at the line the OBJECT NAME is on — not at the line the literal opens.
#[test]
fn a_qualified_database_object_is_anchored_to_its_own_line() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[(
            "Repo.cs",
            "namespace N;\n\npublic class Rep\n{\n    public void Load()\n    {\n        var sql = @\"\n            SELECT Id\n            FROM dbo.Invoices\n            WHERE Id = 1\";\n    }\n}\n",
        )],
    );

    let g = graph_over(tmp.path());
    assert_eq!(ext_keys(&g), vec!["res:db:dbo.Invoices".to_string()]);
    assert_eq!(
        ext_texts(&g, "res:db:dbo.Invoices", FactKind::ExternalKind),
        vec!["db_object".to_string()]
    );

    // `FROM dbo.Invoices` is on line 9; the literal opens on line 7.
    let edge = g
        .edges_sorted()
        .into_iter()
        .find(|e| e.edge.kind == EdgeKind::Uses)
        .expect("Uses edge missing");
    assert_eq!(edge.from.as_str(), "sym:csharp:N#Rep.Load()");
    assert_eq!(edge.edge.provenance.file, "Repo.cs");
    assert_eq!(edge.edge.provenance.span.start(), Some(9));
}

/// An absolute URL is unambiguous; a relative path and an interpolated fragment
/// are not, so they leave the graph untouched (KTD8).
#[test]
fn only_an_absolute_uninterpolated_url_becomes_a_resource() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[(
            "Api.cs",
            "namespace N;\n\npublic class Api\n{\n    private string host = \"h\";\n\n    public void Call()\n    {\n        var abs = \"https://api.example.com/v1/orders\";\n        var rel = \"/v1/orders\";\n        var interp = $\"https://{this.host}/v1/orders\";\n    }\n}\n",
        )],
    );

    let g = graph_over(tmp.path());
    assert_eq!(
        ext_keys(&g),
        vec!["res:url:api.example.com/v1/orders".to_string()]
    );
    assert_eq!(
        ext_texts(
            &g,
            "res:url:api.example.com/v1/orders",
            FactKind::ExternalKind
        ),
        vec!["url".to_string()]
    );
}

/// A configuration key is emitted only where a NAMED configuration API reads it.
/// A free string constant that merely looks like a key is not evidence of a read.
#[test]
fn a_config_key_needs_a_known_api_not_a_plausible_string() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[(
            "Cfg.cs",
            "namespace N;\n\npublic class Cfg\n{\n    public void Read()\n    {\n        var s = ConfigurationManager.AppSettings[\"Orbit:Server\"];\n        var free = \"Orbit:Timeout\";\n    }\n}\n",
        )],
    );

    let g = graph_over(tmp.path());
    assert_eq!(ext_keys(&g), vec!["res:cfg:Orbit:Server".to_string()]);
    assert_eq!(
        ext_texts(&g, "res:cfg:Orbit:Server", FactKind::ExternalKind),
        vec!["config_key".to_string()]
    );
}

/// A literal inside a comment is commented-out code or prose, never a live
/// reference — the whole point of lexing comments out before matching.
#[test]
fn a_literal_inside_a_comment_is_never_a_resource() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[(
            "Old.cs",
            "namespace N;\n\npublic class Old\n{\n    public void Load()\n    {\n        // var url = \"https://dead.example.com/x\";\n        /* var sql = \"SELECT Id FROM dbo.Gone\"; */\n    }\n}\n",
        )],
    );

    let g = graph_over(tmp.path());
    assert_eq!(ext_keys(&g), Vec::<String>::new());
}

/// The sentinel value planted in the fixture's connection string. It must appear
/// NOWHERE downstream (R22).
const SECRET_SENTINEL: &str = "Sentinel0Pa55word";

/// Ten literals in one member, of which only the unambiguous ones may be
/// emitted. The decoys are the shapes a naive pattern set actually fires on in
/// real C#: a `{0}`-placeholder log format carrying `Server=`, prose containing
/// "Update … from …", an unqualified `FROM`, a relative URL, an interpolated URL,
/// and a plausible-looking free config string.
fn ten_literal_fixture(dir: &std::path::Path) {
    init_repo_with_files(
        dir,
        "Alice",
        &[(
            "Ledger.cs",
            &format!(
                "namespace L;\n\npublic class Ledger\n{{\n    private string host;\n\n    public void Load()\n    {{\n        var a = \"SELECT Id FROM dbo.Orders WHERE Id = 1\";\n        var b = \"select id from orders\";\n        var c = \"https://api.example.com/v1/orders\";\n        var d = \"/v1/orders\";\n        var e = $\"https://{{this.host}}/v1/orders\";\n        var f = ConfigurationManager.AppSettings[\"Orbit:Server\"];\n        var g = \"Orbit:Timeout\";\n        var h = \"Data Source=srv;Initial Catalog=ORBIT;User ID=sa;Password={SECRET_SENTINEL}\";\n        var i = \"(2->3) response Type={{1}} Server={{4}} Sql={{5}} Data=\";\n        var j = \"Update config.value from settings.json before deploy\";\n    }}\n}}\n"
            ),
        )],
    );
}

/// The verification the plan asks for: over ten literals, only the pattern
/// matches are emitted and the other six change nothing at all.
#[test]
fn of_ten_literals_only_the_unambiguous_ones_are_emitted() {
    let tmp = tempfile::tempdir().unwrap();
    ten_literal_fixture(tmp.path());

    let g = graph_over(tmp.path());
    assert_eq!(
        ext_keys(&g),
        vec![
            "res:cfg:Orbit:Server".to_string(),
            "res:db:dbo.Orders".to_string(),
            "res:url:api.example.com/v1/orders".to_string(),
        ],
        "a decoy literal was emitted, or a real one was missed"
    );

    // The connection string is the tenth emission — a redacted reference, not a
    // node — and the six decoys produce no fact of their own.
    let redactions = ext_texts(&g, "sym:csharp:L#Ledger.Load()", FactKind::RedactedLiteral);
    assert_eq!(redactions, vec!["password".to_string()]);
}

/// R22: a literal matching a secret pattern yields a reference to its PLACE and
/// nothing else. The value must not survive into the graph or into any export.
#[test]
fn a_connection_string_password_reaches_neither_the_graph_nor_the_manifest() {
    let tmp = tempfile::tempdir().unwrap();
    ten_literal_fixture(tmp.path());

    let repo = Repo::open(tmp.path()).unwrap();
    let mut g = Graph::new();
    run_all(&repo, &mut g).unwrap();
    reverse_architect::graph::merge(&mut g, &reverse_architect::extract::trust_ranks());

    // The reference: the owning member, the line, no value.
    let fact = g
        .node(&NodeKey::new("sym:csharp:L#Ledger.Load()"))
        .unwrap()
        .facts
        .iter()
        .find(|f| f.kind == FactKind::RedactedLiteral)
        .expect("no redacted-literal reference for the connection string");
    assert_eq!(fact.value, FactValue::Text("password".to_string()));
    assert_eq!(fact.provenance.span.start(), Some(16));

    // The whole serialized graph — nodes, facts, edges, dissent.
    let dumped = format!(
        "{}{}",
        serde_json::to_string(&g.nodes_sorted()).unwrap(),
        serde_json::to_string(
            &g.edges_sorted()
                .iter()
                .map(|e| (e.from.clone(), e.to.clone(), e.edge.provenance.clone()))
                .collect::<Vec<_>>()
        )
        .unwrap()
    );
    assert!(
        !dumped.contains(SECRET_SENTINEL),
        "the secret value leaked into the graph"
    );
    assert!(
        !dumped.contains("Data Source=srv"),
        "the connection string leaked into the graph"
    );

    // And the machine export built from it.
    let manifest = reverse_architect::manifest::build_manifest(&g, None).unwrap();
    let json = reverse_architect::manifest::to_json(&manifest);
    assert!(
        !json.contains(SECRET_SENTINEL),
        "the secret value leaked into the manifest"
    );
}

/// R22 through the other common shape: a connection string in a FIELD
/// initializer rather than a method body.
///
/// This extractor's own half holds — the reference is on the field, it names the
/// pattern and not the value, and nothing `external` emits carries the literal.
///
/// It also pins down a PRE-EXISTING gap this unit does not own: U2's
/// `SymbolSignature` quotes a C# field's declaration header *including its
/// initializer*, so a secret written into a field initializer still reaches the
/// graph and the manifest through that fact — not through anything here. Closing it
/// means running the same redaction filter over the declaration facts tree-sitter
/// emits, which is U2 territory (`src/extract/treesitter.rs`) and a decision for
/// the unit that owns it. The assertion below is therefore scoped to `external`'s
/// own emissions rather than weakened away, so the day the signature side is fixed
/// this test still says exactly what it means.
#[test]
fn a_secret_in_a_field_initializer_is_redacted_by_this_extractor() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[(
            "Conn.cs",
            &format!(
                "namespace C;

public class Conn
{{
    private string conn = \"Data Source=srv;Initial Catalog=ORBIT;User ID=sa;Password={SECRET_SENTINEL}\";
}}
"
            ),
        )],
    );

    let repo = Repo::open(tmp.path()).unwrap();
    let mut g = Graph::new();
    run_all(&repo, &mut g).unwrap();
    reverse_architect::graph::merge(&mut g, &reverse_architect::extract::trust_ranks());

    // The reference is on the field, and it names the pattern, not the value.
    assert_eq!(
        ext_texts(&g, "sym:csharp:C#Conn.conn", FactKind::RedactedLiteral),
        vec!["password".to_string()],
        "no redacted-literal reference on the field"
    );

    // Nothing this extractor emitted carries the value — no fact of its own, and
    // no External node key (a `res:`/`ext:` key IS published verbatim, so a
    // secret-shaped literal must never have become one).
    for node in g.nodes_sorted() {
        if node.kind == NodeKind::External {
            assert!(
                !node.key.as_str().contains(SECRET_SENTINEL),
                "a secret literal became an External node key"
            );
        }
        for fact in &node.facts {
            if *fact.provenance.extractor != *"external" {
                continue;
            }
            assert!(
                !format!("{:?}", fact.value).contains(SECRET_SENTINEL),
                "the secret value leaked into an external fact on {}",
                node.key.as_str()
            );
        }
    }

    // And no `Uses` edge was built out of it either.
    assert!(
        !ext_uses(&g)
            .iter()
            .any(|(_, to)| to.contains(SECRET_SENTINEL)),
        "a secret literal became a Uses target"
    );
}

/// KD2/KTD8: every claim this extractor makes is a textual pattern match, so
/// every fact and every edge it emits is `Heuristic`. A `Resolved` external can
/// only come from an external index, which is a later unit.
#[test]
fn every_external_fact_and_edge_is_heuristic() {
    let tmp = tempfile::tempdir().unwrap();
    foreign_type_fixture(tmp.path());
    ten_literal_fixture(tmp.path());

    let g = graph_over(tmp.path());
    let mut emitted = 0usize;

    for node in g.nodes_sorted() {
        for fact in &node.facts {
            if *fact.provenance.extractor != *"external" {
                continue;
            }
            emitted += 1;
            assert_eq!(
                fact.provenance.resolution,
                Resolution::Heuristic,
                "non-heuristic external fact on {}",
                node.key.as_str()
            );
            assert_eq!(fact.provenance.tier, Tier::Measured);
        }
    }
    for e in g.edges_sorted() {
        if e.edge.kind != EdgeKind::Uses {
            continue;
        }
        emitted += 1;
        assert_eq!(&*e.edge.provenance.extractor, "external");
        assert_eq!(e.edge.provenance.resolution, Resolution::Heuristic);
    }
    assert!(emitted > 0, "the sweep asserted nothing");
}

#[test]
fn external_output_is_byte_identical_across_two_runs() {
    let tmp = tempfile::tempdir().unwrap();
    foreign_type_fixture(tmp.path());
    ten_literal_fixture(tmp.path());

    let dump = |g: &Graph| {
        let externals: Vec<_> = g
            .nodes_sorted()
            .iter()
            .filter(|n| n.kind == NodeKind::External)
            .map(|n| (n.key.clone(), n.facts.clone()))
            .collect();
        (
            serde_json::to_string(&externals).unwrap(),
            serde_json::to_string(&ext_uses(g)).unwrap(),
        )
    };

    assert_eq!(dump(&graph_over(tmp.path())), dump(&graph_over(tmp.path())));
}

/// U4: the registry reaches `metrics`, and it reaches it FIRST.
///
/// Both directions matter and neither is provable from the other. A language
/// the registry brings must get the counter its entry names, or a plugin
/// language is reported by coverage and still zero-LOC in every card that
/// consumes it. A registry entry with `loc: None` must win over tokei's own
/// table, or the entry is a suggestion rather than the answer — and `.md` is
/// the case that shows it, because tokei knows that extension perfectly well
/// on its own.
#[test]
fn the_language_registry_decides_loc_before_tokei_does() {
    use reverse_architect::extract::languages::{LanguageEntry, Languages};
    use reverse_architect::extract::{run_all_with, ExtractOptions};

    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            ("vzor/faktura.ktd", "direktiva Faktura\nkonec\n"),
            ("README.md", "# readme\n\nsome prose\n"),
        ],
    );

    let mut languages = Languages::bundled();
    // A language this build has no grammar for, counted as plain text.
    languages
        .register(
            "ktd",
            LanguageEntry {
                name: "otevrenost".to_string(),
                loc: Some(tokei::LanguageType::Text),
            },
        )
        .expect("a language tokei has never heard of registers");
    // A language tokei WOULD have counted, deliberately declared uncountable.
    languages
        .register(
            "md",
            LanguageEntry {
                name: "dokumentace".to_string(),
                loc: None,
            },
        )
        .expect("an extension no bundled language claims registers");

    let repo = Repo::open(tmp.path()).unwrap();
    let mut g = Graph::new();
    run_all_with(
        &repo,
        &mut g,
        &ExtractOptions {
            languages,
            ..ExtractOptions::default()
        },
        &reverse_architect::progress::Silent,
    )
    .unwrap();

    let loc_of = |key: &str| {
        g.node(&NodeKey::new(key))
            .unwrap_or_else(|| panic!("{key} is not in the graph"))
            .facts
            .iter()
            .find(|f| f.kind == FactKind::Loc)
            .map(|f| f.value.clone())
    };

    assert_eq!(
        loc_of("vzor/faktura.ktd"),
        Some(FactValue::Int(2)),
        "a registered language must be counted, not silently zero-LOC"
    );
    assert_eq!(
        loc_of("README.md"),
        None,
        "`loc: None` is a decision the entry makes, not a fall-through to tokei"
    );
}

// ---- Database access per occurrence (plan B, U2: R11, KTD2) ----

/// Every `DbAccess` fact in the graph as `(owner, text, line)`, sorted.
fn db_accesses(g: &Graph) -> Vec<(String, String, u32)> {
    let mut out: Vec<(String, String, u32)> = g
        .nodes_sorted()
        .iter()
        .flat_map(|n| {
            n.facts
                .iter()
                .filter(|f| f.kind == FactKind::DbAccess)
                .map(|f| {
                    let FactValue::Text(text) = &f.value else {
                        panic!("a DbAccess fact is text: {f:?}");
                    };
                    (
                        n.key.as_str().to_string(),
                        text.clone(),
                        f.provenance.span.start().expect("an access has a line"),
                    )
                })
                .collect::<Vec<_>>()
        })
        .collect();
    out.sort();
    out
}

/// One C# class `Rep` with one method per literal: method `M<i>` holds its
/// literal on line `7 + 4 * i`, so every expected line is arithmetic.
fn sql_methods_fixture(dir: &std::path::Path, literals: &[&str]) {
    let mut src = String::from("namespace N;\n\npublic class Rep\n{\n");
    for (i, lit) in literals.iter().enumerate() {
        src.push_str(&format!(
            "    public void M{i}()\n    {{\n        var s = \"{lit}\";\n    }}\n"
        ));
    }
    src.push_str("}\n");
    init_repo_with_files(dir, "Alice", &[("Repo.cs", &src)]);
}

fn access(method: usize, text: &str) -> (String, String, u32) {
    (
        format!("sym:csharp:N#Rep.M{method}()"),
        text.to_string(),
        7 + 4 * method as u32,
    )
}

/// KTD2: the state comes from the statement — the target of a change, through
/// an alias where the statement names one — never from the word right before
/// the object. `UPDATE t … FROM dba.t t` and `DELETE t FROM dba.t t` write the
/// table the alias stands for, although `dba.t` itself follows `FROM`.
#[test]
fn a_database_access_is_read_written_or_called_per_statement() {
    let tmp = tempfile::tempdir().unwrap();
    sql_methods_fixture(
        tmp.path(),
        &[
            "update dba.t set x = 1 where id = 2",
            "UPDATE t SET x=1 FROM dba.t t",
            "DELETE t FROM dba.t t WHERE t.id = 1",
            "select a.x from dba.a a join dba.b b on a.id = b.id",
            "delete from dba.t where id = 1",
            "exec dba.proc {0}",
            "UPDATE x SET a = 1 FROM dba.a a JOIN dba.b b ON a.id = b.id",
            "insert into dba.log (msg) select msg from dba.queue",
        ],
    );
    let g = graph_over(tmp.path());
    let mut expected = vec![
        access(0, "writes dba.t"),
        access(1, "writes dba.t"),
        access(2, "writes dba.t"),
        access(3, "reads dba.a"),
        access(3, "reads dba.b"),
        access(4, "writes dba.t"),
        access(5, "calls dba.proc"),
        // The target `x` is no alias the statement declares: either table
        // could be the one changed, so neither is said to be read.
        access(6, "mentions dba.a"),
        access(6, "mentions dba.b"),
        access(7, "reads dba.queue"),
        access(7, "writes dba.log"),
    ];
    expected.sort();
    assert_eq!(db_accesses(&g), expected);
}

/// R11, the case behind it: one method that reads a table and later changes
/// it carries both, each at its own line — two statements of one verbatim
/// literal, the second with no `;` before it.
#[test]
fn a_read_and_a_write_of_one_table_keep_their_own_lines() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[(
            "Repo.cs",
            "namespace N;\n\npublic class Rep\n{\n    public void Load()\n    {\n        var sql = @\"\n            SELECT x FROM dba.t\n            UPDATE dba.t SET x = 1\";\n    }\n}\n",
        )],
    );
    let g = graph_over(tmp.path());
    assert_eq!(
        db_accesses(&g),
        vec![
            ("sym:csharp:N#Rep.Load()".into(), "reads dba.t".into(), 8),
            ("sym:csharp:N#Rep.Load()".into(), "writes dba.t".into(), 9),
        ]
    );
    // The `Uses` edge is what it was: one per member and object.
    assert_eq!(
        ext_uses(&g),
        vec![(
            "sym:csharp:N#Rep.Load()".to_string(),
            "res:db:dba.t".to_string()
        )]
    );
    // The facts reach the manifest as the core kind its v10 names.
    let manifest = reverse_architect::manifest::build_manifest(&g, None).unwrap();
    let json = reverse_architect::manifest::to_json(&manifest);
    assert!(
        json.contains("\"db_access\"") && json.contains("\"writes dba.t\""),
        "{json}"
    );
}

/// Plan B, AE9 (R11): a procedure call is not a write. A method that executes
/// a procedure and reads a table says exactly that — a call and a read.
#[test]
fn a_procedure_call_and_a_read_are_no_write() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[(
            "Repo.cs",
            "namespace N;\n\npublic class Rep\n{\n    public void Run()\n    {\n        Exec(\"exec dba.proc\");\n        var rows = Query(\"select y from dba.x\");\n    }\n}\n",
        )],
    );
    let g = graph_over(tmp.path());
    assert_eq!(
        db_accesses(&g),
        vec![
            ("sym:csharp:N#Rep.Run()".into(), "calls dba.proc".into(), 7),
            ("sym:csharp:N#Rep.Run()".into(), "reads dba.x".into(), 8),
        ]
    );
}

/// KTD2: a `#`-directive is not read. `#SELECT;` is no statement and names no
/// table, and a SQL string inside a statement is not part of it.
#[test]
fn a_hash_directive_is_no_database_access() {
    let tmp = tempfile::tempdir().unwrap();
    sql_methods_fixture(
        tmp.path(),
        &["#SELECT;", "exec dba.geo_run '#SELECT; from dba.decoy'"],
    );
    let g = graph_over(tmp.path());
    assert_eq!(db_accesses(&g), vec![access(1, "calls dba.geo_run")]);
}

/// R10 before R11: a literal the secret filter withholds yields no access at
/// all, though it carries a statement — the filter runs first.
#[test]
fn a_secret_literal_yields_no_database_access() {
    let tmp = tempfile::tempdir().unwrap();
    sql_methods_fixture(
        tmp.path(),
        &[&format!(
            "Data Source=srv;User ID=sa;Password={SECRET_SENTINEL};select x from dba.t"
        )],
    );
    let g = graph_over(tmp.path());
    assert_eq!(db_accesses(&g), Vec::<(String, String, u32)>::new());
    assert_eq!(
        ext_texts(&g, "sym:csharp:N#Rep.M0()", FactKind::RedactedLiteral),
        vec!["password".to_string()],
        "control: the literal was withheld, not missed"
    );
}

/// R13: the host lexer reads every language's literals, so the same SQL on
/// the same lines gives the same accesses in Python, Java and Go — on the
/// function, never on the file.
#[test]
fn python_java_and_go_give_the_same_database_accesses() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            (
                "py/rep.py",
                "class Rep:\n    def load(self):\n        a = \"exec dba.proc {0}\"\n        b = \"select y from dba.x\"\n",
            ),
            (
                "java/Rep.java",
                "class Rep {\n    void load() {\n        String a = \"exec dba.proc {0}\";\n        String b = \"select y from dba.x\";\n    }\n}\n",
            ),
            (
                "go/rep.go",
                "package rep\nfunc Load() {\n\ta := \"exec dba.proc {0}\"\n\tb := \"select y from dba.x\"\n\t_, _ = a, b\n}\n",
            ),
        ],
    );
    let g = graph_over(tmp.path());
    let all = db_accesses(&g);
    for dir in ["py/", "java/", "go/"] {
        let of_language: Vec<(String, u32)> = all
            .iter()
            .filter(|(owner, _, _)| {
                let node = g.node(&NodeKey::new(owner.as_str())).unwrap();
                assert_eq!(node.kind, NodeKind::Symbol, "{owner} is not a declaration");
                node.facts
                    .iter()
                    .any(|f| f.kind == FactKind::SymbolName && f.provenance.file.starts_with(dir))
            })
            .map(|(_, text, line)| (text.clone(), *line))
            .collect();
        assert_eq!(
            of_language,
            vec![
                ("calls dba.proc".to_string(), 3),
                ("reads dba.x".to_string(), 4)
            ],
            "{dir}: {all:#?}"
        );
    }
}

// ---- Anchors from literals and comments (plan B, U3: R9, R10, R13, KTD1, KTD11) ----

use reverse_architect::graph::{AnchorSource, AnchorText, ANCHOR_CAP, ANCHOR_TEXT_CAP};

/// The 1-based line of the first occurrence of `needle` in a fixture source.
fn line_of(src: &str, needle: &str) -> u32 {
    let at = src
        .find(needle)
        .unwrap_or_else(|| panic!("`{needle}` is not in the fixture"));
    src[..at].matches('\n').count() as u32 + 1
}

/// Every `Anchor` fact in the graph as `(owner, anchor, line)`, sorted.
fn anchors(g: &Graph) -> Vec<(String, AnchorText, u32)> {
    let mut out: Vec<(String, AnchorText, u32)> = g
        .nodes_sorted()
        .iter()
        .flat_map(|n| {
            n.facts
                .iter()
                .filter(|f| f.kind == FactKind::Anchor)
                .map(|f| {
                    let FactValue::Text(text) = &f.value else {
                        panic!("an Anchor fact is text: {f:?}");
                    };
                    (
                        n.key.as_str().to_string(),
                        AnchorText::decode(text).expect("an anchor decodes"),
                        f.provenance.span.start().expect("an anchor has a line"),
                    )
                })
                .collect::<Vec<_>>()
        })
        .collect();
    out.sort_by(|a, b| (&a.0, a.2, &a.1.text).cmp(&(&b.0, b.2, &b.1.text)));
    out
}

/// `(owner, text, line)` of every anchor — the shape most assertions need.
fn anchor_texts(g: &Graph) -> Vec<(String, String, u32)> {
    anchors(g)
        .into_iter()
        .map(|(owner, a, line)| (owner, a.text, line))
        .collect()
}

/// Every `AnchorSkipped` fact as `(owner, reason, line)`, sorted.
fn skipped(g: &Graph) -> Vec<(String, String, u32)> {
    let mut out: Vec<(String, String, u32)> = g
        .nodes_sorted()
        .iter()
        .flat_map(|n| {
            n.facts
                .iter()
                .filter(|f| f.kind == FactKind::AnchorSkipped)
                .map(|f| {
                    let FactValue::Text(reason) = &f.value else {
                        panic!("an AnchorSkipped fact is text: {f:?}");
                    };
                    (
                        n.key.as_str().to_string(),
                        reason.clone(),
                        f.provenance.span.start().expect("a skip has a line"),
                    )
                })
                .collect::<Vec<_>>()
        })
        .collect();
    out.sort();
    out
}

/// Every text value of every fact on every node — what a secret must never be
/// part of, wherever the graph would carry it.
fn all_texts(g: &Graph) -> Vec<String> {
    g.nodes_sorted()
        .iter()
        .flat_map(|n| n.facts.iter())
        .filter_map(|f| match &f.value {
            FactValue::Text(t) => Some(t.clone()),
            _ => None,
        })
        .collect()
}

fn assert_nowhere(g: &Graph, values: &[&str]) {
    for text in all_texts(g) {
        for value in values {
            assert!(!text.contains(value), "`{value}` reached the graph: {text}");
        }
    }
}

const REP: &str = "sym:csharp:N#Rep";

/// R9: a message in a method leads to that method and its line.
#[test]
fn a_message_literal_anchors_its_method_and_line() {
    let tmp = tempfile::tempdir().unwrap();
    let src = "namespace N;\n\npublic class Rep\n{\n    public void Restore()\n    {\n        throw new OrbitException(\"Chyba při obnově databáze\");\n    }\n}\n";
    init_repo_with_files(tmp.path(), "Alice", &[("Rep.cs", src)]);
    let g = graph_over(tmp.path());
    let found = anchors(&g);
    let restore = format!("{REP}.Restore()");
    let (_, anchor, line) = found
        .iter()
        .find(|(owner, a, _)| *owner == restore && a.text == "Chyba při obnově databáze")
        .unwrap_or_else(|| panic!("no anchor for the message: {found:#?}"));
    assert_eq!(*line, line_of(src, "Chyba při"));
    assert_eq!(anchor.source, AnchorSource::Literal);
    assert_eq!(anchor.occurrences, 1);
    assert_eq!(
        anchor.length,
        "Chyba při obnově databáze".chars().count() as u32
    );
}

/// KTD1: a comment is an anchor too, a doc comment is not — `DocComment`
/// carries it already.
#[test]
fn an_ordinary_comment_anchors_and_a_doc_comment_does_not() {
    let tmp = tempfile::tempdir().unwrap();
    let src = "namespace N;\n\npublic class Rep\n{\n    /// Obnoví plánované úlohy.\n    public void Restore()\n    {\n        // Úlohy agenta se zneplatní ručně\n        Run();\n    }\n}\n";
    init_repo_with_files(tmp.path(), "Alice", &[("Rep.cs", src)]);
    let g = graph_over(tmp.path());
    let texts = anchor_texts(&g);
    assert!(
        texts.contains(&(
            format!("{REP}.Restore()"),
            "Úlohy agenta se zneplatní ručně".to_string(),
            line_of(src, "Úlohy agenta"),
        )),
        "{texts:#?}"
    );
    assert!(
        !texts.iter().any(|(_, t, _)| t.contains("Obnoví plánované")),
        "a doc comment became an anchor: {texts:#?}"
    );
    let comment = anchors(&g)
        .into_iter()
        .find(|(_, a, _)| a.text.starts_with("Úlohy"))
        .unwrap();
    assert_eq!(comment.1.source, AnchorSource::Comment);
}

/// KTD11 rule 3 in a doc comment: a key-shaped word is cut out of the
/// `DocComment` fact itself — so the card's quote and purpose, the map, the
/// manifest and the model's input all go without it — and the cut leaves its
/// reason and its line, as a comment line's does.
#[test]
fn a_long_token_is_cut_out_of_a_doc_comment_and_says_so() {
    let tmp = tempfile::tempdir().unwrap();
    let src = "namespace N;\n\n/// <summary>\n/// Checks the checksum AbCdEf0123456789XyZ987abc of a payload.\n/// </summary>\npublic class Rep\n{\n    public void Run() { }\n}\n";
    init_repo_with_files(tmp.path(), "Alice", &[("Rep.cs", src)]);
    let g = graph_over(tmp.path());
    let doc = ext_texts(&g, REP, FactKind::DocComment);
    assert_eq!(doc.len(), 1, "{doc:?}");
    assert!(
        doc[0].contains("Checks the checksum of a payload.")
            && !doc[0].contains("AbCdEf0123456789XyZ987abc"),
        "{doc:?}"
    );
    assert_eq!(
        skipped(&g),
        vec![(
            REP.to_string(),
            "long token".to_string(),
            line_of(src, "Checks the")
        )]
    );
}

/// KTD11 rule 3 inside doc markup: a key written right between two tags —
/// inline `<c>KEY</c>` or a whole `<summary>KEY</summary>` — is cut out of
/// the text between them, the markup stays, and the cut says so at its line.
#[test]
fn a_long_token_between_doc_tags_is_cut_and_the_markup_stays() {
    let tmp = tempfile::tempdir().unwrap();
    let src = "namespace N;\n\n/// <summary>\n/// Checks <c>AbCdEf0123456789XyZ987abc</c>.\n/// </summary>\npublic class Rep\n{\n    public void Run() { }\n}\n\n\
               /// <summary>Zq7RtVx3Kp9WmN2bYc5Hd8Fg</summary>\npublic class Seal\n{\n    public void Run() { }\n}\n";
    init_repo_with_files(tmp.path(), "Alice", &[("Rep.cs", src)]);
    let g = graph_over(tmp.path());
    assert_nowhere(
        &g,
        &["AbCdEf0123456789XyZ987abc", "Zq7RtVx3Kp9WmN2bYc5Hd8Fg"],
    );
    let doc = ext_texts(&g, REP, FactKind::DocComment);
    assert!(
        doc.len() == 1 && doc[0].contains("Checks <c></c>."),
        "{doc:?}"
    );
    let seal = "sym:csharp:N#Seal";
    assert_eq!(
        skipped(&g),
        vec![
            (
                REP.to_string(),
                "long token".to_string(),
                line_of(src, "Checks <c>")
            ),
            (
                seal.to_string(),
                "long token".to_string(),
                line_of(src, "<summary>Zq7")
            ),
        ]
    );
}

/// R10, KTD11 rule 1: a secret in an ordinary comment leaves its place and the
/// reason, never the text — in a `//` line, in a block comment with a PEM
/// block, and with the value on the line below its label, both in one block
/// comment and across two `//` lines that only together read as one.
#[test]
fn a_secret_in_a_comment_leaves_only_its_place_and_reason() {
    let tmp = tempfile::tempdir().unwrap();
    let src = "namespace N;\n\npublic class Rep\n{\n    public void Run()\n    {\n        // password=hunter2-inline\n        Go();\n        /* deployment key:\n           -----BEGIN RSA PRIVATE KEY-----\n           MIIEfakekeyblockinacomment\n           -----END RSA PRIVATE KEY-----\n           password:\n             hunter2-nextline */\n        Go();\n        // -----BEGIN RSA PRIVATE KEY-----\n        // MIIEfakekeyinlinecomments\n        // -----END RSA PRIVATE KEY-----\n        Go();\n        // password:\n        // hunter2-twolines\n        Go();\n    }\n}\n";
    init_repo_with_files(tmp.path(), "Alice", &[("Rep.cs", src)]);
    let g = graph_over(tmp.path());
    assert_nowhere(
        &g,
        &[
            "hunter2-inline",
            "MIIEfakekeyblockinacomment",
            "hunter2-nextline",
            "MIIEfakekeyinlinecomments",
            "hunter2-twolines",
        ],
    );
    let run = format!("{REP}.Run()");
    let reasons = skipped(&g);
    for (needle, reason) in [
        ("password=hunter2-inline", "password"),
        ("MIIEfakekeyblockinacomment", "private key"),
        ("hunter2-nextline", "secret value"),
        ("MIIEfakekeyinlinecomments", "private key"),
        ("hunter2-twolines", "secret value"),
    ] {
        let place = (run.clone(), reason.to_string(), line_of(src, needle));
        assert!(reasons.contains(&place), "{place:?} missing: {reasons:#?}");
    }
    // The prose around the key block is still an anchor.
    assert!(
        anchor_texts(&g)
            .iter()
            .any(|(o, t, _)| *o == run && t == "deployment key:"),
        "{:#?}",
        anchor_texts(&g)
    );
}

/// R10, KTD11 rules 2–4: a literal is read with the code on its line, a long
/// token is never an anchor, and the Czech and SQL spellings are secrets too.
#[test]
fn a_secret_literal_is_never_an_anchor() {
    let tmp = tempfile::tempdir().unwrap();
    let src = "namespace N;\n\npublic class Rep\n{\n    public void Run()\n    {\n        password = \"hunter2-assigned\";\n        var apiKey = \"9f2b7c11ea3d40aa9c1f\";\n        heslo = \"pivo-heslo\";\n        secret = \"kocka-secret\";\n        var id = \"3f2504e04f8911d39a0c0305e82c3301\";\n        Exec(\"create login orbit with password 'hunter2-sql'\");\n    }\n}\n";
    init_repo_with_files(tmp.path(), "Alice", &[("Rep.cs", src)]);
    let g = graph_over(tmp.path());
    assert_nowhere(
        &g,
        &[
            "hunter2-assigned",
            "9f2b7c11ea3d40aa9c1f",
            "pivo-heslo",
            "kocka-secret",
            "3f2504e04f8911d39a0c0305e82c3301",
            "hunter2-sql",
        ],
    );
    let run = format!("{REP}.Run()");
    // The secrets are withheld whole, as a literal always was; the long token
    // is no secret, only never an anchor, and says so.
    let mut redacted = ext_texts(&g, &run, FactKind::RedactedLiteral);
    redacted.sort();
    assert_eq!(
        redacted,
        ["password", "password", "password", "token", "token"]
    );
    assert_eq!(
        skipped(&g),
        vec![(run, "long token".to_string(), line_of(src, "3f2504e0"))]
    );
}

/// A message assigned to a name that says "password" is still a message: the
/// line only withholds a value-shaped literal, so a support search for the
/// text of a label or an error finds it (plan B stop condition), while a
/// value assigned to the same names stays withheld.
#[test]
fn a_message_assigned_to_a_password_name_stays_an_anchor() {
    let tmp = tempfile::tempdir().unwrap();
    let src = "namespace N;\n\npublic class Rep\n{\n    public void Run()\n    {\n        Show(lblPassword: \"Zadejte heslo\");\n        Errors.Password = \"Heslo je příliš krátké\";\n        Errors.Password = \"kocka-value-pw\";\n    }\n}\n";
    init_repo_with_files(tmp.path(), "Alice", &[("Rep.cs", src)]);
    let g = graph_over(tmp.path());
    assert_nowhere(&g, &["kocka-value-pw"]);
    let run = format!("{REP}.Run()");
    let texts = anchor_texts(&g);
    for message in ["Zadejte heslo", "Heslo je příliš krátké"] {
        assert!(
            texts.contains(&(run.clone(), message.to_string(), line_of(src, message))),
            "{message} must stay an anchor: {texts:#?}"
        );
    }
}

/// R10, KTD11 rule 2: a C# property hands out its value past its accessors —
/// an initializer after `{ get; set; }` or an expression body `=>` — and a
/// credential property is withheld like a credential field, one word or
/// several. A property named as a message keeps its text as an anchor.
#[test]
fn a_credential_property_initializer_is_withheld() {
    let tmp = tempfile::tempdir().unwrap();
    let src = "namespace N;\n\npublic class Settings\n{\n    \
               public string Password { get; set; } = \"maple grove river\";\n    \
               public string ApiKey { get; } = \"kocka-prop-key\";\n    \
               public string Secret { get; private set; } = \"kocka-prop-secret\";\n    \
               public string Token => \"kocka-body-token\";\n    \
               public string PasswordHint { get; init; } = \"Zadejte heslo k databázi\";\n    \
               public string DbPassword\n    {\n        get;\n        set;\n    } = \"oak shelf lantern\";\n    \
               public string PasswordLabel\n    {\n        get;\n    } = \"Heslo uživatele\";\n    \
               public string SmtpPassword\n    {\n        get;\n        set;\n    } =\n        \"pine cove ember\";\n    \
               public string ApiSecret\n    {\n        get;\n    } =\n\"birch reed quartz\";\n}\n";
    init_repo_with_files(tmp.path(), "Alice", &[("Settings.cs", src)]);
    let g = graph_over(tmp.path());
    assert_nowhere(
        &g,
        &[
            "maple grove river",
            "kocka-prop-key",
            "kocka-prop-secret",
            "kocka-body-token",
            "oak shelf lantern",
            "pine cove ember",
            "birch reed quartz",
        ],
    );
    let texts = anchor_texts(&g);
    for message in ["Zadejte heslo k databázi", "Heslo uživatele"] {
        assert!(
            texts.iter().any(|(_, text, _)| text == message),
            "{message} stays an anchor: {texts:#?}"
        );
    }
}

/// R10, KTD11 rule 2: a value of several words assigned to a secret's name
/// is a passphrase, unless a word of the WHOLE assigned name says the value
/// is a message (`lblPassword`, `Errors.Password`, `PasswordPrompt`). And a
/// value written on the line below its assignment — after a blank line too
/// — is still the assigned value.
#[test]
fn a_passphrase_assigned_to_a_secret_name_is_withheld_on_its_line_or_the_next() {
    let tmp = tempfile::tempdir().unwrap();
    let src = "namespace N;\n\npublic class Rep\n{\n    public void Run()\n    {\n        \
               password = \"correct horse battery staple\";\n        \
               var heslo = \"moje tajne heslo\";\n        \
               apiKey =\n            \"kocka-nextline-key\";\n        \
               var pwd =\n\n            \"two words nextline\";\n        \
               Show(lblPassword: \"Zadejte heslo\");\n        \
               Errors.Password = \"Heslo je příliš krátké\";\n        \
               PasswordPrompt = \"Zadejte nové heslo\";\n    }\n}\n";
    init_repo_with_files(tmp.path(), "Alice", &[("Rep.cs", src)]);
    let g = graph_over(tmp.path());
    assert_nowhere(
        &g,
        &[
            "correct horse battery staple",
            "moje tajne heslo",
            "kocka-nextline-key",
            "two words nextline",
        ],
    );
    let run = format!("{REP}.Run()");
    let mut redacted = ext_texts(&g, &run, FactKind::RedactedLiteral);
    redacted.sort();
    assert_eq!(redacted, ["password", "password", "password", "token"]);
    let texts = anchor_texts(&g);
    for message in [
        "Zadejte heslo",
        "Heslo je příliš krátké",
        "Zadejte nové heslo",
    ] {
        assert!(
            texts.contains(&(run.clone(), message.to_string(), line_of(src, message))),
            "{message} must stay an anchor: {texts:#?}"
        );
    }
}

/// R10, KTD11 rule 2: a value on the line below its assignment is the
/// assigned value when it starts at column zero too — a single word and a
/// passphrase alike — while a message assigned to a message name stays an
/// anchor there as well.
#[test]
fn a_value_at_column_zero_below_its_assignment_is_withheld() {
    let tmp = tempfile::tempdir().unwrap();
    let src = "namespace N;\n\npublic class Rep\n{\n    public void Run()\n    {\n        \
               var password =\n\"colzerosingle\";\n        \
               var pwd =\n\"two words colzero\";\n        \
               lblPassword =\n\"Zadejte heslo\";\n    }\n}\n";
    init_repo_with_files(tmp.path(), "Alice", &[("Rep.cs", src)]);
    let g = graph_over(tmp.path());
    assert_nowhere(&g, &["colzerosingle", "two words colzero"]);
    let run = format!("{REP}.Run()");
    assert_eq!(
        ext_texts(&g, &run, FactKind::RedactedLiteral),
        ["password", "password"]
    );
    assert!(
        anchor_texts(&g).contains(&(
            run.clone(),
            "Zadejte heslo".to_string(),
            line_of(src, "\"Zadejte heslo")
        )),
        "{:#?}",
        anchor_texts(&g)
    );
}

/// R10, KTD11 rule 2: a JS/TS object written with quoted keys assigns each
/// value to its key's text — `{ "password": "…" }` withholds a passphrase as
/// `password = "…"` does, and so does an indexer (`c["apiKey"] = "…"`) —
/// and a message under a message key (`"passwordPrompt"`) stays an anchor.
#[test]
fn a_passphrase_under_a_quoted_key_is_withheld_and_a_prompt_stays() {
    let tmp = tempfile::tempdir().unwrap();
    let src = "export function cfg() {\n    \
               const c = { \"password\": \"correct horse battery staple\" };\n    \
               const p = { \"passwordPrompt\": \"Zadejte heslo\" };\n    \
               c[\"apiKey\"] = \"zelena indexer fraze\";\n    \
               return [c, p];\n}\n";
    init_repo_with_files(tmp.path(), "Alice", &[("src/cfg.ts", src)]);
    let g = graph_over(tmp.path());
    assert_nowhere(
        &g,
        &["correct horse battery staple", "zelena indexer fraze"],
    );
    let cfg = "sym:ts:src/cfg.ts#cfg()";
    assert_eq!(
        ext_texts(&g, cfg, FactKind::RedactedLiteral),
        ["password", "token"]
    );
    assert!(
        anchor_texts(&g).contains(&(
            cfg.to_string(),
            "Zadejte heslo".to_string(),
            line_of(src, "Zadejte heslo")
        )),
        "{:#?}",
        anchor_texts(&g)
    );
}

/// R13: a JS/TS template literal without a hole is one fixed text, read as
/// any string is — its SQL and its message — while a template with a hole
/// stays unread.
#[test]
fn a_static_template_literal_is_read_like_a_string() {
    let tmp = tempfile::tempdir().unwrap();
    let src = "export function load(id) {\n    const q = `select name from dba.items`;\n    \
               const m = `Import selhal`;\n    \
               const h = `select name from dba.holes where id = ${id}`;\n    \
               return [q, m, h];\n}\n";
    init_repo_with_files(tmp.path(), "Alice", &[("src/load.ts", src)]);
    let g = graph_over(tmp.path());
    let load = "sym:ts:src/load.ts#load(id)";
    assert_eq!(
        db_accesses(&g),
        vec![(
            load.to_string(),
            "reads dba.items".to_string(),
            line_of(src, "dba.items")
        )]
    );
    let texts = anchor_texts(&g);
    assert!(
        texts.contains(&(
            load.to_string(),
            "Import selhal".to_string(),
            line_of(src, "Import selhal")
        )),
        "{texts:#?}"
    );
    assert!(
        !texts.iter().any(|(_, t, _)| t.contains("dba.holes")),
        "a template with a hole is read: {texts:#?}"
    );
}

/// KTD11 rule 3 holds inside a message as it does in a comment: a key-shaped
/// word of a multi-word literal is cut out and says so, and the words around
/// it stay an anchor — an `Authorization` value is written exactly like this.
#[test]
fn a_long_token_inside_a_multi_word_literal_is_cut_out_of_its_anchor() {
    let tmp = tempfile::tempdir().unwrap();
    let src = "namespace N;\n\npublic class Rep\n{\n    public void Run()\n    {\n        var auth = \"Bearer 9f2b7c11ea3d40aa9c1fdeadbeef0123\";\n        var basic = \"Basic dXNlcjpwYXNzd29yZDEyMzQ1Ng==\";\n    }\n}\n";
    init_repo_with_files(tmp.path(), "Alice", &[("Rep.cs", src)]);
    let g = graph_over(tmp.path());
    assert_nowhere(
        &g,
        &[
            "9f2b7c11ea3d40aa9c1fdeadbeef0123",
            "dXNlcjpwYXNzd29yZDEyMzQ1Ng",
        ],
    );
    let run = format!("{REP}.Run()");
    let texts = anchor_texts(&g);
    for (word, needle) in [("Bearer", "Bearer"), ("Basic", "Basic")] {
        assert!(
            texts.contains(&(run.clone(), word.to_string(), line_of(src, needle))),
            "{word} stays an anchor: {texts:#?}"
        );
    }
    assert_eq!(
        skipped(&g),
        vec![
            (
                run.clone(),
                "long token".to_string(),
                line_of(src, "Bearer")
            ),
            (run, "long token".to_string(), line_of(src, "Basic")),
        ]
    );
}

/// AE5 (R10): a connection string with a password and a `// password=…`
/// comment leave no value anywhere, while the message a support desk searches for
/// — with the word "heslo" in it — stays an anchor.
#[test]
fn the_message_chybne_heslo_survives_the_secret_filter() {
    let tmp = tempfile::tempdir().unwrap();
    let src = "namespace N;\n\npublic class Rep\n{\n    public void Login()\n    {\n        // password=Sentinel-comment-pw\n        var cs = \"Data Source=srv;User ID=sa;Password=Sentinel-conn-pw\";\n        throw new OrbitException(\"Chybné heslo\");\n    }\n}\n";
    init_repo_with_files(tmp.path(), "Alice", &[("Rep.cs", src)]);
    let g = graph_over(tmp.path());
    assert_nowhere(&g, &["Sentinel-comment-pw", "Sentinel-conn-pw"]);
    let login = format!("{REP}.Login()");
    assert!(
        anchor_texts(&g).contains(&(
            login.clone(),
            "Chybné heslo".to_string(),
            line_of(src, "Chybné heslo")
        )),
        "{:#?}",
        anchor_texts(&g)
    );
    assert_eq!(
        skipped(&g),
        vec![(
            login,
            "password".to_string(),
            line_of(src, "Sentinel-comment")
        )]
    );
}

/// KTD1: an anchor names the innermost declaration around it — in every part
/// of a partial type, and between two members written on one line.
#[test]
fn anchors_find_their_owner_in_a_second_partial_part_and_on_a_shared_line() {
    let tmp = tempfile::tempdir().unwrap();
    let first = "namespace N;\n\npublic partial class P\n{\n    public void A() { }\n}\n";
    let second = "namespace N;\n\n[Description(\"Popis druhé části\")]\npublic partial class P\n{\n    private string f = \"Výchozí hodnota pole\";\n    public void B() { Log(\"zpráva bé\"); } public void C() { Log(\"zpráva cé\"); }\n}\n";
    init_repo_with_files(tmp.path(), "Alice", &[("P1.cs", first), ("P2.cs", second)]);
    let g = graph_over(tmp.path());
    let texts = anchor_texts(&g);
    let expect = |owner: &str, text: &str| {
        let row = (owner.to_string(), text.to_string(), line_of(second, text));
        assert!(texts.contains(&row), "{row:?} missing: {texts:#?}");
    };
    expect("sym:csharp:N#P", "Popis druhé části");
    expect("sym:csharp:N#P.f", "Výchozí hodnota pole");
    expect("sym:csharp:N#P.B()", "zpráva bé");
    expect("sym:csharp:N#P.C()", "zpráva cé");
}

/// KTD1: one text repeated is one anchor with a count and its first line.
#[test]
fn a_repeated_literal_is_one_anchor_with_its_count() {
    let tmp = tempfile::tempdir().unwrap();
    let mut src =
        String::from("namespace N;\n\npublic class Rep\n{\n    public void Log()\n    {\n");
    for _ in 0..300 {
        src.push_str("        Write(\"Error\");\n");
    }
    src.push_str("    }\n}\n");
    init_repo_with_files(tmp.path(), "Alice", &[("Rep.cs", &src)]);
    let g = graph_over(tmp.path());
    let found: Vec<(String, AnchorText, u32)> = anchors(&g)
        .into_iter()
        .filter(|(_, a, _)| a.text == "Error")
        .collect();
    assert_eq!(found.len(), 1, "{found:#?}");
    let (owner, anchor, line) = &found[0];
    assert_eq!(owner, &format!("{REP}.Log()"));
    assert_eq!(anchor.occurrences, 300);
    assert_eq!(*line, 7);
}

/// KTD1: a text longer than the cap is cut, and says how long it was.
#[test]
fn a_long_literal_is_cut_and_keeps_its_length() {
    let tmp = tempfile::tempdir().unwrap();
    let long = "Obnova zálohy selhala ".repeat(10);
    let long = long.trim_end();
    let src = format!(
        "namespace N;\n\npublic class Rep\n{{\n    public void Run()\n    {{\n        Fail(\"{long}\");\n    }}\n}}\n"
    );
    init_repo_with_files(tmp.path(), "Alice", &[("Rep.cs", &src)]);
    let g = graph_over(tmp.path());
    let found = anchors(&g);
    let (_, anchor, _) = found
        .iter()
        .find(|(_, a, _)| a.text.starts_with("Obnova zálohy"))
        .unwrap_or_else(|| panic!("{found:#?}"));
    assert_eq!(anchor.length, long.chars().count() as u32);
    assert_eq!(anchor.text.chars().count(), ANCHOR_TEXT_CAP);
    assert!(long.starts_with(&anchor.text));
}

/// KTD1: past the cap, a multi-word text outranks single words, lower lines
/// outrank higher ones, and what is dropped is counted.
#[test]
fn the_anchor_cap_keeps_messages_before_words_and_counts_the_rest() {
    let tmp = tempfile::tempdir().unwrap();
    let mut src =
        String::from("namespace N;\n\npublic class Rep\n{\n    public void Run()\n    {\n");
    let words = ANCHOR_CAP + 12;
    for i in 0..words {
        src.push_str(&format!("        Use(\"Slovo{i:03}x\");\n"));
    }
    src.push_str("        Fail(\"Úloha agenta se nespustila\");\n    }\n}\n");
    init_repo_with_files(tmp.path(), "Alice", &[("Rep.cs", &src)]);
    let g = graph_over(tmp.path());
    let run = format!("{REP}.Run()");
    let kept: Vec<String> = anchor_texts(&g)
        .into_iter()
        .filter(|(o, _, _)| *o == run)
        .map(|(_, t, _)| t)
        .collect();
    assert_eq!(kept.len(), ANCHOR_CAP);
    assert!(
        kept.contains(&"Úloha agenta se nespustila".to_string()),
        "{kept:#?}"
    );
    // The single words kept are those on the lowest lines.
    assert!(kept.contains(&"Slovo000x".to_string()));
    assert!(kept.contains(&format!("Slovo{:03}x", ANCHOR_CAP - 2)));
    assert!(!kept.contains(&format!("Slovo{:03}x", ANCHOR_CAP - 1)));
    let dropped: Vec<i64> = g
        .node(&NodeKey::new(run.as_str()))
        .unwrap()
        .facts
        .iter()
        .filter(|f| f.kind == FactKind::AnchorsDropped)
        .map(|f| match f.value {
            FactValue::Int(n) => n,
            ref other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(dropped, vec![(words + 1 - ANCHOR_CAP) as i64]);
}

/// KTD1: anchors are the detail's, never the manifest's.
#[test]
fn the_manifest_carries_no_anchor() {
    let tmp = tempfile::tempdir().unwrap();
    let src = "namespace N;\n\npublic class Rep\n{\n    public void Restore()\n    {\n        // Ruční zásah do plánovače\n        throw new OrbitException(\"Chyba při obnově databáze\");\n    }\n}\n";
    init_repo_with_files(tmp.path(), "Alice", &[("Rep.cs", src)]);
    let g = graph_over(tmp.path());
    assert!(!anchors(&g).is_empty(), "control: the graph has anchors");
    let manifest = reverse_architect::manifest::build_manifest(&g, None).unwrap();
    let json = reverse_architect::manifest::to_json(&manifest);
    for absent in ["\"anchor", "Chyba při obnově", "Ruční zásah"] {
        assert!(!json.contains(absent), "`{absent}` reached the manifest");
    }
}

/// AE1, on the real shape of `LoadAction.SetScheduledTasksInvalid`: its SQL is
/// a C# raw literal (`"""…"""`), which the lexer used to read as an empty
/// string followed by code.
#[test]
fn sql_in_a_csharp_raw_literal_is_read() {
    let tmp = tempfile::tempdir().unwrap();
    let src = "namespace Orbit.Backup;\n\npublic class LoadAction\n{\n    public void SetScheduledTasksInvalid(AgConnection agConnection)\n    {\n        agConnection.OrbitExecute(\"\"\"\n            begin try\n              update dba.ops_schedule_tasks set validity = 0, xcmd = 'I-#AGENT;' from dba.ops_schedule_tasks where 1=1\n            end try\n            begin catch\n            end catch\n            \"\"\");\n    }\n}\n";
    init_repo_with_files(tmp.path(), "Alice", &[("LoadAction.cs", src)]);
    let g = graph_over(tmp.path());
    let method = "sym:csharp:Orbit.Backup#LoadAction.SetScheduledTasksInvalid(AgConnection)";
    let update = line_of(src, "update dba.ops_schedule_tasks");
    // The statement names the table twice — as its target and after FROM —
    // and each occurrence is an access (U2).
    let write = (
        method.to_string(),
        "writes dba.ops_schedule_tasks".to_string(),
        update,
    );
    assert_eq!(db_accesses(&g), vec![write.clone(), write]);
    assert!(
        ext_uses(&g).contains(&(
            method.to_string(),
            "res:db:dba.ops_schedule_tasks".to_string()
        )),
        "{:#?}",
        ext_uses(&g)
    );
    let texts = anchor_texts(&g);
    assert!(
        texts.iter().any(|(o, t, line)| o == method
            && t.contains("update dba.ops_schedule_tasks set validity = 0")
            && *line == line_of(src, "OrbitExecute")),
        "{texts:#?}"
    );
}

/// An interpolated literal is read by its text: its holes are blanked, never
/// quoted, and an object a hole is glued to is no object at all.
#[test]
fn an_interpolated_literal_is_read_without_its_holes() {
    let tmp = tempfile::tempdir().unwrap();
    let src = "namespace N;\n\npublic class Rep\n{\n    public void Mark(int id)\n    {\n        Exec($\"update dba.t set x = {id} where y = 1\");\n        Exec($@\"select a\n            from dba.u where z = {Pick(\"dba.decoy\", '\"')}\");\n        Exec($\"select * from dba.v{suffix}\");\n        Exec($$\"\"\"select {{hole}} from dba.w where s = '{}'\"\"\");\n    }\n}\n";
    init_repo_with_files(tmp.path(), "Alice", &[("Rep.cs", src)]);
    let g = graph_over(tmp.path());
    let mark = format!("{REP}.Mark(int)");
    assert_eq!(
        db_accesses(&g),
        vec![
            (
                mark.clone(),
                "reads dba.u".to_string(),
                line_of(src, "from dba.u")
            ),
            (
                mark.clone(),
                "reads dba.w".to_string(),
                line_of(src, "from dba.w")
            ),
            (
                mark.clone(),
                "writes dba.t".to_string(),
                line_of(src, "update dba.t")
            ),
        ]
    );
    let texts = anchor_texts(&g);
    assert!(
        texts.contains(&(
            mark.clone(),
            "update dba.t set x = where y = 1".to_string(),
            line_of(src, "update dba.t")
        )),
        "{texts:#?}"
    );
    for (_, text, _) in &texts {
        for code in ["id}", "Pick", "decoy", "hole", "suffix"] {
            assert!(
                !text.contains(code),
                "hole code `{code}` quoted in {text:?}"
            );
        }
    }
}

// ---- C and C++ through the host lexer (plan B, U4: R13, KTD1) ----

/// R13: C and C++ have no plugin, so their texts belong to the file — and are
/// read all the same: database accesses, anchors, and the secret filter.
#[test]
fn c_and_cpp_files_give_file_owned_accesses_and_anchors() {
    let tmp = tempfile::tempdir().unwrap();
    let c = "#include \"q.h\"\n\nvoid run(void) {\n    const char* q = \"update dba.t set x=1\";\n    exec_sql(q);\n}\n";
    let cpp =
        "#include <string>\n\nstd::string load() {\n    return R\"(select * from dba.a)\";\n}\n";
    let h = "/* deployment notes\n   password: hunter2-header\n*/\n#define TIMEOUT 30\n// Chyba spojení s databází\n";
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[("native/q.c", c), ("native/r.cpp", cpp), ("native/k.h", h)],
    );
    let g = graph_over(tmp.path());
    assert_eq!(
        db_accesses(&g),
        vec![
            (
                "native/q.c".to_string(),
                "writes dba.t".to_string(),
                line_of(c, "update dba.t")
            ),
            (
                "native/r.cpp".to_string(),
                "reads dba.a".to_string(),
                line_of(cpp, "select *")
            ),
        ]
    );
    for (file, used) in [
        ("native/q.c", "res:db:dba.t"),
        ("native/r.cpp", "res:db:dba.a"),
    ] {
        assert!(
            ext_uses(&g).contains(&(file.to_string(), used.to_string())),
            "{:#?}",
            ext_uses(&g)
        );
    }
    let texts = anchor_texts(&g);
    for (file, text, line) in [
        ("native/k.h", "Chyba spojení s databází", 5),
        ("native/k.h", "deployment notes", 1),
        ("native/q.c", "update dba.t set x=1", 4),
    ] {
        let row = (file.to_string(), text.to_string(), line);
        assert!(texts.contains(&row), "{row:?} missing: {texts:#?}");
    }
    assert_nowhere(&g, &["hunter2-header"]);
    assert_eq!(
        skipped(&g),
        vec![("native/k.h".to_string(), "password".to_string(), 2)]
    );
}

// ---- Decisions by an enum's value (plan B, U5: R12, R13, AE6, KTD3) ----

use reverse_architect::graph::EnumDecision;

/// Every `EnumDecision` fact in the graph as `(owner, value, line)`, sorted.
/// The value is checked to decode, then compared as written.
fn decisions(g: &Graph) -> Vec<(String, String, u32)> {
    let mut out: Vec<(String, String, u32)> = g
        .nodes_sorted()
        .iter()
        .flat_map(|n| {
            n.facts
                .iter()
                .filter(|f| f.kind == FactKind::EnumDecision)
                .map(|f| {
                    let FactValue::Text(text) = &f.value else {
                        panic!("an EnumDecision fact is text: {f:?}");
                    };
                    assert!(EnumDecision::decode(text).is_some(), "{text}");
                    (
                        n.key.as_str().to_string(),
                        text.clone(),
                        f.provenance.span.start().expect("a decision has a line"),
                    )
                })
                .collect::<Vec<_>>()
        })
        .collect();
    out.sort();
    out
}

/// The `EnumDecisionsUnmatched` counts on a file node.
fn unmatched(g: &Graph, file: &str) -> Vec<i64> {
    g.node(&NodeKey::new(file))
        .unwrap_or_else(|| panic!("{file} is not in the graph"))
        .facts
        .iter()
        .filter(|f| f.kind == FactKind::EnumDecisionsUnmatched)
        .map(|f| match f.value {
            FactValue::Int(n) => n,
            ref other => panic!("{other:?}"),
        })
        .collect()
}

fn decision(owner: &str, value: &str, src: &str, needle: &str) -> (String, String, u32) {
    (owner.to_string(), value.to_string(), line_of(src, needle))
}

const BACKUP_LOAD_TYPE_CS: &str = "namespace Orbit.Actions.Backup;\n\npublic enum BackupLoadType\n{\n    Never = 0,\n    Full = 10,\n    Diff = 20,\n}\n";

/// AE6: the enum's values decide in `LoadAction` — a `case` label (with a
/// `when` clause too) and a switch expression arm are verified places; a
/// comparison of any form is a place, never a verified one; `goto case` is a
/// jump, not a decision.
#[test]
fn case_labels_and_switch_arms_on_an_enum_are_verified_decisions() {
    let tmp = tempfile::tempdir().unwrap();
    let load_action = "namespace Orbit.Actions.Backup;

public class LoadAction
{
    public void Run()
    {
        var t = Load();
        switch (t)
        {
            case BackupLoadType.Full:
                break;
            case BackupLoadType.Never when Ready():
                break;
            default:
                goto case BackupLoadType.Full;
        }
    }

    public int Weight()
    {
        var t = Load();
        return t switch
        {
            BackupLoadType.Full => 1,
            // A comment line above an arm leaves it an arm.
            BackupLoadType.Diff when Ready() => 2,
            _ => 0,
        };
    }

    public bool Changed()
    {
        var t = Load();
        if (t == BackupLoadType.Diff) return true;
        if (BackupLoadType.Full != t) return false;
        if (t is BackupLoadType.Diff) return true;
        return t is not BackupLoadType.Never;
    }
}
";
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            ("Backup/BackupLoadType.cs", BACKUP_LOAD_TYPE_CS),
            ("Backup/LoadAction.cs", load_action),
        ],
    );
    let g = graph_over(tmp.path());
    let e = "sym:csharp:Orbit.Actions.Backup#BackupLoadType";
    let run = "sym:csharp:Orbit.Actions.Backup#LoadAction.Run()";
    let weight = "sym:csharp:Orbit.Actions.Backup#LoadAction.Weight()";
    let changed = "sym:csharp:Orbit.Actions.Backup#LoadAction.Changed()";
    let src = load_action;
    let mut expected = vec![
        decision(
            run,
            &format!("verified label {e} Full"),
            src,
            "case BackupLoadType.Full:",
        ),
        decision(
            run,
            &format!("verified label {e} Never"),
            src,
            "case BackupLoadType.Never",
        ),
        decision(
            weight,
            &format!("verified arm {e} Full"),
            src,
            "BackupLoadType.Full => 1",
        ),
        decision(
            weight,
            &format!("verified arm {e} Diff"),
            src,
            "BackupLoadType.Diff when",
        ),
        decision(
            changed,
            &format!("unverified comparison {e} Diff"),
            src,
            "t == BackupLoadType.Diff",
        ),
        decision(
            changed,
            &format!("unverified comparison {e} Full"),
            src,
            "BackupLoadType.Full != t",
        ),
        decision(
            changed,
            &format!("unverified comparison {e} Diff"),
            src,
            "t is BackupLoadType.Diff",
        ),
        decision(
            changed,
            &format!("unverified comparison {e} Never"),
            src,
            "is not BackupLoadType.Never",
        ),
    ];
    expected.sort();
    assert_eq!(decisions(&g), expected);
    assert!(unmatched(&g, "Backup/LoadAction.cs").is_empty());
}

/// A qualified constant in a `case` label is no enum's value: nothing is
/// said about it, and it is not counted as an unmatched decision either —
/// its qualifier names no enum at all.
#[test]
fn a_case_on_a_class_constant_is_no_decision() {
    let tmp = tempfile::tempdir().unwrap();
    let src = "namespace N;

public static class Consts
{
    public const int Max = 9;
}

public class W
{
    public void Run(int v)
    {
        switch (v)
        {
            case Consts.Max:
                break;
        }
        if (v == Consts.Max) { }
    }
}
";
    init_repo_with_files(tmp.path(), "Alice", &[("W.cs", src)]);
    let g = graph_over(tmp.path());
    assert_eq!(decisions(&g), Vec::new());
    assert!(unmatched(&g, "W.cs").is_empty());
}

/// A namespace-less C# file is keyed by its path stem, and a path may hold a
/// space — so an enum's node key can too. Its decisions are written with
/// that key and must read back, or the detail and coverage drop them all.
#[test]
fn an_enum_keyed_by_a_path_with_a_space_keeps_its_decisions() {
    let tmp = tempfile::tempdir().unwrap();
    let status = "public enum Status\n{\n    Open,\n    Closed,\n}\n";
    let gate = "public class Gate\n{\n    public int Run(Status s)\n    {\n        switch (s)\n        {\n            case Status.Open:\n                return 1;\n        }\n        return 0;\n    }\n}\n";
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            ("Shared Code/Status.cs", status),
            ("Shared Code/Gate.cs", gate),
        ],
    );
    let g = graph_over(tmp.path());
    let e = NodeKey::new("sym:csharp:Shared Code/Status#Status");
    assert!(g.node(&e).is_some(), "the enum is keyed with the space");
    let found = decisions(&g);
    assert_eq!(found.len(), 1, "{found:#?}");
    let decision = EnumDecision::decode(&found[0].1).expect("decodes");
    assert_eq!(decision.enum_key, e);
    assert_eq!(decision.member, "Open");
    assert_eq!(EnumDecision::decode(&decision.encode()), Some(decision));
}

const LIB_MODE_CS: &str = "namespace Lib.Enums;\n\npublic enum Mode\n{\n    Full,\n    Diff,\n}\n";

/// A `Run(int m)` method of a class in `App` holding `case Mode.Full:` and
/// `m == Mode.Full`, preceded by `head` (usings) and with `members` inside
/// the class.
fn mode_worker(head: &str, class: &str, members: &str) -> String {
    format!(
        "{head}namespace App;

public class {class}
{{
{members}    public void Run(int m)
    {{
        switch (m)
        {{
            case Mode.Full:
                break;
        }}
        if (m == Mode.Full) {{ }}
    }}
}}
"
    )
}

/// KTD3: an alias named like the enum that points elsewhere makes the label
/// unverified — in C# the alias wins over the imported namespace, so the
/// place may well not be the enum's at all. The comparison beside it is
/// never anything but unverified.
#[test]
fn an_alias_pointing_elsewhere_leaves_the_label_unverified() {
    let tmp = tempfile::tempdir().unwrap();
    let worker = mode_worker(
        "using Lib.Enums;\nusing Mode = Other.Kind;\n\n",
        "Worker",
        "",
    );
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            ("Lib/Mode.cs", LIB_MODE_CS),
            (
                "Other/Kind.cs",
                "namespace Other;\n\npublic static class Kind\n{\n    public const int Full = 1;\n}\n",
            ),
            ("App/Worker.cs", &worker),
        ],
    );
    let g = graph_over(tmp.path());
    let run = "sym:csharp:App#Worker.Run(int)";
    let mut expected = vec![
        decision(
            run,
            "unverified label sym:csharp:Lib.Enums#Mode Full",
            &worker,
            "case Mode.Full",
        ),
        decision(
            run,
            "unverified comparison sym:csharp:Lib.Enums#Mode Full",
            &worker,
            "m == Mode.Full",
        ),
    ];
    expected.sort();
    assert_eq!(decisions(&g), expected);
}

/// KTD3: a class `Mode` in the site's own namespace is the `Mode` the label
/// means, while the enum lies in a namespace the file does not import — the
/// place is shown on the enum, unverified.
#[test]
fn a_class_of_the_same_name_in_the_sites_namespace_leaves_the_label_unverified() {
    let tmp = tempfile::tempdir().unwrap();
    let worker = mode_worker("", "Worker", "");
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            ("Lib/Mode.cs", LIB_MODE_CS),
            (
                "App/Mode.cs",
                "namespace App;\n\npublic static class Mode\n{\n    public const int Full = 1;\n}\n",
            ),
            ("App/Worker.cs", &worker),
        ],
    );
    let g = graph_over(tmp.path());
    let labels: Vec<String> = decisions(&g)
        .into_iter()
        .filter(|(_, v, _)| v.contains(" label "))
        .map(|(_, v, _)| v)
        .collect();
    assert_eq!(
        labels,
        vec!["unverified label sym:csharp:Lib.Enums#Mode Full".to_string()]
    );
}

/// KTD3, the regression the owner settled: a class nested in the enclosing
/// type — or in a type it derives from — named like an imported enum, with a
/// constant named like the enum's value, is what `case Mode.Full:` means
/// there. The label stays unverified. A class with no such nested type, in
/// the same namespace with the same import, is verified: the import alone
/// does reach the enum.
#[test]
fn a_nested_class_shadowing_an_imported_enum_leaves_the_label_unverified() {
    let tmp = tempfile::tempdir().unwrap();
    let nested =
        "    public static class Mode\n    {\n        public const int Full = 1;\n    }\n\n";
    let worker = mode_worker("using Lib.Enums;\n\n", "Worker", nested);
    let derived = mode_worker("using Lib.Enums;\n\n", "Derived : Base", "");
    let plain = mode_worker("using Lib.Enums;\n\n", "Plain", "");
    let base = format!("namespace App;\n\npublic class Base\n{{\n{nested}}}\n");
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            ("Lib/Mode.cs", LIB_MODE_CS),
            ("App/Worker.cs", &worker),
            ("App/Base.cs", &base),
            ("App/Derived.cs", &derived),
            ("App/Plain.cs", &plain),
        ],
    );
    let g = graph_over(tmp.path());
    let labels: Vec<(String, String)> = decisions(&g)
        .into_iter()
        .filter(|(_, v, _)| v.contains(" label "))
        .map(|(o, v, _)| (o, v))
        .collect();
    let e = "sym:csharp:Lib.Enums#Mode";
    assert_eq!(
        labels,
        vec![
            (
                "sym:csharp:App#Derived.Run(int)".to_string(),
                format!("unverified label {e} Full")
            ),
            (
                "sym:csharp:App#Plain.Run(int)".to_string(),
                format!("verified label {e} Full")
            ),
            (
                "sym:csharp:App#Worker.Run(int)".to_string(),
                format!("unverified label {e} Full")
            ),
        ]
    );
}

/// KTD3: two enums named `Mode`, both with `Full`. Where neither is visible
/// the place cannot be tied to one and leaves no fact — only a count on its
/// file; where the file imports one of them, the label is that one's.
#[test]
fn two_enums_of_one_name_decide_nothing_without_a_unique_match() {
    let tmp = tempfile::tempdir().unwrap();
    let neither = mode_worker("", "Neither", "");
    let only_a = mode_worker("using A;\n\n", "OnlyA", "");
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            (
                "A/Mode.cs",
                "namespace A;\n\npublic enum Mode\n{\n    Full,\n}\n",
            ),
            (
                "B/Mode.cs",
                "namespace B;\n\npublic enum Mode\n{\n    Full,\n}\n",
            ),
            ("App/Neither.cs", &neither),
            ("App/OnlyA.cs", &only_a),
        ],
    );
    let g = graph_over(tmp.path());
    let run = "sym:csharp:App#OnlyA.Run(int)";
    let mut expected = vec![
        decision(
            run,
            "verified label sym:csharp:A#Mode Full",
            &only_a,
            "case Mode.Full",
        ),
        decision(
            run,
            "unverified comparison sym:csharp:A#Mode Full",
            &only_a,
            "m == Mode.Full",
        ),
    ];
    expected.sort();
    assert_eq!(decisions(&g), expected);
    // The label and the comparison of `Neither`.
    assert_eq!(unmatched(&g, "App/Neither.cs"), vec![2]);
    assert!(unmatched(&g, "App/OnlyA.cs").is_empty());
}

/// Two enums that share a value's name are told apart by the qualifier.
#[test]
fn two_enums_sharing_a_value_name_are_told_apart_by_the_qualifier() {
    let tmp = tempfile::tempdir().unwrap();
    let src = "namespace N;

public enum Color
{
    Red,
    Green,
}

public enum Alert
{
    Red,
    Amber,
}

public class Painter
{
    public void Paint(Color c, Alert a)
    {
        switch (c)
        {
            case Color.Red:
                break;
        }
        switch (a)
        {
            case Alert.Red:
                break;
        }
    }
}
";
    init_repo_with_files(tmp.path(), "Alice", &[("Painter.cs", src)]);
    let g = graph_over(tmp.path());
    let paint = "sym:csharp:N#Painter.Paint(Color,Alert)";
    let mut expected = vec![
        decision(
            paint,
            "verified label sym:csharp:N#Color Red",
            src,
            "case Color.Red",
        ),
        decision(
            paint,
            "verified label sym:csharp:N#Alert Red",
            src,
            "case Alert.Red",
        ),
    ];
    expected.sort();
    assert_eq!(decisions(&g), expected);
}

/// A `global using` in another file of the project imports the enum's
/// namespace as surely as a `using` in the file does — read the way the
/// shared resolver reads the project-wide usings (plan A, KTD6).
#[test]
fn a_global_using_elsewhere_makes_the_enum_visible() {
    let tmp = tempfile::tempdir().unwrap();
    let worker = mode_worker("", "Worker", "");
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            ("GlobalUsings.cs", "global using Lib.Enums;\n"),
            ("Lib/Mode.cs", LIB_MODE_CS),
            ("App/Worker.cs", &worker),
        ],
    );
    let g = graph_over(tmp.path());
    let labels: Vec<String> = decisions(&g)
        .into_iter()
        .filter(|(_, v, _)| v.contains(" label "))
        .map(|(_, v, _)| v)
        .collect();
    assert_eq!(
        labels,
        vec!["verified label sym:csharp:Lib.Enums#Mode Full".to_string()]
    );
}

/// KTD3: an alias of another name (`using BLT = …BackupLoadType;`) stands for
/// the enum it points at, so `case BLT.Full:` is that enum's decision — read
/// through the alias, not dropped because no enum is called `BLT`.
#[test]
fn a_decision_through_an_alias_of_another_name_is_the_enums() {
    let tmp = tempfile::tempdir().unwrap();
    let worker = "using BLT = Orbit.Actions.Backup.BackupLoadType;

namespace App;

public class Worker
{
    public void Run(int m)
    {
        switch (m)
        {
            case BLT.Full:
                break;
        }
        if (m == BLT.Diff) { }
    }
}
";
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            ("Backup/BackupLoadType.cs", BACKUP_LOAD_TYPE_CS),
            ("App/Worker.cs", worker),
        ],
    );
    let g = graph_over(tmp.path());
    let e = "sym:csharp:Orbit.Actions.Backup#BackupLoadType";
    let run = "sym:csharp:App#Worker.Run(int)";
    let mut expected = vec![
        decision(
            run,
            &format!("verified label {e} Full"),
            worker,
            "case BLT.Full",
        ),
        decision(
            run,
            &format!("unverified comparison {e} Diff"),
            worker,
            "m == BLT.Diff",
        ),
    ];
    expected.sort();
    assert_eq!(decisions(&g), expected);
    assert!(unmatched(&g, "App/Worker.cs").is_empty());
}

/// The decisions are the detail's, never the manifest's (like the anchors).
#[test]
fn the_manifest_carries_no_enum_decision() {
    let tmp = tempfile::tempdir().unwrap();
    let neither = mode_worker("", "Neither", "");
    let only_a = mode_worker("using A;\n\n", "OnlyA", "");
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            (
                "A/Mode.cs",
                "namespace A;\n\npublic enum Mode\n{\n    Full,\n}\n",
            ),
            (
                "B/Mode.cs",
                "namespace B;\n\npublic enum Mode\n{\n    Full,\n}\n",
            ),
            ("App/Neither.cs", &neither),
            ("App/OnlyA.cs", &only_a),
        ],
    );
    let g = graph_over(tmp.path());
    assert!(
        !decisions(&g).is_empty(),
        "control: the graph has decisions"
    );
    assert!(!unmatched(&g, "App/Neither.cs").is_empty(), "control");
    let manifest = reverse_architect::manifest::build_manifest(&g, None).unwrap();
    let json = reverse_architect::manifest::to_json(&manifest);
    for absent in ["enum_decision", "verified label"] {
        assert!(!json.contains(absent), "`{absent}` reached the manifest");
    }
}
