//! U5: automatic cache activation from `generate`/`generate_reporting`.
//!
//! These are black-box tests against the crate's PUBLIC surface only
//! (`Config`, `generate`) — the activation decision itself
//! (`extract::activate`, `extract::compute_fingerprint`) and the
//! `RunFingerprint` type it reasons about are `pub(crate)`, not reachable
//! from this external test crate. The exact-match / KTD8-ordering proofs
//! (including the "interrupted first run" scenario) live as white-box unit
//! tests inside `src/extract/cache.rs`, which has that access; this file
//! only proves what is observable from outside: what `generate` returns and
//! what it leaves on disk.
//!
//! The cache directory name (`.ra-cache`, directly under `corpus_dir`) is an
//! internal implementation detail with no public constant — these tests
//! hardcode it deliberately, as a fixed point this file is entitled to
//! assume (the plan requires it never collide with a real corpus filename,
//! never that the exact name be part of the public contract).

use reverse_architect::generate;

mod common;

const CACHE_DIR_NAME: &str = ".ra-cache";

/// Scenario 1 (black-box half): a clean run leaves a readable header file on
/// disk. The exact byte-for-byte fingerprint match is proven at the unit
/// level (`activation_on_a_cold_cache_writes_a_header_matching_a_fresh_fingerprint`
/// in `src/extract/cache.rs`, which has access to the private `RunFingerprint`
/// type); here we only confirm the externally observable shape: the file
/// exists, is valid JSON, and mentions the one tracked file's path.
#[test]
fn a_clean_run_leaves_a_readable_header_on_disk() {
    let repo = tempfile::tempdir().unwrap();
    common::init_repo_with_files(repo.path(), "Alice", &[("src/lib.rs", "pub fn f() {}\n")]);
    let out = tempfile::tempdir().unwrap();

    generate(&common::config_for(repo.path(), out.path())).expect("the pipeline runs");

    let header_path = out.path().join(CACHE_DIR_NAME).join("fingerprint.json");
    let header_bytes = std::fs::read(&header_path).expect("a clean run must leave a header");
    let header: serde_json::Value =
        serde_json::from_slice(&header_bytes).expect("the header must be valid JSON");
    let as_text = header.to_string();
    assert!(
        as_text.contains("src/lib.rs"),
        "the header must describe the tracked file it fingerprinted, got: {as_text}"
    );
}

/// Scenario 3: a run whose cache directory cannot be created (a plain file
/// already sits where `.ra-cache` would go — the portable way to force this
/// failure, since permission bits behave differently across platforms)
/// degrades gracefully: `generate` still returns `Ok(Generated { .. })`, and
/// the corpus it produces is unaffected.
#[test]
fn an_unwritable_cache_directory_degrades_gracefully_without_failing_the_run() {
    let repo = tempfile::tempdir().unwrap();
    common::init_repo_with_files(repo.path(), "Alice", &[("src/lib.rs", "pub fn f() {}\n")]);
    let out = tempfile::tempdir().unwrap();
    // Block the cache directory: a plain file occupies the path `activate`
    // needs to create as a directory.
    std::fs::write(out.path().join(CACHE_DIR_NAME), b"not a directory").unwrap();

    let result = generate(&common::config_for(repo.path(), out.path()));

    let generated = result.expect("a cache-plumbing failure must never fail the run");
    assert!(
        !generated.corpus.files.is_empty(),
        "the corpus must still be produced when caching is unavailable"
    );
}

/// Scenario 4: changing the repo's content between two runs over the SAME
/// `corpus_dir` causes the second run's cache activation to clear the store
/// and write a new header — proven here without ever calling any
/// cache-internal API: run 1 leaves REAL per-file records (through ordinary
/// extraction), and run 2, after the repo changed, must both rewrite the
/// header and remove the old blob's record.
#[test]
fn changing_repo_content_between_runs_clears_the_store_and_writes_a_new_header() {
    let repo = tempfile::tempdir().unwrap();
    let git =
        common::init_repo_with_files(repo.path(), "Alice", &[("src/lib.rs", "pub fn f() {}\n")]);
    let out = tempfile::tempdir().unwrap();
    let config = common::config_for(repo.path(), out.path());

    let old_blob = common::blob_id_for(repo.path(), "src/lib.rs");

    generate(&config).expect("run 1 must succeed");

    let header_path = out.path().join(CACHE_DIR_NAME).join("fingerprint.json");
    let header_before = std::fs::read(&header_path).expect("run 1 must leave a header");
    let old_record_path = out
        .path()
        .join(CACHE_DIR_NAME)
        .join("languages-decl")
        .join(format!("{old_blob}.json"));
    assert!(
        old_record_path.is_file(),
        "run 1 must leave a real per-file cache record for the tracked file"
    );

    common::write_and_commit(
        &git,
        repo.path(),
        "Alice",
        "v2",
        &[("src/lib.rs", "pub fn g() {}\n")],
    );

    generate(&config).expect("run 2 must succeed");

    let header_after = std::fs::read(&header_path).expect("run 2 must leave a header");
    assert_ne!(
        header_before, header_after,
        "changed repo content must change the on-disk header"
    );
    assert!(
        !old_record_path.is_file(),
        "a fingerprint mismatch must clear the OLD blob's record, not just add a new one"
    );
}

