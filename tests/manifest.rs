//! U5 integration tests: the JSON manifest projection of the graph.

mod common;

use common::init_repo_with_files;
use reverse_architect::extract::{run_all, trust_ranks, Repo};
use reverse_architect::graph::{merge, Graph};
use reverse_architect::manifest::{
    build_manifest, owned_filenames, should_write_line_delimited, to_documents_jsonl,
    to_edges_jsonl, to_json, to_json_pretty, to_nodes_jsonl, to_strings_json, Manifest,
    ManifestEdge, ManifestNode, ManifestStrings, DOCUMENTS_FILENAME, EDGES_FILENAME,
    LINE_DELIMITED_THRESHOLD, NODES_FILENAME, SCHEMA_VERSION, STRINGS_FILENAME,
};
use std::path::Path;

fn manifest_over(dir: &Path) -> Manifest {
    let repo = Repo::open(dir).unwrap();
    let mut graph = Graph::new();
    run_all(&repo, &mut graph).unwrap();
    // Match the real pipeline (`generate`): merge before projecting, so the
    // golden manifest reflects what a user actually gets (KTD-B5).
    merge(&mut graph, &trust_ranks());
    build_manifest(&graph, None).unwrap()
}

fn standard_fixture(dir: &Path) {
    init_repo_with_files(
        dir,
        "Alice",
        &[
            ("src/lib.rs", "// lib"),
            ("src/extract/git.rs", "// git"),
            ("README.md", "# readme"),
        ],
    );
}

#[test]
fn has_a_document_per_leaf_with_matching_guiding_questions() {
    let tmp = tempfile::tempdir().unwrap();
    standard_fixture(tmp.path());
    let manifest = manifest_over(tmp.path());

    let ids: Vec<&str> = manifest.documents.iter().map(|d| d.id.as_str()).collect();
    assert!(ids.contains(&"overview"));
    assert!(ids.contains(&"module-structure"));

    let overview = manifest
        .documents
        .iter()
        .find(|d| d.id == "overview")
        .unwrap();
    assert_eq!(
        overview.guiding_question,
        "What is this repository, and how is it shaped at the top level?"
    );
    assert_eq!(overview.path, "overview.md");
    assert!(!overview.chunks.is_empty());
}

#[test]
fn every_edge_carries_provenance() {
    let tmp = tempfile::tempdir().unwrap();
    standard_fixture(tmp.path());
    let manifest = manifest_over(tmp.path());

    assert!(!manifest.edges.is_empty());
    for edge in &manifest.edges {
        assert!(!edge.provenance.extractor.is_empty());
        assert!(!manifest.string(edge.provenance.file_id).is_empty());
    }
}

#[test]
fn canonical_edges_live_in_the_manifest_even_when_absent_from_leaf_bodies() {
    let tmp = tempfile::tempdir().unwrap();
    standard_fixture(tmp.path());
    let manifest = manifest_over(tmp.path());

    // A Contains edge the leaf prose never enumerates is still canonical here.
    assert!(manifest
        .edges
        .iter()
        .any(|e| manifest.string(e.from_id) == "src"
            && manifest.string(e.to_id) == "src/extract"
            && e.kind == "contains"));
}

#[test]
fn facts_carry_provenance_and_values() {
    let tmp = tempfile::tempdir().unwrap();
    standard_fixture(tmp.path());
    let manifest = manifest_over(tmp.path());

    let lib = manifest
        .nodes
        .iter()
        .find(|n| manifest.string(n.key_id) == "src/lib.rs")
        .unwrap();
    // `loc` rather than the `commit_count` this used to read: history extraction
    // is gone, and the property under test was never about git — it is that a
    // fact on the wire carries its value, the extractor that asserted it and the
    // tier it sits on.
    let loc = lib
        .facts
        .iter()
        .find(|f| f.kind == "loc")
        .unwrap_or_else(|| panic!("src/lib.rs carries no loc fact: {:?}", lib.facts));
    assert!(
        loc.value.as_u64().is_some_and(|n| n > 0),
        "a line count is a positive number: {:?}",
        loc.value
    );
    assert_eq!(&*loc.provenance.extractor, "metrics");
    assert_eq!(loc.provenance.tier, "measured");
}

#[test]
fn manifest_is_valid_json_and_round_trips() {
    let tmp = tempfile::tempdir().unwrap();
    standard_fixture(tmp.path());
    let manifest = manifest_over(tmp.path());

    let json = to_json(&manifest);
    let parsed: Manifest = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, manifest);
    assert_eq!(parsed.schema_version, SCHEMA_VERSION);
}

#[test]
fn manifest_is_deterministic_across_builds() {
    let tmp = tempfile::tempdir().unwrap();
    standard_fixture(tmp.path());

    let a = to_json(&manifest_over(tmp.path()));
    let b = to_json(&manifest_over(tmp.path()));
    assert_eq!(a, b);
}

