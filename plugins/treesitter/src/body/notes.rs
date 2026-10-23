//! What the `bodies` pass says beside the model of bodies (contract 0.4,
//! `notes`): the ordinary comments of a file and the general roles of its
//! literals.
//!
//! # Comments
//!
//! Every `//` and `/* */` comment of the file that the host does not take for
//! a doc comment (the host's rule: `///` but not `////`, `//!`, `/**` but not
//! `/**/`, and `/*!`), with its text stripped of the comment markers. Whether
//! a comment holds words for a reader or code that was commented out is
//! decided by one fixed rule (plan vrstva 3, KTD5): its text is parsed with
//! the C# grammar, and it is code when the parse has at least one complete
//! statement or declaration — no error or missing token inside it, not a
//! label (`TODO: fix this` parses as one) and not a nested comment — and no
//! more than [`CODE_MAX_ERROR_PERCENT`] of its nodes are errors or missing
//! tokens. A sentence almost never parses: `Disabled on purpose` lacks the
//! `;` a statement needs, a single word is an expression without one. A text
//! none of whose lines ends with `;` or `}` (a trailing `// comment` aside)
//! is prose without being parsed: commented-out code ends its lines that
//! way, sentences do not. A text longer than [`CLASSIFY_MAX_BYTES`] that
//! passes, whose parse takes more work than its length allows
//! ([`PARSE_CHECKS_PER_KIB`]), or that comes after the file has spent what
//! its failed parses may cost ([`FAILED_PARSES_BASE`]) is code.
//!
//! One-line `//` comments that stand alone on consecutive lines are also
//! classified together: when the joined run is code, every line of it is
//! code, so a commented-out call split over lines (`// var q = db.Orders`,
//! `// .ToList();`) does not leave fragments that read as prose. A run
//! longer than [`CLASSIFY_MAX_BYTES`] is code on every line when any of its
//! lines ends like code, without a parse. Otherwise each line keeps its own
//! kind.
//!
//! # Literal roles
//!
//! - **key** — a string literal that is the subscript of an element access
//!   (`p["BOOK"]`, `p?["BOOK"]`), or the first argument of a dictionary's
//!   lookup or update: [`KEY_METHODS`] on any receiver
//!   (`TryGetValue("K", out v)`), [`DICTIONARY_METHODS`] only on a receiver
//!   the file declares with a dictionary type (`map.Add("K", v)`), so
//!   `list.Add("x")`, `logger.Add("x", 1)` and a call on a receiver of
//!   unknown type give no role;
//! - **compared** — a literal the model records as compared with a path
//!   (`s == "X"`), a literal on either side of `==` or `!=` whatever the
//!   other side is (`p["K"] == "V"`), and a literal that is a constant
//!   pattern: a `case "X":` label, a switch expression arm, `s is "X"`.
//!
//! A literal has one role; a key is not also compared. An attribute's
//! arguments are not literals of a member body here (they are the
//! `attributes` of a declaration's meta), so no role points at them and this
//! component sends no `attribute-arg`.

use super::{
    argument_expression, is_string_literal, named_children, range, text, unparen, FileBodies,
};
use crate::ra::plugin::notes::{Comment, CommentKind, LiteralRole, RoleKind};
use crate::ra::plugin::types::{AccessPath, LiteralContext, MemberBody, PathRoot, Range, ValueRef};
use std::collections::HashMap;
use std::ops::ControlFlow;
use tree_sitter::{Node, ParseOptions, ParseState, Parser, Tree};

/// The share of error and missing nodes, in percent of every node of a
/// comment's parse, at or below which a comment that parses into a statement
/// or a declaration is code.
pub(super) const CODE_MAX_ERROR_PERCENT: usize = 10;

/// The longest text, in bytes, the rule parses. A longer comment is code
/// without being parsed, which keeps it from the model, and a longer run of
/// line comments is classified line by line.
const CLASSIFY_MAX_BYTES: usize = 2048;

