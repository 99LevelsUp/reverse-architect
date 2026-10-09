//! Symlink entries (index mode 0o120000) hold their target-path text as the blob,
//! not source content — so content extractors must skip them (code review). The
//! symlink still becomes a File node with git history, like any non-source tracked
//! file; only symbol/LOC/dep extraction is skipped. Hermetic: a hand-built symlink
//! index entry, no real filesystem symlink (which Windows/CI cannot always create).

use git2::{IndexEntry, IndexTime, Repository};
use reverse_architect::extract::{run_all, Repo};
use reverse_architect::graph::{FactKind, Graph, NodeKey};
use tempfile::TempDir;

mod common;

/// Add a symlink index entry at `path` whose blob is `content` (a real blob in the
/// odb). Deliberately code-shaped content so that, if it were wrongly parsed as
/// source, a symbol would appear — letting the test prove the skip.
fn add_symlink(repo: &Repository, path: &str, content: &str) {
    let oid = repo.blob(content.as_bytes()).unwrap();
    let mut index = repo.index().unwrap();
    let entry = IndexEntry {
        ctime: IndexTime::new(0, 0),
        mtime: IndexTime::new(0, 0),
        dev: 0,
        ino: 0,
        mode: 0o120000, // GIT_FILEMODE_LINK — a symlink.
        uid: 0,
        gid: 0,
        file_size: content.len() as u32,
        id: oid,
        flags: 0,
        flags_extended: 0,
        path: path.as_bytes().to_vec(),
    };
    index.add(&entry).unwrap();
    index.write().unwrap();
}

#[test]
fn symlink_is_a_file_node_but_its_content_is_not_parsed() {
    let tmp = TempDir::new().unwrap();
    let repo =
        common::init_repo_with_files(tmp.path(), "Ann", &[("core/lib.rs", "pub fn f() {}\n")]);
    // Code-shaped blob: if the symlink were parsed as Rust, `through_link` would
    // surface as a symbol. It must not, because symlinks are skipped for content.
    add_symlink(&repo, "link.rs", "pub fn through_link() {}\n");

    let ra_repo = Repo::open(tmp.path()).unwrap();
    let mut graph = Graph::new();
    run_all(&ra_repo, &mut graph).unwrap();

    // The symlink IS a tracked File node...
    let node = graph
        .node(&NodeKey::new("link.rs"))
        .expect("symlink is a File node");
    // ...but its content was not parsed: no symbol, no LOC fact.
    assert!(
        graph
            .node(&NodeKey::new("sym:rust:link#through_link()"))
            .is_none(),
        "symlink target-text must not be parsed as source"
    );
    assert!(
        !node.facts.iter().any(|f| f.kind == FactKind::Loc),
        "no LOC fact for a symlink; facts: {:?}",
        node.facts
    );
    // The real source file is unaffected.
    assert!(graph.node(&NodeKey::new("sym:rust:core/lib#f()")).is_some());
}
