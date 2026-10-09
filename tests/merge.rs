//! U9 integration tests: the merge layer — additive union, scalar precedence,
//! dissent survival, idempotency, and end-to-end determinism (E5, E6, E7, KTD7).

mod common;

use common::init_repo_with_files;
use reverse_architect::cli::LinkStyle;
use reverse_architect::config::Config;
use reverse_architect::extract::{run_all, trust_ranks, Repo};
use reverse_architect::graph::{
    merge, ExtractorId, Fact, FactKind, FactValue, Graph, NodeKey, NodeKind, Provenance, Ranks,
    Tier,
};
use reverse_architect::manifest::{build_manifest, to_json};

/// A ranks map for the fake extractors used in the unit-style tests.
///
/// Allocates an id per entry, which is right here and wrong on an emission
/// path — production sites clone a cached id from `extract::ids` instead.
fn ranks(pairs: &[(&'static str, u8)]) -> Ranks {
    Ranks::new(
        pairs
            .iter()
            .map(|(id, rank)| (ExtractorId::from(*id), *rank))
            .collect(),
    )
}

fn fact(kind: FactKind, value: FactValue, extractor: &'static str) -> Fact {
    Fact::new(
        kind,
        value,
        Provenance::new("f", None, extractor.into(), Tier::Measured),
    )
}

/// (a) SCALAR conflict: two extractors of different trust-rank assert a
/// different CommitCount on one node → higher rank wins, loser survives in
/// `dissent` with its own provenance. A naive merge that kept the last-written
/// value (or dropped the loser) fails this.
#[test]
fn scalar_conflict_higher_rank_wins_loser_kept_as_dissent() {
    let mut g = Graph::new();
    let key = NodeKey::new("f");
    g.ensure_node(key.clone(), NodeKind::File).unwrap();
    g.add_fact(&key, fact(FactKind::Loc, FactValue::Int(3), "low"));
    g.add_fact(&key, fact(FactKind::Loc, FactValue::Int(9), "high"));

    merge(&mut g, &ranks(&[("low", 10), ("high", 20)]));

    let facts = &g.node(&key).unwrap().facts;
    assert_eq!(facts.len(), 1, "scalar kind collapses to one resolved fact");
    let cc = &facts[0];
    assert_eq!(cc.value, FactValue::Int(9), "higher-rank claim wins");
    assert_eq!(&*cc.provenance.extractor, "high");
    // The loser is never deleted — it is retrievable in dissent, with its own
    // provenance intact.
    assert_eq!(cc.dissent.len(), 1);
    assert_eq!(cc.dissent[0].value, FactValue::Int(3));
    assert_eq!(&*cc.dissent[0].provenance.extractor, "low");
}

/// (b) ADDITIVE: two Author facts (different names) on one node — the case that
/// exists in real Phase A git data → BOTH survive as facts, NEITHER moved to
/// dissent. This is the KTD-B7 regression a "one winner per (node,kind)" merge
/// would corrupt on existing data.
#[test]
fn additive_authors_both_survive_neither_demoted() {
    let mut g = Graph::new();
    let key = NodeKey::new("f");
    g.ensure_node(key.clone(), NodeKind::File).unwrap();
    g.add_fact(
        &key,
        fact(
            FactKind::DependencyScope,
            FactValue::Text("Alice".into()),
            "git",
        ),
    );
    g.add_fact(
        &key,
        fact(
            FactKind::DependencyScope,
            FactValue::Text("Bob".into()),
            "git",
        ),
    );

    merge(&mut g, &ranks(&[("git", 20)]));

    let authors: Vec<&Fact> = g
        .node(&key)
        .unwrap()
        .facts
        .iter()
        .filter(|f| f.kind == FactKind::DependencyScope)
        .collect();
    assert_eq!(authors.len(), 2, "both authors survive as facts");
    let names: Vec<&FactValue> = authors.iter().map(|f| &f.value).collect();
    assert!(names.contains(&&FactValue::Text("Alice".into())));
    assert!(names.contains(&&FactValue::Text("Bob".into())));
    for a in authors {
        assert!(
            a.dissent.is_empty(),
            "an additive fact never carries dissent"
        );
    }
}

/// Two equal-rank claims that differ ONLY in extractor id break the tie the
/// same way an owned id as they did a `&'static str` (U3).
///
/// This is the scenario U3 exists to protect. `Provenance` derives `Ord` with
/// `extractor` third, `merge_facts` emits from an ordered `BTreeSet<Claim>`, and
/// the corpus is written in that order — so if `Arc<str>` compared differently
/// from `&'static str`, every artifact would reorder and no snapshot would
/// explain why. It does not: both compare the bytes of the `str`. The winner is
/// the lexicographically smaller id, and the loser survives as dissent.
///
/// Asserted on a concrete winner rather than on "the two builds agree": two
/// runs of a broken comparison agree with each other perfectly.
#[test]
fn an_equal_rank_tie_between_two_extractor_ids_breaks_on_content_order() {
    let build = |reversed: bool| {
        let mut g = Graph::new();
        let key = NodeKey::new("f");
        g.ensure_node(key.clone(), NodeKind::File).unwrap();
        // Same value, same file, same line — the id is the ONLY axis left, so
        // the tie can only be decided by comparing it.
        let pair = [
            fact(FactKind::Loc, FactValue::Int(7), "alpha"),
            fact(FactKind::Loc, FactValue::Int(7), "omega"),
        ];
        for f in if reversed {
            vec![pair[1].clone(), pair[0].clone()]
        } else {
            vec![pair[0].clone(), pair[1].clone()]
        } {
            g.add_fact(&key, f);
        }
        merge(&mut g, &ranks(&[("alpha", 20), ("omega", 20)]));
        g.node(&key).unwrap().facts.clone()
    };

    let forward = build(false);
    assert_eq!(
        forward,
        build(true),
        "insertion order must not decide the tie"
    );

    assert_eq!(forward.len(), 1, "a scalar kind collapses to one fact");
    assert_eq!(
        &*forward[0].provenance.extractor, "alpha",
        "the lexicographically smaller id wins, as it did when ids were statics"
    );
    assert_eq!(
        forward[0].dissent.len(),
        1,
        "the loser is kept, never dropped"
    );
    assert_eq!(&*forward[0].dissent[0].provenance.extractor, "omega");
}

/// Equal-rank scalar conflict (two `git`-sourced claims) → the tie is broken by
/// the (FactValue, file, line) total order, and the result is identical across
/// two independent merges (no arbitrary winner).
#[test]
fn equal_rank_scalar_tie_is_deterministic() {
    let build = || {
        let mut g = Graph::new();
        let key = NodeKey::new("f");
        g.ensure_node(key.clone(), NodeKind::File).unwrap();
        // Same extractor (equal rank), two different values, inserted in
        // opposite orders across the two builds.
        g.add_fact(&key, fact(FactKind::Loc, FactValue::Int(9), "git"));
        g.add_fact(&key, fact(FactKind::Loc, FactValue::Int(3), "git"));
        merge(&mut g, &ranks(&[("git", 20)]));
        g.node(&key).unwrap().facts.clone()
    };
    let build_reversed = || {
        let mut g = Graph::new();
        let key = NodeKey::new("f");
        g.ensure_node(key.clone(), NodeKind::File).unwrap();
        g.add_fact(&key, fact(FactKind::Loc, FactValue::Int(3), "git"));
        g.add_fact(&key, fact(FactKind::Loc, FactValue::Int(9), "git"));
        merge(&mut g, &ranks(&[("git", 20)]));
        g.node(&key).unwrap().facts.clone()
    };

    let a = build();
    let b = build_reversed();
    assert_eq!(a, b, "insertion order must not change the tie outcome");
    // The smaller value under the total order wins deterministically.
    assert_eq!(a[0].value, FactValue::Int(3));
    assert_eq!(a[0].dissent.len(), 1);
    assert_eq!(a[0].dissent[0].value, FactValue::Int(9));
}

/// Identical additive facts from two sources → deduplicated to one, not
/// double-counted.
#[test]
fn identical_additive_facts_dedup_to_one() {
    let mut g = Graph::new();
    let key = NodeKey::new("f");
    g.ensure_node(key.clone(), NodeKind::File).unwrap();
    g.add_fact(
        &key,
        fact(
            FactKind::DependencyScope,
            FactValue::Text("Alice".into()),
            "git",
        ),
    );
    g.add_fact(
        &key,
        fact(
            FactKind::DependencyScope,
            FactValue::Text("Alice".into()),
            "git",
        ),
    );

    merge(&mut g, &ranks(&[("git", 20)]));

    let authors = g
        .node(&key)
        .unwrap()
        .facts
        .iter()
        .filter(|f| f.kind == FactKind::DependencyScope)
        .count();
    assert_eq!(authors, 1, "identical (value+provenance) collapse to one");
}

/// Merge is idempotent: a second merge over an already-merged graph produces
/// byte-identical output — losers in dissent are re-pooled and re-resolved to
/// the same result, never dropped.
#[test]
fn merge_is_idempotent() {
    let build = |extra_merge: bool| {
        let mut g = Graph::new();
        let key = NodeKey::new("f");
        g.ensure_node(key.clone(), NodeKind::File).unwrap();
        g.add_fact(&key, fact(FactKind::Loc, FactValue::Int(3), "low"));
        g.add_fact(&key, fact(FactKind::Loc, FactValue::Int(9), "high"));
        g.add_fact(
            &key,
            fact(
                FactKind::DependencyScope,
                FactValue::Text("Bob".into()),
                "git",
            ),
        );
        g.add_fact(
            &key,
            fact(
                FactKind::DependencyScope,
                FactValue::Text("Alice".into()),
                "git",
            ),
        );
        let r = ranks(&[("low", 10), ("high", 20), ("git", 20)]);
        merge(&mut g, &r);
        if extra_merge {
            merge(&mut g, &r);
        }
        g.node(&key).unwrap().facts.clone()
    };
    assert_eq!(build(false), build(true), "merge twice == merge once");
}

/// petgraph permits parallel edges and `add_edge` does not dedup; merge must
/// collapse two identical edges to one.
#[test]
fn identical_parallel_edges_dedup_to_one() {
    use reverse_architect::graph::{Edge, EdgeKind};
    let mut g = Graph::new();
    let a = NodeKey::new("a");
    let b = NodeKey::new("b");
    g.ensure_node(a.clone(), NodeKind::File).unwrap();
    g.ensure_node(b.clone(), NodeKind::File).unwrap();
    let edge = || {
        Edge::new(
            EdgeKind::DependsOn,
            Provenance::new("a", None, "manifest-deps".into(), Tier::Measured),
        )
    };
    g.add_edge(&a, &b, edge());
    g.add_edge(&a, &b, edge());
    assert_eq!(g.edge_count(), 2, "precondition: parallel edges exist");

    merge(&mut g, &ranks(&[("manifest-deps", 40)]));

    assert_eq!(g.edge_count(), 1, "identical edges collapse to one");
}

/// KTD6 on the edge side: two sources asserting ONE relation — a name heuristic
/// and an external index — become one edge at the `resolved` precision, and the
/// heuristic assertion survives as dissent ON THAT SAME EDGE. Before U4 these were
/// two parallel edges and every consumer had to reconcile them itself.
#[test]
fn a_resolved_edge_outranks_a_heuristic_one_and_keeps_it_as_dissent() {
    use reverse_architect::graph::{Edge, EdgeKind, Resolution};
    let mut g = Graph::new();
    let caller = NodeKey::new("sym:csharp:N#A.Go()");
    let callee = NodeKey::new("sym:csharp:N#B.Run()");
    g.ensure_node(caller.clone(), NodeKind::Symbol).unwrap();
    g.ensure_node(callee.clone(), NodeKind::Symbol).unwrap();
    // Same relation, two sources: the tree-sitter name heuristic and a (future)
    // external index. Different lines, so provenance differs — which is exactly the
    // case pre-U4 edge dedup could not collapse.
    let heuristic = Provenance::new("A.cs", Some(4), "treesitter".into(), Tier::Measured);
    let resolved = Provenance::new("A.cs", Some(9), "scip".into(), Tier::Measured).resolved();
    g.add_edge(&caller, &callee, Edge::new(EdgeKind::Calls, heuristic));
    g.add_edge(&caller, &callee, Edge::new(EdgeKind::Calls, resolved));

    merge(&mut g, &ranks(&[("treesitter", 30), ("scip", 50)]));

    let edges = g.edges_sorted();
    assert_eq!(edges.len(), 1, "one relation collapses to one edge");
    let edge = edges[0].edge;
    assert_eq!(
        edge.provenance.resolution,
        Resolution::Resolved,
        "resolved wins on conflict (KTD6)"
    );
    assert_eq!(&*edge.provenance.extractor, "scip");
    assert_eq!(edge.dissent.len(), 1, "the heuristic claim is not deleted");
    assert_eq!(&*edge.dissent[0].provenance.extractor, "treesitter");
    assert_eq!(edge.dissent[0].provenance.resolution, Resolution::Heuristic);
    assert_eq!(edge.dissent[0].provenance.span.start(), Some(4));
}

/// Two claims for the SAME edge, equal on every `Provenance` field except the
/// new `ambiguous_candidates` (added for the ambiguity-visibility fix), still
/// resolve deterministically — `merge_edges`' tie-break derives `Ord` over the
/// whole `Provenance` (KTD7), so the new field became its lowest-priority
/// axis. This is an artificial fixture (real claims for one edge always share
/// the same candidate count, since it comes from the same repo-wide symbol
/// table) proving the interaction the plan's Stop condition flagged is inert,
/// not merely assumed inert.
#[test]
fn ambiguous_candidates_participates_in_the_tie_break_without_breaking_determinism() {
    use reverse_architect::graph::{Edge, EdgeKind};
    let build = |reversed: bool| {
        let mut g = Graph::new();
        let caller = NodeKey::new("sym:csharp:N#A.Go()");
        let callee = NodeKey::new("sym:csharp:N#B.Run()");
        g.ensure_node(caller.clone(), NodeKind::Symbol).unwrap();
        g.ensure_node(callee.clone(), NodeKind::Symbol).unwrap();
        let two_candidates =
            Provenance::new("A.cs", Some(4), "treesitter".into(), Tier::Measured).ambiguous(2);
        let three_candidates =
            Provenance::new("A.cs", Some(4), "treesitter".into(), Tier::Measured).ambiguous(3);
        let claims = if reversed {
            [three_candidates, two_candidates]
        } else {
            [two_candidates, three_candidates]
        };
        for claim in claims {
            g.add_edge(&caller, &callee, Edge::new(EdgeKind::Calls, claim));
        }
        merge(&mut g, &ranks(&[("treesitter", 30)]));
        g
    };

    let a = build(false);
    let b = build(true);
    let edges_a = a.edges_sorted();
    let edges_b = b.edges_sorted();
    assert_eq!(edges_a.len(), 1, "one relation collapses to one edge");
    assert_eq!(edges_b.len(), 1, "one relation collapses to one edge");
    // Every other `Provenance` field is equal between the two claims, so the
    // tie falls all the way to `ambiguous_candidates`, the field's lowest
    // priority: the smaller `Option<usize>` wins, same as any other scalar
    // tie-break axis (`equal_rank_scalar_tie_is_deterministic` above) — not
    // an arbitrary insertion-order pick, proven by building with the claims
    // in both orders and getting the identical winner either way.
    assert_eq!(
        edges_a[0].edge.provenance.ambiguous_candidates,
        Some(2),
        "the smaller candidate count wins the tie"
    );
    assert_eq!(
        edges_b[0].edge.provenance.ambiguous_candidates,
        Some(2),
        "insertion order must not change the tie outcome"
    );
    assert_eq!(
        edges_a[0].edge.dissent.len(),
        1,
        "the losing claim survives as dissent, not dropped"
    );
    assert_eq!(
        edges_a[0].edge.dissent[0].provenance.ambiguous_candidates,
        Some(3)
    );
}

/// Plan A, U1 (KTD1): two claims on one relation that tie on `resolution` are
/// decided by their confidence rung — the stronger one wins, the weaker one
/// survives as dissent.
///
/// The weaker claim is built to win every comparison the derived `Provenance`
/// order would make before it reached the rung: same file, same extractor, an
/// EARLIER line. A merge that left the tie to the full provenance order would
/// pick it, so this fails unless merge compares the rung explicitly. A claim
/// that carries a rung also beats one that carries none, whatever the lines
/// say. Built in both insertion orders, so the outcome is not an accident of
/// which claim arrived first.
#[test]
fn the_stronger_rung_wins_an_equal_resolution_tie_whatever_the_line_order() {
    use reverse_architect::graph::{Confidence, Edge, EdgeKind, Rule};
    let merged = |weaker: Provenance, stronger: Provenance, reversed: bool| {
        let mut g = Graph::new();
        let caller = NodeKey::new("sym:csharp:N#A.Go()");
        let callee = NodeKey::new("sym:csharp:N#B.Run()");
        g.ensure_node(caller.clone(), NodeKind::Symbol).unwrap();
        g.ensure_node(callee.clone(), NodeKind::Symbol).unwrap();
        let claims = if reversed {
            [stronger, weaker]
        } else {
            [weaker, stronger]
        };
        for claim in claims {
            g.add_edge(&caller, &callee, Edge::new(EdgeKind::Calls, claim));
        }
        merge(&mut g, &ranks(&[("csharp", 30)]));
        let edges = g.edges_sorted();
        assert_eq!(edges.len(), 1, "one relation collapses to one edge");
        let edge = edges[0].edge.clone();
        assert_eq!(edge.dissent.len(), 1, "the weaker claim is kept as dissent");
        (edge.provenance, edge.dissent[0].provenance.clone())
    };
    let at = |line| Provenance::new("A.cs", Some(line), "csharp".into(), Tier::Measured);

    for reversed in [false, true] {
        let (winner, loser) = merged(
            at(4).rated(Confidence::Ambiguous, Rule::NameMatch),
            at(9).rated(Confidence::RulesBacked, Rule::NameMatch),
            reversed,
        );
        assert_eq!(winner.confidence, Some(Confidence::RulesBacked));
        assert_eq!(winner.span.start(), Some(9));
        assert_eq!(loser.confidence, Some(Confidence::Ambiguous));

        let (winner, loser) = merged(
            at(4),
            at(9).rated(Confidence::NameOnly, Rule::NameMatch),
            reversed,
        );
        assert_eq!(
            winner.confidence,
            Some(Confidence::NameOnly),
            "a claim with a rung outranks one without"
        );
        assert_eq!(loser.confidence, None);

        // An undisputed name-only claim keeps the relation in the main lists,
        // so it must beat a site that left the same relation ambiguous.
        let (winner, loser) = merged(
            at(4).rated(Confidence::Ambiguous, Rule::NameMatch),
            at(9).rated(Confidence::NameOnly, Rule::NameMatch),
            reversed,
        );
        assert_eq!(winner.confidence, Some(Confidence::NameOnly));
        assert_eq!(loser.confidence, Some(Confidence::Ambiguous));
    }
}

/// Plan A, U3 (KTD5, KTD7): an edge is the summary of its sites. Two claims on
/// one relation — an ambiguous one read at lines 9 and 12, an undisputed one
/// read at line 4 and again at line 9 — merge into ONE edge whose rung is the
/// strongest of them and whose site list is the sorted union of both, with the
/// shared site once. A second merge changes nothing, in either insertion order.
#[test]
fn merging_claims_of_one_relation_unions_their_sites_and_keeps_the_strongest_rung() {
    use reverse_architect::graph::{Confidence, Edge, EdgeKind, Rule, SiteId};
    let site = |line| SiteId {
        file: "A.cs".to_string(),
        line,
        column: None,
    };
    let at = |line| Provenance::new("A.cs", Some(line), "csharp".into(), Tier::Measured);
    let merged = |reversed: bool, twice: bool| {
        let mut g = Graph::new();
        let caller = NodeKey::new("sym:csharp:N#A.Go()");
        let callee = NodeKey::new("sym:csharp:N#B.Run()");
        g.ensure_node(caller.clone(), NodeKind::Symbol).unwrap();
        g.ensure_node(callee.clone(), NodeKind::Symbol).unwrap();
        let ambiguous = Edge::new(
            EdgeKind::Calls,
            at(9)
                .rated(Confidence::Ambiguous, Rule::NameMatch)
                .ambiguous(2),
        )
        .at_sites([site(12), site(9)]);
        let undisputed = Edge::new(
            EdgeKind::Calls,
            at(4).rated(Confidence::NameOnly, Rule::NameMatch),
        )
        .at_sites([site(9), site(4)]);
        let claims = if reversed {
            [undisputed, ambiguous]
        } else {
            [ambiguous, undisputed]
        };
        for claim in claims {
            g.add_edge(&caller, &callee, claim);
        }
        let r = ranks(&[("csharp", 30)]);
        merge(&mut g, &r);
        if twice {
            merge(&mut g, &r);
        }
        let edges = g.edges_sorted();
        assert_eq!(edges.len(), 1, "one relation collapses to one edge");
        edges[0].edge.clone()
    };

    let once = merged(false, false);
    assert_eq!(once.provenance.rung(), Some(Confidence::NameOnly));
    assert_eq!(once.sites, [site(4), site(9), site(12)]);
    assert_eq!(once, merged(true, false), "insertion order does not matter");
    assert_eq!(once, merged(false, true), "merge twice == merge once");
}

/// Edge merge is idempotent with a NON-EMPTY dissent: a second merge re-pools the
/// winner with its own dissent and re-resolves to the identical edge. Without the
/// re-pooling, the loser would be silently dropped on the second pass.
#[test]
fn edge_merge_is_idempotent_with_dissent() {
    use reverse_architect::graph::{Edge, EdgeKind};
    let build = |extra_merge: bool| {
        let mut g = Graph::new();
        let a = NodeKey::new("a");
        let b = NodeKey::new("b");
        g.ensure_node(a.clone(), NodeKind::File).unwrap();
        g.ensure_node(b.clone(), NodeKind::File).unwrap();
        g.add_edge(
            &a,
            &b,
            Edge::new(
                EdgeKind::Imports,
                Provenance::new("a", Some(1), "treesitter".into(), Tier::Measured),
            ),
        );
        g.add_edge(
            &a,
            &b,
            Edge::new(
                EdgeKind::Imports,
                Provenance::new("a", Some(2), "scip".into(), Tier::Measured).resolved(),
            ),
        );
        let r = ranks(&[("treesitter", 30), ("scip", 50)]);
        merge(&mut g, &r);
        if extra_merge {
            merge(&mut g, &r);
        }
        format!(
            "{:?}",
            g.edges_sorted().iter().map(|e| e.edge).collect::<Vec<_>>()
        )
    };
    let once = build(false);
    assert!(
        once.contains("dissent: [Claim"),
        "precondition: the merged edge carries dissent — {once}"
    );
    assert_eq!(once, build(true), "merge twice == merge once");
}

/// A losing scalar claim survives all the way into the machine projection: the
/// manifest JSON carries a non-empty `dissent` array with the loser's value and
/// provenance (the plan's "manifest shows dissent on a conflicting fact"). This
/// is the guard that matters most for a unit whose purpose is "no data silently
/// dropped" — the graph-level tests never exercise a serialized non-empty
/// dissent.
#[test]
fn losing_scalar_claim_is_visible_in_the_manifest() {
    let mut g = Graph::new();
    let key = NodeKey::new("f");
    g.ensure_node(key.clone(), NodeKind::File).unwrap();
    // metrics (rank 25) outranks git (rank 20) under the real ladder, so the
    // git claim loses and must appear as dissent.
    g.add_fact(&key, fact(FactKind::Loc, FactValue::Int(3), "git"));
    g.add_fact(&key, fact(FactKind::Loc, FactValue::Int(9), "metrics"));

    merge(&mut g, &trust_ranks());
    let json = to_json(&build_manifest(&g, None).unwrap());

    // Winner value present, loser value + provenance retrievable in the JSON.
    let loc = g
        .node(&key)
        .unwrap()
        .facts
        .iter()
        .find(|f| f.kind == FactKind::Loc)
        .unwrap();
    assert_eq!(loc.value, FactValue::Int(9), "higher-rank metrics wins");
    assert_eq!(loc.dissent.len(), 1, "loser preserved as dissent");
    // Serialized shape, asserted on the PARSED value rather than on substrings
    // of the text: the artifact is written compact, and a test that matched
    // `"dissent": []` with its pretty-printer's spacing was testing the
    // formatter, not the manifest.
    let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
    let dissent = &parsed["nodes"][0]["facts"][0]["dissent"];
    assert_eq!(
        dissent.as_array().map(Vec::len),
        Some(1),
        "the conflicting fact's dissent is serialized non-empty: {json}"
    );
    let losing_git = loc
        .dissent
        .iter()
        .any(|c| c.value == FactValue::Int(3) && *c.provenance.extractor == *"git");
    assert!(losing_git, "the git claim survives with its provenance");
    // And it is actually in the serialized bytes, not just the in-memory graph.
    assert_eq!(
        dissent[0]["provenance"]["extractor"],
        serde_json::json!("git"),
        "losing claim provenance is serialized: {json}"
    );
}

// ---- End-to-end determinism over a real fixture (KTD7) --------------------

fn fixture(dir: &std::path::Path) {
    init_repo_with_files(
        dir,
        "Alice",
        &[
            ("src/lib.rs", "// lib\npub fn a() {}\n"),
            ("src/extract/git.rs", "// git\n"),
            ("README.md", "# readme\n"),
        ],
    );
}

fn config_for(dir: &std::path::Path) -> Config {
    Config {
        secret_prefixes: Vec::new(),
        plugins: Vec::new(),
        repo_root: dir.to_path_buf(),
        corpus_dir: dir.join("_corpus"),
        links: LinkStyle::Portable,
        profile: None,
        ambiguity_ceiling: reverse_architect::config::DEFAULT_AMBIGUITY_CEILING,
        scip_index: None,
        lang: "en".to_string(),
        sites: false,
        rights: reverse_architect::extract::RightsTable::granted(),
        model_verification: false,
        model_purpose: false,
        trace_over_ceiling: false,
    }
}

/// End-to-end: `generate()` (extraction + merge + projection) run twice over the
/// same fixture yields a byte-identical corpus AND manifest.
#[test]
fn generate_is_byte_identical_across_double_run() {
    let tmp = tempfile::tempdir().unwrap();
    fixture(tmp.path());
    let cfg = config_for(tmp.path());

    let first = reverse_architect::generate(&cfg).unwrap();
    let second = reverse_architect::generate(&cfg).unwrap();

    assert_eq!(first.corpus, second.corpus, "corpus is byte-identical");
    assert_eq!(
        to_json(&first.manifest),
        to_json(&second.manifest),
        "manifest is byte-identical"
    );
}

/// U4 verification: over a real fixture repo whose method name is overloaded, the
/// candidate count reaches the machine projection — a reader of the corpus can see
/// how ambiguous the call was, instead of trusting one arbitrary edge.
#[test]
fn an_overloaded_call_publishes_its_candidate_count_in_the_manifest() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            (
                "Caller.cs",
                "namespace N;\npublic class Caller {\n  public void Go() {\n    Save();\n  }\n}\n",
            ),
            (
                "Repo.cs",
                "namespace N;\npublic class Repo {\n  public void Save() {}\n  public void Save(int id) {}\n}\n",
            ),
        ],
    );
    let generated = reverse_architect::generate(&config_for(tmp.path())).unwrap();
    let json = to_json(&generated.manifest);

    assert!(
        json.contains("\"candidate_count\""),
        "the ambiguity is published, not hidden"
    );
    assert!(
        json.contains("Save: 2 candidates"),
        "both overloads are counted: {json}"
    );
    // Both candidates got their edge — two overloads are inside the ceiling.
    // Counted over the parsed edges, not over occurrences of a formatted
    // substring: the artifact is compact, and the count is a property of the
    // manifest rather than of its indentation.
    let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
    let calls = parsed["edges"]
        .as_array()
        .expect("edges is an array")
        .iter()
        .filter(|e| e["kind"] == serde_json::json!("calls"))
        .count();
    assert_eq!(calls, 2, "one edge per candidate below the ceiling");
}

/// The merge in `generate()` is idempotent end-to-end: running merge a second
/// time on the produced graph does not change the manifest.
#[test]
fn generate_then_extra_merge_is_stable() {
    let tmp = tempfile::tempdir().unwrap();
    fixture(tmp.path());

    let repo = Repo::open(tmp.path()).unwrap();
    let mut graph = Graph::new();
    run_all(&repo, &mut graph).unwrap();
    let r = trust_ranks();
    merge(&mut graph, &r);
    let once = to_json(&build_manifest(&graph, None).unwrap());
    merge(&mut graph, &r);
    let twice = to_json(&build_manifest(&graph, None).unwrap());
    assert_eq!(once, twice, "a redundant merge changes nothing");
}
