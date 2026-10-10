//! P3 integration tests: a plugin the operator declares takes part in a run
//! exactly as a bundled one does — its files, its facts, its language in the
//! coverage artifact, its identity in the manifest — and a declaration that
//! does not hold is refused before any file is read.

mod common;

use common::{config_for, init_repo_with_files};
use reverse_architect::plugin::PluginSpec;
use std::path::{Path, PathBuf};

fn probe() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("plugins/prebuilt/probe.wasm")
}

fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    init_repo_with_files(
        dir.path(),
        "a",
        &[
            ("src/a.probe", "anything\n"),
            ("src/lib.rs", "pub struct Helper;\n"),
        ],
    );
    dir
}

#[test]
fn a_declared_plugin_parses_its_files_beside_the_bundled_ones() {
    let repo = repo();
    let out = tempfile::tempdir().unwrap();
    let mut config = config_for(repo.path(), out.path());
    config.plugins = vec![PluginSpec {
        path: probe(),
        sha256: None,
    }];

    let generated = reverse_architect::generate(&config).unwrap();

    // Its symbol is in the graph, written under its own id.
    let key = reverse_architect::graph::NodeKey::new("sym:probe:src/a.probe#Thing");
    let node = generated
        .graph
        .node(&key)
        .expect("the declared plugin's symbol must reach the graph");
    assert!(node
        .facts
        .iter()
        .all(|f| &*f.provenance.extractor == "probe"));
    // …and the bundled plugins still ran.
    assert!(generated
        .graph
        .node(&reverse_architect::graph::NodeKey::new(
            "sym:rust:crate#Helper"
        ))
        .is_some());

    // Coverage names the file under the plugin's language, as parsed.
    let file = generated
        .coverage
        .files
        .iter()
        .find(|f| f.path == "src/a.probe")
        .unwrap();
    assert_eq!(file.language.as_deref(), Some("probe"));
    // The probe returns a symbol but not the `declares` edge from its file,
    // which the contract leaves to the plugin (`wit/extractor.wit`, pass 1). The
    // artifact says so rather than crediting the file with structure it was
    // never given.
    assert!(!file.structural);
    assert_eq!(
        file.not_structural_because.as_deref(),
        Some(reverse_architect::coverage::NO_DECLARATION)
    );

    // The manifest says which plugins built the corpus, declared first.
    let plugins = &generated.manifest.plugins;
    assert_eq!(plugins[0].id, "probe");
    assert_eq!(plugins[0].origin, "declared");
    assert_eq!(plugins[0].sha256.len(), 64);
    assert!(plugins[1..].iter().all(|p| p.origin == "bundled"));
    assert!(plugins.iter().any(|p| p.id == "rust"));
}

/// Coverage says under which plugin contract each language was read, and
/// counts the run's plugins per contract (plan A, U5). The probe is built
/// against the frozen 0.1 contract, beside the bundled plugins.
#[test]
fn coverage_names_the_contract_each_language_was_parsed_under() {
    let repo = repo();
    let out = tempfile::tempdir().unwrap();
    let mut config = config_for(repo.path(), out.path());
    config.plugins = vec![PluginSpec {
        path: probe(),
        sha256: None,
    }];

    let coverage = reverse_architect::generate(&config).unwrap().coverage;

    let contracts = |name: &str| -> Vec<String> {
        coverage
            .languages
            .iter()
            .find(|l| l.language == name)
            .unwrap_or_else(|| panic!("no `{name}` in {:?}", coverage.languages))
            .contracts
            .clone()
    };
    assert_eq!(contracts("probe"), ["0.1"]);
    assert_eq!(contracts("rust"), ["0.2"]);
    // The probe and the SQL example on 0.1, the six bundled tree-sitter
    // families on 0.2.
    let noise = coverage.noise.expect("every run writes noise");
    assert_eq!(
        noise.plugins_by_contract,
        [("0.1".to_string(), 2), ("0.2".to_string(), 6)]
            .into_iter()
            .collect()
    );
}

#[test]
fn a_declared_digest_that_does_not_match_fails_the_run_before_it_starts() {
    let repo = repo();
    let out = tempfile::tempdir().unwrap();
    let mut config = config_for(repo.path(), out.path());
    config.plugins = vec![PluginSpec {
        path: probe(),
        sha256: Some("0".repeat(64)),
    }];

    let err = reverse_architect::generate(&config)
        .map(|_| ())
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("does not have the declared digest"),
        "{err}"
    );
}

#[test]
fn a_run_without_declared_plugins_records_only_the_bundled_ones() {
    let repo = repo();
    let out = tempfile::tempdir().unwrap();
    let generated = reverse_architect::generate(&config_for(repo.path(), out.path())).unwrap();
    let ids: Vec<&str> = generated
        .manifest
        .plugins
        .iter()
        .map(|p| p.id.as_str())
        .collect();
    assert_eq!(
        ids,
        [
            "csharp",
            "ecmascript",
            "go",
            "java",
            "python",
            "rust",
            "sql"
        ]
    );
    // A file no plugin claims is still reported, not dropped.
    let file = generated
        .coverage
        .files
        .iter()
        .find(|f| f.path == "src/a.probe")
        .unwrap();
    assert_eq!(file.language, None);
}

