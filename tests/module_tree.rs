//! U4: snapshot-independent invariant tests for the module-structure tree (R6,
//! Success criteria), plus a deep+wide pathological fixture that MEASURES the
//! indent boundary for the deferred depth-cap decision (KTD5).
//!
//! R2 rewrites every module-structure snapshot, so a "changed snapshot =
//! regression" review cannot catch a dropped module or an off-by-one count.
//! These invariants are the real guard: they assert structural facts about the
//! rendered tree against an INDEPENDENT re-derivation from the graph, never
//! against a golden blob. `recursive_module_file_counts` is `pub(crate)`, so
//! invariant (c) re-derives the recursive counts here from the File nodes rather
//! than calling the helper — an independent oracle is stronger than reusing the
//! code under test.

mod common;

use common::init_repo_with_files;
use git2::{IndexEntry, IndexTime, Repository};
use reverse_architect::cli::LinkStyle;
use reverse_architect::extract::{run_all, Repo};
use reverse_architect::graph::{Graph, NodeKind};
use reverse_architect::manifest::build_manifest;
use reverse_architect::render::build_corpus;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// Build the graph and its corpus over a temp repo — the real pipeline, so the
/// invariants hold over exactly what a user renders.
fn graph_and_leaf(dir: &Path) -> (Graph, String) {
    let repo = Repo::open(dir).unwrap();
    let mut graph = Graph::new();
    run_all(&repo, &mut graph).unwrap();
    let corpus = build_corpus(&graph, LinkStyle::Portable, None).unwrap();
    let leaf = corpus
        .files
        .iter()
        .find(|f| f.path == "module-structure.md")
        .expect("module-structure leaf present")
        .contents
        .clone();
    (graph, leaf)
}

/// The bullet lines of the `In this repository` section ONLY — bounded at the
/// next `## ` heading so a `## Failure modes` section (U12 low-cohesion smell)
/// never leaks its bullets into the module count.
fn instance_lines(leaf: &str) -> Vec<String> {
    let after = leaf
        .split("## In this repository")
        .nth(1)
        .expect("instances section");
    let section = after.split("\n## ").next().unwrap_or(after);
    section
        .lines()
        .filter(|l| l.trim_start().starts_with("- "))
        .map(|l| l.to_string())
        .collect()
}

/// Parse a rendered module line into `(key, indent_level, recursive_file_count)`.
/// A container reads `` `k` — N entries, M files ``; a leaf `` `k` — M files ``.
/// In both, the number immediately before `file`/`files` is the recursive count.
fn parse_module_line(line: &str) -> (String, usize, usize) {
    let indent = (line.len() - line.trim_start().len()) / 2;
    let body = line
        .trim_start()
        .strip_prefix("- `")
        .expect("bullet with a `key`");
    let (key, rest) = body.split_once('`').expect("closing backtick");
    let words: Vec<&str> = rest.split_whitespace().collect();
    let idx = words
        .iter()
        .position(|w| *w == "file" || *w == "files")
        .expect("a file count word");
    let count: usize = words[idx - 1].parse().expect("numeric file count");
    (key.to_string(), indent, count)
}

/// Independent re-derivation of the U1 recursive counts: credit every file to
/// each of its ancestor directories. This is the oracle invariant (c) checks the
/// rendered numbers against — deliberately NOT the helper under test.
fn derived_recursive_counts(graph: &Graph) -> BTreeMap<String, usize> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for node in graph.nodes_sorted() {
        if node.kind != NodeKind::File {
            continue;
        }
        let path = node.key.as_str();
        let mut end = path.len();
        while let Some(slash) = path[..end].rfind('/') {
            *counts.entry(path[..slash].to_string()).or_insert(0) += 1;
            end = slash;
        }
    }
    counts
}

fn module_count_excluding_root(graph: &Graph) -> usize {
    graph
        .nodes_sorted()
        .iter()
        .filter(|n| n.kind == NodeKind::Module && n.key.as_str() != ".")
        .count()
}

