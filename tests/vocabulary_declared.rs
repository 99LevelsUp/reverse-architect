//! The declared-vocabulary gate (U1's second baseline, and the R2 behavioural
//! gate the Verification Contract asks for).
//!
//! **Why this file exists at all.** Its absence was the largest finding of the
//! RG1 review: after U2 and U3 landed, *not one test in the repository ever
//! constructed a `Declared` value*. Every branch the work added — cardinality
//! from a declaration, the structural roles, the `Container` key check, the
//! declared arms in the manifest and the card projection — was compiled and
//! never executed, while four cargo gates reported green. A doc comment even
//! named this file in the present tense before it was written.
//!
//! Its twin is `tests/vocabulary_baseline.rs`, which proves the *old* path
//! stands still. This one proves the *new* path does what it claims.

use reverse_architect::card::index::{append_signpost, render_signpost};
use reverse_architect::card::links::CardIndex;
use reverse_architect::card::render::Lang;
use reverse_architect::card::{build_cards, card_path, render_card_files, CardKind};
use reverse_architect::cli::LinkStyle;
use reverse_architect::coverage::build_coverage;
use reverse_architect::extract::languages::Languages;
use reverse_architect::graph::{
    merge, CardPolicy, Cardinality, Edge, EdgeKind, ExtractorId, Fact, FactKind, FactValue, Graph,
    GraphError, LinkRole, NodeKey, NodeKind, Provenance, Ranks, StructuralRole, Tier,
    VocabularyError,
};
use reverse_architect::manifest::{self, build_manifest};
use reverse_architect::render::build_corpus;
use reverse_architect::smell::cohesion_coupling_notes;
use std::collections::BTreeMap;

fn prov(file: &str) -> Provenance {
    Provenance::new(file, Some(1), ExtractorId::from("declared"), Tier::Measured)
}

fn ranked(id: &str, rank: u8) -> Ranks {
    Ranks::new(BTreeMap::from([(ExtractorId::from(id), rank)]))
}

// ---------------------------------------------------------------------------
// R7 — a declared fact kind's cardinality comes from its declaration
// ---------------------------------------------------------------------------

/// The whole reason a declaration is *required* to carry a cardinality: without
/// it `merge` cannot tell "a node legitimately holds several of these" from
/// "these are competing claims and one must win".
#[test]
fn a_declared_additive_fact_keeps_every_value_and_a_scalar_one_resolves() {
    let mut g = Graph::new();
    let multi = g
        .declare_fact_kind("open", "scope", Cardinality::Additive)
        .unwrap();
    let single = g
        .declare_fact_kind("open", "layer", Cardinality::Scalar)
        .unwrap();

    let key = NodeKey::new("src/lib.rs");
    g.ensure_node(key.clone(), NodeKind::File).unwrap();
    for value in ["a", "b"] {
        assert!(g.add_fact(
            &key,
            Fact::new(
                FactKind::Declared(multi),
                FactValue::Text(value.into()),
                prov("src/lib.rs"),
            ),
        ));
        assert!(g.add_fact(
            &key,
            Fact::new(
                FactKind::Declared(single),
                FactValue::Text(value.into()),
                prov("src/lib.rs"),
            ),
        ));
    }

    merge(&mut g, &ranked("declared", 10));

    let node = g.node(&key).unwrap();
    let additive = node
        .facts
        .iter()
        .filter(|f| f.kind == FactKind::Declared(multi))
        .count();
    let scalar: Vec<_> = node
        .facts
        .iter()
        .filter(|f| f.kind == FactKind::Declared(single))
        .collect();

    assert_eq!(additive, 2, "an Additive declaration must not be collapsed");
    assert_eq!(scalar.len(), 1, "a Scalar declaration must resolve to one");
    // And the loser is kept, not deleted — the same promise merge makes for
    // core kinds.
    assert_eq!(scalar[0].dissent.len(), 1);
}

// ---------------------------------------------------------------------------
// R9 / R16 — structural roles, and what a Container owes the tree
// ---------------------------------------------------------------------------

#[test]
fn a_declared_kind_answers_with_the_role_it_declared() {
    let mut g = Graph::new();
    let container = g
        .declare_node_kind(
            "open",
            "schema",
            StructuralRole::Container,
            CardPolicy::None,
        )
        .unwrap();
    let quiet = g
        .declare_node_kind("open", "note", StructuralRole::Opaque, CardPolicy::None)
        .unwrap();
    let link = g
        .declare_edge_kind("open", "holds", LinkRole::Containment)
        .unwrap();

    assert_eq!(
        g.node_role(NodeKind::Declared(container)),
        StructuralRole::Container
    );
    assert_eq!(
        g.node_role(NodeKind::Declared(quiet)),
        StructuralRole::Opaque
    );
    assert_eq!(g.edge_role(EdgeKind::Declared(link)), LinkRole::Containment);
}

/// R16. A `Container` takes part in the module tree, so it inherits the key
/// precondition — and a violation has to be a named error, because the failure
/// it otherwise produces is an empty view with nothing to explain it.
#[test]
fn a_declared_container_with_a_non_hierarchical_key_is_a_named_error() {
    let mut g = Graph::new();
    let id = g
        .declare_node_kind(
            "open",
            "schema",
            StructuralRole::Container,
            CardPolicy::None,
        )
        .unwrap();

    for bad in ["/leading", "trailing/", "double//segment", ""] {
        let err = g
            .ensure_node(NodeKey::new(bad), NodeKind::Declared(id))
            .unwrap_err();
        assert!(
            matches!(err, GraphError::NonHierarchicalContainer { .. }),
            "{bad:?} was accepted as a container key: {err:?}"
        );
    }

    // The same keys are fine for a kind that takes no part in the tree.
    let quiet = g
        .declare_node_kind("open", "note", StructuralRole::Opaque, CardPolicy::None)
        .unwrap();
    assert!(g
        .ensure_node(NodeKey::new("trailing/"), NodeKind::Declared(quiet))
        .is_ok());
}

// ---------------------------------------------------------------------------
// A2 — an id this graph never declared is refused at every door
// ---------------------------------------------------------------------------

#[test]
fn an_id_from_another_graph_is_refused_at_every_entry_point() {
    let mut other = Graph::new();
    let foreign_node = other
        .declare_node_kind("open", "table", StructuralRole::Leaf, CardPolicy::None)
        .unwrap();
    let foreign_fact = other
        .declare_fact_kind("open", "layer", Cardinality::Scalar)
        .unwrap();
    let foreign_edge = other
        .declare_edge_kind("open", "holds", LinkRole::Opaque)
        .unwrap();

    let mut g = Graph::new();
    let key = NodeKey::new("src/lib.rs");
    g.ensure_node(key.clone(), NodeKind::File).unwrap();

    assert!(matches!(
        g.ensure_node(NodeKey::new("t"), NodeKind::Declared(foreign_node)),
        Err(GraphError::UndeclaredKind { .. })
    ));
    assert!(!g.add_fact(
        &key,
        Fact::new(
            FactKind::Declared(foreign_fact),
            FactValue::Bool(true),
            prov("src/lib.rs"),
        ),
    ));
    assert!(!g.add_edge(
        &key,
        &key,
        Edge::new(EdgeKind::Declared(foreign_edge), prov("src/lib.rs")),
    ));
}

/// The gate RG1 found missing: a node declaration may name a card kind, and
/// nothing checked that the card kind was declared here. Ids are `Copy` u64
/// wrappers, so one minted elsewhere type-checks.
#[test]
fn a_node_declaration_naming_an_undeclared_card_kind_is_refused() {
    let mut other = Graph::new();
    let foreign_card = other.declare_card_kind("open", "table").unwrap();

    let mut g = Graph::new();
    let err = g
        .declare_node_kind(
            "open",
            "table",
            StructuralRole::Leaf,
            CardPolicy::Card(foreign_card),
        )
        .unwrap_err();
    assert!(
        matches!(err, VocabularyError::UndeclaredCardPolicy { .. }),
        "{err:?}"
    );
}

// ---------------------------------------------------------------------------
// R15 — a declared card kind's path is safe and unambiguous
// ---------------------------------------------------------------------------

#[test]
fn two_declared_card_kinds_sharing_a_name_get_different_paths() {
    let mut g = Graph::new();
    let a = g.declare_card_kind("alpha", "table").unwrap();
    let b = g.declare_card_kind("beta", "table").unwrap();

    let pa = card_path(CardKind::Declared(a), "x", g.vocabulary());
    let pb = card_path(CardKind::Declared(b), "x", g.vocabulary());

    assert_ne!(pa, pb, "the namespace is what keeps these apart");
    assert!(pa.starts_with("cards/declared/alpha/table/"), "{pa}");
    assert!(!pa.contains("//"), "{pa}");
    assert!(!pa.contains(':'), "no colon may reach a Windows path: {pa}");
}

/// A name the vocabulary accepts can still sanitize to nothing. An empty path
/// segment is not cosmetic: relative links are built by counting components, so
/// every link on such a card would climb one directory too far.
#[test]
fn a_card_kind_name_that_sanitizes_to_nothing_falls_back_instead_of_emptying_a_segment() {
    let mut g = Graph::new();

    // `.` and `..` survive `check_name` and then vanish in `sanitize_segment`,
    // which trims trailing dots.
    for name in [".", ".."] {
        let id = g.declare_card_kind("open", name).unwrap();
        let path = card_path(CardKind::Declared(id), "x", g.vocabulary());
        assert!(!path.contains("//"), "{name:?} produced {path}");
        assert!(path.starts_with("cards/declared/_hashed/"), "{path}");
    }

    // A name with an interior space does NOT need the fallback: `sanitize_segment`
    // maps each space to `_`, so the segment is non-empty. Asserted so the
    // fallback's scope is recorded rather than assumed to be wider than it is.
    //
    // This used to be a run of spaces, and RG3's second round took that case
    // away: `check_name` now refuses a half that is not equal to its own `trim()`,
    // because an untrimmed name is invisible on the page and folded away on disk
    // while still being a distinct join key. The fallback's scope did not change;
    // the set of names that can reach it did.
    let spaces = g.declare_card_kind("open", "two words").unwrap();
    let path = card_path(CardKind::Declared(spaces), "x", g.vocabulary());
    assert_eq!(path, "cards/declared/open/two_words/x.md");
}

