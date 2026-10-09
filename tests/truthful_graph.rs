//! The truthful-graph regression fixture (plan A, U8: R20, R21, R22; AE1-AE3,
//! AE6, AE7).
//!
//! A synthetic C# repository shaped like `GeoSolver` from the Problem Frame:
//! the solver makes the five BCL calls whose names collide with declarations
//! of this repository — `string.IsNullOrEmpty`, `StringBuilder.Append`, LINQ
//! `Max` with one argument, `List<>`, `string.IndexOfAny` — and each collision
//! has its repository twin in a namespace of its own (`OctetString`,
//! `LedgerItem`, `FunctionEvaluator` with four parameters, `McpToolCatalog`'s
//! `List` method, an AvalonEdit-like `TextDocument`). None of the five may
//! stand in `## Relations` or `## Referenced by`; where a rule only disputed
//! one, it may sit in `## Maybe also`. Since plan B, U9 those three sections
//! are a type's detail, and its short card names no guess at all.
//!
//! Beside them, what must NOT be lost: the solver's real foreign caller
//! (`cleanup_jobs.CleanServer`), a cross-type call on a receiver nothing is
//! known about, and a call through an interface that dynamic dispatch carries
//! to its implementation — the last one even with a right to delete granted to
//! `declared-receiver`.
//!
//! Not part of the six-grammar fixture: that one's header forbids deliberate
//! name collisions, and this one is nothing but.

mod common;

use common::{declared_receiver_right, find, hash_dir, init_repo_with_files};
use reverse_architect::card::detail::render_detail;
use reverse_architect::card::links::CardIndex;
use reverse_architect::card::render::{render_card, Lang};
use reverse_architect::card::{build_cards, Card, CardKind, MaybeReason};
use reverse_architect::cli::LinkStyle;
use reverse_architect::extract::RightsTable;
use reverse_architect::graph::{Confidence, EdgeKind, Rule};
use reverse_architect::{generate, Generated};
use std::path::Path;

/// `string.IsNullOrEmpty`'s twin: an SNMP library's octet string.
const OCTET_STRING_CS: &str = r#"namespace Lextm.SharpSnmpLib
{
    public sealed class OctetString
    {
        public static bool IsNullOrEmpty(OctetString value) { return value == null; }
    }
}
"#;

/// `StringBuilder.Append`'s twin — the only `Append` in the repository, so
/// a call to it is the disputed SINGLE candidate of AE2.
const KAL_KAP_ITEM_CS: &str = r#"namespace Orbit.Kalkulace
{
    public class LedgerItem
    {
        public void Append(string text) { }
    }
}
"#;

/// LINQ `Max`'s twin, with four parameters where the LINQ call passes one.
const FUNCTION_EVALUATOR_CS: &str = r#"namespace Orbit.Vzorce
{
    public static class FunctionEvaluator
    {
        public static double Max(double a, double b, double c, double d) { return a; }
    }
}
"#;

/// `List<>`'s twin: a catalog's `List` operation, a method.
const MCP_TOOL_CATALOG_CS: &str = r#"namespace Orbit.Mcp
{
    public class McpToolCatalog
    {
        public string[] List() { return null; }
    }
}
"#;

/// `string.IndexOfAny`'s twin, shaped like AvalonEdit's document.
const TEXT_DOCUMENT_CS: &str = r#"namespace ICSharpCode.AvalonEdit.Document
{
    public sealed class TextDocument
    {
        public int IndexOfAny(char[] anyOf, int startIndex, int count) { return -1; }
    }
}
"#;

/// The solver. `CleanTyp` makes three of the five BCL calls and calls its
/// sibling `FoundTables` (AE7); `Nejvetsi` constructs a `List<int>`, declares a
/// `List<string>` and takes LINQ's one-argument `Max`; `Hledej` calls
/// `IndexOfAny` on a `string` parameter. `Spolupracuj` holds the two negative
/// controls: `pomocnik`, a local initialized from a factory call — nothing is
/// known about its type, so its `Spocitej()` is a name match and nothing
/// more — and `zdroj`, declared as the interface `ITableSource`, whose
/// `Load()` dynamic dispatch carries to `DbTableSource.Load`.
const GEO_SOLVER_CS: &str = r#"using System.Collections.Generic;
using System.Linq;
using System.Text;

namespace Orbit.Geo
{
    public class GeoSolver
    {
        public void CleanTyp(StringBuilder result, string x)
        {
            if (string.IsNullOrEmpty(x)) { return; }
            result.Append(x);
            FoundTables();
        }

        public int Nejvetsi()
        {
            var list = new List<int>();
            List<string> names = new List<string>();
            return list.Max(v => v);
        }

        public int Hledej(string text, char[] chars)
        {
            return text.IndexOfAny(chars);
        }

        public void Spolupracuj(ITableSource zdroj)
        {
            var pomocnik = PomocnikFactory.Vytvor();
            pomocnik.Spocitej();
            zdroj.Load();
        }

        public void FoundTables() { }

        public int Solve(int n) { return n; }

        public int Solve(string s) { return 0; }
    }
}
"#;

/// The negative controls' targets, in the solver's own namespace.
const HELPERS_CS: &str = r#"namespace Orbit.Geo
{
    public class Pomocnik
    {
        public void Spocitej() { }
    }

    public static class PomocnikFactory
    {
        public static object Vytvor() { return null; }
    }

    public interface ITableSource
    {
        void Load();
    }

    public class DbTableSource : ITableSource
    {
        public void Load() { }
    }
}
"#;

/// The solver's real foreign caller (AE7): `FoundTables` from two lines — one
/// relation, two sites — and a bare `Solve(1)` that the solver's two overloads
/// both answer (AE3). It imports the solver's namespace, as valid C# must.
const CLEAN_SERVER_CS: &str = r#"using Orbit.Geo;