fn file_count(graph: &Graph) -> usize {
    graph
        .nodes_sorted()
        .iter()
        .filter(|n| n.kind == NodeKind::File)
        .count()
}

/// A fixture WITH root-level files (`Cargo.toml`, `README.md`) — the case an
/// earlier draft of invariant (b) got wrong, because root files hang off `.`,
/// not off any rendered root module.
fn fixture_with_root_files(dir: &Path) {
    init_repo_with_files(
        dir,
        "Alice",
        &[
            ("Cargo.toml", "[package]\nname = \"x\"\n"),
            ("README.md", "# x"),
            ("src/lib.rs", "// lib"),
            ("src/extract/git.rs", "// git"),
            ("docs/intro.md", "# intro"),
        ],
    );
}

/// Invariant (a): exactly one rendered module line per module node (excluding the
/// synthetic root `.`) — no module dropped, none duplicated, and failure-mode
/// bullets excluded by the section bound.
#[test]
fn every_module_is_rendered_exactly_once() {
    let tmp = tempfile::tempdir().unwrap();
    fixture_with_root_files(tmp.path());
    let (graph, leaf) = graph_and_leaf(tmp.path());

    let lines = instance_lines(&leaf);
    assert_eq!(
        lines.len(),
        module_count_excluding_root(&graph),
        "one line per non-root module: {lines:?}"
    );

    // Uniqueness: no key appears twice.
    let mut keys: Vec<String> = lines.iter().map(|l| parse_module_line(l).0).collect();
    keys.sort();
    let unique = keys.len();
    keys.dedup();
    assert_eq!(keys.len(), unique, "no module rendered twice");
}

/// Invariant (b): the recursive file counts of the ROOT modules, plus the files
/// held directly at the repo root (under `.`), account for every file node.
#[test]
fn root_module_counts_plus_root_files_cover_all_files() {
    let tmp = tempfile::tempdir().unwrap();
    fixture_with_root_files(tmp.path());
    let (graph, _leaf) = graph_and_leaf(tmp.path());

    let derived = derived_recursive_counts(&graph);
    let root_module_sum: usize = derived
        .iter()
        .filter(|(k, _)| !k.contains('/')) // top-level modules
        .map(|(_, c)| *c)
        .sum();
    let root_files = graph
        .nodes_sorted()
        .iter()
        .filter(|n| n.kind == NodeKind::File && !n.key.as_str().contains('/'))
        .count();

    assert_eq!(
        root_module_sum + root_files,
        file_count(&graph),
        "root subtrees ({root_module_sum}) + root files ({root_files}) == all files"
    );
}

/// Invariant (c): every recursive file count rendered in the tree equals the
/// independently derived count for that module.
#[test]
fn rendered_counts_match_the_derived_recursive_counts() {
    let tmp = tempfile::tempdir().unwrap();
    fixture_with_root_files(tmp.path());
    let (graph, leaf) = graph_and_leaf(tmp.path());

    let derived = derived_recursive_counts(&graph);
    for line in instance_lines(&leaf) {
        let (key, _indent, rendered) = parse_module_line(&line);
        assert_eq!(
            Some(&rendered),
            derived.get(&key),
            "rendered recursive count for `{key}` matches the derivation: {line}"
        );
    }
}