// ---------------------------------------------------------------------------
// R5 — the corpus must not depend on the order kinds were declared in
// ---------------------------------------------------------------------------

/// Ids are a digest of the name rather than a counter, and the vocabulary is a
/// `BTreeMap`, so this should hold by construction. It is asserted anyway: "by
/// construction" is what the review found to be doing all the work everywhere
/// else in this unit.
#[test]
fn declaring_the_same_kinds_in_the_opposite_order_gives_identical_bytes() {
    // All FOUR categories, and the whole emitted corpus — not fact kinds and the
    // manifest alone. The narrow version could not see an order-dependent
    // regression in a card path, a signpost section, a view or the coverage
    // exclusion list, because it declared nothing that reaches them and compared
    // nothing that renders them. Named by the external round's test lens.
    fn corpus(reverse: bool) -> BTreeMap<String, String> {
        let mut g = Graph::new();
        let mut names = ["alpha", "beta", "gamma"];
        if reverse {
            names.reverse();
        }

        let mut facts = BTreeMap::new();
        let mut nodes = BTreeMap::new();
        let mut edges = BTreeMap::new();
        for n in names {
            facts.insert(
                n,
                g.declare_fact_kind("open", n, Cardinality::Scalar).unwrap(),
            );
            let card = g.declare_card_kind("open", n).unwrap();
            nodes.insert(
                n,
                g.declare_node_kind("open", n, StructuralRole::Leaf, CardPolicy::Card(card))
                    .unwrap(),
            );
            edges.insert(n, g.declare_edge_kind("open", n, LinkRole::Opaque).unwrap());
        }

        let file = NodeKey::new("src/lib.rs");
        g.ensure_node(NodeKey::new("."), NodeKind::Module).unwrap();
        g.ensure_node(file.clone(), NodeKind::File).unwrap();
        g.add_edge(
            &NodeKey::new("."),
            &file,
            Edge::new(EdgeKind::Contains, prov(".")),
        );
        for (name, id) in &facts {
            g.add_fact(
                &file,
                Fact::new(
                    FactKind::Declared(*id),
                    FactValue::Text((*name).into()),
                    prov("src/lib.rs"),
                ),
            );
        }
        // One declared node per declared kind, each earning a card, plus one
        // declared edge into it — so paths, signpost sections and relation rows
        // all have siblings to be ordered against.
        for (name, id) in &nodes {
            let key = NodeKey::new(format!("thing/{name}"));
            g.ensure_node(key.clone(), NodeKind::Declared(*id)).unwrap();
            g.add_edge(
                &file,
                &key,
                Edge::new(EdgeKind::Declared(edges[name]), prov("src/lib.rs")),
            );
        }
        merge(&mut g, &ranked("declared", 10));

        // Every emitted file, by path, with its bytes.
        let mut out = BTreeMap::new();
        let mut corpus = build_corpus(&g, LinkStyle::Portable, None).unwrap();
        let cards = build_cards(&g);
        let index = CardIndex::build(&cards);
        let rendered = render_card_files(&cards, &index, Lang::En, LinkStyle::Portable);
        append_signpost(&mut corpus, &cards, &index, LinkStyle::Portable);
        for f in corpus.files.into_iter().chain(rendered) {
            out.insert(f.path, f.contents);
        }
        out.insert(
            "manifest.json".into(),
            reverse_architect::manifest::to_json(&build_manifest(&g, None).unwrap()),
        );
        out.insert(
            "coverage.json".into(),
            reverse_architect::coverage::to_json(&build_coverage(
                &g,
                None,
                &Languages::bundled(),
                None,
            )),
        );
        out
    }

    let a = corpus(false);
    let b = corpus(true);
    assert_eq!(
        a.keys().collect::<Vec<_>>(),
        b.keys().collect::<Vec<_>>(),
        "declaration order changed WHICH files are emitted"
    );
    for (path, bytes) in &a {
        assert_eq!(bytes, &b[path], "{path} depends on declaration order");
    }
}

// ---------------------------------------------------------------------------
// R2 — the behavioural gate: the core must not read a declared kind's identity
// ---------------------------------------------------------------------------

/// The Verification Contract's own words: an enumeration test "would never
/// fail", because the core's variant set is a compile-time constant. So this
/// builds the *same* graph twice under two vocabularies that differ only in the
/// spelling of their names, and asserts the two corpora differ only in those
/// strings. Any core decision taken from a declared kind's identity — its
/// digest, its ordering, its position — turns this red.
#[test]
fn two_vocabularies_differing_only_in_names_give_corpora_differing_only_in_names() {
    fn corpus(fact: &str, node: &str, edge: &str) -> String {
        let mut g = Graph::new();
        let f = g
            .declare_fact_kind("open", fact, Cardinality::Scalar)
            .unwrap();
        let n = g
            .declare_node_kind("open", node, StructuralRole::Leaf, CardPolicy::None)
            .unwrap();
        let e = g.declare_edge_kind("open", edge, LinkRole::Opaque).unwrap();

        g.ensure_node(NodeKey::new("."), NodeKind::Module).unwrap();
        let leaf = NodeKey::new("thing");
        g.ensure_node(leaf.clone(), NodeKind::Declared(n)).unwrap();
        g.add_edge(
            &NodeKey::new("."),
            &leaf,
            Edge::new(EdgeKind::Declared(e), prov(".")),
        );
        g.add_fact(
            &leaf,
            Fact::new(
                FactKind::Declared(f),
                FactValue::Text("value".into()),
                prov("."),
            ),
        );
        merge(&mut g, &ranked("declared", 10));

        let mut out = serde_json::to_string_pretty(&build_manifest(&g, None).unwrap()).unwrap();
        for f in build_corpus(&g, LinkStyle::Portable, None).unwrap().files {
            out.push_str(&f.contents);
        }
        out
    }

    // One kind per category, so the two runs cannot differ in the ORDER of
    // declared kinds. That is a real limit of this gate and not an oversight:
    // declared kinds sort by digest (KTD2), so renaming one moves its position
    // among its siblings. Ordering is therefore name-derived by design; what
    // this gate asserts is that nothing ELSE is.
    let a = corpus("layer", "table", "holds");
    let b = corpus("uroven", "tabulka", "drzi");

    let normalized = b
        .replace("uroven", "layer")
        .replace("tabulka", "table")
        .replace("drzi", "holds");
    assert_eq!(
        a, normalized,
        "the core kept something other than the declared names"
    );
}

// ---------------------------------------------------------------------------
// The declared path end to end
// ---------------------------------------------------------------------------

/// KTD6's promise, exercised: a plugin that wants its nodes in the backbone
/// views declares a role for them, and the tree is built from those.
#[test]
fn a_tree_built_from_declared_containers_and_leaves_renders_a_non_empty_view() {
    let mut g = Graph::new();
    let dir = g
        .declare_node_kind(
            "open",
            "schema",
            StructuralRole::Container,
            CardPolicy::None,
        )
        .unwrap();
    let leaf = g
        .declare_node_kind("open", "table", StructuralRole::Leaf, CardPolicy::None)
        .unwrap();
    let holds = g
        .declare_edge_kind("open", "holds", LinkRole::Containment)
        .unwrap();

    g.ensure_node(NodeKey::new("."), NodeKind::Module).unwrap();
    g.ensure_node(NodeKey::new("sales"), NodeKind::Declared(dir))
        .unwrap();
    g.ensure_node(NodeKey::new("sales/invoice"), NodeKind::Declared(leaf))
        .unwrap();
    g.add_edge(
        &NodeKey::new("."),
        &NodeKey::new("sales"),
        Edge::new(EdgeKind::Declared(holds), prov(".")),
    );
    g.add_edge(
        &NodeKey::new("sales"),
        &NodeKey::new("sales/invoice"),
        Edge::new(EdgeKind::Declared(holds), prov("sales")),
    );
    merge(&mut g, &ranked("declared", 10));

    let structure = module_structure(&g);

    // Tightened by U6, as the earlier version of this test said it would be.
    // Before the views asked for the role, the strongest thing this could assert
    // was "something was rendered" — which a view that dropped every declared
    // node satisfies just as well as one that placed them.
    assert!(
        structure.contains("sales"),
        "a declared Container must appear in the tree it declared itself into: {structure}"
    );
    // The NUMBER is U6's claim and it stands: a declared Leaf is counted under its
    // container, or a declared tree renders as a container holding nothing.
    //
    // The WORD changed in RG3's external round. Two reviewers, reading this view
    // against `coverage.json`, found it calling declared entities "files" while
    // coverage said in so many words that they are not tracked files — one of the
    // two artifacts had to be lying. So the count stayed and the noun moved: a
    // corpus that holds a declared leaf says "entries", one that does not keeps
    // "files" and does not move a byte (R4).
    assert!(
        structure.contains("1 entry"),
        "a declared Leaf must be counted under its container, under a word that \
         does not claim it is a file: {structure}"
    );
    assert!(
        !structure.contains("1 file"),
        "and the old word must be gone, not merely joined: {structure}"
    );
}

/// The module-structure view's rendered text, for the assertions below.
fn module_structure(g: &Graph) -> String {
    view(g, "module-structure.md")
}

