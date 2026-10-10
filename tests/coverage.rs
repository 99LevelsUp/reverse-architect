//! U7 integration tests: the coverage artifact.
//!
//! Every assertion here is about what the corpus is ALLOWED to claim. The
//! artifact's job is to make an absence claim checkable (R20), so the tests care
//! most about the honest negatives: a file nothing analyzed, a module only
//! partly covered, an index whose declarations moved.

mod common;

use common::{init_repo_with_files, scip_document, scip_occurrence, scip_sym, write_scip_index};
use reverse_architect::coverage::{
    build_coverage, not_structural_because, to_json, Coverage, LanguageStatus, LEXED_ONLY,
    NO_DECLARATION, NO_GRAMMAR, SCHEMA_VERSION,
};
use reverse_architect::extract::languages::{LanguageEntry, Languages};
use reverse_architect::extract::{run_all_with, ExtractOptions, Repo};
use reverse_architect::graph::{EdgeKind, FactKind, FactValue, Graph, NodeKind};
use std::path::{Path, PathBuf};

const INVOICE_CS: &str = r#"namespace Orbit.Billing
{
    public class Invoice
    {
        public int Total(int quantity)
        {
            return quantity;
        }
    }
}
"#;

const HELPER_RS: &str = r#"pub struct Helper {
    pub value: i64,
}
"#;

fn coverage_over(repo_dir: &Path, scip_index: Option<PathBuf>) -> Coverage {
    let repo = Repo::open(repo_dir).unwrap();
    let mut graph = Graph::new();
    run_all_with(
        &repo,
        &mut graph,
        &ExtractOptions {
            scip_index,
            ..ExtractOptions::default()
        },
        &reverse_architect::progress::Silent,
    )
    .unwrap();
    reverse_architect::graph::merge(&mut graph, &reverse_architect::extract::trust_ranks());
    build_coverage(
        &graph,
        repo.head_revision(),
        &reverse_architect::extract::languages::Languages::bundled(),
        None,
    )
}

fn file<'a>(coverage: &'a Coverage, path: &str) -> &'a reverse_architect::coverage::FileCoverage {
    coverage
        .files
        .iter()
        .find(|f| f.path == path)
        .unwrap_or_else(|| {
            let seen: Vec<&str> = coverage.files.iter().map(|f| f.path.as_str()).collect();
            panic!("{path} missing from coverage; saw {seen:?}")
        })
}

fn module<'a>(
    coverage: &'a Coverage,
    path: &str,
) -> &'a reverse_architect::coverage::ModuleCoverage {
    coverage
        .modules
        .iter()
        .find(|m| m.path == path)
        .unwrap_or_else(|| panic!("{path} missing from module coverage"))
}

/// Build an index that agrees with the grammar about every file it names.
fn agreeing_index(index_dir: &Path, graph_source: &Path, files: &[&str]) -> PathBuf {
    let repo = Repo::open(graph_source).unwrap();
    let mut graph = Graph::new();
    run_all_with(
        &repo,
        &mut graph,
        &ExtractOptions::default(),
        &reverse_architect::progress::Silent,
    )
    .unwrap();

    let documents = files
        .iter()
        .map(|file| {
            // Every symbol the grammar found in this file, placed exactly where
            // the grammar found it.
            let occurrences = graph
                .nodes_sorted()
                .into_iter()
                .filter(|n| n.kind == NodeKind::Symbol)
                .filter_map(|n| {
                    let fact = n.facts.iter().find(|f| f.kind == FactKind::SymbolName)?;
                    let FactValue::Text(name) = &fact.value else {
                        return None;
                    };
                    if fact.provenance.file != *file {
                        return None;
                    }
                    let line = fact.provenance.span.start()?;
                    Some(scip_occurrence(
                        &scip_sym(&format!("Fixture/{name}#")),
                        line,
                        true,
                    ))
                })
                .collect();
            scip_document(file, occurrences)
        })
        .collect();

    write_scip_index(index_dir, documents)
}

#[test]
fn two_languages_each_get_an_aggregate_whose_counts_match_the_file_list() {
    let repo = tempfile::tempdir().unwrap();
    init_repo_with_files(
        repo.path(),
        "Alice",
        &[
            ("src/Invoice.cs", INVOICE_CS),
            ("src/helper.rs", HELPER_RS),
            ("README.md", "# readme"),
        ],
    );

    let coverage = coverage_over(repo.path(), None);

    let languages: Vec<&str> = coverage
        .languages
        .iter()
        .map(|l| l.language.as_str())
        .collect();
    assert_eq!(languages, vec!["csharp", "rust"]);

    // Every aggregate must be the size of a subset of `files` — an aggregate that
    // cannot be recomputed from the list is a number nobody can check.
    for language in &coverage.languages {
        let of_language: Vec<_> = coverage
            .files
            .iter()
            .filter(|f| f.language.as_deref() == Some(language.language.as_str()))
            .collect();
        assert_eq!(language.files, of_language.len(), "{}", language.language);
        assert_eq!(
            language.structural,
            of_language.iter().filter(|f| f.structural).count(),
            "{}",
            language.language
        );
        assert_eq!(
            language.resolved,
            of_language
                .iter()
                .filter(|f| f.resolution == "resolved")
                .count(),
            "{}",
            language.language
        );
    }
}

