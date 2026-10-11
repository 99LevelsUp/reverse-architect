//! The `bodies` pass over C# (`--features csharp-body`), read natively. The
//! host's suite runs the same code through the built component.

use super::bodies;
use crate::ra::plugin::types::{
    AccessMode, AccessPath, BodyKind, CallKind, CompareOp, Constant, DeclarationKind, Expr,
    FileBodies, FlowTarget, Handler, Invocation, LiteralContext, MemberBody, NodeKind, PathRoot,
    Position, SubscriptionMode, Tag, ValueRef,
};

fn read(src: &str) -> FileBodies {
    bodies("src/a.cs", src.as_bytes()).expect("the source is read")
}

fn path(root: PathRoot, segments: &[&str]) -> AccessPath {
    AccessPath {
        root,
        segments: segments.iter().map(|s| s.to_string()).collect(),
    }
}

/// The position of the first occurrence of `needle` on line `line`, 1-based.
fn at(src: &str, line: u32, needle: &str) -> Position {
    let text = src.lines().nth(line as usize - 1).expect("no such line");
    let byte = text
        .find(needle)
        .unwrap_or_else(|| panic!("`{needle}` not on line {line}"));
    Position {
        line,
        column: text[..byte].chars().count() as u32 + 1,
    }
}

fn member(file: &FileBodies, at: Position, kind: BodyKind) -> &MemberBody {
    file.members
        .iter()
        .find(|m| m.name_position == at && m.kind == kind)
        .unwrap_or_else(|| panic!("no {kind:?} body at {at:?}"))
}

fn tags_named<'m>(m: &'m MemberBody, name: &str) -> Vec<(u32, &'m Tag)> {
    m.nodes
        .iter()
        .flat_map(|n| n.tags.iter().map(move |t| (n.id, t)))
        .filter(|(_, t)| t.name == name)
        .collect()
}

fn call<'m>(m: &'m MemberBody, last: &str) -> &'m Invocation {
    m.invocations
        .iter()
        .find(|i| i.callee.segments.last().map(String::as_str) == Some(last))
        .unwrap_or_else(|| panic!("no call of `{last}`"))
}

/// The source the execution note asks for: an editor view with a command
/// built lazily from two lambdas, key handling with an F4 branch, a menu
/// filled with `Items.Add`, event subscriptions, and the command class
/// itself.
const EDITOR: &str = r#"namespace Acme.Views
{
    public class EditorView : ViewBase<EditorViewModel>
    {
        private RelayCommand f;
        private bool canErrors;
        public RelayCommand CheckCommand
        {
            get
            {
                if (f == null) f = new RelayCommand(p => Check(), p => canErrors);
                return f;
            }
        }
        public RelayCommand X => new RelayCommand(p => Check());
        void OnKeyDown(object sender, KeyEventArgs e)
        {
            if (e.Key == Key.F4 && (Keyboard.Modifiers & ModifierKeys.Shift) == ModifierKeys.Shift)
            {
                this.Check((Keyboard.Modifiers & ModifierKeys.Shift) == ModifierKeys.Shift);
            }
        }
        void BuildMenu(ContextMenu hotKeys)
        {
            ContextMenuItem f4 = new ContextMenuItem();
            f4.Command = this.viewModel.CheckCommand;
            f4.Header = GetResourceString("k");
            hotKeys.Items.Add(f4);
            var g = new KeyGesture(Key.F4, ModifierKeys.Control);
            var b = new KeyBinding(this.CheckCommand, Key.F5, ModifierKeys.Shift);
        }
        void Attach(Editor editor)
        {
            editor.Loaded -= OnLoaded;
            editor.Loaded += OnLoaded;
            Closed += (s, a) => Check();
        }
        void Check(bool shift = false) { }
        void OnLoaded(object s, EventArgs a) { }
    }

    public class RelayCommand
    {
        private readonly Action<object> _execute;
        private readonly Predicate<object> _canExecute;
        public RelayCommand(Action<object> execute, Predicate<object> canExecute)
            : this(execute, canExecute, null) { }
        public RelayCommand(Action<object> execute, Predicate<object> canExecute, string name)
            : base(execute, canExecute, null)
        {
            this._execute = execute;
        }
        public bool CanExecute(object p) { return this._canExecute?.Invoke(p) ?? true; }
    }
}
"#;

#[test]
fn a_lazy_getter_builds_its_command_from_two_lambdas_into_the_field() {
    let file = read(EDITOR);
    let getter = member(&file, at(EDITOR, 7, "CheckCommand"), BodyKind::Getter);
    let new = call(getter, "RelayCommand");
    assert_eq!(new.kind, CallKind::Construct);
    assert_eq!(new.flow, Some(FlowTarget::Member("f".into())));
    assert_eq!(getter.lambdas.len(), 2);
    for (i, lambda) in getter.lambdas.iter().enumerate() {
        assert_eq!(lambda.id, i as u32);
        assert_eq!(lambda.argument_of, Some((0, i as u32)));
        assert_eq!(lambda.arity, 1);
        assert_eq!(lambda.flow, Some(FlowTarget::Member("f".into())));
        assert_eq!(getter.nodes[lambda.body as usize].kind, NodeKind::Lambda);
        assert_eq!(new.args[i].value, ValueRef::Lambda(i as u32));
    }
    // `return f;` reads the field as the member's result.
    assert!(getter
        .accesses
        .iter()
        .any(|a| a.target == path(PathRoot::Name, &["f"]) && a.mode == AccessMode::Return));
    // The field is the one initialized on first read.
    let field = file
        .metas
        .iter()
        .find(|m| m.name_position == at(EDITOR, 5, "f;"))
        .expect("the field has a meta");
    assert_eq!(field.modifiers, ["private"]);
    assert!(field.tags.contains(&Tag {
        name: "lazy-init".into(),
        value: Some("CheckCommand".into()),
        args: Vec::new(),
    }));
}

