//! Dynamic-call markers (plan 2c, U6 and U7): the bindings, static members,
//! attached properties, setters and handlers of XAML paired with the C#
//! members they read, write or call — the binding through its template's
//! view and the view's model — and the literals and `nameof`s of C# that
//! name a member or an element of a template.
//!
//! The fixture is the project `tests/fixtures/xaml/Acme.Ui` alone: views on
//! a generic base (`ViewA : ViewBase<ModelB>`), a view holding its model in a
//! field (`PanelD`), one whose model is not known (`Bare`), two views named
//! `ViewA` in `Acme.A` and `Acme.B`, a `UserControl` with `x:Class`
//! (`ViewE`), two looks with a template for `ViewA` each, and models with
//! literals and a `nameof`.

mod common;

use common::{config_for, init_repo_with_files};
use reverse_architect::card::layer2::{
    self, Analysis, DynamicAccess, DynamicMarks, Mark, MarkKind, MarkUse, SiteOutcome, TriggerKind,
    Unpaired, XamlRefKind,
};
use reverse_architect::config::{Config, DEFAULT_AMBIGUITY_CEILING};
use reverse_architect::graph::{Confidence, NodeKey, Rule};
use reverse_architect::plugin::PluginSpec;
use reverse_architect::Generated;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const LOOK_PLAIN: &str = "Acme.Ui/Looks/Plain.xaml";
const LOOK_BOLD: &str = "Acme.Ui/Looks/Bold.xaml";
const MORE: &str = "Acme.Ui/Screens/More.xaml";
const VIEW_E: &str = "Acme.Ui/Screens/ViewE.xaml";
const VIEWS: &str = "Acme.Ui/Screens/Views.cs";
const MODELS: &str = "Acme.Ui/Models/Models.cs";

fn xaml_component() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("plugins/prebuilt/xaml.wasm")
}

/// Every file of the fixture project, repository-relative and `/`-separated,
/// by path, with LF line endings whatever the checkout did.
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
    walk(&root, &root.join("Acme.Ui"), &mut out);
    out.sort();
    out
}

/// The 1-based line of the `nth` (0-based) line holding `needle` in the
/// fixture file `path`.
fn line_of(path: &str, needle: &str, nth: usize) -> u32 {
    let files = fixture_files();
    let text = &files
        .iter()
        .find(|(p, _)| p == path)
        .unwrap_or_else(|| panic!("no fixture file {path}"))
        .1;
    text.lines()
        .enumerate()
        .filter(|(_, l)| l.contains(needle))
        .nth(nth)
        .map(|(i, _)| i as u32 + 1)
        .unwrap_or_else(|| panic!("no {needle:?} #{nth} in {path}"))
}

/// One run over the fixture: with or without the XAML component, with or
/// without the second layer. The directory lives as long as the run.
struct Run {
    _dir: tempfile::TempDir,
    generated: Generated,
}

fn run(xaml: bool, level: Option<u8>) -> Run {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    let files = fixture_files();
    let files: Vec<(&str, &str)> = files
        .iter()
        .map(|(p, t)| (p.as_str(), t.as_str()))
        .collect();
    init_repo_with_files(&repo, "Alice", &files);
    let plugins = if xaml {
        vec![PluginSpec {
            path: xaml_component(),
            sha256: None,
        }]
    } else {
        Vec::new()
    };
    let config = Config {
        detail_level: level,
        plugins,
        ..config_for(&repo, &dir.path().join("out"))
    };
    let generated = reverse_architect::generate(&config).unwrap();
    Run {
        _dir: dir,
        generated,
    }
}

/// The analysis a run renders from, markers included.
fn analysis_of(g: &Generated) -> Analysis {
    let layer = g.bodies.as_ref().expect("a run with the second layer");
    layer2::analyze_run(&g.graph, layer, DEFAULT_AMBIGUITY_CEILING)
}

/// The run with the XAML component and the second layer, and its markers,
/// shared by every test that only reads them.
fn layered() -> &'static (Run, Analysis) {
    static RUN: OnceLock<(Run, Analysis)> = OnceLock::new();
    RUN.get_or_init(|| {
        let run = run(true, Some(2));
        let analysis = analysis_of(&run.generated);
        (run, analysis)
    })
}

