//! U6: acceptance tests for the cross-run extraction cache (U1-U5) — the
//! final gate for R1, R2, R4, R5, R6. U7 adds one more scenario below for R7
//! (per-record fingerprint tag), the P0 `ce-code-review` finding closed after
//! U1-U6 shipped.
//!
//! `tests/cache_activation.rs` (U5) already proves: a clean run leaves a
//! readable header; an unwritable cache directory degrades gracefully; a
//! changed repo rewrites the header and drops the old blob's record; and the
//! corpus is byte-identical across baseline/cold/warm runs. This file proves
//! what that one does not:
//!
//! - R1: reusing a cache a real prior run populated — through the public
//!   `generate` entry point, with the per-file records read back to PROVE a
//!   genuine cache HIT occurred, not merely that the output happened to match.
//! - R2: a change to ONE tracked file invalidates the ENTIRE cache, for every
//!   phase and every file — including files whose content never changed.
//! - R4: restated end-to-end here as this unit's own acceptance proof (see
//!   `cache_activation.rs` for the original).
//! - R5: a single corrupted per-file record, reached through the real
//!   pipeline, degrades to a miss for that one file only — the run still
//!   succeeds and the rest of the cache is left untouched.
//! - R6: the on-disk cache directory holds only the three `walk_files`-based
//!   phases (plus the header) — never a trace of the other five extractors.
//!
//! The exact-match/KTD8-ordering mechanics themselves, and the "interrupted
//! first run" simulation at the lowest level, are proven in
//! `src/extract/cache.rs`'s own unit tests; this file only reasons about the
//! public API (`Config`, `generate`) and what lands on disk under
//! `<corpus_dir>/.ra-cache/`.

use reverse_architect::generate;
use std::path::{Path, PathBuf};

mod common;

use common::hash_dir;

/// Hidden cache directory name, directly under `corpus_dir` — an internal
/// implementation detail with no public constant (see `cache_activation.rs`'s
/// module doc for why hardcoding it here is deliberate, not an oversight).
const CACHE_DIR_NAME: &str = ".ra-cache";

/// The three `walk_files`-based phases the cache covers (R6) — every OTHER
/// extractor (`metrics`, `manifest_deps`, `scip`, `filetree`, `test_pairing`)
/// never calls `walk_files` and so can never produce a record under any of
/// these.
const PHASES: [&str; 3] = ["languages-decl", "languages-usages", "external"];

/// A small fixture that is `wanted` by all three cached phases: `.rs` files
/// are lexed by `external` (`Lang::from_path`) regardless of whether they
/// reference anything foreign, and parsed by both tree-sitter passes — so a
/// run over this fixture always leaves a record under every one of `PHASES`.
/// The exact fixture `cache_activation.rs`'s R4 test already uses, reused
/// here rather than invented anew.
fn fixture_files() -> Vec<(&'static str, &'static str)> {
    vec![
        ("proj.csproj", "<Project></Project>\n"),
        ("src/lib.rs", "pub fn f() -> i32 { 1 }\n"),
        ("src/main.rs", "fn main() { println!(\"{}\", lib::f()); }\n"),
    ]
}

/// Block a `corpus_dir`'s cache directory before it is ever used: a plain
/// file occupies the path `activate` would need to create as a directory, so
/// every run against this `corpus_dir` is unconditionally cold — the
/// independent "ground truth" every scenario below compares against.
fn block_cache(out: &Path) {
    std::fs::write(out.join(CACHE_DIR_NAME), b"not a directory").unwrap();
}

/// The exact on-disk path a `(phase, blob)` record lives at — mirrors
/// `CacheStore::path_for`, which is private to `src/extract/cache.rs`.
fn record_path(cache_root: &Path, phase: &str, blob_id: git2::Oid) -> PathBuf {
    cache_root.join(phase).join(format!("{blob_id}.json"))
}