#[test]
fn a_file_the_plugin_fails_on_says_so_in_coverage_and_the_run_goes_on() {
    let repo = tempfile::tempdir().unwrap();
    init_repo_with_files(
        repo.path(),
        "a",
        &[
            // The probe panics on exactly this path.
            ("trap.probe", "x\n"),
            ("fine.probe", "x\n"),
        ],
    );
    let out = tempfile::tempdir().unwrap();
    let mut config = config_for(repo.path(), out.path());
    config.plugins = vec![PluginSpec {
        path: probe(),
        sha256: None,
    }];

    let generated = reverse_architect::generate(&config).unwrap();
    let file = |path: &str| {
        generated
            .coverage
            .files
            .iter()
            .find(|f| f.path == path)
            .unwrap()
            .clone()
    };

    let failed = file("trap.probe");
    let reason = failed.not_structural_because.unwrap();
    assert!(reason.contains("`probe` failed"), "{reason}");
    assert!(reason.contains("declarations"), "{reason}");
    assert!(
        !reason.contains(reverse_architect::coverage::NO_DECLARATION),
        "a file nothing finished reading must not be said to declare nothing"
    );
    assert!(failed.extractors.contains(&"probe".to_string()));

    // The file's card is what a reader meets first; it must not print the
    // wire word, and it must not claim the file declares nothing.
    let card = generated
        .cards
        .iter()
        .find(|c| c.path.ends_with("trap.probe.md"))
        .expect("a failed file still gets its card");
    assert!(
        !card.contents.contains("extraction_failed"),
        "{}",
        card.contents
    );
    assert!(
        card.contents
            .contains("the `probe` plugin failed on this file"),
        "{}",
        card.contents
    );

    // The failure cost exactly one file.
    assert!(generated
        .graph
        .node(&reverse_architect::graph::NodeKey::new(
            "sym:probe:fine.probe#Thing"
        ))
        .is_some());
}

/// The bundled SQL plugin — built outside the plugin workspace, from the WIT
/// alone — links a script's objects across files the way a language plugin
/// links types: a procedure to the table it reads, a trigger to its table.
#[test]
fn sql_objects_are_cards_linked_to_the_tables_they_touch() {
    let repo = tempfile::tempdir().unwrap();
    init_repo_with_files(
        repo.path(),
        "a",
        &[
            (
                "db/tables.sql",
                "-- Orders placed through the shop.\nCREATE TABLE orders (\n  id INT,\n  total INT\n);\n",
            ),
            (
                "db/procs.sql",
                "CREATE PROCEDURE archive_orders AS\n  SELECT * FROM orders;\n\ncreate trigger orders_audit on orders for insert as\n  exec archive_orders;\n",
            ),
        ],
    );
    let out = tempfile::tempdir().unwrap();
    let generated = reverse_architect::generate(&config_for(repo.path(), out.path())).unwrap();
    let graph = &generated.graph;
    let key = reverse_architect::graph::NodeKey::new;

    assert!(graph.node(&key("sym:sql:_#orders.total")).is_some());
    let edges: Vec<(String, String, String)> = graph
        .edges_sorted()
        .into_iter()
        .map(|e| {
            (
                e.from.as_str().to_string(),
                e.to.as_str().to_string(),
                format!("{:?}", e.edge.kind),
            )
        })
        .collect();
    for (from, to, kind) in [
        ("sym:sql:_#archive_orders", "sym:sql:_#orders", "References"),
        ("sym:sql:_#orders_audit", "sym:sql:_#orders", "References"),
        (
            "sym:sql:_#orders_audit",
            "sym:sql:_#archive_orders",
            "Calls",
        ),
        ("db/tables.sql", "sym:sql:_#orders", "Declares"),
    ] {
        assert!(
            edges.contains(&(from.to_string(), to.to_string(), kind.to_string())),
            "missing {from} -{kind}-> {to}: {edges:?}"
        );
    }

    let file = generated
        .coverage
        .files
        .iter()
        .find(|f| f.path == "db/tables.sql")
        .unwrap();
    assert_eq!(file.language.as_deref(), Some("sql"));
    assert!(file.structural);

    // The table earns a card of its own.
    assert!(
        generated
            .cards
            .iter()
            .any(|c| c.path.contains("orders")
                && c.contents.contains("Orders placed through the shop")),
        "no card for the table"
    );
}

fn probe3() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("plugins/prebuilt/probe3.wasm")
}

