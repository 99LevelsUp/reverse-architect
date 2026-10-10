//! Pass 2: the types a XAML file names, with the one scope it has.
//!
//! Reported, as `reference` usages: the type of `TargetType` and `DataType`,
//! of every `{x:Type T}`, of the type part of every `{x:Static T.M}`, and of
//! every element written with a prefix (`<local:ViewA>`; a property element
//! `<local:ViewA.Header>` names `ViewA`). Elements of the XAML language
//! namespace (`x:Array`) are not types.
//!
//! The file is scope 0. Its usings are the CLR namespaces the file maps to
//! prefixes (`clr-namespace:N`, `using:N`), so the host's namespace rule
//! sees what a C# file with those usings would see. A site under such a
//! prefix names its type by its simple name in that scope, with no
//! qualifier: the prefix is not written in C# terms, and what precedes the
//! name of a nested type (`local:Outer+Inner`) is its outer type, which is
//! the qualifier then. A prefix mapped to a URI (an `XmlnsDefinition`, the
//! default WPF namespace) does not say which namespace it means, so its site
//! carries no context, and the namespace rule cannot dispute the right type.
//!
//! Not reported on 0.2: bindings, commands, event handlers, setters and
//! literals.

use crate::parse::{markup, parse, split_prefix, ArgValue, Doc, Error, Markup};
use crate::ra::plugin::types::{FileUsages, Scope, Usage};
use crate::xmlns::{clr_usings, is_xaml, type_of_value, x_type, TypeRef};

/// Every type one file names, in source order.
pub fn usages(source: &[u8]) -> Result<FileUsages, Error> {
    let doc = parse(source)?;
    let mut out = Vec::new();
    for index in 0..doc.elements.len() {
        let element = &doc.elements[index];
        let qname = doc.name(element);
        if let (Some(prefix), local) = split_prefix(qname) {
            if !is_xaml(&doc, index, Some(prefix)) {
                let owner = local.split('.').next().unwrap_or(local);
                let at = element.name.start;
                if let Some(ty) =
                    TypeRef::read(&doc, index, &qname[..prefix.len() + 1 + owner.len()], at)
                {
                    out.push(ty.usage(&doc));
                }
            }
        }
        for attr in &element.attrs {
            let name = doc.text(&attr.name);
            if name == "xmlns" || name.starts_with("xmlns:") {
                continue;
            }
            let value = doc.text(&attr.value);
            if let Some(ext) = markup(value, attr.value.start) {
                extensions(&doc, index, &ext, &mut out);
            } else if name == "TargetType" || name == "DataType" {
                if let Some(ty) = type_of_value(&doc, index, &attr.value) {
                    out.push(ty.usage(&doc));
                }
            }
        }
    }
    let scopes = vec![Scope {
        namespace: String::new(),
        lines: (1, doc.last_line()),
        parent: None,
        usings: clr_usings(&doc),
    }];
    Ok(FileUsages {
        scopes,
        usages: out,
    })
}

/// The types an extension and the extensions nested in it name.
fn extensions(doc: &Doc<'_>, index: usize, ext: &Markup<'_>, out: &mut Vec<Usage>) {
    if let Some(ty) = x_type(doc, index, ext) {
        out.push(ty.usage(doc));
    } else if let Some(ty) = x_static(doc, index, ext) {
        out.push(ty.usage(doc));
    }
    for arg in &ext.args {
        if let ArgValue::Markup(inner) = &arg.value {
            extensions(doc, index, inner, out);
        }
    }
}