fn markers() -> &'static DynamicAccess {
    &layered().1.dynamic
}

fn key(k: &str) -> NodeKey {
    NodeKey::new(k)
}

fn model(member: &str) -> NodeKey {
    key(&format!("sym:csharp:Acme.Models#{member}"))
}

type Seen = (MarkKind, MarkUse, String, u32, Confidence);

/// A member's marks as `(kind, mode, path, line, grade)`, by place.
fn marks(d: &DynamicAccess, member: &NodeKey) -> Vec<Seen> {
    d.of(member)
        .map(|m| m.marks.as_slice())
        .unwrap_or_default()
        .iter()
        .map(|k: &Mark| {
            (
                k.kind,
                k.mode,
                k.place.path.clone(),
                k.place.range.start.line,
                k.grade,
            )
        })
        .collect()
}

fn name_matches(d: &DynamicAccess, member: &NodeKey) -> usize {
    d.of(member).map_or(0, |m| m.name_matches)
}

fn binding(path: &str, line: u32) -> Seen {
    (
        MarkKind::Binding,
        MarkUse::Read,
        path.to_string(),
        line,
        Confidence::RulesBacked,
    )
}

// ---- U6 ----

/// AE2. The binding in each look's template for `ViewA` marks `ModelB.P`,
/// the model `ViewBase<ModelB>` gives the view, at the binding's line; the
/// unrelated `ModelC.P` has no mark with a place, only a match by name — the
/// item template's binding, whose source is the item, not the view, and
/// which marks neither.
#[test]
fn a_binding_through_the_generic_base_marks_the_view_model_and_only_it() {
    let d = markers();
    let item = line_of(LOOK_PLAIN, "{Binding P}", 1);
    assert_eq!(
        marks(d, &model("ModelB.P")),
        [
            binding(LOOK_BOLD, line_of(LOOK_BOLD, "{Binding P}", 0)),
            binding(LOOK_PLAIN, line_of(LOOK_PLAIN, "{Binding P}", 0)),
        ]
    );
    assert!(marks(d, &model("ModelC.P")).is_empty());
    assert_eq!(name_matches(d, &model("ModelC.P")), 1);
    let site = d
        .sites()
        .iter()
        .find(|s| s.place.path == LOOK_PLAIN && s.place.range.start.line == item)
        .expect("the item template's binding is read");
    assert_eq!(
        site.outcome,
        SiteOutcome::Unpaired {
            why: Unpaired::ItemSource
        }
    );
    assert!(site.links.is_empty());
}

/// `Command="{Binding Holder.Command}"`: a mark on `ModelB.Holder` and on
/// `Command` of the type `Holder` is declared with.
#[test]
fn a_command_binding_marks_every_segment_through_the_declared_type() {
    let d = markers();
    let line = line_of(LOOK_PLAIN, "{Binding Holder.Command}", 0);
    assert_eq!(
        marks(d, &model("ModelB.Holder")),
        [binding(LOOK_PLAIN, line)]
    );
    assert_eq!(
        marks(d, &model("Holder.Command")),
        [binding(LOOK_PLAIN, line)]
    );
    // The mark keeps the node it was written at, where the control's tags
    // are (U9).
    let mark = &d.of(&model("Holder.Command")).unwrap().marks[0];
    let node = mark.node.expect("the binding's node");
    let at: Vec<_> = d.sites_at(LOOK_PLAIN, mark.body, node).collect();
    assert_eq!(at.len(), 1);
    assert_eq!(at[0].path, ["Holder", "Command"]);
}

/// A view with `private ModelD viewModel;` and no generic base: the chain
/// goes through the declared field.
#[test]
fn a_view_without_a_generic_base_reaches_its_model_through_the_field() {
    let d = markers();
    assert_eq!(
        marks(d, &model("ModelD.Q")),
        [binding(MORE, line_of(MORE, "{Binding Q}", 0))]
    );
    // The field itself is the chain, not what the markup reads.
    assert!(d
        .of(&key("sym:csharp:Acme.Screens#PanelD.viewModel"))
        .is_none());
}

