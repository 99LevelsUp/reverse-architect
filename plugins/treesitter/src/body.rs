//! The bodies of C# members: the `bodies` pass of contract 0.3, served by the
//! `csharp-body` component (`--features csharp-body`).
//!
//! One parse per file, the same [`crate::walkers::parse`] the first layer
//! uses, and every position in the same units ([`crate::walkers::position`]:
//! 1-based line, column in characters). The position of a member's declared
//! name is exactly the one `walkers.rs` puts into `place.name` of the
//! declaration — the `name` field of the declaration, or of the declarator
//! for a field — which is what the host pairs the two layers by.
//!
//! The tree is normalized by logic, not by spelling: an `if` / `else if` /
//! `else` chain and a `switch` are each one `choice` of `arm`s, every loop is a
//! `loop`, a block that is the body of an arm, a loop, a `try`, a `catch` or a
//! lambda is not a node of its own, and `=> e` is the same `return` node
//! `{ return e; }` is. Nothing that can be long — an `else if` chain, a chain
//! of binary operators, any expression — is walked by recursion: every walk
//! over an expression keeps its own stack, so a guest's call stack depends only
//! on how deeply statements and lambdas nest, and that is capped
//! ([`MAX_NESTING`]).
//!
//! # Heuristics
//!
//! What the syntax alone cannot decide is decided by these rules, each a
//! guess a reader of this file can check:
//!
//! - **Enum member** — a dotted path read as a value whose root is not
//!   `this`, `base` or a name the body declares, of at least two segments,
//!   every one starting with an upper-case letter (`Key.F4`,
//!   `ModifierKeys.Shift`, `Acme.Input.Key.F4`), which is not called, not
//!   written and not the LEFT operand of a binary operator whose right operand
//!   looks like one too. The last clause is what keeps a static property
//!   compared with a constant (`(Keyboard.Modifiers & ModifierKeys.Shift)`) a
//!   read: the constant is conventionally on the right. A static property read
//!   on its own (`Check(Keyboard.Modifiers)`) is still taken for an enum
//!   member; the syntax does not tell them apart. An enum member is reported
//!   in `enum-refs` and as `enum-member`, never as a `member-access`.
//! - **Subscription** — `a.E += h` / `-=` where `h` is a lambda or an
//!   anonymous method, a construction of a type whose name ends in `Handler`,
//!   or a path not rooted in a local whose last segment starts upper-case,
//!   `E` starting upper-case too, and `h` not a field, property or event this
//!   type declares (`Total += Price` is arithmetic).
//! - **`menu-item`** — on the node where an object is constructed into a
//!   local or a member: the type's name contains `MenuItem`, or the object is
//!   given both a `Command` and a `Header` (in its initializer or by writes to
//!   the variable in the same body). Value: the variable; args: the type.
//! - **`lazy-init`** — on the declaration of a field `f` of the type, when a
//!   getter of the type returns `f` after `if (f == null) f = …;` (or
//!   `null == f`, `f is null`, `f ??= …;`), or returns `f ??= …` or
//!   `f ?? (f = …)`. Value: the property.
//! - **`notifies-change`** — on the declaration of a property whose setter
//!   calls `OnPropertyChanged`, `RaisePropertyChanged`,
//!   `NotifyPropertyChanged`, `OnNotifyPropertyChanged`,
//!   `NotifyOfPropertyChange`, `RaisePropertyChange` or `SetProperty`, or
//!   raises `PropertyChanged` (`PropertyChanged?.Invoke(…)`,
//!   `PropertyChanged(…)`).
//! - **`swallow-exception`** — on a `catch` whose body is empty, or holds no
//!   `throw` and nothing but calls whose callee looks like logging (a segment
//!   that, without leading underscores, starts with `log` or ends with
//!   `logger`, or is `Trace`, `Debug` or `Console`, or a last segment such as
//!   `WriteLine`, `Error`, `Warn`, `Info`).
//! - **`key-compare`** — on an arm whose condition names an enum member of a
//!   type named `Key` (value), with the members of `ModifierKeys` in the same
//!   condition as args; one tag per key.
//! - **`key-gesture`** — on the node where `KeyGesture` or `KeyBinding` is
//!   constructed with a `Key` member among its arguments or initializer;
//!   modifiers as above.
//! - **`full-traversal`** — a `foreach` whose body never leaves the loop
//!   (`return`, `throw`, `yield break`, `goto`, a `break` of this loop).
//!   **`find-first`** — a loop that returns or breaks out from inside an `if`.
//!   **`accumulate`** — a loop that writes a compound assignment, `x = x + …`,
//!   `++`/`--`, or calls `Add`/`Append`/`Push`/… on a path not declared inside
//!   the loop. **`guard-return`** — an arm with a condition whose body is one
//!   `return`, `throw` or `yield break`.
//! - **Mutation** — a call of `Add`, `AddRange`, `Insert`, `Remove`, `Clear`,
//!   `Push`, `Enqueue`, `Append`… on a path is `mutate` of the receiver; a call
//!   of `Invoke` is `invoke` of the receiver (the delegate).
//! - **Statement or value** — an expression body is a `return` node when the
//!   member returns a value (a method not declared `void`, a getter, an
//!   expression-bodied property). A lambda's expression body is a `statement`
//!   when it is an invocation, an assignment, `++`/`--`, `await` or a
//!   construction, and a `return` otherwise. A `return` node's value is its
//!   `returns` when it is a constant and its `condition` (the tree an arm's
//!   condition has) otherwise, whether or not it is a test: `return x && y;`
//!   and `return items.Count;` both have one.
//!
//! A callee the syntax cannot spell as a path (`items.Where(…).Select(…)`,
//! `GetX().Run()`) has root `expression` and the names written after the
//! expression: `Select`, `Run`. A construction whose type is not written
//! (`new()`) has root `expression` and no segments.
//!
//! A generated file is read as empty, whatever its bytes. Any other file that
//! is not valid UTF-8, or that the grammar cannot be loaded for or the parser
//! gives up on, is refused (`err`) with the reason: its positions count
//! characters and its texts are strings, and neither can be told for bytes
//! that are not text. A file that parses with syntax errors is read as far as
//! it goes.

use crate::ra::plugin::types::{
    AccessMode, AccessPath, Argument, Attribute, BaseType, BodyKind, BodyNode, CallKind, CompareOp,
    Condition, Constant, DeclarationKind, DeclarationMeta, Expr, FileBodies, FlowTarget, Handler,
    Invocation, Lambda, LiteralContext, MemberAccess, MemberBody, NameOf, NodeKind, PathRoot,
    Position, Range, StringLiteral, Subscription, SubscriptionMode, Tag, TypeShape, TypedName,
    ValueRef,
};
use crate::walkers::{parse, position, Language};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use tree_sitter::Node;

#[cfg(test)]
mod tests;

/// The record kinds and tags this component reports (`manifest.emits`).
pub(crate) const EMITS: &[&str] = &[
    "member-access",
    "invocation",
    "lambda",
    "subscription",
    "string-literal",
    "name-of",
    "typed-name",
    "type-shape",
    "declaration-meta",
    "full-traversal",
    "find-first",
    "accumulate",
    "guard-return",
    "swallow-exception",
    "lazy-init",
    "notifies-change",
    "key-compare",
    "key-gesture",
    "menu-item",
];

/// How deeply statements and lambdas may nest before a body stops being
/// broken down. The walk over them recurses (everything else keeps its own
/// stack), and a guest's stack is small; past this depth a statement becomes
/// one `other("nesting-limit")` node.
const MAX_NESTING: u32 = 64;

/// Calls that change their receiver.
const MUTATORS: &[&str] = &[
    "Add",
    "AddRange",
    "Append",
    "AppendLine",
    "Clear",
    "Dequeue",
    "Enqueue",
    "Insert",
    "InsertRange",
    "Pop",
    "Push",
    "Remove",
    "RemoveAll",
    "RemoveAt",
    "RemoveRange",
    "Reverse",
    "Sort",
    "TryAdd",
    "TryRemove",
];

/// Calls that add to their receiver — what makes a loop `accumulate`.
const ACCUMULATORS: &[&str] = &[
    "Add",
    "AddRange",
    "Append",
    "AppendLine",
    "Enqueue",
    "Insert",
    "Push",
    "TryAdd",
];

/// Calls that raise a change notification from a setter.
const NOTIFIERS: &[&str] = &[
    "NotifyOfPropertyChange",
    "NotifyPropertyChanged",
    "OnNotifyPropertyChanged",
    "OnPropertyChanged",
    "RaisePropertyChange",
    "RaisePropertyChanged",
    "SetProperty",
];

/// Last segments of a call that writes to a log.
const LOG_CALLS: &[&str] = &[
    "Critical",
    "Error",
    "Exception",
    "Fatal",
    "Info",
    "Information",
    "TraceError",
    "TraceInformation",
    "TraceWarning",
    "Verbose",
    "Warn",
    "Warning",
    "Write",
    "WriteLine",
];

/// The bodies of one `.cs` file, or why it cannot be read.
pub(crate) fn bodies(path: &str, source: &[u8]) -> Result<FileBodies, String> {
    let mut out = FileBodies {
        types: Vec::new(),
        members: Vec::new(),
        typed_members: Vec::new(),
        metas: Vec::new(),
    };
    if is_generated(path, source) {
        return Ok(out);
    }
    if let Err(e) = std::str::from_utf8(source) {
        return Err(format!("not valid UTF-8 at byte {}", e.valid_up_to()));
    }
    let Some(grammar) = Language::CSharp.grammar() else {
        return Err("the C# grammar could not be loaded".into());
    };
    let Some(tree) = parse(source, grammar) else {
        return Err("the C# parser gave up on the file".into());
    };
    read_file(tree.root_node(), source, &mut out);
    Ok(out)
}

/// Generated code (KTD14): a designer or `.g.cs` file, or one whose first
/// lines carry an `<auto-generated>` header, has no bodies worth reading.
fn is_generated(path: &str, source: &[u8]) -> bool {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".designer.cs") || lower.ends_with(".g.cs") || lower.ends_with(".g.i.cs") {
        return true;
    }
    let mut end = source.len().min(4096);
    let mut lines = 0;
    for (i, b) in source[..end].iter().enumerate() {
        if *b == b'\n' {
            lines += 1;
            if lines == 20 {
                end = i;
                break;
            }
        }
    }
    let needle = b"<auto-generated";
    source[..end]
        .windows(needle.len())
        .any(|w| w.eq_ignore_ascii_case(needle))
}

// ---- Small syntax helpers ----

fn text<'t>(node: Node, src: &'t [u8]) -> &'t str {
    node.utf8_text(src).unwrap_or("")
}

fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn pos(node: Node, src: &[u8]) -> Position {
    let (line, column) = position(node, src);
    Position { line, column }
}

/// Where the last character of `node` is, in the units of [`position`].
fn end_pos(node: Node, src: &[u8]) -> Position {
    let (start, end) = (node.start_byte(), node.end_byte());
    if end <= start || end > src.len() {
        return pos(node, src);
    }
    let at = node.end_position();
    let chars = |bytes: &[u8]| bytes.iter().filter(|&&b| b & 0xC0 != 0x80).count() as u32;
    if at.column == 0 {
        // The node ends with a line break: its last character is that break.
        let last = end - 1;
        let line_start = src[..last]
            .iter()
            .rposition(|&b| b == b'\n')
            .map_or(0, |i| i + 1);
        return Position {
            line: at.row as u32,
            column: chars(&src[line_start..last]) + 1,
        };
    }
    let line_start = end - at.column;
    let mut last = end - 1;
    while last > line_start && src[last] & 0xC0 == 0x80 {
        last -= 1;
    }
    Position {
        line: at.row as u32 + 1,
        column: chars(&src[line_start..last]) + 1,
    }
}

fn range(node: Node, src: &[u8]) -> Range {
    Range {
        start: pos(node, src),
        end: end_pos(node, src),
    }
}

fn span(first: Node, last: Node, src: &[u8]) -> Range {
    Range {
        start: pos(first, src),
        end: end_pos(last, src),
    }
}

fn named_children(node: Node) -> Vec<Node> {
    let mut cursor = node.walk();
    let children: Vec<Node> = node.named_children(&mut cursor).collect();
    children
}

/// Every child with the name of the field it fills, anonymous tokens included.
fn fielded_children(node: Node) -> Vec<(Option<&'static str>, Node)> {
    let mut out = Vec::new();
    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            out.push((cursor.field_name(), cursor.node()));
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
    out
}

fn has_token(node: Node, token: &str) -> bool {
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .any(|c| !c.is_named() && c.kind() == token);
    found
}

fn first_of_kind<'t>(node: Node<'t>, kind: &str) -> Option<Node<'t>> {
    named_children(node).into_iter().find(|c| c.kind() == kind)
}

/// `node` without the parentheses around it.
fn unparen(mut node: Node) -> Node {
    while matches!(
        node.kind(),
        "parenthesized_expression" | "parenthesized_pattern"
    ) {
        match node.named_child(0) {
            Some(inner) => node = inner,
            None => break,
        }
    }
    node
}

fn starts_upper(s: &str) -> bool {
    s.chars().next().is_some_and(char::is_uppercase)
}

fn is_lambda(kind: &str) -> bool {
    matches!(kind, "lambda_expression" | "anonymous_method_expression")
}

fn is_string_literal(kind: &str) -> bool {
    matches!(
        kind,
        "string_literal" | "verbatim_string_literal" | "raw_string_literal"
    )
}

fn is_statement_kind(kind: &str) -> bool {
    kind == "block" || kind.ends_with("_statement")
}

fn is_path_kind(kind: &str) -> bool {
    matches!(
        kind,
        "identifier"
            | "generic_name"
            | "this"
            | "member_access_expression"
            | "conditional_access_expression"
            | "qualified_name"
            | "predefined_type"
    )
}

