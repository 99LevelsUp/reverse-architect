//! Step 2b, U2 and U3: the links of an access graded across types, and the
//! binding of a delegate from the code of the class constructed — over the
//! same walk and the same binding the entry points of plan 2a resolve by, so
//! the analysis of a card and its flow never disagree. U1: the reverse index
//! of callers and the class of a member. U4: the results of a functionality
//! and the state read and written from outside, over the editor of the
//! acceptance examples (`common/editor_fixture.rs`). U5: the triggers and
//! their flows. U6: the `### Context` the cards draw of them. U7: the
//! registry of triggers, `cards/triggers.md`, and its line of coverage. U8:
//! that line in the view `entry-points`.

mod common;

use reverse_architect::card::layer2::{
    self, bindings_of, raised_events, Analysis, Basis, Caller, CallerClass, Callers, CommonPart,
    DelegateBinding, EntryPoint, FlowResult, FlowResults, FlowStepKind, Functionality,
    FunctionalityContext, FunctionalityRef, Index, Links, Outcome, PathState, Resolution,
    ResultKind, Site, Source, StopCause, StoppedPath, TriggerFlow, TriggerKind, TriggerRow,
    TypeAnalysis, TypeResults, Unresolved, Via,
};
use reverse_architect::config::{Config, DEFAULT_AMBIGUITY_CEILING};
use reverse_architect::extract::bodies::Constant;
use reverse_architect::graph::{
    Confidence, Edge, EdgeKind, ExtractorId, NodeKey, Provenance, Rule, SiteId, Tier,
};
use reverse_architect::{generate, Generated};
use std::path::Path;

fn config(repo: &Path, out: &Path) -> Config {
    Config {
        detail_level: Some(2),
        ..common::config_for(repo, out)
    }
}

/// A run with the second layer over `files`, and its analysis.
fn analyzed(files: &[(&str, &str)]) -> (Generated, Analysis) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    common::init_repo_with_files(&repo, "Alice", files);
    let generated = generate(&config(&repo, &dir.path().join("out"))).unwrap();
    let analysis = layer2::analyze(&generated.graph, generated.bodies.as_ref().unwrap());
    (generated, analysis)
}

fn key(ns: &str, rest: &str) -> NodeKey {
    NodeKey::new(format!("{ns}{rest}"))
}

fn type_of<'a>(analysis: &'a Analysis, key: &NodeKey) -> &'a TypeAnalysis {
    analysis
        .of(key)
        .unwrap_or_else(|| panic!("{key:?} is not analyzed"))
}

fn entry<'a>(t: &'a TypeAnalysis, key: &str) -> &'a EntryPoint {
    let all: Vec<&EntryPoint> = t
        .functionalities
        .iter()
        .flat_map(|f| f.entry_points.iter())
        .collect();
    all.iter()
        .copied()
        .find(|e| e.key == key)
        .unwrap_or_else(|| {
            panic!(
                "no entry point {key}: {:?}",
                all.iter().map(|e| &e.key).collect::<Vec<_>>()
            )
        })
}

/// The site of `member` whose path is `written` (`viewModel.CheckCommand`),
/// a call when `call` says so.
fn site<'s>(sites: &'s [Site], written: &str, call: bool) -> &'s Site {
    sites
        .iter()
        .find(|s| s.path.segments.join(".") == written && s.call.is_some() == call)
        .unwrap_or_else(|| {
            panic!(
                "no site {written}: {:?}",
                sites
                    .iter()
                    .map(|s| s.path.segments.join("."))
                    .collect::<Vec<_>>()
            )
        })
}

fn links_of(r: &Resolution) -> Vec<(&str, Basis, Confidence)> {
    r.links
        .iter()
        .map(|l| (l.link.member.as_str(), l.basis, l.grade))
        .collect()
}

fn names(keys: &[NodeKey]) -> Vec<&str> {
    keys.iter().map(NodeKey::as_str).collect()
}

// ---- U2: the links of an access, graded ----

const VIEWS: &str = r#"using Acme.ViewModels;

namespace Acme.Views
{
    public class ViewBase<T>
    {
        protected T viewModel;
        void Refresh() { viewModel.CheckCommand.Execute(null); }
    }

    public class EditorView : ViewBase<EditorViewModel>
    {
        void OnKey(KeyEventArgs e)
        {
            if (e.Key == Key.F4) { this.viewModel.CheckCommand.Execute(null); }
            else if (e.Key == Key.F5) { viewModel.Check(true); }
            else if (e.Key == Key.F6) { viewModel.Do(1); }
        }
        public void Focus(object o)
        {
            if (o is not EditorView other) return;
            other.viewModel.Save();
        }
    }

    public class NestedView : ViewBase<List<EditorViewModel>>
    {
        void OnKey(KeyEventArgs e)
        {
            if (e.Key == Key.F4) { viewModel.CheckCommand.Execute(null); }
        }
    }

    public class OuterView : ForeignView<EditorViewModel>
    {
        void OnKey(KeyEventArgs e)
        {
            if (e.Key == Key.F4) { viewModel.CheckCommand.Execute(null); }
        }
    }

    public partial class SplitView
    {
        void OnKey(KeyEventArgs e)
        {
            if (e.Key == Key.F4) { viewModel.CheckCommand.Execute(null); }
        }
    }

    public partial class TwoBases : ViewBase<EditorViewModelNew>
    {
        void OnKey(KeyEventArgs e)
        {
            if (e.Key == Key.F4) { viewModel.CheckCommand.Execute(null); }
        }
    }
}
"#;

const SPLIT: &str = r#"namespace Acme.Views
{
    public partial class SplitView : ViewBase<EditorViewModel>
    {
    }

    public partial class TwoBases : ViewBase<EditorViewModel>
    {
    }
}
"#;

const MODELS: &str = r#"namespace Acme.ViewModels
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
        public RelayCommand CheckCommand => new RelayCommand(p => Check());
        public void Check() { }
        public void Check(bool syntaxOnly) { }
        public void Do(int a) { }
        public void Do(string b) { }
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

fn editor() -> (Generated, Analysis) {
    analyzed(&[
        ("src/Views.cs", VIEWS),
        ("src/Split.cs", SPLIT),
        ("src/ViewModels.cs", MODELS),
    ])
}

/// AE3: `this.viewModel.CheckCommand` in `EditorView : ViewBase<EditorViewModel>`
/// goes to `EditorViewModel.CheckCommand` by the base list, rules-backed —
/// never to `EditorViewModelNew`, which declares a `CheckCommand` too — and
/// on through the property's declared type to `RelayCommand.Execute`.
#[test]
fn a_member_through_the_base_list_is_rules_backed_and_never_the_other_type() {
    let (generated, _) = editor();
    let ix = Index::new(&generated.graph, generated.bodies.as_ref().unwrap());
    let links = Links::new(&ix, DEFAULT_AMBIGUITY_CEILING);
    let sites = links.sites_of(&key(VIEWS_NS, "EditorView.OnKey(KeyEventArgs)"));

    let f4 = &site(&sites, "viewModel.CheckCommand.Execute", true).resolution;
    assert_eq!(
        links_of(f4),
        [
            (
                format!("{VIEWS_NS}ViewBase.viewModel").as_str(),
                Basis::Enclosing,
                Confidence::RulesBacked
            ),
            (
                format!("{MODELS_NS}EditorViewModel.CheckCommand").as_str(),
                Basis::BaseTypeArgument,
                Confidence::RulesBacked
            ),
            (
                format!("{MODELS_NS}RelayCommand.Execute(object)").as_str(),
                Basis::DeclaredType,
                Confidence::RulesBacked
            ),
        ]
    );
    assert_eq!(
        f4.target(),
        Some(&key(MODELS_NS, "RelayCommand.Execute(object)"))
    );
    assert_eq!(f4.grade(), Some(Confidence::RulesBacked));
    assert!(f4.candidates.is_empty());
    for s in &sites {
        for l in &s.resolution.links {
            assert_ne!(
                l.link.member,
                key(MODELS_NS, "EditorViewModelNew.CheckCommand"),
                "{s:?}"
            );
        }
    }

    // A declaration pattern types its variable.
    let focus = links.sites_of(&key(VIEWS_NS, "EditorView.Focus(object)"));
    let save = &site(&focus, "other.viewModel.Save", true).resolution;
    assert_eq!(
        links_of(save)
            .iter()
            .map(|(_, b, _)| *b)
            .collect::<Vec<_>>(),
        [Basis::Pattern, Basis::BaseTypeArgument]
    );
    assert_eq!(
        save.target(),
        Some(&key(MODELS_NS, "EditorViewModel.Save()"))
    );
}

