//! The shared resolver end to end (plan A, U7): rights to delete arrive through
//! the generation config, deletions reach `coverage.json`, and the usage-site
//! records come out the same whatever state the cache is in.

use reverse_architect::extract::RightsTable;
use reverse_architect::{generate, Generated};
use std::path::Path;

mod common;

use common::declared_receiver_right;

/// A C# fixture with every kind of verdict in it: a receiver of an external
/// declared type (disputed, deletable), an interface receiver (backed, an
/// implementation left alone), a bare call (implicit-this), a name over the
/// ceiling, and two members on one line, whose calls only the columns of the
/// members' names can attribute.
fn fixture() -> Vec<(String, String)> {
    let mut many = String::from("namespace N;\n");
    for i in 0..12 {
        many.push_str(&format!("public class T{i} {{ public void Get() {{}} }}\n"));
    }
    vec![
        (
            "src/Report.cs".to_string(),
            "namespace N;\npublic class Report {\n  public void Write(StringBuilder result, IFoo f) {\n    result.Append(\"x\");\n    f.Run();\n    Helper();\n    Get();\n  }\n  private void Helper() {}\n}\n"
                .to_string(),
        ),
        (
            "src/Items.cs".to_string(),
            "namespace N;\npublic class LedgerItem {\n  public void Append(string s) {}\n}\npublic interface IFoo {\n  void Run();\n}\npublic class Foo : IFoo {\n  public void Run() {}\n  public void Helper() {}\n}\npublic class Bar {\n  public void Run() {}\n}\n"
                .to_string(),
        ),
        ("src/Many.cs".to_string(), many),
        (
            "src/Line.cs".to_string(),
            "namespace N;\npublic class Line { public void A() { Bx(); } public void B() { Ax(); } public void Ax() {} public void Bx() {} }\n"
                .to_string(),
        ),
    ]
}

fn repo_with_fixture(dir: &Path) {
    let owned = fixture();
    let files: Vec<(&str, &str)> = owned
        .iter()
        .map(|(p, c)| (p.as_str(), c.as_str()))
        .collect();
    common::init_repo_with_files(dir, "Alice", &files);
}

fn generate_with(repo: &Path, out: &Path, rights: RightsTable) -> Generated {
    let mut config = common::config_for(repo, out);
    config.rights = rights;
    config.sites = true;
    generate(&config).expect("the pipeline runs")
}

fn calls_to(g: &Generated, target: &str) -> usize {
    g.manifest
        .edges
        .iter()
        .filter(|e| e.kind == "calls" && g.manifest.strings[e.to_id] == target)
        .count()
}

/// Without a right the disputed candidate stays, in "maybe also"; with the
/// right the run's config hands in, it is gone and `coverage.json` counts it
/// under the rule. The implementation of the declared interface survives
/// both ways: the rule never disputes it.
#[test]
fn a_granted_right_deletes_and_coverage_counts_the_deletion() {
    let repo = tempfile::tempdir().unwrap();
    repo_with_fixture(repo.path());

    let out = tempfile::tempdir().unwrap();
    let plain = generate_with(repo.path(), out.path(), RightsTable::granted());
    let noise = plain.coverage.noise.as_ref().expect("v5 noise");
    assert!(noise.deleted_by_rule.is_empty());
    assert_eq!(noise.over_cap_sites, 1, "the twelve `Get`s");
    assert_eq!(
        calls_to(&plain, "sym:csharp:N#LedgerItem.Append(string)"),
        1
    );

    let out = tempfile::tempdir().unwrap();
    let granted = generate_with(repo.path(), out.path(), declared_receiver_right());
    let noise = granted.coverage.noise.as_ref().expect("v5 noise");
    // `LedgerItem.Append` (external declared type) and `Bar.Run` (unrelated
    // to `IFoo`).
    assert_eq!(
        noise.deleted_by_rule.get("declared-receiver"),
        Some(&2),
        "{:?}",
        noise.deleted_by_rule
    );
    assert_eq!(
        calls_to(&granted, "sym:csharp:N#LedgerItem.Append(string)"),
        0
    );
    assert_eq!(calls_to(&granted, "sym:csharp:N#Bar.Run()"), 0);
    assert_eq!(calls_to(&granted, "sym:csharp:N#Foo.Run()"), 1);
    assert_eq!(calls_to(&granted, "sym:csharp:N#IFoo.Run()"), 1);
}

