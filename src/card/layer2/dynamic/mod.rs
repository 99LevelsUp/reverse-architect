//! The dynamic-call markers of a run (plan 2c, U6 and U7: R12–R16, R22;
//! KTD7, KTD8, KTD10): every member another place may read, write or call
//! without a static call — a binding, a static member, an attached property
//! or a setter written in markup, an event handler a markup file names, a
//! string literal or a `nameof` in C# — with where, how and how sure. Nothing
//! here renders; the `## Dynamic access` of a card (U8) and the registry of
//! triggers (U9) read it, and the walk of triggers (2b, U5) asks it which
//! members not to root as an entry from another component ([`DynamicMarks`]).
//!
//! U8 adds the markers a declaration gives with no place (R17,
//! [`GenericMark`]): a member with the name and kind of a member of an
//! interface its type implements, a `virtual`, `abstract` or `override`
//! member, a method with an event handler's parameters — and the lines of a
//! type's card ([`DynamicAccess::lines_of`]). U9 adds the triggers of markup
//! ([`MarkupTrigger`]): every element with `trigger: control` — a menu item
//! when its type is WPF's `MenuItem` or derives from it in the repository,
//! else a control (KTD15) — and every `key-gesture`, with the members its
//! command may be: the last member of each access at the element the walk
//! told (an access names no attribute, so a `Content` binding beside the
//! command is a candidate too; the registry keeps the one a functionality
//! enters at), for a key binding the access that spells the command it
//! names.
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
//! A node tagged `unparsed-markup` holds a value written as a markup
//! extension the plugin could not take apart: what it says is lost, so it is
//! counted per file ([`DynamicAccess::unparsed`]) and lowers the run's state
//! of XAML coverage (R19).
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
use super::link::Links;
use super::resolve::{Index, Unresolved};
use super::triggers::DynamicMarks;
use crate::coverage::{MarkupSites, MarkupState, UnparsedMarkup};
use crate::extract::bodies::Tag;
use crate::graph::{Confidence, FactKind, FactValue, Graph, NodeKey, Rule};
use serde::Serialize;
use std::collections::BTreeMap;

mod generic;
mod markup;
mod triggers;

/// The member a view holds its model in when its base list does not say
/// the model's type (plan 2c, KTD7).
pub const VIEW_MODEL_FIELD: &str = "viewModel";

/// The tag a markup body names the type it is about with.
pub(crate) const TARGET_TYPE: &str = "target-type";

/// How the key of a view's root ends: the plugin keys it `<file>#x:Class`
/// (plan 2c, KTD3).
const X_CLASS_KEY: &str = "#x:Class";

/// The tag a markup node names the source of its bindings' data with.
pub(crate) const DATA_SOURCE: &str = "data-source";

/// The tag of an element with a command: `trigger: control` (U9).
pub(crate) const TRIGGER: &str = "trigger";

/// The tag that names a control's identity and how it was found (U9).
pub(crate) const CONTROL_KEY: &str = "control-key";

/// The tag of a key binding: its gesture, and the command (step 2b, U9).
pub(crate) const KEY_GESTURE: &str = "key-gesture";

/// The tag of an element with a value written as a markup extension the
/// plugin could not take apart (R19): its file was not read in full.
pub(crate) const UNPARSED_MARKUP: &str = "unparsed-markup";

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
    /// The type that declares it (`HasMember`).
    pub owner: Option<NodeKey>,
}

/// A marker the declaration itself gives (plan 2c, R17): a member another
/// part of the program may call without a static call to it, said in one
/// word on the card and with no place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum GenericMark {
    /// It has the name and kind of a member of an interface its type, or a
    /// type its type derives from, implements (an `Implements` relation).
    Interface,
    /// It is `virtual` or `abstract`: a deriving type may replace it.
    Virtual,
    /// It is an `override`: called where its base's member is.
    Override,
    /// Its parameters are an event handler's: `object` and a type whose name
    /// ends in `EventArgs`.
    Handler,
}

