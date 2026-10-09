//! `measure` end to end (plan A, U11, R10, R11, KTD10, KTD11): what the report
//! counts for each rule against an index, when it proposes a right to delete,
//! which indexes it refuses, and the consistency check that keeps the rights
//! table honest (AE5).
//!
//! The fixture is one call whose receiver has a declared repository type, so
//! `declared-receiver` backs one candidate and excludes the other; swapping
//! where the index says the call went gives each outcome the report counts.

mod common;

use common::{
    at, at_end, index_record, init_repo_with_files, occ, scip_document as document,
    scip_occurrence_at, scip_sym as sym, write_index_record, write_scip_index,
};
use reverse_architect::config::DEFAULT_AMBIGUITY_CEILING;
use reverse_architect::extract::resolve::rights::current_revision;
use reverse_architect::extract::{DeleteRight, RightsTable};
use reverse_architect::graph::Rule;
use reverse_architect::index::ProjectOutcome;
use reverse_architect::measure::{
    self, MeasureError, MeasureOptions, OriginReport, Proposal, Report, RuleReport, Thresholds,
};
use reverse_architect::progress::Silent;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// Two types with a `Trim` each.
const TEXT_CS: &str = r#"namespace N
{
    public class Text
    {
        public string Trim() { return null; }
    }

    public class Other
    {
        public string Trim() { return null; }
    }
}
"#;

/// The one call: its receiver is declared `Text`, so `declared-receiver`
/// backs `Text.Trim` and excludes `Other.Trim`.
const CALLER_CS: &str = r#"namespace N
{
    public class Caller
    {
        public void Go(Text t)
        {
            t.Trim();
        }
    }
}
"#;

/// The candidate `declared-receiver` backs.
fn text_trim() -> String {
    sym("N/Text#Trim().")
}

/// The candidate `declared-receiver` excludes.
fn other_trim() -> String {
    sym("N/Other#Trim().")
}

/// What a compiler binds `t.Trim()` to when `t` is a `string`.
const SYSTEM_TRIM: &str = "scip-dotnet nuget System.Runtime 10.0.0.0 System/String#Trim().";

/// Every name of the fixture's code — none may reach a report.
const FIXTURE_NAMES: &[&str] = &[
    "Text", "Trim", "Other", "Caller", "\"Go", "Go(", "src/", ".cs", "N#", "N/",
];

struct Fixture {
    repo: TempDir,
    out: TempDir,
    index: PathBuf,
}

/// The fixture repository, an index whose call occurrence names
/// `call_target`, and a clean record beside it. With `drifted`, every
/// definition in `src/Text.cs` sits forty lines from where the grammar finds
/// it, so the drift check refuses that document and the call's target has no
/// verified definition.
fn fixture(call_target: &str, drifted: bool) -> Fixture {
    let repo = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    init_repo_with_files(
        repo.path(),
        "Alice",
        &[("src/Text.cs", TEXT_CS), ("src/Caller.cs", CALLER_CS)],
    );
    let shift = if drifted { 40 } else { 0 };
    let definition = |symbol: &str, needle: &str, nth: usize| {
        let (line, column) = at_end(TEXT_CS, needle, nth);
        scip_occurrence_at(&sym(symbol), (line + shift, column), true)
    };
    let index = write_scip_index(
        out.path(),
        vec![
            document(
                "src/Caller.cs",
                vec![
                    occ(CALLER_CS, "N/Caller#", "Caller", 0, true),
                    occ(CALLER_CS, "N/Caller#Go().", "Go(", 0, true),
                    scip_occurrence_at(call_target, at(CALLER_CS, "Trim(", 0), false),
                ],
            ),
            document(
                "src/Text.cs",
                vec![
                    definition("N/Text#", "Text", 0),
                    definition("N/Text#Trim().", "Trim(", 0),
                    definition("N/Other#", "Other", 0),
                    definition("N/Other#Trim().", "Trim(", 1),
                ],
            ),
        ],
    );
    write_index_record(&index, &index_record(&index, repo.path()));
    Fixture { repo, out, index }
}

