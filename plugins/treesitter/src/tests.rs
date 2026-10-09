//! Tests of the walkers themselves, run natively (`cargo test -p treesitter
//! --features <lang>` in `plugins/`). The host's own suite drives the same code
//! through the built components; these cover what only the crate can reach.

use crate::shim::{NodeKey, PathKeys};
use crate::walkers::Language;

/// Every grammar this build carries loads against tree-sitter core and parses a
/// trivial snippet without an error at the root (KTD-B2).
#[test]
fn every_enabled_grammar_loads_and_parses() {
    let cases: &[(&str, &str)] = &[
        ("a.rs", "fn f() {}"),
        ("a.ts", "function f() {}"),
        ("a.tsx", "function f() { return <a/>; }"),
        ("a.js", "function f() {}"),
        ("a.py", "def f():\n    pass\n"),
        ("a.cs", "class C {}"),
        ("a.go", "package p\nfunc f() {}\n"),
        ("a.java", "class C {}"),
    ];
    let mut loaded = 0;
    for (path, snippet) in cases {
        let Some(lang) = Language::enabled(path) else {
            continue;
        };
        let grammar = lang.grammar().expect("enabled means the grammar is there");
        let tree = crate::walkers::parse(snippet.as_bytes(), grammar)
            .unwrap_or_else(|| panic!("{path}: grammar incompatible with core 0.26"));
        assert!(
            !tree.root_node().has_error(),
            "{path}: unexpected parse error"
        );
        loaded += 1;
    }
    assert!(
        loaded > 0,
        "a build of this crate carries at least one grammar"
    );
}

/// A candidate is a path assembled by string arithmetic; a node key has its
/// separators normalized. The lookup goes through the same normalization, so a
/// specifier written with a backslash still resolves.
#[test]
fn a_candidate_is_normalized_the_way_a_node_key_is_before_it_is_looked_up() {
    let paths = PathKeys {
        known: ["src/sub/b.ts".to_string()].into_iter().collect(),
    };
    assert_eq!(
        paths.first_existing(&["src\\sub\\b.ts".to_string()]),
        Some(NodeKey::new("src/sub/b.ts"))
    );
    assert_eq!(paths.first_existing(&["src/sub/c.ts".to_string()]), None);
}

/// The wire conversion keeps every op and spells kinds as the host's words.
#[cfg(feature = "rust")]
#[test]
fn a_rust_file_converts_to_symbol_ops_with_host_kind_words() {
    use crate::ra::plugin::types::{Op, Value};
    let ops = crate::convert::declarations("src/a.rs", b"/// Docs.\npub struct Foo;\n", &[]);
    let sym = ops
        .iter()
        .find_map(|op| match op {
            Op::Symbol(s) => Some(s),
            _ => None,
        })
        .expect("a struct is a symbol");
    assert_eq!(sym.key, "sym:rust:crate::a#Foo");
    let kinds: Vec<&str> = sym.facts.iter().map(|f| f.kind.as_str()).collect();
    assert!(kinds.contains(&"symbol_name"), "{kinds:?}");
    assert!(kinds.contains(&"doc_comment"), "{kinds:?}");
    assert!(sym
        .facts
        .iter()
        .any(|f| f.kind == "symbol_name" && matches!(&f.value, Value::Text(t) if t == "Foo")));
    assert!(sym
        .edges
        .iter()
        .any(|e| e.kind == "declares" && e.source == "src/a.rs"));
}

/// A Rust file's usages carry no context and no column: only C# reads context
/// so far, and every other language reports its sites as 0.1 did (R6).
#[cfg(feature = "rust")]
#[test]
fn a_language_without_context_reports_its_sites_as_0_1_did() {
    let file = crate::convert::usages("src/a.rs", b"fn f() {\n    g();\n}\n");
    assert!(file.scopes.is_empty());
    let g = file.usages.iter().find(|u| u.name == "g").expect("a call");
    assert_eq!(g.line, 2);
    assert_eq!(g.column, None);
    assert!(g.context.is_none());
}

/// The usage pass over C# with context (plan A, U6): what the syntax of one
/// file says about each site's receiver, its arguments and the scopes around
/// it — read in the same parse that finds the sites.
#[cfg(feature = "csharp")]
mod csharp_context {
    use crate::ra::plugin::types::{
        FileUsages, Op, Receiver, SiteContext, Usage, UsageKind, UsingDirective, Value,
    };