/// One overload of the call's arity on another type is rules-backed; two of
/// one arity are ambiguous and carry both as candidates, with no target.
#[test]
fn an_overload_of_another_type_is_chosen_by_arity_and_two_of_one_arity_are_ambiguous() {
    let (generated, _) = editor();
    let ix = Index::new(&generated.graph, generated.bodies.as_ref().unwrap());
    let links = Links::new(&ix, DEFAULT_AMBIGUITY_CEILING);
    let sites = links.sites_of(&key(VIEWS_NS, "EditorView.OnKey(KeyEventArgs)"));

    let check = &site(&sites, "viewModel.Check", true).resolution;
    assert_eq!(
        check.target(),
        Some(&key(MODELS_NS, "EditorViewModel.Check(bool)"))
    );
    assert_eq!(check.links[1].basis, Basis::Overload);
    assert_eq!(check.grade(), Some(Confidence::RulesBacked));

    let r#do = &site(&sites, "viewModel.Do", true).resolution;
    assert_eq!(r#do.target(), None);
    assert_eq!(
        r#do.outcome,
        Outcome::Ambiguous {
            reason: Unresolved::AmbiguousMember("Do".into())
        }
    );
    assert_eq!(r#do.grade(), Some(Confidence::Ambiguous));
    assert_eq!(
        names(&r#do.candidates.shown),
        [
            format!("{MODELS_NS}EditorViewModel.Do(int)").as_str(),
            format!("{MODELS_NS}EditorViewModel.Do(string)").as_str(),
        ]
    );
    assert_eq!(r#do.candidates.more, 0);
}

/// Without a base list to substitute from (the generic base's own code), the
/// chain breaks after `viewModel`: unresolved, both `CheckCommand`s as
/// candidates by key, no target and no grade — no edge from a name.
#[test]
fn a_chain_without_a_base_list_is_unresolved_with_both_candidates() {
    let (generated, _) = editor();
    let ix = Index::new(&generated.graph, generated.bodies.as_ref().unwrap());
    let links = Links::new(&ix, DEFAULT_AMBIGUITY_CEILING);
    let sites = links.sites_of(&key(VIEWS_NS, "ViewBase.Refresh()"));
    let r = &site(&sites, "viewModel.CheckCommand.Execute", true).resolution;
    assert_eq!(
        r.outcome,
        Outcome::Unresolved {
            reason: Unresolved::UnboundTypeParameter("T".into())
        }
    );
    assert_eq!(r.target(), None);
    assert_eq!(r.grade(), None);
    assert_eq!(links_of(r).len(), 1, "{r:?}");
    assert_eq!(
        names(&r.candidates.shown),
        [
            format!("{MODELS_NS}EditorViewModel.CheckCommand").as_str(),
            format!("{MODELS_NS}EditorViewModelNew.CheckCommand").as_str(),
        ]
    );
}

/// The limits of 2a hold for the links of 2b, by the same walk: a nested
/// generic argument and an ancestor outside the repository leave the access
/// unresolved, and the entry point of the same card admits its target for
/// the same reason. A base list written on another part of a partial type is
/// read from that part; parts that give the base different arguments leave
/// it unresolved.
#[test]
fn the_limits_of_2a_hold_and_the_entry_point_admits_the_same_reason() {
    let (generated, analysis) = editor();
    let ix = Index::new(&generated.graph, generated.bodies.as_ref().unwrap());
    let links = Links::new(&ix, DEFAULT_AMBIGUITY_CEILING);
    let same = |ty: &str| -> Resolution {
        let member = format!("{ty}.OnKey(KeyEventArgs)");
        let sites = links.sites_of(&key(VIEWS_NS, &member));
        let r = site(&sites, "viewModel.CheckCommand.Execute", true)
            .resolution
            .clone();
        let card = type_of(&analysis, &key(VIEWS_NS, ty));
        let f4 = entry(card, &format!("{VIEWS_NS}{member}#Key.F4"));
        assert_eq!(f4.targets.len(), 1, "{ty}");
        assert_eq!(f4.targets[0].chain, r.chain(), "{ty}");
        assert_eq!(f4.targets[0].unresolved(), r.reason(), "{ty}");
        r
    };

    let nested = same("NestedView");
    assert_eq!(
        nested.reason(),
        Some(&Unresolved::NestedGeneric("List<EditorViewModel>".into()))
    );
    assert_eq!(nested.target(), None);

    // The base outside the repository may declare the name: its members
    // are not known, so the name is not said to be no member.
    let outer = same("OuterView");
    assert_eq!(
        outer.reason(),
        Some(&Unresolved::ExternalType("ForeignView".into()))
    );
    assert_eq!(outer.target(), None);

    let two = same("TwoBases");
    assert_eq!(
        two.reason(),
        Some(&Unresolved::BaseListNotRead(key(VIEWS_NS, "TwoBases")))
    );
    assert_eq!(two.target(), None);
    assert_eq!(two.candidates.total(), 2, "{two:?}");

    let split = same("SplitView");
    assert_eq!(
        split.target(),
        Some(&key(MODELS_NS, "RelayCommand.Execute(object)")),
        "{split:?}"
    );
}

const MANY: &str = r#"namespace Acme.Many
{
    public class A0 { public void Ping() { } }
    public class A1 { public void Ping() { } }
    public class A2 { public void Ping() { } }
    public class A3 { public void Ping() { } }
    public class A4 { public void Ping() { } }
    public class A5 { public void Ping() { } }
    public class A6 { public void Ping() { } }
    public class A7 { public void Ping() { } }
    public class A8 { public void Ping() { } }
    public class A9 { public void Ping() { } }

    public class Caller
    {
        void Go() { Lookup().Ping(); }
        object Lookup() { return null; }
    }
}
"#;

const MANY_NS: &str = "sym:csharp:Acme.Many#";

/// KTD9: more candidates than the ceiling — the first by key up to the
/// ceiling, and how many more.
#[test]
fn candidates_past_the_ceiling_are_counted_not_named() {
    let (generated, _) = analyzed(&[("src/Many.cs", MANY)]);
    let ix = Index::new(&generated.graph, generated.bodies.as_ref().unwrap());
    let go = key(MANY_NS, "Caller.Go()");

    let links = Links::new(&ix, DEFAULT_AMBIGUITY_CEILING);
    let sites = links.sites_of(&go);
    let r = &site(&sites, "Ping", true).resolution;
    assert_eq!(r.reason(), Some(&Unresolved::NoReceiverChain));
    let expected: Vec<String> = (0..8).map(|i| format!("{MANY_NS}A{i}.Ping()")).collect();
    assert_eq!(names(&r.candidates.shown), expected);
    assert_eq!(r.candidates.more, 2);

    let low = Links::new(&ix, 3);
    let sites = low.sites_of(&go);
    let r = &site(&sites, "Ping", true).resolution;
    assert_eq!(r.candidates.shown.len(), 3);
    assert_eq!(r.candidates.more, 7);
}

/// The line of the site `written` of `member` in `src/Views.cs`.
fn line_of(links: &Links<'_, '_>, member: &NodeKey, written: &str) -> u32 {
    let sites = links.sites_of(member);
    let s = site(&sites, written, true);
    assert_eq!(s.place.path, "src/Views.cs");
    s.place.range.start.line
}

/// A relation an external index pinned, as the first layer holds one after
/// the index's pairing: resolved, at its site.
fn indexed(file: &str, line: u32) -> Edge {
    Edge::new(
        EdgeKind::Calls,
        Provenance::new(file, Some(line), ExtractorId::from("scip"), Tier::Measured).resolved(),
    )
    .at_sites([SiteId {
        file: file.to_string(),
        line,
        column: None,
    }])
}

/// KTD3: a relation the external index pinned from the member at the line of
/// the access comes first — `ResolvedByIndex` on that link, also where the
/// rules could not tell the type in hand — and the access's grade is the
/// weakest of its links. A pinned relation read at another line is not this
/// access's.
#[test]
fn an_index_relation_at_the_line_comes_first_and_is_resolved_by_index() {
    let (mut generated, _) = editor();
    let refresh = key(VIEWS_NS, "ViewBase.Refresh()");
    let on_key = key(VIEWS_NS, "EditorView.OnKey(KeyEventArgs)");
    let command = key(MODELS_NS, "EditorViewModel.CheckCommand");
    let (refresh_line, f4_line) = {
        let ix = Index::new(&generated.graph, generated.bodies.as_ref().unwrap());
        let links = Links::new(&ix, DEFAULT_AMBIGUITY_CEILING);
        (
            line_of(&links, &refresh, "viewModel.CheckCommand.Execute"),
            line_of(&links, &on_key, "viewModel.CheckCommand.Execute"),
        )
    };
    let graph = &mut generated.graph;
    assert!(graph.add_edge(&refresh, &command, indexed("src/Views.cs", refresh_line)));
    assert!(graph.add_edge(&on_key, &command, indexed("src/Views.cs", f4_line)));
    // At another line of the same member: not the access at F5.
    assert!(graph.add_edge(
        &on_key,
        &key(MODELS_NS, "EditorViewModel.Do(int)"),
        indexed("src/Views.cs", f4_line)
    ));

    let ix = Index::new(&generated.graph, generated.bodies.as_ref().unwrap());
    let links = Links::new(&ix, DEFAULT_AMBIGUITY_CEILING);

    let sites = links.sites_of(&refresh);
    let r = &site(&sites, "viewModel.CheckCommand.Execute", true).resolution;
    assert_eq!(
        links_of(r),
        [
            (
                format!("{VIEWS_NS}ViewBase.viewModel").as_str(),
                Basis::Enclosing,
                Confidence::RulesBacked
            ),
            (command.as_str(), Basis::Index, Confidence::ResolvedByIndex),
            (
                format!("{MODELS_NS}RelayCommand.Execute(object)").as_str(),
                Basis::DeclaredType,
                Confidence::RulesBacked
            ),
        ]
    );
    assert_eq!(r.grade(), Some(Confidence::RulesBacked));

    let sites = links.sites_of(&on_key);
    let f4 = &site(&sites, "viewModel.CheckCommand.Execute", true).resolution;
    assert_eq!(f4.links[1].link.member, command);
    assert_eq!(f4.links[1].grade, Confidence::ResolvedByIndex);
    // `Do(int)` was pinned at the F4 line, not at the F6 one: the F6 call
    // stays ambiguous.
    let r#do = &site(&sites, "viewModel.Do", true).resolution;
    assert_eq!(r#do.grade(), Some(Confidence::Ambiguous));
}

// ---- U3: the binding of a delegate ----

const COMMANDS: &str = r#"namespace Acme.Commands
{
    public class RelayCommand
    {
        private readonly Action<object> _execute;
        private readonly Predicate<object> _canExecute;
        private readonly string _name;
        public event EventHandler Executed;
        public RelayCommand(Action<object> execute) : this(execute, null) { }
        public RelayCommand(Action<object> execute, Predicate<object> canExecute) : this(execute, canExecute, null) { }
        public RelayCommand(Action<object> execute, Predicate<object> canExecute, string name)
        {
            _execute = execute;
            _canExecute = canExecute;
            _name = name;
        }
        public bool CanExecute(object parameter) { return _canExecute == null || _canExecute(parameter); }
        public void Execute(object parameter)
        {
            _execute?.Invoke(parameter);
            Executed?.Invoke(this, null);
        }
    }

    public class Runner
    {
        private Action<int> _go;
        public Runner(Action<int> go) { _go = go; }
        public void Start() { _go(1); }
    }

    public class Twice
    {
        private Action _run;
        public Twice(Action a, Action b) { _run = a; _run = b; }
        public void Execute() { _run(); }
    }

    public class Panel
    {
        public RelayCommand CheckCommand => new RelayCommand(p => Check(), p => CanCheck());
        public RelayCommand QuickCommand => new RelayCommand(p => Check());
        public RelayCommand GroupCommand => new RelayCommand(Check);
        public Runner Background => new Runner(n => Check());
        public Twice Both => new Twice(() => Check(), () => CanCheck());
        void Check() { }
        void Check(object p) { }
        bool CanCheck() { return true; }
    }
}
"#;

const COMMANDS_NS: &str = "sym:csharp:Acme.Commands#";

/// (method, target, grade) of a binding's pairs.
type Pairs = Vec<(String, Option<String>, Option<Confidence>)>;

/// The binding written in `property` of `Panel`, and its pairs.
fn pairs_in(links: &Links<'_, '_>, property: &str) -> (DelegateBinding, Pairs) {
    let member = key(COMMANDS_NS, &format!("Panel.{property}"));
    let site = bindings_of(links, &key(COMMANDS_NS, "Panel"))
        .into_iter()
        .find(|s| s.member == member)
        .unwrap_or_else(|| panic!("no binding in {property}"));
    let binding = site.binding.expect("bound");
    let pairs = binding
        .pairs()
        .into_iter()
        .map(|p| {
            (
                p.method.as_str().to_string(),
                p.target.target().map(|t| t.as_str().to_string()),
                p.grade,
            )
        })
        .collect();
    (binding, pairs)
}

fn commands() -> (Generated, Analysis) {
    analyzed(&[("src/Commands.cs", COMMANDS)])
}

/// `new RelayCommand(p => Check(), p => CanCheck())` through a constructor of
/// two parameters chained on to one of three: `Execute` → `Check`,
/// `CanExecute` → `CanCheck`, each rules-backed; `Execute` also raises
/// `Executed`, which no constructor writes.
#[test]
fn a_construction_through_two_constructors_pairs_each_method_with_its_lambda() {
    let (generated, _) = commands();
    let ix = Index::new(&generated.graph, generated.bodies.as_ref().unwrap());
    let links = Links::new(&ix, DEFAULT_AMBIGUITY_CEILING);
    let (binding, pairs) = pairs_in(&links, "CheckCommand");
    assert_eq!(
        binding.constructor,
        key(
            COMMANDS_NS,
            "RelayCommand.RelayCommand(Action<object>,Predicate<object>)"
        )
    );
    let rules = Some(Confidence::RulesBacked);
    assert_eq!(
        pairs,
        [
            (
                format!("{COMMANDS_NS}RelayCommand.CanExecute(object)"),
                Some(format!("{COMMANDS_NS}Panel.CanCheck()")),
                rules
            ),
            (
                format!("{COMMANDS_NS}RelayCommand.Execute(object)"),
                Some(format!("{COMMANDS_NS}Panel.Check()")),
                rules
            ),
        ]
    );
    assert!(binding.idle.is_empty(), "{:?}", binding.idle);
    assert!(binding.ambiguous.is_empty());
    assert_eq!(binding.grade(), rules);
    assert!(binding.slots.iter().all(|s| s.lambda.is_some()));
    let raises: Vec<(&str, &str)> = binding
        .raises
        .iter()
        .map(|r| (r.method.as_str(), r.field.as_str()))
        .collect();
    assert_eq!(
        raises,
        [(
            format!("{COMMANDS_NS}RelayCommand.Execute(object)").as_str(),
            format!("{COMMANDS_NS}RelayCommand.Executed").as_str()
        )]
    );
}

/// A constructor of one parameter, `: this(execute, null)`: `CanExecute`
/// calls a field this construction passes nothing to — no target, idle.
#[test]
fn the_constructor_of_one_parameter_leaves_can_execute_without_a_target() {
    let (generated, _) = commands();
    let ix = Index::new(&generated.graph, generated.bodies.as_ref().unwrap());
    let links = Links::new(&ix, DEFAULT_AMBIGUITY_CEILING);
    let (binding, pairs) = pairs_in(&links, "QuickCommand");
    assert_eq!(
        pairs,
        [(
            format!("{COMMANDS_NS}RelayCommand.Execute(object)"),
            Some(format!("{COMMANDS_NS}Panel.Check()")),
            Some(Confidence::RulesBacked)
        )]
    );
    let idle: Vec<(&str, &str)> = binding
        .idle
        .iter()
        .map(|i| (i.method.as_str(), i.field.as_str()))
        .collect();
    assert_eq!(
        idle,
        [(
            format!("{COMMANDS_NS}RelayCommand.CanExecute(object)").as_str(),
            format!("{COMMANDS_NS}RelayCommand._canExecute").as_str()
        )]
    );
    // `_canExecute` is written from a parameter of another constructor: its
    // call is no event.
    assert!(binding
        .raises
        .iter()
        .all(|r| r.field != key(COMMANDS_NS, "RelayCommand._canExecute")));
}

/// A field written from two arguments is ambiguous and bound to neither.
#[test]
fn a_field_written_from_two_arguments_is_ambiguous() {
    let (generated, _) = commands();
    let ix = Index::new(&generated.graph, generated.bodies.as_ref().unwrap());
    let links = Links::new(&ix, DEFAULT_AMBIGUITY_CEILING);
    let (binding, pairs) = pairs_in(&links, "Both");
    assert!(pairs.is_empty(), "{pairs:?}");
    assert_eq!(binding.ambiguous.len(), 1);
    assert_eq!(binding.ambiguous[0].field, key(COMMANDS_NS, "Twice._run"));
    assert_eq!(binding.ambiguous[0].positions, [0, 1]);
    assert_eq!(
        binding.ambiguous[0].invoked_by,
        [key(COMMANDS_NS, "Twice.Execute()")]
    );
    assert_eq!(binding.grade(), Some(Confidence::Ambiguous));
}

/// A method group is a delegate: `new RelayCommand(Check)` leads to the
/// `Check` of the argument count the class calls `_execute` with, among two
/// overloads — by the overload choice of U2.
#[test]
fn a_method_group_leads_to_the_overload_of_the_fields_call() {
    let (generated, _) = commands();
    let ix = Index::new(&generated.graph, generated.bodies.as_ref().unwrap());
    let links = Links::new(&ix, DEFAULT_AMBIGUITY_CEILING);
    assert_eq!(
        layer2::field_call_arity(
            &ix,
            &key(COMMANDS_NS, "RelayCommand"),
            &key(COMMANDS_NS, "RelayCommand._execute")
        ),
        Some(1)
    );
    let (binding, pairs) = pairs_in(&links, "GroupCommand");
    assert_eq!(
        pairs,
        [(
            format!("{COMMANDS_NS}RelayCommand.Execute(object)"),
            Some(format!("{COMMANDS_NS}Panel.Check(object)")),
            Some(Confidence::RulesBacked)
        )]
    );
    assert_eq!(binding.slots[0].targets[0].links[0].basis, Basis::Overload);
    assert_eq!(binding.slots[0].lambda, None);
}

/// The binding reads the shape, not the name: a class that is no command
/// binds its delegate the same way.
#[test]
fn a_class_not_named_command_with_the_shape_binds_the_same_way() {
    let (generated, _) = commands();
    let ix = Index::new(&generated.graph, generated.bodies.as_ref().unwrap());
    let links = Links::new(&ix, DEFAULT_AMBIGUITY_CEILING);
    let (binding, pairs) = pairs_in(&links, "Background");
    assert_eq!(binding.class, key(COMMANDS_NS, "Runner"));
    assert_eq!(
        pairs,
        [(
            format!("{COMMANDS_NS}Runner.Start()"),
            Some(format!("{COMMANDS_NS}Panel.Check()")),
            Some(Confidence::RulesBacked)
        )]
    );
    assert!(binding.raises.is_empty());
}

/// `Executed?.Invoke(this, null)` in `Execute` raises `Executed`; the calls
/// of fields written from a constructor's parameters raise nothing.
#[test]
fn a_call_of_an_event_no_constructor_writes_is_marked_as_raised() {
    let (generated, _) = commands();
    let ix = Index::new(&generated.graph, generated.bodies.as_ref().unwrap());
    let raised = raised_events(&ix, &key(COMMANDS_NS, "RelayCommand"));
    assert_eq!(raised.len(), 1, "{raised:?}");
    assert_eq!(
        raised[0].method,
        key(COMMANDS_NS, "RelayCommand.Execute(object)")
    );
    assert_eq!(raised[0].field, key(COMMANDS_NS, "RelayCommand.Executed"));
    assert_eq!(raised[0].place.path, "src/Commands.cs");
    assert!(raised_events(&ix, &key(COMMANDS_NS, "Runner")).is_empty());
}

/// The common command class stores its parameters behind a null guard
/// (`_execute = execute ?? throw …`): the fields are written from the
/// parameters, so the construction binds them and nothing is raised.
#[test]
fn a_parameter_stored_behind_a_null_guard_is_bound_and_raises_nothing() {
    let src = r#"namespace Acme.Guarded
{
    public class RelayCommand
    {
        private readonly Action<object> _execute;
        private readonly Predicate<object> _canExecute;
        public RelayCommand(Action<object> execute, Predicate<object> canExecute)
        {
            _execute = execute ?? throw new ArgumentNullException(nameof(execute));
            _canExecute = canExecute ?? throw new ArgumentNullException(nameof(canExecute));
        }
        public bool CanExecute(object parameter) { return _canExecute(parameter); }
        public void Execute(object parameter) { _execute(parameter); }
    }

    public class Panel
    {
        public RelayCommand CheckCommand => new RelayCommand(p => Check(), p => Ready());
        void Check() { }
        bool Ready() { return true; }
    }
}
"#;
    let ns = "sym:csharp:Acme.Guarded#";
    let (generated, _) = analyzed(&[("src/Guarded.cs", src)]);
    let ix = Index::new(&generated.graph, generated.bodies.as_ref().unwrap());
    let command = key(ns, "RelayCommand");
    assert_eq!(raised_events(&ix, &command), []);
    let links = Links::new(&ix, DEFAULT_AMBIGUITY_CEILING);
    let site = bindings_of(&links, &key(ns, "Panel"))
        .into_iter()
        .find(|s| s.member == key(ns, "Panel.CheckCommand"))
        .expect("the construction in CheckCommand");
    let binding = site.binding.expect("bound");
    assert!(binding.raises.is_empty(), "{:?}", binding.raises);
    let fields: Vec<(u32, &str)> = binding
        .slots
        .iter()
        .map(|s| (s.position, s.field.as_str()))
        .collect();
    assert_eq!(
        fields,
        [
            (0, format!("{ns}RelayCommand._execute").as_str()),
            (1, format!("{ns}RelayCommand._canExecute").as_str()),
        ]
    );
}

/// The entry point of 2a and the binding of 2b lead to the same target, by
/// one binding: every command of `Panel`, argument by argument.
#[test]
fn the_entry_point_and_the_binding_lead_to_the_same_target() {
    let (generated, analysis) = commands();
    let ix = Index::new(&generated.graph, generated.bodies.as_ref().unwrap());
    let links = Links::new(&ix, DEFAULT_AMBIGUITY_CEILING);
    let panel = type_of(&analysis, &key(COMMANDS_NS, "Panel"));
    for property in ["CheckCommand", "QuickCommand", "GroupCommand", "Background"] {
        let e = entry(panel, &format!("{COMMANDS_NS}Panel.{property}"));
        let from_2a: Vec<(u32, Vec<NodeKey>)> = e.commands[0]
            .delegates
            .iter()
            .map(|d| {
                (
                    d.position,
                    d.targets.iter().filter_map(|t| t.member.clone()).collect(),
                )
            })
            .collect();
        let (binding, _) = pairs_in(&links, property);
        let from_2b: Vec<(u32, Vec<NodeKey>)> = binding
            .slots
            .iter()
            .map(|s| {
                (
                    s.position,
                    s.targets
                        .iter()
                        .filter_map(|t| t.target().cloned())
                        .collect(),
                )
            })
            .collect();
        assert_eq!(from_2a, from_2b, "{property}");
        assert!(!from_2b.is_empty(), "{property}");
    }
}

// ---- The editor of the acceptance examples ----

#[path = "common/editor_fixture.rs"]
mod editor_fixture;

use editor_fixture::{MODELS_NS as EDITOR_MODELS, VIEWS_NS as EDITOR_VIEWS};

/// A run over the editor of the acceptance examples.
fn editor_2b() -> (Generated, Analysis) {
    analyzed(&editor_fixture::files())
}

/// The index of callers of a run, at the default ceiling.
fn callers_of(generated: &Generated) -> Callers {
    let ix = Index::new(&generated.graph, generated.bodies.as_ref().unwrap());
    let links = Links::new(&ix, DEFAULT_AMBIGUITY_CEILING);
    Callers::new(&generated.graph, &links)
}

// ---- U1: the reverse index of callers ----

/// The caller `member` of `callee`.
fn caller<'c>(callers: &'c Callers, callee: &NodeKey, member: &NodeKey) -> &'c Caller {
    callers
        .of(callee)
        .iter()
        .find(|c| &c.member == member)
        .unwrap_or_else(|| {
            panic!(
                "{member:?} does not call {callee:?}: {:?}",
                callers
                    .of(callee)
                    .iter()
                    .map(|c| c.member.as_str())
                    .collect::<Vec<_>>()
            )
        })
}

/// U1 verification: `Check(bool)` is called from both of its ways in — the
/// command's `Check()` and `ExternalCheck` — by rules; `Check()` by the
/// command class through the delegate binding; `CheckCommand`, reached only
/// as a link of `this.viewModel.CheckCommand.Execute`, by the view's key
/// handler; the handler subscribed with `+=` by the constructor; and the
/// subscription to `Executed` is counted on the command class.
#[test]
fn the_check_is_called_from_the_command_and_from_the_external_check() {
    let (generated, _) = editor_2b();
    let callers = callers_of(&generated);
    let check_bool = key(EDITOR_MODELS, "EditorViewModel.Check(bool)");
    for from in ["EditorViewModel.Check()", "ExternalCheck.Run()"] {
        let c = caller(&callers, &check_bool, &key(EDITOR_MODELS, from));
        assert_eq!(c.grade, Confidence::RulesBacked, "{from}");
        assert!(
            c.links().any(|s| s.last && s.call == Some(1)),
            "{from}: {c:?}"
        );
    }
    assert_eq!(callers.class_of(&check_bool), CallerClass::Strong);

    let check = key(EDITOR_MODELS, "EditorViewModel.Check()");
    let execute = caller(
        &callers,
        &check,
        &key(EDITOR_MODELS, "RelayCommand.Execute(object)"),
    );
    assert!(
        execute.evidence.iter().any(|e| matches!(
            &e.via,
            Via::Delegate { built_in, .. } if built_in == &key(EDITOR_MODELS, "EditorViewModel.CheckCommand")
        ) && e.grade == Confidence::RulesBacked),
        "{execute:?}"
    );
    // The lambda the command is built from names `Check()` too.
    let lambda = caller(
        &callers,
        &check,
        &key(EDITOR_MODELS, "EditorViewModel.CheckCommand"),
    );
    assert!(lambda.links().any(|s| s.last), "{lambda:?}");

    // A member reached only through a property access is not uncalled: the
    // link it is in `this.viewModel.CheckCommand.Execute` makes the caller.
    let command = key(EDITOR_MODELS, "EditorViewModel.CheckCommand");
    let on_key = key(
        EDITOR_VIEWS,
        "EditorView.Editor_PreviewKeyDown(object,KeyEventArgs)",
    );
    let c = caller(&callers, &command, &on_key);
    assert!(c.is_strong(), "{c:?}");
    assert!(c.links().any(|s| !s.last), "{c:?}");

    // A handler is called by its subscription.
    let on_checked = key(EDITOR_VIEWS, "EditorView.OnChecked(object,EventArgs)");
    let ctor = key(EDITOR_VIEWS, "EditorView.EditorView()");
    let c = caller(&callers, &on_checked, &ctor);
    assert!(
        c.evidence
            .iter()
            .any(|e| matches!(&e.via, Via::Subscription { event, .. } if event == "Executed")),
        "{c:?}"
    );
    assert_eq!(callers.class_of(&on_checked), CallerClass::Strong);
    let subscribers = callers.subscribers_of(&key(EDITOR_MODELS, "RelayCommand.Executed"));
    assert_eq!(subscribers.len(), 1, "{subscribers:?}");
    assert_eq!(subscribers[0].member, ctor);
    assert_eq!(subscribers[0].handler.as_ref(), Some(&on_checked));
}

/// A first-layer relation, rated by name resolution.
fn rated(confidence: Confidence, rule: Rule) -> Edge {
    Edge::new(
        EdgeKind::Calls,
        Provenance::new(
            "src/ViewModels/EditorViewModel.cs",
            Some(1),
            ExtractorId::from("languages"),
            Tier::Measured,
        )
        .rated(confidence, rule),
    )
}

/// Nothing calls a member: none. Called by a `NameOnly` relation alone: weak
/// only. One `RulesBacked` relation beside it: strong.
#[test]
fn a_member_called_by_name_alone_is_weak_and_one_rule_backed_caller_makes_it_strong() {
    let (mut generated, _) = editor_2b();
    let refresh = key(EDITOR_MODELS, "EditorViewModel.Refresh()");
    assert_eq!(
        callers_of(&generated).class_of(&refresh),
        CallerClass::Uncalled
    );
    assert!(callers_of(&generated).of(&refresh).is_empty());

    let run = key(EDITOR_MODELS, "ExternalCheck.Run()");
    assert!(generated
        .graph
        .add_edge(&run, &refresh, rated(Confidence::NameOnly, Rule::NameMatch)));
    let callers = callers_of(&generated);
    assert_eq!(callers.class_of(&refresh), CallerClass::WeakOnly);
    assert_eq!(caller(&callers, &refresh, &run).grade, Confidence::NameOnly);

    let save = key(EDITOR_VIEWS, "EditorView.Save()");
    assert!(generated.graph.add_edge(
        &save,
        &refresh,
        rated(Confidence::RulesBacked, Rule::DeclaredReceiver)
    ));
    let callers = callers_of(&generated);
    assert_eq!(callers.class_of(&refresh), CallerClass::Strong);
    let members: Vec<&NodeKey> = callers.of(&refresh).iter().map(|c| &c.member).collect();
    assert_eq!(members, [&run, &save]);
}

/// A name the first layer matched past the ceiling (`Ping: 10 candidates`)
/// gives every candidate a weak caller — never a member nothing calls, never
/// a strong one; past the ceiling of shown candidates too. The caller is read
/// through the candidate's name, not kept as a caller of each candidate.
#[test]
fn a_place_over_the_ceiling_makes_its_candidates_weakly_called_not_roots() {
    let (generated, _) = analyzed(&[("src/Many.cs", MANY)]);
    let callers = callers_of(&generated);
    let go = key(MANY_NS, "Caller.Go()");
    for i in 0..10 {
        let ping = key(MANY_NS, &format!("A{i}.Ping()"));
        assert_eq!(callers.class_of(&ping), CallerClass::WeakOnly, "{ping:?}");
        let weak: Vec<&NodeKey> = callers.over_ceiling_of(&ping).collect();
        assert_eq!(weak, [&go], "{ping:?}");
        assert!(callers.of(&ping).is_empty(), "{:?}", callers.of(&ping));
    }
    assert!(callers.over_ceiling_of(&go).next().is_none());
}

/// Many members of one name past the ceiling, each referenced from many
/// members: every candidate is weakly called by every referencing member,
/// and the index holds one entry per referencing member — never one per
/// candidate and referencing member. The walk reads them as before: the
/// candidate is a root only weak callers reach, its row unresolved with every
/// one of them.
#[test]
fn many_homonyms_over_the_ceiling_take_one_entry_per_referencing_member() {
    const N: usize = 20;
    let mut src = String::from(
        "namespace Acme.Fan\n{\n    public class Hub { public static object Lookup() { return null; } }\n",
    );
    for i in 0..N {
        src.push_str(&format!(
            "    public class H{i} {{ int runs; public void Execute() {{ runs++; }} }}\n"
        ));
    }
    for j in 0..N {
        src.push_str(&format!(
            "    public class R{j} {{ void Go() {{ Hub.Lookup().Execute(); }} }}\n"
        ));
    }
    src.push_str("}\n");
    let (generated, analysis) = analyzed(&[("src/Fan.cs", &src)]);
    let callers = callers_of(&generated);
    let ns = "sym:csharp:Acme.Fan#";
    let mut goes: Vec<NodeKey> = (0..N).map(|j| key(ns, &format!("R{j}.Go()"))).collect();
    goes.sort();
    for i in 0..N {
        let execute = key(ns, &format!("H{i}.Execute()"));
        assert_eq!(
            callers.class_of(&execute),
            CallerClass::WeakOnly,
            "{execute:?}"
        );
        let weak: Vec<&NodeKey> = callers.over_ceiling_of(&execute).collect();
        assert_eq!(weak, goes.iter().collect::<Vec<_>>(), "{execute:?}");
        assert!(callers.of(&execute).is_empty(), "{execute:?}");

        let ty = key(ns, &format!("H{i}"));
        let f = holding(&analysis, &ty, &execute);
        let (_, row) = row_to(&analysis, &ty, f, TriggerKind::External, &execute);
        assert_eq!(
            row.state,
            PathState::Unresolved {
                callers: goes.clone().into()
            },
            "{execute:?}"
        );
    }
    assert_eq!(callers.over_ceiling_entries(), N);
}

/// An access that stopped at two overloads of one arity makes each a weak
/// caller's candidate — shown, not followed.
#[test]
fn an_access_stopped_at_two_overloads_makes_both_weakly_called() {
    let (generated, _) = editor();
    let callers = callers_of(&generated);
    let on_key = key(VIEWS_NS, "EditorView.OnKey(KeyEventArgs)");
    for overload in ["EditorViewModel.Do(int)", "EditorViewModel.Do(string)"] {
        let member = key(MODELS_NS, overload);
        assert_eq!(
            callers.class_of(&member),
            CallerClass::WeakOnly,
            "{overload}"
        );
        let c = caller(&callers, &member, &on_key);
        assert!(
            c.evidence.iter().any(
                |e| matches!(&e.via, Via::Candidate(s) if s.last && s.call == Some(1))
                    && e.grade == Confidence::Ambiguous
            ),
            "{overload}: {c:?}"
        );
    }
}

/// Two builds of the index over one graph — and over a second run of the
/// same repository — are equal, the members and each member's callers in key
/// order.
#[test]
fn two_builds_of_the_index_are_equal_and_in_key_order() {
    let (generated, _) = editor_2b();
    let ix = Index::new(&generated.graph, generated.bodies.as_ref().unwrap());
    let links = Links::new(&ix, DEFAULT_AMBIGUITY_CEILING);
    let first = Callers::new(&generated.graph, &links);
    assert_eq!(first, Callers::new(&generated.graph, &links));
    let (again, _) = editor_2b();
    assert_eq!(first, callers_of(&again));

    let members: Vec<&NodeKey> = first.members().collect();
    assert!(members.windows(2).all(|w| w[0] < w[1]), "{members:?}");
    for m in &members {
        let of: Vec<&NodeKey> = first.of(m).iter().map(|c| &c.member).collect();
        assert!(of.windows(2).all(|w| w[0] < w[1]), "{m:?}: {of:?}");
    }
}

// ---- U4: results and the state from outside ----

/// The results of the type `ty` in a run.
fn type_results(generated: &Generated, analysis: &Analysis, ty: &NodeKey) -> TypeResults {
    let ix = Index::new(&generated.graph, generated.bodies.as_ref().unwrap());
    let links = Links::new(&ix, DEFAULT_AMBIGUITY_CEILING);
    let callers = Callers::new(&generated.graph, &links);
    FlowResults::new(&generated.graph, &links, &callers).of_type(type_of(analysis, ty))
}

/// The member part of a key (`EditorViewModel.Errors`).
fn short(k: &NodeKey) -> &str {
    k.as_str().rsplit_once('#').map_or(k.as_str(), |(_, m)| m)
}

fn value(v: &Option<Constant>) -> String {
    match v {
        Some(Constant::Boolean(b)) => format!(" = {b}"),
        Some(Constant::Number(n)) => format!(" = {n}"),
        Some(Constant::Null) => " = null".to_string(),
        None => String::new(),
    }
}

/// A result as a line: its kind, what it is of, the value written.
fn describe(r: &FlowResult) -> String {
    match &r.kind {
        ResultKind::NotifiesChange { target, value: v } => {
            format!("notifies {}{}", short(target), value(v))
        }
        ResultKind::RaisesEvent { event, subscribers } => {
            format!("raises {} ({subscribers})", short(event))
        }
        ResultKind::ReadElsewhere {
            target,
            value: v,
            mutation,
            ..
        } => format!(
            "read elsewhere {}{}{}",
            short(target),
            value(v),
            if *mutation { " (changed)" } else { "" }
        ),
        ResultKind::Database { object, .. } => format!("database {}", object.as_str()),
        ResultKind::Throws => "throws".to_string(),
        ResultKind::Returns => "returns".to_string(),
    }
}

/// The results of `member`'s code, of the functionality that holds it.
fn results_of<'r>(
    results: &'r TypeResults,
    analysis: &Analysis,
    ty: &NodeKey,
    member: &NodeKey,
) -> Vec<&'r FlowResult> {
    let t = type_of(analysis, ty);
    let f = t
        .functionalities
        .iter()
        .find(|f| f.members.contains(member))
        .unwrap_or_else(|| panic!("no functionality holds {member:?}"));
    results
        .of(&f.key)
        .unwrap_or_else(|| panic!("no results of {}", f.key))
        .of_member(member)
}

fn described(results: &[&FlowResult]) -> Vec<String> {
    results.iter().map(|r| describe(r)).collect()
}

/// AE10: the results of `Check(bool)` are the writes of the properties that
/// notify their change, then `CheckWasDone = false` at the start and `= true`
/// at the end — read by `ExternalCheck`, not by the save, which clears the
/// state itself before it reads it. Each of the three silent returns is a
/// stopped path on which `CheckWasDone = false` still happens. The context of
/// the state names `ExternalCheck` as its reader and the save as a writer.
#[test]
fn the_results_of_the_check_and_the_state_read_by_the_external_check() {
    let (generated, analysis) = editor_2b();
    let vm = key(EDITOR_MODELS, "EditorViewModel");
    let results = type_results(&generated, &analysis, &vm);
    let check_bool = key(EDITOR_MODELS, "EditorViewModel.Check(bool)");
    let of_check = results_of(&results, &analysis, &vm, &check_bool);
    assert_eq!(
        described(&of_check),
        [
            "notifies EditorViewModel.Errors",
            "notifies EditorViewModel.ShowErrors",
            "notifies EditorViewModel.MessageErrors",
            "notifies EditorViewModel.SelectedIndex = 0",
            "read elsewhere EditorViewModel.CheckWasDone = false",
            "read elsewhere EditorViewModel.CheckWasDone = true",
        ]
    );
    let run = key(EDITOR_MODELS, "ExternalCheck.Run()");
    let before_save = key(EDITOR_VIEWS, "EditorView.BeforeSave()");
    for r in &of_check[4..] {
        let ResultKind::ReadElsewhere { readers, .. } = &r.kind else {
            unreachable!()
        };
        let who: Vec<&NodeKey> = readers.iter().map(|t| &t.member).collect();
        assert_eq!(who, [&run], "{r:?}");
    }

    // The three silent returns of `Check(bool)`, each with the write before
    // it.
    let f = type_of(&analysis, &vm)
        .functionalities
        .iter()
        .find(|f| f.members.contains(&check_bool))
        .unwrap();
    let stops: Vec<&StoppedPath> = results
        .of(&f.key)
        .unwrap()
        .stops
        .iter()
        .filter(|s| s.member == check_bool)
        .collect();
    let texts: Vec<&str> = stops
        .iter()
        .map(|s| match &s.cause {
            StopCause::Precondition { text, .. } => text.as_str(),
            StopCause::Dead(_) => "dead",
        })
        .collect();
    assert_eq!(
        texts,
        [
            "!canErrors",
            "syntax != SyntaxKind.Sql && syntax != SyntaxKind.Script",
            "Text == null"
        ]
    );
    for s in &stops {
        let on_it: Vec<&FlowResult> = s.results.iter().collect();
        assert_eq!(
            described(&on_it),
            ["read elsewhere EditorViewModel.CheckWasDone = false"],
            "{s:?}"
        );
    }

    let state = results
        .state(&key(EDITOR_MODELS, "EditorViewModel.CheckWasDone"))
        .expect("CheckWasDone is a state");
    let readers: Vec<&NodeKey> = state.reads.iter().map(|t| &t.member).collect();
    assert_eq!(readers, [&run]);
    let writers: Vec<&NodeKey> = state.writes.iter().map(|t| &t.member).collect();
    assert_eq!(writers, [&before_save]);
}

/// AE9: `Executed?.Invoke(…)` in the command class's `Execute` is the result
/// "raises `Executed`" with the one subscription the corpus makes to it. The
/// event raised before the class's own guard `evt.Cancel` still happens on
/// the path that guard stops; the one after it does not.
#[test]
fn an_event_the_command_class_raises_is_a_result_with_its_subscribers() {
    let (generated, analysis) = editor_2b();
    let command = key(EDITOR_MODELS, "RelayCommand");
    let results = type_results(&generated, &analysis, &command);
    let execute = key(EDITOR_MODELS, "RelayCommand.Execute(object)");
    assert_eq!(
        described(&results_of(&results, &analysis, &command, &execute)),
        [
            "raises RelayCommand.ExecutingStarted (0)",
            "raises RelayCommand.Executed (1)"
        ]
    );
    let f = type_of(&analysis, &command)
        .functionalities
        .iter()
        .find(|f| f.members.contains(&execute))
        .unwrap();
    let cancel = results
        .of(&f.key)
        .unwrap()
        .stops
        .iter()
        .find(|s| matches!(&s.cause, StopCause::Precondition { text, .. } if text == "evt.Cancel"))
        .expect("the cancel guard stops a path");
    let on_it: Vec<&FlowResult> = cancel.results.iter().collect();
    assert_eq!(
        described(&on_it),
        ["raises RelayCommand.ExecutingStarted (0)"]
    );
}

/// Kinds 3 and 4: the save's database object, and the value the external
/// check returns to its caller.
#[test]
fn the_database_and_a_returned_value_are_results() {
    let (generated, analysis) = editor_2b();
    let view = key(EDITOR_VIEWS, "EditorView");
    let results = type_results(&generated, &analysis, &view);
    let store = key(EDITOR_VIEWS, "EditorView.Store()");
    let of_store = results_of(&results, &analysis, &view, &store);
    assert_eq!(described(&of_store), ["database res:db:dbo.Documents"]);
    assert!(of_store[0].place.is_none());

    let external = key(EDITOR_MODELS, "ExternalCheck");
    let results = type_results(&generated, &analysis, &external);
    let run = key(EDITOR_MODELS, "ExternalCheck.Run()");
    assert_eq!(
        described(&results_of(&results, &analysis, &external, &run)),
        ["returns"]
    );
}

/// The negative cases of R6 and KTD6, each beside the case that is a result.
const EFFECTS: &str = r#"namespace Acme.Effects
{
    public class Args { public string Text; }

    public class Sink
    {
        public void Take(Args a) { Keep(a.Text); }
        void Keep(string t) { }
    }

    public class Cache { }

    public class Worker
    {
        private Cache data;
        public Cache Data { get { if (data == null) data = new Cache(); return data; } }
        public void Send(Sink s) { var a = new Args(); a.Text = "x"; s.Take(a); }
        public void Mark(Args a) { a.Text = "y"; }
        public void Reset() { data = null; }
        public void Fail() { throw new InvalidOperationException(); }
        public void Safe() { try { throw new InvalidOperationException(); } catch (InvalidOperationException) { } }
        public void Guarded(string s) { if (s == null) throw new ArgumentNullException(); var n = s.Length; }
    }

    public class Reader
    {
        public void Show(Worker w) { Use(w.Data); }
        void Use(Cache c) { }
    }

    public class Flag { public bool On; public int Value; }

    public class Writer
    {
        public void Set(Flag f) { f.Value = 1; }
    }

    public class Branchy
    {
        public void Show(Flag f) { if (f.On) { f.Value = 2; } else { Use(f.Value); } }
        public void Fresh(Flag f) { f.Value = 3; Use(f.Value); }
        void Use(int v) { }
    }
}
"#;

const EFFECTS_NS: &str = "sym:csharp:Acme.Effects#";

fn effects() -> (Generated, Analysis) {
    analyzed(&[("src/Effects.cs", EFFECTS)])
}

/// The results of `member` of the type `ty` of the effects fixture.
fn effects_of(generated: &Generated, analysis: &Analysis, ty: &str, member: &str) -> Vec<String> {
    let ty = key(EFFECTS_NS, ty);
    let results = type_results(generated, analysis, &ty);
    described(&results_of(
        &results,
        analysis,
        &ty,
        &key(EFFECTS_NS, member),
    ))
}

/// A write into an object the functionality constructed itself and hands on
/// is no result; the same write into an object it was given is one.
#[test]
fn a_write_into_an_object_the_functionality_built_is_no_result() {
    let (generated, analysis) = effects();
    assert!(effects_of(&generated, &analysis, "Worker", "Worker.Send(Sink)").is_empty());
    assert_eq!(
        effects_of(&generated, &analysis, "Worker", "Worker.Mark(Args)"),
        ["read elsewhere Args.Text"]
    );
}

/// Resetting a field tagged `lazy-init` is no result, though the getter that
/// stands for it is read elsewhere.
#[test]
fn resetting_a_lazy_init_field_is_no_result() {
    let (generated, analysis) = effects();
    let callers = callers_of(&generated);
    let data = key(EFFECTS_NS, "Worker.Data");
    assert!(callers
        .of(&data)
        .iter()
        .any(|c| c.member == key(EFFECTS_NS, "Reader.Show(Worker)")));
    assert!(effects_of(&generated, &analysis, "Worker", "Worker.Reset()").is_empty());
}

/// A `throw` outside a `try` is a result of kind 4; one inside a `try` with
/// a `catch` of the same member is not.
#[test]
fn a_throw_outside_a_try_is_a_result_and_one_caught_is_not() {
    let (generated, analysis) = effects();
    assert_eq!(
        effects_of(&generated, &analysis, "Worker", "Worker.Fail()"),
        ["throws"]
    );
    assert!(effects_of(&generated, &analysis, "Worker", "Worker.Safe()").is_empty());

    // A guard's own `throw` is what happens on the path it stops.
    let worker = key(EFFECTS_NS, "Worker");
    let guarded = key(EFFECTS_NS, "Worker.Guarded(string)");
    let results = type_results(&generated, &analysis, &worker);
    assert_eq!(
        described(&results_of(&results, &analysis, &worker, &guarded)),
        ["throws"]
    );
    let f = type_of(&analysis, &worker)
        .functionalities
        .iter()
        .find(|f| f.members.contains(&guarded))
        .unwrap();
    let stop = results
        .of(&f.key)
        .unwrap()
        .stops
        .iter()
        .find(|s| s.member == guarded)
        .expect("the guard stops a path");
    let on_it: Vec<&FlowResult> = stop.results.iter().collect();
    assert_eq!(described(&on_it), ["throws"]);
}

/// A read in one branch of an `if`, after a write only in the other, counts;
/// a read after an unconditional write at the top level does not.
#[test]
fn a_read_after_a_write_in_another_branch_counts() {
    let (generated, analysis) = effects();
    let ty = key(EFFECTS_NS, "Writer");
    let results = type_results(&generated, &analysis, &ty);
    let set = results_of(
        &results,
        &analysis,
        &ty,
        &key(EFFECTS_NS, "Writer.Set(Flag)"),
    );
    assert_eq!(described(&set), ["read elsewhere Flag.Value = 1"]);
    let ResultKind::ReadElsewhere { readers, .. } = &set[0].kind else {
        unreachable!()
    };
    let who: Vec<&NodeKey> = readers.iter().map(|t| &t.member).collect();
    assert_eq!(who, [&key(EFFECTS_NS, "Branchy.Show(Flag)")]);
}

// ---- U5: triggers, the walk back and the flows ----

/// The functionality of `ty` that holds `member`.
fn holding<'a>(analysis: &'a Analysis, ty: &NodeKey, member: &NodeKey) -> &'a Functionality {
    type_of(analysis, ty)
        .functionalities
        .iter()
        .find(|f| f.members.contains(member))
        .unwrap_or_else(|| panic!("no functionality of {ty:?} holds {member:?}"))
}

