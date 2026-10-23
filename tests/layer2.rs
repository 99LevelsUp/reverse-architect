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
#[path = "common/coverage_line.rs"]
mod coverage_line;

use assert_cmd::Command;
use common::output_files;
use coverage_line::{without_coverage_bytes, without_coverage_line};
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
            "sql",
            "xaml"
        ]
    );

    insta::assert_snapshot!("layer1_corpus_fingerprint", a);
}

// ---- The second layer (plan 2a, U4) ----

use reverse_architect::config::Config;
use reverse_architect::extract::bodies::{BodyKind, CommentKind, PairedBy, RoleKind, SecondLayer};
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

/// A card's front matter, without its fences, and the rest of it.
fn front_matter(card: &str) -> (&str, &str) {
    let rest = card.strip_prefix("---\n").expect("front matter");
    let end = rest.find("\n---\n\n").expect("the end of the front matter");
    (&rest[..end], &rest[end + "\n---\n\n".len()..])
}

/// The parts of a card the second layer leaves as they are: everything but
/// the purpose and the keywords, which it rewrites, and the functionalities
/// and states, which it adds (plan 2a, U6: KTD10), and its dynamic access
/// (step 2c, U8: KTD11).
fn first_layer_sections(body: &str) -> String {
    let mut out = String::new();
    let mut keep = true;
    for line in body.split_inclusive('\n') {
        if let Some(heading) = line.strip_prefix("## ") {
            keep = ![
                "Purpose",
                "Keywords",
                "Functionality: ",
                "State: ",
                "Dynamic access",
            ]
            .iter()
            .any(|h| heading.starts_with(h));
        }
        if keep {
            out.push_str(line);
        }
    }
    out
}

/// KTD9: with the second layer every view, detail and map is what a plain
/// run writes — but for the line of trigger coverage `entry-points.md` gains
/// right above E9 (plan 2b, U8: KTD12) — and so is every card but a type's.
/// The registry of triggers (plan 2b, U7) is a file of its own beside them.
/// A type's card says
/// `schema: 7`, the level and its `layer2:` word in its front matter; with
/// its analysis (`present`) it rewrites its purpose and keywords and adds its
/// functionalities and states, every other part as it was, and without one
/// (`failed`, `none`) the rest of it is the plain run's byte for byte.
fn assert_same_first_layer(plain: &Generated, layered: &Generated) {
    assert_eq!(
        plain.corpus,
        without_coverage_line(&layered.corpus),
        "the views differ"
    );
    assert!(layered.triggers.is_some() && plain.triggers.is_none());
    assert_eq!(plain.details, layered.details, "the details differ");
    assert_eq!(plain.maps, layered.maps, "the maps differ");
    assert_eq!(plain.cards.len(), layered.cards.len());
    let level = layered.bodies.as_ref().expect("the second layer").level();
    for (p, l) in plain.cards.iter().zip(&layered.cards) {
        assert_eq!(p.path, l.path);
        if !p.path.starts_with("cards/type/") {
            assert_eq!(p.contents, l.contents, "{}", p.path);
            continue;
        }
        let (plain_front, plain_body) = front_matter(&p.contents);
        let (front, body) = front_matter(&l.contents);
        let (_, word) = front
            .rsplit_once("\nlayer2: ")
            .unwrap_or_else(|| panic!("{}: no layer2 word: {front}", p.path));
        assert!(["present", "failed", "none"].contains(&word), "{word}");
        assert_eq!(
            front,
            format!(
                "{}\ndetail_level: {level}\nlayer2: {word}",
                plain_front.replace("schema: 6\n", "schema: 7\n")
            ),
            "{}",
            p.path
        );
        if word == "present" {
            assert_eq!(
                first_layer_sections(body),
                first_layer_sections(plain_body),
                "{}",
                p.path
            );
        } else {
            assert_eq!(body, plain_body, "{}", p.path);
        }
    }
}

/// With `--detail-level` every view, detail and map is what a plain run
/// writes, and every card as [`assert_same_first_layer`] allows; the manifest
/// gains the level and the body component, a type card's document the
/// chunks of its new sections, and the one document of the registry of
/// triggers (plan 2b, U7), and nothing else; coverage gains the `bodies`
/// section and the component's contract, and the type cards' own counts.
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
    // A type card's document keeps every chunk it had and gains those of
    // its functionalities and states; nothing else of any document changes.
    let chunks = |d: &serde_json::Value| -> Vec<String> {
        d["chunks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["id"].as_str().unwrap().to_string())
            .collect()
    };
    let plain_docs = plain_manifest["documents"].as_array().unwrap();
    let all_docs = manifest["documents"].as_array().unwrap();
    assert_eq!(
        all_docs
            .iter()
            .filter(|d| d["id"] == "triggers:index")
            .count(),
        1,
        "the registry of triggers is one document (plan 2b, U7)"
    );
    let docs: Vec<&serde_json::Value> = all_docs
        .iter()
        .filter(|d| d["kind"] != "triggers")
        .collect();
    assert_eq!(plain_docs.len(), docs.len());
    for (p, d) in plain_docs.iter().zip(docs) {
        if d["kind"] == "card:type" {
            let have = chunks(d);
            assert!(
                chunks(p).iter().all(|c| have.contains(c)),
                "{}: {:?} lost a chunk of {:?}",
                d["id"],
                have,
                chunks(p)
            );
        } else {
            assert_eq!(p, d);
        }
    }
    let unchunked = |mut m: serde_json::Value| {
        m["documents"]
            .as_array_mut()
            .unwrap()
            .retain(|d| d["kind"] != "triggers");
        for d in m["documents"].as_array_mut().unwrap() {
            if d["kind"] == "card:type" {
                d.as_object_mut().unwrap().remove("chunks");
            }
        }
        m
    };
    assert_eq!(
        unchunked(without(plain_manifest, &["plugins"])),
        unchunked(without(manifest, &["plugins", "detail_level"]))
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
    // The body component runs under contract 0.4.
    more["0.4"] = serde_json::json!(more["0.4"].as_u64().unwrap_or(0) + 1);
    assert_eq!(contracts(&coverage), more);
    // The type cards are measured as they were written: their sizes, and
    // how many carry no keyword, are the second layer's; every other level
    // is the plain run's.
    let other_levels = |c: &serde_json::Value| {
        let mut cards = c["cards"].clone();
        cards["sizes"].as_object_mut().unwrap().remove("type_card");
        cards.as_object_mut().unwrap().remove("without_keywords");
        cards
    };
    assert_eq!(other_levels(&plain_coverage), other_levels(&coverage));
    assert_eq!(
        without(plain_coverage, &["noise.plugins_by_contract", "cards"]),
        without(
            coverage,
            &["bodies", "markup", "noise.plugins_by_contract", "cards"]
        )
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
    // The type whose bodies failed has its first-layer card: its front
    // matter says the second layer failed, and the rest of it is the plain
    // run's (compared above with every other card).
    let card = |g: &Generated| {
        g.cards
            .iter()
            .find(|c| c.path.starts_with("cards/type/") && c.contents.contains("loop.probe3"))
            .map(|c| c.contents.clone())
    };
    let failed_card = card(&layered).expect("the failed type has a card");
    let plain_card = card(&plain).unwrap();
    assert!(
        front_matter(&failed_card).0.ends_with("\nlayer2: failed"),
        "{failed_card}"
    );
    assert_eq!(front_matter(&failed_card).1, front_matter(&plain_card).1);
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
    // XAML claims no file of this corpus: absent from the list (plan 2c,
    // U10), as it is from the vocabulary and the state of XAML.
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
    // Comments the body component now returns: a value beside its label, a
    // value on the line below its label, and a token inside a block comment.
    let password = "Wq8Tz3Lm6Rv1Np4K";
    let below = "Hd5Kp9Xs2Bv7Mn3Q";
    let token2 = "ghp_B8nM2kL5pQ9wX3zR7tY1vC4dF6hJ0sA";
    let src = format!(
        "namespace Acme.Views\n{{\n    public class Vault\n    {{\n        public string Token()\n        {{\n            // password: {password}\n            // rotate monthly\n            // password:\n            // {below}\n            /* backup key {token2} */\n            return \"{token}\";\n        }}\n        public bool Is(string t) {{ if (t == \"{own}\") {{ return true; }} return false; }}\n    }}\n}}\n"
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
    assert!(
        record.contains("rotate monthly"),
        "the comments were stored: {record}"
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
        for secret in [token, own, password, below, token2] {
            assert!(!text.contains(secret), "{} holds {secret}", path.display());
        }
    }
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(out.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["detail_level"], 2);
}

// ---- The analysis of the second layer (plan 2a, U5) ----

use reverse_architect::card::layer2::{
    self, bind_construction, Analysis, EntryKind, EntryPoint, Functionality, Index, Label,
    TypeAnalysis, Unresolved,
};
use reverse_architect::extract::bodies::{AccessPath, PathRoot, ValueRef};

/// A view on a generic base, its view model and a command class with chained
/// constructors: a key handler with eleven branches, a context menu with a
/// submenu, a lambda subscribed in the constructor, a declaration pattern; a
/// second view whose accesses have no receiver chain; a view on a nested
/// generic argument.
const EDITOR_VIEWS: &str = r#"using Acme.ViewModels;

namespace Acme.Views
{
    public class ViewBase<T>
    {
        protected T viewModel;
        protected List<T> history;
        public string GetResourceString(string key) { return key; }
    }

    public class EditorView : ViewBase<EditorViewModel>
    {
        private ContextMenu menu;
        private TextEditor editor;
        public EditorView()
        {
            editor.PreviewKeyDown -= Editor_PreviewKeyDown;
            editor.PreviewKeyDown += Editor_PreviewKeyDown;
            Loaded += (s, a) => BuildMenu();
        }
        void Editor_PreviewKeyDown(object sender, KeyEventArgs e)
        {
            if (e.Key == Key.F1) { Help(); }
            else if (e.Key == Key.F2) { Rename(); }
            else if (e.Key == Key.F3) { this.viewModel.Find(); }
            else if (e.Key == Key.F4 && (viewModel.ShowErrors || viewModel.CanErrors)) { this.viewModel.CheckCommand.Execute(null); }
            else if (e.Key == Key.F5) { viewModel.Run(); }
            else if (e.Key == Key.F6) { Help(); }
            else if (e.Key == Key.F7) { history.Clear(); }
            else if (e.Key == Key.F8) { viewModel.Format(); }
            else if (e.Key == Key.F9) { viewModel.Toggle(); }
            else if (e.Key == Key.F10) { viewModel.Save(); }
            else if (e.Key == Key.F11) { viewModel.FullscreenCommand.Execute(null); }
        }
        void BuildMenu()
        {
            ContextMenuItem hotKeys = new ContextMenuItem();
            hotKeys.Header = GetResourceString("EditorView_settings");
            ContextMenuItem f4 = new ContextMenuItem();
            f4.Header = GetResourceString("EditorView_checkErrors");
            f4.Command = this.viewModel.CheckCommand;
            hotKeys.Items.Add(f4);
            ContextMenuItem full = new ContextMenuItem();
            full.Header = viewModel.ShowHeaderText;
            full.Command = viewModel.FullscreenCommand;
            menu.Items.Add(hotKeys);
            menu.Items.Add(full);
        }
        void Help() { }
        void Rename() { }
        public void Focus(object o)
        {
            if (o is not EditorView other) return;
            other.viewModel.Save();
        }
    }

    public class EditorViewNew : ViewBase<EditorViewModelNew>
    {
        void OnKey(KeyEventArgs e)
        {
            if (e.Key == Key.F4) { Lookup().CheckCommand.Execute(null); }
            else if (e.Key == Key.F5) { CheckCommand.Execute(null); }
        }
        EditorViewModelNew Lookup() { return viewModel; }
    }

    public class NestedView : ViewBase<List<EditorViewModel>>
    {
        void OnKey(KeyEventArgs e)
        {
            if (e.Key == Key.F4) { viewModel.CheckCommand.Execute(null); }
        }
    }
}
"#;

const EDITOR_MODELS: &str = r#"namespace Acme.ViewModels
{
    public class RelayCommand
    {
        private readonly Action<object> _execute;
        private readonly Predicate<object> _canExecute;
        public RelayCommand(Action<object> execute) : this(execute, null) { }
        public RelayCommand(Action<object> execute, Predicate<object> canExecute)
        {
            _execute = execute;
            _canExecute = canExecute;
        }
        public bool CanExecute(object parameter) { return _canExecute == null || _canExecute(parameter); }
        public void Execute(object parameter) { _execute?.Invoke(parameter); }
    }

    public class EditorViewModel
    {
        private RelayCommand checkCommand;
        private bool canErrors;
        private int syntax;
        public bool ShowErrors { get; set; }
        public bool CanErrors => canErrors;
        public string ShowHeaderText => "x";
        public RelayCommand CheckCommand
        {
            get
            {
                if (checkCommand == null) checkCommand = new RelayCommand(p => Check(), p => CanCheck());
                return checkCommand;
            }
        }
        public RelayCommand FullscreenCommand => new RelayCommand(ToggleFullscreen);
        public void Check() { Check(false); }
        public void Check(bool syntaxOnly) { if (!canErrors) return; Parse(syntax); }
        bool CanCheck() { return canErrors && syntax > 0; }
        void Parse(int s) { }
        void ToggleFullscreen(object p) { }
        public void Find() { }
        public void Run() { }
        public void Format() { }
        public void Toggle() { }
        public void Save() { }
    }

    public class EditorViewModelNew
    {
        public RelayCommand CheckCommand => new RelayCommand(p => Check());
        public void Check() { }
    }
}
"#;

const VIEWS_NS: &str = "sym:csharp:Acme.Views#";
const MODELS_NS: &str = "sym:csharp:Acme.ViewModels#";

/// A run with the second layer over `files`, and its analysis.
fn analyzed(files: &[(&str, &str)]) -> (Generated, Analysis) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    common::init_repo_with_files(&repo, "Alice", files);
    let generated = generate(&config(&repo, &dir.path().join("out"), Some(2))).unwrap();
    let analysis = layer2::analyze(&generated.graph, generated.bodies.as_ref().unwrap());
    (generated, analysis)
}

fn editor() -> (Generated, Analysis) {
    analyzed(&[
        ("src/Views.cs", EDITOR_VIEWS),
        ("src/ViewModels.cs", EDITOR_MODELS),
    ])
}

fn key(ns: &str, rest: &str) -> NodeKey {
    NodeKey::new(format!("{ns}{rest}"))
}

fn type_of<'a>(analysis: &'a Analysis, key: &NodeKey) -> &'a TypeAnalysis {
    analysis
        .of(key)
        .unwrap_or_else(|| panic!("{key:?} is not analyzed"))
}

fn entries(t: &TypeAnalysis) -> Vec<&EntryPoint> {
    t.functionalities
        .iter()
        .flat_map(|f| f.entry_points.iter())
        .collect()
}

fn entry<'a>(t: &'a TypeAnalysis, key: &str) -> &'a EntryPoint {
    entries(t)
        .into_iter()
        .find(|e| e.key == key)
        .unwrap_or_else(|| panic!("no entry point {key}: {:?}", keys_of(t)))
}

fn keys_of(t: &TypeAnalysis) -> Vec<&str> {
    entries(t).iter().map(|e| e.key.as_str()).collect()
}

fn functionality<'a>(t: &'a TypeAnalysis, entry: &str) -> &'a Functionality {
    t.functionality_of(entry)
        .unwrap_or_else(|| panic!("no functionality holds {entry}: {:?}", keys_of(t)))
}

fn resolved_targets(e: &EntryPoint) -> Vec<&NodeKey> {
    e.targets.iter().filter_map(|t| t.member.as_ref()).collect()
}

