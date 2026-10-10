//! The dynamic-call markers of a run (plan 2c, U6 and U7: R12–R16, R22;
//! KTD7, KTD8, KTD10): every member another place may read, write or call
//! without a static call — a binding, a static member, an attached property
//! or a setter written in markup, an event handler a markup file names, a
//! string literal or a `nameof` in C# — with where, how and how sure. Nothing
//! here renders; the `## Dynamic access` of a card (U8) and the registry of
//! triggers (U9) read it, and the walk of triggers (2b, U5) asks it which
//! members not to root as an entry from another component ([`DynamicMarks`]).
//!
//! # Markup (U6)
//!
//! Every body of kind `markup` is read; its accesses carry no source of
//! their own, so the node they are on says it (the tag `data-source`) and the
//! body the type it is about (the tag `target-type`, the full CLR name of a
//! template's or style's `TargetType`, or of the `x:Class` of a view's root).
//! An access is, in this order:
//!
//! - **a write with no read beside it** (same path, same place): a setter
//!   when its node is a `Setter` element, else an attached property;
//!   `[T, P]`, `T` the full name of the type;
//! - **a read whose first name is qualified** (`Acme.Settings`), or one on a
//!   node with no `data-source`: a static member (`{x:Static T.M}`, a plain
//!   `Command="T.M"`) or an attached property read in a binding path — the
//!   first name a type, the rest its members;
//! - **otherwise a binding**, a read (and with `Mode=TwoWay` a write beside
//!   it) of the path from the node's one source of data.
//!
//! A type another file names fully qualified is the C# type of exactly that
//! full name ([`Index::type_exactly`]); a type of the same simple name in
//! another namespace backs nothing (KTD8). From the type in hand every name
//! is resolved by the one walk of the analyses ([`Links::resolve`]): one
//! level of inheritance, the next name in the type the member is declared
//! with. The binding's first type is its source's (KTD7):
//!
//! - `view` — the body's target type, then its view model: the one type
//!   argument the first entry of its base list gives (`ViewA :
//!   ViewBase<ModelB>`), else the type of a member [`VIEW_MODEL_FIELD`] of
//!   it or its base (`private ModelD viewModel;`); without either the binding
//!   stays unpaired ([`Unpaired::NoViewModel`]);
//! - `typed` — the template's `DataType` (the tag's argument);
//! - `element` — the type of the element the name names: a named element the
//!   body's declaration holds, at the one element node of its line;
//! - `self` — the node's own element type, or the body's target type for a
//!   setter's value; `templated-parent` — the body's target type;
//! - `explicit` with `Source={x:Static T.M}` — the type `T.M` is declared
//!   with (its own read is the static access beside it);
//! - `item`, any other `explicit` and a node with several sources: unpaired.
//!
//! A handler a markup body subscribes is a method of the type of the
//! outermost body around it in its file — the view's root carries its
//! `x:Class`.
//!
//! # Grades (KTD8)
//!
//! A link the walk told is [`Confidence::RulesBacked`]: a mark with a place.
//! Several members where the walk wanted one are [`Confidence::Ambiguous`],
//! a mark on each candidate shown. A name that could not be told — no type
//! of that full name, no source, a name the type does not declare — is no
//! mark: every C# member of that name counts one match by name alone
//! ([`MemberMarks::name_matches`], the summary line of R15), and so does
//! every name after it. A name the walk told gives no match by name to any
//! other member of that name.
//!
//! # C# (U7)
//!
//! See [`super::literal_refs`]: a literal argument and a `nameof` mark a
//! member too, and a literal may refer to a named element of a template or
//! to a keyed resource — [`XamlRef`].
//!
//! # Limits
//!
//! An access names no attribute, so a `Command=` binding is a binding like
//! any other: a mark keeps the body and the node it was written at, so the
//! registry of triggers (U9) can read the command of a control from the
//! node's tags. A static member whose type a URI prefix spells (`{x:Static
//! SystemColors.X}`, an unqualified first name) on a node with a binding is
//! read as a binding path. A `Mode` left implicit on a two-way property is a
//! read only (R14). Everything is ordered: files by path, bodies and
//! accesses in the model's order, marks by place (R20).