namespace Orbit.Server
{
    public class cleanup_jobs
    {
        public void CleanServer(GeoSolver solver)
        {
            solver.FoundTables();
            solver.FoundTables();
            Solve(1);
        }
    }
}
"#;

const FILES: &[(&str, &str)] = &[
    ("Snmp/OctetString.cs", OCTET_STRING_CS),
    ("Kalkulace/LedgerItem.cs", KAL_KAP_ITEM_CS),
    ("Vzorce/FunctionEvaluator.cs", FUNCTION_EVALUATOR_CS),
    ("Mcp/McpToolCatalog.cs", MCP_TOOL_CATALOG_CS),
    ("Editor/TextDocument.cs", TEXT_DOCUMENT_CS),
    ("Geo/GeoSolver.cs", GEO_SOLVER_CS),
    ("Geo/Helpers.cs", HELPERS_CS),
    ("Server/CleanServer.cs", CLEAN_SERVER_CS),
];

/// The five false edges of the Problem Frame, by the target's identity as the
/// C# plugin spells it.
const APPEND: &str = "sym:csharp:Orbit.Kalkulace#LedgerItem.Append(string)";
const MAX: &str = "sym:csharp:Orbit.Vzorce#FunctionEvaluator.Max(double,double,double,double)";
const INDEX_OF_ANY: &str =
    "sym:csharp:ICSharpCode.AvalonEdit.Document#TextDocument.IndexOfAny(char[],int,int)";
/// Every type a false edge would lead into — a relation whose identity starts
/// with one of these is one of the five, whichever member it names.
const COLLIDING_TYPES: &[&str] = &[
    "sym:csharp:Lextm.SharpSnmpLib#OctetString",
    "sym:csharp:Orbit.Kalkulace#LedgerItem",
    "sym:csharp:Orbit.Vzorce#FunctionEvaluator",
    "sym:csharp:Orbit.Mcp#McpToolCatalog",
    "sym:csharp:ICSharpCode.AvalonEdit.Document#TextDocument",
];

const SOLVER: &str = "sym:csharp:Orbit.Geo#GeoSolver";
const SPOCITEJ: &str = "sym:csharp:Orbit.Geo#Pomocnik.Spocitej()";
const DB_LOAD: &str = "sym:csharp:Orbit.Geo#DbTableSource.Load()";
const FOREIGN_CALLER: &str = "sym:csharp:Orbit.Server#cleanup_jobs.CleanServer(GeoSolver)";

fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    init_repo_with_files(dir.path(), "Alice", FILES);
    dir
}

fn run(repo: &Path, out: &Path, rights: RightsTable) -> Generated {
    let mut config = common::config_for(repo, out);
    config.rights = rights;
    config.sites = true;
    generate(&config).expect("the pipeline runs")
}

/// The relations a usage site can produce — what a false edge would be. A
/// declaration's `Declares` and `HasMember` edges are structure, not usage.
fn is_usage(kind: EdgeKind) -> bool {
    matches!(
        kind,
        EdgeKind::Calls | EdgeKind::Instantiates | EdgeKind::References
    )
}

/// Every usage edge into `target`, as (source, kind, rung, rule).
fn edges_into(
    g: &Generated,
    target: &str,
) -> Vec<(String, EdgeKind, Option<Confidence>, Option<Rule>)> {
    g.graph
        .edges_sorted()
        .into_iter()
        .filter(|e| e.to.as_str() == target && is_usage(e.edge.kind))
        .map(|e| {
            (
                e.from.as_str().to_string(),
                e.edge.kind,
                e.edge.provenance.rung(),
                e.edge.provenance.rule,
            )
        })
        .collect()
}

fn is_colliding(identity: &str) -> bool {
    COLLIDING_TYPES
        .iter()
        .any(|t| identity == *t || identity.starts_with(&format!("{t}.")))
}

/// Every alternative any `## Maybe also` claim of `card` names.
fn maybe_also_targets(card: &Card) -> Vec<&str> {
    card.maybe_also
        .iter()
        .flat_map(|c| c.alternatives.iter().map(|a| a.identity.as_str()))
        .collect()
}

/// The rendered detail whose path ends in `path_suffix`: since plan B, U9 a
/// type's `## Relations`, `## Referenced by` and `## Maybe also` are there,
/// not on its short card.
fn rendered_detail<'g>(g: &'g Generated, path_suffix: &str) -> &'g str {
    &g.details
        .iter()
        .find(|f| f.path.ends_with(path_suffix))
        .unwrap_or_else(|| panic!("no rendered detail ending {path_suffix}"))
        .contents
}