/// AE2: eleven branches of one key handler are eleven seeds; F4 and the
/// menu item go through the inherited `viewModel` to the same command and are
/// one functionality with two entry points — and nothing else joins them.
#[test]
fn eleven_key_branches_are_eleven_seeds_and_f4_with_its_menu_item_is_one_functionality() {
    let (_, analysis) = editor();
    let view = type_of(&analysis, &key(VIEWS_NS, "EditorView"));
    let dispatcher = format!("{VIEWS_NS}EditorView.Editor_PreviewKeyDown(object,KeyEventArgs)");
    let branches: Vec<&str> = entries(view)
        .into_iter()
        .filter(|e| e.kind == EntryKind::Key)
        .map(|e| e.key.as_str())
        .collect();
    assert_eq!(branches.len(), 11, "{branches:?}");
    assert!(branches
        .iter()
        .all(|k| k.starts_with(&format!("{dispatcher}#Key.F"))));
    // The dispatcher is no entry point of its own.
    assert!(!keys_of(view).contains(&dispatcher.as_str()));

    let f4_key = format!("{dispatcher}#Key.F4");
    let check = functionality(view, &f4_key);
    let menu_key = format!("{VIEWS_NS}EditorView.BuildMenu()#f4");
    let held: Vec<&str> = check.entry_points.iter().map(|e| e.key.as_str()).collect();
    assert_eq!(held, [menu_key.as_str(), f4_key.as_str()]);
    // A menu item and a key branch are of one rank: the least key represents.
    assert_eq!(check.key, menu_key);
    assert_eq!(check.name.representative, "BuildMenu#f4");
    assert_eq!(
        check.targets,
        [key(MODELS_NS, "EditorViewModel.CheckCommand")]
    );
    assert_eq!(check.name.words, ["check", "command"]);

    let f4 = entry(view, &f4_key);
    assert_eq!(
        f4.condition.as_ref().map(|c| c.text.as_str()),
        Some("e.Key == Key.F4 && (viewModel.ShowErrors || viewModel.CanErrors)")
    );
    // Through `viewModel: T`, T = EditorViewModel, to the command property.
    let target = &f4.targets[0];
    let chain: Vec<&str> = target
        .chain
        .links
        .iter()
        .map(|l| l.member.as_str())
        .collect();
    assert_eq!(
        chain,
        [
            format!("{VIEWS_NS}ViewBase.viewModel").as_str(),
            format!("{MODELS_NS}EditorViewModel.CheckCommand").as_str(),
            format!("{MODELS_NS}RelayCommand.Execute(object)").as_str(),
        ]
    );
    // The branch names where its dispatcher is subscribed (`-=` and `+=`
    // are one subscription).
    assert_eq!(f4.subscriptions.len(), 1);
    assert_eq!(f4.subscriptions[0].event, "PreviewKeyDown");

    // F3 is a functionality of its own.
    let find = functionality(view, &format!("{dispatcher}#Key.F3"));
    assert_eq!(find.entry_points.len(), 1);
    assert_eq!(find.targets, [key(MODELS_NS, "EditorViewModel.Find()")]);
    // F1 and F6 call one helper of the view: one functionality.
    let help = functionality(view, &format!("{dispatcher}#Key.F1"));
    assert_eq!(help.entry_points.len(), 2);
    assert_eq!(help.members, [key(VIEWS_NS, "EditorView.Help()")]);
    // F11 and the other menu item execute the other command.
    let full = functionality(view, &format!("{dispatcher}#Key.F11"));
    let held: Vec<&str> = full.entry_points.iter().map(|e| e.key.as_str()).collect();
    assert_eq!(
        held,
        [
            format!("{VIEWS_NS}EditorView.BuildMenu()#full").as_str(),
            format!("{dispatcher}#Key.F11").as_str()
        ]
    );
    // The lambda subscribed in the constructor builds the menu: it reaches
    // `BuildMenu`, but not the commands of the items `BuildMenu` builds.
    let loaded = functionality(view, &format!("{VIEWS_NS}EditorView.EditorView()#Loaded"));
    assert_eq!(loaded.entry_points.len(), 1);
    assert!(loaded
        .members
        .contains(&key(VIEWS_NS, "EditorView.BuildMenu()")));
    assert!(loaded.targets.is_empty(), "{:?}", loaded.targets);

    // A declaration pattern types its variable (R17): `other.viewModel`
    // leads into the view model too, and joins F10.
    let focus = entry(view, &format!("{VIEWS_NS}EditorView.Focus(object)"));
    assert_eq!(focus.kind, EntryKind::PublicMethod);
    assert_eq!(
        resolved_targets(focus),
        [&key(MODELS_NS, "EditorViewModel.Save()")]
    );
    let save = functionality(view, &format!("{VIEWS_NS}EditorView.Focus(object)"));
    assert_eq!(save.key, format!("{dispatcher}#Key.F10"));

    // In the view model, the command, the method it executes and the
    // overload that one calls are one functionality, keyed by the command.
    let model = type_of(&analysis, &key(MODELS_NS, "EditorViewModel"));
    let command = functionality(model, &format!("{MODELS_NS}EditorViewModel.CheckCommand"));
    assert_eq!(
        command.key,
        format!("{MODELS_NS}EditorViewModel.CheckCommand")
    );
    assert_eq!(command.representative().kind, EntryKind::Command);
    for member in [
        "Check()",
        "Check(bool)",
        "CanCheck()",
        "Parse(int)",
        "checkCommand",
    ] {
        assert!(
            command
                .members
                .contains(&key(MODELS_NS, &format!("EditorViewModel.{member}"))),
            "{member}: {:?}",
            command.members
        );
    }
    let delegates = &command.representative().commands[0].delegates;
    let roles: Vec<Vec<&str>> = delegates
        .iter()
        .map(|d| d.invoked_by.iter().map(NodeKey::as_str).collect())
        .collect();
    assert_eq!(
        roles,
        [
            vec![format!("{MODELS_NS}RelayCommand.Execute(object)").as_str()],
            vec![format!("{MODELS_NS}RelayCommand.CanExecute(object)").as_str()],
        ]
    );
}

/// R18: a menu item carries the resource key of its label, its submenu with
/// that one's key, and the target of its command; an item labelled from a
/// property carries the property's path.
#[test]
fn a_menu_item_carries_its_resource_key_submenu_and_command_target() {
    let (_, analysis) = editor();
    let view = type_of(&analysis, &key(VIEWS_NS, "EditorView"));
    let f4 = entry(view, &format!("{VIEWS_NS}EditorView.BuildMenu()#f4"));
    assert_eq!(f4.kind, EntryKind::MenuItem);
    let menu = f4.menu.as_ref().expect("a menu item");
    assert_eq!(menu.type_name.as_deref(), Some("ContextMenuItem"));
    let resource = |label: &Option<Label>| match label {
        Some(Label::ResourceKey { key, quote, .. }) => {
            assert!(quote.is_none(), "no comment follows the call");
            key.clone()
        }
        other => panic!("not a resource key: {other:?}"),
    };
    assert_eq!(resource(&menu.label), "EditorView_checkErrors");
    let submenu = menu.submenu.as_ref().expect("a submenu");
    assert_eq!(submenu.variable, "hotKeys");
    assert_eq!(resource(&submenu.label), "EditorView_settings");
    assert_eq!(
        menu.command,
        Some(AccessPath {
            root: PathRoot::This,
            segments: vec!["viewModel".into(), "CheckCommand".into()],
        })
    );
    assert_eq!(
        resolved_targets(f4),
        [&key(MODELS_NS, "EditorViewModel.CheckCommand")]
    );

    let full = entry(view, &format!("{VIEWS_NS}EditorView.BuildMenu()#full"));
    let menu = full.menu.as_ref().unwrap();
    assert_eq!(
        menu.label,
        Some(Label::Expression {
            path: AccessPath {
                root: PathRoot::Name,
                segments: vec!["viewModel".into(), "ShowHeaderText".into()],
            }
        })
    );
    // Added to a field, not to a local: no submenu.
    assert!(menu.submenu.is_none());
    // The submenu itself is given no command: no entry point.
    assert!(!keys_of(view).iter().any(|k| k.ends_with("#hotKeys")));
}

/// KTD4: an access without a receiver chain stays unresolved and leads into
/// none of the types that declare a command of that name; a nested generic
/// argument stays unresolved too.
#[test]
fn an_access_without_a_receiver_chain_leads_nowhere() {
    let (_, analysis) = editor();
    let view = type_of(&analysis, &key(VIEWS_NS, "EditorViewNew"));
    let commands = [
        key(MODELS_NS, "EditorViewModel.CheckCommand"),
        key(MODELS_NS, "EditorViewModelNew.CheckCommand"),
    ];
    for e in entries(view) {
        for t in &e.targets {
            for link in &t.chain.links {
                assert!(!commands.contains(&link.member), "{}: {link:?}", e.key);
            }
        }
    }
    for f in &view.functionalities {
        assert!(
            f.targets.iter().all(|t| !commands.contains(t)),
            "{:?}",
            f.targets
        );
    }
    let on_key = format!("{VIEWS_NS}EditorViewNew.OnKey(KeyEventArgs)");
    let f4 = entry(view, &format!("{on_key}#Key.F4"));
    let unresolved: Vec<&Unresolved> = f4.targets.iter().filter_map(|t| t.unresolved()).collect();
    assert_eq!(unresolved, [&Unresolved::NoReceiverChain]);
    // `Lookup()` itself is the view's own method.
    assert_eq!(
        resolved_targets(f4),
        [&key(VIEWS_NS, "EditorViewNew.Lookup()")]
    );
    let f5 = entry(view, &format!("{on_key}#Key.F5"));
    assert_eq!(
        f5.targets[0].unresolved(),
        Some(&Unresolved::NotAMember("CheckCommand".into()))
    );

    let nested = type_of(&analysis, &key(VIEWS_NS, "NestedView"));
    let f4 = entry(
        nested,
        &format!("{VIEWS_NS}NestedView.OnKey(KeyEventArgs)#Key.F4"),
    );
    assert_eq!(
        f4.targets[0].unresolved(),
        Some(&Unresolved::NestedGeneric("List<EditorViewModel>".into()))
    );
}

const COMMANDS: &str = r#"namespace Acme.Commands
{
    public class Cmd
    {
        private readonly Action<object> _execute;
        private readonly Func<object, bool> _canExecute;
        public Cmd(Action<object> e) : this(e, null) { }
        public Cmd(Action<object> execute, Func<object, bool> canExecute)
        {
            _execute = execute;
            _canExecute = canExecute;
        }
        public void Execute(object parameter) { _execute(parameter); }
        public bool CanExecute(object parameter) { return _canExecute == null || _canExecute(parameter); }
    }

    public class Twice
    {
        private Action _run;
        public Twice(Action a, Action b) { _run = a; _run = b; }
        public void Execute() { _run(); }
    }

    public class Overloaded
    {
        private Action _run;
        public Overloaded(Action a) { _run = a; }
        public Overloaded(string name) { }
        public void Execute() { _run(); }
    }

    public class Panel
    {
        private Cmd checkCommand;
        public Cmd CheckCommand
        {
            get
            {
                if (checkCommand == null) checkCommand = new Cmd(p => Check(), p => Ready());
                return checkCommand;
            }
        }
        public Cmd RunCommand => new Cmd(p => Check());
        public Cmd SaveCommand => new Cmd(Save);
        public Overloaded RunTwice => new Overloaded(() => Save(null));
        public Twice Both => new Twice(() => Ready(), () => Ready());
        void Check() { }
        bool Ready() { return true; }
        void Save(object p) { }
    }
}
"#;

const COMMANDS_NS: &str = "sym:csharp:Acme.Commands#";

/// The command binding from the command class's own code: argument 0 of
/// `new Cmd(p => Check())` goes through `: this(e, null)` to the parameter the
/// target constructor writes to `_execute`, which `Execute` calls.
#[test]
fn a_command_binds_its_argument_through_this_to_the_field_its_method_calls() {
    let (generated, analysis) = analyzed(&[("src/Commands.cs", COMMANDS)]);
    let ix = Index::new(&generated.graph, generated.bodies.as_ref().unwrap());
    let cmd = key(COMMANDS_NS, "Cmd");

    let one = bind_construction(&ix, &cmd, 1).unwrap();
    assert_eq!(one.constructor, key(COMMANDS_NS, "Cmd.Cmd(Action<object>)"));
    assert_eq!(one.slots.len(), 1, "{one:?}");
    assert_eq!(one.slots[0].position, 0);
    assert_eq!(one.slots[0].field, key(COMMANDS_NS, "Cmd._execute"));
    assert_eq!(
        one.slots[0].invoked_by,
        [key(COMMANDS_NS, "Cmd.Execute(object)")]
    );
    let two = bind_construction(&ix, &cmd, 2).unwrap();
    let slots: Vec<(u32, &str, Vec<&str>)> = two
        .slots
        .iter()
        .map(|s| {
            (
                s.position,
                s.field.as_str(),
                s.invoked_by.iter().map(NodeKey::as_str).collect(),
            )
        })
        .collect();
    assert_eq!(
        slots,
        [
            (
                0,
                format!("{COMMANDS_NS}Cmd._execute").as_str(),
                vec![format!("{COMMANDS_NS}Cmd.Execute(object)").as_str()]
            ),
            (
                1,
                format!("{COMMANDS_NS}Cmd._canExecute").as_str(),
                vec![format!("{COMMANDS_NS}Cmd.CanExecute(object)").as_str()]
            ),
        ]
    );
    // A field written from two parameters is admitted, not bound.
    let twice = bind_construction(&ix, &key(COMMANDS_NS, "Twice"), 2).unwrap();
    assert!(twice.slots.is_empty(), "{twice:?}");
    assert_eq!(twice.ambiguous, [key(COMMANDS_NS, "Twice._run")]);
    // Two constructors of one arity: no binding.
    assert_eq!(
        bind_construction(&ix, &key(COMMANDS_NS, "Overloaded"), 1),
        Err(Unresolved::AmbiguousConstructor(1))
    );

    let panel = type_of(&analysis, &key(COMMANDS_NS, "Panel"));
    let target_of = |property: &str| -> Vec<(u32, Vec<String>)> {
        let e = entry(panel, &format!("{COMMANDS_NS}Panel.{property}"));
        assert_eq!(e.kind, EntryKind::Command, "{property}");
        e.commands[0]
            .delegates
            .iter()
            .map(|d| {
                (
                    d.position,
                    d.targets
                        .iter()
                        .filter_map(|t| t.member.as_ref().map(|m| m.as_str().to_string()))
                        .collect(),
                )
            })
            .collect()
    };
    let check = format!("{COMMANDS_NS}Panel.Check()");
    assert_eq!(target_of("RunCommand"), [(0, vec![check.clone()])]);
    assert_eq!(
        target_of("CheckCommand"),
        [
            (0, vec![check.clone()]),
            (1, vec![format!("{COMMANDS_NS}Panel.Ready()")])
        ]
    );
    // A method group is a delegate as well.
    assert_eq!(
        target_of("SaveCommand"),
        [(0, vec![format!("{COMMANDS_NS}Panel.Save(object)")])]
    );
    let lazy = entry(panel, &format!("{COMMANDS_NS}Panel.CheckCommand"));
    assert!(matches!(
        lazy.commands[0].delegates[0].value,
        ValueRef::Lambda(0)
    ));
    // The site names the body its lambdas are indices into.
    let region = &lazy.commands[0].region;
    let body = &generated
        .bodies
        .as_ref()
        .unwrap()
        .file(&region.path)
        .unwrap()
        .model
        .members[region.body];
    assert_eq!(body.lambdas.len(), 2);
    // A construction whose binding cannot be told is admitted, and is no
    // entry point.
    let unbound: Vec<(&str, &str, &Unresolved)> = panel
        .unbound_commands
        .iter()
        .map(|u| (u.member.as_str(), u.class.as_str(), &u.reason))
        .collect();
    assert_eq!(
        unbound,
        [
            (
                format!("{COMMANDS_NS}Panel.RunTwice").as_str(),
                format!("{COMMANDS_NS}Overloaded").as_str(),
                &Unresolved::AmbiguousConstructor(1)
            ),
            (
                format!("{COMMANDS_NS}Panel.Both").as_str(),
                format!("{COMMANDS_NS}Twice").as_str(),
                &Unresolved::AmbiguousFields(vec![key(COMMANDS_NS, "Twice._run")])
            ),
        ]
    );
    assert!(!keys_of(panel)
        .iter()
        .any(|k| k.ends_with("RunTwice") || k.ends_with("Both")));
    // Two command properties executing one method are one functionality,
    // keyed by the least of them.
    let run = functionality(panel, &format!("{COMMANDS_NS}Panel.RunCommand"));
    assert_eq!(run.key, format!("{COMMANDS_NS}Panel.CheckCommand"));
}

const SHAPES: &str = r#"namespace Acme.Shapes
{
    public class Base<T>
    {
        protected T vm;
        protected List<T> all;
    }

    public class M
    {
        public void Run() { }
    }

    public class V : Base<M>
    {
        public void Go() { vm.Run(); }
        public void All() { all.Clear(); }
    }

    public class W : Base<List<M>>
    {
        public void Go() { vm.Run(); }
    }
}
"#;

const SHAPES_NS: &str = "sym:csharp:Acme.Shapes#";