/// Overwrite the `(phase, blob_id)` record with a deliberately WRONG-but-
/// still-valid value tagged under `key` — "no facts learned from this file"
/// — and hand back the bytes written, so a caller can later assert they were
/// NOT what survived.
///
/// This is deliberately NOT "delete the file and check it comes back": for a
/// file whose content (and therefore blob id) is UNCHANGED across two runs,
/// deterministic parsing means a genuine wholesale clear-then-reparse writes
/// back BYTE-IDENTICAL content to the same path anyway — so "the file exists
/// again afterward" proves nothing about whether it was ever cleared. Poison
/// beats that: only an actual reparse can produce the TRUE facts for the
/// file, so if the poisoned bytes are gone afterward (replaced by the true
/// per-file record and reflected in a correct corpus), a real reparse
/// happened — a per-file heuristic that trusted the still-matching
/// `(phase, blob_id, key)` triple would instead have kept exactly this
/// poison and never noticed the surrounding cache was invalidated.
///
/// The wire shape deliberately omits `fingerprint` (U7's per-record digest
/// tag, added to `src/extract/cache.rs`'s private `RecordRef { fingerprint,
/// key, value }` envelope) — a real record always carries it, so a hand-
/// written one missing it fails to deserialize at all, which is just another
/// shape of "malformed record" and degrades to a miss the same way (KTD6),
/// exactly what this poison needs. `value`'s own shape differs per phase: `external`
/// caches `walk::GraphOps`, a tuple newtype around `Vec<GraphOp>` that
/// serializes as a bare JSON array; `languages-decl`/`languages-usages`
/// cache `treesitter::FileOutputWire`, a `#[derive(Default)]` struct with two
/// `Vec` fields. Both empty forms are cheap to spell out directly — neither
/// type is reachable from this external test crate to construct for real.
fn poison_record(cache_root: &Path, phase: &str, blob_id: git2::Oid, key: &str) -> Vec<u8> {
    let value_json = if phase == "external" {
        "[]"
    } else {
        "{\"ops\":[],\"relations\":[]}"
    };
    let bytes = format!("{{\"key\":{key:?},\"value\":{value_json}}}").into_bytes();
    std::fs::write(record_path(cache_root, phase, blob_id), &bytes).unwrap();
    bytes
}

