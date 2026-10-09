//! U1: the byte-identity baseline the per-corpus kind vocabulary work is
//! measured against (R4).
//!
//! **Why a new baseline when the repo already has snapshots.** The existing
//! goldens (`render`, `manifest`, `card`, `six_grammar_baseline`) all start at a
//! *repository*: they open a `Repo`, run the extractors, and snapshot what comes
//! out. That makes them regression tests for extraction, and they move whenever a
//! grammar or a walker changes — which is exactly what you want from them and
//! exactly wrong for this gate. The vocabulary work does not touch extraction at
//! all; it touches the four kind enums and every projection that reads them. A
//! gate that can go red for an unrelated reason is a gate nobody trusts by U6.
//!
//! So this fixture builds the graph **by hand** and runs only the projection:
//! corpus, both manifest forms, cards, coverage. No git, no tree-sitter, no
//! filesystem walk. If a byte moves here, the vocabulary work moved it.
//!
//! **Co tahle brána nekryje, a ať to nikdo nečte jinak.** Běží nad knihovní
//! projekcí, ne nad zapisovací cestou v `src/main.rs`. Změna v tom, které soubory
//! se zapíšou, jak se pojmenují nebo co se uklidí jako osiřelé, tuhle bránu
//! nerozhýbe. RG3 to našla jako mezeru a našla ji oprávněně: druhý blocker toho
//! kola ležel právě v `main.rs`. Od RG3 základna aspoň čte **obě formy
//! manifestu**, protože o tom, která se zapíše, rozhoduje velikost korpusu a ne
//! volající — zbytek zapisovací cesty zůstává nekrytý a je to zapsané v backlogu.
//!
//! **What the fixture deliberately contains.** One node of every `NodeKind` and
//! one edge of each kind the backbone views actually branch on (`Contains`,
//! `Declares`, `DependsOn`), because U6 rewrites those branches from a
//! comparison against a concrete variant into a question about a structural
//! role (KTD6). A role table that silently reordered `Module` and `File` would
//! have to move a byte in here to escape.
//!
//! **Its twin.** From U4 onward a second fixture lives beside this one, built the
//! same way but with declared kinds, and it is the gate for the path that *does*
//! change. This one proves the old path stands still; that one proves the new
//! path does not drift. Verze 1 plánu měla jen tuhle a nechránila nic z nové
//! cesty — viz plán, oddíl "Co změnilo review".

use reverse_architect::card::detail::render_detail_files;
use reverse_architect::card::documents::manifest_documents;
use reverse_architect::card::index::append_signpost;
use reverse_architect::card::links::CardIndex;
use reverse_architect::card::map::render_map_files;
use reverse_architect::card::render::Lang;
use reverse_architect::card::{build_cards, build_cards_with_insight, render_card_files, CardKind};
use reverse_architect::cli::LinkStyle;
use reverse_architect::coverage::build_coverage;
use reverse_architect::extract::languages::Languages;
use reverse_architect::graph::{
    merge, Edge, EdgeKind, ExtractorId, Fact, FactKind, FactValue, Graph, NodeKey, NodeKind,
    Provenance, Ranks, Tier,
};
use reverse_architect::manifest::build_manifest;
use reverse_architect::render::build_corpus;
use std::collections::{BTreeMap, BTreeSet};

fn prov(file: &str) -> Provenance {
    Provenance::new(file, Some(1), ExtractorId::from("baseline"), Tier::Measured)
}

