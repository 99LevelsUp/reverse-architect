//! Git submodules (gitlinks, index mode 160000) are pointers to another
//! repository, not blobs. They must never be emitted as `NodeKind::File` (Task 3
//! of the Tier 0 determinism hardening). The fixture hand-adds a gitlink index
//! entry (hermetic — no network, no second repo checkout) and asserts it does not
//! surface as a File, while ordinary files are unaffected.

use git2::{IndexEntry, IndexTime, Repository};
use reverse_architect::extract::{run_all, Repo};
use reverse_architect::graph::{Graph, NodeKey};
use tempfile::TempDir;

mod common;

/// Add a gitlink (submodule) entry to the repo index at `path`, pointing at
/// `target` (any commit oid — a submodule's target need not exist in this odb).
fn add_gitlink(repo: &Repository, path: &str, target: git2::Oid) {
    let mut index = repo.index().unwrap();
    let entry = IndexEntry {
        ctime: IndexTime::new(0, 0),
        mtime: IndexTime::new(0, 0),
        dev: 0,
        ino: 0,
        mode: 0o160000, // GIT_FILEMODE_COMMIT — a gitlink.
        uid: 0,
        gid: 0,
        file_size: 0,
        id: target,
        flags: 0,
        flags_extended: 0,
        path: path.as_bytes().to_vec(),
    };
    index.add(&entry).unwrap();
    index.write().unwrap();
}

#[test]
fn gitlink_is_not_emitted_as_a_file_node() {
    let tmp = TempDir::new().unwrap();
    let repo =
        common::init_repo_with_files(tmp.path(), "Ann", &[("core/lib.rs", "pub fn f() {}\n")]);
    // The commit we just made is a convenient real oid to point the gitlink at.
    let head = repo.head().unwrap().target().unwrap();
    add_gitlink(&repo, "vendor/sub", head);

    let ra_repo = Repo::open(tmp.path()).unwrap();
    let mut graph = Graph::new();
    run_all(&ra_repo, &mut graph).unwrap();

    // The gitlink is not a File...
    assert!(
        graph.node(&NodeKey::new("vendor/sub")).is_none(),
        "a gitlink must not be emitted as a File node"
    );
    // ...and its otherwise-empty parent directory does not become a phantom module.
    assert!(
        graph.node(&NodeKey::new("vendor")).is_none(),
        "a gitlink-only directory must not spawn an empty module"
    );
    // Ordinary tracked files are unaffected.
    assert!(graph.node(&NodeKey::new("core/lib.rs")).is_some());

    // The centralization contract: the gitlink key is absent from BOTH
    // enumerations, so no consumer (filetree, test-pairing) ever sees it.
    let keys = ra_repo.tracked_files().unwrap();
    assert!(
        !keys.iter().any(|k| k == "vendor/sub"),
        "gitlink filtered from enumeration"
    );
    assert!(ra_repo
        .tracked_files_with_paths()
        .unwrap()
        .iter()
        .all(|t| t.key != "vendor/sub"));
}

#[test]
fn committed_gitlink_does_not_break_the_history_walk() {
    // Testing-gap closure (code review): the git extractor walks COMMITS, so a
    // gitlink must be exercised as a committed delta, not only a live index entry.
    let tmp = TempDir::new().unwrap();
    let repo =
        common::init_repo_with_files(tmp.path(), "Ann", &[("core/lib.rs", "pub fn f() {}\n")]);
    let head = repo.head().unwrap().target().unwrap();
    add_gitlink(&repo, "vendor/sub", head);
    // Commit the index (with the gitlink) so the revwalk/diff path sees a gitlink delta.
    {
        let mut index = repo.index().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let sig = common::fixed_signature("Ann");
        let parent = repo.find_commit(head).unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "add submodule", &tree, &[&parent])
            .unwrap();
    }

    let ra_repo = Repo::open(tmp.path()).unwrap();
    let mut graph = Graph::new();
    // Must not panic or error on the committed gitlink delta (E4).
    run_all(&ra_repo, &mut graph).unwrap();
    assert!(
        graph.node(&NodeKey::new("vendor/sub")).is_none(),
        "committed gitlink is still not a File"
    );
    assert!(graph.node(&NodeKey::new("core/lib.rs")).is_some());
}