/// R1 (end-to-end half — the low-level KTD8 ordering proof is
/// `src/extract/cache.rs`'s `a_header_written_before_extraction_survives_...`
/// test): a cache a real prior run populated, through the real public
/// pipeline, is genuinely REUSED by a second run — not merely "produces the
/// same output," which two independent cold runs would also do vacuously.
///
/// Proof of a genuine HIT: every byte under `.ra-cache/` — every per-file
/// record, the header, all three phase directories together — is IDENTICAL
/// before and after the second call. A silent full reparse that happened to
/// recompute the same facts would still rewrite each record's file (new mtime
/// aside, `CacheStore::put` unconditionally overwrites), so byte-for-byte
/// stability is only explained by the records having been READ, never
/// written, on the second call.
#[test]
fn reusing_a_populated_cache_across_two_full_runs_yields_the_same_corpus_and_reuses_records() {
    let repo = tempfile::tempdir().unwrap();
    common::init_repo_with_files(repo.path(), "Alice", &fixture_files());

    // Ground truth: an independent corpus_dir whose cache is never consulted
    // at all.
    let baseline_out = tempfile::tempdir().unwrap();
    block_cache(baseline_out.path());
    let baseline =
        generate(&common::config_for(repo.path(), baseline_out.path())).expect("baseline run");

    let out = tempfile::tempdir().unwrap();
    let config = common::config_for(repo.path(), out.path());
    let cache_root = out.path().join(CACHE_DIR_NAME);

    // "Run 1": a fresh corpus_dir, first-ever use of this cache — this is
    // exactly what any real first run leaves behind, whether or not a LATER
    // run over the same cache is a resume after a crash or a plain repeat.
    let run1 = generate(&config).expect("run 1 (fresh cache)");
    assert_eq!(
        baseline.corpus, run1.corpus,
        "precondition: run 1 must already agree with the uncached baseline"
    );

    let lib_blob = common::blob_id_for(repo.path(), "src/lib.rs");
    let mut per_phase_before: Vec<(&str, Vec<u8>)> = Vec::new();
    for phase in PHASES {
        let path = record_path(&cache_root, phase, lib_blob);
        assert!(
            path.is_file(),
            "run 1 must leave a {phase} record for src/lib.rs"
        );
        per_phase_before.push((phase, std::fs::read(&path).unwrap()));
    }
    let cache_digest_before = hash_dir(&cache_root);

    // "Run 2": a retry over the SAME corpus_dir/cache root, unchanged repo —
    // the scenario a resumed or simply repeated run actually faces.
    let run2 = generate(&config).expect("run 2 (reusing the populated cache)");

    let cache_digest_after = hash_dir(&cache_root);
    assert_eq!(
        cache_digest_before, cache_digest_after,
        "run 2 must not add, remove, or rewrite a single byte under the cache \
         directory when nothing changed — proof of a genuine cache HIT, not a \
         silent full reparse that happened to produce the same corpus"
    );
    for (phase, before) in &per_phase_before {
        let after = std::fs::read(record_path(&cache_root, phase, lib_blob)).unwrap();
        assert_eq!(
            *before, after,
            "{phase}'s record for src/lib.rs must be byte-identical before \
             and after run 2"
        );
    }

    assert_eq!(
        baseline.corpus, run2.corpus,
        "R4: corpus differs after reuse"
    );
    assert_eq!(
        baseline.manifest, run2.manifest,
        "R4: manifest differs after reuse"
    );
    assert_eq!(baseline.cards, run2.cards, "R4: cards differ after reuse");
    assert_eq!(
        baseline.coverage, run2.coverage,
        "R4: coverage differs after reuse"
    );
}

