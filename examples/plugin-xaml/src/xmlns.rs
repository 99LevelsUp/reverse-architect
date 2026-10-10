//! What a prefix means: the `xmlns` mappings in force at an element, and the
//! type a spelling such as `local:ViewA` names.
//!
//! A prefix mapped with `clr-namespace:N` (or `using:N`) names exactly one
//! CLR namespace. A prefix mapped to a URI (`http://…`) names whatever the
//! assemblies' `XmlnsDefinition` attributes put under it, which the file
//! itself does not say.

use crate::parse::{markup, split_prefix, ArgValue, Doc, Markup};
use crate::ra::plugin::types::{Receiver, SiteContext, Usage, UsageKind, UsingDirective};
use std::ops::Range;

/// The XAML language namespace (`x:Key`, `x:Name`, `x:Class`, `x:Type`).
pub const XAML_NS: &str = "http://schemas.microsoft.com/winfx/2006/xaml";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ns<'a> {
    /// `clr-namespace:N;assembly=A` or `using:N`: the namespace `N`.
    Clr(&'a str),
    /// Any other mapping.
    Uri(&'a str),
    /// No mapping for the prefix is in force.
    Unmapped,
}

fn classify(value: &str) -> Ns<'_> {
    let value = value.trim();
    if let Some(rest) = value.strip_prefix("clr-namespace:") {
        Ns::Clr(rest.split(';').next().unwrap_or("").trim())
    } else if let Some(rest) = value.strip_prefix("using:") {
        Ns::Clr(rest.trim())
    } else {
        Ns::Uri(value)
    }
}

/// What `prefix` (`None`: the default namespace) means at element `at`: the
/// nearest `xmlns` declaration on it or an ancestor.
pub fn resolve<'a>(doc: &Doc<'a>, at: usize, prefix: Option<&str>) -> Ns<'a> {
    for index in std::iter::once(at).chain(doc.ancestors(at)) {
        for attr in &doc.elements[index].attrs {
            let declares = match (doc.text(&attr.name).strip_prefix("xmlns"), prefix) {
                (Some(""), None) => true,
                (Some(rest), Some(p)) => rest.strip_prefix(':') == Some(p),
                _ => false,
            };
            if declares {
                return classify(doc.text(&attr.value));
            }
        }
    }
    Ns::Unmapped
}

/// The CLR namespaces the file maps to a prefix anywhere, in source order,
/// each once, as using directives. A mapping on an inner element joins them
/// too: that only widens what is visible, so the namespace rule never
/// disputes the type a prefix names. No alias: in C# an alias does not make
/// a namespace's types visible, and the prefix is not a C# alias anyway.
pub fn clr_usings(doc: &Doc<'_>) -> Vec<UsingDirective> {
    let mut usings: Vec<UsingDirective> = Vec::new();
    for element in &doc.elements {
        for attr in &element.attrs {
            let name = doc.text(&attr.name);
            if name != "xmlns" && !name.starts_with("xmlns:") {
                continue;
            }
            if let Ns::Clr(ns) = classify(doc.text(&attr.value)) {
                if !ns.is_empty() && !usings.iter().any(|u| u.target == ns) {
                    usings.push(UsingDirective {
                        target: ns.to_string(),
                        alias: None,
                        is_static: false,
                        is_global: false,
                    });
                }
            }
        }
    }
    usings
}

/// Whether `prefix` names the XAML language namespace at element `at`. A
/// file that never maps `x` (a fragment) still means it by `x:`.
pub fn is_xaml(doc: &Doc<'_>, at: usize, prefix: Option<&str>) -> bool {
    match resolve(doc, at, prefix) {
        Ns::Uri(uri) => uri == XAML_NS,
        Ns::Unmapped => prefix == Some("x"),
        Ns::Clr(_) => false,
    }
}

/// The value range of the attribute `x:<local>` of element `at`, whatever
/// prefix the file maps to the XAML namespace.
pub fn x_value(doc: &Doc<'_>, at: usize, local: &str) -> Option<Range<usize>> {
    doc.elements[at].attrs.iter().find_map(|attr| {
        let (prefix, name) = split_prefix(doc.text(&attr.name));
        (prefix.is_some() && name == local && is_xaml(doc, at, prefix)).then(|| attr.value.clone())
    })
}

/// The type an `{x:Type T}` extension names (positionally or as `TypeName=`),
/// read in the scope of element `at`. `None` for any other extension.
pub fn x_type<'a>(doc: &Doc<'a>, at: usize, ext: &Markup<'a>) -> Option<TypeRef<'a>> {
    let (prefix, name) = split_prefix(ext.name);
    if name != "Type" || !is_xaml(doc, at, prefix) {
        return None;
    }
    match ext.positional().or_else(|| ext.named("TypeName"))? {
        ArgValue::Text { text, at: offset } => TypeRef::read(doc, at, text, *offset),
        ArgValue::Markup(_) => None,
    }
}

