//! The second layer of a run (`--detail-level`, contract 0.3) must leave the
//! first one alone: a run without it writes the same bytes as before the layer
//! existed.
//!
//! The gate is a fingerprint of every file a plain run writes, taken over the
//! six-grammar fixture (`tests/common/six_grammar.rs`): eight languages, C#
//! among them, so the fingerprint covers type cards, details and maps, not
//! only the views. It holds no `.xaml`, so no markup plugin of a later unit
//! can take part in it.
//!
//! The run goes through the binary, as an operator's does, so the fingerprint
//! is of what reaches the disk. It is taken twice, on one thread and on many,
//! from two repositories at two paths: a fingerprint that depended on the
//! scheduler or on where the checkout lies would be no gate at all.

mod common;

use assert_cmd::Command;
use common::output_files;
use sha2::{Digest, Sha256};
use std::path::Path;

/// Every file a plain run over `repo` writes into `out`, on `threads` worker
/// threads. The binary is a process of its own, so `RAYON_NUM_THREADS` sizes
/// its pool for this run alone.
fn plain_run(repo: &Path, out: &Path, threads: usize) -> Vec<(String, Vec<u8>)> {
    Command::cargo_bin("reverse-architect")
        .unwrap()
        .env("RAYON_NUM_THREADS", threads.to_string())
        .arg(repo)
        .arg("--out")
        .arg(out)
        .assert()
        .success();
    output_files(out)
}

/// `<sha256>  <path>` per file, sorted by path — the shape `sha256sum` prints.
fn fingerprint(files: &[(String, Vec<u8>)]) -> String {
    files
        .iter()
        .map(|(path, bytes)| format!("{:x}  {path}\n", Sha256::digest(bytes)))
        .collect()
}

#[test]
fn a_plain_run_writes_the_pinned_bytes_on_one_thread_and_on_many() {
    assert!(
        common::six_grammar::files()
            .iter()
            .all(|(path, _)| !path.ends_with(".xaml")),
        "the layer-1 fixture must not hold markup a later plugin could read"
    );

    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    common::six_grammar::init(&first.path().join("repo"));
    common::six_grammar::init(&second.path().join("checkout"));

    let one = plain_run(&first.path().join("repo"), &first.path().join("corpus"), 1);
    let many = plain_run(
        &second.path().join("checkout"),
        &second.path().join("out"),
        8,
    );

    // Guard the guard: a corpus without type cards would pin nothing the
    // second layer could disturb.
    assert!(
        one.iter().any(|(path, _)| path.starts_with("cards/type/")),
        "the fixture must produce type cards"
    );
    assert!(
        one.iter()
            .any(|(path, _)| path.starts_with("cards/detail/type/")),
        "the fixture must produce details"
    );

    let a = fingerprint(&one);
    let b = fingerprint(&many);
    assert_eq!(a, b, "the corpus depends on the thread count or the paths");

    // The manifest records every plugin the run loaded — all the bundled
    // ones, not only those that read a file.
    let manifest = &one
        .iter()
        .find(|(path, _)| path == "manifest.json")
        .expect("a manifest")
        .1;
    let manifest: serde_json::Value = serde_json::from_slice(manifest).unwrap();
    let plugins: Vec<&str> = manifest["plugins"]
        .as_array()
        .expect("the manifest lists its plugins")
        .iter()
        .map(|p| p["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        plugins,
        [
            "csharp",
            "ecmascript",
            "go",
            "java",
            "python",
            "rust",
            "sql"
        ]
    );

    insta::assert_snapshot!("layer1_corpus_fingerprint", a);
}