/// R2: a content change to ONE tracked file invalidates the cache for EVERY
/// tracked file across EVERY phase — never a per-file heuristic.
/// `src/Shared.cs` changes between two commits; `src/Service.cs` does not,
/// and keeps the same blob id.
///
/// Proving this by mere file EXISTENCE would be a false proof: since parsing
/// is deterministic and `src/Service.cs`'s blob id does not change, a genuine
/// wholesale clear-then-reparse writes a record right back at the exact same
/// `(phase, blob_id)` path — so "the file exists after run 2" is true either
/// way and proves nothing. Instead, `src/Service.cs`'s commit-1 records are
/// POISONED with a deliberately wrong "no facts" value before commit 2's run
/// ([`poison_record`]). Only an actual reparse can replace that poison with
/// the true facts; a per-file heuristic that trusted the still-matching
/// `(phase, blob_id, key)` triple would keep the poison and silently corrupt
/// the corpus. `src/Service.cs` is deliberately built to have GENUINELY
/// non-empty facts in every one of the three phases — it declares a method
/// (`languages-decl`), calls a symbol declared in the OTHER file
/// (`languages-usages`, a real cross-file edge, so an empty poison can never
/// coincide with the truth by accident — an earlier, simpler fixture of two
/// argument-less leaf functions failed exactly this way: their real
/// `languages-usages` output was ALSO empty, so poisoning with "no facts"
/// proved nothing there), and references a declared NuGet package
/// (`external`).
///
/// `src/Shared.cs`'s OLD (commit-1) blob id, by contrast, is genuinely
/// abandoned — no future run ever asks for it again — so its record's
/// disappearance IS a valid existence proof on its own, no poisoning needed.
#[test]
fn a_content_change_to_one_file_invalidates_the_cache_for_every_tracked_file_across_all_phases() {
    let repo_dir = tempfile::tempdir().unwrap();
    let git = common::init_repo_with_files(
        repo_dir.path(),
        "Alice",
        &[
            (
                "proj.csproj",
                "<Project>\n  <ItemGroup>\n    \
                 <PackageReference Include=\"Newtonsoft.Json\" Version=\"13.0.3\" />\n  \
                 </ItemGroup>\n</Project>\n",
            ),
            (
                "src/Shared.cs",
                "namespace Shared;\n\npublic class Repository\n{\n    \
                 public void Save() {}\n}\n",
            ),
            (
                "src/Service.cs",
                "using Newtonsoft.Json;\nusing Shared;\n\nnamespace App;\n\n\
                 public class Service\n{\n    public string Run()\n    {\n        \
                 var repo = new Repository();\n        repo.Save();\n        \
                 return JsonConvert.SerializeObject(repo);\n    }\n}\n",
            ),
        ],
    );
    let out = tempfile::tempdir().unwrap();
    let config = common::config_for(repo_dir.path(), out.path());
    let cache_root = out.path().join(CACHE_DIR_NAME);

    let shared_blob_commit1 = common::blob_id_for(repo_dir.path(), "src/Shared.cs");
    let service_blob_commit1 = common::blob_id_for(repo_dir.path(), "src/Service.cs");

    generate(&config).expect("run over commit 1");

    let mut poison_bytes: Vec<(&str, Vec<u8>)> = Vec::new();
    for phase in PHASES {
        assert!(
            record_path(&cache_root, phase, shared_blob_commit1).is_file(),
            "commit 1 must leave a {phase} record for src/Shared.cs"
        );
        assert!(
            record_path(&cache_root, phase, service_blob_commit1).is_file(),
            "commit 1 must leave a {phase} record for src/Service.cs"
        );
        // Poison the UNCHANGED file's record in every phase — see the test's
        // doc comment for why existence alone cannot prove wholesale
        // invalidation here.
        let bytes = poison_record(&cache_root, phase, service_blob_commit1, "src/Service.cs");
        poison_bytes.push((phase, bytes));
    }

    // Commit 2 changes ONLY src/Shared.cs. src/Service.cs's content — and
    // therefore its content-addressed blob id — does not change.
    common::write_and_commit(
        &git,
        repo_dir.path(),
        "Alice",
        "v2",
        &[(
            "src/Shared.cs",
            "namespace Shared;\n\npublic class Repository\n{\n    \
             public void Save() { /* v2 */ }\n}\n",
        )],
    );
    let service_blob_commit2 = common::blob_id_for(repo_dir.path(), "src/Service.cs");
    assert_eq!(
        service_blob_commit1, service_blob_commit2,
        "precondition: src/Service.cs's content — and blob id — did not \
         change between the two commits"
    );

    let run2 = generate(&config).expect("run over commit 2, same cache root");

    // Ground truth for commit 2's output: a corpus_dir that never had a cache
    // to begin with (proves correctness) and a SEPARATE corpus_dir with its
    // own fresh, unpoisoned cache (proves what a genuine reparse's record
    // bytes actually look like, for the byte-level comparison below).
    let baseline_out = tempfile::tempdir().unwrap();
    block_cache(baseline_out.path());
    let baseline = generate(&common::config_for(repo_dir.path(), baseline_out.path()))
        .expect("baseline run over commit 2");

    let truth_out = tempfile::tempdir().unwrap();
    let truth_config = common::config_for(repo_dir.path(), truth_out.path());
    generate(&truth_config).expect("truth run over commit 2, its own fresh cache");
    let truth_cache_root = truth_out.path().join(CACHE_DIR_NAME);

    assert_eq!(
        baseline.corpus, run2.corpus,
        "commit-2 corpus must be correct even though the cache carried \
         commit-1 (and poisoned) records going in"
    );
    assert_eq!(baseline.manifest, run2.manifest);
    assert_eq!(baseline.cards, run2.cards);
    assert_eq!(baseline.coverage, run2.coverage);

    // The core R2 proof, part 1: src/Service.cs's record, in every phase, is
    // no longer the poison — it was overwritten with the TRUE record a real
    // reparse produces (byte-identical to the independent truth run's own
    // record for the same blob, since parsing is deterministic).
    for (phase, poison) in &poison_bytes {
        let after = std::fs::read(record_path(&cache_root, phase, service_blob_commit2)).unwrap();
        assert_ne!(
            &after, poison,
            "{phase}'s poisoned record for src/Service.cs must have been \
             overwritten by a real reparse, not preserved because its \
             (phase, blob_id, key) triple still matched"
        );
        let truth = std::fs::read(record_path(&truth_cache_root, phase, service_blob_commit2))
            .expect("the truth run must leave its own record for this blob");
        assert_eq!(
            after, truth,
            "{phase}'s record for src/Service.cs must match what an \
             independent, never-poisoned reparse of the same content produces"
        );
    }

    // The core R2 proof, part 2: src/Shared.cs's OLD (commit-1) blob id is
    // genuinely abandoned — no run ever asks for it again — so its record
    // must be gone, in every phase, not left behind alongside the new one.
    for phase in PHASES {
        assert!(
            !record_path(&cache_root, phase, shared_blob_commit1).is_file(),
            "{phase}'s record for commit 1's (now-abandoned) src/Shared.cs \
             blob must be cleared, not just superseded by a new one"
        );
    }

    // And the cache is genuinely usable again after the wipe, not just left
    // empty: both files have fresh records under the new regime.
    let shared_blob_commit2 = common::blob_id_for(repo_dir.path(), "src/Shared.cs");
    for phase in PHASES {
        assert!(
            record_path(&cache_root, phase, shared_blob_commit2).is_file(),
            "{phase} must hold a fresh record for commit 2's src/Shared.cs"
        );
        assert!(
            record_path(&cache_root, phase, service_blob_commit2).is_file(),
            "{phase} must hold a fresh record for src/Service.cs under the \
             new regime too"
        );
    }
}