fn target_of(ty: &NodeKey, f: &Functionality) -> FunctionalityRef {
    FunctionalityRef {
        ty: ty.clone(),
        key: f.key.clone(),
    }
}

/// The rows of the registry that lead to `f` of `ty`, with their indices.
fn rows_to<'a>(
    analysis: &'a Analysis,
    ty: &NodeKey,
    f: &Functionality,
) -> Vec<(usize, &'a TriggerRow)> {
    let target = target_of(ty, f);
    analysis
        .triggers
        .iter()
        .enumerate()
        .filter(|(_, r)| r.target == target)
        .collect()
}

/// The one row to `f` of `ty` whose trigger is of `kind` and written in
/// `member`.
fn row_to<'a>(
    analysis: &'a Analysis,
    ty: &NodeKey,
    f: &Functionality,
    kind: TriggerKind,
    member: &NodeKey,
) -> (usize, &'a TriggerRow) {
    let rows = rows_to(analysis, ty, f);
    let found: Vec<(usize, &TriggerRow)> = rows
        .iter()
        .copied()
        .filter(|(_, r)| r.trigger.kind == kind && &r.trigger.member == member)
        .collect();
    assert_eq!(
        found.len(),
        1,
        "{kind:?} of {member:?} to {}: {:?}",
        f.key,
        rows.iter()
            .map(|(_, r)| (r.trigger.kind, r.trigger.member.as_str()))
            .collect::<Vec<_>>()
    );
    found[0]
}