#[test]
fn a_file_no_grammar_covers_is_listed_with_a_reason_not_silently_dropped() {
    let repo = tempfile::tempdir().unwrap();
    init_repo_with_files(
        repo.path(),
        "Alice",
        &[
            ("src/Invoice.cs", INVOICE_CS),
            ("README.md", "# readme"),
            ("appsettings.json", "{}"),
        ],
    );

    let coverage = coverage_over(repo.path(), None);

    for path in ["README.md", "appsettings.json"] {
        let entry = file(&coverage, path);
        assert!(!entry.structural, "{path} declares nothing");
        assert_eq!(entry.language, None);
        assert_eq!(
            entry.not_structural_because.as_deref(),
            Some("no grammar covers this file type"),
            "{path} must say WHY it carries no structure"
        );
        // It was still touched — omitting it would read as "not in the repo".
        assert!(
            entry.extractors.contains(&"filetree".to_string()),
            "{path}: {:?}",
            entry.extractors
        );
    }

    let analyzed = file(&coverage, "src/Invoice.cs");
    assert!(analyzed.structural);
    assert_eq!(analyzed.language.as_deref(), Some("csharp"));
    assert!(analyzed.not_structural_because.is_none());
    // The facts name the plugin that wrote them, not the host orchestrator.
    assert!(analyzed.extractors.contains(&"csharp".to_string()));
}

#[test]
fn a_supported_file_that_declares_nothing_is_told_apart_from_an_unsupported_one() {
    let repo = tempfile::tempdir().unwrap();
    init_repo_with_files(
        repo.path(),
        "Alice",
        &[
            ("src/Invoice.cs", INVOICE_CS),
            // A grammar covers `.cs`, but there is nothing to declare here.
            ("src/empty.cs", "// nothing but a comment\n"),
        ],
    );

    let coverage = coverage_over(repo.path(), None);
    let empty = file(&coverage, "src/empty.cs");

    assert!(!empty.structural);
    assert_eq!(empty.language.as_deref(), Some("csharp"));
    assert_eq!(
        empty.not_structural_because.as_deref(),
        Some("a grammar covers this file type but it declares no symbol"),
        "a covered-but-empty file is a different state from an uncovered one"
    );
}

#[test]
fn a_module_is_marked_fully_resolved_only_when_every_file_beneath_it_is() {
    let repo = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    // Distinct type names on purpose: two files declaring the SAME type in the
    // same namespace share one identity by design (KTD3), which would make this
    // fixture about identity collision instead of about coverage.
    init_repo_with_files(
        repo.path(),
        "Alice",
        &[
            ("covered/Invoice.cs", INVOICE_CS),
            (
                "mixed/Order.cs",
                "namespace Orbit.Billing\n{\n    public class Order\n    {\n    }\n}\n",
            ),
            ("mixed/README.md", "# not covered by any index"),
        ],
    );

    // The index covers everything under `covered/` and only part of `mixed/`.
    let index = agreeing_index(
        out.path(),
        repo.path(),
        &["covered/Invoice.cs", "mixed/Order.cs"],
    );
    let coverage = coverage_over(repo.path(), Some(index));

    assert!(
        module(&coverage, "covered").fully_resolved,
        "every file beneath `covered` is resolved: {:?} / {:?}",
        coverage.resolved_index,
        coverage.files
    );
    assert!(
        !module(&coverage, "mixed").fully_resolved,
        "a partial licence is no licence (R20)"
    );
    // And the root, which holds both, is therefore not fully resolved either.
    assert!(!module(&coverage, ".").fully_resolved);
    assert_eq!(module(&coverage, "mixed").files, 2);
}

#[test]
fn an_index_whose_declarations_moved_leaves_coverage_heuristic() {
    let repo = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    init_repo_with_files(repo.path(), "Alice", &[("src/Invoice.cs", INVOICE_CS)]);

    // Same names, wrong lines — an index built against an older revision.
    let drifted = write_scip_index(
        out.path(),
        vec![scip_document(
            "src\\Invoice.cs",
            vec![
                scip_occurrence(&scip_sym("Orbit/Billing/Invoice#"), 90, true),
                scip_occurrence(&scip_sym("Orbit/Billing/Invoice#Total()."), 95, true),
            ],
        )],
    );

    let coverage = coverage_over(repo.path(), Some(drifted));

    assert_eq!(
        file(&coverage, "src/Invoice.cs").resolution,
        "heuristic",
        "a drifted file must not be reported as resolved (R6/R20)"
    );
    assert!(!module(&coverage, "src").fully_resolved);
    // The rejection is reported, not silent.
    let index = coverage.resolved_index.expect("an index was supplied");
    assert!(index.summary.contains("1 drifted"), "{}", index.summary);
    assert_eq!(index.indexed_by, "scip-dotnet 0.2.14");
}