#[test]
fn an_expression_bodied_property_returns_its_construction() {
    let file = read(EDITOR);
    let x = member(&file, at(EDITOR, 15, "X =>"), BodyKind::ExpressionProperty);
    let new = call(x, "RelayCommand");
    assert_eq!(new.flow, Some(FlowTarget::MemberReturn));
    assert_eq!(x.lambdas[0].flow, Some(FlowTarget::MemberReturn));
    assert_eq!(x.nodes[1].kind, NodeKind::Return);
}

#[test]
fn a_command_stores_its_parameter_and_invokes_its_delegate_conditionally() {
    let file = read(EDITOR);
    let ctor = member(
        &file,
        at(EDITOR, 48, "RelayCommand("),
        BodyKind::Constructor,
    );
    let write = ctor
        .accesses
        .iter()
        .find(|a| a.mode == AccessMode::Write)
        .expect("a write");
    assert_eq!(write.target, path(PathRoot::This, &["_execute"]));
    assert_eq!(write.rhs, Some(ValueRef::Param(0)));

    let can = member(&file, at(EDITOR, 53, "CanExecute"), BodyKind::Method);
    let invoke = call(can, "Invoke");
    assert!(invoke.conditional);
    assert_eq!(invoke.callee.root, PathRoot::This);
    assert_eq!(invoke.callee.segments, ["_canExecute", "Invoke"]);
    assert_eq!(invoke.args[0].value, ValueRef::Param(0));
    assert!(can.accesses.iter().any(
        |a| a.mode == AccessMode::Invoke && a.target == path(PathRoot::This, &["_canExecute"])
    ));
}

#[test]
fn constructor_initializers_pass_parameters_and_a_null_constant() {
    let file = read(EDITOR);
    for (line, kind, root) in [
        (46, CallKind::ThisInit, PathRoot::This),
        (48, CallKind::BaseInit, PathRoot::Base),
    ] {
        let ctor = member(
            &file,
            at(EDITOR, line, "RelayCommand("),
            BodyKind::Constructor,
        );
        let init = ctor
            .invocations
            .iter()
            .find(|i| i.kind == kind)
            .unwrap_or_else(|| panic!("no {kind:?}"));
        assert_eq!(init.callee, path(root, &[]));
        assert_eq!(init.arity, 3);
        let positions: Vec<u32> = init.args.iter().map(|a| a.position).collect();
        assert_eq!(positions, [0, 1, 2]);
        let values: Vec<&ValueRef> = init.args.iter().map(|a| &a.value).collect();
        assert_eq!(
            values,
            [
                &ValueRef::Param(0),
                &ValueRef::Param(1),
                &ValueRef::Constant(Constant::Null)
            ]
        );
    }
}

#[test]
fn an_argument_names_an_enum_member_and_reads_a_static_property() {
    let file = read(EDITOR);
    let on_key = member(&file, at(EDITOR, 16, "OnKeyDown"), BodyKind::Method);
    let check = call(on_key, "Check");
    assert_eq!(check.callee, path(PathRoot::This, &["Check"]));
    let arg = &check.args[0];
    assert_eq!(arg.position, 0);
    assert!(arg.enum_refs.iter().any(|r| r == "ModifierKeys.Shift"));
    assert!(!arg.enum_refs.iter().any(|r| r == "Keyboard.Modifiers"));
    assert!(arg
        .reads
        .contains(&path(PathRoot::Name, &["Keyboard", "Modifiers"])));
}

#[test]
fn an_arm_comparing_a_key_carries_key_compare_and_a_structured_condition() {
    let file = read(EDITOR);
    let on_key = member(&file, at(EDITOR, 16, "OnKeyDown"), BodyKind::Method);
    let tags = tags_named(on_key, "key-compare");
    assert_eq!(tags.len(), 1);
    let (arm, t) = tags[0];
    assert_eq!(on_key.nodes[arm as usize].kind, NodeKind::Arm);
    assert_eq!(t.value.as_deref(), Some("Key.F4"));
    assert_eq!(t.args, ["ModifierKeys.Shift"]);
    let condition = on_key.nodes[arm as usize]
        .condition
        .as_ref()
        .expect("the arm has a condition");
    let root = condition.expr.last().expect("a tree");
    assert!(matches!(root, Expr::And(operands) if operands.len() == 2));
    assert!(condition.expr.contains(&Expr::EnumMember("Key.F4".into())));
    assert!(condition
        .expr
        .iter()
        .any(|e| matches!(e, Expr::Compare((CompareOp::Eq, _, _)))));
    assert!(condition.enum_refs.iter().any(|r| r == "Key.F4"));
    assert!(condition
        .reads
        .contains(&path(PathRoot::Local, &["e", "Key"])));
    assert!(condition
        .reads
        .contains(&path(PathRoot::Name, &["Keyboard", "Modifiers"])));
}

#[test]
fn a_menu_item_is_constructed_given_a_command_and_a_header_and_added() {
    let file = read(EDITOR);
    let menu = member(&file, at(EDITOR, 23, "BuildMenu"), BodyKind::Method);
    let tags = tags_named(menu, "menu-item");
    assert_eq!(tags.len(), 1);
    assert_eq!(tags[0].1.value.as_deref(), Some("f4"));
    assert_eq!(tags[0].1.args, ["ContextMenuItem"]);
    assert_eq!(menu.nodes[tags[0].0 as usize].kind, NodeKind::Statement);

    let command = menu
        .accesses
        .iter()
        .find(|a| a.target == path(PathRoot::Local, &["f4", "Command"]))
        .expect("the command is written");
    assert_eq!(command.mode, AccessMode::Write);
    assert_eq!(
        command.rhs,
        Some(ValueRef::Access(path(
            PathRoot::This,
            &["viewModel", "CheckCommand"]
        )))
    );
    let k = menu
        .literals
        .iter()
        .find(|l| l.value == "k")
        .expect("the resource key");
    assert_eq!(
        k.context,
        LiteralContext::CallArg((path(PathRoot::Name, &["GetResourceString"]), 0))
    );

    let add = call(menu, "Add");
    assert_eq!(add.args[0].position, 0);
    assert_eq!(
        add.args[0].value,
        ValueRef::Access(path(PathRoot::Local, &["f4"]))
    );
    assert!(menu.accesses.iter().any(|a| a.mode == AccessMode::Mutate
        && a.target == path(PathRoot::Local, &["hotKeys", "Items"])));
}