/// One rendered backbone view by file name.
fn view(g: &Graph, path: &str) -> String {
    build_corpus(g, LinkStyle::Portable, None)
        .unwrap()
        .files
        .into_iter()
        .find(|f| f.path == path)
        .unwrap_or_else(|| panic!("{path}"))
        .contents
}

/// A container, a leaf beneath it, and the containment edge between them — all
/// three declared, nothing core but the synthetic root.
///
/// All-declared on purpose. A declared container holding core `File` children
/// would flow through the core paths and prove nothing about what the rewritten
/// views do with a declared node once they have admitted one.
fn declared_tree(dir_role: StructuralRole, link_role: LinkRole) -> Graph {
    let mut g = Graph::new();
    let dir = g
        .declare_node_kind("open", "schema", dir_role, CardPolicy::None)
        .unwrap();
    let leaf = g
        .declare_node_kind("open", "table", StructuralRole::Leaf, CardPolicy::None)
        .unwrap();
    let holds = g.declare_edge_kind("open", "holds", link_role).unwrap();

    g.ensure_node(NodeKey::new("."), NodeKind::Module).unwrap();
    g.ensure_node(NodeKey::new("sales"), NodeKind::Declared(dir))
        .unwrap();
    g.ensure_node(NodeKey::new("sales/invoice"), NodeKind::Declared(leaf))
        .unwrap();
    g.add_edge(
        &NodeKey::new("."),
        &NodeKey::new("sales"),
        Edge::new(EdgeKind::Declared(holds), prov(".")),
    );
    g.add_edge(
        &NodeKey::new("sales"),
        &NodeKey::new("sales/invoice"),
        Edge::new(EdgeKind::Declared(holds), prov("sales")),
    );
    merge(&mut g, &ranked("declared", 10));
    g
}

/// R9's two halves in one test: the role is what admits a kind to the tree, and
/// `Opaque` is a real answer rather than a missing one.
#[test]
fn a_declared_node_enters_the_tree_by_its_role_and_stays_out_without_one() {
    let placed = module_structure(&declared_tree(
        StructuralRole::Container,
        LinkRole::Containment,
    ));
    let quiet = module_structure(&declared_tree(
        StructuralRole::Opaque,
        LinkRole::Containment,
    ));

    assert!(
        placed.contains("sales"),
        "role Container puts the node in the tree: {placed}"
    );
    assert!(
        !quiet.contains("sales"),
        "role Opaque keeps it out — the default for a declared kind, so a plugin          that says nothing changes no view: {quiet}"
    );

    // EVERY backbone view, not just this one. R9 says a declared kind enters the
    // backbone views only through a role, and the overview is one of them — it
    // ranks modules and lists top-level entries, both from the same graph. An
    // assertion on one view would have let the other two leak.
    let overview = view(
        &declared_tree(StructuralRole::Opaque, LinkRole::Containment),
        "overview.md",
    );
    assert!(
        !overview.contains("sales"),
        "the overview must not name a node the module tree excluded — three \
         backbone claims disagreeing about one node is the asymmetry this unit \
         exists to end: {overview}"
    );
}

/// The same question for the link. A declared edge binds the tree only when its
/// declaration says `Containment`.
#[test]
fn a_declared_edge_binds_the_tree_only_in_the_containment_role() {
    let bound = module_structure(&declared_tree(
        StructuralRole::Container,
        LinkRole::Containment,
    ));
    let loose = module_structure(&declared_tree(StructuralRole::Container, LinkRole::Opaque));

    // The observable is the DIAGRAM, not the prose count — and that is a fact
    // about the view worth writing down. `recursive_module_file_counts` walks
    // the KEY, crediting `sales/invoice` to `sales` whatever any edge says, so
    // the "1 file" line appears in both runs. Only the diagram is built from the
    // edges, so only the diagram can tell a bound tree from a loose one. An
    // assertion on the count would have passed in both and said nothing.
    assert!(
        bound.contains("-->"),
        "a Containment edge draws the tree: {bound}"
    );
    assert!(
        !loose.contains("-->"),
        "an Opaque edge binds nothing, so there is no tree to draw: {loose}"
    );
}

/// U6 step 4: coverage excludes declared kinds, and says so by name.
///
/// The exclusion itself is the right call — coverage measures how far the
/// language extractors reached over tracked files, and a declared node was never
/// handed to a grammar. Saying it is what keeps the number honest.
#[test]
fn coverage_names_the_declared_kinds_it_did_not_count() {
    let g = declared_tree(StructuralRole::Container, LinkRole::Containment);
    let coverage = build_coverage(&g, None, &Languages::bundled(), None);

    let excluded = coverage
        .declared_kinds_excluded
        .as_ref()
        .expect("a corpus with declared kinds must say what it left out");
    // These two names sort the same way by name and by digest (verified in RG2),
    // so this assertion checks CONTENT, not order. The ordering claim is tested
    // on a deliberately disagreeing pair in
    // `the_exclusion_list_is_ordered_by_name_and_not_by_digest`.
    assert_eq!(
        excluded.node_kinds,
        vec!["open:schema".to_string(), "open:table".to_string()],
        "by name and in name order, not as a count"
    );
    assert_eq!(excluded.nodes, 2);
    assert!(
        coverage.files.is_empty(),
        "no declared node may be counted as a tracked file"
    );
    assert_eq!(
        coverage.schema_version,
        reverse_architect::coverage::SCHEMA_VERSION_WITH_VOCABULARY,
        "the version rises exactly when the section is present (R12)"
    );
}

/// The other half of R12, and the reason the version rule exists: a corpus that
/// declares nothing carries no such section and keeps the old version.
#[test]
fn coverage_over_a_corpus_without_declarations_is_untouched() {
    let mut g = Graph::new();
    g.ensure_node(NodeKey::new("."), NodeKind::Module).unwrap();
    g.ensure_node(NodeKey::new("src/lib.rs"), NodeKind::File)
        .unwrap();
    let coverage = build_coverage(&g, None, &Languages::bundled(), None);

    assert!(coverage.declared_kinds_excluded.is_none());
    assert_eq!(
        coverage.schema_version,
        reverse_architect::coverage::SCHEMA_VERSION
    );
    let json = serde_json::to_string(&coverage).unwrap();
    assert!(
        !json.contains("declared_kinds_excluded"),
        "an absent section must not appear as null: {json}"
    );
}

/// U6 step 3's decisions, asserted rather than merely commented.
///
/// Both are deliberate exclusions. A declared kind whose declaration asks for no
/// card earns none — which is what this fixture declares, so the signpost has
/// nothing to lead to (the card-bearing case is
/// `a_declared_card_reaches_the_reader_through_its_own_signpost_section`). The
/// cohesion note is a claim about how source files in a directory reference each
/// other, which a declared node is not, whatever its card policy.
#[test]
fn the_signpost_and_the_cohesion_note_leave_declared_kinds_alone() {
    let g = declared_tree(StructuralRole::Container, LinkRole::Containment);

    // The synthetic root `.` is a core Module and earns its card as always — the
    // claim is about DECLARED kinds, not about the card count. An earlier draft
    // of this test asserted the whole set was empty and failed on the root, which
    // is the test being wrong rather than the code.
    let cards = build_cards(&g);
    assert!(
        !cards.is_empty(),
        "the core node in the fixture must still get its card"
    );
    assert!(
        !cards
            .iter()
            .any(|c| matches!(c.kind, CardKind::Declared(_))),
        "this fixture's kinds declare `CardPolicy::None`, so no card is owed"
    );

    // An `Imports` edge between two DECLARED leaves: the shape cohesion counts,
    // on nodes it must not count. Without the concrete-kind filter this would
    // land in the intra/inter module tally and move a verdict about source code
    // that has nothing to do with the plugin.
    let mut with_imports = g;
    with_imports.add_edge(
        &NodeKey::new("sales/invoice"),
        &NodeKey::new("sales"),
        Edge::new(EdgeKind::Imports, prov("sales/invoice")),
    );
    assert!(
        cohesion_coupling_notes(&with_imports).is_empty(),
        "a declared node is not a source file, so it cannot move a cohesion verdict"
    );
}

/// The manifest is where a corpus describes itself, so a declared kind has to
/// arrive there under the name its declaration gave it — never under a digest.
#[test]
fn the_manifest_names_declared_kinds_by_name() {
    let mut g = Graph::new();
    let n = g
        .declare_node_kind("open", "table", StructuralRole::Leaf, CardPolicy::None)
        .unwrap();
    let f = g
        .declare_fact_kind("open", "layer", Cardinality::Scalar)
        .unwrap();
    let e = g
        .declare_edge_kind("open", "holds", LinkRole::Opaque)
        .unwrap();

    let root = NodeKey::new(".");
    let leaf = NodeKey::new("thing");
    g.ensure_node(root.clone(), NodeKind::Module).unwrap();
    g.ensure_node(leaf.clone(), NodeKind::Declared(n)).unwrap();
    g.add_edge(&root, &leaf, Edge::new(EdgeKind::Declared(e), prov(".")));
    g.add_fact(
        &leaf,
        Fact::new(FactKind::Declared(f), FactValue::Int(3), prov(".")),
    );
    merge(&mut g, &ranked("declared", 10));

    let manifest = build_manifest(&g, None).unwrap();
    let json = serde_json::to_string(&manifest).unwrap();

    assert!(json.contains("\"open:table\""), "{json}");
    assert!(json.contains("\"open:layer\""), "{json}");
    assert!(json.contains("\"open:holds\""), "{json}");
    assert!(
        !json.contains("undeclared:"),
        "a declared kind fell through to the undeclared stub: {json}"
    );
}

// ---------------------------------------------------------------------------
// U4 — R3 and R12: the manifest carries the vocabulary, and only then rises
// ---------------------------------------------------------------------------