/// A right recorded for another build of the component does not apply: the
/// rule only disputes, and nothing is deleted (plan A, KTD11; the U11 scenario
/// "a right for `csharp.wasm` with another SHA-256 than the running
/// component").
#[test]
fn a_right_for_another_component_build_deletes_nothing() {
    let repo = tempfile::tempdir().unwrap();
    repo_with_fixture(repo.path());
    let other_build = RightsTable::new(
        declared_receiver_right()
            .rights()
            .iter()
            .cloned()
            .map(|mut r| {
                r.component_sha256 = "f".repeat(64);
                r
            })
            .collect(),
    );
    let out = tempfile::tempdir().unwrap();
    let g = generate_with(repo.path(), out.path(), other_build);
    assert!(g
        .coverage
        .noise
        .as_ref()
        .unwrap()
        .deleted_by_rule
        .is_empty());
    assert_eq!(calls_to(&g, "sym:csharp:N#LedgerItem.Append(string)"), 1);
    // Still there, and only disputed: "maybe also", naming the rule.
    let edge = g
        .manifest
        .edges
        .iter()
        .find(|e| {
            e.kind == "calls"
                && g.manifest.strings[e.to_id] == "sym:csharp:N#LedgerItem.Append(string)"
        })
        .unwrap();
    assert_eq!(edge.provenance.confidence.as_deref(), Some("ambiguous"));
    assert_eq!(
        edge.provenance
            .rule_id
            .map(|id| g.manifest.strings[id].as_str()),
        Some("declared-receiver")
    );
}

/// Blocked, cold and warm cache over the mixed fixture, with a right in force,
/// agree on everything the resolver decided: the cards, the coverage with its
/// deletion counters, the manifest, and every site record (R22, KTD7). A run
/// that keeps the records reads no usage records, so its warm run is warm in
/// the declarations only; the cached usage counters are proven by
/// `a_run_without_records_decides_and_counts_as_one_that_keeps_them`.
#[test]
fn blocked_cold_and_warm_cache_agree_on_verdicts_records_and_deletions() {
    let repo = tempfile::tempdir().unwrap();
    repo_with_fixture(repo.path());

    let out_blocked = tempfile::tempdir().unwrap();
    std::fs::write(out_blocked.path().join(".ra-cache"), b"not a directory").unwrap();
    let blocked = generate_with(repo.path(), out_blocked.path(), declared_receiver_right());

    let out_cached = tempfile::tempdir().unwrap();
    let cold = generate_with(repo.path(), out_cached.path(), declared_receiver_right());
    let warm = generate_with(repo.path(), out_cached.path(), declared_receiver_right());

    let records = |g: &Generated| reverse_architect::manifest::to_sites_jsonl(&g.graph);
    assert!(
        records(&blocked).contains("\"outcome\":\"deleted\""),
        "precondition: the fixture deletes a candidate"
    );
    assert!(
        records(&blocked).contains("\"rung\":\"rules_backed\""),
        "precondition: the fixture backs a candidate"
    );
    assert!(
        records(&blocked).contains("\"owner\":\"sym:csharp:N#Line.A()\""),
        "precondition: a call on the one-line type is owned by its member"
    );
    for (name, other) in [("cold", &cold), ("warm", &warm)] {
        assert_eq!(blocked.cards, other.cards, "cards: blocked vs {name}");
        assert_eq!(
            blocked.coverage, other.coverage,
            "coverage: blocked vs {name}"
        );
        assert_eq!(
            blocked.manifest, other.manifest,
            "manifest: blocked vs {name}"
        );
        assert_eq!(
            records(&blocked),
            records(other),
            "site records: blocked vs {name}"
        );
        assert_eq!(
            blocked.graph.sites().deleted_by_rule(),
            other.graph.sites().deleted_by_rule(),
            "deletions: blocked vs {name}"
        );
    }
}