#[test]
fn an_object_given_a_command_and_a_header_is_a_menu_item_too() {
    let src = "class C {\n    void M(ICommand c) {\n        var entry = new Entry { Command = c, Header = \"x\" };\n        var plain = new Entry { Command = c };\n    }\n}\n";
    let file = read(src);
    let m = member(&file, at(src, 2, "M("), BodyKind::Method);
    let tags = tags_named(m, "menu-item");
    assert_eq!(tags.len(), 1);
    assert_eq!(tags[0].1.value.as_deref(), Some("entry"));
    assert!(m.accesses.iter().any(|a| a.mode == AccessMode::Write
        && a.target == path(PathRoot::Local, &["entry", "Command"])
        && a.rhs == Some(ValueRef::Param(0))));
}

#[test]
fn a_constructed_key_gesture_and_key_binding_carry_their_key_and_modifiers() {
    let file = read(EDITOR);
    let menu = member(&file, at(EDITOR, 23, "BuildMenu"), BodyKind::Method);
    let tags = tags_named(menu, "key-gesture");
    let found: Vec<(Option<&str>, &[String])> = tags
        .iter()
        .map(|(_, t)| (t.value.as_deref(), t.args.as_slice()))
        .collect();
    assert_eq!(
        found,
        [
            (Some("Key.F4"), &["ModifierKeys.Control".to_string()][..]),
            (Some("Key.F5"), &["ModifierKeys.Shift".to_string()][..]),
        ]
    );
}

#[test]
fn subscriptions_name_their_event_handler_and_mode() {
    let file = read(EDITOR);
    let attach = member(&file, at(EDITOR, 32, "Attach"), BodyKind::Method);
    let s = &attach.subscriptions;
    assert_eq!(s.len(), 3);
    assert_eq!(s[0].mode, SubscriptionMode::Remove);
    assert_eq!(s[1].mode, SubscriptionMode::Add);
    assert_eq!(s[1].source, Some(path(PathRoot::Local, &["editor"])));
    assert_eq!(s[1].event, "Loaded");
    assert_eq!(
        s[1].handler,
        Handler::MethodGroup(path(PathRoot::Name, &["OnLoaded"]))
    );
    assert_eq!(s[2].source, None);
    assert_eq!(s[2].event, "Closed");
    assert_eq!(s[2].handler, Handler::Lambda(0));
    assert_eq!(attach.lambdas[0].arity, 2);
}

#[test]
fn arithmetic_on_a_property_is_not_a_subscription() {
    let src = "class C {\n    int Total { get; set; }\n    int Price { get; set; }\n    void M() { Total += Price; }\n}\n";
    let file = read(src);
    let m = member(&file, at(src, 4, "M("), BodyKind::Method);
    assert!(m.subscriptions.is_empty());
    assert!(m
        .accesses
        .iter()
        .any(|a| a.mode == AccessMode::Write && a.target == path(PathRoot::Name, &["Total"])));
}

#[test]
fn a_setter_raising_property_changed_marks_its_property() {
    let src = r#"class EditorViewModel {
    private string title;
    public string Title
    {
        get { return title; }
        set { if (value == title) return; title = value; OnPropertyChanged(); }
    }
    public string Plain { get; set; }
}
"#;
    let file = read(src);
    let title = file
        .metas
        .iter()
        .find(|m| m.name_position == at(src, 3, "Title"))
        .expect("a meta");
    assert!(title.tags.iter().any(|t| t.name == "notifies-change"));
    let plain = file
        .metas
        .iter()
        .find(|m| m.name_position == at(src, 8, "Plain"))
        .expect("a meta");
    assert!(plain.tags.is_empty());
    let setter = member(&file, at(src, 3, "Title"), BodyKind::Setter);
    assert_eq!(tags_named(setter, "guard-return").len(), 1);
    assert!(setter
        .accesses
        .iter()
        .any(|a| a.target == path(PathRoot::Name, &["title"])
            && a.rhs == Some(ValueRef::Access(path(PathRoot::Local, &["value"])))));
}

#[test]
fn loops_carry_their_idioms() {
    let src = r#"class C {
    int M(List<EditorView> views) {
        foreach (var v in views) v.Refresh();
        foreach (var v in views) { if (v.Ok) return 1; }
        int total = 0;
        foreach (var v in views) total += v.Count;
        return total;
    }
}
"#;
    let file = read(src);
    let m = member(&file, at(src, 2, "M("), BodyKind::Method);
    let loops: Vec<Vec<&str>> = m
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::Loop)
        .map(|n| n.tags.iter().map(|t| t.name.as_str()).collect())
        .collect();
    assert_eq!(
        loops,
        [
            vec!["full-traversal"],
            vec!["find-first"],
            vec!["full-traversal", "accumulate"],
        ]
    );
}