/// R12. The version follows the presence of the section, not the release. A
/// corpus that declared nothing must keep the version it had, or R4 could not
/// hold and every existing consumer would have to upgrade for a field it will
/// never see.
#[test]
fn the_schema_version_rises_only_for_a_corpus_that_declares_something() {
    let mut plain = Graph::new();
    plain
        .ensure_node(NodeKey::new("."), NodeKind::Module)
        .unwrap();
    let m = build_manifest(&plain, None).unwrap();
    assert_eq!(m.schema_version, manifest::SCHEMA_VERSION);
    assert!(m.vocabulary.is_none(), "no declarations, no section");

    let mut declaring = Graph::new();
    declaring
        .ensure_node(NodeKey::new("."), NodeKind::Module)
        .unwrap();
    declaring
        .declare_fact_kind("open", "layer", Cardinality::Scalar)
        .unwrap();
    let m = build_manifest(&declaring, None).unwrap();
    assert_eq!(m.schema_version, manifest::SCHEMA_VERSION_WITH_VOCABULARY);
    assert!(m.vocabulary.is_some());
}

/// R3. The point of the section: a consumer that never heard of the extractor
/// can still enumerate every kind and read how the core treated it — without
/// being told what any of it means.
#[test]
fn a_consumer_reads_the_declared_properties_without_knowing_what_the_kinds_mean() {
    let mut g = Graph::new();
    let card = g.declare_card_kind("open", "table").unwrap();
    g.declare_node_kind(
        "open",
        "table",
        StructuralRole::Leaf,
        CardPolicy::Card(card),
    )
    .unwrap();
    g.declare_node_kind("open", "note", StructuralRole::Opaque, CardPolicy::None)
        .unwrap();
    g.declare_edge_kind("open", "holds", LinkRole::Containment)
        .unwrap();
    g.declare_fact_kind("open", "layer", Cardinality::Scalar)
        .unwrap();
    g.declare_fact_kind("open", "scope", Cardinality::Additive)
        .unwrap();
    g.ensure_node(NodeKey::new("."), NodeKind::Module).unwrap();

    let v = build_manifest(&g, None).unwrap().vocabulary.unwrap();

    assert_eq!(
        v.digest, "fnv1a64",
        "the digest is named, not implied (R14)"
    );

    let table = v
        .node_kinds
        .iter()
        .find(|k| k.kind.name == "open:table")
        .unwrap();
    assert_eq!(table.kind.namespace, "open");
    assert_eq!(table.role, "leaf");
    // Joins to `card_kinds` by name — the id would mean nothing to a consumer.
    assert_eq!(table.card.as_deref(), Some("open:table"));
    assert!(v.card_kinds.iter().any(|c| c.kind.name == "open:table"));

    let note = v
        .node_kinds
        .iter()
        .find(|k| k.kind.name == "open:note")
        .unwrap();
    assert_eq!(note.role, "opaque");
    assert!(note.card.is_none(), "a non-card kind writes no card field");

    assert_eq!(v.edge_kinds[0].role, "containment");

    let layer = v
        .fact_kinds
        .iter()
        .find(|k| k.kind.name == "open:layer")
        .unwrap();
    let scope = v
        .fact_kinds
        .iter()
        .find(|k| k.kind.name == "open:scope")
        .unwrap();
    assert_eq!(layer.cardinality, "scalar");
    assert_eq!(scope.cardinality, "additive");

    // The section publishes no id. It would be derivable from `name` and
    // `digest`, and — because all four categories share one digest — the node
    // kind `open:table` and the card kind `open:table` above would have carried
    // the same one.
    let json = serde_json::to_string(&v).unwrap();
    assert!(
        !json.contains("\"id\""),
        "the vocabulary section publishes no id: {json}"
    );
}

/// Sorted by name, not by digest. Digest order is deterministic but arbitrary to
/// a reader — one plugin's kinds would be scattered through the list in an order
/// nothing explains.
#[test]
fn the_vocabulary_section_is_ordered_by_name() {
    let mut g = Graph::new();
    for name in ["zeta", "alpha", "mid"] {
        g.declare_fact_kind("open", name, Cardinality::Scalar)
            .unwrap();
    }
    g.ensure_node(NodeKey::new("."), NodeKind::Module).unwrap();

    let v = build_manifest(&g, None).unwrap().vocabulary.unwrap();
    let names: Vec<_> = v.fact_kinds.iter().map(|k| k.kind.name.as_str()).collect();
    assert_eq!(names, ["open:alpha", "open:mid", "open:zeta"]);
}

/// The manifest round-trips: a consumer decodes what this wrote, including the
/// new section. Without this the section is write-only.
#[test]
fn a_manifest_carrying_a_vocabulary_round_trips() {
    let mut g = Graph::new();
    g.declare_fact_kind("open", "layer", Cardinality::Scalar)
        .unwrap();
    g.ensure_node(NodeKey::new("."), NodeKind::Module).unwrap();

    let built = build_manifest(&g, None).unwrap();
    let json = serde_json::to_string(&built).unwrap();
    let decoded: reverse_architect::manifest::Manifest = serde_json::from_str(&json).unwrap();
    assert_eq!(built, decoded);
}

/// R8, both halves in one graph: the card policy is what decides, and `None` is
/// a real answer rather than an unimplemented one.
///
/// This test used to assert the opposite — that a card-bearing declaration was
/// inert — precisely so it would turn red when U5 landed. It did, and this is
/// what it was turned into.
#[test]
fn a_declared_kind_earns_a_card_only_when_its_declaration_asks_for_one() {
    let mut g = Graph::new();
    let card = g.declare_card_kind("open", "table").unwrap();
    let bearing = g
        .declare_node_kind(
            "open",
            "table",
            StructuralRole::Leaf,
            CardPolicy::Card(card),
        )
        .unwrap();
    let silent = g
        .declare_node_kind("open", "column", StructuralRole::Leaf, CardPolicy::None)
        .unwrap();

    g.ensure_node(NodeKey::new("."), NodeKind::Module).unwrap();
    g.ensure_node(NodeKey::new("thing"), NodeKind::Declared(bearing))
        .unwrap();
    g.ensure_node(NodeKey::new("thing.part"), NodeKind::Declared(silent))
        .unwrap();

    let cards = build_cards(&g);
    let declared: Vec<_> = cards
        .iter()
        .filter(|c| matches!(c.kind, CardKind::Declared(_)))
        .collect();
    assert_eq!(
        declared.len(),
        1,
        "one declaration asked for a card and one did not: {:?}",
        cards.iter().map(|c| &c.identity).collect::<Vec<_>>()
    );
    assert_eq!(declared[0].identity, "thing");
    assert_eq!(
        declared[0].kind,
        CardKind::Declared(card),
        "under the card kind the declaration named, not under a core kind it would \
         have to pretend to be"
    );
    assert_eq!(
        declared[0].kind_word, "open:table",
        "and named by the word the declaration gave it (KTD4)"
    );
    assert!(
        !cards.iter().any(|c| c.identity == "thing.part"),
        "`CardPolicy::None` is an answer, not a gap: {:?}",
        cards.iter().map(|c| &c.identity).collect::<Vec<_>>()
    );
}

/// Two card kinds, two trees. The declaration picks WHICH card kind, so two
/// declared kinds must not land on one path — the same separation the three core
/// kinds get (R15).
#[test]
fn two_declared_card_kinds_put_their_cards_in_two_trees() {
    let mut g = Graph::new();
    let table_card = g.declare_card_kind("open", "table").unwrap();
    let view_card = g.declare_card_kind("open", "view").unwrap();
    let table = g
        .declare_node_kind(
            "open",
            "table",
            StructuralRole::Leaf,
            CardPolicy::Card(table_card),
        )
        .unwrap();
    let view = g
        .declare_node_kind(
            "open",
            "view",
            StructuralRole::Leaf,
            CardPolicy::Card(view_card),
        )
        .unwrap();

    g.ensure_node(NodeKey::new("."), NodeKind::Module).unwrap();
    // The SAME identity under two node kinds would be one node, so the two keys
    // differ — but only after the last segment, which is what makes the card
    // kind, not the key, the thing that separates the two paths.
    g.ensure_node(NodeKey::new("sales/invoice"), NodeKind::Declared(table))
        .unwrap();
    g.ensure_node(NodeKey::new("sales/invoice_open"), NodeKind::Declared(view))
        .unwrap();

    let cards = build_cards(&g);
    let path = |identity: &str| {
        cards
            .iter()
            .find(|c| c.identity == identity)
            .unwrap_or_else(|| panic!("{identity} earned no card"))
            .path
            .clone()
    };
    assert!(
        path("sales/invoice").starts_with("cards/declared/open/table/"),
        "{}",
        path("sales/invoice")
    );
    assert!(
        path("sales/invoice_open").starts_with("cards/declared/open/view/"),
        "{}",
        path("sales/invoice_open")
    );
}

