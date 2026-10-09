//! A type's header read from its signature (plan B, U1: R4, KTD6).
//!
//! The header under a type card's title used to be the stated visibility plus
//! the symbol kind. That is wrong by construction for C#: the plugin keeps only
//! the ACCESS modifiers as [`FactKind::SymbolVisibility`] — `static`,
//! `abstract`, `sealed` are not visibility — so `public static class GeoSolver`
//! read as an ordinary `public class`. The signature carries the whole
//! declaration head, so the header is read from there instead, and the plugin
//! contract does not change.
//!
//! The reading is deliberately closed. After the attribute lists (`[...]`),
//! every leading token must be a modifier this module knows, and the first
//! token that is not must be the kind keyword the [`FactKind::SymbolKind`] fact
//! names. Anything else — a Go `type X struct`, a Java annotation, a Rust
//! `pub type` alias — yields no header, and the card keeps the visibility-and-
//! kind strip it always had. A header that guessed at an unknown word would be
//! the same false statement this replaces, only in a different place.
//!
//! [`FactKind::SymbolVisibility`]: crate::graph::FactKind::SymbolVisibility
//! [`FactKind::SymbolKind`]: crate::graph::FactKind::SymbolKind

/// The modifiers a header may carry, in the one order it prints them.
///
/// The order is fixed rather than taken from the source because a `partial`
/// type's parts may state their modifiers in different orders and in different
/// files, and the header must not depend on which part sorts first. It follows
/// the usual C# spelling — access first, then `static`/`abstract`/`sealed`,
/// `partial` last — so a single-part header reads the way it is written. Access
/// is ordered so the two-word C# forms come out as written: `private protected`
/// and `protected internal`.
const MODIFIERS: &[&str] = &[
    "export",
    "default",
    "declare",
    "public",
    "private",
    "protected",
    "internal",
    "file",
    "new",
    "static",
    "abstract",
    "sealed",
    "non-sealed",
    "final",
    "strictfp",
    "readonly",
    "ref",
    "unsafe",
    "extern",
    "const",
    "partial",
];

/// A type's header: its modifiers in [`MODIFIERS`] order and its kind word as
/// the signature spells it (`class`, `struct`, `record struct`, …).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeHeader {
    pub modifiers: Vec<String>,
    pub kind: String,
}

impl TypeHeader {
    /// `public static class` — the header as one phrase.
    pub fn text(&self) -> String {
        let mut words: Vec<&str> = self.modifiers.iter().map(String::as_str).collect();
        words.push(&self.kind);
        words.join(" ")
    }
}

/// The header of a type from the signatures of all its parts, or `None` when
/// none of them can be read (see the module doc).
///
/// `parts` are `(file, signature)` pairs. They are sorted by file before they
/// are read, so the kind word comes from the same part whatever order the graph
/// listed them in; the modifiers are the union over every part, because
/// `abstract` or `sealed` stated on any part of a `partial` type holds for the
/// whole type. A part that cannot be read contributes nothing, and the parts
/// that can still make the header.
pub fn from_signatures<'a>(
    kind: &str,
    parts: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Option<TypeHeader> {
    let mut parts: Vec<(&str, &str)> = parts.into_iter().collect();
    parts.sort_unstable();
    let mut rank: Vec<usize> = Vec::new();
    let mut pub_forms: Vec<String> = Vec::new();
    let mut kind_word: Option<String> = None;
    for (_, signature) in parts {
        let Some(read) = read_signature(kind, signature) else {
            continue;
        };
        rank.extend(read.ranks);
        pub_forms.extend(read.pub_forms);
        kind_word.get_or_insert(read.kind);
    }
    let kind = kind_word?;
    rank.sort_unstable();
    rank.dedup();
    pub_forms.sort();
    pub_forms.dedup();
    // Rust's `pub`/`pub(crate)` is the access word, so it goes where access
    // goes: before everything a C#-style list would put after `file`.
    let access_end = MODIFIERS.iter().position(|m| *m == "new").unwrap_or(0);
    let mut modifiers: Vec<String> = Vec::new();
    modifiers.extend(
        rank.iter()
            .filter(|r| **r < access_end)
            .map(|r| MODIFIERS[*r].to_string()),
    );
    modifiers.extend(pub_forms);
    modifiers.extend(
        rank.iter()
            .filter(|r| **r >= access_end)
            .map(|r| MODIFIERS[*r].to_string()),
    );
    Some(TypeHeader { modifiers, kind })
}

