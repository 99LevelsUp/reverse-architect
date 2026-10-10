//! The second layer of a run (`--detail-level`, contract 0.3) must leave the
//! first one alone: a run without it writes the same bytes as before the layer
//! existed.
//!
//! The gate is a fingerprint of every file a plain run writes, taken over the
//! six-grammar fixture (`tests/common/six_grammar.rs`): eight languages, C#
//! among them, so the fingerprint covers type cards, details and maps, not
//! only the views. It holds no `.xaml`, so no markup plugin of a later unit
//! can take part in it.
//!
//! The run goes through the binary, as an operator's does, so the fingerprint
//! is of what reaches the disk. It is taken twice, on one thread and on many,
//! from two repositories at two paths: a fingerprint that depended on the
//! scheduler or on where the checkout lies would be no gate at all.

mod common;

use assert_cmd::Command;
use common::output_files;
use sha2::{Digest, Sha256};
use std::path::Path;

/// Every file a plain run over `repo` writes into `out`, on `threads` worker
/// threads. The binary is a process of its own, so `RAYON_NUM_THREADS` sizes
/// its pool for this run alone.
fn plain_run(repo: &Path, out: &Path, threads: usize) -> Vec<(String, Vec<u8>)> {
    Command::cargo_bin("reverse-architect")
        .unwrap()
        .env("RAYON_NUM_THREADS", threads.to_string())
        .arg(repo)
        .arg("--out")
        .arg(out)
        .assert()
        .success();
    output_files(out)
}

/// `<sha256>  <path>` per file, sorted by path — the shape `sha256sum` prints.
fn fingerprint(files: &[(String, Vec<u8>)]) -> String {
    files
        .iter()
        .map(|(path, bytes)| format!("{:x}  {path}\n", Sha256::digest(bytes)))
        .collect()
}

