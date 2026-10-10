//! Pass 1: what a XAML file declares, as contract 0.3 says it.
//!
//! - Styles, templates and other resources in a `ResourceDictionary` or a
//!   `*.Resources` block are symbols of the plugin's own node kinds
//!   `xaml:style`, `xaml:template` and `xaml:resource`, keyed
//!   `sym:xaml:<path>#<key>`. A style or template without `x:Key` is keyed by
//!   its type: `type:<T>`, `T` qualified by the CLR namespace its prefix maps
//!   (see [`TypeRef::identity`]).
//! - An element with `x:Name` inside a template is a symbol of kind
//!   `xaml:element`, keyed `<container>/<name>` under the nearest resource
//!   around it, which owns it by a `has_member` edge (and names it as its
//!   `symbol_container`): a template has a card of its own on 0.3.
//! - The root of a file with `x:Class` is a symbol of kind `xaml:view`, keyed
//!   `sym:xaml:<path>#x:Class`. The C# class is the C# plugin's to declare,
//!   and so are the fields `x:Name` generates outside templates.
//! - A merged dictionary (`<ResourceDictionary Source="…"/>`) is an `imports`
//!   edge to the file it names, when the host confirms that path exists.
//! - Every `*.Resources` property element is a lookup block, in document
//!   order, with the index of the block around it; a declaration carries the
//!   innermost block it is written in.
//! - `{StaticResource K}` and `{DynamicResource K}`, at any depth of an
//!   attribute's markup, are references by name (`name-ref`) from the nearest
//!   declaration around them (or the file): to a `xaml:style` when the
//!   property they set is `BasedOn`, `Style` or ends in `Style`, to a
//!   `xaml:resource` otherwise. `BasedOn` on a declared style is a reference
//!   whose resolved edge is `extends`. A key written `{x:Type T}` is spelled
//!   `type:<T>`, like the key of a style keyed by its type.

use crate::parse::{markup, parse, split_prefix, ArgValue, Doc, Error, Markup};
use crate::ra::plugin::types::{
    Edge, Fact, FileDeclarations, LookupBlock, NameRef, Op, Place, Position, Span, Symbol, Value,
};
use crate::xmlns::{type_of_value, x_type, x_value};
use std::collections::BTreeMap;
use std::ops::Range;

pub const STYLE: &str = "xaml:style";
pub const TEMPLATE: &str = "xaml:template";
pub const RESOURCE: &str = "xaml:resource";
pub const ELEMENT: &str = "xaml:element";
pub const VIEW: &str = "xaml:view";

/// A resource declared earlier in the file.
struct Declared {
    /// The key after `#`, with its ordinal.
    local: String,
    /// Its `symbol_name`.
    name: String,
}

/// The local part of an element's name (`Style`, `Window.Resources`).
pub fn local_name<'a>(doc: &Doc<'a>, index: usize) -> &'a str {
    split_prefix(doc.name(&doc.elements[index])).1
}

/// A style, template or resource directly inside a dictionary: its node
/// kind, its key and where the key's name is written. The same function
/// gives `bodies` the name position of a style or template, so the two
/// passes cannot disagree on it.
pub fn resource(doc: &Doc<'_>, index: usize) -> Option<(&'static str, String, usize)> {
    let parent = doc.elements[index].parent?;
    let holder = local_name(doc, parent);
    let name = local_name(doc, index);
    // A dictionary written inside `*.Resources` is that block's dictionary,
    // not an entry in it; a property element (`Style.Triggers`) is no entry.
    if !(holder == "ResourceDictionary" || holder.ends_with(".Resources"))
        || name.contains('.')
        || name == "ResourceDictionary"
    {
        return None;
    }
    let kind = if name == "Style" {
        STYLE
    } else if name.ends_with("Template") {
        TEMPLATE
    } else {
        RESOURCE
    };
    if let Some(value) = x_value(doc, index, "Key") {
        let text = doc.text(&value).trim();
        if text.is_empty() {
            return None;
        }
        let key = match markup(doc.text(&value), value.start) {
            Some(ext) => match x_type(doc, index, &ext) {
                Some(ty) => format!("type:{}", ty.identity()),
                None => text.to_string(),
            },
            None => text.to_string(),
        };
        return Some((kind, key, value.start));
    }
    if kind == RESOURCE {
        return None;
    }
    let element = &doc.elements[index];
    let value = doc
        .attr(element, "TargetType")
        .or_else(|| doc.attr(element, "DataType"))?;
    let ty = type_of_value(doc, index, &value.value)?;
    Some((kind, format!("type:{}", ty.identity()), value.value.start))
}

