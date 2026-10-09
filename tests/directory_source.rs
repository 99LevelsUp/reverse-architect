//! The non-git directory source (U15, R15/R16).
//!
//! Every test here answers one question: does a tree with no commits get the
//! same treatment a git checkout gets? The enumeration must not vary with the
//! machine, must not reach outside the root, and must land on the same graph keys
//! and the same content ids git would have produced — because if it does not, the
//! two sources produce two different corpora for one tree and the artifact stops
//! meaning one thing.

use assert_cmd::Command;
use git2::Repository;
use reverse_architect::extract::Repo;
use std::path::Path;
use tempfile::TempDir;

mod common;

/// A tree with **no** repository: files written, nothing initialized.
fn plain_tree(files: &[(&str, &str)]) -> TempDir {
    let tmp = TempDir::new().unwrap();
    write_files(tmp.path(), files);
    tmp
}

fn write_files(root: &Path, files: &[(&str, &str)]) {
    for (rel, contents) in files {
        let full = root.join(rel);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(&full, contents).unwrap();
    }
}

/// The same files, committed to a real repository — with `core.autocrlf` pinned
/// off.
///
/// The pin is not decoration. With autocrlf on (the Git-for-Windows default) the
/// index holds LF while the working tree holds CRLF, so the two sources would
/// hash different bytes for one file and every cross-source comparison below
/// would fail for a reason that has nothing to do with what it is testing.
fn committed_tree(files: &[(&str, &str)]) -> TempDir {
    let tmp = TempDir::new().unwrap();
    let repo = Repository::init(tmp.path()).unwrap();
    repo.config()
        .unwrap()
        .set_bool("core.autocrlf", false)
        .unwrap();
    common::write_and_commit(&repo, tmp.path(), "Ann", "initial", files);
    tmp
}

fn keys(root: &Path) -> Vec<String> {
    Repo::open(root).unwrap().tracked_files().unwrap()
}

#[test]
fn a_directory_with_no_repository_enumerates_its_files() {
    let tree = plain_tree(&[("src/lib.rs", "pub fn f() {}\n"), ("README.md", "# hi\n")]);
    assert_eq!(keys(tree.path()), vec!["README.md", "src/lib.rs"]);
}

/// The tree's own `.gitignore` is the one ignore source that IS respected, and
/// respecting it has to reach the artifact, not just the enumeration: a file
/// absent from `tracked_files` but present in `coverage.files` would be a
/// coverage claim about a file that was never read.
#[test]
fn a_gitignored_file_is_absent_from_the_enumeration_and_from_coverage() {
    let tree = plain_tree(&[
        (".gitignore", "secrets/\n*.tmp\n"),
        ("src/lib.rs", "pub fn f() {}\n"),
        ("secrets/token.rs", "pub const T: &str = \"sk-live\";\n"),
        ("scratch.tmp", "junk\n"),
    ]);

    let enumerated = keys(tree.path());
    assert_eq!(enumerated, vec![".gitignore", "src/lib.rs"]);

    let out = TempDir::new().unwrap();
    let generated =
        reverse_architect::generate(&common::config_for(tree.path(), out.path())).unwrap();
    let covered: Vec<&str> = generated
        .coverage
        .files
        .iter()
        .map(|f| f.path.as_str())
        .collect();
    assert_eq!(covered, vec![".gitignore", "src/lib.rs"]);
}

/// `.gitignore` files in directories ABOVE the root are per-machine state as far
/// as this root is concerned — the root could be anywhere — so they are not
/// consulted. This is the one of the four disabled ignore sources that a test can
/// reach without writing to the operator's home directory; the other three
/// (`core.excludesFile`, the XDG global ignore, `.git/info/exclude`) are switched
/// off at the same call site by the same explicit flags.
#[test]
fn an_ignore_file_above_the_root_is_not_consulted() {
    let outer = TempDir::new().unwrap();
    std::fs::write(outer.path().join(".gitignore"), "*.rs\n").unwrap();
    let root = outer.path().join("inner");
    write_files(&root, &[("lib.rs", "pub fn f() {}\n")]);

    assert_eq!(keys(&root), vec!["lib.rs"]);
}