/// KTD1's rule, asked through the new question. A file whose every declaration
/// already owns a card documents none of them itself — and "owns a card"
/// includes a card-bearing DECLARED kind, not just a type.
///
/// Before U5 this file documented the same source twice. The duplication was
/// invisible because nothing could produce a declared card yet; the two call
/// sites asking the question have to agree, so they now ask one function.
///
/// Card schema v4: every file has a card now, so the file card exists — the
/// rule this test keeps is that it does not re-list what the declared card
/// already documents.
#[test]
fn a_file_whose_every_declaration_owns_a_declared_card_re_lists_none_of_them() {
    let mut g = Graph::new();
    let card = g.declare_card_kind("open", "table").unwrap();
    let kind = g
        .declare_node_kind(
            "open",
            "table",
            StructuralRole::Definition,
            CardPolicy::Card(card),
        )
        .unwrap();

    let file = NodeKey::new("schema/sales.sql");
    let thing = NodeKey::new("open:table:invoice");
    g.ensure_node(NodeKey::new("."), NodeKind::Module).unwrap();
    g.ensure_node(file.clone(), NodeKind::File).unwrap();
    g.ensure_node(thing.clone(), NodeKind::Declared(kind))
        .unwrap();
    g.add_edge(
        &file,
        &thing,
        Edge::new(EdgeKind::Declares, prov("schema/sales.sql")),
    );
    merge(&mut g, &ranked("declared", 10));

    let cards = build_cards(&g);
    assert!(
        cards.iter().any(|c| c.identity == thing.as_str()),
        "the declared kind owns the documentation"
    );
    let file_card = cards
        .iter()
        .find(|c| c.identity == file.as_str())
        .unwrap_or_else(|| {
            panic!(
                "every file has a card (v4): {:?}",
                cards.iter().map(|c| &c.identity).collect::<Vec<_>>()
            )
        });
    assert!(
        file_card.members.is_empty(),
        "so its member rows would duplicate the declared card: {:?}",
        file_card.members
    );
    // …and on the rendered card too: no member list of any shape.
    let index = CardIndex::build(&cards);
    let rendered = render_card_files(&cards, &index, Lang::En, LinkStyle::Portable)
        .into_iter()
        .find(|f| f.path.starts_with("cards/file/"))
        .expect("the file card is rendered")
        .contents;
    for section in ["## Members", "## Functions", "## Internals"] {
        assert!(
            !rendered.contains(section),
            "{section} would re-list the declared card's content:\n{rendered}"
        );
    }
}

/// KTD1's other branch, and the one RG3 found unguarded: a file whose
/// declarations are NOT all card-owning keeps its card.
///
/// The companion test above covers "every declaration owns a card". This one
/// covers "not every one does", and it is the branch that actually exercises the
/// third `own_card` call site — the `Declares` arm in `build_card`. RG3 proved
/// that site decided nothing in any test: reverting it to the old `is_type`
/// semantics left the whole suite green. Both halves of that arm are asserted
/// here, so the substitution cannot be undone silently.
#[test]
fn a_file_with_one_card_less_declaration_keeps_its_own_card_and_lists_it() {
    let mut g = Graph::new();
    let card = g.declare_card_kind("open", "table").unwrap();
    let bearing = g
        .declare_node_kind(
            "open",
            "table",
            StructuralRole::Definition,
            CardPolicy::Card(card),
        )
        .unwrap();
    let silent = g
        .declare_node_kind(
            "open",
            "column",
            StructuralRole::Definition,
            CardPolicy::None,
        )
        .unwrap();

    let file = NodeKey::new("schema/sales.sql");
    let owns = NodeKey::new("open:table:invoice");
    let owned_by_nobody = NodeKey::new("open:column:invoice.total");
    g.ensure_node(NodeKey::new("."), NodeKind::Module).unwrap();
    g.ensure_node(file.clone(), NodeKind::File).unwrap();
    g.ensure_node(owns.clone(), NodeKind::Declared(bearing))
        .unwrap();
    g.ensure_node(owned_by_nobody.clone(), NodeKind::Declared(silent))
        .unwrap();
    for target in [&owns, &owned_by_nobody] {
        g.add_edge(
            &file,
            target,
            Edge::new(EdgeKind::Declares, prov("schema/sales.sql")),
        );
    }
    merge(&mut g, &ranked("declared", 10));

    let cards = build_cards(&g);
    let file_card = cards
        .iter()
        .find(|c| c.identity == file.as_str())
        .unwrap_or_else(|| {
            panic!(
                "one declaration owns no card, so the file still documents something: {:?}",
                cards.iter().map(|c| &c.identity).collect::<Vec<_>>()
            )
        });
    assert!(
        file_card
            .members
            .iter()
            .any(|m| m.identity == owned_by_nobody.as_str()),
        "and the declaration nothing else documents is a member row on it: {:?}",
        file_card.members
    );
    assert!(
        !file_card
            .members
            .iter()
            .any(|m| m.identity == owns.as_str()),
        "while the one that owns a card of its own is not re-listed here (KTD4/R5): {:?}",
        file_card.members
    );
}

/// R22/R23 on a card kind that had never been asked: a member of a DECLARED card
/// owns no card, so its redaction has to be confessed on its owner's.
///
/// RG3 found this swallowed. The confession and the relation lifting had been
/// written as one block, so fencing the lifting to `Type | File` silently fenced
/// the confession with it.
#[test]
fn a_declared_cards_member_has_its_redaction_confessed() {
    let mut g = Graph::new();
    let card = g.declare_card_kind("open", "table").unwrap();
    let kind = g
        .declare_node_kind(
            "open",
            "table",
            StructuralRole::Definition,
            CardPolicy::Card(card),
        )
        .unwrap();

    let owner = NodeKey::new("open:table:invoice");
    let member = NodeKey::new("sym:sql:invoice.total");
    g.ensure_node(NodeKey::new("."), NodeKind::Module).unwrap();
    g.ensure_node(owner.clone(), NodeKind::Declared(kind))
        .unwrap();
    g.ensure_node(member.clone(), NodeKind::Symbol).unwrap();
    g.add_fact(
        &member,
        Fact::new(
            FactKind::SymbolName,
            FactValue::Text("total".into()),
            prov("schema/sales.sql"),
        ),
    );
    // The fact the core withholds: its presence is the claim, its value is not.
    g.add_fact(
        &member,
        Fact::new(
            FactKind::RedactedLiteral,
            FactValue::Text("api key".into()),
            prov("schema/sales.sql"),
        ),
    );
    g.add_edge(
        &owner,
        &member,
        Edge::new(EdgeKind::HasMember, prov("schema/sales.sql")),
    );
    merge(&mut g, &ranked("declared", 10));

    let cards = build_cards(&g);
    let owner_card = cards
        .iter()
        .find(|c| c.identity == owner.as_str())
        .expect("the declared kind earns a card");
    assert!(
        owner_card
            .members
            .iter()
            .any(|m| m.identity == member.as_str()),
        "the member row is there, which is what makes the silence possible: {:?}",
        owner_card.members
    );
    assert!(
        owner_card.caveats.iter().any(|c| c.text.contains("total")),
        "so the member's withheld literal must be confessed by name on the only \
         card a reader can reach it from (R22): {:?}",
        owner_card.caveats
    );

    // …and on the card a reader actually receives, not only in the struct behind
    // it. R22 says the confession has to be where someone looks; a renderer that
    // stopped emitting caveats would leave every assertion above green. The
    // external round's test lens named exactly that gap.
    let index = CardIndex::build(&cards);
    let rendered = render_card_files(&cards, &index, Lang::En, LinkStyle::Portable)
        .into_iter()
        .find(|f| f.path.contains("declared/open/table/"))
        .expect("the declared card is rendered")
        .contents;
    assert!(
        rendered.contains("total"),
        "the admission has to name the member it is about: {rendered}"
    );
    assert!(
        rendered.contains("the value is not carried"),
        "…and say what happened to the literal: {rendered}"
    );

    // What is deliberately NOT asserted here: that the card does not leak the
    // secret. It cannot. A `RedactedLiteral` fact's value is the name of the
    // PATTERN that matched (`src/card/mod.rs`, `push_caveats`), never the literal
    // — the secret is not withheld from the card, it never enters the graph at
    // all. A first draft of this test asserted the absence of the value and
    // failed, because the fixture had used the pattern name as the value and the
    // caveat names the pattern on purpose. The stronger claim is the one above:
    // the admission reaches the page.
}

/// The line-delimited manifest carries the vocabulary its own version promises.
///
/// `should_write_line_delimited` picks the form by SIZE, so nothing a caller does
/// selects it — a corpus simply grows into it. The four-file form wrote strings,
/// nodes, edges and documents and no vocabulary at all, while the header
/// document's `schema_version` already said `SCHEMA_VERSION_WITH_VOCABULARY`,
/// which by R12 means "this manifest carries a vocabulary section". A consumer
/// reading the version went looking for something that was not there, and the
/// corpora it happened to were the big ones — the shape the whole work is for.
///
/// Found by the external review round's api-design lens, on a file no internal
/// round had looked at: the writer lives in `src/main.rs`, outside the library
/// every other gate reads.
#[test]
fn the_line_delimited_manifest_carries_the_vocabulary_its_version_promises() {
    let mut g = Graph::new();
    let card = g.declare_card_kind("open", "table").unwrap();
    g.declare_node_kind(
        "open",
        "table",
        StructuralRole::Leaf,
        CardPolicy::Card(card),
    )
    .unwrap();
    g.declare_edge_kind("open", "holds", LinkRole::Containment)
        .unwrap();
    g.ensure_node(NodeKey::new("."), NodeKind::Module).unwrap();

    let built = build_manifest(&g, None).unwrap();
    let header = reverse_architect::manifest::to_strings_json(&built);
    let decoded: reverse_architect::manifest::ManifestStrings =
        serde_json::from_str(&header).expect("a consumer decodes the header document");

    assert_eq!(
        decoded.schema_version,
        reverse_architect::manifest::SCHEMA_VERSION_WITH_VOCABULARY,
        "the header announces the version that promises a vocabulary"
    );
    let v = decoded
        .vocabulary
        .expect("…so the vocabulary has to actually be in it (R3, R12)");
    assert_eq!(
        v.node_kinds
            .iter()
            .map(|k| k.kind.name.as_str())
            .collect::<Vec<_>>(),
        vec!["open:table"]
    );
    assert_eq!(
        v.edge_kinds
            .iter()
            .map(|k| k.kind.name.as_str())
            .collect::<Vec<_>>(),
        vec!["open:holds"]
    );

    // R4's half: a corpus that declares nothing writes no such key at all — not
    // an empty object, not a null.
    let mut core = Graph::new();
    core.ensure_node(NodeKey::new("."), NodeKind::Module)
        .unwrap();
    let plain = reverse_architect::manifest::to_strings_json(&build_manifest(&core, None).unwrap());
    assert!(
        !plain.contains("vocabulary"),
        "an absent section must not appear at all: {plain}"
    );
}