#[test]
fn a_run_without_an_index_claims_no_resolved_coverage_at_all() {
    let repo = tempfile::tempdir().unwrap();
    init_repo_with_files(repo.path(), "Alice", &[("src/Invoice.cs", INVOICE_CS)]);

    let coverage = coverage_over(repo.path(), None);

    assert!(coverage.resolved_index.is_none());
    assert!(coverage.files.iter().all(|f| f.resolution == "heuristic"));
    assert!(coverage.modules.iter().all(|m| !m.fully_resolved));
    // The artifact still exists and still lists every file: a heuristic run is a
    // complete run, not a degraded one.
    assert_eq!(coverage.files.len(), 1);
}

#[test]
fn the_artifact_records_the_revision_it_was_produced_over() {
    let repo = tempfile::tempdir().unwrap();
    init_repo_with_files(repo.path(), "Alice", &[("src/Invoice.cs", INVOICE_CS)]);

    let coverage = coverage_over(repo.path(), None);
    let revision = coverage.repo_revision.expect("the fixture has a commit");
    assert_eq!(revision.len(), 40, "a full git object id: {revision}");
    assert!(
        revision.chars().all(|c| c.is_ascii_hexdigit()),
        "{revision}"
    );
}

#[test]
fn the_artifact_is_valid_stably_ordered_json_and_two_runs_agree() {
    let repo = tempfile::tempdir().unwrap();
    init_repo_with_files(
        repo.path(),
        "Alice",
        &[
            ("src/Invoice.cs", INVOICE_CS),
            ("src/helper.rs", HELPER_RS),
            ("README.md", "# readme"),
        ],
    );

    let first = to_json(&coverage_over(repo.path(), None));
    let second = to_json(&coverage_over(repo.path(), None));
    assert_eq!(first, second, "two runs must agree byte for byte (R18)");

    let parsed: serde_json::Value = serde_json::from_str(&first).expect("valid JSON");
    assert_eq!(parsed["schema_version"], SCHEMA_VERSION);

    // Stable ordering is what makes a diff of this artifact readable at all.
    let paths: Vec<&str> = parsed["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap())
        .collect();
    let mut sorted = paths.clone();
    sorted.sort();
    assert_eq!(paths, sorted, "files must be ordered");

    // It round-trips, so a consumer can deserialize it with the same types.
    let back: Coverage = serde_json::from_str(&first).expect("round-trips");
    assert_eq!(to_json(&back), first);
}

#[test]
fn the_language_totals_and_the_unclassified_count_close_over_every_file() {
    // Files the language table does not recognize — a `.csproj`, a `.json`, a
    // file with no extension — are listed in `files` but belong to no language
    // entry. Without a number for them a consumer who sums `languages[].files`
    // silently gets less than `files.len()` and cannot tell a classification
    // gap from a missing file.
    let repo = tempfile::tempdir().unwrap();
    init_repo_with_files(
        repo.path(),
        "Alice",
        &[
            (
                "src/Billing.cs",
                "namespace N\n{\n    public class A\n    {\n    }\n}\n",
            ),
            ("src/lib.rs", "pub struct B;\n"),
            ("Orbit.csproj", "<Project Sdk=\"Microsoft.NET.Sdk\" />\n"),
            ("appsettings.json", "{ \"Logging\": {} }\n"),
            ("LICENSE", "MIT\n"),
        ],
    );

    let coverage = coverage_over(repo.path(), None);

    let classified: usize = coverage.languages.iter().map(|l| l.files).sum();
    assert_eq!(
        classified + coverage.unclassified_files,
        coverage.files.len(),
        "the totals must close: {} classified + {} unclassified != {} files",
        classified,
        coverage.unclassified_files,
        coverage.files.len()
    );
    assert!(
        coverage.unclassified_files >= 3,
        "the three non-source files must be counted, not dropped: {coverage:?}"
    );
    // And the number must be earned, not a constant: the classified side is real.
    assert!(classified >= 2, "{:?}", coverage.languages);
}

#[test]
fn a_partially_resolved_module_says_how_far_off_the_licence_it_is() {
    // `fully_resolved: false` is the same answer for eleven of twelve and for
    // zero of twelve. The count is what a reader of coverage actually arrives
    // for, and deriving it from the `files` array means re-implementing this
    // artifact's own module-to-file containment rule.
    let repo = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    init_repo_with_files(
        repo.path(),
        "Alice",
        &[
            (
                "mixed/A.cs",
                "namespace N\n{\n    public class A\n    {\n    }\n}\n",
            ),
            (
                "mixed/B.cs",
                "namespace N\n{\n    public class B\n    {\n    }\n}\n",
            ),
            ("empty/notes.txt", "nothing a grammar covers\n"),
        ],
    );

    // An index that verifies only ONE of the two files under `mixed`.
    let index = agreeing_index(out.path(), repo.path(), &["mixed/A.cs"]);
    let coverage = coverage_over(repo.path(), Some(index));

    let mixed = module(&coverage, "mixed");
    assert_eq!((mixed.files, mixed.resolved), (2, 1));
    assert!(!mixed.fully_resolved, "one of two is not a licence (R20)");

    // The flag stays derivable from the counts, in every module. A consumer that
    // trusts one and computes the other must never see them disagree.
    for m in &coverage.modules {
        assert_eq!(
            m.fully_resolved,
            m.files > 0 && m.resolved == m.files,
            "{} disagrees with its own counts: {m:?}",
            m.path
        );
        assert!(m.resolved <= m.files, "{m:?}");
    }

    // And an empty module was already distinguishable — `files == 0` says so.
    // That half of the finding this test came from was rejected, not fixed.
    let empty = module(&coverage, "empty");
    assert_eq!(empty.files, 1, "a file no grammar covers is still a file");
    assert_eq!(empty.resolved, 0);
}

// ---------------------------------------------------------------------------
// U4 / R26: languages arrive at run time, and a plugin failure has its own
// vocabulary. These read the registry as an operator would — through the
// artifact — rather than asserting on the table itself.
// ---------------------------------------------------------------------------

/// The same pipeline as `coverage_over`, on a registry the caller supplied.
///
/// The registry goes to BOTH `run_all_with` and `build_coverage`, which is the
/// point: the extractors that classify by language and the projection that
/// reports it must read one table, or the artifact would name a language the
/// run never actually counted.
fn coverage_over_languages(repo_dir: &Path, languages: Languages) -> Coverage {
    let repo = Repo::open(repo_dir).unwrap();
    let mut graph = Graph::new();
    run_all_with(
        &repo,
        &mut graph,
        &ExtractOptions {
            languages: languages.clone(),
            ..ExtractOptions::default()
        },
        &reverse_architect::progress::Silent,
    )
    .unwrap();
    reverse_architect::graph::merge(&mut graph, &reverse_architect::extract::trust_ranks());
    build_coverage(&graph, repo.head_revision(), &languages, None)
}

/// A registry that knows a language this build has no grammar for.
fn with_plugin_language() -> Languages {
    let mut languages = Languages::bundled();
    languages
        .register(
            "ktd",
            LanguageEntry {
                name: "otevrenost".to_string(),
                loc: None,
            },
        )
        .expect("a plugin language registers onto the bundled set");
    languages
}

/// R26, stated as the artifact states it: the file is NAMED, and it is not told
/// that no grammar covers its type.
///
/// It is still told it declares no symbol, which is true — nothing parsed it,
/// because a registered language with no extractor behind it is a state only a
/// test can reach today. Distinguishing "parsed and empty" from "never parsed"
/// is what `LanguageStatus` exists for and what U10 wires up.
#[test]
fn a_registered_plugin_language_is_named_rather_than_null() {
    let repo = tempfile::tempdir().unwrap();
    init_repo_with_files(
        repo.path(),
        "Alice",
        &[
            ("src/helper.rs", HELPER_RS),
            ("vzor/faktura.ktd", "direktiva Faktura\n"),
        ],
    );

    let coverage = coverage_over_languages(repo.path(), with_plugin_language());

    let plugin_file = file(&coverage, "vzor/faktura.ktd");
    assert_eq!(plugin_file.language.as_deref(), Some("otevrenost"));
    assert_ne!(
        plugin_file.not_structural_because.as_deref(),
        Some(NO_GRAMMAR),
        "a file the registry claims must not be told nothing covers its type"
    );
}

/// The documented invariant — `sum(languages[].files) + unclassified_files ==
/// files.len()` — has to survive a language the build was not compiled with,
/// or the aggregate silently stops accounting for part of the repository.
#[test]
fn the_language_totals_close_with_a_registered_language_present() {
    let repo = tempfile::tempdir().unwrap();
    init_repo_with_files(
        repo.path(),
        "Alice",
        &[
            ("src/helper.rs", HELPER_RS),
            ("vzor/faktura.ktd", "direktiva Faktura\n"),
            ("README.md", "# readme"),
        ],
    );

    let coverage = coverage_over_languages(repo.path(), with_plugin_language());

    let counted: usize = coverage.languages.iter().map(|l| l.files).sum();
    assert_eq!(
        counted + coverage.unclassified_files,
        coverage.files.len(),
        "languages {:?}, unclassified {}, files {}",
        coverage.languages,
        coverage.unclassified_files,
        coverage.files.len()
    );
    assert!(
        coverage
            .languages
            .iter()
            .any(|l| l.language == "otevrenost" && l.files == 1),
        "the registered language must get its own aggregate: {:?}",
        coverage.languages
    );
    // README.md is claimed by nothing and must still be accounted for.
    assert_eq!(coverage.unclassified_files, 1);
}

/// R14/R29: each terminal state gets its own sentence, and none of them may say
/// "no grammar covers this file type" — the file HAS a language; what failed is
/// the thing that was supposed to read it.
#[test]
fn every_plugin_failure_states_a_distinct_reason_of_its_own() {
    let statuses = [
        LanguageStatus::PluginMissing {
            plugin: "otevrenost".to_string(),
        },
        LanguageStatus::VersionMismatch {
            plugin: "otevrenost".to_string(),
            wanted: "2.0.0".to_string(),
            found: "1.4.0".to_string(),
        },
        LanguageStatus::UnsatisfiedImport {
            plugin: "otevrenost".to_string(),
            interface: "wasi:filesystem/types".to_string(),
            version: "0.2.0".to_string(),
        },
        LanguageStatus::Failed {
            plugin: "otevrenost".to_string(),
            reason: "declarations: trapped".to_string(),
        },
    ];

    let mut seen = std::collections::BTreeSet::new();
    for status in &statuses {
        let reason = not_structural_because(false, Some("otevrenost"), status)
            .expect("a file with no structure always carries a reason");
        assert_ne!(reason, NO_GRAMMAR, "{status:?}");
        assert_ne!(reason, NO_DECLARATION, "{status:?}");
        assert!(
            reason.contains("otevrenost"),
            "the reason must name the plugin: {reason}"
        );
        assert!(
            seen.insert(reason.clone()),
            "two terminal states collapsed into one reason: {reason}"
        );
    }
    assert_eq!(seen.len(), statuses.len());
}

/// "It did not run" hides two very different fixes: install the plugin, or
/// upgrade the host. The import reason has to say which, so it names the
/// interface and the version that could not be satisfied.
#[test]
fn the_unsatisfied_import_reason_names_the_interface_and_version() {
    let reason = not_structural_because(
        false,
        Some("otevrenost"),
        &LanguageStatus::UnsatisfiedImport {
            plugin: "otevrenost".to_string(),
            interface: "wasi:filesystem/types".to_string(),
            version: "0.2.0".to_string(),
        },
    )
    .unwrap();
    assert!(reason.contains("wasi:filesystem/types"), "{reason}");
    assert!(reason.contains("0.2.0"), "{reason}");
}

/// `not_structural_because` is documented as present EXACTLY when `structural`
/// is false. A failing plugin must not start annotating files that do carry
/// structure — some other extractor's facts are still facts.
#[test]
fn a_file_that_carries_structure_gets_no_reason_whatever_failed() {
    for status in [
        LanguageStatus::Parsed,
        LanguageStatus::Failed {
            plugin: "otevrenost".to_string(),
            reason: "declarations: trapped".to_string(),
        },
    ] {
        assert_eq!(not_structural_because(true, Some("rust"), &status), None);
    }
}

/// The two pre-plugin reasons keep their exact wording and their exact
/// trigger — this is the arm every existing corpus was built with.
#[test]
fn an_unclaimed_file_and_an_empty_one_keep_their_original_reasons() {
    assert_eq!(
        not_structural_because(false, None, &LanguageStatus::Parsed).as_deref(),
        Some(NO_GRAMMAR)
    );
    assert_eq!(
        not_structural_because(false, Some("rust"), &LanguageStatus::Parsed).as_deref(),
        Some(NO_DECLARATION)
    );
}

// ---------------------------------------------------------------------------
// Plan A, U4 (R21, KTD14): the noise counters. Read through `generate`, the
// pipeline an operator runs, so the card layer's count reaching the artifact
// is what is tested, not only the count itself.
// ---------------------------------------------------------------------------

/// The `GeoSolver` shape from plan A's Problem Frame, in miniature (the same
/// fixture `tests/card.rs` renders). Inside the solver `CleanTyp` calls
/// `FoundTables` — a call between two members of one type. A foreign
/// `CleanServer.Run` calls `FoundTables` from three lines and makes a bare
/// `Solve(1)` call that the solver's two overloads both answer. The server
/// imports the solver's namespace on its first line, as valid C# must (plan A,
/// U7: `namespace-visibility` disputes a type the file cannot see).
const GEO_SOLVER_CS: &str = "namespace Orbit.Geo\n{\n    public class GeoSolver\n    {\n        \
    public void CleanTyp()\n        {\n            FoundTables();\n        }\n\n        \
    public void FoundTables() { }\n\n        public int Solve(int n) { return n; }\n\n        \
    public int Solve(string s) { return 0; }\n    }\n}\n";
const CLEAN_SERVER_CS: &str = "using Orbit.Geo; namespace Orbit.Server\n{\n    public class CleanServer\n    {\n        \
    public void Run(GeoSolver solver)\n        {\n            solver.FoundTables();\n            \
    solver.FoundTables();\n            solver.FoundTables();\n            Solve(1);\n        }\n    \
    }\n}\n";

fn geo_repo() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            ("Geo/GeoSolver.cs", GEO_SOLVER_CS),
            ("Server/CleanServer.cs", CLEAN_SERVER_CS),
        ],
    );
    tmp
}

