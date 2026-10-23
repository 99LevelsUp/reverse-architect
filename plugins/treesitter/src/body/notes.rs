//! What the `bodies` pass says beside the model of bodies (contract 0.4,
//! `notes`): the ordinary comments of a file and the general roles of its
//! literals.
//!
//! # Comments
//!
//! Every `//` and `/* */` comment of the file, never a doc comment (`///`,
//! `/** */`), with its text stripped of the comment markers. Whether a comment
//! holds words for a reader or code that was commented out is decided by one
//! fixed rule (plan vrstva 3, KTD5): its text is parsed with the C# grammar,
//! and it is code when the parse has at least one statement or declaration
//! and no more than [`CODE_MAX_ERROR_PERCENT`] of its nodes are errors or
//! missing tokens. A sentence almost never parses: `Disabled on purpose`
//! lacks the `;` a statement needs, a single word is an expression without
//! one.
//!
//! # Literal roles
//!
//! - **key** — a string literal that is the subscript of an element access
//!   (`p["BOOK"]`), or the first argument of a dictionary's lookup or update
//!   ([`KEY_METHODS`]: `TryGetValue("K", out v)`);
//! - **compared** — a literal the model records as compared with a path
//!   (`s == "X"`).
//!
//! An attribute's arguments are not literals of a member body here (they are
//! the `attributes` of a declaration's meta), so no role points at them.

use super::{named_children, range, text, FileBodies};
use crate::ra::plugin::notes::{Comment, CommentKind, LiteralRole, RoleKind};
use crate::ra::plugin::types::{LiteralContext, Range, ValueRef};
use crate::walkers::parse;
use std::collections::HashMap;
use tree_sitter::Node;

/// The share of error and missing nodes, in percent of every node of a
/// comment's parse, at or below which a comment that parses into a statement
/// or a declaration is code.
pub(super) const CODE_MAX_ERROR_PERCENT: usize = 10;

/// The methods whose first argument is the key of a dictionary.
const KEY_METHODS: &[&str] = &[
    "TryGetValue",
    "ContainsKey",
    "GetValueOrDefault",
    "TryAdd",
    "Add",
    "Remove",
];

/// Every ordinary comment under `root`, in source order.
pub(super) fn comments(root: Node, src: &[u8], grammar: &tree_sitter::Language) -> Vec<Comment> {
    let mut out = Vec::new();
    let mut stack = vec![root];
    let mut found: Vec<Node> = Vec::new();
    while let Some(n) = stack.pop() {
        if n.kind() == "comment" {
            found.push(n);
            continue;
        }
        let mut cursor = n.walk();
        let children: Vec<Node> = n.children(&mut cursor).collect();
        stack.extend(children.into_iter().rev());
    }
    for node in found {
        let raw = text(node, src);
        if is_doc_comment(raw) {
            continue;
        }
        let body = without_markers(raw);
        if body.is_empty() {
            continue;
        }
        out.push(Comment {
            place: range(node, src),
            kind: kind_of(&body, grammar),
            text: body,
        });
    }
    out
}

fn is_doc_comment(raw: &str) -> bool {
    raw.starts_with("///") || (raw.starts_with("/**") && !raw.starts_with("/**/"))
}

