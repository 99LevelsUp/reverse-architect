//! U6 integration tests: consuming an externally produced SCIP index.
//!
//! The indexes here are BUILT, not recorded. A real `scip-dotnet` run is what
//! proved the tier viable (gate, 19.08.2026), but a recorded blob cannot express
//! the cases that matter most — an index whose declarations have moved, or one
//! written from the wrong working directory. Constructing the index gives exact
//! control over every range, and the moniker syntax is the real one either way.

mod common;

use common::{
    at, at_end, decl_line, init_repo_with_files, occ, scip_document as document,
    scip_occurrence as occurrence, scip_occurrence_at, scip_sym as sym, symbol_nodes,
    write_scip_index as write_index,
};
use reverse_architect::extract::{run_all_with, ExtractError, ExtractOptions, Repo};
use reverse_architect::graph::{
    Confidence, Edge, EdgeKind, FactKind, FactValue, Graph, IndexPairing, NodeKey, Resolution,
    Rule, SiteId,
};
use scip::types::{Relationship, SymbolInformation};
use std::path::{Path, PathBuf};

/// Two types and a member, so there is something for the index to point at and
/// something for the heuristic tier to disagree with.
const BILLING_CS: &str = r#"namespace Orbit.Billing
{
    public class Money
    {
    }

    public class Invoice
    {
        public Money Total(Money amount)
        {
            return amount;
        }
    }
}
"#;

fn graph_with(repo_dir: &Path, scip_index: Option<PathBuf>) -> Result<Graph, ExtractError> {
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
    )?;
    reverse_architect::graph::merge(&mut graph, &reverse_architect::extract::trust_ranks());
    Ok(graph)
}