/// A constant the syntax names: `true`, `false`, `null`, a number.
fn constant_of(node: Node, src: &[u8]) -> Option<Constant> {
    let node = unparen(node);
    match node.kind() {
        "boolean_literal" => Some(Constant::Boolean(text(node, src) == "true")),
        "null_literal" => Some(Constant::Null),
        "integer_literal" | "real_literal" => Some(Constant::Number(text(node, src).to_string())),
        "prefix_unary_expression"
            if has_token(node, "-")
                && node
                    .named_child(0)
                    .is_some_and(|c| matches!(c.kind(), "integer_literal" | "real_literal")) =>
        {
            Some(Constant::Number(
                text(node, src).split_whitespace().collect(),
            ))
        }
        _ => None,
    }
}

/// The value of a string literal, escapes resolved.
fn literal_value(node: Node, src: &[u8]) -> String {
    match node.kind() {
        "verbatim_string_literal" => {
            let t = text(node, src);
            let t = t.strip_prefix('@').unwrap_or(t);
            let t = t.strip_prefix('"').unwrap_or(t);
            let t = t.strip_suffix('"').unwrap_or(t);
            t.replace("\"\"", "\"")
        }
        "raw_string_literal" => named_children(node)
            .into_iter()
            .filter(|c| c.kind() == "raw_string_content")
            .map(|c| text(c, src))
            .collect(),
        _ => {
            let mut out = String::new();
            for part in named_children(node) {
                match part.kind() {
                    "string_literal_content" => out.push_str(text(part, src)),
                    "escape_sequence" => unescape(text(part, src), &mut out),
                    _ => {}
                }
            }
            out
        }
    }
}

fn unescape(escape: &str, out: &mut String) {
    let body = escape.strip_prefix('\\').unwrap_or(escape);
    let simple = match body {
        "n" => Some('\n'),
        "t" => Some('\t'),
        "r" => Some('\r'),
        "0" => Some('\0'),
        "\\" => Some('\\'),
        "\"" => Some('"'),
        "'" => Some('\''),
        "a" => Some('\u{7}'),
        "b" => Some('\u{8}'),
        "f" => Some('\u{c}'),
        "v" => Some('\u{b}'),
        "e" => Some('\u{1b}'),
        _ => None,
    };
    if let Some(c) = simple {
        out.push(c);
        return;
    }
    let hex = body
        .strip_prefix('u')
        .or_else(|| body.strip_prefix('U'))
        .or_else(|| body.strip_prefix('x'));
    match hex
        .and_then(|h| u32::from_str_radix(h, 16).ok())
        .and_then(char::from_u32)
    {
        Some(c) => out.push(c),
        None => out.push_str(escape),
    }
}

/// The segments a type spells, generic arguments dropped, and the generic
/// arguments of its last segment as written: `Acme.ViewBase<EditorViewModel>`
/// is [`Acme`, `ViewBase`] and [`EditorViewModel`].
fn type_segments(node: Node, src: &[u8]) -> Option<(Vec<String>, Vec<String>)> {
    match node.kind() {
        "identifier" | "predefined_type" => Some((vec![text(node, src).to_string()], Vec::new())),
        "generic_name" => {
            let name = first_of_kind(node, "identifier")?;
            let args = first_of_kind(node, "type_argument_list")
                .map(|list| {
                    named_children(list)
                        .into_iter()
                        .map(|a| collapse(text(a, src)))
                        .collect()
                })
                .unwrap_or_default();
            Some((vec![text(name, src).to_string()], args))
        }
        "qualified_name" => {
            let (mut segments, _) = type_segments(node.child_by_field_name("qualifier")?, src)?;
            let (name, args) = type_segments(node.child_by_field_name("name")?, src)?;
            segments.extend(name);
            Some((segments, args))
        }
        "nullable_type" => type_segments(node.child_by_field_name("type")?, src),
        _ => None,
    }
}

/// A declared type as a typed name states it: its dotted name, generic
/// arguments dropped, and those arguments; any other type (an array, a
/// tuple) as written.
fn type_name(node: Node, src: &[u8]) -> (String, Vec<String>) {
    match type_segments(node, src) {
        Some((segments, args)) => (segments.join("."), args),
        None => (collapse(text(node, src)), Vec::new()),
    }
}

/// The segment before the member of an enum reference as written
/// (`Key` of `Acme.Key.F4`).
fn enum_type(reference: &str) -> Option<&str> {
    let mut parts = reference.rsplit('.');
    parts.next()?;
    parts.next().map(str::trim)
}

/// One tag per `Key` member among `enum_refs`, the `ModifierKeys` members
/// as its args.
fn key_tags(name: &str, enum_refs: &[String]) -> Vec<Tag> {
    let mut keys: Vec<&String> = Vec::new();
    let mut modifiers: Vec<String> = Vec::new();
    for r in enum_refs {
        match enum_type(r) {
            Some("Key") if !keys.contains(&r) => keys.push(r),
            Some("ModifierKeys") if !modifiers.contains(r) => modifiers.push(r.clone()),
            _ => {}
        }
    }
    keys.into_iter()
        .map(|k| tag(name, Some(k.clone()), modifiers.clone()))
        .collect()
}

fn tag(name: &str, value: Option<String>, args: Vec<String>) -> Tag {
    Tag {
        name: name.into(),
        value,
        args,
    }
}

fn push_tag(tags: &mut Vec<Tag>, t: Tag) {
    if !tags.contains(&t) {
        tags.push(t);
    }
}

fn is_enum_candidate(path: &AccessPath) -> bool {
    path.root == PathRoot::Name
        && path.segments.len() >= 2
        && path.segments.iter().all(|s| starts_upper(s))
}

fn is_log_like(callee: &AccessPath) -> bool {
    let segments: Vec<&str> = callee.segments.iter().map(String::as_str).collect();
    let named = segments.iter().any(|s| {
        let lower = s.trim_start_matches('_').to_ascii_lowercase();
        lower.starts_with("log") || lower.ends_with("logger")
    }) || segments
        .iter()
        .any(|s| matches!(*s, "Trace" | "Debug" | "Console"));
    named || segments.last().is_some_and(|s| LOG_CALLS.contains(s))
}

// ---- The file: types and their members ----

/// What a type declares that a body's heuristics look up.
#[derive(Default)]
struct TypeCtx {
    /// Fields, properties and events.
    data: BTreeSet<String>,
    fields: BTreeSet<String>,
}

fn is_type_declaration(kind: &str) -> bool {
    matches!(
        kind,
        "class_declaration"
            | "struct_declaration"
            | "record_declaration"
            | "interface_declaration"
            | "enum_declaration"
    )
}

fn read_file(root: Node, src: &[u8], out: &mut FileBodies) {
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        match node.kind() {
            "compilation_unit" | "declaration_list" | "file_scoped_namespace_declaration" => {
                for child in named_children(node).into_iter().rev() {
                    stack.push(child);
                }
            }
            "namespace_declaration" => {
                if let Some(body) = node.child_by_field_name("body") {
                    stack.push(body);
                }
            }
            kind if is_type_declaration(kind) => {
                read_type(node, src, out);
                if kind != "enum_declaration" {
                    if let Some(body) = node.child_by_field_name("body") {
                        stack.push(body);
                    }
                }
            }
            _ => {}
        }
    }
}

fn modifiers(node: Node, src: &[u8]) -> Vec<String> {
    named_children(node)
        .into_iter()
        .filter(|c| c.kind() == "modifier")
        .map(|c| collapse(text(c, src)))
        .collect()
}

fn attributes(node: Node, src: &[u8]) -> Vec<Attribute> {
    let mut out = Vec::new();
    for list in named_children(node) {
        if list.kind() != "attribute_list" {
            continue;
        }
        for attr in named_children(list) {
            if attr.kind() != "attribute" {
                continue;
            }
            let name = attr
                .child_by_field_name("name")
                .map(|n| collapse(text(n, src)))
                .unwrap_or_default();
            let args = first_of_kind(attr, "attribute_argument_list")
                .map(|l| {
                    named_children(l)
                        .into_iter()
                        .filter(|a| a.kind() == "attribute_argument")
                        .map(|a| collapse(text(a, src)))
                        .collect()
                })
                .unwrap_or_default();
            out.push(Attribute { name, args });
        }
    }
    out
}

fn meta(node: Node, name_position: Position, src: &[u8]) -> DeclarationMeta {
    DeclarationMeta {
        name_position,
        modifiers: modifiers(node, src),
        attributes: attributes(node, src),
        tags: Vec::new(),
    }
}

fn type_shape(node: Node, name: Node, src: &[u8]) -> TypeShape {
    let type_params = first_of_kind(node, "type_parameter_list")
        .map(|list| {
            named_children(list)
                .into_iter()
                .filter(|p| p.kind() == "type_parameter")
                .filter_map(|p| p.child_by_field_name("name"))
                .map(|n| text(n, src).to_string())
                .collect()
        })
        .unwrap_or_default();
    let mut bases = Vec::new();
    if node.kind() != "enum_declaration" {
        if let Some(list) = first_of_kind(node, "base_list") {
            for entry in named_children(list) {
                let ty = match entry.kind() {
                    "argument_list" => continue,
                    "primary_constructor_base_type" => match entry.child_by_field_name("type") {
                        Some(t) => t,
                        None => continue,
                    },
                    _ => entry,
                };
                if ty.kind() == "predefined_type" {
                    continue;
                }
                if let Some((base, type_args)) = type_segments(ty, src) {
                    bases.push(BaseType { base, type_args });
                }
            }
        }
    }
    TypeShape {
        name_position: pos(name, src),
        type_params,
        bases,
    }
}

/// `int a, b = 1;`: every declarator's name and initializer.
fn declarators(decl: Node) -> Vec<(Node, Option<Node>)> {
    let mut out = Vec::new();
    for d in named_children(decl) {
        if d.kind() != "variable_declarator" {
            continue;
        }
        let Some(name) = d.child_by_field_name("name") else {
            continue;
        };
        let init = fielded_children(d).into_iter().find_map(|(field, c)| {
            (field.is_none()
                && c.is_named()
                && !matches!(c.kind(), "bracketed_argument_list" | "tuple_pattern"))
            .then_some(c)
        });
        out.push((name, init));
    }
    out
}