/// A graph with one node of every kind and every structurally-read edge.
///
/// Keys are `/`-hierarchical with a `.` root, which is the precondition the
/// module tree documents: every intermediate directory needs its own Module node
/// (see `NodeKey`). U6's `Container` role inherits that precondition (R16), so
/// the fixture has to satisfy it here or the two halves would not be comparable.
fn fixture() -> Graph {
    let mut g = Graph::new();

    for m in [".", "src", "src/extract"] {
        g.ensure_node(NodeKey::new(m), NodeKind::Module).unwrap();
    }
    for f in [
        "src/lib.rs",
        "src/extract/walk.rs",
        "README.md",
        // The dependency edge hangs off the manifest FILE, which is what
        // `manifest_deps` actually emits. An earlier draft hung it off the root
        // module, so `dependencies.md` rendered "declared in `.`" — a shape no
        // real run can produce, which meant the gate was characterizing
        // something other than the pipeline.
        "Cargo.toml",
    ] {
        g.ensure_node(NodeKey::new(f), NodeKind::File).unwrap();
    }
    g.ensure_node(NodeKey::new("sym:src/lib.rs#Corpus"), NodeKind::Symbol)
        .unwrap();
    g.ensure_node(NodeKey::new("dep:cargo:serde"), NodeKind::Dependency)
        .unwrap();
    g.ensure_node(
        NodeKey::new("ext:rust:std::fmt::Display"),
        NodeKind::External,
    )
    .unwrap();

    for (parent, child) in [
        (".", "src"),
        (".", "README.md"),
        (".", "Cargo.toml"),
        ("src", "src/extract"),
        ("src", "src/lib.rs"),
        ("src/extract", "src/extract/walk.rs"),
    ] {
        g.add_edge(
            &NodeKey::new(parent),
            &NodeKey::new(child),
            Edge::new(EdgeKind::Contains, prov(parent)),
        );
    }
    g.add_edge(
        &NodeKey::new("src/lib.rs"),
        &NodeKey::new("sym:src/lib.rs#Corpus"),
        Edge::new(EdgeKind::Declares, prov("src/lib.rs")),
    );
    g.add_edge(
        &NodeKey::new("Cargo.toml"),
        &NodeKey::new("dep:cargo:serde"),
        Edge::new(EdgeKind::DependsOn, prov("Cargo.toml")),
    );
    // Reaches the card `externals` branch, which is `Uses` into an External
    // node. Without it the External node is an orphan that contributes a
    // manifest row and nothing else, so the guard below would pass while the
    // branch that reads it stayed uncovered.
    g.add_edge(
        &NodeKey::new("sym:src/lib.rs#Corpus"),
        &NodeKey::new("ext:rust:std::fmt::Display"),
        Edge::new(EdgeKind::Uses, prov("src/lib.rs")),
    );

    // A symbol needs its role fact to be card-eligible — that is the projection
    // policy U5 extends, so the baseline has to exercise it.
    g.add_fact(
        &NodeKey::new("sym:src/lib.rs#Corpus"),
        Fact::new(
            FactKind::SymbolKind,
            FactValue::Text("struct".into()),
            prov("src/lib.rs"),
        ),
    );
    g.add_fact(
        &NodeKey::new("src/lib.rs"),
        Fact::new(FactKind::Loc, FactValue::Int(42), prov("src/lib.rs")),
    );
    g.add_fact(
        &NodeKey::new("dep:cargo:serde"),
        Fact::new(
            FactKind::DependencyName,
            FactValue::Text("serde".into()),
            prov("Cargo.toml"),
        ),
    );
    g.add_fact(
        &NodeKey::new("ext:rust:std::fmt::Display"),
        Fact::new(
            FactKind::ExternalName,
            FactValue::Text("std::fmt::Display".into()),
            prov("src/lib.rs"),
        ),
    );

    merge(&mut g, &Ranks::new(BTreeMap::new()));
    g
}

