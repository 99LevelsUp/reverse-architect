//! Pass 1: what a XAML file declares, as contract 0.2 can say it.
//!
//! - Styles, templates and other resources in a `ResourceDictionary` or a
//!   `*.Resources` block are symbols of kind `style`, `template` and
//!   `resource`, keyed `sym:xaml:<path>#<key>`. A style or template without
//!   `x:Key` is keyed by its type: `type:<T>`, `T` qualified by the CLR
//!   namespace its prefix maps (see [`TypeRef::identity`]).
//! - An element with `x:Name` inside a template is a symbol of kind
//!   `element`, keyed `<container>/<name>` under the nearest resource around
//!   it, with that resource's name as its `symbol_container`. No `has_member`
//!   edge: a template has no card of its own on 0.2, and the file's card
//!   leaves out every `has_member` target, so the element would be on no card.
//! - The root of a file with `x:Class` is a symbol of kind `view`, keyed
//!   `sym:xaml:<path>#x:Class`. The C# class is the C# plugin's to declare,
//!   and so are the fields `x:Name` generates outside templates.
//! - A merged dictionary (`<ResourceDictionary Source="…"/>`) is an `imports`
//!   edge to the file it names, when the host confirms that path exists.
//! - `BasedOn="{StaticResource {x:Type T}}"` extends the style keyed by `T`
//!   in the same file.

use crate::parse::{markup, parse, split_prefix, ArgValue, Doc, Error};
use crate::ra::plugin::types::{
    Edge, Fact, Op, Place, Position, Relation, RelationKind, Span, Symbol, Value,
};
use crate::xmlns::{type_of_value, x_type, x_value};
use std::collections::BTreeMap;
use std::ops::Range;

/// A resource declared earlier in the file.
struct Declared {
    /// The key after `#`, with its ordinal.
    local: String,
    /// Its `symbol_name`.
    name: String,
    kind: &'static str,
}

/// The local part of an element's name (`Style`, `Window.Resources`).
fn local_name<'a>(doc: &Doc<'a>, index: usize) -> &'a str {
    split_prefix(doc.name(&doc.elements[index])).1
}

/// A style, template or resource directly inside a dictionary: its kind,
/// its key and where the key's name is written.
fn resource(doc: &Doc<'_>, index: usize) -> Option<(&'static str, String, usize)> {
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
        "style"
    } else if name.ends_with("Template") {
        "template"
    } else {
        "resource"
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
    if kind == "resource" {
        return None;
    }
    let element = &doc.elements[index];
    let value = doc
        .attr(element, "TargetType")
        .or_else(|| doc.attr(element, "DataType"))?;
    let ty = type_of_value(doc, index, &value.value)?;
    Some((kind, format!("type:{}", ty.identity()), value.value.start))
}