fn context_of<'a>(
    analysis: &'a Analysis,
    ty: &NodeKey,
    f: &Functionality,
) -> &'a FunctionalityContext {
    type_of(analysis, ty)
        .context
        .of(&f.key)
        .unwrap_or_else(|| panic!("no context of {}", f.key))
}

/// The flow of the row `row` at `context`, and the part it shares.
fn flow_of(context: &FunctionalityContext, row: usize) -> (&TriggerFlow, &CommonPart) {
    let flow = context
        .flows
        .iter()
        .find(|f| f.row == row)
        .unwrap_or_else(|| panic!("no flow of row {row} at {}", context.key));
    (flow, &context.common[flow.common])
}

fn line_in(src: &str, needle: &str) -> u32 {
    src.lines()
        .position(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("{needle} is not in the source")) as u32
        + 1
}

fn owner(k: &NodeKey) -> &str {
    k.as_str()
        .rsplit_once('#')
        .map_or(k.as_str(), |(_, m)| m)
        .split('.')
        .next()
        .unwrap_or_default()
}

/// AE1: the flow of F4 in the old editor, at the check of `EditorViewModel`:
/// the key branch's condition as the trigger's precondition, the one step of
/// the view that executes the command, the CanExecute of the command, the
/// command class as one step `Execute → Check()`, the branch on Shift of the
/// argument `Check()` passes, `Parser.Parse()` as one step with its card, and
/// the results of AE10 — every line with its anchor.
#[test]
fn the_f4_flow_of_the_old_editor_goes_through_the_command_to_the_results() {
    let (_, analysis) = editor_2b();
    let vm = key(EDITOR_MODELS, "EditorViewModel");
    let check_bool = key(EDITOR_MODELS, "EditorViewModel.Check(bool)");
    let f = holding(&analysis, &vm, &check_bool);
    let on_key = key(
        EDITOR_VIEWS,
        "EditorView.Editor_PreviewKeyDown(object,KeyEventArgs)",
    );
    let (i, row) = row_to(&analysis, &vm, f, TriggerKind::Key, &on_key);
    assert_eq!(row.trigger.key, "Key.F4");
    assert_eq!(row.state, PathState::Active);
    assert_eq!(row.grade, Some(Confidence::RulesBacked));
    assert_eq!(
        names(&row.entries),
        [format!("{EDITOR_MODELS}EditorViewModel.CheckCommand").as_str()]
    );
    assert_eq!(
        row.trigger.place.as_ref().unwrap().path,
        "src/Views/EditorView.cs"
    );

    let context = context_of(&analysis, &vm, f);
    let (flow, common) = flow_of(context, i);
    let condition = flow.condition.as_ref().expect("the key branch's condition");
    assert_eq!(
        condition.text,
        "e.Key == Key.F4 && (viewModel.ShowErrors || viewModel.CanErrors)"
    );
    // The view's one step: the call that executes the command.
    assert_eq!(flow.path.len(), 1, "{:?}", flow.path);
    let through = &flow.path[0];
    assert_eq!(through.member, on_key);
    assert_eq!(through.kind, FlowStepKind::Through);
    let place = through.place.as_ref().unwrap();
    assert_eq!(place.path, "src/Views/EditorView.cs");
    assert_eq!(
        place.range.start.line,
        line_in(
            editor_fixture::VIEW,
            "this.viewModel.CheckCommand.Execute(null)"
        )
    );

    // CanExecute of the command, and the check's own guards.
    let can_execute: Vec<&str> = common
        .preconditions
        .iter()
        .filter(|p| p.occurrence.source == Source::CanExecute)
        .map(|p| p.text.as_str())
        .collect();
    assert_eq!(
        can_execute,
        [
            "syntax != SyntaxKind.Sql && syntax != SyntaxKind.Script",
            "!canErrors"
        ]
    );
    assert!(common
        .preconditions
        .iter()
        .any(|p| p.occurrence.source == Source::Guard && p.occurrence.member == check_bool));

    // The command class is one step: Execute → Check().
    let check = key(EDITOR_MODELS, "EditorViewModel.Check()");
    let execute = key(EDITOR_MODELS, "RelayCommand.Execute(object)");
    assert!(
        common.steps.iter().any(|s| matches!(&s.kind,
            FlowStepKind::Delegate { invoker, targets, can_execute: false, .. }
                if invoker == &execute && targets == std::slice::from_ref(&check))),
        "{:?}",
        common.steps
    );
    // The branch on Shift, at the call of `Check()`.
    let shift = common
        .steps
        .iter()
        .find(|s| matches!(s.kind, FlowStepKind::ArgumentChoice { .. }))
        .expect("the argument chooses the mode");
    let FlowStepKind::ArgumentChoice {
        callee,
        parameter,
        enum_refs,
        ..
    } = &shift.kind
    else {
        unreachable!()
    };
    assert_eq!(shift.member, check);
    assert_eq!(callee, &check_bool);
    assert_eq!(parameter.as_deref(), Some("syntaxOnly"));
    assert_eq!(enum_refs, &["ModifierKeys.Shift".to_string()]);
    assert_eq!(
        shift.place.as_ref().unwrap().range.start.line,
        line_in(editor_fixture::VIEW_MODEL, "public void Check() {")
    );
    // The parser is one step, with its card.
    let parse = key(PARSING, "Parser.Parse(string,bool)");
    assert!(
        common.steps.iter().any(|s| s.member == check_bool
            && matches!(&s.kind,
                FlowStepKind::Call { target, owner, grade: Confidence::RulesBacked }
                    if target == &parse && owner == &key(PARSING, "Parser"))),
        "{:?}",
        common.steps
    );
    // The results of AE10.
    let results: Vec<&FlowResult> = common.results.iter().collect();
    assert_eq!(
        described(&results),
        [
            "notifies EditorViewModel.Errors",
            "notifies EditorViewModel.ShowErrors",
            "notifies EditorViewModel.MessageErrors",
            "notifies EditorViewModel.SelectedIndex = 0",
            "read elsewhere EditorViewModel.CheckWasDone = false",
            "read elsewhere EditorViewModel.CheckWasDone = true",
        ]
    );
    // Every step and result has its anchor.
    assert!(common.steps.iter().all(|s| s.place.is_some()));
    assert!(common.results.iter().all(|r| r.place.is_some()));
    // The command class's own guard is not repeated in the flow (AE9).
    assert!(common
        .preconditions
        .iter()
        .all(|p| owner(&p.occurrence.member) != "RelayCommand"));
}

const PARSING: &str = editor_fixture::PARSING_NS;

/// AE2: F4 in the new editor ends in "nothing happens" at `CanCheck()`, which
/// returns `false`; its row is dead with the same anchor, and so is the row of
/// the view's own key branch, which refers to it.
#[test]
fn the_f4_flow_of_the_new_editor_is_dead_at_can_check() {
    let (_, analysis) = editor_2b();
    let vm_new = key(EDITOR_MODELS, "EditorViewModelNew");
    let command = key(EDITOR_MODELS, "EditorViewModelNew.CheckCommand");
    let f = holding(&analysis, &vm_new, &command);
    let on_key = key(
        EDITOR_VIEWS,
        "EditorViewNew.Editor_PreviewKeyDown(object,KeyEventArgs)",
    );
    let (i, row) = row_to(&analysis, &vm_new, f, TriggerKind::Key, &on_key);
    let can_check = key(EDITOR_MODELS, "EditorViewModelNew.CanCheck()");
    let PathState::Dead { member, place } = &row.state else {
        panic!("{:?}", row.state)
    };
    assert_eq!(member, &can_check);
    assert_eq!(place.path, "src/ViewModels/EditorViewModelNew.cs");
    assert_eq!(
        place.range.start.line,
        line_in(editor_fixture::VIEW_MODEL_NEW, "bool CanCheck()")
    );
    let (_, common) = flow_of(context_of(&analysis, &vm_new, f), i);
    assert_eq!(
        common.dead.as_ref(),
        Some(&(can_check.clone(), place.clone()))
    );
    assert!(common
        .stops
        .iter()
        .any(|s| s.member == can_check && matches!(s.cause, StopCause::Dead(_))));

    // The view's own key branch refers to the view model's flow, and is dead
    // with it.
    let view_new = key(EDITOR_VIEWS, "EditorViewNew");
    let own = type_of(&analysis, &view_new)
        .functionalities
        .iter()
        .find(|f| f.entry_points.iter().any(|e| e.member == on_key))
        .unwrap();
    let (j, own_row) = row_to(&analysis, &view_new, own, TriggerKind::Key, &on_key);
    assert!(own_row.own);
    assert_eq!(own_row.state, row.state);
    let context = context_of(&analysis, &view_new, own);
    assert!(context.flows.iter().all(|fl| fl.row != j));
    assert_eq!(context.refers, [target_of(&vm_new, f)]);
}

/// AE3 and the stop condition of the plan: F4 of `EditorView` and F4 of
/// `EditorViewNew` are two rows, and no flow of F4 ever leads to both view
/// models — the walk never goes back through the command class's `Execute`,
/// which every command shares.
#[test]
fn the_f4_flow_never_leads_to_both_view_models() {
    let (_, analysis) = editor_2b();
    let old = key(
        EDITOR_VIEWS,
        "EditorView.Editor_PreviewKeyDown(object,KeyEventArgs)",
    );
    let new = key(
        EDITOR_VIEWS,
        "EditorViewNew.Editor_PreviewKeyDown(object,KeyEventArgs)",
    );
    let vm = key(EDITOR_MODELS, "EditorViewModel");
    let vm_new = key(EDITOR_MODELS, "EditorViewModelNew");
    let in_type = |m: &NodeKey, ty: &str| m.as_str().contains(&format!("#{ty}."));
    for r in &analysis.triggers {
        let from_old =
            r.trigger.member == old || r.path.iter().any(|h| in_type(&h.member, "EditorView"));
        let from_new =
            r.trigger.member == new || r.path.iter().any(|h| in_type(&h.member, "EditorViewNew"));
        if r.target.ty == vm {
            assert!(!from_new, "{r:?}");
            assert!(
                r.path
                    .iter()
                    .all(|h| !in_type(&h.member, "EditorViewModelNew")),
                "{r:?}"
            );
        }
        if r.target.ty == vm_new {
            assert!(!from_old, "{r:?}");
            assert!(
                r.path
                    .iter()
                    .all(|h| !in_type(&h.member, "EditorViewModel")),
                "{r:?}"
            );
        }
    }
    // Two F4 rows of the registry: one of each view, to its own view model.
    let f4: Vec<(&NodeKey, &NodeKey)> = analysis
        .triggers
        .iter()
        .filter(|r| {
            r.trigger.kind == TriggerKind::Key
                && r.trigger.key == "Key.F4"
                && (r.target.ty == vm || r.target.ty == vm_new)
        })
        .map(|r| (&r.trigger.member, &r.target.ty))
        .collect();
    assert_eq!(f4, [(&old, &vm), (&new, &vm_new)]);
}

/// A command class for the fixtures below, as the editor's.
const RELAY: &str = r#"
    public class RelayCommand
    {
        private readonly Action<object> _execute;
        private readonly Predicate<object> _canExecute;
        public KeyGesture[] KeyGestures { get; set; }
        public RelayCommand(Action<object> execute) : this(execute, null) { }
        public RelayCommand(Action<object> execute, Predicate<object> canExecute)
        {
            _execute = execute;
            _canExecute = canExecute;
        }
        public bool CanExecute(object parameter) { return _canExecute == null || _canExecute(parameter); }
        public void Execute(object parameter) { _execute(parameter); }
    }
"#;

/// A view whose F4 runs a command that can never run, whose target calls into
/// the core functionality of another analyzed type.
const CHAIN_VIEWS: &str = r#"namespace Acme.Chain
{
    public class ChainView
    {
        private ChainViewModel viewModel;

        public ChainView()
        {
            PreviewKeyDown += OnKey;
        }

        void OnKey(object sender, KeyEventArgs e)
        {
            if (e.Key == Key.F4)
            {
                this.viewModel.CheckCommand.Execute(null);
            }
        }
    }
}
"#;

