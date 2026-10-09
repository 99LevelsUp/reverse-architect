//! The language walkers, moved from the host (`src/extract/treesitter.rs`) in
//! U11 with their bodies unchanged. Everything they emit goes into a
//! [`crate::shim::FileOutput`]; `crate::convert` turns that into wire records.
//!
//! The module doc of the host's `treesitter.rs` still describes what the
//! walkers read and why (identity, relations, usages). This file is the code.

#![allow(dead_code)]

use crate::shim::{
    ids, Edge, EdgeKind, Fact, FactKind, FactValue, FileOutput, NodeKey, PathKeys, Provenance, Tier,
};
use ra_redact::SecretPrefixes;
use std::collections::{BTreeMap, HashSet};
use tree_sitter::{Node, Parser, Tree};

/// The languages this extractor can parse. Grows as grammars are added.
#[derive(Clone, Copy)]
pub(crate) enum Language {
    Rust,
    /// `.ts` — parsed with `LANGUAGE_TYPESCRIPT`.
    TypeScript,
    /// `.tsx` — parsed with `LANGUAGE_TSX` (the one grammar with two languages).
    Tsx,
    Python,
    CSharp,
    Go,
    /// `.js`, `.jsx`, `.mjs`, `.cjs` — parsed with `tree-sitter-javascript`
    /// (which reads JSX too) over the TypeScript walker: the two grammars share
    /// their node kinds for everything the walker reads, and where they differ
    /// (`field_definition`, a bare `class_heritage`) the walker handles both.
    JavaScript,
    /// `.java` — packages are the module path, the way C# namespaces are.
    Java,
}

impl Language {
    fn from_path(path: &str) -> Option<Language> {
        match path.rsplit_once('.').map(|(_, ext)| ext) {
            Some("rs") => Some(Language::Rust),
            Some("ts") => Some(Language::TypeScript),
            Some("tsx") => Some(Language::Tsx),
            Some("py") => Some(Language::Python),
            Some("cs") => Some(Language::CSharp),
            Some("go") => Some(Language::Go),
            Some("js" | "jsx" | "mjs" | "cjs") => Some(Language::JavaScript),
            Some("java") => Some(Language::Java),
            _ => None,
        }
    }

    /// The compiled grammar for this language. `tree-sitter-typescript` is the one
    /// crate exporting two languages, which is exactly why `.ts` and `.tsx` are
    /// separate variants over one shared walker.
    pub(crate) fn grammar(self) -> Option<tree_sitter::Language> {
        #[allow(unreachable_patterns)]
        match self {
            #[cfg(feature = "rust")]
            Language::Rust => Some(tree_sitter_rust::LANGUAGE.into()),
            #[cfg(feature = "ecmascript")]
            Language::TypeScript => Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
            #[cfg(feature = "ecmascript")]
            Language::Tsx => Some(tree_sitter_typescript::LANGUAGE_TSX.into()),
            #[cfg(feature = "ecmascript")]
            Language::JavaScript => Some(tree_sitter_javascript::LANGUAGE.into()),
            #[cfg(feature = "python")]
            Language::Python => Some(tree_sitter_python::LANGUAGE.into()),
            #[cfg(feature = "csharp")]
            Language::CSharp => Some(tree_sitter_c_sharp::LANGUAGE.into()),
            #[cfg(feature = "go")]
            Language::Go => Some(tree_sitter_go::LANGUAGE.into()),
            #[cfg(feature = "java")]
            Language::Java => Some(tree_sitter_java::LANGUAGE.into()),
            _ => None,
        }
    }

    /// The language of `path`, when this build carries its grammar.
    pub(crate) fn enabled(path: &str) -> Option<Language> {
        let lang = Language::from_path(path)?;
        lang.grammar().map(|_| lang)
    }
}

/// Parse `src` with `language`, degrading to `None` (never a panic, E4) if the
/// grammar cannot be set or the source cannot be parsed.
pub(crate) fn parse(src: &[u8], language: tree_sitter::Language) -> Option<Tree> {
    let mut parser = Parser::new();
    parser.set_language(&language).ok()?;
    parser.parse(src, None)
}

// ---- Emission state and the symbol record ----

/// Per-file emission state threaded through a walker.
///
/// Deliberately NOT `Default`: `secret_prefixes` carries the run's configured
/// token shapes (R22), and a state built without them would filter with the
/// built-in list while the operator believed their own prefixes were in force.
/// Making that unrepresentable is cheaper than remembering it.
struct EmitState {
    /// Identity → how many symbols have already claimed it, so a residual
    /// duplicate gets a stable ORDINAL suffix. A byte offset (the Phase-B
    /// scheme) would move whenever the file is reformatted, and KTD3 requires
    /// identity to survive reformatting.
    used: BTreeMap<String, u32>,
    /// `type --HasMember--> member` edges, added after the walk rather than
    /// during it: a member can be declared *before* its type (a Go method above
    /// its `type` block, a Rust `impl` above its `struct`), and `add_edge`
    /// silently drops an edge whose endpoint does not exist yet.
    members: Vec<(NodeKey, NodeKey, Provenance)>,
    /// `Extends`/`Implements` relations this file's declarations NAME, resolved
    /// only once the whole repo has been walked — one scope wider than `members`,
    /// because a base type lives in another file far more often than a member
    /// does. The walkers hand these up to the run; nothing here touches the graph.
    relations: Vec<TypeRelation>,
    /// Token prefixes this run recognizes with no keyword nearby (R22). Lives on
    /// the emission state because every walker already threads it, and the
    /// redaction happens at the leaves where symbols are emitted.
    secret_prefixes: SecretPrefixes,
}

impl EmitState {
    fn new(secret_prefixes: SecretPrefixes) -> EmitState {
        EmitState {
            used: BTreeMap::new(),
            members: Vec::new(),
            relations: Vec::new(),
            secret_prefixes,
        }
    }
}

/// One type relation a declaration names, buffered until the whole repo has been
/// walked (U3). Nothing is resolved at collection time: both ends are CANDIDATE
/// identities, and the relation becomes an edge in [`flush_type_relations`] only
/// if both exist by then. A base type this repo does not declare therefore yields
/// no edge and no phantom node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TypeRelation {
    /// Candidate identities of the DERIVING type, most specific first. A list
    /// rather than one key because Rust's `impl Trait for Type` names its own type
    /// by a spelling that may be bare or module-qualified, exactly like the base.
    pub(crate) from: Vec<NodeKey>,
    /// Candidate identities of the BASE type, most specific first.
    pub(crate) to: Vec<NodeKey>,
    pub(crate) kind: RelationKind,
    /// Anchored at the base type's own line — the bytes the relation was read
    /// from, the same discipline `DocComment` follows (and, for the ordinary
    /// single-line declaration, the declaration's line).
    pub(crate) prov: Provenance,
}

/// Which relation a declaration names — or, in C#, that the syntax does not say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RelationKind {
    Extends,
    Implements,
    /// A C# base list writes a base class and an implemented interface
    /// identically (`class D : B, IFoo`). Decided at flush time from the resolved
    /// target's own `SymbolKind` fact — never from an `I`-prefix naming
    /// convention, which would be a guess the graph must not make.
    ByTargetKind,
}

/// A doc comment exactly as the source writes it: the text verbatim (only the
/// comment delimiters removed — never the language, the diacritics, the wrapping
/// or the indentation, R10) plus the 1-based inclusive line range of the
/// *comment*, which is not the declaration's range.
struct DocText {
    text: String,
    start_line: u32,
    end_line: u32,
}

/// One symbol about to be emitted: its identity inputs plus the facts read off
/// the declaration. A record rather than a long argument list, because U2 gives
/// every symbol a signature, a visibility and a doc comment.
struct SymbolDecl<'a> {
    /// Language tag of the identity (KTD3): `rust`, `ts`, `python`, `csharp`, `go`.
    lang: &'static str,
    /// Module path the identity is rooted at (KTD3) — never the file path except
    /// as the documented fallback.
    module: &'a str,
    /// Dotted path of the ENCLOSING types, `""` at module scope. Its last segment
    /// becomes the `SymbolContainer` fact; the whole path is what a nested type's
    /// own identity and its members' identities are built on.
    type_path: &'a str,
    name: &'a str,
    /// `"function"`, `"class"`, `"method"`, `"property"`, `"field"`, …
    kind: &'a str,
    /// `Some("(int,string)")` for a callable — the normalized parameter list that
    /// keeps overloads apart (KTD3). `None` for types, for value members
    /// (property, field, enum variant) and for synthetic entry points, none of
    /// which have a parameter list at all.
    params: Option<String>,
    is_entry: bool,
}

impl<'a> SymbolDecl<'a> {
    /// A type, an entry point or any other symbol identified by name alone.
    fn named(
        lang: &'static str,
        module: &'a str,
        type_path: &'a str,
        name: &'a str,
        kind: &'a str,
    ) -> Self {
        SymbolDecl {
            lang,
            module,
            type_path,
            name,
            kind,
            params: None,
            is_entry: false,
        }
    }

    /// A callable — its identity carries the normalized parameter list, so
    /// overloads never merge.
    fn callable(
        lang: &'static str,
        module: &'a str,
        type_path: &'a str,
        name: &'a str,
        kind: &'a str,
        node: Node,
        src: &[u8],
    ) -> Self {
        SymbolDecl {
            lang,
            module,
            type_path,
            name,
            kind,
            params: Some(param_list(node, src)),
            is_entry: false,
        }
    }

    fn entry(mut self, is_entry: bool) -> Self {
        self.is_entry = is_entry;
        self
    }
}

/// The identity of a type (or of any symbol identified by name alone) under
/// `lang`/`module`. Used to address a member's owner for the `HasMember` edge.
fn type_key(lang: &str, module: &str, type_path: &str) -> NodeKey {
    NodeKey::new(format!("sym:{lang}:{module}#{type_path}"))
}

// ---- Per-language entry points ----

/// Pass 1 for one file: emit its Symbol nodes, facts, `Declares`/`HasMember` and
/// `Imports` edges, and hand back the type relations it NAMES (resolved once the
/// whole repo has been walked).
pub(crate) fn extract_declarations(
    lang: Language,
    file: &str,
    src: &[u8],
    out: &mut FileOutput,
    paths: &PathKeys,
    prefixes: &SecretPrefixes,
) {
    match lang {
        Language::Rust => extract_rust(file, src, out, paths, prefixes),
        Language::TypeScript | Language::Tsx => {
            if let Some(grammar) = lang.grammar() {
                extract_ts(file, src, out, paths, grammar, prefixes)
            }
        }
        Language::JavaScript => {
            if let Some(grammar) = lang.grammar() {
                extract_ecma(file, src, out, paths, grammar, "js", prefixes)
            }
        }
        Language::Java => extract_java(file, src, out, prefixes),
        Language::Python => extract_python(file, src, out, paths, prefixes),
        Language::CSharp => extract_csharp(file, src, out, prefixes),
        Language::Go => extract_go(file, src, out, prefixes),
    }
}

/// Parse Rust source and emit Symbol nodes + facts + `Declares`/`HasMember`
/// edges. A parser or parse failure yields no facts for the file, never a panic
/// (E4).
fn extract_rust(
    file: &str,
    src: &[u8],
    out: &mut FileOutput,
    paths: &PathKeys,
    prefixes: &SecretPrefixes,
) {
    let Some(language) = Language::Rust.grammar() else {
        return;
    };
    let Some(tree) = parse(src, language) else {
        return;
    };
    let root = tree.root_node();
    let module = rust_module_path(file);
    let mut st = EmitState::new(prefixes.clone());
    rust_file_doc(file, src, root, out, prefixes);
    walk_rust(file, src, root, &module, "", out, &mut st);
    flush_members(out, &mut st);
    let mut imported: HashSet<String> = HashSet::new();
    walk_rust_imports(file, src, root, out, paths, &mut imported, false);
    out.relations = st.relations;
}

/// Parse TypeScript or TSX (same node kinds; only the grammar constant differs).
fn extract_ts(
    file: &str,
    src: &[u8],
    out: &mut FileOutput,
    paths: &PathKeys,
    language: tree_sitter::Language,
    prefixes: &SecretPrefixes,
) {
    extract_ecma(file, src, out, paths, language, "ts", prefixes)
}

/// Parse any ECMAScript dialect — TypeScript, TSX, JavaScript, JSX — with the
/// one walker they share, under the identity tag `tag` (`ts` or `js`).
fn extract_ecma(
    file: &str,
    src: &[u8],
    out: &mut FileOutput,
    paths: &PathKeys,
    language: tree_sitter::Language,
    tag: &'static str,
    prefixes: &SecretPrefixes,
) {
    let Some(tree) = parse(src, language) else {
        return;
    };
    let root = tree.root_node();
    let module = ts_module_path(file);
    let mut st = EmitState::new(prefixes.clone());
    walk_ts(file, src, root, &module, "", tag, out, &mut st);
    flush_members(out, &mut st);
    let mut imported: HashSet<String> = HashSet::new();
    walk_ts_imports(file, src, root, out, paths, &mut imported);
    out.relations = st.relations;
}

/// Parse Python source. The `if __name__ == "__main__"` guard is surfaced as a
/// synthetic `__main__` entry-point symbol (the language has no named entry).
fn extract_python(
    file: &str,
    src: &[u8],
    out: &mut FileOutput,
    paths: &PathKeys,
    prefixes: &SecretPrefixes,
) {
    let Some(language) = Language::Python.grammar() else {
        return;
    };
    let Some(tree) = parse(src, language) else {
        return;
    };
    let root = tree.root_node();
    let module = python_module_path(file);
    let mut st = EmitState::new(prefixes.clone());
    walk_python(file, src, root, &module, "", out, &mut st);
    flush_members(out, &mut st);
    let mut imported: HashSet<String> = HashSet::new();
    walk_python_imports(file, src, root, out, paths, &mut imported);
    out.relations = st.relations;
}