/// Granting a right between two runs over one cache is a different run: the
/// warm records, decided without the right, are not served — the second run
/// deletes as if cold.
#[test]
fn granting_a_right_between_runs_is_not_served_from_the_cache() {
    let repo = tempfile::tempdir().unwrap();
    repo_with_fixture(repo.path());
    let out = tempfile::tempdir().unwrap();
    let before = generate_with(repo.path(), out.path(), RightsTable::granted());
    assert_eq!(calls_to(&before, "sym:csharp:N#Bar.Run()"), 1);
    let after = generate_with(repo.path(), out.path(), declared_receiver_right());
    assert_eq!(calls_to(&after, "sym:csharp:N#Bar.Run()"), 0);
}

/// A run that keeps no site records builds none and caches only their two
/// counters (`SiteCounts`); blocked, cold and warm, it decides and counts
/// exactly as a run that keeps them. The fixture gains a site that ends over
/// the ceiling AFTER its deletions — `b.Get()` on a declared `Base`, whose ten
/// overrides stay and whose unrelated namesakes go — so a counter cached for
/// a site with no edges is the one that has to survive the warm run.
#[test]
fn a_run_without_records_decides_and_counts_as_one_that_keeps_them() {
    let repo = tempfile::tempdir().unwrap();
    let mut over = String::from(
        "namespace N;\npublic class Base { public virtual void Get() {} }\n\
         public class Use { public void Go(Base b) { b.Get(); } }\n",
    );
    for i in 0..10 {
        over.push_str(&format!(
            "public class D{i} : Base {{ public override void Get() {{}} }}\n"
        ));
    }
    let mut owned = fixture();
    owned.push(("src/Over.cs".to_string(), over));
    let files: Vec<(&str, &str)> = owned
        .iter()
        .map(|(p, c)| (p.as_str(), c.as_str()))
        .collect();
    common::init_repo_with_files(repo.path(), "Alice", &files);

    let out_kept = tempfile::tempdir().unwrap();
    let kept = generate_with(repo.path(), out_kept.path(), declared_receiver_right());
    use reverse_architect::graph::CandidateOutcome;
    assert!(
        kept.graph
            .sites()
            .records_sorted()
            .iter()
            .any(|r| r.over_cap()
                && r.candidates
                    .iter()
                    .any(|c| matches!(c.outcome, CandidateOutcome::Deleted { .. }))),
        "precondition: a site ends over the ceiling after deleting candidates"
    );

    let without = |out: &Path| {
        let mut config = common::config_for(repo.path(), out);
        config.rights = declared_receiver_right();
        generate(&config).expect("the pipeline runs")
    };
    let out_blocked = tempfile::tempdir().unwrap();
    std::fs::write(out_blocked.path().join(".ra-cache"), b"not a directory").unwrap();
    let blocked = without(out_blocked.path());
    let out_cached = tempfile::tempdir().unwrap();
    let cold = without(out_cached.path());
    let warm = without(out_cached.path());

    for (name, other) in [("blocked", &blocked), ("cold", &cold), ("warm", &warm)] {
        assert!(
            other.graph.sites().records_sorted().is_empty(),
            "{name}: no records kept"
        );
        assert_eq!(kept.cards, other.cards, "cards: kept vs {name}");
        assert_eq!(kept.coverage, other.coverage, "coverage: kept vs {name}");
        assert_eq!(kept.manifest, other.manifest, "manifest: kept vs {name}");
        assert_eq!(
            kept.graph.sites().deleted_by_rule(),
            other.graph.sites().deleted_by_rule(),
            "deletions: kept vs {name}"
        );
        assert_eq!(
            kept.graph.sites().over_cap_sites(),
            other.graph.sites().over_cap_sites(),
            "over-cap sites: kept vs {name}"
        );
    }
}