/// The markers a member's declaration gives, in [`GenericMark`] order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GenericMarks {
    pub member: NodeKey,
    pub owner: NodeKey,
    pub marks: Vec<GenericMark>,
}

/// One member as the `## Dynamic access` of its type's card lists it (U8):
/// the words of its declaration, its marks with a place, by place, and its
/// matches by name alone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MemberLine {
    pub member: NodeKey,
    pub generic: Vec<GenericMark>,
    pub marks: Vec<Mark>,
    pub name_matches: usize,
}

/// What the `## Dynamic access` of a type's card draws (U8: KTD11): its
/// members' lines and the state of XAML coverage of the run, which the
/// section ends with. The state is the run's ([`crate::coverage::markup`]);
/// an analysis built without one says XAML was not read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TypeDynamic {
    /// By member key.
    pub lines: Vec<MemberLine>,
    pub state: MarkupState,
}

impl Default for TypeDynamic {
    fn default() -> TypeDynamic {
        TypeDynamic {
            lines: Vec::new(),
            state: MarkupState::NotRead,
        }
    }
}

/// The kind of a trigger written in markup (plan 2c, U9: R9, KTD15).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum MarkupTriggerKind {
    /// An element with `Command=` (`trigger: control`) whose type is no menu
    /// item.
    Control,
    /// An element with `Command=` whose type is `MenuItem` or, in the
    /// repository, derives from it.
    MenuItem,
    /// A `KeyBinding` (`key-gesture`): its gesture and command in markup.
    Key,
}

/// A trigger written in markup, for the registry of triggers of step 2b
/// (U9): what it is, its key, where it is and the members its command may
/// be. The registry turns it into rows by the functionalities those members
/// are entry points of.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct MarkupTrigger {
    pub kind: MarkupTriggerKind,
    /// The control's identity (`control-key`: its name, its key, the
    /// resource or the text of its label), else the element's name; a key's
    /// gesture. As written, before the secret filter.
    pub key: String,
    /// The declaration the body belongs to — a template, a style — else the
    /// markup file; for the root of a view with `x:Class`, the full name of
    /// its class (`Acme.Tools.ViewK`).
    pub member: NodeKey,
    /// The element, all its lines.
    pub place: Place,
    /// The last member of every access at the element the walk told, in the
    /// model's order; for a key binding the one of the command it names
    /// first, when one is that.
    pub targets: Vec<NodeKey>,
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
    /// By member key.
    generic: Vec<GenericMarks>,
    /// By kind, key, place and member.
    triggers: Vec<MarkupTrigger>,
    /// The markup files with a value the plugin could not take apart, by
    /// path (R19: what lowers the state of XAML coverage).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    unparsed: Vec<UnparsedMarkup>,
    /// The indexes into `generic` and into `members` of every type's
    /// members, by type: [`DynamicAccess::lines_of`] reads one type's.
    #[serde(skip)]
    by_owner: BTreeMap<NodeKey, (Vec<usize>, Vec<usize>)>,
}

impl DynamicAccess {
    /// Every marker of the run whose first layer and second layer `links`
    /// walks: the markup bodies (U6), the literals and `nameof`s of C# (U7),
    /// the words of the declarations (U8) and the triggers of markup (U9).
    pub fn build(graph: &Graph, links: &Links<'_, '_>) -> DynamicAccess {
        let mut b = Builder {
            graph,
            links,
            marks: BTreeMap::new(),
            names: BTreeMap::new(),
            sites: Vec::new(),
            refs: Vec::new(),
            generic: Vec::new(),
            triggers: Vec::new(),
            unparsed: BTreeMap::new(),
        };
        b.markup();
        super::literal_refs::collect(&mut b);
        b.generic();
        b.markup_triggers();
        b.finish()
    }

    /// Every member with a mark or a match by name, by key.
    pub fn members(&self) -> &[MemberMarks] {
        &self.members
    }

    /// Every member whose declaration gives a marker (R17), by key.
    pub fn generic(&self) -> &[GenericMarks] {
        &self.generic
    }