/// R17: `class V : Base<M>` and `Base<T> { protected T vm; }` type `vm` as
/// `M` from the base-type record and the typed member; a nested generic
/// argument and a member generic over the parameter stay unresolved.
#[test]
fn a_base_list_types_an_inherited_member_and_a_nested_generic_stays_unresolved() {
    let (_, analysis) = analyzed(&[("src/Shapes.cs", SHAPES)]);
    let v = type_of(&analysis, &key(SHAPES_NS, "V"));
    let go = entry(v, &format!("{SHAPES_NS}V.Go()"));
    let links: Vec<(&str, &str)> = go.targets[0]
        .chain
        .links
        .iter()
        .map(|l| (l.member.as_str(), l.owner.as_str()))
        .collect();
    assert_eq!(
        links,
        [
            (
                format!("{SHAPES_NS}Base.vm").as_str(),
                format!("{SHAPES_NS}Base").as_str()
            ),
            (
                format!("{SHAPES_NS}M.Run()").as_str(),
                format!("{SHAPES_NS}M").as_str()
            ),
        ]
    );
    assert_eq!(go.targets[0].member, Some(key(SHAPES_NS, "M.Run()")));
    let all = entry(v, &format!("{SHAPES_NS}V.All()"));
    assert_eq!(
        all.targets[0].chain.stopped,
        Some(Unresolved::NestedGeneric("List<T>".into()))
    );

    let w = type_of(&analysis, &key(SHAPES_NS, "W"));
    let go = entry(w, &format!("{SHAPES_NS}W.Go()"));
    assert_eq!(go.targets[0].member, None);
    assert_eq!(
        go.targets[0].unresolved(),
        Some(&Unresolved::NestedGeneric("List<M>".into()))
    );
}

/// What a bare name of `ty` resolves to, called with no argument.
fn called(ix: &Index<'_>, ty: &NodeKey, name: &str) -> layer2::Chain {
    let scope = layer2::Scope {
        ty,
        body: None,
        node: None,
    };
    let path = AccessPath {
        root: PathRoot::Name,
        segments: vec![name.to_string()],
    };
    ix.resolve(&scope, &path, Some(0))
}

/// A qualified type name binds only a type its qualifier names: a field
/// typed `System.Timers.Timer` is not the repository's `Timer`, which the
/// bare and the partly qualified name still bind.
#[test]
fn a_qualified_external_type_name_is_not_a_repository_type_of_its_last_name() {
    let src = r#"namespace Acme.Tools
{
    public class Timer { public void Start() { } }
    public class Clock
    {
        private System.Timers.Timer external;
        private Timer own;
        public void Tick() { external.Start(); own.Start(); }
    }
}
"#;
    let ns = "sym:csharp:Acme.Tools#";
    let (generated, _) = analyzed(&[("src/Tools.cs", src)]);
    let ix = Index::new(&generated.graph, generated.bodies.as_ref().unwrap());
    let timer = key(ns, "Timer");
    assert_eq!(
        ix.type_named("System.Timers.Timer"),
        Err(Unresolved::ExternalType("System.Timers.Timer".into()))
    );
    for written in [
        "Timer",
        "Tools.Timer",
        "Acme.Tools.Timer",
        "global::Acme.Tools.Timer",
    ] {
        assert_eq!(ix.type_named(written), Ok(timer.clone()), "{written}");
    }
    assert_eq!(
        ix.type_named("Other.Timer"),
        Err(Unresolved::ExternalType("Other.Timer".into()))
    );
    let clock = key(ns, "Clock");
    let scope = layer2::Scope {
        ty: &clock,
        body: None,
        node: None,
    };
    let start = |field: &str| {
        let path = AccessPath {
            root: PathRoot::Name,
            segments: vec![field.to_string(), "Start".to_string()],
        };
        ix.resolve(&scope, &path, Some(0))
    };
    let external = start("external");
    assert_eq!(
        external.stopped,
        Some(Unresolved::ExternalType("System.Timers.Timer".into()))
    );
    assert_eq!(external.links.len(), 1, "{external:?}");
    let own = start("own");
    assert_eq!(own.stopped, None);
    assert_eq!(
        own.links.last().map(|l| l.member.clone()),
        Some(key(ns, "Timer.Start()"))
    );
}

/// A type the second layer could not read still shares its name: the
/// first layer declares it, so a name two types have stays ambiguous.
#[test]
fn a_type_in_a_file_whose_bodies_failed_keeps_a_shared_name_ambiguous() {
    let read = r#"namespace Acme.A
{
    public class Ticker { public void Run() { } }
}
"#;
    let dir = tempfile::tempdir().unwrap();
    let repo_dir = dir.path().join("repo");
    let repo = common::init_repo_with_files(&repo_dir, "Alice", &[("src/A.cs", read)]);
    // A file that is no UTF-8: the body component refuses it, the first
    // layer reads its declarations.
    let mut failed =
        b"namespace Acme.B\n{\n    public class Ticker { public string S = \"".to_vec();
    failed.push(0xF8);
    failed.extend_from_slice(b"\"; public void Run() { } }\n}\n");
    std::fs::write(repo_dir.join("src/B.cs"), &failed).unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(Path::new("src/B.cs")).unwrap();
    index.write().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let sig = common::fixed_signature("Alice");
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "failed", &tree, &[&head])
        .unwrap();
    let generated = generate(&config(&repo_dir, &dir.path().join("out"), Some(2))).unwrap();
    let layer = generated.bodies.as_ref().unwrap();
    let failed_ticker = NodeKey::new("sym:csharp:Acme.B#Ticker");
    assert!(generated.graph.node(&failed_ticker).is_some());
    assert_eq!(layer.layer_of(&failed_ticker), SecondLayer::Failed);
    let ix = Index::new(&generated.graph, layer);
    assert_eq!(
        ix.type_named("Ticker"),
        Err(Unresolved::AmbiguousType("Ticker".into()))
    );
    assert_eq!(
        ix.type_named("A.Ticker"),
        Ok(NodeKey::new("sym:csharp:Acme.A#Ticker"))
    );
    // The type that was not read is not one the analyses read.
    assert!(!ix.read_types().contains(&&failed_ticker));
}

/// A name is declared beyond one level only when a type further up
/// declares it; a name no type of the line declares is no member, and
/// one a type with a base outside the repository does not declare may be
/// that base's.
#[test]
fn beyond_one_level_is_claimed_only_for_a_name_a_type_further_up_declares() {
    let src = r#"namespace Acme.Up
{
    public class Top { public void Far() { } }
    public class Mid : Top { }
    public class Low : Mid
    {
        public void A() { Far(); }
        public void B() { Missing(); }
    }
    public class Win : Window
    {
        public void C() { Close(); }
    }
}
"#;
    let ns = "sym:csharp:Acme.Up#";
    let (generated, _) = analyzed(&[("src/Up.cs", src)]);
    let ix = Index::new(&generated.graph, generated.bodies.as_ref().unwrap());
    let low = key(ns, "Low");
    assert_eq!(
        called(&ix, &low, "Far").stopped,
        Some(Unresolved::BeyondOneLevel("Far".into()))
    );
    assert_eq!(
        called(&ix, &low, "Missing").stopped,
        Some(Unresolved::NotAMember("Missing".into()))
    );
    let mid = key(ns, "Mid");
    assert_eq!(
        called(&ix, &mid, "Missing").stopped,
        Some(Unresolved::NotAMember("Missing".into()))
    );
    assert_eq!(
        called(&ix, &key(ns, "Win"), "Close").stopped,
        Some(Unresolved::ExternalType("Window".into()))
    );
}

const CLUSTERS: &str = r#"namespace Acme.Clusters
{
    public class AcmeState
    {
        private int state;
        public void P() { if (state > 0) { P(); } }
        public void Q() { state = 1; }
        public void R() { Use(state); }
    }

    public class AcmeOne
    {
        private int x;
        public void A() { H(); }
        public void B() { H(); }
        private void H() { x = 1; }
        public void D() { }
    }

    public class AcmeTwo
    {
        private int x;
        public void A() { H(); }
        public void B() { H(); }
        private void H() { x = 1; }
        public void D() { }
        public void E() { }
    }

    public class AcmeThree
    {
        private int x;
        public void A() { H(); }
        public void B() { H(); }
        private void H() { x = 1; }
        public void D() { }
        public void C() { H(); }
    }
}
"#;

const CLUSTERS_NS: &str = "sym:csharp:Acme.Clusters#";

fn functionality_keys(t: &TypeAnalysis) -> Vec<&str> {
    t.functionalities.iter().map(|f| f.key.as_str()).collect()
}

/// KTD12: a field three seeds read is a hub and joins none of them; a helper
/// two of five members call stays in their cluster; a new public member that
/// defines nothing, and a new seed that turns the helper into a hub, leave
/// the key of the functionality as it was (KTD11).
#[test]
fn hubs_join_nothing_and_keys_survive_new_members() {
    let (_, analysis) = analyzed(&[("src/Clusters.cs", CLUSTERS)]);
    let state = type_of(&analysis, &key(CLUSTERS_NS, "AcmeState"));
    assert_eq!(
        state.functionalities.len(),
        3,
        "{:?}",
        functionality_keys(state)
    );
    assert_eq!(state.hubs, [key(CLUSTERS_NS, "AcmeState.state")]);
    for f in &state.functionalities {
        assert_eq!(f.entry_points.len(), 1);
        assert!(!f.members.contains(&key(CLUSTERS_NS, "AcmeState.state")));
    }

    let one = type_of(&analysis, &key(CLUSTERS_NS, "AcmeOne"));
    let a = format!("{CLUSTERS_NS}AcmeOne.A()");
    let f = one.functionality(&a).expect("A's functionality");
    assert_eq!(
        f.members,
        [
            key(CLUSTERS_NS, "AcmeOne.A()"),
            key(CLUSTERS_NS, "AcmeOne.B()"),
            key(CLUSTERS_NS, "AcmeOne.H()"),
            key(CLUSTERS_NS, "AcmeOne.x"),
        ]
    );
    assert!(one.hubs.is_empty(), "{:?}", one.hubs);
    assert_eq!(
        functionality_keys(one),
        [a.as_str(), format!("{CLUSTERS_NS}AcmeOne.D()").as_str()]
    );

    // A new public member that defines nothing: the key and the members stay.
    let two = type_of(&analysis, &key(CLUSTERS_NS, "AcmeTwo"));
    let f = two
        .functionality(&format!("{CLUSTERS_NS}AcmeTwo.A()"))
        .expect("the key stays");
    assert_eq!(f.members.len(), 4);
    assert_eq!(two.functionalities.len(), 3);

    // A new seed makes the helper a hub: the clusters part, the key stays.
    let three = type_of(&analysis, &key(CLUSTERS_NS, "AcmeThree"));
    assert_eq!(three.hubs, [key(CLUSTERS_NS, "AcmeThree.H()")]);
    let f = three
        .functionality(&format!("{CLUSTERS_NS}AcmeThree.A()"))
        .expect("the key stays");
    assert_eq!(f.entry_points.len(), 1);
    assert_eq!(three.functionalities.len(), 4);
}

/// R3: two analyses of one input, on one worker thread and on eight, give
/// the same bytes.
#[test]
fn the_analysis_is_the_same_bytes_on_one_thread_and_on_many() {
    let run = |threads: usize| -> Vec<u8> {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap()
            .install(|| {
                let (_, analysis) = editor();
                serde_json::to_vec(&analysis).unwrap()
            })
    };
    let one = run(1);
    let many = run(8);
    assert!(one.len() > 1000, "the analysis holds something");
    assert_eq!(one, many);
}

// ---- Preconditions, dead paths, states, steps and mentions (plan 2a, U11) ----

use reverse_architect::card::layer2::{
    DeadKind, EffectKind, FunctionalityFlow, Gap, Guarding, Precondition, Source, StepKind, Written,
};
use reverse_architect::extract::bodies::Constant;
use std::collections::BTreeSet;

/// The editor's check with its literal conditions, a command class whose
/// `Execute` raises events before its guard, the new editor's constant
/// CanExecute, a toggled state beside a static read of another type, and
/// the shapes of main work.
const FLOW: &str = r#"namespace Acme.Flow
{
    public enum SyntaxKind { Sql, Script, Text }
    public enum CheckKind { Syntax, Check }

    public class RelayCommand
    {
        private readonly Action<object> _execute;
        private readonly Predicate<object> _canExecute;
        private Action pre;
        private bool busy;
        public event EventHandler ExecutingStarted;
        public RelayCommand(Action<object> execute) : this(execute, null) { }
        public RelayCommand(Action<object> execute, Predicate<object> canExecute)
        {
            _execute = execute;
            _canExecute = canExecute;
        }
        public bool CanExecute(object parameter) { return _canExecute == null || _canExecute(parameter); }
        public void Execute(object p)
        {
            var evt = new CancelEventArgs();
            SetBusyState();
            pre?.Invoke();
            ExecutingStarted?.Invoke(this, evt);
            if (evt.Cancel) return;
            _execute(p);
        }
        void SetBusyState() { busy = true; }
    }

    public class EditorViewModel
    {
        private RelayCommand checkCommand;
        private bool canErrors;
        private SyntaxKind syntax;
        public bool CheckWasDone;
        public RelayCommand CheckCommand
        {
            get
            {
                if (checkCommand == null) checkCommand = new RelayCommand(p => Check(), p => (syntax == SyntaxKind.Sql || syntax == SyntaxKind.Script) && canErrors);
                return checkCommand;
            }
        }
        public RelayCommand QuickCommand => new RelayCommand(p => Quick(), p => canErrors);
        public void Check() { Check(false); }
        void Check(bool syntaxOnly)
        {
            CheckWasDone = false;
            if (!canErrors) return;
            if (syntax != SyntaxKind.Sql && syntax != SyntaxKind.Script) return;
            if (syntaxOnly) { Parse(CheckKind.Syntax); } else { Parse(CheckKind.Check); }
        }
        void Quick() { if (!canErrors) return; Lint(); }
        void Parse(CheckKind kind)
        {
            var parser = new Parser();
            parser.Run(GetConfigValue("SQLPREFIX"), "Heslo123");
        }
        void Lint() { CheckWasDone = true; }
        public void Twice() { Do(1); }
        void Do(int a) { if (a < 0) return; }
        void Do(string s) { if (s == null) return; }
    }

    public class EditorViewModelNew
    {
        private bool busy;
        public RelayCommand CheckCommand => new RelayCommand(p => Check(), p => CanCheck());
        public void Check() { Lint(); }
        bool CanCheck() { return false; }
        bool Other() { return false; }
        void Lint() { busy = true; }
    }

    public class AcmeScreen
    {
        private bool isFullscreen;
        private int Modifiers;
        public bool IsFullscreen
        {
            get { return isFullscreen; }
            set { if (isFullscreen == value) return; isFullscreen = value; OnPropertyChanged(); }
        }
        public void Toggle() { IsFullscreen = !IsFullscreen; }
        public void Leave() { IsFullscreen = false; }
        public void Draw() { if (IsFullscreen) { Paint(); } }
        public void Reset() { Modifiers = 0; }
        public void Shift() { Modifiers = 1; }
        public void Key() { if (Keyboard.Modifiers == ModifierKeys.Shift) { Paint(); } }
        void OnPropertyChanged() { }
        void Paint() { }
    }

    public class AcmeLoad
    {
        private object data;
        public void Load() { var x = Fetch(); if (x == null) return; data = x; }
        object Fetch() { return data; }
    }

    public class AcmeRead
    {
        private object data;
        public void Read() { var x = Load(); if (x == null) return; data = x; }
        object Load() { return data; }
    }

    public class AcmeFresh
    {
        private int state;
        public void Start() { var n = 0; var args = new CancelEventArgs(); if (state == 0) return; state = n; }
    }

    public class AcmeOut
    {
        private int state;
        public void Fill(out object r) { r = null; if (state == 0) return; r = state; }
    }

    public class AcmeGate
    {
        private bool busy;
        public RelayCommand RunCommand => new RelayCommand(p => Run(), p => CanRun());
        public void Run() { busy = true; }
        bool CanRun() { return Ready(); }
        bool Ready() => !busy;
    }

    public class AcmeLater
    {
        private int state;
        public void After() { var n = Count(); Guarded(); }
        void Guarded() { if (state == 0) return; state = 2; }
        int Count() { return state; }
    }

    public class AcmeNow
    {
        private int state;
        public void Direct() { Guarded(); }
        void Guarded() { if (state == 0) return; state = 2; }
    }

    public class AcmeBase
    {
        private bool ready;
        protected void Helper() { if (!ready) return; ready = false; }
    }

    public class AcmeDerived : AcmeBase
    {
        public void Run() { Helper(); }
    }
}
"#;

const FLOW_NS: &str = "sym:csharp:Acme.Flow#";