/// AE1, AE2 and the other three of the Problem Frame: no card in the corpus
/// holds a main-list row into a colliding type — not the solver's
/// `## Relations`, not the colliding type's own `## Referenced by` — and
/// `string.IsNullOrEmpty` produces no edge at all, not even a guess: a call on
/// a language keyword never reaches the repository (R5).
#[test]
fn no_false_edge_reaches_a_main_list() {
    let repo = repo();
    let out = tempfile::tempdir().unwrap();
    let g = run(repo.path(), out.path(), RightsTable::granted());
    let cards = build_cards(&g.graph);

    // AE1: nothing at all into `OctetString`, in the graph or on a card.
    let octet: Vec<_> = g
        .graph
        .edges_sorted()
        .into_iter()
        // The octet string's own method names its type as a parameter; that is
        // the type's inside, not a usage from elsewhere.
        .filter(|e| {
            e.to.as_str().starts_with(COLLIDING_TYPES[0])
                && !e.from.as_str().starts_with(COLLIDING_TYPES[0])
                && is_usage(e.edge.kind)
        })
        .map(|e| e.from.as_str().to_string())
        .collect();
    assert!(octet.is_empty(), "AE1: edges into OctetString: {octet:?}");

    for card in &cards {
        for r in card.outgoing.iter().chain(&card.incoming) {
            // A colliding type's card lists its own members and declaring
            // file; what must not be there is a row to or from the solver.
            let solver_to_colliding = card.identity == SOLVER && is_colliding(&r.identity);
            let colliding_from_solver =
                is_colliding(&card.identity) && r.identity.starts_with(SOLVER);
            assert!(
                !(solver_to_colliding || colliding_from_solver),
                "a false edge in a main list of {}: {} {} ({:?})",
                card.identity,
                r.kind_word,
                r.identity,
                r.rung
            );
        }
        assert!(
            !maybe_also_targets(card)
                .iter()
                .any(|t| t.contains("OctetString")),
            "AE1: OctetString may not be a guess either: {}",
            card.identity
        );
    }

    // `List<>`: the only `List` in the repository is a method, and neither a
    // construction nor a type position may lead to a method — no edge at all.
    let list = edges_into(&g, "sym:csharp:Orbit.Mcp#McpToolCatalog.List()");
    assert!(list.is_empty(), "List<> leads to the catalog: {list:?}");
}

/// AE2 and the two other disputed single candidates: `result.Append` on a
/// `StringBuilder`, LINQ `Max` on a `List<int>`, `IndexOfAny` on a `string` —
/// each in the graph as ambiguous, disputed by `declared-receiver`, and on the
/// solver's card as a `## Maybe also` claim, never as a row.
#[test]
fn a_disputed_single_candidate_is_a_maybe_also_claim() {
    let repo = repo();
    let out = tempfile::tempdir().unwrap();
    let g = run(repo.path(), out.path(), RightsTable::granted());
    let cards = build_cards(&g.graph);
    let solver = find(&cards, CardKind::Type, "GeoSolver");

    for target in [APPEND, MAX, INDEX_OF_ANY] {
        let edges = edges_into(&g, target);
        let calls: Vec<_> = edges
            .iter()
            .filter(|(_, kind, _, _)| *kind == EdgeKind::Calls)
            .collect();
        assert_eq!(calls.len(), 1, "{target}: {edges:?}");
        assert_eq!(
            (calls[0].2, calls[0].3),
            (Some(Confidence::Ambiguous), Some(Rule::DeclaredReceiver)),
            "{target}"
        );
        assert!(
            calls[0].0.starts_with(&format!("{SOLVER}.")),
            "{target} is called from the solver: {calls:?}"
        );
        assert!(
            maybe_also_targets(solver).contains(&target),
            "{target} is a Maybe also claim of the solver: {:#?}",
            solver.maybe_also
        );
    }
}

/// AE3: the bare `Solve(1)` both overloads answer is ONE claim, on the solver's
/// card and on the caller's, naming both and counting two.
#[test]
fn a_bare_call_to_two_overloads_is_one_claim() {
    let repo = repo();
    let out = tempfile::tempdir().unwrap();
    let g = run(repo.path(), out.path(), RightsTable::granted());
    let cards = build_cards(&g.graph);

    for title in ["GeoSolver", "cleanup_jobs"] {
        let card = find(&cards, CardKind::Type, title);
        let solve: Vec<_> = card
            .maybe_also
            .iter()
            .filter(|c| c.alternatives.iter().any(|a| a.label.contains("Solve")))
            .collect();
        assert_eq!(solve.len(), 1, "{title}: {:#?}", card.maybe_also);
        let labels: Vec<&str> = solve[0]
            .alternatives
            .iter()
            .map(|a| a.label.as_str())
            .collect();
        assert_eq!(
            labels,
            ["GeoSolver.Solve(int)", "GeoSolver.Solve(string)"],
            "{title}"
        );
        assert_eq!(solve[0].candidates, 2, "{title}");
        assert_eq!(solve[0].source.identity, FOREIGN_CALLER, "{title}");
    }
}

/// AE7 (R17, R18): the solver's `## Referenced by` names its foreign caller's
/// call once — one relation however many sites — and no call between the
/// solver's own members, in either direction.
#[test]
fn referenced_by_holds_the_foreign_caller_once_and_no_own_member() {
    let repo = repo();
    let out = tempfile::tempdir().unwrap();
    let g = run(repo.path(), out.path(), RightsTable::granted());
    let cards = build_cards(&g.graph);
    let solver = find(&cards, CardKind::Type, "GeoSolver");

    let calls: Vec<_> = solver
        .incoming
        .iter()
        .filter(|r| r.kind == EdgeKind::Calls)
        .collect();
    assert_eq!(calls.len(), 1, "{calls:#?}");
    assert_eq!(calls[0].identity, FOREIGN_CALLER);
    assert_eq!(calls[0].via.as_deref(), Some("FoundTables()"));
    assert_eq!(calls[0].sites.len(), 2, "two lines, one relation");
    assert_eq!(calls[0].rung, Some(Confidence::RulesBacked));

    for r in solver.outgoing.iter().chain(&solver.incoming) {
        assert!(
            !(r.identity.starts_with(&format!("{SOLVER}.")) && r.kind == EdgeKind::Calls),
            "a call between the solver's own members is listed: {} {}",
            r.kind_word,
            r.identity
        );
    }
    // …while the graph still has it: the card leaves it out, nothing deleted it.
    let own = edges_into(&g, &format!("{SOLVER}.FoundTables()"));
    assert!(
        own.iter().any(|(from, kind, _, _)| from
            == &format!("{SOLVER}.CleanTyp(StringBuilder,string)")
            && *kind == EdgeKind::Calls),
        "{own:?}"
    );
}