#[test]
fn a_type_states_its_base_with_type_arguments_and_its_own_type_parameters() {
    let src = "public class EditorView : ViewBase<EditorViewModel> { }\nclass ViewBase<T> { protected T viewModel; }\n";
    let file = read(src);
    let view = file
        .types
        .iter()
        .find(|t| t.name_position == at(src, 1, "EditorView"))
        .expect("the view");
    assert_eq!(view.bases.len(), 1);
    assert_eq!(view.bases[0].base, ["ViewBase"]);
    assert_eq!(view.bases[0].type_args, ["EditorViewModel"]);
    let base = file
        .types
        .iter()
        .find(|t| t.name_position == at(src, 2, "ViewBase"))
        .expect("the base");
    assert_eq!(base.type_params, ["T"]);
    let field = file
        .typed_members
        .iter()
        .find(|t| t.name == "viewModel")
        .expect("the field");
    assert_eq!(field.type_name, "T");
    assert_eq!(field.kind, DeclarationKind::Field);
    assert_eq!(field.position, at(src, 2, "viewModel"));
}

#[test]
fn an_expression_body_and_a_block_returning_the_same_give_the_same_tree() {
    let src =
        "class C {\n    bool CanCheck() => false;\n    bool CanCheck2() { return false; }\n}\n";
    let file = read(src);
    let shape = |m: &MemberBody| {
        m.nodes
            .iter()
            .map(|n| (n.kind.clone(), n.parent, n.returns.clone(), n.tags.clone()))
            .collect::<Vec<_>>()
    };
    let arrow = member(&file, at(src, 2, "CanCheck"), BodyKind::Method);
    let block = member(&file, at(src, 3, "CanCheck2"), BodyKind::Method);
    assert_eq!(shape(arrow), shape(block));
    assert_eq!(arrow.nodes.len(), 2);
    assert_eq!(arrow.nodes[0].kind, NodeKind::Body);
    assert_eq!(arrow.nodes[1].kind, NodeKind::Return);
    assert_eq!(arrow.nodes[1].returns, Some(Constant::Boolean(false)));
}

/// The shape of a condition's tree: `and(or(compare,compare),path)`.
fn tree_shape(expr: &[Expr]) -> String {
    fn node(expr: &[Expr], i: u32) -> String {
        let list = |name: &str, xs: &[u32]| {
            let parts: Vec<String> = xs.iter().map(|&j| node(expr, j)).collect();
            format!("{name}({})", parts.join(","))
        };
        match &expr[i as usize] {
            Expr::And(xs) => list("and", xs),
            Expr::Or(xs) => list("or", xs),
            Expr::Not(x) => list("not", &[*x]),
            Expr::Compare(_) => "compare".to_string(),
            Expr::Path(_) => "path".to_string(),
            Expr::EnumMember(_) => "enum-member".to_string(),
            Expr::Constant(_) => "constant".to_string(),
            Expr::Call(_) => "call".to_string(),
            _ => "other".to_string(),
        }
    }
    match expr.len() {
        0 => String::new(),
        n => node(expr, n as u32 - 1),
    }
}

/// A predicate written as a lambda's expression body carries its tree on the
/// lambda's `return` node, its enum members and reads beside it.
#[test]
fn a_lambda_returning_a_predicate_carries_its_tree() {
    let src = "class C {\n    X.Kind a;\n    bool b;\n    void M() { Run(p => (a == X.Y || a == X.Z) && b); }\n}\n";
    let file = read(src);
    let m = member(&file, at(src, 4, "M"), BodyKind::Method);
    let [lambda] = &m.lambdas[..] else {
        panic!("one lambda: {:?}", m.lambdas)
    };
    let body: Vec<_> = m
        .nodes
        .iter()
        .filter(|n| n.parent == Some(lambda.body))
        .collect();
    let [ret] = body[..] else {
        panic!("one node under the lambda: {body:?}")
    };
    assert_eq!(ret.kind, NodeKind::Return);
    assert_eq!(ret.returns, None);
    let condition = ret.condition.as_ref().expect("the returned predicate");
    assert_eq!(tree_shape(&condition.expr), "and(or(compare,compare),path)");
    assert_eq!(condition.text, "(a == X.Y || a == X.Z) && b");
    assert_eq!(condition.enum_refs, ["X.Y", "X.Z"]);
    assert!(condition.reads.contains(&path(PathRoot::Name, &["a"])));
    assert!(condition.reads.contains(&path(PathRoot::Name, &["b"])));
    assert_eq!(condition.constant, None);
}

/// `=> x && y` and `{ return x && y; }` give one tree; a constant is only
/// `returns`; a value that is no test has its tree too.
#[test]
fn a_returned_expression_has_one_tree_and_a_constant_none() {
    let src = "class C {\n    bool M() => x && y;\n    bool N() { return x && y; }\n    bool F() => false;\n    int Count() { return items.Count; }\n}\n";
    let file = read(src);
    let returned = |line: u32, name: &str| {
        let m = member(&file, at(src, line, name), BodyKind::Method);
        let r = m
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::Return)
            .unwrap_or_else(|| panic!("no return in {name}"));
        (r.returns.clone(), r.condition.clone())
    };
    let (arrow_returns, arrow) = returned(2, "M");
    let (block_returns, block) = returned(3, "N");
    assert_eq!(arrow_returns, None);
    assert_eq!(block_returns, None);
    let arrow = arrow.expect("the arrow's tree");
    assert_eq!(tree_shape(&arrow.expr), "and(path,path)");
    assert_eq!(Some(arrow), block);
    assert_eq!(
        returned(4, "F"),
        (Some(Constant::Boolean(false)), None),
        "a constant is only `returns`"
    );
    let (count_returns, count) = returned(5, "Count");
    assert_eq!(count_returns, None);
    let count = count.expect("a value has its tree too");
    assert_eq!(
        count.expr,
        [Expr::Path(path(PathRoot::Name, &["items", "Count"]))]
    );
}