const CHAIN_MODELS: &str = r#"namespace Acme.Chain
{
    public class ChainViewModel
    {
        private RelayCommand checkCommand;
        private Ledger ledger;

        public RelayCommand CheckCommand
        {
            get
            {
                if (checkCommand == null) checkCommand = new RelayCommand(p => Check(), p => CanCheck());
                return checkCommand;
            }
        }

        public void Check() { ledger.Validate(); }

        bool CanCheck() { return false; }
    }

    public class Ledger
    {
        private int validated;

        public void Validate() { validated++; }
    }
"#;

const CHAIN_NS: &str = "sym:csharp:Acme.Chain#";

/// R12: F4 runs a command whose CanExecute is `false`; its target calls the
/// ledger's validation. The row of F4 to the ledger passes through the dead
/// command, so it is dead at `CanCheck()` like the row to the view model,
/// and the view's own row, which refers to both, is dead too.
#[test]
fn a_row_past_a_dead_command_is_dead_and_so_is_the_view_row_referring_to_it() {
    let models = format!("{CHAIN_MODELS}{RELAY}}}\n");
    let (_, analysis) = analyzed(&[
        ("src/ChainView.cs", CHAIN_VIEWS),
        ("src/ChainViewModel.cs", &models),
    ]);
    let on_key = key(CHAIN_NS, "ChainView.OnKey(object,KeyEventArgs)");
    let can_check = key(CHAIN_NS, "ChainViewModel.CanCheck()");
    let dead_at_can_check = |row: &TriggerRow| matches!(&row.state, PathState::Dead { member, .. } if member == &can_check);

    let vm = key(CHAIN_NS, "ChainViewModel");
    let command = holding(
        &analysis,
        &vm,
        &key(CHAIN_NS, "ChainViewModel.CheckCommand"),
    );
    let (_, to_vm) = row_to(&analysis, &vm, command, TriggerKind::Key, &on_key);
    assert!(dead_at_can_check(to_vm), "{to_vm:?}");

    let ledger = key(CHAIN_NS, "Ledger");
    let validate = key(CHAIN_NS, "Ledger.Validate()");
    let f = holding(&analysis, &ledger, &validate);
    let (i, downstream) = row_to(&analysis, &ledger, f, TriggerKind::Key, &on_key);
    assert!(
        downstream
            .path
            .iter()
            .any(|h| command.members.contains(&h.member)),
        "the way goes through the command: {downstream:?}"
    );
    assert!(dead_at_can_check(downstream), "{downstream:?}");
    let (flow, _) = flow_of(context_of(&analysis, &ledger, f), i);
    assert_eq!(flow.state, downstream.state);

    let view = key(CHAIN_NS, "ChainView");
    let own = type_of(&analysis, &view)
        .functionalities
        .iter()
        .find(|f| f.entry_points.iter().any(|e| e.member == on_key))
        .unwrap();
    let (_, own_row) = row_to(&analysis, &view, own, TriggerKind::Key, &on_key);
    assert!(own_row.own);
    assert!(dead_at_can_check(own_row), "{own_row:?}");
}

/// A view model whose two commands share its state, so they are one
/// functionality: the check can never run, the save can, and the save has a
/// key gesture of its own.
const TWIN: &str = r#"namespace Acme.Twin
{
    public class TwinViewModel
    {
        private RelayCommand saveCommand;
        private RelayCommand checkCommand;
        private bool dirty;

        public RelayCommand SaveCommand
        {
            get
            {
                if (saveCommand == null)
                {
                    saveCommand = new RelayCommand(p => Save());
                    saveCommand.KeyGestures = new KeyGesture[] { new KeyGesture(Key.S, ModifierKeys.Control) };
                }
                return saveCommand;
            }
        }

        public RelayCommand CheckCommand
        {
            get
            {
                if (checkCommand == null) checkCommand = new RelayCommand(p => Check(), p => CanCheck());
                return checkCommand;
            }
        }

        public void Save() { dirty = false; }

        public void Check() { dirty = true; }

        bool CanCheck() { return false; }
    }
"#;

const TWIN_NS: &str = "sym:csharp:Acme.Twin#";

/// R12: a trigger written in the functionality's own type enters it where its
/// code does — the save's gesture at the save command — so the check's dead
/// CanExecute in the same functionality does not turn the save's way off.
#[test]
fn an_own_gesture_of_a_live_command_stays_active_beside_a_dead_one() {
    let src = format!("{TWIN}{RELAY}}}\n");
    let (_, analysis) = analyzed(&[("src/Twin.cs", &src)]);
    let vm = key(TWIN_NS, "TwinViewModel");
    let save = key(TWIN_NS, "TwinViewModel.SaveCommand");
    let check = key(TWIN_NS, "TwinViewModel.CheckCommand");
    let can_check = key(TWIN_NS, "TwinViewModel.CanCheck()");
    let f = holding(&analysis, &vm, &save);
    assert!(
        f.members.contains(&check) && f.members.contains(&can_check),
        "both commands are one functionality: {:?}",
        f.members
    );
    // The check's way is dead in it.
    assert!(
        rows_to(&analysis, &vm, f).iter().any(|(_, r)| matches!(
            &r.state,
            PathState::Dead { member, .. } if member == &can_check
        )),
        "{:?}",
        rows_to(&analysis, &vm, f)
    );

    let (i, gesture) = row_to(&analysis, &vm, f, TriggerKind::Key, &save);
    assert!(gesture.own);
    assert_eq!(gesture.trigger.key, "Key.S+ModifierKeys.Control");
    assert_eq!(gesture.state, PathState::Active);
    assert!(gesture.entries.contains(&save), "{:?}", gesture.entries);
    assert!(
        !gesture.entries.contains(&check) && !gesture.entries.contains(&can_check),
        "{:?}",
        gesture.entries
    );
    let (flow, common) = flow_of(context_of(&analysis, &vm, f), i);
    assert_eq!(flow.state, PathState::Active);
    assert_eq!(common.dead, None);
}

/// AE4: `ExternalCheck`, nothing calls it, is an entry from another component
/// of the check; its flow enters at `Check(bool)`: no F4, no CanExecute, no
/// branch on Shift.
#[test]
fn the_external_check_is_an_entry_from_another_component_without_the_commands_steps() {
    let (_, analysis) = editor_2b();
    let vm = key(EDITOR_MODELS, "EditorViewModel");
    let check_bool = key(EDITOR_MODELS, "EditorViewModel.Check(bool)");
    let f = holding(&analysis, &vm, &check_bool);
    let run = key(EDITOR_MODELS, "ExternalCheck.Run()");
    let (i, row) = row_to(&analysis, &vm, f, TriggerKind::External, &run);
    assert_eq!(row.state, PathState::Active);
    assert_eq!(row.trigger.key, run.as_str());
    assert_eq!(names(&row.entries), [check_bool.as_str()]);
    let (flow, common) = flow_of(context_of(&analysis, &vm, f), i);
    assert!(flow.condition.is_none());
    assert!(common
        .preconditions
        .iter()
        .all(|p| p.occurrence.source == Source::Guard));
    assert!(!common.preconditions.is_empty());
    assert!(common.steps.iter().all(|s| !matches!(
        s.kind,
        FlowStepKind::Delegate { .. } | FlowStepKind::ArgumentChoice { .. }
    )));
    // The two ways in share nothing: two parts, one per entry.
    assert_ne!(flow.common, {
        let on_key = key(
            EDITOR_VIEWS,
            "EditorView.Editor_PreviewKeyDown(object,KeyEventArgs)",
        );
        let (k, _) = row_to(&analysis, &vm, f, TriggerKind::Key, &on_key);
        flow_of(context_of(&analysis, &vm, f), k).0.common
    });
}

/// AE5: a key gesture constructed into a command in C# is the key row
/// "Ctrl+F4" of the command's functionality, anchored at the gesture.
#[test]
fn a_key_gesture_given_to_a_command_is_a_key_row() {
    let (_, analysis) = editor_2b();
    let vm = key(EDITOR_MODELS, "EditorViewModel");
    let close = key(EDITOR_MODELS, "EditorViewModel.CloseErrors()");
    let f = holding(&analysis, &vm, &close);
    let property = key(EDITOR_MODELS, "EditorViewModel.CloseErrorsCommand");
    let (i, row) = row_to(&analysis, &vm, f, TriggerKind::Key, &property);
    assert_eq!(row.trigger.key, "Key.F4+ModifierKeys.Control");
    assert_eq!(row.trigger.modifiers, ["ModifierKeys.Control"]);
    assert_eq!(row.state, PathState::Active);
    let place = row.trigger.place.as_ref().unwrap();
    assert_eq!(place.path, "src/ViewModels/EditorViewModel.cs");
    assert_eq!(
        place.range.start.line,
        line_in(
            editor_fixture::VIEW_MODEL,
            "closeErrorsCommand.KeyGestures ="
        )
    );
    // Its flow reaches the command's target.
    let (_, common) = flow_of(context_of(&analysis, &vm, f), i);
    assert!(common.steps.iter().any(|s| matches!(&s.kind,
        FlowStepKind::Delegate { targets, .. } if targets == std::slice::from_ref(&close))));
    // The command property is no root of its own: the gesture is its way in.
    assert!(rows_to(&analysis, &vm, f)
        .iter()
        .all(|(_, r)| r.trigger.kind != TriggerKind::External));
}

/// A member only a `NameOnly` relation calls is an entry from another
/// component with the state "unresolved"; the weak caller is never followed.
#[test]
fn a_member_called_by_name_alone_is_an_unresolved_entry() {
    let (mut generated, analysis) = editor_2b();
    let vm = key(EDITOR_MODELS, "EditorViewModel");
    let check_bool = key(EDITOR_MODELS, "EditorViewModel.Check(bool)");
    let run = key(EDITOR_MODELS, "ExternalCheck.Run()");
    let f = holding(&analysis, &vm, &check_bool).clone();
    assert_eq!(
        row_to(&analysis, &vm, &f, TriggerKind::External, &run)
            .1
            .state,
        PathState::Active
    );
    let store = key(EDITOR_VIEWS, "EditorView.Store()");
    assert!(generated
        .graph
        .add_edge(&store, &run, rated(Confidence::NameOnly, Rule::NameMatch)));
    let analysis = layer2::analyze(&generated.graph, generated.bodies.as_ref().unwrap());
    let (_, row) = row_to(&analysis, &vm, &f, TriggerKind::External, &run);
    assert_eq!(
        row.state,
        PathState::Unresolved {
            callers: vec![store.clone()].into()
        }
    );
    assert!(analysis
        .triggers
        .iter()
        .all(|r| r.trigger.member != store && r.path.iter().all(|h| h.member != store)));
}

/// The save comes into the registry through the trigger of the save — the
/// public `Save()` nothing calls — never as a root of its own `BeforeSave()`.
#[test]
fn the_save_reaches_the_registry_through_the_save_trigger() {
    let (_, analysis) = editor_2b();
    let vm = key(EDITOR_MODELS, "EditorViewModel");
    let check_bool = key(EDITOR_MODELS, "EditorViewModel.Check(bool)");
    let f = holding(&analysis, &vm, &check_bool);
    let save = key(EDITOR_VIEWS, "EditorView.Save()");
    let before_save = key(EDITOR_VIEWS, "EditorView.BeforeSave()");
    let (i, row) = row_to(&analysis, &vm, f, TriggerKind::External, &save);
    let path: Vec<&NodeKey> = row.path.iter().map(|h| &h.member).collect();
    assert_eq!(path, [&save, &before_save]);
    assert!(rows_to(&analysis, &vm, f)
        .iter()
        .all(|(_, r)| r.trigger.member != before_save));
    // It enters through the command: the CanExecute is its precondition too.
    let (_, common) = flow_of(context_of(&analysis, &vm, f), i);
    assert!(common
        .preconditions
        .iter()
        .any(|p| p.occurrence.source == Source::CanExecute));
}

/// A utility three types share, called in the middle of a flow.
const SHARED: &str = r#"namespace Acme.Shared
{
    public class TextUtil
    {
        public string Trim(string s) { return s; }
    }

    public class Formatter
    {
        private TextUtil util;
        public string Format(string s) { return util.Trim(s); }
    }

    public class Exporter
    {
        private Formatter formatter;
        public void Export() { formatter.Format("x"); }
    }
}
"#;

const SHARED_NS: &str = "sym:csharp:Acme.Shared#";

/// The call into a shared utility in the middle of a flow is no trigger: the
/// utility's one row is the root its chain of callers ends at.
#[test]
fn a_call_of_a_shared_utility_in_the_middle_of_a_flow_is_no_trigger() {
    let (_, analysis) = analyzed(&[("src/Shared.cs", SHARED)]);
    let util = key(SHARED_NS, "TextUtil");
    let f = holding(&analysis, &util, &key(SHARED_NS, "TextUtil.Trim(string)"));
    let rows = rows_to(&analysis, &util, f);
    let found: Vec<(TriggerKind, &str)> = rows
        .iter()
        .map(|(_, r)| (r.trigger.kind, r.trigger.member.as_str()))
        .collect();
    let export = key(SHARED_NS, "Exporter.Export()");
    assert_eq!(found, [(TriggerKind::External, export.as_str())]);
    let path: Vec<&str> = rows[0].1.path.iter().map(|h| h.member.as_str()).collect();
    assert_eq!(
        path,
        [
            export.as_str(),
            key(SHARED_NS, "Formatter.Format(string)").as_str()
        ]
    );
}

/// Callers that call each other end the walk: each member is walked once per
/// set of members it leads to, and the one root gets one row.
const CYCLE: &str = r#"namespace Acme.Cycle
{
    public class Target
    {
        public int Count;
        public void Hit() { Count++; }
    }

    public class Ping
    {
        private Target target;
        private Pong pong;
        public void Go() { target.Hit(); pong.Back(); }
    }

    public class Pong
    {
        private Ping ping;
        public void Back() { ping.Go(); }
    }

    public class Starter
    {
        private Ping ping;
        public void Start() { ping.Go(); }
    }
}
"#;

const CYCLE_NS: &str = "sym:csharp:Acme.Cycle#";

#[test]
fn a_cycle_of_callers_ends_the_walk_without_repeating() {
    let (_, analysis) = analyzed(&[("src/Cycle.cs", CYCLE)]);
    let target = key(CYCLE_NS, "Target");
    let f = holding(&analysis, &target, &key(CYCLE_NS, "Target.Hit()"));
    let found: Vec<(TriggerKind, String)> = rows_to(&analysis, &target, f)
        .iter()
        .map(|(_, r)| (r.trigger.kind, r.trigger.member.as_str().to_string()))
        .collect();
    assert_eq!(
        found,
        [(TriggerKind::External, format!("{CYCLE_NS}Starter.Start()"))]
    );
}

/// A member with a dynamic-call marker with a place (the model of step 2c,
/// simulated) gets no row: the trigger that calls it is written elsewhere.
#[test]
fn a_member_with_a_dynamic_call_marker_gets_no_row() {
    let (generated, analysis) = analyzed(&[("src/Shared.cs", SHARED)]);
    let util = key(SHARED_NS, "TextUtil");
    let export = key(SHARED_NS, "Exporter.Export()");
    let f = holding(&analysis, &util, &key(SHARED_NS, "TextUtil.Trim(string)")).clone();
    assert_eq!(rows_to(&analysis, &util, &f).len(), 1);
    let marks: std::collections::BTreeSet<NodeKey> = [export.clone()].into();
    let marked = layer2::analyze_with(
        &generated.graph,
        generated.bodies.as_ref().unwrap(),
        DEFAULT_AMBIGUITY_CEILING,
        &marks,
    );
    assert!(rows_to(&marked, &util, &f).is_empty());
    assert!(marked.triggers.iter().all(|r| r.trigger.member != export));
}

/// AE9: the functionality of the command class carries one flow — from its
/// `Execute` on, with its guard and the event it raises — and the number of
/// the triggers that lead to it, not a flow per trigger.
#[test]
fn the_command_class_has_one_flow_and_the_number_of_its_triggers() {
    let (_, analysis) = editor_2b();
    let command = key(EDITOR_MODELS, "RelayCommand");
    let execute = key(EDITOR_MODELS, "RelayCommand.Execute(object)");
    let f = holding(&analysis, &command, &execute);
    let context = context_of(&analysis, &command, f);
    let class = context.command_class.as_ref().expect("a command class");
    assert!(context.flows.is_empty());
    assert_eq!(context.common.len(), 1);
    let rows = rows_to(&analysis, &command, f);
    assert_eq!(class.triggers, rows.len());
    let kinds: Vec<(TriggerKind, &str)> = rows
        .iter()
        .map(|(_, r)| (r.trigger.kind, r.trigger.member.as_str()))
        .collect();
    let on_key = key(
        EDITOR_VIEWS,
        "EditorView.Editor_PreviewKeyDown(object,KeyEventArgs)",
    );
    let menu = key(EDITOR_VIEWS, "EditorView.BuildMenu()");
    assert!(
        kinds.contains(&(TriggerKind::Key, on_key.as_str())),
        "{kinds:?}"
    );
    assert!(
        kinds.contains(&(TriggerKind::MenuItem, menu.as_str())),
        "{kinds:?}"
    );
    let common = &context.common[class.common];
    let results: Vec<&FlowResult> = common
        .results
        .iter()
        .filter(|r| matches!(r.kind, ResultKind::RaisesEvent { .. }))
        .collect();
    assert_eq!(
        described(&results),
        [
            "raises RelayCommand.ExecutingStarted (0)",
            "raises RelayCommand.Executed (1)"
        ]
    );
    assert!(common.preconditions.iter().any(|p| p.text == "evt.Cancel"));
}

/// R9: the view's functionality of F4 and the menu item refers to the flow of
/// the check it leads to; the flow is drawn there, not twice. A trigger whose
/// code never leaves the view (the event its handler stores on) keeps its flow
/// at the view.
#[test]
fn the_views_entry_refers_to_the_flow_of_its_target() {
    let (_, analysis) = editor_2b();
    let view = key(EDITOR_VIEWS, "EditorView");
    let on_key = key(
        EDITOR_VIEWS,
        "EditorView.Editor_PreviewKeyDown(object,KeyEventArgs)",
    );
    let own = type_of(&analysis, &view)
        .functionalities
        .iter()
        .find(|f| f.entry_points.iter().any(|e| e.member == on_key))
        .unwrap();
    let (j, row) = row_to(&analysis, &view, own, TriggerKind::Key, &on_key);
    assert!(row.own);
    let context = context_of(&analysis, &view, own);
    assert!(context.flows.iter().all(|f| f.row != j));
    let vm = key(EDITOR_MODELS, "EditorViewModel");
    let check = holding(
        &analysis,
        &vm,
        &key(EDITOR_MODELS, "EditorViewModel.Check(bool)"),
    );
    assert!(
        context.refers.contains(&target_of(&vm, check)),
        "{:?}",
        context.refers
    );
    // The command class is counted, never referred to.
    assert!(context
        .refers
        .iter()
        .all(|r| r.ty != key(EDITOR_MODELS, "RelayCommand")));
    // The event `Executed` the view's handler stores on stays in the view.
    let ctor = key(EDITOR_VIEWS, "EditorView.EditorView()");
    let (k, event) = row_to(&analysis, &view, own, TriggerKind::Event, &ctor);
    assert_eq!(event.trigger.key, "Executed");
    assert!(context.flows.iter().any(|f| f.row == k));
}

/// R-25: a 2a entry point's target follows the external index's relation at
/// the access, as the links of 2b do — `Lookup().Run()` is told only so.
const PINNED: &str = r#"namespace Acme.Pinned
{
    public class Worker
    {
        public int Runs;
        public void Run() { Runs++; }
    }

    public class Panel
    {
        Worker Lookup() { return null; }

        void OnKey(KeyEventArgs e)
        {
            if (e.Key == Key.F4) { Lookup().Run(); }
        }
    }
}
"#;

const PINNED_NS: &str = "sym:csharp:Acme.Pinned#";

#[test]
fn an_entry_points_target_follows_the_index_relation_like_a_link() {
    let (mut generated, analysis) = analyzed(&[("src/Pinned.cs", PINNED)]);
    let panel = key(PINNED_NS, "Panel");
    let on_key = key(PINNED_NS, "Panel.OnKey(KeyEventArgs)");
    let run = key(PINNED_NS, "Worker.Run()");
    let target_of_f4 = |analysis: &Analysis| {
        entry(
            type_of(analysis, &panel),
            &format!("{}#Key.F4", on_key.as_str()),
        )
        .targets
        .iter()
        .find(|t| t.path.segments == ["Run"])
        .expect("the call of Run")
        .clone()
    };
    let before = target_of_f4(&analysis);
    assert_eq!(before.member, None);
    assert_eq!(before.unresolved(), Some(&Unresolved::NoReceiverChain));

    let line = line_in(PINNED, "Lookup().Run();");
    assert!(generated
        .graph
        .add_edge(&on_key, &run, indexed("src/Pinned.cs", line)));
    let analysis = layer2::analyze(&generated.graph, generated.bodies.as_ref().unwrap());
    let after = target_of_f4(&analysis);
    assert_eq!(after.member.as_ref(), Some(&run));
    // The link of 2b says the same.
    let ix = Index::new(&generated.graph, generated.bodies.as_ref().unwrap());
    let links = Links::new(&ix, DEFAULT_AMBIGUITY_CEILING);
    let sites = links.sites_of(&on_key);
    let r = &site(&sites, "Run", true).resolution;
    assert_eq!(r.target(), Some(&run));
    assert_eq!(r.grade(), Some(Confidence::ResolvedByIndex));
}

/// KTD13: two runs over the same repository give the same registry and the
/// same flows.
#[test]
fn two_runs_give_the_same_triggers_and_flows() {
    let (_, first) = editor_2b();
    let (_, again) = editor_2b();
    assert_eq!(first.triggers, again.triggers);
    assert_eq!(first, again);
    let order: Vec<(TriggerKind, &str)> = first
        .triggers
        .iter()
        .map(|r| (r.trigger.kind, r.trigger.key.as_str()))
        .collect();
    assert!(order.windows(2).all(|w| w[0] <= w[1]), "{order:?}");
}

// ---- U6: the `### Context` of a card ----

use reverse_architect::card::build_cards;

/// A run with the second layer at `level` in `lang` over `files`.
fn layered(files: &[(&str, &str)], level: u8, lang: &str) -> Generated {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    common::init_repo_with_files(&repo, "Alice", files);
    generate(&Config {
        lang: lang.to_string(),
        detail_level: Some(level),
        ..common::config_for(&repo, &dir.path().join("out"))
    })
    .unwrap()
}

/// The path of the card of the type `key`.
fn card_path(g: &Generated, key: &NodeKey) -> String {
    build_cards(&g.graph)
        .into_iter()
        .find(|c| c.identity == key.as_str())
        .unwrap_or_else(|| panic!("no card for {key:?}"))
        .path
}

/// The rendered card of the type `key`.
fn card_text<'g>(g: &'g Generated, key: &NodeKey) -> &'g str {
    let path = card_path(g, key);
    g.cards
        .iter()
        .find(|c| c.path == path)
        .map(|c| c.contents.as_str())
        .unwrap_or_else(|| panic!("no card at {path}"))
}

/// The part of `text` under `heading` (`## …` or `### …`), through the line
/// before the next heading of its level or above (`####` is below both).
fn part<'c>(text: &'c str, heading: &str) -> &'c str {
    let start = text
        .find(&format!("\n{heading}\n"))
        .unwrap_or_else(|| panic!("no {heading:?} in:\n{text}"))
        + 1;
    let rest = &text[start..];
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

/// The flow of a context under `#### heading`, through the next `####`.
fn flow_block<'c>(context: &'c str, heading: &str) -> &'c str {
    let start = context
        .find(&format!("\n#### {heading}\n"))
        .unwrap_or_else(|| panic!("no flow {heading:?} in:\n{context}"))
        + 1;
    let rest = &context[start..];
    let end = rest[1..].find("\n#### ").map_or(rest.len(), |i| i + 2);
    &rest[..end]
}

/// Every `### Context` of a card.
fn contexts(card: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = card[from..].find("\n### Context\n") {
        let start = from + i + 1;
        let rest = &card[start..];
        let mut end = rest.len();
        let mut at = 0;
        for (n, line) in rest.split_inclusive('\n').enumerate() {
            if n > 0 && (line.starts_with("## ") || line.starts_with("### ")) {
                end = at;
                break;
            }
            at += line.len();
        }
        out.push(&rest[..end]);
        from = start + end.max(1);
    }
    out
}