/// Parse C# source. A `static` method named `Main` is the entry point; a C# 9+
/// program made only of top-level statements (no `Main`) surfaces a synthetic
/// `__top_level__` entry-point symbol — the modern `Program.cs` form.
fn extract_csharp(file: &str, src: &[u8], out: &mut FileOutput, prefixes: &SecretPrefixes) {
    let Some(language) = Language::CSharp.grammar() else {
        return;
    };
    let Some(tree) = parse(src, language) else {
        return;
    };
    let root = tree.root_node();
    // A C# file's module path is its NAMESPACE; the file stem only stands in
    // while no namespace declaration has been seen (and for a namespace-less
    // file it is the honest fallback — it also keeps two global-namespace types
    // of the same name in different files from converging onto one node).
    let module = path_stem(file);
    let mut st = EmitState::new(prefixes.clone());
    walk_csharp(file, src, root, &module, "", out, &mut st);

    // C# 9+ top-level statements: the compilation unit holds `global_statement`
    // nodes and no `Main` method. The first one anchors a single synthetic
    // entry-point symbol (the language has no named entry here, like Python's
    // `__main__` guard). A file with a `Main` method has no `global_statement`,
    // so the two entry forms never double-count.
    let mut cursor = root.walk();
    for c in root.named_children(&mut cursor) {
        if c.kind() == "global_statement" {
            emit_symbol(
                file,
                SymbolDecl::named("csharp", &module, "", "__top_level__", "entrypoint").entry(true),
                c,
                src,
                out,
                &mut st,
            );
            break;
        }
    }
    flush_members(out, &mut st);
    out.relations = st.relations;
}

/// Parse Java source. The `package` declaration is the module path; a file
/// without one falls back to its path stem, like a namespace-less C# file.
fn extract_java(file: &str, src: &[u8], out: &mut FileOutput, prefixes: &SecretPrefixes) {
    let Some(language) = Language::Java.grammar() else {
        return;
    };
    let Some(tree) = parse(src, language) else {
        return;
    };
    let root = tree.root_node();
    let mut module = path_stem(file);
    let mut cursor = root.walk();
    for c in root.named_children(&mut cursor) {
        if c.kind() == "package_declaration" {
            let mut inner = c.walk();
            let name = c
                .named_children(&mut inner)
                .find(|n| matches!(n.kind(), "scoped_identifier" | "identifier"))
                .and_then(|n| n.utf8_text(src).ok())
                .map(collapse_ws);
            if let Some(name) = name {
                module = name;
            }
        }
    }
    let mut st = EmitState::new(prefixes.clone());
    walk_java(file, src, root, &module, "", out, &mut st);
    flush_members(out, &mut st);
    out.relations = st.relations;
}

/// Walk Java. Types own their members; an enum's constants are its variants,
/// and a `public static void main` is the entry point.
fn walk_java(
    file: &str,
    src: &[u8],
    node: Node,
    module: &str,
    type_path: &str,
    out: &mut FileOutput,
    st: &mut EmitState,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "class_declaration"
            | "interface_declaration"
            | "enum_declaration"
            | "record_declaration"
            | "annotation_type_declaration" => {
                let Some(name) = field_text(child, "name", src) else {
                    continue;
                };
                let kind = match child.kind() {
                    "class_declaration" => "class",
                    "enum_declaration" => "enum",
                    "record_declaration" => "record",
                    _ => "interface",
                };
                emit_symbol(
                    file,
                    SymbolDecl::named("java", module, type_path, &name, kind),
                    child,
                    src,
                    out,
                    st,
                );
                let own = join_type_path(type_path, &name);
                java_base_relations(file, src, child, module, &own, st);
                if let Some(body) = child.child_by_field_name("body") {
                    walk_java(file, src, body, module, &own, out, st);
                }
            }
            // An enum body holds its constants, then optionally ordinary members.
            "enum_body_declarations" => walk_java(file, src, child, module, type_path, out, st),
            "enum_constant" => {
                if let Some(name) = field_text(child, "name", src) {
                    emit_symbol(
                        file,
                        SymbolDecl::named("java", module, type_path, &name, "variant"),
                        child,
                        src,
                        out,
                        st,
                    );
                }
            }
            "method_declaration" => {
                if let Some(name) = field_text(child, "name", src) {
                    let is_entry = name == "main" && java_has_modifier(child, "static", src);
                    emit_symbol(
                        file,
                        SymbolDecl::callable(
                            "java", module, type_path, &name, "method", child, src,
                        )
                        .entry(is_entry),
                        child,
                        src,
                        out,
                        st,
                    );
                }
            }
            "constructor_declaration" | "compact_constructor_declaration" => {
                if let Some(name) = field_text(child, "name", src) {
                    emit_symbol(
                        file,
                        SymbolDecl::callable(
                            "java",
                            module,
                            type_path,
                            &name,
                            "constructor",
                            child,
                            src,
                        ),
                        child,
                        src,
                        out,
                        st,
                    );
                }
            }
            "field_declaration" | "constant_declaration" => {
                let mut dc = child.walk();
                let names: Vec<String> = child
                    .named_children(&mut dc)
                    .filter(|d| d.kind() == "variable_declarator")
                    .filter_map(|d| field_text(d, "name", src))
                    .collect();
                for name in names {
                    emit_symbol(
                        file,
                        SymbolDecl::named("java", module, type_path, &name, "field"),
                        child,
                        src,
                        out,
                        st,
                    );
                }
            }
            _ => {}
        }
    }
}

/// Whether a Java declaration's `modifiers` include `word`.
fn java_has_modifier(node: Node, word: &str, src: &[u8]) -> bool {
    let mut cursor = node.walk();
    let found = node.children(&mut cursor).any(|c| {
        c.kind() == "modifiers" && {
            let mut inner = c.walk();
            let hit = c
                .children(&mut inner)
                .any(|m| m.utf8_text(src).ok() == Some(word));
            hit
        }
    });
    found
}

/// A Java type spelling: `Foo`, `Foo<T>` → `Foo`, `a.b.Foo` kept qualified.
fn java_type_path(node: Node, src: &[u8]) -> Option<String> {
    match node.kind() {
        "type_identifier" | "identifier" => node.utf8_text(src).ok().map(str::to_string),
        "scoped_type_identifier" => node.utf8_text(src).ok().map(collapse_ws),
        "generic_type" => java_type_path(node.named_child(0)?, src),
        _ => None,
    }
}

/// Java heritage: a class's `extends` (one superclass) and `implements` list;
/// an interface's `extends` list, which is interface inheritance.
fn java_base_relations(
    file: &str,
    src: &[u8],
    decl: Node,
    module: &str,
    own_path: &str,
    st: &mut EmitState,
) {
    let mut cursor = decl.walk();
    for clause in decl.named_children(&mut cursor) {
        let kind = match clause.kind() {
            "superclass" | "extends_interfaces" => RelationKind::Extends,
            "super_interfaces" => RelationKind::Implements,
            _ => continue,
        };
        // Each clause holds its types directly or inside a `type_list`.
        let mut stack = vec![clause];
        while let Some(n) = stack.pop() {
            let mut inner = n.walk();
            for t in n.named_children(&mut inner) {
                if t.kind() == "type_list" {
                    stack.push(t);
                } else if let Some(path) = java_type_path(t, src) {
                    push_relation(st, file, "java", module, own_path, t, &path, kind);
                }
            }
        }
    }
}

/// Parse Go source. A top-level `func main` is the entry point.
fn extract_go(file: &str, src: &[u8], out: &mut FileOutput, prefixes: &SecretPrefixes) {
    let Some(language) = Language::Go.grammar() else {
        return;
    };
    let Some(tree) = parse(src, language) else {
        return;
    };
    let root = tree.root_node();
    let module = go_module_path(file);
    let mut st = EmitState::new(prefixes.clone());
    walk_go(file, src, root, &module, out, &mut st);
    flush_members(out, &mut st);
    // Go names no relation at all — see `flush_type_relations`.
    out.relations = st.relations;
}

// ---- Module paths (the KTD3 identity root) ----

/// A path with its final extension removed — the fallback module path when a
/// language's own module notion cannot be determined.
fn path_stem(file: &str) -> String {
    match file.rsplit_once('.') {
        // Only a trailing extension, never a dot inside a directory name.
        Some((stem, ext)) if !ext.contains('/') && !stem.is_empty() => stem.to_string(),
        _ => file.to_string(),
    }
}

/// A Rust file's module path, `crate::a::b`, rooted at the nearest ancestor
/// `src/` (the same anchor `crate::` imports resolve against). A module root file
/// (`lib.rs`/`main.rs`/`mod.rs`) *is* its module, so its own stem drops out.
/// Where no `src/` ancestor exists the crate root cannot be determined, so the
/// file path without its extension stands in.
fn rust_module_path(file: &str) -> String {
    let stem = path_stem(file);
    let Some(root) = crate_root_dir(file) else {
        return stem;
    };
    let rel = stem.strip_prefix(&format!("{root}/")).unwrap_or(&stem);
    let mut comps: Vec<&str> = rel.split('/').filter(|c| !c.is_empty()).collect();
    if matches!(comps.last().copied(), Some("mod" | "lib" | "main")) {
        comps.pop();
    }
    if comps.is_empty() {
        "crate".to_string()
    } else {
        format!("crate::{}", comps.join("::"))
    }
}

/// A TS/TSX module path: the file path itself. A TypeScript module *is* its
/// file, so there is nothing else to root the identity at — and the extension
/// stays on, because `a.ts` and `a.tsx` are two modules, not one, and dropping it
/// would silently merge their symbols onto shared nodes.
fn ts_module_path(file: &str) -> String {
    file.to_string()
}

/// A Python module path, `pkg.mod`. A `__init__.py` names its package rather
/// than a module inside it.
fn python_module_path(file: &str) -> String {
    let stem = path_stem(file);
    let stem = stem.strip_suffix("/__init__").unwrap_or(&stem);
    stem.replace('/', ".")
}

/// A Go module path: the package directory, since a Go package *is* a directory.
/// `.` at the repo root, matching the root module key.
fn go_module_path(file: &str) -> String {
    let dir = parent_dir(file);
    if dir.is_empty() {
        ".".to_string()
    } else {
        dir
    }
}

// ---- Walkers ----