fn read_type(node: Node, src: &[u8], out: &mut FileBodies) {
    let Some(name) = node.child_by_field_name("name") else {
        return;
    };
    out.types.push(type_shape(node, name, src));
    let mut metas = vec![meta(node, pos(name, src), src)];
    let Some(body) = node.child_by_field_name("body") else {
        out.metas
            .extend(metas.into_iter().filter(meta_says_something));
        return;
    };
    let members = named_children(body);

    let mut ty = TypeCtx::default();
    for m in &members {
        match m.kind() {
            "field_declaration" | "event_field_declaration" => {
                if let Some(decl) = first_of_kind(*m, "variable_declaration") {
                    for (n, _) in declarators(decl) {
                        let n = text(n, src).to_string();
                        if m.kind() == "field_declaration" {
                            ty.fields.insert(n.clone());
                        }
                        ty.data.insert(n);
                    }
                }
            }
            "property_declaration" | "event_declaration" => {
                if let Some(n) = m.child_by_field_name("name") {
                    ty.data.insert(text(n, src).to_string());
                }
            }
            _ => {}
        }
    }

    // Indices into `metas` of the fields and properties a tag can land on.
    let mut field_meta: BTreeMap<String, usize> = BTreeMap::new();
    let mut property_meta: BTreeMap<String, usize> = BTreeMap::new();
    let mut lazy: Vec<(String, String)> = Vec::new();
    let mut notifying: Vec<String> = Vec::new();

    for m in members {
        match m.kind() {
            "field_declaration" | "event_field_declaration" => {
                let Some(decl) = first_of_kind(m, "variable_declaration") else {
                    continue;
                };
                let kind = if m.kind() == "field_declaration" {
                    DeclarationKind::Field
                } else {
                    DeclarationKind::Event
                };
                let declared = decl.child_by_field_name("type");
                for (n, init) in declarators(decl) {
                    let name_position = pos(n, src);
                    let name_text = text(n, src).to_string();
                    if let Some(t) = declared {
                        let (type_name, type_args) = type_name(t, src);
                        out.typed_members.push(TypedName {
                            name: name_text.clone(),
                            type_name,
                            type_args,
                            kind,
                            position: name_position,
                            scope: None,
                        });
                    }
                    if kind == DeclarationKind::Field {
                        field_meta.insert(name_text.clone(), metas.len());
                    }
                    metas.push(meta(m, name_position, src));
                    if let Some(e) = init {
                        let b = Builder::new(
                            src,
                            &ty,
                            BodyKind::MemberInitializer,
                            name_position,
                            None,
                            false,
                            Vec::new(),
                        );
                        out.members.extend(b.build(Root::Initializer(e, name_text)));
                    }
                }
            }
            "property_declaration" | "event_declaration" => {
                let Some(n) = m.child_by_field_name("name") else {
                    continue;
                };
                let name_position = pos(n, src);
                let name_text = text(n, src).to_string();
                let is_property = m.kind() == "property_declaration";
                if let Some(t) = m.child_by_field_name("type") {
                    let (type_name, type_args) = type_name(t, src);
                    out.typed_members.push(TypedName {
                        name: name_text.clone(),
                        type_name,
                        type_args,
                        kind: if is_property {
                            DeclarationKind::Property
                        } else {
                            DeclarationKind::Event
                        },
                        position: name_position,
                        scope: None,
                    });
                }
                if is_property {
                    property_meta.insert(name_text.clone(), metas.len());
                }
                metas.push(meta(m, name_position, src));
                if let Some(value) = m.child_by_field_name("value") {
                    let (kind, root) = if value.kind() == "arrow_expression_clause" {
                        if let Some(field) = lazy_field_arrow(value, &ty, src) {
                            lazy.push((field, name_text.clone()));
                        }
                        (BodyKind::ExpressionProperty, Root::Arrow(value, true))
                    } else {
                        (
                            BodyKind::MemberInitializer,
                            Root::Initializer(value, name_text.clone()),
                        )
                    };
                    let b = Builder::new(src, &ty, kind, name_position, None, false, Vec::new());
                    out.members.extend(b.build(root));
                }
                let Some(accessors) = m.child_by_field_name("accessors") else {
                    continue;
                };
                for acc in named_children(accessors) {
                    if acc.kind() != "accessor_declaration" {
                        continue;
                    }
                    let Some(body) = acc.child_by_field_name("body") else {
                        continue;
                    };
                    let word = acc
                        .child_by_field_name("name")
                        .map(|w| text(w, src))
                        .unwrap_or("");
                    let kind = match word {
                        "get" => BodyKind::Getter,
                        "set" => BodyKind::Setter,
                        "init" => BodyKind::Initializer,
                        "add" => BodyKind::Adder,
                        "remove" => BodyKind::Remover,
                        _ => BodyKind::Other,
                    };
                    let root = if body.kind() == "block" {
                        Root::Block(body)
                    } else {
                        Root::Arrow(body, kind == BodyKind::Getter)
                    };
                    if kind == BodyKind::Getter {
                        let field = if body.kind() == "block" {
                            lazy_field_block(body, &ty, src)
                        } else {
                            lazy_field_arrow(body, &ty, src)
                        };
                        if let Some(field) = field {
                            lazy.push((field, name_text.clone()));
                        }
                    }
                    let implicit_value = kind != BodyKind::Getter;
                    let b = Builder::new(
                        src,
                        &ty,
                        kind,
                        name_position,
                        None,
                        implicit_value,
                        Vec::new(),
                    );
                    let built = b.build(root);
                    if kind == BodyKind::Setter && is_property && notifies(&built[0]) {
                        notifying.push(name_text.clone());
                    }
                    out.members.extend(built);
                }
            }
            "method_declaration" => {
                let Some(n) = m.child_by_field_name("name") else {
                    continue;
                };
                let name_position = pos(n, src);
                metas.push(meta(m, name_position, src));
                let Some(body) = m.child_by_field_name("body") else {
                    continue;
                };
                let returns_value = m
                    .child_by_field_name("returns")
                    .is_some_and(|t| text(t, src) != "void");
                let root = if body.kind() == "block" {
                    Root::Block(body)
                } else {
                    Root::Arrow(body, returns_value)
                };
                let b = Builder::new(
                    src,
                    &ty,
                    BodyKind::Method,
                    name_position,
                    m.child_by_field_name("parameters"),
                    false,
                    Vec::new(),
                );
                out.members.extend(b.build(root));
            }
            "constructor_declaration" => {
                let Some(n) = m.child_by_field_name("name") else {
                    continue;
                };
                let name_position = pos(n, src);
                metas.push(meta(m, name_position, src));
                let init = first_of_kind(m, "constructor_initializer");
                let body = m.child_by_field_name("body");
                if init.is_none() && body.is_none() {
                    continue;
                }
                let kind = if modifiers(m, src).iter().any(|w| w == "static") {
                    BodyKind::TypeInitializer
                } else {
                    BodyKind::Constructor
                };
                let b = Builder::new(
                    src,
                    &ty,
                    kind,
                    name_position,
                    m.child_by_field_name("parameters"),
                    false,
                    Vec::new(),
                );
                out.members
                    .extend(b.build(Root::Constructor { init, body }));
            }
            "enum_member_declaration" => {
                if let Some(n) = m.child_by_field_name("name") {
                    metas.push(meta(m, pos(n, src), src));
                }
            }
            _ => {}
        }
    }

    for (field, property) in lazy {
        if let Some(&i) = field_meta.get(&field) {
            push_tag(
                &mut metas[i].tags,
                tag("lazy-init", Some(property), Vec::new()),
            );
        }
    }
    for property in notifying {
        if let Some(&i) = property_meta.get(&property) {
            push_tag(&mut metas[i].tags, tag("notifies-change", None, Vec::new()));
        }
    }
    out.metas
        .extend(metas.into_iter().filter(meta_says_something));
}

fn meta_says_something(m: &DeclarationMeta) -> bool {
    !(m.modifiers.is_empty() && m.attributes.is_empty() && m.tags.is_empty())
}

/// Whether a setter's body raises a change notification.
fn notifies(setter: &MemberBody) -> bool {
    setter.invocations.iter().any(|i| {
        let s = &i.callee.segments;
        let last = s.last().map(String::as_str).unwrap_or("");
        NOTIFIERS.contains(&last)
            || (last == "Invoke" && s.iter().any(|x| x == "PropertyChanged"))
            || (s.len() == 1 && last == "PropertyChanged")
    })
}

/// The field a path names when it is `f` or `this.f`.
fn field_name(node: Node, ty: &TypeCtx, src: &[u8]) -> Option<String> {
    let node = unparen(node);
    let name = match node.kind() {
        "identifier" => text(node, src),
        "member_access_expression"
            if node
                .child_by_field_name("expression")
                .is_some_and(|e| e.kind() == "this") =>
        {
            text(node.child_by_field_name("name")?, src)
        }
        _ => return None,
    };
    ty.fields.contains(name).then(|| name.to_string())
}

/// `f ??= …` or `f ?? (f = …)` — the field initialized on first read.
fn lazy_expression(expr: Node, ty: &TypeCtx, src: &[u8]) -> Option<String> {
    let expr = unparen(expr);
    let op = expr.child_by_field_name("operator").map(|o| text(o, src));
    match (expr.kind(), op) {
        ("assignment_expression", Some("??=")) => {
            field_name(expr.child_by_field_name("left")?, ty, src)
        }
        ("binary_expression", Some("??")) => {
            let field = field_name(expr.child_by_field_name("left")?, ty, src)?;
            let right = unparen(expr.child_by_field_name("right")?);
            let assigned = right.kind() == "assignment_expression"
                && right
                    .child_by_field_name("left")
                    .and_then(|l| field_name(l, ty, src))
                    .as_deref()
                    == Some(field.as_str());
            assigned.then_some(field)
        }
        _ => None,
    }
}

fn lazy_field_arrow(clause: Node, ty: &TypeCtx, src: &[u8]) -> Option<String> {
    lazy_expression(clause.named_child(0)?, ty, src)
}

/// Whether `cond` tests `field` for null: `f == null`, `null == f`,
/// `f is null`.
fn is_null_test(cond: Node, field: &str, ty: &TypeCtx, src: &[u8]) -> bool {
    let cond = unparen(cond);
    match cond.kind() {
        "binary_expression" => {
            let op = cond.child_by_field_name("operator").map(|o| text(o, src));
            let (Some(l), Some(r)) = (
                cond.child_by_field_name("left"),
                cond.child_by_field_name("right"),
            ) else {
                return false;
            };
            let is = |side: Node, other: Node| {
                field_name(side, ty, src).as_deref() == Some(field)
                    && unparen(other).kind() == "null_literal"
            };
            op == Some("==") && (is(l, r) || is(r, l))
        }
        "is_pattern_expression" => {
            cond.child_by_field_name("expression")
                .and_then(|e| field_name(e, ty, src))
                .as_deref()
                == Some(field)
                && cond.child_by_field_name("pattern").is_some_and(|p| {
                    p.kind() == "constant_pattern"
                        && p.named_child(0).is_some_and(|c| c.kind() == "null_literal")
                })
        }
        _ => false,
    }
}

/// Whether `stmt` (a statement or a block) assigns `field` at its top level.
fn assigns_field(stmt: Node, field: &str, ty: &TypeCtx, src: &[u8]) -> bool {
    let statements = if stmt.kind() == "block" {
        named_children(stmt)
    } else {
        vec![stmt]
    };
    statements.into_iter().any(|s| {
        s.kind() == "expression_statement"
            && s.named_child(0).is_some_and(|e| {
                e.kind() == "assignment_expression"
                    && e.child_by_field_name("left")
                        .and_then(|l| field_name(l, ty, src))
                        .as_deref()
                        == Some(field)
            })
    })
}

/// A getter block that initializes a field on first read.
fn lazy_field_block(block: Node, ty: &TypeCtx, src: &[u8]) -> Option<String> {
    let statements = named_children(block);
    let (i, ret) = statements
        .iter()
        .enumerate()
        .find(|(_, s)| s.kind() == "return_statement")?;
    let value = ret.named_child(0)?;
    if let Some(field) = lazy_expression(value, ty, src) {
        return Some(field);
    }
    let field = field_name(value, ty, src)?;
    let initialized = statements[..i].iter().any(|before| match before.kind() {
        "if_statement" => {
            before
                .child_by_field_name("condition")
                .is_some_and(|c| is_null_test(c, &field, ty, src))
                && before
                    .child_by_field_name("consequence")
                    .is_some_and(|c| assigns_field(c, &field, ty, src))
        }
        "expression_statement" => {
            before
                .named_child(0)
                .and_then(|e| lazy_expression(e, ty, src))
                .as_deref()
                == Some(field.as_str())
        }
        _ => false,
    });
    initialized.then_some(field)
}

// ---- One member body ----

/// How a body begins.
enum Root<'t> {
    Block(Node<'t>),
    /// `=> e`: the clause, and whether the member returns its value.
    Arrow(Node<'t>, bool),
    /// `= e` of a field or property, and the name it initializes.
    Initializer(Node<'t>, String),
    Constructor {
        init: Option<Node<'t>>,
        body: Option<Node<'t>>,
    },
}

/// What an expression is being read as, handed down the walk.
#[derive(Clone)]
struct Ctx {
    /// The innermost body node.
    node: u32,
    /// Where this value flows, when it is the whole value of an assignment,
    /// an argument or a return.
    flow: Option<FlowTarget>,
    /// The context a string literal written as this value has.
    lit: LiteralContext,
    /// How a path written as this value is accessed: `read` or `return`.
    mode: AccessMode,
    /// The invocation and position this value is an argument of.
    arg_of: Option<(u32, u32)>,
    /// When it is an argument of a construction: where the construction
    /// flows (what a lambda argument flows to).
    construction_flow: Option<FlowTarget>,
    /// Never an enum member: the left operand of a binary operator whose
    /// right operand looks like one.
    excluded: bool,
}

impl Ctx {
    fn plain(node: u32) -> Ctx {
        Ctx {
            node,
            flow: None,
            lit: LiteralContext::Other,
            mode: AccessMode::Read,
            arg_of: None,
            construction_flow: None,
            excluded: false,
        }
    }
}

/// A value whose reference is known only once the walk has reached it.
enum Fixup<'t> {
    Argument {
        invocation: usize,
        argument: usize,
        expr: Node<'t>,
    },
    Write {
        access: usize,
        expr: Node<'t>,
    },
}

/// An object constructed into a variable — a `menu-item` candidate.
struct Built {
    variable: String,
    node: u32,
    type_text: String,
    members: BTreeSet<String>,
}

type Work<'t> = Vec<(Node<'t>, Ctx)>;

struct Builder<'t> {
    src: &'t [u8],
    ty: &'t TypeCtx,
    kind: BodyKind,
    name_position: Position,
    params: Option<Node<'t>>,
    implicit_value: bool,
    outer: Vec<String>,
    nodes: Vec<BodyNode>,
    accesses: Vec<MemberAccess>,
    invocations: Vec<Invocation>,
    lambdas: Vec<Lambda>,
    subscriptions: Vec<Subscription>,
    literals: Vec<StringLiteral>,
    name_ofs: Vec<NameOf>,
    typed_names: Vec<TypedName>,
    /// Names in scope, innermost frame last; a parameter of the member with
    /// its index.
    scopes: Vec<Vec<(String, Option<u32>)>>,
    literal_of: HashMap<usize, u32>,
    invocation_of: HashMap<usize, u32>,
    lambda_of: HashMap<usize, u32>,
    lambda_depth: u32,
    depth: u32,
    built: Vec<Built>,
    /// Local functions, each a body of its own.
    locals: Vec<MemberBody>,
}

