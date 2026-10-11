//! String literals and `nameof` in C# as dynamic-call markers, and the
//! references they make from C# to markup (plan 2c, U7: R12, R15, R22;
//! KTD10). Read into the one model of [`super::dynamic`].
//!
//! A literal is matched **by its value**, never by the method it is passed to
//! (KTD10: there is no list of `GetTemplateChild`, `OnPropertyChanged`…): a
//! literal argument of a call (`call-arg`) whose value is spelled like a name
//!
//! - **marks a member** of the type of the member it is written in, or of
//!   the nearest of that type's ancestors that declares one of that name —
//!   [`Confidence::RulesBacked`] for the one member, [`Confidence::Ambiguous`]
//!   for each of several (overloads); with none there, every C# member of the
//!   name counts a match by name alone (R15);
//! - **refers to markup** when a template for that type or an ancestor (a
//!   markup body whose `target-type` is the type's exact full name) holds a
//!   named element of that value (R22): one [`XamlRef`] per such element, so
//!   a template in two looks gives two;
//! - **refers to a resource** whose key it is (R22): the resources, styles
//!   and templates of that key the host's scopes find from the literal's file
//!   ([`GraphScopes`]) — those of its project, [`Confidence::RulesBacked`],
//!   else those of the whole repository, [`Confidence::NameOnly`]; several
//!   that are not variants are [`Confidence::Ambiguous`], each a reference,
//!   and over the ambiguity ceiling none. A key nothing declares refers to
//!   nothing.
//!
//! A `nameof(X)` is the use of the member `X` (KTD10): the path it spells is
//! walked as an access written where the `nameof` is ([`Links::resolve`]),
//! and a path that starts at a type (`nameof(ModelB.P)`) from that type; the
//! member it ends in is marked. A `nameof` of a local or a parameter names
//! no member; one that cannot be told is a match by name of its last name.
//!
//! Neither needs the markup plugin: without one, a literal and a `nameof`
//! still mark C# members, and no literal refers to markup.
//!
//! Limit of the key: the scopes of a key (plan 2c, KTD5) are the host's
//! (`extract::name_ref`), but the lookup blocks and every file's keys outside
//! them are the declaration pass's and are dropped once the markup's own
//! references are resolved. From C# only two scopes are left — the project,
//! then the repository; no block, no merged dictionary, no file of the
//! markup itself — and two candidates are variants only by their paths under
//! sibling project roots, never by the same set of keys.

use super::dynamic::{Builder, Mark, MarkKind, MarkUse, XamlRef, XamlRefKind, TARGET_TYPE};
use super::entry::Place;
use super::link::{Links, Outcome};
use super::resolve::{full_name, Scope};
use crate::extract::bodies::{
    AccessPath, BodyKind, DeclarationKind, LiteralContext, MemberBody, NameOf, PathRoot,
    StringLiteral,
};
use crate::extract::name_ref::{GraphScopes, RESOURCE_KIND};
use crate::graph::{Confidence, NodeKey};
use std::collections::{BTreeMap, BTreeSet};

/// Whether `s` is spelled like a C# identifier — the only values a member's
/// or an element's name can be.
fn is_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_alphabetic() || c == '_')
        && chars.all(|c| c.is_alphanumeric() || c == '_')
}

/// One C# body being read.
struct Written<'w> {
    path: &'w str,
    index: usize,
    body: &'w MemberBody,
    /// The member the body belongs to.
    member: &'w NodeKey,
    /// Its type, then that type's ancestors, nearest first.
    line: Vec<NodeKey>,
}

impl Written<'_> {
    fn place(&self, range: crate::extract::bodies::Range) -> Place {
        Place {
            path: self.path.to_string(),
            range,
        }
    }

    fn mark(
        &self,
        range: crate::extract::bodies::Range,
        kind: MarkKind,
        node: Option<u32>,
        grade: Confidence,
    ) -> Mark {
        Mark {
            place: self.place(range),
            kind,
            mode: MarkUse::Named,
            body: self.index,
            node,
            from: Some(self.member.clone()),
            grade,
        }
    }
}

/// Read every literal argument and every `nameof` of the run's C# bodies.
pub(super) fn collect(b: &mut Builder<'_, '_, '_>) {
    let ix = b.index();
    let csharp: BTreeSet<&NodeKey> = ix.csharp_types().into_iter().collect();
    // The templates and styles of markup by the full name of the type they
    // are for.
    let mut templates: BTreeMap<String, Vec<NodeKey>> = BTreeMap::new();
    for (_, file) in ix.layer.files() {
        for (i, body) in file.model.members.iter().enumerate() {
            if body.kind != BodyKind::Markup {
                continue;
            }
            let target = body
                .tags
                .iter()
                .find(|t| t.name == TARGET_TYPE)
                .and_then(|t| t.value.clone());
            if let (Some(target), Some(Some((owner, _)))) = (target, file.owners.get(i)) {
                templates.entry(target).or_default().push(owner.clone());
            }
        }
    }
    for list in templates.values_mut() {
        list.sort();
        list.dedup();
    }
    let keys = GraphScopes::of_graph(b.graph(), RESOURCE_KIND);
    for (path, file) in ix.layer.files() {
        for (index, body) in file.model.members.iter().enumerate() {
            if body.kind == BodyKind::Markup {
                continue;
            }
            let Some(Some((member, _))) = file.owners.get(index) else {
                continue;
            };
            let ty = match ix.symbol(member) {
                Some(s) if s.is_type() => member.clone(),
                _ => match ix.owner_of(member) {
                    Some(owner) => owner.clone(),
                    None => continue,
                },
            };
            if !csharp.contains(&ty) {
                continue;
            }
            let mut line = vec![ty.clone()];
            line.extend(ix.ancestors(&ty).into_iter().cloned());
            let at = Written {
                path,
                index,
                body,
                member,
                line,
            };
            for literal in &body.literals {
                if matches!(literal.context, LiteralContext::CallArg(..))
                    && is_identifier(&literal.value)
                {
                    self::literal(b, &at, literal, &templates, &keys);
                }
            }
            for name_of in &body.name_ofs {
                self::name_of(b, &at, name_of);
            }
        }
    }
}