/// Two views named `ViewA`, in `Acme.A` and `Acme.B`; the template targets
/// `b:ViewA` with `b` mapped to `Acme.B`: only the model of `Acme.B.ViewA`
/// is marked.
#[test]
fn the_template_type_is_the_view_of_exactly_its_full_name() {
    let d = markers();
    assert_eq!(
        marks(d, &model("ModelG.R")),
        [binding(MORE, line_of(MORE, "{Binding R}", 0))]
    );
    assert!(marks(d, &model("ModelF.R")).is_empty());
    assert_eq!(name_matches(d, &model("ModelF.R")), 0);
}

/// A `UserControl` with `x:Class` on `ViewBase<ModelE>`: the binding in its
/// content marks `ModelE.P`; and the handler it names is a method of the
/// class.
#[test]
fn an_x_class_root_starts_the_chain_at_its_class() {
    let d = markers();
    assert_eq!(
        marks(d, &model("ModelE.P")),
        [binding(VIEW_E, line_of(VIEW_E, "{Binding P}", 0))]
    );
    assert_eq!(
        marks(
            d,
            &key("sym:csharp:Acme.Screens#ViewE.OnSave(object,object)")
        ),
        [(
            MarkKind::Handler,
            MarkUse::Call,
            VIEW_E.to_string(),
            line_of(VIEW_E, "Click=\"OnSave\"", 0),
            Confidence::RulesBacked
        )]
    );
}

/// A `DataTemplate` with `DataType` on `ItemModel` inside the view's
/// template: `{Binding Name}` marks `ItemModel.Name`, not the view model's
/// member of the same name.
#[test]
fn a_typed_data_template_marks_the_member_of_its_data_type() {
    let d = markers();
    assert_eq!(
        marks(d, &model("ItemModel.Name")),
        [binding(
            LOOK_PLAIN,
            line_of(LOOK_PLAIN, "{Binding Name}", 0)
        )]
    );
    assert!(marks(d, &model("ModelB.Name")).is_empty());
}

/// AE8. `{x:Static Acme.Settings.Default}` reads the static member, the
/// attached property `beh:Focus.IsInitial` writes through both accessors,
/// and the setter of a style for `ViewA` writes `ViewA.IsBusy` — each with
/// its file and line.
#[test]
fn a_static_member_an_attached_property_and_a_setter_are_marked() {
    let d = markers();
    let rules = Confidence::RulesBacked;
    assert_eq!(
        marks(d, &key("sym:csharp:Acme#Settings.Default")),
        [(
            MarkKind::Static,
            MarkUse::Read,
            LOOK_PLAIN.to_string(),
            line_of(LOOK_PLAIN, "x:Static Acme.Settings.Default", 0),
            rules
        )]
    );
    let attached = line_of(LOOK_PLAIN, "beh:Focus.IsInitial", 0);
    for accessor in ["GetIsInitial(object)", "SetIsInitial(object,bool)"] {
        assert_eq!(
            marks(
                d,
                &key(&format!("sym:csharp:Acme.Behaviors#Focus.{accessor}"))
            ),
            [(
                MarkKind::Attached,
                MarkUse::Write,
                LOOK_PLAIN.to_string(),
                attached,
                rules
            )],
            "{accessor}"
        );
    }
    assert_eq!(
        marks(d, &key("sym:csharp:Acme.Screens#ViewA.IsBusy")),
        [(
            MarkKind::Setter,
            MarkUse::Write,
            LOOK_PLAIN.to_string(),
            line_of(LOOK_PLAIN, "Property=\"IsBusy\"", 0),
            rules
        )]
    );
}

/// A view whose model is not known: the binding stays unpaired, and gives
/// no mark with a place to any member — a match by name only.
#[test]
fn a_view_without_a_known_model_leaves_its_binding_unpaired() {
    let d = markers();
    let line = line_of(MORE, "{Binding Q}", 1);
    let site = d
        .sites()
        .iter()
        .find(|s| s.place.path == MORE && s.place.range.start.line == line)
        .expect("the binding is read");
    assert_eq!(
        site.outcome,
        SiteOutcome::Unpaired {
            why: Unpaired::NoViewModel(key("sym:csharp:Acme.Screens#Bare"))
        }
    );
    assert!(d
        .members()
        .iter()
        .flat_map(|m| &m.marks)
        .all(|k| !(k.place.path == MORE && k.place.range.start.line == line)));
    assert_eq!(name_matches(d, &model("ModelD.Q")), 1);
}