/// The type of `{x:Static T.M}` (or `Member=T.M`).
fn x_static<'a>(doc: &Doc<'a>, index: usize, ext: &Markup<'a>) -> Option<TypeRef<'a>> {
    let (prefix, name) = split_prefix(ext.name);
    if name != "Static" || !is_xaml(doc, index, prefix) {
        return None;
    }
    let ArgValue::Text { text, at } = ext.positional().or_else(|| ext.named("Member"))? else {
        return None;
    };
    let dot = text.rfind('.')?;
    TypeRef::read(doc, index, &text[..dot], *at)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ra::plugin::types::{Receiver, UsageKind};

    const GENERIC: &str = include_str!("testdata/Generic.xaml");

    #[test]
    fn a_clr_prefixed_target_type_is_a_reference_in_a_scope_using_its_namespace() {
        let source = "<ResourceDictionary xmlns:local=\"clr-namespace:Acme.Views;assembly=Acme\">\n  <Style TargetType=\"local:ViewA\" />\n</ResourceDictionary>\n";
        let file = usages(source.as_bytes()).expect("valid XAML");
        assert_eq!(file.scopes.len(), 1);
        assert_eq!(file.scopes[0].namespace, "");
        assert_eq!(file.scopes[0].lines, (1, 3));
        assert_eq!(file.scopes[0].parent, None);
        let usings: Vec<&str> = file.scopes[0]
            .usings
            .iter()
            .map(|u| u.target.as_str())
            .collect();
        assert_eq!(usings, ["Acme.Views"]);
        let using = &file.scopes[0].usings[0];
        assert!(using.alias.is_none() && !using.is_static && !using.is_global);
        assert_eq!(file.usages.len(), 1);
        let site = &file.usages[0];
        assert_eq!(site.name, "ViewA");
        assert!(matches!(site.kind, UsageKind::Reference));
        assert_eq!((site.line, site.column), (2, Some(28)));
        let context = site
            .context
            .as_ref()
            .expect("context under a clr-namespace prefix");
        assert_eq!(context.scope, 0);
        assert!(matches!(context.receiver, Receiver::None));
        assert_eq!(context.qualifier, None);
        assert!(context.declared_type.is_none());
        assert!(context.argument_count.is_none());
    }

    #[test]
    fn a_dictionary_names_its_types_in_source_order() {
        let file = usages(GENERIC.as_bytes()).expect("valid XAML");
        let names: Vec<(&str, bool)> = file
            .usages
            .iter()
            .map(|u| (u.name.as_str(), u.context.is_some()))
            .collect();
        assert_eq!(
            names,
            [
                ("Button", false),
                ("ViewA", true),
                ("ViewA", true),
                ("EditorViewModel", true),
                ("Palette", true),
                ("ViewA", true),
                ("ViewA", true),
            ]
        );
        // Under the default (URI) namespace the file does not say which
        // namespace `Button` is in.
        assert!(file.usages[0].context.is_none());
    }

    #[test]
    fn a_prefixed_element_and_a_property_element_name_their_type() {
        let source = b"<Grid xmlns:x=\"http://schemas.microsoft.com/winfx/2006/xaml\" xmlns:v=\"clr-namespace:Acme.Views\" xmlns:c=\"http://acme/controls\">\n<v:ViewA><v:ViewA.Header/></v:ViewA>\n<c:Gauge/>\n<x:Array Type=\"v:Item\"/>\n</Grid>";
        let file = usages(source).expect("valid XAML");
        let sites: Vec<(&str, u32, Option<u32>, bool)> = file
            .usages
            .iter()
            .map(|u| (u.name.as_str(), u.line, u.column, u.context.is_some()))
            .collect();
        assert_eq!(
            sites,
            [
                ("ViewA", 2, Some(4), true),
                ("ViewA", 2, Some(13), true),
                ("Gauge", 3, Some(4), false),
            ]
        );
    }

    #[test]
    fn a_nested_type_spelled_with_plus_is_qualified_by_its_outer_type() {
        let source = b"<a xmlns:x=\"http://schemas.microsoft.com/winfx/2006/xaml\" xmlns:v=\"clr-namespace:Acme\" T=\"{x:Type v:Outer+Inner}\" S=\"{x:Static v:Keys.Accent}\"/>";
        let file = usages(source).expect("valid XAML");
        let sites: Vec<(&str, Option<&str>)> = file
            .usages
            .iter()
            .map(|u| {
                (
                    u.name.as_str(),
                    u.context.as_ref().and_then(|c| c.qualifier.as_deref()),
                )
            })
            .collect();
        assert_eq!(sites, [("Inner", Some("Outer")), ("Keys", None)]);
    }

    #[test]
    fn every_clr_namespace_of_the_file_is_a_using_once() {
        let source = b"<a xmlns:v=\"clr-namespace:Acme.Views\" xmlns:w=\"using:Acme.Other\" xmlns:u=\"http://p\"><b xmlns:z=\"clr-namespace:Acme.Views;assembly=Acme\" xmlns:g=\"clr-namespace:;assembly=Acme\"/></a>";
        let file = usages(source).expect("valid XAML");
        let usings: Vec<&str> = file.scopes[0]
            .usings
            .iter()
            .map(|u| u.target.as_str())
            .collect();
        assert_eq!(usings, ["Acme.Views", "Acme.Other"]);
    }

    #[test]
    fn bindings_commands_and_handlers_are_not_reported() {
        let source = b"<Button xmlns:x=\"http://schemas.microsoft.com/winfx/2006/xaml\" Command=\"{Binding SaveCommand}\" Click=\"OnSave\" Content=\"{Binding Title, Mode=TwoWay}\"/>";
        assert!(usages(source).expect("valid XAML").usages.is_empty());
    }

    #[test]
    fn the_same_file_gives_the_same_usages() {
        let first = usages(GENERIC.as_bytes()).expect("valid XAML");
        let second = usages(GENERIC.as_bytes()).expect("valid XAML");
        assert_eq!(format!("{first:?}"), format!("{second:?}"));
        assert!(usages(b"<Grid>").is_err());
    }
}