/// Walk a Rust node's named children, emitting symbols. `module` is the identity
/// root (extended by an inline `mod`), `type_path` the enclosing type path ("" at
/// module scope). Recurses into `impl` bodies so methods carry their type, and
/// into struct/enum bodies so fields and variants become members (KTD5).
#[allow(clippy::too_many_arguments)]
fn walk_rust(
    file: &str,
    src: &[u8],
    node: Node,
    module: &str,
    type_path: &str,
    out: &mut FileOutput,
    st: &mut EmitState,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "function_item" => {
                if let Some(name) = field_text(child, "name", src) {
                    let is_entry = type_path.is_empty() && name == "main";
                    let kind = if type_path.is_empty() {
                        "function"
                    } else {
                        "method"
                    };
                    emit_symbol(
                        file,
                        SymbolDecl::callable("rust", module, type_path, &name, kind, child, src)
                            .entry(is_entry),
                        child,
                        src,
                        out,
                        st,
                    );
                }
            }
            "struct_item" | "enum_item" | "trait_item" | "union_item" | "type_item" => {
                if let Some(name) = field_text(child, "name", src) {
                    let kind = match child.kind() {
                        "struct_item" => "struct",
                        "enum_item" => "enum",
                        "trait_item" => "trait",
                        "union_item" => "union",
                        _ => "type",
                    };
                    emit_symbol(
                        file,
                        SymbolDecl::named("rust", module, type_path, &name, kind),
                        child,
                        src,
                        out,
                        st,
                    );
                    let own = join_type_path(type_path, &name);
                    // A supertrait bound is inheritance between traits — the same
                    // declared source shape as a TS `interface A extends B`. A
                    // lifetime or a `?Sized` in the bound list names no type, so
                    // `rust_type_path` filters it out.
                    if child.kind() == "trait_item" {
                        if let Some(bounds) = child.child_by_field_name("bounds") {
                            let mut bc = bounds.walk();
                            for bound in bounds.named_children(&mut bc) {
                                if let Some(path) = rust_type_path(bound, src) {
                                    push_relation(
                                        st,
                                        file,
                                        "rust",
                                        module,
                                        &own,
                                        bound,
                                        &path,
                                        RelationKind::Extends,
                                    );
                                }
                            }
                        }
                    }
                    // Fields / variants / trait items are members of this type.
                    if let Some(body) = child.child_by_field_name("body") {
                        walk_rust_members(file, src, body, module, &own, out, st);
                    }
                }
            }
            "impl_item" => {
                // The impl's type becomes the type path for its methods.
                let ty = field_text(child, "type", src).unwrap_or_default();
                // `impl Trait for Type` implements the trait; a bare `impl Type`
                // is an inherent impl and names no relation at all. Both ends are
                // spellings here — the implementing type may itself be written
                // module-qualified — so both are resolved as candidates.
                if let Some(trait_node) = child.child_by_field_name("trait") {
                    let self_path = child
                        .child_by_field_name("type")
                        .and_then(|n| rust_type_path(n, src));
                    if let (Some(self_path), Some(trait_path)) =
                        (self_path, rust_type_path(trait_node, src))
                    {
                        st.relations.push(TypeRelation {
                            from: base_candidates("rust", module, &self_path),
                            to: base_candidates("rust", module, &trait_path),
                            kind: RelationKind::Implements,
                            prov: relation_prov(file, trait_node),
                        });
                    }
                }
                if let Some(body) = child.child_by_field_name("body") {
                    walk_rust(file, src, body, module, &ty, out, st);
                }
            }
            "mod_item" => {
                // An inline module extends the identity's module path.
                if let Some(body) = child.child_by_field_name("body") {
                    if let Some(name) = field_text(child, "name", src) {
                        let inner = format!("{module}::{name}");
                        walk_rust(file, src, body, &inner, type_path, out, st);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Rust struct fields, enum variants and trait items — the members of the type
/// whose body this is.
#[allow(clippy::too_many_arguments)]
fn walk_rust_members(
    file: &str,
    src: &[u8],
    body: Node,
    module: &str,
    type_path: &str,
    out: &mut FileOutput,
    st: &mut EmitState,
) {
    let mut cursor = body.walk();
    for child in body.named_children(&mut cursor) {
        match child.kind() {
            "field_declaration" => {
                if let Some(name) = field_text(child, "name", src) {
                    emit_symbol(
                        file,
                        SymbolDecl::named("rust", module, type_path, &name, "field"),
                        child,
                        src,
                        out,
                        st,
                    );
                }
            }
            "enum_variant" => {
                if let Some(name) = field_text(child, "name", src) {
                    emit_symbol(
                        file,
                        SymbolDecl::named("rust", module, type_path, &name, "variant"),
                        child,
                        src,
                        out,
                        st,
                    );
                }
            }
            // A trait's required/provided methods and associated items.
            "function_item" | "function_signature_item" => {
                if let Some(name) = field_text(child, "name", src) {
                    emit_symbol(
                        file,
                        SymbolDecl::callable(
                            "rust", module, type_path, &name, "method", child, src,
                        ),
                        child,
                        src,
                        out,
                        st,
                    );
                }
            }
            _ => {}
        }
    }
}

/// The file's own inner doc (`//!`): it documents the module, not any item, so it
/// lands on the File node — the node that stands for the module here.
fn rust_file_doc(
    file: &str,
    src: &[u8],
    root: Node,
    out: &mut FileOutput,
    prefixes: &SecretPrefixes,
) {
    let mut lines: Vec<String> = Vec::new();
    let mut start = 0u32;
    let mut end = 0u32;
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        if child.kind() != "line_comment" && child.kind() != "block_comment" {
            break;
        }
        // `inner:` is the `//!` marker; anything else (an outer `///`, a plain
        // comment) belongs to the item below it, not to the file.
        if child.child_by_field_name("inner").is_none() {
            break;
        }
        let Some(doc) = child.child_by_field_name("doc") else {
            break;
        };
        let Ok(text) = doc.utf8_text(src) else { break };
        if lines.is_empty() {
            start = (child.start_position().row + 1) as u32;
        }
        end = node_end_line(child);
        lines.push(strip_one_space(text.trim_end_matches('\n')));
    }
    if lines.is_empty() {
        return;
    }
    let key = NodeKey::new(file.to_string());
    // Same R22 filter as the per-symbol doc comment: redact before the out.
    let (text, _) = ra_redact::redact_secret_block(&lines.join("\n"), prefixes);
    out.fact(
        key,
        Fact::new(
            FactKind::DocComment,
            FactValue::Text(text),
            Provenance::new(file, None, ids::treesitter(), Tier::Measured).with_range(start, end),
        ),
    );
}

/// Walk TypeScript/TSX. `export` wrappers are transparent (recurse at the same
/// type path). Classes become the owner of their methods and fields (KTD5).
#[allow(clippy::too_many_arguments)]
fn walk_ts(
    file: &str,
    src: &[u8],
    node: Node,
    module: &str,
    type_path: &str,
    tag: &'static str,
    out: &mut FileOutput,
    st: &mut EmitState,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            // `export function/class/…` — the declaration is a child; recurse
            // transparently so it is emitted at the same type path.
            "export_statement" => walk_ts(file, src, child, module, type_path, tag, out, st),
            "function_declaration" | "generator_function_declaration" => {
                if let Some(name) = field_text(child, "name", src) {
                    emit_symbol(
                        file,
                        SymbolDecl::callable(tag, module, type_path, &name, "function", child, src),
                        child,
                        src,
                        out,
                        st,
                    );
                }
            }
            "class_declaration" => {
                if let Some(name) = field_text(child, "name", src) {
                    emit_symbol(
                        file,
                        SymbolDecl::named(tag, module, type_path, &name, "class"),
                        child,
                        src,
                        out,
                        st,
                    );
                    let own = join_type_path(type_path, &name);
                    ts_heritage_relations(file, src, child, module, &own, tag, st);
                    if let Some(body) = child.child_by_field_name("body") {
                        walk_ts(file, src, body, module, &own, tag, out, st);
                    }
                }
            }
            // Class-body members: they carry the class as their owner.
            "method_definition" => {
                if let Some(name) = field_text(child, "name", src) {
                    emit_symbol(
                        file,
                        SymbolDecl::callable(tag, module, type_path, &name, "method", child, src),
                        child,
                        src,
                        out,
                        st,
                    );
                }
            }
            "public_field_definition" => {
                if let Some(name) = field_text(child, "name", src) {
                    emit_symbol(
                        file,
                        SymbolDecl::named(tag, module, type_path, &name, "field"),
                        child,
                        src,
                        out,
                        st,
                    );
                }
            }
            // JavaScript's class field: the name sits under `property`, not `name`.
            "field_definition" => {
                if let Some(name) = field_text(child, "property", src) {
                    emit_symbol(
                        file,
                        SymbolDecl::named(tag, module, type_path, &name, "field"),
                        child,
                        src,
                        out,
                        st,
                    );
                }
            }
            // `const f = (…) => …`, `let g = function (…) { … }` — in modern
            // TypeScript and JavaScript most functions are written this way, and
            // a walker that saw only `function` declarations missed them all.
            // Module scope only: a class body holds fields, not declarations.
            "lexical_declaration" | "variable_declaration" if type_path.is_empty() => {
                let mut dc = child.walk();
                let declarators: Vec<Node> = child
                    .named_children(&mut dc)
                    .filter(|d| d.kind() == "variable_declarator")
                    .collect();
                // One declarator: the statement is the declaration, so its doc
                // comment and its `export` are found where they are written.
                let single = declarators.len() == 1;
                for d in declarators {
                    let Some(value) = d.child_by_field_name("value") else {
                        continue;
                    };
                    if !matches!(
                        value.kind(),
                        "arrow_function"
                            | "function_expression"
                            | "function"
                            | "generator_function"
                    ) {
                        continue;
                    }
                    let Some(name_node) = d.child_by_field_name("name") else {
                        continue;
                    };
                    if name_node.kind() != "identifier" {
                        continue;
                    }
                    let Ok(name) = name_node.utf8_text(src) else {
                        continue;
                    };
                    let name = name.to_string();
                    emit_symbol(
                        file,
                        SymbolDecl::callable(tag, module, type_path, &name, "function", value, src),
                        if single { child } else { d },
                        src,
                        out,
                        st,
                    );
                }
            }
            "interface_declaration" => {
                if let Some(name) = field_text(child, "name", src) {
                    emit_symbol(
                        file,
                        SymbolDecl::named(tag, module, type_path, &name, "interface"),
                        child,
                        src,
                        out,
                        st,
                    );
                    let own = join_type_path(type_path, &name);
                    ts_heritage_relations(file, src, child, module, &own, tag, st);
                }
            }
            "type_alias_declaration" => {
                if let Some(name) = field_text(child, "name", src) {
                    emit_symbol(
                        file,
                        SymbolDecl::named(tag, module, type_path, &name, "type"),
                        child,
                        src,
                        out,
                        st,
                    );
                }
            }
            "enum_declaration" => {
                if let Some(name) = field_text(child, "name", src) {
                    emit_symbol(
                        file,
                        SymbolDecl::named(tag, module, type_path, &name, "enum"),
                        child,
                        src,
                        out,
                        st,
                    );
                }
            }
            _ => {}
        }
    }
}

/// Walk Python. Classes become the owner of their methods and of nested classes;
/// a top-level `if __name__ == "__main__"` becomes a synthetic `__main__`
/// entry-point symbol. `@decorator` wrappers are transparent.
#[allow(clippy::too_many_arguments)]
fn walk_python(
    file: &str,
    src: &[u8],
    node: Node,
    module: &str,
    type_path: &str,
    out: &mut FileOutput,
    st: &mut EmitState,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "decorated_definition" => walk_python(file, src, child, module, type_path, out, st),
            "function_definition" => {
                if let Some(name) = field_text(child, "name", src) {
                    let kind = if type_path.is_empty() {
                        "function"
                    } else {
                        "method"
                    };
                    emit_symbol(
                        file,
                        SymbolDecl::callable("python", module, type_path, &name, kind, child, src),
                        child,
                        src,
                        out,
                        st,
                    );
                }
            }
            "class_definition" => {
                if let Some(name) = field_text(child, "name", src) {
                    emit_symbol(
                        file,
                        SymbolDecl::named("python", module, type_path, &name, "class"),
                        child,
                        src,
                        out,
                        st,
                    );
                    let own = join_type_path(type_path, &name);
                    // Every Python base is inheritance — the language has no
                    // interfaces, so there is nothing to `Implements`. A
                    // `metaclass=`/`**kwargs` entry in the list is not a base, and
                    // `python_type_path` filters it out.
                    if let Some(supers) = child.child_by_field_name("superclasses") {
                        let mut sc = supers.walk();
                        for base in supers.named_children(&mut sc) {
                            if let Some(path) = python_type_path(base, src) {
                                push_relation(
                                    st,
                                    file,
                                    "python",
                                    module,
                                    &own,
                                    base,
                                    &path,
                                    RelationKind::Extends,
                                );
                            }
                        }
                    }
                    if let Some(body) = child.child_by_field_name("body") {
                        walk_python(file, src, body, module, &own, out, st);
                    }
                }
            }
            // Language-level entry point: only at module scope, only the
            // `__name__ == "__main__"` guard (KTD honesty — not any `if`).
            "if_statement" if type_path.is_empty() => {
                if let Some(cond) = child.child_by_field_name("condition") {
                    let text = cond.utf8_text(src).unwrap_or("");
                    if text.contains("__name__") && text.contains("__main__") {
                        emit_symbol(
                            file,
                            SymbolDecl::named("python", module, "", "__main__", "entrypoint")
                                .entry(true),
                            child,
                            src,
                            out,
                            st,
                        );
                    }
                }
            }
            _ => {}
        }
    }
}

/// Walk C#. A namespace sets the identity's module path (block-bodied by
/// recursion, file-scoped for every following sibling); types become the owner of
/// their members. A `static` method named `Main` is an entry point.
#[allow(clippy::too_many_arguments)]
fn walk_csharp(
    file: &str,
    src: &[u8],
    node: Node,
    module: &str,
    type_path: &str,
    out: &mut FileOutput,
    st: &mut EmitState,
) {
    // File-scoped `namespace N;` declares the namespace for everything that
    // follows it as a SIBLING, so the module path is mutable across this loop.
    let mut module = module.to_string();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            // Block-bodied namespace: recurse into its body under the namespace.
            "namespace_declaration" => {
                if let (Some(name), Some(body)) = (
                    field_text(child, "name", src),
                    child.child_by_field_name("body"),
                ) {
                    let inner = join_namespace(&module, &name, node);
                    walk_csharp(file, src, body, &inner, type_path, out, st);
                }
            }
            // `namespace N;` — no body; it renames the module for its siblings.
            "file_scoped_namespace_declaration" => {
                if let Some(name) = field_text(child, "name", src) {
                    module = name;
                }
            }
            "class_declaration"
            | "struct_declaration"
            | "record_declaration"
            | "record_struct_declaration"
            | "interface_declaration"
            | "enum_declaration" => {
                if let Some(name) = field_text(child, "name", src) {
                    let kind = match child.kind() {
                        "class_declaration" => "class",
                        "struct_declaration" => "struct",
                        "record_declaration" | "record_struct_declaration" => "record",
                        "interface_declaration" => "interface",
                        _ => "enum",
                    };
                    emit_symbol(
                        file,
                        SymbolDecl::named("csharp", &module, type_path, &name, kind),
                        child,
                        src,
                        out,
                        st,
                    );
                    let own = join_type_path(type_path, &name);
                    // An enum's "base list" is its underlying integral type
                    // (`enum E : byte`), which is not inheritance at all.
                    if child.kind() != "enum_declaration" {
                        csharp_base_relations(
                            file,
                            src,
                            child,
                            &module,
                            &own,
                            kind == "interface",
                            st,
                        );
                    }
                    // Recurse into the type body so members carry it as owner.
                    if let Some(body) = child.child_by_field_name("body") {
                        walk_csharp(file, src, body, &module, &own, out, st);
                    }
                }
            }
            "method_declaration" => {
                if let Some(name) = field_text(child, "name", src) {
                    // Only a `static Main` is an entry point — a random instance
                    // method named `Main` is not the program entry.
                    let is_entry = name == "Main" && has_modifier(child, "static", src);
                    emit_symbol(
                        file,
                        SymbolDecl::callable(
                            "csharp", &module, type_path, &name, "method", child, src,
                        )
                        .entry(is_entry),
                        child,
                        src,
                        out,
                        st,
                    );
                }
            }
            "constructor_declaration" => {
                if let Some(name) = field_text(child, "name", src) {
                    emit_symbol(
                        file,
                        SymbolDecl::callable(
                            "csharp",
                            &module,
                            type_path,
                            &name,
                            "constructor",
                            child,
                            src,
                        ),
                        child,
                        src,
                        out,
                        st,
                    );
                }
            }
            "property_declaration" => {
                if let Some(name) = field_text(child, "name", src) {
                    emit_symbol(
                        file,
                        SymbolDecl::named("csharp", &module, type_path, &name, "property"),
                        child,
                        src,
                        out,
                        st,
                    );
                }
            }
            "event_field_declaration" | "field_declaration" => {
                let kind = if child.kind() == "field_declaration" {
                    "field"
                } else {
                    "event"
                };
                // `int a, b;` declares two members off one declaration; the
                // declaration node carries the range and the signature of both.
                for name in csharp_declarator_names(child, src) {
                    emit_symbol(
                        file,
                        SymbolDecl::named("csharp", &module, type_path, &name, kind),
                        child,
                        src,
                        out,
                        st,
                    );
                }
            }
            "enum_member_declaration" => {
                if let Some(name) = field_text(child, "name", src) {
                    emit_symbol(
                        file,
                        SymbolDecl::named("csharp", &module, type_path, &name, "variant"),
                        child,
                        src,
                        out,
                        st,
                    );
                }
            }
            _ => {}
        }
    }
}

/// Every `variable_declarator` name under a C# field/event declaration.
fn csharp_declarator_names(node: Node, src: &[u8]) -> Vec<String> {
    let mut names = Vec::new();
    let mut cursor = node.walk();
    for decl in node.named_children(&mut cursor) {
        if decl.kind() != "variable_declaration" {
            continue;
        }
        let mut inner = decl.walk();
        for d in decl.named_children(&mut inner) {
            if d.kind() == "variable_declarator" {
                if let Some(name) = field_text(d, "name", src) {
                    names.push(name);
                }
            }
        }
    }
    names
}

/// Append a nested namespace segment. A namespace nested inside a *block* body
/// composes (`namespace A { namespace B { … } }` is `A.B`); a namespace at the
/// compilation-unit level replaces the file-stem fallback outright.
fn join_namespace(module: &str, name: &str, parent: Node) -> String {
    if parent.kind() == "compilation_unit" {
        name.to_string()
    } else {
        format!("{module}.{name}")
    }
}