/// Negative controls, without a right: a cross-type call on a receiver
/// nothing is known about stays — the one candidate, by name only, in the
/// solver's `## Relations` — and the interface call reaches the
/// implementation in `## Maybe also`.
#[test]
fn real_relations_without_receiver_facts_are_not_lost() {
    let repo = repo();
    let out = tempfile::tempdir().unwrap();
    let g = run(repo.path(), out.path(), RightsTable::granted());
    let cards = build_cards(&g.graph);
    let solver = find(&cards, CardKind::Type, "GeoSolver");

    let spocitej: Vec<_> = solver
        .outgoing
        .iter()
        .filter(|r| r.identity == SPOCITEJ)
        .collect();
    assert_eq!(spocitej.len(), 1, "{:#?}", solver.outgoing);
    assert_eq!(spocitej[0].rung, Some(Confidence::NameOnly));

    assert!(
        maybe_also_targets(solver).contains(&DB_LOAD),
        "the implementation is a Maybe also claim: {:#?}",
        solver.maybe_also
    );
}

/// With `declared-receiver` holding a right to delete: the three disputed
/// candidates of an external or keyword declared type — `LedgerItem.Append`
/// (AE2), `FunctionEvaluator.Max`, `TextDocument.IndexOfAny` — are gone from
/// the graph and every card, and `coverage.json` counts exactly them. The
/// interface call still reaches its implementation: the rule never disputes
/// what dynamic dispatch reaches. The foreign caller and the receiver-less
/// call are untouched.
#[test]
fn a_granted_right_deletes_the_disputed_and_keeps_the_dispatched() {
    let repo = repo();
    let out = tempfile::tempdir().unwrap();
    let g = run(repo.path(), out.path(), declared_receiver_right());
    let cards = build_cards(&g.graph);

    let noise = g.coverage.noise.as_ref().expect("v5 noise");
    assert_eq!(
        noise.deleted_by_rule.get("declared-receiver"),
        Some(&3),
        "Append, Max, IndexOfAny: {:?}",
        noise.deleted_by_rule
    );
    for target in [APPEND, MAX, INDEX_OF_ANY] {
        assert_eq!(edges_into(&g, target), [], "{target}");
        for card in &cards {
            assert!(
                !maybe_also_targets(card).contains(&target)
                    && !card
                        .outgoing
                        .iter()
                        .chain(&card.incoming)
                        .any(|r| r.identity == target),
                "{target} survives on {}",
                card.identity
            );
        }
    }

    let load = edges_into(&g, DB_LOAD);
    assert_eq!(
        load.iter()
            .filter(|(_, kind, _, _)| *kind == EdgeKind::Calls)
            .count(),
        1,
        "{load:?}"
    );
    let solver = find(&cards, CardKind::Type, "GeoSolver");
    assert!(maybe_also_targets(solver).contains(&DB_LOAD));
    assert!(solver.outgoing.iter().any(|r| r.identity == SPOCITEJ));
    assert!(solver
        .incoming
        .iter()
        .any(|r| r.identity == FOREIGN_CALLER && r.kind == EdgeKind::Calls));
}

/// AE6 (R22): two runs of the binary over one commit, each into a fresh
/// directory, write the same bytes — cards, views, manifest, coverage, site
/// records and cache alike — and a third run over the first directory's warm
/// cache writes them again. Through the CLI, because only it writes the whole
/// corpus; `generate` hands it back in memory.
#[test]
fn two_runs_write_byte_identical_corpora() {
    let repo = repo();
    let outs = tempfile::tempdir().unwrap();
    let (first, second) = (outs.path().join("first"), outs.path().join("second"));
    let ra = |out: &Path| {
        assert_cmd::Command::cargo_bin("reverse-architect")
            .unwrap()
            .arg(repo.path())
            .arg("--out")
            .arg(out)
            .arg("--sites")
            .assert()
            .success();
    };
    ra(&first);
    ra(&second);
    // `hash_dir` folds two empty trees into one digest; prove a corpus exists.
    assert!(first.join("sites.jsonl").is_file());
    assert!(first.join("coverage.json").is_file());
    assert!(first.join("cards").is_dir());
    let cold = hash_dir(&first);
    assert_eq!(cold, hash_dir(&second), "two cold runs");
    ra(&first);
    assert_eq!(cold, hash_dir(&first), "a cold and a warm run");
}

/// R21 over this fixture: the noise thresholds U4 set hold — no duplicate row
/// and no reversed row survive the card filter. The share of edges per rung is
/// printed as a check on the fixture, not held as a snapshot.
#[test]
fn noise_thresholds_hold_and_the_rung_shares_are_reported() {
    let repo = repo();
    let out = tempfile::tempdir().unwrap();
    let g = run(repo.path(), out.path(), RightsTable::granted());
    let noise = g.coverage.noise.as_ref().expect("v5 noise");
    let rows = noise.card_rows.expect("the pipeline built cards");
    assert_eq!(rows.duplicate_rows.after_filter, 0, "{rows:?}");
    assert_eq!(rows.reversed_rows.after_filter, 0, "{rows:?}");

    let rungs = &noise.edges_by_rung;
    let total = rungs.resolved_by_index + rungs.rules_backed + rungs.name_only + rungs.ambiguous;
    assert!(total > 0, "the fixture resolves names");
    let share = |n: u64| 100.0 * n as f64 / total as f64;
    println!(
        "edges by rung over {total}: resolved by index {} ({:.0}%), rules backed {} ({:.0}%), \
         name only {} ({:.0}%), ambiguous {} ({:.0}%)",
        rungs.resolved_by_index,
        share(rungs.resolved_by_index),
        rungs.rules_backed,
        share(rungs.rules_backed),
        rungs.name_only,
        share(rungs.name_only),
        rungs.ambiguous,
        share(rungs.ambiguous),
    );
    // No index in this run, so nothing may claim to come from one.
    assert_eq!(rungs.resolved_by_index, 0);
}