/// R4, restated as this unit's own end-to-end acceptance proof (the original
/// is `cache_activation.rs`'s `generated_output_is_byte_identical_regardless_of_cache_state`):
/// a run whose cache is blocked entirely, a first ("cold") run that populates
/// a real cache, and a second ("warm") run that reads it back, all over the
/// identical unchanged repo, must produce byte-identical corpus, manifest,
/// cards, and coverage.
#[test]
fn generated_output_is_byte_identical_across_cold_warm_and_no_cache_runs() {
    let repo = tempfile::tempdir().unwrap();
    common::init_repo_with_files(repo.path(), "Alice", &fixture_files());

    let baseline_out = tempfile::tempdir().unwrap();
    block_cache(baseline_out.path());
    let baseline = generate(&common::config_for(repo.path(), baseline_out.path()))
        .expect("baseline (no cache) run");

    let cached_out = tempfile::tempdir().unwrap();
    let cached_config = common::config_for(repo.path(), cached_out.path());
    let cold = generate(&cached_config).expect("cold run");
    let warm = generate(&cached_config).expect("warm run");

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
    assert_eq!(
        baseline.coverage, cold.coverage,
        "baseline vs cold coverage differs"
    );
    assert_eq!(
        baseline.coverage, warm.coverage,
        "baseline vs warm coverage differs"
    );
}