/// R2 on the one surface nothing was watching: the ORDER of relation rows.
///
/// `EdgeKind` derives `Ord`, so `Declared(EdgeKindId)` compares by the FNV-1a
/// digest — and which row comes first on a card is a decision. The external review
/// round found the core taking it from a declared kind's identity in two places
/// (`sort_relations` and `backlinks`), and a second reviewer found independently
/// why no internal gate could see it: every fixture declared one kind per
/// category, and one kind has no sibling to be ordered against.
///
/// So this declares two, and asserts first that their digest order and their name
/// order DISAGREE — computed here, not assumed. Without that assertion the test
/// would be the third ordering test in this work whose two orderings happened to
/// agree.
#[test]
fn two_declared_edge_kinds_on_one_card_render_in_name_order() {
    // `alfa` < `echo` by name. The assertion below pins that the digest disagrees.
    let (by_name_first, by_name_second) = ("alfa", "echo");
    assert!(
        fnv1a64(&format!("open:{by_name_second}")) < fnv1a64(&format!("open:{by_name_first}")),
        "the fixture is only a test if the two orders disagree: digest order must \
         put {by_name_second:?} first while name order puts {by_name_first:?} first"
    );

    let mut g = Graph::new();
    let first = g
        .declare_edge_kind("open", by_name_first, LinkRole::Opaque)
        .unwrap();
    let second = g
        .declare_edge_kind("open", by_name_second, LinkRole::Opaque)
        .unwrap();

    let from = NodeKey::new("src/a.rs");
    let to = NodeKey::new("src/b.rs");
    g.ensure_node(NodeKey::new("."), NodeKind::Module).unwrap();
    g.ensure_node(from.clone(), NodeKind::File).unwrap();
    g.ensure_node(to.clone(), NodeKind::File).unwrap();
    // Declared in digest order, so insertion order cannot carry the assertion.
    g.add_edge(
        &from,
        &to,
        Edge::new(EdgeKind::Declared(second), prov("src/a.rs")),
    );
    g.add_edge(
        &from,
        &to,
        Edge::new(EdgeKind::Declared(first), prov("src/a.rs")),
    );
    merge(&mut g, &ranked("declared", 10));

    let cards = build_cards(&g);
    let card = cards
        .iter()
        .find(|c| c.identity == from.as_str())
        .expect("the file earns a card");
    let words: Vec<&str> = card.outgoing.iter().map(|r| r.kind_word.as_ref()).collect();
    assert_eq!(
        words,
        vec![
            format!("open:{by_name_first}"),
            format!("open:{by_name_second}")
        ],
        "by name — the digest would have put them the other way round"
    );
}

/// The same rule on the backlink side, which had the same defect.
#[test]
fn two_declared_edge_kinds_backlink_in_name_order() {
    let mut g = Graph::new();
    let alfa = g
        .declare_edge_kind("open", "alfa", LinkRole::Opaque)
        .unwrap();
    let echo = g
        .declare_edge_kind("open", "echo", LinkRole::Opaque)
        .unwrap();

    let target = NodeKey::new("src/t.rs");
    g.ensure_node(NodeKey::new("."), NodeKind::Module).unwrap();
    g.ensure_node(target.clone(), NodeKind::File).unwrap();
    for (i, kind) in [echo, alfa].into_iter().enumerate() {
        let src = NodeKey::new(format!("src/s{i}.rs"));
        g.ensure_node(src.clone(), NodeKind::File).unwrap();
        g.add_edge(
            &src,
            &target,
            Edge::new(EdgeKind::Declared(kind), prov("src/s.rs")),
        );
    }
    merge(&mut g, &ranked("declared", 10));

    let cards = build_cards(&g);
    let card = cards
        .iter()
        .find(|c| c.identity == target.as_str())
        .expect("the target earns a card");
    let (shown, _) = reverse_architect::card::links::backlinks(card);
    let words: Vec<&str> = shown.iter().map(|r| r.kind_word.as_ref()).collect();
    assert_eq!(words, vec!["open:alfa", "open:echo"]);
}

/// A Windows device name with an EXTENSION is still that device.
///
/// `RESERVED`'s own doc comment said "invalid as a file stem with ANY extension"
/// while the code compared the whole segment, so `con.txt` walked straight
/// through. The external round read the comment, tested the code, and found the
/// two disagreeing — and the underscore had to go after the stem, because
/// `con.txt_` is still `con` to Windows.
#[test]
fn a_device_name_with_an_extension_does_not_reach_a_path() {
    for name in ["con.txt", "NUL.md", "aux.tar.gz", "com1.rs"] {
        let mut g = Graph::new();
        let card = g.declare_card_kind("open", name).unwrap();
        let kind = g
            .declare_node_kind("open", name, StructuralRole::Leaf, CardPolicy::Card(card))
            .unwrap();
        g.ensure_node(NodeKey::new("."), NodeKind::Module).unwrap();
        g.ensure_node(NodeKey::new("thing"), NodeKind::Declared(kind))
            .unwrap();

        let path = build_cards(&g)
            .into_iter()
            .find(|c| matches!(c.kind, CardKind::Declared(_)))
            .expect("the declared kind earns a card")
            .path;
        // Lowercased on BOTH sides: the path preserves the case the plugin chose,
        // while Windows resolves a device name without regard to it, so the check
        // has to be as case-blind as the filesystem is.
        let stem = name.split('.').next().unwrap().to_lowercase();
        let lower = path.to_lowercase();
        assert!(
            !lower.contains(&format!("/{stem}.")),
            "{name:?} put the bare device stem back in the path: {path}"
        );
        assert!(
            lower.contains(&format!("{stem}_")),
            "{name:?} must be broken AFTER the stem, not at the end: {path}"
        );
    }
}

/// What the signpost cap leaves out, it says (R10).
///
/// Three external reviewers, independently, called the silent truncation a
/// violation: a list that stops at twenty without a word tells the reader it is
/// complete. The core `Notable types` section keeps its own silence on purpose —
/// adding a line there would move the bytes of every existing corpus with more
/// than twenty types, which R4 forbids — and that asymmetry is written down in
/// `render_ranked`.
#[test]
fn a_capped_declared_section_says_what_it_left_out() {
    let mut g = Graph::new();
    let card = g.declare_card_kind("open", "table").unwrap();
    let kind = g
        .declare_node_kind(
            "open",
            "table",
            StructuralRole::Leaf,
            CardPolicy::Card(card),
        )
        .unwrap();
    g.ensure_node(NodeKey::new("."), NodeKind::Module).unwrap();
    for i in 0..25 {
        g.ensure_node(NodeKey::new(format!("t{i:03}")), NodeKind::Declared(kind))
            .unwrap();
    }

    let cards = build_cards(&g);
    let signpost = render_signpost(&cards, &CardIndex::build(&cards), LinkStyle::Portable);
    assert!(
        signpost.contains("and 5 further cards of this kind, not listed here"),
        "25 cards, a cap of 20, so five are owed a sentence: {signpost}"
    );

    // And a section that fits says nothing — the confession is about omission,
    // not a footer.
    let mut small = Graph::new();
    let c2 = small.declare_card_kind("open", "table").unwrap();
    let k2 = small
        .declare_node_kind("open", "table", StructuralRole::Leaf, CardPolicy::Card(c2))
        .unwrap();
    small
        .ensure_node(NodeKey::new("."), NodeKind::Module)
        .unwrap();
    small
        .ensure_node(NodeKey::new("only"), NodeKind::Declared(k2))
        .unwrap();
    let cards = build_cards(&small);
    let signpost = render_signpost(&cards, &CardIndex::build(&cards), LinkStyle::Portable);
    assert!(
        !signpost.contains("not listed here"),
        "nothing was left out, so nothing is owed: {signpost}"
    );
}

/// A name the vocabulary must refuse, because it would not stay a name.
///
/// RG3 declared a kind whose name carried a newline and got a heading into
/// `index.md` that no corpus had declared, while the path side folded the same
/// characters away — two sinks disagreeing about what the kind was called. The
/// gate is at declaration, not at each sink, so the manifest's `namespace:name`
/// stays the join key it claims to be.
#[test]
fn a_name_that_would_not_survive_a_heading_is_refused_at_declaration() {
    for (namespace, name) in [
        ("open", "tab\nle"),
        ("open", "tab\rle"),
        ("open", "tab\u{0}le"),
        ("open", "tab`le"),
        ("op\nen", "table"),
        // Format characters (category `Cf`). None of these is a control
        // character, so `char::is_control` alone let every one of them through
        // into the heading and into the directory name — found by three
        // reviewers independently in RG3's second round.
        ("open", "tab\u{202e}le"),
        ("open", "tab\u{200b}le"),
        ("open", "tab\u{feff}le"),
        ("open", "tab\u{00ad}le"),
        ("open", "tab\u{2066}le"),
        // Untrimmed and blank halves: invisible on the page, folded away on
        // disk, and two distinct join keys all the same.
        ("open", "table "),
        ("open", " table"),
        ("open", "   "),
        (" open", "table"),
    ] {
        let mut g = Graph::new();
        assert!(
            matches!(
                g.declare_card_kind(namespace, name),
                Err(VocabularyError::MalformedName { .. })
            ),
            "{namespace:?}/{name:?} must be refused at declaration, not rewritten \
             on the way to one sink and not the other"
        );
    }
}