/// The parser's progress checks (one per hundred of its operations) a
/// comment's parse may take: a base, and so many per KiB of its text. Text
/// that is not C# makes the parser recover from its errors at a cost that
/// grows faster than its length. Measured natively (tree-sitter-c-sharp
/// 0.23.5): code takes about 35 checks per KiB, prose about 15, a 1 KiB line
/// of an ASCII table with `;` and `}` 555 and a 4 KiB one 2 194 per KiB. A
/// parse that runs out is code, which keeps the text from the model.
const PARSE_CHECKS_BASE: usize = 20;
const PARSE_CHECKS_PER_KIB: usize = 100;

/// What the parses that end in an error may spend in one file: so many of
/// them, and one more for so many bytes of the file, and their text at most
/// a share of the file. In the sandbox such a parse costs about five million
/// units of fuel and up to some hundred thousand per byte of its text,
/// against the host's 500 million per file and 50 000 per byte of it, while
/// a clean parse costs about ten thousand per byte. Once the budget is spent,
/// a text that would need a parse is code without one. The sandbox test of
/// the bundled component holds every shape of comment it tries under half
/// the file's fuel.
const FAILED_PARSES_BASE: usize = 20;
const FILE_BYTES_PER_FAILED_PARSE: usize = 4000;
const FILE_BYTES_PER_FAILED_BYTE: usize = 16;

/// What the parses of one file's comments may still spend
/// ([`FAILED_PARSES_BASE`]).
pub(super) struct Budget {
    failed_parses: usize,
    failed_bytes: usize,
}

impl Budget {
    pub(super) fn for_file(len: usize) -> Self {
        Budget {
            failed_parses: FAILED_PARSES_BASE + len / FILE_BYTES_PER_FAILED_PARSE,
            failed_bytes: len / FILE_BYTES_PER_FAILED_BYTE,
        }
    }

    #[cfg(test)]
    pub(super) fn unlimited() -> Self {
        Budget {
            failed_parses: usize::MAX,
            failed_bytes: usize::MAX,
        }
    }
}

/// The methods whose first argument is the key of a dictionary, whatever the
/// receiver: no list or set has them.
const KEY_METHODS: &[&str] = &["TryGetValue", "ContainsKey", "GetValueOrDefault"];

/// The methods whose first argument is the key of a dictionary only when the
/// receiver is declared as one ([`is_dictionary_type`]): a list, a set or a
/// logger has them too.
const DICTIONARY_METHODS: &[&str] = &["Add", "TryAdd", "Remove"];

/// Whether a type, as written, is a dictionary: its last name holds
/// `Dictionary`, or it is `Hashtable` or `SortedList`.
fn is_dictionary_type(type_name: &str) -> bool {
    let last = type_name.rsplit('.').next().unwrap_or(type_name);
    last.contains("Dictionary") || last == "Hashtable" || last == "SortedList"
}

/// A comment found in the file, before its kind is decided.
struct Found {
    place: Range,
    text: String,
    /// A `//` comment with nothing but whitespace before it on its line.
    alone: bool,
}

/// Every ordinary comment under `root`, in source order.
pub(super) fn comments(root: Node, src: &[u8], grammar: &tree_sitter::Language) -> Vec<Comment> {
    let mut found: Vec<Found> = Vec::new();
    let mut stack = vec![root];
    while let Some(n) = stack.pop() {
        if n.kind() == "comment" {
            let raw = text(n, src);
            if is_doc_comment(raw) {
                continue;
            }
            let body = without_markers(raw);
            if !body.is_empty() {
                found.push(Found {
                    place: range(n, src),
                    alone: raw.starts_with("//") && alone_on_line(n, src),
                    text: body,
                });
            }
            continue;
        }
        let mut cursor = n.walk();
        let children: Vec<Node> = n.children(&mut cursor).collect();
        stack.extend(children.into_iter().rev());
    }
    let mut budget = Budget::for_file(src.len());
    let mut out = Vec::with_capacity(found.len());
    let mut i = 0;
    while i < found.len() {
        let mut j = i + 1;
        if found[i].alone {
            while j < found.len()
                && found[j].alone
                && found[j].place.start.line == found[j - 1].place.end.line + 1
            {
                j += 1;
            }
        }
        let run_is_code = j - i > 1 && {
            let joined: Vec<&str> = found[i..j].iter().map(|f| f.text.as_str()).collect();
            let joined = joined.join("\n");
            if joined.len() > CLASSIFY_MAX_BYTES {
                joined.lines().any(ends_like_code)
            } else {
                kind_of(&joined, grammar, &mut budget) == CommentKind::Code
            }
        };
        for f in &found[i..j] {
            out.push(Comment {
                place: f.place,
                kind: if run_is_code {
                    CommentKind::Code
                } else {
                    kind_of(&f.text, grammar, &mut budget)
                },
                text: f.text.clone(),
            });
        }
        i = j;
    }
    out
}