/// Cross-prefix: identical content at two different absolute paths must produce
/// a byte-identical manifest (repo-root-relative paths, KTD7).
#[test]
fn manifest_is_identical_across_absolute_prefixes() {
    let one = tempfile::tempdir().unwrap();
    standard_fixture(one.path());
    let two = tempfile::tempdir().unwrap();
    standard_fixture(two.path());

    assert_eq!(
        to_json(&manifest_over(one.path())),
        to_json(&manifest_over(two.path()))
    );
}

#[test]
fn manifest_has_a_document_for_every_backbone_view() {
    // U10: the full backbone shows up as retrievable documents, deps and all.
    let tmp = tempfile::tempdir().unwrap();
    standard_fixture(tmp.path());
    let manifest = manifest_over(tmp.path());

    let ids: Vec<&str> = manifest.documents.iter().map(|d| d.id.as_str()).collect();
    for id in [
        "overview",
        "module-structure",
        "dependencies",
        "entry-points",
        "test-coverage",
    ] {
        assert!(ids.contains(&id), "missing document {id}; have {ids:?}");
    }
}

#[test]
fn diagram_chunk_is_present_only_for_a_leaf_that_has_a_diagram() {
    // KTD-C2/B1: a leaf carrying a diagram must expose a distinct, skippable
    // `diagram` chunk so a RAG consumer can target or skip the mermaid instead of
    // it folding into the Mechanism chunk. A leaf without one must not.
    let tmp = tempfile::tempdir().unwrap();
    standard_fixture(tmp.path());
    let manifest = manifest_over(tmp.path());

    // module-structure has Contains edges → a diagram → a diagram chunk.
    let ms = manifest
        .documents
        .iter()
        .find(|d| d.id == "module-structure")
        .unwrap();
    let ms_headings: Vec<&str> = ms.chunks.iter().map(|c| c.heading.as_str()).collect();
    assert_eq!(
        ms_headings,
        ["Problem", "Mechanism", "Diagram", "In this repository"],
        "diagram chunk slotted after Mechanism"
    );
    assert!(ms
        .chunks
        .iter()
        .any(|c| c.id == "module-structure#diagram" && c.heading == "Diagram"));

    // overview has no diagram fn → no diagram chunk.
    let ov = manifest
        .documents
        .iter()
        .find(|d| d.id == "overview")
        .unwrap();
    assert!(
        !ov.chunks.iter().any(|c| c.heading == "Diagram"),
        "no diagram chunk without a diagram"
    );
}

/// R4: the precision axis is not optional decoration — every fact AND every edge
/// carries a `resolution`. Asserted over the serialized JSON (not just the typed
/// struct) because the RAG consumer reads the wire form, and a `skip_serializing_if`
/// slipped onto the field would be invisible to a field-access assertion.
#[test]
fn every_fact_and_edge_provenance_carries_a_resolution() {
    let tmp = tempfile::tempdir().unwrap();
    standard_fixture(tmp.path());
    let manifest = manifest_over(tmp.path());

    let json = to_json(&manifest);
    let provenances = json.matches("\"extractor\"").count();
    let resolutions = json.matches("\"resolution\"").count();
    assert!(provenances > 0, "fixture produced no provenance at all");
    assert_eq!(
        resolutions, provenances,
        "every serialized provenance must carry a resolution"
    );

    // Nothing in Tier 0 resolves through an external index yet, so the honest
    // value everywhere is `heuristic` — never a silently defaulted `resolved`
    // (R20: "used nowhere" may only be claimed inside resolved scope).
    for edge in &manifest.edges {
        assert_eq!(edge.provenance.resolution, "heuristic");
        assert_eq!(edge.provenance.end_line, None);
    }
    for node in &manifest.nodes {
        for fact in &node.facts {
            assert_eq!(fact.provenance.resolution, "heuristic");
        }
    }
}

/// A provenance anchored to a line RANGE round-trips both ends; one without a
/// range omits `end_line` from the wire form entirely rather than writing `null`.
#[test]
fn manifest_provenance_round_trips_with_and_without_a_line_range() {
    use reverse_architect::manifest::ManifestProvenance;

    let ranged = ManifestProvenance {
        file_id: 0,
        line: Some(10),
        end_line: Some(42),
        extractor: "treesitter".to_string(),
        tier: "measured".to_string(),
        resolution: "resolved".to_string(),
        ambiguous_candidates: None,
        confidence: None,
        rule_id: None,
    };
    let json = serde_json::to_string(&ranged).unwrap();
    assert!(json.contains("\"end_line\":42"), "{json}");
    assert_eq!(
        serde_json::from_str::<ManifestProvenance>(&json).unwrap(),
        ranged
    );

    let unranged = ManifestProvenance {
        end_line: None,
        ..ranged
    };
    let json = serde_json::to_string(&unranged).unwrap();
    assert!(!json.contains("end_line"), "{json}");
    assert_eq!(
        serde_json::from_str::<ManifestProvenance>(&json).unwrap(),
        unranged
    );
}