#[test]
fn a_plain_run_writes_the_pinned_bytes_on_one_thread_and_on_many() {
    assert!(
        common::six_grammar::files()
            .iter()
            .all(|(path, _)| !path.ends_with(".xaml")),
        "the layer-1 fixture must not hold markup a later plugin could read"
    );

    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    common::six_grammar::init(&first.path().join("repo"));
    common::six_grammar::init(&second.path().join("checkout"));

    let one = plain_run(&first.path().join("repo"), &first.path().join("corpus"), 1);
    let many = plain_run(
        &second.path().join("checkout"),
        &second.path().join("out"),
        8,
    );

    // Guard the guard: a corpus without type cards would pin nothing the
    // second layer could disturb.
    assert!(
        one.iter().any(|(path, _)| path.starts_with("cards/type/")),
        "the fixture must produce type cards"
    );
    assert!(
        one.iter()
            .any(|(path, _)| path.starts_with("cards/detail/type/")),
        "the fixture must produce details"
    );

    let a = fingerprint(&one);
    let b = fingerprint(&many);
    assert_eq!(a, b, "the corpus depends on the thread count or the paths");

    // The manifest records every plugin the run loaded — all the bundled
    // ones, not only those that read a file.
    let manifest = &one
        .iter()
        .find(|(path, _)| path == "manifest.json")
        .expect("a manifest")
        .1;
    let manifest: serde_json::Value = serde_json::from_slice(manifest).unwrap();
    let plugins: Vec<&str> = manifest["plugins"]
        .as_array()
        .expect("the manifest lists its plugins")
        .iter()
        .map(|p| p["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        plugins,
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

    insta::assert_snapshot!("layer1_corpus_fingerprint", a);
}

// ---- The second layer (plan 2a, U4) ----

use reverse_architect::config::Config;
use reverse_architect::extract::bodies::{BodyKind, PairedBy, SecondLayer};
use reverse_architect::graph::{FactKind, FactValue, NodeKey};
use reverse_architect::plugin::PluginSpec;
use reverse_architect::{generate, Generated};
use std::collections::BTreeMap;
use std::time::SystemTime;

const CACHE: &str = ".ra-cache";

fn config(repo: &Path, out: &Path, level: Option<u8>) -> Config {
    Config {
        detail_level: level,
        ..common::config_for(repo, out)
    }
}

fn prebuilt(name: &str) -> PluginSpec {
    PluginSpec {
        path: Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("plugins/prebuilt")
            .join(name),
        sha256: None,
    }
}

/// `value` without the keys named, at the top level or one level down
/// (`noise.plugins_by_contract`).
fn without(mut value: serde_json::Value, keys: &[&str]) -> serde_json::Value {
    for key in keys {
        match key.split_once('.') {
            Some((outer, inner)) => {
                value[outer].as_object_mut().unwrap().remove(inner);
            }
            None => {
                value.as_object_mut().unwrap().remove(*key);
            }
        }
    }
    value
}

fn assert_same_first_layer(plain: &Generated, layered: &Generated) {
    assert_eq!(plain.corpus, layered.corpus, "the views differ");
    assert_eq!(plain.cards, layered.cards, "the cards differ");
    assert_eq!(plain.details, layered.details, "the details differ");
    assert_eq!(plain.maps, layered.maps, "the maps differ");
}

/// With `--detail-level` every view, card, detail and map is what a plain run
/// writes; the manifest gains the level and the body component and nothing
/// else, and coverage the `bodies` section and the component's contract.
#[test]
fn the_second_layer_leaves_every_first_layer_output_as_it_was() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    common::six_grammar::init(&repo);
    let plain = generate(&config(&repo, &dir.path().join("plain"), None)).unwrap();
    let layered = generate(&config(&repo, &dir.path().join("layered"), Some(2))).unwrap();

    assert_same_first_layer(&plain, &layered);
    assert!(plain.bodies.is_none());
    let layer = layered.bodies.as_ref().expect("the second layer");
    assert!(
        layer.files().count() > 0,
        "the fixture's C# bodies were read"
    );
    assert_eq!(layer.failures().count(), 0);

    let plain_manifest = serde_json::to_value(&plain.manifest).unwrap();
    let manifest = serde_json::to_value(&layered.manifest).unwrap();
    assert!(plain_manifest.get("detail_level").is_none());
    assert_eq!(manifest["detail_level"], 2);
    let ids = |m: &serde_json::Value| -> Vec<String> {
        m["plugins"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["id"].as_str().unwrap().to_string())
            .collect()
    };
    let mut expected = ids(&plain_manifest);
    expected.push("csharp-body".into());
    assert_eq!(ids(&manifest), expected);
    let component = &manifest["plugins"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    assert_eq!(component["origin"], "bundled");
    assert_eq!(component["languages"], serde_json::json!(["csharp"]));
    assert_eq!(
        without(plain_manifest, &["plugins"]),
        without(manifest, &["plugins", "detail_level"])
    );

    let plain_coverage = serde_json::to_value(&plain.coverage).unwrap();
    let coverage = serde_json::to_value(&layered.coverage).unwrap();
    assert!(plain_coverage.get("bodies").is_none());
    assert_eq!(coverage["bodies"]["detail_level"], 2);
    assert_eq!(
        coverage["bodies"]["plugins"],
        serde_json::json!(["csharp-body"])
    );
    let contracts = |c: &serde_json::Value| c["noise"]["plugins_by_contract"].clone();
    let mut more = contracts(&plain_coverage);
    more["0.3"] = serde_json::json!(more["0.3"].as_u64().unwrap_or(0) + 1);
    assert_eq!(contracts(&coverage), more);
    assert_eq!(
        without(plain_coverage, &["noise.plugins_by_contract"]),
        without(coverage, &["bodies", "noise.plugins_by_contract"])
    );
}

/// Every record of `phase` under `out`'s cache: its modification time and
/// its bytes, by file name.
fn records(out: &Path, phase: &str) -> BTreeMap<String, (SystemTime, Vec<u8>)> {
    let dir = out.join(CACHE).join(phase);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return BTreeMap::new();
    };
    entries
        .map(|e| {
            let path = e.unwrap().path();
            let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
            (
                path.file_name().unwrap().to_string_lossy().into_owned(),
                (modified, std::fs::read(&path).unwrap()),
            )
        })
        .collect()
}

fn header(out: &Path) -> Vec<u8> {
    std::fs::read(out.join(CACHE).join("fingerprint.json")).unwrap()
}

const VIEW: &str = "namespace Acme.Views\n{\n    public class EditorView\n    {\n        private int count;\n        public void Check() { if (count == 0) { Save(); } }\n        public void Save() { count = 1; }\n    }\n}\n";

fn layered_repo(dir: &Path) -> std::path::PathBuf {
    let repo = dir.join("repo");
    common::init_repo_with_files(
        &repo,
        "Alice",
        &[
            ("src/EditorView.cs", VIEW),
            ("src/lib.rs", "pub fn f() -> i32 { 1 }\n"),
        ],
    );
    repo
}

/// Plain, then with the second layer, then plain again over one corpus
/// directory: the header never changes and the first layer's records are
/// written once and never again. Level 1 and level 3 share the second
/// layer's records too.
#[test]
fn switching_the_second_layer_keeps_the_first_layers_records_warm() {
    let dir = tempfile::tempdir().unwrap();
    let repo = layered_repo(dir.path());
    let out = dir.path().join("out");
    let first_layer = |out: &Path| {
        let mut all = BTreeMap::new();
        for phase in ["languages-decl", "languages-usages"] {
            for (name, record) in records(out, phase) {
                all.insert(format!("{phase}/{name}"), record);
            }
        }
        all
    };

    generate(&config(&repo, &out, None)).unwrap();
    let head = header(&out);
    let warm = first_layer(&out);
    assert!(warm.len() >= 3, "{:?}", warm.keys());
    assert!(records(&out, "languages-bodies").is_empty());

    generate(&config(&repo, &out, Some(1))).unwrap();
    assert_eq!(header(&out), head);
    assert_eq!(first_layer(&out), warm, "a layered run rewrote layer 1");
    let bodies = records(&out, "languages-bodies");
    assert_eq!(bodies.len(), 1, "the one C# file");

    let level3 = generate(&config(&repo, &out, Some(3))).unwrap();
    assert_eq!(header(&out), head);
    assert_eq!(records(&out, "languages-bodies"), bodies, "level 3 missed");
    assert_eq!(level3.bodies.unwrap().level(), 3);

    generate(&config(&repo, &out, None)).unwrap();
    assert_eq!(header(&out), head);
    assert_eq!(first_layer(&out), warm, "a plain run rewrote layer 1");
    assert_eq!(records(&out, "languages-bodies"), bodies);
}

/// A rebuilt body component — simulated by putting another digest into the
/// tag of every record it wrote — misses its records, which are read and
/// written again, while the first layer's stay warm.
#[test]
fn a_rebuilt_body_component_misses_its_records_and_nothing_else() {
    let dir = tempfile::tempdir().unwrap();
    let repo = layered_repo(dir.path());
    let out = dir.path().join("out");
    let before = generate(&config(&repo, &out, Some(2))).unwrap();
    let first_layer = |out: &Path| {
        let mut all = BTreeMap::new();
        for phase in ["languages-decl", "languages-usages"] {
            for (name, record) in records(out, phase) {
                all.insert(format!("{phase}/{name}"), record);
            }
        }
        all
    };
    let warm = first_layer(&out);
    assert!(warm.len() >= 3, "{:?}", warm.keys());
    let bodies_dir = out.join(CACHE).join("languages-bodies");
    let mut real_tags = Vec::new();
    for (name, (_, bytes)) in records(&out, "languages-bodies") {
        let mut record: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let tag = record["fingerprint"].as_str().unwrap().to_string();
        let (head, digest) = tag.rsplit_once("csharp-body@").expect("a component tag");
        assert_eq!(digest.len(), 64, "{tag}");
        record["fingerprint"] = format!("{head}csharp-body@{}", "0".repeat(64)).into();
        std::fs::write(bodies_dir.join(&name), serde_json::to_vec(&record).unwrap()).unwrap();
        real_tags.push(tag);
    }
    assert!(!real_tags.is_empty());

    let after = generate(&config(&repo, &out, Some(2))).unwrap();
    let tags: Vec<String> = records(&out, "languages-bodies")
        .into_values()
        .map(|(_, bytes)| {
            let record: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            record["fingerprint"].as_str().unwrap().to_string()
        })
        .collect();
    assert_eq!(tags, real_tags, "the stale records were not read again");
    assert_eq!(first_layer(&out), warm);
    assert_eq!(before.bodies, after.bodies);
}

/// A component of another language (probe3, contract 0.3) next to the 0.1
/// probe: a symbol of its own node kind takes the body read around it; a
/// body in a file whose declarations carry no name position pairs with
/// nothing and is in coverage; a file whose bodies run out of fuel has the
/// second layer's failure fact, its coverage row and no second layer — and
/// every card, detail and map is a plain run's.
#[test]
fn another_languages_bodies_pair_fail_and_are_counted() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    common::init_repo_with_files(
        &repo,
        "Alice",
        &[
            ("widget.probe3", "x"),
            ("loop.probe3", "x"),
            ("a.probe", "x"),
        ],
    );
    let with_probes = |out: &str, level| Config {
        plugins: vec![prebuilt("probe.wasm"), prebuilt("probe3.wasm")],
        ..config(&repo, &dir.path().join(out), level)
    };
    let plain = generate(&with_probes("plain", None)).unwrap();
    let layered = generate(&with_probes("layered", Some(1))).unwrap();
    assert_same_first_layer(&plain, &layered);
    let layer = layered.bodies.as_ref().unwrap();

    let widget = NodeKey::new("sym:probe3:widget.probe3#Main");
    assert!(layered.graph.node(&widget).is_some());
    assert_eq!(layer.layer_of(&widget), SecondLayer::Present);
    let bodies = layer.bodies_of(&widget);
    assert_eq!(bodies.len(), 1, "{bodies:?}");
    assert_eq!(bodies[0].by, PairedBy::Enclosing);
    assert_eq!(bodies[0].body.kind, BodyKind::Method);

    let looping = NodeKey::new("sym:probe3:loop.probe3#Thing");
    assert_eq!(layer.layer_of(&looping), SecondLayer::Failed);
    // The type whose bodies failed has its first-layer card, the same in
    // both runs (compared above with every other card).
    let card = |g: &Generated| {
        g.cards
            .iter()
            .find(|c| c.path.starts_with("cards/type/") && c.contents.contains("loop.probe3"))
            .map(|c| c.contents.clone())
    };
    assert!(card(&layered).is_some(), "the failed type has a card");
    assert_eq!(card(&plain), card(&layered));
    assert!(layer.bodies_of(&looping).is_empty());
    let file = layered.graph.node(&NodeKey::new("loop.probe3")).unwrap();
    let failed: Vec<_> = file
        .facts
        .iter()
        .filter(|f| f.kind == FactKind::BodyExtractionFailed)
        .collect();
    assert_eq!(failed.len(), 1);
    assert_eq!(
        failed[0].value,
        FactValue::Text("bodies: fuel exhausted".into())
    );
    assert!(file
        .facts
        .iter()
        .all(|f| f.kind != FactKind::ExtractionFailed));

    let coverage = layered.coverage.bodies.as_ref().unwrap();
    assert_eq!(coverage.plugins, ["probe3", "csharp-body"]);
    assert_eq!(coverage.failed.len(), 1);
    assert_eq!(coverage.failed[0].path, "loop.probe3");
    assert_eq!(coverage.failed[0].plugin, "probe3");
    assert_eq!(coverage.unpaired.len(), 1, "{:?}", coverage.unpaired);
    assert_eq!(coverage.unpaired[0].path, "a.probe");
    assert_eq!(
        (coverage.unpaired[0].line, coverage.unpaired[0].column),
        (1, 1)
    );
    assert_eq!(coverage.paired_by_enclosing, 1);
    // The failure is the second layer's: the file's first-layer coverage
    // row says what a plain run's says about its structure.
    let row = |g: &Generated| {
        g.coverage
            .files
            .iter()
            .find(|f| f.path == "loop.probe3")
            .map(|f| (f.structural, f.not_structural_because.clone()))
    };
    assert_eq!(row(&plain), row(&layered));
}