/// The value of the root's `x:Class`: where the view's name is written.
pub fn view_class(doc: &Doc<'_>) -> Option<Range<usize>> {
    if doc.elements.is_empty() {
        return None;
    }
    x_value(doc, 0, "Class").filter(|v| !doc.text(v).trim().is_empty())
}

fn is_template(doc: &Doc<'_>, index: usize) -> bool {
    let name = local_name(doc, index);
    !name.contains('.') && name.ends_with("Template")
}

/// The file's lookup blocks (every `*.Resources` property element, in
/// document order) and, for every element, the innermost block it is
/// written in (a strict ancestor).
pub fn blocks(doc: &Doc<'_>) -> (Vec<LookupBlock>, Vec<Option<u32>>) {
    let mut list: Vec<LookupBlock> = Vec::new();
    let mut inside: Vec<Option<u32>> = Vec::with_capacity(doc.elements.len());
    // Index of the block an element is itself, if it is one.
    let mut own: Vec<Option<u32>> = Vec::with_capacity(doc.elements.len());
    for index in 0..doc.elements.len() {
        let around = doc.elements[index]
            .parent
            .and_then(|p| own[p].or(inside[p]));
        inside.push(around);
        if local_name(doc, index).ends_with(".Resources") {
            own.push(Some(u32::try_from(list.len()).unwrap_or(u32::MAX)));
            list.push(LookupBlock {
                lines: doc.lines(&doc.elements[index]),
                parent: around,
            });
        } else {
            own.push(None);
        }
    }
    (list, inside)
}

/// The header a card shows for an element: its name and the attributes
/// that identify it, as written. Built rather than quoted, so a start tag
/// carrying megabytes of geometry never ends up in a fact.
fn signature(doc: &Doc<'_>, index: usize) -> String {
    const SHOWN: [&str; 6] = ["Key", "Name", "Class", "TargetType", "DataType", "BasedOn"];
    const LONGEST: usize = 120;
    let element = &doc.elements[index];
    let mut out = format!("<{}", doc.name(element));
    for attr in &element.attrs {
        let name = doc.text(&attr.name);
        if SHOWN.contains(&split_prefix(name).1) {
            let value = doc.text(&attr.value);
            let value: String = value.chars().take(LONGEST).collect();
            out.push_str(&format!(" {name}=\"{value}\""));
        }
    }
    out.push('>');
    out
}

/// Turns a dictionary's `Source` into a repository path: relative to the
/// file's directory, or `pack://application:,,,/<Assembly>;component/<path>`
/// (and its short form `/<Assembly>;component/<path>`) read as the
/// top-level directory named like the assembly. Anything else is `None`.
pub fn source_path(file: &str, source: &str) -> Option<String> {
    let source = source.trim();
    const PACK: &str = "pack://application:,,,/";
    let pack = source
        .get(..PACK.len())
        .filter(|p| p.eq_ignore_ascii_case(PACK))
        .map(|_| &source[PACK.len()..]);
    if let Some(rest) = pack.or_else(|| source.strip_prefix('/')) {
        let (assembly, path) = rest.split_once(";component/")?;
        let assembly = assembly.split(';').next()?.trim();
        if assembly.is_empty() {
            return None;
        }
        return normalize("", &format!("{assembly}/{path}"));
    }
    if source.contains(':') || source.is_empty() {
        return None;
    }
    let dir = file.rsplit_once('/').map_or("", |(dir, _)| dir);
    normalize(dir, source)
}