/// The whole projection, flattened into one comparable blob.
///
/// Deliberately one snapshot rather than four: the gate's question is "did any
/// byte of the corpus move", and four snapshots invite fixing three and
/// re-recording the fourth.
fn projection() -> String {
    let g = fixture();
    let mut out = String::new();

    // The same call sequence `generate_reporting` runs, in the same order —
    // including the signpost, which is appended to `index.md` after the cards
    // are built. An earlier draft of this gate recorded only each card's kind
    // and path and never rendered one, so the whole of `render.rs` (the lead
    // sentence, the relation lines, the backlink-cap note) could change without
    // moving a byte here. The gate's own claim was false for the file this work
    // edits most; review caught it.
    let mut corpus = build_corpus(&g, LinkStyle::Portable, None).unwrap();
    let (card_set, _, card_rows) = build_cards_with_insight(&g);
    let card_index = CardIndex::build(&card_set);
    append_signpost(&mut corpus, &card_set, &card_index, LinkStyle::Portable);

    for f in &corpus.files {
        out.push_str(&format!("=== corpus/{} ===\n{}\n", f.path, f.contents));
    }

    let manifest = build_manifest(&g, None).unwrap();
    out.push_str(&format!(
        "=== manifest ===\n{}\n",
        serde_json::to_string_pretty(&manifest).unwrap()
    ));

    // BOTH manifest forms, because which one a run writes is decided by SIZE
    // (`should_write_line_delimited`), not by anything a caller chooses — a corpus
    // simply grows into the line-delimited one. Until RG3 no gate read those four
    // files at all, and that is exactly where the round's second blocker lived:
    // the header document announced a schema version that promises a vocabulary
    // section, and the form carried none. A gate that covers one form of an
    // artifact the tool picks for itself covers half the artifact.
    for (name, render) in [
        (
            reverse_architect::manifest::STRINGS_FILENAME,
            reverse_architect::manifest::to_strings_json
                as fn(&reverse_architect::manifest::Manifest) -> String,
        ),
        (
            reverse_architect::manifest::NODES_FILENAME,
            reverse_architect::manifest::to_nodes_jsonl,
        ),
        (
            reverse_architect::manifest::EDGES_FILENAME,
            reverse_architect::manifest::to_edges_jsonl,
        ),
        (
            reverse_architect::manifest::DOCUMENTS_FILENAME,
            reverse_architect::manifest::to_documents_jsonl,
        ),
    ] {
        out.push_str(&format!("=== {name} ===\n{}\n", render(&manifest)));
    }

    // Both languages: `lead` is a `(Lang, CardKind)` match, so recording one of
    // them would leave half the table unguarded.
    //
    // Since plan B (U15) the cards are one level of three: a type's lists live
    // in its detail and a module's types in its map, and each of the three
    // becomes manifest documents with a header (U11). Until U15 this gate
    // rendered the cards alone, so a kind word moving in a detail, a map row
    // or a document's header escaped it. They are recorded after the cards,
    // in their own sections, so the bytes recorded before stand still.
    for lang in [Lang::En, Lang::Cs] {
        let cards = render_card_files(&card_set, &card_index, lang, LinkStyle::Portable);
        for f in &cards {
            out.push_str(&format!(
                "=== card {lang:?} {} ===\n{}\n",
                f.path, f.contents
            ));
        }
        let details = render_detail_files(&card_set, &card_index, lang, LinkStyle::Portable);
        for f in &details {
            out.push_str(&format!(
                "=== detail {lang:?} {} ===\n{}\n",
                f.path, f.contents
            ));
        }
        let maps = render_map_files(&card_set, &card_index, lang, LinkStyle::Portable);
        for f in &maps {
            out.push_str(&format!(
                "=== map {lang:?} {} ===\n{}\n",
                f.path, f.contents
            ));
        }
        out.push_str(&format!(
            "=== card documents {lang:?} ===\n{}\n",
            serde_json::to_string_pretty(&manifest_documents(
                &card_set, &cards, &maps, &details, lang
            ))
            .unwrap()
        ));
    }

    let coverage = build_coverage(&g, None, &Languages::bundled(), Some(card_rows));
    out.push_str(&format!(
        "=== coverage ===\n{}\n",
        serde_json::to_string_pretty(&coverage).unwrap()
    ));

    out
}

/// The R4 gate. Runs after every unit, not just at the end.
#[test]
fn a_graph_with_no_declarations_projects_to_the_same_bytes() {
    insta::assert_snapshot!("zero_declarations", projection());
}

/// Guards the gate itself: a baseline that silently stopped covering something
/// would keep passing while protecting nothing, and the snapshot could then be
/// re-accepted with a hole in it.
///
/// Covers all four axes the projection branches on. An earlier draft asserted
/// only the node kinds, so the fixture could have lost every edge kind, fact
/// kind and card kind and still passed after one `cargo insta accept`.
#[test]
fn the_baseline_exercises_every_axis_the_projection_branches_on() {
    let g = fixture();

    let nodes: BTreeSet<_> = g.nodes_sorted().iter().map(|n| n.kind).collect();
    for expected in [
        NodeKind::Module,
        NodeKind::File,
        NodeKind::Symbol,
        NodeKind::Dependency,
        NodeKind::External,
    ] {
        assert!(
            nodes.contains(&expected),
            "baseline stopped covering {expected:?} — re-add a node of that kind \
             rather than deleting this assertion"
        );
    }

    // The three the backbone views read, plus the one the card `externals`
    // section reads. These are exactly the branches U6 rewrites into role
    // questions (KTD6).
    let edges: BTreeSet<_> = g.edges_sorted().iter().map(|e| e.edge.kind).collect();
    for expected in [
        EdgeKind::Contains,
        EdgeKind::Declares,
        EdgeKind::DependsOn,
        EdgeKind::Uses,
    ] {
        assert!(
            edges.contains(&expected),
            "baseline lost {expected:?} edges"
        );
    }

    let facts: BTreeSet<_> = g
        .nodes_sorted()
        .iter()
        .flat_map(|n| n.facts.iter().map(|f| f.kind))
        .collect();
    for expected in [
        FactKind::SymbolKind,
        FactKind::Loc,
        FactKind::DependencyName,
        FactKind::ExternalName,
    ] {
        assert!(
            facts.contains(&expected),
            "baseline lost {expected:?} facts"
        );
    }

    // A Vec, not a set: `CardKind` carries a declared id and deliberately does
    // not derive `Ord` — ordering card kinds by digest would be meaningless.
    let cards: Vec<_> = build_cards(&g).iter().map(|c| c.kind).collect();
    for expected in [CardKind::Type, CardKind::File, CardKind::Module] {
        assert!(
            cards.contains(&expected),
            "baseline produces no {expected:?} card"
        );
    }
}