/// R7 on the rendered solver card: every `## Maybe also` line says why it is
/// a guess. The three lone disputed candidates name the receiver's declared
/// type and carry no "1 of 1"; the bare `Solve(1)` and the interface call,
/// where the name fits two declarations, carry the count and the shared name.
#[test]
fn the_rendered_card_gives_each_guess_its_reason() {
    let repo = repo();
    let out = tempfile::tempdir().unwrap();
    let g = run(repo.path(), out.path(), RightsTable::granted());
    let text = rendered_detail(&g, "/GeoSolver.md");
    let claim = |target: &str| -> &str {
        text.lines()
            .find(|l| l.starts_with("- ") && l.contains(target) && l.contains("ambiguous"))
            .unwrap_or_else(|| panic!("no Maybe also line for {target}:\n{text}"))
    };

    for target in [
        "`LedgerItem.Append(string)`",
        "`FunctionEvaluator.Max(double,double,double,double)`",
        "`TextDocument.IndexOfAny(char[],int,int)`",
    ] {
        let line = claim(target);
        assert!(!line.contains("candidates"), "{line}");
        assert!(
            line.contains(&format!(
                "calls {target} (heuristic, ambiguous: the receiver is declared as another type)"
            )),
            "{line}"
        );
    }
    for target in ["`GeoSolver.Solve(int)`", "`DbTableSource.Load()`"] {
        let line = claim(target);
        assert!(
            line.contains("calls 1 of 2 candidates: ")
                && line.contains("(heuristic, ambiguous: several declarations share the name)"),
            "{line}"
        );
    }
}

/// A rendered document with its `Maybe also` section cut out — every other
/// section, aggregates included. A detail cut into `## Part N` reopens the
/// section as `### Maybe also` in each part it runs into, so any heading
/// level ends or opens it.
fn outside_maybe_also(text: &str) -> String {
    let mut inside = false;
    let mut kept = String::new();
    for line in text.lines() {
        if line.starts_with("## ") || line.starts_with("### ") {
            inside = line.trim_start_matches('#').trim() == "Maybe also";
        }
        if !inside {
            kept.push_str(line);
            kept.push('\n');
        }
    }
    kept
}

/// The type a member identity belongs to: everything before the first `.`
/// after the `#` — the fixture nests no type.
fn owning_type(identity: &str) -> &str {
    let hash = identity.find('#').map_or(0, |i| i + 1);
    match identity[hash..].find('.') {
        Some(dot) => &identity[..hash + dot],
        None => identity,
    }
}

/// The objective "no relation sends the reader elsewhere" (plan A, R7) on the
/// card's AGGREGATES: a type the solver reaches only through ambiguous
/// relations — the three disputed BCL twins and the interface call's
/// implementation — is not a peer in the role text, the collaboration diagram
/// or the `Works with` table. Since plan B, U9 the short card names it
/// nowhere at all: the card counts the neighbours its detail lists beyond the
/// ones it names, so many of them guesses, and the detail names them in
/// `Maybe also` and nowhere else. A neighbour with an undisputed relation
/// stays a peer of both. The other side mirrors it: no guessed-at type lists
/// the solver among its users, on its card or outside its detail's
/// `Maybe also`.
#[test]
fn a_neighbour_reached_only_by_guesses_is_no_peer_of_the_card() {
    let repo = repo();
    let out = tempfile::tempdir().unwrap();
    let g = run(repo.path(), out.path(), RightsTable::granted());
    let cards = build_cards(&g.graph);
    let index = CardIndex::build(&cards);
    let card_text = |title: &str, lang: Lang| {
        render_card(
            find(&cards, CardKind::Type, title),
            lang,
            LinkStyle::Portable,
            &index,
        )
    };
    let detail_text = |title: &str| {
        render_detail(
            find(&cards, CardKind::Type, title),
            Lang::En,
            LinkStyle::Portable,
            &index,
        )
        .unwrap_or_else(|| panic!("{title} has a detail"))
    };

    // The undisputed neighbours, read from the graph: the types the solver's
    // members reach on a usage edge whose rung is anything but ambiguous.
    let undisputed: std::collections::BTreeSet<&str> = g
        .graph
        .edges_sorted()
        .into_iter()
        .filter(|e| {
            e.from.as_str().starts_with(SOLVER)
                && is_usage(e.edge.kind)
                && e.edge.provenance.rung() != Some(Confidence::Ambiguous)
                && e.to.as_str().starts_with("sym:")
        })
        .map(|e| owning_type(e.to.as_str()))
        .filter(|t| *t != SOLVER)
        .collect();
    assert_eq!(
        undisputed.iter().copied().collect::<Vec<_>>(),
        [
            "sym:csharp:Orbit.Geo#ITableSource",
            "sym:csharp:Orbit.Geo#Pomocnik",
            "sym:csharp:Orbit.Geo#PomocnikFactory",
        ],
        "the fixture's undisputed neighbours"
    );

    let en = card_text("GeoSolver", Lang::En);
    let detail = detail_text("GeoSolver");
    let kept = outside_maybe_also(&detail);
    for guessed in [
        "LedgerItem",
        "FunctionEvaluator",
        "TextDocument",
        "DbTableSource",
    ] {
        assert!(
            detail.contains(guessed),
            "{guessed} is still a Maybe also claim of the detail:\n{detail}"
        );
        assert!(
            !kept.contains(guessed),
            "{guessed} is reached only through guesses, yet the detail names it \
             outside Maybe also:\n{kept}"
        );
        assert!(
            !en.contains(guessed),
            "{guessed} is a guess, yet the short card names it:\n{en}"
        );
    }
    for real in ["ITableSource", "Pomocnik", "PomocnikFactory"] {
        assert!(
            en.contains(&format!("[{real}]")),
            "{real} is an undisputed neighbour and stays a peer:\n{en}"
        );
        assert!(kept.contains(&format!("[{real}]")), "{kept}");
    }
    assert!(
        en.contains(&format!("It uses {} others, chiefly", undisputed.len())),
        "the role counts the undisputed neighbours:\n{en}"
    );
    assert!(
        en.contains("The detail lists 4 more neighbours (all uncertain).\n"),
        "the four guessed-at types are one count:\n{en}"
    );
    let cs = card_text("GeoSolver", Lang::Cs);
    assert!(
        cs.contains("Pracuje s 3 dalšími, hlavně")
            && cs.contains("Detail uvádí ještě 4 další sousedy (všichni nejistí).\n"),
        "{cs}"
    );

    // The other side: the solver reaches each guessed-at type only through a
    // guess, so none of their cards or details lists it as a user.
    for title in [
        "LedgerItem",
        "FunctionEvaluator",
        "TextDocument",
        "DbTableSource",
    ] {
        let text = card_text(title, Lang::En);
        assert!(
            !text.contains("GeoSolver"),
            "{title}'s card names the solver:\n{text}"
        );
        let kept = outside_maybe_also(&detail_text(title));
        assert!(
            !kept.contains("GeoSolver"),
            "{title}'s detail names the solver outside Maybe also:\n{kept}"
        );
        assert!(
            text.contains("The detail lists 1 more neighbour (uncertain).\n"),
            "{title}'s card counts the solver as a guess:\n{text}"
        );
    }

    // One grain up: the solver's module reaches the guessed-at types' modules
    // only through those guesses. A module card has no `## Maybe also`, so the
    // count sentence is the whole trace — and the module it is in names none
    // of them, nor ranks `DbTableSource` as used.
    let module = render_card(
        find(&cards, CardKind::Module, "Geo"),
        Lang::En,
        LinkStyle::Portable,
        &index,
    );
    for guessed in ["Kalkulace", "Vzorce", "Editor"] {
        assert!(
            !module.contains(guessed),
            "the Geo module names {guessed}, reached only through guesses:\n{module}"
        );
    }
    assert!(
        module.contains(
            "It may depend on 3 other modules too, through name matches the rules did not settle."
        ),
        "{module}"
    );
    assert!(
        module
            .contains("| [DbTableSource](../type/csharp/Orbit.Geo/DbTableSource.md) | class | 0 |"),
        "a type used only through a guess is used by no one:\n{module}"
    );
}