fn flow_analysis() -> (Generated, Analysis) {
    analyzed(&[("src/Flow.cs", FLOW)])
}

/// The flow of the functionality the entry point `entry` (a member of
/// `ty`, without the namespace) belongs to.
fn flow_of<'a>(analysis: &'a Analysis, ty: &str, entry: &str) -> &'a FunctionalityFlow {
    let t = type_of(analysis, &key(FLOW_NS, ty));
    let f = functionality(t, &format!("{FLOW_NS}{ty}.{entry}"));
    t.flow_of(&f.key)
        .unwrap_or_else(|| panic!("no flow of {}", f.key))
}

fn precondition<'a>(flow: &'a FunctionalityFlow, text: &str) -> &'a Precondition {
    flow.preconditions
        .iter()
        .find(|p| p.text == text)
        .unwrap_or_else(|| {
            let texts: Vec<&str> = flow.preconditions.iter().map(|p| p.text.as_str()).collect();
            panic!("no precondition {text}: {texts:?}")
        })
}

fn texts(flow: &FunctionalityFlow) -> Vec<&str> {
    flow.preconditions.iter().map(|p| p.text.as_str()).collect()
}

/// AE1: the check has each precondition once — the guards of `Check(bool)`,
/// reached from the command's lambda through `Check()` → `Check(bool)` (by
/// arity) — nothing of the CanExecute predicate is admitted, and
/// `syntaxOnly` chooses `Syntax` instead of `Check`.
#[test]
fn the_check_has_each_precondition_once_and_its_parameter_chooses_the_check() {
    let (_, analysis) = flow_analysis();
    let flow = flow_of(&analysis, "EditorViewModel", "CheckCommand");
    let all = texts(flow);
    let unique: BTreeSet<&str> = all.iter().copied().collect();
    assert_eq!(all.len(), unique.len(), "a precondition twice: {all:?}");
    let check_bool = key(FLOW_NS, "EditorViewModel.Check(bool)");
    for text in [
        "!canErrors",
        "syntax != SyntaxKind.Sql && syntax != SyntaxKind.Script",
    ] {
        let p = precondition(flow, text);
        let guard = p
            .occurrences
            .iter()
            .find(|o| o.source == Source::Guard && o.member == check_bool)
            .unwrap_or_else(|| panic!("{text}: {:#?}", p.occurrences));
        assert_eq!(guard.via.last(), Some(&check_bool), "{text}");
        assert_eq!(guard.place.path, "src/Flow.cs");
        assert!(guard.place.range.start.line > 1);
    }
    // The CanExecute predicate is read from the tree of its lambda's
    // returned expression: nothing is admitted.
    assert!(flow.admitted.is_empty(), "{:#?}", flow.admitted);

    // `syntaxOnly` chooses `Syntax` instead of `Check`.
    let choice = flow
        .steps
        .iter()
        .find_map(|s| match &s.kind {
            StepKind::ParameterChoice { parameter, arms } if s.member == check_bool => {
                Some((parameter, arms))
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("no parameter choice: {:#?}", flow.steps));
    assert_eq!(choice.0, "syntaxOnly");
    let selects: Vec<&[String]> = choice.1.iter().map(|a| a.selects.as_slice()).collect();
    assert_eq!(
        selects,
        [
            &["CheckKind.Syntax".to_string()][..],
            &["CheckKind.Check".to_string()][..]
        ]
    );
    assert_eq!(choice.1[0].condition.as_deref(), Some("syntaxOnly"));
    assert_eq!(choice.1[1].condition, None);
    // Each guard is a step too, anchored on its arm.
    let guard_steps = flow
        .steps
        .iter()
        .filter(|s| s.member == check_bool && s.kind == StepKind::Idiom("guard-return".into()))
        .count();
    assert_eq!(guard_steps, 2);
    // The lazy getter is one `lazy-init` step, its null test no choice.
    let command = key(FLOW_NS, "EditorViewModel.CheckCommand");
    let getter: Vec<&StepKind> = flow
        .steps
        .iter()
        .filter(|s| s.member == command)
        .map(|s| &s.kind)
        .filter(|k| !matches!(k, StepKind::Call { .. }))
        .collect();
    assert_eq!(
        getter,
        [&StepKind::LazyInit {
            field: key(FLOW_NS, "EditorViewModel.checkCommand")
        }]
    );

    // A CanExecute that is one read (`p => canErrors`) is inverted and is one
    // precondition with the guard of the method the command executes.
    let quick = flow_of(&analysis, "EditorViewModel", "QuickCommand");
    let p = precondition(quick, "!canErrors");
    let sources: BTreeSet<Source> = p.occurrences.iter().map(|o| o.source).collect();
    assert_eq!(
        sources,
        [Source::Guard, Source::CanExecute].into_iter().collect()
    );
}

/// From the C# source to the analysis: the CanExecute lambda
/// `p => (syntax == SyntaxKind.Sql || syntax == SyntaxKind.Script) &&
/// canErrors`, read from the tree the component gives its returned
/// expression, and the guards `!canErrors` and `syntax != SyntaxKind.Sql &&
/// syntax != SyntaxKind.Script` of `Check(bool)` give the check each
/// precondition once, found twice by the check: by the predicate and by the
/// guard. (`QuickCommand`, clustered with the check through `canErrors`,
/// adds its own two places to `!canErrors`.)
#[test]
fn the_can_execute_lambda_and_the_guards_give_each_precondition_once() {
    let (_, analysis) = flow_analysis();
    let flow = flow_of(&analysis, "EditorViewModel", "CheckCommand");
    let all: BTreeSet<&str> = texts(flow).into_iter().collect();
    assert_eq!(texts(flow).len(), 2, "{:?}", texts(flow));
    assert_eq!(
        all,
        [
            "!canErrors",
            "syntax != SyntaxKind.Sql && syntax != SyntaxKind.Script"
        ]
        .into_iter()
        .collect()
    );
    let command = key(FLOW_NS, "EditorViewModel.CheckCommand");
    let check_bool = key(FLOW_NS, "EditorViewModel.Check(bool)");
    for p in &flow.preconditions {
        let found: Vec<(Source, &NodeKey)> = p
            .occurrences
            .iter()
            .map(|o| (o.source, &o.member))
            .filter(|(_, m)| *m == &command || *m == &check_bool)
            .collect();
        assert_eq!(
            found,
            [(Source::CanExecute, &command), (Source::Guard, &check_bool)],
            "{}: {:#?}",
            p.text,
            p.occurrences
        );
    }
    let syntax = precondition(
        flow,
        "syntax != SyntaxKind.Sql && syntax != SyntaxKind.Script",
    );
    assert_eq!(syntax.occurrences.len(), 2, "{:#?}", syntax.occurrences);
    assert!(flow.admitted.is_empty(), "{:#?}", flow.admitted);
}

/// The editor's check alone: the command class, the lazy command with its
/// CanExecute lambda, `Check()` → `Check(bool)` with its two guards.
const CHECK: &str = r#"namespace Acme.Check
{
    public enum SyntaxKind { Sql, Script, Text }

    public class RelayCommand
    {
        private readonly Action<object> _execute;
        private readonly Predicate<object> _canExecute;
        public RelayCommand(Action<object> execute, Predicate<object> canExecute)
        {
            _execute = execute;
            _canExecute = canExecute;
        }
        public bool CanExecute(object parameter) { return _canExecute == null || _canExecute(parameter); }
        public void Execute(object p) { _execute(p); }
    }

    public class EditorViewModel
    {
        private RelayCommand checkCommand;
        private bool canErrors;
        private SyntaxKind syntax;
        public bool CheckWasDone;
        public RelayCommand CheckCommand
        {
            get
            {
                if (checkCommand == null) checkCommand = new RelayCommand(p => Check(), p => (syntax == SyntaxKind.Sql || syntax == SyntaxKind.Script) && canErrors);
                return checkCommand;
            }
        }
        public void Check() { Check(false); }
        void Check(bool syntaxOnly)
        {
            if (!canErrors) return;
            if (syntax != SyntaxKind.Sql && syntax != SyntaxKind.Script) return;
            CheckWasDone = true;
        }
    }
}
"#;

/// The same on the check alone, from its C# source to the analysis: each
/// precondition exactly twice, by the CanExecute lambda and by the guard of
/// `Check(bool)`, nothing admitted.
#[test]
fn the_check_alone_has_each_precondition_from_the_predicate_and_the_guard() {
    const NS: &str = "sym:csharp:Acme.Check#";
    let (_, analysis) = analyzed(&[("src/Check.cs", CHECK)]);
    let t = type_of(&analysis, &key(NS, "EditorViewModel"));
    let f = functionality(t, &format!("{NS}EditorViewModel.CheckCommand"));
    let flow = t.flow_of(&f.key).expect("the check's flow");
    let mut all = texts(flow);
    all.sort();
    assert_eq!(
        all,
        [
            "!canErrors",
            "syntax != SyntaxKind.Sql && syntax != SyntaxKind.Script"
        ]
    );
    let command = key(NS, "EditorViewModel.CheckCommand");
    let check_bool = key(NS, "EditorViewModel.Check(bool)");
    for p in &flow.preconditions {
        let found: Vec<(Source, &NodeKey)> = p
            .occurrences
            .iter()
            .map(|o| (o.source, &o.member))
            .collect();
        assert_eq!(
            found,
            [(Source::CanExecute, &command), (Source::Guard, &check_bool)],
            "{}: {:#?}",
            p.text,
            p.occurrences
        );
    }
    assert!(flow.admitted.is_empty(), "{:#?}", flow.admitted);
}

/// A CanExecute method returning a call of the type's own member is followed
/// into it, not read as the call: `CanRun()` → `Ready()` → `!busy`.
#[test]
fn a_predicate_returning_an_own_call_is_followed_into_it() {
    let (_, analysis) = flow_analysis();
    let flow = flow_of(&analysis, "AcmeGate", "RunCommand");
    assert_eq!(texts(flow), ["busy"]);
    assert_eq!(
        flow.preconditions[0].occurrences[0].via,
        [
            key(FLOW_NS, "AcmeGate.CanRun()"),
            key(FLOW_NS, "AcmeGate.Ready()")
        ]
    );
    assert!(flow.admitted.is_empty(), "{:#?}", flow.admitted);
}

/// Guards after the write `CheckWasDone = false;` are preconditions, and the
/// write is an effect of the path they stop.
#[test]
fn a_guard_after_a_field_write_is_a_precondition_with_the_write_as_its_effect() {
    let (_, analysis) = flow_analysis();
    let flow = flow_of(&analysis, "EditorViewModel", "CheckCommand");
    for text in [
        "!canErrors",
        "syntax != SyntaxKind.Sql && syntax != SyntaxKind.Script",
    ] {
        let p = precondition(flow, text);
        let guard = p
            .occurrences
            .iter()
            .find(|o| o.source == Source::Guard && !o.via.is_empty())
            .unwrap_or_else(|| panic!("{text}: {:#?}", p.occurrences));
        let effects: Vec<(EffectKind, String)> = guard
            .effects
            .iter()
            .map(|e| (e.kind, e.path.segments.join(".")))
            .collect();
        assert_eq!(
            effects,
            [(EffectKind::Write, "CheckWasDone".to_string())],
            "{text}"
        );
    }
}

/// `Execute` of a command class: `var evt = new CancelEventArgs();
/// SetBusyState(); pre?.Invoke(); ExecutingStarted?.Invoke(this, evt); if
/// (evt.Cancel) return; _execute(p);` has the precondition `evt.Cancel` once,
/// with the call and both raised events as the effects of the path it stops;
/// the local constructed first is neither an effect nor the main work.
#[test]
fn execute_of_a_command_class_has_its_cancel_guard_and_the_events_raised_before_it() {
    let (_, analysis) = flow_analysis();
    let flow = flow_of(&analysis, "RelayCommand", "Execute(object)");
    let p = precondition(flow, "evt.Cancel");
    assert_eq!(p.occurrences.len(), 1, "{:#?}", p.occurrences);
    let effects: Vec<(EffectKind, String)> = p.occurrences[0]
        .effects
        .iter()
        .map(|e| (e.kind, e.path.segments.join(".")))
        .collect();
    assert_eq!(
        effects,
        [
            (EffectKind::Call, "SetBusyState".to_string()),
            (EffectKind::Raise, "pre".to_string()),
            (EffectKind::Raise, "ExecutingStarted".to_string()),
        ]
    );
    // `_execute(p)` after the guard calls a field, not a method: nothing to
    // follow, nothing admitted.
    assert_eq!(texts(flow), ["evt.Cancel"]);
}

/// A call whose result is assigned is the main work, so the `if` after it is
/// no precondition; a call after the main work passes the callee's guards to
/// nobody, while the same call as the main work does.
#[test]
fn an_assigned_call_is_main_work_and_a_call_after_it_passes_nothing() {
    let (_, analysis) = flow_analysis();
    let load = flow_of(&analysis, "AcmeLoad", "Load()");
    assert!(load.preconditions.is_empty(), "{:?}", texts(load));
    let later = flow_of(&analysis, "AcmeLater", "After()");
    assert!(later.preconditions.is_empty(), "{:?}", texts(later));
    assert!(later.admitted.is_empty());
    let now = flow_of(&analysis, "AcmeNow", "Direct()");
    let p = precondition(now, "state == 0");
    assert_eq!(p.occurrences[0].via, [key(FLOW_NS, "AcmeNow.Guarded()")]);
}

/// A local written a call (`var x = Load();`) is the main work, so the `if`
/// after it is no precondition; locals written a constant or a construction
/// (`var n = 0; var args = new CancelEventArgs();`) are not, and the guard
/// after them is one, with no effect.
#[test]
fn a_local_from_a_call_is_main_work_and_one_from_a_constant_or_a_construction_is_not() {
    let (_, analysis) = flow_analysis();
    let read = flow_of(&analysis, "AcmeRead", "Read()");
    assert!(read.preconditions.is_empty(), "{:?}", texts(read));
    assert!(read.admitted.is_empty());
    let start = flow_of(&analysis, "AcmeFresh", "Start()");
    assert_eq!(texts(start), ["state == 0"]);
    let o = &start.preconditions[0].occurrences;
    assert_eq!(o.len(), 1, "{o:#?}");
    assert_eq!(o[0].source, Source::Guard);
    assert!(o[0].effects.is_empty(), "{:?}", o[0].effects);
    // A parameter written a constant reaches the caller (`out`): main work.
    let fill = flow_of(&analysis, "AcmeOut", "Fill(object)");
    assert!(fill.preconditions.is_empty(), "{:?}", texts(fill));
}

/// `Check()` → `Check(bool)` by arity; two overloads of one arity, and a
/// callee in an ancestor, are admitted, not followed.
#[test]
fn an_overload_resolves_by_arity_and_two_of_one_arity_are_admitted() {
    let (_, analysis) = flow_analysis();
    let flow = flow_of(&analysis, "EditorViewModel", "Check()");
    let p = precondition(flow, "!canErrors");
    assert!(p
        .occurrences
        .iter()
        .any(|o| o.via == [key(FLOW_NS, "EditorViewModel.Check(bool)")]
            && o.member == key(FLOW_NS, "EditorViewModel.Check(bool)")));
    let twice = flow_of(&analysis, "EditorViewModel", "Twice()");
    assert!(twice.preconditions.is_empty(), "{:?}", texts(twice));
    let gaps: Vec<&Gap> = twice.admitted.iter().map(|a| &a.gap).collect();
    assert_eq!(gaps, [&Gap::Call(Unresolved::AmbiguousMember("Do".into()))]);
    // An overload of the type's own is no call of another type.
    assert!(twice.steps.is_empty(), "{:?}", twice.steps);
    // The main work calls a member of the base: admitted, not followed.
    let run = flow_of(&analysis, "AcmeDerived", "Run()");
    assert!(run.preconditions.is_empty(), "{:?}", texts(run));
    let gaps: Vec<&Gap> = run.admitted.iter().map(|a| &a.gap).collect();
    assert_eq!(gaps, [&Gap::Ancestor(key(FLOW_NS, "AcmeBase.Helper()"))]);
}

/// The new editor's `CanCheck()` returning `false` is a dead path because it
/// is the CanExecute; another method returning `false` that guards nothing
/// is listed nowhere.
#[test]
fn a_can_execute_returning_false_is_a_dead_path_and_an_unguarding_constant_is_not() {
    let (_, analysis) = flow_analysis();
    let flow = flow_of(&analysis, "EditorViewModelNew", "CheckCommand");
    let can_check = key(FLOW_NS, "EditorViewModelNew.CanCheck()");
    let dead: Vec<(&NodeKey, &DeadKind)> = flow
        .dead_paths
        .iter()
        .map(|d| (&d.member, &d.kind))
        .collect();
    assert_eq!(
        dead,
        [(
            &can_check,
            &DeadKind::ConstantReturn(Constant::Boolean(false))
        )]
    );
    assert!(flow.dead_paths[0].guarding.contains(&Guarding::CanExecute));
    // Following the CanExecute into `CanCheck()` finds a constant: no
    // precondition, nothing admitted.
    assert!(flow.preconditions.is_empty(), "{:?}", texts(flow));
    assert!(flow.admitted.is_empty(), "{:?}", flow.admitted);
    let t = type_of(&analysis, &key(FLOW_NS, "EditorViewModelNew"));
    let other = key(FLOW_NS, "EditorViewModelNew.Other()");
    assert!(t
        .flow
        .functionalities
        .iter()
        .flat_map(|f| &f.dead_paths)
        .chain(&t.flow.dead_paths)
        .all(|d| d.member != other));
}

/// A CanExecute a deriving type may replace — `virtual`, an `override` that
/// is not `sealed` — returning a constant is no dead path; a `sealed
/// override` returning one is.
#[test]
fn a_replaceable_can_execute_returning_a_constant_is_no_dead_path() {
    let src = r#"namespace Acme.Hooks
{
    public class RelayCommand
    {
        private readonly Action<object> _execute;
        private readonly Predicate<object> _canExecute;
        public RelayCommand(Action<object> execute, Predicate<object> canExecute)
        {
            _execute = execute;
            _canExecute = canExecute;
        }
        public bool CanExecute(object parameter) { return _canExecute == null || _canExecute(parameter); }
        public void Execute(object p) { _execute(p); }
    }

    public class HookBase
    {
        protected virtual bool CanLoad() { return true; }
        protected virtual bool CanSeal() { return true; }
    }

    public class HookModel : HookBase
    {
        private int saved;
        private int loaded;
        private int sealedCount;
        public RelayCommand SaveCommand => new RelayCommand(p => Save(), p => CanSave());
        public RelayCommand LoadCommand => new RelayCommand(p => Load(), p => CanLoad());
        public RelayCommand SealCommand => new RelayCommand(p => Seal(), p => CanSeal());
        protected virtual bool CanSave() { return false; }
        protected override bool CanLoad() { return false; }
        protected sealed override bool CanSeal() { return false; }
        void Save() { saved = 1; }
        void Load() { loaded = 1; }
        void Seal() { sealedCount = 1; }
    }
}
"#;
    let ns = "sym:csharp:Acme.Hooks#";
    let (_, analysis) = analyzed(&[("src/Hooks.cs", src)]);
    let t = type_of(&analysis, &key(ns, "HookModel"));
    let dead = |command: &str| -> Vec<(String, DeadKind)> {
        let f = functionality(t, &format!("{ns}HookModel.{command}"));
        t.flow_of(&f.key)
            .unwrap_or_else(|| panic!("no flow of {}", f.key))
            .dead_paths
            .iter()
            .map(|d| (d.member.as_str().to_string(), d.kind.clone()))
            .collect()
    };
    assert_eq!(dead("SaveCommand"), []);
    assert_eq!(dead("LoadCommand"), []);
    assert_eq!(
        dead("SealCommand"),
        [(
            format!("{ns}HookModel.CanSeal()"),
            DeadKind::ConstantReturn(Constant::Boolean(false))
        )]
    );
}

/// `IsFullscreen = !IsFullscreen` is a change of a state; the property
/// stands for the field its setter writes, whose guard `notifies-change`
/// absorbs; `Keyboard.Modifiers` read in a condition makes no state of the
/// type's own `Modifiers`, written constants twice but never read.
#[test]
fn a_toggle_is_a_state_change_and_a_static_read_of_another_type_is_no_state() {
    let (_, analysis) = flow_analysis();
    let t = type_of(&analysis, &key(FLOW_NS, "AcmeScreen"));
    let states: Vec<&str> = t.flow.states.iter().map(|s| s.member.as_str()).collect();
    let full = format!("{FLOW_NS}AcmeScreen.IsFullscreen");
    assert_eq!(states, [full.as_str()]);
    let state = &t.flow.states[0];
    assert_eq!(state.field, Some(key(FLOW_NS, "AcmeScreen.isFullscreen")));
    let values: Vec<&Written> = state.values.iter().map(|v| &v.value).collect();
    assert_eq!(
        values,
        [
            &Written::Constant(Constant::Boolean(false)),
            &Written::Toggle
        ]
    );
    // Read by `Draw`'s condition only: the setter's guard is absorbed.
    assert_eq!(state.reads.len(), 1, "{:?}", state.reads);
    // The toggle is a step of `Toggle`.
    let toggle = flow_of(&analysis, "AcmeScreen", "Toggle()");
    assert!(toggle.steps.iter().any(|s| s.kind
        == StepKind::StateChange {
            state: key(FLOW_NS, "AcmeScreen.IsFullscreen"),
            value: Written::Toggle,
        }));
}

/// R9: `GetConfigValue("SQLPREFIX")` gives the keyword `SQLPREFIX`; the
/// literal `"Heslo123"` gives none; the parser, a type of no repository
/// member, is mentioned.
#[test]
fn a_config_key_passed_to_a_call_is_a_keyword_and_another_literal_is_not() {
    let (_, analysis) = flow_analysis();
    let flow = flow_of(&analysis, "EditorViewModel", "CheckCommand");
    let keys: Vec<&str> = flow
        .mentions
        .config_keys
        .iter()
        .map(|k| k.key.as_str())
        .collect();
    assert_eq!(keys, ["SQLPREFIX"]);
    assert_eq!(
        flow.mentions.config_keys[0].callee.segments,
        ["GetConfigValue"]
    );
    let keywords = &flow.mentions.keywords;
    assert!(keywords.iter().any(|k| k == "SQLPREFIX"), "{keywords:?}");
    assert!(keywords.iter().any(|k| k == "Parser"), "{keywords:?}");
    assert!(
        !keywords.iter().any(|k| k.contains("Heslo")),
        "{keywords:?}"
    );
    assert!(flow.mentions.works_with().contains(&"Parser"));
}

/// R3: the flow of one input is the same bytes on one thread and on eight.
#[test]
fn the_flow_is_the_same_bytes_on_one_thread_and_on_many() {
    let run = |threads: usize| -> Vec<u8> {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap()
            .install(|| {
                let (_, analysis) = flow_analysis();
                serde_json::to_vec(&analysis).unwrap()
            })
    };
    let one = run(1);
    assert!(one.len() > 1000, "the flow holds something");
    assert_eq!(one, run(8));
}

// ---- The purpose from the code, the quote and the copied comment (2a, U7) ----

use reverse_architect::card::layer2_render::Layer2;
use reverse_architect::card::links::CardIndex;
use reverse_architect::card::purpose::{
    layered, Origin, Quote, COPIED_COMMENT_CS, COPIED_COMMENT_EN, QUOTED_COMMENT_CS,
    QUOTED_COMMENT_EN,
};
use reverse_architect::card::render::{render_card, render_card_layered, Lang};
use reverse_architect::card::{build_cards, Card};
use reverse_architect::cli::LinkStyle;
use reverse_architect::graph::EdgeKind;

/// Doc comments that share their first sentence: two unrelated types, a
/// group of four, two of three words only, a base and its derived type, the
/// two parts of a `partial` type (the second in [`COPIES_PART`]), and one
/// type whose comment is its own.
const COPIES: &str = r#"namespace Acme.Copies
{
    /// Opens the selected file in the editor.
    public class PairA
    {
        private int mode;
        public void Open() { mode = 1; }
        public void Close() { mode = 0; }
        public bool IsOpen() { if (mode == 1) { return true; } return false; }
    }

    /// Opens the selected file in the editor.
    public class PairB
    {
        public void Show() { }
    }

    /// Represents one grid row.
    public class ViewA { public void Show() { } }

    /// Represents one grid row.
    public class ViewB { public void Show() { } }

    /// Represents one grid row.
    public class ViewC { public void Show() { } }

    /// Represents one grid row.
    public class ViewD { public void Show() { } }

    /// Shows the panel.
    public class ShortA { public void Show() { } }

    /// Shows the panel.
    public class ShortB { public void Show() { } }

    /// Holds the state every view shares.
    public class BaseView { public void Show() { } }

    /// Holds the state every view shares.
    public class DerivedView : BaseView { public void Hide() { } }

    /// Edits the text of one document.
    public partial class PartView { public void Edit() { } }

    /// Formats the rows of one report.
    public class ReportView { public void Format() { } }

    /// Keeps the work of one quiet helper.
    public class QuietView { void Run() { } }
}
"#;

const COPIES_PART: &str = r#"namespace Acme.Copies
{
    /// Edits the text of one document.
    public partial class PartView { public void Save() { } }
}
"#;

const COPIES_NS: &str = "sym:csharp:Acme.Copies#";

fn copies_repo(dir: &Path) -> std::path::PathBuf {
    let repo = dir.join("repo");
    common::init_repo_with_files(
        &repo,
        "Alice",
        &[
            ("src/Copies.cs", COPIES),
            ("src/CopiesPart.cs", COPIES_PART),
        ],
    );
    repo
}

fn card_of<'a>(cards: &'a [Card], key: &NodeKey) -> &'a Card {
    cards
        .iter()
        .find(|c| c.identity == key.as_str())
        .unwrap_or_else(|| panic!("no card for {key:?}"))
}