/// Walk Go. All declarations are top-level; a method's owner is its receiver
/// type. A top-level `func main` is the entry point.
///
/// No type relation is emitted here, deliberately: Go has no inheritance.
/// Embedding a struct or an interface is COMPOSITION, not subtyping, and which
/// interfaces a type satisfies is implicit — decidable only with type
/// information, i.e. `Resolution::Resolved` territory, never a name heuristic.
fn walk_go(
    file: &str,
    src: &[u8],
    node: Node,
    module: &str,
    out: &mut FileOutput,
    st: &mut EmitState,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "function_declaration" => {
                if let Some(name) = field_text(child, "name", src) {
                    let is_entry = name == "main";
                    emit_symbol(
                        file,
                        SymbolDecl::callable("go", module, "", &name, "function", child, src)
                            .entry(is_entry),
                        child,
                        src,
                        out,
                        st,
                    );
                }
            }
            "method_declaration" => {
                if let Some(name) = field_text(child, "name", src) {
                    let recv = child
                        .child_by_field_name("receiver")
                        .and_then(|r| first_kind_text(r, "type_identifier", src))
                        .unwrap_or_default();
                    emit_symbol(
                        file,
                        SymbolDecl::callable("go", module, &recv, &name, "method", child, src),
                        child,
                        src,
                        out,
                        st,
                    );
                }
            }
            "type_declaration" => {
                let mut spec_cursor = child.walk();
                for spec in child.named_children(&mut spec_cursor) {
                    if spec.kind() != "type_spec" {
                        continue;
                    }
                    if let Some(name) = field_text(spec, "name", src) {
                        let ty = spec.child_by_field_name("type");
                        let kind = match ty.map(|t| t.kind()) {
                            Some("struct_type") => "struct",
                            Some("interface_type") => "interface",
                            _ => "type",
                        };
                        emit_symbol(
                            file,
                            SymbolDecl::named("go", module, "", &name, kind),
                            spec,
                            src,
                            out,
                            st,
                        );
                        // A struct's fields are its members (KTD5). The field
                        // list is an unlabelled child of the struct type.
                        if let Some(fields) = ty
                            .filter(|t| t.kind() == "struct_type")
                            .and_then(|t| first_child_of_kind(t, "field_declaration_list"))
                        {
                            walk_go_fields(file, src, fields, module, &name, out, st);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// Go struct fields — members of the struct whose field list this is.
#[allow(clippy::too_many_arguments)]
fn walk_go_fields(
    file: &str,
    src: &[u8],
    fields: Node,
    module: &str,
    type_path: &str,
    out: &mut FileOutput,
    st: &mut EmitState,
) {
    let mut cursor = fields.walk();
    for f in fields.named_children(&mut cursor) {
        if f.kind() != "field_declaration" {
            continue;
        }
        if let Some(name) = field_text(f, "name", src) {
            emit_symbol(
                file,
                SymbolDecl::named("go", module, type_path, &name, "field"),
                f,
                src,
                out,
                st,
            );
        }
    }
}

// ---- Shared node helpers ----

/// Append `name` to a dotted type path, per KTD3's `<Type>.<member>` separator.
fn join_type_path(type_path: &str, name: &str) -> String {
    if type_path.is_empty() {
        name.to_string()
    } else {
        format!("{type_path}.{name}")
    }
}

/// A node's 1-based inclusive END line. A node whose text ends in a newline (a
/// line comment) reports the *next* row at column 0; the last line it actually
/// covers is the row before that.
fn node_end_line(node: Node) -> u32 {
    let end = node.end_position();
    if end.column == 0 && end.row > node.start_position().row {
        end.row as u32
    } else {
        (end.row + 1) as u32
    }
}

/// `(line, column)` where `node` starts, both 1-based, the column counted in
/// characters from the start of its line (contract 0.2). Tree-sitter's own
/// column is in bytes; a byte that continues a UTF-8 sequence is not a new
/// character, which counts a malformed line by its bytes rather than failing.
pub(crate) fn position(node: Node, src: &[u8]) -> (u32, u32) {
    let start = node.start_position();
    let line_start = node.start_byte() - start.column;
    let chars = src[line_start..node.start_byte()]
        .iter()
        .filter(|&&b| b & 0xC0 != 0x80)
        .count();
    ((start.row + 1) as u32, (chars + 1) as u32)
}

/// The identifier that names the declaration `node` under `name`: its own
/// `name` field, or — for a declaration naming several things at once, like
/// a C# `int a, b;` — the first `name` field below it that spells `name`. A
/// `name` field, never any identifier, so `Color Color;` finds the field and
/// not its type. The search goes three levels down (C#'s
/// `field_declaration` → `variable_declaration` → `variable_declarator` is
/// the deepest a declarator sits) and never into a body, so a declaration
/// whose name field is spelled otherwise costs nothing like its size.
fn declared_name_node<'t>(node: Node<'t>, name: &str, src: &[u8]) -> Option<Node<'t>> {
    fn search<'t>(node: Node<'t>, name: &str, src: &[u8], depth: u8) -> Option<Node<'t>> {
        let mut cursor = node.walk();
        let found = node
            .children_by_field_name("name", &mut cursor)
            .find(|n| n.utf8_text(src).ok() == Some(name));
        if found.is_some() || depth == 0 {
            return found;
        }
        let body = node.child_by_field_name("body").map(|b| b.id());
        let mut cursor = node.walk();
        let children: Vec<Node<'t>> = node.named_children(&mut cursor).collect();
        children
            .into_iter()
            .filter(|child| Some(child.id()) != body)
            .find_map(|child| search(child, name, src, depth - 1))
    }
    search(node, name, src, 3)
}

/// The first direct named child of `kind` — for a grammar node that holds its
/// payload as an unlabelled child (a Go `struct_type`'s field list).
fn first_child_of_kind<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    let found = node.named_children(&mut cursor).find(|c| c.kind() == kind);
    found
}

/// Depth-first text of the first descendant (or self) of `kind`.
fn first_kind_text(node: Node, kind: &str, src: &[u8]) -> Option<String> {
    if node.kind() == kind {
        return node.utf8_text(src).ok().map(str::to_string);
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if let Some(found) = first_kind_text(child, kind, src) {
            return Some(found);
        }
    }
    None
}

/// Whether `node` has a direct `modifier` child equal to `modifier` (e.g.
/// `static`). C# represents each modifier keyword as its own `modifier` node.
fn has_modifier(node: Node, modifier: &str, src: &[u8]) -> bool {
    let mut cursor = node.walk();
    for c in node.named_children(&mut cursor) {
        if c.kind() == "modifier" && c.utf8_text(src).map(|t| t == modifier).unwrap_or(false) {
            return true;
        }
    }
    false
}

/// Text of a node's named field, as an owned `String`.
fn field_text(node: Node, field: &str, src: &[u8]) -> Option<String> {
    node.child_by_field_name(field)?
        .utf8_text(src)
        .ok()
        .map(str::to_string)
}

/// Runs of whitespace collapsed to single spaces and the ends trimmed. Applied to
/// every derived-from-source string that feeds an identity or a signature fact,
/// so reformatting a declaration cannot churn either.
fn collapse_ws(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Drop exactly one leading space — a comment marker's separator from its text,
/// never the text's own indentation (R10: no dedent, no reflow).
fn strip_one_space(text: &str) -> String {
    text.strip_prefix(' ')
        .unwrap_or(text)
        .trim_end()
        .to_string()
}

/// The normalized parameter list of a callable, `(int,string)` — the part of a
/// KTD3 identity that keeps overloads apart. Parameter *types* where the grammar
/// has them, the pattern or name where it does not (Python, untyped JS). Every
/// grammar here names the field `parameters`; a callable without one yields `()`.
fn param_list(node: Node, src: &[u8]) -> String {
    let Some(params) = node.child_by_field_name("parameters") else {
        // `x => …`: an arrow function with one bare parameter has no list.
        if let Some(single) = node.child_by_field_name("parameter") {
            if let Ok(text) = single.utf8_text(src) {
                return format!("({})", collapse_ws(text));
            }
        }
        return "()".to_string();
    };
    let mut parts: Vec<String> = Vec::new();
    let mut cursor = params.walk();
    for p in params.named_children(&mut cursor) {
        let text = p
            .child_by_field_name("type")
            .or_else(|| p.child_by_field_name("pattern"))
            .or_else(|| p.child_by_field_name("name"))
            .and_then(|n| n.utf8_text(src).ok())
            .or_else(|| p.utf8_text(src).ok())
            .unwrap_or_default();
        // A TS type arrives as a `type_annotation`, i.e. `: T`.
        let text = text.trim_start().trim_start_matches(':');
        parts.push(collapse_ws(text));
    }
    format!("({})", parts.join(","))
}

/// The declaration header the source writes, whitespace-collapsed: what a card
/// quotes as the signature. Everything before the body, so a 400-line method
/// yields its one-line signature and never its implementation.
fn signature_header(node: Node, src: &[u8]) -> Option<String> {
    let text = node.utf8_text(src).ok()?;
    // The body under its usual field name; a C# property calls its own
    // `accessors`, and a Go `type_spec` embeds the struct literal directly — for
    // those, the first brace is the boundary.
    let cut = node
        .child_by_field_name("body")
        .or_else(|| node.child_by_field_name("accessors"))
        .or_else(|| declared_function_body(node))
        .map(|b| b.start_byte().saturating_sub(node.start_byte()))
        .or_else(|| text.find('{'))
        .unwrap_or(text.len());
    let head = collapse_ws(text.get(..cut).unwrap_or(text));
    (!head.is_empty()).then_some(head)
}

/// The body of the function a `const f = (…) => …` declaration holds, so its
/// signature stops at `=>` instead of carrying the implementation.
fn declared_function_body(node: Node) -> Option<Node> {
    let declarator = match node.kind() {
        "variable_declarator" => node,
        "lexical_declaration" | "variable_declaration" => {
            let mut c = node.walk();
            let found = node
                .named_children(&mut c)
                .find(|d| d.kind() == "variable_declarator")?;
            found
        }
        _ => return None,
    };
    declarator
        .child_by_field_name("value")?
        .child_by_field_name("body")
}

/// The access modifier the source actually states, verbatim: Rust
/// `pub`/`pub(crate)`, C#'s access `modifier` keywords, a TS
/// `accessibility_modifier` or the `export` that wraps a module-scope
/// declaration. `None` when the source states none — Go and Python express
/// visibility by naming convention, and reading a convention is interpretation,
/// not a measured fact (E9), so they emit no visibility at all.
fn visibility_text(node: Node, src: &[u8]) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    let mut cursor = node.walk();
    for c in node.named_children(&mut cursor) {
        match c.kind() {
            "visibility_modifier" | "accessibility_modifier" => {
                if let Ok(t) = c.utf8_text(src) {
                    parts.push(collapse_ws(t));
                }
            }
            // Java groups its modifiers (and annotations) under one node.
            "modifiers" => {
                let mut inner = c.walk();
                for m in c.children(&mut inner) {
                    if let Ok(t) = m.utf8_text(src) {
                        if matches!(t, "public" | "private" | "protected") {
                            parts.push(t.to_string());
                        }
                    }
                }
            }
            // C# states each modifier as its own node; only the ACCESS ones are
            // visibility (`static`, `async`, `partial`, … are not).
            "modifier" => {
                if let Ok(t) = c.utf8_text(src) {
                    if matches!(t, "public" | "private" | "protected" | "internal" | "file") {
                        parts.push(t.to_string());
                    }
                }
            }
            _ => {}
        }
    }
    if !parts.is_empty() {
        return Some(parts.join(" "));
    }
    // TS/JS states no modifier at module scope — `export` is the visibility, and
    // it sits on the wrapping `export_statement`.
    (node.parent().map(|p| p.kind()) == Some("export_statement")).then(|| "export".to_string())
}

/// The doc comment attached to a declaration, verbatim (R10): only the comment
/// DELIMITERS are removed — the inner text, its language, its diacritics and its
/// line breaks are never touched, never reflowed, never translated. `None` when
/// the declaration carries no doc comment, so no empty fact is ever emitted.
fn doc_comment(node: Node, src: &[u8]) -> Option<DocText> {
    python_docstring(node, src).or_else(|| preceding_doc_comment(node, src))
}

/// A Python docstring: the first string statement in a function or class body.
/// The grammar already exposes its content without the quotes, so the text is
/// taken byte-for-byte.
fn python_docstring(node: Node, src: &[u8]) -> Option<DocText> {
    if !matches!(node.kind(), "function_definition" | "class_definition") {
        return None;
    }
    let body = node.child_by_field_name("body")?;
    let mut cursor = body.walk();
    let first = body.named_children(&mut cursor).next()?;
    if first.kind() != "expression_statement" {
        return None;
    }
    let mut inner = first.walk();
    let string = first.named_children(&mut inner).next()?;
    if string.kind() != "string" {
        return None;
    }
    let mut parts = string.walk();
    let content = string
        .named_children(&mut parts)
        .find(|n| n.kind() == "string_content")?;
    Some(DocText {
        text: content.utf8_text(src).ok()?.to_string(),
        start_line: (string.start_position().row + 1) as u32,
        end_line: node_end_line(string),
    })
}

/// The doc comment above a declaration: Rust `///`, C# `///`, TS/JS `/** … */`.
/// Rust's `//!` is deliberately excluded — it documents the ENCLOSING item, so
/// attaching it to the declaration below would misattribute it.
fn preceding_doc_comment(node: Node, src: &[u8]) -> Option<DocText> {
    // A doc comment sits above the whole declaration, so start from the outermost
    // wrapper: TS `export class …`, Python `@decorator def …`.
    let mut anchor = node;
    while matches!(
        anchor.parent().map(|p| p.kind()),
        Some("export_statement") | Some("decorated_definition")
    ) {
        anchor = anchor.parent()?;
    }
    // Collected bottom-up, then reversed into source order.
    let mut collected: Vec<(String, u32, u32)> = Vec::new();
    let mut cur = anchor.prev_named_sibling();
    while let Some(n) = cur {
        // An attribute/annotation sits between the comment and the declaration.
        if !matches!(n.kind(), "attribute_item" | "attribute_list") {
            match doc_delimiters_stripped(n, src) {
                Some(text) => {
                    collected.push((text, (n.start_position().row + 1) as u32, node_end_line(n)))
                }
                None => break,
            }
        }
        cur = n.prev_named_sibling();
    }
    if collected.is_empty() {
        return None;
    }
    collected.reverse();
    let start_line = collected.first()?.1;
    let end_line = collected.last()?.2;
    Some(DocText {
        text: collected
            .into_iter()
            .map(|(t, _, _)| t)
            .collect::<Vec<_>>()
            .join("\n"),
        start_line,
        end_line,
    })
}