/// Review #2 (R2, R8): a type imported only through the project — a
/// `GlobalUsings.cs` in another file, a `<Using Include>` item of the
/// `.csproj` — is visible in every file of that project, so its lone
/// candidate stays a main-list relation. A type no using reaches is still
/// disputed, and so is the same global using's type from another project.
#[test]
fn a_type_imported_by_the_project_is_not_disputed() {
    const WIDGET_CS: &str = "namespace Orbit.Model\n{\n    public class Widget { }\n}\n";
    const GADGET_CS: &str = "namespace Orbit.Utils\n{\n    public class Gadget { }\n}\n";
    const THING_CS: &str = "namespace Orbit.Other\n{\n    public class Thing { }\n}\n";
    const APP_CSPROJ: &str = "<Project Sdk=\"Microsoft.NET.Sdk\">\n  <ItemGroup>\n    <Using Include=\"Orbit.Utils\" />\n  </ItemGroup>\n</Project>\n";
    const GLOBAL_USINGS_CS: &str = "global using Orbit.Model;\n";
    const BUILDER_CS: &str = "namespace Orbit.App\n{\n    public class Builder\n    {\n        public void Build()\n        {\n            var w = new Widget();\n            var g = new Gadget();\n            var t = new Thing();\n        }\n    }\n}\n";
    const ELSEWHERE_CSPROJ: &str = "<Project Sdk=\"Microsoft.NET.Sdk\">\n</Project>\n";
    const STRANGER_CS: &str = "namespace Orbit.Elsewhere\n{\n    public class Stranger\n    {\n        public void Go()\n        {\n            var w = new Widget();\n        }\n    }\n}\n";
    let dir = tempfile::tempdir().unwrap();
    init_repo_with_files(
        dir.path(),
        "Alice",
        &[
            ("Model/Widget.cs", WIDGET_CS),
            ("Utils/Gadget.cs", GADGET_CS),
            ("Other/Thing.cs", THING_CS),
            ("App/App.csproj", APP_CSPROJ),
            ("App/GlobalUsings.cs", GLOBAL_USINGS_CS),
            ("App/Builder.cs", BUILDER_CS),
            ("Elsewhere/Elsewhere.csproj", ELSEWHERE_CSPROJ),
            ("Elsewhere/Stranger.cs", STRANGER_CS),
        ],
    );
    let out = tempfile::tempdir().unwrap();
    let g = run(dir.path(), out.path(), RightsTable::granted());

    const BUILD: &str = "sym:csharp:Orbit.App#Builder.Build()";
    const GO: &str = "sym:csharp:Orbit.Elsewhere#Stranger.Go()";
    let from = |source: &str, target: &str| -> Vec<(Option<Confidence>, Option<Rule>)> {
        edges_into(&g, target)
            .into_iter()
            .filter(|(s, kind, _, _)| s == source && *kind == EdgeKind::Instantiates)
            .map(|(_, _, rung, rule)| (rung, rule))
            .collect()
    };
    let main = vec![(Some(Confidence::NameOnly), Some(Rule::NameMatch))];
    let disputed = vec![(Some(Confidence::Ambiguous), Some(Rule::NamespaceVisibility))];
    assert_eq!(
        from(BUILD, "sym:csharp:Orbit.Model#Widget"),
        main,
        "GlobalUsings.cs"
    );
    assert_eq!(
        from(BUILD, "sym:csharp:Orbit.Utils#Gadget"),
        main,
        "<Using>"
    );
    assert_eq!(from(BUILD, "sym:csharp:Orbit.Other#Thing"), disputed);
    assert_eq!(
        from(GO, "sym:csharp:Orbit.Model#Widget"),
        disputed,
        "another project"
    );

    let cards = build_cards(&g.graph);
    let builder = find(&cards, CardKind::Type, "Builder");
    let outgoing: Vec<&str> = builder.outgoing.iter().map(|r| r.label.as_str()).collect();
    for kept in ["Widget", "Gadget"] {
        assert!(
            outgoing.iter().any(|l| l.contains(kept)),
            "{kept} in Builder's main list: {outgoing:?}"
        );
        assert!(!maybe_also_targets(builder)
            .iter()
            .any(|t| t.ends_with(&format!("#{kept}"))));
    }
    assert!(maybe_also_targets(builder).contains(&"sym:csharp:Orbit.Other#Thing"));
}