/// And an ignore file above the root cannot fail the run either.
///
/// `parents(false)` stops an ancestor's patterns being APPLIED but not its file
/// being READ — the early return that skips ancestors entirely needs
/// `git_ignore` false as well, and this walk needs it true. So a malformed glob
/// in a `.gitignore` somewhere above the source root reached the walk as a hard
/// error and aborted it: a file in a directory the operator may not even own
/// deciding whether an unrelated tree can be indexed at all. That is the
/// per-machine dependency this source exists to remove, arriving through the
/// error path instead of the matching path.
#[test]
fn a_malformed_ignore_file_above_the_root_does_not_fail_the_run() {
    let outer = TempDir::new().unwrap();
    // `a{b` and not `[unterminated`: globset accepts an unclosed `[` as a
    // literal, so that spelling would make this test pass without ever
    // reaching the failure it names. An unclosed alternate group does fail.
    std::fs::write(outer.path().join(".gitignore"), "a{b\n").unwrap();
    let root = outer.path().join("inner");
    write_files(&root, &[("lib.rs", "pub fn f() {}\n")]);

    assert_eq!(keys(&root), vec!["lib.rs"]);
}

/// The mirror image, and the reason the two belong in one change: a malformed
/// ignore file INSIDE the tree must fail loudly.
///
/// `ignore` reports it by attaching the failure to an entry it can still
/// traverse, so a walk that only inspects `Result` never sees it and applies the
/// file's readable patterns while dropping the rest. The file set is then neither
/// what the tree declared nor an error — and since the whole reason in-tree
/// ignore files are consulted is that they keep secrets out of the corpus (R22),
/// a half-applied one is exactly the case that must not pass quietly.
#[test]
fn a_malformed_ignore_file_inside_the_tree_fails_the_run_rather_than_being_half_applied() {
    let tree = plain_tree(&[
        (".gitignore", "a{b\nsecrets/\n"),
        ("lib.rs", "pub fn f() {}\n"),
        ("secrets/token.rs", "pub const T: &str = \"placeholder\";\n"),
    ]);

    let message = match Repo::open(tree.path()).unwrap().tracked_files() {
        Ok(enumerated) => panic!(
            "a malformed in-tree ignore file must not be half-applied silently: {enumerated:?}"
        ),
        Err(err) => err.to_string(),
    };
    assert!(
        message.contains(".gitignore"),
        "the failure must name the file that could not be understood: {message}"
    );
}

/// A bare `.ignore` file is ripgrep's convention, not git's. Honoring it would
/// make the corpus depend on which tools the operator happens to run.
#[test]
fn a_dot_ignore_file_is_not_consulted_because_it_is_not_a_git_concept() {
    let tree = plain_tree(&[(".ignore", "*.rs\n"), ("lib.rs", "pub fn f() {}\n")]);
    assert_eq!(keys(tree.path()), vec![".ignore", "lib.rs"]);
}

/// The walker's default hides dotfiles. Taking it would silently drop `.github/`
/// and `.editorconfig` — files git tracks like any other.
#[test]
fn dotfiles_and_dot_directories_are_enumerated() {
    let tree = plain_tree(&[
        (".github/workflows/ci.yml", "on: push\n"),
        (".editorconfig", "root = true\n"),
        ("lib.rs", "pub fn f() {}\n"),
    ]);
    assert_eq!(
        keys(tree.path()),
        vec![".editorconfig", ".github/workflows/ci.yml", "lib.rs"]
    );
}

/// The plain-directory analogue of the gitlink filter: a nested repository's
/// contents belong to that repository, and its `.git` holds credentials in
/// shapes `SecretPrefixes` does not match.
#[test]
fn a_nested_repository_contributes_neither_its_files_nor_its_git_directory() {
    let tree = plain_tree(&[("lib.rs", "pub fn f() {}\n")]);
    let nested = tree.path().join("vendor/dep");
    write_files(&nested, &[("dep.rs", "pub fn g() {}\n")]);
    Repository::init(&nested).unwrap();

    let enumerated = keys(tree.path());
    assert_eq!(enumerated, vec!["lib.rs"]);
    assert!(
        !enumerated.iter().any(|k| k.contains(".git")),
        "nothing from a .git directory may be enumerated: {enumerated:?}"
    );
}

