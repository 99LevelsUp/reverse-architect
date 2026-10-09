//! End-to-end CLI behavior: the binary parses, resolves config, generates a
//! corpus over a git repo, and fails with a readable error otherwise — never a
//! panic.

mod common;

use assert_cmd::Command;
use common::{init_repo_with_files, output_files};
use predicates::prelude::*;
use reverse_architect::manifest::{
    DOCUMENTS_FILENAME, EDGES_FILENAME, NODES_FILENAME, STRINGS_FILENAME,
};

#[test]
fn generates_a_corpus_over_a_git_repo() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(tmp.path(), "Alice", &[("src/lib.rs", "// lib")]);
    let out = tmp.path().join("corpus");

    Command::cargo_bin("reverse-architect")
        .unwrap()
        .arg(tmp.path())
        .arg("--out")
        .arg(&out)
        .assert()
        .success()
        // Writing is a phase of its own in the run's summary (plan B, U11).
        .stderr(predicate::str::contains("write corpus"));

    assert!(out.join("index.md").exists());
    assert!(out.join("overview.md").exists());
}

/// Plan B, U12 (KTD13): the run's summary ends with the sizes per level, so
/// the measurement on a large repository needs no tool of its own — the
/// numbers coverage.json carries, in a line.
#[test]
fn the_summary_states_the_sizes_per_level() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[(
            "App/Loader.cs",
            "namespace App\n{\n    public class Loader\n    {\n        public void Run() { }\n    }\n}\n",
        )],
    );
    let out = tmp.path().join("corpus");
    let assert = Command::cargo_bin("reverse-architect")
        .unwrap()
        .arg(tmp.path())
        .arg("--out")
        .arg(&out)
        .assert()
        .success();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).to_string();
    let coverage: serde_json::Value =
        serde_json::from_slice(&std::fs::read(out.join("coverage.json")).unwrap()).unwrap();
    let card = &coverage["cards"]["sizes"]["type_card"];
    assert_eq!(card["count"], 1, "{coverage}");
    let line = stderr
        .lines()
        .find(|l| l.contains("sizes in bytes"))
        .unwrap_or_else(|| panic!("no size line: {stderr}"));
    assert!(
        line.contains(&format!(
            "type card {} / {} (1 card)",
            card["median"], card["p90"]
        )),
        "{line}"
    );
}

#[test]
fn nonexistent_repo_fails_with_readable_error() {
    Command::cargo_bin("reverse-architect")
        .unwrap()
        .arg("no/such/repo/here")
        .assert()
        .failure()
        .stderr(predicate::str::contains("does not exist"));
}

/// Since U15 a directory with no repository is a **source**, not an error.
///
/// This test asserted the opposite until U15 — that the run failed — and that
/// failure was the friction the unit exists to remove: `ev3-resources`'
/// decompiled projects could only be indexed after a `git init` nobody wanted to
/// perform on the thing they were trying to describe. The inversion is the
/// change, so the test states it rather than being quietly deleted.
#[test]
fn a_directory_with_no_repository_is_indexed_rather_than_refused() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    std::fs::write(tmp.path().join("src/lib.rs"), "pub fn f() {}\n").unwrap();
    let out = tmp.path().join("corpus");

    Command::cargo_bin("reverse-architect")
        .unwrap()
        .arg(tmp.path())
        .arg("--out")
        .arg(&out)
        .assert()
        .success();

    assert!(out.join("index.md").exists());
}

/// A path that exists but is not a tree still fails, and says which path.
/// "Not a git repository" stopped being a reason to refuse; "not a directory"
/// is the reason that is left.
///
/// The message and the path are both asserted. Checking only for `error:` would
/// pass for any refusal whatever — including the pre-U15 "not a git repository"
/// this replaced, which is the one thing the test exists to rule out.
///
/// It also pins WHICH layer refuses. `Config::resolve` validates the path before
/// `Repo::open` is ever called, so `ExtractError::NotASource` is unreachable
/// through the binary; asserting its wording here would have been asserting a
/// message no operator can see.
#[test]
fn a_file_given_where_a_tree_was_expected_fails_with_a_readable_error() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("not-a-tree.txt");
    std::fs::write(&file, "hello").unwrap();

    Command::cargo_bin("reverse-architect")
        .unwrap()
        .arg(&file)
        .assert()
        .failure()
        .stderr(predicate::str::contains("source path is not a directory"))
        .stderr(predicate::str::contains("not-a-tree.txt"));
}