/// A member marked from markup by a rule is no entry from another component
/// in the walk of triggers; a mark from a C# literal is not one from
/// another language, and a match by name is no mark.
#[test]
fn a_member_marked_from_markup_is_no_external_entry() {
    let (run, analysis) = layered();
    let d = &analysis.dynamic;
    let on_save = key("sym:csharp:Acme.Screens#ViewE.OnSave(object,object)");
    // The control: without the markers the handler nothing in C# calls is
    // such an entry.
    let g = &run.generated;
    let unmarked = layer2::analyze(&g.graph, g.bodies.as_ref().unwrap());
    assert!(
        unmarked
            .triggers
            .iter()
            .any(|r| r.trigger.kind == TriggerKind::External && r.trigger.member == on_save),
        "without the markers OnSave is no external entry: the test proves nothing"
    );
    assert!(d.marked(&on_save));
    assert!(d.marked(&model("ModelB.P")));
    assert!(!d.marked(&model("ModelB.Title")), "a C# literal's mark");
    assert!(!d.marked(&model("ModelC.P")), "a match by name");
    assert!(analysis
        .triggers
        .iter()
        .all(|r| !(r.trigger.kind == TriggerKind::External && r.trigger.member == on_save)));
}

/// Without `--detail-level` nothing is analyzed and no marker exists; the
/// cards of the C# types and files of a run with the component differ from
/// those of a run without it only in what references from XAML add: the
/// lines of `## Referenced by` and the sentence on who uses the type ("Used
/// by N in M modules", or "No other type or file … was seen using" without
/// any), and for a reference several types of one name answer the link to
/// the detail and its count of uncertain neighbours. The cards of the
/// modules and of the XAML files are the component's own first layer (lines
/// counted, dependencies, relations) and are not compared.
#[test]
fn without_the_second_layer_the_component_changes_only_what_xaml_references() {
    let with = run(true, None);
    let without = run(false, None);
    for g in [&with.generated, &without.generated] {
        assert!(g.bodies.is_none());
        assert!(g.triggers.is_none());
    }
    let plain: std::collections::BTreeMap<&str, &str> = without
        .generated
        .cards
        .iter()
        .map(|c| (c.path.as_str(), c.contents.as_str()))
        .collect();
    // The cards of the component's own declarations (styles, templates,
    // resources, elements) are new; every other card is in both runs.
    let paths: std::collections::BTreeSet<&str> = with
        .generated
        .cards
        .iter()
        .map(|c| c.path.as_str())
        .collect();
    assert!(plain.keys().all(|p| paths.contains(p)));
    let added: Vec<&&str> = paths.iter().filter(|p| !plain.contains_key(*p)).collect();
    assert!(!added.is_empty());
    assert!(
        added.iter().all(|p| p.starts_with("cards/declared/xaml/")),
        "{added:?}"
    );
    let mut compared = 0;
    let mut differ = 0;
    for card in &with.generated.cards {
        if card.path.starts_with("cards/module/") || card.path.ends_with(".xaml.md") {
            continue;
        }
        let Some(before) = plain.get(card.path.as_str()) else {
            continue;
        };
        compared += 1;
        differ += usize::from(card.contents != *before);
        assert_eq!(
            without_xaml_references(&card.contents),
            without_xaml_references(before),
            "{}",
            card.path
        );
    }
    assert!(
        compared > 10 && differ > 0,
        "{compared} cards, {differ} changed"
    );
}

/// A card without what references from XAML change on it.
fn without_xaml_references(card: &str) -> String {
    let mut out = String::new();
    let mut keep = true;
    for line in card.split_inclusive('\n') {
        if let Some(heading) = line.strip_prefix("## ") {
            keep = !heading.starts_with("Referenced by");
        }
        let reference = [
            "Used by ",
            "was seen using",
            "- Detail: [detail]",
            "more neighbours",
        ]
        .iter()
        .any(|s| line.contains(s));
        if keep && !reference {
            out.push_str(line);
        }
    }
    out
}