/// Joins `relative` onto `dir` and resolves `.` and `..`; `None` when it
/// leaves the repository.
fn normalize(dir: &str, relative: &str) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    for part in dir.split('/').chain(relative.split(['/', '\\'])) {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            part => parts.push(part),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

struct Builder<'d, 'a> {
    path: &'d str,
    doc: &'d Doc<'a>,
    inside: &'d [Option<u32>],
    seen: BTreeMap<String, u32>,
    ops: Vec<Op>,
}

impl Builder<'_, '_> {
    /// The key once more of the same in one file gets an ordinal.
    fn unique(&mut self, local: String) -> String {
        let n = self.seen.entry(local.clone()).or_insert(0);
        *n += 1;
        if *n == 1 {
            local
        } else {
            format!("{local}~{n}")
        }
    }

    fn key(&self, local: &str) -> String {
        format!("sym:xaml:{}#{local}", self.path)
    }

    fn position(&self, offset: usize) -> Position {
        let (line, column) = self.doc.position(offset);
        Position { line, column }
    }

    /// Declares a symbol of node kind `kind` and returns its key.
    fn symbol(
        &mut self,
        index: usize,
        local: &str,
        name: &str,
        kind: &'static str,
        name_at: usize,
        container: Option<&str>,
    ) -> String {
        let lines = self.doc.lines(&self.doc.elements[index]);
        let place = Place {
            span: Span::Range(lines),
            name: Some(self.position(name_at)),
        };
        let text = |kind: &str, value: &str| Fact {
            kind: kind.into(),
            value: Value::Text(value.into()),
            place,
        };
        // `symbol_kind` stays the plain word 0.2 used; the node kind is what
        // gives the symbol its kind and card under 0.3.
        let plain = kind.strip_prefix("xaml:").unwrap_or(kind);
        let mut facts = vec![
            text("symbol_name", name),
            text("symbol_kind", plain),
            text("symbol_signature", &signature(self.doc, index)),
        ];
        if let Some(container) = container {
            facts.push(text("symbol_container", container));
        }
        let key = self.key(local);
        // A file declares what it holds. The host never infers this:
        // without it the file reads as one that declares nothing.
        let edges = vec![Edge {
            source: self.path.into(),
            target: key.clone(),
            kind: "declares".into(),
            place: Place {
                span: Span::Range(lines),
                name: None,
            },
        }];
        self.ops.push(Op::Symbol(Symbol {
            key: key.clone(),
            facts,
            edges,
            node_kind: Some(kind.into()),
            block: self.inside[index],
        }));
        key
    }
}

/// The wire record for one file. `exists` answers the host's `path-exists`,
/// so the logic runs natively in tests.
pub fn declarations(
    path: &str,
    source: &[u8],
    exists: &dyn Fn(&str) -> bool,
) -> Result<FileDeclarations, Error> {
    let doc = parse(source)?;
    let (blocks, inside) = blocks(&doc);
    let mut b = Builder {
        path,
        doc: &doc,
        inside: &inside,
        seen: BTreeMap::new(),
        ops: Vec::new(),
    };
    let mut resources: Vec<Option<Declared>> = Vec::with_capacity(doc.elements.len());
    // The key and node kind of the symbol each element declares, if any.
    let mut symbols: Vec<Option<(String, &'static str)>> = Vec::with_capacity(doc.elements.len());
    let mut imports = Vec::new();
    let mut members = Vec::new();
    for index in 0..doc.elements.len() {
        let element = &doc.elements[index];
        let mut declared = None;
        let mut symbol = None;
        if index == 0 {
            if let Some(class) = view_class(&doc) {
                let local = b.unique("x:Class".into());
                let name = doc.text(&class).trim();
                let key = b.symbol(0, &local, name, VIEW, class.start, None);
                symbol = Some((key, VIEW));
            }
        }
        if let Some((kind, key, name_at)) = resource(&doc, index) {
            let local = b.unique(key.clone());
            let full = b.symbol(index, &local, &key, kind, name_at, None);
            symbol = Some((full, kind));
            declared = Some(Declared { local, name: key });
        } else if let Some(name) = x_value(&doc, index, "Name") {
            // Outside a template, `x:Name` is a field of the generated class.
            let in_template = doc.ancestors(index).any(|i| is_template(&doc, i));
            let container = doc
                .ancestors(index)
                .find_map(|i| resources.get(i).and_then(Option::as_ref));
            let simple = doc.text(&name).trim();
            if let (true, Some(container), false) = (in_template, container, simple.is_empty()) {
                let local = format!("{}/{simple}", container.local);
                let owner = b.key(&container.local);
                let container = container.name.clone();
                let local = b.unique(local);
                let key = b.symbol(index, &local, simple, ELEMENT, name.start, Some(&container));
                members.push(Op::Edge(Edge {
                    source: owner,
                    target: key.clone(),
                    kind: "has_member".into(),
                    place: Place {
                        span: Span::Range(doc.lines(element)),
                        name: None,
                    },
                }));
                symbol = Some((key, ELEMENT));
            }
        }
        if local_name(&doc, index) == "ResourceDictionary" {
            if let Some(source) = doc.attr(element, "Source") {
                let target = source_path(path, doc.text(&source.value));
                if let Some(target) = target.filter(|t| exists(t)) {
                    imports.push(Op::Edge(Edge {
                        source: path.into(),
                        target,
                        kind: "imports".into(),
                        place: Place {
                            span: Span::Range(doc.lines(element)),
                            name: None,
                        },
                    }));
                }
            }
        }
        resources.push(declared);
        symbols.push(symbol);
    }
    let mut ops = b.ops;
    // Edges after every symbol: an edge to a missing node is dropped.
    ops.extend(members);
    ops.extend(imports);
    let name_refs = name_refs(path, &doc, &symbols, &inside);
    Ok(FileDeclarations {
        ops,
        name_refs,
        blocks,
    })
}

/// What a `{StaticResource}` or `{DynamicResource}` that sets `property`
/// can resolve to.
fn target_kind(property: &str) -> &'static str {
    if property == "BasedOn" || property.ends_with("Style") {
        STYLE
    } else {
        RESOURCE
    }
}

/// The property an attribute sets: its local name after the last `.`
/// (`Style`, `Grid.Row` → `Row`); for a `Setter`'s `Value`, the setter's
/// `Property`.
pub fn property_set<'a>(doc: &Doc<'a>, index: usize, attr_name: &'a str) -> &'a str {
    let local = split_prefix(attr_name).1;
    let local = local.rsplit('.').next().unwrap_or(local);
    if local == "Value" && local_name(doc, index) == "Setter" {
        if let Some(property) = doc.attr(&doc.elements[index], "Property") {
            let property = doc.text(&property.value).trim();
            let property = split_prefix(property).1;
            return property.rsplit('.').next().unwrap_or(property);
        }
    }
    local
}

/// The key a `{StaticResource}` or `{DynamicResource}` names and where it is
/// written; `None` for any other extension and for a key the plugin cannot
/// spell (`{x:Static …}`, a `ComponentResourceKey`).
pub fn resource_ref<'a>(doc: &Doc<'a>, index: usize, ext: &Markup<'a>) -> Option<(String, usize)> {
    let name = split_prefix(ext.name).1;
    if name != "StaticResource" && name != "DynamicResource" {
        return None;
    }
    match ext.positional().or_else(|| ext.named("ResourceKey"))? {
        ArgValue::Text { text, at } => (!text.is_empty()).then(|| (text.to_string(), *at)),
        ArgValue::Markup(inner) => {
            let ty = x_type(doc, index, inner)?;
            Some((format!("type:{}", ty.identity()), ty.name_at))
        }
    }
}