/// A character that only needs folding is NOT refused — it is folded, and the
/// wire name keeps it.
///
/// The split this pins is the whole design: the manifest publishes
/// `namespace:name` as the join key a consumer ties a card, a coverage exclusion
/// and a vocabulary entry together with, so the core does not get to rewrite it.
/// The PATH is a derivative and may be folded freely.
///
/// `#` is the case that made the rule visible. RG3's first round added it to this
/// file as a name that "breaks nothing"; the second round showed it reaching the
/// directory verbatim, where a `#` starts a fragment and breaks every relative
/// link on every card beneath it. A kind honestly called `C#` still has to be
/// callable that, so the answer was to fold in the path, not to refuse.
#[test]
fn a_name_that_only_needs_folding_keeps_its_word_and_gets_a_safe_path() {
    let mut g = Graph::new();
    let card = g.declare_card_kind("ext", "C#").unwrap();
    let kind = g
        .declare_node_kind("ext", "C#", StructuralRole::Leaf, CardPolicy::Card(card))
        .unwrap();
    g.ensure_node(NodeKey::new("."), NodeKind::Module).unwrap();
    g.ensure_node(NodeKey::new("thing"), NodeKind::Declared(kind))
        .unwrap();

    let declared = build_cards(&g)
        .into_iter()
        .find(|c| matches!(c.kind, CardKind::Declared(_)))
        .expect("the declared kind earns a card");

    assert_eq!(
        declared.kind_word, "ext:C#",
        "the wire name keeps every character the plugin chose"
    );
    assert!(
        !declared.path.contains('#'),
        "and the path carries none of them into a link target: {}",
        declared.path
    );
    assert!(
        declared.path.starts_with("cards/declared/ext/C_/"),
        "folded, not hashed — the name is still readable on disk: {}",
        declared.path
    );
}

/// The core's own fallback tree is not a namespace a plugin may take.
///
/// Same argument RG2 used to reserve `undeclared`, and found the same way: by
/// asking what of the core's own vocabulary is still unclaimed. A kind declared
/// here would compose `declared/_hashed/<name>` and share a tree with every card
/// whose own name or identity could not be spelled as a path.
#[test]
fn the_card_path_fallback_namespace_is_reserved() {
    let mut g = Graph::new();
    assert!(matches!(
        g.declare_card_kind("_hashed", "table"),
        Err(VocabularyError::ReservedNamespace { .. })
    ));
    assert!(matches!(
        g.declare_node_kind("_hashed", "table", StructuralRole::Leaf, CardPolicy::None),
        Err(VocabularyError::ReservedNamespace { .. })
    ));
}

/// R15's other half, which the character rules never covered: length.
///
/// `card_path` bounds itself by falling back to `hashed_path` — but that fallback
/// composes the SAME `declared/<namespace>/<name>` directory, whose length a
/// plugin chooses. RG3 measured a 157-character path through the fallback against
/// a 120-character budget.
#[test]
fn a_declared_kind_with_a_very_long_name_still_fits_the_path_budget() {
    let mut g = Graph::new();
    let long = "n".repeat(100);
    let card = g.declare_card_kind(&long, "table").unwrap();
    let kind = g
        .declare_node_kind(&long, "table", StructuralRole::Leaf, CardPolicy::Card(card))
        .unwrap();
    g.ensure_node(NodeKey::new("."), NodeKind::Module).unwrap();
    g.ensure_node(NodeKey::new("sales/invoice"), NodeKind::Declared(kind))
        .unwrap();

    let path = build_cards(&g)
        .into_iter()
        .find(|c| matches!(c.kind, CardKind::Declared(_)))
        .expect("the declared kind earns a card")
        .path;
    assert!(
        path.len() <= 120,
        "{} chars, over the budget the fallback is supposed to enforce: {path}",
        path.len()
    );
    assert!(
        path.starts_with("cards/declared/_hashed/"),
        "and it gets there by the digest branch, not by truncating a half: {path}"
    );
}

/// The boundary itself, which the oversized case above cannot pin.
///
/// `MAX_DECLARED_DIR` is an arithmetic claim — what is left of the 120-byte
/// budget after everything `hashed_path` adds — and an off-by-one in it would
/// still pass a test that only ever asks about a wildly oversized name. So this
/// asks about 45 and 46.
#[test]
fn the_declared_directory_budget_flips_at_its_stated_boundary() {
    // `declared/` + ns + `/` + name == 9 + ns + 1 + name.
    fn dir_of(ns_len: usize, name_len: usize) -> String {
        let (ns, name) = ("a".repeat(ns_len), "b".repeat(name_len));
        let mut g = Graph::new();
        let card = g.declare_card_kind(&ns, &name).unwrap();
        let kind = g
            .declare_node_kind(&ns, &name, StructuralRole::Leaf, CardPolicy::Card(card))
            .unwrap();
        g.ensure_node(NodeKey::new("."), NodeKind::Module).unwrap();
        g.ensure_node(NodeKey::new("x"), NodeKind::Declared(kind))
            .unwrap();
        build_cards(&g)
            .into_iter()
            .find(|c| matches!(c.kind, CardKind::Declared(_)))
            .expect("the declared kind earns a card")
            .path
    }

    let at_the_limit = dir_of(17, 18); // 9 + 17 + 1 + 18 == 45
    assert!(
        at_the_limit.starts_with(&format!(
            "cards/declared/{}/{}/",
            "a".repeat(17),
            "b".repeat(18)
        )),
        "45 is inside the budget and stays readable: {at_the_limit}"
    );
    let one_over = dir_of(17, 19); // == 46
    assert!(
        one_over.starts_with("cards/declared/_hashed/"),
        "46 is over it: {one_over}"
    );
}

/// The budget is in bytes, and an identity can spend more than one per character.
///
/// `hashed_path` truncates the readable stub to [`TRUNCATED_SEGMENT`], which RG3
/// found was counting CHARACTERS while `card_path` compares the result against
/// `MAX_REL_PATH` in BYTES. A two-byte alphabet therefore bought twice the
/// budget. This drives the worst case the arithmetic was written for: a directory
/// exactly at the limit, and an identity long enough and wide enough to force the
/// fallback.
#[test]
fn a_multibyte_identity_cannot_spend_more_than_its_share_of_the_path_budget() {
    let (ns, name) = ("a".repeat(17), "b".repeat(18)); // dir == 45 bytes
    let mut g = Graph::new();
    let card = g.declare_card_kind(&ns, &name).unwrap();
    let kind = g
        .declare_node_kind(&ns, &name, StructuralRole::Leaf, CardPolicy::Card(card))
        .unwrap();
    // Fifty two-byte characters: a hundred bytes in one segment, so the readable
    // path is far over the budget and the fallback has to take it.
    let identity = "ž".repeat(50);
    g.ensure_node(NodeKey::new("."), NodeKind::Module).unwrap();
    g.ensure_node(NodeKey::new(&identity), NodeKind::Declared(kind))
        .unwrap();

    let path = build_cards(&g)
        .into_iter()
        .find(|c| matches!(c.kind, CardKind::Declared(_)))
        .expect("the declared kind earns a card")
        .path;
    assert!(
        path.contains("/_hashed/"),
        "the readable form is far over the budget, so the fallback owns this: {path}"
    );
    assert!(
        path.len() <= 120,
        "{} bytes — the fallback is what enforces the budget, so it may not be \
         the thing that breaks it: {path}",
        path.len()
    );
}

/// The signpost leads to declared cards under the plugin's own heading — and to
/// nothing at all when the corpus declared none (R4).
#[test]
fn a_declared_card_reaches_the_reader_through_its_own_signpost_section() {
    let mut g = Graph::new();
    let card = g.declare_card_kind("open", "table").unwrap();
    let kind = g
        .declare_node_kind(
            "open",
            "table",
            StructuralRole::Leaf,
            CardPolicy::Card(card),
        )
        .unwrap();
    g.ensure_node(NodeKey::new("."), NodeKind::Module).unwrap();
    g.ensure_node(NodeKey::new("sales/invoice"), NodeKind::Declared(kind))
        .unwrap();

    let cards = build_cards(&g);
    let signpost = render_signpost(&cards, &CardIndex::build(&cards), LinkStyle::Portable);
    assert!(
        signpost.contains("\n## open:table\n"),
        "under the word the declaration gave it, never under a core heading: {signpost}"
    );
    assert!(
        signpost.contains("sales/invoice"),
        "and it has to actually lead somewhere: {signpost}"
    );

    // R4's half. A corpus that declares nothing must produce what it always did,
    // so the section is absent rather than empty — no heading, no "none" line.
    let mut core = Graph::new();
    core.ensure_node(NodeKey::new("."), NodeKind::Module)
        .unwrap();
    let core_cards = build_cards(&core);
    let core_signpost = render_signpost(
        &core_cards,
        &CardIndex::build(&core_cards),
        LinkStyle::Portable,
    );
    assert_eq!(
        core_signpost.matches("\n## ").count(),
        2,
        "`Modules` and `Notable types`, and nothing a declaration would have \
         added: {core_signpost}"
    );
}