/// Thresholds a one-call fixture can meet. Explicit, so no test leans on the
/// defaults U14 will retune.
const LENIENT: Thresholds = Thresholds {
    min_evaluated_sites: 1,
    max_unmapped_percent: 50,
};

fn options(f: &Fixture, thresholds: Thresholds) -> MeasureOptions {
    MeasureOptions {
        repo: f.repo.path().to_path_buf(),
        scip_index: f.index.clone(),
        corpus_dir: Some(f.out.path().join("corpus")),
        plugins: Vec::new(),
        ambiguity_ceiling: DEFAULT_AMBIGUITY_CEILING,
        secret_prefixes: Vec::new(),
        thresholds,
    }
}

fn run(f: &Fixture, thresholds: Thresholds) -> Report {
    measure::run(&options(f, thresholds), &Silent).expect("the measurement runs")
}

fn refusal(f: &Fixture) -> MeasureError {
    measure::run(&options(f, LENIENT), &Silent).expect_err("the measurement is refused")
}

fn csharp(report: &Report) -> &OriginReport {
    let found: Vec<&OriginReport> = report
        .origins
        .iter()
        .filter(|o| o.language == "csharp")
        .collect();
    assert_eq!(found.len(), 1, "one C# origin: {:?}", report.origins);
    found[0]
}

fn rule<'r>(report: &'r Report, id: &str) -> &'r RuleReport {
    csharp(report)
        .rules
        .get(id)
        .unwrap_or_else(|| panic!("no `{id}` in {:?}", csharp(report).rules.keys()))
}

/// The partition every rule entry must keep (see `measure::RuleReport`).
fn assert_partitions(report: &Report) {
    for (id, r) in &csharp(report).rules {
        assert_eq!(r.judged, r.evaluated + r.unmapped, "{id}");
        assert_eq!(
            r.evaluated,
            r.would_remove_confirmed + r.confirmed + r.refuted + r.disputed_refuted,
            "{id}"
        );
    }
}

#[test]
fn a_rule_backing_the_candidate_the_index_confirmed_counts_confirmed_and_is_proposed() {
    let f = fixture(&text_trim(), false);
    let report = run(&f, LENIENT);

    let declared = rule(&report, "declared-receiver");
    assert_eq!(
        (
            declared.judged,
            declared.confirmed,
            declared.would_remove_confirmed,
            declared.refuted,
            declared.unmapped
        ),
        (1, 1, 0, 0, 0),
        "{declared:?}"
    );
    assert_eq!(declared.proposal, Proposal::Proposed);
    assert_eq!(
        Some(declared.revision),
        current_revision(Rule::DeclaredReceiver)
    );
    assert_eq!(csharp(&report).sites.resolved, 1, "the call paired");
    assert_eq!(report.commit.len(), 40, "the full commit id");
    assert_partitions(&report);
}

#[test]
fn a_rule_that_excluded_the_candidate_the_index_confirmed_would_remove_it_and_is_not_proposed() {
    let f = fixture(&other_trim(), false);
    let report = run(&f, LENIENT);

    let declared = rule(&report, "declared-receiver");
    assert_eq!(declared.would_remove_confirmed, 1, "{declared:?}");
    assert_eq!(declared.confirmed, 0);
    assert_eq!(declared.proposal, Proposal::RemovedConfirmed);
    assert_partitions(&report);
}

/// A site over the ambiguity ceiling settles into one fact before any rule
/// runs — in generation. A right to delete would make the rules run there,
/// so measurement must have run them too: here `declared-receiver` excludes
/// the very candidate the index confirms, at a site with two candidates and a
/// ceiling of one.
#[test]
fn a_rule_is_measured_at_a_site_over_the_ceiling_too() {
    let f = fixture(&other_trim(), false);
    let report = measure::run(
        &MeasureOptions {
            ambiguity_ceiling: 1,
            ..options(&f, LENIENT)
        },
        &Silent,
    )
    .expect("the measurement runs");

    let declared = rule(&report, "declared-receiver");
    assert_eq!(
        (declared.judged, declared.would_remove_confirmed),
        (1, 1),
        "{declared:?}"
    );
    assert_eq!(declared.proposal, Proposal::RemovedConfirmed);
    assert_eq!(csharp(&report).sites.resolved, 1, "the call paired");
    assert_partitions(&report);
}