#[test]
fn a_generated_file_has_no_bodies() {
    let src = "// <auto-generated>\n//     by a tool\n// </auto-generated>\nclass C { void M() { N(); } }\n";
    let empty = FileBodies {
        types: Vec::new(),
        members: Vec::new(),
        typed_members: Vec::new(),
        metas: Vec::new(),
    };
    assert_eq!(read(src), empty);
    let plain = "class C { void M() { N(); } }\n";
    assert_eq!(
        bodies("src/Form1.Designer.cs", plain.as_bytes()),
        Ok(empty.clone())
    );
    assert_eq!(bodies("obj/App.g.cs", plain.as_bytes()), Ok(empty));
    assert_eq!(read(plain).members.len(), 1);
}

/// The host pairs a body with its declaration by the position of the declared
/// name: both passes, over one source, must agree on it for every member the
/// first layer declares.
#[test]
fn a_member_body_sits_at_the_name_position_of_its_declaration() {
    use crate::shim::{FactKind, FactValue, FileOutput, GraphOp, PathKeys};
    use crate::walkers::{extract_declarations, Language};
    use ra_redact::SecretPrefixes;

    let src = "namespace N {\n  [Obsolete] public class Ünï : Base {\n    int a, b = 2;\n    public Ünï() : base() { }\n    static Ünï() { }\n    public int P { get => a; set { a = value; } }\n    public int Q => 1;\n    public event EventHandler E = null;\n    void Ŵork(int x) { int Local() => x; }\n  }\n}\n";
    let mut out = FileOutput::default();
    extract_declarations(
        Language::CSharp,
        "src/a.cs",
        src.as_bytes(),
        &mut out,
        &PathKeys::default(),
        &SecretPrefixes::new(&[]),
    );
    let mut declared: Vec<(String, Position)> = Vec::new();
    for op in out.ops {
        if let GraphOp::Symbol { facts, .. } = op {
            for f in facts {
                if let (FactKind::SymbolName, FactValue::Text(name), Some((line, column))) =
                    (f.kind, &f.value, f.provenance.name)
                {
                    declared.push((name.clone(), Position { line, column }));
                }
            }
        }
    }
    let file = read(src);
    let kinds: Vec<BodyKind> = file.members.iter().map(|m| m.kind).collect();
    assert_eq!(
        kinds,
        [
            BodyKind::MemberInitializer,
            BodyKind::Constructor,
            BodyKind::TypeInitializer,
            BodyKind::Getter,
            BodyKind::Setter,
            BodyKind::ExpressionProperty,
            BodyKind::MemberInitializer,
            BodyKind::Method,
            BodyKind::LocalFunction,
        ]
    );
    for m in &file.members {
        assert!(
            declared.iter().any(|(_, p)| *p == m.name_position),
            "{:?} body at {:?} pairs with no declaration in {declared:?}",
            m.kind,
            m.name_position
        );
    }
    // `int a, b = 2;`: the initializer belongs to `b`, not to the field
    // declaration's first name.
    assert!(declared
        .iter()
        .any(|(n, p)| n == "b" && *p == file.members[0].name_position));
    // A local function is paired with the member it is declared in.
    assert_eq!(file.members[8].name_position, file.members[7].name_position);
    for t in &file.types {
        assert!(declared.iter().any(|(_, p)| *p == t.name_position));
    }
    for m in &file.metas {
        assert!(declared.iter().any(|(_, p)| *p == m.name_position));
    }
    for t in &file.typed_members {
        assert!(declared
            .iter()
            .any(|(n, p)| *p == t.position && *n == t.name));
    }
}

/// An `else if` chain of 2 000 arms, a sum of 5 000 operands and a condition
/// of 10 000, on a stack a fraction of a native thread's.
#[test]
fn a_long_else_if_chain_and_a_long_sum_do_not_exhaust_the_stack() {
    let mut src = String::from("class C {\n  int M(int x) {\n    if (x == 0) return 0;\n");
    for i in 1..2000 {
        src.push_str(&format!("    else if (x == {i}) return {i};\n"));
    }
    src.push_str("    else return -1;\n  }\n  int S(int a) => a");
    for _ in 1..5000 {
        src.push_str(" + a");
    }
    src.push_str(";\n  bool B(int a) { return a");
    for _ in 1..5000 {
        src.push_str(" + a");
    }
    src.push_str(" > 0 && a > 0");
    for _ in 1..5000 {
        src.push_str(" || a > 0");
    }
    src.push_str("; }\n}\n");
    let file = std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn({
            let src = src.clone();
            move || read(&src)
        })
        .expect("spawned")
        .join()
        .expect("the walk did not overflow its stack");
    let m = member(&file, at(&src, 2, "M("), BodyKind::Method);
    let choices = m
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::Choice)
        .count();
    let arms = m.nodes.iter().filter(|n| n.kind == NodeKind::Arm).count();
    assert_eq!((choices, arms), (1, 2001));
    assert_eq!(tags_named(m, "guard-return").len(), 2000);
    let s = file
        .members
        .iter()
        .find(|m| m.name_position.line == 2005)
        .expect("the sum");
    assert_eq!(s.accesses.len(), 5000);
    let b = file
        .members
        .iter()
        .find(|m| m.name_position.line == 2006)
        .expect("the condition");
    assert_eq!(b.nodes[1].kind, NodeKind::Return);
    assert_eq!(b.accesses.len(), 10000);
}

#[test]
fn a_switch_is_a_choice_of_case_arms_over_its_subject() {
    let src = r#"class C {
    void M(KeyEventArgs e) {
        switch (e.Key) {
            case Key.F4:
                Run();
                break;
            case Key.F5 when Keyboard.Modifiers == ModifierKeys.Control:
                return;
            default:
                return;
        }
    }
}
"#;
    let file = read(src);
    let m = member(&file, at(src, 2, "M("), BodyKind::Method);
    let choice = &m.nodes[1];
    assert_eq!(choice.kind, NodeKind::Choice);
    let subject = choice.condition.as_ref().expect("the subject");
    assert_eq!(subject.text, "e.Key");
    let arms: Vec<_> = m.nodes.iter().filter(|n| n.kind == NodeKind::Arm).collect();
    assert_eq!(arms.len(), 3);
    let first = arms[0].condition.as_ref().expect("a label");
    assert_eq!(first.expr, [Expr::EnumMember("Key.F4".into())]);
    assert!(arms[0].tags.iter().any(|t| t.name == "key-compare"));
    let second = arms[1].condition.as_ref().expect("a label");
    assert!(matches!(second.expr.last(), Some(Expr::And(o)) if o.len() == 2));
    assert!(arms[1].tags.contains(&Tag {
        name: "key-compare".into(),
        value: Some("Key.F5".into()),
        args: vec!["ModifierKeys.Control".into()],
    }));
    assert!(arms[2].condition.is_none());
}