#[test]
fn help_prints_usage() {
    Command::cargo_bin("reverse-architect")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Usage"));
}

/// The conventional in-project mirror path (R14) — kept in sync with
/// `config::MIRROR_DIR_NAME`.
const MIRROR_DIR: &str = ".reverse-architect";

#[test]
fn mirror_writes_the_corpus_to_both_the_sidecar_and_the_in_project_path() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    init_repo_with_files(&repo, "Alice", &[("src/lib.rs", "// lib")]);
    let out = tmp.path().join("corpus");

    let assert = Command::cargo_bin("reverse-architect")
        .unwrap()
        .arg(&repo)
        .arg("--out")
        .arg(&out)
        .arg("--mirror")
        .assert()
        .success();

    // The primary sidecar (explicit --out) has the corpus.
    assert!(out.join("index.md").exists());
    assert!(out.join("manifest.json").exists());

    // The in-project mirror has a byte-identical copy.
    let mirror = repo.join(MIRROR_DIR);
    assert!(mirror.join("index.md").exists());
    assert!(mirror.join("manifest.json").exists());
    assert_eq!(
        std::fs::read(out.join("index.md")).unwrap(),
        std::fs::read(mirror.join("index.md")).unwrap(),
        "mirror is a byte-identical copy of the sidecar corpus"
    );
    assert_eq!(
        std::fs::read(out.join("manifest.json")).unwrap(),
        std::fs::read(mirror.join("manifest.json")).unwrap(),
        "mirror manifest is byte-identical to the sidecar manifest"
    );

    // The gitignore recommendation names exactly the path that was written.
    assert
        .stderr(predicate::str::contains(format!("{MIRROR_DIR}/")))
        .stderr(predicate::str::contains(".gitignore"));
}

#[test]
fn without_mirror_only_the_sidecar_is_written() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    init_repo_with_files(&repo, "Alice", &[("src/lib.rs", "// lib")]);
    let out = tmp.path().join("corpus");

    Command::cargo_bin("reverse-architect")
        .unwrap()
        .arg(&repo)
        .arg("--out")
        .arg(&out)
        .assert()
        .success();

    assert!(out.join("index.md").exists());
    // No in-project mirror by default.
    assert!(
        !repo.join(MIRROR_DIR).exists(),
        "default run leaves no in-project mirror"
    );
}

#[test]
fn profile_minimal_writes_only_the_profile_views() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(tmp.path(), "Alice", &[("src/lib.rs", "// lib")]);
    let out = tmp.path().join("corpus");

    Command::cargo_bin("reverse-architect")
        .unwrap()
        .arg(tmp.path())
        .arg("--out")
        .arg(&out)
        .arg("--profile")
        .arg("minimal")
        .assert()
        .success();

    // The two profile views are present…
    assert!(out.join("overview.md").exists());
    assert!(out.join("module-structure.md").exists());
    // …and the rest of the automatic backbone is not (the override narrows it).
    assert!(!out.join("dependencies.md").exists());
    assert!(!out.join("entry-points.md").exists());
    assert!(!out.join("test-coverage.md").exists());
}

#[test]
fn rerun_with_a_narrower_profile_removes_stale_leaves() {
    // A full run then a narrower --profile run into the SAME output dir must not
    // leave orphaned leaves the new index.md/manifest.json no longer reference —
    // a directory-scanning RAG consumer would otherwise ingest stale corpus text.
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(tmp.path(), "Alice", &[("src/lib.rs", "// lib")]);
    let out = tmp.path().join("corpus");
    let run = |extra: &[&str]| {
        let mut cmd = Command::cargo_bin("reverse-architect").unwrap();
        cmd.arg(tmp.path()).arg("--out").arg(&out);
        for a in extra {
            cmd.arg(a);
        }
        cmd.assert().success();
    };

    run(&[]); // full automatic taxonomy → all leaves present
    assert!(out.join("dependencies.md").exists());

    run(&["--profile", "minimal"]); // narrower → the extra leaves must be gone
    assert!(out.join("overview.md").exists());
    assert!(
        !out.join("dependencies.md").exists(),
        "stale leaf from the prior full run must be removed"
    );
    assert!(!out.join("entry-points.md").exists());
    assert!(!out.join("test-coverage.md").exists());
}