/// Two declared card kinds get two sections, ordered by NAME.
///
/// **This test was written once and did not work.** RG3's first round found the
/// ordering in `render_declared` guarded by nothing; the fix was this test, and
/// the second round proved by mutation that it stayed GREEN with the `BTreeMap`
/// replaced by insertion order. Declaring the kinds in reverse was not enough:
/// cards do not arrive in declaration order, they arrive in NODE KEY order
/// (`build_cards` iterates a `BTreeMap` keyed by identity), and the first
/// fixture's keys happened to sort the same way its kind names do.
///
/// So the keys are chosen to sort **against** the kind names: `aaa/...` carries
/// `open:view` and `zzz/...` carries `open:table`. Name order demands
/// `open:table` first, arrival order offers `open:view` first, and only a
/// name-ordered grouping can satisfy the assertion. That is the third time this
/// work has produced an ordering test whose two orderings agreed by accident —
/// which is why the mutation is now written down rather than assumed.
#[test]
fn two_declared_card_kinds_get_two_signpost_sections_in_name_order() {
    let mut g = Graph::new();
    let view_card = g.declare_card_kind("open", "view").unwrap();
    let table_card = g.declare_card_kind("open", "table").unwrap();
    let view = g
        .declare_node_kind(
            "open",
            "view",
            StructuralRole::Leaf,
            CardPolicy::Card(view_card),
        )
        .unwrap();
    let table = g
        .declare_node_kind(
            "open",
            "table",
            StructuralRole::Leaf,
            CardPolicy::Card(table_card),
        )
        .unwrap();

    g.ensure_node(NodeKey::new("."), NodeKind::Module).unwrap();
    // `aaa` sorts first and carries the kind that must render SECOND.
    g.ensure_node(NodeKey::new("aaa/early"), NodeKind::Declared(view))
        .unwrap();
    g.ensure_node(NodeKey::new("zzz/late"), NodeKind::Declared(table))
        .unwrap();

    let cards = build_cards(&g);
    assert_eq!(
        cards
            .iter()
            .filter(|c| matches!(c.kind, CardKind::Declared(_)))
            .map(|c| c.identity.as_str())
            .collect::<Vec<_>>(),
        vec!["aaa/early", "zzz/late"],
        "the premise: cards arrive in node-key order, which here is the OPPOSITE \
         of the order the sections must come out in"
    );

    let signpost = render_signpost(&cards, &CardIndex::build(&cards), LinkStyle::Portable);
    let table_at = signpost
        .find("\n## open:table\n")
        .unwrap_or_else(|| panic!("no section for open:table: {signpost}"));
    let view_at = signpost
        .find("\n## open:view\n")
        .unwrap_or_else(|| panic!("no section for open:view: {signpost}"));
    assert!(
        table_at < view_at,
        "by name — and the card that carries `open:table` arrived last, so this \
         cannot pass on arrival order: {signpost}"
    );

    // And each section leads only to its own kind's cards. Sliced rather than
    // searched, because `contains` over the whole string would pass even if both
    // entries landed in one section.
    let table_section = &signpost[table_at..view_at];
    let view_section = &signpost[view_at..];
    assert!(
        table_section.contains("zzz/late") && !table_section.contains("aaa/early"),
        "the table section: {table_section}"
    );
    assert!(
        view_section.contains("aaa/early") && !view_section.contains("zzz/late"),
        "the view section: {view_section}"
    );
}

/// The `lead` sentence has had a `Declared` arm per language since U2 and no
/// test had ever run either: nothing produced a declared card to render.
#[test]
fn a_declared_card_renders_in_both_languages_and_names_its_own_kind() {
    let mut g = Graph::new();
    let card = g.declare_card_kind("open", "table").unwrap();
    let kind = g
        .declare_node_kind(
            "open",
            "table",
            StructuralRole::Leaf,
            CardPolicy::Card(card),
        )
        .unwrap();
    g.ensure_node(NodeKey::new("."), NodeKind::Module).unwrap();
    g.ensure_node(NodeKey::new("sales/invoice"), NodeKind::Declared(kind))
        .unwrap();

    let cards = build_cards(&g);
    let index = CardIndex::build(&cards);
    for lang in [Lang::En, Lang::Cs] {
        let rendered = render_card_files(&cards, &index, lang, LinkStyle::Portable)
            .into_iter()
            .find(|f| f.path.contains("declared/open/table/"))
            .unwrap_or_else(|| panic!("{lang:?}: no declared card was rendered"));
        assert!(
            rendered.contents.contains("open:table"),
            "{lang:?}: the sentence names the kind, because naming it and pointing \
             at the manifest is the whole of what the core can honestly say: {}",
            rendered.contents
        );
    }
}

// ---------------------------------------------------------------------------
// RG2 — gaps the review round named
// ---------------------------------------------------------------------------

/// FNV-1a 64, reimplemented here on purpose.
///
/// The crate's own is `pub(crate)`, and reaching for it would anyway prove only
/// that the crate agrees with itself. The digest is a wire format (R14), so an
/// independent implementation outside the crate is the stronger check — the same
/// argument the pinned-literal assertions in this file already make.
fn fnv1a64(s: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in s.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Name order and digest order have to be told apart, or the ordering claim is
/// untestable.
///
/// RG2 found that the fixture this file already had — `open:schema` and
/// `open:table` — sorts the SAME way by name and by digest, so its ordering
/// assertion would have passed with the `.sort()` deleted. A test that cannot
/// fail is not a weaker test, it is a false statement about what is checked.
#[test]
fn the_exclusion_list_is_ordered_by_name_and_not_by_digest() {
    // Chosen because the two orders disagree, and asserted so the choice cannot
    // silently stop disagreeing if the digest ever changes.
    let (first_by_name, first_by_digest) = ("alfa", "echo");
    assert!(first_by_name < first_by_digest, "must disagree by name");
    assert!(
        fnv1a64(&format!("open:{first_by_digest}")) < fnv1a64(&format!("open:{first_by_name}")),
        "must disagree by digest, or this test proves nothing"
    );

    let mut g = Graph::new();
    // Declared in digest order, so only a name sort can produce the expectation.
    for name in [first_by_digest, first_by_name] {
        g.declare_node_kind("open", name, StructuralRole::Leaf, CardPolicy::None)
            .unwrap();
    }
    let coverage = build_coverage(&g, None, &Languages::bundled(), None);
    assert_eq!(
        coverage.declared_kinds_excluded.unwrap().node_kinds,
        vec!["open:alfa".to_string(), "open:echo".to_string()],
        "by name — the order a reader can follow, not the one the map iterates"
    );
}

/// U6 routed the card's "externals" section by ROLE rather than by
/// `NodeKind::External`. Nothing exercised that, so the change was a claim.
#[test]
fn a_declared_node_in_the_external_role_lands_in_a_cards_externals_section() {
    let mut g = Graph::new();
    let outside = g
        .declare_node_kind(
            "open",
            "service",
            StructuralRole::External,
            CardPolicy::None,
        )
        .unwrap();

    let file = NodeKey::new("src/a.rs");
    let sym = NodeKey::new("sym:rust:crate::a#T");
    let ext = NodeKey::new("open:service:billing");
    g.ensure_node(NodeKey::new("."), NodeKind::Module).unwrap();
    g.ensure_node(file.clone(), NodeKind::File).unwrap();
    g.ensure_node(sym.clone(), NodeKind::Symbol).unwrap();
    g.ensure_node(ext.clone(), NodeKind::Declared(outside))
        .unwrap();
    // A type, so the symbol earns a card of its own.
    g.add_fact(
        &sym,
        Fact::new(
            FactKind::SymbolKind,
            FactValue::Text("struct".into()),
            prov("src/a.rs"),
        ),
    );
    g.add_fact(
        &sym,
        Fact::new(
            FactKind::SymbolName,
            FactValue::Text("T".into()),
            prov("src/a.rs"),
        ),
    );
    g.add_edge(&file, &sym, Edge::new(EdgeKind::Declares, prov("src/a.rs")));
    g.add_edge(&sym, &ext, Edge::new(EdgeKind::Uses, prov("src/a.rs")));
    merge(&mut g, &ranked("declared", 10));

    let cards = build_cards(&g);
    let card = cards
        .iter()
        .find(|c| c.kind == CardKind::Type)
        .expect("the symbol must earn a type card");
    assert!(
        card.externals.iter().any(|r| r.identity == ext.as_str()),
        "a declared kind whose declaration says External is referenced-but-not-contained \
         in exactly the sense that section is about: {:?}",
        card.externals
    );
    assert!(
        !card.outgoing.iter().any(|r| r.identity == ext.as_str()),
        "and it must not ALSO appear as a generic outgoing relation: {:?}",
        card.outgoing
    );
}

/// The exclusion is a RULE, not an incident: a corpus that declared a node kind
/// and happens to hold none of it still owes the reader that statement.
#[test]
fn coverage_states_the_exclusion_even_when_nothing_was_actually_excluded() {
    let mut g = Graph::new();
    g.declare_node_kind("open", "table", StructuralRole::Leaf, CardPolicy::None)
        .unwrap();
    g.ensure_node(NodeKey::new("."), NodeKind::Module).unwrap();

    let coverage = build_coverage(&g, None, &Languages::bundled(), None);
    let excluded = coverage
        .declared_kinds_excluded
        .as_ref()
        .expect("a declared kind makes the exclusion a rule, whether or not it bit");
    assert_eq!(excluded.nodes, 0);
    assert_eq!(excluded.node_kinds, vec!["open:table".to_string()]);
    assert_eq!(
        coverage.schema_version,
        reverse_architect::coverage::SCHEMA_VERSION_WITH_VOCABULARY,
        "the version follows the section's presence, not the count"
    );
}

/// The two artifacts' version predicates deliberately differ, and that is a
/// contract a consumer has to be able to rely on — so it is pinned rather than
/// left as an accident of two `if`s written a day apart (RG2, two reviewers).
#[test]
fn a_fact_only_vocabulary_raises_the_manifest_version_and_not_the_coverage_one() {
    let mut g = Graph::new();
    g.declare_fact_kind("open", "layer", Cardinality::Scalar)
        .unwrap();
    g.ensure_node(NodeKey::new("."), NodeKind::Module).unwrap();

    let manifest = build_manifest(&g, None).unwrap();
    let coverage = build_coverage(&g, None, &Languages::bundled(), None);

    assert_eq!(
        manifest.schema_version,
        reverse_architect::manifest::SCHEMA_VERSION_WITH_VOCABULARY,
        "the manifest publishes all four categories, so any declaration raises it"
    );
    assert_eq!(
        coverage.schema_version,
        reverse_architect::coverage::SCHEMA_VERSION,
        "coverage speaks only about nodes it did not count, and no node kind was declared"
    );
    assert!(coverage.declared_kinds_excluded.is_none());
}