#[test]
fn catches_that_only_log_swallow_the_exception() {
    let src = r#"class C {
    void M() {
        try { Run(); } catch (IOException ex) { _logger.LogError(ex, "failed"); }
        try { Run(); } catch { }
        try { Run(); } catch (Exception ex) { Log(ex); throw; }
        try { Run(); } finally { Done(); }
    }
}
"#;
    let file = read(src);
    let m = member(&file, at(src, 2, "M("), BodyKind::Method);
    let catches: Vec<bool> = m
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::Catch)
        .map(|n| n.tags.iter().any(|t| t.name == "swallow-exception"))
        .collect();
    assert_eq!(catches, [true, true, false]);
    let finallies = m
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::Finally)
        .count();
    assert_eq!(finallies, 1);
    assert!(m.typed_names.iter().any(|t| t.name == "ex"
        && t.type_name == "IOException"
        && t.kind == DeclarationKind::Local));
}

#[test]
fn names_declared_in_a_body_are_typed_and_scoped() {
    let src = r#"class C {
    void M(object view, List<string> items) {
        if (view is not EditorView editor) return;
        editor.Refresh();
        EditorViewModel model = editor.Model;
        var other = new EditorView();
    }
}
"#;
    let file = read(src);
    let m = member(&file, at(src, 2, "M("), BodyKind::Method);
    let typed: Vec<(&str, &str, DeclarationKind)> = m
        .typed_names
        .iter()
        .map(|t| (t.name.as_str(), t.type_name.as_str(), t.kind))
        .collect();
    assert_eq!(
        typed,
        [
            ("view", "object", DeclarationKind::Parameter),
            ("items", "List", DeclarationKind::Parameter),
            ("editor", "EditorView", DeclarationKind::Pattern),
            ("model", "EditorViewModel", DeclarationKind::Local),
            ("other", "EditorView", DeclarationKind::Local),
        ]
    );
    assert_eq!(m.typed_names[1].type_args, ["string"]);
    assert!(m.typed_names[2].scope.is_some());
    // The pattern variable is a local for what follows it.
    assert!(m.accesses.iter().any(|a| a.mode == AccessMode::Invoke
        && a.target == path(PathRoot::Local, &["editor", "Refresh"])));
}

#[test]
fn string_literals_keep_their_value_and_context() {
    let src = r#"class C {
    string name;
    string M(Item item) {
        if (item.Kind == "a\tb") Use(nameof(Title));
        name = @"c""d";
        return "done";
    }
}
"#;
    let file = read(src);
    let m = member(&file, at(src, 3, "M("), BodyKind::Method);
    let literals: Vec<(&str, &LiteralContext)> = m
        .literals
        .iter()
        .map(|l| (l.value.as_str(), &l.context))
        .collect();
    assert_eq!(
        literals,
        [
            (
                "a\tb",
                &LiteralContext::Compared(path(PathRoot::Local, &["item", "Kind"]))
            ),
            (
                "c\"d",
                &LiteralContext::Assigned(path(PathRoot::Name, &["name"]))
            ),
            ("done", &LiteralContext::Returned),
        ]
    );
    for (i, l) in m.literals.iter().enumerate() {
        assert_eq!(l.id, i as u32);
    }
    assert_eq!(m.name_ofs.len(), 1);
    assert_eq!(m.name_ofs[0].text, "Title");
    assert_eq!(
        m.name_ofs[0].context,
        LiteralContext::CallArg((path(PathRoot::Name, &["Use"]), 0))
    );
    let arm = m
        .nodes
        .iter()
        .find(|n| n.kind == NodeKind::Arm)
        .expect("the arm");
    let condition = arm.condition.as_ref().expect("its condition");
    assert!(condition.expr.contains(&Expr::Literal(0)));
}

#[test]
fn a_call_on_an_unspellable_receiver_says_so() {
    let src = "class C {\n    void M(List<int> items) { items.Where(x => x > 0).Select(x => x * 2); }\n}\n";
    let file = read(src);
    let m = member(&file, at(src, 2, "M("), BodyKind::Method);
    let select = call(m, "Select");
    assert_eq!(select.callee, path(PathRoot::Expression, &["Select"]));
    let where_ = call(m, "Where");
    assert_eq!(where_.callee, path(PathRoot::Local, &["items", "Where"]));
    // Ids follow the order the calls are reached in, the outer call first;
    // each lambda is an argument of its own call.
    assert_eq!(m.invocations[0].callee, select.callee);
    assert_eq!(m.lambdas.len(), 2);
    let owners: Vec<Option<(u32, u32)>> = m.lambdas.iter().map(|l| l.argument_of).collect();
    assert_eq!(owners, [Some((1, 0)), Some((0, 0))]);
}

#[test]
fn the_component_lists_what_it_reports() {
    for name in [
        "member-access",
        "invocation",
        "lambda",
        "subscription",
        "string-literal",
        "name-of",
        "key-compare",
        "key-gesture",
        "menu-item",
        "notifies-change",
        "lazy-init",
        "full-traversal",
        "find-first",
        "accumulate",
        "guard-return",
        "swallow-exception",
    ] {
        assert!(super::EMITS.contains(&name), "`{name}` is not in emits");
    }
}