/// The `## Purpose` section of a rendered card, through the line before the
/// next heading.
fn purpose_section(card: &str) -> &str {
    let start = card.find("\n## Purpose\n").expect("a purpose section");
    let rest = &card[start + 1..];
    let end = rest[3..].find("\n## ").map_or(rest.len(), |i| i + 4);
    &rest[..end]
}

/// The marks are fixed strings the owner's check of step 0 recognizes.
#[test]
fn the_quote_label_and_the_copied_mark_are_fixed_strings() {
    assert_eq!(
        COPIED_COMMENT_EN,
        "Doc comment probably copied from another type; not used as the purpose."
    );
    assert_eq!(
        COPIED_COMMENT_CS,
        "Doc komentář je pravděpodobně zkopírovaný z jiného typu; jako účel se nepoužije."
    );
    assert_eq!(
        QUOTED_COMMENT_EN,
        "quoted from the doc comment, not read from the code"
    );
    assert_eq!(
        QUOTED_COMMENT_CS,
        "citace doc komentáře, není přečteno z kódu"
    );
}

/// AE3, KTD15: a first sentence of at least four words that an unrelated
/// type's comment has too marks both, and every type of a group of four; a
/// sentence of three words, a base with its derived type and the parts of a
/// `partial` type are no copies.
#[test]
fn a_first_sentence_another_unrelated_type_shares_marks_every_type_of_the_group() {
    let dir = tempfile::tempdir().unwrap();
    let repo = copies_repo(dir.path());
    let generated = generate(&config(&repo, &dir.path().join("out"), Some(2))).unwrap();
    let analysis = layer2::analyze(&generated.graph, generated.bodies.as_ref().unwrap());
    let copied = |name: &str| {
        type_of(&analysis, &key(COPIES_NS, name))
            .purpose
            .copied_comment
    };

    for name in ["PairA", "PairB", "ViewA", "ViewB", "ViewC", "ViewD"] {
        assert!(copied(name), "{name} shares its first sentence");
    }
    for name in [
        "ShortA",
        "ShortB",
        "BaseView",
        "DerivedView",
        "PartView",
        "ReportView",
    ] {
        assert!(!copied(name), "{name} is no copy");
    }
    // Not vacuous: the base and the derived type are joined by `Extends`,
    // and the two parts are one analyzed type.
    let (base, derived) = (key(COPIES_NS, "BaseView"), key(COPIES_NS, "DerivedView"));
    assert!(generated
        .graph
        .edges_sorted()
        .iter()
        .any(|e| e.edge.kind == EdgeKind::Extends && *e.from == derived && *e.to == base));
    let part = key(COPIES_NS, "PartView");
    assert_eq!(analysis.types.iter().filter(|t| t.key == part).count(), 1);
}

/// R12, R13: with the second layer the purpose is the code's, and the doc
/// comment is under it as a labelled quote — or, when it is copied, only the
/// fixed mark; never a claim of the card.
#[test]
fn the_purpose_is_the_codes_and_the_comment_a_quote_or_the_copied_mark() {
    let dir = tempfile::tempdir().unwrap();
    let repo = copies_repo(dir.path());
    let generated = generate(&config(&repo, &dir.path().join("out"), Some(2))).unwrap();
    let analysis = layer2::analyze(&generated.graph, generated.bodies.as_ref().unwrap());
    let cards = build_cards(&generated.graph);
    let index = CardIndex::build(&cards);
    let render = |name: &str, lang: Lang| {
        let k = key(COPIES_NS, name);
        render_card_layered(
            card_of(&cards, &k),
            lang,
            LinkStyle::Portable,
            &index,
            Some(Layer2::present(2, type_of(&analysis, &k))),
        )
    };

    // A copied comment: the code says what the type is for, the comment is
    // not quoted, the mark says why.
    let pair = key(COPIES_NS, "PairA");
    let purpose = layered(card_of(&cards, &pair), &type_of(&analysis, &pair).purpose);
    assert_eq!(purpose.sentences[0].origin, Origin::Code);
    assert!(purpose
        .sentences
        .iter()
        .all(|s| s.origin != Origin::Comment));
    assert_eq!(purpose.quote, Some(Quote::Copied));
    for (lang, mark) in [(Lang::En, COPIED_COMMENT_EN), (Lang::Cs, COPIED_COMMENT_CS)] {
        let card = render("PairA", lang);
        let section = purpose_section(&card);
        assert!(section.contains(mark), "{section}");
        assert!(
            !section.contains("Opens the selected file"),
            "the copied comment is no purpose: {section}"
        );
    }
    let card = render("PairA", Lang::En);
    assert_eq!(
        purpose_section(&card),
        format!(
            "## Purpose\n\n3 core functionalities: `Close` (close), `IsOpen` (open) and `Open` \
             (open); 1 state: `mode`. (from the code)\n{COPIED_COMMENT_EN}\n\n"
        )
    );
    let card = render("PairA", Lang::Cs);
    assert!(
        purpose_section(&card).contains(
            "3 jádrové funkčnosti: `Close` (close), `IsOpen` (open) a `Open` (open); 1 stav: \
             `mode`. (z kódu)\n"
        ),
        "{card}"
    );

    // The type's own comment: the code first, the comment quoted under it.
    let card = render("ReportView", Lang::En);
    let section = purpose_section(&card);
    let lines: Vec<&str> = section.lines().skip(2).collect();
    assert!(lines[0].ends_with(" (from the code)"), "{section}");
    assert_eq!(
        lines[1],
        format!("Formats the rows of one report. ({QUOTED_COMMENT_EN})"),
        "{section}"
    );
    let card = render("ReportView", Lang::Cs);
    assert!(
        purpose_section(&card).contains(&format!(
            "Formats the rows of one report. ({QUOTED_COMMENT_CS})"
        )),
        "{card}"
    );
    assert!(purpose_section(&card).contains(" (z kódu)\n"), "{card}");

    // Neither the code nor a stereotype says anything, and the comment is
    // no claim: the kind and the shape, which the first layer left out
    // because the comment spoke.
    let card = render("QuietView", Lang::En);
    assert_eq!(
        purpose_section(&card),
        format!(
            "## Purpose\n\n`class`: 1 method. (from the structure)\nKeeps the work of one quiet \
             helper. ({QUOTED_COMMENT_EN})\n\n"
        )
    );
}

