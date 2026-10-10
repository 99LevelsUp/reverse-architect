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
use super::link::{Links, Outcome};
use super::resolve::{Index, Scope, Unresolved};
use super::triggers::DynamicMarks;
use crate::coverage::{MarkupSites, MarkupState, UnparsedMarkup};
use crate::extract::bodies::{
    AccessMode, AccessPath, BodyKind, Handler, MemberAccess, MemberBody, NodeKind as BodyNodeKind,
    PathRoot, Range, Subscription, Tag,
};
use crate::graph::{Confidence, EdgeKind, FactKind, FactValue, Graph, NodeKey, Rule};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

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
    generic: Vec<GenericMarks>,
    triggers: Vec<MarkupTrigger>,
    /// The values markup could not take apart, by file.
    unparsed: BTreeMap<String, u64>,
}

/// The parameter types a member's key spells (`OnSave(object,EventArgs)`),
/// split at the commas outside generic arguments; `None` for a key with no
/// parameter list.
fn parameters(key: &str) -> Option<Vec<&str>> {
    let inner = key.strip_suffix(')')?;
    let open = inner.rfind('(')?;
    let list = &inner[open + 1..];
    if list.trim().is_empty() {
        return Some(Vec::new());
    }
    let mut out = Vec::new();
    let (mut depth, mut start) = (0usize, 0usize);
    for (i, c) in list.char_indices() {
        match c {
            '<' | '[' | '(' => depth += 1,
            '>' | ']' | ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                out.push(list[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(list[start..].trim());
    Some(out)
}

/// An event handler's parameters: `object` (or `object?`), then a type whose
/// last name ends in `EventArgs`.
fn is_handler_signature(key: &str) -> bool {
    match parameters(key).as_deref() {
        Some([sender, args]) => {
            let sender = sender.trim_end_matches('?');
            let args = args.split('<').next().unwrap_or(args);
            let last = args.rsplit('.').next().unwrap_or(args);
            matches!(sender, "object" | "Object" | "System.Object") && last.ends_with("EventArgs")
        }
        _ => false,
    }
}

/// The last name of an element's type as markup spells it: `Acme.Ui.Item`,
/// `http://…|MenuItem`, `MenuItem`.
fn last_type_name(name: &str) -> &str {
    name.rsplit(['.', '|']).next().unwrap_or(name)
}

/// The name WPF's menu item has; a type is a menu item when it is one or
/// derives from one (plan 2c, KTD15).
const MENU_ITEM: &str = "MenuItem";

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

    // ---- The words of a declaration (U8: R17) ----

    /// Every member of the run's C# types whose declaration gives a marker.
    fn generic(&mut self) {
        let ix = self.index();
        // The interfaces each type implements, by the relation the first
        // layer read — gathered into the set of faces below, whose order is
        // its own, so in no order of the graph's.
        let mut implements: BTreeMap<&NodeKey, Vec<&NodeKey>> = BTreeMap::new();
        for e in self.graph.edges_unordered() {
            if e.edge.kind == EdgeKind::Implements {
                implements.entry(e.from).or_default().push(e.to);
            }
        }
        let mut out = Vec::new();
        for ty in ix.csharp_types() {
            // The interfaces of the type and of its line of bases, and the
            // interfaces those extend, each once.
            let mut faces: BTreeSet<&NodeKey> = BTreeSet::new();
            let mut todo: Vec<&NodeKey> = std::iter::once(ty)
                .chain(ix.ancestors(ty))
                .flat_map(|t| implements.get(t).into_iter().flatten().copied())
                .collect();
            while let Some(face) = todo.pop() {
                if faces.insert(face) {
                    todo.extend(implements.get(face).into_iter().flatten().copied());
                }
            }
            let declared: BTreeSet<(&str, &str)> = faces
                .iter()
                .flat_map(|f| ix.members_of(f))
                .filter_map(|m| ix.symbol(m))
                .map(|s| (s.name.as_str(), s.kind.as_str()))
                .collect();
            for m in ix.members_of(ty) {
                let Some(s) = ix.symbol(m) else { continue };
                if s.is_type() {
                    continue;
                }
                let mut marks = Vec::new();
                if declared.contains(&(s.name.as_str(), s.kind.as_str())) {
                    marks.push(GenericMark::Interface);
                }
                let metas = ix.layer.metas_of(m);
                let has = |word: &str| {
                    metas
                        .iter()
                        .any(|(_, meta)| meta.modifiers.iter().any(|w| w == word))
                };
                if has("virtual") || has("abstract") {
                    marks.push(GenericMark::Virtual);
                }
                if has("override") {
                    marks.push(GenericMark::Override);
                }
                if s.is_method() && is_handler_signature(m.as_str()) {
                    marks.push(GenericMark::Handler);
                }
                if !marks.is_empty() {
                    out.push(GenericMarks {
                        member: m.clone(),
                        owner: ty.clone(),
                        marks,
                    });
                }
            }
        }
        self.generic = out;
    }

    // ---- The triggers of markup (U9: R9, KTD15) ----

    /// Every element with a command (`trigger: control`) and every key
    /// binding (`key-gesture`) of the markup bodies.
    fn markup_triggers(&mut self) {
        let layer = self.index().layer;
        let mut out = Vec::new();
        // The sites the walk told, by the element they are written at, built
        // once: a large dictionary has many elements and many sites.
        let mut told_at: BTreeMap<(&str, usize, u32), Vec<&MarkupSite>> = BTreeMap::new();
        for s in &self.sites {
            if let (Some(node), SiteOutcome::Resolved { .. }, false) =
                (s.node, &s.outcome, s.links.is_empty())
            {
                told_at
                    .entry((s.place.path.as_str(), s.body, node))
                    .or_default()
                    .push(s);
            }
        }
        for (path, file) in layer.files() {
            for (index, body) in file.model.members.iter().enumerate() {
                if body.kind != BodyKind::Markup {
                    continue;
                }
                let member = file
                    .owners
                    .get(index)
                    .and_then(Option::as_ref)
                    .map_or_else(|| NodeKey::new(path), |(k, _)| k.clone());
                // The root of a view with code-behind is written in its
                // class: the full name `x:Class` gives (the body's
                // `target-type`), as a template names the type it is for.
                let member = match tag_value(&body.tags, TARGET_TYPE) {
                    Some(class) if member.as_str().ends_with(X_CLASS_KEY) => NodeKey::new(class),
                    _ => member,
                };
                for node in &body.nodes {
                    let BodyNodeKind::Element(element) = &node.kind else {
                        continue;
                    };
                    let control = tag_value(&node.tags, TRIGGER) == Some("control");
                    if !control && !node.tags.iter().any(|t| t.name == KEY_GESTURE) {
                        continue;
                    }
                    let place = Place {
                        path: path.to_string(),
                        range: node.place,
                    };
                    let told: Vec<&MarkupSite> = told_at
                        .get(&(path, index, node.id))
                        .cloned()
                        .unwrap_or_default();
                    let last = |sites: &[&MarkupSite]| -> Vec<NodeKey> {
                        let mut v: Vec<NodeKey> = Vec::new();
                        for s in sites {
                            if let Some(l) = s.links.last() {
                                if !v.contains(l) {
                                    v.push(l.clone());
                                }
                            }
                        }
                        v
                    };
                    if control {
                        let key = tag_value(&node.tags, CONTROL_KEY)
                            .unwrap_or_else(|| last_type_name(element));
                        let kind = if self.is_menu_item(element) {
                            MarkupTriggerKind::MenuItem
                        } else {
                            MarkupTriggerKind::Control
                        };
                        out.push(MarkupTrigger {
                            kind,
                            key: key.to_string(),
                            member: member.clone(),
                            place: place.clone(),
                            targets: last(&told),
                        });
                    }
                    for gesture in tags(&node.tags, KEY_GESTURE) {
                        let Some(value) = &gesture.value else {
                            continue;
                        };
                        // The command as written, when an access spells it.
                        let named: Vec<&MarkupSite> = told
                            .iter()
                            .copied()
                            .filter(|s| gesture.args.first() == Some(&s.path.join(".")))
                            .collect();
                        let targets = if named.is_empty() {
                            last(&told)
                        } else {
                            last(&named)
                        };
                        out.push(MarkupTrigger {
                            kind: MarkupTriggerKind::Key,
                            key: value.clone(),
                            member: member.clone(),
                            place: place.clone(),
                            targets,
                        });
                    }
                }
            }
        }
        self.triggers = out;
    }

    /// Whether markup's element type `element` is a menu item: WPF's own by
    /// its name, or a C# type of exactly that full name whose base list, or
    /// that of a type it derives from, names one (KTD15: by the type, never
    /// by a list of controls).
    fn is_menu_item(&self, element: &str) -> bool {
        if last_type_name(element) == MENU_ITEM {
            return true;
        }
        let ix = self.index();
        let Ok(ty) = ix.type_exactly(element) else {
            return false;
        };
        std::iter::once(&ty).chain(ix.ancestors(&ty)).any(|t| {
            ix.layer.type_shapes_of(t).iter().any(|(_, shape)| {
                shape
                    .bases
                    .iter()
                    .any(|b| b.base.last().is_some_and(|l| l == MENU_ITEM))
            })
        })
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
                let unparsed = body
                    .nodes
                    .iter()
                    .flat_map(|n| &n.tags)
                    .filter(|t| t.name == UNPARSED_MARKUP)
                    .count() as u64;
                if unparsed > 0 {
                    *self.unparsed.entry(path.to_string()).or_default() += unparsed;
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