fn is_template(doc: &Doc<'_>, index: usize) -> bool {
    let name = local_name(doc, index);
    !name.contains('.') && name.ends_with("Template")
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

    fn symbol(
        &mut self,
        index: usize,
        local: &str,
        name: &str,
        kind: &str,
        name_at: usize,
        container: Option<&str>,
    ) {
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
        let mut facts = vec![
            text("symbol_name", name),
            text("symbol_kind", kind),
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
        self.ops.push(Op::Symbol(Symbol { key, facts, edges }));
    }
}

/// The wire operations for one file. `exists` answers the host's
/// `path-exists`, so the logic runs natively in tests.
pub fn declarations(
    path: &str,
    source: &[u8],
    exists: &dyn Fn(&str) -> bool,
) -> Result<Vec<Op>, Error> {
    let doc = parse(source)?;
    let mut b = Builder {
        path,
        doc: &doc,
        seen: BTreeMap::new(),
        ops: Vec::new(),
    };
    let mut resources: Vec<Option<Declared>> = Vec::with_capacity(doc.elements.len());
    let mut imports = Vec::new();
    let mut bases: Vec<(usize, String, Range<usize>)> = Vec::new();
    for index in 0..doc.elements.len() {
        let element = &doc.elements[index];
        let mut declared = None;
        if index == 0 {
            if let Some(class) = x_value(&doc, 0, "Class") {
                let local = b.unique("x:Class".into());
                let name = doc.text(&class).trim();
                b.symbol(0, &local, name, "view", class.start, None);
            }
        }
        if let Some((kind, key, name_at)) = resource(&doc, index) {
            let local = b.unique(key.clone());
            b.symbol(index, &local, &key, kind, name_at, None);
            declared = Some(Declared {
                local,
                name: key,
                kind,
            });
        } else if let Some(name) = x_value(&doc, index, "Name") {
            // Outside a template, `x:Name` is a field of the generated class.
            let in_template = doc.ancestors(index).any(|i| is_template(&doc, i));
            let container = doc
                .ancestors(index)
                .find_map(|i| resources.get(i).and_then(Option::as_ref));
            let simple = doc.text(&name).trim();
            if let (true, Some(container), false) = (in_template, container, simple.is_empty()) {
                let local = format!("{}/{simple}", container.local);
                let container = container.name.clone();
                let local = b.unique(local);
                b.symbol(
                    index,
                    &local,
                    simple,
                    "element",
                    name.start,
                    Some(&container),
                );
            }
        }
        let name = local_name(&doc, index);
        if name == "ResourceDictionary" {
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
        if declared.is_some() && name == "Style" {
            if let Some(based) = doc.attr(element, "BasedOn") {
                let base =
                    markup(doc.text(&based.value), based.value.start).and_then(|ext| {
                        match (split_prefix(ext.name).1, ext.positional()) {
                            ("StaticResource", Some(ArgValue::Markup(inner))) => {
                                x_type(&doc, index, inner)
                            }
                            _ => None,
                        }
                    });
                if let Some(base) = base {
                    bases.push((
                        index,
                        format!("type:{}", base.identity()),
                        based.value.clone(),
                    ));
                }
            }
        }
        resources.push(declared);
    }
    let mut ops = b.ops;
    ops.extend(imports);
    for (index, base, at) in bases {
        let Some(style) = resources[index].as_ref() else {
            continue;
        };
        // Only a base this file declares: anywhere else, the key is not
        // known on 0.2 (the base may be a theme's or the framework's).
        let declared_here = resources
            .iter()
            .flatten()
            .any(|d| d.kind == "style" && d.local == base);
        if declared_here && base != style.local {
            ops.push(Op::Relation(Relation {
                sources: vec![format!("sym:xaml:{path}#{}", style.local)],
                targets: vec![format!("sym:xaml:{path}#{base}")],
                kind: RelationKind::Extends,
                place: Place {
                    span: Span::Line(doc.position(at.start).0),
                    name: None,
                },
            }));
        }
    }
    Ok(ops)
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

    fn symbols(ops: &[Op]) -> Vec<&Symbol> {
        ops.iter()
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

    fn by_key<'s>(ops: &'s [Op], key: &str) -> &'s Symbol {
        symbols(ops)
            .into_iter()
            .find(|s| s.key == key)
            .unwrap_or_else(|| panic!("no symbol {key} in {:#?}", symbols(ops)))
    }

    fn span_and_name(symbol: &Symbol) -> ((u32, u32), (u32, u32)) {
        let place = &fact(symbol, "symbol_name").expect("symbol_name").place;
        let Span::Range(range) = place.span else {
            panic!("a line range, found {:?}", place.span)
        };
        let name = place.name.expect("a name position");
        (range, (name.line, name.column))
    }

    const PATH: &str = "Themes/Generic.xaml";

    #[test]
    fn a_dictionary_declares_its_styles_and_templates_by_key() {
        let ops = declarations(PATH, GENERIC.as_bytes(), &none).expect("valid XAML");
        let keys: Vec<&str> = symbols(&ops).iter().map(|s| s.key.as_str()).collect();
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

        let k = by_key(&ops, "sym:xaml:Themes/Generic.xaml#K");
        assert_eq!(text(k, "symbol_kind"), Some("style"));
        assert_eq!(text(k, "symbol_name"), Some("K"));
        let k_line = line_of(GENERIC, "x:Key=\"K\"");
        assert_eq!(
            span_and_name(k),
            ((k_line, k_line + 2), at(GENERIC, "K\" TargetType"))
        );

        let implicit = by_key(&ops, "sym:xaml:Themes/Generic.xaml#type:Acme.Views.ViewA");
        assert_eq!(text(implicit, "symbol_kind"), Some("style"));
        let line = line_of(GENERIC, "<Style TargetType=\"local:ViewA\">");
        assert_eq!(
            span_and_name(implicit),
            ((line, line + 2), at(GENERIC, "local:ViewA\">"))
        );

        let template = by_key(&ops, "sym:xaml:Themes/Generic.xaml#ListTemplate");
        assert_eq!(text(template, "symbol_kind"), Some("template"));
        let line = line_of(GENERIC, "<ControlTemplate");
        assert_eq!(
            span_and_name(template),
            ((line, line + 4), at(GENERIC, "ListTemplate\""))
        );

        let part = by_key(&ops, "sym:xaml:Themes/Generic.xaml#ListTemplate/PART_List");
        assert_eq!(text(part, "symbol_kind"), Some("element"));
        assert_eq!(text(part, "symbol_name"), Some("PART_List"));
        assert_eq!(text(part, "symbol_container"), Some("ListTemplate"));
        let line = line_of(GENERIC, "PART_List");
        assert_eq!(
            span_and_name(part),
            ((line, line), at(GENERIC, "PART_List"))
        );

        let data = by_key(
            &ops,
            "sym:xaml:Themes/Generic.xaml#type:Acme.Views.EditorViewModel",
        );
        assert_eq!(text(data, "symbol_kind"), Some("template"));
        let accent = by_key(&ops, "sym:xaml:Themes/Generic.xaml#Accent");
        assert_eq!(text(accent, "symbol_kind"), Some("resource"));

        // Every symbol comes with its `declares` edge from the file.
        for symbol in symbols(&ops) {
            assert_eq!(symbol.edges.len(), 1, "{}", symbol.key);
            assert_eq!(symbol.edges[0].source, PATH);
            assert_eq!(symbol.edges[0].target, symbol.key);
            assert_eq!(symbol.edges[0].kind, "declares");
        }
        // The element is a member by `symbol_container`, never by an edge.
        assert!(!ops
            .iter()
            .any(|op| matches!(op, Op::Edge(e) if e.kind == "has_member")));
        assert!(!symbols(&ops)
            .iter()
            .flat_map(|s| &s.edges)
            .any(|e| e.kind == "has_member"));
    }

    #[test]
    fn a_style_based_on_a_type_keyed_style_of_the_file_extends_it() {
        let ops = declarations(PATH, GENERIC.as_bytes(), &none).expect("valid XAML");
        let relations: Vec<&Relation> = ops
            .iter()
            .filter_map(|op| match op {
                Op::Relation(r) => Some(r),
                _ => None,
            })
            .collect();
        assert_eq!(relations.len(), 1);
        assert_eq!(
            relations[0].sources,
            ["sym:xaml:Themes/Generic.xaml#Derived"]
        );
        assert_eq!(
            relations[0].targets,
            ["sym:xaml:Themes/Generic.xaml#type:Acme.Views.ViewA"]
        );
        assert!(matches!(relations[0].kind, RelationKind::Extends));
        // A base the file does not declare is not reported.
        let lone = "<ResourceDictionary xmlns:x=\"http://schemas.microsoft.com/winfx/2006/xaml\"><Style x:Key=\"D\" BasedOn=\"{StaticResource {x:Type Button}}\"/></ResourceDictionary>";
        let ops = declarations("a.xaml", lone.as_bytes(), &none).expect("valid XAML");
        assert!(!ops.iter().any(|op| matches!(op, Op::Relation(_))));
    }

    fn imports(ops: &[Op]) -> Vec<&str> {
        ops.iter()
            .filter_map(|op| match op {
                Op::Edge(e) if e.kind == "imports" => Some(e.target.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_merged_dictionary_imports_a_path_the_host_confirms() {
        let exists = |key: &str| key == "Themes/Colors.xaml" || key == "Acme.Themes/Colors.xaml";
        let ops = declarations(PATH, GENERIC.as_bytes(), &exists).expect("valid XAML");
        assert_eq!(
            imports(&ops),
            ["Themes/Colors.xaml", "Acme.Themes/Colors.xaml"]
        );
        for op in &ops {
            if let Op::Edge(e) = op {
                assert_eq!(e.source, PATH);
            }
        }
        // Nothing the host does not confirm is reported.
        let ops = declarations(PATH, GENERIC.as_bytes(), &none).expect("valid XAML");
        assert!(imports(&ops).is_empty());
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
        let ops = declarations(path, EDITOR.as_bytes(), &none).expect("valid XAML");
        let view = by_key(&ops, "sym:xaml:Views/EditorView.xaml#x:Class");
        assert_eq!(text(view, "symbol_kind"), Some("view"));
        assert_eq!(text(view, "symbol_name"), Some("Acme.Views.EditorView"));
        assert_eq!(
            span_and_name(view),
            (
                (1, line_of(EDITOR, "</UserControl>")),
                at(EDITOR, "Acme.Views.EditorView")
            )
        );
        let keys: Vec<&str> = symbols(&ops).iter().map(|s| s.key.as_str()).collect();
        assert!(keys.iter().all(|k| k.starts_with("sym:xaml:")), "{keys:?}");
        // `x:Name` outside a template is a field of the generated class.
        assert!(!keys
            .iter()
            .any(|k| k.contains("TitleBox") || k.contains("Preview")));
    }

    #[test]
    fn one_key_in_two_resource_blocks_gets_an_ordinal() {
        let ops = declarations("V.xaml", EDITOR.as_bytes(), &none).expect("valid XAML");
        let keys: Vec<&str> = symbols(&ops).iter().map(|s| s.key.as_str()).collect();
        assert_eq!(
            keys,
            [
                "sym:xaml:V.xaml#x:Class",
                "sym:xaml:V.xaml#K",
                "sym:xaml:V.xaml#K~2"
            ]
        );
        let second = by_key(&ops, "sym:xaml:V.xaml#K~2");
        assert_eq!(text(second, "symbol_name"), Some("K"));
        let first_line = line_of(EDITOR, "<Style x:Key=\"K\"");
        assert!(span_and_name(second).0 .0 > first_line);
    }

    #[test]
    fn a_named_part_in_a_style_template_belongs_to_the_style() {
        let source = "<ResourceDictionary xmlns:x=\"http://schemas.microsoft.com/winfx/2006/xaml\" xmlns:local=\"clr-namespace:Acme.Controls\">\n<Style TargetType=\"local:Gauge\">\n<Setter Property=\"Template\">\n<Setter.Value>\n<ControlTemplate TargetType=\"local:Gauge\">\n<Border x:Name=\"PART_Track\"/>\n</ControlTemplate>\n</Setter.Value>\n</Setter>\n</Style>\n</ResourceDictionary>\n";
        let ops = declarations("G.xaml", source.as_bytes(), &none).expect("valid XAML");
        let part = by_key(&ops, "sym:xaml:G.xaml#type:Acme.Controls.Gauge/PART_Track");
        assert_eq!(
            text(part, "symbol_container"),
            Some("type:Acme.Controls.Gauge")
        );
        assert_eq!(symbols(&ops).len(), 2);
    }

    #[test]
    fn the_same_dictionary_gives_the_same_operations() {
        let exists = |key: &str| key == "Themes/Colors.xaml";
        let first = declarations(PATH, GENERIC.as_bytes(), &exists).expect("valid XAML");
        let second = declarations(PATH, GENERIC.as_bytes(), &exists).expect("valid XAML");
        assert_eq!(format!("{first:?}"), format!("{second:?}"));
    }

    #[test]
    fn invalid_xml_is_an_error() {
        assert!(declarations("x.xaml", b"<Grid><Button></Grid>", &none).is_err());
    }
}