/// Generation's cached usage records hold no rule verdicts for a site over
/// the ceiling, so measurement neither reads them nor writes its own, which
/// hold verdicts generation never computes. The declarations are shared.
#[test]
fn a_measurement_shares_the_declaration_cache_but_not_the_usage_cache() {
    let f = fixture(&text_trim(), false);
    run(&f, LENIENT);
    let cache = f.out.path().join("corpus").join(".ra-cache");
    let records = |phase: &str| {
        std::fs::read_dir(cache.join(phase))
            .map(|entries| entries.count())
            .unwrap_or(0)
    };
    assert!(records("languages-decl") > 0, "declarations cached");
    assert_eq!(records("languages-usages"), 0, "no usage record written");
}

#[test]
fn an_index_pointing_outside_the_repository_refutes_the_backed_candidate() {
    let f = fixture(SYSTEM_TRIM, false);
    let report = run(&f, LENIENT);

    let declared = rule(&report, "declared-receiver");
    assert_eq!(
        (
            declared.refuted,
            declared.confirmed,
            declared.would_remove_confirmed
        ),
        (1, 0, 0),
        "{declared:?}"
    );
    assert_eq!(csharp(&report).sites.external, 1);
    assert_partitions(&report);
}

#[test]
fn an_occurrence_into_a_drifted_document_is_unmapped_not_refuted() {
    let f = fixture(&text_trim(), true);
    let report = run(&f, LENIENT);

    let declared = rule(&report, "declared-receiver");
    assert_eq!(
        (
            declared.judged,
            declared.unmapped,
            declared.evaluated,
            declared.refuted,
            declared.would_remove_confirmed
        ),
        (1, 1, 0, 0, 0),
        "{declared:?}"
    );
    assert_eq!(declared.proposal, Proposal::TooFewEvaluated);
    assert_partitions(&report);
}

/// `static-receiver` never speaks in the fixture: no evaluated site, nothing
/// removed — and still no proposal, even with the minimum at zero.
#[test]
fn a_rule_with_no_evaluated_sites_is_not_proposed_even_with_no_minimum() {
    let f = fixture(&text_trim(), false);
    let report = run(
        &f,
        Thresholds {
            min_evaluated_sites: 0,
            max_unmapped_percent: 100,
        },
    );

    let r = rule(&report, "static-receiver");
    assert_eq!((r.judged, r.would_remove_confirmed), (0, 0), "{r:?}");
    assert_eq!(r.proposal, Proposal::TooFewEvaluated);
    assert_eq!(
        report.thresholds.min_evaluated_sites, 0,
        "the report states them"
    );
}

/// A zero unmapped share passes even the strictest threshold: 0 means "no
/// unmapped site", not "never propose".
#[test]
fn a_rule_with_no_unmapped_site_passes_a_zero_unmapped_threshold() {
    let f = fixture(&text_trim(), false);
    let report = run(
        &f,
        Thresholds {
            min_evaluated_sites: 1,
            max_unmapped_percent: 0,
        },
    );
    assert_eq!(
        rule(&report, "declared-receiver").proposal,
        Proposal::Proposed
    );
}

/// The catalog is measured whole; the rules KTD6 marks "never" are never
/// proposed, and the index's own verdicts are no rules to measure.
#[test]
fn the_report_lists_every_catalog_rule_and_never_proposes_the_never_rules() {
    let f = fixture(&text_trim(), false);
    let report = run(&f, LENIENT);
    let ids: Vec<&str> = csharp(&report).rules.keys().map(String::as_str).collect();
    assert_eq!(
        ids,
        [
            "arity",
            "base-spelling",
            "declared-receiver",
            "implicit-this",
            "namespace-visibility",
            "nested-type-visibility",
            "role-mismatch",
            "static-receiver",
        ]
    );
    for id in [
        "arity",
        "namespace-visibility",
        "implicit-this",
        "base-spelling",
    ] {
        assert_eq!(rule(&report, id).proposal, Proposal::NeverDeletes, "{id}");
        assert!(!rule(&report, id).may_delete, "{id}");
    }
    // C#'s own name lookup: a right is earned by measurement, not ruled out.
    assert!(rule(&report, "nested-type-visibility").may_delete);
    let json = measure::to_json(&report);
    for absent in ["index-elsewhere", "index-outside", "name-match"] {
        assert!(!json.contains(absent), "{absent} in {json}");
    }
}