/// KTD9: without the second layer every card, detail and purpose is what it
/// was — a copied comment's detail included — and with it a card differs in
/// its front matter, its `## Purpose` and the sections it adds after
/// `## Keywords` alone.
#[test]
fn only_the_purpose_of_the_card_changes_and_only_with_the_second_layer() {
    let dir = tempfile::tempdir().unwrap();
    let repo = copies_repo(dir.path());
    let plain = generate(&config(&repo, &dir.path().join("plain"), None)).unwrap();
    let with = generate(&config(&repo, &dir.path().join("with"), Some(2))).unwrap();
    assert_same_first_layer(&plain, &with);

    let analysis = layer2::analyze(&with.graph, with.bodies.as_ref().unwrap());
    let cards = build_cards(&with.graph);
    let index = CardIndex::build(&cards);
    let pair = key(COPIES_NS, "PairA");
    let card = card_of(&cards, &pair);
    // The detail of the type with the copied comment is the plain run's.
    if let Some(detail) = &card.detail {
        let of = |g: &Generated| g.details.iter().find(|d| &d.path == detail).cloned();
        assert!(of(&plain).is_some());
        assert_eq!(of(&plain), of(&with));
    }
    for card in &cards {
        let plain_card = render_card(card, Lang::En, LinkStyle::Portable, &index);
        assert_eq!(
            render_card_layered(card, Lang::En, LinkStyle::Portable, &index, None),
            plain_card
        );
        let Some(analyzed) = analysis.of(&NodeKey::new(card.identity.as_str())) else {
            continue;
        };
        let layered_card = render_card_layered(
            card,
            Lang::En,
            LinkStyle::Portable,
            &index,
            Some(Layer2::present(2, analyzed)),
        );
        assert_eq!(
            first_layer_sections(front_matter(&layered_card).1),
            first_layer_sections(front_matter(&plain_card).1),
            "{}",
            card.identity
        );
        // The cards built here have the first layer's keywords: the
        // section is the plain card's.
        let keywords = |s: &str| {
            s.find("\n## Keywords\n")
                .map(|at| s[at..].lines().nth(3).map(str::to_string))
        };
        assert_eq!(keywords(&layered_card), keywords(&plain_card));
    }
}

/// R18, AE2: the label's comment right after the resource key's call is the
/// menu item's quote, with where the comment is written — even past the cap
/// of the type's comment anchors, which the label never goes through.
#[test]
fn a_menu_item_quotes_the_comment_after_its_resource_key_past_the_anchor_cap() {
    let comments: String = (1..=60)
        .map(|i| format!("            // Setup step number {i} of the context menu goes here.\n"))
        .collect();
    let view = format!(
        r#"namespace Acme.Menus
{{
    public class MenuView
    {{
        private ContextMenu menu;
        public string GetResourceString(string key) {{ return key; }}
        void BuildMenu()
        {{
{comments}            ContextMenuItem f4 = new ContextMenuItem();
            f4.Header = GetResourceString("EditorView_checkErrors") /*"Kontrola chyb"*/;
            f4.Command = CheckCommand;
            menu.Items.Add(f4);
        }}
        public RelayCommand CheckCommand => new RelayCommand(p => Check());
        public void Check() {{ }}
    }}
}}
"#
    );
    let (generated, analysis) = analyzed(&[("src/Menus.cs", view.as_str())]);
    let ns = "sym:csharp:Acme.Menus#";
    // Not vacuous: the type's comments are past the cap.
    assert!(
        generated.graph.nodes_sorted().iter().any(|n| {
            n.key.as_str().starts_with(&format!("{ns}MenuView"))
                && n.facts.iter().any(|f| f.kind == FactKind::AnchorsDropped)
        }),
        "the comment anchors were capped"
    );
    let menu = type_of(&analysis, &key(ns, "MenuView"));
    let f4 = entry(menu, &format!("{ns}MenuView.BuildMenu()#f4"));
    match &f4.menu.as_ref().expect("a menu item").label {
        Some(Label::ResourceKey { key, quote, .. }) => {
            assert_eq!(key, "EditorView_checkErrors");
            let quote = quote.as_ref().expect("the comment after the call");
            assert_eq!(quote.text, "Kontrola chyb");
            // The comment's own place: the line of the call, after it.
            let line = 10 + 60;
            let place: [u32; 4] = quote.place.into();
            assert_eq!(place, [line, 69, line, 87]);
        }
        other => panic!("not a resource key: {other:?}"),
    }
}

/// R18: the label's comment goes through the secret filter, and a comment
/// separated from the call by a comma is no label's.
#[test]
fn a_label_comment_is_redacted_and_one_after_a_comma_is_not_read() {
    let view = r#"namespace Acme.Menus
{
    public class MenuView
    {
        private ContextMenu menu;
        public string GetResourceString(string key) { return key; }
        public string Pick(string a, string b) { return a; }
        void BuildMenu()
        {
            ContextMenuItem f4 = new ContextMenuItem();
            f4.Header = GetResourceString("EditorView_checkErrors") /* password: quillfrond-7Hx */;
            f4.Command = CheckCommand;
            ContextMenuItem f5 = new ContextMenuItem();
            f5.Header = Pick("EditorView_other", /* Ostatní */ "x");
            f5.Command = CheckCommand;
            menu.Items.Add(f4);
            menu.Items.Add(f5);
        }
        public RelayCommand CheckCommand => new RelayCommand(p => Check());
        public void Check() { }
    }
}
"#;
    let (_, analysis) = analyzed(&[("src/Menus.cs", view)]);
    let ns = "sym:csharp:Acme.Menus#";
    let menu = type_of(&analysis, &key(ns, "MenuView"));
    let quote = |item: &str| match &entry(menu, &format!("{ns}MenuView.BuildMenu()#{item}"))
        .menu
        .as_ref()
        .unwrap()
        .label
    {
        Some(Label::ResourceKey { quote, .. }) => quote.clone(),
        other => panic!("not a resource key: {other:?}"),
    };
    let redacted = quote("f4").expect("the comment, redacted");
    assert!(!redacted.text.contains("quillfrond"), "{redacted:?}");
    assert!(redacted.text.contains("[redacted"), "{redacted:?}");
    assert_eq!(quote("f5"), None);
    let serialized = serde_json::to_string(&analysis).unwrap();
    assert!(!serialized.contains("quillfrond"));
}

// ---- The card with the second layer (plan 2a, U6) ----

/// The editor views of [`EDITOR_VIEWS`] with the comments that spell the
/// labels of the menu item and of its submenu (R18).
fn quoted_views() -> String {
    EDITOR_VIEWS
        .replace(
            "GetResourceString(\"EditorView_checkErrors\");",
            "GetResourceString(\"EditorView_checkErrors\") /*\"Kontrola chyb\"*/;",
        )
        .replace(
            "GetResourceString(\"EditorView_settings\");",
            "GetResourceString(\"EditorView_settings\") /*\"Možnosti editoru\"*/;",
        )
}

/// A run over `files` at `level` in `lang`, written to `dir/<out>`.
fn layered_run(
    dir: &Path,
    out: &str,
    files: &[(&str, &str)],
    level: Option<u8>,
    lang: &str,
) -> Generated {
    let repo = dir.join(format!("{out}-repo"));
    common::init_repo_with_files(&repo, "Alice", files);
    generate(&Config {
        lang: lang.to_string(),
        ..config(&repo, &dir.join(out), level)
    })
    .unwrap()
}

/// The rendered card of the type `key`.
fn rendered<'g>(g: &'g Generated, key: &NodeKey) -> &'g str {
    let cards = build_cards(&g.graph);
    let path = card_of(&cards, key).path.clone();
    g.cards
        .iter()
        .find(|c| c.path == path)
        .map(|c| c.contents.as_str())
        .unwrap_or_else(|| panic!("no card at {path}"))
}

/// The section of `card` under `heading` (`## …` or `### …`), through the
/// line before the next heading of its level or above.
fn section<'c>(card: &'c str, heading: &str) -> &'c str {
    let start = card
        .find(&format!("\n{heading}\n"))
        .unwrap_or_else(|| panic!("no {heading:?} in:\n{card}"))
        + 1;
    let rest = &card[start..];
    let sub = heading.starts_with("### ");
    let mut end = rest.len();
    let mut at = 0;
    for (i, line) in rest.split_inclusive('\n').enumerate() {
        if i > 0 && (line.starts_with("## ") || (sub && line.starts_with("### "))) {
            end = at;
            break;
        }
        at += line.len();
    }
    &rest[..end]
}

/// The bullets of a section, the nested ones included.
fn bullets(section: &str) -> Vec<&str> {
    section
        .lines()
        .filter(|l| l.trim_start().starts_with("- "))
        .collect()
}

/// A line ends in its anchors: `` — `path:N` `` or `` `path:N-M` ``, several
/// separated by commas, never with a column.
fn ends_with_anchor(line: &str) -> bool {
    let Some((_, anchors)) = line.rsplit_once(" — ") else {
        return false;
    };
    anchors.split(", ").all(|a| {
        let Some(inner) = a.strip_prefix('`').and_then(|a| a.strip_suffix('`')) else {
            return false;
        };
        let Some((path, lines)) = inner.rsplit_once(':') else {
            return false;
        };
        let number = |n: &str| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit());
        !path.is_empty()
            && !path.contains(':')
            && match lines.split_once('-') {
                Some((a, b)) => number(a) && number(b),
                None => number(lines),
            }
    })
}

/// The `## Keywords` line of a card.
fn keywords_line(card: &str) -> &str {
    section(card, "## Keywords").lines().nth(2).unwrap_or("")
}

/// Every mermaid block of a card.
fn mermaid_blocks(card: &str) -> Vec<&str> {
    card.split("```mermaid\n")
        .skip(1)
        .map(|b| b.split("```").next().unwrap())
        .collect()
}

/// AE2, R17, R18: the view's card holds the check as one functionality with
/// two entry points — F4 with its condition and the menu item with its
/// resource key, the quote of its label and its submenu — both leading to
/// the command of the view model; its detail is the plain run's byte for
/// byte (KTD9 point 3).
#[test]
fn the_view_card_has_one_check_with_the_key_and_the_menu_item() {
    let dir = tempfile::tempdir().unwrap();
    let views = quoted_views();
    let files = [
        ("src/Views.cs", views.as_str()),
        ("src/ViewModels.cs", EDITOR_MODELS),
    ];
    let plain = layered_run(dir.path(), "plain", &files, None, "en");
    let with = layered_run(dir.path(), "with", &files, Some(2), "en");
    assert_same_first_layer(&plain, &with);
    let view = key(VIEWS_NS, "EditorView");
    let card = rendered(&with, &view);
    let (front, _) = front_matter(card);
    assert!(front.starts_with("schema: 7\n"), "{front}");
    assert!(
        front.ends_with("\ndetail_level: 2\nlayer2: present"),
        "{front}"
    );

    let check = section(card, "## Functionality: EditorView.BuildMenu()#f4");
    assert!(
        check.contains("\n- Key: `sym:csharp:Acme.Views#EditorView.BuildMenu()#f4`\n"),
        "{check}"
    );
    assert!(check.contains("\n- Words: check, command\n"), "{check}");
    let entries = bullets(section(check, "### Entry points"));
    assert_eq!(entries.len(), 2, "{entries:?}");
    let menu = entries[0];
    assert!(
        menu.starts_with("- Menu item `f4` (`ContextMenuItem`)"),
        "{menu}"
    );
    for part in [
        "label resource key `EditorView_checkErrors`",
        "quoted “Kontrola chyb”",
        "in the submenu `hotKeys` (label resource key `EditorView_settings`, quoted “Možnosti \
         editoru”",
        "leads to `EditorViewModel.CheckCommand` (through `ViewBase.viewModel`)",
    ] {
        assert!(menu.contains(part), "{part}: {menu}");
    }
    let f4 = entries[1];
    assert!(
        f4.starts_with(
            "- Key `Key.F4` in `Editor_PreviewKeyDown(object,KeyEventArgs)` when \
             `viewModel.ShowErrors || viewModel.CanErrors`: leads to \
             `EditorViewModel.CheckCommand` (through `ViewBase.viewModel`)"
        ),
        "{f4}"
    );
    assert!(entries.iter().all(|e| ends_with_anchor(e)), "{entries:?}");
    // The other branches are functionalities of their own.
    assert!(card.contains(
        "\n## Functionality: EditorView.Editor_PreviewKeyDown(object,KeyEventArgs)#Key.F3\n"
    ));
    assert!(
        card.contains("\n### Context\n"),
        "step 2b draws the context"
    );

    // The detail does not know the second layer.
    let cards = build_cards(&with.graph);
    let detail = card_of(&cards, &view).detail.clone().expect("a detail");
    let of = |g: &Generated| g.details.iter().find(|d| d.path == detail).cloned();
    assert!(of(&plain).is_some());
    assert_eq!(of(&plain), of(&with));
}

/// AE6, R11: on level 1 a type has its functionalities and states with their
/// preconditions and dead paths and its members by name, and no step and no
/// diagram; on level 2 steps and diagrams; on level 3 every member has its
/// sentence. The keywords are the same on every level.
#[test]
fn the_levels_add_steps_diagrams_and_sentences_and_keep_the_keywords() {
    let dir = tempfile::tempdir().unwrap();
    let files = [("src/Flow.cs", FLOW)];
    let runs: Vec<Generated> = (1..=3)
        .map(|level| layered_run(dir.path(), &format!("l{level}"), &files, Some(level), "en"))
        .collect();
    let model = key(FLOW_NS, "EditorViewModel");
    let screen = key(FLOW_NS, "AcmeScreen");
    let cards: Vec<(&str, &str)> = runs
        .iter()
        .map(|g| (rendered(g, &model), rendered(g, &screen)))
        .collect();
    let check = "## Functionality: EditorViewModel.CheckCommand";

    let (one, one_screen) = cards[0];
    for heading in ["### Entry points", "### Preconditions", "### Members"] {
        section(section(one, check), heading);
    }
    for absent in [
        "### Steps",
        "### Flowchart",
        "### State diagram",
        "```mermaid",
    ] {
        assert!(!one.contains(absent), "{absent} on level 1:\n{one}");
        assert!(
            !one_screen.contains(absent),
            "{absent} on level 1:\n{one_screen}"
        );
    }
    // The state, its values, reads and changes are on level 1 too.
    let state = section(one_screen, "## State: AcmeScreen.IsFullscreen");
    for heading in ["### Values", "### Read in conditions", "### Changed by"] {
        section(state, heading);
    }
    // Members by name: one line of names, no sentence.
    let members = section(section(one, check), "### Members");
    let names: Vec<&str> = members.lines().skip(2).filter(|l| !l.is_empty()).collect();
    assert_eq!(names.len(), 1, "{members}");
    assert!(
        names[0].starts_with('`') && !names[0].contains(": "),
        "{members}"
    );

    let (two, two_screen) = cards[1];
    section(section(two, check), "### Steps");
    let chart = section(section(two, check), "### Flowchart");
    assert!(chart.contains("```mermaid\nflowchart TD\n"), "{chart}");
    let diagram = section(
        section(two_screen, "## State: AcmeScreen.IsFullscreen"),
        "### State diagram",
    );
    assert!(diagram.contains("stateDiagram-v2"), "{diagram}");

    let (three, _) = cards[2];
    let members = bullets(section(section(three, check), "### Members"));
    let own: Vec<&&str> = members.iter().filter(|l| l.starts_with("- ")).collect();
    assert!(own.len() >= 5, "{members:?}");
    for m in &own {
        assert!(m.contains(": "), "every member has its sentence: {m}");
        assert!(ends_with_anchor(m), "{m}");
    }
    // Fields and properties too.
    assert!(own.iter().any(|m| m.contains("(field)")), "{own:?}");
    assert!(own.iter().any(|m| m.contains("(property)")), "{own:?}");
    // Their steps under them, each with its anchor.
    assert!(members.iter().any(|m| m.starts_with("  - ")), "{members:?}");

    for (card, screen) in &cards {
        assert_eq!(keywords_line(card), keywords_line(one));
        assert_eq!(keywords_line(screen), keywords_line(one_screen));
    }
}

/// Every step, precondition, dead path and entry point ends in its anchor,
/// `path:N` or `path:N-M`, never a column.
#[test]
fn every_fact_of_the_second_layer_ends_in_its_line_anchor() {
    let dir = tempfile::tempdir().unwrap();
    let g = layered_run(dir.path(), "flow", &[("src/Flow.cs", FLOW)], Some(3), "en");
    let cards = build_cards(&g.graph);
    let mut seen = 0;
    for card in cards.iter().filter(|c| c.identity.starts_with(FLOW_NS)) {
        let Some(file) = g.cards.iter().find(|f| f.path == card.path) else {
            continue;
        };
        let mut sub: Option<&str> = None;
        for line in file.contents.lines() {
            if let Some(h) = line.strip_prefix("### ") {
                sub = Some(h);
                continue;
            }
            if line.starts_with("## ") {
                sub = None;
                continue;
            }
            let anchored = matches!(
                sub,
                Some(
                    "Entry points"
                        | "Preconditions"
                        | "Dead paths"
                        | "Steps"
                        | "Values"
                        | "Changed by"
                )
            );
            if anchored && line.trim_start().starts_with("- ") {
                assert!(ends_with_anchor(line), "{}: {line}", card.identity);
                seen += 1;
            }
        }
    }
    assert!(seen > 20, "not vacuous: {seen} lines");
}