fn fact_texts(graph: &Graph, key: &str, kind: FactKind) -> Vec<String> {
    graph
        .node(&NodeKey::new(key))
        .map(|n| {
            n.facts
                .iter()
                .filter(|f| f.kind == kind)
                .filter_map(|f| match &f.value {
                    FactValue::Text(t) => Some(t.clone()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn edges(graph: &Graph, kind: EdgeKind) -> Vec<(String, String, Resolution)> {
    graph
        .edges_sorted()
        .iter()
        .filter(|e| e.edge.kind == kind)
        .map(|e| {
            (
                e.from.as_str().to_string(),
                e.to.as_str().to_string(),
                e.edge.provenance.resolution,
            )
        })
        .collect()
}

/// Build the fixture repo, then an index that agrees with it.
fn agreeing_fixture(repo: &Path, index_dir: &Path) -> PathBuf {
    init_repo_with_files(repo, "Alice", &[("src/Billing.cs", BILLING_CS)]);
    let baseline = graph_with(repo, None).unwrap();

    let money = sym("Orbit/Billing/Money#");
    let invoice = sym("Orbit/Billing/Invoice#");
    let total = sym("Orbit/Billing/Invoice#Total().");

    write_index(
        index_dir,
        vec![document(
            "src\\Billing.cs",
            vec![
                occurrence(&money, decl_line(&baseline, "Money"), true),
                occurrence(&invoice, decl_line(&baseline, "Invoice"), true),
                occurrence(&total, decl_line(&baseline, "Total"), true),
                // A reference to Money from inside Total's body.
                occurrence(&money, decl_line(&baseline, "Total") + 1, false),
            ],
        )],
    )
}

#[test]
fn an_index_that_agrees_with_the_tree_yields_resolved_facts_and_edges() {
    let repo = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let index = agreeing_fixture(repo.path(), out.path());

    let graph = graph_with(repo.path(), Some(index)).unwrap();

    // The file is marked covered, naming the indexer that covered it (R6).
    assert_eq!(
        fact_texts(&graph, "src/Billing.cs", FactKind::ResolvedByIndex),
        vec!["scip-dotnet 0.2.14".to_string()]
    );

    // The matched type carries its moniker as a resolved fact.
    let invoice = symbol_nodes(&graph, "Invoice");
    assert_eq!(invoice.len(), 1);
    assert_eq!(
        fact_texts(&graph, invoice[0].as_str(), FactKind::SymbolMoniker),
        vec![sym("Orbit/Billing/Invoice#")]
    );

    // The reference occurrence became a resolved edge, anchored at its line.
    let resolved: Vec<_> = graph
        .edges_sorted()
        .iter()
        .filter(|e| {
            e.edge.kind == EdgeKind::References
                && e.edge.provenance.resolution == Resolution::Resolved
        })
        .map(|e| {
            (
                e.from.as_str().to_string(),
                e.to.as_str().to_string(),
                e.edge.provenance.span.start(),
            )
        })
        .collect();
    assert!(
        !resolved.is_empty(),
        "expected a resolved reference edge, saw: {:?}",
        edges(&graph, EdgeKind::References)
    );
    for (_, _, line) in &resolved {
        assert!(line.is_some(), "a resolved edge must carry its anchor");
    }

    // The run reports what it accepted AND what it rejected (E9).
    let summary = fact_texts(&graph, ".", FactKind::ResolvedCoverage);
    assert_eq!(summary.len(), 1);
    assert!(
        summary[0].contains("1 documents verified")
            && summary[0].contains("0 drifted")
            && summary[0].contains("0 outside"),
        "{}",
        summary[0]
    );
}

#[test]
fn a_symbol_the_index_and_the_grammar_both_describe_stays_one_node() {
    let repo = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let index = agreeing_fixture(repo.path(), out.path());

    let heuristic = graph_with(repo.path(), None).unwrap();
    let resolved = graph_with(repo.path(), Some(index)).unwrap();

    // Consuming an index adds facts and edges; it must never add a second node
    // for a symbol the grammar already emitted (KTD3).
    assert_eq!(
        heuristic.node_count(),
        resolved.node_count(),
        "the index created a node of its own"
    );
    assert_eq!(symbol_nodes(&resolved, "Invoice").len(), 1);
    assert_eq!(symbol_nodes(&resolved, "Total").len(), 1);
}

#[test]
fn no_index_leaves_a_complete_heuristic_corpus_and_does_not_fail() {
    let repo = tempfile::tempdir().unwrap();
    init_repo_with_files(repo.path(), "Alice", &[("src/Billing.cs", BILLING_CS)]);

    let graph = graph_with(repo.path(), None).unwrap();

    assert!(
        !symbol_nodes(&graph, "Invoice").is_empty(),
        "facts still land"
    );
    assert!(
        fact_texts(&graph, ".", FactKind::ResolvedCoverage).is_empty(),
        "with no index there is no resolved coverage to claim"
    );
    assert!(
        fact_texts(&graph, "src/Billing.cs", FactKind::ResolvedByIndex).is_empty(),
        "and no file may be marked covered"
    );
    assert!(
        graph
            .edges_sorted()
            .iter()
            .all(|e| e.edge.provenance.resolution == Resolution::Heuristic),
        "every edge stays heuristic"
    );
}

#[test]
fn an_index_named_but_absent_fails_loudly_rather_than_degrading_silently() {
    let repo = tempfile::tempdir().unwrap();
    init_repo_with_files(repo.path(), "Alice", &[("src/Billing.cs", BILLING_CS)]);

    // Distinct from "no index": the caller asked for THIS file by name.
    let err = graph_with(repo.path(), Some(repo.path().join("nowhere.scip"))).unwrap_err();
    assert!(
        matches!(err, ExtractError::ScipRead { .. }),
        "unexpected error: {err}"
    );
}

#[test]
fn a_corrupt_index_fails_loudly_rather_than_yielding_an_empty_result() {
    let repo = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    init_repo_with_files(repo.path(), "Alice", &[("src/Billing.cs", BILLING_CS)]);

    let corrupt = out.path().join("index.scip");
    std::fs::write(&corrupt, b"this is not a protobuf message at all").unwrap();

    let err = graph_with(repo.path(), Some(corrupt)).unwrap_err();
    assert!(
        matches!(err, ExtractError::ScipFormat { .. }),
        "unexpected error: {err}"
    );
    // An empty corpus would be indistinguishable from a project with no
    // cross-references, which is exactly the confusion this refuses to create.
    assert!(err.to_string().contains("not a valid index"), "{err}");
}

#[test]
fn an_index_whose_declarations_have_moved_contributes_nothing() {
    let repo = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    init_repo_with_files(repo.path(), "Alice", &[("src/Billing.cs", BILLING_CS)]);
    let baseline = graph_with(repo.path(), None).unwrap();

    // The index describes the same names at lines far from where the grammar
    // finds them — what an index built against an older revision looks like.
    let drifted = write_index(
        out.path(),
        vec![document(
            "src\\Billing.cs",
            vec![
                occurrence(
                    &sym("Orbit/Billing/Money#"),
                    decl_line(&baseline, "Money") + 40,
                    true,
                ),
                occurrence(
                    &sym("Orbit/Billing/Invoice#"),
                    decl_line(&baseline, "Invoice") + 40,
                    true,
                ),
            ],
        )],
    );

    let graph = graph_with(repo.path(), Some(drifted)).unwrap();

    assert!(
        fact_texts(&graph, "src/Billing.cs", FactKind::ResolvedByIndex).is_empty(),
        "a drifted document must not license absence claims (R20)"
    );
    assert!(
        graph
            .edges_sorted()
            .iter()
            .all(|e| e.edge.provenance.resolution == Resolution::Heuristic),
        "no resolved edge may come from a drifted document"
    );
    let summary = fact_texts(&graph, ".", FactKind::ResolvedCoverage);
    assert!(
        summary[0].contains("0 documents verified") && summary[0].contains("1 drifted"),
        "the rejection must be reported, not silent: {}",
        summary[0]
    );
}

#[test]
fn an_index_from_a_different_working_directory_reports_itself() {
    let repo = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    init_repo_with_files(repo.path(), "Alice", &[("src/Billing.cs", BILLING_CS)]);

    // Indexed one directory up, so every path is prefixed and matches nothing
    // git tracks. Guessing at the prefix would be how wrong anchors get shipped.
    let elsewhere = write_index(
        out.path(),
        vec![document(
            "MyProject\\src\\Billing.cs",
            vec![occurrence(&sym("Orbit/Billing/Invoice#"), 7, true)],
        )],
    );

    let graph = graph_with(repo.path(), Some(elsewhere)).unwrap();
    let summary = fact_texts(&graph, ".", FactKind::ResolvedCoverage);
    assert!(
        summary[0].contains("0 documents verified") && summary[0].contains("1 outside"),
        "a wrong working directory must diagnose itself: {}",
        summary[0]
    );
}

#[test]
fn overloads_map_to_the_node_whose_range_holds_them_not_to_a_shared_name() {
    const OVERLOADS_CS: &str = r#"namespace Orbit.Billing
{
    public class Money
    {
    }

    public class Invoice
    {
        public Money Total(Money amount)
        {
            return amount;
        }

        public Money Total(Money amount, Money discount)
        {
            return amount;
        }
    }
}
"#;
    let repo = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    init_repo_with_files(repo.path(), "Alice", &[("src/Billing.cs", OVERLOADS_CS)]);
    let baseline = graph_with(repo.path(), None).unwrap();

    // Both overloads are declared under one name; only the LINE tells them apart.
    let totals = symbol_nodes(&baseline, "Total");
    assert_eq!(totals.len(), 2, "the fixture must declare two overloads");
    let mut lines: Vec<(u32, NodeKey)> = totals
        .iter()
        .map(|key| {
            let line = baseline
                .node(key)
                .unwrap()
                .facts
                .iter()
                .find(|f| f.kind == FactKind::SymbolName)
                .unwrap()
                .provenance
                .span
                .start()
                .unwrap();
            (line, key.clone())
        })
        .collect();
    lines.sort();

    // The disambiguator syntax a real `scip-dotnet` index uses for overloads —
    // an ordinal, not the parameter types (verified against an index over
    // a real C# library, 19.08.2026).
    let one = sym("Orbit/Billing/Invoice#Total().");
    let two = sym("Orbit/Billing/Invoice#Total(+1).");
    let index = write_index(
        out.path(),
        vec![document(
            "src\\Billing.cs",
            vec![
                occurrence(
                    &sym("Orbit/Billing/Invoice#"),
                    decl_line(&baseline, "Invoice"),
                    true,
                ),
                occurrence(&one, lines[0].0, true),
                occurrence(&two, lines[1].0, true),
            ],
        )],
    );

    let graph = graph_with(repo.path(), Some(index)).unwrap();

    // Each moniker landed on the overload whose declaration range holds its line.
    assert_eq!(
        fact_texts(&graph, lines[0].1.as_str(), FactKind::SymbolMoniker),
        vec![one]
    );
    assert_eq!(
        fact_texts(&graph, lines[1].1.as_str(), FactKind::SymbolMoniker),
        vec![two]
    );
}

#[test]
fn a_relation_both_tiers_assert_survives_once_at_the_index_precision() {
    let repo = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    init_repo_with_files(repo.path(), "Alice", &[("src/Billing.cs", BILLING_CS)]);
    let baseline = graph_with(repo.path(), None).unwrap();

    // The heuristic tier already sees Total → Money (a parameter type). Assert the
    // same pair from the index and the two must collapse into one resolved edge
    // (KTD6), not stand as two edges saying the same thing at two precisions.
    let heuristic_pairs = edges(&baseline, EdgeKind::References);
    let (from, to, _) = heuristic_pairs
        .iter()
        .find(|(from, to, _)| from.contains("Total") && to.ends_with("Money"))
        .expect("the heuristic tier should reference Money from Total");

    let index = write_index(
        out.path(),
        vec![document(
            "src\\Billing.cs",
            vec![
                occurrence(
                    &sym("Orbit/Billing/Money#"),
                    decl_line(&baseline, "Money"),
                    true,
                ),
                occurrence(
                    &sym("Orbit/Billing/Invoice#"),
                    decl_line(&baseline, "Invoice"),
                    true,
                ),
                occurrence(
                    &sym("Orbit/Billing/Invoice#Total()."),
                    decl_line(&baseline, "Total"),
                    true,
                ),
                occurrence(
                    &sym("Orbit/Billing/Money#"),
                    decl_line(&baseline, "Total"),
                    false,
                ),
            ],
        )],
    );

    let graph = graph_with(repo.path(), Some(index)).unwrap();

    let same_pair: Vec<_> = edges(&graph, EdgeKind::References)
        .into_iter()
        .filter(|(f, t, _)| f == from && t == to)
        .collect();
    assert_eq!(
        same_pair.len(),
        1,
        "one relation must be one edge: {same_pair:?}"
    );
    assert_eq!(
        same_pair[0].2,
        Resolution::Resolved,
        "the index must win over the heuristic"
    );
}

#[test]
fn an_implementation_relationship_becomes_a_resolved_implements_edge() {
    const IFACE_CS: &str = r#"namespace Orbit.Billing
{
    public interface IDocument
    {
    }

    public class Invoice : IDocument
    {
    }
}
"#;
    let repo = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    init_repo_with_files(repo.path(), "Alice", &[("src/Billing.cs", IFACE_CS)]);
    let baseline = graph_with(repo.path(), None).unwrap();

    let iface = sym("Orbit/Billing/IDocument#");
    let invoice = sym("Orbit/Billing/Invoice#");

    let mut information = SymbolInformation::new();
    information.symbol = invoice.clone();
    let mut relationship = Relationship::new();
    relationship.symbol = iface.clone();
    relationship.is_implementation = true;
    information.relationships = vec![relationship];

    let mut doc = document(
        "src\\Billing.cs",
        vec![
            occurrence(&iface, decl_line(&baseline, "IDocument"), true),
            occurrence(&invoice, decl_line(&baseline, "Invoice"), true),
        ],
    );
    doc.symbols = vec![information];
    let index = write_index(out.path(), vec![doc]);

    let graph = graph_with(repo.path(), Some(index)).unwrap();

    // Named endpoints, not "some resolved edge": a reversed or self-referential
    // implements edge is exactly the bug worth catching here, and it would
    // satisfy any weaker assertion.
    let invoice_key = symbol_nodes(&graph, "Invoice");
    let iface_key = symbol_nodes(&graph, "IDocument");
    assert_eq!(invoice_key.len(), 1);
    assert_eq!(iface_key.len(), 1);

    let implements = edges(&graph, EdgeKind::Implements);
    assert!(
        implements.contains(&(
            invoice_key[0].as_str().to_string(),
            iface_key[0].as_str().to_string(),
            Resolution::Resolved,
        )),
        "the resolved edge must run from the class to the interface, saw: {implements:?}"
    );
}

/// Every file/line at which the graph claims `name` is declared — the winning
/// `SymbolName` claim plus the dissent it demoted. A partial type is one node
/// with several declaration sites, and only the pair says where they all are.
fn decl_sites(graph: &Graph, name: &str) -> Vec<(String, u32)> {
    let mut sites: Vec<(String, u32)> = symbol_nodes(graph, name)
        .iter()
        .filter_map(|key| graph.node(key))
        .flat_map(|node| node.facts.iter())
        .filter(|f| f.kind == FactKind::SymbolName)
        .flat_map(|f| {
            std::iter::once((&f.value, &f.provenance))
                .chain(f.dissent.iter().map(|c| (&c.value, &c.provenance)))
        })
        .filter(|(value, _)| **value == FactValue::Text(name.to_string()))
        .filter_map(|(_, prov)| Some((prov.file.clone(), prov.span.start()?)))
        .collect();
    sites.sort();
    sites.dedup();
    sites
}

#[test]
fn a_partial_type_is_verified_in_every_file_that_declares_it() {
    // A C# partial class is ONE symbol declared in several files. KTD3 merges
    // those declarations into one node, and `SymbolName` is scalar, so all but
    // one site survives only as dissent. If the drift gate reads the winner
    // alone, the other file has no declaration to check against, fails to
    // verify, and silently loses every resolved fact it should have earned.
    let repo = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let part = |member: &str| {
        format!(
            "namespace Orbit.Sales\n{{\n    public partial class Order\n    {{\n        public int {member}()\n        {{\n            return 1;\n        }}\n    }}\n}}\n"
        )
    };
    init_repo_with_files(
        repo.path(),
        "Alice",
        &[
            ("src/Order.Ids.cs", part("Id").as_str()),
            ("src/Order.Totals.cs", part("Total").as_str()),
        ],
    );

    let baseline = graph_with(repo.path(), None).unwrap();
    // The premise: one node, two declaration sites. Without this the test would
    // pass for the wrong reason — two separate nodes each own their own file.
    assert_eq!(
        symbol_nodes(&baseline, "Order").len(),
        1,
        "the two halves must merge into one symbol"
    );
    let sites = decl_sites(&baseline, "Order");
    assert_eq!(
        sites.len(),
        2,
        "both halves must be recorded as declaration sites: {sites:?}"
    );

    let order = sym("Orbit/Sales/Order#");
    let index = write_index(
        out.path(),
        sites
            .iter()
            .map(|(file, line)| document(file, vec![occurrence(&order, *line, true)]))
            .collect(),
    );

    let graph = graph_with(repo.path(), Some(index)).unwrap();
    for (file, _) in &sites {
        assert_eq!(
            fact_texts(&graph, file, FactKind::ResolvedByIndex),
            vec!["scip-dotnet 0.2.14".to_string()],
            "{file} declares the partial type and must verify against the index"
        );
    }
}

#[test]
fn a_declaration_renamed_in_place_is_caught_as_drift() {
    // A rename moves nothing. Every other declaration in the file still lands
    // exactly where the index put it, so a gate that only checks the names both
    // sides know passes the file — and then answers questions about a symbol
    // that no longer exists, at full `resolved` precision.
    let repo = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    init_repo_with_files(repo.path(), "Alice", &[("src/Billing.cs", BILLING_CS)]);
    let baseline = graph_with(repo.path(), None).unwrap();

    let money = sym("Orbit/Billing/Money#");
    // What the indexer saw before the rename: `Invoice` was called `Bill`, at
    // the very line `Invoice` occupies now.
    let bill = sym("Orbit/Billing/Bill#");
    let total = sym("Orbit/Billing/Invoice#Total().");
    let index = write_index(
        out.path(),
        vec![document(
            "src/Billing.cs",
            vec![
                occurrence(&money, decl_line(&baseline, "Money"), true),
                occurrence(&bill, decl_line(&baseline, "Invoice"), true),
                occurrence(&total, decl_line(&baseline, "Total"), true),
            ],
        )],
    );

    let graph = graph_with(repo.path(), Some(index)).unwrap();

    assert!(
        fact_texts(&graph, "src/Billing.cs", FactKind::ResolvedByIndex).is_empty(),
        "a file whose index names a declaration the tree no longer has must not \
         be marked resolved — that mark is the licence for absence claims (R20)"
    );
    let summary = fact_texts(&graph, ".", FactKind::ResolvedCoverage);
    assert!(
        summary[0].contains("1 drifted (1 declaration renamed in place)"),
        "and the summary must say which disagreement it was: {}",
        summary[0]
    );
}

#[test]
fn a_document_with_nothing_in_common_is_reported_as_unverifiable_not_drifted() {
    // "The index is stale here" and "there was nothing here to check" say
    // opposite things about how far the index may be trusted. Counting both as
    // drift overstates staleness to whoever reads coverage.json.
    let repo = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    init_repo_with_files(repo.path(), "Alice", &[("src/Billing.cs", BILLING_CS)]);
    let baseline = graph_with(repo.path(), None).unwrap();

    // A name the grammar never declares, inside a body rather than on any
    // declaration's own line — so there is nothing at all to agree or disagree
    // about, which is exactly the state this test is about.
    let generated = sym("Orbit/Billing/Invoice#k__BackingField.");
    let index = write_index(
        out.path(),
        vec![document(
            "src/Billing.cs",
            vec![occurrence(
                &generated,
                decl_line(&baseline, "Total") + 2,
                true,
            )],
        )],
    );

    let graph = graph_with(repo.path(), Some(index)).unwrap();
    let summary = fact_texts(&graph, ".", FactKind::ResolvedCoverage);
    assert!(
        summary[0].contains("0 drifted") && summary[0].contains("1 unverifiable"),
        "an unverifiable document must not be reported as staleness: {}",
        summary[0]
    );
}

#[test]
fn a_hostile_tool_name_cannot_write_itself_into_the_coverage_artifact() {
    // The tool label is the one string in the corpus that comes from the index
    // rather than from the repository, and it lands verbatim in coverage.json.
    // A secret pasted there is a secret in an artifact exactly like one pasted
    // into a doc comment, and a newline there breaks the summary sentence.
    use protobuf::Message;
    let repo = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    init_repo_with_files(repo.path(), "Alice", &[("src/Billing.cs", BILLING_CS)]);

    let mut index = scip::types::Index::new();
    let mut info = scip::types::ToolInfo::new();
    info.name = "indexer\npassword = hunter2-from-the-index".to_string();
    info.version = "x".repeat(500);
    index.metadata.mut_or_insert_default().tool_info = Some(info).into();
    index.documents = vec![document("src/Billing.cs", vec![])];
    let path = out.path().join("hostile.scip");
    std::fs::write(&path, index.write_to_bytes().unwrap()).unwrap();

    let graph = graph_with(repo.path(), Some(path)).unwrap();
    let summary = fact_texts(&graph, ".", FactKind::ResolvedCoverage);

    assert!(
        !summary[0].contains("hunter2-from-the-index"),
        "R22: a secret in the index's own metadata reached the artifact: {}",
        summary[0]
    );
    assert!(
        !summary[0].contains('\n'),
        "the summary must stay one line: {:?}",
        summary[0]
    );
    assert!(
        summary[0].len() < 300,
        "and must not be paddable to any length: {} chars",
        summary[0].len()
    );
}

// ---- Pairing usage sites with the index (plan A, U9, KTD8) ----
//
// The heuristic tier emits `Calls`; an index emits only occurrences. These
// tests pair the two at the level of one usage site — `(file, line, column,
// name)` — and check what each pairing does to the site's candidates.

/// The single edge of `kind` from the node whose key contains `from` to the one
/// whose key contains `to`.
fn the_edge<'g>(graph: &'g Graph, kind: EdgeKind, from: &str, to: &str) -> &'g Edge {
    let found: Vec<_> = graph
        .edges_sorted()
        .into_iter()
        .filter(|e| {
            e.edge.kind == kind && e.from.as_str().contains(from) && e.to.as_str().contains(to)
        })
        .collect();
    assert_eq!(
        found.len(),
        1,
        "expected one {kind:?} edge {from} -> {to}, saw {:?}",
        graph
            .edges_sorted()
            .iter()
            .filter(|e| e.edge.kind == kind)
            .map(|e| (e.from.as_str(), e.to.as_str()))
            .collect::<Vec<_>>()
    );
    found[0].edge
}

fn rung(edge: &Edge) -> Option<Confidence> {
    edge.provenance.rung()
}

/// The pairing the index pass wrote into the site record at `(line, column)`.
fn pairing_at(graph: &Graph, file: &str, (line, column): (u32, u32)) -> Option<IndexPairing> {
    let records: Vec<_> = graph
        .sites()
        .records_sorted()
        .into_iter()
        .filter(|r| r.file == file && r.line == line && r.column == Some(column))
        .collect();
    assert_eq!(
        records.len(),
        1,
        "one site record at {file}:{line}:{column}, saw {:?}",
        graph.sites().records_sorted()
    );
    records[0].index.clone()
}

fn node_containing(graph: &Graph, part: &str) -> NodeKey {
    let found: Vec<NodeKey> = graph
        .nodes_sorted()
        .into_iter()
        .filter(|n| n.key.as_str().contains(part))
        .map(|n| n.key.clone())
        .collect();
    assert_eq!(found.len(), 1, "one node containing {part}: {found:?}");
    found[0].clone()
}

/// Two types with a `Run` each, and a call whose receiver says nothing, so the
/// heuristic tier leaves both candidates standing.
const RUN_CS: &str = r#"namespace N
{
    public class A
    {
        public void Run() { }
    }

    public class B
    {
        public void Run() { }
    }

    public class Caller
    {
        public object Make() { return null; }

        public void Go()
        {
            Make().Run();
        }
    }
}
"#;

/// The definitions of [`RUN_CS`], which verify the document, plus the call's
/// occurrence pointing at `target`.
fn run_index(dir: &Path, target: &str) -> PathBuf {
    write_index(
        dir,
        vec![document(
            "src/Run.cs",
            vec![
                occ(RUN_CS, "N/A#", "A", 0, true),
                occ(RUN_CS, "N/A#Run().", "Run(", 0, true),
                occ(RUN_CS, "N/B#", "B", 0, true),
                occ(RUN_CS, "N/B#Run().", "Run(", 1, true),
                occ(RUN_CS, "N/Caller#", "Caller", 0, true),
                occ(RUN_CS, "N/Caller#Make().", "Make(", 0, true),
                occ(RUN_CS, "N/Caller#Go().", "Go(", 0, true),
                occ(RUN_CS, target, "Run(", 2, false),
            ],
        )],
    )
}

#[test]
fn the_candidate_the_index_points_at_is_resolved_and_the_other_stays_maybe_also() {
    let repo = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    init_repo_with_files(repo.path(), "Alice", &[("src/Run.cs", RUN_CS)]);
    let baseline = graph_with(repo.path(), None).unwrap();
    for target in ["A.Run", "B.Run"] {
        assert_eq!(
            rung(the_edge(&baseline, EdgeKind::Calls, "Caller.Go", target)),
            Some(Confidence::Ambiguous),
            "precondition: without an index both `Run`s are only a guess"
        );
    }

    let graph = graph_with(repo.path(), Some(run_index(out.path(), "N/A#Run()."))).unwrap();

    let chosen = the_edge(&graph, EdgeKind::Calls, "Caller.Go", "A.Run");
    assert_eq!(rung(chosen), Some(Confidence::ResolvedByIndex));
    assert_eq!(chosen.provenance.resolution, Resolution::Resolved);
    assert_eq!(
        rung(the_edge(&graph, EdgeKind::Calls, "Caller.Go", "B.Run")),
        Some(Confidence::Ambiguous),
        "the candidate the index did not point at is only a guess"
    );
    assert_eq!(
        pairing_at(&graph, "src/Run.cs", at(RUN_CS, "Run(", 2)),
        Some(IndexPairing::Resolved {
            target: node_containing(&graph, "A.Run")
        })
    );
}

#[test]
fn a_paired_call_does_not_add_a_duplicate_references_edge_beside_its_calls() {
    let repo = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    init_repo_with_files(repo.path(), "Alice", &[("src/Run.cs", RUN_CS)]);

    let graph = graph_with(repo.path(), Some(run_index(out.path(), "N/A#Run()."))).unwrap();

    // The occurrence IS the call: one relation, `Calls`, at the index's
    // precision — not a `Calls` from the grammar and a `References` from the
    // index saying the same thing twice.
    assert_eq!(
        the_edge(&graph, EdgeKind::Calls, "Caller.Go", "A.Run")
            .provenance
            .resolution,
        Resolution::Resolved
    );
    let references: Vec<_> = edges(&graph, EdgeKind::References)
        .into_iter()
        .filter(|(from, to, _)| from.contains("Caller.Go") && to.contains("A.Run"))
        .collect();
    assert!(references.is_empty(), "{references:?}");
}

/// A receiver of a declared repository type backs one `Trim` and disputes the
/// other — so an index verdict that refutes both is visible on the backed one.
const TRIM_CS: &str = r#"namespace N
{
    public class Text
    {
        public string Trim() { return null; }
    }

    public class Other
    {
        public string Trim() { return null; }
    }

    public class Caller
    {
        public void Go(Text t)
        {
            t.Trim();
        }
    }
}
"#;

/// The definitions of [`TRIM_CS`], plus the call bound to `System.String`.
fn trim_index(dir: &Path) -> PathBuf {
    write_index(
        dir,
        vec![document(
            "src/Text.cs",
            vec![
                occ(TRIM_CS, "N/Text#", "Text", 0, true),
                occ(TRIM_CS, "N/Text#Trim().", "Trim(", 0, true),
                occ(TRIM_CS, "N/Other#", "Other", 0, true),
                occ(TRIM_CS, "N/Other#Trim().", "Trim(", 1, true),
                occ(TRIM_CS, "N/Caller#", "Caller", 0, true),
                occ(TRIM_CS, "N/Caller#Go().", "Go(", 0, true),
                // What the compiler actually bound the call to.
                scip_occurrence_at(
                    "scip-dotnet nuget System.Runtime 10.0.0.0 System/String#Trim().",
                    at(TRIM_CS, "Trim(", 2),
                    false,
                ),
            ],
        )],
    )
}

#[test]
fn an_index_pointing_outside_the_repository_leaves_every_candidate_maybe_also() {
    let repo = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    init_repo_with_files(repo.path(), "Alice", &[("src/Text.cs", TRIM_CS)]);
    let baseline = graph_with(repo.path(), None).unwrap();
    assert_eq!(
        rung(the_edge(
            &baseline,
            EdgeKind::Calls,
            "Caller.Go",
            "Text.Trim"
        )),
        Some(Confidence::RulesBacked),
        "precondition: the declared receiver backs `Text.Trim`"
    );

    let graph = graph_with(repo.path(), Some(trim_index(out.path()))).unwrap();

    for target in ["Text.Trim", "Other.Trim"] {
        let edge = the_edge(&graph, EdgeKind::Calls, "Caller.Go", target);
        assert_eq!(rung(edge), Some(Confidence::Ambiguous), "{target}");
        assert_eq!(edge.provenance.resolution, Resolution::Heuristic);
    }
    assert_eq!(
        pairing_at(&graph, "src/Text.cs", at(TRIM_CS, "Trim(", 2)),
        Some(IndexPairing::External)
    );
}

/// One folder built for two frameworks: both projects compile `t.Trim()`
/// into `System.String`, but number its overloads differently, so the merged
/// index keeps both targets at the call and marks the range conflicting.
/// Every project still says the call leaves the repository, and that refutes
/// the repository candidates exactly as one external target would.
#[test]
fn a_call_two_frameworks_send_outside_under_different_overloads_is_external() {
    let repo = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    init_repo_with_files(repo.path(), "Alice", &[("src/Text.cs", TRIM_CS)]);

    let project = |name: &str, call: &str| {
        let mut index = scip::types::Index::new();
        index.documents = vec![document(
            "src/Text.cs",
            vec![
                occ(TRIM_CS, "N/Text#", "Text", 0, true),
                occ(TRIM_CS, "N/Text#Trim().", "Trim(", 0, true),
                occ(TRIM_CS, "N/Other#", "Other", 0, true),
                occ(TRIM_CS, "N/Other#Trim().", "Trim(", 1, true),
                occ(TRIM_CS, "N/Caller#", "Caller", 0, true),
                occ(TRIM_CS, "N/Caller#Go().", "Go(", 0, true),
                scip_occurrence_at(call, at(TRIM_CS, "Trim(", 2), false),
            ],
        )];
        reverse_architect::index::ProjectIndex {
            project: name.to_string(),
            root: ".".to_string(),
            index,
        }
    };
    let merged = reverse_architect::index::merge(
        vec![
            project(
                "X.NetCore.csproj",
                "scip-dotnet nuget System.Runtime 10.0.0.0 System/String#Trim().",
            ),
            project(
                "X.csproj",
                "scip-dotnet nuget mscorlib 4.0.0.0 System/String#Trim(+1).",
            ),
        ],
        &["src/Text.cs".to_string()].into_iter().collect(),
        "file:///repo",
    );
    assert_eq!(
        merged.conflicts.len(),
        1,
        "precondition: the overload numbers differ, so the call is a conflict"
    );
    let path = out.path().join("index.scip");
    {
        use protobuf::Message;
        std::fs::write(&path, merged.index.write_to_bytes().unwrap()).unwrap();
    }

    let graph = graph_with(repo.path(), Some(path)).unwrap();

    assert_eq!(
        pairing_at(&graph, "src/Text.cs", at(TRIM_CS, "Trim(", 2)),
        Some(IndexPairing::External)
    );
    for target in ["Text.Trim", "Other.Trim"] {
        assert_eq!(
            rung(the_edge(&graph, EdgeKind::Calls, "Caller.Go", target)),
            Some(Confidence::Ambiguous),
            "{target}"
        );
    }
}

#[test]
fn an_index_reference_with_no_heuristic_site_stays_as_it_was() {
    let repo = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    // The fixture's one reference sits in `Total`'s body, where the grammar
    // sees no usage of `Money` at all.
    let index = agreeing_fixture(repo.path(), out.path());
    let baseline = graph_with(repo.path(), None).unwrap();
    let line = decl_line(&baseline, "Total") + 1;

    let graph = graph_with(repo.path(), Some(index)).unwrap();

    let edge = the_edge(&graph, EdgeKind::References, "Total", "Money");
    assert_eq!(edge.provenance.resolution, Resolution::Resolved);
    assert_eq!(&*edge.provenance.extractor, "scip");
    assert_eq!(edge.provenance.span.start(), Some(line));
    // The index's own site, by line alone as before, beside the heuristic
    // tier's sites of the same relation (the parameter and return types on
    // the declaration line).
    assert!(
        edge.sites.contains(&SiteId {
            file: "src/Billing.cs".to_string(),
            line,
            column: None,
        }),
        "{:?}",
        edge.sites
    );
}

#[test]
fn a_drifted_document_is_not_paired_and_the_match_rate_says_so() {
    let repo = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    init_repo_with_files(repo.path(), "Alice", &[("src/Run.cs", RUN_CS)]);

    // Every definition forty lines from where the grammar finds it.
    let shifted = |symbol: &str, needle: &str, nth: usize| {
        let (line, column) = at_end(RUN_CS, needle, nth);
        scip_occurrence_at(&sym(symbol), (line + 40, column), true)
    };
    let index = write_index(
        out.path(),
        vec![document(
            "src/Run.cs",
            vec![
                shifted("N/A#", "A", 0),
                shifted("N/A#Run().", "Run(", 0),
                shifted("N/B#", "B", 0),
                shifted("N/B#Run().", "Run(", 1),
                occ(RUN_CS, "N/A#Run().", "Run(", 2, false),
            ],
        )],
    );
    let mut config = common::config_for(repo.path(), out.path());
    config.scip_index = Some(index);
    let generated = reverse_architect::generate(&config).unwrap();

    assert_eq!(
        rung(the_edge(
            &generated.graph,
            EdgeKind::Calls,
            "Caller.Go",
            "A.Run"
        )),
        Some(Confidence::Ambiguous),
        "a drifted document refutes and confirms nothing"
    );
    assert_eq!(
        pairing_at(&generated.graph, "src/Run.cs", at(RUN_CS, "Run(", 2)),
        Some(IndexPairing::UnverifiedDocument)
    );
    let rate = generated
        .coverage
        .noise
        .as_ref()
        .and_then(|n| n.index_match)
        .expect("a run with an index reports its match rate");
    assert!(
        rate.considered >= 2,
        "`Make()` and `Run()` at least: {rate:?}"
    );
    assert_eq!(rate.unverified_document, rate.considered, "{rate:?}");
    assert_eq!(rate.matched, 0);
}

/// Two overloads on separate lines: a moniker's ordinal says nothing about
/// which, the declaration line does.
const OVERLOADS_ON_LINES_CS: &str = r#"namespace N
{
    public class Money { }

    public class Invoice
    {
        public Money Total(Money amount)
        {
            return amount;
        }

        public Money Total(Money amount, Money discount)
        {
            return amount;
        }
    }

    public class Caller
    {
        public void Go(Invoice inv, Money m)
        {
            inv.Total(m);
        }
    }
}
"#;

#[test]
fn overloads_on_separate_lines_are_told_apart_by_their_declaration_line() {
    let src = OVERLOADS_ON_LINES_CS;
    let repo = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    init_repo_with_files(repo.path(), "Alice", &[("src/Billing.cs", src)]);
    let baseline = graph_with(repo.path(), None).unwrap();
    assert_eq!(
        rung(the_edge(
            &baseline,
            EdgeKind::Calls,
            "Caller.Go",
            "Total(Money)"
        )),
        Some(Confidence::RulesBacked),
        "precondition: one argument backs the one-parameter overload"
    );

    // The compiler bound the call to the SECOND overload — contrived against
    // the argument count, and exactly what shows the index is read by line and
    // wins over the rules.
    let index = write_index(
        out.path(),
        vec![document(
            "src/Billing.cs",
            vec![
                occ(src, "N/Money#", "Money {", 0, true),
                occ(src, "N/Invoice#", "Invoice", 0, true),
                occ(src, "N/Invoice#Total().", "Total(", 0, true),
                occ(src, "N/Invoice#Total(+1).", "Total(", 1, true),
                occ(src, "N/Caller#", "Caller", 0, true),
                occ(src, "N/Caller#Go().", "Go(", 0, true),
                occ(src, "N/Invoice#Total(+1).", "Total(", 2, false),
            ],
        )],
    );
    let graph = graph_with(repo.path(), Some(index)).unwrap();

    assert_eq!(
        rung(the_edge(
            &graph,
            EdgeKind::Calls,
            "Caller.Go",
            "Total(Money,Money)"
        )),
        Some(Confidence::ResolvedByIndex)
    );
    assert_eq!(
        rung(the_edge(
            &graph,
            EdgeKind::Calls,
            "Caller.Go",
            "Total(Money)"
        )),
        Some(Confidence::Ambiguous),
        "the overload the rules backed and the index refuted is only a guess"
    );
}

/// Two overloads declared on ONE line: only the column of each name tells
/// them apart.
const ONE_LINE_CS: &str = r#"namespace N
{
    public class Invoice
    {
        public int Total(int a) { return a; } public int Total(int a, int b) { return a; }
    }

    public class Caller
    {
        public void Go(Invoice inv)
        {
            inv.Total(1);
        }
    }
}
"#;

#[test]
fn overloads_declared_on_one_line_are_told_apart_by_the_column_of_their_names() {
    let src = ONE_LINE_CS;
    let repo = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    init_repo_with_files(repo.path(), "Alice", &[("src/Invoice.cs", src)]);

    let index = write_index(
        out.path(),
        vec![document(
            "src/Invoice.cs",
            vec![
                occ(src, "N/Invoice#", "Invoice", 0, true),
                occ(src, "N/Invoice#Total().", "Total(", 0, true),
                occ(src, "N/Invoice#Total(+1).", "Total(", 1, true),
                occ(src, "N/Caller#", "Caller", 0, true),
                occ(src, "N/Caller#Go().", "Go(", 0, true),
                occ(src, "N/Invoice#Total(+1).", "Total(", 2, false),
            ],
        )],
    );
    let graph = graph_with(repo.path(), Some(index)).unwrap();

    let one = node_containing(&graph, "Total(int)");
    let two = node_containing(&graph, "Total(int,int)");
    assert_eq!(
        fact_texts(&graph, one.as_str(), FactKind::SymbolMoniker),
        vec![sym("N/Invoice#Total().")]
    );
    assert_eq!(
        fact_texts(&graph, two.as_str(), FactKind::SymbolMoniker),
        vec![sym("N/Invoice#Total(+1).")]
    );
    assert_eq!(
        pairing_at(&graph, "src/Invoice.cs", at(src, "Total(", 2)),
        Some(IndexPairing::Resolved { target: two })
    );
    assert_eq!(
        rung(the_edge(
            &graph,
            EdgeKind::Calls,
            "Caller.Go",
            "Total(int,int)"
        )),
        Some(Confidence::ResolvedByIndex)
    );
}

/// Two calls of one name on one line, each on a different type.
const APPEND_CS: &str = r#"namespace N
{
    public class A
    {
        public void Append(int x) { }
    }

    public class B
    {
        public void Append(int x) { }
    }

    public class Caller
    {
        public void Go(A a, B b)
        {
            a.Append(1); b.Append(2);
        }
    }
}
"#;

#[test]
fn two_calls_of_one_name_on_one_line_pair_by_column_with_their_own_sites() {
    let src = APPEND_CS;
    let repo = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    init_repo_with_files(repo.path(), "Alice", &[("src/Append.cs", src)]);

    let index = write_index(
        out.path(),
        vec![document(
            "src/Append.cs",
            vec![
                occ(src, "N/A#", "A", 0, true),
                occ(src, "N/A#Append().", "Append(", 0, true),
                occ(src, "N/B#", "B", 0, true),
                occ(src, "N/B#Append().", "Append(", 1, true),
                occ(src, "N/Caller#", "Caller", 0, true),
                occ(src, "N/Caller#Go().", "Go(", 0, true),
                occ(src, "N/A#Append().", "Append(", 2, false),
                occ(src, "N/B#Append().", "Append(", 3, false),
            ],
        )],
    );
    let graph = graph_with(repo.path(), Some(index)).unwrap();

    let first = at(src, "Append(", 2);
    let second = at(src, "Append(", 3);
    assert_eq!(first.0, second.0, "precondition: one line");
    assert_eq!(
        pairing_at(&graph, "src/Append.cs", first),
        Some(IndexPairing::Resolved {
            target: node_containing(&graph, "A.Append")
        })
    );
    assert_eq!(
        pairing_at(&graph, "src/Append.cs", second),
        Some(IndexPairing::Resolved {
            target: node_containing(&graph, "B.Append")
        })
    );
    for target in ["A.Append", "B.Append"] {
        assert_eq!(
            rung(the_edge(&graph, EdgeKind::Calls, "Caller.Go", target)),
            Some(Confidence::ResolvedByIndex),
            "{target}"
        );
    }
}

/// Two calls of one name on one line after six characters outside the Basic
/// Multilingual Plane: each is twelve bytes and two UTF-16 units further than
/// its character count says.
const ASTRAL_CS: &str = "namespace N
{
    public class A
    {
        public void F() { }
    }

    public class B
    {
        public void F() { }
    }

    public class Caller
    {
        public void Go(A a, B b)
        {
/*😀😀😀😀😀😀*/a.F();b.F();
        }
    }
}
";

/// [`at`] in UTF-16 code units, as `scip-dotnet` counts a column.
fn at_utf16(src: &str, needle: &str, nth: usize) -> (u32, u32) {
    let (line, column) = at(src, needle, nth);
    let text = src.lines().nth(line as usize - 1).unwrap();
    let before: String = text.chars().take(column as usize - 1).collect();
    (line, before.encode_utf16().count() as u32 + 1)
}

/// The index counts the calls' columns in UTF-16 units, the plugin in
/// characters. Compared raw, the second call's column in characters is the
/// first call's in UTF-16 units, and the second site would take the first
/// call's target at the index's precision.
#[test]
fn two_calls_after_astral_characters_pair_with_their_own_occurrences() {
    let src = ASTRAL_CS;
    let repo = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    init_repo_with_files(repo.path(), "Alice", &[("src/Astral.cs", src)]);
    let first = at(src, "F(", 2);
    let second = at(src, "F(", 3);
    assert_eq!(
        at_utf16(src, "F(", 2).1,
        second.1,
        "precondition: the first call's UTF-16 column is the second call's character column"
    );

    let index = write_index(
        out.path(),
        vec![document(
            "src/Astral.cs",
            vec![
                occ(src, "N/A#", "A", 0, true),
                occ(src, "N/A#F().", "F(", 0, true),
                occ(src, "N/B#", "B", 0, true),
                occ(src, "N/B#F().", "F(", 1, true),
                occ(src, "N/Caller#", "Caller", 0, true),
                occ(src, "N/Caller#Go().", "Go(", 0, true),
                scip_occurrence_at(&sym("N/A#F()."), at_utf16(src, "F(", 2), false),
                scip_occurrence_at(&sym("N/B#F()."), at_utf16(src, "F(", 3), false),
            ],
        )],
    );
    let graph = graph_with(repo.path(), Some(index)).unwrap();

    assert_eq!(
        pairing_at(&graph, "src/Astral.cs", second),
        Some(IndexPairing::Resolved {
            target: node_containing(&graph, "B.F")
        })
    );
    assert_eq!(
        pairing_at(&graph, "src/Astral.cs", first),
        Some(IndexPairing::Resolved {
            target: node_containing(&graph, "A.F")
        })
    );
}

#[test]
fn a_call_into_another_project_pairs_with_its_definition_in_the_merged_index() {
    const CALLER_CS: &str = "namespace A;\npublic class Caller\n{\n    public string Run()\n    {\n        var g = new B.Greeter();\n        return g.Hello(\"x\");\n    }\n}\n";
    const GREETER_CS: &str = "namespace B;\npublic class Greeter\n{\n    public string Hello(string name) => \"Hello \" + name;\n}\n";
    let repo = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    init_repo_with_files(
        repo.path(),
        "Alice",
        &[("A/Class1.cs", CALLER_CS), ("B/Class1.cs", GREETER_CS)],
    );

    // Each project indexed on its own, its paths relative to its own directory,
    // then merged by `ra index`'s own merge. The call carries the symbol of the
    // definition in the other project, as measured on scip-dotnet (U10).
    let project = |name: &str, documents| {
        let mut index = scip::types::Index::new();
        index.documents = documents;
        reverse_architect::index::ProjectIndex {
            project: format!("{name}/{name}.csproj"),
            root: name.to_string(),
            index,
        }
    };
    let a = project(
        "A",
        vec![document(
            "Class1.cs",
            vec![
                occ(CALLER_CS, "A/Caller#", "Caller", 0, true),
                occ(CALLER_CS, "A/Caller#Run().", "Run(", 0, true),
                occ(CALLER_CS, "B/Greeter#Hello().", "Hello(", 0, false),
            ],
        )],
    );
    let b = project(
        "B",
        vec![document(
            "Class1.cs",
            vec![
                occ(GREETER_CS, "B/Greeter#", "Greeter", 0, true),
                occ(GREETER_CS, "B/Greeter#Hello().", "Hello(", 0, true),
            ],
        )],
    );
    let tracked = ["A/Class1.cs".to_string(), "B/Class1.cs".to_string()]
        .into_iter()
        .collect();
    let merged = reverse_architect::index::merge(vec![a, b], &tracked, "file:///repo");
    let path = out.path().join("index.scip");
    {
        use protobuf::Message;
        std::fs::write(&path, merged.index.write_to_bytes().unwrap()).unwrap();
    }

    let graph = graph_with(repo.path(), Some(path)).unwrap();

    assert_eq!(
        pairing_at(&graph, "A/Class1.cs", at(CALLER_CS, "Hello(", 0)),
        Some(IndexPairing::Resolved {
            target: node_containing(&graph, "Greeter.Hello")
        }),
        "a definition in another project of the repository is not outside it"
    );
    assert_eq!(
        rung(the_edge(
            &graph,
            EdgeKind::Calls,
            "Caller.Run",
            "Greeter.Hello"
        )),
        Some(Confidence::ResolvedByIndex)
    );
}

/// The rendered detail of `Caller` of a run over `repo` with `index`, in
/// `lang` — where its "maybe also" claims are since the short card (plan B,
/// U9).
fn caller_detail(repo: &Path, index: PathBuf, lang: &str) -> String {
    let out = tempfile::tempdir().unwrap();
    let mut config = common::config_for(repo, out.path());
    config.scip_index = Some(index);
    config.lang = lang.to_string();
    let generated = reverse_architect::generate(&config).unwrap();
    generated
        .details
        .iter()
        .find(|f| f.path.ends_with("/Caller.md"))
        .unwrap_or_else(|| panic!("no Caller detail"))
        .contents
        .clone()
}

/// The `## Maybe also` line naming `target`.
fn maybe_also_line<'t>(card: &'t str, target: &str) -> &'t str {
    card.lines()
        .find(|l| l.starts_with("- ") && l.contains(target) && l.contains("ambiguous"))
        .unwrap_or_else(|| panic!("no Maybe also line for {target}:\n{card}"))
}

/// A candidate the index refuted says so in "maybe also" (R7: with its
/// reason) — not the shared name or the rule that rated it before.
#[test]
fn a_candidate_the_index_refuted_gives_the_index_as_its_reason() {
    let repo = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    init_repo_with_files(repo.path(), "Alice", &[("src/Run.cs", RUN_CS)]);
    let index = run_index(out.path(), "N/A#Run().");

    let graph = graph_with(repo.path(), Some(index.clone())).unwrap();
    assert_eq!(
        the_edge(&graph, EdgeKind::Calls, "Caller.Go", "B.Run")
            .provenance
            .rule,
        Some(Rule::IndexElsewhere)
    );
    // The site record's trace is the resolver's, untouched: no rule there
    // names the index.
    let records = graph.sites().records_sorted();
    assert!(records
        .iter()
        .flat_map(|r| &r.candidates)
        .flat_map(|c| &c.trace)
        .all(|v| !matches!(v.rule, Rule::IndexElsewhere | Rule::IndexOutside)));

    let card = caller_detail(repo.path(), index.clone(), "en");
    let line = maybe_also_line(&card, "`B.Run()`");
    assert!(
        line.contains("(heuristic, ambiguous: the index points elsewhere)"),
        "{line}"
    );
    assert!(
        !card
            .lines()
            .any(|l| l.contains("`A.Run()`") && l.contains("ambiguous")),
        "the confirmed candidate is no guess:\n{card}"
    );
    let card = caller_detail(repo.path(), index, "cs");
    assert!(
        maybe_also_line(&card, "`B.Run()`").contains("index ukazuje jinam"),
        "{card}"
    );
}

#[test]
fn candidates_refuted_by_an_external_symbol_give_the_outside_as_their_reason() {
    let repo = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    init_repo_with_files(repo.path(), "Alice", &[("src/Text.cs", TRIM_CS)]);
    let index = trim_index(out.path());

    let card = caller_detail(repo.path(), index.clone(), "en");
    for target in ["`Text.Trim()`", "`Other.Trim()`"] {
        let line = maybe_also_line(&card, target);
        assert!(
            line.contains("ambiguous: the index points outside the repository)"),
            "{line}"
        );
    }
    let card = caller_detail(repo.path(), index, "cs");
    assert!(
        maybe_also_line(&card, "`Text.Trim()`").contains("index ukazuje mimo repozitář"),
        "{card}"
    );
}

#[test]
fn the_same_index_twice_gives_the_same_bytes_and_names_itself() {
    use sha2::Digest;
    let repo = tempfile::tempdir().unwrap();
    let index_dir = tempfile::tempdir().unwrap();
    init_repo_with_files(repo.path(), "Alice", &[("src/Run.cs", RUN_CS)]);
    let index = run_index(index_dir.path(), "N/A#Run().");

    // Through the CLI, because only it writes the whole corpus — `sites.jsonl`
    // with each site's pairing included.
    let outs = tempfile::tempdir().unwrap();
    let (first, second) = (outs.path().join("first"), outs.path().join("second"));
    let ra = |out: &Path| {
        assert_cmd::Command::cargo_bin("reverse-architect")
            .unwrap()
            .arg(repo.path())
            .arg("--out")
            .arg(out)
            .arg("--scip-index")
            .arg(&index)
            .arg("--sites")
            .assert()
            .success();
    };
    ra(&first);
    ra(&second);
    let sites = std::fs::read_to_string(first.join("sites.jsonl")).unwrap();
    assert!(sites.contains("\"pairing\":\"resolved\""), "{sites}");
    let cold = common::hash_dir(&first);
    assert_eq!(
        cold,
        common::hash_dir(&second),
        "one index, one commit: one set of bytes"
    );
    // And over the first directory's warm cache, where the site records and
    // the declarations' name positions the pairing reads come back out of the
    // cache rather than out of the walk.
    ra(&first);
    assert_eq!(cold, common::hash_dir(&first), "a cold and a warm run");

    let coverage: reverse_architect::coverage::Coverage =
        serde_json::from_slice(&std::fs::read(first.join("coverage.json")).unwrap()).unwrap();
    let expected = format!("{:x}", sha2::Sha256::digest(std::fs::read(&index).unwrap()));
    assert_eq!(
        coverage.resolved_index.and_then(|r| r.sha256),
        Some(expected),
        "coverage.json names the index it paired against"
    );
    let rate = coverage.noise.and_then(|n| n.index_match).unwrap();
    assert_eq!(rate.matched, 1, "{rate:?}");
    assert_eq!(
        rate.considered,
        rate.matched + rate.external + rate.unverified_document + rate.unmapped
    );
}
