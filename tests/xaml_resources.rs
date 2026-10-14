//! References by name to resources (plan 2c, U4 and U5): the XAML component
//! loads under contract 0.3, and the host resolves every `{StaticResource}`,
//! `{DynamicResource}` and `BasedOn` it reports once every file is read —
//! nearest scope first, configuration variants apart from ambiguity, and a
//! name nothing declares admitted on its file rather than dropped.
//!
//! The fixture (`tests/fixtures/xaml`) is one application project with two
//! themes, three colour schemes, a merged dictionary, a dictionary only code
//! composes, a cycle of merged dictionaries and a file with nested blocks,
//! beside two sibling skin projects.

mod common;

use common::{config_for, init_repo_with_files};
use reverse_architect::config::Config;
use reverse_architect::extract::bodies::{PairedBy, SecondLayer};
use reverse_architect::graph::{
    Confidence, EdgeKind, FactKind, FactValue, Graph, NodeKey, NodeKind, Rule, Span,
};
use reverse_architect::plugin::PluginSpec;
use std::path::{Path, PathBuf};

fn xaml_component() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("plugins/prebuilt/xaml.wasm")
}

/// Every fixture file, repository-relative and `/`-separated, by path, with
/// LF line endings whatever the checkout did.
fn fixture_files() -> Vec<(String, String)> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                let rel = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                let text = std::fs::read_to_string(&path)
                    .unwrap()
                    .replace("\r\n", "\n");
                out.push((rel, text));
            }
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/xaml");
    let mut out = Vec::new();
    walk(&root, &root, &mut out);
    out.sort();
    out
}

fn init(dir: &Path) {
    let files = fixture_files();
    let files: Vec<(&str, &str)> = files
        .iter()
        .map(|(p, t)| (p.as_str(), t.as_str()))
        .collect();
    init_repo_with_files(dir, "Alice", &files);
}

fn with_xaml(repo: &Path, out: &Path) -> Config {
    Config {
        plugins: vec![PluginSpec {
            path: xaml_component(),
            sha256: None,
        }],
        ..config_for(repo, out)
    }
}

fn sym(path: &str, local: &str) -> NodeKey {
    NodeKey::new(format!("sym:xaml:Acme.App/{path}#{local}"))
}

/// One resolved relation as a test reads it.
#[derive(Debug, PartialEq)]
struct Link {
    to: String,
    confidence: Option<Confidence>,
    rule: Option<Rule>,
    ambiguous: Option<usize>,
}

/// The edges of `kind` leaving `from`, by target.
fn links(graph: &Graph, from: &NodeKey, kind: EdgeKind) -> Vec<Link> {
    graph
        .edges_sorted()
        .into_iter()
        .filter(|e| e.from == from && e.edge.kind == kind)
        .map(|e| Link {
            to: e.to.as_str().to_string(),
            confidence: e.edge.provenance.confidence,
            rule: e.edge.provenance.rule,
            ambiguous: e.edge.provenance.ambiguous_candidates,
        })
        .collect()
}

fn backed(to: NodeKey, rule: Rule) -> Link {
    Link {
        to: to.as_str().to_string(),
        confidence: Some(Confidence::RulesBacked),
        rule: Some(rule),
        ambiguous: None,
    }
}

fn texts(graph: &Graph, key: &NodeKey, kind: FactKind) -> Vec<String> {
    graph
        .node(key)
        .unwrap_or_else(|| panic!("no node {key:?}"))
        .facts
        .iter()
        .filter(|f| f.kind == kind)
        .map(|f| match &f.value {
            FactValue::Text(t) => t.clone(),
            other => panic!("{other:?}"),
        })
        .collect()
}

