//! Step 2b, U2 and U3: the links of an access graded across types, and the
//! binding of a delegate from the code of the class constructed — over the
//! same walk and the same binding the entry points of plan 2a resolve by, so
//! the analysis of a card and its flow never disagree.

mod common;

use reverse_architect::card::layer2::{
    self, bindings_of, raised_events, Analysis, Basis, DelegateBinding, EntryPoint, Index, Links,
    Outcome, Resolution, Site, TypeAnalysis, Unresolved,
};
use reverse_architect::config::{Config, DEFAULT_AMBIGUITY_CEILING};
use reverse_architect::graph::{
    Confidence, Edge, EdgeKind, ExtractorId, NodeKey, Provenance, SiteId, Tier,
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

    let outer = same("OuterView");
    assert_eq!(
        outer.reason(),
        Some(&Unresolved::NotAMember("viewModel".into()))
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