/// `ambiguous_candidates` follows the same additive pattern as `end_line`
/// (see the test above): present and round-tripping when `Some`, entirely
/// absent from the wire form — not `null` — when `None`. A manifest written
/// before this field existed has neither key and still deserializes, which is
/// why `SCHEMA_VERSION` did not need a bump for it (KTD4).
#[test]
fn manifest_provenance_round_trips_with_and_without_ambiguous_candidates() {
    use reverse_architect::manifest::ManifestProvenance;

    let ambiguous = ManifestProvenance {
        file_id: 0,
        line: Some(10),
        end_line: None,
        extractor: "treesitter".to_string(),
        tier: "measured".to_string(),
        resolution: "heuristic".to_string(),
        ambiguous_candidates: Some(3),
        confidence: None,
        rule_id: None,
    };
    let json = serde_json::to_string(&ambiguous).unwrap();
    assert!(json.contains("\"ambiguous_candidates\":3"), "{json}");
    assert_eq!(
        serde_json::from_str::<ManifestProvenance>(&json).unwrap(),
        ambiguous
    );

    let unambiguous = ManifestProvenance {
        ambiguous_candidates: None,
        ..ambiguous
    };
    let json = serde_json::to_string(&unambiguous).unwrap();
    assert!(!json.contains("ambiguous_candidates"), "{json}");
    assert_eq!(
        serde_json::from_str::<ManifestProvenance>(&json).unwrap(),
        unambiguous
    );

    // A manifest from before this field existed has no such key at all.
    let pre_field_json = r#"{"file_id":0,"line":10,"extractor":"treesitter","tier":"heuristic","resolution":"heuristic"}"#;
    let decoded: ManifestProvenance = serde_json::from_str(pre_field_json).unwrap();
    assert_eq!(decoded.ambiguous_candidates, None);
}

// ---- The confidence rung and the rule (plan A, U1: R1, R2, KTD1) ----

/// One repository with every shape the rung is about: a C# call whose name two
/// overloads declare (`Save`), a C# call only one declaration answers (`Load`),
/// a base type found by its unique name in another namespace, and a TypeScript
/// import, so a structural `imports` edge sits beside the name-resolved ones.
fn rung_fixture(dir: &Path) {
    init_repo_with_files(
        dir,
        "Alice",
        &[
            (
                "Caller.cs",
                "namespace N;\npublic class Caller {\n  public void Go() {\n    Save();\n    Load();\n  }\n}\n",
            ),
            (
                "Repo.cs",
                "namespace N;\npublic class Repo : Store {\n  public void Save() {}\n  public void Save(int id) {}\n  public void Load() {}\n}\n",
            ),
            // In ANOTHER namespace, so `: Store` finds no spelled candidate and
            // resolves through the repo-wide unique type name (`unique_base`).
            ("Store.cs", "namespace M;\npublic class Store {}\n"),
            ("web/a.ts", "import { b } from \"./b\";\nexport function a() {}\n"),
            ("web/b.ts", "export function b() {}\n"),
        ],
    );
}

/// The `calls` edges leaving `from`, as `(callee key, confidence, rule id)`.
fn calls_from(manifest: &Manifest, from: &str) -> Vec<(String, Option<String>, Option<String>)> {
    let mut out: Vec<_> = manifest
        .edges
        .iter()
        .filter(|e| e.kind == "calls" && manifest.string(e.from_id) == from)
        .map(|e| {
            (
                manifest.string(e.to_id).to_string(),
                e.provenance.confidence.clone(),
                e.provenance
                    .rule_id
                    .map(|id| manifest.string(id).to_string()),
            )
        })
        .collect();
    out.sort();
    out
}

/// R2: a call two declarations answer is ambiguous on BOTH of its edges. The
/// overload the argument count rules out names `arity`, the rule that
/// disputed it (plan A, U7); the other, which no rule decided, names
/// `name-match`. A call one declaration answers is `name_only`, and it says so
/// rather than carrying no rung at all.
#[test]
fn a_call_two_overloads_answer_is_ambiguous_and_a_single_candidate_is_name_only() {
    let tmp = tempfile::tempdir().unwrap();
    rung_fixture(tmp.path());
    let manifest = manifest_over(tmp.path());

    let ambiguous = Some("ambiguous".to_string());
    let name_only = Some("name_only".to_string());
    let name_match = Some("name-match".to_string());
    assert_eq!(
        calls_from(&manifest, "sym:csharp:N#Caller.Go()"),
        vec![
            (
                "sym:csharp:N#Repo.Load()".to_string(),
                name_only,
                name_match.clone()
            ),
            (
                "sym:csharp:N#Repo.Save()".to_string(),
                ambiguous.clone(),
                name_match.clone()
            ),
            (
                "sym:csharp:N#Repo.Save(int)".to_string(),
                ambiguous,
                Some("arity".to_string())
            ),
        ]
    );
}