/// R5, reached through the real pipeline (not `CacheStore` directly, which is
/// `src/extract/cache.rs`'s own unit-level proof): a SINGLE corrupted
/// per-file record degrades to a miss for that one file, while an unchanged
/// fingerprint means the rest of the cache is left exactly as it was — this
/// is what distinguishes a corrupted-record miss (scoped to one file) from an
/// R2 fingerprint mismatch (the whole store, always).
#[test]
fn a_corrupted_single_record_degrades_to_a_miss_for_that_file_and_the_run_still_succeeds() {
    let repo = tempfile::tempdir().unwrap();
    common::init_repo_with_files(
        repo.path(),
        "Alice",
        &[
            ("src/lib.rs", "pub fn f() -> i32 { 1 }\n"),
            ("src/other.rs", "pub fn g() -> i32 { 2 }\n"),
        ],
    );
    let out = tempfile::tempdir().unwrap();
    let config = common::config_for(repo.path(), out.path());
    generate(&config).expect("run 1 populates the cache");

    let cache_root = out.path().join(CACHE_DIR_NAME);
    let lib_blob = common::blob_id_for(repo.path(), "src/lib.rs");
    let other_blob = common::blob_id_for(repo.path(), "src/other.rs");

    // A record untouched by the corruption, kept as a control: if the whole
    // store had been wiped (R2's behaviour, wrongly triggered here) rather
    // than just this one record missing, this file's bytes would change too.
    let control_path = record_path(&cache_root, "external", other_blob);
    let control_before = std::fs::read(&control_path).expect("a real control record exists");

    // Corrupt exactly one per-file record, in one phase, leaving the header
    // and every other record untouched.
    let corrupted_path = record_path(&cache_root, "languages-decl", lib_blob);
    assert!(
        corrupted_path.is_file(),
        "precondition: a real record exists to corrupt"
    );
    std::fs::write(&corrupted_path, b"{ not valid json, truncated").unwrap();

    let run2 = generate(&config).expect(
        "a corrupted single cache record must never fail the run — only \
         degrade to a miss for that one file",
    );

    let baseline_out = tempfile::tempdir().unwrap();
    block_cache(baseline_out.path());
    let baseline =
        generate(&common::config_for(repo.path(), baseline_out.path())).expect("baseline run");

    assert_eq!(
        baseline.corpus, run2.corpus,
        "the corpus must still be complete and correct despite the corrupted \
         record"
    );
    assert_eq!(baseline.manifest, run2.manifest);
    assert_eq!(baseline.cards, run2.cards);
    assert_eq!(baseline.coverage, run2.coverage);

    // The corrupted record was reparsed and rewritten with valid content — a
    // miss reparses the file rather than leaving a permanently broken record.
    let after = std::fs::read(&corrupted_path).unwrap();
    let parsed: Result<serde_json::Value, _> = serde_json::from_slice(&after);
    assert!(
        parsed.is_ok(),
        "the corrupted record must be replaced with valid JSON after a reparse"
    );

    // The control record — for a DIFFERENT file, in a DIFFERENT phase — is
    // byte-for-byte untouched: the corruption degraded only its own record,
    // never the whole store.
    let control_after = std::fs::read(&control_path).unwrap();
    assert_eq!(
        control_before, control_after,
        "an unrelated file's record must be untouched by a single corrupted \
         record elsewhere in the cache"
    );
}