/// Every reference by name in the file, in document order.
fn name_refs(
    path: &str,
    doc: &Doc<'_>,
    symbols: &[Option<(String, &'static str)>],
    inside: &[Option<u32>],
) -> Vec<NameRef> {
    let mut out = Vec::new();
    for index in 0..doc.elements.len() {
        // The nearest declaration around the reference, the element itself
        // included; the file when there is none.
        let source = std::iter::once(index)
            .chain(doc.ancestors(index))
            .find_map(|i| symbols[i].as_ref().map(|(k, _)| k.clone()))
            .unwrap_or_else(|| path.to_string());
        let is_style = matches!(&symbols[index], Some((_, kind)) if *kind == STYLE);
        for attr in &doc.elements[index].attrs {
            let attr_name = doc.text(&attr.name);
            if attr_name == "xmlns" || attr_name.starts_with("xmlns:") {
                continue;
            }
            let Some(ext) = markup(doc.text(&attr.value), attr.value.start) else {
                continue;
            };
            let property = property_set(doc, index, attr_name);
            // Only an extension that is the attribute's whole value is the
            // declared style's base.
            let extends =
                is_style && attr_name == "BasedOn" && resource_ref(doc, index, &ext).is_some();
            let mut found = Vec::new();
            collect_refs(doc, index, &ext, property, &mut found);
            for (key, at, property) in found {
                out.push(NameRef {
                    source: source.clone(),
                    name: key,
                    target_kind: target_kind(property).into(),
                    relation: extends.then(|| "extends".to_string()),
                    block: inside[index],
                    place: Place {
                        span: Span::Line(doc.position(at).0),
                        name: None,
                    },
                });
            }
        }
    }
    out
}

/// The resource references in `ext` and the extensions nested in it, each
/// with the property it sets: the attribute's for the outermost, the named
/// argument's for a nested one (`Converter={StaticResource C}`).
fn collect_refs<'a>(
    doc: &Doc<'a>,
    index: usize,
    ext: &Markup<'a>,
    property: &'a str,
    out: &mut Vec<(String, usize, &'a str)>,
) {
    if let Some((key, at)) = resource_ref(doc, index, ext) {
        out.push((key, at, property));
        return;
    }
    for arg in &ext.args {
        if let ArgValue::Markup(inner) = &arg.value {
            collect_refs(doc, index, inner, arg.name.unwrap_or(property), out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GENERIC: &str = include_str!("testdata/Generic.xaml");
    const EDITOR: &str = include_str!("testdata/EditorView.xaml");

    fn none(_: &str) -> bool {
        false
    }

    /// Line and column (1-based, in characters) of the first `needle`.
    fn at(source: &str, needle: &str) -> (u32, u32) {
        let offset = source.find(needle).expect("needle in the fixture");
        let before = &source[..offset];
        let line = before.matches('\n').count() + 1;
        let start = before.rfind('\n').map_or(0, |i| i + 1);
        let column = before[start..].chars().count() + 1;
        (line as u32, column as u32)
    }

    fn line_of(source: &str, needle: &str) -> u32 {
        at(source, needle).0
    }

    fn symbols(file: &FileDeclarations) -> Vec<&Symbol> {
        file.ops
            .iter()
            .filter_map(|op| match op {
                Op::Symbol(s) => Some(s),
                _ => None,
            })
            .collect()
    }

    fn fact<'s>(symbol: &'s Symbol, kind: &str) -> Option<&'s Fact> {
        symbol.facts.iter().find(|f| f.kind == kind)
    }

    fn text<'s>(symbol: &'s Symbol, kind: &str) -> Option<&'s str> {
        match &fact(symbol, kind)?.value {
            Value::Text(t) => Some(t),
            _ => None,
        }
    }

    fn by_key<'s>(file: &'s FileDeclarations, key: &str) -> &'s Symbol {
        symbols(file)
            .into_iter()
            .find(|s| s.key == key)
            .unwrap_or_else(|| panic!("no symbol {key} in {:#?}", symbols(file)))
    }

    fn span_and_name(symbol: &Symbol) -> ((u32, u32), (u32, u32)) {
        let place = &fact(symbol, "symbol_name").expect("symbol_name").place;
        let Span::Range(range) = place.span else {
            panic!("a line range, found {:?}", place.span)
        };
        let name = place.name.expect("a name position");
        (range, (name.line, name.column))
    }

    fn edges<'f>(file: &'f FileDeclarations, kind: &str) -> Vec<(&'f str, &'f str)> {
        file.ops
            .iter()
            .filter_map(|op| match op {
                Op::Edge(e) if e.kind == kind => Some((e.source.as_str(), e.target.as_str())),
                _ => None,
            })
            .collect()
    }

    const PATH: &str = "Themes/Generic.xaml";

    #[test]
    fn a_dictionary_declares_its_styles_and_templates_by_key() {
        let file = declarations(PATH, GENERIC.as_bytes(), &none).expect("valid XAML");
        let keys: Vec<&str> = symbols(&file).iter().map(|s| s.key.as_str()).collect();
        assert_eq!(
            keys,
            [
                "sym:xaml:Themes/Generic.xaml#K",
                "sym:xaml:Themes/Generic.xaml#type:Acme.Views.ViewA",
                "sym:xaml:Themes/Generic.xaml#ListTemplate",
                "sym:xaml:Themes/Generic.xaml#ListTemplate/PART_List",
                "sym:xaml:Themes/Generic.xaml#type:Acme.Views.EditorViewModel",
                "sym:xaml:Themes/Generic.xaml#Derived",
                "sym:xaml:Themes/Generic.xaml#Accent",
            ]
        );

        let k = by_key(&file, "sym:xaml:Themes/Generic.xaml#K");
        assert_eq!(k.node_kind.as_deref(), Some("xaml:style"));
        assert_eq!(text(k, "symbol_kind"), Some("style"));
        assert_eq!(text(k, "symbol_name"), Some("K"));
        let k_line = line_of(GENERIC, "x:Key=\"K\"");
        assert_eq!(
            span_and_name(k),
            ((k_line, k_line + 2), at(GENERIC, "K\" TargetType"))
        );

        let implicit = by_key(&file, "sym:xaml:Themes/Generic.xaml#type:Acme.Views.ViewA");
        assert_eq!(implicit.node_kind.as_deref(), Some("xaml:style"));
        let line = line_of(GENERIC, "<Style TargetType=\"local:ViewA\">");
        assert_eq!(
            span_and_name(implicit),
            ((line, line + 2), at(GENERIC, "local:ViewA\">"))
        );

        let template = by_key(&file, "sym:xaml:Themes/Generic.xaml#ListTemplate");
        assert_eq!(template.node_kind.as_deref(), Some("xaml:template"));
        let line = line_of(GENERIC, "<ControlTemplate");
        assert_eq!(
            span_and_name(template),
            ((line, line + 4), at(GENERIC, "ListTemplate\""))
        );

        let part = by_key(&file, "sym:xaml:Themes/Generic.xaml#ListTemplate/PART_List");
        assert_eq!(part.node_kind.as_deref(), Some("xaml:element"));
        assert_eq!(text(part, "symbol_name"), Some("PART_List"));
        assert_eq!(text(part, "symbol_container"), Some("ListTemplate"));
        let line = line_of(GENERIC, "PART_List");
        assert_eq!(
            span_and_name(part),
            ((line, line), at(GENERIC, "PART_List"))
        );

        let data = by_key(
            &file,
            "sym:xaml:Themes/Generic.xaml#type:Acme.Views.EditorViewModel",
        );
        assert_eq!(data.node_kind.as_deref(), Some("xaml:template"));
        let accent = by_key(&file, "sym:xaml:Themes/Generic.xaml#Accent");
        assert_eq!(accent.node_kind.as_deref(), Some("xaml:resource"));

        // Every symbol comes with its `declares` edge from the file, and
        // none of a dictionary's is inside a `*.Resources` block.
        for symbol in symbols(&file) {
            assert_eq!(symbol.edges.len(), 1, "{}", symbol.key);
            assert_eq!(symbol.edges[0].source, PATH);
            assert_eq!(symbol.edges[0].target, symbol.key);
            assert_eq!(symbol.edges[0].kind, "declares");
            assert_eq!(symbol.block, None);
        }
        assert!(file.blocks.is_empty());
    }

    #[test]
    fn a_named_template_part_is_a_member_of_its_template() {
        let file = declarations(PATH, GENERIC.as_bytes(), &none).expect("valid XAML");
        assert_eq!(
            edges(&file, "has_member"),
            [(
                "sym:xaml:Themes/Generic.xaml#ListTemplate",
                "sym:xaml:Themes/Generic.xaml#ListTemplate/PART_List"
            )]
        );
        // The edge comes after both symbols.
        let edge_at = file
            .ops
            .iter()
            .position(|op| matches!(op, Op::Edge(e) if e.kind == "has_member"))
            .expect("the edge");
        let before = file.ops[..edge_at]
            .iter()
            .filter(|op| matches!(op, Op::Symbol(_)))
            .count();
        assert_eq!(before, symbols(&file).len());
    }

    #[test]
    fn based_on_is_a_reference_by_name_that_extends_and_a_static_resource_a_plain_one() {
        let file = declarations(PATH, GENERIC.as_bytes(), &none).expect("valid XAML");
        // No relation any more: the base is a name the host resolves.
        assert!(!file.ops.iter().any(|op| matches!(op, Op::Relation(_))));
        type Seen<'s> = (&'s str, &'s str, &'s str, Option<&'s str>, Option<u32>);
        let refs: Vec<Seen> = file
            .name_refs
            .iter()
            .map(|r| {
                (
                    r.source.as_str(),
                    r.name.as_str(),
                    r.target_kind.as_str(),
                    r.relation.as_deref(),
                    r.block,
                )
            })
            .collect();
        assert_eq!(
            refs,
            [
                (
                    "sym:xaml:Themes/Generic.xaml#type:Acme.Views.ViewA",
                    "Accent",
                    "xaml:resource",
                    None,
                    None
                ),
                (
                    "sym:xaml:Themes/Generic.xaml#Derived",
                    "type:Acme.Views.ViewA",
                    "xaml:style",
                    Some("extends"),
                    None
                ),
            ]
        );
        assert_eq!(
            file.name_refs[0].place,
            Place {
                span: Span::Line(line_of(GENERIC, "{StaticResource Accent}")),
                name: None
            }
        );
        // A base the file does not declare is a reference all the same.
        let lone = "<ResourceDictionary xmlns:x=\"http://schemas.microsoft.com/winfx/2006/xaml\"><Style x:Key=\"D\" BasedOn=\"{StaticResource {x:Type Button}}\"/></ResourceDictionary>";
        let file = declarations("a.xaml", lone.as_bytes(), &none).expect("valid XAML");
        assert_eq!(file.name_refs.len(), 1);
        assert_eq!(file.name_refs[0].name, "type:Button");
        assert_eq!(file.name_refs[0].relation.as_deref(), Some("extends"));
    }

    #[test]
    fn a_static_resource_names_its_key_with_a_place_and_a_target_kind_by_property() {
        let source = "<Grid xmlns:x=\"http://schemas.microsoft.com/winfx/2006/xaml\">\n<Button Style=\"{StaticResource K}\" ItemContainerStyle=\"{DynamicResource Row}\"\n Content=\"{Binding Title, Converter={StaticResource Upper}}\"/>\n<Style><Setter Property=\"Template\" Value=\"{StaticResource T}\"/><Setter Property=\"FocusVisualStyle\" Value=\"{StaticResource F}\"/><Setter Property=\"Style\" BasedOn=\"{StaticResource B}\"/></Style>\n</Grid>\n";
        let file = declarations("G.xaml", source.as_bytes(), &none).expect("valid XAML");
        let refs: Vec<(&str, &str, Option<&str>, &str, Span)> = file
            .name_refs
            .iter()
            .map(|r| {
                (
                    r.name.as_str(),
                    r.target_kind.as_str(),
                    r.relation.as_deref(),
                    r.source.as_str(),
                    r.place.span,
                )
            })
            .collect();
        assert_eq!(
            refs,
            [
                ("K", "xaml:style", None, "G.xaml", Span::Line(2)),
                ("Row", "xaml:style", None, "G.xaml", Span::Line(2)),
                ("Upper", "xaml:resource", None, "G.xaml", Span::Line(3)),
                ("T", "xaml:resource", None, "G.xaml", Span::Line(4)),
                ("F", "xaml:style", None, "G.xaml", Span::Line(4)),
                // `BasedOn` on something that is not a declared style is
                // a plain reference.
                ("B", "xaml:style", None, "G.xaml", Span::Line(4)),
            ]
        );
    }

    #[test]
    fn two_nested_resource_blocks_are_listed_and_indexed() {
        let source = "<UserControl x:Class=\"Acme.Views.ViewB\" xmlns:x=\"http://schemas.microsoft.com/winfx/2006/xaml\">\n  <UserControl.Resources>\n    <Style x:Key=\"Outer\"/>\n    <DataTemplate x:Key=\"Row\">\n      <Grid>\n        <Grid.Resources>\n          <SolidColorBrush x:Key=\"Ink\"/>\n          <Style x:Key=\"Inner\" BasedOn=\"{StaticResource Outer}\"/>\n        </Grid.Resources>\n        <TextBlock Foreground=\"{StaticResource Ink}\"/>\n      </Grid>\n    </DataTemplate>\n  </UserControl.Resources>\n  <Border Background=\"{DynamicResource Ink}\"/>\n</UserControl>\n";
        let file = declarations("V.xaml", source.as_bytes(), &none).expect("valid XAML");
        assert_eq!(
            file.blocks,
            [
                LookupBlock {
                    lines: (2, 13),
                    parent: None
                },
                LookupBlock {
                    lines: (6, 9),
                    parent: Some(0)
                },
            ]
        );
        let blocks: Vec<(&str, Option<u32>)> = symbols(&file)
            .iter()
            .map(|s| (s.key.as_str(), s.block))
            .collect();
        assert_eq!(
            blocks,
            [
                ("sym:xaml:V.xaml#x:Class", None),
                ("sym:xaml:V.xaml#Outer", Some(0)),
                ("sym:xaml:V.xaml#Row", Some(0)),
                ("sym:xaml:V.xaml#Ink", Some(1)),
                ("sym:xaml:V.xaml#Inner", Some(1)),
            ]
        );
        let refs: Vec<(&str, &str, Option<u32>, Option<&str>)> = file
            .name_refs
            .iter()
            .map(|r| {
                (
                    r.name.as_str(),
                    r.source.as_str(),
                    r.block,
                    r.relation.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            refs,
            [
                ("Outer", "sym:xaml:V.xaml#Inner", Some(1), Some("extends")),
                ("Ink", "sym:xaml:V.xaml#Row", Some(0), None),
                ("Ink", "sym:xaml:V.xaml#x:Class", None, None),
            ]
        );
    }

    #[test]
    fn a_merged_dictionary_imports_a_path_the_host_confirms() {
        let exists = |key: &str| key == "Themes/Colors.xaml" || key == "Acme.Themes/Colors.xaml";
        let file = declarations(PATH, GENERIC.as_bytes(), &exists).expect("valid XAML");
        assert_eq!(
            edges(&file, "imports")
                .iter()
                .map(|e| e.1)
                .collect::<Vec<_>>(),
            ["Themes/Colors.xaml", "Acme.Themes/Colors.xaml"]
        );
        for (source, _) in edges(&file, "imports") {
            assert_eq!(source, PATH);
        }
        // Nothing the host does not confirm is reported.
        let file = declarations(PATH, GENERIC.as_bytes(), &none).expect("valid XAML");
        assert!(edges(&file, "imports").is_empty());
    }

    #[test]
    fn a_pack_uri_reads_as_the_assembly_directory() {
        assert_eq!(
            source_path(
                PATH,
                "pack://application:,,,/Acme.Themes;component/Colors.xaml"
            )
            .as_deref(),
            Some("Acme.Themes/Colors.xaml")
        );
        assert_eq!(
            source_path(PATH, "/Acme.Themes;v1.0.0.0;component/Brushes/Ink.xaml").as_deref(),
            Some("Acme.Themes/Brushes/Ink.xaml")
        );
        assert_eq!(
            source_path(PATH, "../Shared/Colors.xaml").as_deref(),
            Some("Shared/Colors.xaml")
        );
        assert_eq!(
            source_path(PATH, "Colors.xaml").as_deref(),
            Some("Themes/Colors.xaml")
        );
        assert_eq!(source_path(PATH, "../../Colors.xaml"), None);
        assert_eq!(source_path(PATH, "/Colors.xaml"), None);
        assert_eq!(
            source_path(PATH, "pack://siteoforigin:,,,/Colors.xaml"),
            None
        );
    }

    #[test]
    fn a_file_with_x_class_declares_its_view_and_no_csharp_key() {
        let path = "Views/EditorView.xaml";
        let file = declarations(path, EDITOR.as_bytes(), &none).expect("valid XAML");
        let view = by_key(&file, "sym:xaml:Views/EditorView.xaml#x:Class");
        assert_eq!(view.node_kind.as_deref(), Some("xaml:view"));
        assert_eq!(text(view, "symbol_kind"), Some("view"));
        assert_eq!(text(view, "symbol_name"), Some("Acme.Views.EditorView"));
        assert_eq!(
            span_and_name(view),
            (
                (1, line_of(EDITOR, "</UserControl>")),
                at(EDITOR, "Acme.Views.EditorView")
            )
        );
        let keys: Vec<&str> = symbols(&file).iter().map(|s| s.key.as_str()).collect();
        assert!(keys.iter().all(|k| k.starts_with("sym:xaml:")), "{keys:?}");
        // `x:Name` outside a template is a field of the generated class.
        assert!(!keys
            .iter()
            .any(|k| k.contains("TitleBox") || k.contains("Preview")));
    }

    #[test]
    fn one_key_in_two_resource_blocks_gets_an_ordinal() {
        let file = declarations("V.xaml", EDITOR.as_bytes(), &none).expect("valid XAML");
        let keys: Vec<(&str, Option<u32>)> = symbols(&file)
            .iter()
            .map(|s| (s.key.as_str(), s.block))
            .collect();
        assert_eq!(
            keys,
            [
                ("sym:xaml:V.xaml#x:Class", None),
                ("sym:xaml:V.xaml#K", Some(0)),
                ("sym:xaml:V.xaml#K~2", Some(1))
            ]
        );
        // Sibling blocks: neither is inside the other.
        assert!(file.blocks.iter().all(|b| b.parent.is_none()));
        let second = by_key(&file, "sym:xaml:V.xaml#K~2");
        assert_eq!(text(second, "symbol_name"), Some("K"));
        let first_line = line_of(EDITOR, "<Style x:Key=\"K\"");
        assert!(span_and_name(second).0 .0 > first_line);
    }

    #[test]
    fn a_named_part_in_a_style_template_belongs_to_the_style() {
        let source = "<ResourceDictionary xmlns:x=\"http://schemas.microsoft.com/winfx/2006/xaml\" xmlns:local=\"clr-namespace:Acme.Controls\">\n<Style TargetType=\"local:Gauge\">\n<Setter Property=\"Template\">\n<Setter.Value>\n<ControlTemplate TargetType=\"local:Gauge\">\n<Border x:Name=\"PART_Track\"/>\n</ControlTemplate>\n</Setter.Value>\n</Setter>\n</Style>\n</ResourceDictionary>\n";
        let file = declarations("G.xaml", source.as_bytes(), &none).expect("valid XAML");
        let part = by_key(&file, "sym:xaml:G.xaml#type:Acme.Controls.Gauge/PART_Track");
        assert_eq!(
            text(part, "symbol_container"),
            Some("type:Acme.Controls.Gauge")
        );
        assert_eq!(symbols(&file).len(), 2);
        assert_eq!(
            edges(&file, "has_member"),
            [(
                "sym:xaml:G.xaml#type:Acme.Controls.Gauge",
                "sym:xaml:G.xaml#type:Acme.Controls.Gauge/PART_Track"
            )]
        );
    }

    #[test]
    fn the_same_dictionary_gives_the_same_record() {
        let exists = |key: &str| key == "Themes/Colors.xaml";
        let first = declarations(PATH, GENERIC.as_bytes(), &exists).expect("valid XAML");
        let second = declarations(PATH, GENERIC.as_bytes(), &exists).expect("valid XAML");
        assert_eq!(first, second);
    }

    #[test]
    fn invalid_xml_is_an_error() {
        assert!(declarations("x.xaml", b"<Grid><Button></Grid>", &none).is_err());
    }
}