/// U4, Verification: the component loads as a 0.3 plugin, its symbols are
/// nodes of its own kinds, and a run with the second layer pairs the body of
/// a style with the style.
#[test]
fn the_xaml_component_loads_under_0_3_and_its_bodies_pair_with_its_symbols() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    init(&repo);
    let config = Config {
        detail_level: Some(2),
        ..with_xaml(&repo, &dir.path().join("out"))
    };
    let generated = reverse_architect::generate(&config).unwrap();

    let xaml = generated
        .coverage
        .languages
        .iter()
        .find(|l| l.language == "xaml")
        .unwrap_or_else(|| panic!("no xaml in {:?}", generated.coverage.languages));
    assert_eq!(xaml.contracts, ["0.3"]);

    let graph = &generated.graph;
    let style = sym("Styles/Buttons.xaml", "K");
    let node = graph.node(&style).expect("the style is a node");
    let NodeKind::Declared(id) = node.kind else {
        panic!("the style is a {:?}", node.kind)
    };
    assert_eq!(
        graph.vocabulary().node_name(id).as_deref(),
        Some("xaml:style")
    );

    let layer = generated.bodies.as_ref().expect("the second layer");
    assert_eq!(layer.layer_of(&style), SecondLayer::Present);
    let bodies = layer.bodies_of(&style);
    assert_eq!(bodies.len(), 1, "{bodies:?}");
    assert_eq!(bodies[0].by, PairedBy::Name);
}

/// Every scenario of U5 over one run, and the same manifest bytes from a
/// second run served from the cache.
#[test]
fn references_by_name_resolve_scope_by_scope_and_admit_what_nothing_declares() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    init(&repo);
    let out = dir.path().join("out");
    let config = with_xaml(&repo, &out);
    let generated = reverse_architect::generate(&config).unwrap();
    let graph = &generated.graph;

    // AE4. `K` in the merged dictionary, `L` in the file itself although
    // another theme declares it too, `M` only in a dictionary of the same
    // project that no XAML merges.
    let view_b = sym("Views/ViewB.xaml", "x:Class");
    assert_eq!(
        links(graph, &view_b, EdgeKind::References),
        [
            backed(sym("Dictionaries/Composed.xaml", "M"), Rule::ScopeProject),
            backed(sym("Styles/Buttons.xaml", "K"), Rule::ScopeImports),
            backed(sym("Views/ViewB.xaml", "L"), Rule::ScopeFile),
        ]
    );
    // `BasedOn` is the relation the reference names.
    assert_eq!(
        links(graph, &sym("Styles/Buttons.xaml", "K2"), EdgeKind::Extends),
        [backed(sym("Styles/Buttons.xaml", "K"), Rule::ScopeFile)]
    );

    // A key nothing declares: one admission on the file, at its first site,
    // under the plugin's name — not on the view it was written in.
    let view_c_file = NodeKey::new("Acme.App/Views/ViewC.xaml");
    assert_eq!(
        texts(graph, &view_c_file, FactKind::UnresolvedReference),
        ["Missing: no xaml:resource declares this name"]
    );
    let admission = graph
        .node(&view_c_file)
        .unwrap()
        .facts
        .iter()
        .find(|f| f.kind == FactKind::UnresolvedReference)
        .unwrap();
    assert_eq!(&*admission.provenance.extractor, "xaml");
    assert_eq!(admission.provenance.span, Span::Line(5));
    let view_c = sym("Views/ViewC.xaml", "x:Class");
    assert!(texts(graph, &view_c, FactKind::UnresolvedReference).is_empty());

    // Two definitions of one key in nested blocks are two nodes; a reference
    // in the inner block finds the inner one, one in the outer the outer.
    let nested = "Views/Nested.xaml";
    assert!(graph.node(&sym(nested, "N")).is_some());
    assert!(graph.node(&sym(nested, "N~2")).is_some());
    assert_eq!(
        links(graph, &sym(nested, "Innermost"), EdgeKind::References),
        [backed(sym(nested, "N~2"), Rule::ScopeBlock)]
    );
    assert_eq!(
        links(graph, &sym(nested, "Side"), EdgeKind::References),
        [backed(sym(nested, "N"), Rule::ScopeBlock)]
    );

    // AE3. Three colour schemes with the same keys: three variants from each
    // theme, each backed, none ambiguous, no candidate count.
    let schemes = |rule| {
        ["Contrast", "Dark", "Light"]
            .map(|s| backed(sym(&format!("Colors/{s}.xaml"), "B"), rule))
            .into_iter()
            .collect::<Vec<_>>()
    };
    for theme in [
        sym("Themes/Classic.xaml", "ClassicButton"),
        sym("Themes/Modern.xaml", "ModernButton"),
    ] {
        assert_eq!(
            links(graph, &theme, EdgeKind::References),
            schemes(Rule::ScopeProject),
            "{theme:?}"
        );
        assert!(texts(graph, &theme, FactKind::CandidateCount).is_empty());
    }

    // Two unrelated dictionaries of one project: ambiguous, said twice —
    // on every edge and in a candidate count. The skins of two sibling
    // projects are variants by their path, found only by name.
    let view_c_links = links(graph, &view_c, EdgeKind::References);
    let skin = |s: &str| format!("sym:xaml:Acme.Skins/{s}/Brushes.xaml#Accent");
    assert_eq!(
        view_c_links,
        [
            Link {
                to: sym("Dictionaries/One.xaml", "Shared").as_str().to_string(),
                confidence: Some(Confidence::Ambiguous),
                rule: Some(Rule::ScopeProject),
                ambiguous: Some(2),
            },
            Link {
                to: sym("Dictionaries/Two.xaml", "Shared").as_str().to_string(),
                confidence: Some(Confidence::Ambiguous),
                rule: Some(Rule::ScopeProject),
                ambiguous: Some(2),
            },
            Link {
                to: skin("Blue"),
                confidence: Some(Confidence::NameOnly),
                rule: Some(Rule::NameMatch),
                ambiguous: None,
            },
            Link {
                to: skin("Red"),
                confidence: Some(Confidence::NameOnly),
                rule: Some(Rule::NameMatch),
                ambiguous: None,
            },
        ]
    );
    assert_eq!(
        texts(graph, &view_c, FactKind::CandidateCount),
        ["Shared: 2 candidates"]
    );

    // A cycle of merged dictionaries ends, and finds the key across it.
    assert_eq!(
        links(
            graph,
            &sym("Cycle/A.xaml", "CycleUser"),
            EdgeKind::References
        ),
        [backed(sym("Cycle/B.xaml", "CycleKey"), Rule::ScopeImports)]
    );

    // R20: a second run, served from the cache the first one wrote, gives the
    // same manifest bytes — which carry the resolved references.
    let first = reverse_architect::manifest::to_json(&generated.manifest);
    assert!(
        first.contains("scope-imports"),
        "the manifest pins no reference"
    );
    assert!(
        cache_holds(&out.join(".ra-cache"), "\"name_refs\""),
        "no declaration record carries the references, so the second run proves nothing"
    );
    let again = reverse_architect::generate(&config).unwrap();
    assert_eq!(
        links(&again.graph, &view_c, EdgeKind::References),
        view_c_links
    );
    let second = reverse_architect::manifest::to_json(&again.manifest);
    assert!(first == second, "the manifest moved between two runs");
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