/// R4: the generated corpus/manifest/cards are byte-identical no matter what
/// state the cache is in — a run with caching entirely blocked (baseline), a
/// first ("cold") run that populates the cache, and a second ("warm") run
/// that reads it back, all over the identical unchanged repo, must agree
/// exactly.
#[test]
fn generated_output_is_byte_identical_regardless_of_cache_state() {
    let repo = tempfile::tempdir().unwrap();
    common::init_repo_with_files(
        repo.path(),
        "Alice",
        &[
            ("proj.csproj", "<Project></Project>\n"),
            ("src/lib.rs", "pub fn f() -> i32 { 1 }\n"),
            // The call sits outside the macro: a macro's arguments are a token
            // tree, not a call the grammar can see, and the comparison below
            // must cover a name-resolved edge (see the precondition).
            (
                "src/main.rs",
                "fn main() { let n = lib::f(); println!(\"{}\", n); }\n",
            ),
            // Plan B (U15): the facts a detail is built from — anchors,
            // database accesses, enum decisions, and a C file's own texts —
            // travel through the cached records too.
            (
                "App/Loader.cs",
                "namespace App\n{\n    public enum Kind\n    {\n        Full,\n        Diff,\n    }\n\n    \
                 public class Loader\n    {\n        public void Run(Kind kind)\n        {\n            \
                 switch (kind)\n            {\n                case Kind.Full:\n                    \
                 Db.Execute(\"update dba.loads set done = 1\");\n                    break;\n            \
                 }\n            // Loads the whole backup\n            \
                 throw new System.Exception(\"The backup cannot be loaded\");\n        }\n    }\n}\n",
            ),
            (
                "native/load.c",
                "void load(void) {\n    const char* q = \"insert into dba.load_log (done) values (1)\";\n}\n",
            ),
        ],
    );

    // Baseline: caching is blocked for this corpus_dir, so extraction always
    // runs cold, every time.
    let out_baseline = tempfile::tempdir().unwrap();
    std::fs::write(out_baseline.path().join(CACHE_DIR_NAME), b"not a directory").unwrap();
    let baseline =
        generate(&common::config_for(repo.path(), out_baseline.path())).expect("baseline run");

    // Cache-enabled: two runs over the SAME corpus_dir/cache root — the first
    // is cold (populates the cache), the second is warm (reads it back).
    let out_cached = tempfile::tempdir().unwrap();
    let config_cached = common::config_for(repo.path(), out_cached.path());
    let cold = generate(&config_cached).expect("cold run");
    let warm = generate(&config_cached).expect("warm run");

    // The confidence rung (plan A, U1) travels through the cached usages
    // record, so the comparison below must cover at least one rated edge.
    assert!(
        warm.manifest
            .edges
            .iter()
            .any(|e| e.provenance.confidence.is_some()),
        "precondition: the fixture produces a name-resolved edge with a rung"
    );
    let detail = |suffix: &str| {
        warm.details
            .iter()
            .find(|f| f.path.ends_with(suffix))
            .unwrap_or_else(|| panic!("no detail ending {suffix}"))
            .contents
            .as_str()
    };
    let loader = detail("/Loader.md");
    assert!(
        loader.contains("\n## Anchors\n") && loader.contains("writes `dba.loads`"),
        "precondition: a warm run still reads anchors and accesses: {loader}"
    );
    assert!(
        detail("/Kind.md").contains("\n## Enum decisions\n"),
        "precondition: and enum decisions"
    );
    assert!(
        detail("native/load.c.md").contains("writes `dba.load_log`"),
        "precondition: and a C file's own texts"
    );
    assert_eq!(
        baseline.corpus, cold.corpus,
        "baseline vs cold corpus differs"
    );
    assert_eq!(
        baseline.corpus, warm.corpus,
        "baseline vs warm corpus differs"
    );
    assert_eq!(
        baseline.manifest, cold.manifest,
        "baseline vs cold manifest differs"
    );
    assert_eq!(
        baseline.manifest, warm.manifest,
        "baseline vs warm manifest differs"
    );
    assert_eq!(baseline.cards, cold.cards, "baseline vs cold cards differ");
    assert_eq!(baseline.cards, warm.cards, "baseline vs warm cards differ");
    // Plan B, U11: the details and the maps are written like the cards.
    assert_eq!(baseline.details, cold.details, "baseline vs cold details");
    assert_eq!(baseline.details, warm.details, "baseline vs warm details");
    assert_eq!(baseline.maps, cold.maps, "baseline vs cold maps differ");
    assert_eq!(baseline.maps, warm.maps, "baseline vs warm maps differ");
    assert_eq!(
        baseline.coverage, cold.coverage,
        "baseline vs cold coverage differs"
    );
    assert_eq!(
        baseline.coverage, warm.coverage,
        "baseline vs warm coverage differs"
    );
}