fn generate_over(repo: &Path, out: &Path) -> reverse_architect::Generated {
    reverse_architect::generate(&reverse_architect::config::Config {
        repo_root: repo.to_path_buf(),
        corpus_dir: out.to_path_buf(),
        links: reverse_architect::cli::LinkStyle::Portable,
        profile: None,
        ambiguity_ceiling: reverse_architect::config::DEFAULT_AMBIGUITY_CEILING,
        secret_prefixes: Vec::new(),
        plugins: Vec::new(),
        scip_index: None,
        lang: "en".to_string(),
        sites: false,
        rights: reverse_architect::extract::RightsTable::granted(),
        model_verification: false,
        model_purpose: false,
        trace_over_ceiling: false,
        detail_level: None,
    })
    .expect("the pipeline runs")
}

/// R21's thresholds, on the fixture that has both kinds of card noise to
/// offer. `CleanTyp -> FoundTables` stands in both lists of the solver's card
/// as assembled — once as `Relations` via `CleanTyp()`, once as `Referenced by`
/// via `FoundTables()` — so 2 reversed rows before the filter, and none after.
///
/// Duplicates are 0 before the filter too, and that is the healthy reading:
/// the pipeline hands the cards a merged graph, which holds one edge per
/// relation, and the overloaded `Solve(1)` is two relations (two members), not
/// one relation twice. A rise there means the graph stopped being merged.
#[test]
fn card_noise_is_counted_before_the_filter_and_is_zero_after_it() {
    let repo = geo_repo();
    let out = tempfile::tempdir().unwrap();
    let generated = generate_over(repo.path(), out.path());

    let noise = generated.coverage.noise.as_ref().expect("v5 writes noise");
    let rows = noise.card_rows.expect("the pipeline built cards");
    assert_eq!(rows.reversed_rows.before_filter, 2, "{rows:?}");
    assert_eq!(rows.duplicate_rows.before_filter, 0, "{rows:?}");
    // The thresholds R21 asks for.
    assert_eq!(rows.reversed_rows.after_filter, 0, "{rows:?}");
    assert_eq!(rows.duplicate_rows.after_filter, 0, "{rows:?}");
}