/// The type an attribute value names, written either as a bare spelling
/// (`TargetType="local:ViewA"`) or as `{x:Type local:ViewA}`.
pub fn type_of_value<'a>(doc: &Doc<'a>, at: usize, value: &Range<usize>) -> Option<TypeRef<'a>> {
    let text = doc.text(value);
    if text.trim_start().starts_with('{') {
        return x_type(doc, at, &markup(text, value.start)?);
    }
    TypeRef::read(doc, at, text, value.start)
}

/// A type named by a spelling (`local:ViewA`, `Button`, `local:Outer+Inner`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeRef<'a> {
    pub ns: Ns<'a>,
    pub prefix: Option<&'a str>,
    /// After the prefix, as written.
    pub name: &'a str,
    /// Byte offset of `name` in the document.
    pub name_at: usize,
}

impl<'a> TypeRef<'a> {
    /// Reads `spelled` (written at byte offset `at`) in the scope of element
    /// `element`. `None` for an empty spelling.
    pub fn read(doc: &Doc<'a>, element: usize, spelled: &'a str, at: usize) -> Option<Self> {
        let lead = spelled.len() - spelled.trim_start().len();
        let spelled = spelled.trim();
        let (prefix, name) = split_prefix(spelled);
        if name.is_empty() {
            return None;
        }
        let name_at = at + lead + prefix.map_or(0, |p| p.len() + 1);
        Some(TypeRef {
            ns: resolve(doc, element, prefix),
            prefix,
            name,
            name_at,
        })
    }

    /// The type's identity in a `type:<T>` key: qualified by the CLR
    /// namespace its prefix maps, or `<URI>|<name>` for a URI mapping, so the
    /// same type spelled with different prefixes in different files gives
    /// the same key. A nested type's `+` reads as `.`.
    pub fn identity(&self) -> String {
        let name = self.name.replace('+', ".");
        match self.ns {
            Ns::Clr("") => name,
            Ns::Clr(ns) => format!("{ns}.{name}"),
            Ns::Uri(uri) => format!("{uri}|{name}"),
            Ns::Unmapped => match self.prefix {
                Some(prefix) => format!("{prefix}:{name}"),
                None => name,
            },
        }
    }

    /// The usage of this type: a reference by its simple name. Under a
    /// `clr-namespace` prefix the site sits in the file's scope, whose usings
    /// are the file's CLR namespaces ([`clr_usings`]); a nested type
    /// (`local:Outer+Inner`) says the type it is written under as its
    /// qualifier, which is what precedes the name. Under a URI the file does
    /// not say which namespace it means, so the site carries no context.
    pub fn usage(&self, doc: &Doc<'_>) -> Usage {
        let last = self.name.rfind(['.', '+']).map_or(0, |i| i + 1);
        let simple = &self.name[last..];
        let (line, column) = doc.position(self.name_at + last);
        let context = match self.ns {
            Ns::Clr(_) => {
                let outer = self.name[..last.saturating_sub(1)].replace('+', ".");
                Some(SiteContext {
                    scope: 0,
                    receiver: Receiver::None,
                    declared_type: None,
                    argument_count: None,
                    qualifier: (!outer.is_empty()).then_some(outer),
                })
            }
            Ns::Uri(_) | Ns::Unmapped => None,
        };
        Usage {
            name: simple.to_string(),
            kind: UsageKind::Reference,
            line,
            column: Some(column),
            context,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse;

    const SOURCE: &[u8] = b"<ResourceDictionary xmlns=\"http://p\" xmlns:x=\"http://schemas.microsoft.com/winfx/2006/xaml\" xmlns:local=\"clr-namespace:Acme.Views;assembly=Acme\"><Grid xmlns:local=\"using:Other\"/></ResourceDictionary>";

    #[test]
    fn the_nearest_mapping_wins() {
        let doc = parse(SOURCE).expect("valid XML");
        assert_eq!(resolve(&doc, 0, Some("local")), Ns::Clr("Acme.Views"));
        assert_eq!(resolve(&doc, 1, Some("local")), Ns::Clr("Other"));
        assert_eq!(resolve(&doc, 1, None), Ns::Uri("http://p"));
        assert_eq!(resolve(&doc, 1, Some("v")), Ns::Unmapped);
        assert!(is_xaml(&doc, 1, Some("x")));
        assert!(!is_xaml(&doc, 1, Some("local")));
    }

    #[test]
    fn two_prefixes_for_one_namespace_give_one_identity() {
        let a = parse(b"<a xmlns:local=\"clr-namespace:Acme.Views;assembly=Acme\"/>").unwrap();
        let b = parse(b"<a xmlns:v=\"clr-namespace:Acme.Views\"/>").unwrap();
        let ta = TypeRef::read(&a, 0, "local:ViewA", 0).unwrap();
        let tb = TypeRef::read(&b, 0, "v:ViewA", 0).unwrap();
        assert_eq!(ta.identity(), "Acme.Views.ViewA");
        assert_eq!(ta.identity(), tb.identity());
        let uri = parse(b"<a xmlns=\"http://p\"/>").unwrap();
        assert_eq!(
            TypeRef::read(&uri, 0, "Button", 0).unwrap().identity(),
            "http://p|Button"
        );
    }
}