/// Review #7 (R7): `Foo.Create(1)` fits both static overloads, a tie the
/// receiver rule backed alike; `Foo.Create("a", 1)` settles one of them at a
/// second site. The claim left with the other overload is a shared name — it
/// must never say the receiver rule disputed what that rule backed.
#[test]
fn a_tie_left_with_one_alternative_keeps_the_shared_name_reason() {
    const FOO_CS: &str = "namespace Orbit.Fab\n{\n    public class Foo\n    {\n        public static Foo Create(int n) { return null; }\n        public static Foo Create(string s, int n = 0) { return null; }\n    }\n}\n";
    const USER_CS: &str = "namespace Orbit.Fab\n{\n    public class User\n    {\n        public void Make()\n        {\n            Foo.Create(1);\n            Foo.Create(\"a\", 1);\n        }\n    }\n}\n";
    let dir = tempfile::tempdir().unwrap();
    init_repo_with_files(
        dir.path(),
        "Alice",
        &[("Fab/Foo.cs", FOO_CS), ("Fab/User.cs", USER_CS)],
    );
    let out = tempfile::tempdir().unwrap();
    let g = run(dir.path(), out.path(), RightsTable::granted());
    const BY_INT: &str = "sym:csharp:Orbit.Fab#Foo.Create(int)";
    const BY_STRING: &str = "sym:csharp:Orbit.Fab#Foo.Create(string,int)";
    let rung_of = |target: &str| {
        edges_into(&g, target)
            .into_iter()
            .find(|(_, kind, _, _)| *kind == EdgeKind::Calls)
            .map(|(_, _, rung, rule)| (rung, rule))
    };
    assert_eq!(
        rung_of(BY_STRING),
        Some((Some(Confidence::RulesBacked), Some(Rule::StaticReceiver)))
    );
    assert_eq!(
        rung_of(BY_INT),
        Some((Some(Confidence::Ambiguous), Some(Rule::NameMatch)))
    );

    let cards = build_cards(&g.graph);
    let user = find(&cards, CardKind::Type, "User");
    let claims: Vec<_> = user
        .maybe_also
        .iter()
        .filter(|c| c.alternatives.iter().any(|a| a.identity == BY_INT))
        .collect();
    assert!(!claims.is_empty(), "{:#?}", user.maybe_also);
    for claim in claims {
        assert_eq!(claim.reason, MaybeReason::SharedName, "{claim:#?}");
    }
    let text = rendered_detail(&g, "/User.md");
    assert!(
        !text.contains("the call names another type"),
        "a backed tie read as a dispute:\n{text}"
    );
    assert!(
        text.contains("several declarations share the name"),
        "the claim is rendered, with its reason:\n{text}"
    );
}