/// The instances section counts only tree lines, not the `## Failure modes`
/// bullets a low-cohesion module surfaces (the (a) trap). A module large and
/// incoherent enough to trip the U12 smell must still count as exactly one line.
#[test]
fn failure_mode_bullets_are_not_counted_as_modules() {
    let tmp = tempfile::tempdir().unwrap();
    // Mirrors the U12 end-to-end fixture: a low-cohesion module above the size
    // guard, plus a below-guard module with a real import so the zero-signal
    // guard does not suppress the smell.
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            ("src/widgets/a.rs", "// a"),
            ("src/widgets/b.rs", "// b"),
            ("src/widgets/c.rs", "// c"),
            ("src/sig/s0.rs", "use crate::sig::s1;\n"),
            ("src/sig/s1.rs", "// s1"),
        ],
    );
    let (graph, leaf) = graph_and_leaf(tmp.path());

    assert!(
        leaf.contains("## Failure modes"),
        "the smell fires on this fixture: {leaf}"
    );
    let lines = instance_lines(&leaf);
    assert_eq!(
        lines.len(),
        module_count_excluding_root(&graph),
        "the failure-mode bullets are excluded from the module count: {lines:?}"
    );
}

/// Add a tracked symlink index entry (mode 0o120000) at `path` — a hand-built
/// entry, not a real FS symlink, so it works on Windows/CI (mirrors the helper
/// in `tests/symlink.rs`).
fn add_symlink(repo: &Repository, path: &str, target: &str) {
    let oid = repo.blob(target.as_bytes()).unwrap();
    let mut index = repo.index().unwrap();
    index
        .add(&IndexEntry {
            ctime: IndexTime::new(0, 0),
            mtime: IndexTime::new(0, 0),
            dev: 0,
            ino: 0,
            mode: 0o120000,
            uid: 0,
            gid: 0,
            file_size: target.len() as u32,
            id: oid,
            flags: 0,
            flags_extended: 0,
            path: path.as_bytes().to_vec(),
        })
        .unwrap();
    index.write().unwrap();
}

/// A symlink is a tracked File node (only its content extraction is skipped), so
/// the module holding it counts it like any file — the tree needs no symlink
/// special-casing. Gitlinks never become File nodes (excluded at enumeration),
/// so they never inflate a count. This exercises the plan's symlink scenario on
/// the real path rather than inheriting it as an assumption.
#[test]
fn a_symlink_under_a_module_is_counted_as_a_file() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = init_repo_with_files(tmp.path(), "Alice", &[("core/lib.rs", "// lib")]);
    add_symlink(&repo, "core/link.rs", "lib.rs");

    let (graph, leaf) = graph_and_leaf(tmp.path());

    // `core` holds two files (the real source + the symlink).
    let derived = derived_recursive_counts(&graph);
    assert_eq!(
        derived.get("core"),
        Some(&2),
        "symlink counted in the derivation"
    );
    let core = instance_lines(&leaf)
        .into_iter()
        .find(|l| l.contains("`core`"))
        .expect("core module rendered");
    assert_eq!(
        parse_module_line(&core).2,
        2,
        "rendered count includes the symlink: {core}"
    );
}

/// A repository wide enough to trip the section split (U4) still renders every
/// module exactly once, and the manifest keeps the complete enumeration no
/// matter how the view was cut. This is the R8 contract end to end: the view is
/// a readable projection, the manifest is the source of truth.
#[test]
fn a_split_view_renders_every_module_once_and_leaves_the_manifest_complete() {
    let tmp = tempfile::tempdir().unwrap();
    // Comfortably past the 30-module section threshold, all top-level, so the
    // greedy packer has to open several sections.
    let files: Vec<(String, String)> = (0..40)
        .map(|i| (format!("m{i:02}/f.rs"), "// f".to_string()))
        .collect();
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(p, c)| (p.as_str(), c.as_str()))
        .collect();
    init_repo_with_files(tmp.path(), "Alice", &refs);

    let (graph, leaf) = graph_and_leaf(tmp.path());

    // The view really did split — otherwise the rest of this proves nothing.
    assert!(
        leaf.contains("### Part 1"),
        "the wide view is cut into sections: {leaf}"
    );
    assert!(
        leaf.contains("### Part 2"),
        "into more than one section: {leaf}"
    );

    // Invariant (a) again, this time across section boundaries.
    let lines = instance_lines(&leaf);
    assert_eq!(
        lines.len(),
        module_count_excluding_root(&graph),
        "one line per non-root module even when the view splits"
    );
    let mut keys: Vec<String> = lines.iter().map(|l| parse_module_line(l).0).collect();
    keys.sort();
    let unique = keys.len();
    keys.dedup();
    assert_eq!(
        keys.len(),
        unique,
        "no module rendered twice across sections"
    );

    // R8: the manifest enumeration is untouched by how the view was cut.
    let manifest = build_manifest(&graph, None).unwrap();
    let enumerated: BTreeSet<&str> = manifest
        .nodes
        .iter()
        .map(|n| manifest.string(n.key_id))
        .collect();
    for node in graph.nodes_sorted() {
        if node.kind == NodeKind::Module {
            assert!(
                enumerated.contains(node.key.as_str()),
                "manifest carries module `{}` regardless of the view split",
                node.key.as_str()
            );
        }
    }
}