/// Without the component nothing reports a reference by name: no scope rule
/// on any edge, no admission anywhere.
#[test]
fn a_run_without_the_xaml_component_resolves_nothing_by_name() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    // The component is bundled: "without" is a corpus without XAML files.
    let files = fixture_files();
    let files: Vec<(&str, &str)> = files
        .iter()
        .filter(|(p, _)| !p.ends_with(".xaml"))
        .map(|(p, t)| (p.as_str(), t.as_str()))
        .collect();
    init_repo_with_files(&repo, "Alice", &files);
    let generated =
        reverse_architect::generate(&config_for(&repo, &dir.path().join("out"))).unwrap();
    let graph = &generated.graph;
    assert!(graph.edges_sorted().iter().all(|e| !matches!(
        e.edge.provenance.rule,
        Some(Rule::ScopeBlock | Rule::ScopeFile | Rule::ScopeImports | Rule::ScopeProject)
    )));
    assert!(graph.nodes_sorted().iter().all(|n| n
        .facts
        .iter()
        .all(|f| f.kind != FactKind::UnresolvedReference)));
}

/// Three tokens of a provider's shape, as the redaction tests spell them.
const TOKEN_A: &str = "ghp_Sm7Tp2Pw9Rd4Xk8Lq3Vn6Bz1";
const TOKEN_B: &str = "ghp_Lq4Wm8Zt2Xv6Nb1Kp9Rs3Dy7Hc5";
const TOKEN_C: &str = "ghp_Q7rT2mX9vB4nL6kP1sD8fH3jW5zC0aE";