#[test]
fn a_below_threshold_rerun_removes_stale_line_delimited_manifest_files() {
    // A directory can hold a prior above-threshold run's line-delimited
    // manifest (U10/KTD5) even though the current, smaller fixture stays
    // below the threshold and writes manifest.json instead. That shrinking
    // direction is reachable in this small CLI fixture (unlike growing past
    // the threshold, which needs tens of thousands of nodes — see the U10
    // plan's Scope Boundaries) by seeding the four filenames directly rather
    // than by an actual prior above-threshold run.
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(tmp.path(), "Alice", &[("src/lib.rs", "// lib")]);
    let out = tmp.path().join("corpus");
    std::fs::create_dir_all(&out).unwrap();
    for name in [
        STRINGS_FILENAME,
        NODES_FILENAME,
        EDGES_FILENAME,
        DOCUMENTS_FILENAME,
    ] {
        std::fs::write(out.join(name), b"stale").unwrap();
    }

    Command::cargo_bin("reverse-architect")
        .unwrap()
        .arg(tmp.path())
        .arg("--out")
        .arg(&out)
        .assert()
        .success();

    assert!(out.join("manifest.json").exists());
    for name in [
        STRINGS_FILENAME,
        NODES_FILENAME,
        EDGES_FILENAME,
        DOCUMENTS_FILENAME,
    ] {
        assert!(
            !out.join(name).exists(),
            "stale line-delimited file {name} must be removed by a monolith-mode rerun"
        );
    }
}

#[test]
fn user_authored_markdown_in_the_output_dir_survives() {
    // Cleanup must delete only tool-owned filenames, never user Markdown that
    // shares the --out directory. A README (not a catalog view id) must survive.
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(tmp.path(), "Alice", &[("src/lib.rs", "// lib")]);
    let out = tmp.path().join("shared");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("README.md"), b"hand-written, must survive").unwrap();
    std::fs::write(out.join("notes.md"), b"also mine").unwrap();

    Command::cargo_bin("reverse-architect")
        .unwrap()
        .arg(tmp.path())
        .arg("--out")
        .arg(&out)
        .assert()
        .success();

    // User files untouched…
    assert_eq!(
        std::fs::read(out.join("README.md")).unwrap(),
        b"hand-written, must survive"
    );
    assert!(out.join("notes.md").exists());
    // …and the corpus was still written alongside them.
    assert!(out.join("index.md").exists());
    assert!(out.join("overview.md").exists());
}

#[test]
fn manifest_and_coverage_survive_a_rerun() {
    // Regression for a plan-drafting bug caught in doc review: manifest.json
    // is itself a member of taxonomy::generated_filenames()'s namespace, so an
    // orphan-cleanup pass that isn't careful deletes it and never rewrites it.
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(tmp.path(), "Alice", &[("src/lib.rs", "// lib")]);
    let out = tmp.path().join("corpus");
    let run = || {
        Command::cargo_bin("reverse-architect")
            .unwrap()
            .arg(tmp.path())
            .arg("--out")
            .arg(&out)
            .assert()
            .success();
    };

    run();
    assert!(out.join("manifest.json").exists());
    assert!(out.join("coverage.json").exists());

    run(); // second run into the same dir — both must still be there
    assert!(out.join("manifest.json").exists());
    assert!(out.join("coverage.json").exists());
}

/// `--sites` (plan A, KTD7) adds `sites.jsonl` beside the manifest and
/// changes no other byte of the corpus; a rerun without it removes the file,
/// so a directory never pairs a corpus with site records of another run.
#[test]
fn sites_adds_one_file_and_changes_no_other_byte() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            (
                "src/A.cs",
                "namespace N;\npublic class Caller {\n  public void Go(Foo f) {\n    f.Run();\n    Run();\n  }\n}\n",
            ),
            (
                "src/B.cs",
                "namespace N;\npublic class Foo { public void Run() {} }\npublic class Bar { public void Run() {} }\n",
            ),
        ],
    );
    let plain = tmp.path().join("plain");
    let with_sites = tmp.path().join("with-sites");
    let run = |out: &std::path::Path, sites: bool| {
        let mut cmd = Command::cargo_bin("reverse-architect").unwrap();
        cmd.arg(tmp.path()).arg("--out").arg(out);
        if sites {
            cmd.arg("--sites");
        }
        cmd.assert().success();
    };
    run(&plain, false);
    run(&with_sites, true);

    assert!(!plain.join("sites.jsonl").exists());
    let records = std::fs::read_to_string(with_sites.join("sites.jsonl")).unwrap();
    assert!(
        records.lines().count() >= 2 && records.contains("\"declared-receiver\""),
        "one line per site, with the rules' verdicts: {records}"
    );
    let mut sited = output_files(&with_sites);
    sited.retain(|(path, _)| path != "sites.jsonl");
    assert_eq!(output_files(&plain), sited);

    run(&with_sites, false);
    assert!(!with_sites.join("sites.jsonl").exists());
}