#[test]
fn two_runs_give_identical_report_bytes() {
    let f = fixture(&other_trim(), false);
    let cold = measure::to_json(&run(&f, LENIENT));
    let warm = measure::to_json(&run(&f, LENIENT));
    assert_eq!(cold, warm, "a warm cache changes nothing");

    let mut fresh = options(&f, LENIENT);
    fresh.corpus_dir = Some(f.out.path().join("another-corpus"));
    let elsewhere = measure::to_json(&measure::run(&fresh, &Silent).unwrap());
    assert_eq!(cold, elsewhere);
}

#[test]
fn the_report_names_nothing_of_the_measured_code_and_sorts_its_keys() {
    let f = fixture(&text_trim(), false);
    let json = measure::to_json(&run(&f, LENIENT));
    for name in FIXTURE_NAMES {
        assert!(!json.contains(name), "`{name}` leaked into {json}");
    }
    let repo_path = f.repo.path().to_string_lossy().replace('\\', "/");
    assert!(!json.replace("\\\\", "/").contains(&repo_path));

    let top: Vec<usize> = [
        "\"commit\"",
        "\"index_sha256\"",
        "\"origins\"",
        "\"resolver\"",
        "\"schema\"",
        "\"thresholds\"",
    ]
    .iter()
    .map(|k| json.find(k).unwrap_or_else(|| panic!("{k} in {json}")))
    .collect();
    assert!(top.windows(2).all(|w| w[0] < w[1]), "{json}");
    assert!(json.ends_with("}\n"));
}

// ---- Refusals ----

#[test]
fn an_index_without_a_record_is_refused() {
    let f = fixture(&text_trim(), false);
    std::fs::remove_file(measure::record_path(&f.index)).unwrap();
    let err = refusal(&f);
    assert!(matches!(err, MeasureError::MissingRecord { .. }), "{err}");
    assert!(err.to_string().contains("reverse-architect index"), "{err}");
}

#[test]
fn an_index_from_another_commit_is_refused() {
    let f = fixture(&text_trim(), false);
    let mut record = index_record(&f.index, f.repo.path());
    record.commit = Some("0".repeat(40));
    write_index_record(&f.index, &record);
    let err = refusal(&f);
    assert!(matches!(err, MeasureError::OtherCommit { .. }), "{err}");
    assert!(err.to_string().contains(&"0".repeat(40)), "{err}");
}

#[test]
fn an_index_from_a_dirty_working_tree_is_refused() {
    let f = fixture(&text_trim(), false);
    let mut record = index_record(&f.index, f.repo.path());
    record.dirty = Some(true);
    write_index_record(&f.index, &record);
    let err = refusal(&f);
    assert!(matches!(err, MeasureError::DirtyIndex), "{err}");
    assert!(err.to_string().contains("uncommitted"), "{err}");
}

#[test]
fn a_partial_index_is_refused_and_names_the_failed_project() {
    let f = fixture(&text_trim(), false);
    let mut record = index_record(&f.index, f.repo.path());
    record.partial = true;
    record.projects[0].outcome = ProjectOutcome::Failed;
    record.projects[0].detail = Some("exit code 1".to_string());
    write_index_record(&f.index, &record);
    let err = refusal(&f);
    assert!(matches!(err, MeasureError::PartialIndex { .. }), "{err}");
    assert!(err.to_string().contains("src/App.csproj"), "{err}");
}

#[test]
fn an_index_replaced_under_its_old_record_is_refused_by_its_sha256() {
    let f = fixture(&text_trim(), false);
    // Another index in the same place; the record still describes the first.
    let other = tempfile::tempdir().unwrap();
    let replacement = write_scip_index(other.path(), Vec::new());
    std::fs::copy(&replacement, &f.index).unwrap();
    let err = refusal(&f);
    assert!(matches!(err, MeasureError::IndexChanged { .. }), "{err}");
    assert!(err.to_string().contains("SHA-256"), "{err}");
}

