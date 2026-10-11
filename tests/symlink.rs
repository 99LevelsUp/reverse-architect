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

/// A view, as the blob of a symlink it is never read: the plugin would find
/// a declaration in it.
const VIEW: &str = "<UserControl x:Class=\"Acme.Views.ViewA\"\n\
    xmlns=\"http://schemas.microsoft.com/winfx/2006/xaml/presentation\"\n\
    xmlns:x=\"http://schemas.microsoft.com/winfx/2006/xaml\"/>\n";

/// A run with the second layer over `files`, a symlink `link` beside them
/// when given.
fn generate_with_link(
    files: &[(&str, &str)],
    link: Option<&str>,
) -> (TempDir, reverse_architect::Generated) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("repo");
    let repo = common::init_repo_with_files(&root, "Ann", files);
    if let Some(path) = link {
        add_symlink(&repo, path, VIEW);
    }
    let config = reverse_architect::config::Config {
        detail_level: Some(2),
        ..common::config_for(&root, &tmp.path().join("out"))
    };
    let generated = reverse_architect::generate(&config).unwrap();
    (tmp, generated)
}

/// Review of 2c: a tracked `.xaml` symlink is not a XAML file the run read.
/// It does not activate the bundled XAML plugin — no vocabulary, no XAML
/// coverage that says "read in full" — and beside a real XAML file it is not
/// counted as one.
#[test]
fn a_xaml_symlink_neither_activates_the_plugin_nor_counts_as_a_xaml_file() {
    let code = [("Acme/A.cs", "namespace Acme { public class A {} }\n")];
    let (_a, without) = generate_with_link(&code, None);
    let (_b, with) = generate_with_link(&code, Some("Acme/Link.xaml"));
    assert!(with.graph.node(&NodeKey::new("Acme/Link.xaml")).is_some());
    assert_eq!(with.graph.vocabulary(), without.graph.vocabulary());
    assert_eq!(with.coverage.markup, without.coverage.markup);
    let markup = with
        .coverage
        .markup
        .as_ref()
        .expect("a run with the second layer");
    assert!(markup.plugins.is_empty(), "{markup:?}");
    assert_eq!(markup.files, 0);

    let real = [code[0], ("Acme/Real.xaml", VIEW)];
    let (_c, mixed) = generate_with_link(&real, Some("Acme/Link.xaml"));
    let markup = mixed.coverage.markup.as_ref().expect("a markup section");
    assert_eq!(markup.plugins, ["xaml"]);
    assert_eq!(markup.files, 1, "{markup:?}");
}