/// A line ends in its anchors: `` — `path:N` `` or `` `path:N-M` ``, several
/// separated by commas, never with a column.
fn ends_in_anchor(line: &str) -> bool {
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

/// The `sequenceDiagram` blocks of a text.
fn sequence_blocks(text: &str) -> Vec<&str> {
    text.split("```mermaid\n")
        .skip(1)
        .map(|b| b.split("```").next().unwrap())
        .filter(|b| b.starts_with("sequenceDiagram\n"))
        .collect()
}

/// A sequence diagram is valid: every participant is declared once as `pN`,
/// every message joins two declared participants, and no text holds a raw
/// `;`, `<`, `>` or `"`. Its messages.
fn valid_sequence(block: &str) -> Vec<&str> {
    let mut declared: Vec<&str> = Vec::new();
    let mut messages = Vec::new();
    for line in block.lines().skip(1) {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("participant ") {
            let (id, name) = rest.split_once(" as ").expect("an alias");
            assert!(!declared.contains(&id), "{id} twice:\n{block}");
            for raw in [';', '<', '>', '"'] {
                assert!(!name.contains(raw), "raw {raw:?}: {line}");
            }
            declared.push(id);
            continue;
        }
        let (arrow, text) = line.split_once(": ").expect("a message");
        let (from, to) = arrow.split_once("->>").expect("an arrow");
        assert!(declared.contains(&from) && declared.contains(&to), "{line}");
        for raw in [';', '<', '>', '"'] {
            assert!(!text.contains(raw), "raw {raw:?}: {line}");
        }
        messages.push(line);
    }
    messages
}

/// The heading of a functionality's section on its card.
fn functionality_heading(f: &Functionality) -> String {
    let key = f.key.strip_prefix("sym:").unwrap_or(&f.key);
    format!(
        "## Functionality: {}",
        key.split_once('#').map_or(key, |(_, k)| k)
    )
}

/// The `### Context` of the functionality of `ty` that holds `member`, in a
/// run at `level` in `lang` over the editor.
fn editor_context(g: &Generated, ty: &NodeKey, member: &NodeKey) -> String {
    let analysis = layer2::analyze(&g.graph, g.bodies.as_ref().unwrap());
    // A key branch's functionality holds the branch, not always its handler:
    // then the one it is an entry point of.
    let all = &type_of(&analysis, ty).functionalities;
    let f = all
        .iter()
        .find(|f| f.members.contains(member))
        .or_else(|| {
            all.iter()
                .find(|f| f.entry_points.iter().any(|e| &e.member == member))
        })
        .unwrap_or_else(|| panic!("no functionality of {ty:?} has {member:?}"));
    let card = card_text(g, ty);
    part(part(card, &functionality_heading(f)), "### Context").to_string()
}

/// AE8, R16: the context of the check in `EditorViewModelNew` has on level 1
/// the trigger F4 and its dead path with its anchor and no step; on level 2
/// the steps and the sequence diagram too.
#[test]
fn the_new_editors_check_has_the_dead_f4_on_level_1_and_its_steps_on_level_2() {
    let vm_new = key(EDITOR_MODELS, "EditorViewModelNew");
    let command = key(EDITOR_MODELS, "EditorViewModelNew.CheckCommand");
    let can_check = format!(
        "`src/ViewModels/EditorViewModelNew.cs:{}`",
        line_in(editor_fixture::VIEW_MODEL_NEW, "bool CanCheck()")
    );
    let one = layered(&editor_fixture::files(), 1, "en");
    let context = editor_context(&one, &vm_new, &command);
    let f4 = flow_block(&context, "Key: Key.F4 — EditorViewNew");
    let trigger = f4
        .lines()
        .find(|l| l.starts_with("- Trigger: key `Key.F4` in `EditorViewNew."))
        .unwrap_or_else(|| panic!("the trigger:\n{f4}"));
    assert!(ends_in_anchor(trigger), "{trigger}");
    let dead = f4
        .lines()
        .find(|l| l.starts_with("- Path: dead, turned off at `CanCheck()`"))
        .unwrap_or_else(|| panic!("the dead path:\n{f4}"));
    assert!(dead.ends_with(&format!(" — {can_check}")), "{dead}");
    assert!(!context.contains("- Step:"), "{context}");
    assert!(!context.contains("```mermaid"), "{context}");

    let two = layered(&editor_fixture::files(), 2, "en");
    let context = editor_context(&two, &vm_new, &command);
    let f4 = flow_block(&context, "Key: Key.F4 — EditorViewNew");
    assert!(f4.contains(&format!(" — {can_check}\n")), "{f4}");
    assert!(
        f4.lines().any(|l| l.starts_with(
            "- Step: `EditorViewNew.Editor_PreviewKeyDown(object,KeyEventArgs)` → `CheckCommand` \
             (card [EditorViewNew]("
        )),
        "the view's step into the command:\n{f4}"
    );
    assert!(
        f4.lines().any(|l| l.starts_with(
            "- Step: `CheckCommand`: `RelayCommand.CanExecute(object)` → `CanCheck()`"
        )),
        "{f4}"
    );
    let blocks = sequence_blocks(f4);
    assert_eq!(blocks.len(), 1, "{f4}");
    let messages = valid_sequence(blocks[0]);
    assert!(!messages.is_empty(), "{}", blocks[0]);
    assert!(
        blocks[0].contains("participant p0 as EditorViewNew\n"),
        "{}",
        blocks[0]
    );
}

/// R2, KTD8: every line of a step or a result of every `### Context` of the
/// editor's cards ends in its anchor in backticks after ` — `, and so does
/// every precondition and dead path; the sequence diagrams are valid.
#[test]
fn every_step_and_result_of_a_context_ends_in_its_anchor() {
    let g = layered(&editor_fixture::files(), 2, "en");
    let (mut steps, mut results) = (0, 0);
    for card in g.cards.iter().filter(|c| c.path.starts_with("cards/type/")) {
        for context in contexts(&card.contents) {
            for line in context.lines() {
                let anchored = ["- Step: ", "- Result: ", "- Precondition: ", "- Path: dead"];
                if !anchored.iter().any(|p| line.starts_with(p)) {
                    continue;
                }
                steps += usize::from(line.starts_with("- Step: "));
                results += usize::from(line.starts_with("- Result: "));
                assert!(ends_in_anchor(line), "{}: {line}", card.path);
            }
            for block in sequence_blocks(context) {
                valid_sequence(block);
            }
        }
    }
    assert!(steps > 5 && results > 5, "{steps} steps, {results} results");
}

/// AE1, AE10, R1, R7: on the card of `EditorViewModel`, the flow of F4 has
/// the key branch's condition, the command class as one step `Execute →
/// Check()`, the branch on Shift, the parser as one step with its card, and
/// the results; the menu item that enters at the same command refers to the
/// part F4 wrote instead of repeating it.
#[test]
fn the_f4_flow_of_the_old_editor_is_drawn_once_and_the_menu_item_refers_to_it() {
    let g = layered(&editor_fixture::files(), 2, "en");
    let vm = key(EDITOR_MODELS, "EditorViewModel");
    let context = editor_context(&g, &vm, &key(EDITOR_MODELS, "EditorViewModel.Check(bool)"));
    let f4 = flow_block(&context, "Key: Key.F4 — EditorView");
    for part in [
        "- Trigger: key `Key.F4` in `EditorView.Editor_PreviewKeyDown(object,KeyEventArgs)`, \
         when `e.Key == Key.F4 && (viewModel.ShowErrors || viewModel.CanErrors)` — ",
        "- Precondition: does not run when `!canErrors` (the CanExecute predicate in \
         `CheckCommand`) — ",
        ": `RelayCommand.Execute(object)` → `Check()` (the command class ",
        "- Step: `Check()` calls `Check(bool)`, the argument for `syntaxOnly` branches on \
         `ModifierKeys.Shift` — ",
        "- Step: `Check(bool)` calls `Parser.Parse(string,bool)`, one step (card [Parser](",
        "- Result: `Check(bool)` writes `Errors`, which notifies its change — ",
        "- Result: `Check(bool)` writes `CheckWasDone` = `true`, read by ",
    ] {
        assert!(f4.contains(part), "{part}\n{f4}");
    }
    assert!(!f4.contains("EditorViewModelNew"), "{f4}");
    let menu = flow_block(&context, "Menu item: Check errors — EditorView");
    assert!(
        menu.contains("- From `CheckCommand` on: as under “Key: Key.F4 — EditorView” above.\n"),
        "{menu}"
    );
    assert!(
        !menu.contains("- Result: "),
        "the shared part is not repeated:\n{menu}"
    );
    // The precondition is said once in the context, under F4.
    assert_eq!(
        context
            .matches("does not run when `!canErrors` (the CanExecute")
            .count(),
        1,
        "{context}"
    );
}

/// A functionality with `n` calls of another type: one flow of more steps
/// than a diagram draws. The callee is an interface, which has no
/// functionality of its own, so the flow is drawn at the hub.
fn wide(n: usize) -> String {
    let members: String = (0..n).map(|i| format!("        void M{i}();\n")).collect();
    let calls: String = (0..n).map(|i| format!(" worker.M{i}();")).collect();
    format!(
        "namespace Acme.Wide\n{{\n    public interface IAcmeWorker\n    {{\n{members}    }}\n\n\
         \x20   public class AcmeHub\n    {{\n        private IAcmeWorker worker;\n\
         \x20       public void Run() {{{calls} }}\n    }}\n}}\n"
    )
}

/// KTD8: a sequence diagram with more messages than the cap
/// ([`SPLIT_THRESHOLD`]) draws the cap and says, in English and in Czech,
/// how many there are and that the steps list them all.
#[test]
fn a_sequence_diagram_over_the_cap_draws_the_cap_and_says_the_rest() {
    use reverse_architect::render::mermaid::SPLIT_THRESHOLD;
    let n = SPLIT_THRESHOLD + 5;
    let source = wide(n);
    let hub = key("sym:csharp:Acme.Wide#", "AcmeHub");
    let run = key("sym:csharp:Acme.Wide#", "AcmeHub.Run()");
    for (lang, sentence) in [
        (
            "en",
            format!(
                "The sequence diagram draws {SPLIT_THRESHOLD} of {n} messages; the steps above \
                 list them all."
            ),
        ),
        (
            "cs",
            format!(
                "Sekvenční diagram kreslí {SPLIT_THRESHOLD} zpráv z celkem {n}; všechny uvádějí \
                 kroky výše."
            ),
        ),
    ] {
        let g = layered(&[("src/Wide.cs", source.as_str())], 2, lang);
        let context = editor_context(&g, &hub, &run);
        let blocks = sequence_blocks(&context);
        assert_eq!(blocks.len(), 1, "{lang}: {context}");
        assert_eq!(valid_sequence(blocks[0]).len(), SPLIT_THRESHOLD, "{lang}");
        assert_eq!(
            context.matches(sentence.as_str()).count(),
            1,
            "{lang}: {context}"
        );
        let steps = context
            .lines()
            .filter(|l| l.starts_with("- Step: ") || l.starts_with("- Krok: "))
            .count();
        assert_eq!(steps, n, "{lang}: every step is listed: {context}");
    }
    // Under the cap, no sentence.
    let g = layered(&[("src/Wide.cs", wide(3).as_str())], 2, "en");
    let context = editor_context(&g, &hub, &run);
    assert_eq!(valid_sequence(sequence_blocks(&context)[0]).len(), 3);
    assert!(!context.contains("The sequence diagram draws"), "{context}");
}

/// R18, KTD8: a card in Czech has Czech sentences in its context and the
/// headings in English.
#[test]
fn a_czech_context_has_czech_sentences_and_english_headings() {
    let g = layered(&editor_fixture::files(), 2, "cs");
    let vm = key(EDITOR_MODELS, "EditorViewModel");
    let context = editor_context(&g, &vm, &key(EDITOR_MODELS, "EditorViewModel.Check(bool)"));
    let f4 = flow_block(&context, "Key: Key.F4 — EditorView");
    for part in [
        "- Spouštěč: klávesa `Key.F4` v `EditorView.Editor_PreviewKeyDown(object,KeyEventArgs)`, \
         když ",
        "- Předpoklad: nespustí se, když `!canErrors` (podmínka CanExecute v `CheckCommand`) — ",
        "- Krok: `Check()` volá `Check(bool)`, argument pro `syntaxOnly` větví podle \
         `ModifierKeys.Shift` — ",
        "- Výsledek: `Check(bool)` zapíše `Errors`, vlastnost hlásí změnu — ",
    ] {
        assert!(f4.contains(part), "{part}\n{f4}");
    }
    for english in ["- Trigger:", "- Step:", "- Result:", " calls ", " writes "] {
        assert!(!context.contains(english), "{english}\n{context}");
    }
    assert!(context.starts_with("### Context\n"), "{context}");
    let menu = flow_block(&context, "Menu item: Check errors — EditorView");
    assert!(
        menu.contains("- Od `CheckCommand` dál: jako u „Key: Key.F4 — EditorView“ výše.\n"),
        "{menu}"
    );
    // The state's context too.
    let card = card_text(&g, &vm);
    let state = part(
        part(card, "## State: EditorViewModel.CheckWasDone"),
        "### Context",
    );
    assert!(
        state.contains("\n- Čte `ExternalCheck.Run()` — `"),
        "{state}"
    );
}

/// A key branch whose condition compares a key-named field with a literal.
const LOCK: &str = r#"namespace Acme.Locks
{
    public class AcmeLock
    {
        private string token;
        public int Opened;

        void OnKey(KeyEventArgs e)
        {
            if (e.Key == Key.F4 && token == "brindlecairn") { Open(); }
        }

        void Open() { Opened++; }
    }
}
"#;

/// R18, U12 of 2a: a literal of a secret's shape in the condition of a
/// trigger is not on the card; the trigger's line in the context keeps the
/// marker and says once that a value was left out — in English and Czech.
#[test]
fn a_secret_in_a_triggers_condition_is_left_out_of_the_context_and_admitted() {
    let lock = key("sym:csharp:Acme.Locks#", "AcmeLock");
    let on_key = key("sym:csharp:Acme.Locks#", "AcmeLock.OnKey(KeyEventArgs)");
    for (lang, lead, sentence) in [
        (
            "en",
            "- Trigger: key `Key.F4` in `OnKey(KeyEventArgs)`, when `e.Key == Key.F4 && token == ",
            "a value was left out as a likely secret; see the source",
        ),
        (
            "cs",
            "- Spouštěč: klávesa `Key.F4` v `OnKey(KeyEventArgs)`, když `e.Key == Key.F4 && token \
             == ",
            "hodnota vynechána jako pravděpodobné tajemství; viz zdroj",
        ),
    ] {
        let g = layered(&[("src/Lock.cs", LOCK)], 1, lang);
        let card = card_text(&g, &lock);
        let context = editor_context(&g, &lock, &on_key);
        let line = context
            .lines()
            .find(|l| l.starts_with(lead))
            .unwrap_or_else(|| panic!("{lang}: no trigger line:\n{context}"));
        assert!(line.contains("[redacted: "), "{lang}: {line}");
        assert_eq!(line.matches(sentence).count(), 1, "{lang}: {line}");
        assert!(ends_in_anchor(line), "{lang}: {line}");
        assert!(!card.contains("brindlecairn"), "{lang}: {card}");
    }
}

/// KTD8: `### Context` and its `####` flows are no chunks of the manifest —
/// a type card's chunks are its `## ` headings, as without step 2b.
#[test]
fn the_context_makes_no_chunk_of_the_manifest() {
    let g = layered(&editor_fixture::files(), 2, "en");
    let manifest = serde_json::to_value(&g.manifest).unwrap();
    let vm = key(EDITOR_MODELS, "EditorViewModel");
    let path = card_path(&g, &vm);
    let card = card_text(&g, &vm);
    assert!(
        card.contains("\n### Context\n") && card.contains("\n#### "),
        "{card}"
    );
    let doc = manifest["documents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["path"] == path.as_str())
        .unwrap_or_else(|| panic!("no document at {path}"));
    let headings: Vec<&str> = doc["chunks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["heading"].as_str().unwrap())
        .collect();
    let sections: Vec<&str> = card.lines().filter_map(|l| l.strip_prefix("## ")).collect();
    assert_eq!(headings, sections);
    assert!(
        headings.iter().all(|h| !h.contains("Context")),
        "{headings:?}"
    );
}

/// R10, AE10: the context of the state `CheckWasDone` names the external
/// check among its readers — not the save, which writes the field before it
/// reads it — and the save among its writers, each with its anchor.
#[test]
fn the_context_of_a_state_says_who_reads_and_writes_it_from_outside() {
    let g = layered(&editor_fixture::files(), 1, "en");
    let card = card_text(&g, &key(EDITOR_MODELS, "EditorViewModel"));
    let state = part(
        part(card, "## State: EditorViewModel.CheckWasDone"),
        "### Context",
    );
    let reads: Vec<&str> = state
        .lines()
        .filter(|l| l.starts_with("- Read by "))
        .collect();
    let writes: Vec<&str> = state
        .lines()
        .filter(|l| l.starts_with("- Written by "))
        .collect();
    assert!(
        reads
            .iter()
            .any(|l| l.starts_with("- Read by `ExternalCheck.Run()` — ")),
        "{state}"
    );
    assert!(reads.iter().all(|l| !l.contains("BeforeSave")), "{state}");
    assert!(
        writes
            .iter()
            .any(|l| l.starts_with("- Written by `EditorView.BeforeSave()` — ")),
        "{state}"
    );
    assert!(
        reads.iter().chain(&writes).all(|l| ends_in_anchor(l)),
        "{state}"
    );
}

/// AE9, KTD7: the command class has one flow `Execute → target` with the
/// number of the triggers that lead to it and the event it raises, and no
/// flow per trigger; R9: the view's functionality of F4 says where its flow
/// is drawn.
#[test]
fn the_command_class_has_one_flow_and_the_view_refers_to_the_check() {
    let g = layered(&editor_fixture::files(), 2, "en");
    let analysis = layer2::analyze(&g.graph, g.bodies.as_ref().unwrap());
    let command = key(EDITOR_MODELS, "RelayCommand");
    let execute = key(EDITOR_MODELS, "RelayCommand.Execute(object)");
    let f = holding(&analysis, &command, &execute);
    let n = rows_to(&analysis, &command, f).len();
    let context = editor_context(&g, &command, &execute);
    let flow = flow_block(&context, "Execute → target");
    // The registry is a file since U7: the card links it.
    let registry = format!(
        "[cards/triggers.md]({})",
        reverse_architect::card::links::relative(&card_path(&g, &command), "cards/triggers.md")
    );
    assert!(
        flow.contains(&format!(
            "- {n} triggers lead to it: one flow for every command built of this class; the \
             registry of triggers ({registry}) lists every one.\n"
        )),
        "{flow}"
    );
    assert!(
        flow.contains("raises the event `Executed`; 1 subscription"),
        "{flow}"
    );
    assert_eq!(context.matches("\n#### ").count(), 1, "one flow: {context}");

    let view = key(EDITOR_VIEWS, "EditorView");
    let on_key = key(
        EDITOR_VIEWS,
        "EditorView.Editor_PreviewKeyDown(object,KeyEventArgs)",
    );
    let own = type_of(&analysis, &view)
        .functionalities
        .iter()
        .find(|f| f.entry_points.iter().any(|e| e.member == on_key))
        .unwrap();
    let card = card_text(&g, &view);
    let context = part(part(card, &functionality_heading(own)), "### Context");
    assert!(
        context.contains(
            "- The flows of its triggers are drawn at the functionality \
             `EditorViewModel.CheckCommand` of [EditorViewModel]("
        ),
        "{context}"
    );
    assert!(!context.contains("#### Key: Key.F4"), "{context}");
}

/// R17, KTD13: two runs give the same cards with their contexts, byte for
/// byte.
#[test]
fn two_runs_give_the_same_contexts() {
    let one = layered(&editor_fixture::files(), 2, "en");
    let two = layered(&editor_fixture::files(), 2, "en");
    assert_eq!(one.cards, two.cards);
    assert!(one
        .cards
        .iter()
        .any(|c| c.contents.contains("\n### Context\n")));
}

// ---- U7: the registry of triggers, `cards/triggers.md` ----

use reverse_architect::card::links::relative;
use reverse_architect::card::map::MAP_PART_ROWS;
use reverse_architect::card::render::Lang;
use reverse_architect::card::triggers::{coverage, BodyComponent, REGISTRY_PATH, TRIGGERS_ID};

/// The registry a run with the second layer wrote.
fn registry(g: &Generated) -> &str {
    let file = g
        .triggers
        .as_ref()
        .expect("a run with the second layer writes the registry");
    assert_eq!(file.path, REGISTRY_PATH);
    &file.contents
}

/// The rows of a registry: every bullet below its first `##`.
fn registry_rows(text: &str) -> Vec<&str> {
    text.lines()
        .skip_while(|l| !l.starts_with("## "))
        .filter(|l| l.starts_with("- "))
        .collect()
}

/// The link from the registry to the card of the type `ty`.
fn card_link(g: &Generated, ty: &NodeKey) -> String {
    relative(REGISTRY_PATH, &card_path(g, ty))
}

/// AE6, R15, KTD11: without a component for XAML the registry carries, above
/// its rows, the line of coverage — C# with the event, the key, the menu item
/// and the entry from another component, XAML not covered — in the run's
/// language.
#[test]
fn the_registry_without_a_xaml_component_says_xaml_is_not_covered_above_its_rows() {
    for (lang, line) in [
        (
            "cs",
            "**Pokrytí:** C#: událost, klávesa, položka menu, vstup z jiné komponenty; XAML: \
             nepokryto.\n",
        ),
        (
            "en",
            "**Coverage:** C#: event, key, menu item, entry from another component; XAML: not \
             covered.\n",
        ),
    ] {
        let g = layered(&editor_fixture::files(), 1, lang);
        let text = registry(&g);
        assert!(
            text.starts_with("---\nschema: 7\ntool: reverse-architect\nkind: triggers\n---\n"),
            "{lang}: {text}"
        );
        assert_eq!(text.matches(line).count(), 1, "{lang}: {text}");
        let at = text.find(line).unwrap();
        assert!(at < text.find("\n## ").unwrap(), "{lang}: above the rows");
    }
}

/// AE3, AE2, R11, R12: F4 of `EditorView` and F4 of `EditorViewNew` are rows
/// of their own; the old one leads to the old view model's command and never
/// to the new one, the new one's way to its command is dead, with the anchor
/// of `CanCheck()` in the same row.
#[test]
fn f4_of_each_editor_is_a_row_of_its_own_and_the_new_ones_is_dead_with_its_anchor() {
    let g = layered(&editor_fixture::files(), 1, "en");
    let rows = registry_rows(registry(&g));
    let f4 = |view: &str| -> Vec<&str> {
        let lead =
            format!("- key · `Key.F4` in `{view}.Editor_PreviewKeyDown(object,KeyEventArgs)` · ");
        rows.iter()
            .copied()
            .filter(|r| r.starts_with(&lead))
            .collect()
    };
    let (old, new) = (f4("EditorView"), f4("EditorViewNew"));
    let vm = key(EDITOR_MODELS, "EditorViewModel");
    let vm_new = key(EDITOR_MODELS, "EditorViewModelNew");
    let to_old = format!(
        " · active → [EditorViewModel.CheckCommand]({}) — `src/Views/EditorView.cs:",
        card_link(&g, &vm)
    );
    assert!(old.iter().any(|r| r.contains(&to_old)), "{old:#?}");
    assert!(
        old.iter().all(|r| !r.contains("EditorViewModelNew")),
        "{old:#?}"
    );

    let can_check = format!(
        "`src/ViewModels/EditorViewModelNew.cs:{}`",
        line_in(editor_fixture::VIEW_MODEL_NEW, "bool CanCheck()")
    );
    let to_new = format!(
        " · dead, turned off at `EditorViewModelNew.CanCheck()` → \
         [EditorViewModelNew.CheckCommand]({}) — `src/Views/EditorViewNew.cs:",
        card_link(&g, &vm_new)
    );
    let dead = new
        .iter()
        .find(|r| r.contains(&to_new))
        .unwrap_or_else(|| panic!("{new:#?}"));
    assert!(dead.ends_with(&format!(", {can_check}")), "{dead}");
    assert!(ends_in_anchor(dead), "{dead}");
    assert!(
        new.iter().all(|r| !r.contains("[EditorViewModel.")),
        "{new:#?}"
    );
}

/// R11, R12, KTD10: every row of the analysis is a row of the registry —
/// nothing merged — in the order of kind, key and place, each ending in its
/// anchor; the registry counts them, and in Czech its words are Czech.
#[test]
fn every_trigger_of_the_analysis_is_a_row_in_order_and_ends_in_its_anchor() {
    let g = layered(&editor_fixture::files(), 1, "en");
    let analysis = layer2::analyze(&g.graph, g.bodies.as_ref().unwrap());
    let text = registry(&g);
    let rows = registry_rows(text);
    assert_eq!(rows.len(), analysis.triggers.len(), "{text}");
    assert!(text.contains(&format!("**registry of triggers** · {} rows\n", rows.len())));
    assert!(rows.iter().all(|r| ends_in_anchor(r)), "{text}");
    let leads = [
        "- event · ",
        "- key · ",
        "- menu item · ",
        "- entry from another component · ",
    ];
    let kind = |r: &str| {
        leads
            .iter()
            .position(|p| r.starts_with(p))
            .unwrap_or_else(|| panic!("no kind: {r}"))
    };
    assert!(rows.windows(2).all(|w| kind(w[0]) <= kind(w[1])), "{text}");
    for (r, t) in rows.iter().zip(&analysis.triggers) {
        assert_eq!(kind(r), t.trigger.kind as usize, "{r}");
    }
    let order: Vec<(usize, &str)> = rows
        .iter()
        .zip(&analysis.triggers)
        .map(|(r, t)| (kind(r), t.trigger.key.as_str()))
        .collect();
    assert!(order.windows(2).all(|w| w[0] <= w[1]), "{order:?}");
    // The gesture `Ctrl+F4`, given to the command in C#, is a key row (AE5).
    assert!(
        rows.iter()
            .any(|r| r.starts_with("- key · `Key.F4+ModifierKeys.Control` in ")),
        "{text}"
    );

    let cs = layered(&editor_fixture::files(), 1, "cs");
    let text = registry(&cs);
    assert!(text.contains(&format!("**rejstřík spouštěčů** · {} řádků\n", rows.len())));
    let czech = registry_rows(text);
    assert!(
        czech.iter().any(|r| r.starts_with(
            "- klávesa · `Key.F4` v `EditorViewNew.Editor_PreviewKeyDown(object,KeyEventArgs)` · \
             mrtvá, vypíná ji `EditorViewModelNew.CanCheck()` → "
        )),
        "{text}"
    );
    assert!(czech
        .iter()
        .any(|r| r.starts_with("- vstup z jiné komponenty · `ExternalCheck.Run()` · aktivní → ")));
    assert!(
        text.contains("\n## Triggers\n"),
        "the headings stay English"
    );
}

/// `n` classes of one method nothing calls: `n` entries from another
/// component.
fn workers(n: usize) -> String {
    let classes: String = (0..n)
        .map(|i| {
            format!(
                "    public class AcmeWorker{i:03}\n    {{\n        public int Count;\n        \
                 public void Run() {{ Count = {i}; }}\n    }}\n"
            )
        })
        .collect();
    format!("namespace Acme.Workers\n{{\n{classes}}}\n")
}

/// R14, KTD10: a registry of more rows than a map's part holds is cut into
/// `## Part N`, none over the threshold, and its document in the manifest has
/// one chunk per part, `triggers:index#part-N`.
#[test]
fn a_large_registry_is_cut_into_parts_that_are_the_chunks_of_its_document() {
    let n = MAP_PART_ROWS + 10;
    let source = workers(n);
    let g = layered(&[("src/Workers.cs", source.as_str())], 1, "en");
    let text = registry(&g);
    let rows = registry_rows(text);
    assert!(rows.len() >= n, "{} rows:\n{text}", rows.len());
    assert!(!text.contains("\n## Triggers\n"), "{text}");
    let parts: Vec<&str> = text.split("\n## Part ").skip(1).collect();
    assert_eq!(parts.len(), rows.len().div_ceil(MAP_PART_ROWS), "{text}");
    for (i, part) in parts.iter().enumerate() {
        assert!(part.starts_with(&format!("{}\n", i + 1)), "{part}");
        let count = part.lines().filter(|l| l.starts_with("- ")).count();
        assert!(
            count > 0 && count <= MAP_PART_ROWS,
            "part {}: {count}",
            i + 1
        );
    }
    let doc = g
        .manifest
        .documents
        .iter()
        .find(|d| d.id == TRIGGERS_ID)
        .expect("the registry's document");
    let chunks: Vec<&str> = doc.chunks.iter().map(|c| c.id.as_str()).collect();
    let expected: Vec<String> = (1..=parts.len())
        .map(|i| format!("{TRIGGERS_ID}#part-{i}"))
        .collect();
    assert_eq!(chunks, expected);
    // Under the threshold: one section, one chunk.
    let small = layered(&[("src/Workers.cs", workers(3).as_str())], 1, "en");
    assert!(registry(&small).contains("\n## Triggers\n"));
    let doc = small
        .manifest
        .documents
        .iter()
        .find(|d| d.id == TRIGGERS_ID)
        .unwrap();
    assert_eq!(
        doc.chunks.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
        [format!("{TRIGGERS_ID}#triggers")]
    );
}

/// KTD10, R17: a run without `--detail-level` writes no registry, and the
/// copy a run with it left in the corpus is removed — through the binary,
/// which owns the files of the corpus.
#[test]
fn a_run_without_the_detail_level_writes_no_registry_and_removes_the_old_copy() {
    use assert_cmd::Command;
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    common::init_repo_with_files(&repo, "Alice", &editor_fixture::files());
    let out = dir.path().join("out");
    let run = |level: Option<&str>| {
        let mut command = Command::cargo_bin("reverse-architect").unwrap();
        command.arg(&repo).arg("--out").arg(&out);
        if let Some(level) = level {
            command.arg("--detail-level").arg(level);
        }
        command.assert().success();
    };
    let file = out.join("cards").join("triggers.md");
    run(Some("1"));
    let written = std::fs::read_to_string(&file).expect("the registry is written");
    assert!(written.contains("\nkind: triggers\n"), "{written}");
    run(None);
    assert!(!file.exists(), "the old copy stays");
    assert!(
        out.join("cards").join("type").is_dir(),
        "the cards stay where they are"
    );

    let plain = generate(&common::config_for(&repo, &dir.path().join("plain"))).unwrap();
    assert_eq!(plain.triggers, None);
    assert!(plain.manifest.documents.iter().all(|d| d.id != TRIGGERS_ID));
}

/// R15, KTD11: a component that declares no `emits`, or that read no file,
/// covers nothing, so its language is not covered; a language another
/// component claims is listed; a component for XAML that read files covers
/// the kinds its rows can have (step 2c, U9) and never an event — the line
/// never claims a kind no row of the registry can have.
#[test]
fn a_component_without_emits_leaves_its_language_not_covered() {
    let component = |languages: &[&str], emits: &[&str], read: bool| BodyComponent {
        languages: languages.iter().map(|s| s.to_string()).collect(),
        emits: emits.iter().map(|s| s.to_string()).collect(),
        read,
    };
    let csharp = component(
        &["csharp"],
        &[
            "member-access",
            "subscription",
            "key-compare",
            "key-gesture",
            "menu-item",
        ],
        true,
    );
    let none = coverage(&[component(&["csharp"], &[], true)]);
    assert_eq!(none.summary(Lang::En), "C#: not covered; XAML: not covered");
    assert_eq!(none.summary(Lang::Cs), "C#: nepokryto; XAML: nepokryto");
    let idle = coverage(&[component(&["csharp"], &["subscription"], false)]);
    assert_eq!(idle.summary(Lang::En), "C#: not covered; XAML: not covered");
    let other = coverage(&[csharp.clone(), component(&["probe3"], &[], true)]);
    assert_eq!(
        other.summary(Lang::En),
        "C#: event, key, menu item, entry from another component; probe3: not covered; XAML: \
         not covered"
    );
    let xaml = coverage(&[
        csharp,
        component(
            &["xaml"],
            &["subscription", "key-gesture", "control-key"],
            true,
        ),
    ]);
    assert_eq!(
        xaml.summary(Lang::En),
        "C#: event, key, menu item, entry from another component; XAML: key, menu item, control"
    );
    assert_eq!(coverage(&[]).summary(Lang::En), none.summary(Lang::En));
}

/// Plan 2c, U9 (R9, AE6, KTD11, KTD15): a component for XAML that read files
/// covers the kinds a row of XAML can have that its `emits` map to — a
/// control and a menu item by `control-key`, a key by `key-gesture` — named
/// apart, in the registry's order; never an event (a handler of XAML is a
/// marker, no row) nor an entry from another component; C# is what it was.
/// A component for XAML that emits nothing of the three, or read nothing,
/// leaves XAML not covered.
#[test]
fn a_component_for_xaml_covers_the_kinds_of_its_rows_by_its_emits() {
    let component = |languages: &[&str], emits: &[&str], read: bool| BodyComponent {
        languages: languages.iter().map(|s| s.to_string()).collect(),
        emits: emits.iter().map(|s| s.to_string()).collect(),
        read,
    };
    let csharp = component(
        &["csharp"],
        &["subscription", "key-compare", "key-gesture", "menu-item"],
        true,
    );
    let xaml = coverage(&[
        csharp.clone(),
        component(
            &["xaml"],
            &[
                "member-access",
                "subscription",
                "key-gesture",
                "control-key",
            ],
            true,
        ),
    ]);
    assert_eq!(
        xaml.summary(Lang::En),
        "C#: event, key, menu item, entry from another component; XAML: key, menu item, control"
    );
    assert_eq!(
        xaml.summary(Lang::Cs),
        "C#: událost, klávesa, položka menu, vstup z jiné komponenty; XAML: klávesa, položka \
         menu, ovládací prvek"
    );
    let keys_only = coverage(&[component(&["xaml"], &["key-gesture"], true)]);
    assert_eq!(keys_only.summary(Lang::En), "C#: not covered; XAML: key");
    for idle in [
        component(&["xaml"], &["subscription", "member-access"], true),
        component(&["xaml"], &["key-gesture", "control-key"], false),
    ] {
        assert_eq!(
            coverage(&[csharp.clone(), idle]).summary(Lang::En),
            "C#: event, key, menu item, entry from another component; XAML: not covered"
        );
    }
}

/// The menu of `SECRET_MENU`: an item whose label is a literal of a secret's
/// shape, given a command.
const SECRET_MENU: &str = r#"namespace Acme.Menus
{
    public class AcmeMenu
    {
        public int Opened;
        void BuildMenu()
        {
            ContextMenuItem open = new ContextMenuItem();
            open.Header = "ghp_Q7rT2mX9vB4nL6kP1sD8fH3jW5zC0aE";
            open.Command = OpenCommand;
        }
        public RelayCommand OpenCommand => new RelayCommand(p => Open());
        public void Open() { Opened = 1; }
    }
}
"#;

/// R18: a menu item's label of a secret's shape is not in the registry; its
/// row keeps the marker and says once, before its anchor, that a value was
/// left out — in English and in Czech.
#[test]
fn a_secret_in_a_menu_items_label_is_left_out_of_the_registry_and_admitted() {
    for (lang, lead, sentence) in [
        (
            "en",
            "- menu item · `[redacted: ",
            " (a value was left out as a likely secret; see the source) — ",
        ),
        (
            "cs",
            "- položka menu · `[redacted: ",
            " (hodnota vynechána jako pravděpodobné tajemství; viz zdroj) — ",
        ),
    ] {
        let g = layered(&[("src/Menu.cs", SECRET_MENU)], 1, lang);
        let text = registry(&g);
        assert!(
            !text.contains("ghp_Q7rT2mX9vB4nL6kP1sD8fH3jW5zC0aE"),
            "{text}"
        );
        let rows: Vec<&str> = registry_rows(text)
            .into_iter()
            .filter(|r| r.starts_with(lead))
            .collect();
        assert!(!rows.is_empty(), "{lang}: no menu item row:\n{text}");
        for row in rows {
            assert_eq!(row.matches(sentence).count(), 1, "{lang}: {row}");
            assert!(ends_in_anchor(row), "{lang}: {row}");
        }
    }
}

/// R17, KTD13: two runs give the same registry and the same view
/// `entry-points`, byte for byte.
#[test]
fn two_runs_give_the_same_registry() {
    let one = layered(&editor_fixture::files(), 2, "en");
    let two = layered(&editor_fixture::files(), 2, "en");
    assert_eq!(one.triggers, two.triggers);
    assert_eq!(one.corpus, two.corpus);
    assert!(!registry_rows(registry(&one)).is_empty());
}

/// U6 with U7: the card links the registry, relative to the card, where it
/// named it as text before the file existed — and the link resolves.
#[test]
fn the_card_links_the_registry_it_names() {
    let g = layered(&editor_fixture::files(), 2, "en");
    let command = key(EDITOR_MODELS, "RelayCommand");
    let card = card_text(&g, &command);
    let link = format!(
        "[{REGISTRY_PATH}]({})",
        relative(&card_path(&g, &command), REGISTRY_PATH)
    );
    assert!(
        card.contains(&format!(
            "the registry of triggers ({link}) lists every one."
        )),
        "{card}"
    );
    assert!(!card.contains("(`cards/triggers.md`)"), "{card}");
    assert_eq!(g.triggers.as_ref().unwrap().path, REGISTRY_PATH);
}

// ---- U8: the line of coverage in `entry-points.md` ----

/// A run over `repo` into `out`, with the second layer at `level` or without.
fn run_over(repo: &Path, out: &Path, level: Option<u8>) -> Generated {
    generate(&Config {
        detail_level: level,
        ..common::config_for(repo, out)
    })
    .unwrap()
}

fn view<'g>(g: &'g Generated, path: &str) -> &'g str {
    &g.corpus
        .files
        .iter()
        .find(|f| f.path == path)
        .unwrap_or_else(|| panic!("no {path}"))
        .contents
}

/// AE6, AE7, KTD12, R15: with the second layer the view `entry-points` has
/// the line of coverage, in English, linking the registry, right above the
/// sentence E9, which is unchanged; without the line the view is the plain
/// run's to the byte, and so is every other view.
#[test]
fn the_entry_points_view_carries_the_line_of_coverage_right_above_e9() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    common::init_repo_with_files(&repo, "Alice", &editor_fixture::files());
    let plain = run_over(&repo, &dir.path().join("plain"), None);
    let line = "- Coverage of triggers by the second layer, by language — C#: event, key, menu \
                item, entry from another component; XAML: not covered. The [registry of \
                triggers](cards/triggers.md) lists every trigger it found.\n";
    let e9 = view(&plain, "entry-points.md")
        .lines()
        .find(|l| l.starts_with("- Framework-level entry points — "))
        .expect("the sentence E9")
        .to_string();
    for (lang, level) in [("en", 1), ("cs", 2)] {
        let layered = generate(&Config {
            lang: lang.to_string(),
            detail_level: Some(level),
            ..common::config_for(&repo, &dir.path().join(format!("layered-{lang}")))
        })
        .unwrap();
        let entry = view(&layered, "entry-points.md");
        assert!(
            entry.contains(&format!("{line}{e9}\n")),
            "{lang}: the line right above E9:\n{entry}"
        );
        assert_eq!(entry.matches(line).count(), 1, "{lang}");
        assert_eq!(
            entry.replacen(line, "", 1),
            view(&plain, "entry-points.md"),
            "{lang}"
        );
        for f in &plain.corpus.files {
            if f.path != "entry-points.md" {
                assert_eq!(view(&layered, &f.path), f.contents, "{lang}: {}", f.path);
            }
        }
    }
    assert!(!view(&plain, "entry-points.md").contains("Coverage of triggers"));
}