/// A directory link, by whichever mechanism this host allows.
///
/// A real symlink needs Developer Mode or elevation on Windows, and a developer
/// machine commonly has neither — which turned the tests below into skips that
/// reported as passes. A **junction** needs no privilege at all, and Rust's std
/// reports one as `is_symlink` exactly as it does a symlink, which is the
/// property under test. The module doc names junctions alongside symlinks for
/// the same reason, so exercising one is not a weaker test but the other half of
/// the claim.
/// The `.git`-name guard on its own, which the nested-repository test above
/// cannot reach.
///
/// There the second guard prunes `vendor/dep` as a directory containing a
/// `.git`, so the walk never descends far enough to meet the `.git` directory
/// itself — the name guard could be deleted and that test would still pass. A
/// `.git` directory with no repository around it (a leftover, a partial clone, a
/// copied tree) is only excluded by the name.
#[test]
fn a_git_directory_with_no_repository_around_it_is_excluded_by_name() {
    let tree = plain_tree(&[
        ("lib.rs", "pub fn f() {}\n"),
        (
            ".git/config",
            "[remote \"origin\"]\n\turl = https://example.invalid/x\n",
        ),
        (".git/HEAD", "ref: refs/heads/main\n"),
    ]);

    // The `.git` here is not a repository — no `objects/`, no `refs/` — so
    // libgit2 reports "no repository here" and `Repo::open` falls back to the
    // directory walk. The nested-repo guard cannot help: the root is at depth 0
    // and exempt, and `.git` itself contains no `.git`. Only the name excludes
    // it, and its `config` is exactly the file that must not be enumerated.
    assert_eq!(keys(tree.path()), vec!["lib.rs"]);
}

/// `Repo::open`'s own guard, which the CLI never reaches — `Config::resolve`
/// refuses a non-directory first. It is asserted here because a library caller
/// that skips config resolution gets this message and nothing else, and because
/// it is the variant that replaced "not a git repository" as the reason to
/// refuse.
#[test]
fn opening_a_file_as_a_source_names_the_path_and_the_reason() {
    let tmp = TempDir::new().unwrap();
    let file = tmp.path().join("not-a-tree.txt");
    std::fs::write(&file, "hello").unwrap();

    let message = match Repo::open(&file) {
        Ok(_) => panic!("a file is not a source"),
        Err(err) => err.to_string(),
    };
    assert!(
        message.contains("is not a directory that can be indexed")
            && message.contains("not-a-tree.txt"),
        "the refusal must give the reason and the path: {message}"
    );
}

/// A regular file from a directory source carries git's regular-file mode. Only
/// the symlink mode was ever asserted, so the pair could have been swapped and
/// every other test would still pass.
#[test]
fn a_regular_file_from_a_directory_carries_gits_regular_file_mode() {
    let tree = plain_tree(&[("lib.rs", "pub fn f() {}\n")]);
    let tracked = Repo::open(tree.path())
        .unwrap()
        .tracked_files_with_paths()
        .unwrap();

    let file = tracked.iter().find(|tf| tf.key == "lib.rs").unwrap();
    assert_eq!(file.mode, 0o100644, "git's regular-file mode");
    assert!(!file.is_symlink);
}

#[cfg(windows)]
fn dir_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    match std::os::windows::fs::symlink_dir(target, link) {
        Ok(()) => Ok(()),
        Err(_) => junction(target, link),
    }
}

#[cfg(windows)]
fn junction(target: &Path, link: &Path) -> std::io::Result<()> {
    let status = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(
            "mklink /J declined to create a junction",
        ))
    }
}
#[cfg(unix)]
fn dir_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