use super::entry::Place;
use super::link::{Links, Outcome};
use super::resolve::{Index, Scope, Unresolved};
use super::triggers::DynamicMarks;
use crate::extract::bodies::{
    AccessMode, AccessPath, BodyKind, Handler, MemberAccess, MemberBody, NodeKind as BodyNodeKind,
    PathRoot, Range, Subscription, Tag,
};
use crate::graph::{Confidence, FactKind, FactValue, Graph, NodeKey, Rule};
use serde::Serialize;
use std::collections::BTreeMap;

/// The member a view holds its model in when its base list does not say
/// the model's type (plan 2c, KTD7).
pub const VIEW_MODEL_FIELD: &str = "viewModel";

/// The tag a markup body names the type it is about with.
const TARGET_TYPE: &str = "target-type";

/// The tag a markup node names the source of its bindings' data with.
const DATA_SOURCE: &str = "data-source";

/// What wrote a mark (R16).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum MarkKind {
    /// A binding path's member.
    Binding,
    /// A static member read (`{x:Static T.M}`).
    Static,
    /// An attached property's accessor, written (`Owner.Property="…"`).
    Attached,
    /// A property a style's setter writes.
    Setter,
    /// An event handler a markup file subscribes.
    Handler,
    /// A string literal argument of a C# call that spells the member's name.
    Literal,
    /// A C# `nameof` of the member.
    NameOf,
}

impl MarkKind {
    /// Whether the mark is written in markup — another language — rather
    /// than in C#.
    pub fn in_markup(self) -> bool {
        !matches!(self, MarkKind::Literal | MarkKind::NameOf)
    }
}

/// How the place uses the member.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum MarkUse {
    Read,
    Write,
    /// A handler: the member is called when the event is raised.
    Call,
    /// A literal or a `nameof`: the member is named, not used.
    Named,
}

/// One dynamic-call marker with a place (R16).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Mark {
    /// Where it is written: the access's path, the literal, the handler.
    pub place: Place,
    pub kind: MarkKind,
    pub mode: MarkUse,
    /// The index of the body in its file's `members`, and the node of the
    /// body it is written at (U9 reads a control's tags there).
    pub body: usize,
    pub node: Option<u32>,
    /// The declaration the body belongs to: a style, a template or a view
    /// of the markup file, a C# member.
    pub from: Option<NodeKey>,
    /// [`Confidence::RulesBacked`], or [`Confidence::Ambiguous`] for one of
    /// several candidates (KTD8).
    pub grade: Confidence,
}

/// What the run says of one member: its marks with a place, by place, and
/// how many places name it by its name alone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MemberMarks {
    pub member: NodeKey,
    pub marks: Vec<Mark>,
    /// The places whose name no rule could tell, of this member's name
    /// (R15: one summary line, never a mark).
    pub name_matches: usize,
}

/// Why a markup access was not walked at all.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Unpaired {
    /// The body names no type (`target-type`), or a setter names no type it
    /// sets a property of.
    NoTargetType,
    /// The view's base list gives no one type argument and it holds no
    /// [`VIEW_MODEL_FIELD`].
    NoViewModel(NodeKey),
    /// The data of an item template without `DataType`.
    ItemSource,
    /// A source set where the file does not say what it is (`DataContext`,
    /// a `Source` that is no static member, a `RelativeSource` up the tree).
    UnknownSource,
    /// The node's bindings read from several sources: which access reads
    /// from which is not told.
    SeveralSources,
    /// The element a binding's `ElementName` names is not one the body's
    /// declaration holds, or no element node is at its line.
    NoElement(String),
}