/// The duplicate counter, where duplicates exist: a graph nobody merged, in
/// which a second source asserted `Run -> FoundTables` again — what an index
/// beside the grammar does before `merge` folds the two into one edge. The
/// relation is a row of `CleanServer`'s `Relations` and of `GeoSolver`'s
/// `Referenced by`, so the repeat is 2 rows before the filter; the card's
/// dedup folds each pair into one row.
#[test]
fn duplicate_rows_of_an_unmerged_graph_are_counted_and_folded() {
    let repo = geo_repo();
    let handle = Repo::open(repo.path()).unwrap();
    let mut graph = Graph::new();
    reverse_architect::extract::run_all(&handle, &mut graph).unwrap();
    let (from, to, edge) = graph
        .edges_sorted()
        .into_iter()
        .find(|e| {
            e.edge.kind == EdgeKind::Calls
                && e.from.as_str().contains("CleanServer.Run")
                && e.to.as_str().contains("GeoSolver.FoundTables")
        })
        .map(|e| (e.from.clone(), e.to.clone(), e.edge.clone()))
        .expect("the fixture calls FoundTables from Run");
    assert!(graph.add_edge(&from, &to, edge));

    let (_, _, rows) = reverse_architect::card::build_cards_with_insight(&graph);
    assert_eq!(rows.duplicate_rows.before_filter, 2, "{rows:?}");
    assert_eq!(rows.duplicate_rows.after_filter, 0, "{rows:?}");
    assert_eq!(rows.reversed_rows.after_filter, 0, "{rows:?}");
}