/// Two members on one line, behind a comment with letters outside ASCII, and
/// two declarations under one key (`X` and `X~2`) — two members the branches
/// of an `#if` would leave, written here without the directives, since the
/// first layer reads no member inside an `#if` region, and the two halves of
/// a partial type in one file: each pairs by the position of its own name,
/// counted in characters on both layers.
#[test]
fn bodies_pair_by_name_position_on_a_shared_line_and_under_one_key() {
    let line = "        /* čeština */ public int A => 1; public int B => 2;";
    let src = format!(
        "namespace Acme.Views\n{{\n    public class EditorView\n    {{\n{line}\n        // DEBUG\n        public void Run() {{ Check(); }}\n        // RELEASE\n        public void Run() {{ Save(); }}\n        public void Check() {{ }}\n        public void Save() {{ }}\n    }}\n    public partial class ViewC {{ public void M() {{ }} }}\n    public partial class ViewC {{ public void N() {{ }} }}\n}}\n"
    );
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    common::init_repo_with_files(&repo, "Alice", &[("src/EditorView.cs", &src)]);
    let generated = generate(&config(&repo, &dir.path().join("out"), Some(2))).unwrap();
    let layer = generated.bodies.as_ref().unwrap();
    let column = |name: &str| {
        let at = line.find(&format!(" {name} =>")).unwrap() + 1;
        line[..at].chars().count() as u32 + 1
    };
    for name in ["A", "B"] {
        let keys = common::symbol_nodes(&generated.graph, name);
        assert_eq!(keys.len(), 1, "{name}: {keys:?}");
        let bodies = layer.bodies_of(&keys[0]);
        assert_eq!(bodies.len(), 1, "{name}: {bodies:?}");
        assert_eq!(bodies[0].by, PairedBy::Name, "{name}");
        assert_eq!(bodies[0].body.kind, BodyKind::ExpressionProperty);
        let at = bodies[0].body.name_position;
        assert_eq!((at.line, at.column), (5, column(name)), "{name}");
    }
    let runs = common::symbol_nodes(&generated.graph, "Run");
    assert_eq!(runs.len(), 2, "{runs:?}");
    assert!(runs.iter().any(|k| k.as_str().ends_with("~2")), "{runs:?}");
    let mut lines: Vec<u32> = runs
        .iter()
        .map(|key| {
            let bodies = layer.bodies_of(key);
            assert_eq!(bodies.len(), 1, "{key:?}: {bodies:?}");
            assert_eq!(bodies[0].by, PairedBy::Name, "{key:?}");
            bodies[0].body.name_position.line
        })
        .collect();
    lines.sort();
    assert_eq!(lines, [7, 9]);
    let halves: Vec<u32> = [
        "sym:csharp:Acme.Views#ViewC",
        "sym:csharp:Acme.Views#ViewC~2",
    ]
    .into_iter()
    .map(|key| {
        let shapes = layer.type_shapes_of(&NodeKey::new(key));
        assert_eq!(shapes.len(), 1, "{key}: {shapes:?}");
        shapes[0].1.name_position.line
    })
    .collect();
    assert_eq!(halves, [13, 14]);
    assert!(layer.unpaired().is_empty());
}