/// R20: two runs over the same fixture give the same markers, byte for byte.
#[test]
fn two_runs_give_the_same_markers() {
    let a = run(true, Some(2));
    let b = run(true, Some(2));
    let first = serde_json::to_string(&analysis_of(&a.generated).dynamic).unwrap();
    let second = serde_json::to_string(&analysis_of(&b.generated).dynamic).unwrap();
    assert!(first.contains("rules_backed"), "no marker to compare");
    assert!(first == second, "the markers moved between two runs");
}

// ---- U7 ----

/// AE7. `GetTemplateChild("PART_List")` in `ViewA` and an element
/// `x:Name="PART_List"` in the template for `ViewA` in both looks: two
/// references, each with the place of the call in C#; the literal names no
/// member, so it marks none.
#[test]
fn a_literal_refers_to_the_named_element_of_each_template_for_its_type() {
    let d = markers();
    let line = line_of(VIEWS, "\"PART_List\"", 0);
    let refs: Vec<(&str, &str, u32, &str, XamlRefKind, Confidence)> = d
        .xaml_refs()
        .iter()
        .filter(|r| r.kind == XamlRefKind::Name)
        .map(|r| {
            (
                r.target.as_str(),
                r.place.path.as_str(),
                r.place.range.start.line,
                r.member.as_str(),
                r.kind,
                r.grade,
            )
        })
        .collect();
    let attach = "sym:csharp:Acme.Screens#ViewA.Attach()";
    assert_eq!(
        refs,
        [
            (
                "sym:xaml:Acme.Ui/Looks/Bold.xaml#type:Acme.Screens.ViewA/PART_List",
                VIEWS,
                line,
                attach,
                XamlRefKind::Name,
                Confidence::RulesBacked
            ),
            (
                "sym:xaml:Acme.Ui/Looks/Plain.xaml#type:Acme.Screens.ViewA/PART_List",
                VIEWS,
                line,
                attach,
                XamlRefKind::Name,
                Confidence::RulesBacked
            ),
        ]
    );
    let element = key("sym:xaml:Acme.Ui/Looks/Plain.xaml#type:Acme.Screens.ViewA/PART_List");
    assert_eq!(d.refs_to(&element).count(), 1);
    assert!(d
        .members()
        .iter()
        .flat_map(|m| &m.marks)
        .all(|k| !(k.place.path == VIEWS && k.place.range.start.line == line)));
}

/// `OnPropertyChanged("Title")` in the class that declares `Title`: a mark
/// of the literal, backed by the rule.
#[test]
fn a_literal_in_the_class_that_declares_the_name_marks_the_member() {
    let d = markers();
    assert_eq!(
        marks(d, &model("ModelB.Title")),
        [(
            MarkKind::Literal,
            MarkUse::Named,
            MODELS.to_string(),
            line_of(MODELS, "OnPropertyChanged(\"Title\")", 0),
            Confidence::RulesBacked
        )]
    );
}

/// The literal `"Title"` in a class that does not declare `Title`, while
/// another type does: a match by name only.
#[test]
fn a_literal_in_a_foreign_class_is_only_a_match_by_name() {
    let d = markers();
    let foreign = line_of(MODELS, "Write(\"Title\")", 0);
    assert_eq!(name_matches(d, &model("ModelB.Title")), 1);
    assert!(marks(d, &model("ModelB.Title"))
        .iter()
        .all(|(_, _, _, line, _)| *line != foreign));
}

/// `nameof(Caption)`: a mark as a use of the member `Caption`.
#[test]
fn a_nameof_marks_the_member_it_names() {
    let d = markers();
    assert_eq!(
        marks(d, &model("ModelB.Caption")),
        [(
            MarkKind::NameOf,
            MarkUse::Named,
            MODELS.to_string(),
            line_of(MODELS, "nameof(Caption)", 0),
            Confidence::RulesBacked
        )]
    );
}