/// Following a directory symlink would enumerate files from OUTSIDE the root
/// under in-root keys, and can loop. The link is still recorded — as a link, with
/// git's symlink mode — rather than dropped silently.
///
/// `dir_symlink` falls back to a junction where a real symlink needs privileges
/// the host does not grant, so this normally runs. If even that is refused the
/// test says so and stops, rather than passing vacuously: a skipped assertion
/// that prints nothing is a test that lies.
#[test]
fn a_directory_symlink_is_recorded_as_a_link_and_never_descended() {
    let outside = plain_tree(&[("hidden.rs", "pub fn should_not_appear() {}\n")]);
    let tree = plain_tree(&[("lib.rs", "pub fn f() {}\n")]);
    if dir_symlink(outside.path(), &tree.path().join("link")).is_err() {
        eprintln!(
            "SKIPPED a_directory_symlink_is_recorded_as_a_link_and_never_descended: \
             this host does not permit creating directory symlinks"
        );
        return;
    }

    let repo = Repo::open(tree.path()).unwrap();
    let tracked = repo.tracked_files_with_paths().unwrap();
    let enumerated: Vec<&str> = tracked.iter().map(|tf| tf.key.as_str()).collect();
    assert_eq!(enumerated, vec!["lib.rs", "link"]);

    let link = tracked.iter().find(|tf| tf.key == "link").unwrap();
    assert!(link.is_symlink, "the link must be recorded as one");
    assert_eq!(link.mode, 0o120000, "git's symlink mode");
}

/// A root that is ITSELF a symlink must not enumerate itself as a file.
///
/// `ignore` never runs `filter_entry` at depth 0, so the closure that prunes
/// nested repositories cannot see the root; `walkdir`'s `follow_root_links`
/// defaults to true, so a link given as the root is resolved and descended even
/// though `follow_links(false)` stops every link below it. The root entry
/// therefore arrives as a symlink, and before the depth-0 skip it passed the
/// file-type test and produced a `TrackedFile` whose relative path was empty: a
/// phantom entry with key `""` and git's symlink mode, standing for a file that
/// does not exist.
///
/// Descending IS the right behavior here — an operator who points the tool at a
/// symlink means the tree it names. What must not happen is the link appearing
/// inside its own enumeration.
#[test]
fn a_root_that_is_itself_a_symlink_enumerates_the_tree_and_not_the_link() {
    let target = plain_tree(&[("src/lib.rs", "pub fn f() {}\n"), ("README.md", "# hi\n")]);
    let holder = TempDir::new().unwrap();
    let link = holder.path().join("link");
    if dir_symlink(target.path(), &link).is_err() {
        eprintln!(
            "SKIPPED a_root_that_is_itself_a_symlink_enumerates_the_tree_and_not_the_link: \
             this host does not permit creating directory symlinks"
        );
        return;
    }

    let tracked = Repo::open(&link)
        .unwrap()
        .tracked_files_with_paths()
        .unwrap();
    let enumerated: Vec<&str> = tracked.iter().map(|tf| tf.key.as_str()).collect();
    assert_eq!(
        enumerated,
        vec!["README.md", "src/lib.rs"],
        "the tree behind the link is what was asked for"
    );
    assert!(
        tracked.iter().all(|tf| !tf.key.is_empty()),
        "the root's own entry must never become a file: {enumerated:?}"
    );
    assert!(
        tracked.iter().all(|tf| !tf.is_symlink),
        "nothing in this tree is a link; the root must not be reported as one"
    );
}

#[test]
fn two_runs_over_the_same_tree_enumerate_in_the_same_order() {
    let tree = plain_tree(&[
        ("z.rs", "pub fn z() {}\n"),
        ("a.rs", "pub fn a() {}\n"),
        ("m/n.rs", "pub fn n() {}\n"),
        ("m.rs", "pub fn m() {}\n"),
    ]);
    let first = keys(tree.path());
    let second = keys(tree.path());
    assert_eq!(first, second);
    assert_eq!(first, vec!["a.rs", "m.rs", "m/n.rs", "z.rs"]);
}