// ---- U9: the acceptance examples over the output of a run ----
//
// AE → test: the tests above, which hold the examples over the analysis or a
// part of the output, and these, which hold the rest over what a run writes
// (the card, the registry, `entry-points`).
//
// AE1  the_f4_flow_of_the_old_editor_is_drawn_once_and_the_menu_item_refers_to_it
// AE2  the_new_editors_check_has_the_dead_f4_on_level_1_and_its_steps_on_level_2,
//      f4_of_each_editor_is_a_row_of_its_own_and_the_new_ones_is_dead_with_its_anchor
// AE3  f4_of_each_editor_is_a_row_of_its_own_and_the_new_ones_is_dead_with_its_anchor,
//      the_f4_flow_never_leads_to_both_view_models
// AE4  ae4_the_external_check_is_an_entry_row_and_a_flow_of_its_own_in_the_output
// AE5  ae5_a_key_gesture_is_a_registry_row_anchored_at_the_gesture
// AE6  the_registry_without_a_xaml_component_says_xaml_is_not_covered_above_its_rows,
//      the_entry_points_view_carries_the_line_of_coverage_right_above_e9
// AE7  the_entry_points_view_carries_the_line_of_coverage_right_above_e9,
//      ae7_the_second_layer_is_the_same_bytes_twice_and_without_it_nothing_is_added
//      (and `layer2.rs`: the pinned fingerprint of a plain run)
// AE8  the_new_editors_check_has_the_dead_f4_on_level_1_and_its_steps_on_level_2
// AE9  the_command_class_has_one_flow_and_the_view_refers_to_the_check
// AE10 ae10_the_card_tells_the_results_of_the_check_and_who_reads_its_state