/// Plan A, U3 (KTD7): an edge is a relation that carries the sorted list of
/// the sites it was read at. A member called from three lines is ONE edge with
/// three sites; the two alternatives of one ambiguous site are two edges that
/// share that site; a structural edge carries none and writes no `sites` key.
#[test]
fn an_edge_carries_the_sorted_sites_it_was_read_at() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo_with_files(
        tmp.path(),
        "Alice",
        &[
            (
                "Caller.cs",
                "namespace N;\npublic class Caller {\n  public void Go() {\n    Load();\n    Save();\n    Load();\n    Load();\n  }\n}\n",
            ),
            (
                "Repo.cs",
                "namespace N;\npublic class Repo {\n  public void Save() {}\n  public void Save(int id) {}\n  public void Load() {}\n}\n",
            ),
        ],
    );
    let manifest = manifest_over(tmp.path());
    let sites_of = |to: &str| -> Vec<(String, u32, Option<u32>)> {
        let edge = manifest
            .edges
            .iter()
            .find(|e| {
                e.kind == "calls"
                    && manifest.string(e.from_id) == "sym:csharp:N#Caller.Go()"
                    && manifest.string(e.to_id) == to
            })
            .unwrap_or_else(|| panic!("no calls edge to {to}"));
        edge.sites
            .iter()
            .map(|s| (manifest.string(s.file_id).to_string(), s.line, s.column))
            .collect()
    };
    // Every call is indented by four spaces, so its name starts at column 5,
    // which the C# plugin reports since contract 0.2 (plan A, U6).
    let at = |line| ("Caller.cs".to_string(), line, Some(5));

    assert_eq!(sites_of("sym:csharp:N#Repo.Load()"), [at(4), at(6), at(7)]);
    assert_eq!(sites_of("sym:csharp:N#Repo.Save()"), [at(5)]);
    assert_eq!(
        sites_of("sym:csharp:N#Repo.Save(int)"),
        [at(5)],
        "the alternatives of one ambiguous site share its id"
    );

    let json = to_json(&manifest);
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    for edge in parsed["edges"].as_array().unwrap() {
        let kind = edge["kind"].as_str().unwrap();
        if ["contains", "declares", "has_member", "imports"].contains(&kind) {
            assert!(edge.get("sites").is_none(), "{kind} carries sites: {edge}");
        }
        if let Some(sites) = edge.get("sites") {
            for site in sites.as_array().unwrap() {
                // The C# plugin reports a column since contract 0.2; a site
                // without one writes no key (`SiteId`'s own test).
                assert_eq!(site["column"], 5, "{site}");
            }
        }
    }
}

/// An inheritance edge is name-resolved too: a base found by the one type the
/// repository declares under that name is `name_only`, by `name-match`.
#[test]
fn a_base_type_found_by_its_unique_name_is_name_only() {
    let tmp = tempfile::tempdir().unwrap();
    rung_fixture(tmp.path());
    let manifest = manifest_over(tmp.path());

    let extends: Vec<_> = manifest
        .edges
        .iter()
        .filter(|e| e.kind == "extends")
        .map(|e| {
            (
                manifest.string(e.from_id),
                manifest.string(e.to_id),
                e.provenance.confidence.as_deref(),
                e.provenance.rule_id.map(|id| manifest.string(id)),
            )
        })
        .collect();
    assert_eq!(
        extends,
        [(
            "sym:csharp:N#Repo",
            "sym:csharp:M#Store",
            Some("name_only"),
            Some("name-match")
        )]
    );
}

/// KTD1: the rung is a property of NAME-RESOLVED relations only. A structural
/// edge — `contains`, `declares`, `has_member`, `imports` — and every fact carry
/// neither key on the wire, so their bytes are the bytes the pre-rung binary
/// wrote. Asserted over the parsed JSON, because a consumer reads the wire
/// form and a field that serialized as `null` would pass a typed check.
#[test]
fn structural_edges_and_facts_carry_no_rung_and_no_rule() {
    let tmp = tempfile::tempdir().unwrap();
    rung_fixture(tmp.path());
    let manifest = manifest_over(tmp.path());
    let parsed: serde_json::Value = serde_json::from_str(&to_json(&manifest)).unwrap();

    let mut structural_kinds = std::collections::BTreeSet::new();
    for edge in parsed["edges"].as_array().unwrap() {
        let kind = edge["kind"].as_str().unwrap();
        if matches!(kind, "contains" | "declares" | "has_member" | "imports") {
            structural_kinds.insert(kind.to_string());
            let provenance = edge["provenance"].as_object().unwrap();
            assert!(
                !provenance.contains_key("confidence") && !provenance.contains_key("rule_id"),
                "a {kind} edge must not carry a rung: {edge}"
            );
        }
    }
    assert_eq!(
        structural_kinds.into_iter().collect::<Vec<_>>(),
        ["contains", "declares", "has_member", "imports"],
        "the fixture must produce every structural kind, or this proves nothing"
    );
    for node in parsed["nodes"].as_array().unwrap() {
        for fact in node["facts"].as_array().unwrap() {
            let provenance = fact["provenance"].as_object().unwrap();
            assert!(
                !provenance.contains_key("confidence") && !provenance.contains_key("rule_id"),
                "a fact must not carry a rung: {fact}"
            );
        }
    }
}