/// One comment node with its delimiters removed, or `None` when the comment is
/// not a doc comment — a plain `//` / `/* */`, or Rust's inner `//!`. Go is
/// absent by construction: its doc convention is a plain `//` comment, which is
/// indistinguishable from an ordinary one without interpretation.
fn doc_delimiters_stripped(node: Node, src: &[u8]) -> Option<String> {
    match node.kind() {
        // Rust: the grammar splits marker from content. `outer:` is `///` or
        // `/** */` (documents what follows); `inner:` is `//!`/`/*! */`.
        // Java names its comments the same way but without Rust's fields: a
        // `/** … */` block there is a Javadoc comment.
        "block_comment"
            if node.child_by_field_name("outer").is_none()
                && node.child_by_field_name("inner").is_none()
                && node.child_by_field_name("doc").is_none() =>
        {
            let text = node.utf8_text(src).ok()?;
            text.trim_start()
                .starts_with("/**")
                .then(|| strip_block_frame(text))
        }
        "line_comment" | "block_comment" => {
            node.child_by_field_name("outer")?;
            let doc = node.child_by_field_name("doc")?;
            Some(strip_one_space(
                doc.utf8_text(src).ok()?.trim_end_matches('\n'),
            ))
        }
        // C#/TS/JS: one `comment` node per `///` line, or one whole `/** … */`.
        "comment" => {
            let text = node.utf8_text(src).ok()?;
            let trimmed = text.trim_start();
            if let Some(rest) = trimmed.strip_prefix("///") {
                Some(strip_one_space(rest.trim_end_matches('\n')))
            } else if trimmed.starts_with("/**") {
                Some(strip_block_frame(text))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Remove a `/** … */` frame: the opening and closing delimiters and the per-line
/// `*` gutter. Nothing else — no dedent of the surviving text, no reflow (R10).
fn strip_block_frame(text: &str) -> String {
    let inner = text.trim_start().strip_prefix("/**").unwrap_or(text);
    let inner = inner.trim_end().strip_suffix("*/").unwrap_or(inner);
    let mut out: Vec<String> = Vec::new();
    for (i, line) in inner.lines().enumerate() {
        let trimmed = line.trim_start();
        out.push(match trimmed.strip_prefix('*') {
            Some(rest) => strip_one_space(rest),
            // The text on the opening `/**` line has no gutter of its own.
            None if i == 0 => strip_one_space(line),
            None => line.trim_end().to_string(),
        });
    }
    while out.first().is_some_and(|l| l.trim().is_empty()) {
        out.remove(0);
    }
    while out.last().is_some_and(|l| l.trim().is_empty()) {
        out.pop();
    }
    out.join("\n")
}

/// Emit one Symbol node with its facts, a `Declares` edge from the file, and a
/// pending `HasMember` edge from its owning type when it is a member (KTD5).
///
/// The node key is `sym:` + the KTD3 identity (see the module docs); a residual
/// duplicate identity within one file takes an ordinal `~n` suffix.
fn emit_symbol(
    file: &str,
    decl: SymbolDecl<'_>,
    node: Node,
    src: &[u8],
    out: &mut FileOutput,
    st: &mut EmitState,
) {
    let own_path = join_type_path(decl.type_path, decl.name);
    let params = decl.params.as_deref().unwrap_or("");
    let identity = format!("{}:{}#{own_path}{params}", decl.lang, decl.module);
    let count = st.used.entry(identity.clone()).or_insert(0);
    *count += 1;
    let key_str = if *count == 1 {
        format!("sym:{identity}")
    } else {
        // Two declarations that the fallback identity cannot tell apart (a
        // macro-duplicated item, two `partial` halves in one file). An ordinal
        // keeps the key unique AND stable under reformatting (KTD3).
        format!("sym:{identity}~{count}")
    };

    let key = NodeKey::new(key_str);
    // Everything this declaration contributes travels as one group: creating the
    // node guards the facts and the Declares edge behind it, and the guard fires
    // where the graph is (see `GraphOp::Symbol`).
    let sym = out.symbol(key.clone());

    // The declaration's own line range, carried by provenance (U1) rather than by
    // a second range mechanism. `Resolution` stays the default `Heuristic`:
    // tree-sitter without type resolution cannot claim more (R4).
    let prov = Provenance::new(file, None, ids::treesitter(), Tier::Measured)
        .with_range((node.start_position().row + 1) as u32, node_end_line(node))
        .with_name(declared_name_node(node, decl.name, src).map(|n| position(n, src)));

    sym.push_fact(Fact::new(
        FactKind::SymbolName,
        FactValue::Text(decl.name.to_string()),
        prov.clone(),
    ));
    sym.push_fact(Fact::new(
        FactKind::SymbolKind,
        FactValue::Text(decl.kind.to_string()),
        prov.clone(),
    ));
    if let Some(signature) = signature_header(node, src) {
        // A declaration with no body — a field, a const, a parameter default —
        // keeps its initializer in the header, so the signature is a secret-value
        // carrier just like a doc comment (R22). Same filter, same reason.
        let (signature, _) = ra_redact::redact_secret_block(&signature, &st.secret_prefixes);
        sym.push_fact(Fact::new(
            FactKind::SymbolSignature,
            FactValue::Text(signature),
            prov.clone(),
        ));
    }
    if let Some(visibility) = visibility_text(node, src) {
        sym.push_fact(Fact::new(
            FactKind::SymbolVisibility,
            FactValue::Text(visibility),
            prov.clone(),
        ));
    }
    // The immediate enclosing type — the last segment of the type path, so a
    // nested type reports the type it is nested in, not the whole chain.
    if let Some(container) = decl.type_path.rsplit('.').next().filter(|c| !c.is_empty()) {
        sym.push_fact(Fact::new(
            FactKind::SymbolContainer,
            FactValue::Text(container.to_string()),
            prov.clone(),
        ));
    }
    if decl.is_entry {
        sym.push_fact(Fact::new(
            FactKind::EntryPoint,
            FactValue::Bool(true),
            prov.clone(),
        ));
    }
    // The doc comment is anchored to the COMMENT's own lines, not the
    // declaration's — provenance must point at the bytes that were quoted.
    if let Some(doc) = doc_comment(node, src) {
        // R22 at the source of truth: a secret-shaped line never enters the out,
        // so no projection — card, manifest, any future export — can publish it.
        let (text, _) = ra_redact::redact_secret_block(&doc.text, &st.secret_prefixes);
        sym.push_fact(Fact::new(
            FactKind::DocComment,
            FactValue::Text(text),
            Provenance::new(file, None, ids::treesitter(), Tier::Measured)
                .with_range(doc.start_line, doc.end_line),
        ));
    }

    // file --Declares--> symbol (the file node exists; filetree runs first).
    // Declares (not Contains) so module-level containment consumers never see it.
    let file_key = NodeKey::new(file.to_string());
    sym.push_edge(
        file_key,
        key.clone(),
        Edge::new(EdgeKind::Declares, prov.clone()),
    );

    if !decl.type_path.is_empty() {
        st.members
            .push((type_key(decl.lang, decl.module, decl.type_path), key, prov));
    }
}

/// Add the `type --HasMember--> member` edges collected during a walk. Deferred
/// so a member declared above its own type still finds its owner; a member whose
/// type is not declared in this file (a Rust `impl` of a type from another module)
/// has no owner node and is simply not linked — no phantom type is fabricated.
fn flush_members(out: &mut FileOutput, st: &mut EmitState) {
    for (owner, member, prov) in st.members.drain(..) {
        out.edge(owner, member, Edge::new(EdgeKind::HasMember, prov));
    }
}

// ---- Type relations: extends / implements (U3) ----

/// Point-anchored provenance at a base type's own first line — the bytes the
/// relation was read from, the same discipline `DocComment` follows by anchoring
/// at the comment rather than at the declaration. For the ordinary single-line
/// declaration this IS the declaration's line.
fn relation_prov(file: &str, base: Node) -> Provenance {
    Provenance::new(
        file,
        Some((base.start_position().row + 1) as u32),
        ids::treesitter(),
        Tier::Measured,
    )
}

/// The module separator a language's own qualified type spelling uses, or `None`
/// where a qualifier can never name a module: a TS module path is a FILE path, so
/// `ns.IFoo` names a TS namespace that no module identity can spell.
fn module_separator(lang: &str) -> Option<&'static str> {
    match lang {
        "rust" => Some("::"),
        "csharp" | "python" | "java" => Some("."),
        _ => None,
    }
}

/// Candidate identities a base-type spelling can mean, most specific first.
///
/// A bare `Base` can only mean the declaring type's own module — that is the
/// furthest a name heuristic reaches without guessing (KD2), so a base in another
/// module of the repo goes UNRESOLVED rather than mis-linked. A qualified spelling
/// is genuinely ambiguous and yields both readings: C# `Outer.Inner` may be a
/// nested type of the declaring module (candidate 1, the more local reading) or
/// `Inner` in module `Outer` (candidate 2), and Rust `crate::graph::Provenance`
/// only ever resolves as the second. Whichever candidate exists after the walk
/// wins; a spelling that matches neither is a type this repo does not declare.
fn base_candidates(lang: &'static str, module: &str, path: &str) -> Vec<NodeKey> {
    let mut out = vec![type_key(lang, module, path)];
    if let Some(sep) = module_separator(lang) {
        if let Some((prefix, name)) = path.rsplit_once(sep) {
            if !prefix.is_empty() && !name.is_empty() {
                out.push(type_key(lang, prefix, name));
            }
        }
    }
    out
}

/// Buffer one relation whose deriving side is a type just emitted, so its
/// identity is exact rather than a spelling to resolve.
#[allow(clippy::too_many_arguments)]
fn push_relation(
    st: &mut EmitState,
    file: &str,
    lang: &'static str,
    module: &str,
    own_path: &str,
    base: Node,
    base_path: &str,
    kind: RelationKind,
) {
    st.relations.push(TypeRelation {
        from: vec![type_key(lang, module, own_path)],
        to: base_candidates(lang, module, base_path),
        kind,
        prov: relation_prov(file, base),
    });
}

/// The plain type path a Rust type node names, generic arguments dropped
/// (`Vec<T>` and `Vec<U>` are one type). `None` for anything that is not a
/// nameable type — a reference, a tuple, a `dyn` object, a lifetime in a bound
/// list — because a relation must never be invented for a shape we cannot name.
fn rust_type_path(node: Node, src: &[u8]) -> Option<String> {
    match node.kind() {
        "type_identifier" | "identifier" => node.utf8_text(src).ok().map(str::to_string),
        "scoped_type_identifier" => node.utf8_text(src).ok().map(collapse_ws),
        "generic_type" => rust_type_path(node.child_by_field_name("type")?, src),
        _ => None,
    }
}

/// The dotted type path a C# base-list entry names, generic arguments dropped.
/// `None` for a `predefined_type` (`enum E : byte`) and for anything else that is
/// not a plain name — an alias-qualified or pointer type resolves to nothing
/// rather than to a guess.
fn csharp_type_path(node: Node, src: &[u8]) -> Option<String> {
    match node.kind() {
        "identifier" => node.utf8_text(src).ok().map(str::to_string),
        "generic_name" => csharp_type_path(node.named_child(0)?, src),
        "qualified_name" => {
            let qualifier = csharp_type_path(node.child_by_field_name("qualifier")?, src)?;
            let name = csharp_type_path(node.child_by_field_name("name")?, src)?;
            Some(format!("{qualifier}.{name}"))
        }
        _ => None,
    }
}

/// The type name a TS heritage entry names, generic arguments dropped. `None` for
/// a mixin (`extends mix(B)`), which names a call and not a type, and for a
/// namespace-qualified `ns.IFoo`, whose module is a TS namespace that a file-path
/// module identity can never spell.
fn ts_type_path(node: Node, src: &[u8]) -> Option<String> {
    match node.kind() {
        "type_identifier" | "identifier" => node.utf8_text(src).ok().map(str::to_string),
        "generic_type" => ts_type_path(node.child_by_field_name("name")?, src),
        _ => None,
    }
}

/// The dotted base name a Python superclass entry names. `None` for a
/// `keyword_argument` (`metaclass=M`), a subscript (`Generic[T]`) or a call —
/// none of which is a base class.
fn python_type_path(node: Node, src: &[u8]) -> Option<String> {
    match node.kind() {
        "identifier" | "attribute" => node.utf8_text(src).ok().map(collapse_ws),
        _ => None,
    }
}

/// C# base-list entries. The syntax does not distinguish a base class from an
/// implemented interface, so a class/struct/record defers the relation kind to
/// whatever the target turns out to be ([`RelationKind::ByTargetKind`]); an
/// INTERFACE's base list is always interface inheritance, which extends rather
/// than implements.
fn csharp_base_relations(
    file: &str,
    src: &[u8],
    decl: Node,
    module: &str,
    own_path: &str,
    declaring_is_interface: bool,
    st: &mut EmitState,
) {
    let Some(bases) = first_child_of_kind(decl, "base_list") else {
        return;
    };
    let kind = if declaring_is_interface {
        RelationKind::Extends
    } else {
        RelationKind::ByTargetKind
    };
    let mut cursor = bases.walk();
    for base in bases.named_children(&mut cursor) {
        if let Some(path) = csharp_type_path(base, src) {
            push_relation(st, file, "csharp", module, own_path, base, &path, kind);
        }
    }
}

/// TS heritage: a class's `extends` (one base) plus its `implements` list, and an
/// interface's `extends` list (any number, all inheritance). A class wraps both
/// clauses in a `class_heritage`, so the walk recurses through it.
fn ts_heritage_relations(
    file: &str,
    src: &[u8],
    decl: Node,
    module: &str,
    own_path: &str,
    tag: &'static str,
    st: &mut EmitState,
) {
    let mut cursor = decl.walk();
    for clause in decl.named_children(&mut cursor) {
        // `extends_clause` states its single base under a `value` field (it is an
        // expression, not a type list); the other two list their types as children.
        let (kind, from_value_field) = match clause.kind() {
            "class_heritage" => {
                // TypeScript wraps its bases in `extends_clause`/`implements_clause`;
                // JavaScript's `class_heritage` holds the base expression directly.
                let mut probe = clause.walk();
                let typed = clause
                    .named_children(&mut probe)
                    .any(|c| matches!(c.kind(), "extends_clause" | "implements_clause"));
                if typed {
                    ts_heritage_relations(file, src, clause, module, own_path, tag, st);
                } else {
                    let mut inner = clause.walk();
                    for base in clause.named_children(&mut inner) {
                        if let Some(path) = ts_type_path(base, src) {
                            push_relation(
                                st,
                                file,
                                tag,
                                module,
                                own_path,
                                base,
                                &path,
                                RelationKind::Extends,
                            );
                        }
                    }
                }
                continue;
            }
            "extends_clause" => (RelationKind::Extends, true),
            "extends_type_clause" => (RelationKind::Extends, false),
            "implements_clause" => (RelationKind::Implements, false),
            _ => continue,
        };
        if from_value_field {
            if let Some(value) = clause.child_by_field_name("value") {
                if let Some(path) = ts_type_path(value, src) {
                    push_relation(st, file, tag, module, own_path, value, &path, kind);
                }
            }
            continue;
        }
        let mut inner = clause.walk();
        for base in clause.named_children(&mut inner) {
            if let Some(path) = ts_type_path(base, src) {
                push_relation(st, file, tag, module, own_path, base, &path, kind);
            }
        }
    }
}

/// What a used identifier implies about the relation it creates.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum UsageKind {
    /// A syntactic call — `f(…)`, `x.m(…)`.
    Call,
    /// An explicit construction — `new T(…)`, a Rust struct literal, a Go
    /// composite literal.
    Instantiate,
    /// A type named in a type position — a parameter, a field, a return type, a
    /// local declaration, an alias.
    Reference,
}

/// One identifier a file uses, before resolution. Collected in tree order, so the
/// first site of a repeated relation is the one that anchors its edge.
pub(crate) struct UsageSite {
    /// The SIMPLE name to resolve — a qualified spelling is reduced to its last
    /// segment, because the symbol table is keyed by simple name.
    pub(crate) name: String,
    pub(crate) kind: UsageKind,
    /// 1-based line of the usage itself, which is what the edge's provenance points
    /// at and what decides which declaration encloses it. With a `column`, the
    /// line of the used NAME — see [`collect_csharp_usages`].
    pub(crate) line: u32,
    /// 1-based column of the used name, in characters (contract 0.2). `None`
    /// for a language whose walker reads no context.
    pub(crate) column: Option<u32>,
    /// What the syntax says about the site (contract 0.2). C# only, so far.
    pub(crate) context: Option<SiteContext>,
}

/// What one file's syntax says about a usage site — plan A, KTD3/KTD4. The
/// plugin sees one file; what only the whole repository knows (an inherited
/// field, the other half of a `partial` class) the host adds.
pub(crate) struct SiteContext {
    /// Index of the innermost [`LexicalScope`] around the site.
    pub(crate) scope: u32,
    pub(crate) receiver: Receiver,
    pub(crate) declared_type: Option<DeclaredType>,
    /// Arguments written at a call or a construction; `None` for a type
    /// reference.
    pub(crate) argument_count: Option<u32>,
    /// What precedes the name of a qualified construction or type reference.
    pub(crate) qualifier: Option<String>,
}

/// What a call is made on — the wire's `receiver`, documented there.
pub(crate) enum Receiver {
    None,
    Name(String),
    Value(String),
    Keyword(String),
    Expression,
}

/// A type as a declaration in the file states it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DeclaredType {
    /// As written, generic arguments dropped.
    pub(crate) name: String,
    /// A built-in type spelled as its keyword (`string`, `int`).
    pub(crate) keyword: bool,
}

/// The file, or one namespace in it, with the using directives written
/// directly inside.
pub(crate) struct LexicalScope {
    pub(crate) namespace: String,
    pub(crate) lines: (u32, u32),
    pub(crate) parent: Option<u32>,
    pub(crate) usings: Vec<UsingDirective>,
}

pub(crate) struct UsingDirective {
    pub(crate) target: String,
    pub(crate) alias: Option<String>,
    pub(crate) is_static: bool,
    pub(crate) is_global: bool,
}

/// Walk the whole tree, recording every usage site in source order.
pub(crate) fn collect_usages(lang: Language, node: Node, src: &[u8], out: &mut Vec<UsageSite>) {
    if let Some((name, kind)) = usage_at(lang, node, src) {
        out.push(UsageSite {
            name,
            kind,
            line: (node.start_position().row + 1) as u32,
            column: None,
            context: None,
        });
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_usages(lang, child, src, out);
    }
}

/// Whether a bare `Name(...)` can be a construction in this language.
pub(crate) fn construction_reads_as_a_call(lang: Language) -> bool {
    matches!(lang, Language::Python | Language::Rust)
}

/// The simple name of a possibly-qualified spelling — `crate::graph::NodeKey` and
/// `Model.Order` both reduce to their last segment, because the symbol table is
/// keyed by simple name and a heuristic cannot verify a qualifier anyway.
fn simple_name(path: &str) -> String {
    let tail = path.rsplit("::").next().unwrap_or(path);
    tail.rsplit('.').next().unwrap_or(tail).to_string()
}

/// True when `node` is the `name` field of its own parent — a declaration naming
/// ITSELF, which is never a reference to something else.
fn is_declaration_name(node: Node) -> bool {
    node.parent()
        .and_then(|p| p.child_by_field_name("name"))
        .is_some_and(|n| n.id() == node.id())
}

/// True when `node` is the `type` field of its parent — the grammar-marked type
/// position C#, Go and Python annotations use, where TS and Rust instead have a
/// dedicated `type_identifier` node kind.
fn is_type_field(node: Node) -> bool {
    node.parent()
        .and_then(|p| p.child_by_field_name("type"))
        .is_some_and(|n| n.id() == node.id())
}

/// True when `node`'s parent is a construction, whose type already produced an
/// `Instantiates` — so the same spelling does not also become a `References`.
fn under_construction(node: Node) -> bool {
    node.parent().is_some_and(|p| {
        matches!(
            p.kind(),
            "object_creation_expression" | "composite_literal" | "new_expression"
        )
    })
}

/// What one node means as a usage, per language. `None` for every node that is not
/// a usage at all — which is the overwhelming majority.
fn usage_at(lang: Language, node: Node, src: &[u8]) -> Option<(String, UsageKind)> {
    match lang {
        Language::Rust => rust_usage(node, src),
        Language::TypeScript | Language::Tsx | Language::JavaScript => ts_usage(node, src),
        Language::Python => python_usage(node, src),
        Language::CSharp => csharp_usage(node, src),
        Language::Go => go_usage(node, src),
        Language::Java => java_usage(node, src),
    }
}

fn java_usage(node: Node, src: &[u8]) -> Option<(String, UsageKind)> {
    match node.kind() {
        "method_invocation" => Some((
            node.child_by_field_name("name")?
                .utf8_text(src)
                .ok()?
                .to_string(),
            UsageKind::Call,
        )),
        "object_creation_expression" => {
            let name = java_type_path(node.child_by_field_name("type")?, src)?;
            Some((simple_name(&name), UsageKind::Instantiate))
        }
        "type_identifier" => {
            if is_declaration_name(node) || under_construction(node) {
                return None;
            }
            Some((node.utf8_text(src).ok()?.to_string(), UsageKind::Reference))
        }
        _ => None,
    }
}

fn rust_usage(node: Node, src: &[u8]) -> Option<(String, UsageKind)> {
    match node.kind() {
        "call_expression" => {
            let callee = node.child_by_field_name("function")?;
            Some((rust_callee_name(callee, src)?, UsageKind::Call))
        }
        // `Type { … }` — the only construction syntax Rust has (a tuple struct is
        // spelled as a call and reaches the `Call` fallback instead).
        "struct_expression" => {
            let name = rust_type_path(node.child_by_field_name("name")?, src)?;
            Some((simple_name(&name), UsageKind::Instantiate))
        }
        "type_identifier" | "scoped_type_identifier" => {
            if is_declaration_name(node) || under_construction(node) {
                return None;
            }
            let name = rust_type_path(node, src)?;
            Some((simple_name(&name), UsageKind::Reference))
        }
        _ => None,
    }
}

/// The name a Rust callee spelling ends in: `f`, `x.m`, `Type::assoc`, a
/// turbofish. `None` for a callee that names nothing addressable (a call through a
/// closure variable expression, an index).
fn rust_callee_name(callee: Node, src: &[u8]) -> Option<String> {
    match callee.kind() {
        "identifier" | "field_identifier" => callee.utf8_text(src).ok().map(str::to_string),
        "field_expression" => rust_callee_name(callee.child_by_field_name("field")?, src),
        "scoped_identifier" => rust_callee_name(callee.child_by_field_name("name")?, src),
        "generic_function" => rust_callee_name(callee.child_by_field_name("function")?, src),
        _ => None,
    }
}

fn ts_usage(node: Node, src: &[u8]) -> Option<(String, UsageKind)> {
    match node.kind() {
        "call_expression" => {
            let callee = node.child_by_field_name("function")?;
            Some((ts_callee_name(callee, src)?, UsageKind::Call))
        }
        "new_expression" => {
            let name = ts_type_path(node.child_by_field_name("constructor")?, src)?;
            Some((simple_name(&name), UsageKind::Instantiate))
        }
        "type_identifier" => {
            if is_declaration_name(node) {
                return None;
            }
            let name = ts_type_path(node, src)?;
            Some((simple_name(&name), UsageKind::Reference))
        }
        _ => None,
    }
}

fn ts_callee_name(callee: Node, src: &[u8]) -> Option<String> {
    match callee.kind() {
        "identifier" | "property_identifier" => callee.utf8_text(src).ok().map(str::to_string),
        "member_expression" => ts_callee_name(callee.child_by_field_name("property")?, src),
        _ => None,
    }
}

fn python_usage(node: Node, src: &[u8]) -> Option<(String, UsageKind)> {
    match node.kind() {
        "call" => {
            let callee = node.child_by_field_name("function")?;
            Some((python_callee_name(callee, src)?, UsageKind::Call))
        }
        // Python wraps every annotation — parameter, return, variable — in a `type`
        // node, so one arm covers all three positions.
        "type" => {
            let inner = node.named_child(0)?;
            let name = python_type_path(inner, src)?;
            Some((simple_name(&name), UsageKind::Reference))
        }
        _ => None,
    }
}

fn python_callee_name(callee: Node, src: &[u8]) -> Option<String> {
    match callee.kind() {
        "identifier" => callee.utf8_text(src).ok().map(str::to_string),
        "attribute" => python_callee_name(callee.child_by_field_name("attribute")?, src),
        _ => None,
    }
}

fn csharp_usage(node: Node, src: &[u8]) -> Option<(String, UsageKind)> {
    match node.kind() {
        "invocation_expression" => {
            let callee = node.child_by_field_name("function")?;
            Some((csharp_callee_name(callee, src)?, UsageKind::Call))
        }
        "object_creation_expression" => {
            let name = csharp_type_path(node.child_by_field_name("type")?, src)?;
            Some((simple_name(&name), UsageKind::Instantiate))
        }
        // C# marks a type position with a `type` field rather than a distinct node
        // kind, so the field is what identifies one. A `predefined_type` (`int`,
        // `string`) resolves to no path and is dropped by `csharp_type_path`.
        _ if is_type_field(node) && !under_construction(node) => {
            let name = csharp_type_path(node, src)?;
            Some((simple_name(&name), UsageKind::Reference))
        }
        _ => None,
    }
}

fn csharp_callee_name(callee: Node, src: &[u8]) -> Option<String> {
    csharp_callee_name_node(callee)?
        .utf8_text(src)
        .ok()
        .map(str::to_string)
}

/// The identifier a C# callee spelling ends in: `f`, `x.m`, `f<T>`, and
/// `x?.m` — which until plan A, U6 emitted no site at all.
fn csharp_callee_name_node(callee: Node) -> Option<Node> {
    match callee.kind() {
        "identifier" => Some(callee),
        "member_access_expression" => csharp_callee_name_node(callee.child_by_field_name("name")?),
        "generic_name" => csharp_callee_name_node(callee.named_child(0)?),
        "conditional_access_expression" => {
            let binding = first_child_of_kind(callee, "member_binding_expression")?;
            csharp_callee_name_node(binding.child_by_field_name("name")?)
        }
        _ => None,
    }
}

/// The identifier a C# type spelling ends in: `T`, `T<U>`, `A.B.T`.
fn csharp_type_name_node(node: Node) -> Option<Node> {
    match node.kind() {
        "identifier" => Some(node),
        "generic_name" => csharp_type_name_node(node.named_child(0)?),
        "qualified_name" => csharp_type_name_node(node.child_by_field_name("name")?),
        _ => None,
    }
}

// ---- C# usages with context (plan A, U6) ----

/// Every C# usage site with its context, and the file's lexical scopes.
///
/// One walk over the one parse that finds the sites (the fuel a call gets is
/// budgeted per file, not per pass), and the same sites in the same order as
/// [`collect_usages`] would find them — plus `x?.m()`, which it never did.
/// Beside the sites it keeps two stacks:
///
/// - the namespaces, each a [`LexicalScope`] with the using directives
///   written in it, read from the grammar — `global`, `static` and aliases
///   included — rather than the host's regular expression, which reads only
///   external namespaces;
/// - the names a body can see (KTD4): a type's fields, properties and
///   primary-constructor parameters for the whole type, then parameters,
///   locals, loop, `using`, `catch`, pattern and `out` variables in the frame
///   that declares them, innermost first, so a local shadows a field.
///
/// A site's line and column are those of the used NAME — the called
/// identifier, not the start of the invocation. Both invocations of
/// `sb.Append(a).Append(b)` start at `sb`, so only the name's position tells
/// them apart; in a chain spread over lines, a call sits on the line it is
/// written on.
pub(crate) fn collect_csharp_usages(root: Node, src: &[u8]) -> (Vec<LexicalScope>, Vec<UsageSite>) {
    let file_end = node_end_line(root).max(1);
    let mut pass = CsharpPass {
        src,
        file_end,
        scopes: vec![LexicalScope {
            namespace: String::new(),
            lines: (1, file_end),
            parent: None,
            usings: Vec::new(),
        }],
        namespaces: vec![0],
        // The file's own frame: top-level statements declare into it.
        frames: vec![Frame::default()],
        sites: Vec::new(),
    };
    pass.walk(root);
    (pass.scopes, pass.sites)
}

/// The names one lexical frame declares, each with the type its declaration
/// states — `None` when it states none (`var x = Load()`), which still
/// shadows an outer name of the same spelling.
#[derive(Default)]
struct Frame {
    names: BTreeMap<String, Option<DeclaredType>>,
    /// A type body: what `this.x` looks in.
    is_type: bool,
}

struct CsharpPass<'s> {
    src: &'s [u8],
    /// The file's last line, where a file-scoped namespace ends.
    file_end: u32,
    scopes: Vec<LexicalScope>,
    /// Indices into `scopes`, the innermost last.
    namespaces: Vec<u32>,
    frames: Vec<Frame>,
    sites: Vec<UsageSite>,
}