#[test]
fn the_same_source_gives_the_same_model() {
    assert_eq!(read(EDITOR), read(EDITOR));
}

#[test]
fn a_call_on_a_call_result_and_a_target_typed_construction_start_at_an_expression() {
    let src = "class C {\n    void M(List<Item> items) {\n        GetX().Run();\n        Target t = new();\n        GetY().Foo.Go();\n        items[0].Model.Refresh();\n    }\n}\n";
    let file = read(src);
    let m = member(&file, at(src, 2, "M("), BodyKind::Method);
    assert_eq!(call(m, "Run").callee, path(PathRoot::Expression, &["Run"]));
    assert_eq!(call(m, "GetX").callee, path(PathRoot::Name, &["GetX"]));
    // Every name written after the expression, in order.
    assert_eq!(
        call(m, "Go").callee,
        path(PathRoot::Expression, &["Foo", "Go"])
    );
    assert_eq!(call(m, "GetY").callee, path(PathRoot::Name, &["GetY"]));
    assert_eq!(
        call(m, "Refresh").callee,
        path(PathRoot::Expression, &["Model", "Refresh"])
    );
    // The receiver of an index is still walked: `items` is read.
    assert!(m
        .accesses
        .iter()
        .any(|a| a.target == path(PathRoot::Local, &["items"]) && a.mode == AccessMode::Read));
    // Nothing is said about the receiver: it is no member access of its own.
    assert!(m
        .accesses
        .iter()
        .all(|a| a.target.root != PathRoot::Expression));
    let built = m
        .invocations
        .iter()
        .find(|i| i.kind == CallKind::Construct)
        .expect("the construction");
    assert_eq!(built.callee, path(PathRoot::Expression, &[]));
    assert_eq!(built.flow, Some(FlowTarget::Local("t".into())));
}

#[test]
fn a_write_to_a_member_of_another_object_flows_to_its_path() {
    let src = "class C {\n    void M(ContextMenuItem f4) {\n        f4.Command = new RelayCommand(p => Check());\n        this.view.Model = Build();\n        var item = new MenuItem { Command = new RelayCommand(p => Check()) };\n        f = new RelayCommand(p => Check());\n    }\n}\n";
    let file = read(src);
    let m = member(&file, at(src, 2, "M("), BodyKind::Method);
    let flows: Vec<(String, Option<FlowTarget>)> = m
        .invocations
        .iter()
        .map(|i| (i.callee.segments.join("."), i.flow.clone()))
        .collect();
    let command = Some(FlowTarget::Path(path(PathRoot::Local, &["f4", "Command"])));
    let model = Some(FlowTarget::Path(path(PathRoot::This, &["view", "Model"])));
    let initialized = Some(FlowTarget::Path(path(
        PathRoot::Local,
        &["item", "Command"],
    )));
    let member = Some(FlowTarget::Member("f".into()));
    for expected in [
        ("RelayCommand".to_string(), command.clone()),
        ("Build".to_string(), model),
        ("RelayCommand".to_string(), initialized),
        ("RelayCommand".to_string(), member),
    ] {
        assert!(flows.contains(&expected), "{expected:?} not in {flows:?}");
    }
    // The lambda given to the construction flows where the construction does.
    assert!(
        m.lambdas.iter().any(|l| l.flow == command),
        "{:?}",
        m.lambdas
    );
}

#[test]
fn a_yield_return_is_a_node_of_its_own_kind() {
    let src = "class C {\n    IEnumerable<int> M() {\n        yield return 1;\n        yield break;\n    }\n}\n";
    let file = read(src);
    let m = member(&file, at(src, 2, "M("), BodyKind::Method);
    let kinds: Vec<&NodeKind> = m.nodes.iter().map(|n| &n.kind).collect();
    assert!(kinds.contains(&&NodeKind::YieldReturn), "{kinds:?}");
    assert!(kinds.contains(&&NodeKind::YieldBreak), "{kinds:?}");
    assert!(
        !kinds.iter().any(|k| matches!(k, NodeKind::Other(_))),
        "{kinds:?}"
    );
}

#[test]
fn a_file_that_is_not_utf8_is_refused_with_the_reason() {
    // A letter in a legacy single-byte code page: one byte that starts no
    // UTF-8 character.
    let mut src = b"class C { string s = \"".to_vec();
    src.push(0xF8);
    src.extend_from_slice(b"\"; }\n");
    assert_eq!(
        bodies("src/a.cs", &src),
        Err("not valid UTF-8 at byte 22".to_string())
    );
    // A generated file is not read, so it is not refused either.
    assert!(bodies("src/Form1.Designer.cs", &src).is_ok_and(|f| f.members.is_empty()));
    // A byte order mark is UTF-8, and a syntax error is read as far as it
    // goes.
    let bom = "\u{feff}class C { void M() { N(); } }\n";
    assert_eq!(read(bom).members.len(), 1);
    assert_eq!(read("class C { void M() { N( } }\n").members.len(), 1);
}

/// Every branch of a `#if` region is in the model — `#elif` and `#else`
/// too — so no configuration's return reads as the member's only one.
#[test]
fn every_branch_of_a_preprocessor_region_is_in_the_model() {
    let src = "class C {\n    bool M() {\n#if DEMO\n        return false;\n#elif TRIAL\n        return Trial();\n#else\n        return _dirty;\n#endif\n    }\n}\n";
    let file = read(src);
    let m = member(&file, at(src, 2, "M("), BodyKind::Method);
    let returns: Vec<(Option<Constant>, Option<Vec<Expr>>)> = m
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::Return)
        .map(|n| {
            (
                n.returns.clone(),
                n.condition.as_ref().map(|c| c.expr.clone()),
            )
        })
        .collect();
    assert_eq!(returns.len(), 3, "{returns:?}");
    assert_eq!(returns[0], (Some(Constant::Boolean(false)), None));
    assert_eq!(
        returns[2],
        (
            None,
            Some(vec![Expr::Path(path(PathRoot::Name, &["_dirty"]))])
        )
    );
    assert_eq!(call(m, "Trial").kind, CallKind::Call);
}