/// The `List<…>` edge as the reference C# repository has it (plan A, Problem Frame, U14
/// acceptance): the repository's only `List` is a `public static class`
/// NESTED in `McpToolCatalog`, and the solver's `out List<KeyValuePair<…>>`
/// parameter is the BCL's generic list. A nested type's simple name reaches
/// only into its declaring type (and the types nested in it or deriving from
/// it), or through a `using static` of it — so the solver's reference is no
/// main-list relation, only a `## Maybe also` guess with that reason, while
/// inside the catalog and under `using static` the name still binds.
#[test]
fn a_nested_type_is_no_relation_where_its_simple_name_cannot_reach_it() {
    const CATALOG_CS: &str = r#"namespace Orbit.Mcp
{
    public partial class McpToolCatalog
    {
        public static class List
        {
            public static string[] All() { return null; }
        }

        public System.Type Druh() { return typeof(List); }

        public string[] Vse() { return List.All(); }
    }
}
"#;
    const SOLVER_CS: &str = r#"using System.Collections.Generic;

namespace Orbit.Geo
{
    public class GeoSolver
    {
        public bool FindAllColumns(string name, out List<KeyValuePair<string, int>> result)
        {
            result = null;
            return false;
        }
    }
}
"#;
    const STATIC_USER_CS: &str = r#"using static Orbit.Mcp.McpToolCatalog;

namespace Orbit.Tools
{
    public class Vypis
    {
        public System.Type Druh() { return typeof(List); }

        public string[] Vse() { return List.All(); }
    }
}
"#;
    let dir = tempfile::tempdir().unwrap();
    init_repo_with_files(
        dir.path(),
        "Alice",
        &[
            ("Mcp/McpToolCatalog.cs", CATALOG_CS),
            ("Geo/GeoSolver.cs", SOLVER_CS),
            ("Tools/Vypis.cs", STATIC_USER_CS),
        ],
    );
    let out = tempfile::tempdir().unwrap();
    let g = run(dir.path(), out.path(), RightsTable::granted());

    const LIST: &str = "sym:csharp:Orbit.Mcp#McpToolCatalog.List";
    const ALL: &str = "sym:csharp:Orbit.Mcp#McpToolCatalog.List.All()";
    let from =
        |source: &str, target: &str, kind: EdgeKind| -> Vec<(Option<Confidence>, Option<Rule>)> {
            edges_into(&g, target)
                .into_iter()
                .filter(|(s, k, _, _)| s == source && *k == kind)
                .map(|(_, _, rung, rule)| (rung, rule))
                .collect()
        };

    let into_list = edges_into(&g, LIST);
    let solver_rows: Vec<_> = into_list
        .iter()
        .filter(|(s, _, _, _)| s.starts_with("sym:csharp:Orbit.Geo#GeoSolver"))
        .collect();
    assert!(
        !solver_rows.is_empty(),
        "the solver's guess is kept: {into_list:?}"
    );
    for (source, kind, rung, rule) in &solver_rows {
        assert_eq!(
            (*kind, *rung, *rule),
            (
                EdgeKind::References,
                Some(Confidence::Ambiguous),
                Some(Rule::NestedTypeVisibility)
            ),
            "{source}"
        );
    }

    let cards = build_cards(&g.graph);
    let solver = find(&cards, CardKind::Type, "GeoSolver");
    assert!(
        !solver.outgoing.iter().any(|r| r.identity.starts_with(LIST)),
        "List<…> is a main-list relation of the solver: {:#?}",
        solver.outgoing
    );
    let claims: Vec<_> = solver
        .maybe_also
        .iter()
        .filter(|c| c.alternatives.iter().any(|a| a.identity == LIST))
        .collect();
    assert_eq!(claims.len(), 1, "{:#?}", solver.maybe_also);
    assert_eq!(
        claims[0].reason,
        MaybeReason::Disputed(Rule::NestedTypeVisibility)
    );
    assert!(
        rendered_detail(&g, "/GeoSolver.md")
            .contains("a nested type is not visible here by its simple name"),
        "{}",
        rendered_detail(&g, "/GeoSolver.md")
    );

    // Inside the declaring type and under `using static` the name binds: the
    // static call is backed by its receiver, the `typeof` stays a relation.
    for (caller, label) in [
        ("sym:csharp:Orbit.Mcp#McpToolCatalog", "inside the catalog"),
        ("sym:csharp:Orbit.Tools#Vypis", "under using static"),
    ] {
        assert_eq!(
            from(&format!("{caller}.Vse()"), ALL, EdgeKind::Calls),
            vec![(Some(Confidence::RulesBacked), Some(Rule::StaticReceiver))],
            "{label}"
        );
        assert_eq!(
            from(&format!("{caller}.Druh()"), LIST, EdgeKind::References),
            vec![(Some(Confidence::NameOnly), Some(Rule::NameMatch))],
            "{label}"
        );
    }
}

/// `xsrc.IndexOfAny(GeoSolver.GeoBegins, p1)` on a `string` (plan A, Problem
/// Frame; U14 acceptance): C# takes the instance method
/// `string.IndexOfAny(char[], int)`, which the repository cannot show, over
/// the repository's `this string` extension. The extension stays a relation —
/// it may still be meant where no instance member fits — but name-only,
/// never backed by the declared receiver. An extension on a repository type
/// is still backed.
#[test]
fn an_extension_on_a_string_receiver_is_name_only() {
    const EXTENSIONS_CS: &str = r#"using System.Collections.Generic;

namespace Orbit.Text
{
    public static class StringExtensions
    {
        public static int IndexOfAny(this string s, IReadOnlyCollection<char> chars, int start) { return -1; }
    }
}
"#;
    const SOLVER_CS: &str = r#"using Orbit.Text;

namespace Orbit.Geo
{
    public class GeoSolver
    {
        public static readonly char[] GeoBegins = new char[] { '{' };

        public int Najdi(string xsrc, int p1)
        {
            return xsrc.IndexOfAny(GeoSolver.GeoBegins, p1);
        }

        public string Popis(Uzel uzel)
        {
            return uzel.Describe();
        }
    }

    public class Uzel { }

    public static class UzelExtensions
    {
        public static string Describe(this Uzel u) { return null; }
    }
}
"#;
    let dir = tempfile::tempdir().unwrap();
    init_repo_with_files(
        dir.path(),
        "Alice",
        &[
            ("Text/StringExtensions.cs", EXTENSIONS_CS),
            ("Geo/GeoSolver.cs", SOLVER_CS),
        ],
    );
    let out = tempfile::tempdir().unwrap();
    let g = run(dir.path(), out.path(), RightsTable::granted());

    let calls_from = |source: &str, target: &str| -> Vec<(Option<Confidence>, Option<Rule>)> {
        edges_into(&g, target)
            .into_iter()
            .filter(|(s, k, _, _)| s == source && *k == EdgeKind::Calls)
            .map(|(_, _, rung, rule)| (rung, rule))
            .collect()
    };
    assert_eq!(
        calls_from(
            "sym:csharp:Orbit.Geo#GeoSolver.Najdi(string,int)",
            "sym:csharp:Orbit.Text#StringExtensions.IndexOfAny(string,IReadOnlyCollection<char>,int)"
        ),
        vec![(Some(Confidence::NameOnly), Some(Rule::NameMatch))]
    );
    assert_eq!(
        calls_from(
            "sym:csharp:Orbit.Geo#GeoSolver.Popis(Uzel)",
            "sym:csharp:Orbit.Geo#UzelExtensions.Describe(Uzel)"
        ),
        vec![(Some(Confidence::RulesBacked), Some(Rule::DeclaredReceiver))]
    );
}