const KEYS: &str = "Acme.Keys/Keys.xaml";

/// A dictionary keyed by tokens: a brush keyed `A`, a template keyed `C`
/// with a named element and a button whose content and a style's setter
/// name the resource `B`, which nothing declares.
fn keyed_by_tokens() -> String {
    r#"<ResourceDictionary xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
                    xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml">
    <SolidColorBrush x:Key="@A@" Color="Red"/>
    <ControlTemplate x:Key="@C@" TargetType="Button">
        <Border x:Name="PART_Root">
            <Button Command="{Binding Save}" Content="{StaticResource @B@}"/>
        </Border>
    </ControlTemplate>
    <Style x:Key="Plain" TargetType="Button">
        <Setter Property="Background" Value="{StaticResource @B@}"/>
    </Style>
</ResourceDictionary>
"#
    .replace("@A@", TOKEN_A)
    .replace("@B@", TOKEN_B)
    .replace("@C@", TOKEN_C)
}

/// A run with the second layer over the dictionary keyed by tokens; the
/// directory lives as long as the run.
fn run_keyed_by_tokens() -> (tempfile::TempDir, Config, reverse_architect::Generated) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    let xaml = keyed_by_tokens();
    init_repo_with_files(
        &repo,
        "Alice",
        &[
            (
                "Acme.Keys/Acme.Keys.csproj",
                "<Project Sdk=\"Microsoft.NET.Sdk\" />\n",
            ),
            (KEYS, &xaml),
        ],
    );
    let config = Config {
        detail_level: Some(2),
        ..with_xaml(&repo, &dir.path().join("out"))
    };
    let generated = reverse_architect::generate(&config).unwrap();
    (dir, config, generated)
}

/// Every rendered artifact of a run, as (label, contents): the views, the
/// cards, the details, the maps, the registry of triggers, the manifest in
/// both its forms, the sites and the coverage.
fn rendered(g: &reverse_architect::Generated) -> Vec<(String, String)> {
    use reverse_architect::{coverage, manifest};
    let files = |label: &str, files: &[reverse_architect::render::RenderedFile]| {
        files
            .iter()
            .map(|f| (format!("{label} {}", f.path), f.contents.clone()))
            .collect::<Vec<_>>()
    };
    let mut out = files("view", &g.corpus.files);
    out.extend(files("card", &g.cards));
    out.extend(files("detail", &g.details));
    out.extend(files("map", &g.maps));
    out.extend(files("registry", g.triggers.as_slice()));
    out.push(("manifest.json".into(), manifest::to_json(&g.manifest)));
    out.push(("strings".into(), manifest::to_strings_json(&g.manifest)));
    out.push(("nodes".into(), manifest::to_nodes_jsonl(&g.manifest)));
    out.push(("edges".into(), manifest::to_edges_jsonl(&g.manifest)));
    out.push((
        "documents".into(),
        manifest::to_documents_jsonl(&g.manifest),
    ));
    out.push(("sites".into(), manifest::to_sites_jsonl(&g.graph)));
    out.push(("coverage.json".into(), coverage::to_json(&g.coverage)));
    out
}

/// The mark of a file's opaque keys: the first sixteen hex digits of the
/// SHA-256 of its path.
fn mark(file: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(file.as_bytes()))[..16].to_string()
}

fn opaque(n: usize) -> NodeKey {
    NodeKey::new(format!("sym:xaml:{KEYS}#withheld~{}~{n}", mark(KEYS)))
}