/// Node kinds that open a frame of their own: a callable and its parameters,
/// and every statement or expression C# scopes names to.
fn opens_csharp_frame(kind: &str) -> bool {
    matches!(
        kind,
        "method_declaration"
            | "constructor_declaration"
            | "destructor_declaration"
            | "operator_declaration"
            | "conversion_operator_declaration"
            | "indexer_declaration"
            | "accessor_declaration"
            | "local_function_statement"
            | "lambda_expression"
            | "anonymous_method_expression"
            | "block"
            | "for_statement"
            | "foreach_statement"
            | "using_statement"
            | "fixed_statement"
            | "catch_clause"
            | "switch_section"
            | "switch_expression_arm"
    )
}

impl CsharpPass<'_> {
    fn text(&self, node: Node) -> String {
        node.utf8_text(self.src).unwrap_or("").to_string()
    }

    /// Source text with every whitespace character removed — how a name, a
    /// path or a using target is compared, whatever the formatting.
    fn compact(&self, node: Node) -> String {
        node.utf8_text(self.src)
            .unwrap_or("")
            .split_whitespace()
            .collect()
    }

    fn walk(&mut self, node: Node) {
        if let Some((name, kind)) = csharp_usage(node, self.src) {
            let site = self.site(node, name, kind);
            self.sites.push(site);
        }
        let frames = self.frames.len();
        let namespaces = self.namespaces.len();
        match node.kind() {
            "using_directive" => self.using(node),
            "namespace_declaration" => self.enter_namespace(node, node_end_line(node)),
            // `namespace N;` holds everything after it: the scope stays
            // open past this node, until its parent ends.
            "file_scoped_namespace_declaration" => self.enter_namespace(node, self.file_end),
            "class_declaration"
            | "struct_declaration"
            | "record_declaration"
            | "record_struct_declaration"
            | "interface_declaration"
            | "enum_declaration" => self.enter_type(node),
            kind if opens_csharp_frame(kind) => {
                self.frames.push(Frame::default());
                if let Some(params) = node.child_by_field_name("parameters") {
                    self.declare_parameters(params);
                }
                if kind == "foreach_statement" {
                    if let Some(left) = node.child_by_field_name("left") {
                        if left.kind() == "identifier" {
                            let ty = node
                                .child_by_field_name("type")
                                .and_then(|t| self.declared_type(t));
                            self.declare(left, ty);
                        }
                    }
                }
            }
            "variable_declaration" => self.declare_variables(node),
            "catch_declaration"
            | "declaration_pattern"
            | "declaration_expression"
            | "recursive_pattern" => {
                if let Some(name) = node.child_by_field_name("name") {
                    let ty = node
                        .child_by_field_name("type")
                        .and_then(|t| self.declared_type(t));
                    self.declare(name, ty);
                }
            }
            "var_pattern" => {
                if let Some(name) = node.child_by_field_name("name") {
                    self.declare(name, None);
                }
            }
            _ => {}
        }
        let mut cursor = node.walk();
        let children: Vec<Node> = node.named_children(&mut cursor).collect();
        for child in children {
            self.walk(child);
        }
        self.frames.truncate(frames);
        if node.kind() != "file_scoped_namespace_declaration" {
            self.namespaces.truncate(namespaces);
        }
    }

    fn enter_namespace(&mut self, node: Node, end: u32) {
        let Some(name) = node.child_by_field_name("name").map(|n| self.compact(n)) else {
            return;
        };
        let parent = *self
            .namespaces
            .last()
            .expect("the file scope is never popped");
        let outer = &self.scopes[parent as usize].namespace;
        let namespace = if outer.is_empty() {
            name
        } else {
            format!("{outer}.{name}")
        };
        self.scopes.push(LexicalScope {
            namespace,
            lines: ((node.start_position().row + 1) as u32, end),
            parent: Some(parent),
            usings: Vec::new(),
        });
        self.namespaces.push((self.scopes.len() - 1) as u32);
    }

    /// `using X;`, `global using X;`, `using static X;`, `using A = X;` — the
    /// keywords are anonymous tokens, the alias is the `name` field and the
    /// target is the one named child besides it.
    fn using(&mut self, node: Node) {
        let alias = node.child_by_field_name("name");
        let mut is_global = false;
        let mut is_static = false;
        let mut target = None;
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "global" => is_global = true,
                "static" => is_static = true,
                _ if child.is_named() && Some(child.id()) != alias.map(|a| a.id()) => {
                    target.get_or_insert(child);
                }
                _ => {}
            }
        }
        let Some(target) = target else {
            return;
        };
        let directive = UsingDirective {
            target: self.compact(target),
            alias: alias.map(|a| self.text(a)),
            is_static,
            is_global,
        };
        let scope = *self
            .namespaces
            .last()
            .expect("the file scope is never popped");
        self.scopes[scope as usize].usings.push(directive);
    }

    /// A type's frame: everything its body can name without a qualifier,
    /// declared before any member is walked, so a field below a method is in
    /// scope of the method.
    fn enter_type(&mut self, node: Node) {
        self.frames.push(Frame {
            is_type: true,
            ..Frame::default()
        });
        let mut cursor = node.walk();
        let children: Vec<Node> = node.named_children(&mut cursor).collect();
        for child in children {
            if child.kind() == "parameter_list" {
                // A primary constructor's parameters.
                self.declare_parameters(child);
            }
        }
        let Some(body) = node.child_by_field_name("body") else {
            return;
        };
        let mut cursor = body.walk();
        let members: Vec<Node> = body.named_children(&mut cursor).collect();
        for member in members {
            match member.kind() {
                "field_declaration" | "event_field_declaration" => {
                    if let Some(decl) = first_child_of_kind(member, "variable_declaration") {
                        self.declare_variables(decl);
                    }
                }
                "property_declaration" => {
                    if let Some(name) = member.child_by_field_name("name") {
                        let ty = member
                            .child_by_field_name("type")
                            .and_then(|t| self.declared_type(t));
                        self.declare(name, ty);
                    }
                }
                _ => {}
            }
        }
    }

    /// Parameters of a callable, a lambda or a primary constructor. An
    /// untyped lambda parameter (`q => …`, `(a, b) => …`) declares a name
    /// with no type: it shadows all the same.
    fn declare_parameters(&mut self, params: Node) {
        if params.kind() == "implicit_parameter" || params.kind() == "identifier" {
            self.declare(params, None);
            return;
        }
        let mut cursor = params.walk();
        let list: Vec<Node> = params.named_children(&mut cursor).collect();
        for param in list {
            if param.kind() != "parameter" {
                continue;
            }
            if let Some(name) = param.child_by_field_name("name") {
                let ty = param
                    .child_by_field_name("type")
                    .and_then(|t| self.declared_type(t));
                self.declare(name, ty);
            }
        }
    }

    /// `T a, b = …;` declares each with `T`; `var x = new T(…)` declares `T`
    /// (a syntactic fact, Key Decision); `var` with any other initializer —
    /// a call, a target-typed `new()` — declares no type.
    fn declare_variables(&mut self, decl: Node) {
        let Some(ty) = decl.child_by_field_name("type") else {
            return;
        };
        let implicit = ty.kind() == "implicit_type";
        let stated = if implicit {
            None
        } else {
            self.declared_type(ty)
        };
        let mut cursor = decl.walk();
        let declarators: Vec<Node> = decl.named_children(&mut cursor).collect();
        for declarator in declarators {
            if declarator.kind() != "variable_declarator" {
                continue;
            }
            let Some(name) = declarator.child_by_field_name("name") else {
                continue;
            };
            let ty = if implicit {
                first_child_of_kind(declarator, "object_creation_expression")
                    .and_then(|new| new.child_by_field_name("type"))
                    .and_then(|t| self.declared_type(t))
            } else {
                stated.clone()
            };
            self.declare(name, ty);
        }
    }

    fn declare(&mut self, name: Node, ty: Option<DeclaredType>) {
        let name = self.text(name);
        self.frames
            .last_mut()
            .expect("the file frame is never popped")
            .names
            .insert(name, ty);
    }

    /// The type a type node states: a keyword type flagged as one, a nullable
    /// type as what it wraps, a named type as its path with generic arguments
    /// dropped. `None` for anything else — an array, a tuple, a pointer —
    /// whose members no repository declaration can be.
    fn declared_type(&self, ty: Node) -> Option<DeclaredType> {
        match ty.kind() {
            "predefined_type" => Some(DeclaredType {
                name: self.text(ty),
                keyword: true,
            }),
            "nullable_type" => self.declared_type(ty.child_by_field_name("type")?),
            _ => csharp_type_path(ty, self.src).map(|name| DeclaredType {
                name,
                keyword: false,
            }),
        }
    }

    /// The innermost declaration of `name`: `Some(None)` when it states no
    /// type, `None` when the file does not declare the name at all.
    fn lookup(&self, name: &str) -> Option<&Option<DeclaredType>> {
        self.frames.iter().rev().find_map(|f| f.names.get(name))
    }

    /// `name` as a member of the innermost enclosing type — what `this.name`
    /// means.
    fn lookup_member(&self, name: &str) -> Option<&Option<DeclaredType>> {
        self.frames
            .iter()
            .rev()
            .find(|f| f.is_type)
            .and_then(|f| f.names.get(name))
    }

    /// What a call's receiver expression says: a keyword, a value the file
    /// declares (with its declared type when the declaration states one), a
    /// name or path of names it does not declare, or another expression — of
    /// which only `new T(…)` states a type.
    ///
    /// A declared value is a `value` even when its type is unknown: flattening
    /// "declared without a type" into "not declared" would let the host read
    /// `Worker` in `var Worker = Load(); Worker.Run()` as the type `Worker`.
    fn receiver(&self, expr: Node) -> (Receiver, Option<DeclaredType>) {
        match expr.kind() {
            "predefined_type" => (Receiver::Keyword(self.text(expr)), None),
            "identifier" => {
                let name = self.text(expr);
                match self.lookup(&name) {
                    Some(ty) => {
                        let ty = ty.clone();
                        (Receiver::Value(name), ty)
                    }
                    None => (Receiver::Name(name), None),
                }
            }
            "member_access_expression"
                if expr
                    .child_by_field_name("expression")
                    .is_some_and(|e| e.kind() == "this")
                    && expr
                        .child_by_field_name("name")
                        .is_some_and(|n| n.kind() == "identifier") =>
            {
                let member = self.text(expr.child_by_field_name("name").expect("matched above"));
                let ty = self.lookup_member(&member).cloned().flatten();
                (Receiver::Name(format!("this.{member}")), ty)
            }
            "object_creation_expression" => (
                Receiver::Expression,
                expr.child_by_field_name("type")
                    .and_then(|t| self.declared_type(t)),
            ),
            // `store.Items`: a path through a declared value is a value, its
            // type unknown — what `Items` holds the file does not say.
            _ if is_csharp_name_path(expr) => {
                let path = self.compact(expr);
                if leftmost_identifier(expr)
                    .is_some_and(|first| self.lookup(&self.text(first)).is_some())
                {
                    (Receiver::Value(path), None)
                } else {
                    (Receiver::Name(path), None)
                }
            }
            _ => (Receiver::Expression, None),
        }
    }

    fn site(&self, node: Node, name: String, kind: UsageKind) -> UsageSite {
        let scope = *self
            .namespaces
            .last()
            .expect("the file scope is never popped");
        let (name_node, receiver, declared_type, argument_count, qualifier) = match kind {
            UsageKind::Call => {
                let callee = node.child_by_field_name("function");
                let (receiver, declared) = match callee.map(|c| (c.kind(), c)) {
                    Some(("member_access_expression", c)) => c
                        .child_by_field_name("expression")
                        .map_or((Receiver::Expression, None), |e| self.receiver(e)),
                    Some(("conditional_access_expression", c)) => c
                        .child_by_field_name("condition")
                        .map_or((Receiver::Expression, None), |e| self.receiver(e)),
                    _ => (Receiver::None, None),
                };
                (
                    callee.and_then(csharp_callee_name_node),
                    receiver,
                    declared,
                    Some(argument_count(node.child_by_field_name("arguments"))),
                    None,
                )
            }
            UsageKind::Instantiate => {
                let ty = node.child_by_field_name("type");
                (
                    ty.and_then(csharp_type_name_node),
                    Receiver::None,
                    None,
                    Some(argument_count(node.child_by_field_name("arguments"))),
                    ty.and_then(|t| self.qualifier(t)),
                )
            }
            UsageKind::Reference => (
                csharp_type_name_node(node),
                Receiver::None,
                None,
                None,
                self.qualifier(node),
            ),
        };
        let (line, column) = position(name_node.unwrap_or(node), self.src);
        UsageSite {
            name,
            kind,
            line,
            column: Some(column),
            context: Some(SiteContext {
                scope,
                receiver,
                declared_type,
                argument_count,
                qualifier,
            }),
        }
    }

    /// `System.Text` of `System.Text.StringBuilder`; `None` for a simple name.
    fn qualifier(&self, ty: Node) -> Option<String> {
        let path = csharp_type_path(ty, self.src)?;
        path.rsplit_once('.').map(|(q, _)| q.to_string())
    }
}