/// Contract 0.3: a component declares a node kind with a card policy and
/// emits a symbol of that kind. The graph holds it as a node of the declared
/// kind, not as a core `Symbol`, and it gets the card its policy names. A
/// symbol of a kind the component's vocabulary does not declare — or of
/// another namespace's — is dropped whole; an ordinary symbol beside it is
/// not. The second run is served from the cache, which must carry the kind.
#[test]
fn a_symbol_of_a_declared_node_kind_gets_its_own_card_on_a_cold_and_a_warm_run() {
    use reverse_architect::card::{build_cards, CardKind};
    use reverse_architect::graph::{NodeKey, NodeKind};

    let repo = tempfile::tempdir().unwrap();
    init_repo_with_files(
        repo.path(),
        "a",
        &[("widget.probe3", "w\n"), ("stray.probe3", "s\n")],
    );
    let out = tempfile::tempdir().unwrap();
    let mut config = config_for(repo.path(), out.path());
    config.plugins = vec![PluginSpec {
        path: probe3(),
        sha256: None,
    }];

    for run in ["cold", "warm"] {
        let generated = reverse_architect::generate(&config).unwrap();
        if run == "cold" {
            // The warm run proves something only if the kind was written to
            // the declaration records it is served from.
            assert!(
                cache_holds(&out.path().join(".ra-cache"), "\"node_kinds\""),
                "the cold run wrote no declaration record carrying the node kind"
            );
        }
        let graph = &generated.graph;
        let key = NodeKey::new("sym:probe3:widget.probe3#Main");
        let node = graph
            .node(&key)
            .unwrap_or_else(|| panic!("{run}: the widget must reach the graph"));
        let NodeKind::Declared(id) = node.kind else {
            panic!(
                "{run}: the widget is a {:?}, not its declared kind",
                node.kind
            );
        };
        assert_eq!(
            graph.vocabulary().node_name(id).as_deref(),
            Some("probe3:widget"),
            "{run}"
        );

        let cards = build_cards(graph);
        let card = cards
            .iter()
            .find(|c| c.identity == key.as_str())
            .unwrap_or_else(|| panic!("{run}: the widget has no card"));
        assert!(matches!(card.kind, CardKind::Declared(_)), "{run}");
        assert_eq!(card.kind_word, "probe3:widget", "{run}");

        for dropped in ["Stray", "Foreign"] {
            assert!(
                graph
                    .node(&NodeKey::new(format!("sym:probe3:stray.probe3#{dropped}")))
                    .is_none(),
                "{run}: a symbol of an undeclared kind must be dropped whole"
            );
        }
        let plain = graph
            .node(&NodeKey::new("sym:probe3:stray.probe3#Plain"))
            .unwrap_or_else(|| panic!("{run}: the ordinary symbol must survive"));
        assert_eq!(plain.kind, NodeKind::Symbol, "{run}");
    }
}

/// Contract 0.3: a component that returns `err` for a file fails that file
/// with its own reason, and `coverage.json` carries the reason rather than a
/// trap; the next file of the same run is read as usual.
#[test]
fn a_file_the_plugin_refuses_carries_the_plugins_reason_in_coverage() {
    let repo = tempfile::tempdir().unwrap();
    init_repo_with_files(
        repo.path(),
        "a",
        &[("refuse.probe3", "x\n"), ("fine.probe3", "x\n")],
    );
    let out = tempfile::tempdir().unwrap();
    let mut config = config_for(repo.path(), out.path());
    config.plugins = vec![PluginSpec {
        path: probe3(),
        sha256: None,
    }];

    let generated = reverse_architect::generate(&config).unwrap();
    let refused = generated
        .coverage
        .files
        .iter()
        .find(|f| f.path == "refuse.probe3")
        .unwrap();
    let reason = refused.not_structural_because.as_deref().unwrap();
    assert!(
        reason.contains("declarations: declarations: probe3 cannot read this file"),
        "{reason}"
    );
    assert!(reason.contains("`probe3` failed"), "{reason}");
    assert!(!reason.contains("trapped"), "{reason}");

    // The artifact as written says the same.
    let json = reverse_architect::coverage::to_json(&generated.coverage);
    let written: serde_json::Value = serde_json::from_str(&json).unwrap();
    let file = written["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["path"] == "refuse.probe3")
        .unwrap();
    assert_eq!(file["not_structural_because"], reason);

    assert!(generated
        .graph
        .node(&reverse_architect::graph::NodeKey::new(
            "sym:probe3:fine.probe3#Thing"
        ))
        .is_some());
}

/// Whether any file under `dir` contains `needle`.
fn cache_holds(dir: &Path, needle: &str) -> bool {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .any(|entry| {
            let path = entry.path();
            if path.is_dir() {
                cache_holds(&path, needle)
            } else {
                std::fs::read(&path)
                    .map(|bytes| String::from_utf8_lossy(&bytes).contains(needle))
                    .unwrap_or(false)
            }
        })
}