/// KTD1: an index-resolved relation reads `resolved_by_index` in the manifest,
/// and that rung is DERIVED from `resolution`, never stored beside it — the
/// graph's provenance holds no rung at all, so the two cannot disagree. It
/// names no rule either: no rule admitted it, the index did. A resolved FACT
/// gains nothing, because the rung describes relations.
#[test]
fn an_index_resolved_edge_reads_resolved_by_index_without_storing_the_rung() {
    use reverse_architect::graph::{
        Edge, EdgeKind, Fact, FactKind, FactValue, NodeKey, NodeKind, Provenance, Tier,
    };
    let mut g = Graph::new();
    let caller = NodeKey::new("sym:csharp:N#A.Go()");
    let callee = NodeKey::new("sym:csharp:N#B.Run()");
    g.ensure_node(caller.clone(), NodeKind::Symbol).unwrap();
    g.ensure_node(callee.clone(), NodeKind::Symbol).unwrap();
    let resolved = Provenance::new("A.cs", Some(4), "scip".into(), Tier::Measured).resolved();
    assert_eq!(resolved.confidence, None, "nothing stores the index rung");
    g.add_edge(
        &caller,
        &callee,
        Edge::new(EdgeKind::Calls, resolved.clone()),
    );
    g.add_fact(
        &callee,
        Fact::new(
            FactKind::SymbolMoniker,
            FactValue::Text("scip-dotnet N/B#Run().".to_string()),
            resolved,
        ),
    );

    let manifest = build_manifest(&g, None).unwrap();
    assert_eq!(
        manifest.edges[0].provenance.confidence.as_deref(),
        Some("resolved_by_index")
    );
    assert_eq!(manifest.edges[0].provenance.rule_id, None);
    let fact = &manifest
        .nodes
        .iter()
        .find(|n| manifest.string(n.key_id) == callee.as_str())
        .unwrap()
        .facts[0];
    assert_eq!(fact.provenance.confidence, None, "facts carry no rung");
}

/// The two new keys follow the `ambiguous_candidates` pattern: written when
/// set, absent — not `null` — when not, and a manifest written before they
/// existed still decodes, with both `None`.
#[test]
fn manifest_provenance_round_trips_with_and_without_a_rung() {
    use reverse_architect::manifest::ManifestProvenance;

    let rated = ManifestProvenance {
        file_id: 0,
        line: Some(10),
        end_line: None,
        extractor: "csharp".to_string(),
        tier: "measured".to_string(),
        resolution: "heuristic".to_string(),
        ambiguous_candidates: Some(2),
        confidence: Some("ambiguous".to_string()),
        rule_id: Some(1),
    };
    let json = serde_json::to_string(&rated).unwrap();
    assert!(json.contains("\"confidence\":\"ambiguous\""), "{json}");
    assert!(json.contains("\"rule_id\":1"), "{json}");
    assert_eq!(
        serde_json::from_str::<ManifestProvenance>(&json).unwrap(),
        rated
    );

    let unrated = ManifestProvenance {
        confidence: None,
        rule_id: None,
        ..rated
    };
    let json = serde_json::to_string(&unrated).unwrap();
    assert!(
        !json.contains("confidence") && !json.contains("rule_id"),
        "{json}"
    );

    let pre_field_json = r#"{"file_id":0,"line":10,"extractor":"csharp","tier":"measured","resolution":"heuristic"}"#;
    let decoded: ManifestProvenance = serde_json::from_str(pre_field_json).unwrap();
    assert_eq!((decoded.confidence, decoded.rule_id), (None, None));
}

/// R22: two runs over the same repository give the same manifest bytes, rungs
/// and interned rule ids included.
#[test]
fn the_rung_and_the_rule_are_byte_identical_across_two_runs() {
    let tmp = tempfile::tempdir().unwrap();
    rung_fixture(tmp.path());
    let first = to_json(&manifest_over(tmp.path()));
    let second = to_json(&manifest_over(tmp.path()));
    assert!(
        first.contains("\"confidence\":\"ambiguous\""),
        "precondition: the fixture produces a rung"
    );
    assert_eq!(first, second);
}

#[test]
fn golden_manifest_snapshot() {
    let tmp = tempfile::tempdir().unwrap();
    standard_fixture(tmp.path());
    let manifest = manifest_over(tmp.path());

    // Pretty here, compact in the artifact: a snapshot diff ten million
    // characters wide on one line is a snapshot nobody can approve.
    insta::assert_snapshot!("phase_a_manifest", to_json_pretty(&manifest));
}