/// Arguments in a C# argument list, named ones included; an absent list (a
/// construction with only an initializer) has none.
fn argument_count(arguments: Option<Node>) -> u32 {
    let Some(arguments) = arguments else {
        return 0;
    };
    let mut cursor = arguments.walk();
    let count = arguments
        .named_children(&mut cursor)
        .filter(|a| a.kind() == "argument")
        .count();
    count as u32
}

/// A name, or a dotted path made only of names — `x`, `this`, `Foo<int>`,
/// `System.IO.File`, `global::N` — whose text says what it names. Anything
/// else (a call, an index, a literal) is an expression whose text would not.
/// The first name of a member-access path (`store` in `store.Items.First`),
/// when it is a plain identifier — `this`, `base`, a generic or qualified name
/// and an alias-qualified one are not values a file declares.
fn leftmost_identifier(node: Node) -> Option<Node> {
    let mut at = node;
    while at.kind() == "member_access_expression" {
        at = at.child_by_field_name("expression")?;
    }
    (at.kind() == "identifier").then_some(at)
}

fn is_csharp_name_path(node: Node) -> bool {
    match node.kind() {
        "identifier"
        | "this"
        | "base"
        | "generic_name"
        | "qualified_name"
        | "alias_qualified_name" => true,
        "member_access_expression" => {
            node.child_by_field_name("expression")
                .is_some_and(is_csharp_name_path)
                && node
                    .child_by_field_name("name")
                    .is_some_and(|n| matches!(n.kind(), "identifier" | "generic_name"))
        }
        _ => false,
    }
}

fn go_usage(node: Node, src: &[u8]) -> Option<(String, UsageKind)> {
    match node.kind() {
        "call_expression" => {
            let callee = node.child_by_field_name("function")?;
            Some((go_callee_name(callee, src)?, UsageKind::Call))
        }
        "composite_literal" => {
            let name = go_type_path(node.child_by_field_name("type")?, src)?;
            Some((simple_name(&name), UsageKind::Instantiate))
        }
        _ if is_type_field(node) && !under_construction(node) => {
            let name = go_type_path(node, src)?;
            Some((simple_name(&name), UsageKind::Reference))
        }
        _ => None,
    }
}

fn go_callee_name(callee: Node, src: &[u8]) -> Option<String> {
    match callee.kind() {
        "identifier" | "field_identifier" => callee.utf8_text(src).ok().map(str::to_string),
        "selector_expression" => go_callee_name(callee.child_by_field_name("field")?, src),
        _ => None,
    }
}