/// The host's rule for a doc comment (`src/extract/external.rs`,
/// `is_doc_comment`): what it takes for one reaches the first layer, so this
/// pass leaves exactly that out.
fn is_doc_comment(raw: &str) -> bool {
    (raw.starts_with("///") && !raw.starts_with("////"))
        || raw.starts_with("//!")
        || (raw.starts_with("/**") && !raw.starts_with("/**/"))
        || raw.starts_with("/*!")
}

/// Whether only whitespace precedes `node` on its line.
fn alone_on_line(node: Node, src: &[u8]) -> bool {
    src[..node.start_byte()]
        .iter()
        .rev()
        .take_while(|&&b| b != b'\n')
        .all(u8::is_ascii_whitespace)
}

/// A comment's text without its leading `/`s, or without `/*`, `*/` and the
/// `*` that starts each line of a block, trimmed.
fn without_markers(raw: &str) -> String {
    if raw.starts_with("//") {
        return raw.trim_start_matches('/').trim().to_string();
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
pub(super) fn kind_of(
    body: &str,
    grammar: &tree_sitter::Language,
    budget: &mut Budget,
) -> CommentKind {
    if !body.lines().any(ends_like_code) {
        return CommentKind::Prose;
    }
    if body.len() > CLASSIFY_MAX_BYTES || budget.failed_parses == 0 || budget.failed_bytes == 0 {
        return CommentKind::Code;
    }
    let parsed = parse_bounded(body.as_bytes(), grammar);
    if parsed.as_ref().is_none_or(|t| t.root_node().has_error()) {
        budget.failed_parses -= 1;
        budget.failed_bytes = budget.failed_bytes.saturating_sub(body.len());
    }
    let Some(tree) = parsed else {
        return CommentKind::Code;
    };
    let root = tree.root_node();
    let statements = named_children(root)
        .iter()
        .filter(|n| is_complete_statement(**n))
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

/// Whether a line ends the way a statement or a block does, with `;` or `}`,
/// a comment after it aside (`x = 1; // old`, `x = 1;/* old */`). A sentence
/// ends otherwise, so
/// prose is told apart without a parse, which in the sandbox costs several
/// times more fuel for text that is not C# than for code.
fn ends_like_code(line: &str) -> bool {
    let code = without_comments(line);
    let code = code.trim_end();
    code.ends_with(';') || code.ends_with('}')
}

/// `line` without its comments — a `//` and all after it, every closed
/// `/* */` and an unclosed `/*` with all after it — one inside a string or a
/// character literal aside (`"http://x"`, and `@"C:\"`, where a backslash
/// escapes nothing).
fn without_comments(line: &str) -> String {
    let b = line.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    // The quote a string or character literal is open with, and whether it
    // is verbatim (`@"…"`: no escapes, `""` is a quote).
    let mut open: Option<(u8, bool)> = None;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        match open {
            Some((q, verbatim)) => {
                out.push(c);
                if c == b'\\' && !verbatim {
                    if let Some(&next) = b.get(i + 1) {
                        out.push(next);
                        i += 1;
                    }
                } else if c == q {
                    if verbatim && b.get(i + 1) == Some(&q) {
                        out.push(q);
                        i += 1;
                    } else {
                        open = None;
                    }
                }
            }
            None if c == b'/' && b.get(i + 1) == Some(&b'/') => break,
            None if c == b'/' && b.get(i + 1) == Some(&b'*') => match line[i + 2..].find("*/") {
                Some(end) => {
                    i += 2 + end + 2;
                    continue;
                }
                None => break,
            },
            None => {
                if c == b'"' || c == b'\'' {
                    let verbatim = c == b'"' && (b[..i].ends_with(b"@") || b[..i].ends_with(b"@$"));
                    open = Some((c, verbatim));
                }
                out.push(c);
            }
        }
        i += 1;
    }
    // Only ASCII delimiters were left out, so the bytes are still UTF-8.
    String::from_utf8(out).unwrap_or_default()
}

/// The parse of `text`, or `None` when it takes more than
/// [`PARSE_CHECKS_PER_KIB`] allows.
fn parse_bounded(text: &[u8], grammar: &tree_sitter::Language) -> Option<Tree> {
    let mut parser = Parser::new();
    parser.set_language(grammar).ok()?;
    let budget = PARSE_CHECKS_BASE + text.len() * PARSE_CHECKS_PER_KIB / 1024;
    let mut checks = 0usize;
    let mut progress = |_: &ParseState| {
        checks += 1;
        if checks > budget {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    };
    parser.parse_with_options(
        &mut |i, _| text.get(i..).unwrap_or_default(),
        None,
        Some(ParseOptions::new().progress_callback(&mut progress)),
    )
}

/// A top-level node of a comment's parse that counts as a statement: no
/// error or missing token anywhere in it, not a comment, and not a label —
/// `TODO: fix this` and `Note: see Load()` parse as labelled statements.
fn is_complete_statement(n: Node) -> bool {
    if n.is_extra() || n.has_error() {
        return false;
    }
    let inner = if n.kind() == "global_statement" {
        n.named_child(0).unwrap_or(n)
    } else {
        n
    };
    inner.kind() != "labeled_statement"
}

/// The roles of the literals of `model`, read from `root`'s syntax and the
/// model's own records, in member and literal order.
pub(super) fn roles(root: Node, src: &[u8], model: &FileBodies) -> Vec<LiteralRole> {
    // Where each literal of the model is written: the key a literal found in
    // the syntax is looked up by.
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
            let segments = &invocation.callee.segments;
            let Some(name) = segments.last() else {
                continue;
            };
            let keyed = KEY_METHODS.contains(&name.as_str())
                || (DICTIONARY_METHODS.contains(&name.as_str())
                    && declared_dictionary(&invocation.callee, member, model));
            if !keyed {
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
    let push = |node: Node, kind: RoleKind, roles: &mut Vec<LiteralRole>| {
        let node = unparen(node);
        if is_string_literal(node.kind()) {
            if let Some(&(m, id)) = at.get(&key(range(node, src))) {
                roles.push(role(m, id, kind));
            }
        }
    };
    let mut stack = vec![root];
    while let Some(n) = stack.pop() {
        match n.kind() {
            "element_access_expression" => {
                if let Some(subscript) = n.child_by_field_name("subscript") {
                    for a in named_children(subscript) {
                        if let Some(e) = argument_expression(a) {
                            push(e, RoleKind::Key, &mut roles);
                        }
                    }
                }
            }
            // `p?["K"]`.
            "element_binding_expression" => {
                for child in named_children(n) {
                    let args = if child.kind() == "bracketed_argument_list" {
                        named_children(child)
                    } else {
                        vec![child]
                    };
                    for a in args {
                        if let Some(e) = argument_expression(a) {
                            push(e, RoleKind::Key, &mut roles);
                        }
                    }
                }
            }
            // `s == "X"` with any operand, not only a path the model reads.
            "binary_expression" => {
                let operator = n.child_by_field_name("operator").map(|o| o.kind());
                if matches!(operator, Some("==" | "!=")) {
                    for side in ["left", "right"] {
                        if let Some(e) = n.child_by_field_name(side) {
                            push(e, RoleKind::Compared, &mut roles);
                        }
                    }
                }
            }
            "constant_pattern" => {
                if let Some(e) = n.named_child(0) {
                    push(e, RoleKind::Compared, &mut roles);
                }
            }
            // A `case` label written as a plain expression.
            "switch_section" => {
                for child in named_children(n) {
                    push(child, RoleKind::Compared, &mut roles);
                }
            }
            "switch_expression_arm" => {
                if let Some(pattern) = n.named_child(0) {
                    push(pattern, RoleKind::Compared, &mut roles);
                }
            }
            "is_pattern_expression" => {
                if let Some(pattern) = n.child_by_field_name("pattern") {
                    push(pattern, RoleKind::Compared, &mut roles);
                }
            }
            _ => {}
        }
        let mut cursor = n.walk();
        stack.extend(n.children(&mut cursor));
    }
    roles.sort_by_key(|r| (r.member, r.literal, r.role as u8));
    roles.dedup_by_key(|r| (r.member, r.literal));
    roles
}

/// Whether the receiver of a call `callee` (`map.Add`, `this.map.Add`) is
/// declared with a dictionary type: a local, a parameter or a pattern of
/// `member` when the path starts at one, a field, a property or an event of
/// the file otherwise. Every declaration of that name there must be a
/// dictionary, so a name declared twice with other types, a longer path and
/// a name the file does not declare give no role.
fn declared_dictionary(callee: &AccessPath, member: &MemberBody, model: &FileBodies) -> bool {
    let [receiver, _] = callee.segments.as_slice() else {
        return false;
    };
    let declared = match callee.root {
        PathRoot::Local => &member.typed_names,
        PathRoot::This | PathRoot::Name => &model.typed_members,
        PathRoot::Base | PathRoot::Expression => return false,
    };
    let mut of_name = declared.iter().filter(|t| t.name == *receiver).peekable();
    of_name.peek().is_some() && of_name.all(|t| is_dictionary_type(&t.type_name))
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
        super::kind_of(
            text,
            &Language::CSharp.grammar().unwrap(),
            &mut super::Budget::unlimited(),
        )
    }

    /// The role of the literal `value` of the file's first member.
    fn role_of(notes: &FileNotes, value: &str) -> Option<RoleKind> {
        let literals = &notes.bodies.members[0].literals;
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
    }

    fn in_method(body: &str) -> String {
        format!("class C {{\n    void M(Dictionary<string, string> p, List<string> l, string s) {{\n{body}\n    }}\n}}\n")
    }

    #[test]
    fn every_ordinary_comment_comes_with_its_lines_and_no_doc_comment() {
        let src = "/// <summary>Doc.</summary>\nclass C {\n    // above the method\n    void M() {\n        /* inside\n         * the body */\n        N();\n        /** doc */\n        //\n        /**/\n    }\n}\n";
        let notes = read(src);
        let found: Vec<(&str, u32, u32)> = notes
            .comments
            .iter()
            .map(|c| (c.text.as_str(), c.place.start.line, c.place.end.line))
            .collect();
        assert_eq!(
            found,
            [("above the method", 3, 3), ("inside\nthe body", 5, 6)]
        );
        assert_eq!(notes.bodies.members.len(), 1);
    }

    #[test]
    fn a_four_slash_comment_is_an_ordinary_comment_without_its_slashes() {
        let notes =
            read("class C {\n    void M() {\n        //// var x = Load(\"a\");\n    }\n}\n");
        let found: Vec<(&str, CommentKind)> = notes
            .comments
            .iter()
            .map(|c| (c.text.as_str(), c.kind))
            .collect();
        assert_eq!(found, [("var x = Load(\"a\");", CommentKind::Code)]);
    }

    /// The fixture set that fixes the KTD5 threshold: descriptive, commented
    /// out and mixed comments in English and Czech.
    #[test]
    fn commented_out_code_is_code_and_a_sentence_is_prose() {
        let code = [
            "var x = Load(\"a\");",
            "if (x == null) return;\nSave(x);",
            "_cache.Clear();",
            "return result;",
            "foreach (var o in orders) { Ship(o); }",
            "await SaveAsync(order, token);",
            "x = y + 1; // old",
            "Save();// old",
            "Save(); /* old */",
            "Save(/* old */);",
            "var u = \"http://example.com\";",
            "var p = @\"C:\\\"; // old",
        ];
        let prose = [
            "Vypnuto záměrně, viz ticket",
            "disabled on purpose",
            "TODO",
            "TODO: fix this",
            "TODO: fix this;",
            "Note: see Load()",
            "Pozor: volá se z UI vlákna",
            "the \"quoted\" word stays a sentence",
            "x is null",
            "// nested note",
            "------------------------------",
            "===== Helpers =====",
            "| Key | Value |\n|-----|-------|",
            "Called from the UI thread only.",
            "Hodnota se přepočítá při uložení dokladu.",
            "See ticket 123; the old path broke paging.",
            "Tady se načítá objednávka; nemazat.",
        ];
        for text in code {
            assert_eq!(kind(text), CommentKind::Code, "{text}");
        }
        for text in prose {
            assert_eq!(kind(text), CommentKind::Prose, "{text}");
        }
    }

    #[test]
    fn a_commented_out_call_over_several_lines_is_code_on_every_line() {
        let src = "class C {\n    void M() {\n        // var q = db.Orders\n        //     .Where(o => o.Paid)\n        //     .ToList();\n        // Vypnuto záměrně\n        N(); // a call\n        // beside it\n    }\n}\n";
        let notes = read(src);
        let found: Vec<(&str, CommentKind)> = notes
            .comments
            .iter()
            .map(|c| (c.text.as_str(), c.kind))
            .collect();
        assert_eq!(
            found,
            [
                ("var q = db.Orders", CommentKind::Code),
                (".Where(o => o.Paid)", CommentKind::Code),
                (".ToList();", CommentKind::Code),
                ("Vypnuto záměrně", CommentKind::Code),
                ("a call", CommentKind::Prose),
                ("beside it", CommentKind::Prose),
            ]
        );
    }

    #[test]
    fn text_past_the_parse_limit_is_code_and_so_is_a_long_run_that_ends_like_code() {
        let long = "x = 1; ".repeat(super::CLASSIFY_MAX_BYTES / 7 + 1);
        assert_eq!(kind(&long), CommentKind::Code);
        assert_eq!(kind(&"word ".repeat(1000)), CommentKind::Prose);
        let mut src = String::from("class C {\n    void M() {\n        // var q = db.Orders\n");
        for _ in 0..super::CLASSIFY_MAX_BYTES / 20 {
            src.push_str("        // .Where(o => o.Paid)\n");
        }
        src.push_str("        // .ToList();\n    }\n}\n");
        let notes = read(&src);
        assert!(notes.comments.iter().all(|c| c.kind == CommentKind::Code));
        // A long run of prose stays prose line by line.
        let mut src = String::from("class C {\n    void M() {\n");
        for _ in 0..super::CLASSIFY_MAX_BYTES / 20 {
            src.push_str("        // a long note, line after line\n");
        }
        src.push_str("    }\n}\n");
        let notes = read(&src);
        assert!(notes.comments.iter().all(|c| c.kind == CommentKind::Prose));
    }

    #[test]
    fn a_file_that_has_spent_its_failed_parses_gets_code_without_a_parse() {
        let mut src = String::from("class C {\n    void M() {\n");
        for _ in 0..60 {
            src.push_str("        // see the old path;\n\n");
        }
        src.push_str("    }\n}\n");
        let notes = read(&src);
        assert_eq!(notes.comments.len(), 60);
        assert_eq!(notes.comments[0].kind, CommentKind::Prose);
        assert_eq!(notes.comments[59].kind, CommentKind::Code);
    }

    #[test]
    fn a_subscript_and_a_dictionary_key_are_keys_and_a_compared_literal_is_compared() {
        let notes = read(&in_method(
            "        var b = p[\"BOOK\"];\n        var c = p?[\"NULLABLE\"];\n        p.TryGetValue(\"SERIES\", out var v);\n        if (s == \"X\") { Run(\"plain\"); }\n        if (p[\"K\"] == \"V\") { }",
        ));
        assert_eq!(role_of(&notes, "BOOK"), Some(RoleKind::Key));
        assert_eq!(role_of(&notes, "NULLABLE"), Some(RoleKind::Key));
        assert_eq!(role_of(&notes, "SERIES"), Some(RoleKind::Key));
        assert_eq!(role_of(&notes, "X"), Some(RoleKind::Compared));
        assert_eq!(role_of(&notes, "K"), Some(RoleKind::Key));
        assert_eq!(role_of(&notes, "V"), Some(RoleKind::Compared));
        assert_eq!(role_of(&notes, "plain"), None);
    }

    #[test]
    fn every_dictionary_method_keys_its_first_argument_and_a_list_call_keys_nothing() {
        let notes = read(&in_method(
            "        p.TryGetValue(\"A\", out var a);\n        p.ContainsKey(\"B\");\n        p.GetValueOrDefault(\"C\");\n        p.TryAdd(\"D\", \"d-value\");\n        p.Add(\"E\", \"e-value\");\n        p.Remove(\"F\", out var f);\n        p.Remove(\"F1\");\n        l.Add(\"G\");\n        l.Remove(\"H\");",
        ));
        for k in ["A", "B", "C", "D", "E", "F", "F1"] {
            assert_eq!(role_of(&notes, k), Some(RoleKind::Key), "{k}");
        }
        for none in ["d-value", "e-value", "G", "H"] {
            assert_eq!(role_of(&notes, none), None, "{none}");
        }
    }

    /// `Add` and `Remove` key a literal only on a receiver declared as a
    /// dictionary: a field counts, a logger and an unknown receiver do not.
    #[test]
    fn an_update_keys_its_literal_only_on_a_declared_dictionary() {
        let notes = read("class C {\n    private Dictionary<string, int> _map;\n    private Logger _log;\n    void M() {\n        _map.Add(\"FIELD\", 1);\n        this._map.Remove(\"THIS\");\n        _log.Add(\"LOGGED\", 1);\n        Other.Add(\"UNKNOWN\", 1);\n    }\n}\n");
        assert_eq!(role_of(&notes, "FIELD"), Some(RoleKind::Key));
        assert_eq!(role_of(&notes, "THIS"), Some(RoleKind::Key));
        assert_eq!(role_of(&notes, "LOGGED"), None);
        assert_eq!(role_of(&notes, "UNKNOWN"), None);
        // `this.map` is the list field, `map` the dictionary parameter.
        let notes = read("class C {\n    private List<string> map;\n    void M(Dictionary<string, int> map) {\n        this.map.Add(\"FIELD\");\n        map.Add(\"PARAM\", 1);\n    }\n}\n");
        assert_eq!(role_of(&notes, "FIELD"), None);
        assert_eq!(role_of(&notes, "PARAM"), Some(RoleKind::Key));
    }

    #[test]
    fn a_case_label_a_switch_arm_and_an_is_pattern_are_compared() {
        let notes = read(&in_method(
            "        switch (s) { case \"CASE\": break; case (\"PAREN\"): break; }\n        var r = s switch { \"ARM\" => 1, _ => 0 };\n        if (s is \"IS\") { }",
        ));
        for c in ["CASE", "PAREN", "ARM", "IS"] {
            assert_eq!(role_of(&notes, c), Some(RoleKind::Compared), "{c}");
        }
    }

    /// Code and prose parse within the work their length buys, an ASCII
    /// table with `;` and `}` does not and is code without a parse.
    #[test]
    fn a_parse_that_takes_more_work_than_its_length_allows_gives_up() {
        let g = Language::CSharp.grammar().unwrap();
        let code =
            "var q = db.Orders.Where(o => o.Paid).ToList(); if (x == null) return; Save(x);\n"
                .repeat(12);
        let prose = "See ticket 123; the old path broke paging and nobody noticed. ".repeat(16);
        let table = "| -- ; == } ~~ ".repeat(66) + ";";
        assert!(super::parse_bounded(code.as_bytes(), &g).is_some());
        assert!(super::parse_bounded(prose.as_bytes(), &g).is_some());
        assert!(super::parse_bounded(table.as_bytes(), &g).is_none());
        assert_eq!(kind(&table), CommentKind::Code);
    }

    #[test]
    fn a_generated_file_has_no_notes() {
        let notes = file_notes("src/Form1.Designer.cs", b"// a comment\nclass C { }\n").unwrap();
        assert!(notes.comments.is_empty() && notes.roles.is_empty());
    }
}