/// F1/KTD-C5 crux: under a profile, the corpus and the manifest MUST select the
/// same view set. Threading the profile into only one projection would make the
/// RAG-indexed manifest list a different set than the corpus — a silent
/// divergence the compiler cannot catch. Drive the real `generate` and assert
/// the manifest's document ids equal the corpus leaf ids.
#[test]
fn corpus_and_manifest_agree_on_the_view_set_under_a_profile() {
    use reverse_architect::config::Config;

    let tmp = tempfile::tempdir().unwrap();
    standard_fixture(tmp.path());

    let cfg = Config {
        secret_prefixes: Vec::new(),
        plugins: Vec::new(),
        repo_root: tmp.path().to_path_buf(),
        corpus_dir: tmp.path().join("_corpus"),
        links: reverse_architect::cli::LinkStyle::Portable,
        profile: Some("minimal".to_string()),
        ambiguity_ceiling: reverse_architect::config::DEFAULT_AMBIGUITY_CEILING,
        scip_index: None,
        lang: "en".to_string(),
        sites: false,
        rights: reverse_architect::extract::RightsTable::granted(),
        model_verification: false,
        model_purpose: false,
        trace_over_ceiling: false,
    };
    let generated = reverse_architect::generate(&cfg).unwrap();

    // Corpus leaves = every `.md` file except the `index.md` map and the
    // architecture pages (card schema v4). The pages are not views: a profile
    // narrows views only and the pages are always written. The manifest lists
    // them — and every card — as documents with a `kind` (v8); a view document
    // has none, so the view comparison below filters on that.
    use reverse_architect::card::pages::PAGE_FILENAMES;
    let mut corpus_leaves: Vec<String> = generated
        .corpus
        .files
        .iter()
        .filter(|f| {
            f.path != "index.md"
                && f.path.ends_with(".md")
                && !PAGE_FILENAMES.contains(&f.path.as_str())
        })
        .map(|f| f.path.trim_end_matches(".md").to_string())
        .collect();
    corpus_leaves.sort();
    for page in PAGE_FILENAMES {
        assert!(
            generated.corpus.files.iter().any(|f| f.path == page),
            "{page} is written under a profile too"
        );
    }

    let mut manifest_ids: Vec<String> = generated
        .manifest
        .documents
        .iter()
        .filter(|d| d.kind.is_none())
        .map(|d| d.id.clone())
        .collect();
    for page in PAGE_FILENAMES {
        let id = page.trim_end_matches(".md");
        assert!(
            generated
                .manifest
                .documents
                .iter()
                .any(|d| d.id == id && d.kind.as_deref() == Some("page")),
            "{page} is a manifest document of kind `page`"
        );
    }
    assert!(
        generated
            .manifest
            .documents
            .iter()
            .any(|d| d.kind.as_deref() == Some("card:module")),
        "module cards are manifest documents"
    );
    manifest_ids.sort();

    // Both projections honored the same profile → identical view sets.
    assert_eq!(
        corpus_leaves, manifest_ids,
        "corpus leaves and manifest documents must match under a profile"
    );
    // And it is actually the minimal profile's set, not the full backbone.
    assert_eq!(manifest_ids, vec!["module-structure", "overview"]);
}

/// Drift guard: the chunk headings the manifest declares must be exactly the
/// `## ` headings the renderer actually emits for the same lens.
#[test]
fn chunk_headings_match_the_rendered_leaf_headings() {
    let tmp = tempfile::tempdir().unwrap();
    standard_fixture(tmp.path());

    let repo = Repo::open(tmp.path()).unwrap();
    let mut graph = Graph::new();
    run_all(&repo, &mut graph).unwrap();
    merge(&mut graph, &trust_ranks());
    let manifest = build_manifest(&graph, None).unwrap();

    for view in reverse_architect::taxonomy::select_views(&graph, None).unwrap() {
        let lens = reverse_architect::lens::compose(&view, &graph);
        let rendered = reverse_architect::render::markdown::render_leaf(&lens);
        let rendered_headings: Vec<&str> = rendered
            .lines()
            .filter_map(|l| l.strip_prefix("## "))
            .collect();

        let doc = manifest
            .documents
            .iter()
            .find(|d| d.id == lens.view_id)
            .unwrap();
        let chunk_headings: Vec<&str> = doc.chunks.iter().map(|c| c.heading.as_str()).collect();

        assert_eq!(chunk_headings, rendered_headings, "view {}", lens.view_id);
    }
}

#[test]
fn the_line_delimited_threshold_switches_exactly_at_its_boundary() {
    assert!(!should_write_line_delimited(LINE_DELIMITED_THRESHOLD - 1));
    assert!(!should_write_line_delimited(LINE_DELIMITED_THRESHOLD));
    assert!(should_write_line_delimited(LINE_DELIMITED_THRESHOLD + 1));
}

#[test]
fn every_line_of_the_line_delimited_export_is_independently_valid_json() {
    let tmp = tempfile::tempdir().unwrap();
    standard_fixture(tmp.path());
    let manifest = manifest_over(tmp.path());

    for line in to_strings_json(&manifest).lines() {
        serde_json::from_str::<serde_json::Value>(line).expect("strings line is valid JSON");
    }
    for line in to_nodes_jsonl(&manifest).lines() {
        serde_json::from_str::<serde_json::Value>(line).expect("node line is valid JSON");
    }
    for line in to_edges_jsonl(&manifest).lines() {
        serde_json::from_str::<serde_json::Value>(line).expect("edge line is valid JSON");
    }
    for line in to_documents_jsonl(&manifest).lines() {
        serde_json::from_str::<serde_json::Value>(line).expect("document line is valid JSON");
    }
}