/// How a markup access ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SiteOutcome {
    /// Every name told, at the weakest grade of its links.
    Resolved { grade: Confidence },
    /// Several candidates where the walk wanted one.
    Ambiguous { reason: Unresolved },
    /// A name, or the first type, could not be told.
    Unresolved { reason: Unresolved },
    /// It was not walked.
    Unpaired { why: Unpaired },
}

/// One access or subscription of a markup body, as U6 read it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MarkupSite {
    pub place: Place,
    pub body: usize,
    pub node: Option<u32>,
    pub from: Option<NodeKey>,
    pub kind: MarkKind,
    pub mode: MarkUse,
    /// The names as written (the handler's for a subscription).
    pub path: Vec<String>,
    /// The members it marks with a place, link by link.
    pub links: Vec<NodeKey>,
    pub outcome: SiteOutcome,
}

/// What a literal refers to in markup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum XamlRefKind {
    /// A named element of a template for the caller's type or an ancestor.
    Name,
    /// A resource (a style, a template, any keyed resource) whose key is the
    /// literal, in the scope of the literal's file (plan 2c, KTD5): its
    /// project, else the repository.
    Key,
}

/// A reference from C# to an element or a resource of a markup file (R22):
/// for the markup file's document (U8).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct XamlRef {
    /// The element or resource.
    pub target: NodeKey,
    /// The literal in C#.
    pub place: Place,
    /// The C# member the literal is written in.
    pub member: NodeKey,
    pub kind: XamlRefKind,
    pub grade: Confidence,
    /// The scope a key was found in ([`Rule::ScopeProject`],
    /// [`Rule::NameMatch`]); `None` for a named element.
    pub rule: Option<Rule>,
}

/// The dynamic-call markers of a run (see the module docs). Empty for a run
/// without them ([`DynamicAccess::default`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct DynamicAccess {
    /// By member key.
    members: Vec<MemberMarks>,
    /// In file, body and model order.
    sites: Vec<MarkupSite>,
    /// By target, place and member.
    refs: Vec<XamlRef>,
}

impl DynamicAccess {
    /// Every marker of the run whose first layer and second layer `links`
    /// walks: the markup bodies (U6), the literals and `nameof`s of C# (U7).
    pub fn build(graph: &Graph, links: &Links<'_, '_>) -> DynamicAccess {
        let mut b = Builder {
            graph,
            links,
            marks: BTreeMap::new(),
            names: BTreeMap::new(),
            sites: Vec::new(),
            refs: Vec::new(),
        };
        b.markup();
        super::literal_refs::collect(&mut b);
        b.finish()
    }

    /// Every member with a mark or a match by name, by key.
    pub fn members(&self) -> &[MemberMarks] {
        &self.members
    }

    /// What the run says of `member`.
    pub fn of(&self, member: &NodeKey) -> Option<&MemberMarks> {
        self.members
            .binary_search_by(|m| m.member.cmp(member))
            .ok()
            .map(|i| &self.members[i])
    }

    /// Every markup access and subscription read, in file, body and model
    /// order.
    pub fn sites(&self) -> &[MarkupSite] {
        &self.sites
    }

    /// The sites written at `node` of the body `body` of `path`.
    pub fn sites_at<'s>(
        &'s self,
        path: &'s str,
        body: usize,
        node: u32,
    ) -> impl Iterator<Item = &'s MarkupSite> {
        self.sites
            .iter()
            .filter(move |s| s.place.path == path && s.body == body && s.node == Some(node))
    }

    /// Every reference from C# to markup, by target, place and member.
    pub fn xaml_refs(&self) -> &[XamlRef] {
        &self.refs
    }

    /// The references from C# to the element or resource `target`.
    pub fn refs_to<'s>(&'s self, target: &'s NodeKey) -> impl Iterator<Item = &'s XamlRef> {
        self.refs.iter().filter(move |r| &r.target == target)
    }
}