impl<'t> Builder<'t> {
    fn new(
        src: &'t [u8],
        ty: &'t TypeCtx,
        kind: BodyKind,
        name_position: Position,
        params: Option<Node<'t>>,
        implicit_value: bool,
        outer: Vec<String>,
    ) -> Builder<'t> {
        Builder {
            src,
            ty,
            kind,
            name_position,
            params,
            implicit_value,
            outer,
            nodes: Vec::new(),
            accesses: Vec::new(),
            invocations: Vec::new(),
            lambdas: Vec::new(),
            subscriptions: Vec::new(),
            literals: Vec::new(),
            name_ofs: Vec::new(),
            typed_names: Vec::new(),
            scopes: Vec::new(),
            literal_of: HashMap::new(),
            invocation_of: HashMap::new(),
            lambda_of: HashMap::new(),
            lambda_depth: 0,
            depth: 0,
            built: Vec::new(),
            locals: Vec::new(),
        }
    }

    fn text(&self, node: Node) -> &'t str {
        text(node, self.src)
    }

    fn range(&self, node: Node) -> Range {
        range(node, self.src)
    }

    /// The body, then the bodies of the local functions declared in it.
    fn build(mut self, root: Root<'t>) -> Vec<MemberBody> {
        let mut frame: Vec<(String, Option<u32>)> =
            self.outer.iter().map(|n| (n.clone(), None)).collect();
        if let Some(params) = self.params {
            let mut index = 0;
            for p in named_children(params) {
                if p.kind() != "parameter" {
                    continue;
                }
                if let Some(n) = p.child_by_field_name("name") {
                    let name = self.text(n).to_string();
                    if let Some(t) = p.child_by_field_name("type") {
                        let (type_name, type_args) = type_name(t, self.src);
                        self.typed_names.push(TypedName {
                            name: name.clone(),
                            type_name,
                            type_args,
                            kind: DeclarationKind::Parameter,
                            position: pos(n, self.src),
                            scope: None,
                        });
                    }
                    frame.push((name, Some(index)));
                }
                index += 1;
            }
        }
        if self.implicit_value {
            frame.push(("value".into(), None));
        }
        self.scopes.push(frame);

        match root {
            Root::Block(block) => {
                let r = self.add(NodeKind::Body, self.range(block), None);
                self.block_into(block, r);
            }
            Root::Arrow(clause, returns_value) => {
                let r = self.add(NodeKind::Body, self.range(clause), None);
                if let Some(e) = clause.named_child(0) {
                    self.expression_body(e, r, returns_value);
                }
            }
            Root::Initializer(expr, name) => {
                let r = self.add(NodeKind::Body, self.range(expr), None);
                let s = self.add(NodeKind::Statement, self.range(expr), Some(r));
                let target = AccessPath {
                    root: PathRoot::Name,
                    segments: vec![name.clone()],
                };
                let place = self.range(expr);
                self.write_from(target, place, expr, s, Some(FlowTarget::Member(name)));
            }
            Root::Constructor { init, body } => {
                let (Some(first), Some(last)) = (init.or(body), body.or(init)) else {
                    return self.finish();
                };
                let r = self.add(NodeKind::Body, span(first, last, self.src), None);
                if let Some(init) = init {
                    self.constructor_initializer(init, r);
                }
                match body {
                    Some(b) if b.kind() == "block" => self.block_into(b, r),
                    Some(clause) => {
                        if let Some(e) = clause.named_child(0) {
                            self.expression_body(e, r, false);
                        }
                    }
                    None => {}
                }
            }
        }
        self.finish()
    }

    fn finish(mut self) -> Vec<MemberBody> {
        for b in std::mem::take(&mut self.built) {
            let mut members = b.members;
            for a in &self.accesses {
                let s = &a.target.segments;
                if a.mode == AccessMode::Write && s.len() == 2 && s[0] == b.variable {
                    members.insert(s[1].clone());
                }
            }
            let is_menu = b.type_text.contains("MenuItem")
                || (members.contains("Command") && members.contains("Header"));
            if is_menu {
                let t = tag("menu-item", Some(b.variable), vec![b.type_text]);
                push_tag(&mut self.nodes[b.node as usize].tags, t);
            }
        }
        let mut out = vec![MemberBody {
            name_position: self.name_position,
            kind: self.kind,
            root: 0,
            nodes: self.nodes,
            accesses: self.accesses,
            invocations: self.invocations,
            lambdas: self.lambdas,
            subscriptions: self.subscriptions,
            literals: self.literals,
            name_ofs: self.name_ofs,
            typed_names: self.typed_names,
            tags: Vec::new(),
        }];
        out.extend(self.locals);
        out
    }

    // ---- nodes and scope ----

    fn add(&mut self, kind: NodeKind, place: Range, parent: Option<u32>) -> u32 {
        let id = self.nodes.len() as u32;
        self.nodes.push(BodyNode {
            id,
            kind,
            place,
            parent,
            condition: None,
            returns: None,
            tags: Vec::new(),
        });
        id
    }

    fn add_at(&mut self, kind: NodeKind, node: Node, parent: u32) -> u32 {
        let place = self.range(node);
        self.add(kind, place, Some(parent))
    }

    fn tag_node(&mut self, node: u32, t: Tag) {
        push_tag(&mut self.nodes[node as usize].tags, t);
    }

    fn declare(&mut self, name: &str) {
        if let Some(frame) = self.scopes.last_mut() {
            frame.push((name.to_string(), None));
        }
    }

    /// The innermost declaration of `name`: `Some(Some(i))` for the `i`-th
    /// parameter of the member, `Some(None)` for any other name in scope.
    fn lookup(&self, name: &str) -> Option<Option<u32>> {
        self.scopes
            .iter()
            .rev()
            .find_map(|f| f.iter().rev().find(|(n, _)| n == name).map(|(_, p)| *p))
    }

    fn is_local(&self, name: &str) -> bool {
        self.lookup(name).is_some()
    }

    fn typed(&mut self, name: Node, ty: Node, kind: DeclarationKind, scope: Option<u32>) {
        if ty.kind() == "implicit_type" {
            return;
        }
        let (type_name, type_args) = type_name(ty, self.src);
        self.typed_names.push(TypedName {
            name: self.text(name).to_string(),
            type_name,
            type_args,
            kind,
            position: pos(name, self.src),
            scope,
        });
    }

    // ---- paths ----

    /// The access path `node` spells, and whether it goes through `?.`.
    fn path(&self, node: Node) -> Option<(AccessPath, bool)> {
        let mut segments: Vec<String> = Vec::new();
        let mut conditional = false;
        let mut at = node;
        let root = loop {
            match at.kind() {
                "member_access_expression" => {
                    let name = at.child_by_field_name("name")?;
                    segments.push(self.simple_name(name)?);
                    at = at.child_by_field_name("expression")?;
                }
                "conditional_access_expression" => {
                    let binding = first_of_kind(at, "member_binding_expression")?;
                    segments.push(self.simple_name(binding.child_by_field_name("name")?)?);
                    conditional = true;
                    at = at.child_by_field_name("condition")?;
                }
                "identifier" | "generic_name" => {
                    let name = self.simple_name(at)?;
                    let root = if self.is_local(&name) {
                        PathRoot::Local
                    } else {
                        PathRoot::Name
                    };
                    segments.push(name);
                    break root;
                }
                "predefined_type" => {
                    segments.push(self.text(at).to_string());
                    break PathRoot::Name;
                }
                "qualified_name" => {
                    let (s, _) = type_segments(at, self.src)?;
                    segments.extend(s.into_iter().rev());
                    break PathRoot::Name;
                }
                "this" => break PathRoot::This,
                "base" => break PathRoot::Base,
                _ => return None,
            }
        };
        segments.reverse();
        Some((AccessPath { root, segments }, conditional))
    }

    fn simple_name(&self, node: Node) -> Option<String> {
        match node.kind() {
            "identifier" => Some(self.text(node).to_string()),
            "generic_name" => first_of_kind(node, "identifier").map(|i| self.text(i).to_string()),
            _ => None,
        }
    }

    fn enum_candidate_node(&self, node: Node) -> bool {
        self.path(unparen(node))
            .is_some_and(|(p, _)| is_enum_candidate(&p))
    }

    /// Where a write to `target` sends its value: a member of this type, a
    /// local, or — a longer path (`f4.Command`, `this.view.Model`) — a member
    /// of another object.
    fn flow_to(target: &AccessPath) -> Option<FlowTarget> {
        match (target.root, target.segments.as_slice()) {
            (PathRoot::This | PathRoot::Name, [name]) => Some(FlowTarget::Member(name.clone())),
            (PathRoot::Local, [name]) => Some(FlowTarget::Local(name.clone())),
            (_, [_, _, ..]) => Some(FlowTarget::Path(target.clone())),
            _ => None,
        }
    }

    fn value_ref(&self, expr: Node) -> ValueRef {
        let e = unparen(expr);
        match e.kind() {
            "identifier" => {
                if let Some(Some(i)) = self.lookup(self.text(e)) {
                    return ValueRef::Param(i);
                }
                self.path(e)
                    .map_or(ValueRef::Other, |(p, _)| ValueRef::Access(p))
            }
            k if is_path_kind(k) => self
                .path(e)
                .map_or(ValueRef::Other, |(p, _)| ValueRef::Access(p)),
            k if is_lambda(k) => self
                .lambda_of
                .get(&e.id())
                .map_or(ValueRef::Other, |&i| ValueRef::Lambda(i)),
            k if is_string_literal(k) => self
                .literal_of
                .get(&e.id())
                .map_or(ValueRef::Other, |&i| ValueRef::Literal(i)),
            "object_creation_expression" | "implicit_object_creation_expression" => self
                .invocation_of
                .get(&e.id())
                .map_or(ValueRef::Other, |&i| ValueRef::Construction(i)),
            // `p ?? throw …` is `p` or no value at all: the guard of a
            // constructor that stores its parameter.
            "binary_expression"
                if has_token(e, "??")
                    && e.child_by_field_name("right")
                        .is_some_and(|r| unparen(r).kind() == "throw_expression") =>
            {
                e.child_by_field_name("left")
                    .map_or(ValueRef::Other, |l| self.value_ref(l))
            }
            _ => constant_of(e, self.src).map_or(ValueRef::Other, ValueRef::Constant),
        }
    }

    // ---- statements ----

    /// The statements of `node` as children of `parent`: a block is not a
    /// node of its own here, so `if (c) return;` and `if (c) { return; }`
    /// are the same arm.
    fn block_into(&mut self, node: Node<'t>, parent: u32) {
        if node.kind() != "block" {
            self.stmt(node, parent);
            return;
        }
        self.scopes.push(Vec::new());
        let statements = named_children(node);
        for s in &statements {
            if s.kind() == "local_function_statement" {
                if let Some(n) = s.child_by_field_name("name") {
                    let name = self.text(n);
                    self.declare(name);
                }
            }
        }
        for s in statements {
            self.stmt(s, parent);
        }
        self.scopes.pop();
    }

    fn stmt(&mut self, node: Node<'t>, parent: u32) {
        if self.depth >= MAX_NESTING {
            self.add_at(NodeKind::Other("nesting-limit".into()), node, parent);
            return;
        }
        self.depth += 1;
        match node.kind() {
            "block" => {
                let b = self.add_at(NodeKind::Block, node, parent);
                self.block_into(node, b);
            }
            "expression_statement" => {
                let s = self.add_at(NodeKind::Statement, node, parent);
                if let Some(e) = node.named_child(0) {
                    self.expr(e, Ctx::plain(s));
                }
            }
            "local_declaration_statement" => {
                let s = self.add_at(NodeKind::Statement, node, parent);
                for decl in named_children(node) {
                    if decl.kind() == "variable_declaration" {
                        self.variables(decl, s);
                    }
                }
            }
            "return_statement" => {
                let r = self.add_at(NodeKind::Return, node, parent);
                if let Some(e) = node.named_child(0) {
                    let ctx = self.return_ctx(r);
                    self.expr(e, ctx);
                    self.returned(r, e);
                }
            }
            "throw_statement" => {
                let t = self.add_at(NodeKind::Throw, node, parent);
                if let Some(e) = node.named_child(0) {
                    self.expr(e, Ctx::plain(t));
                }
            }
            "yield_statement" => {
                if has_token(node, "break") {
                    self.add_at(NodeKind::YieldBreak, node, parent);
                } else {
                    let y = self.add_at(NodeKind::YieldReturn, node, parent);
                    if let Some(e) = node.named_child(0) {
                        self.expr(e, Ctx::plain(y));
                    }
                }
            }
            "break_statement" => {
                self.add_at(NodeKind::Break, node, parent);
            }
            "continue_statement" => {
                self.add_at(NodeKind::Continue, node, parent);
            }
            "if_statement" => self.if_chain(node, parent),
            "switch_statement" => self.switch(node, parent),
            "while_statement" | "do_statement" => {
                let l = self.add_at(NodeKind::Loop, node, parent);
                let cond = node.child_by_field_name("condition");
                let body = node.child_by_field_name("body");
                let is_do = node.kind() == "do_statement";
                if is_do {
                    if let Some(b) = body {
                        self.block_into(b, l);
                    }
                }
                if let Some(c) = cond {
                    self.expr(c, Ctx::plain(l));
                    let condition = self.condition(c);
                    self.nodes[l as usize].condition = Some(condition);
                }
                if let Some(b) = body {
                    if !is_do {
                        self.block_into(b, l);
                    }
                    self.loop_tags(l, b, false, &[]);
                }
            }
            "for_statement" => {
                let l = self.add_at(NodeKind::Loop, node, parent);
                self.scopes.push(Vec::new());
                let mut declared = Vec::new();
                for (field, child) in fielded_children(node) {
                    if !child.is_named() {
                        continue;
                    }
                    match field {
                        Some("initializer") if child.kind() == "variable_declaration" => {
                            for (n, _) in declarators(child) {
                                declared.push(self.text(n).to_string());
                            }
                            self.variables(child, l);
                        }
                        Some("initializer") | Some("update") => self.expr(child, Ctx::plain(l)),
                        Some("condition") => {
                            self.expr(child, Ctx::plain(l));
                            let condition = self.condition(child);
                            self.nodes[l as usize].condition = Some(condition);
                        }
                        _ => {}
                    }
                }
                if let Some(b) = node.child_by_field_name("body") {
                    self.block_into(b, l);
                    self.loop_tags(l, b, false, &declared);
                }
                self.scopes.pop();
            }
            "foreach_statement" => {
                let l = self.add_at(NodeKind::Loop, node, parent);
                if let Some(r) = node.child_by_field_name("right") {
                    self.expr(r, Ctx::plain(l));
                }
                self.scopes.push(Vec::new());
                let mut declared = Vec::new();
                if let Some(left) = node.child_by_field_name("left") {
                    for n in identifiers(left) {
                        let name = self.text(n);
                        declared.push(name.to_string());
                        self.declare(name);
                    }
                    if left.kind() == "identifier" {
                        if let Some(t) = node.child_by_field_name("type") {
                            self.typed(left, t, DeclarationKind::Local, Some(l));
                        }
                    }
                }
                if let Some(b) = node.child_by_field_name("body") {
                    self.block_into(b, l);
                    self.loop_tags(l, b, true, &declared);
                }
                self.scopes.pop();
            }
            "try_statement" => {
                let t = self.add_at(NodeKind::Try, node, parent);
                if let Some(b) = node.child_by_field_name("body") {
                    self.block_into(b, t);
                }
                for clause in named_children(node) {
                    match clause.kind() {
                        "catch_clause" => self.catch(clause, t),
                        "finally_clause" => {
                            let f = self.add_at(NodeKind::Finally, clause, t);
                            if let Some(b) = first_of_kind(clause, "block") {
                                self.block_into(b, f);
                            }
                        }
                        _ => {}
                    }
                }
            }
            "using_statement" | "fixed_statement" | "lock_statement" => {
                let word = match node.kind() {
                    "using_statement" => "using",
                    "fixed_statement" => "fixed",
                    _ => "lock",
                };
                let o = self.add_at(NodeKind::Other(word.into()), node, parent);
                self.scopes.push(Vec::new());
                for child in named_children(node) {
                    match child.kind() {
                        "variable_declaration" => self.variables(child, o),
                        k if is_statement_kind(k) => self.block_into(child, o),
                        _ => self.expr(child, Ctx::plain(o)),
                    }
                }
                self.scopes.pop();
            }
            "checked_statement" | "unsafe_statement" => {
                let word = node.kind().trim_end_matches("_statement").to_string();
                let o = self.add_at(NodeKind::Other(word), node, parent);
                if let Some(b) = first_of_kind(node, "block") {
                    self.block_into(b, o);
                }
            }
            "local_function_statement" => self.local_function(node),
            "empty_statement" => {}
            "goto_statement" => {
                self.add_at(NodeKind::Other("goto".into()), node, parent);
            }
            // A label or a preprocessor region is not a node: what it holds
            // is. Every branch of a `#if` is held — the `#elif` and `#else`
            // in its `alternative` too — so the model carries every
            // configuration and no branch reads as the only path.
            kind if kind == "labeled_statement" || kind.starts_with("preproc") => {
                for child in named_children(node) {
                    if is_statement_kind(child.kind())
                        || matches!(child.kind(), "preproc_elif" | "preproc_else")
                    {
                        self.stmt(child, parent);
                    }
                }
            }
            kind => {
                let o = self.add_at(NodeKind::Other(kind.to_string()), node, parent);
                for child in named_children(node) {
                    if is_statement_kind(child.kind()) {
                        self.stmt(child, o);
                    } else {
                        self.expr(child, Ctx::plain(o));
                    }
                }
            }
        }
        self.depth -= 1;
    }

    fn return_ctx(&self, node: u32) -> Ctx {
        if self.lambda_depth > 0 {
            return Ctx::plain(node);
        }
        Ctx {
            flow: Some(FlowTarget::MemberReturn),
            lit: LiteralContext::Returned,
            mode: AccessMode::Return,
            ..Ctx::plain(node)
        }
    }

    /// The value `e` the `return` node `r` returns: a constant is its
    /// `returns`, anything else its `condition`, so a predicate can be read
    /// as a tree. After [`Self::expr`]: the tree's literals are the walk's.
    fn returned(&mut self, r: u32, e: Node<'t>) {
        match constant_of(e, self.src) {
            Some(c) => self.nodes[r as usize].returns = Some(c),
            None => {
                let condition = self.condition(e);
                self.nodes[r as usize].condition = Some(condition);
            }
        }
    }

    /// `=> e` as the body under `root`: the `return` (or, for a member that
    /// returns nothing, the statement) `{ return e; }` would hold.
    fn expression_body(&mut self, e: Node<'t>, root: u32, returns_value: bool) {
        if returns_value {
            let r = self.add_at(NodeKind::Return, e, root);
            let ctx = self.return_ctx(r);
            self.expr(e, ctx);
            self.returned(r, e);
        } else {
            let s = self.add_at(NodeKind::Statement, e, root);
            self.expr(e, Ctx::plain(s));
        }
    }

    /// `T a = x, b;` — declared in the current frame, each initializer a
    /// write of the local.
    fn variables(&mut self, decl: Node<'t>, node: u32) {
        let ty = decl.child_by_field_name("type");
        for (name, init) in declarators(decl) {
            let name_text = self.text(name).to_string();
            self.declare(&name_text);
            match ty {
                Some(t) if t.kind() != "implicit_type" => {
                    self.typed(name, t, DeclarationKind::Local, Some(node));
                }
                _ => {
                    // `var x = new T(…)` states `T`, as the usage pass reads it.
                    if let Some(t) = init
                        .filter(|e| e.kind() == "object_creation_expression")
                        .and_then(|e| e.child_by_field_name("type"))
                    {
                        self.typed(name, t, DeclarationKind::Local, Some(node));
                    }
                }
            }
            if let Some(e) = init {
                let target = AccessPath {
                    root: PathRoot::Local,
                    segments: vec![name_text.clone()],
                };
                let place = self.range(name);
                self.write_from(target, place, e, node, Some(FlowTarget::Local(name_text)));
            }
        }
    }

    /// A write of `target` from `value`, walked at once.
    fn write_from(
        &mut self,
        target: AccessPath,
        place: Range,
        value: Node<'t>,
        node: u32,
        flow: Option<FlowTarget>,
    ) {
        let access = self.accesses.len();
        self.accesses.push(MemberAccess {
            target: target.clone(),
            mode: AccessMode::Write,
            place,
            node: Some(node),
            rhs: None,
        });
        self.expr(
            value,
            Ctx {
                flow,
                lit: LiteralContext::Assigned(target),
                ..Ctx::plain(node)
            },
        );
        self.accesses[access].rhs = Some(self.value_ref(value));
    }

    fn if_chain(&mut self, node: Node<'t>, parent: u32) {
        let choice = self.add_at(NodeKind::Choice, node, parent);
        let mut current = node;
        loop {
            let consequence = current.child_by_field_name("consequence");
            let place = span(current, consequence.unwrap_or(current), self.src);
            let arm = self.add(NodeKind::Arm, place, Some(choice));
            if let Some(c) = current.child_by_field_name("condition") {
                self.expr(c, Ctx::plain(arm));
                let condition = self.condition(c);
                for t in key_tags("key-compare", &condition.enum_refs) {
                    self.tag_node(arm, t);
                }
                self.nodes[arm as usize].condition = Some(condition);
            }
            if let Some(b) = consequence {
                self.block_into(b, arm);
            }
            self.guard_tag(arm);
            match current.child_by_field_name("alternative") {
                Some(alt) if alt.kind() == "if_statement" => current = alt,
                Some(alt) => {
                    let mut cursor = current.walk();
                    let else_token = current
                        .children(&mut cursor)
                        .find(|c| !c.is_named() && c.kind() == "else");
                    let place = span(else_token.unwrap_or(alt), alt, self.src);
                    let arm = self.add(NodeKind::Arm, place, Some(choice));
                    self.block_into(alt, arm);
                    break;
                }
                None => break,
            }
        }
    }

    fn guard_tag(&mut self, arm: u32) {
        if self.nodes[arm as usize].condition.is_none() {
            return;
        }
        let mut children = self.nodes[arm as usize + 1..]
            .iter()
            .filter(|n| n.parent == Some(arm));
        let first = children.next().map(|n| n.kind.clone());
        let guard = children.next().is_none()
            && matches!(
                first,
                Some(NodeKind::Return | NodeKind::Throw | NodeKind::YieldBreak)
            );
        if guard {
            self.tag_node(arm, tag("guard-return", None, Vec::new()));
        }
    }

    fn switch(&mut self, node: Node<'t>, parent: u32) {
        let choice = self.add_at(NodeKind::Choice, node, parent);
        if let Some(subject) = node.child_by_field_name("value") {
            self.expr(subject, Ctx::plain(choice));
            let condition = self.condition(subject);
            self.nodes[choice as usize].condition = Some(condition);
        }
        let Some(body) = node.child_by_field_name("body") else {
            return;
        };
        // One declaration space for the whole switch block.
        self.scopes.push(Vec::new());
        for section in named_children(body) {
            if section.kind() != "switch_section" {
                continue;
            }
            // Each `case` is a section of its own: `case A: case B: …` is an
            // empty arm A falling into arm B.
            let arm = self.add_at(NodeKind::Arm, section, choice);
            let mut label = None;
            let mut when = None;
            let mut statements = Vec::new();
            let mut after_colon = false;
            let mut cursor = section.walk();
            for child in section.children(&mut cursor) {
                if after_colon {
                    if child.is_named() {
                        statements.push(child);
                    }
                    continue;
                }
                match child.kind() {
                    ":" => after_colon = true,
                    "when_clause" => when = child.named_child(0),
                    _ if child.is_named() => label = Some(child),
                    _ => {}
                }
            }
            if let Some(l) = label {
                self.pattern(l, arm);
                if let Some(w) = when {
                    self.expr(w, Ctx::plain(arm));
                }
                let mut roots = vec![l];
                roots.extend(when);
                let written: Vec<&str> = roots.iter().map(|r| self.text(*r)).collect();
                let condition = self.condition_of(collapse(&written.join(" when ")), &roots);
                for t in key_tags("key-compare", &condition.enum_refs) {
                    self.tag_node(arm, t);
                }
                self.nodes[arm as usize].condition = Some(condition);
            }
            for s in statements {
                self.stmt(s, arm);
            }
            self.guard_tag(arm);
        }
        self.scopes.pop();
    }

    fn catch(&mut self, clause: Node<'t>, parent: u32) {
        let c = self.add_at(NodeKind::Catch, clause, parent);
        self.scopes.push(Vec::new());
        let mut body = None;
        for (field, child) in fielded_children(clause) {
            match (field, child.kind()) {
                (Some("body"), _) => body = Some(child),
                (_, "catch_declaration") => {
                    if let Some(n) = child.child_by_field_name("name") {
                        let name = self.text(n);
                        self.declare(name);
                        if let Some(t) = child.child_by_field_name("type") {
                            self.typed(n, t, DeclarationKind::Local, Some(c));
                        }
                    }
                }
                (_, "catch_filter_clause") => {
                    if let Some(e) = child.named_child(0) {
                        self.expr(e, Ctx::plain(c));
                        let condition = self.condition(e);
                        self.nodes[c as usize].condition = Some(condition);
                    }
                }
                _ => {}
            }
        }
        if let Some(b) = body {
            self.block_into(b, c);
            if swallows(b, self) {
                self.tag_node(c, tag("swallow-exception", None, Vec::new()));
            }
        }
        self.scopes.pop();
    }

    fn local_function(&mut self, node: Node<'t>) {
        let Some(body) = node.child_by_field_name("body") else {
            return;
        };
        let mut outer: Vec<String> = Vec::new();
        for frame in &self.scopes {
            for (n, _) in frame {
                if !outer.contains(n) {
                    outer.push(n.clone());
                }
            }
        }
        let returns_value = node
            .child_by_field_name("type")
            .is_some_and(|t| self.text(t) != "void");
        let root = if body.kind() == "block" {
            Root::Block(body)
        } else {
            Root::Arrow(body, returns_value)
        };
        let mut b = Builder::new(
            self.src,
            self.ty,
            BodyKind::LocalFunction,
            self.name_position,
            node.child_by_field_name("parameters"),
            false,
            outer,
        );
        b.depth = self.depth;
        let built = b.build(root);
        self.locals.extend(built);
    }

    fn constructor_initializer(&mut self, init: Node<'t>, root: u32) {
        let base = has_token(init, "base");
        let callee = AccessPath {
            root: if base { PathRoot::Base } else { PathRoot::This },
            segments: Vec::new(),
        };
        let id = self.invocations.len() as u32;
        let mut stack = Vec::new();
        let mut fixups = Vec::new();
        let (arity, args) = match first_of_kind(init, "argument_list") {
            Some(list) => self.arguments(list, id, &callee, None, root, &mut stack, &mut fixups),
            None => (0, Vec::new()),
        };
        self.invocations.push(Invocation {
            callee,
            kind: if base {
                CallKind::BaseInit
            } else {
                CallKind::ThisInit
            },
            arity,
            args,
            conditional: false,
            place: self.range(init),
            node: Some(root),
            flow: None,
        });
        self.run(stack, fixups);
    }

    /// A loop's idioms, read off its body's syntax.
    fn loop_tags(&mut self, l: u32, body: Node, is_foreach: bool, declared: &[String]) {
        let mut declared: BTreeSet<String> = declared.iter().cloned().collect();
        let (mut exits, mut finds, mut accumulates) = (false, false, false);
        // (node, inside a nested loop or switch, inside an `if`)
        let mut stack = vec![(body, false, false)];
        while let Some((n, nested, in_if)) = stack.pop() {
            let kind = n.kind();
            match kind {
                k if is_lambda(k) || k == "local_function_statement" => continue,
                "return_statement" => {
                    exits = true;
                    finds |= in_if;
                }
                "throw_statement" | "goto_statement" => exits = true,
                "yield_statement" if has_token(n, "break") => exits = true,
                "break_statement" if !nested => {
                    exits = true;
                    finds |= in_if;
                }
                "variable_declarator" | "declaration_pattern" | "declaration_expression" => {
                    if let Some(name) = n.child_by_field_name("name") {
                        declared.insert(self.text(name).to_string());
                    }
                }
                "assignment_expression" => {
                    let op = n
                        .child_by_field_name("operator")
                        .map(|o| self.text(o))
                        .unwrap_or("=");
                    if let (Some(left), Some(right)) = (
                        n.child_by_field_name("left"),
                        n.child_by_field_name("right"),
                    ) {
                        let right = unparen(right);
                        let outside = leftmost_name(left, self.src)
                            .is_some_and(|name| !declared.contains(name));
                        let compound = op != "=" && op != "??=" && !is_lambda(right.kind());
                        let rebuilt = op == "="
                            && right.kind() == "binary_expression"
                            && leftmost_operand(right).is_some_and(|first| {
                                collapse(self.text(first)) == collapse(self.text(left))
                            });
                        accumulates |= outside && (compound || rebuilt);
                    }
                }
                "prefix_unary_expression" | "postfix_unary_expression"
                    if has_token(n, "++") || has_token(n, "--") =>
                {
                    if let Some(operand) = n.named_child(0) {
                        accumulates |= leftmost_name(operand, self.src)
                            .is_some_and(|name| !declared.contains(name));
                    }
                }
                "invocation_expression" => {
                    if let Some((callee, _)) =
                        n.child_by_field_name("function").and_then(|f| self.path(f))
                    {
                        let s = &callee.segments;
                        accumulates |= s.len() >= 2
                            && ACCUMULATORS.contains(&s[s.len() - 1].as_str())
                            && (callee.root != PathRoot::Local || !declared.contains(&s[0]));
                    }
                }
                _ => {}
            }
            let nested = nested
                || matches!(
                    kind,
                    "while_statement"
                        | "do_statement"
                        | "for_statement"
                        | "foreach_statement"
                        | "switch_statement"
                );
            let in_if = in_if || kind == "if_statement";
            for child in named_children(n).into_iter().rev() {
                stack.push((child, nested, in_if));
            }
        }
        if is_foreach && !exits {
            self.tag_node(l, tag("full-traversal", None, Vec::new()));
        }
        if finds {
            self.tag_node(l, tag("find-first", None, Vec::new()));
        }
        if accumulates {
            self.tag_node(l, tag("accumulate", None, Vec::new()));
        }
    }

    // ---- patterns ----

    /// The names a pattern declares, and the values it reads.
    fn pattern(&mut self, pattern: Node<'t>, node: u32) {
        let mut stack = vec![pattern];
        while let Some(p) = stack.pop() {
            match p.kind() {
                "constant_pattern" | "relational_pattern" => {
                    if let Some(e) = p.named_child(0) {
                        self.expr(e, Ctx::plain(node));
                    }
                }
                "declaration_pattern" | "recursive_pattern" | "var_pattern" => {
                    if let Some(n) = p.child_by_field_name("name") {
                        let name = self.text(n);
                        self.declare(name);
                        if let Some(t) = p.child_by_field_name("type") {
                            self.typed(n, t, DeclarationKind::Pattern, Some(node));
                        }
                    }
                    for child in named_children(p).into_iter().rev() {
                        if matches!(
                            child.kind(),
                            "property_pattern_clause" | "positional_pattern_clause"
                        ) {
                            stack.push(child);
                        }
                    }
                }
                "property_pattern_clause" | "positional_pattern_clause" => {
                    for child in named_children(p).into_iter().rev() {
                        stack.push(child);
                    }
                }
                // `Name: pattern` — the name is a member of the subject.
                "subpattern" => {
                    for child in named_children(p).into_iter().rev() {
                        if child.kind().ends_with("pattern") {
                            stack.push(child);
                        }
                    }
                }
                "type_pattern" | "discard" => {}
                k if k.ends_with("pattern") => {
                    for child in named_children(p).into_iter().rev() {
                        stack.push(child);
                    }
                }
                // A `case` label written as a plain expression.
                _ => self.expr(p, Ctx::plain(node)),
            }
        }
    }

    // ---- expressions ----

    fn expr(&mut self, root: Node<'t>, ctx: Ctx) {
        self.run(vec![(root, ctx)], Vec::new());
    }

    fn run(&mut self, mut stack: Work<'t>, mut fixups: Vec<Fixup<'t>>) {
        while let Some((n, c)) = stack.pop() {
            self.visit(n, c, &mut stack, &mut fixups);
        }
        for f in fixups {
            match f {
                Fixup::Argument {
                    invocation,
                    argument,
                    expr,
                } => {
                    let value = self.value_ref(expr);
                    self.invocations[invocation].args[argument].value = value;
                }
                Fixup::Write { access, expr } => {
                    let value = self.value_ref(expr);
                    self.accesses[access].rhs = Some(value);
                }
            }
        }
    }

    fn read(&mut self, node: Node, path: AccessPath, c: &Ctx) {
        if !c.excluded && is_enum_candidate(&path) {
            return;
        }
        self.accesses.push(MemberAccess {
            target: path,
            mode: c.mode,
            place: self.range(node),
            node: Some(c.node),
            rhs: None,
        });
    }

    fn visit(&mut self, n: Node<'t>, c: Ctx, stack: &mut Work<'t>, fixups: &mut Vec<Fixup<'t>>) {
        let plain = Ctx::plain(c.node);
        match n.kind() {
            "parenthesized_expression" => {
                if let Some(inner) = n.named_child(0) {
                    stack.push((inner, c));
                }
            }
            "identifier" | "generic_name" | "this" | "qualified_name" | "predefined_type" => {
                if let Some((p, _)) = self.path(n) {
                    self.read(n, p, &c);
                }
            }
            "member_access_expression" | "conditional_access_expression" => {
                if let Some((p, _)) = self.path(n) {
                    self.read(n, p, &c);
                } else if n.kind() == "member_access_expression" {
                    if let Some(e) = n.child_by_field_name("expression") {
                        stack.push((e, plain));
                    }
                } else {
                    for child in named_children(n).into_iter().rev() {
                        if child.kind() != "member_binding_expression" {
                            stack.push((child, plain.clone()));
                        }
                    }
                }
            }
            "element_binding_expression" => {
                for a in named_children(n).into_iter().rev() {
                    if let Some(e) = argument_expression(a) {
                        stack.push((e, plain.clone()));
                    }
                }
            }
            k if is_string_literal(k) => {
                let id = self.literals.len() as u32;
                self.literal_of.insert(n.id(), id);
                self.literals.push(StringLiteral {
                    id,
                    value: literal_value(n, self.src),
                    context: c.lit,
                    place: self.range(n),
                    node: Some(c.node),
                });
            }
            "interpolated_string_expression" => {
                for part in named_children(n).into_iter().rev() {
                    if part.kind() != "interpolation" {
                        continue;
                    }
                    for e in named_children(part).into_iter().rev() {
                        if !e.kind().starts_with("interpolation_") {
                            stack.push((e, plain.clone()));
                        }
                    }
                }
            }
            "invocation_expression" => self.invocation(n, c, stack, fixups),
            "object_creation_expression" | "implicit_object_creation_expression" => {
                self.construction(n, c, stack, fixups)
            }
            "assignment_expression" => self.assignment(n, c, stack, fixups),
            k if is_lambda(k) => {
                self.lambda(n, &c);
            }
            "binary_expression" => {
                let (Some(left), Some(right)) = (
                    n.child_by_field_name("left"),
                    n.child_by_field_name("right"),
                ) else {
                    return;
                };
                let op = n
                    .child_by_field_name("operator")
                    .map(|o| self.text(o))
                    .unwrap_or("");
                let mut lc = plain.clone();
                let mut rc = plain;
                lc.excluded = self.enum_candidate_node(right);
                if op == "==" || op == "!=" {
                    if let Some((p, _)) = self.path(unparen(right)) {
                        lc.lit = LiteralContext::Compared(p);
                    }
                    if let Some((p, _)) = self.path(unparen(left)) {
                        rc.lit = LiteralContext::Compared(p);
                    }
                }
                stack.push((right, rc));
                stack.push((left, lc));
            }
            "conditional_expression" => {
                let branch = Ctx {
                    arg_of: None,
                    construction_flow: None,
                    excluded: false,
                    ..c
                };
                if let Some(a) = n.child_by_field_name("alternative") {
                    stack.push((a, branch.clone()));
                }
                if let Some(b) = n.child_by_field_name("consequence") {
                    stack.push((b, branch));
                }
                if let Some(cond) = n.child_by_field_name("condition") {
                    stack.push((cond, plain));
                }
            }
            "cast_expression" => {
                if let Some(v) = n.child_by_field_name("value") {
                    stack.push((v, c));
                }
            }
            "await_expression" => {
                if let Some(v) = n.named_child(0) {
                    stack.push((v, c));
                }
            }
            "as_expression" | "is_expression" => {
                if let Some(l) = n.child_by_field_name("left") {
                    stack.push((l, plain));
                }
            }
            "is_pattern_expression" => {
                // The pattern after the subject: its names are declared in
                // the frame the expression is in.
                if let Some(p) = n.child_by_field_name("pattern") {
                    stack.push((p, plain.clone()));
                }
                if let Some(e) = n.child_by_field_name("expression") {
                    stack.push((e, plain));
                }
            }
            k if k.ends_with("pattern") && k != "tuple_pattern" => self.pattern(n, c.node),
            "declaration_expression" => {
                if let Some(name) = n.child_by_field_name("name") {
                    let name_text = self.text(name);
                    self.declare(name_text);
                    if let Some(t) = n.child_by_field_name("type") {
                        self.typed(name, t, DeclarationKind::Local, Some(c.node));
                    }
                }
            }
            "prefix_unary_expression" | "postfix_unary_expression" => {
                let Some(operand) = n.named_child(0) else {
                    return;
                };
                if has_token(n, "++") || has_token(n, "--") {
                    if let Some((p, _)) = self.path(unparen(operand)) {
                        self.accesses.push(MemberAccess {
                            target: p,
                            mode: AccessMode::Write,
                            place: self.range(operand),
                            node: Some(c.node),
                            rhs: Some(ValueRef::Other),
                        });
                        return;
                    }
                }
                stack.push((operand, plain));
            }
            "element_access_expression" => {
                if let Some(s) = n.child_by_field_name("subscript") {
                    for a in named_children(s).into_iter().rev() {
                        if let Some(e) = argument_expression(a) {
                            stack.push((e, plain.clone()));
                        }
                    }
                }
                if let Some(e) = n.child_by_field_name("expression") {
                    stack.push((e, plain));
                }
            }
            "switch_expression" => {
                let branch = Ctx {
                    arg_of: None,
                    construction_flow: None,
                    excluded: false,
                    ..c
                };
                let children = named_children(n);
                for arm in children.iter().skip(1).rev() {
                    if arm.kind() != "switch_expression_arm" {
                        continue;
                    }
                    // The pattern, its `when`, then the value; pushed in
                    // reverse.
                    let parts = named_children(*arm);
                    let mut items: Work<'t> = Vec::new();
                    for (i, part) in parts.iter().enumerate() {
                        match part.kind() {
                            "when_clause" => {
                                if let Some(e) = part.named_child(0) {
                                    items.push((e, plain.clone()));
                                }
                            }
                            _ if i + 1 == parts.len() => items.push((*part, branch.clone())),
                            _ => items.push((*part, plain.clone())),
                        }
                    }
                    stack.extend(items.into_iter().rev());
                }
                if let Some(subject) = children.first() {
                    stack.push((*subject, plain));
                }
            }
            "boolean_literal" | "null_literal" | "integer_literal" | "real_literal"
            | "character_literal" | "typeof_expression" | "sizeof_expression"
            | "default_expression" | "name_equals" | "discard" | "implicit_type" => {}
            _ => {
                let mut items = Vec::new();
                for (field, child) in fielded_children(n) {
                    if !child.is_named()
                        || matches!(field, Some("type" | "returns" | "name"))
                        || is_statement_kind(child.kind())
                    {
                        continue;
                    }
                    items.push((child, plain.clone()));
                }
                stack.extend(items.into_iter().rev());
            }
        }
    }

    /// The callee of a call: a path, whether it is conditional, and the
    /// receiver expression to walk when the path cannot spell it — then the
    /// path has root `expression` and the names written after the expression
    /// (`GetX().Foo.Run` is [`Foo`, `Run`]).
    fn callee(&self, f: Node<'t>) -> (AccessPath, bool, Option<Node<'t>>) {
        if let Some((p, conditional)) = self.path(f) {
            return (p, conditional, None);
        }
        let mut segments = Vec::new();
        let mut conditional = false;
        let mut at = Some(f);
        while let Some(n) = at {
            let (name, next) = match n.kind() {
                "member_access_expression" => (
                    n.child_by_field_name("name")
                        .and_then(|name| self.simple_name(name)),
                    n.child_by_field_name("expression"),
                ),
                "conditional_access_expression" => {
                    conditional = true;
                    (
                        first_of_kind(n, "member_binding_expression")
                            .and_then(|b| b.child_by_field_name("name"))
                            .and_then(|name| self.simple_name(name)),
                        n.child_by_field_name("condition"),
                    )
                }
                _ => break,
            };
            segments.extend(name);
            at = next;
        }
        segments.reverse();
        let callee = AccessPath {
            root: PathRoot::Expression,
            segments,
        };
        (callee, conditional, at)
    }

    fn invocation(
        &mut self,
        n: Node<'t>,
        c: Ctx,
        stack: &mut Work<'t>,
        fixups: &mut Vec<Fixup<'t>>,
    ) {
        let Some(f) = n.child_by_field_name("function") else {
            return;
        };
        let list = n.child_by_field_name("arguments");
        if f.kind() == "identifier" && self.text(f) == "nameof" && !self.is_local("nameof") {
            let spelled = list
                .and_then(|l| named_children(l).into_iter().next())
                .map(|a| collapse(self.text(a)))
                .unwrap_or_default();
            self.name_ofs.push(NameOf {
                text: spelled,
                context: c.lit,
                place: self.range(n),
                node: Some(c.node),
            });
            return;
        }
        let (callee, conditional, receiver) = self.callee(f);
        if callee.root != PathRoot::Expression {
            self.call_access(&callee, f, c.node);
        }
        let id = self.invocations.len() as u32;
        self.invocation_of.insert(n.id(), id);
        let mut items = Vec::new();
        let (arity, args) = match list {
            Some(l) => self.arguments(l, id, &callee, None, c.node, &mut items, fixups),
            None => (0, Vec::new()),
        };
        self.invocations.push(Invocation {
            callee,
            kind: CallKind::Call,
            arity,
            args,
            conditional,
            place: self.range(n),
            node: Some(c.node),
            flow: c.flow,
        });
        // The receiver is written first: walked first.
        stack.extend(items);
        if let Some(r) = receiver {
            stack.push((r, Ctx::plain(c.node)));
        }
    }

    /// `x.m()` invokes `x.m`; `x.Invoke()` invokes the delegate `x`;
    /// `items.Add(y)` mutates `items`.
    fn call_access(&mut self, callee: &AccessPath, f: Node, node: u32) {
        let s = &callee.segments;
        let receiver = || AccessPath {
            root: callee.root,
            segments: s[..s.len() - 1].to_vec(),
        };
        let (target, mode) = match s.last().map(String::as_str) {
            Some(last) if s.len() >= 2 && MUTATORS.contains(&last) => {
                (receiver(), AccessMode::Mutate)
            }
            Some("Invoke") if s.len() >= 2 => (receiver(), AccessMode::Invoke),
            _ => (callee.clone(), AccessMode::Invoke),
        };
        self.accesses.push(MemberAccess {
            target,
            mode,
            place: self.range(f),
            node: Some(node),
            rhs: None,
        });
    }

    /// The arguments of a call or construction. Their values are walked from
    /// `items` (pushed so that they pop in source order) and their references
    /// filled in by `fixups`. `construction_flow` is `Some` for a
    /// construction: where it flows.
    #[allow(clippy::too_many_arguments)]
    fn arguments(
        &mut self,
        list: Node<'t>,
        invocation: u32,
        callee: &AccessPath,
        construction_flow: Option<Option<FlowTarget>>,
        node: u32,
        items: &mut Work<'t>,
        fixups: &mut Vec<Fixup<'t>>,
    ) -> (u32, Vec<Argument>) {
        let mut out = Vec::new();
        let mut walk = Vec::new();
        let arguments: Vec<Node<'t>> = named_children(list)
            .into_iter()
            .filter(|a| a.kind() == "argument")
            .collect();
        for (position, a) in arguments.iter().enumerate() {
            let position = position as u32;
            let name = a
                .child_by_field_name("name")
                .map(|n| self.text(n).to_string());
            let Some(expr) = argument_expression(*a) else {
                continue;
            };
            let (enum_refs, reads) = self.scan(expr);
            fixups.push(Fixup::Argument {
                invocation: invocation as usize,
                argument: out.len(),
                expr,
            });
            out.push(Argument {
                position,
                name,
                value: ValueRef::Other,
                enum_refs,
                reads,
            });
            if has_token(*a, "out") || has_token(*a, "ref") {
                if let Some((p, _)) = self.path(unparen(expr)) {
                    self.accesses.push(MemberAccess {
                        target: p,
                        mode: AccessMode::Write,
                        place: self.range(expr),
                        node: Some(node),
                        rhs: None,
                    });
                    continue;
                }
            }
            walk.push((
                expr,
                Ctx {
                    node,
                    flow: Some(FlowTarget::Argument((invocation, position))),
                    lit: LiteralContext::CallArg((callee.clone(), position)),
                    mode: AccessMode::Read,
                    arg_of: Some((invocation, position)),
                    construction_flow: construction_flow.clone().flatten(),
                    excluded: false,
                },
            ));
        }
        items.extend(walk.into_iter().rev());
        (arguments.len() as u32, out)
    }

    fn construction(
        &mut self,
        n: Node<'t>,
        c: Ctx,
        stack: &mut Work<'t>,
        fixups: &mut Vec<Fixup<'t>>,
    ) {
        // `new()`: no type written, nothing to name.
        let callee = n
            .child_by_field_name("type")
            .and_then(|t| type_segments(t, self.src))
            .map_or_else(
                || AccessPath {
                    root: PathRoot::Expression,
                    segments: Vec::new(),
                },
                |(segments, _)| AccessPath {
                    root: PathRoot::Name,
                    segments,
                },
            );
        let id = self.invocations.len() as u32;
        self.invocation_of.insert(n.id(), id);
        let list = n
            .child_by_field_name("arguments")
            .or_else(|| first_of_kind(n, "argument_list"));
        let initializer = n
            .child_by_field_name("initializer")
            .or_else(|| first_of_kind(n, "initializer_expression"));
        let mut items = Vec::new();
        let (arity, args) = match list {
            Some(l) => {
                let flow = Some(c.flow.clone());
                self.arguments(l, id, &callee, flow, c.node, &mut items, fixups)
            }
            None => (0, Vec::new()),
        };
        let type_text = callee.segments.join(".");
        let last = callee.segments.last().cloned().unwrap_or_default();

        // `new T { A = x, … }`: the members it is given, written through the
        // variable it is constructed into when there is one.
        let variable = match &c.flow {
            Some(FlowTarget::Local(v)) => Some((PathRoot::Local, v.clone())),
            Some(FlowTarget::Member(v)) => Some((PathRoot::Name, v.clone())),
            _ => None,
        };
        let mut members = BTreeSet::new();
        let mut init_items: Work<'t> = Vec::new();
        for child in initializer.map(named_children).unwrap_or_default() {
            let member_init = (child.kind() == "assignment_expression")
                .then(|| {
                    let left = child.child_by_field_name("left")?;
                    let right = child.child_by_field_name("right")?;
                    (left.kind() == "identifier").then_some((left, right))
                })
                .flatten();
            let Some((left, right)) = member_init else {
                init_items.push((child, Ctx::plain(c.node)));
                continue;
            };
            let member = self.text(left).to_string();
            members.insert(member.clone());
            let mut ctx = Ctx::plain(c.node);
            if let Some((root, v)) = &variable {
                let target = AccessPath {
                    root: *root,
                    segments: vec![v.clone(), member],
                };
                fixups.push(Fixup::Write {
                    access: self.accesses.len(),
                    expr: right,
                });
                self.accesses.push(MemberAccess {
                    target: target.clone(),
                    mode: AccessMode::Write,
                    place: self.range(left),
                    node: Some(c.node),
                    rhs: None,
                });
                ctx.flow = Some(FlowTarget::Path(target.clone()));
                ctx.lit = LiteralContext::Assigned(target);
            }
            init_items.push((right, ctx));
        }

        if last == "KeyGesture" || last == "KeyBinding" {
            let mut refs: Vec<String> = args.iter().flat_map(|a| a.enum_refs.clone()).collect();
            if let Some(init) = initializer {
                refs.extend(self.scan(init).0);
            }
            if let Some(t) = key_tags("key-gesture", &refs).into_iter().next() {
                self.tag_node(c.node, t);
            }
        }
        if let Some((_, v)) = variable {
            self.built.push(Built {
                variable: v,
                node: c.node,
                type_text,
                members,
            });
        }
        self.invocations.push(Invocation {
            callee,
            kind: CallKind::Construct,
            arity,
            args,
            conditional: false,
            place: self.range(n),
            node: Some(c.node),
            flow: c.flow,
        });
        stack.extend(init_items.into_iter().rev());
        stack.extend(items);
    }

    fn assignment(
        &mut self,
        n: Node<'t>,
        c: Ctx,
        stack: &mut Work<'t>,
        fixups: &mut Vec<Fixup<'t>>,
    ) {
        let (Some(left), Some(right)) = (
            n.child_by_field_name("left"),
            n.child_by_field_name("right"),
        ) else {
            return;
        };
        let op = n
            .child_by_field_name("operator")
            .map(|o| self.text(o))
            .unwrap_or("=");
        let Some((target, _)) = self.path(unparen(left)) else {
            stack.push((right, Ctx::plain(c.node)));
            stack.push((left, Ctx::plain(c.node)));
            return;
        };
        if (op == "+=" || op == "-=") && self.is_subscription(&target, right) {
            self.subscription(n, target, right, op == "+=", c.node, stack);
            return;
        }
        fixups.push(Fixup::Write {
            access: self.accesses.len(),
            expr: right,
        });
        self.accesses.push(MemberAccess {
            target: target.clone(),
            mode: AccessMode::Write,
            place: self.range(left),
            node: Some(c.node),
            rhs: None,
        });
        let ctx = Ctx {
            flow: Self::flow_to(&target),
            lit: LiteralContext::Assigned(target),
            ..Ctx::plain(c.node)
        };
        stack.push((right, ctx));
    }

    fn is_subscription(&self, target: &AccessPath, right: Node) -> bool {
        let r = unparen(right);
        match r.kind() {
            k if is_lambda(k) => true,
            "object_creation_expression" => r
                .child_by_field_name("type")
                .and_then(|t| type_segments(t, self.src))
                .is_some_and(|(s, _)| s.last().is_some_and(|l| l.ends_with("Handler"))),
            _ => {
                let Some((handler, _)) = self.path(r) else {
                    return false;
                };
                let (Some(event), Some(last)) = (target.segments.last(), handler.segments.last())
                else {
                    return false;
                };
                let own_data = matches!(
                    (handler.root, handler.segments.len()),
                    (PathRoot::This | PathRoot::Name, 1)
                ) && self.ty.data.contains(last);
                handler.root != PathRoot::Local
                    && starts_upper(last)
                    && starts_upper(event)
                    && !own_data
            }
        }
    }

    fn subscription(
        &mut self,
        n: Node<'t>,
        target: AccessPath,
        right: Node<'t>,
        add: bool,
        node: u32,
        stack: &mut Work<'t>,
    ) {
        let r = unparen(right);
        let handler = if is_lambda(r.kind()) {
            Handler::Lambda(self.lambda(r, &Ctx::plain(node)))
        } else if let Some((p, _)) = self.path(r) {
            Handler::MethodGroup(p)
        } else {
            // `new EventHandler(OnE)`: the method group it wraps.
            let group = r
                .child_by_field_name("arguments")
                .map(|l| {
                    named_children(l)
                        .into_iter()
                        .filter(|a| a.kind() == "argument")
                        .collect::<Vec<_>>()
                })
                .filter(|a| a.len() == 1)
                .and_then(|a| argument_expression(a[0]))
                .and_then(|e| self.path(unparen(e)));
            stack.push((right, Ctx::plain(node)));
            match group {
                Some((p, _)) => Handler::MethodGroup(p),
                None => Handler::Other,
            }
        };
        let segments = &target.segments;
        let source = (segments.len() > 1).then(|| AccessPath {
            root: target.root,
            segments: segments[..segments.len() - 1].to_vec(),
        });
        self.subscriptions.push(Subscription {
            source,
            event: segments.last().cloned().unwrap_or_default(),
            handler,
            mode: if add {
                SubscriptionMode::Add
            } else {
                SubscriptionMode::Remove
            },
            place: self.range(n),
            node: Some(node),
        });
    }

    fn lambda(&mut self, n: Node<'t>, c: &Ctx) -> u32 {
        let id = self.lambdas.len() as u32;
        self.lambda_of.insert(n.id(), id);
        let params = n
            .child_by_field_name("parameters")
            .or_else(|| first_of_kind(n, "parameter_list"));
        let parameters: Vec<Node<'t>> = match params {
            Some(p) if p.kind() == "implicit_parameter" => vec![p],
            Some(p) => named_children(p)
                .into_iter()
                .filter(|x| x.kind() == "parameter")
                .collect(),
            None => Vec::new(),
        };
        let body_node = self.add_at(NodeKind::Lambda, n, c.node);
        let flow = if c.arg_of.is_some() {
            c.construction_flow.clone()
        } else {
            c.flow.clone()
        };
        self.lambdas.push(Lambda {
            id,
            argument_of: c.arg_of,
            arity: parameters.len() as u32,
            place: self.range(n),
            flow,
            body: body_node,
        });
        if self.depth >= MAX_NESTING {
            return id;
        }
        self.depth += 1;
        self.lambda_depth += 1;
        self.scopes.push(Vec::new());
        for p in parameters {
            if p.kind() == "implicit_parameter" {
                let name = self.text(p);
                self.declare(name);
            } else if let Some(name) = p.child_by_field_name("name") {
                let name_text = self.text(name);
                self.declare(name_text);
                if let Some(t) = p.child_by_field_name("type") {
                    self.typed(name, t, DeclarationKind::Parameter, Some(body_node));
                }
            }
        }
        let body = n
            .child_by_field_name("body")
            .or_else(|| first_of_kind(n, "block"));
        match body {
            Some(b) if b.kind() == "block" => self.block_into(b, body_node),
            Some(e) => {
                let returns = !is_statement_expression(e);
                let kind = if returns {
                    NodeKind::Return
                } else {
                    NodeKind::Statement
                };
                let s = self.add_at(kind, e, body_node);
                self.expr(e, Ctx::plain(s));
                if returns {
                    self.returned(s, e);
                }
            }
            None => {}
        }
        self.scopes.pop();
        self.lambda_depth -= 1;
        self.depth -= 1;
        id
    }

    // ---- conditions ----

    fn condition(&mut self, expr: Node<'t>) -> Condition {
        let text = collapse(self.text(expr));
        let mut condition = self.condition_of(text, &[expr]);
        let e = unparen(expr);
        condition.constant = (e.kind() == "boolean_literal").then(|| self.text(e) == "true");
        condition
    }

    /// A condition over `roots`, `and`-ed when there are several (a `case`
    /// label and its `when`).
    fn condition_of(&mut self, text: String, roots: &[Node<'t>]) -> Condition {
        let mut expr = Vec::new();
        let mut tops = Vec::new();
        let mut enum_refs = Vec::new();
        let mut reads = Vec::new();
        for r in roots {
            tops.push(self.convert(*r, &mut expr));
            let (e, p) = self.scan(*r);
            enum_refs.extend(e);
            reads.extend(p);
        }
        if tops.len() > 1 {
            expr.push(Expr::And(tops));
        }
        Condition {
            text,
            expr,
            enum_refs,
            reads,
            constant: None,
        }
    }

    /// `root` as an expression tree appended to `out`, operands first; the
    /// index of its root.
    fn convert(&self, root: Node<'t>, out: &mut Vec<Expr>) -> u32 {
        enum Op {
            And,
            Or,
            Not,
            Compare(CompareOp),
            Binary(String),
            Is,
            IsNot,
            IsType(String),
            Call(AccessPath),
        }
        enum Item<'n> {
            Enter(Node<'n>, bool),
            Exit(Op, usize),
        }
        fn leaf(out: &mut Vec<Expr>, values: &mut Vec<u32>, e: Expr) {
            out.push(e);
            values.push(out.len() as u32 - 1);
        }
        let mut work = vec![Item::Enter(root, false)];
        let mut values: Vec<u32> = Vec::new();
        while let Some(item) = work.pop() {
            let (n, excluded) = match item {
                Item::Enter(n, excluded) => (unparen(n), excluded),
                Item::Exit(op, count) => {
                    let operands = values.split_off(values.len() - count);
                    let e = match op {
                        Op::And => Expr::And(operands),
                        Op::Or => Expr::Or(operands),
                        Op::Not => Expr::Not(operands[0]),
                        Op::Compare(c) => Expr::Compare((c, operands[0], operands[1])),
                        Op::Binary(op) => Expr::Binary((op, operands[0], operands[1])),
                        Op::Is => Expr::Binary(("is".into(), operands[0], operands[1])),
                        Op::IsNot => {
                            out.push(Expr::Binary(("is".into(), operands[0], operands[1])));
                            Expr::Not(out.len() as u32 - 1)
                        }
                        Op::IsType(t) => {
                            out.push(Expr::Other(t));
                            Expr::Binary(("is".into(), operands[0], out.len() as u32 - 1))
                        }
                        Op::Call(path) => Expr::Call((path, operands)),
                    };
                    leaf(out, &mut values, e);
                    continue;
                }
            };
            let other = || Expr::Other(collapse(self.text(n)));
            match n.kind() {
                "binary_expression" => {
                    let op = n
                        .child_by_field_name("operator")
                        .map(|o| self.text(o))
                        .unwrap_or("");
                    let (Some(l), Some(r)) = (
                        n.child_by_field_name("left"),
                        n.child_by_field_name("right"),
                    ) else {
                        leaf(out, &mut values, other());
                        continue;
                    };
                    if op == "&&" || op == "||" {
                        let operands = flatten(n, op, self.src);
                        let op = if op == "&&" { Op::And } else { Op::Or };
                        work.push(Item::Exit(op, operands.len()));
                        for o in operands.into_iter().rev() {
                            work.push(Item::Enter(o, false));
                        }
                        continue;
                    }
                    let op = match op {
                        "==" => Op::Compare(CompareOp::Eq),
                        "!=" => Op::Compare(CompareOp::Ne),
                        "<" => Op::Compare(CompareOp::Lt),
                        "<=" => Op::Compare(CompareOp::Le),
                        ">" => Op::Compare(CompareOp::Gt),
                        ">=" => Op::Compare(CompareOp::Ge),
                        _ => Op::Binary(op.to_string()),
                    };
                    work.push(Item::Exit(op, 2));
                    work.push(Item::Enter(r, false));
                    work.push(Item::Enter(l, self.enum_candidate_node(r)));
                }
                "prefix_unary_expression" if has_token(n, "!") => match n.named_child(0) {
                    Some(operand) => {
                        work.push(Item::Exit(Op::Not, 1));
                        work.push(Item::Enter(operand, false));
                    }
                    None => leaf(out, &mut values, other()),
                },
                "is_pattern_expression" => {
                    let (Some(e), Some(p)) = (
                        n.child_by_field_name("expression"),
                        n.child_by_field_name("pattern"),
                    ) else {
                        leaf(out, &mut values, other());
                        continue;
                    };
                    let p = unparen(p);
                    match (p.kind(), p.named_child(0)) {
                        ("negated_pattern", Some(inner)) => {
                            work.push(Item::Exit(Op::IsNot, 2));
                            work.push(Item::Enter(inner, false));
                        }
                        _ => {
                            work.push(Item::Exit(Op::Is, 2));
                            work.push(Item::Enter(p, false));
                        }
                    }
                    work.push(Item::Enter(e, false));
                }
                "is_expression" => match n.child_by_field_name("left") {
                    Some(l) => {
                        let right = n
                            .child_by_field_name("right")
                            .map(|r| collapse(self.text(r)))
                            .unwrap_or_default();
                        work.push(Item::Exit(Op::IsType(right), 1));
                        work.push(Item::Enter(l, false));
                    }
                    None => leaf(out, &mut values, other()),
                },
                "constant_pattern" => match n.named_child(0) {
                    Some(inner) => work.push(Item::Enter(inner, excluded)),
                    None => leaf(out, &mut values, other()),
                },
                "negated_pattern" => match n.named_child(0) {
                    Some(inner) => {
                        work.push(Item::Exit(Op::Not, 1));
                        work.push(Item::Enter(inner, false));
                    }
                    None => leaf(out, &mut values, other()),
                },
                "and_pattern" | "or_pattern" => {
                    let (Some(l), Some(r)) = (
                        n.child_by_field_name("left"),
                        n.child_by_field_name("right"),
                    ) else {
                        leaf(out, &mut values, other());
                        continue;
                    };
                    let op = if n.kind() == "and_pattern" {
                        Op::And
                    } else {
                        Op::Or
                    };
                    work.push(Item::Exit(op, 2));
                    work.push(Item::Enter(r, false));
                    work.push(Item::Enter(l, false));
                }
                "invocation_expression" => {
                    let callee = n
                        .child_by_field_name("function")
                        .filter(|f| !(f.kind() == "identifier" && self.text(*f) == "nameof"))
                        .and_then(|f| self.path(f));
                    let Some((path, _)) = callee else {
                        leaf(out, &mut values, other());
                        continue;
                    };
                    let args: Vec<Node<'t>> = n
                        .child_by_field_name("arguments")
                        .map(|l| {
                            named_children(l)
                                .into_iter()
                                .filter_map(argument_expression)
                                .collect()
                        })
                        .unwrap_or_default();
                    work.push(Item::Exit(Op::Call(path), args.len()));
                    for a in args.into_iter().rev() {
                        work.push(Item::Enter(a, false));
                    }
                }
                k if is_string_literal(k) => {
                    let e = match self.literal_of.get(&n.id()) {
                        Some(&id) => Expr::Literal(id),
                        None => other(),
                    };
                    leaf(out, &mut values, e);
                }
                k if is_path_kind(k) => {
                    let e = match self.path(n) {
                        Some((p, _)) if !excluded && is_enum_candidate(&p) => {
                            Expr::EnumMember(collapse(self.text(n)))
                        }
                        Some((p, _)) => Expr::Path(p),
                        None => other(),
                    };
                    leaf(out, &mut values, e);
                }
                _ => {
                    let e = constant_of(n, self.src).map_or_else(other, Expr::Constant);
                    leaf(out, &mut values, e);
                }
            }
        }
        values.pop().unwrap_or(0)
    }

    /// The enum members (as written) and the paths `root` reads, in source
    /// order. A call's callee is not read, a written path is not read, and a
    /// lambda's body is not part of the expression.
    fn scan(&self, root: Node<'t>) -> (Vec<String>, Vec<AccessPath>) {
        let mut enums = Vec::new();
        let mut reads = Vec::new();
        let mut stack = vec![(root, false)];
        while let Some((n, excluded)) = stack.pop() {
            match n.kind() {
                "parenthesized_expression" | "constant_pattern" => {
                    if let Some(inner) = n.named_child(0) {
                        stack.push((inner, excluded));
                    }
                }
                k if is_path_kind(k) => match self.path(n) {
                    Some((p, _)) if !excluded && is_enum_candidate(&p) => {
                        enums.push(collapse(self.text(n)));
                    }
                    Some((p, _)) => reads.push(p),
                    None => {
                        for (field, child) in fielded_children(n).into_iter().rev() {
                            if child.is_named()
                                && field != Some("name")
                                && child.kind() != "member_binding_expression"
                            {
                                stack.push((child, false));
                            }
                        }
                    }
                },
                k if is_lambda(k) => {}
                "invocation_expression" => {
                    let f = n.child_by_field_name("function");
                    if f.is_some_and(|f| f.kind() == "identifier" && self.text(f) == "nameof") {
                        continue;
                    }
                    if let Some(l) = n.child_by_field_name("arguments") {
                        for a in named_children(l).into_iter().rev() {
                            if let Some(e) = argument_expression(a) {
                                stack.push((e, false));
                            }
                        }
                    }
                    if let Some((_, _, Some(receiver))) = f.map(|f| self.callee(f)) {
                        stack.push((receiver, false));
                    }
                }
                "assignment_expression" => {
                    if let Some(r) = n.child_by_field_name("right") {
                        stack.push((r, false));
                    }
                    if let Some(l) = n.child_by_field_name("left") {
                        if self.path(unparen(l)).is_none() {
                            stack.push((l, false));
                        }
                    }
                }
                "binary_expression" => {
                    if let (Some(l), Some(r)) = (
                        n.child_by_field_name("left"),
                        n.child_by_field_name("right"),
                    ) {
                        stack.push((r, false));
                        stack.push((l, self.enum_candidate_node(r)));
                    }
                }
                "declaration_pattern"
                | "type_pattern"
                | "var_pattern"
                | "discard"
                | "declaration_expression"
                | "name_equals"
                | "typeof_expression"
                | "sizeof_expression"
                | "default_expression" => {}
                "subpattern" => {
                    for child in named_children(n).into_iter().rev() {
                        if child.kind().ends_with("pattern") {
                            stack.push((child, false));
                        }
                    }
                }
                _ => {
                    for (field, child) in fielded_children(n).into_iter().rev() {
                        if child.is_named() && !matches!(field, Some("type" | "returns" | "name")) {
                            stack.push((child, false));
                        }
                    }
                }
            }
        }
        (enums, reads)
    }
}