/// R6: the on-disk cache directory holds only the three `walk_files`-based
/// phases plus the fingerprint header — never a trace of `metrics`,
/// `manifest_deps`, `scip`, `filetree`, or `test_pairing`, none of which ever
/// call `walk_files` and so can never produce a cache entry of their own.
/// Run twice (cold, then warm) so a warm run reusing the cache is exercised
/// too, not just a first write.
#[test]
fn the_cache_directory_holds_only_the_three_walk_files_phases_and_the_header() {
    let repo = tempfile::tempdir().unwrap();
    common::init_repo_with_files(repo.path(), "Alice", &fixture_files());
    let out = tempfile::tempdir().unwrap();
    let config = common::config_for(repo.path(), out.path());

    generate(&config).expect("cold run");
    generate(&config).expect("warm run");

    let cache_root = out.path().join(CACHE_DIR_NAME);
    let mut children: Vec<String> = std::fs::read_dir(&cache_root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    children.sort();

    assert_eq!(
        children,
        vec![
            "external".to_string(),
            "fingerprint.json".to_string(),
            "languages-decl".to_string(),
            "languages-usages".to_string(),
        ],
        "the cache root must hold exactly the three walk_files-based phases \
         plus the fingerprint header — the five extractors that never call \
         walk_files must never appear here"
    );
}

/// U7/KTD9: the exact contamination `ce-code-review` found after U1-U6
/// shipped — two runs sharing one `corpus_dir` with DIFFERENT
/// `ExtractOptions` (here, `secret_prefixes`) could previously let a record
/// left behind by one configuration be read as a valid hit under the other's
/// header, because per-file records were keyed by `(phase_id, blob_id, key)`
/// alone, never by the fingerprint that produced them.
///
/// `src/lib.rs`'s doc comment carries a token shape (`SEKRET_...`) that ONLY
/// config A's `secret_prefixes` recognizes — so config A's record for it is
/// REDACTED, while config B's own parse of the same file is not. This makes
/// the two configurations' facts for the same file genuinely differ, so a
/// wrongly-served stray record is something this test can actually detect,
/// not just something that happens not to matter for this fixture.
///
/// The race itself is reproduced structurally rather than with real
/// concurrent processes: config A populates the cache, config B's own clean
/// run establishes ground truth, then config A's stray record for the same
/// file is dropped back into the (now config-B-headed) cache directory — the
/// state a straggling write from an interleaved run A could leave behind.
/// A third run under config B, over the same unchanged commit, must reject
/// that stray record (wrong fingerprint digest) and reparse, matching config
/// B's own clean run rather than silently reusing config A's redacted facts.
#[test]
fn a_stray_record_from_a_different_run_configuration_is_never_served_as_a_hit() {
    let repo = tempfile::tempdir().unwrap();
    common::init_repo_with_files(
        repo.path(),
        "Alice",
        &[(
            "src/lib.rs",
            "/// token: SEKRET_1234567890abcdef\npub fn f() -> i32 { 1 }\n",
        )],
    );

    let out = tempfile::tempdir().unwrap();
    let mut config_a = common::config_for(repo.path(), out.path());
    config_a.secret_prefixes = vec!["SEKRET_".to_string()];
    generate(&config_a).expect("run A populates the cache under its own configuration");

    let cache_root = out.path().join(CACHE_DIR_NAME);
    let lib_blob = common::blob_id_for(repo.path(), "src/lib.rs");
    let stray_bytes = std::fs::read(record_path(&cache_root, "languages-decl", lib_blob)).unwrap();

    let mut config_b = common::config_for(repo.path(), out.path());
    config_b.secret_prefixes = vec!["DIFFERENT_".to_string()];
    // Config B's own first run over this corpus_dir: the differing
    // configuration changes the fingerprint, so `activate` clears run A's
    // records and writes a fresh header before extracting — the ordinary,
    // correct path (R2). This is the ground truth the contaminated run below
    // must match.
    let run_b_clean = generate(&config_b).expect("run B's own clean first run");

    // Simulate the race: after run B's own correct header and records are in
    // place, drop run A's stray record back into run B's phase directory, as
    // if run A's writer straggled and landed there after run B's clear.
    std::fs::write(
        record_path(&cache_root, "languages-decl", lib_blob),
        &stray_bytes,
    )
    .unwrap();

    // A third run, still config B, same unchanged commit: the header still
    // matches config B's fingerprint, so `activate` takes the no-op branch
    // and `walk_files` reads straight from the now-contaminated store.
    let run_b_after_contamination = generate(&config_b).expect("run B, contaminated cache");

    assert_eq!(
        run_b_clean.corpus, run_b_after_contamination.corpus,
        "a stray record from a different run's configuration must never be \
         served as a hit — the contaminated run must reparse and match a \
         clean run of the same configuration"
    );
    assert_eq!(run_b_clean.manifest, run_b_after_contamination.manifest);
    assert_eq!(run_b_clean.cards, run_b_after_contamination.cards);
    assert_eq!(run_b_clean.coverage, run_b_after_contamination.coverage);

    let repaired = std::fs::read(record_path(&cache_root, "languages-decl", lib_blob)).unwrap();
    assert_ne!(
        repaired, stray_bytes,
        "the stray record must have been overwritten by a real reparse under \
         config B's own fingerprint, not preserved because (phase, blob_id, \
         key) still matched"
    );
}