/// A directory inside a repository is refused, not walked as a flat tree.
///
/// `git2`'s `open` does not search upward, so `<repo>/src` reports "no
/// repository here" exactly as a standalone tree does — and taking the fallback
/// there is wrong three times over. It is the R22 hazard first: the walk rooted
/// at `src` never sees the repository's `.gitignore`, so a credential file
/// ignored at the root enters the corpus. It also reports `repo_revision: null`
/// for a tree that is at a known commit, and keys everything relative to `src`,
/// disagreeing with a full run of the same repository.
#[test]
fn a_directory_inside_a_repository_is_refused_rather_than_walked_as_a_flat_tree() {
    let tmp = committed_tree(&[(".gitignore", "*.env\n"), ("src/lib.rs", "pub fn f() {}\n")]);
    // Ignored at the ROOT, and invisible to a walk rooted at `src`.
    std::fs::write(
        tmp.path().join("src/.env"),
        "TOKEN=this-must-never-be-indexed\n",
    )
    .unwrap();

    // `Repo` is not `Debug` (nor was it before U15 — `git2::Repository` is not
    // either), so the refusal is unwrapped by hand rather than by `expect_err`.
    let message = match Repo::open(&tmp.path().join("src")) {
        Ok(_) => panic!("a directory inside a repository must be refused"),
        Err(err) => err.to_string(),
    };
    assert!(
        message.contains("is inside the one at"),
        "the error must name the enclosing repository: {message}"
    );
    // And must not send the operator to a run that may not cover what they
    // asked for. This fixture is exactly that case: `src/.env` is ignored at
    // the root, so indexing the repository — the remedy an earlier version of
    // this message prescribed outright — excludes the very file the subtree
    // run would have reached.
    assert!(
        message.contains("if it ignores this subtree"),
        "the error must say when the suggested run would not cover this tree: {message}"
    );

    // The repository root itself still opens, and still excludes the ignored
    // file — proving the refusal above is not hiding a broken ignore path.
    let keys = keys(tmp.path());
    assert_eq!(keys, vec![".gitignore", "src/lib.rs"]);
}

/// The same refusal against THIS crate's own checkout, which is a different test
/// than it looks and is the one that caught the bug.
///
/// The fixture above builds its repository in a temp directory, where
/// `Repository::discover` answers cleanly. A real checkout can answer with
/// libgit2's ownership check instead (`repository path ... is not owned by
/// current user`) — an error that fires *while searching upward*, which is to
/// say exactly when there IS a repository above. The first version of the guard
/// discarded any discover error as "no enclosing repository", passed the temp-dir
/// test, and still walked this crate's own `src/` as a flat tree: 49 files, root
/// `.gitignore` never consulted, `repo_revision: null` for a tree sitting at a
/// known commit.
///
/// So this asserts the outcome and not the reason. Both refusals are correct;
/// which one arrives is a property of the machine, not of the code.
///
/// **It is no longer the only cover for that branch, and that is the point.**
/// Because which refusal arrives depends on the machine's git configuration,
/// this test can be a no-op on a host that is not a checkout, or one where
/// libgit2 is content with the ownership — a skip that reports as a pass. The
/// classification itself is now unit-tested directly by handing
/// `enclosing_from` a constructed `NotFound` and a constructed ownership error
/// (`src/extract/mod.rs`), which fires everywhere. What remains here is the
/// end-to-end probe over a real checkout: worth keeping, redundant when it
/// skips.
#[test]
fn this_crate_s_own_src_directory_is_refused_however_the_machine_answers() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    if !manifest.join(".git").exists() {
        eprintln!(
            "SKIPPED this_crate_s_own_src_directory_is_refused_however_the_machine_answers: \
             not built from a git checkout"
        );
        return;
    }

    let message = match Repo::open(&manifest.join("src")) {
        Ok(_) => panic!("a subdirectory of this crate's own repository must be refused"),
        Err(err) => err.to_string(),
    };
    assert!(
        message.contains("is inside the one at") || message.contains("cannot tell whether"),
        "the refusal must say either which repository encloses it, or that it \
         could not tell: {message}"
    );
}

/// An empty directory produces an empty corpus rather than a panic. The test
/// this replaces asserted a failure on exactly this input; the "never a panic"
/// half of its intent is the half that survives U15.
#[test]
fn an_empty_directory_produces_a_corpus_rather_than_a_panic() {
    let tree = plain_tree(&[]);
    assert!(keys(tree.path()).is_empty());

    let out = TempDir::new().unwrap();
    let generated =
        reverse_architect::generate(&common::config_for(tree.path(), out.path())).unwrap();
    assert!(generated.coverage.files.is_empty());
    assert_eq!(generated.coverage.repo_revision, None);
}

