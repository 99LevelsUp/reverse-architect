//! Reading XAML: a tree of elements and attributes that remembers where each
//! part was written, and markup extensions (`{Binding …}`) taken apart.
//!
//! The tree holds byte ranges into the source, not copies: a resource
//! dictionary can carry megabytes of path geometry in its attributes, and
//! the plugin only ever looks at a handful of short ones.
//!
//! Positions follow the contract: lines and columns are 1-based, a column
//! counts characters (Unicode scalar values) from the start of its line, a
//! byte-order mark is not part of the text, and a line ends at `\n` whether
//! or not a `\r` precedes it.

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use std::borrow::Cow;
use std::fmt;
use std::ops::Range;

/// Why a file could not be read as XML.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub message: String,
    /// Byte offset into the text (after any byte-order mark).
    pub offset: usize,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (at byte {})", self.message, self.offset)
    }
}

impl std::error::Error for Error {}

fn error(message: impl Into<String>, offset: usize) -> Error {
    Error {
        message: message.into(),
        offset,
    }
}

/// One attribute: where its qualified name and its raw value (between the
/// quotes, entities not expanded) are written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attr {
    pub name: Range<usize>,
    pub value: Range<usize>,
}

/// One element, in the order the source opens them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Element {
    /// The qualified name in the start tag (`local:ViewA`, `Window.Resources`).
    pub name: Range<usize>,
    /// In source order.
    pub attrs: Vec<Attr>,
    pub parent: Option<usize>,
    /// In source order.
    pub children: Vec<usize>,
    /// Byte offset of the `<` that opens it.
    pub start: usize,
    /// Byte offset just past the `>` that closes it (its end tag, or `/>`).
    pub end: usize,
}

/// A parsed file. `elements` is in document order, so an element's parent
/// always comes before it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Doc<'a> {
    text: &'a str,
    line_starts: Vec<usize>,
    pub elements: Vec<Element>,
}

impl<'a> Doc<'a> {
    /// The text a range of this document covers.
    pub fn text(&self, range: &Range<usize>) -> &'a str {
        self.text.get(range.clone()).unwrap_or("")
    }

    pub fn len(&self) -> usize {
        self.text.len()
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// The line and column of a byte offset.
    pub fn position(&self, offset: usize) -> (u32, u32) {
        let line = self.line_starts.partition_point(|&start| start <= offset);
        let start = self.line_starts[line.max(1) - 1];
        let column = self
            .text
            .get(start..offset.max(start))
            .map_or(0, |s| s.chars().count());
        (to_u32(line.max(1)), to_u32(column + 1))
    }

    /// The lines an element spans, inclusive.
    pub fn lines(&self, element: &Element) -> (u32, u32) {
        let first = self.position(element.start).0;
        let last = self
            .position(element.end.saturating_sub(1).max(element.start))
            .0;
        (first, last)
    }

    /// The last line of the file.
    pub fn last_line(&self) -> u32 {
        self.position(self.text.len().saturating_sub(1)).0
    }

    /// An element's qualified name.
    pub fn name(&self, element: &Element) -> &'a str {
        self.text(&element.name)
    }

    /// The attribute with this qualified name, as written.
    pub fn attr<'e>(&self, element: &'e Element, name: &str) -> Option<&'e Attr> {
        element.attrs.iter().find(|a| self.text(&a.name) == name)
    }

    /// The ancestors of an element, nearest first.
    pub fn ancestors(&self, index: usize) -> impl Iterator<Item = usize> + '_ {
        std::iter::successors(self.elements[index].parent, |&i| self.elements[i].parent)
    }
}

fn to_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Splits a qualified name into its prefix and its local part.
pub fn split_prefix(qname: &str) -> (Option<&str>, &str) {
    match qname.split_once(':') {
        Some((prefix, local)) => (Some(prefix), local),
        None => (None, qname),
    }
}

/// Where a slice handed out by the reader starts in the text. The slice
/// reader borrows every name and raw value from its input, so anything else
/// is a broken assumption, reported rather than guessed around.
fn offset_in(text: &str, part: &[u8]) -> Result<usize, Error> {
    let base = text.as_ptr() as usize;
    let at = part.as_ptr() as usize;
    if at >= base && at + part.len() <= base + text.len() {
        Ok(at - base)
    } else {
        Err(error("internal: a name or value outside the source", 0))
    }
}