    fn usages(src: &str) -> FileUsages {
        crate::convert::usages("src/a.cs", src.as_bytes())
    }

    fn all<'f>(file: &'f FileUsages, name: &str) -> Vec<&'f Usage> {
        file.usages.iter().filter(|u| u.name == name).collect()
    }

    fn one<'f>(file: &'f FileUsages, name: &str) -> &'f Usage {
        match all(file, name).as_slice() {
            [only] => only,
            other => panic!(
                "expected one `{name}` site, found {}: {:?}",
                other.len(),
                file.usages.iter().map(|u| &u.name).collect::<Vec<_>>()
            ),
        }
    }

    fn ctx(u: &Usage) -> &SiteContext {
        u.context
            .as_ref()
            .unwrap_or_else(|| panic!("`{}` carries no context", u.name))
    }

    fn receiver(u: &Usage) -> String {
        match &ctx(u).receiver {
            Receiver::None => "none".into(),
            Receiver::Name(n) => format!("name:{n}"),
            Receiver::Value(n) => format!("value:{n}"),
            Receiver::Keyword(k) => format!("keyword:{k}"),
            Receiver::Expression => "expression".into(),
        }
    }

    /// `(name, keyword)` of the declared receiver type, or `None`.
    fn declared(u: &Usage) -> Option<(String, bool)> {
        ctx(u)
            .declared_type
            .as_ref()
            .map(|t| (t.name.clone(), t.keyword))
    }

    fn typed(name: &str) -> Option<(String, bool)> {
        Some((name.to_string(), false))
    }

    fn using(u: &UsingDirective) -> String {
        format!(
            "{}{}{}{}",
            if u.is_global { "global " } else { "" },
            if u.is_static { "static " } else { "" },
            u.alias
                .as_deref()
                .map(|a| format!("{a} = "))
                .unwrap_or_default(),
            u.target
        )
    }

    /// AE1: a call on a built-in type spelled as its keyword says so — it
    /// cannot reach `OctetString.IsNullOrEmpty` in the repository (R5).
    #[test]
    fn a_call_on_a_keyword_type_carries_the_keyword_flag() {
        let file = usages(
            "class C {\n    bool M(string s) {\n        return string.IsNullOrEmpty(s);\n    }\n}\n",
        );
        let site = one(&file, "IsNullOrEmpty");
        assert_eq!(receiver(site), "keyword:string");
        assert_eq!(declared(site), None);
        assert_eq!(ctx(site).argument_count, Some(1));
    }

    /// …and so does a call on a receiver DECLARED with a keyword type, a
    /// nullable one included.
    #[test]
    fn a_receiver_declared_with_a_keyword_type_is_flagged_too() {
        let file = usages(
            "class C {\n    void M(string s, int? n) {\n        s.Trim();\n        n.GetValueOrDefault();\n    }\n}\n",
        );
        assert_eq!(declared(one(&file, "Trim")), Some(("string".into(), true)));
        assert_eq!(
            declared(one(&file, "GetValueOrDefault")),
            Some(("int".into(), true))
        );
    }

    /// AE2: the declared type of a parameter reaches the call on it.
    #[test]
    fn a_parameter_declares_the_type_of_its_receiver() {
        let file = usages(
            "class C {\n    void M(StringBuilder result) {\n        result.Append(\"x\");\n    }\n}\n",
        );
        let site = one(&file, "Append");
        assert_eq!(receiver(site), "value:result");
        assert_eq!(declared(site), typed("StringBuilder"));
    }

    /// `var x = new T(...)` declares `T` — a syntactic fact, not inference;
    /// `var x = Load()` declares nothing, and neither does a target-typed
    /// `new()`, whose type only the target knows. An explicit type on the
    /// left is a declaration whatever the right says.
    #[test]
    fn var_takes_its_type_from_new_t_and_from_nothing_else() {
        let file = usages(
            "class C {\n    void M() {\n        var calc = new CsCalculator();\n        calc.Compute();\n        var x = Load();\n        x.Run();\n        var t = new();\n        t.Go();\n        Foo f = new();\n        f.Stop();\n        new Engine().Start();\n    }\n}\n",
        );
        assert_eq!(declared(one(&file, "Compute")), typed("CsCalculator"));
        assert_eq!(receiver(one(&file, "Run")), "value:x");
        assert_eq!(declared(one(&file, "Run")), None);
        assert_eq!(declared(one(&file, "Go")), None);
        assert_eq!(declared(one(&file, "Stop")), typed("Foo"));
        // A receiver that is itself `new T(...)`: its type is written there.
        assert_eq!(receiver(one(&file, "Start")), "expression");
        assert_eq!(declared(one(&file, "Start")), typed("Engine"));
    }

    /// A local in a nested block shadows a field of the same name for as long
    /// as the block lasts — and a field declared BELOW the method is in scope
    /// of its whole body. A local declared without a type still shadows.
    #[test]
    fn a_local_in_a_nested_block_shadows_a_field() {
        let file = usages(
            "class C {\n    void M() {\n        sb.Append(1);\n        {\n            var sb = Load();\n            sb.Append(2);\n        }\n        {\n            Writer sb = Open();\n            sb.Append(3);\n        }\n        sb.Append(4);\n    }\n    private StringBuilder sb;\n}\n",
        );
        let appends: Vec<Option<(String, bool)>> =
            all(&file, "Append").into_iter().map(declared).collect();
        assert_eq!(
            appends,
            [
                typed("StringBuilder"),
                None,
                typed("Writer"),
                typed("StringBuilder")
            ]
        );
    }

    /// Fields, properties and primary-constructor parameters are in scope of
    /// the whole type body; `this.x` names the type's own member.
    #[test]
    fn members_and_primary_constructor_parameters_declare_their_types() {
        let file = usages(
            "class K(Repo repo) {\n    Cache Store { get; }\n    Log log;\n    void M() {\n        repo.Load();\n        Store.Flush();\n        this.log.Write();\n        base.Close();\n    }\n}\n",
        );
        assert_eq!(declared(one(&file, "Load")), typed("Repo"));
        assert_eq!(declared(one(&file, "Flush")), typed("Cache"));
        assert_eq!(receiver(one(&file, "Write")), "name:this.log");
        assert_eq!(declared(one(&file, "Write")), typed("Log"));
        assert_eq!(receiver(one(&file, "Close")), "name:base");
        assert_eq!(declared(one(&file, "Close")), None);
    }

    /// Every other place C# declares a name: foreach, catch, using, a
    /// pattern, a typed and an untyped lambda parameter, a local function's
    /// parameter, an `out` declaration.
    #[test]
    fn statement_and_pattern_variables_declare_their_types() {
        let file = usages(
            "class C {\n    void M(object o, Item[] xs) {\n        foreach (Item it in xs) it.Touch();\n        try { } catch (IOException e) { e.Report(); }\n        using (var r = new Reader()) { r.Read(); }\n        if (o is Bar bb) { bb.Open(); }\n        xs.Where((Item w) => w.Keep());\n        xs.Select(q => q.Pick());\n        int Local(Meter m) => m.Measure();\n        Parse(out Token tok);\n        tok.Use();\n    }\n}\n",
        );
        assert_eq!(declared(one(&file, "Touch")), typed("Item"));
        assert_eq!(declared(one(&file, "Report")), typed("IOException"));
        assert_eq!(declared(one(&file, "Read")), typed("Reader"));
        assert_eq!(declared(one(&file, "Open")), typed("Bar"));
        assert_eq!(declared(one(&file, "Keep")), typed("Item"));
        assert_eq!(receiver(one(&file, "Pick")), "value:q");
        assert_eq!(declared(one(&file, "Pick")), None);
        assert_eq!(declared(one(&file, "Measure")), typed("Meter"));
        assert_eq!(declared(one(&file, "Use")), typed("Token"));
    }

    /// A bare call says it has no receiver — what the implicit-this rule
    /// needs and a 0.1 site cannot say; a type name is a named receiver with
    /// no declared type.
    #[test]
    fn a_bare_call_says_it_has_no_receiver() {
        let file = usages(
            "class C {\n    void M() {\n        Solve(1, 2);\n        Helper.Do();\n    }\n}\n",
        );
        assert_eq!(receiver(one(&file, "Solve")), "none");
        assert_eq!(ctx(one(&file, "Solve")).argument_count, Some(2));
        assert_eq!(receiver(one(&file, "Do")), "name:Helper");
        assert_eq!(declared(one(&file, "Do")), None);
    }

    /// A receiver the file declares as a value is a `value`, whether or not
    /// its declaration states a type — a local, a parameter (an untyped
    /// lambda's too), a field — even when it is named like a type. So is a
    /// path that starts with one. Only a name the file does not declare
    /// stays a `name`, which the host may read as a type.
    #[test]
    fn a_value_named_like_a_type_is_a_value_not_a_name() {
        let file = usages(
            "class C {\n    Gadget[] Gadget;\n    void M(Engine Engine, Helper[] Helper) {\n        var Worker = Load();\n        Worker.Run();\n        Engine.Start();\n        Helper.Help();\n        Gadget.Clear();\n        xs.Select(Probe => Probe.Pick());\n        var Store = Open();\n        Store.Items.Add();\n        Other.Do();\n    }\n}\n",
        );
        assert_eq!(receiver(one(&file, "Run")), "value:Worker");
        assert_eq!(declared(one(&file, "Run")), None);
        assert_eq!(receiver(one(&file, "Start")), "value:Engine");
        assert_eq!(declared(one(&file, "Start")), typed("Engine"));
        assert_eq!(receiver(one(&file, "Help")), "value:Helper");
        assert_eq!(declared(one(&file, "Help")), None);
        assert_eq!(receiver(one(&file, "Clear")), "value:Gadget");
        assert_eq!(declared(one(&file, "Clear")), None);
        assert_eq!(receiver(one(&file, "Pick")), "value:Probe");
        assert_eq!(receiver(one(&file, "Add")), "value:Store.Items");
        assert_eq!(declared(one(&file, "Add")), None);
        // Declared nowhere in the file: a type, or a member it cannot see.
        assert_eq!(receiver(one(&file, "Do")), "name:Other");
    }

    /// `x?.Append(...)` used to emit no site at all; it is a call on `x`.
    #[test]
    fn a_conditional_access_call_is_a_site_on_its_receiver() {
        let file =
            usages("class C {\n    void M(StringBuilder x) {\n        x?.Append(1);\n    }\n}\n");
        let site = one(&file, "Append");
        assert!(matches!(site.kind, UsageKind::Call));
        assert_eq!(receiver(site), "value:x");
        assert_eq!(declared(site), typed("StringBuilder"));
        assert_eq!((site.line, site.column), (3, Some(12)));
    }

    /// Two calls on one line are two positions: the column is the called
    /// NAME's, and both invocations start at `sb`. In a chain that spans
    /// lines, each call sits on the line its name is written on.
    #[test]
    fn two_calls_on_one_line_have_different_columns() {
        let file = usages(
            "class C {\n    void M(StringBuilder sb) {\nsb.Append(a).Append(b);\nsb\n    .Append(c)\n    .Append(d);\n    }\n}\n",
        );
        let appends = all(&file, "Append");
        let positions: Vec<(u32, Option<u32>)> =
            appends.iter().map(|u| (u.line, u.column)).collect();
        // Tree order: the outer call of a chain comes first.
        assert_eq!(
            positions,
            [(3, Some(14)), (3, Some(4)), (6, Some(6)), (5, Some(6))]
        );
        let first_line: Vec<String> = appends[..2].iter().map(|u| receiver(u)).collect();
        assert_eq!(first_line, ["expression", "value:sb"]);
    }

    /// A column counts characters, not bytes: two Czech letters before the
    /// call are two columns, though four bytes.
    #[test]
    fn a_column_counts_characters() {
        let file = usages("class C {\n    void M() {\nvar s = \"žš\"; s.Trim();\n    }\n}\n");
        assert_eq!(one(&file, "Trim").column, Some(17));
    }

    /// Named arguments count like any other.
    #[test]
    fn the_argument_count_includes_named_arguments() {
        let file = usages(
            "class C {\n    void M(Calc c) {\n        c.Compute(1, y: 2, z: 3);\n        new Point(1, 2);\n        new Box { W = 1 };\n    }\n}\n",
        );
        assert_eq!(ctx(one(&file, "Compute")).argument_count, Some(3));
        assert_eq!(ctx(one(&file, "Point")).argument_count, Some(2));
        assert_eq!(ctx(one(&file, "Box")).argument_count, Some(0));
    }

    /// A qualified construction or type reference says what precedes the
    /// name; a type reference has no argument count and no receiver.
    #[test]
    fn a_qualified_type_carries_its_qualifier() {
        let file = usages(
            "class C {\n    void M(System.IO.Stream s) {\n        var b = new System.Text.StringBuilder();\n    }\n}\n",
        );
        let built = one(&file, "StringBuilder");
        assert!(matches!(built.kind, UsageKind::Construct));
        assert_eq!(ctx(built).qualifier.as_deref(), Some("System.Text"));
        assert_eq!(ctx(built).argument_count, Some(0));
        let stream = one(&file, "Stream");
        assert!(matches!(stream.kind, UsageKind::Reference));
        assert_eq!(ctx(stream).qualifier.as_deref(), Some("System.IO"));
        assert_eq!(ctx(stream).argument_count, None);
        assert_eq!(receiver(stream), "none");
    }

    /// `global using`, `using static` and an alias land in the file's own
    /// scope; a using written inside a namespace belongs to that namespace
    /// alone, and a site points at the innermost scope around it.
    #[test]
    fn usings_land_in_the_scope_they_are_written_in() {
        let file = usages(
            "global using System;\nglobal using static System.Math;\nusing Txt = System.Text;\nusing static System.Console;\nnamespace A.B\n{\n    using System.Linq;\n    namespace C\n    {\n        class X { void M() { Foo(); } }\n    }\n}\nclass Y { void N() { Bar(); } }\n",
        );
        type Row = (String, Option<u32>, (u32, u32), Vec<String>);
        let scopes: Vec<Row> = file
            .scopes
            .iter()
            .map(|s| {
                (
                    s.namespace.clone(),
                    s.parent,
                    s.lines,
                    s.usings.iter().map(using).collect(),
                )
            })
            .collect();
        let expected: Vec<Row> = vec![
            (
                String::new(),
                None,
                (1, 13),
                vec![
                    "global System".into(),
                    "global static System.Math".into(),
                    "Txt = System.Text".into(),
                    "static System.Console".into(),
                ],
            ),
            ("A.B".into(), Some(0), (5, 12), vec!["System.Linq".into()]),
            ("A.B.C".into(), Some(1), (8, 11), vec![]),
        ];
        assert_eq!(scopes, expected);
        assert_eq!(ctx(one(&file, "Foo")).scope, 2);
        assert_eq!(ctx(one(&file, "Bar")).scope, 0);
    }

    /// A file-scoped `namespace N;` is a scope from its own line to the end
    /// of the file, and the usings written after it are its own.
    #[test]
    fn a_file_scoped_namespace_holds_what_follows_it() {
        let file =
            usages("using Outer;\nnamespace N;\nusing Inner;\nclass Z { void M() { Q(); } }\n");
        let usings =
            |i: usize| -> Vec<String> { file.scopes[i].usings.iter().map(using).collect() };
        assert_eq!(file.scopes.len(), 2);
        assert_eq!(usings(0), ["Outer"]);
        assert_eq!(file.scopes[1].namespace, "N");
        assert_eq!(file.scopes[1].parent, Some(0));
        assert_eq!(file.scopes[1].lines, (2, 4));
        assert_eq!(usings(1), ["Inner"]);
        assert_eq!(ctx(one(&file, "Q")).scope, 1);
    }

    /// A declaration's place says where its name is written, so two
    /// declarations on one line are two positions (KTD3).
    #[test]
    fn a_declaration_place_carries_its_name_position() {
        let ops = crate::convert::declarations(
            "src/a.cs",
            b"class C {\n    [Obsolete]\n    public void Run() {}\n    int a, b;\n}\n",
            &[],
        );
        let name_at = |name: &str| {
            ops.iter()
                .find_map(|op| match op {
                    Op::Symbol(s) => s.facts.iter().find(|f| {
                        f.kind == "symbol_name" && matches!(&f.value, Value::Text(t) if t == name)
                    }),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("no symbol `{name}`"))
                .place
                .name
                .map(|p| (p.line, p.column))
        };
        assert_eq!(name_at("C"), Some((1, 7)));
        assert_eq!(name_at("Run"), Some((3, 17)));
        assert_eq!(name_at("a"), Some((4, 9)));
        assert_eq!(name_at("b"), Some((4, 12)));
    }
}