/// AE4: `ExternalCheck.Run()` is a row "entry from another component" of the
/// registry, with the anchor of the place it is declared at, and on the card
/// of `EditorViewModel` a flow of its own: it enters at `Check(bool)`, with
/// no F4 condition, no CanExecute and no branch on Shift.
#[test]
fn ae4_the_external_check_is_an_entry_row_and_a_flow_of_its_own_in_the_output() {
    let g = layered(&editor_fixture::files(), 2, "en");
    let rows = registry_rows(registry(&g));
    let vm = key(EDITOR_MODELS, "EditorViewModel");
    let to_check = format!(
        "- entry from another component · `ExternalCheck.Run()` · active → \
         [EditorViewModel.CheckCommand]({}) — `src/ViewModels/EditorViewModel.cs:",
        card_link(&g, &vm)
    );
    let row = rows
        .iter()
        .find(|r| r.starts_with(&to_check))
        .unwrap_or_else(|| panic!("{rows:#?}"));
    assert!(ends_in_anchor(row), "{row}");

    let context = editor_context(&g, &vm, &key(EDITOR_MODELS, "EditorViewModel.Check(bool)"));
    let flow = flow_block(
        &context,
        "Entry from another component: ExternalCheck.Run()",
    );
    let trigger = flow
        .lines()
        .find(|l| l.starts_with("- Trigger: entry from another component `ExternalCheck.Run()`"))
        .unwrap_or_else(|| panic!("{flow}"));
    assert!(ends_in_anchor(trigger), "{trigger}");
    assert!(
        flow.lines().any(|l| l
            .starts_with("- Step: `ExternalCheck.Run()` → `Check(bool)` (card [ExternalCheck](")),
        "{flow}"
    );
    for absent in [
        "Key.F4",
        "CanExecute",
        "ModifierKeys.Shift",
        "RelayCommand",
        "From `CheckCommand` on",
    ] {
        assert!(!flow.contains(absent), "{absent} in:\n{flow}");
    }
    assert!(flow.contains("- Result: `Check(bool)` writes `CheckWasDone` = `true`"));
    // The flow of F4 is another block of the same context.
    assert!(flow_block(&context, "Key: Key.F4 — EditorView").contains("ModifierKeys.Shift"));
}

/// AE5: `new KeyGesture(Key.F4, ModifierKeys.Control)` given to the command
/// is a registry row "key Ctrl+F4" leading to the command of closing the
/// errors, anchored at the assignment of the gesture.
#[test]
fn ae5_a_key_gesture_is_a_registry_row_anchored_at_the_gesture() {
    let g = layered(&editor_fixture::files(), 1, "en");
    let rows = registry_rows(registry(&g));
    let vm = key(EDITOR_MODELS, "EditorViewModel");
    let line = line_in(
        editor_fixture::VIEW_MODEL,
        "closeErrorsCommand.KeyGestures =",
    );
    let expected = format!(
        "- key · `Key.F4+ModifierKeys.Control` in `EditorViewModel.CloseErrorsCommand` · active → \
         [EditorViewModel.CloseErrorsCommand]({}) — `src/ViewModels/EditorViewModel.cs:{line}`",
        card_link(&g, &vm)
    );
    assert!(rows.contains(&expected.as_str()), "{rows:#?}");
    // The gesture is a row of its own, not a second F4 of a view.
    assert!(rows
        .iter()
        .filter(|r| r.contains("`Key.F4+ModifierKeys.Control`"))
        .all(|r| r.ends_with(&format!(":{line}`"))));
}

/// AE10: the card of `EditorViewModel` tells, under the flow of F4, the
/// writes of the properties that notify their change, `CheckWasDone = true`
/// at the end, and `CheckWasDone = false` on each of the three silent
/// returns, read by `ExternalCheck` — and the context of the state names
/// `ExternalCheck` as a reader and the save as a writer, never as a reader.
#[test]
fn ae10_the_card_tells_the_results_of_the_check_and_who_reads_its_state() {
    let g = layered(&editor_fixture::files(), 2, "en");
    let vm = key(EDITOR_MODELS, "EditorViewModel");
    let context = editor_context(&g, &vm, &key(EDITOR_MODELS, "EditorViewModel.Check(bool)"));
    let f4 = flow_block(&context, "Key: Key.F4 — EditorView");
    for property in ["Errors", "ShowErrors", "MessageErrors", "SelectedIndex"] {
        let start = format!("- Result: `Check(bool)` writes `{property}`");
        let line = f4
            .lines()
            .find(|l| l.starts_with(&start))
            .unwrap_or_else(|| panic!("{start}\n{f4}"));
        assert!(
            line.contains(", which notifies its change") && ends_in_anchor(line),
            "{line}"
        );
    }
    let done = |value: &str| {
        let start = format!("- Result: `Check(bool)` writes `CheckWasDone` = `{value}`, read by ");
        let line = f4
            .lines()
            .find(|l| l.starts_with(&start))
            .unwrap_or_else(|| panic!("{start}\n{f4}"));
        assert!(
            line.starts_with(&format!("{start}`ExternalCheck.Run()` — ")) && ends_in_anchor(line),
            "{line}"
        );
    };
    done("false");
    done("true");
    // The three silent returns: the guard of each says that the reset of the
    // state still happens.
    for guard in [
        "!canErrors",
        "syntax != SyntaxKind.Sql && syntax != SyntaxKind.Script",
        "Text == null",
    ] {
        let start =
            format!("- Precondition: does not run when `{guard}` (a guard in `Check(bool)`); ");
        let line = f4
            .lines()
            .find(|l| l.starts_with(&start))
            .unwrap_or_else(|| panic!("{start}\n{f4}"));
        assert!(
            line.contains("still happens: `Check(bool)` writes `CheckWasDone` = `false`")
                && ends_in_anchor(line),
            "{line}"
        );
    }
    assert!(!f4.contains("BeforeSave"), "{f4}");

    let card = card_text(&g, &vm);
    let state = part(
        part(card, "## State: EditorViewModel.CheckWasDone"),
        "### Context",
    );
    assert!(
        state.contains("\n- Read by `ExternalCheck.Run()` — "),
        "{state}"
    );
    assert!(
        state.contains("\n- Written by `EditorView.BeforeSave()` — "),
        "{state}"
    );
    assert!(
        !state
            .lines()
            .any(|l| l.starts_with("- Read by ") && l.contains("BeforeSave")),
        "{state}"
    );
}

/// AE7, KTD13: without `--detail-level` a run adds nothing of the second
/// layer — no registry, no context — and is the same bytes twice; with the
/// second layer, two runs on separate directories write the same files, the
/// same manifest and the same registry.
#[test]
fn ae7_the_second_layer_is_the_same_bytes_twice_and_without_it_nothing_is_added() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    common::init_repo_with_files(&repo, "Alice", &editor_fixture::files());
    let plain: Vec<Generated> = (0..2)
        .map(|n| run_over(&repo, &dir.path().join(format!("plain-{n}")), None))
        .collect();
    assert!(plain.iter().all(|g| g.triggers.is_none()));
    assert!(plain
        .iter()
        .all(|g| g.cards.iter().all(|c| !c.contents.contains("### Context"))));
    assert_eq!(plain[0].corpus, plain[1].corpus);
    assert_eq!(plain[0].cards, plain[1].cards);
    assert_eq!(plain[0].manifest, plain[1].manifest);
    assert!(plain[0]
        .corpus
        .files
        .iter()
        .all(|f| !f.contents.contains("Coverage of triggers")));

    let layered_runs: Vec<Generated> = (0..2)
        .map(|n| run_over(&repo, &dir.path().join(format!("layered-{n}")), Some(2)))
        .collect();
    let (a, b) = (&layered_runs[0], &layered_runs[1]);
    assert!(a.triggers.is_some());
    assert_eq!(a.corpus, b.corpus);
    assert_eq!(a.cards, b.cards);
    assert_eq!(a.details, b.details);
    assert_eq!(a.maps, b.maps);
    assert_eq!(a.triggers, b.triggers);
    assert_eq!(a.manifest, b.manifest);
    assert_eq!(a.coverage, b.coverage);
}

// ---- The nearest functionality (plan 2b, risk table of KTD5; R-41) ----

const RELAY_NS: &str = "sym:csharp:Acme.Relay#";

/// `callers` classes each calling a shared utility `Util1` from a public
/// method nothing calls, and the utility a chain of `depth` utilities, each
/// a type of its own: every caller's entry reaches its own functionality and
/// every utility's, `callers × (depth + 1)` rows for `callers` occurrences.
fn chain(callers: usize, depth: usize) -> String {
    let mut s = String::from("namespace Acme.Relay\n{\n");
    for k in 1..depth {
        s.push_str(&format!(
            "    public class Util{k}\n    {{\n        private Util{} next;\n        public void Run() {{ next.Run(); }}\n    }}\n\n",
            k + 1
        ));
    }
    s.push_str(&format!(
        "    public class Util{depth}\n    {{\n        public int Count;\n        public void Run() {{ Count++; }}\n    }}\n"
    ));
    for i in 1..=callers {
        s.push_str(&format!(
            "\n    public class Caller{i}\n    {{\n        private Util1 util;\n        public void Go() {{ util.Run(); }}\n    }}\n"
        ));
    }
    s.push_str("}\n");
    s
}

fn util(k: usize) -> NodeKey {
    key(RELAY_NS, &format!("Util{k}"))
}

fn run_of(k: usize) -> NodeKey {
    key(RELAY_NS, &format!("Util{k}.Run()"))
}

fn go(i: usize) -> NodeKey {
    key(RELAY_NS, &format!("Caller{i}.Go()"))
}

/// The registry's text of a run.
fn registry_text(g: &Generated) -> &str {
    &g.triggers.as_ref().expect("the registry").contents
}

/// Over ten times as many rows as occurrences: the walk is cut at the
/// functionality nearest each trigger. The shared utility keeps a row per
/// caller, the utilities past it keep none and say where the walk stopped,
/// every occurrence keeps a row, and the registry and coverage admit it —
/// the same bytes twice.
#[test]
fn a_registry_over_ten_times_its_triggers_stops_at_the_nearest_functionality() {
    let (callers, depth) = (3, 11);
    let src = chain(callers, depth);
    let files = [("src/Relay.cs", src.as_str())];
    let (_, analysis) = analyzed(&files);

    // The premise: the full walk is over the ratio.
    let walk = analysis.walk;
    assert_eq!(walk.occurrences, callers);
    assert_eq!(walk.transitive_rows, callers * (depth + 1));
    assert!(walk.transitive_rows > layer2::NEAREST_RATIO * walk.occurrences);
    assert!(walk.nearest);
    assert_eq!(walk.rows, analysis.triggers.len());

    // Every occurrence keeps a row; each caller's own and the utility's.
    let members: std::collections::BTreeSet<&NodeKey> = analysis
        .triggers
        .iter()
        .map(|r| &r.trigger.member)
        .collect();
    let callers_go: Vec<NodeKey> = (1..=callers).map(go).collect();
    assert_eq!(members, callers_go.iter().collect());
    assert_eq!(walk.rows, 2 * callers);
    let first = holding(&analysis, &util(1), &run_of(1));
    let rows = rows_to(&analysis, &util(1), first);
    let found: Vec<(TriggerKind, &NodeKey, Vec<&NodeKey>)> = rows
        .iter()
        .map(|(_, r)| {
            (
                r.trigger.kind,
                &r.trigger.member,
                r.path.iter().map(|h| &h.member).collect(),
            )
        })
        .collect();
    let expected: Vec<(TriggerKind, &NodeKey, Vec<&NodeKey>)> = callers_go
        .iter()
        .map(|g| (TriggerKind::External, g, vec![g]))
        .collect();
    assert_eq!(found, expected);
    assert!(context_of(&analysis, &util(1), first).narrowed.is_none());

    // Past the utility: no row, and the functionality says where it stopped.
    for k in 2..=depth {
        let f = holding(&analysis, &util(k), &run_of(k));
        assert!(rows_to(&analysis, &util(k), f).is_empty(), "Util{k}");
        let narrowed = context_of(&analysis, &util(k), f)
            .narrowed
            .as_ref()
            .unwrap_or_else(|| panic!("Util{k} says nothing of the walk"));
        assert_eq!(narrowed.at, [target_of(&util(1), first)]);
        assert_eq!(narrowed.total, 1);
    }

    // The registry, coverage and the card admit it.
    let g = layered(&files, 2, "en");
    let text = registry_text(&g);
    assert!(
        text.contains(&format!(
            "The walk back stopped at the nearest functionality: it gave {} rows for {callers} \
             occurrences of triggers",
            callers * (depth + 1)
        )),
        "{text}"
    );
    assert!(text.contains("without its way passing through a functionality of another type"));
    assert!(!text.contains("each functionality it leads to; nothing is merged"));
    let restricted = g
        .coverage
        .bodies
        .as_ref()
        .and_then(|b| b.restricted_walk.as_ref())
        .expect("coverage admits the cut");
    assert_eq!(
        (
            restricted.transitive_rows,
            restricted.trigger_occurrences,
            restricted.rows
        ),
        (
            (callers * (depth + 1)) as u64,
            callers as u64,
            (2 * callers) as u64
        )
    );
    let json = serde_json::to_string(&g.coverage).unwrap();
    assert!(json.contains("\"restricted_walk\""));
    let card = card_text(&g, &util(5));
    let context = contexts(card).concat();
    assert!(
        context.contains("the walk back stops at the functionality nearest the trigger"),
        "{context}"
    );
    assert!(!context.contains("\n#### "), "{context}");

    // Deterministic: the same analysis and the same bytes again.
    let (_, again) = analyzed(&files);
    assert_eq!(analysis, again);
    let h = layered(&files, 2, "en");
    assert_eq!(g.triggers, h.triggers);
    assert_eq!(g.cards, h.cards);
    assert_eq!(g.coverage, h.coverage);
}

/// Ten times as many rows as occurrences, not more: the walk stays
/// transitive and nothing says otherwise — every utility keeps every caller's
/// row, no functionality is narrowed, and neither the registry nor coverage
/// mentions a cut.
#[test]
fn a_registry_of_ten_times_its_triggers_stays_transitive() {
    let (callers, depth) = (3, 9);
    let src = chain(callers, depth);
    let files = [("src/Relay.cs", src.as_str())];
    let (_, analysis) = analyzed(&files);
    let walk = analysis.walk;
    assert_eq!(walk.occurrences, callers);
    assert_eq!(
        walk.transitive_rows,
        layer2::NEAREST_RATIO * walk.occurrences
    );
    assert!(!walk.nearest);
    assert_eq!(walk.rows, walk.transitive_rows);
    assert_eq!(analysis.triggers.len(), walk.transitive_rows);
    for k in 1..=depth {
        let f = holding(&analysis, &util(k), &run_of(k));
        let members: Vec<&NodeKey> = rows_to(&analysis, &util(k), f)
            .iter()
            .map(|(_, r)| &r.trigger.member)
            .collect();
        let expected: Vec<NodeKey> = (1..=callers).map(go).collect();
        assert_eq!(members, expected.iter().collect::<Vec<_>>(), "Util{k}");
    }
    assert!(analysis
        .types
        .iter()
        .flat_map(|t| &t.context.functionalities)
        .all(|f| f.narrowed.is_none()));

    let g = layered(&files, 2, "en");
    let text = registry_text(&g);
    assert!(!text.contains("walk back stopped"), "{text}");
    assert!(text.contains(
        "One row per occurrence of a trigger and each functionality it leads to; nothing is \
         merged. A row gives"
    ));
    assert!(g
        .coverage
        .bodies
        .as_ref()
        .is_some_and(|b| b.restricted_walk.is_none()));
    let json = serde_json::to_string(&g.coverage).unwrap();
    assert!(!json.contains("restricted_walk"));
    assert!(g
        .cards
        .iter()
        .all(|c| !c.contents.contains("walk back stops")));
}