#[test]
fn rerun_removes_a_card_whose_entity_is_gone() {
    // U12/R1: a card whose eligibility flips between two runs must not leave
    // the old card file behind. Schema v4 gives every file a card, so the old
    // flip (a file card vanishing once its function became a type's claimed
    // member) no longer happens; the flip here is the type itself going away
    // — `Widget` is rewritten into a free function — while the file card,
    // which every file now owns, stays.
    let tmp = tempfile::tempdir().unwrap();
    let repo_dir = tmp.path().join("repo");
    std::fs::create_dir(&repo_dir).unwrap();
    let repo = init_repo_with_files(&repo_dir, "Alice", &[("src/helper.rs", WIDGET_RS)]);
    let out = tmp.path().join("corpus");

    Command::cargo_bin("reverse-architect")
        .unwrap()
        .arg(&repo_dir)
        .arg("--out")
        .arg(&out)
        .assert()
        .success();
    let type_card = out.join("cards/type/rust/crate/helper/Widget.md");
    let file_card = out.join("cards/file/src/helper.rs.md");
    assert!(type_card.exists(), "Widget gets a type card");
    assert!(file_card.exists(), "every file gets a file card (v4)");

    common::write_and_commit(
        &repo,
        &repo_dir,
        "Alice",
        "c2",
        &[("src/helper.rs", FREE_FN_RS)],
    );
    Command::cargo_bin("reverse-architect")
        .unwrap()
        .arg(&repo_dir)
        .arg("--out")
        .arg(&out)
        .assert()
        .success();
    assert!(
        !type_card.exists(),
        "the type card must not survive once its type is gone from the source"
    );
    assert!(file_card.exists(), "the file's own card stays");
}

/// The two shapes of `src/helper.rs` the orphan-cleanup tests flip between.
const WIDGET_RS: &str =
    "pub struct Widget;\n\nimpl Widget {\n    pub fn helper() -> i32 {\n        1\n    }\n}\n";
const FREE_FN_RS: &str = "pub fn helper() -> i32 {\n    1\n}\n";

/// `WIDGET_RS` renamed: the same one-method type under another name.
const GADGET_RS: &str =
    "pub struct Gadget;\n\nimpl Gadget {\n    pub fn helper() -> i32 {\n        1\n    }\n}\n";

/// Plan B, U11 (KTD9): the maps and the details live under `cards/` beside
/// the cards, so the orphan cleanup must count them as the run's own — or the
/// next run deletes every one of them before writing it again. An unchanged
/// rerun keeps both untouched, and a renamed type takes its old detail with
/// it while its module's map stays.
#[test]
fn a_rerun_keeps_maps_and_details_and_removes_the_stale_ones() {
    let tmp = tempfile::tempdir().unwrap();
    let repo_dir = tmp.path().join("repo");
    std::fs::create_dir(&repo_dir).unwrap();
    let repo = init_repo_with_files(&repo_dir, "Alice", &[("src/helper.rs", WIDGET_RS)]);
    let out = tmp.path().join("corpus");
    let run = || {
        Command::cargo_bin("reverse-architect")
            .unwrap()
            .arg(&repo_dir)
            .arg("--out")
            .arg(&out)
            .assert()
            .success();
    };

    run();
    let detail = out.join("cards/detail/type/rust/crate/helper/Widget.md");
    let map = out.join("cards/map/src.md");
    assert!(detail.exists(), "Widget's offered method earns a detail");
    assert!(map.exists(), "the module that holds Widget has a map");
    let mtimes = |paths: &[&std::path::Path]| -> Vec<std::time::SystemTime> {
        paths
            .iter()
            .map(|p| std::fs::metadata(p).unwrap().modified().unwrap())
            .collect()
    };
    let before = mtimes(&[&detail, &map]);

    std::thread::sleep(std::time::Duration::from_millis(1100));
    run();
    assert_eq!(
        mtimes(&[&detail, &map]),
        before,
        "an unchanged rerun neither deletes nor rewrites a map or a detail"
    );

    common::write_and_commit(
        &repo,
        &repo_dir,
        "Alice",
        "c2",
        &[("src/helper.rs", GADGET_RS)],
    );
    run();
    assert!(
        !detail.exists(),
        "the detail of a type that is gone must not survive the rerun"
    );
    assert!(out
        .join("cards/detail/type/rust/crate/helper/Gadget.md")
        .exists());
    assert!(
        map.exists(),
        "the module still holds a type, so its map stays"
    );

    // And a module left without a type loses its map.
    common::write_and_commit(
        &repo,
        &repo_dir,
        "Alice",
        "c3",
        &[("src/helper.rs", FREE_FN_RS)],
    );
    run();
    assert!(!map.exists(), "a module with no type card has no map");
}