/// A member marked from markup by a rule (KTD8) is called or read from a
/// place another language writes: no entry from another component.
impl DynamicMarks for DynamicAccess {
    fn marked(&self, member: &NodeKey) -> bool {
        self.of(member).is_some_and(|m| {
            m.marks
                .iter()
                .any(|k| k.kind.in_markup() && k.grade == Confidence::RulesBacked)
        })
    }
}

/// One markup body being read.
struct At<'l> {
    path: &'l str,
    index: usize,
    body: &'l MemberBody,
    from: Option<&'l NodeKey>,
}

/// One place being resolved: where, what, how.
struct Head<'h> {
    at: &'h At<'h>,
    range: Range,
    node: Option<u32>,
    kind: MarkKind,
    mode: MarkUse,
    path: Vec<String>,
}

impl Head<'_> {
    fn place(&self) -> Place {
        Place {
            path: self.at.path.to_string(),
            range: self.range,
        }
    }
}

/// The value and the arguments of every tag `name`.
fn tags<'t>(tags: &'t [Tag], name: &str) -> Vec<&'t Tag> {
    tags.iter().filter(|t| t.name == name).collect()
}

fn tag_value<'t>(list: &'t [Tag], name: &str) -> Option<&'t str> {
    list.iter()
        .find(|t| t.name == name)
        .and_then(|t| t.value.as_deref())
}

/// A name another file writes qualified — a type, not a member.
fn qualified(name: &str) -> bool {
    name.contains('.') || name.contains('|')
}

fn is_setter(kind: &BodyNodeKind) -> bool {
    matches!(kind, BodyNodeKind::Element(n) if n == "Setter" || n.ends_with(".Setter"))
}

/// The collector of the markers, shared by U6 (here) and U7
/// ([`super::literal_refs`]).
pub(super) struct Builder<'b, 'i, 'a> {
    graph: &'b Graph,
    links: &'b Links<'i, 'a>,
    marks: BTreeMap<NodeKey, Vec<Mark>>,
    names: BTreeMap<NodeKey, usize>,
    sites: Vec<MarkupSite>,
    refs: Vec<XamlRef>,
}