#[test]
fn the_line_delimited_export_round_trips_to_the_same_ids_and_content_as_the_monolith() {
    let tmp = tempfile::tempdir().unwrap();
    standard_fixture(tmp.path());
    let manifest = manifest_over(tmp.path());

    let strings: ManifestStrings = serde_json::from_str(&to_strings_json(&manifest)).unwrap();
    assert_eq!(strings.schema_version, manifest.schema_version);
    assert_eq!(strings.strings, manifest.strings);

    let nodes: Vec<ManifestNode> = to_nodes_jsonl(&manifest)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(nodes, manifest.nodes);

    let edges: Vec<ManifestEdge> = to_edges_jsonl(&manifest)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(edges, manifest.edges);

    // The _id references are the SAME indices as the monolithic form -- a
    // consumer that loads manifest-strings.json once and streams the rest
    // resolves every key_id/from_id/to_id/file_id exactly as to_json() would.
    for node in &nodes {
        assert!(strings.strings.get(node.key_id).is_some());
    }
}

#[test]
fn two_runs_over_one_repository_render_a_byte_identical_line_delimited_export() {
    let tmp = tempfile::tempdir().unwrap();
    standard_fixture(tmp.path());
    let first = manifest_over(tmp.path());
    let second = manifest_over(tmp.path());

    assert_eq!(to_strings_json(&first), to_strings_json(&second));
    assert_eq!(to_nodes_jsonl(&first), to_nodes_jsonl(&second));
    assert_eq!(to_edges_jsonl(&first), to_edges_jsonl(&second));
    assert_eq!(to_documents_jsonl(&first), to_documents_jsonl(&second));
}

#[test]
fn line_delimited_filenames_are_distinct_and_carry_the_manifest_prefix() {
    let names = [
        STRINGS_FILENAME,
        NODES_FILENAME,
        EDGES_FILENAME,
        DOCUMENTS_FILENAME,
    ];
    let unique: std::collections::HashSet<&str> = names.iter().copied().collect();
    assert_eq!(unique.len(), names.len(), "filenames must be distinct");
    for name in names {
        assert!(name.starts_with("manifest-"), "{name}");
    }
}

/// The branch `write_corpus`/`run_enrich` actually take is `line_delimited`
/// (a `bool`); nothing about it forces a 50,000+ node fixture to exercise
/// directly, since `owned_filenames` is the pure decision the branch defers to.
#[test]
fn owned_filenames_names_the_monolith_below_the_threshold() {
    assert_eq!(owned_filenames(false), vec!["manifest.json"]);
}

#[test]
fn owned_filenames_names_all_four_line_delimited_files_above_the_threshold() {
    assert_eq!(
        owned_filenames(true),
        vec![
            STRINGS_FILENAME,
            NODES_FILENAME,
            EDGES_FILENAME,
            DOCUMENTS_FILENAME
        ]
    );
}

/// A static class with an offered method — which earns it a detail — in a
/// module of its own, so the run has a map, a type card and a detail of one
/// entity (plan B, U11).
fn three_level_fixture(dir: &Path) {
    init_repo_with_files(
        dir,
        "Alice",
        &[(
            "src/Geo/GeoSolver.cs",
            "namespace Orbit.Geo\n\
             {\n\
             \x20   /// <summary>Solves the directives of a definition.</summary>\n\
             \x20   public static class GeoSolver\n\
             \x20   {\n\
             \x20       public static int Solve(int x)\n\
             \x20       {\n\
             \x20           return x;\n\
             \x20       }\n\
             \x20   }\n\
             }\n",
        )],
    );
}

fn generated_three_levels(dir: &Path) -> reverse_architect::Generated {
    reverse_architect::generate(&common::config_for(dir, &dir.join("_corpus"))).unwrap()
}

fn doc<'a>(manifest: &'a Manifest, id: &str) -> &'a reverse_architect::manifest::ManifestDoc {
    manifest
        .documents
        .iter()
        .find(|d| d.id == id)
        .unwrap_or_else(|| panic!("no document {id}"))
}

const SOLVER: &str = "sym:csharp:Orbit.Geo#GeoSolver";