    /// The members of the type `ty` the run says anything of — a word of
    /// its declaration, a mark, a match by name — by key: the lines of the
    /// `## Dynamic access` of its card (U8).
    pub fn lines_of(&self, ty: &NodeKey) -> Vec<MemberLine> {
        let Some((generic, members)) = self.by_owner.get(ty) else {
            return Vec::new();
        };
        let mut lines: BTreeMap<&NodeKey, MemberLine> = BTreeMap::new();
        let blank = |member: &NodeKey| MemberLine {
            member: member.clone(),
            generic: Vec::new(),
            marks: Vec::new(),
            name_matches: 0,
        };
        for g in generic.iter().map(|&i| &self.generic[i]) {
            lines
                .entry(&g.member)
                .or_insert_with(|| blank(&g.member))
                .generic = g.marks.clone();
        }
        for m in members.iter().map(|&i| &self.members[i]) {
            let line = lines.entry(&m.member).or_insert_with(|| blank(&m.member));
            line.marks = m.marks.clone();
            line.name_matches = m.name_matches;
        }
        lines.into_values().collect()
    }

    /// The markup files with a value written as a markup extension the
    /// plugin could not take apart, by path, with how many (R19).
    pub fn unparsed(&self) -> &[UnparsedMarkup] {
        &self.unparsed
    }

    /// Every trigger written in markup (U9), by kind, key, place and member.
    pub fn triggers(&self) -> &[MarkupTrigger] {
        &self.triggers
    }

    /// The markup places, by how they ended — what coverage counts of them
    /// (U8: KTD12).
    pub fn site_counts(&self) -> MarkupSites {
        let mut c = MarkupSites::default();
        for s in &self.sites {
            *match s.outcome {
                SiteOutcome::Resolved { .. } => &mut c.resolved,
                SiteOutcome::Ambiguous { .. } => &mut c.ambiguous,
                SiteOutcome::Unresolved { .. } => &mut c.unresolved,
                SiteOutcome::Unpaired { .. } => &mut c.unpaired,
            } += 1;
        }
        c
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

/// The value and the arguments of every tag `name`.
fn tags<'t>(tags: &'t [Tag], name: &str) -> Vec<&'t Tag> {
    tags.iter().filter(|t| t.name == name).collect()
}

fn tag_value<'t>(list: &'t [Tag], name: &str) -> Option<&'t str> {
    list.iter()
        .find(|t| t.name == name)
        .and_then(|t| t.value.as_deref())
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
    generic: Vec<GenericMarks>,
    triggers: Vec<MarkupTrigger>,
    /// The values markup could not take apart, by file.
    unparsed: BTreeMap<String, u64>,
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
        let ix = self.index();
        let members = keys
            .into_iter()
            .map(|member| {
                let mut marks = self.marks.remove(&member).unwrap_or_default();
                marks.sort();
                marks.dedup();
                let name_matches = self.names.get(&member).copied().unwrap_or(0);
                let owner = ix.owner_of(&member).cloned();
                MemberMarks {
                    member,
                    marks,
                    name_matches,
                    owner,
                }
            })
            .collect::<Vec<MemberMarks>>();
        self.refs.sort();
        self.refs.dedup();
        self.generic.sort_by(|a, b| a.member.cmp(&b.member));
        self.triggers.sort();
        self.triggers.dedup();
        let mut by_owner: BTreeMap<NodeKey, (Vec<usize>, Vec<usize>)> = BTreeMap::new();
        for (i, g) in self.generic.iter().enumerate() {
            by_owner.entry(g.owner.clone()).or_default().0.push(i);
        }
        for (i, m) in members.iter().enumerate() {
            if let Some(owner) = &m.owner {
                by_owner.entry(owner.clone()).or_default().1.push(i);
            }
        }
        DynamicAccess {
            members,
            sites: self.sites,
            refs: self.refs,
            generic: self.generic,
            triggers: self.triggers,
            unparsed: self
                .unparsed
                .into_iter()
                .map(|(path, count)| UnparsedMarkup { path, count })
                .collect(),
            by_owner,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