/// KTD10, KTD11: a functionality's heading is its representative's whole
/// key and stays when the words of R16 change and when another branch of
/// the dispatcher comes along.
#[test]
fn a_heading_is_the_representatives_key_and_survives_new_words_and_branches() {
    let dir = tempfile::tempdir().unwrap();
    let base_views = quoted_views();
    let more_views = base_views.replace(
        "else if (e.Key == Key.F11) { viewModel.FullscreenCommand.Execute(null); }",
        "else if (e.Key == Key.F11) { viewModel.FullscreenCommand.Execute(null); }\n            \
         else if (e.Key == Key.F12) { viewModel.Print(); }",
    );
    assert_ne!(more_views, base_views);
    let more_models = EDITOR_MODELS.replace(
        "void Parse(int s) { }",
        "void Parse(int s) { ValidateSyntax(); }\n        void ValidateSyntax() { \
         ValidateSyntaxAgain(); }\n        void ValidateSyntaxAgain() { }\n        public void \
         Print() { }",
    );
    assert_ne!(more_models, EDITOR_MODELS);
    let base = layered_run(
        dir.path(),
        "base",
        &[
            ("src/Views.cs", &base_views),
            ("src/ViewModels.cs", EDITOR_MODELS),
        ],
        Some(2),
        "en",
    );
    let more = layered_run(
        dir.path(),
        "more",
        &[
            ("src/Views.cs", &more_views),
            ("src/ViewModels.cs", &more_models),
        ],
        Some(2),
        "en",
    );
    let headings = |card: &str| -> Vec<String> {
        card.lines()
            .filter(|l| l.starts_with("## Functionality: "))
            .map(str::to_string)
            .collect()
    };
    let view = key(VIEWS_NS, "EditorView");
    let (before, after) = (
        headings(rendered(&base, &view)),
        headings(rendered(&more, &view)),
    );
    assert!(
        before.iter().all(|h| after.contains(h)),
        "{before:?}\n{after:?}"
    );
    assert_eq!(after.len(), before.len() + 1, "{after:?}");

    let model = key(MODELS_NS, "EditorViewModel");
    let check = "## Functionality: EditorViewModel.CheckCommand";
    let words = |g: &Generated| {
        section(rendered(g, &model), check)
            .lines()
            .find(|l| l.starts_with("- Words: "))
            .map(str::to_string)
    };
    assert_ne!(words(&base), words(&more), "the words changed");
    assert!(headings(rendered(&more, &model)).contains(&check.to_string()));
}

/// R8: a flowchart with more nodes than `SPLIT_THRESHOLD` draws that many
/// and says how many there are; every edge it draws joins two drawn nodes.
#[test]
fn a_flowchart_over_the_cap_draws_the_cap_and_says_so() {
    let choices: String = (1..=12)
        .map(|i| format!("            if (mode == {i}) {{ helper.A(); }} else {{ helper.B(); }}\n"))
        .collect();
    let source = format!(
        "namespace Acme.Big\n{{\n    public class Helper\n    {{\n        public void A() {{ }}\n        \
         public void B() {{ }}\n    }}\n\n    public class BigView\n    {{\n        private Helper \
         helper;\n        private int mode;\n        public void Run()\n        {{\n{choices}        \
         }}\n    }}\n}}\n"
    );
    let dir = tempfile::tempdir().unwrap();
    let g = layered_run(dir.path(), "big", &[("src/Big.cs", &source)], Some(2), "en");
    let card = rendered(&g, &NodeKey::new("sym:csharp:Acme.Big#BigView"));
    let chart = section(
        section(card, "## Functionality: BigView.Run()"),
        "### Flowchart",
    );
    let block = mermaid_blocks(chart)[0];
    let nodes: Vec<&str> = block
        .lines()
        .filter(|l| l.starts_with("    n") && !l.contains("-->") && !l.contains("-.->"))
        .collect();
    let cap = 30;
    assert_eq!(nodes.len(), cap, "{block}");
    for edge in block
        .lines()
        .filter(|l| l.contains("-->") || l.contains("-.->"))
    {
        for end in edge.split_whitespace().filter(|w| w.starts_with('n')) {
            let n: usize = end[1..].parse().unwrap();
            assert!(n < cap, "{edge}");
        }
    }
    let lead = format!("The flowchart draws {cap} of ");
    let sentence = chart
        .lines()
        .find(|l| l.starts_with(&lead))
        .unwrap_or_else(|| panic!("no sentence: {chart}"));
    let total: usize = sentence[lead.len()..]
        .split(' ')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    assert!(total > cap, "{sentence}");
    assert!(
        sentence.ends_with(" nodes; the steps above list the rest."),
        "{sentence}"
    );
}

/// R15: a card in Czech has Czech sentences under English headings.
#[test]
fn a_czech_card_has_czech_sentences_and_english_headings() {
    let dir = tempfile::tempdir().unwrap();
    let g = layered_run(dir.path(), "cs", &[("src/Flow.cs", FLOW)], Some(2), "cs");
    let card = rendered(&g, &key(FLOW_NS, "EditorViewModel"));
    let check = section(card, "## Functionality: EditorViewModel.CheckCommand");
    for heading in [
        "### Entry points",
        "### Preconditions",
        "### Steps",
        "### Flowchart",
        "### Members",
    ] {
        section(check, heading);
    }
    for czech in [
        "3 vstupní body.",
        "Nespustí se, když",
        "Příkaz `CheckCommand`",
        "parametr `syntaxOnly` volí",
    ] {
        assert!(check.contains(czech), "{czech}:\n{check}");
    }
    for english in [
        "entry point",
        "Does not run when",
        "Command `",
        "the parameter",
        "leads to",
    ] {
        assert!(!check.contains(english), "{english}:\n{check}");
    }
    let screen = rendered(&g, &key(FLOW_NS, "AcmeScreen"));
    let state = section(screen, "## State: AcmeScreen.IsFullscreen");
    assert!(state.contains("Čte se v 1 podmínce"), "{state}");
    assert!(state.contains("ho přepne"), "{state}");
}

/// KTD9 point 2: a type of another language in a run with the second layer
/// says `layer2: none`, and the rest of its card — keywords included — is
/// the plain run's.
#[test]
fn another_languages_type_card_says_none_and_is_otherwise_the_plain_one() {
    let dir = tempfile::tempdir().unwrap();
    let java = "package acme.shop;\n\npublic class Basket {\n    private int items;\n    \
                public void add() { if (items > 9) { return; } items = items + 1; }\n}\n";
    let files = [("src/Flow.cs", FLOW), ("src/Basket.java", java)];
    let plain = layered_run(dir.path(), "plain", &files, None, "en");
    let with = layered_run(dir.path(), "with", &files, Some(2), "en");
    assert_same_first_layer(&plain, &with);
    let card = |g: &Generated| {
        g.cards
            .iter()
            .find(|c| c.path.starts_with("cards/type/") && c.contents.contains("\n# Basket\n"))
            .map(|c| c.contents.clone())
            .expect("the Java type's card")
    };
    let (plain_card, with_card) = (card(&plain), card(&with));
    let (front, body) = front_matter(&with_card);
    assert!(
        front.ends_with("\ndetail_level: 2\nlayer2: none"),
        "{front}"
    );
    assert_eq!(body, front_matter(&plain_card).1);
    assert!(
        !keywords_line(&with_card).is_empty(),
        "not vacuous: {with_card}"
    );
}

/// KTD9 point 4: the words the second layer brings join the keywords of the
/// analyzed type, weighed by the first layer's document frequencies — a word
/// only it brings (`parser`, a type the check constructs) as one in a single
/// document — and a card of a type it did not analyze keeps its keywords.
#[test]
fn the_second_layers_words_join_the_keywords_of_its_type_alone() {
    let dir = tempfile::tempdir().unwrap();
    let files = [("src/Flow.cs", FLOW)];
    let plain = layered_run(dir.path(), "plain", &files, None, "en");
    let with = layered_run(dir.path(), "with", &files, Some(2), "en");
    let model = key(FLOW_NS, "EditorViewModel");
    let words = |g: &Generated, k: &NodeKey| -> Vec<String> {
        keywords_line(rendered(g, k))
            .split(", ")
            .map(|w| w.split(' ').next().unwrap().to_string())
            .collect()
    };
    assert!(!words(&plain, &model).contains(&"parser".to_string()));
    assert!(
        words(&with, &model).contains(&"parser".to_string()),
        "{:?}",
        words(&with, &model)
    );
    // An enum is not analyzed: its keywords are the plain run's.
    let kind = key(FLOW_NS, "SyntaxKind");
    assert_eq!(
        keywords_line(rendered(&plain, &kind)),
        keywords_line(rendered(&with, &kind))
    );
    assert!(front_matter(rendered(&with, &kind))
        .0
        .ends_with("\nlayer2: present"));
}