impl<'b, 'i, 'a> Builder<'b, 'i, 'a> {
    pub(super) fn graph(&self) -> &'b Graph {
        self.graph
    }

    pub(super) fn links(&self) -> &'b Links<'i, 'a> {
        self.links
    }

    pub(super) fn index(&self) -> &'i Index<'a> {
        self.links.index()
    }

    pub(super) fn mark(&mut self, member: &NodeKey, mark: Mark) {
        self.marks.entry(member.clone()).or_default().push(mark);
    }

    /// One place names `name` and no rule told which member: every C# member
    /// of that name counts it.
    pub(super) fn name_only(&mut self, name: &str) {
        for m in self.index().members_named(name) {
            *self.names.entry(m.clone()).or_default() += 1;
        }
    }

    pub(super) fn xaml_ref(&mut self, r: XamlRef) {
        self.refs.push(r);
    }

    /// The text of the first `symbol_name` fact of a node, and the line it
    /// was read at.
    pub(super) fn declared_name(&self, key: &NodeKey) -> Option<(&'b str, Option<u32>)> {
        self.graph
            .node(key)?
            .facts
            .iter()
            .find(|f| f.kind == FactKind::SymbolName)
            .and_then(|f| match &f.value {
                FactValue::Text(t) => Some((t.as_str(), f.provenance.span.start())),
                _ => None,
            })
    }

    fn finish(mut self) -> DynamicAccess {
        let mut keys: Vec<NodeKey> = self.marks.keys().cloned().collect();
        keys.extend(self.names.keys().cloned());
        keys.sort();
        keys.dedup();
        let members = keys
            .into_iter()
            .map(|member| {
                let mut marks = self.marks.remove(&member).unwrap_or_default();
                marks.sort();
                marks.dedup();
                let name_matches = self.names.get(&member).copied().unwrap_or(0);
                MemberMarks {
                    member,
                    marks,
                    name_matches,
                }
            })
            .collect();
        self.refs.sort();
        self.refs.dedup();
        DynamicAccess {
            members,
            sites: self.sites,
            refs: self.refs,
        }
    }

    // ---- Markup (U6) ----

    fn markup(&mut self) {
        let layer = self.index().layer;
        for (path, file) in layer.files() {
            let bodies = &file.model.members;
            for (index, body) in bodies.iter().enumerate() {
                if body.kind != BodyKind::Markup {
                    continue;
                }
                let at = At {
                    path,
                    index,
                    body,
                    from: file
                        .owners
                        .get(index)
                        .and_then(Option::as_ref)
                        .map(|(k, _)| k),
                };
                for access in &body.accesses {
                    self.access(&at, access);
                }
                for s in &body.subscriptions {
                    self.handler(&at, bodies, s);
                }
            }
        }
    }

    fn access(&mut self, at: &At<'_>, a: &MemberAccess) {
        let segments = &a.target.segments;
        if segments.is_empty() || !matches!(a.mode, AccessMode::Read | AccessMode::Write) {
            return;
        }
        let node = a.node.and_then(|n| at.body.nodes.get(n as usize));
        let sources = node.map_or_else(Vec::new, |n| tags(&n.tags, DATA_SOURCE));
        let read_beside = at
            .body
            .accesses
            .iter()
            .any(|b| b.mode == AccessMode::Read && b.target == a.target && b.place == a.place);
        let head = |kind, mode| Head {
            at,
            range: a.place,
            node: a.node,
            kind,
            mode,
            path: segments.clone(),
        };
        if a.mode == AccessMode::Write && !read_beside {
            let kind = if node.is_some_and(|n| is_setter(&n.kind)) {
                MarkKind::Setter
            } else {
                MarkKind::Attached
            };
            self.type_rooted(head(kind, MarkUse::Write));
        } else if qualified(&segments[0]) || sources.is_empty() {
            self.type_rooted(head(MarkKind::Static, MarkUse::Read));
        } else {
            let mode = if a.mode == AccessMode::Write {
                MarkUse::Write
            } else {
                MarkUse::Read
            };
            let node_kind = node.map(|n| &n.kind);
            self.binding(head(MarkKind::Binding, mode), &sources, node_kind);
        }
    }

    /// `[T, M, …]`: `T` a type's full name, the rest its members.
    fn type_rooted(&mut self, head: Head<'_>) {
        let [ty, rest @ ..] = head.path.as_slice() else {
            return;
        };
        if rest.is_empty() {
            return self.unpaired(head, Unpaired::NoTargetType, 0);
        }
        let ty = match self.index().type_exactly(ty) {
            Ok(ty) => ty,
            Err(reason) => return self.stopped(head, reason, 1),
        };
        let names: Vec<String> = rest.to_vec();
        if head.kind == MarkKind::Attached && names.len() == 1 {
            return self.attached(head, &ty, &names[0]);
        }
        self.follow(head, &ty, names, 0);
    }

    /// `Owner.Property="…"`: the owner's accessors `Get<Property>` and
    /// `Set<Property>` — WPF's convention for an attached property — or,
    /// when it has neither, a member of that name.
    fn attached(&mut self, head: Head<'_>, owner: &NodeKey, property: &str) {
        let ix = self.index();
        let accessors: Vec<NodeKey> = [format!("Get{property}"), format!("Set{property}")]
            .iter()
            .flat_map(|name| {
                ix.members_of(owner)
                    .iter()
                    .filter(|m| {
                        ix.symbol(m)
                            .is_some_and(|s| s.name == *name && s.is_method())
                    })
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .collect();
        if accessors.is_empty() {
            return self.follow(head, owner, vec![property.to_string()], 0);
        }
        for m in &accessors {
            self.put(&head, m, Confidence::RulesBacked);
        }
        self.sites.push(MarkupSite {
            place: head.place(),
            body: head.at.index,
            node: head.node,
            from: head.at.from.cloned(),
            kind: head.kind,
            mode: head.mode,
            path: head.path,
            links: accessors,
            outcome: SiteOutcome::Resolved {
                grade: Confidence::RulesBacked,
            },
        });
    }

    fn binding(&mut self, head: Head<'_>, sources: &[&Tag], node: Option<&BodyNodeKind>) {
        let [source] = sources else {
            return self.unpaired(head, Unpaired::SeveralSources, 0);
        };
        match source.value.as_deref() {
            Some("view") => match self.target_type(&head) {
                Ok(view) => self.view_chain(head, &view),
                Err(Err(why)) => self.unpaired(head, why, 0),
                Err(Ok(reason)) => self.stopped(head, reason, 0),
            },
            Some("typed") => match source.args.first() {
                Some(t) => match self.index().type_exactly(t) {
                    Ok(ty) => {
                        let names = head.path.clone();
                        self.follow(head, &ty, names, 0)
                    }
                    Err(reason) => self.stopped(head, reason, 0),
                },
                None => self.unpaired(head, Unpaired::UnknownSource, 0),
            },
            Some("element") => match source.args.first() {
                Some(name) => match self.element_type(head.at, name) {
                    Some(t) => match self.index().type_exactly(&t) {
                        Ok(ty) => {
                            let names = head.path.clone();
                            self.follow(head, &ty, names, 0)
                        }
                        Err(reason) => self.stopped(head, reason, 0),
                    },
                    None => self.unpaired(head, Unpaired::NoElement(name.clone()), 0),
                },
                None => self.unpaired(head, Unpaired::UnknownSource, 0),
            },
            Some("self") => {
                let own = match node {
                    Some(k @ BodyNodeKind::Element(n)) if !is_setter(k) => Some(n.clone()),
                    _ => None,
                };
                let ty = match own {
                    Some(n) => self.index().type_exactly(&n).map_err(Ok),
                    None => self.target_type(&head),
                };
                match ty {
                    Ok(ty) => {
                        let names = head.path.clone();
                        self.follow(head, &ty, names, 0)
                    }
                    Err(Err(why)) => self.unpaired(head, why, 0),
                    Err(Ok(reason)) => self.stopped(head, reason, 0),
                }
            }
            Some("templated-parent") => match self.target_type(&head) {
                Ok(ty) => {
                    let names = head.path.clone();
                    self.follow(head, &ty, names, 0)
                }
                Err(Err(why)) => self.unpaired(head, why, 0),
                Err(Ok(reason)) => self.stopped(head, reason, 0),
            },
            Some("explicit") => match source.args.as_slice() {
                [ty, member] => match self.index().type_exactly(ty) {
                    Ok(ty) => {
                        let mut names = vec![member.clone()];
                        names.extend(head.path.iter().cloned());
                        self.follow(head, &ty, names, 1)
                    }
                    Err(reason) => self.stopped(head, reason, 0),
                },
                _ => self.unpaired(head, Unpaired::UnknownSource, 0),
            },
            Some("item") => self.unpaired(head, Unpaired::ItemSource, 0),
            _ => self.unpaired(head, Unpaired::UnknownSource, 0),
        }
    }

    /// The type the body is about: `Err(Err(_))` when it names none,
    /// `Err(Ok(_))` when the repository declares no C# type of exactly that
    /// name, or several.
    fn target_type(&self, head: &Head<'_>) -> Result<NodeKey, Result<Unresolved, Unpaired>> {
        let Some(full) = tag_value(&head.at.body.tags, TARGET_TYPE) else {
            return Err(Err(Unpaired::NoTargetType));
        };
        self.index().type_exactly(full).map_err(Ok)
    }

    /// From the view to its model, then the binding's path (KTD7).
    fn view_chain(&mut self, head: Head<'_>, view: &NodeKey) {
        if let Some(model) = self.view_model(view) {
            let names = head.path.clone();
            return self.follow(head, &model, names, 0);
        }
        if self.holds_view_model_field(view) {
            let mut names = vec![VIEW_MODEL_FIELD.to_string()];
            names.extend(head.path.iter().cloned());
            return self.follow(head, view, names, 1);
        }
        self.unpaired(head, Unpaired::NoViewModel(view.clone()), 0)
    }

    /// The one type argument the first entry of `view`'s base list gives,
    /// the same on every part that writes one, when it names one type of the
    /// repository.
    fn view_model(&self, view: &NodeKey) -> Option<NodeKey> {
        let ix = self.index();
        let mut given: Vec<&Vec<String>> = ix
            .layer
            .type_shapes_of(view)
            .into_iter()
            .filter_map(|(_, shape)| shape.bases.first())
            .map(|b| &b.type_args)
            .filter(|args| !args.is_empty())
            .collect();
        given.sort();
        given.dedup();
        match given.as_slice() {
            [args] => match args.as_slice() {
                [arg] if !arg.contains('<') => ix.type_named(arg).ok(),
                _ => None,
            },
            _ => None,
        }
    }

    fn holds_view_model_field(&self, view: &NodeKey) -> bool {
        let ix = self.index();
        let holds = |ty: &NodeKey| {
            ix.members_of(ty).iter().any(|m| {
                ix.symbol(m)
                    .is_some_and(|s| s.name == VIEW_MODEL_FIELD && s.is_value())
            })
        };
        holds(view) || matches!(ix.base_of(view), Ok(Some(base)) if holds(base))
    }

    /// The type of the element `name` of the body's declaration: a named
    /// element the declaration holds, at the one element node of its line.
    fn element_type(&self, at: &At<'_>, name: &str) -> Option<String> {
        let from = at.from?;
        let named: Vec<u32> = self
            .index()
            .members_of(from)
            .iter()
            .filter_map(|k| self.declared_name(k))
            .filter(|(n, _)| *n == name)
            .filter_map(|(_, line)| line)
            .collect();
        let [line] = named.as_slice() else {
            return None;
        };
        let mut elements = at.body.nodes.iter().filter_map(|n| match &n.kind {
            BodyNodeKind::Element(t) if n.place.start.line == *line => Some(t),
            _ => None,
        });
        let one = elements.next()?;
        elements.next().is_none().then(|| one.clone())
    }

    /// A handler of an event a markup body subscribes: a method of the type
    /// of the outermost body around it in its file.
    fn handler(&mut self, at: &At<'_>, bodies: &[MemberBody], s: &Subscription) {
        let Handler::MethodGroup(AccessPath { segments, .. }) = &s.handler else {
            return;
        };
        let [name] = segments.as_slice() else {
            return;
        };
        let head = Head {
            at,
            range: s.place,
            node: s.node,
            kind: MarkKind::Handler,
            mode: MarkUse::Call,
            path: vec![name.clone()],
        };
        let around = bodies
            .iter()
            .filter(|b| b.kind == BodyKind::Markup)
            .filter_map(|b| b.nodes.get(b.root as usize).map(|root| (b, root.place)))
            .filter(|(_, r)| r.start <= s.place.start && s.place.end <= r.end)
            .min_by_key(|(_, r)| (r.start, std::cmp::Reverse(r.end)))
            .and_then(|(b, _)| tag_value(&b.tags, TARGET_TYPE));
        let Some(full) = around else {
            return self.unpaired(head, Unpaired::NoTargetType, 0);
        };
        match self.index().type_exactly(full) {
            Ok(ty) => self.follow(head, &ty, vec![name.clone()], 0),
            Err(reason) => self.stopped(head, reason, 0),
        }
    }

    /// Walk `names` from `ty` and mark every link from `skip` on (the names
    /// before it lead to the type the written path starts at); what could
    /// not be told from there on is a match by name alone.
    fn follow(&mut self, head: Head<'_>, ty: &NodeKey, names: Vec<String>, skip: usize) {
        let place = head.place();
        let caller = head.at.from.cloned().unwrap_or_else(|| ty.clone());
        let scope = Scope {
            ty,
            body: None,
            node: None,
        };
        let path = AccessPath {
            root: PathRoot::This,
            segments: names.clone(),
        };
        let res = self.links.resolve(&caller, &scope, &path, None, &place);
        let mut marked = Vec::new();
        for l in res.links.iter().skip(skip) {
            self.put(&head, &l.link.member, l.grade);
            marked.push(l.link.member.clone());
        }
        // The names of `names` from which on nothing was told.
        let told = res.links.len();
        let outcome = match &res.outcome {
            Outcome::Resolved { grade, .. } => SiteOutcome::Resolved { grade: *grade },
            Outcome::Ambiguous { reason } => {
                if told >= skip {
                    for c in &res.candidates.shown {
                        self.put(&head, c, Confidence::Ambiguous);
                    }
                }
                self.names_from(&names, (told + 1).max(skip));
                SiteOutcome::Ambiguous {
                    reason: reason.clone(),
                }
            }
            Outcome::Unresolved { reason } => {
                self.names_from(&names, told.max(skip));
                SiteOutcome::Unresolved {
                    reason: reason.clone(),
                }
            }
        };
        self.sites.push(MarkupSite {
            place,
            body: head.at.index,
            node: head.node,
            from: head.at.from.cloned(),
            kind: head.kind,
            mode: head.mode,
            path: head.path,
            links: marked,
            outcome,
        });
    }

    fn names_from(&mut self, names: &[String], from: usize) {
        for name in names.iter().skip(from) {
            self.name_only(name);
        }
    }

    fn put(&mut self, head: &Head<'_>, member: &NodeKey, grade: Confidence) {
        let mark = Mark {
            place: head.place(),
            kind: head.kind,
            mode: head.mode,
            body: head.at.index,
            node: head.node,
            from: head.at.from.cloned(),
            grade,
        };
        self.mark(member, mark);
    }

    /// The first type could not be told: every written name from `from` on
    /// is a match by name alone.
    fn stopped(&mut self, head: Head<'_>, reason: Unresolved, from: usize) {
        self.names_from(&head.path.clone(), from);
        self.sites.push(MarkupSite {
            place: head.place(),
            body: head.at.index,
            node: head.node,
            from: head.at.from.cloned(),
            kind: head.kind,
            mode: head.mode,
            path: head.path,
            links: Vec::new(),
            outcome: SiteOutcome::Unresolved { reason },
        });
    }

    /// Not walked: every written name from `from` on is a match by name
    /// alone.
    fn unpaired(&mut self, head: Head<'_>, why: Unpaired, from: usize) {
        self.names_from(&head.path.clone(), from);
        self.sites.push(MarkupSite {
            place: head.place(),
            body: head.at.index,
            node: head.node,
            from: head.at.from.cloned(),
            kind: head.kind,
            mode: head.mode,
            path: head.path,
            links: Vec::new(),
            outcome: SiteOutcome::Unpaired { why },
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_qualified_name_is_a_type_and_a_setter_is_told_by_its_element() {
        assert!(qualified("Acme.Settings"));
        assert!(qualified("http://p|Button"));
        assert!(!qualified("Holder"));
        assert!(is_setter(&BodyNodeKind::Element("Setter".into())));
        assert!(is_setter(&BodyNodeKind::Element("Acme.Ui.Setter".into())));
        assert!(!is_setter(&BodyNodeKind::Element("Button".into())));
        assert!(!is_setter(&BodyNodeKind::Body));
    }

    #[test]
    fn only_a_markup_kind_is_written_in_another_language() {
        for kind in [
            MarkKind::Binding,
            MarkKind::Static,
            MarkKind::Attached,
            MarkKind::Setter,
            MarkKind::Handler,
        ] {
            assert!(kind.in_markup(), "{kind:?}");
        }
        assert!(!MarkKind::Literal.in_markup());
        assert!(!MarkKind::NameOf.in_markup());
    }
}