#[test]
fn a_record_of_another_format_version_is_refused() {
    let f = fixture(&text_trim(), false);
    let mut record = index_record(&f.index, f.repo.path());
    record.schema += 1;
    write_index_record(&f.index, &record);
    assert!(matches!(refusal(&f), MeasureError::RecordSchema { .. }));
}

// ---- The rights table against the committed reports (KTD11, AE5) ----

/// The right a report's C# origin would back for `declared-receiver`, naming
/// a report with these bytes.
fn right_from(report: &Report, bytes: &[u8]) -> DeleteRight {
    let origin = csharp(report);
    DeleteRight {
        rule: Rule::DeclaredReceiver,
        revision: rule(report, "declared-receiver").revision,
        language: origin.language.clone(),
        component: origin.component.clone(),
        component_sha256: origin.component_sha256.clone(),
        contract: origin.contract.clone(),
        report: "docs/measurements/fixture.json".to_string(),
        report_sha256: format!("{:x}", Sha256::digest(bytes)),
    }
}

#[test]
fn a_right_backed_by_a_clean_proposing_report_is_consistent() {
    let f = fixture(&text_trim(), false);
    let report = run(&f, LENIENT);
    let bytes = measure::to_json(&report).into_bytes();
    assert_eq!(
        measure::verify_right(&right_from(&report, &bytes), &bytes),
        Ok(())
    );
}

/// AE5: a record whose report shows the rule removing a confirmed edge fails
/// the check — the table can never hold a right the measurement refuted.
#[test]
fn a_right_whose_report_shows_a_removed_confirmed_edge_fails_the_consistency_check() {
    let f = fixture(&other_trim(), false);
    let report = run(&f, LENIENT);
    let bytes = measure::to_json(&report).into_bytes();
    let right = right_from(&report, &bytes);

    let problem = measure::verify_right(&right, &bytes).unwrap_err();
    assert!(problem.contains("confirmed"), "{problem}");

    // The same through the table check over a measurements directory.
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("docs/measurements")).unwrap();
    std::fs::write(root.path().join(&right.report), &bytes).unwrap();
    let problems = measure::check_rights(&RightsTable::new(vec![right]), root.path());
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(
        problems[0].starts_with("declared-receiver:"),
        "{problems:?}"
    );
}

#[test]
fn a_right_whose_report_changed_or_is_missing_fails_the_consistency_check() {
    let f = fixture(&text_trim(), false);
    let report = run(&f, LENIENT);
    let bytes = measure::to_json(&report).into_bytes();
    let right = right_from(&report, &bytes);

    let mut edited = bytes.clone();
    edited.extend_from_slice(b" ");
    let problem = measure::verify_right(&right, &edited).unwrap_err();
    assert!(problem.contains("hashes to"), "{problem}");

    let root = tempfile::tempdir().unwrap();
    let missing = measure::check_rights(&RightsTable::new(vec![right.clone()]), root.path());
    assert_eq!(missing.len(), 1, "{missing:?}");

    let outside = DeleteRight {
        report: "elsewhere/fixture.json".to_string(),
        ..right
    };
    let problems = measure::check_rights(&RightsTable::new(vec![outside]), root.path());
    assert!(problems[0].contains("docs/measurements/"), "{problems:?}");
}