/// What one signature says about the header.
struct Read {
    ranks: Vec<usize>,
    pub_forms: Vec<String>,
    kind: String,
}

/// Read one signature: skip the attribute lists, collect the known modifiers,
/// and stop at the kind keyword. `None` when an unknown word comes first or
/// the keyword is not `kind`.
fn read_signature(kind: &str, signature: &str) -> Option<Read> {
    let mut rest = signature.trim_start();
    // C# attribute lists — `[Serializable]`, `[A, B(typeof(C[]))]` — come
    // first, each one balanced, and are never part of the header.
    while rest.starts_with('[') {
        let mut depth = 0usize;
        let mut end = None;
        for (i, c) in rest.char_indices() {
            match c {
                '[' => depth += 1,
                ']' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(i + 1);
                        break;
                    }
                }
                _ => {}
            }
        }
        rest = rest[end?..].trim_start();
    }

    let mut read = Read {
        ranks: Vec::new(),
        pub_forms: Vec::new(),
        kind: String::new(),
    };
    let mut tokens = rest.split_whitespace().peekable();
    while let Some(token) = tokens.next() {
        if token == kind {
            read.kind = token.to_string();
            // A C# `record struct` / `record class` is one kind in two words.
            if token == "record" {
                if let Some(next) = tokens.peek().filter(|t| matches!(**t, "struct" | "class")) {
                    read.kind = format!("record {next}");
                }
            }
            return Some(read);
        }
        if let Some(rank) = MODIFIERS.iter().position(|m| *m == token) {
            read.ranks.push(rank);
        } else if token == "pub" || token.starts_with("pub(") {
            // Rust's `pub(in crate::a)` is one access word the whitespace
            // collapse split in two; take it up to its closing paren.
            let mut form = token.to_string();
            while form.matches('(').count() > form.matches(')').count() {
                form.push(' ');
                form.push_str(tokens.next()?);
            }
            read.pub_forms.push(form);
        } else {
            return None;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(kind: &str, signature: &str) -> Option<String> {
        from_signatures(kind, [("a.cs", signature)]).map(|h| h.text())
    }

    #[test]
    fn modifiers_come_from_the_signature_and_stop_at_the_kind() {
        assert_eq!(
            header("class", "public static class GeoSolver").as_deref(),
            Some("public static class")
        );
        assert_eq!(
            header(
                "class",
                "[Serializable] [A(typeof(B[]))] public static class X<T> : Base where T : new()"
            )
            .as_deref(),
            Some("public static class")
        );
        assert_eq!(
            header("record", "public sealed record struct P(int X)").as_deref(),
            Some("public sealed record struct")
        );
        assert_eq!(
            header("struct", "pub(in crate::a) struct S").as_deref(),
            Some("pub(in crate::a) struct")
        );
        assert_eq!(
            header("class", "static public class X").as_deref(),
            Some("public static class"),
            "one order, whatever order the source states them in"
        );
    }

    /// The shapes the closed reading refuses, so the card keeps its old strip
    /// rather than a header built from a word this module does not know.
    #[test]
    fn an_unknown_word_or_another_kind_yields_no_header() {
        assert_eq!(header("struct", "type Point struct"), None);
        assert_eq!(header("class", "@Entity public class X"), None);
        assert_eq!(header("class", "public interface X"), None);
        assert_eq!(header("class", ""), None);
        assert_eq!(header("class", "[Unclosed public class X"), None);
    }

    #[test]
    fn partial_parts_union_their_modifiers_in_one_order() {
        let a = from_signatures(
            "class",
            [
                ("b/X.cs", "sealed partial class X"),
                ("a/X.cs", "public partial class X : IThing"),
            ],
        );
        let b = from_signatures(
            "class",
            [
                ("a/X.cs", "sealed partial class X"),
                ("b/X.cs", "public partial class X : IThing"),
            ],
        );
        assert_eq!(a, b);
        assert_eq!(a.unwrap().text(), "public sealed partial class");
    }
}