/// Plan B, U11 (KTD9, R4, R16): the detail is a document of its own kind,
/// one level below its card, never embedded; the map is the level above.
#[test]
fn a_detail_and_a_map_are_documents_with_their_level_parent_and_embed() {
    let tmp = tempfile::tempdir().unwrap();
    three_level_fixture(tmp.path());
    let generated = generated_three_levels(tmp.path());
    let manifest = &generated.manifest;

    let card = doc(manifest, SOLVER);
    assert_eq!(card.kind.as_deref(), Some("card:type"));
    assert_eq!(card.level.as_deref(), Some("card"));
    assert_eq!(card.parent.as_deref(), Some("map:src/Geo"));
    assert_eq!(card.embed, None, "a card may be embedded");

    let detail = doc(manifest, &format!("detail:{SOLVER}"));
    assert_eq!(detail.kind.as_deref(), Some("card:detail"));
    assert_eq!(detail.level.as_deref(), Some("detail"));
    assert_eq!(detail.parent.as_deref(), Some(SOLVER));
    assert_eq!(detail.embed, Some(false));
    assert_eq!(
        detail.path, "cards/detail/type/csharp/Orbit.Geo/GeoSolver.md",
        "the detail document names the file the run writes"
    );
    assert!(
        !detail.chunks.is_empty(),
        "a detail is chunked by its sections"
    );

    let map = doc(manifest, "map:src/Geo");
    assert_eq!(map.kind.as_deref(), Some("map"));
    assert_eq!(map.level.as_deref(), Some("map"));
    assert_eq!(map.parent.as_deref(), Some("src/Geo"), "the module card");
    assert_eq!(map.embed, None, "a map may be embedded");
    assert_eq!(map.path, "cards/map/src/Geo.md");

    // Every rendered card, map and detail has its document, and nothing
    // else is one: the documents name exactly the files the run writes.
    let paths = |level: &str| -> Vec<String> {
        let mut v: Vec<String> = manifest
            .documents
            .iter()
            .filter(|d| d.level.as_deref() == Some(level))
            .map(|d| d.path.clone())
            .collect();
        v.sort();
        v
    };
    let written = |files: &[reverse_architect::render::RenderedFile]| -> Vec<String> {
        let mut v: Vec<String> = files.iter().map(|f| f.path.clone()).collect();
        v.sort();
        v
    };
    assert_eq!(paths("detail"), written(&generated.details));
    assert_eq!(paths("map"), written(&generated.maps));
    assert_eq!(paths("card"), written(&generated.cards));
    assert!(manifest
        .documents
        .iter()
        .filter(|d| d.level.as_deref() == Some("detail"))
        .all(|d| d.embed == Some(false)));
    assert!(manifest
        .documents
        .iter()
        .filter(|d| d.level.as_deref() != Some("detail"))
        .all(|d| d.embed.is_none()));

    // On the wire: `embed:false` on the detail's line, and no new field on
    // a view's.
    let jsonl = to_documents_jsonl(manifest);
    let line = |id: &str| -> String {
        let head = format!("{{\"id\":{}", serde_json::to_string(id).unwrap());
        jsonl
            .lines()
            .find(|l| l.starts_with(&head))
            .unwrap_or_else(|| panic!("no line for {id}"))
            .to_string()
    };
    assert!(line(&format!("detail:{SOLVER}")).contains("\"embed\":false"));
    assert!(!line(SOLVER).contains("\"embed\""));
    let overview = line("overview");
    for field in ["\"level\"", "\"parent\"", "\"embed\"", "\"header\""] {
        assert!(
            !overview.contains(field),
            "a view carries no {field}: {overview}"
        );
    }
}

/// R4: the full context header is on the card, the detail and the map, as
/// fields — and it says what the signature says (AE3).
#[test]
fn every_card_detail_and_map_document_carries_the_full_context_header() {
    let tmp = tempfile::tempdir().unwrap();
    three_level_fixture(tmp.path());
    let manifest = generated_three_levels(tmp.path()).manifest;

    let header = doc(&manifest, SOLVER)
        .header
        .clone()
        .expect("a card's header");
    assert_eq!(header.path, "src/Geo/GeoSolver.cs");
    assert_eq!(header.module.as_deref(), Some("src/Geo"));
    assert_eq!(header.namespace.as_deref(), Some("Orbit.Geo"));
    assert_eq!(header.kind, "public static class");
    assert_eq!(header.name, "GeoSolver");
    assert_eq!(
        header.purpose.as_deref(),
        Some("Solves the directives of a definition.")
    );
    assert_eq!(
        doc(&manifest, &format!("detail:{SOLVER}")).header.as_ref(),
        Some(&header),
        "a detail is about the same entity as its card"
    );

    let map = doc(&manifest, "map:src/Geo")
        .header
        .clone()
        .expect("a map's header");
    assert_eq!(map.path, "src/Geo");
    assert_eq!(map.module.as_deref(), Some("src/Geo"));
    assert_eq!(map.namespace, None);
    assert_eq!(map.kind, "module");
    assert_eq!(
        doc(&manifest, "src/Geo").header.as_ref(),
        Some(&map),
        "a map's header is its module card's"
    );

    // Every card, map and detail has one; no view and no page does.
    for d in &manifest.documents {
        assert_eq!(
            d.header.is_some(),
            d.level.is_some(),
            "header on exactly the three levels: {}",
            d.id
        );
    }
}

/// KTD9: ids are unique across documents and chunks — a detail's id is not
/// one of its card's chunk ids — every `parent` names a document, and the
/// ids are the same on a second run, so an importer can key on them.
#[test]
fn document_ids_are_unique_resolvable_and_stable_across_runs() {
    let tmp = tempfile::tempdir().unwrap();
    three_level_fixture(tmp.path());
    let first = generated_three_levels(tmp.path()).manifest;

    let mut ids: Vec<&str> = Vec::new();
    for d in &first.documents {
        ids.push(&d.id);
        ids.extend(d.chunks.iter().map(|c| c.id.as_str()));
    }
    let unique: std::collections::BTreeSet<&str> = ids.iter().copied().collect();
    assert_eq!(unique.len(), ids.len(), "a document or chunk id repeats");
    for d in &first.documents {
        if let Some(parent) = &d.parent {
            assert!(
                first.documents.iter().any(|p| &p.id == parent),
                "{} names a parent no document has: {parent}",
                d.id
            );
        }
    }

    let second = generated_three_levels(tmp.path()).manifest;
    assert_eq!(first.documents, second.documents);
}