/// The rendered indentation of every bullet still equals its module key's path
/// depth, even once the view is cut into sections.
///
/// This is the guard the section model alone cannot give. `render_section`
/// rebases indentation against its section's shallowest row, so if a cut ever
/// landed mid-subtree, the orphaned children would be renormalised to column 0
/// and would read as legitimate top-level modules — correct-looking labels at
/// the wrong depth. Asserting depth against the key catches that at the only
/// place it becomes visible: the emitted markdown.
#[test]
fn rendered_indent_matches_key_depth_across_sections() {
    let tmp = tempfile::tempdir().unwrap();
    // Wide enough to split, and nested so a mis-cut would have something to orphan.
    let mut files: Vec<(String, String)> = Vec::new();
    for i in 0..20 {
        files.push((format!("m{i:02}/f.rs"), "// f".to_string()));
        files.push((format!("m{i:02}/inner/g.rs"), "// g".to_string()));
    }
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(p, c)| (p.as_str(), c.as_str()))
        .collect();
    init_repo_with_files(tmp.path(), "Alice", &refs);

    let (_graph, leaf) = graph_and_leaf(tmp.path());
    assert!(leaf.contains("### Part 2"), "the fixture splits: {leaf}");

    for line in instance_lines(&leaf) {
        let (key, indent, _) = parse_module_line(&line);
        assert_eq!(
            indent,
            key.matches('/').count(),
            "`{key}` renders at indent {indent} but its path depth is {}",
            key.matches('/').count()
        );
    }
}

/// Deep + wide pathological fixture (KTD5): builds and renders without panic, and
/// records the observed maximum indent depth. This is a MEASUREMENT for the
/// deferred depth-cap decision, not a scannability gate — no cap is asserted.
#[test]
fn deep_wide_fixture_records_the_max_indent() {
    let tmp = tempfile::tempdir().unwrap();

    // One deep chain (8 directory levels) plus a wide fan-out of shallow roots.
    let mut files: Vec<(String, String)> = Vec::new();
    let deep = "l0/l1/l2/l3/l4/l5/l6/l7/deep.rs"; // 8 dirs → deepest module depth 7
    files.push((deep.to_string(), "// deep".to_string()));
    for w in 0..12 {
        files.push((format!("wide{w:02}/f.rs"), "// wide".to_string()));
    }
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(p, c)| (p.as_str(), c.as_str()))
        .collect();
    init_repo_with_files(tmp.path(), "Alice", &refs);

    let (_graph, leaf) = graph_and_leaf(tmp.path());
    let lines = instance_lines(&leaf);
    let max_indent = lines
        .iter()
        .map(|l| parse_module_line(l).1)
        .max()
        .expect("at least one module line");

    // The deepest module is `l0/.../l7` — 7 path separators → indent level 7.
    // Recorded (not gated): this is the boundary the deferred depth cap would use.
    eprintln!("deep+wide fixture: observed max module-tree indent = {max_indent}");
    assert_eq!(
        max_indent, 7,
        "the deep chain reaches the expected indent depth"
    );
}