/// A literal shaped like a secret in a body — a public token shape and the
/// operator's own prefix, one in a `return` and one in a condition — is in
/// no record of the second layer and in no file of the corpus.
#[test]
fn no_secret_from_a_body_reaches_the_cache_or_the_corpus() {
    let token = "ghp_Q7rT2mX9vB4nL6kP1sD8fH3jW5zC0aE";
    let own = "acme_Zx81Lq0Wv3Nd5Rt7Yp2Kd";
    let src = format!(
        "namespace Acme.Views\n{{\n    public class Vault\n    {{\n        public string Token() {{ return \"{token}\"; }}\n        public bool Is(string t) {{ if (t == \"{own}\") {{ return true; }} return false; }}\n    }}\n}}\n"
    );
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    common::init_repo_with_files(&repo, "Alice", &[("src/Vault.cs", &src)]);
    let out = dir.path().join("out");
    Command::cargo_bin("reverse-architect")
        .unwrap()
        .arg(&repo)
        .arg("--out")
        .arg(&out)
        .arg("--detail-level")
        .arg("2")
        .arg("--secret-prefix")
        .arg("acme_")
        .assert()
        .success();

    let bodies = records(&out, "languages-bodies");
    assert_eq!(bodies.len(), 1);
    let record = String::from_utf8(bodies.into_values().next().unwrap().1).unwrap();
    assert!(
        record.contains("[redacted: "),
        "the literals were stored, redacted: {record}"
    );
    fn walk(dir: &Path, found: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, found);
            } else {
                found.push(path);
            }
        }
    }
    let mut files = Vec::new();
    walk(&out, &mut files);
    assert!(files.len() > 5, "{files:?}");
    for path in files {
        let text = String::from_utf8_lossy(&std::fs::read(&path).unwrap()).into_owned();
        for secret in [token, own] {
            assert!(!text.contains(secret), "{} holds {secret}", path.display());
        }
    }
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(out.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["detail_level"], 2);
}