#[test]
fn rerun_over_an_unchanged_commit_never_rewrites_a_file_and_a_content_change_only_touches_the_affected_files(
) {
    let tmp = tempfile::tempdir().unwrap();
    let repo_dir = tmp.path().join("repo");
    std::fs::create_dir(&repo_dir).unwrap();
    let repo = init_repo_with_files(
        &repo_dir,
        "Alice",
        &[
            ("src/lib.rs", "// lib"),
            ("src/helper.rs", "pub fn helper() -> i32 {\n    1\n}\n"),
        ],
    );
    let out = tmp.path().join("corpus");
    let run = || {
        Command::cargo_bin("reverse-architect")
            .unwrap()
            .arg(&repo_dir)
            .arg("--out")
            .arg(&out)
            .assert()
            .success();
    };
    run();

    let watched = [
        out.join("index.md"),
        out.join("manifest.json"),
        out.join("coverage.json"),
        out.join("cards/file/src/helper.rs.md"),
    ];
    let mtimes_before: Vec<_> = watched
        .iter()
        .map(|p| std::fs::metadata(p).unwrap().modified().unwrap())
        .collect();

    std::thread::sleep(std::time::Duration::from_millis(1100));

    // Unchanged rerun: nothing gets rewritten.
    run();
    for (path, before) in watched.iter().zip(&mtimes_before) {
        let after = std::fs::metadata(path).unwrap().modified().unwrap();
        assert_eq!(
            &after,
            before,
            "{} must not be rewritten on an unchanged rerun",
            path.display()
        );
    }

    // A content change: only the file whose content actually changed (and
    // manifest.json/coverage.json, which always re-derive) may get a new mtime;
    // an untouched view leaf must not.
    common::write_and_commit(
        &repo,
        &repo_dir,
        "Alice",
        "c2",
        &[("src/helper.rs", "pub fn helper() -> i32 {\n    2\n}\n")],
    );
    let index_mtime_before = std::fs::metadata(out.join("index.md"))
        .unwrap()
        .modified()
        .unwrap();
    run();
    let index_mtime_after = std::fs::metadata(out.join("index.md"))
        .unwrap()
        .modified()
        .unwrap();
    assert_eq!(
        index_mtime_after, index_mtime_before,
        "index.md's own content did not change, so it must not be rewritten"
    );
}

#[test]
fn mirror_target_gets_the_same_orphan_cleanup_and_skip_unchanged_behavior() {
    let tmp = tempfile::tempdir().unwrap();
    let repo_dir = tmp.path().join("repo");
    std::fs::create_dir(&repo_dir).unwrap();
    // The same flip as `rerun_removes_a_card_whose_entity_is_gone` (v4: a
    // type card goes away, since a file card no longer can).
    let repo = init_repo_with_files(&repo_dir, "Alice", &[("src/helper.rs", WIDGET_RS)]);
    let out = tmp.path().join("corpus");
    let run = || {
        Command::cargo_bin("reverse-architect")
            .unwrap()
            .arg(&repo_dir)
            .arg("--out")
            .arg(&out)
            .arg("--mirror")
            .assert()
            .success();
    };
    run();
    let mirror_type_card = repo_dir
        .join(MIRROR_DIR)
        .join("cards/type/rust/crate/helper/Widget.md");
    assert!(mirror_type_card.exists());
    let mirror_detail = repo_dir
        .join(MIRROR_DIR)
        .join("cards/detail/type/rust/crate/helper/Widget.md");
    assert!(mirror_detail.exists(), "the mirror gets the details too");
    assert!(repo_dir.join(MIRROR_DIR).join("cards/map/src.md").exists());

    common::write_and_commit(
        &repo,
        &repo_dir,
        "Alice",
        "c2",
        &[("src/helper.rs", FREE_FN_RS)],
    );
    run();
    assert!(
        !mirror_type_card.exists(),
        "the mirror target gets the same orphan cleanup as the primary --out"
    );
    assert!(
        !mirror_detail.exists(),
        "and the same cleanup of a stale detail"
    );
    assert!(repo_dir
        .join(MIRROR_DIR)
        .join("cards/file/src/helper.rs.md")
        .exists());
}