/// `await` keeps the call it waits for: a statement, a value given to a
/// local, a lambda's expression body (a statement, not a return), and a
/// catch that only awaits a log call swallows the exception.
#[test]
fn an_awaited_call_is_read_as_the_call() {
    let src = r#"class C {
    async Task M(int x, string k) {
        await Save(x);
        var r = await Load(k);
        Func<int, Task> f = async y => await Notify(y);
        try { Run(); } catch { await Log(r); }
        try { Run(); } catch { await Retry(r); }
    }
}
"#;
    let file = read(src);
    let m = member(&file, at(src, 2, "M("), BodyKind::Method);
    let save = call(m, "Save");
    assert_eq!(
        m.nodes[save.node.expect("its node") as usize].kind,
        NodeKind::Statement
    );
    assert_eq!(call(m, "Load").flow, Some(FlowTarget::Local("r".into())));
    let notify = call(m, "Notify");
    let lambda = m.lambdas.first().expect("the lambda");
    let statement = &m.nodes[notify.node.expect("its node") as usize];
    assert_eq!(statement.kind, NodeKind::Statement);
    assert_eq!(statement.parent, Some(lambda.body));
    assert!(!m
        .nodes
        .iter()
        .any(|n| n.parent == Some(lambda.body) && n.kind == NodeKind::Return));
    call(m, "Log");
    call(m, "Retry");
    let catches: Vec<bool> = m
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::Catch)
        .map(|n| n.tags.iter().any(|t| t.name == "swallow-exception"))
        .collect();
    assert_eq!(catches, [true, false]);
}

/// A `while` reads its condition before its body, a `do … while` after it,
/// and a `using` resource is a local of the region it opens.
#[test]
fn while_do_and_using_keep_the_order_and_the_scope_of_the_source() {
    let src = r#"class C {
    void M() {
        while (More()) Step();
        do { Advance(); } while (Again());
        using (var s = Open()) { s.Read(); }
    }
}
"#;
    let file = read(src);
    let m = member(&file, at(src, 2, "M("), BodyKind::Method);
    let order: Vec<&str> = m
        .invocations
        .iter()
        .map(|i| i.callee.segments.last().map(String::as_str).unwrap_or(""))
        .collect();
    assert_eq!(order, ["More", "Step", "Advance", "Again", "Open", "Read"]);
    let loops: Vec<bool> = m
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::Loop)
        .map(|n| n.condition.is_some())
        .collect();
    assert_eq!(loops, [true, true]);
    let using = m
        .nodes
        .iter()
        .find(|n| n.kind == NodeKind::Other("using".into()))
        .expect("the using region");
    assert_eq!(call(m, "Open").flow, Some(FlowTarget::Local("s".into())));
    let read_call = call(m, "Read");
    assert_eq!(read_call.callee, path(PathRoot::Local, &["s", "Read"]));
    let mut at_node = read_call.node;
    while let Some(id) = at_node.filter(|&id| id != using.id) {
        at_node = m.nodes[id as usize].parent;
    }
    assert_eq!(at_node, Some(using.id), "the call is inside the region");
}

/// An event's `add` and `remove` accessors are bodies of their own kinds.
#[test]
fn event_accessors_are_adder_and_remover_bodies() {
    let src = "class C {\n    EventHandler h;\n    public event EventHandler Changed { add { h += value; } remove { h -= value; } }\n}\n";
    let file = read(src);
    for kind in [BodyKind::Adder, BodyKind::Remover] {
        let m = member(&file, at(src, 3, "Changed"), kind);
        assert!(
            m.accesses
                .iter()
                .any(|a| a.target == path(PathRoot::Name, &["h"])),
            "{kind:?}: {:?}",
            m.accesses
        );
    }
}

/// A member declared without a body — `abstract`, `extern`, the declaring
/// half of a `partial` method, an auto-property's accessors — has no body
/// in the model, only its meta.
#[test]
fn a_member_without_a_body_has_none_in_the_model() {
    let src = "abstract partial class C {\n    public abstract bool CanSave();\n    static extern int Native();\n    partial void OnSaved();\n    public bool Ready { get; set; }\n}\n";
    let file = read(src);
    assert!(file.members.is_empty(), "{:?}", file.members);
    for (line, name) in [(2, "CanSave"), (3, "Native"), (4, "OnSaved")] {
        assert!(
            file.metas
                .iter()
                .any(|m| m.name_position == at(src, line, name)),
            "no meta of {name}"
        );
    }
}

/// A parameter stored behind a null guard (`p ?? throw …`) is written from
/// the parameter; a fallback (`p ?? q`) is not.
#[test]
fn a_parameter_guarded_by_a_throw_is_still_the_value_written() {
    let src = "class Cmd {\n    Cmd(Action<object> execute, Func<object, bool> can) {\n        _execute = execute ?? throw new ArgumentNullException(nameof(execute));\n        _can = (can ?? throw new ArgumentNullException());\n        _other = execute ?? fallback;\n    }\n}\n";
    let file = read(src);
    let ctor = member(&file, at(src, 2, "Cmd("), BodyKind::Constructor);
    let written = |field: &str| {
        ctor.accesses
            .iter()
            .find(|a| a.mode == AccessMode::Write && a.target == path(PathRoot::Name, &[field]))
            .unwrap_or_else(|| panic!("no write of {field}: {:?}", ctor.accesses))
            .rhs
            .clone()
    };
    assert_eq!(written("_execute"), Some(ValueRef::Param(0)));
    assert_eq!(written("_can"), Some(ValueRef::Param(1)));
    assert_eq!(written("_other"), Some(ValueRef::Other));
}