/// A literal argument: a mark on the member it names, a reference to the
/// named elements of the templates for the caller's type.
fn literal(
    b: &mut Builder<'_, '_, '_>,
    at: &Written<'_>,
    literal: &StringLiteral,
    templates: &BTreeMap<String, Vec<NodeKey>>,
    keys: &GraphScopes,
) {
    let ix = b.index();
    let value = literal.value.as_str();
    // The C# members of that name, by key, looked up once: those the
    // nearest type of the line declares.
    let named = ix.members_named(value);
    let found: Option<Vec<NodeKey>> = at.line.iter().find_map(|t| {
        let declared: Vec<NodeKey> = named
            .iter()
            .filter(|m| ix.owner_of(m) == Some(t))
            .cloned()
            .collect();
        (!declared.is_empty()).then_some(declared)
    });
    match found.as_deref() {
        Some([one]) => {
            let mark = at.mark(
                literal.place,
                MarkKind::Literal,
                literal.node,
                Confidence::RulesBacked,
            );
            b.mark(one, mark);
        }
        Some(several) => {
            for m in several {
                let mark = at.mark(
                    literal.place,
                    MarkKind::Literal,
                    literal.node,
                    Confidence::Ambiguous,
                );
                b.mark(m, mark);
            }
        }
        None => b.name_only(value),
    }
    for ty in &at.line {
        let Some(full) = full_name(ty) else {
            continue;
        };
        for template in templates.get(&full).into_iter().flatten() {
            for element in ix.members_of(template) {
                if b.declared_name(element).is_some_and(|(n, _)| n == value) {
                    b.xaml_ref(XamlRef {
                        target: element.clone(),
                        place: at.place(literal.place),
                        member: at.member.clone(),
                        kind: XamlRefKind::Name,
                        grade: Confidence::RulesBacked,
                        rule: None,
                    });
                }
            }
        }
    }
    if let Some(hits) = keys.lookup(at.path, value, b.links().ceiling()) {
        for target in hits.keys {
            b.xaml_ref(XamlRef {
                target,
                place: at.place(literal.place),
                member: at.member.clone(),
                kind: XamlRefKind::Key,
                grade: hits.confidence,
                rule: Some(hits.rule),
            });
        }
    }
}

/// `nameof(X)`: the use of the member `X`.
fn name_of(b: &mut Builder<'_, '_, '_>, at: &Written<'_>, n: &NameOf) {
    let names: Vec<&str> = n.text.split('.').map(str::trim).collect();
    if names.iter().any(|s| !is_identifier(s)) {
        return;
    }
    let (root, names): (PathRoot, Vec<String>) = match names.as_slice() {
        ["this", rest @ ..] => (PathRoot::This, rest.iter().map(|s| s.to_string()).collect()),
        ["base", rest @ ..] => (PathRoot::Base, rest.iter().map(|s| s.to_string()).collect()),
        all => (PathRoot::Name, all.iter().map(|s| s.to_string()).collect()),
    };
    let Some(last) = names.last().cloned() else {
        return;
    };
    let local = root == PathRoot::Name
        && at.body.typed_names.iter().any(|t| {
            t.name == names[0]
                && matches!(
                    t.kind,
                    DeclarationKind::Local | DeclarationKind::Parameter | DeclarationKind::Pattern
                )
        });
    if local {
        return;
    }
    let links: &Links<'_, '_> = b.links();
    let ix = b.index();
    let place = at.place(n.place);
    let scope = Scope {
        ty: &at.line[0],
        body: Some(at.body),
        node: n.node,
    };
    let path = AccessPath {
        root,
        segments: names.clone(),
    };
    let mut res = links.resolve(at.member, &scope, &path, None, &place);
    let mut walked = names.len();
    if res.target().is_none() && root == PathRoot::Name && names.len() > 1 {
        // A path that starts at a type: `nameof(ModelB.P)`.
        for k in (1..names.len()).rev() {
            if let Ok(ty) = ix.type_named(&names[..k].join(".")) {
                let scope = Scope {
                    ty: &ty,
                    body: None,
                    node: None,
                };
                let path = AccessPath {
                    root: PathRoot::This,
                    segments: names[k..].to_vec(),
                };
                res = links.resolve(at.member, &scope, &path, None, &place);
                walked = names.len() - k;
                break;
            }
        }
    }
    match &res.outcome {
        Outcome::Resolved { target, grade } => {
            let mark = at.mark(n.place, MarkKind::NameOf, n.node, *grade);
            b.mark(target, mark);
        }
        Outcome::Ambiguous { .. } if res.links.len() + 1 == walked => {
            for c in &res.candidates.shown {
                let mark = at.mark(n.place, MarkKind::NameOf, n.node, Confidence::Ambiguous);
                b.mark(c, mark);
            }
        }
        _ => b.name_only(&last),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_value_spelled_like_a_name_is_matched() {
        assert!(is_identifier("Title"));
        assert!(is_identifier("PART_List"));
        assert!(is_identifier("_x1"));
        assert!(!is_identifier("Kontrola chyb"));
        assert!(!is_identifier("a.b"));
        assert!(!is_identifier("1st"));
        assert!(!is_identifier(""));
    }
}