/// The value of an `argument`: its expression, without the name a named
/// argument gives and the `ref`/`out` keyword.
fn argument_expression(a: Node) -> Option<Node> {
    if a.kind() != "argument" {
        return None;
    }
    fielded_children(a)
        .into_iter()
        .find_map(|(field, c)| (field.is_none() && c.is_named()).then_some(c))
}

/// The operands of a chain of one operator (`a && b && (c && d)`), in source
/// order.
fn flatten<'t>(node: Node<'t>, op: &str, src: &[u8]) -> Vec<Node<'t>> {
    let mut out = Vec::new();
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        let n = unparen(n);
        let same = n.kind() == "binary_expression"
            && n.child_by_field_name("operator")
                .is_some_and(|o| text(o, src) == op);
        if same {
            if let (Some(l), Some(r)) = (
                n.child_by_field_name("left"),
                n.child_by_field_name("right"),
            ) {
                stack.push(r);
                stack.push(l);
                continue;
            }
        }
        out.push(n);
    }
    out
}

/// The first name a written path starts with (`items` of `items[i].Count`,
/// `f` of `this.f.Count`).
fn leftmost_name<'s>(node: Node, src: &'s [u8]) -> Option<&'s str> {
    let mut at = unparen(node);
    loop {
        at = match at.kind() {
            "member_access_expression" | "element_access_expression" => {
                let e = at.child_by_field_name("expression")?;
                if e.kind() == "this" {
                    return at.child_by_field_name("name").map(|n| text(n, src));
                }
                e
            }
            "conditional_access_expression" => at.child_by_field_name("condition")?,
            "identifier" => return Some(text(at, src)),
            _ => return None,
        };
    }
}