/// A literal no name and no element answers: nothing — no mark, no
/// reference.
#[test]
fn a_literal_without_a_name_or_an_element_makes_nothing() {
    let d = markers();
    let lines = [
        (MODELS, line_of(MODELS, "\"Unknown_Value\"", 0)),
        (VIEWS, line_of(VIEWS, "\"PART_None\"", 0)),
        (VIEWS, line_of(VIEWS, "\"NoSuchKey\"", 0)),
    ];
    let at = |path: &str, line: u32| lines.contains(&(path, line));
    assert!(d
        .members()
        .iter()
        .flat_map(|m| &m.marks)
        .all(|k| !at(&k.place.path, k.place.range.start.line)));
    assert!(d
        .xaml_refs()
        .iter()
        .all(|r| !at(&r.place.path, r.place.range.start.line)));
}

/// The key references of the literal on `line` of `Views.cs`: target,
/// grade and scope.
fn key_refs(d: &DynamicAccess, line: u32) -> Vec<(&str, Confidence, Option<Rule>)> {
    d.xaml_refs()
        .iter()
        .filter(|r| {
            r.kind == XamlRefKind::Key && r.place.path == VIEWS && r.place.range.start.line == line
        })
        .map(|r| (r.target.as_str(), r.grade, r.rule))
        .collect()
}

/// R22, a key: `FindResource("MarkerBrush")` in `ViewA` refers to the brush
/// one dictionary of its own project declares, backed by the scope of the
/// project; the reference carries the C# member it is written in.
#[test]
fn a_literal_refers_to_the_resource_of_its_key_in_its_project() {
    let d = markers();
    let line = line_of(VIEWS, "\"MarkerBrush\"", 0);
    assert_eq!(
        key_refs(d, line),
        [(
            "sym:xaml:Acme.Ui/Looks/Plain.xaml#MarkerBrush",
            Confidence::RulesBacked,
            Some(Rule::ScopeProject)
        )]
    );
    let brush = key("sym:xaml:Acme.Ui/Looks/Plain.xaml#MarkerBrush");
    let r = d.refs_to(&brush).next().unwrap();
    assert_eq!(r.member.as_str(), "sym:csharp:Acme.Screens#ViewA.Attach()");
}

/// R22, a key in two motifs: `MotifInk` is declared by no file of the
/// literal's project but at the same path under two sibling projects — two
/// variants found in the repository, a reference to each by name only, not
/// one ambiguity.
#[test]
fn a_key_in_two_motifs_refers_to_each_variant() {
    let d = markers();
    let line = line_of(VIEWS, "\"MotifInk\"", 0);
    assert_eq!(
        key_refs(d, line),
        [
            (
                "sym:xaml:Acme.Ui/Motifs/Blue/Ink.xaml#MotifInk",
                Confidence::NameOnly,
                Some(Rule::NameMatch)
            ),
            (
                "sym:xaml:Acme.Ui/Motifs/Red/Ink.xaml#MotifInk",
                Confidence::NameOnly,
                Some(Rule::NameMatch)
            ),
        ]
    );
}

/// R22, a key nothing declares refers to nothing; nor does a literal that
/// names an element, not a resource.
#[test]
fn a_key_nothing_declares_refers_to_nothing() {
    let d = markers();
    assert!(key_refs(d, line_of(VIEWS, "\"NoSuchKey\"", 0)).is_empty());
    // A literal names a key only of a resource: `PART_List` is an element.
    assert!(key_refs(d, line_of(VIEWS, "\"PART_List\"", 0)).is_empty());
}

/// The literals and `nameof`s of C# mark their members without the XAML
/// component too; nothing refers to markup and no mark comes from it.
#[test]
fn literals_and_nameof_mark_members_without_the_xaml_component() {
    let plain = run(false, Some(2));
    let analysis = analysis_of(&plain.generated);
    let d = &analysis.dynamic;
    let kinds = |member: &str| -> Vec<MarkKind> {
        marks(d, &model(member))
            .into_iter()
            .map(|(kind, ..)| kind)
            .collect()
    };
    assert_eq!(kinds("ModelB.Title"), [MarkKind::Literal]);
    assert_eq!(kinds("ModelB.Caption"), [MarkKind::NameOf]);
    assert!(d.xaml_refs().is_empty());
    assert!(d.sites().is_empty());
    assert!(d
        .members()
        .iter()
        .flat_map(|m| &m.marks)
        .all(|k| !k.kind.in_markup()));
}