/// Every other reason a right and its report disagree, one at a time: each
/// case changes one thing in a clean, proposing report (or in the right) and
/// names the words the refusal must contain. The report's bytes are those of
/// the changed report, and the right names their hash, so it is the change
/// and not the hash that refuses.
#[test]
fn a_right_disagreeing_with_its_report_in_any_one_respect_fails_the_check() {
    let f = fixture(&text_trim(), false);
    let clean = run(&f, LENIENT);
    let too_few = run(
        &f,
        Thresholds {
            min_evaluated_sites: 99,
            ..LENIENT
        },
    );
    assert_eq!(
        (
            rule(&too_few, "declared-receiver").proposal,
            rule(&too_few, "declared-receiver").would_remove_confirmed
        ),
        (Proposal::TooFewEvaluated, 0),
        "precondition: nothing removed, and still not proposed"
    );

    type Change = fn(&mut Report, &mut DeleteRight);
    let cases: [(&str, &Report, Change, &str); 7] = [
        (
            "a later revision",
            &clean,
            |_, right| right.revision += 1,
            "revision",
        ),
        (
            "a report that proposes nothing",
            &too_few,
            |_, _| {},
            "does not propose",
        ),
        (
            "another build of the component",
            &clean,
            |_, right| right.component_sha256 = "0".repeat(64),
            "measured no sites",
        ),
        (
            "a report of another format version",
            &clean,
            |report, _| report.schema += 1,
            "format version",
        ),
        (
            "a report with no origin at all",
            &clean,
            |report, _| report.origins.clear(),
            "measured no sites",
        ),
        (
            "another language",
            &clean,
            |_, right| right.language = "java".to_string(),
            "measured no sites",
        ),
        (
            "a report missing the rule",
            &clean,
            |report, _| {
                report.origins[0].rules.remove("declared-receiver");
            },
            "does not measure this rule",
        ),
    ];
    for (case, base, change, expected) in cases {
        let mut report = base.clone();
        let mut right = right_from(base, b"");
        change(&mut report, &mut right);
        let bytes = measure::to_json(&report).into_bytes();
        right.report_sha256 = format!("{:x}", Sha256::digest(&bytes));
        let problem = measure::verify_right(&right, &bytes)
            .expect_err(&format!("{case}: the check must refuse"));
        assert!(problem.contains(expected), "{case}: {problem}");
    }
}

/// A report path that climbs out of `docs/measurements/` is refused before
/// anything is read, though it starts with that directory.
#[test]
fn a_right_naming_a_report_outside_the_measurements_directory_fails_the_check() {
    let f = fixture(&text_trim(), false);
    let report = run(&f, LENIENT);
    let bytes = measure::to_json(&report).into_bytes();
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("x.json"), &bytes).unwrap();
    let climbing = DeleteRight {
        report: "docs/measurements/../x.json".to_string(),
        ..right_from(&report, &bytes)
    };

    let problems = measure::check_rights(&RightsTable::new(vec![climbing]), root.path());
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(
        problems[0].contains("not a file under docs/measurements/"),
        "{problems:?}"
    );
}

/// The check over the table this build ships and this repository's
/// `docs/measurements/`: every granted right rests on a committed report that
/// shows the rule removed nothing the index confirmed (KTD11, AE5).
#[test]
fn the_shipped_rights_table_is_consistent_with_the_committed_reports() {
    let problems = measure::check_rights(
        &RightsTable::granted(),
        Path::new(env!("CARGO_MANIFEST_DIR")),
    );
    assert!(problems.is_empty(), "{problems:#?}");
}

// ---- The binary ----

#[test]
fn the_binary_writes_the_report_the_library_measures() {
    let f = fixture(&text_trim(), false);
    let out = f.out.path().join("report.json");
    assert_cmd::Command::cargo_bin("reverse-architect")
        .unwrap()
        .arg("measure")
        .arg(f.repo.path())
        .arg("--scip-index")
        .arg(&f.index)
        .arg("--out")
        .arg(&out)
        .arg("--corpus")
        .arg(f.out.path().join("corpus"))
        .arg("--min-evaluated-sites")
        .arg("1")
        .arg("--max-unmapped-percent")
        .arg("50")
        .assert()
        .success()
        .stderr(predicates::str::contains("declared-receiver"));
    assert_eq!(
        std::fs::read_to_string(&out).unwrap(),
        measure::to_json(&run(&f, LENIENT))
    );
}

#[test]
fn the_binary_refuses_an_index_without_a_record_with_a_non_zero_exit() {
    let f = fixture(&text_trim(), false);
    std::fs::remove_file(measure::record_path(&f.index)).unwrap();
    assert_cmd::Command::cargo_bin("reverse-architect")
        .unwrap()
        .arg("measure")
        .arg(f.repo.path())
        .arg("--scip-index")
        .arg(&f.index)
        .arg("--corpus")
        .arg(f.out.path().join("corpus"))
        .assert()
        .failure()
        .stderr(predicates::str::contains("no record beside the index"));
}