/// The leftmost operand of a binary chain (`x` of `x + a + b`).
fn leftmost_operand(node: Node) -> Option<Node> {
    let mut at = node;
    while at.kind() == "binary_expression" {
        at = unparen(at.child_by_field_name("left")?);
    }
    Some(at)
}

fn is_statement_expression(e: Node) -> bool {
    let e = unparen(e);
    match e.kind() {
        "invocation_expression"
        | "assignment_expression"
        | "await_expression"
        | "object_creation_expression"
        | "throw_expression" => true,
        "prefix_unary_expression" | "postfix_unary_expression" => {
            has_token(e, "++") || has_token(e, "--")
        }
        _ => false,
    }
}

/// Identifiers a `foreach` declares: `x`, or the names of `(a, b)`.
fn identifiers(node: Node) -> Vec<Node> {
    let mut out = Vec::new();
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        if n.kind() == "identifier" {
            out.push(n);
            continue;
        }
        for child in named_children(n).into_iter().rev() {
            stack.push(child);
        }
    }
    out
}

/// A catch body that swallows the exception: empty, or only logging and no
/// `throw`.
fn swallows<'t>(body: Node<'t>, b: &Builder<'t>) -> bool {
    let statements = named_children(body);
    if statements.is_empty() {
        return true;
    }
    let mut stack = vec![body];
    while let Some(n) = stack.pop() {
        match n.kind() {
            "throw_statement" | "throw_expression" => return false,
            k if is_lambda(k) || k == "local_function_statement" => continue,
            _ => stack.extend(named_children(n)),
        }
    }
    statements.into_iter().all(|s| {
        s.kind() == "expression_statement"
            && s.named_child(0).is_some_and(|e| {
                let e = if e.kind() == "await_expression" {
                    e.named_child(0).unwrap_or(e)
                } else {
                    e
                };
                e.kind() == "invocation_expression"
                    && e.child_by_field_name("function")
                        .is_some_and(|f| is_log_like(&b.callee(f).0))
            })
    })
}