/// A comment's text without `//`, or without `/*`, `*/` and the `*` that
/// starts each line of a block, trimmed.
fn without_markers(raw: &str) -> String {
    if let Some(line) = raw.strip_prefix("//") {
        return line.trim().to_string();
    }
    let inner = raw.strip_prefix("/*").unwrap_or(raw);
    let inner = inner.strip_suffix("*/").unwrap_or(inner);
    inner
        .lines()
        .map(|l| {
            let l = l.trim();
            l.strip_prefix('*').map_or(l, str::trim_start)
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// Prose or code, by the rule of the module docs.
pub(super) fn kind_of(body: &str, grammar: &tree_sitter::Language) -> CommentKind {
    let Some(tree) = parse(body.as_bytes(), grammar.clone()) else {
        return CommentKind::Prose;
    };
    let root = tree.root_node();
    let statements = named_children(root)
        .iter()
        .filter(|n| !n.is_error() && !n.is_missing())
        .count();
    let (mut nodes, mut errors) = (0usize, 0usize);
    let mut stack = vec![root];
    while let Some(n) = stack.pop() {
        nodes += 1;
        if n.is_error() || n.is_missing() {
            errors += 1;
        }
        let mut cursor = n.walk();
        stack.extend(n.children(&mut cursor));
    }
    if statements > 0 && errors * 100 <= nodes * CODE_MAX_ERROR_PERCENT {
        CommentKind::Code
    } else {
        CommentKind::Prose
    }
}

/// The roles of the literals of `model`, read from `root`'s syntax and the
/// model's own records, in member and literal order.
pub(super) fn roles(root: Node, src: &[u8], model: &FileBodies) -> Vec<LiteralRole> {
    // Where each literal of the model is written: the key a subscript's
    // literal is found by.
    let mut at: HashMap<(u32, u32, u32, u32), (u32, u32)> = HashMap::new();
    for (m, member) in model.members.iter().enumerate() {
        for literal in &member.literals {
            at.insert(key(literal.place), (m as u32, literal.id));
        }
    }
    let mut roles: Vec<LiteralRole> = Vec::new();
    for (m, member) in model.members.iter().enumerate() {
        for literal in &member.literals {
            if matches!(literal.context, LiteralContext::Compared(_)) {
                roles.push(role(m as u32, literal.id, RoleKind::Compared));
            }
        }
        for invocation in &member.invocations {
            let Some(name) = invocation.callee.segments.last() else {
                continue;
            };
            if !KEY_METHODS.contains(&name.as_str()) {
                continue;
            }
            if let Some(ValueRef::Literal(id)) = invocation
                .args
                .iter()
                .find(|a| a.position == 0)
                .map(|a| &a.value)
            {
                roles.push(role(m as u32, *id, RoleKind::Key));
            }
        }
    }
    let mut stack = vec![root];
    while let Some(n) = stack.pop() {
        if n.kind() == "element_access_expression" {
            if let Some(subscript) = n.child_by_field_name("subscript") {
                for arg in named_children(subscript) {
                    let Some(value) = named_children(arg).into_iter().next_back() else {
                        continue;
                    };
                    if let Some(&(m, id)) = at.get(&key(range(value, src))) {
                        roles.push(role(m, id, RoleKind::Key));
                    }
                }
            }
        }
        let mut cursor = n.walk();
        stack.extend(n.children(&mut cursor));
    }
    roles.sort_by_key(|r| (r.member, r.literal, r.role as u8));
    roles.dedup_by_key(|r| (r.member, r.literal));
    roles
}

fn key(r: Range) -> (u32, u32, u32, u32) {
    (r.start.line, r.start.column, r.end.line, r.end.column)
}

fn role(member: u32, literal: u32, role: RoleKind) -> LiteralRole {
    LiteralRole {
        member,
        literal,
        role,
    }
}

#[cfg(test)]
mod tests {
    use super::super::file_notes;
    use crate::ra::plugin::notes::{CommentKind, FileNotes, RoleKind};
    use crate::walkers::Language;

    fn read(src: &str) -> FileNotes {
        file_notes("src/a.cs", src.as_bytes()).expect("the source is read")
    }

    fn kind(text: &str) -> CommentKind {
        super::kind_of(text, &Language::CSharp.grammar().unwrap())
    }

    #[test]
    fn every_ordinary_comment_comes_with_its_lines_and_no_doc_comment() {
        let src = "/// <summary>Doc.</summary>\nclass C {\n    // above the method\n    void M() {\n        /* inside\n         * the body */\n        N();\n    }\n}\n";
        let notes = read(src);
        let found: Vec<(&str, u32, u32)> = notes
            .comments
            .iter()
            .map(|c| (c.text.as_str(), c.place.start.line, c.place.end.line))
            .collect();
        assert_eq!(found, [("above the method", 3, 3), ("inside\nthe body", 5, 6)]);
        assert_eq!(notes.bodies.members.len(), 1);
    }

    #[test]
    fn commented_out_code_is_code_and_a_sentence_is_prose() {
        assert_eq!(kind("var x = Load(\"a\");"), CommentKind::Code);
        assert_eq!(kind("if (x == null) return;\nSave(x);"), CommentKind::Code);
        assert_eq!(kind("Vypnuto záměrně, viz ticket"), CommentKind::Prose);
        assert_eq!(kind("disabled on purpose"), CommentKind::Prose);
        assert_eq!(kind("TODO"), CommentKind::Prose);
        assert_eq!(kind("the \"quoted\" word stays a sentence"), CommentKind::Prose);
        assert_eq!(kind("x is null"), CommentKind::Prose);
    }

    #[test]
    fn a_subscript_and_a_dictionary_key_are_keys_and_a_compared_literal_is_compared() {
        let src = "class C {\n    void M(Dictionary<string, int> p, string s) {\n        var b = p[\"BOOK\"];\n        p.TryGetValue(\"SERIES\", out var v);\n        if (s == \"X\") { Run(\"plain\"); }\n    }\n}\n";
        let notes = read(src);
        let literals = &notes.bodies.members[0].literals;
        let role_of = |value: &str| {
            let id = literals
                .iter()
                .find(|l| l.value == value)
                .unwrap_or_else(|| panic!("no literal {value}: {literals:?}"))
                .id;
            notes
                .roles
                .iter()
                .find(|r| r.member == 0 && r.literal == id)
                .map(|r| r.role)
        };
        assert_eq!(role_of("BOOK"), Some(RoleKind::Key));
        assert_eq!(role_of("SERIES"), Some(RoleKind::Key));
        assert_eq!(role_of("X"), Some(RoleKind::Compared));
        assert_eq!(role_of("plain"), None);
    }

    #[test]
    fn a_generated_file_has_no_notes() {
        let notes = file_notes("src/Form1.Designer.cs", b"// a comment\nclass C { }\n").unwrap();
        assert!(notes.comments.is_empty() && notes.roles.is_empty());
    }
}
