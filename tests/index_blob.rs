//! Content extractors read the git index blob, not the working tree (Task 2 of
//! the Tier 0 determinism hardening). On a clean checkout `worktree == blob`, so
//! this is the ONLY place the difference is observable: a fixture whose on-disk
//! bytes were changed after the commit must yield facts describing the committed
//! blob, never the dirty working tree. This protects byte-determinism of the
//! manifest across dirty trees and autocrlf-differing checkouts.

use reverse_architect::extract::{run_all, Repo};
use reverse_architect::graph::{FactKind, FactValue, Graph, NodeKey, NodeKind};
use tempfile::TempDir;

mod common;

/// Commit content A, then overwrite the working tree with content B WITHOUT
/// staging — the classic dirty tree. Facts must follow A (the blob).
fn graph_over_dirty_tree() -> (TempDir, Graph) {
    let tmp = TempDir::new().unwrap();
    // Blob A: one function, one line; a Cargo manifest with a single dependency.
    common::init_repo_with_files(
        tmp.path(),
        "Ann",
        &[
            ("core/lib.rs", "pub fn f() {}\n"),
            (
                "Cargo.toml",
                "[package]\nname = \"x\"\n\n[dependencies]\nserde = \"1\"\n",
            ),
        ],
    );

    // Dirty B on disk only (never staged): three functions, an extra dependency.
    std::fs::write(
        tmp.path().join("core/lib.rs"),
        "pub fn f() {}\npub fn g() {}\npub fn h() {}\n",
    )
    .unwrap();
    std::fs::write(
        tmp.path().join("Cargo.toml"),
        "[package]\nname = \"x\"\n\n[dependencies]\nserde = \"1\"\ntokio = \"1\"\n",
    )
    .unwrap();

    let repo = Repo::open(tmp.path()).unwrap();
    let mut graph = Graph::new();
    run_all(&repo, &mut graph).unwrap();
    (tmp, graph)
}

#[test]
fn treesitter_symbols_follow_the_blob_not_the_dirty_tree() {
    let (_tmp, g) = graph_over_dirty_tree();
    // Blob A declares only `f`.
    assert!(
        g.node(&NodeKey::new("sym:rust:core/lib#f()")).is_some(),
        "symbol from the committed blob is present"
    );
    // `g`/`h` exist only in the dirty working tree — they must NOT appear.
    assert!(
        g.node(&NodeKey::new("sym:rust:core/lib#g()")).is_none(),
        "dirty-tree-only symbol must not be extracted"
    );
    assert!(g.node(&NodeKey::new("sym:rust:core/lib#h()")).is_none());
}

#[test]
fn loc_follows_the_blob_not_the_dirty_tree() {
    let (_tmp, g) = graph_over_dirty_tree();
    let node = g.node(&NodeKey::new("core/lib.rs")).unwrap();
    assert_eq!(node.kind, NodeKind::File);
    // Blob A is a single line; the dirty tree has three.
    assert!(
        node.facts
            .iter()
            .any(|f| f.kind == FactKind::Loc && f.value == FactValue::Int(1)),
        "LOC describes the 1-line blob, not the 3-line dirty tree; facts: {:?}",
        node.facts
    );
}

#[test]
fn loc_excludes_embedded_language_blocks() {
    // Regression (code review): metrics must count a file's PARENT language lines
    // only, EXCLUDING embedded-language child blobs — matching the pre-branch
    // per-report sum. A Markdown file with a fenced `rust` block has 5 physical
    // lines, but the `fn main() {}` line is attributed to the embedded Rust blob,
    // so the Markdown Loc fact is 4. `.summarise()` would (wrongly) report 5 and
    // break byte-identity with pre-branch output.
    let tmp = TempDir::new().unwrap();
    common::init_repo_with_files(
        tmp.path(),
        "Ann",
        &[("doc.md", "# title\n\n```rust\nfn main() {}\n```\n")],
    );
    let repo = Repo::open(tmp.path()).unwrap();
    let mut g = Graph::new();
    run_all(&repo, &mut g).unwrap();

    let node = g.node(&NodeKey::new("doc.md")).unwrap();
    let loc = node
        .facts
        .iter()
        .find(|f| f.kind == FactKind::Loc)
        .and_then(|f| {
            if let FactValue::Int(i) = f.value {
                Some(i)
            } else {
                None
            }
        })
        .expect("doc.md carries a Loc fact");
    assert_eq!(
        loc, 4,
        "embedded rust block excluded from Markdown LOC; facts: {:?}",
        node.facts
    );
}

#[test]
fn extensionless_file_gets_no_loc_instead_of_a_disk_read() {
    // Regression (code review, Codex repro): metrics must classify by extension
    // only. tokei's from_path falls back to from_shebang for extensionless files,
    // which File::open()s a cwd-relative working-tree path — non-deterministic and
    // an index-blob violation. An extensionless tracked file must therefore get a
    // File node + git facts but NO LOC (deterministic skip), never a disk read.
    let tmp = TempDir::new().unwrap();
    common::init_repo_with_files(
        tmp.path(),
        "Ann",
        &[
            ("run", "#!/usr/bin/env python\nprint('hi')\n"),
            ("core/lib.rs", "pub fn f() {}\n"),
        ],
    );
    let repo = Repo::open(tmp.path()).unwrap();
    let mut g = Graph::new();
    run_all(&repo, &mut g).unwrap();

    let run = g
        .node(&NodeKey::new("run"))
        .expect("extensionless file is a File node");
    assert!(
        !run.facts.iter().any(|f| f.kind == FactKind::Loc),
        "extensionless file gets no LOC (no cwd disk read); facts: {:?}",
        run.facts
    );
    // A normal extension file is still counted.
    let lib = g.node(&NodeKey::new("core/lib.rs")).unwrap();
    assert!(
        lib.facts.iter().any(|f| f.kind == FactKind::Loc),
        "extension file keeps LOC"
    );
}

#[test]
fn manifest_deps_follow_the_blob_not_the_dirty_tree() {
    let (_tmp, g) = graph_over_dirty_tree();
    let nodes = g.nodes_sorted();
    // The committed manifest declares `serde` only.
    assert!(
        nodes.iter().any(|n| n.key.as_str().contains("serde")),
        "dependency from the committed blob is present"
    );
    // `tokio` was added only in the dirty tree — it must NOT be a node.
    assert!(
        nodes.iter().all(|n| !n.key.as_str().contains("tokio")),
        "dirty-tree-only dependency must not be extracted"
    );
}