/// The rung counters close over the name-resolved edges, counted here from the
/// graph by the predicate the manifest writes a rung under — and they say the
/// specific thing the fixture holds: the bare `Solve(1)` is two ambiguous
/// edges, one per overload, and the shared resolver (plan A, U7) backs two
/// calls of `FoundTables` — `declared-receiver` the server's on its
/// `GeoSolver` parameter, `implicit-this` the solver's own bare call. No index
/// was given, so nothing is resolved by one.
#[test]
fn the_rung_counters_sum_to_the_name_resolved_edges() {
    let repo = geo_repo();
    let out = tempfile::tempdir().unwrap();
    let generated = generate_over(repo.path(), out.path());

    let by_rung = generated
        .coverage
        .noise
        .as_ref()
        .expect("v5 writes noise")
        .edges_by_rung;
    let name_resolved = generated
        .graph
        .edges_sorted()
        .iter()
        .filter(|e| e.edge.provenance.rung().is_some())
        .count() as u64;
    assert_eq!(
        by_rung.resolved_by_index + by_rung.rules_backed + by_rung.name_only + by_rung.ambiguous,
        name_resolved,
        "{by_rung:?}"
    );
    assert_eq!(by_rung.ambiguous, 2, "{by_rung:?}");
    assert_eq!(
        by_rung.resolved_by_index, 0,
        "no index was given: {by_rung:?}"
    );
    assert_eq!(by_rung.rules_backed, 2, "{by_rung:?}");
    assert!(by_rung.name_only > 0, "{by_rung:?}");
}

/// R22 for the new section: two runs give the same bytes — the second over the
/// cache the first one wrote.
#[test]
fn the_noise_section_is_byte_identical_across_runs() {
    let repo = geo_repo();
    let out = tempfile::tempdir().unwrap();
    let first = to_json(&generate_over(repo.path(), out.path()).coverage);
    let second = to_json(&generate_over(repo.path(), out.path()).coverage);
    assert_eq!(first, second);

    let parsed: serde_json::Value = serde_json::from_str(&first).unwrap();
    assert_eq!(parsed["schema_version"], SCHEMA_VERSION);
    // The reserved counters are in the shape already, empty, so the units that
    // fill them change values and not the format.
    let noise = &parsed["noise"];
    assert_eq!(noise["deleted_by_rule"], serde_json::json!({}));
    assert_eq!(noise["over_cap_sites"], 0);
    assert!(noise.get("index_match").is_none(), "{noise}");
    // Filled since plan A, U5: the eight bundled plugins, by contract — the
    // six tree-sitter families on 0.2 (U6), the SQL example still on 0.1, the
    // XAML plugin on 0.3.
    assert_eq!(
        noise["plugins_by_contract"],
        serde_json::json!({"0.1": 1, "0.2": 6, "0.3": 1})
    );
}