/// Touching a file without changing it must change nothing.
///
/// This is the failure mode a filesystem source invites and a git source cannot
/// have: every cache decision keys on `TrackedFile.blob_id`, and an
/// implementation that derived that id from anything the filesystem tracks
/// alongside the content — an mtime, a size, an inode — would invalidate the
/// whole cache every time an editor saved a file back unchanged. Rewriting a
/// file with its own bytes is exactly that event.
#[test]
fn rewriting_a_file_with_its_own_bytes_changes_no_content_id() {
    let tree = plain_tree(&[
        ("src/lib.rs", "pub fn f() {}\n"),
        ("src/other.rs", "pub fn g() {}\n"),
    ]);
    let ids = |root: &Path| -> Vec<(String, String)> {
        Repo::open(root)
            .unwrap()
            .tracked_files_with_paths()
            .unwrap()
            .into_iter()
            .map(|tf| (tf.key, tf.blob_id.to_string()))
            .collect()
    };

    let before = ids(tree.path());
    let untouched = tree.path().join("src/other.rs");
    let same_bytes = std::fs::read(&untouched).unwrap();
    std::fs::write(&untouched, &same_bytes).unwrap();

    assert_eq!(before, ids(tree.path()));
}

/// The identity claim: the same bytes get the same content id from either
/// source. This is what lets `CacheStore` keys, cache hits and every
/// `TrackedFile.blob_id` consumer work unchanged for a directory, and what makes
/// the corpus equivalence below possible at all.
#[test]
fn one_file_gets_one_blob_id_through_git_and_through_a_directory() {
    let files: &[(&str, &str)] = &[("src/lib.rs", "pub fn f() {}\n")];
    let git = committed_tree(files);
    let plain = plain_tree(files);

    let id_of = |root: &Path| {
        Repo::open(root)
            .unwrap()
            .tracked_files_with_paths()
            .unwrap()
            .into_iter()
            .find(|tf| tf.key == "src/lib.rs")
            .expect("src/lib.rs enumerated")
            .blob_id
    };
    assert_eq!(id_of(git.path()), id_of(plain.path()));
}

/// The strongest statement this unit can make: one tree, indexed both ways,
/// produces the same corpus bytes. `coverage.json` is excluded and compared
/// separately — it is the one artifact that is *supposed* to differ, because it
/// records the revision, and a directory has none.
#[test]
fn a_tree_indexed_as_git_and_as_a_directory_produces_the_same_corpus() {
    let files: &[(&str, &str)] = &[
        ("src/lib.rs", "pub fn f() -> u32 { 1 }\n"),
        ("src/app.rs", "use crate::lib::f;\npub fn g() { f(); }\n"),
        ("README.md", "# demo\n"),
    ];
    let git = committed_tree(files);
    let plain = plain_tree(files);

    let run = |root: &Path| {
        let out = TempDir::new().unwrap();
        Command::cargo_bin("reverse-architect")
            .unwrap()
            .arg(root)
            .arg("--out")
            .arg(out.path())
            .assert()
            .success();
        std::fs::remove_file(out.path().join("coverage.json")).unwrap();
        (common::hash_dir(out.path()), out)
    };

    let (from_git, _a) = run(git.path());
    let (from_directory, _b) = run(plain.path());
    assert_eq!(
        from_git, from_directory,
        "the same tree must produce the same corpus whether or not it has commits"
    );
}

/// A directory has no commits, and the artifact says `null` rather than omitting
/// the field — so a consumer can tell "no revision" from "not written" (R6).
#[test]
fn repo_revision_is_null_for_a_directory_and_the_artifact_says_so() {
    let tree = plain_tree(&[("lib.rs", "pub fn f() {}\n")]);
    let out = TempDir::new().unwrap();
    let generated =
        reverse_architect::generate(&common::config_for(tree.path(), out.path())).unwrap();

    assert_eq!(generated.coverage.repo_revision, None);
    let json = serde_json::to_string(&generated.coverage).unwrap();
    assert!(
        json.contains("\"repo_revision\":null"),
        "the absence must be written, not omitted: {json}"
    );
}