/// Review of 2c (KTD16): a key a plugin builds from a resource's `x:Key`
/// spells the key, and a declared card is titled by its key. A key whose
/// local part the secret filter withholds becomes an opaque one, in
/// declaration order, for every node and edge of its file — and no token
/// reaches the cards, the manifest, the details, the views or the cache, in
/// the first run or in the one the cache serves.
#[test]
fn a_resource_key_the_secret_filter_withholds_is_an_opaque_key_everywhere() {
    let (dir, config, generated) = run_keyed_by_tokens();
    let graph = &generated.graph;
    // The brush, the template and its named element; the style keeps its key.
    for n in 1..=3 {
        assert!(graph.node(&opaque(n)).is_some(), "no node {:?}", opaque(n));
    }
    assert!(graph.node(&opaque(4)).is_none());
    assert!(graph
        .node(&NodeKey::new(format!("sym:xaml:{KEYS}#Plain")))
        .is_some());
    assert_eq!(
        texts(graph, &opaque(3), FactKind::SymbolName),
        ["PART_Root"]
    );
    let leaks = |graph: &Graph| -> Vec<String> {
        let nodes = graph.nodes_sorted();
        let edges = graph.edges_sorted();
        nodes
            .iter()
            .map(|n| n.key.as_str())
            .chain(edges.iter().flat_map(|e| [e.from.as_str(), e.to.as_str()]))
            .filter(|k| k.contains("ghp_"))
            .map(str::to_string)
            .collect()
    };
    assert!(leaks(graph).is_empty(), "{:?}", leaks(graph));
    // The template still owns its element, under the opaque keys.
    assert!(graph
        .edges_sorted()
        .iter()
        .any(|e| e.from == &opaque(2) && e.to == &opaque(3)));

    // A declared card is titled by its key: the opaque one.
    let artifacts = rendered(&generated);
    assert!(
        artifacts
            .iter()
            .any(|(label, text)| label.starts_with("card ")
                && text.contains(&format!("withheld~{}~1", mark(KEYS)))),
        "no card titled by the brush's opaque key"
    );
    for (label, text) in &artifacts {
        for token in [TOKEN_A, TOKEN_B, TOKEN_C] {
            assert!(!text.contains(token), "{token} reached {label}");
        }
    }
    let out = &config.corpus_dir;
    assert!(out.join(".ra-cache").is_dir(), "the run kept no cache");
    for token in [TOKEN_A, TOKEN_B, TOKEN_C] {
        assert!(
            !cache_holds(out, token),
            "{token} reached {}",
            out.display()
        );
    }
    let again = reverse_architect::generate(&config).unwrap();
    assert!(leaks(&again.graph).is_empty(), "{:?}", leaks(&again.graph));
    assert_eq!(
        again
            .graph
            .nodes_sorted()
            .iter()
            .map(|n| n.key.as_str().to_string())
            .collect::<Vec<_>>(),
        graph
            .nodes_sorted()
            .iter()
            .map(|n| n.key.as_str().to_string())
            .collect::<Vec<_>>()
    );
    for token in [TOKEN_A, TOKEN_B, TOKEN_C] {
        assert!(
            !cache_holds(out, token),
            "{token} reached {}",
            out.display()
        );
    }
    drop(dir);
}

/// Review of 2c: two different withheld names redact to one marker, and a
/// marker is no name. The setter and the button name `B`, which nothing
/// declares; the brush declares `A`. Neither resolves to the brush: the file
/// admits one unresolved reference, its text the marker's.
#[test]
fn two_different_withheld_names_never_resolve_to_each_other() {
    let (_dir, _config, generated) = run_keyed_by_tokens();
    let graph = &generated.graph;
    let brush = opaque(1);
    assert!(graph
        .edges_sorted()
        .iter()
        .all(|e| e.edge.kind != EdgeKind::References || e.to != &brush));
    assert!(links(
        graph,
        &NodeKey::new(format!("sym:xaml:{KEYS}#Plain")),
        EdgeKind::References
    )
    .is_empty());
    assert_eq!(
        texts(graph, &NodeKey::new(KEYS), FactKind::UnresolvedReference),
        ["[redacted: provider token — see the source anchor]: no xaml:resource declares this name"]
    );
}
