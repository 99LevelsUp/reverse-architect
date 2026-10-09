//! TrackedFile carries the git index entry's mode + blob oid (Task 1 foundation
//! of the Tier 0 determinism hardening). Content extractors read the blob (not
//! the working tree) from `blob_id`; symlinks are distinguishable via `is_symlink`
//! so their target-path-text blob is not parsed as source.

use reverse_architect::extract::Repo;
use tempfile::TempDir;

mod common;

#[test]
fn tracked_file_carries_blob_oid_and_is_not_a_symlink() {
    let tmp = TempDir::new().unwrap();
    common::init_repo_with_files(tmp.path(), "Ann", &[("core/lib.rs", "pub fn f() {}\n")]);
    let repo = Repo::open(tmp.path()).unwrap();

    let tracked = repo.tracked_files_with_paths().unwrap();
    let tf = tracked
        .iter()
        .find(|t| t.key == "core/lib.rs")
        .expect("core/lib.rs is tracked");

    assert!(
        !tf.blob_id.is_zero(),
        "a committed file has a real blob oid"
    );
    assert!(!tf.is_symlink, "a normal file is not a symlink");
    assert_eq!(tf.mode, 0o100644, "a normal file has the regular blob mode");
}
