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
            assert!(quote.is_none(), "the quote is U7's");
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