/// Plan B, U4: a C or C++ file is a recognized language without type coverage
/// — named, counted under its language, and said to have had only its
/// literals and comments read, never that a grammar found no declaration.
#[test]
fn c_and_cpp_files_are_recognized_without_type_coverage() {
    let repo = tempfile::tempdir().unwrap();
    init_repo_with_files(
        repo.path(),
        "Alice",
        &[
            ("native/q.c", "int main(void) { return 0; }\n"),
            ("native/r.hpp", "class R { };\n"),
        ],
    );
    let coverage = coverage_over(repo.path(), None);
    for (path, language) in [("native/q.c", "c"), ("native/r.hpp", "cpp")] {
        let f = file(&coverage, path);
        assert_eq!(f.language.as_deref(), Some(language), "{path}");
        assert!(!f.structural, "{path}");
        assert_eq!(
            f.not_structural_because.as_deref(),
            Some(LEXED_ONLY),
            "{path}"
        );
        let entry = coverage
            .languages
            .iter()
            .find(|l| l.language == language)
            .unwrap_or_else(|| panic!("`{language}` is not among the languages"));
        assert_eq!((entry.files, entry.structural), (1, 0), "{language}");
    }
    assert_eq!(coverage.unclassified_files, 0);
}

/// Plan B, U5 (KTD3): the decisions by an enum's value are counted — the
/// verified labels and arms, the labels and arms left unverified, the
/// comparisons (never verified), and the places no single enum could be tied
/// to, which leave no fact of their own.
#[test]
fn enum_decisions_are_counted_by_what_became_of_them() {
    let tmp = tempfile::tempdir().unwrap();
    let worker = "namespace App;

public class Worker
{
    public void Run(Kind k, int m)
    {
        switch (k)
        {
            case Kind.Full:
                break;
        }
        if (k == Kind.Empty) { }
        switch (m)
        {
            case Twin.Full:
                break;
        }
    }

    public static class Kind
    {
        public const int Other = 2;
    }

    public void Again(int m)
    {
        switch (m)
        {
            case Twin.Full:
                break;
        }
    }
}
";
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            (
                "Lib/Kind.cs",
                "namespace Lib;\n\npublic enum Kind\n{\n    Full,\n    Empty,\n}\n",
            ),
            ("A/Twin.cs", "namespace A;\n\npublic enum Twin\n{\n    Full,\n}\n"),
            ("B/Twin.cs", "namespace B;\n\npublic enum Twin\n{\n    Full,\n}\n"),
            ("App/Plain.cs", "using Lib;\n\nnamespace App;\n\npublic class Plain\n{\n    public void Go(Kind k)\n    {\n        var n = k switch\n        {\n            Kind.Full => 1,\n            _ => 0,\n        };\n    }\n}\n"),
            ("App/Worker.cs", worker),
        ],
    );
    let coverage = coverage_over(tmp.path(), None);
    let counts = coverage
        .enum_decisions
        .expect("every run from coverage v6 on counts the enum decisions");
    // `Plain.Go`'s arm is verified. `Worker.Run`'s label names the enum
    // `Kind`, which a nested class of the same name shadows: unverified. Its
    // comparison is a comparison. The two `Twin.Full` labels see neither
    // `Twin`, and there are two of them: unmatched.
    assert_eq!(
        (
            counts.verified,
            counts.unverified,
            counts.comparisons,
            counts.unmatched
        ),
        (1, 1, 1, 2)
    );
}

/// Plan B, U12: a method holding a secret in a comment, a query that writes,
/// a procedure call and a message. The comment line is withheld — one text,
/// for the reason `password` — and the three literals are anchors.
const LOADER_CS: &str = r#"namespace App
{
    public class Loader
    {
        public void Run()
        {
            // password = hunter2
            Exec("UPDATE dba.jobs SET state = 1");
            Exec("exec dba.reset_jobs");
            Log("Backup restored from the archive");
        }

        private void Exec(string sql) { }

        private void Log(string text) { }
    }
}
"#;

#[test]
fn a_secret_in_a_comment_is_one_withheld_text_and_the_rest_are_counted_by_kind() {
    let repo = tempfile::tempdir().unwrap();
    init_repo_with_files(repo.path(), "Alice", &[("App/Loader.cs", LOADER_CS)]);
    let coverage = coverage_over(repo.path(), None);

    let anchors = coverage
        .anchors
        .expect("every run from v6 on counts anchors");
    assert_eq!(
        anchors.withheld,
        std::collections::BTreeMap::from([("password".to_string(), 1)])
    );
    assert_eq!((anchors.kept, anchors.dropped_by_cap), (3, 0));

    let db = coverage
        .db_accesses
        .expect("every run from v6 on counts database accesses");
    assert_eq!(
        (db.reads, db.writes, db.calls, db.mentions),
        (0, 1, 1, 0),
        "{db:?}"
    );
}