/// R3: the diagrams are the same bytes in two runs from two places.
#[test]
fn the_diagrams_are_the_same_bytes_in_two_runs() {
    let dir = tempfile::tempdir().unwrap();
    let files = [("src/Flow.cs", FLOW)];
    let one = layered_run(dir.path(), "one", &files, Some(2), "en");
    let two = layered_run(dir.path(), "two", &files, Some(2), "en");
    let blocks = |g: &Generated| -> Vec<String> {
        g.cards
            .iter()
            .filter(|c| c.path.starts_with("cards/type/"))
            .flat_map(|c| {
                mermaid_blocks(&c.contents)
                    .into_iter()
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .collect()
    };
    let a = blocks(&one);
    assert!(a.iter().any(|b| b.starts_with("flowchart TD\n")), "{a:?}");
    assert!(
        a.iter().any(|b| b.starts_with("stateDiagram-v2\n")),
        "{a:?}"
    );
    assert_eq!(a, blocks(&two));
    assert_eq!(one.cards, two.cards);
}

// ---- A withheld value is admitted in the card (plan 2a, U12: R14, KTD16, AE5) ----

/// For each language: its `--lang`, the sentence a line with a withheld value
/// gets, and the one under a diagram with one.
const LEFT_OUT: [(&str, &str, &str); 2] = [
    (
        "en",
        "a value was left out as a likely secret; see the source",
        "A value in the diagram was left out as a likely secret; see the source.",
    ),
    (
        "cs",
        "hodnota vynechána jako pravděpodobné tajemství; viz zdroj",
        "Hodnota v diagramu vynechána jako pravděpodobné tajemství; viz zdroj.",
    ),
];

/// R14, R18: the quote of a label's comment that held a secret is admitted —
/// the marker stays, one sentence follows it — and not dropped; the quote of
/// a comment that held none is shown as it was, without the sentence.
#[test]
fn a_label_quote_with_a_withheld_value_is_admitted_and_a_plain_quote_is_not() {
    let view = r#"namespace Acme.Menus
{
    public class MenuView
    {
        private ContextMenu menu;
        public string GetResourceString(string key) { return key; }
        void BuildMenu()
        {
            ContextMenuItem f4 = new ContextMenuItem();
            f4.Header = GetResourceString("EditorView_checkErrors") /* password: quillfrond-7Hx */;
            f4.Command = CheckCommand;
            ContextMenuItem f5 = new ContextMenuItem();
            f5.Header = GetResourceString("EditorView_settings") /*"Možnosti editoru"*/;
            f5.Command = CheckCommand;
            menu.Items.Add(f4);
            menu.Items.Add(f5);
        }
        public RelayCommand CheckCommand => new RelayCommand(p => Check());
        public void Check() { }
    }
}
"#;
    let dir = tempfile::tempdir().unwrap();
    for (lang, sentence, _) in LEFT_OUT {
        let g = layered_run(dir.path(), lang, &[("src/Menus.cs", view)], Some(2), lang);
        let card = rendered(&g, &NodeKey::new("sym:csharp:Acme.Menus#MenuView"));
        assert!(!card.contains("quillfrond"), "{lang}: {card}");
        let item = |name: &str| {
            card.lines()
                .find(|l| {
                    l.starts_with(&format!("- Menu item `{name}`"))
                        || l.starts_with(&format!("- Položka menu `{name}`"))
                })
                .unwrap_or_else(|| panic!("{lang}: no menu item {name}:\n{card}"))
        };
        let secret = item("f4");
        assert!(secret.contains("[redacted: "), "{lang}: {secret}");
        assert_eq!(secret.matches(sentence).count(), 1, "{lang}: {secret}");
        assert!(ends_with_anchor(secret), "{lang}: {secret}");
        let plain = item("f5");
        assert!(plain.contains("Možnosti editoru"), "{lang}: {plain}");
        assert!(!plain.contains(sentence), "{lang}: {plain}");
    }
}

/// The node lines of a mermaid block after checking it is valid: every node
/// is `nN`, a shape and one quoted label with no raw `"`, `<`, `>` or
/// backslash in it, and every edge joins two declared nodes.
fn valid_flowchart(block: &str) -> Vec<&str> {
    let mut declared: Vec<&str> = Vec::new();
    let mut edges: Vec<(&str, &str)> = Vec::new();
    let mut nodes: Vec<&str> = Vec::new();
    for line in block.lines().skip(1) {
        let line = line.trim();
        if let Some((from, to)) = line
            .split_once(" --> ")
            .or_else(|| line.split_once(" -.-> "))
        {
            edges.push((from, to));
            continue;
        }
        let id = line
            .split(|c: char| !c.is_ascii_alphanumeric())
            .next()
            .unwrap();
        assert_eq!(line.matches('"').count(), 2, "unbalanced quotes: {line}");
        let open = line.find('"').unwrap();
        let close = line.rfind('"').unwrap();
        let label = &line[open + 1..close];
        for raw in ['<', '>', '\\'] {
            assert!(!label.contains(raw), "raw {raw:?} in a label: {line}");
        }
        declared.push(id);
        nodes.push(line);
    }
    for (from, to) in edges {
        assert!(declared.contains(&from), "{from} is not drawn:\n{block}");
        assert!(declared.contains(&to), "{to} is not drawn:\n{block}");
    }
    nodes
}

/// R14, KTD16.4: a flowchart whose arm compares with a withheld value draws
/// the arm — the node does not vanish and the diagram stays valid — with the
/// marker as it is (the escape changes nothing in it) and a plain string with
/// its quotes escaped; the table of an `else if` chain over one variable
/// admits the value on its row, and one sentence stands under a diagram that
/// holds a marker and under none that does not.
#[test]
fn a_diagram_label_with_a_withheld_value_is_drawn_escaped_and_admitted() {
    let source = r#"namespace Acme.Gate
{
    public class Gate
    {
        public void Admit(string token)
        {
            if (token == "brindlecairn") { Grant(); }
            else if (token == "spare") { Deny(); }
        }
        public void Route(string token, string mode)
        {
            if (token == "brindlecairn") { Open(); }
            else if (mode == "plain") { Close(); }
        }
        public void Idle(int n)
        {
            if (n == 1) { Pause(); }
            else if (n == 2) { Resume(); }
        }
        void Grant() { }
        void Deny() { }
        void Open() { }
        void Close() { }
        void Pause() { }
        void Resume() { }
    }
}
"#;
    let dir = tempfile::tempdir().unwrap();
    let marker = "[redacted: token — see the source anchor]";
    for (lang, sentence, under_diagram) in LEFT_OUT {
        let g = layered_run(dir.path(), lang, &[("src/Gate.cs", source)], Some(2), lang);
        let card = rendered(&g, &NodeKey::new("sym:csharp:Acme.Gate#Gate"));
        assert!(!card.contains("brindlecairn"), "{lang}: {card}");
        let part = |name: &str| section(card, &format!("## Functionality: Gate.{name}"));
        for name in ["Admit(string)", "Route(string,string)"] {
            let chart = section(part(name), "### Flowchart");
            let blocks = mermaid_blocks(chart);
            assert_eq!(blocks.len(), 1, "{lang}: {chart}");
            let nodes = valid_flowchart(blocks[0]);
            let arms: Vec<&&str> = nodes
                .iter()
                .filter(|n| n.contains("when ") || n.contains("když "))
                .collect();
            assert_eq!(arms.len(), 2, "{lang}: both arms are drawn: {chart}");
            assert!(
                arms.iter()
                    .any(|n| n.contains(&format!("token == {marker}"))),
                "{lang}: the marker is drawn as it is: {chart}"
            );
            let withheld = arms.iter().filter(|n| n.contains(marker)).count();
            if name.starts_with("Route") {
                // `mode` names no secret: its string is drawn as it is, with
                // the quotes escaped.
                assert_eq!(withheld, 1, "{lang}: {chart}");
                assert!(
                    arms.iter().any(|n| n.contains("mode == &quot;plain&quot;")),
                    "{lang}: a plain string is drawn with its quotes escaped: {chart}"
                );
            } else {
                // What `token` is compared with is withheld whatever it is.
                assert_eq!(withheld, 2, "{lang}: {chart}");
            }
            assert_eq!(chart.matches(under_diagram).count(), 1, "{lang}: {chart}");
        }
        // The `else if` chain over one variable is a table; its row admits.
        let steps = section(part("Admit(string)"), "### Steps");
        let row = steps
            .lines()
            .find(|l| l.trim_start().starts_with('|') && l.contains(marker))
            .unwrap_or_else(|| panic!("{lang}: no table row with the marker:\n{steps}"));
        assert_eq!(row.matches(sentence).count(), 1, "{lang}: {row}");
        // A diagram with nothing withheld says nothing of it.
        let idle = part("Idle(int)");
        assert!(idle.contains("```mermaid"), "{lang}: {idle}");
        assert!(
            !idle.contains(sentence) && !idle.contains(under_diagram),
            "{lang}: {idle}"
        );
    }
}

// ---- The gate of byte stability and determinism (plan 2a, U9: R1, R3; KTD7, KTD9; AE4) ----
//
// What the gate holds, and where:
//
// 1. The corpus of a run WITHOUT the second layer is the reference of unit U1
//    to the byte, on one thread and on many, from two places:
//    `a_plain_run_writes_the_pinned_bytes_on_one_thread_and_on_many` (the
//    insta snapshot `layer1_corpus_fingerprint`, at the top of this file) and
//    `tests/six_grammar_baseline.rs` (the golden `u1_coverage` and the
//    snapshots of every grammar). Neither is repeated here.
// 2. A run WITH the layer writes the same bytes of every file when it is
//    cold and when it is warm — the second run reads `languages-bodies` from
//    the cache — and from a second cold directory
//    (`a_layered_run_writes_the_same_bytes_cold_and_warm`).
// 3. The same on one thread and on four, through the binary
//    (`a_layered_run_writes_the_same_bytes_on_one_thread_and_on_four`) and
//    through a scoped pool (`tests/card.rs`).
// 4. Every detail, view and map of a run with the layer is the plain run's,
//    byte for byte, on disk (`a_layered_run_leaves_the_plain_runs_details_views_and_maps_as_they_were`),
//    as it is for the typed outputs in `assert_same_first_layer` — the
//    registry of triggers and the line of coverage in `entry-points.md`
//    aside (plan 2b, U7–U8).

/// The files of the gate's fixture: the editor — a view with a key handler and
/// a context menu whose labels are quoted, and its view model — and the flow
/// fixture, which has choices, a loop-free check with a flowchart and a
/// toggled state with a state diagram.
fn gate_files() -> Vec<(String, String)> {
    vec![
        ("src/Views.cs".into(), quoted_views()),
        ("src/ViewModels.cs".into(), EDITOR_MODELS.into()),
        ("src/Flow.cs".into(), FLOW.into()),
    ]
}

fn gate_repo(dir: &Path) -> std::path::PathBuf {
    let repo = dir.join("repo");
    let files = gate_files();
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(p, c)| (p.as_str(), c.as_str()))
        .collect();
    common::init_repo_with_files(&repo, "Alice", &refs);
    repo
}

/// Every file a run over `repo` writes into `out` — the cache aside — as
/// `--detail-level` `level` (or none) and on `threads` worker threads.
fn cli_run(repo: &Path, out: &Path, level: Option<u8>, threads: usize) -> Vec<(String, Vec<u8>)> {
    let mut command = Command::cargo_bin("reverse-architect").unwrap();
    command
        .env("RAYON_NUM_THREADS", threads.to_string())
        .arg(repo)
        .arg("--out")
        .arg(out);
    if let Some(level) = level {
        command.arg("--detail-level").arg(level.to_string());
    }
    command.assert().success();
    common::output_files(out)
}

/// The first path whose bytes differ, or whose presence does.
fn first_difference(a: &[(String, Vec<u8>)], b: &[(String, Vec<u8>)]) -> Option<String> {
    let names =
        |files: &[(String, Vec<u8>)]| files.iter().map(|(p, _)| p.clone()).collect::<Vec<_>>();
    if names(a) != names(b) {
        let (na, nb) = (names(a), names(b));
        return Some(
            na.iter()
                .chain(&nb)
                .find(|n| na.contains(n) != nb.contains(n))
                .cloned()
                .unwrap_or_else(|| "the order of the files".into()),
        );
    }
    a.iter()
        .zip(b)
        .find(|(x, y)| x.1 != y.1)
        .map(|(x, _)| x.0.clone())
}

/// What the fixture's output must hold for the gate to mean something: the
/// layer's sections, a flowchart and a state diagram with bytes of their own,
/// and a menu item with the quote of its label.
fn assert_gate_has_the_layer(files: &[(String, Vec<u8>)]) {
    let cards: String = files
        .iter()
        .filter(|(p, _)| p.starts_with("cards/type/"))
        .map(|(_, b)| String::from_utf8_lossy(b).into_owned())
        .collect();
    for part in [
        "\nlayer2: present",
        "## Functionality: ",
        "### Flowchart",
        "flowchart TD\n",
        "stateDiagram-v2\n",
        "- Menu item `f4`",
        "quoted “Kontrola chyb”",
    ] {
        assert!(cards.contains(part), "the gate's fixture lost {part:?}");
    }
}

/// R1, R3, AE4: a run with the second layer writes the same bytes of every
/// file when the cache is cold and when it is warm, and in a second cold
/// directory. The warm run is shown to read the second layer's records and
/// not to write them anew: they are the very files — bytes and modification
/// times — the cold run left.
#[test]
fn a_layered_run_writes_the_same_bytes_cold_and_warm() {
    let dir = tempfile::tempdir().unwrap();
    let repo = gate_repo(dir.path());
    let out = dir.path().join("warm");
    let cold = cli_run(&repo, &out, Some(2), 1);
    assert_gate_has_the_layer(&cold);

    let stored = records(&out, "languages-bodies");
    assert_eq!(stored.len(), 3, "a record per C# file: {:?}", stored.keys());
    let head = header(&out);

    // The warm run is on four threads, the cold one on one.
    let warm = cli_run(&repo, &out, Some(2), 4);
    assert_eq!(
        records(&out, "languages-bodies"),
        stored,
        "the warm run did not read the bodies from the cache"
    );
    assert_eq!(header(&out), head, "the warm run reset the cache");
    assert_eq!(first_difference(&cold, &warm), None, "cold against warm");

    let again = cli_run(&repo, &dir.path().join("cold-again"), Some(2), 1);
    assert_eq!(first_difference(&cold, &again), None, "cold against cold");

    // The records are what the warm run read, not only what it left alone: a
    // word changed in the record of the views shows on the next run's card.
    let (name, (_, bytes)) = stored
        .iter()
        .find(|(_, (_, b))| String::from_utf8_lossy(b).contains("Kontrola chyb"))
        .expect("the record of the views, with the quote of a label");
    let edited = String::from_utf8_lossy(bytes).replace("Kontrola chyb", "Kontrola chyb zmenena");
    std::fs::write(out.join(CACHE).join("languages-bodies").join(name), edited).unwrap();
    let read = cli_run(&repo, &out, Some(2), 1);
    assert!(
        read.iter().any(|(p, b)| p.starts_with("cards/type/")
            && String::from_utf8_lossy(b).contains("quoted “Kontrola chyb zmenena”")),
        "the run did not read the record it was given"
    );
}

/// R3: the second layer's bytes do not depend on the number of worker
/// threads — one, four, and a repeat of four — each from a cold directory.
#[test]
fn a_layered_run_writes_the_same_bytes_on_one_thread_and_on_four() {
    let dir = tempfile::tempdir().unwrap();
    let repo = gate_repo(dir.path());
    let one = cli_run(&repo, &dir.path().join("one"), Some(2), 1);
    assert_gate_has_the_layer(&one);
    for (name, threads) in [("four", 4), ("four-again", 4)] {
        let many = cli_run(&repo, &dir.path().join(name), Some(2), threads);
        assert_eq!(
            first_difference(&one, &many),
            None,
            "one thread against {threads}"
        );
    }
}

/// KTD9 point 3: with the layer every detail, view and map is the plain run's
/// bytes on disk, and so is every card that is no type's; the type cards
/// differ, and the manifest and coverage with them. Plan 2b adds the
/// registry of triggers, a file of the layer alone (U7), and the line of
/// coverage in `entry-points.md` (U8), which is the plain run's without it.
#[test]
fn a_layered_run_leaves_the_plain_runs_details_views_and_maps_as_they_were() {
    let dir = tempfile::tempdir().unwrap();
    let repo = gate_repo(dir.path());
    let plain = cli_run(&repo, &dir.path().join("plain"), None, 1);
    let layered = cli_run(&repo, &dir.path().join("layered"), Some(2), 1);
    assert_gate_has_the_layer(&layered);

    let same = |path: &str| {
        !path.starts_with("cards/type/")
            && path != "cards/triggers.md"
            && !matches!(path, "manifest.json" | "coverage.json")
            && !path.starts_with("manifest")
    };
    // The layered run's `entry-points.md` without its line of coverage.
    let kept = |files: &[(String, Vec<u8>)], layer: bool| -> Vec<(String, Vec<u8>)> {
        files
            .iter()
            .filter(|(p, _)| same(p))
            .map(|(p, b)| {
                let bytes = if layer {
                    without_coverage_bytes(p, b)
                } else {
                    b.clone()
                };
                (p.clone(), bytes)
            })
            .collect()
    };
    assert!(
        layered.iter().any(|(p, _)| p == "cards/triggers.md"),
        "the registry of triggers (plan 2b, U7)"
    );
    assert!(plain.iter().all(|(p, _)| p != "cards/triggers.md"));
    let (plain_kept, layered_kept) = (kept(&plain, false), kept(&layered, true));
    assert!(
        plain_kept
            .iter()
            .any(|(p, _)| p.starts_with("cards/detail/type/")),
        "not vacuous: the fixture has details"
    );
    assert!(
        plain_kept.iter().any(|(p, _)| p.starts_with("cards/map")),
        "not vacuous: the fixture has maps: {:?}",
        plain_kept.iter().map(|(p, _)| p).collect::<Vec<_>>()
    );
    assert_eq!(first_difference(&plain_kept, &layered_kept), None);
    let type_cards = |files: &[(String, Vec<u8>)]| {
        files
            .iter()
            .filter(|(p, _)| p.starts_with("cards/type/"))
            .count()
    };
    assert_eq!(type_cards(&plain), type_cards(&layered));
}

// ---- The reference of the second layer (plan vrstva 3, U1) ----

/// Every file a run over `repo` with `--detail-level 3` writes into `out`, on
/// `threads` worker threads.
fn reference_run(repo: &Path, out: &Path, threads: usize) -> Vec<(String, Vec<u8>)> {
    Command::cargo_bin("reverse-architect")
        .unwrap()
        .env("RAYON_NUM_THREADS", threads.to_string())
        .arg(repo)
        .arg("--out")
        .arg(out)
        .arg("--detail-level")
        .arg("3")
        .assert()
        .success();
    output_files(out)
}

/// The six-grammar fixture with every C# fixture of the analyses beside it.
fn layered_reference_files() -> Vec<(&'static str, &'static str)> {
    let mut files = common::six_grammar::files();
    files.extend([
        ("src/l2/Views.cs", EDITOR_VIEWS),
        ("src/l2/ViewModels.cs", EDITOR_MODELS),
        ("src/l2/Commands.cs", COMMANDS),
        ("src/l2/Shapes.cs", SHAPES),
        ("src/l2/Clusters.cs", CLUSTERS),
        ("src/l2/Flow.cs", FLOW),
        ("src/l2/Check.cs", CHECK),
        ("src/l2/Copies.cs", COPIES),
        ("src/l2/CopiesPart.cs", COPIES_PART),
    ]);
    files
}

/// What a run with the second layer writes is pinned: the analyses become
/// language profiles (plan vrstva 3, KTD1) without a byte of the C# output
/// moving, and a unit that means to move it re-pins this on purpose.
#[test]
fn a_layered_run_writes_the_pinned_bytes_on_one_thread_and_on_many() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let files = layered_reference_files();
    common::init_repo_with_files(&first.path().join("repo"), "Alice", &files);
    common::init_repo_with_files(&second.path().join("checkout"), "Alice", &files);

    let one = reference_run(&first.path().join("repo"), &first.path().join("corpus"), 1);
    let many = reference_run(
        &second.path().join("checkout"),
        &second.path().join("out"),
        8,
    );
    let card = |name: &str| -> String {
        let path = format!("cards/type/{name}");
        one.iter()
            .find(|(p, _)| p.contains(&path))
            .map(|(_, b)| String::from_utf8_lossy(b).into_owned())
            .unwrap_or_default()
    };
    // Guard the guard: the pinned corpus holds the second layer's sections.
    assert!(
        one.iter().any(|(p, b)| p.starts_with("cards/type/")
            && String::from_utf8_lossy(b).contains("\n## Functionality: ")),
        "the fixture must produce cards with the second layer: {}",
        card("EditorView")
    );

    let a = fingerprint(&one);
    let b = fingerprint(&many);
    assert_eq!(a, b, "the corpus depends on the thread count or the paths");
    insta::assert_snapshot!("layer2_corpus_fingerprint", a);
}

/// A type of a language with no profile gets no analysis: Rust beside C# in
/// one run is read by the first layer only, and coverage names no plugin
/// that reads its bodies (plan vrstva 3, KTD1).
#[test]
fn a_type_of_a_language_without_a_profile_gets_no_second_layer() {
    let (generated, analysis) = analyzed(&[
        ("src/EditorView.cs", VIEW),
        (
            "src/lib.rs",
            "pub struct Ticker { n: i32 }\nimpl Ticker {\n    pub fn tick(&mut self) { self.n += 1; }\n}\n",
        ),
    ]);
    let keys: Vec<&str> = analysis.types.iter().map(|t| t.key.as_str()).collect();
    assert_eq!(keys, ["sym:csharp:Acme.Views#EditorView"]);
    assert!(generated
        .graph
        .node(&NodeKey::new("sym:rust:crate#Ticker"))
        .is_some());
    let bodies = generated
        .coverage
        .bodies
        .as_ref()
        .expect("the second layer");
    assert!(
        bodies.plugins.iter().all(|p| !p.contains("rust")),
        "no plugin reads Rust bodies: {:?}",
        bodies.plugins
    );
}

/// Two body components of two languages in one run (contract 0.3 and 0.4,
/// plan vrstva 3, KTD2): each file's bodies go to the component that claims
/// its extension, the bundled C# one included, and coverage lists all three.
#[test]
fn every_body_component_reads_the_files_of_its_own_extension() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    common::init_repo_with_files(
        &repo,
        "Alice",
        &[
            ("widget.probe3", "x"),
            ("a.probe4", "x"),
            ("roles.probe4", "x"),
            ("src/EditorView.cs", VIEW),
        ],
    );
    let layered = generate(&Config {
        plugins: vec![prebuilt("probe3.wasm"), prebuilt("probe4.wasm")],
        ..config(&repo, &dir.path().join("out"), Some(1))
    })
    .unwrap();
    let layer = layered.bodies.as_ref().unwrap();
    let readers: BTreeMap<&str, &str> = layer
        .files()
        .map(|(path, file)| (path, file.plugin.as_str()))
        .collect();
    assert_eq!(
        readers,
        BTreeMap::from([
            ("a.probe4", "probe4"),
            ("roles.probe4", "probe4"),
            ("src/EditorView.cs", "csharp-body"),
            ("widget.probe3", "probe3"),
        ])
    );
    let coverage = layered.coverage.bodies.as_ref().unwrap();
    assert_eq!(coverage.plugins, ["probe3", "probe4", "csharp-body"]);
    // `roles.probe4` names a role past its model, which is dropped and counted.
    assert_eq!(coverage.dropped_literal_roles, 1);

    // What the 0.4 component said beside its model is kept with it; the
    // 0.3 and C# files carry nothing beside theirs.
    let notes = |path: &str| {
        layer
            .files()
            .find(|(p, _)| *p == path)
            .map(|(_, f)| f.model.notes.clone())
            .unwrap()
    };
    let probe4 = notes("a.probe4");
    let kinds: Vec<CommentKind> = probe4.comments.iter().map(|c| c.kind).collect();
    assert_eq!(kinds, [CommentKind::Prose, CommentKind::Code]);
    assert_eq!(probe4.roles.len(), 1);
    assert_eq!(probe4.roles[0].role, RoleKind::Key);
    assert_eq!(notes("roles.probe4").roles.len(), 1);
    assert_eq!(notes("roles.probe4").dropped_roles, 1);
    assert!(notes("widget.probe3").is_empty());
    assert!(notes("src/EditorView.cs").is_empty());
}