#[test]
fn default_run_writes_the_full_automatic_taxonomy() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(tmp.path(), "Alice", &[("src/lib.rs", "// lib")]);
    let out = tmp.path().join("corpus");

    Command::cargo_bin("reverse-architect")
        .unwrap()
        .arg(tmp.path())
        .arg("--out")
        .arg(&out)
        .assert()
        .success();

    // No profile → the full backbone renders (differs from `--profile minimal`).
    for leaf in [
        "overview.md",
        "module-structure.md",
        "dependencies.md",
        "entry-points.md",
        "test-coverage.md",
    ] {
        assert!(out.join(leaf).exists(), "missing leaf {leaf}");
    }
}

#[test]
fn unknown_profile_fails_with_a_readable_error() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(tmp.path(), "Alice", &[("src/lib.rs", "// lib")]);
    let out = tmp.path().join("corpus");

    Command::cargo_bin("reverse-architect")
        .unwrap()
        .arg(tmp.path())
        .arg("--out")
        .arg(&out)
        .arg("--profile")
        .arg("nonesuch")
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown profile: nonesuch"))
        .stderr(predicate::str::contains("minimal"));

    // Fails early — no corpus written on an unknown profile.
    assert!(!out.join("index.md").exists());
}

#[test]
fn links_obsidian_renders_wikilinks_default_renders_portable() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(tmp.path(), "Alice", &[("src/lib.rs", "// lib")]);

    // Obsidian → wikilink bodies.
    let obsidian = tmp.path().join("obsidian");
    Command::cargo_bin("reverse-architect")
        .unwrap()
        .arg(tmp.path())
        .arg("--out")
        .arg(&obsidian)
        .arg("--links")
        .arg("obsidian")
        .assert()
        .success();
    let index = std::fs::read_to_string(obsidian.join("index.md")).unwrap();
    assert!(
        index.contains("[[overview|Overview]]"),
        "obsidian link style renders wikilinks; got:\n{index}"
    );

    // Default → portable relative Markdown links (Phase A regression).
    let portable = tmp.path().join("portable");
    Command::cargo_bin("reverse-architect")
        .unwrap()
        .arg(tmp.path())
        .arg("--out")
        .arg(&portable)
        .assert()
        .success();
    let index = std::fs::read_to_string(portable.join("index.md")).unwrap();
    assert!(
        index.contains("[Overview](overview.md)"),
        "default link style is portable relative Markdown; got:\n{index}"
    );
    assert!(!index.contains("[["), "portable index has no wikilinks");
}

#[cfg(unix)]
#[test]
fn write_if_changed_degrades_gracefully_when_an_existing_file_cant_be_read() {
    // R5: a failure reading an existing file for content comparison must
    // never crash the run -- it degrades to an unconditional write. Strip
    // read permission (owner write-only) on an existing corpus file, rerun,
    // and confirm the run still succeeds and the file still ends up correct.
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(tmp.path(), "Alice", &[("src/lib.rs", "// lib")]);
    let out = tmp.path().join("corpus");

    Command::cargo_bin("reverse-architect")
        .unwrap()
        .arg(tmp.path())
        .arg("--out")
        .arg(&out)
        .assert()
        .success();

    let index = out.join("index.md");
    let mut perms = std::fs::metadata(&index).unwrap().permissions();
    perms.set_mode(0o200);
    std::fs::set_permissions(&index, perms).unwrap();

    let assert = Command::cargo_bin("reverse-architect")
        .unwrap()
        .arg(tmp.path())
        .arg("--out")
        .arg(&out)
        .assert();

    // Restore permissions before any content assertion, regardless of outcome.
    let mut perms = std::fs::metadata(&index).unwrap().permissions();
    perms.set_mode(0o644);
    std::fs::set_permissions(&index, perms).unwrap();

    assert.success();
    let contents = std::fs::read_to_string(&index).unwrap();
    assert!(
        contents.contains("Overview"),
        "the file must still be correctly written despite the unreadable pre-existing copy"
    );
}