/// KTD1: an owner keeps [`ANCHOR_CAP`] texts and counts the rest as dropped.
#[test]
fn texts_over_the_anchor_cap_are_counted_as_dropped() {
    let cap = reverse_architect::graph::ANCHOR_CAP;
    let calls: String = (0..cap + 2)
        .map(|i| format!("            Log(\"message number {i} of the batch\");\n"))
        .collect();
    let source = format!(
        "namespace App\n{{\n    public class Chatty\n    {{\n        public void Run()\n        {{\n{calls}        }}\n\n        private void Log(string text) {{ }}\n    }}\n}}\n"
    );
    let repo = tempfile::tempdir().unwrap();
    init_repo_with_files(repo.path(), "Alice", &[("App/Chatty.cs", &source)]);
    let anchors = coverage_over(repo.path(), None).anchors.unwrap();
    assert_eq!((anchors.kept, anchors.dropped_by_cap), (cap as u64, 2));
    assert!(anchors.withheld.is_empty(), "{anchors:?}");
}

/// Plan B, U12: the card levels. `Documented` has a doc comment and a public
/// method — a purpose from its comment and an API, so a detail. `Empty` has
/// neither: its purpose is the structure's alone and its card says all there
/// is, so it gets no detail. `Many` holds one more type than a map part, so
/// its map is cut into two parts.
fn levels_repo() -> tempfile::TempDir {
    let many: String = (0..=reverse_architect::card::map::MAP_PART_ROWS)
        .map(|i| format!("    public class T{i:02} {{ }}\n"))
        .collect();
    let repo = tempfile::tempdir().unwrap();
    init_repo_with_files(
        repo.path(),
        "Alice",
        &[
            (
                "App/Documented.cs",
                "namespace App\n{\n    /// <summary>Restores a backup.</summary>\n    public class Documented\n    {\n        public void Restore() { }\n    }\n}\n",
            ),
            ("App/Empty.cs", "namespace App\n{\n    public class Empty { }\n}\n"),
            ("Many/Many.cs", &format!("namespace Many\n{{\n{many}}}\n")),
        ],
    );
    repo
}

#[test]
fn the_card_levels_are_counted_and_sized() {
    let repo = levels_repo();
    let out = tempfile::tempdir().unwrap();
    let generated = generate_over(repo.path(), out.path());
    let cards = generated
        .coverage
        .cards
        .as_ref()
        .expect("generation builds cards, so it counts them");

    let types = 2 + reverse_architect::card::map::MAP_PART_ROWS as u64 + 1;
    assert_eq!(cards.type_cards, types);
    // Only `Documented` has a comment to take a purpose sentence from.
    assert_eq!(cards.purpose_without_comment, types - 1);
    assert_eq!(cards.details.types, 1, "{:?}", cards.details);
    assert_eq!(
        cards.details.types_without,
        types - 1,
        "{:?}",
        cards.details
    );
    assert_eq!(cards.details.split, 0, "{:?}", cards.details);
    // `App` and `Many` have a map; `Many`'s is cut in two.
    assert_eq!(
        (cards.maps.maps, cards.maps.split, cards.maps.parts),
        (2, 1, 2),
        "{:?}",
        cards.maps
    );

    // The sizes are of what was rendered: every type card, every detail, and
    // every row of every map, its line break included.
    let mut card_sizes: Vec<u64> = generated
        .cards
        .iter()
        .filter(|c| c.path.starts_with("cards/type/"))
        .map(|c| c.contents.len() as u64)
        .collect();
    card_sizes.sort_unstable();
    assert_eq!(cards.sizes.type_card.count, types);
    assert_eq!(card_sizes.len() as u64, types);
    assert!(card_sizes.contains(&cards.sizes.type_card.median));
    assert!(card_sizes.contains(&cards.sizes.type_card.p90));
    assert!(cards.sizes.type_card.median <= cards.sizes.type_card.p90);
    assert_eq!(cards.sizes.detail.count, generated.details.len() as u64);
    // One row per hand-written type of each map.
    assert_eq!(cards.sizes.map_row.count, types);
    assert!(cards.sizes.map_row.p90 <= reverse_architect::card::map::MAP_ROW_BYTES as u64);
}

#[test]
fn the_card_levels_are_byte_identical_across_runs() {
    let repo = levels_repo();
    let out = tempfile::tempdir().unwrap();
    let first = to_json(&generate_over(repo.path(), out.path()).coverage);
    let second = to_json(&generate_over(repo.path(), out.path()).coverage);
    assert_eq!(first, second);
    let parsed: serde_json::Value = serde_json::from_str(&first).unwrap();
    assert!(
        parsed["cards"]["sizes"]["map_row"]["p90"].is_u64(),
        "{first}"
    );
}

/// A projection that built no cards says nothing about them — absent, not
/// zeros that would read as "measured, and empty".
#[test]
fn coverage_built_from_the_graph_alone_has_no_card_levels() {
    let repo = levels_repo();
    let coverage = coverage_over(repo.path(), None);
    assert!(coverage.cards.is_none());
    assert!(coverage.anchors.is_some() && coverage.db_accesses.is_some());
}
