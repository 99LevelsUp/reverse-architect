//! Step 2b, U2 and U3: the links of an access graded across types, and the
//! binding of a delegate from the code of the class constructed — over the
//! same walk and the same binding the entry points of plan 2a resolve by, so
//! the analysis of a card and its flow never disagree. U1: the reverse index
//! of callers and the class of a member. U4: the results of a functionality
//! and the state read and written from outside, over the editor of the
//! acceptance examples (`common/editor_fixture.rs`).

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
/// a strong one; past the ceiling of shown candidates too.
#[test]
fn a_place_over_the_ceiling_makes_its_candidates_weakly_called_not_roots() {
    let (generated, _) = analyzed(&[("src/Many.cs", MANY)]);
    let callers = callers_of(&generated);
    let go = key(MANY_NS, "Caller.Go()");
    for i in 0..10 {
        let ping = key(MANY_NS, &format!("A{i}.Ping()"));
        assert_eq!(callers.class_of(&ping), CallerClass::WeakOnly, "{ping:?}");
        let c = caller(&callers, &ping, &go);
        assert_eq!(c.grade, Confidence::Ambiguous);
        assert!(
            c.evidence
                .iter()
                .any(|e| matches!(&e.via, Via::OverCeiling { name, .. } if name == "Ping")),
            "{c:?}"
        );
    }
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
            callers: vec![store.clone()]
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