fn open(text: &str, tag: &BytesStart<'_>, parent: Option<usize>) -> Result<Element, Error> {
    let qname = tag.name();
    let name_at = offset_in(text, qname.as_ref())?;
    let name = name_at..name_at + qname.as_ref().len();
    let start = name_at
        .checked_sub(1)
        .filter(|&s| text.as_bytes().get(s) == Some(&b'<'))
        .ok_or_else(|| error("internal: a start tag without `<`", name_at))?;
    let mut attrs = Vec::new();
    for attr in tag.attributes() {
        let attr = attr.map_err(|e| error(e.to_string(), start))?;
        let key_at = offset_in(text, attr.key.as_ref())?;
        let value = match &attr.value {
            Cow::Borrowed(raw) => {
                let at = offset_in(text, raw)?;
                at..at + raw.len()
            }
            Cow::Owned(_) => return Err(error("internal: an attribute value was copied", key_at)),
        };
        attrs.push(Attr {
            name: key_at..key_at + attr.key.as_ref().len(),
            value,
        });
    }
    Ok(Element {
        name,
        attrs,
        parent,
        children: Vec::new(),
        start,
        end: start,
    })
}

/// Reads one file. Invalid XML (an unclosed or mismatched element, a broken
/// attribute, bytes that are not UTF-8) is an `Err`.
pub fn parse(source: &[u8]) -> Result<Doc<'_>, Error> {
    let bytes = source.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(source);
    let text = std::str::from_utf8(bytes).map_err(|e| error(e.to_string(), e.valid_up_to()))?;
    let mut line_starts = vec![0];
    line_starts.extend(
        text.bytes()
            .enumerate()
            .filter(|&(_, b)| b == b'\n')
            .map(|(i, _)| i + 1),
    );
    let mut elements: Vec<Element> = Vec::new();
    let mut open_stack: Vec<usize> = Vec::new();
    let mut reader = Reader::from_str(text);
    loop {
        let event = reader
            .read_event()
            .map_err(|e| error(e.to_string(), to_usize(reader.error_position())))?;
        match event {
            Event::Start(tag) => {
                let index = push(&mut elements, open(text, &tag, open_stack.last().copied())?);
                open_stack.push(index);
            }
            Event::Empty(tag) => {
                let index = push(&mut elements, open(text, &tag, open_stack.last().copied())?);
                elements[index].end = to_usize(reader.buffer_position());
            }
            Event::End(_) => {
                let index = open_stack.pop().ok_or_else(|| {
                    error(
                        "an end tag without a start tag",
                        to_usize(reader.buffer_position()),
                    )
                })?;
                elements[index].end = to_usize(reader.buffer_position());
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if let Some(&index) = open_stack.last() {
        return Err(error("an element is never closed", elements[index].start));
    }
    Ok(Doc {
        text,
        line_starts,
        elements,
    })
}

fn push(elements: &mut Vec<Element>, element: Element) -> usize {
    let index = elements.len();
    if let Some(parent) = element.parent {
        elements[parent].children.push(index);
    }
    elements.push(element);
    index
}

fn to_usize(n: u64) -> usize {
    usize::try_from(n).unwrap_or(usize::MAX)
}

/// A markup extension (`{Binding Path=A}`), taken apart. Offsets are byte
/// offsets into the document, so a part can be given a position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Markup<'a> {
    /// As written, prefix included (`Binding`, `x:Type`).
    pub name: &'a str,
    pub name_at: usize,
    /// Positional and named arguments, in source order.
    pub args: Vec<Arg<'a>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Arg<'a> {
    /// `Path` in `Path=A.B`; `None` for a positional argument.
    pub name: Option<&'a str>,
    pub value: ArgValue<'a>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArgValue<'a> {
    /// Text, trimmed and without its quotes; `\` escapes are left as written.
    Text { text: &'a str, at: usize },
    /// A nested extension (`{x:Type local:ViewA}`).
    Markup(Markup<'a>),
}

impl<'a> Markup<'a> {
    /// The first positional argument.
    pub fn positional(&self) -> Option<&ArgValue<'a>> {
        self.args
            .iter()
            .find(|a| a.name.is_none())
            .map(|a| &a.value)
    }

    /// The named argument `name`.
    pub fn named(&self, name: &str) -> Option<&ArgValue<'a>> {
        self.args
            .iter()
            .find(|a| a.name == Some(name))
            .map(|a| &a.value)
    }
}

/// Reads an attribute value as a markup extension. `at` is the byte offset
/// of `value` in the document. `None` for anything else: a plain value, the
/// `{}` escape that makes the rest a literal, and a malformed extension.
pub fn markup(value: &str, at: usize) -> Option<Markup<'_>> {
    let lead = value.len() - value.trim_start().len();
    let rest = &value[lead..];
    if !rest.starts_with('{') || rest.starts_with("{}") {
        return None;
    }
    let (ext, used) = extension(rest, 0, at + lead)?;
    rest[used..].trim().is_empty().then_some(ext)
}

fn skip_ws(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && b[i].is_ascii_whitespace() {
        i += 1;
    }
    i
}

/// Parses the extension that opens at `s[i]` (a `{`). Returns it and the
/// index just past its `}`.
fn extension(s: &str, i: usize, base: usize) -> Option<(Markup<'_>, usize)> {
    let b = s.as_bytes();
    let mut i = skip_ws(b, i + 1);
    let name_start = i;
    while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'}' && b[i] != b',' {
        i += 1;
    }
    if i == name_start {
        return None;
    }
    let mut ext = Markup {
        name: &s[name_start..i],
        name_at: base + name_start,
        args: Vec::new(),
    };
    i = skip_ws(b, i);
    if b.get(i) == Some(&b'}') {
        return Some((ext, i + 1));
    }
    loop {
        let (arg, next) = argument(s, i, base)?;
        ext.args.push(arg);
        i = skip_ws(b, next);
        match b.get(i)? {
            b',' => i += 1,
            b'}' => return Some((ext, i + 1)),
            _ => return None,
        }
    }
}

/// One argument starting at or after `s[i]`.
fn argument(s: &str, i: usize, base: usize) -> Option<(Arg<'_>, usize)> {
    let b = s.as_bytes();
    let i = skip_ws(b, i);
    if starts_nested(b, i) || matches!(b.get(i), Some(b'\'' | b'"')) {
        let (value, next) = value(s, i, base)?;
        return Some((Arg { name: None, value }, next));
    }
    let (start, end, next) = token(b, i, true)?;
    if b.get(next) == Some(&b'=') {
        let name = s[start..end].trim();
        if name.is_empty() {
            return None;
        }
        let (value, next) = value(s, skip_ws(b, next + 1), base)?;
        return Some((
            Arg {
                name: Some(name),
                value,
            },
            next,
        ));
    }
    Some((
        Arg {
            name: None,
            value: text(s, start, end, base),
        },
        next,
    ))
}

fn starts_nested(b: &[u8], i: usize) -> bool {
    b.get(i) == Some(&b'{') && b.get(i + 1) != Some(&b'}')
}

/// A value at `s[i]`: a nested extension, a quoted text or a bare text.
fn value(s: &str, i: usize, base: usize) -> Option<(ArgValue<'_>, usize)> {
    let b = s.as_bytes();
    if starts_nested(b, i) {
        let (ext, next) = extension(s, i, base)?;
        return Some((ArgValue::Markup(ext), next));
    }
    if let Some(&quote) = b.get(i).filter(|&&c| c == b'\'' || c == b'"') {
        let mut j = i + 1;
        while j < b.len() && b[j] != quote {
            j += if b[j] == b'\\' { 2 } else { 1 };
        }
        if j >= b.len() {
            return None;
        }
        return Some((
            ArgValue::Text {
                text: &s[i + 1..j],
                at: base + i + 1,
            },
            j + 1,
        ));
    }
    let (start, end, next) = token(b, i, false)?;
    Some((text(s, start, end, base), next))
}

/// A bare text from `b[i]` up to the `,` or `}` (or, with `stop_at_eq`, the
/// `=`) that ends it at brace depth zero. Braces inside it nest (the `{}{0}`
/// of a `StringFormat`), and `\` escapes the next character.
fn token(b: &[u8], i: usize, stop_at_eq: bool) -> Option<(usize, usize, usize)> {
    let mut depth = 0usize;
    let mut j = i;
    while j < b.len() {
        match b[j] {
            b'\\' => j += 1,
            b'{' => depth += 1,
            b'}' if depth == 0 => return Some((i, j, j)),
            b'}' => depth -= 1,
            b',' if depth == 0 => return Some((i, j, j)),
            b'=' if depth == 0 && stop_at_eq => return Some((i, j, j)),
            _ => {}
        }
        j += 1;
    }
    None
}

fn text(s: &str, start: usize, end: usize, base: usize) -> ArgValue<'_> {
    let raw = &s[start..end];
    let lead = raw.len() - raw.trim_start().len();
    ArgValue::Text {
        text: raw.trim(),
        at: base + start + lead,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of<'a>(v: &ArgValue<'a>) -> &'a str {
        match v {
            ArgValue::Text { text, .. } => text,
            ArgValue::Markup(m) => panic!("expected text, found {m:?}"),
        }
    }

    #[test]
    fn a_dictionary_with_a_bom_and_crlf_counts_lines_and_columns_without_the_bom() {
        let source = "\u{FEFF}<ResourceDictionary xmlns:x=\"urn:x\">\r\n  <!-- styles -->\r\n  <Style x:Key=\"K\">\r\n    <Setter />\r\n  </Style>\r\n</ResourceDictionary>\r\n";
        let doc = parse(source.as_bytes()).expect("valid XML");
        let root = &doc.elements[0];
        assert_eq!(doc.name(root), "ResourceDictionary");
        // The BOM shifts only line 1: the root's name is column 2 there.
        assert_eq!(doc.position(root.name.start), (1, 2));
        let style = &doc.elements[1];
        assert_eq!(doc.name(style), "Style");
        assert_eq!(doc.lines(style), (3, 5));
        assert_eq!(doc.position(style.name.start), (3, 4));
        let key = doc.attr(style, "x:Key").expect("x:Key");
        assert_eq!(doc.text(&key.value), "K");
        assert_eq!(doc.position(key.value.start), (3, 17));
        assert_eq!(doc.lines(root), (1, 6));
        assert_eq!(style.children, [2]);
        assert_eq!(doc.elements[2].parent, Some(1));
    }

    #[test]
    fn a_column_counts_characters_not_bytes() {
        let doc = parse("<a t=\"\u{17E}\u{17E}\"><b/></a>".as_bytes()).expect("valid XML");
        // `<a t="žž">` is 10 characters but 12 bytes.
        assert_eq!(doc.position(doc.elements[1].name.start), (1, 12));
    }

    #[test]
    fn attributes_keep_their_source_order() {
        let doc = parse(b"<a z=\"1\" b=\"2\" m=\"3\"/>").expect("valid XML");
        let names: Vec<&str> = doc.elements[0]
            .attrs
            .iter()
            .map(|a| doc.text(&a.name))
            .collect();
        assert_eq!(names, ["z", "b", "m"]);
    }

    #[test]
    fn a_binding_splits_into_its_named_arguments_in_source_order() {
        let value = "{Binding Path=A.B, Mode=TwoWay}";
        let ext = markup(value, 100).expect("an extension");
        assert_eq!(ext.name, "Binding");
        assert_eq!(ext.name_at, 101);
        let names: Vec<Option<&str>> = ext.args.iter().map(|a| a.name).collect();
        assert_eq!(names, [Some("Path"), Some("Mode")]);
        assert_eq!(text_of(&ext.args[0].value), "A.B");
        assert_eq!(text_of(&ext.args[1].value), "TwoWay");
        let ArgValue::Text { at, .. } = ext.args[0].value else {
            unreachable!()
        };
        assert_eq!(&value[at - 100..at - 100 + 3], "A.B");
    }

    #[test]
    fn a_nested_extension_is_two_levels() {
        let ext = markup("{StaticResource {x:Type local:ViewA}}", 0).expect("an extension");
        assert_eq!(ext.name, "StaticResource");
        let Some(ArgValue::Markup(inner)) = ext.positional() else {
            panic!("expected a nested extension, found {:?}", ext.args)
        };
        assert_eq!(inner.name, "x:Type");
        assert_eq!(inner.positional().map(text_of), Some("local:ViewA"));
        let Some(ArgValue::Text { at, .. }) = inner.positional() else {
            unreachable!()
        };
        assert_eq!(*at, 24);
    }

    #[test]
    fn positional_and_named_arguments_mix_in_source_order() {
        let ext = markup(
            "{Binding Items, Converter={StaticResource C}, StringFormat='{}{0:N2}'}",
            0,
        )
        .expect("an extension");
        assert_eq!(ext.positional().map(text_of), Some("Items"));
        let Some(ArgValue::Markup(conv)) = ext.named("Converter") else {
            panic!("a nested converter")
        };
        assert_eq!(conv.positional().map(text_of), Some("C"));
        assert_eq!(ext.named("StringFormat").map(text_of), Some("{}{0:N2}"));
        // Unquoted, the braces of the format nest inside the value.
        let bare = markup("{Binding A, StringFormat={}{0:N2}}", 0).expect("an extension");
        assert_eq!(bare.named("StringFormat").map(text_of), Some("{}{0:N2}"));
    }

    #[test]
    fn a_leading_empty_pair_of_braces_escapes_a_literal() {
        assert_eq!(markup("{}{Binding A}", 0), None);
        assert_eq!(markup("plain", 0), None);
        assert_eq!(markup("{Binding A", 0), None);
        let ext = markup("{x:Null}", 0).expect("an extension without arguments");
        assert!(ext.args.is_empty());
    }

    #[test]
    fn an_unclosed_element_is_an_error() {
        assert!(parse(b"<a><b></b>").is_err());
        assert!(parse(b"<a><b></a>").is_err());
        assert!(parse(b"<a x=\"1></a>").is_err());
        assert!(parse(b"\xFF<a/>").is_err());
    }

    #[test]
    fn two_reads_of_the_same_bytes_give_the_same_tree() {
        let source = b"<Grid xmlns:local=\"clr-namespace:Acme\"><local:ViewA x:Name=\"A\" /><TextBlock Text=\"{Binding B}\"/></Grid>";
        assert_eq!(parse(source), parse(source));
    }
}