/// The plain type name a Go type node names, unwrapping pointers, slices, arrays
/// and channels (`*T`, `[]T`, `[N]T`, `chan T` all name `T`). `None` for a
/// `qualified_type` — its qualifier is another PACKAGE, which no module identity in
/// this repo can spell — and for a struct/interface literal type, which names
/// nothing.
fn go_type_path(node: Node, src: &[u8]) -> Option<String> {
    match node.kind() {
        "type_identifier" => node.utf8_text(src).ok().map(str::to_string),
        "pointer_type" | "slice_type" | "array_type" | "channel_type" | "parenthesized_type" => {
            let inner = node
                .child_by_field_name("element")
                .or_else(|| node.named_child(0))?;
            go_type_path(inner, src)
        }
        _ => None,
    }
}

// ---- Imports ----
//
// Scope discipline (task/E-plan): an `Imports` edge is emitted ONLY when the
// import resolves to a node that already exists in the graph — an internal
// import to another tracked file/module. External/stdlib/package imports (Rust
// `std`/crates, TS npm packages, Python absolute packages) are U8's dependency
// territory: no phantom target node is ever created, the edge is simply
// skipped. Resolution is best-effort but deterministic (KTD7). Go and C# emit
// no import edges — Go import paths carry the module prefix (unresolvable
// without go.mod) and C# `using` names namespaces, not file paths.

/// Emit one `file --Imports--> target` edge, deduped per target so repeated
/// imports of the same module produce a single edge (mirrors filetree's
/// `Contains` dedup). `target` is always an existing node (the resolvers only
/// return keys that resolve), so `add_edge` cannot drop it.
fn add_import_edge(
    file: &str,
    target: &NodeKey,
    node: Node,
    out: &mut FileOutput,
    imported: &mut HashSet<String>,
) {
    let file_key = NodeKey::new(file.to_string());
    // A file resolving an import to itself (e.g. `index.ts` importing
    // `./index`) is a meaningless self-loop — drop it.
    if *target == file_key {
        return;
    }
    if !imported.insert(target.as_str().to_string()) {
        return;
    }
    let line = Some((node.start_position().row + 1) as u32);
    let prov = Provenance::new(file, line, ids::treesitter(), Tier::Measured);
    out.edge(file_key, target.clone(), Edge::new(EdgeKind::Imports, prov));
}

/// The `/`-normalized parent directory of a path key; `""` at repo root.
fn parent_dir(path: &str) -> String {
    match path.rsplit_once('/') {
        Some((parent, _)) => parent.to_string(),
        None => String::new(),
    }
}

/// Resolve a relative specifier (`./x`, `../a/b`) against `base` (a directory
/// key, `""` = root). `.`/empty segments are dropped; `..` pops a component and
/// returns `None` if it would escape the root. The result is a `/`-joined key.
fn join_relative(base: &str, spec: &str) -> Option<String> {
    let mut comps: Vec<&str> = if base.is_empty() {
        Vec::new()
    } else {
        base.split('/').collect()
    };
    for seg in spec.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                comps.pop()?;
            }
            s => comps.push(s),
        }
    }
    Some(comps.join("/"))
}

/// Walk top-level TS/TSX `import`/re-`export … from` statements and emit an
/// Imports edge for each relative specifier that resolves.
fn walk_ts_imports(
    file: &str,
    src: &[u8],
    node: Node,
    out: &mut FileOutput,
    paths: &PathKeys,
    imported: &mut HashSet<String>,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if matches!(child.kind(), "import_statement" | "export_statement") {
            if let Some(source) = child.child_by_field_name("source") {
                if let Some(spec) = string_fragment_text(source, src) {
                    if let Some(target) = resolve_ts(file, &spec, paths) {
                        add_import_edge(file, &target, child, out, imported);
                    }
                }
            }
        }
        // CommonJS: `const x = require('./y')` at module scope.
        if matches!(child.kind(), "lexical_declaration" | "variable_declaration") {
            let mut dc = child.walk();
            for d in child.named_children(&mut dc) {
                let Some(call) = d.child_by_field_name("value") else {
                    continue;
                };
                if call.kind() != "call_expression" {
                    continue;
                }
                let is_require = call
                    .child_by_field_name("function")
                    .and_then(|f| f.utf8_text(src).ok())
                    == Some("require");
                if !is_require {
                    continue;
                }
                let spec = call.child_by_field_name("arguments").and_then(|args| {
                    let mut ac = args.walk();
                    let first = args.named_children(&mut ac).next()?;
                    (first.kind() == "string").then(|| string_fragment_text(first, src))?
                });
                if let Some(spec) = spec {
                    if let Some(target) = resolve_ts(file, &spec, paths) {
                        add_import_edge(file, &target, child, out, imported);
                    }
                }
            }
        }
    }
}

/// The inner text of a `string` node (its `string_fragment` child), quote-style
/// independent. An empty string literal yields `""`.
fn string_fragment_text(string_node: Node, src: &[u8]) -> Option<String> {
    let mut cursor = string_node.walk();
    for c in string_node.named_children(&mut cursor) {
        if c.kind() == "string_fragment" {
            return c.utf8_text(src).ok().map(str::to_string);
        }
    }
    Some(String::new())
}

/// Resolve a TS/TSX import specifier to an existing file/module node. Only
/// relative specifiers (`./`, `../`) can be internal; bare specifiers are npm
/// packages (external, skipped). Candidate order is deterministic:
/// literal → `.ts` → `.tsx` → `index.ts` → `index.tsx` → directory module.
fn resolve_ts(file: &str, spec: &str, paths: &PathKeys) -> Option<NodeKey> {
    if !spec.starts_with('.') {
        return None;
    }
    let target = join_relative(&parent_dir(file), spec)?;
    if target.is_empty() {
        return None;
    }
    // Drop a redundant JS/TS extension the author may have written explicitly.
    let stem = [".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs"]
        .iter()
        .find_map(|e| target.strip_suffix(e))
        .unwrap_or(&target)
        .to_string();
    // TypeScript sources first, so a `.js` specifier written for ESM output
    // still lands on the `.ts` file it compiles from; JavaScript after.
    let candidates = [
        target.clone(),
        format!("{stem}.ts"),
        format!("{stem}.tsx"),
        format!("{stem}.js"),
        format!("{stem}.jsx"),
        format!("{stem}.mjs"),
        format!("{stem}.cjs"),
        format!("{stem}/index.ts"),
        format!("{stem}/index.tsx"),
        format!("{stem}/index.js"),
        format!("{stem}/index.jsx"),
        stem,
    ];
    paths.first_existing(&candidates)
}

/// Walk top-level Python `from … import …` statements and emit an Imports edge
/// for each relative import that resolves. Absolute imports (`import os`,
/// `from pkg import x`) are external and skipped.
fn walk_python_imports(
    file: &str,
    src: &[u8],
    node: Node,
    out: &mut FileOutput,
    paths: &PathKeys,
    imported: &mut HashSet<String>,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "import_from_statement" {
            if let Some(target) = resolve_python_from(file, child, src, paths) {
                add_import_edge(file, &target, child, out, imported);
            }
        }
    }
}

/// Resolve a Python `from .x import …` to an existing file/module node. Only
/// relative imports (leading dots) are internal; the dot count sets how far up
/// from the importing file's package to start.
fn resolve_python_from(
    file: &str,
    from_stmt: Node,
    src: &[u8],
    paths: &PathKeys,
) -> Option<NodeKey> {
    let module_name = from_stmt.child_by_field_name("module_name")?;
    if module_name.kind() != "relative_import" {
        return None; // absolute import → external
    }
    // Leading dots: 1 = current package, 2 = parent, … → (dots - 1) levels up.
    let mut dots = 0usize;
    let mut dotted: Option<Node> = None;
    let mut cursor = module_name.walk();
    for c in module_name.named_children(&mut cursor) {
        match c.kind() {
            "import_prefix" => dots = c.utf8_text(src).ok()?.matches('.').count(),
            "dotted_name" => dotted = Some(c),
            _ => {}
        }
    }
    if dots == 0 {
        return None;
    }
    let package_dir = parent_dir(file);
    let mut comps: Vec<&str> = if package_dir.is_empty() {
        Vec::new()
    } else {
        package_dir.split('/').collect()
    };
    for _ in 0..(dots - 1) {
        comps.pop()?;
    }
    let dotted_text = dotted.and_then(|d| d.utf8_text(src).ok().map(str::to_string));
    if let Some(ref t) = dotted_text {
        comps.extend(t.split('.'));
    }
    let target = comps.join("/");
    if target.is_empty() {
        return None;
    }
    // `from . import x` (no dotted name) targets the package itself.
    let candidates = if dotted_text.is_some() {
        vec![
            format!("{target}.py"),
            format!("{target}/__init__.py"),
            target,
        ]
    } else {
        vec![format!("{target}/__init__.py"), target]
    };
    paths.first_existing(&candidates)
}

/// Walk Rust `use` declarations (top-level and inside `mod` blocks) and emit an
/// Imports edge for each `crate::`/`self::`/`super::` path that resolves to a
/// file/module node. External crates (`std`, third-party) are skipped.
fn walk_rust_imports(
    file: &str,
    src: &[u8],
    node: Node,
    out: &mut FileOutput,
    paths: &PathKeys,
    imported: &mut HashSet<String>,
    // True once we have descended into an inline `mod m { ... }` body. `self::`
    // and `super::` are relative to the *module* they appear in, and an inline
    // module shifts that anchor away from the file's own module dir — which this
    // best-effort resolver does not track. So inside an inline module we fail
    // closed on `self`/`super` rather than emit a wrong-but-existing edge.
    in_inline_mod: bool,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "use_declaration" => {
                if let Some(target) = resolve_rust_use(file, child, src, paths, in_inline_mod) {
                    add_import_edge(file, &target, child, out, imported);
                }
            }
            "mod_item" => {
                if let Some(body) = child.child_by_field_name("body") {
                    walk_rust_imports(file, src, body, out, paths, imported, true);
                }
            }
            _ => {}
        }
    }
}

/// Resolve a Rust `use` path to a file/module node, best-effort. Reads the
/// leading path segments (before any `{`), roots them at the crate root dir
/// (`crate::`), the current dir (`self::`), or an ancestor (`super::`), then
/// matches the longest existing prefix as `<p>.rs` / `<p>/mod.rs` / `<p>`.
fn resolve_rust_use(
    file: &str,
    use_node: Node,
    src: &[u8],
    paths: &PathKeys,
    in_inline_mod: bool,
) -> Option<NodeKey> {
    let text = use_node.utf8_text(src).ok()?;
    let inner = text.trim().strip_prefix("use ")?.trim();
    // Take the path before a `{` group and drop a trailing `::`/`;`/`*`.
    let path_part = inner
        .split('{')
        .next()
        .unwrap_or(inner)
        .trim_end_matches(';')
        .trim()
        .trim_end_matches("::")
        .trim();
    let segs: Vec<&str> = path_part
        .split("::")
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "*")
        .collect();
    if segs.is_empty() {
        return None;
    }
    // Determine the base directory and the segments below it.
    let (base_dir, rest): (String, &[&str]) = match segs[0] {
        // `crate::` anchors at the source root of the file's OWN crate. If that
        // can't be determined confidently, skip the edge (fail closed) rather
        // than emit a wrong-but-plausible one to a sibling.
        "crate" => (crate_root_dir(file)?, &segs[1..]),
        // `self::` resolves against the file's own module directory (submodules
        // of `src/a.rs` live in `src/a/`), not its parent directory. Inside an
        // inline `mod` the anchor shifts and we don't track it → fail closed.
        "self" if in_inline_mod => return None,
        "self" => (module_dir(file), &segs[1..]),
        // `super::` from an inline module is likewise offset from the file dir.
        "super" if in_inline_mod => return None,
        "super" => {
            // Ascend one directory per leading `super` from the file's own dir.
            let ups = segs.iter().take_while(|s| **s == "super").count();
            let dir = parent_dir(file);
            let mut comps: Vec<&str> = if dir.is_empty() {
                Vec::new()
            } else {
                dir.split('/').collect()
            };
            for _ in 0..ups {
                comps.pop()?;
            }
            (comps.join("/"), &segs[ups..])
        }
        _ => return None, // external crate → skip
    };
    resolve_mod_path(&base_dir, rest, paths)
}

/// The crate source-root directory that `crate::` anchors at, for a
/// repo-relative file key. Cargo puts a crate's roots under its `src/`, so the
/// anchor is the **nearest ancestor `src` directory** of the file — which is
/// correct for both the single-crate layout (`src/extract/git.rs` → `src`) and
/// nested/workspace layouts (`crates/foo/src/a.rs` → `crates/foo/src`), where
/// anchoring at the first component (`crates`) would resolve `crate::b` to a
/// wrong sibling. If the file lives under no `src/` directory the crate root
/// can't be determined confidently, so returns `None` and the caller skips the
/// edge (best-effort must fail closed, never emit a wrong edge).
fn crate_root_dir(file: &str) -> Option<String> {
    let dir = parent_dir(file);
    if dir.is_empty() {
        return None;
    }
    let comps: Vec<&str> = dir.split('/').collect();
    let pos = comps.iter().rposition(|c| *c == "src")?;
    Some(comps[..=pos].join("/"))
}

/// The directory holding a file's own module's submodules — where `self::x`
/// resolves. A file `src/a.rs` defines module `a`, whose submodules live in
/// `src/a/`, so its module dir is `src/a`. A crate/module root file
/// (`lib.rs`/`main.rs`/`mod.rs`) already sits at its module's directory, so its
/// module dir is just its parent directory.
fn module_dir(file: &str) -> String {
    let dir = parent_dir(file);
    let stem = file
        .rsplit('/')
        .next()
        .and_then(|base| base.strip_suffix(".rs"))
        .unwrap_or("");
    if stem.is_empty() || matches!(stem, "mod" | "lib" | "main") {
        dir
    } else if dir.is_empty() {
        stem.to_string()
    } else {
        format!("{dir}/{stem}")
    }
}

/// Longest-prefix match of module path `segs` under `base_dir` to a file/module
/// node. Tries progressively shorter prefixes; within each, file before module.
fn resolve_mod_path(base_dir: &str, segs: &[&str], paths: &PathKeys) -> Option<NodeKey> {
    for k in (1..=segs.len()).rev() {
        let joined = if base_dir.is_empty() {
            segs[..k].join("/")
        } else {
            format!("{base_dir}/{}", segs[..k].join("/"))
        };
        let candidates = [format!("{joined}.rs"), format!("{joined}/mod.rs"), joined];
        if let Some(k) = paths.first_existing(&candidates) {
            return Some(k);
        }
    }
    None
}
