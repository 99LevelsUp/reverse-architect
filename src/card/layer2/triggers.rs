//! The triggers of the core functionalities, found by walking back from each
//! functionality (plan 2b, U5: R11–R13; KTD5). Nothing here renders; the
//! registry of triggers (U7) and the `### Context` of a card (U6) read it.
//!
//! # What a trigger is (R13)
//!
//! - **an event** — a `+=` of a handler (the subscription), or of a lambda;
//! - **a key** — a branch of a handler whose condition compares a key
//!   (`key-compare`), or a `KeyGesture`/`KeyBinding` constructed into a
//!   command (`key-gesture`: `cmd.KeyGestures = new[] { new KeyGesture(Key.F4,
//!   ModifierKeys.Control) }`) — the command is the receiver of the write or
//!   the change the gesture is written in;
//! - **a menu item** — an object tagged `menu-item` given a command or a
//!   click handler;
//! - **an entry from another component** — a member the walk reaches that
//!   nothing in the corpus calls: the root of the chain of static callers.
//!
//! A command itself is no trigger: it is what a key or a menu item leads to.
//!
//! # The walk (KTD5)
//!
//! From every core functionality F the walk goes back from F's members over
//! the reverse index of callers ([`Callers`]) to the callers outside F's type,
//! and on from them, each member once per set of F's members it leads to (a
//! monotone worklist: a member is walked again only when it leads to more of
//! F than before, so a cycle of callers ends). Only what passes control
//! counts:
//!
//! - a call — a link of an access that is a call, a first-layer `Calls` or
//!   `Instantiates`, a method group read;
//! - a member reached through a value only when the value hosts a delegate
//!   (the command property a construction is bound in, U3) and the access
//!   goes on to a call (`this.viewModel.CheckCommand.Execute(null)`), or when
//!   it is assigned where a trigger takes it (`item.Command =
//!   viewModel.CheckCommand`, a gesture's command); a read of a state in a
//!   condition, or the source of a `+=`, passes nothing;
//! - a delegate bound from a class's code is entered from the member the
//!   construction is written in — the instance — never from the method of
//!   the class that calls the field: that method is shared by every instance
//!   of the class, and walking back through it would join two components
//!   that only share a command class (R4). Only the functionality of the
//!   command class itself walks from its method to every command built of the
//!   class (AE9).
//!
//! The walk stops at a trigger whose code holds the access (a key branch's
//! arm, a menu item's statements, a lambda subscribed), at a subscription of
//! the member, at a member nothing calls (an entry from another component,
//! active), and at a member only weak callers reach — `NameOnly`, `Ambiguous`,
//! a name over the ceiling: an entry from another component, unresolved. A
//! weak caller is never followed. A member with a dynamic-call marker with a
//! place (the model of step 2c, [`DynamicMarks`]) gets no row: the trigger
//! that calls it is written elsewhere (XAML).
//!
//! The walk is transitive: a trigger over a shared functionality gives a row
//! for every functionality it reaches (KTD5, risk table of the plan), and a
//! functionality's own entry points that are triggers are rows of it too (a
//! trigger whose code never leaves its type is still in the registry).
//!
//! # The nearest functionality (risk table of the plan, R-41)
//!
//! When the walk gives more than [`NEAREST_RATIO`] times as many rows as
//! occurrences of triggers (distinct [`Trigger::identity`], rows of C# only),
//! the run keeps only the way to the functionality nearest each trigger: a
//! row whose path passes through a member of a core functionality G of a
//! type that is neither the trigger's nor the target's leads to G instead —
//! the first such member from the trigger on, its path cut before it and the
//! member its one entry — and the functionalities reached past G lose the
//! row. A row of G's own walk for the same trigger is kept over the one cut
//! so; every occurrence keeps at least one row. Applied to the rows of the
//! full walk, by the data, deterministically: the path of a row is the one
//! the breadth-first walk recorded, so the nearest is nearest by the length
//! of that path; a member of several functionalities of one type (none
//! today: the clusters partition the members) counts for the least by type
//! key, then functionality key, in code points. The functionality that lost
//! a row says where the walk stopped ([`Narrowed`]), and the run says it
//! was cut ([`WalkCounts`]). Below the ratio nothing changes.
//!
//! Every list is ordered (by key in code points, then place) and nothing
//! iterates a hash map (KTD13).

use super::callers::{is_strong, BodySite, Callers, Evidence, Via};
use super::cluster::Functionality;
use super::dynamic::KEY_GESTURE;
use super::entry::{bodies, is_field_path, statements_of, EntryKind, EntryPoint, Label, Place};
use super::flow::written;
use super::link::{weakest, Links};
use super::resolve::{within, Index, Scope};
use super::TypeAnalysis;
use crate::card::explain::NAMED_IN_SENTENCE;
use crate::extract::bodies::{AccessMode, AccessPath, BodyKind, Condition, MemberBody, PathRoot};
use crate::graph::{Confidence, EdgeKind, NodeKey};
use serde::Serialize;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

/// The kinds of trigger (R13), in the order the registry lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TriggerKind {
    /// A `+=` of a handler or a lambda.
    Event,
    /// A key branch or a key gesture.
    Key,
    /// A menu item given a command or a click handler.
    MenuItem,
    /// A member nothing in the corpus calls: the root of a chain of callers.
    External,
}

/// A core functionality of a type: the type's key and the functionality's.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct FunctionalityRef {
    pub ty: NodeKey,
    pub key: String,
}

/// One occurrence of a trigger (R12).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Trigger {
    pub kind: TriggerKind,
    /// A key with its modifiers (`Key.F4+ModifierKeys.Control`), a menu
    /// item's label as written (a resource key, a literal) or its variable,
    /// an event's name, an external entry's member key.
    pub key: String,
    /// The member it is written in; for an external entry, the root.
    pub member: NodeKey,
    /// Where it is wired: the arm, the gesture's statement, the item's
    /// construction, the `+=`; for an external entry the root's body.
    pub place: Option<Place>,
    /// A key branch's condition: the trigger's own precondition (R7).
    pub condition: Option<Condition>,
    /// A key's modifiers (`ModifierKeys.Control`).
    pub modifiers: Vec<String>,
    /// An event's source as written; `None` for an event of the type itself.
    pub source: Option<AccessPath>,
    /// A menu item's label.
    pub label: Option<Label>,
}

impl Trigger {
    /// What tells one occurrence from another: kind, key, place, member.
    pub(crate) fn identity(&self) -> Identity {
        (
            self.kind,
            self.key.clone(),
            self.place.clone(),
            self.member.clone(),
        )
    }

    /// Whether `other` is the same occurrence.
    pub fn same(&self, other: &Trigger) -> bool {
        self.identity() == other.identity()
    }
}

pub(crate) type Identity = (TriggerKind, String, Option<Place>, NodeKey);

/// Whether a trigger's way to a functionality is open (R12).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PathState {
    Active,
    /// It can never run: the member and the place that turn it off (a
    /// CanExecute that returns `false`, an empty target). Told by U5's
    /// composition of the flow (`context`), from the dead paths of 2a.
    Dead {
        member: NodeKey,
        place: Place,
    },
    /// Only weak callers lead to the root: the members they are, by key.
    ///
    /// One list per root, shared by every row that reaches that root: a root
    /// a name past the ceiling calls has hundreds of weak callers, and the
    /// walk is transitive, so the same root is the row of hundreds of
    /// functionalities — a list copied into each row grew with their product.
    Unresolved {
        callers: Arc<[NodeKey]>,
    },
}

/// One member on a trigger's way to a functionality, and where it calls on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Hop {
    pub member: NodeKey,
    /// The access or call that leads on; `None` for a first-layer relation,
    /// which carries no place of a body.
    pub place: Option<Place>,
    /// The grade of that link; `None` when the hop has none (a root's own
    /// body).
    pub grade: Option<Confidence>,
}

/// One row of the registry of triggers: a trigger and a functionality it
/// leads to (R11, R12). Nothing is merged: two occurrences of one key are two
/// rows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TriggerRow {
    pub trigger: Trigger,
    pub target: FunctionalityRef,
    pub state: PathState,
    /// From the trigger's member to the last member before the
    /// functionality; empty for a trigger that is one of its own entry points.
    pub path: Vec<Hop>,
    /// The functionality's members the way enters at, by key; for its own
    /// entry point, the member the entry is written in and the members of the
    /// functionality its code names (the command a menu item is given among
    /// them); for a gesture given to its command, that command.
    pub entries: Vec<NodeKey>,
    /// The weakest grade of the way; `None` for none.
    pub grade: Option<Confidence>,
    /// The trigger is written in the functionality's own type.
    pub own: bool,
}

/// The model of dynamic-call markers of step 2c: whether a member is called
/// from a place another language writes (`Command="{Binding Check}"`).
/// Without 2c no member is ([`NoMarks`]).
pub trait DynamicMarks {
    fn marked(&self, member: &NodeKey) -> bool;
}

/// No dynamic-call markers: the run without step 2c.
pub struct NoMarks;

impl DynamicMarks for NoMarks {
    fn marked(&self, _: &NodeKey) -> bool {
        false
    }
}

impl DynamicMarks for BTreeSet<NodeKey> {
    fn marked(&self, member: &NodeKey) -> bool {
        self.contains(member)
    }
}

/// A trigger whose code holds some accesses: the nodes of one body whose
/// calls (`calls`) or whose plain reads (`names`, a command assigned) lead
/// through it.
#[derive(Debug, Clone)]
struct Recognized {
    trigger: Trigger,
    path: String,
    body: usize,
    calls: Vec<u32>,
    names: Vec<u32>,
}

impl Recognized {
    /// Whether the site is under one of `roots`.
    fn holds(&self, ix: &Index<'_>, s: &BodySite, names: bool) -> bool {
        let roots = if names { &self.names } else { &self.calls };
        s.place.path == self.path
            && s.body == self.body
            && s.node.is_some_and(|n| {
                ix.body_at(&self.path, self.body)
                    .is_some_and(|b| roots.iter().any(|&r| within(b, n, r)))
            })
    }
}

/// A menu item's key: its label as written, else its variable.
fn label_key(label: Option<&Label>, variable: &str) -> String {
    match label {
        Some(Label::Literal { value, .. }) => value.clone(),
        Some(Label::ResourceKey { key, .. }) => key.clone(),
        Some(Label::Expression { path }) => written(path),
        _ => variable.to_string(),
    }
}

/// The `+=` of `event` written in `member` at `place`: its source as written.
fn subscription_source(ix: &Index<'_>, member: &NodeKey, place: &Place) -> Option<AccessPath> {
    bodies(ix, member).into_iter().find_map(|at| {
        (at.path == place.path)
            .then(|| {
                at.body
                    .subscriptions
                    .iter()
                    .find(|s| s.place == place.range)
                    .and_then(|s| s.source.clone())
            })
            .flatten()
    })
}

fn event_trigger(
    event: &str,
    member: &NodeKey,
    place: &Place,
    source: Option<AccessPath>,
) -> Trigger {
    Trigger {
        kind: TriggerKind::Event,
        key: event.to_string(),
        member: member.clone(),
        place: Some(place.clone()),
        condition: None,
        modifiers: Vec::new(),
        source,
        label: None,
    }
}

fn external(ix: &Index<'_>, member: &NodeKey) -> Trigger {
    let place = bodies(ix, member).into_iter().find_map(|at| {
        at.body.nodes.get(at.body.root as usize).map(|n| Place {
            path: at.path.to_string(),
            range: n.place,
        })
    });
    Trigger {
        kind: TriggerKind::External,
        key: member.as_str().to_string(),
        member: member.clone(),
        place,
        condition: None,
        modifiers: Vec::new(),
        source: None,
        label: None,
    }
}

/// The trigger an entry point of 2a is, with the code it holds; `None` for a
/// command property and a public method (no trigger) and for a handler
/// subscribed by name (its subscriptions are, see [`Corpus::own_rows`]).
fn entry_trigger(ix: &Index<'_>, e: &EntryPoint) -> Option<Recognized> {
    let region = e.regions.first()?;
    let body = ix.body_at(&region.path, region.body)?;
    let (trigger, calls, names) = match e.kind {
        EntryKind::Key => (
            Trigger {
                kind: TriggerKind::Key,
                key: e.discriminator.clone()?,
                member: e.member.clone(),
                place: e.place.clone(),
                condition: e.condition.clone(),
                modifiers: e.modifiers.clone(),
                source: None,
                label: None,
            },
            region.node.into_iter().collect(),
            Vec::new(),
        ),
        EntryKind::MenuItem => {
            let menu = e.menu.as_ref()?;
            let v = menu.variable.as_str();
            let mut calls: Vec<u32> = statements_of(body, v).into_iter().collect();
            calls.extend(region.node);
            let names = body
                .accesses
                .iter()
                .filter(|a| {
                    a.mode == AccessMode::Write
                        && a.target.root == PathRoot::Local
                        && a.target.segments == [v, "Command"]
                })
                .filter_map(|a| a.node)
                .collect();
            (
                Trigger {
                    kind: TriggerKind::MenuItem,
                    key: label_key(menu.label.as_ref(), v),
                    member: e.member.clone(),
                    place: e.place.clone(),
                    condition: None,
                    modifiers: Vec::new(),
                    source: None,
                    label: menu.label.clone(),
                },
                calls,
                names,
            )
        }
        EntryKind::Handler if e.discriminator.is_some() => {
            let site = e.subscriptions.first()?;
            (
                event_trigger(&site.event, &e.member, &site.place, site.source.clone()),
                region.node.into_iter().collect(),
                Vec::new(),
            )
        }
        _ => return None,
    };
    Some(Recognized {
        trigger,
        path: region.path.clone(),
        body: region.body,
        calls,
        names,
    })
}

/// How an evidence of a caller passes control to the member it names.
enum Way<'e> {
    /// A call, or an access that goes on to one; the site when it has one.
    Calls(Option<&'e BodySite>),
    /// A plain read or write.
    Names(&'e BodySite),
    /// A `+=` of the member.
    Subscribed { event: &'e str, place: &'e Place },
    /// A delegate the construction in `built_in` binds the member to.
    Delegate { built_in: &'e NodeKey },
    /// Nothing that passes control.
    No,
}

fn way(e: &Evidence) -> Way<'_> {
    match &e.via {
        Via::Relation {
            kind: EdgeKind::Calls | EdgeKind::Instantiates,
            ..
        } => Way::Calls(None),
        Via::Relation { .. } => Way::No,
        Via::Link(s) | Via::Candidate(s) => {
            if s.call.is_some() {
                Way::Calls(Some(s))
            } else {
                Way::Names(s)
            }
        }
        Via::Delegate { built_in, .. } => Way::Delegate { built_in },
        Via::Subscription { event, place, .. } => Way::Subscribed { event, place },
    }
}

/// The methods of a class that call a delegate field and the members whose
/// construction binds it (U3), with the construction's place: what the
/// functionality of a command class walks back from (AE9). From the
/// [`Via::Delegate`] evidence of the index of callers.
pub(crate) fn delegate_hosts(callers: &Callers) -> BTreeMap<NodeKey, BTreeMap<NodeKey, Place>> {
    let mut out: BTreeMap<NodeKey, BTreeMap<NodeKey, Place>> = BTreeMap::new();
    for m in callers.members() {
        for c in callers.of(m) {
            for e in &c.evidence {
                if let Via::Delegate {
                    built_in,
                    construction,
                    ..
                } = &e.via
                {
                    out.entry(c.member.clone())
                        .or_default()
                        .entry(built_in.clone())
                        .or_insert_with(|| construction.clone());
                }
            }
        }
    }
    out
}

/// What the walk reads of the corpus, built once.
struct Corpus<'a, 'i, 'l> {
    links: &'a Links<'i, 'l>,
    callers: &'a Callers,
    marks: &'a dyn DynamicMarks,
    /// The triggers holding code, by the member they are written in.
    recognized: BTreeMap<NodeKey, Vec<Recognized>>,
    /// The key gestures, by the command they are given to — the member
    /// written, and the property whose getter returns that field.
    gestures: BTreeMap<NodeKey, Vec<Trigger>>,
    /// Invoker → the members whose constructions bind it, with the place.
    hosts_of: BTreeMap<NodeKey, BTreeMap<NodeKey, Place>>,
    /// Every member a construction bound to a delegate is written in.
    hosts: BTreeSet<NodeKey>,
    /// The lists of weak callers of the unresolved roots, each once
    /// ([`PathState::Unresolved`]).
    weak_lists: RefCell<BTreeSet<Arc<[NodeKey]>>>,
}

/// One trigger the walk met, and the members it leads to F through.
struct Hit {
    trigger: Trigger,
    from: BTreeSet<NodeKey>,
    /// The first member reached from the trigger, where and how sure.
    first: Option<(NodeKey, Option<Place>, Option<Confidence>)>,
    state: PathState,
}

/// The walk back from one functionality.
struct Walk<'w> {
    ty: &'w NodeKey,
    /// The members of F each walked member leads to.
    entries: BTreeMap<NodeKey, BTreeSet<NodeKey>>,
    /// The member each walked member calls on toward F, where, how sure.
    parent: BTreeMap<NodeKey, (NodeKey, Option<Place>, Option<Confidence>)>,
    queue: VecDeque<NodeKey>,
    hits: BTreeMap<Identity, Hit>,
}

impl Walk<'_> {
    fn reach(
        &mut self,
        y: &NodeKey,
        from: &NodeKey,
        place: Option<Place>,
        grade: Option<Confidence>,
    ) {
        let add = self.entries.get(from).cloned().unwrap_or_default();
        let set = self.entries.entry(y.clone()).or_default();
        let before = set.len();
        set.extend(add);
        if set.len() != before {
            self.parent
                .entry(y.clone())
                .or_insert_with(|| (from.clone(), place, grade));
            self.queue.push_back(y.clone());
        }
    }

    fn hit(
        &mut self,
        trigger: Trigger,
        from: &NodeKey,
        first: Option<(NodeKey, Option<Place>, Option<Confidence>)>,
        state: PathState,
    ) {
        let h = self.hits.entry(trigger.identity()).or_insert_with(|| Hit {
            trigger,
            from: BTreeSet::new(),
            first,
            state,
        });
        h.from.insert(from.clone());
    }
}

/// What the callers of a member give the walk.
#[derive(Default)]
struct Ways {
    /// Some caller passes control to it at all, strong or weak.
    any: bool,
    /// Some strong caller does.
    strong: bool,
    /// The weak callers, by key.
    weak: BTreeSet<NodeKey>,
}

impl Ways {
    /// The state of a root these callers make: active when none passes
    /// control, unresolved when only weak ones do, `None` when a strong one
    /// does. The list of weak callers is taken from `shared` when an equal
    /// one is there, so every row with the same weak callers holds one list.
    fn root(&self, shared: &RefCell<BTreeSet<Arc<[NodeKey]>>>) -> Option<PathState> {
        if !self.any {
            Some(PathState::Active)
        } else if !self.strong {
            let weak: Vec<NodeKey> = self.weak.iter().cloned().collect();
            let mut shared = shared.borrow_mut();
            let callers = match shared.get(weak.as_slice()) {
                Some(list) => Arc::clone(list),
                None => {
                    let list: Arc<[NodeKey]> = weak.into();
                    shared.insert(Arc::clone(&list));
                    list
                }
            };
            Some(PathState::Unresolved { callers })
        } else {
            None
        }
    }
}

impl<'a, 'i, 'l> Corpus<'a, 'i, 'l> {
    fn ix(&self) -> &'i Index<'l> {
        self.links.index()
    }

    fn new(
        types: &[TypeAnalysis],
        links: &'a Links<'i, 'l>,
        callers: &'a Callers,
        marks: &'a dyn DynamicMarks,
    ) -> Self {
        let ix = links.index();
        let mut recognized: BTreeMap<NodeKey, Vec<Recognized>> = BTreeMap::new();
        for t in types {
            for f in &t.functionalities {
                for e in &f.entry_points {
                    if let Some(r) = entry_trigger(ix, e) {
                        recognized.entry(e.member.clone()).or_default().push(r);
                    }
                }
            }
        }
        let mut gestures: BTreeMap<NodeKey, Vec<Trigger>> = BTreeMap::new();
        for ty in ix.read_types() {
            for m in ix.members_of(ty) {
                if ix.symbol(m).is_none_or(|s| s.is_type()) {
                    continue;
                }
                for at in bodies(ix, m) {
                    for n in &at.body.nodes {
                        for tag in n.tags.iter().filter(|t| t.name == KEY_GESTURE) {
                            let Some(value) = &tag.value else { continue };
                            let place = Place {
                                path: at.path.to_string(),
                                range: n.place,
                            };
                            let trigger = Trigger {
                                kind: TriggerKind::Key,
                                key: std::iter::once(value.as_str())
                                    .chain(tag.args.iter().map(String::as_str))
                                    .collect::<Vec<_>>()
                                    .join("+"),
                                member: m.clone(),
                                place: Some(place),
                                condition: None,
                                modifiers: tag.args.clone(),
                                source: None,
                                label: None,
                            };
                            for command in gesture_commands(links, ty, m, at.path, at.body, n.id) {
                                gestures.entry(command).or_default().push(trigger.clone());
                            }
                            recognized.entry(m.clone()).or_default().push(Recognized {
                                trigger,
                                path: at.path.to_string(),
                                body: at.index,
                                calls: Vec::new(),
                                names: vec![n.id],
                            });
                        }
                    }
                }
            }
        }
        for list in gestures.values_mut() {
            list.sort_by_key(Trigger::identity);
            list.dedup_by(|a, b| a.same(b));
        }
        let hosts_of = delegate_hosts(callers);
        let hosts = hosts_of.values().flat_map(|b| b.keys().cloned()).collect();
        Corpus {
            links,
            callers,
            marks,
            recognized,
            gestures,
            hosts_of,
            hosts,
            weak_lists: RefCell::new(BTreeSet::new()),
        }
    }

    /// The trigger written in `caller` whose code holds `site`.
    fn covering(&self, caller: &NodeKey, site: &BodySite, names: bool) -> Option<&Trigger> {
        self.recognized
            .get(caller)?
            .iter()
            .find(|r| r.holds(self.ix(), site, names))
            .map(|r| &r.trigger)
    }

    fn is_method(&self, m: &NodeKey) -> bool {
        self.ix()
            .symbol(m)
            .is_some_and(|s| matches!(s.kind.as_str(), "method" | "constructor"))
    }

    fn is_property(&self, m: &NodeKey) -> bool {
        self.ix().symbol(m).is_some_and(|s| s.kind == "property")
    }

    /// The rows of the functionality `f` of the type `ty` (see the module
    /// docs).
    fn rows_of(&self, ty: &NodeKey, f: &Functionality) -> Vec<TriggerRow> {
        let ix = self.ix();
        let target = FunctionalityRef {
            ty: ty.clone(),
            key: f.key.clone(),
        };
        let members: BTreeSet<&NodeKey> = f.members.iter().collect();
        let mut walk = Walk {
            ty,
            entries: BTreeMap::new(),
            parent: BTreeMap::new(),
            queue: VecDeque::new(),
            hits: BTreeMap::new(),
        };
        let mut rows: BTreeMap<Identity, TriggerRow> = BTreeMap::new();

        // Its own entry points that are triggers, and the gestures given to
        // its commands: each enters F where its code does, never at every
        // member of F — the shared part, and the dead path in it, are what
        // that trigger reaches (R12).
        for (trigger, entered) in self.own_triggers(f) {
            let row = rows
                .entry(trigger.identity())
                .or_insert_with(|| TriggerRow {
                    trigger,
                    target: target.clone(),
                    state: PathState::Active,
                    path: Vec::new(),
                    entries: Vec::new(),
                    grade: None,
                    own: true,
                });
            // One gesture given to a field and to the property returning it
            // is one trigger entering at both.
            let mut entries: BTreeSet<NodeKey> =
                std::mem::take(&mut row.entries).into_iter().collect();
            entries.extend(entered);
            row.entries = entries.into_iter().collect();
        }

        for m in &f.members {
            walk.entries.entry(m.clone()).or_default().insert(m.clone());
            walk.queue.push_back(m.clone());
        }
        // A command class walks on from its methods to every command built
        // of it (AE9).
        for m in &f.members {
            for (built_in, place) in self.hosts_of.get(m).into_iter().flatten() {
                if ix.owner_of(built_in) != Some(ty) {
                    walk.reach(
                        built_in,
                        m,
                        Some(place.clone()),
                        Some(Confidence::RulesBacked),
                    );
                }
            }
        }

        // Its own ways in that may be roots: a public method or a command
        // property — nothing calls it, or only weak callers do.
        let identities: BTreeSet<&NodeKey> = f
            .entry_points
            .iter()
            .filter(|e| {
                e.discriminator.is_none()
                    && matches!(e.kind, EntryKind::PublicMethod | EntryKind::Command)
            })
            .map(|e| &e.member)
            .collect();

        while let Some(x) = walk.queue.pop_front() {
            let start = members.contains(&x);
            let marked = self.marks.marked(&x);
            if !start {
                if marked {
                    // Called from where step 2c writes the trigger: no row.
                    continue;
                }
                for g in self.gestures.get(&x).into_iter().flatten() {
                    let first = (x.clone(), g.place.clone(), None);
                    walk.hit(g.clone(), &x, Some(first), PathState::Active);
                }
            }
            let ways = self.ways(&mut walk, &x, start);
            let rootable = !marked && (!start || identities.contains(&x));
            let reached = self.hosts_of.contains_key(&x) || self.gestures.contains_key(&x);
            if rootable && !reached {
                if let Some(state) = ways.root(&self.weak_lists) {
                    walk.hit(external(ix, &x), &x, None, state);
                }
            }
        }

        for (identity, h) in std::mem::take(&mut walk.hits) {
            let mut entries: BTreeSet<NodeKey> = BTreeSet::new();
            for from in &h.from {
                entries.extend(walk.entries.get(from).into_iter().flatten().cloned());
            }
            let path = path_of(&walk, &members, &h);
            let grade = weakest(path.iter().filter_map(|hop| hop.grade));
            let own = ix.owner_of(&h.trigger.member) == Some(ty);
            rows.entry(identity).or_insert_with(|| TriggerRow {
                trigger: h.trigger,
                target: target.clone(),
                state: h.state,
                path,
                entries: entries.into_iter().collect(),
                grade,
                own,
            });
        }
        rows.into_values().collect()
    }

    /// The triggers among `f`'s own entry points, and the gestures given to
    /// its members, each with the members of `f` it enters at: an entry
    /// point's member and the members of `f` its targets name (a menu item's
    /// command, a delegate's target, a call), a gesture's command.
    fn own_triggers(&self, f: &Functionality) -> Vec<(Trigger, BTreeSet<NodeKey>)> {
        let ix = self.ix();
        let entered = |e: &EntryPoint| -> BTreeSet<NodeKey> {
            std::iter::once(e.member.clone())
                .chain(
                    e.targets
                        .iter()
                        .filter_map(|t| t.member.as_ref())
                        .filter(|m| f.members.contains(m))
                        .cloned(),
                )
                .collect()
        };
        let mut out = Vec::new();
        for e in &f.entry_points {
            if let Some(r) = entry_trigger(ix, e) {
                out.push((r.trigger, entered(e)));
                continue;
            }
            if e.kind == EntryKind::Handler && e.discriminator.is_none() {
                for site in &e.subscriptions {
                    let subscriber = self
                        .callers
                        .of(&e.member)
                        .iter()
                        .find(|c| {
                            c.evidence.iter().any(|ev| {
                                matches!(&ev.via, Via::Subscription { place, .. } if *place == site.place)
                            })
                        })
                        .map_or_else(|| e.member.clone(), |c| c.member.clone());
                    let trigger =
                        event_trigger(&site.event, &subscriber, &site.place, site.source.clone());
                    out.push((trigger, entered(e)));
                }
            }
        }
        for m in &f.members {
            for g in self.gestures.get(m).into_iter().flatten() {
                out.push((g.clone(), BTreeSet::from([m.clone()])));
            }
        }
        out
    }

    /// Read the callers of `x`, walk on to the strong ones outside F's type
    /// and record the triggers that hold their sites; what they give the
    /// root of `x`. `start` says `x` is a member of F (its own reads and
    /// writes of state pass nothing).
    fn ways(&self, walk: &mut Walk<'_>, x: &NodeKey, start: bool) -> Ways {
        let ix = self.ix();
        let method = self.is_method(x);
        let host = self.hosts.contains(x) || self.gestures.contains_key(x);
        // A property outside F reached because its accessor calls into F:
        // reading or writing it runs that accessor.
        let accessor = !start && !host && self.is_property(x);
        let mut out = Ways::default();
        for c in self.callers.of(x) {
            let inside = ix.owner_of(&c.member) == Some(walk.ty);
            for e in &c.evidence {
                let grade = Some(e.grade);
                // What passes control, and the site and the kind of cover it
                // is looked up in.
                let (site, names) = match way(e) {
                    Way::No => continue,
                    Way::Subscribed { event, place } => {
                        if out.pass(&c.member, e.grade) {
                            let source = subscription_source(ix, &c.member, place);
                            let trigger = event_trigger(event, &c.member, place, source);
                            let first = (x.clone(), Some(place.clone()), grade);
                            walk.hit(trigger, x, Some(first), PathState::Active);
                        }
                        continue;
                    }
                    Way::Delegate { built_in } => {
                        if out.pass(&c.member, e.grade) && ix.owner_of(built_in) != Some(walk.ty) {
                            walk.reach(built_in, x, None, grade);
                        }
                        continue;
                    }
                    Way::Calls(site) if method || host || accessor => (site, false),
                    Way::Names(site)
                        if method || accessor || self.covering(&c.member, site, true).is_some() =>
                    {
                        (Some(site), true)
                    }
                    Way::Calls(_) | Way::Names(_) => continue,
                };
                if !out.pass(&c.member, e.grade) {
                    continue;
                }
                let place = site.map(|s| s.place.clone());
                if let Some(t) = site.and_then(|s| self.covering(&c.member, s, names)) {
                    let first = (x.clone(), place, grade);
                    walk.hit(t.clone(), x, Some(first), PathState::Active);
                } else if !inside {
                    walk.reach(&c.member, x, place, grade);
                }
            }
        }
        // A name over the ceiling calls, weakly, whatever a call passes
        // control to: kept for the root, never followed.
        if method || host || accessor {
            for c in self.callers.over_ceiling_of(x) {
                out.pass(c, Confidence::Ambiguous);
            }
        }
        out
    }
}

impl Ways {
    /// A caller passing control at `grade`: whether it is strong — a weak one
    /// is kept for the root, never followed.
    fn pass(&mut self, caller: &NodeKey, grade: Confidence) -> bool {
        self.any = true;
        if is_strong(grade) {
            self.strong = true;
            true
        } else {
            self.weak.insert(caller.clone());
            false
        }
    }
}

/// The commands a key gesture constructed at `node` of `member`'s body is
/// given to: the receiver of a write or a change at the node with a name
/// after it (`cmd.KeyGestures = …`, `cmd.InputGestures.Add(…)`), resolved
/// (U2); and the property whose getter returns that field.
fn gesture_commands(
    links: &Links<'_, '_>,
    ty: &NodeKey,
    member: &NodeKey,
    path: &str,
    body: &MemberBody,
    node: u32,
) -> Vec<NodeKey> {
    let ix = links.index();
    let mut out: BTreeSet<NodeKey> = BTreeSet::new();
    for a in &body.accesses {
        if a.node != Some(node) || !matches!(a.mode, AccessMode::Write | AccessMode::Mutate) {
            continue;
        }
        let local = usize::from(a.target.root == PathRoot::Local);
        if a.target.segments.len() < local + 2 {
            continue;
        }
        let receiver = AccessPath {
            root: a.target.root,
            segments: a.target.segments[..a.target.segments.len() - 1].to_vec(),
        };
        let scope = Scope {
            ty,
            body: Some(body),
            node: a.node,
        };
        let place = Place {
            path: path.to_string(),
            range: a.place,
        };
        let r = links.resolve(member, &scope, &receiver, None, &place);
        if let (Some(command), true) = (r.target(), r.grade().is_some_and(is_strong)) {
            out.insert(command.clone());
            if let Some(owner) = ix.owner_of(command) {
                let name = ix.name_of(command);
                for p in ix.members_of(owner) {
                    let returns = ix.symbol(p).is_some_and(|s| s.kind == "property")
                        && ix.layer.bodies_of(p).iter().any(|pb| {
                            matches!(
                                pb.body.kind,
                                BodyKind::Getter | BodyKind::ExpressionProperty
                            ) && pb.body.accesses.iter().any(|x| {
                                x.mode == AccessMode::Return
                                    && is_field_path(&x.target) == Some(name)
                            })
                        });
                    if returns && p != command {
                        out.insert(p.clone());
                    }
                }
            }
        }
    }
    out.into_iter().collect()
}

/// The way of a hit from its trigger to F: the trigger's member where it
/// calls on, then each member walked, until a member of F.
fn path_of(walk: &Walk<'_>, members: &BTreeSet<&NodeKey>, h: &Hit) -> Vec<Hop> {
    let mut out = Vec::new();
    let mut at = match &h.first {
        Some((next, place, grade)) => {
            out.push(Hop {
                member: h.trigger.member.clone(),
                place: place.clone(),
                grade: *grade,
            });
            next.clone()
        }
        None => h.trigger.member.clone(),
    };
    for _ in 0..=walk.parent.len() {
        if members.contains(&at) {
            break;
        }
        let Some((next, place, grade)) = walk.parent.get(&at) else {
            break;
        };
        if out.last().is_none_or(|hop: &Hop| hop.member != at) {
            out.push(Hop {
                member: at.clone(),
                place: place.clone(),
                grade: *grade,
            });
        }
        at = next.clone();
    }
    // Kept on every row of the registry, which a large repository has
    // millions of: no slack from the pushes above.
    out.shrink_to_fit();
    out
}

/// How many times as many rows as occurrences of triggers the walk may give
/// before the run keeps only the nearest functionality (see the module docs).
pub const NEAREST_RATIO: usize = 10;

/// What the walk back gave the run (see the module docs): the rows of the
/// full walk, the occurrences of triggers among them, the rows kept, and
/// whether the rule of the nearest functionality cut them. Rows of C# only;
/// the registry adds the rows of markup.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WalkCounts {
    pub transitive_rows: usize,
    pub occurrences: usize,
    pub rows: usize,
    pub nearest: bool,
}

/// Where the walk back from a functionality stopped under the rule of the
/// nearest functionality: the functionalities of other types its rows were
/// moved to, the first [`NAMED_IN_SENTENCE`] by type and key, and how many.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Narrowed {
    pub at: Vec<FunctionalityRef>,
    pub total: usize,
}

/// The rows of every core functionality, what the walk gave, and where it
/// stopped (see the module docs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// Sorted for the registry (R11).
    pub rows: Vec<TriggerRow>,
    /// Empty unless the walk was cut ([`WalkCounts::nearest`]).
    pub narrowed: BTreeMap<FunctionalityRef, Narrowed>,
    pub walk: WalkCounts,
}

/// The order of the registry (R11): by kind, key, the file and place of the
/// trigger, its member, then the functionality.
fn registry_order(a: &TriggerRow, b: &TriggerRow) -> std::cmp::Ordering {
    (
        a.trigger.kind,
        &a.trigger.key,
        a.trigger.place.as_ref(),
        &a.trigger.member,
        &a.target,
    )
        .cmp(&(
            b.trigger.kind,
            &b.trigger.key,
            b.trigger.place.as_ref(),
            &b.trigger.member,
            &b.target,
        ))
}

/// The rows of every core functionality of `types` (see the module docs),
/// sorted for the registry (R11): by kind, key, the file and place of the
/// trigger, its member, then the functionality — cut at the nearest
/// functionality when the full walk gives more than [`NEAREST_RATIO`] times
/// as many rows as occurrences.
pub fn find(
    types: &[TypeAnalysis],
    links: &Links<'_, '_>,
    callers: &Callers,
    marks: &dyn DynamicMarks,
) -> Found {
    let corpus = Corpus::new(types, links, callers, marks);
    let mut out: Vec<TriggerRow> = types
        .iter()
        .flat_map(|t| {
            t.functionalities
                .iter()
                .flat_map(|f| corpus.rows_of(&t.key, f))
                .collect::<Vec<_>>()
        })
        .collect();
    out.sort_by(registry_order);
    // The identity is a prefix of the order: the occurrences are the runs of
    // equal identities, counted without a copy of any.
    let occurrences = usize::from(!out.is_empty())
        + out
            .windows(2)
            .filter(|w| !w[0].trigger.same(&w[1].trigger))
            .count();
    let transitive_rows = out.len();
    let mut walk = WalkCounts {
        transitive_rows,
        occurrences,
        rows: transitive_rows,
        nearest: false,
    };
    if transitive_rows <= NEAREST_RATIO * occurrences {
        return Found {
            rows: out,
            narrowed: BTreeMap::new(),
            walk,
        };
    }
    let (rows, narrowed) = nearest(types, links.index(), out);
    walk.rows = rows.len();
    walk.nearest = true;
    Found {
        rows,
        narrowed,
        walk,
    }
}

/// The rows of the full walk kept at the functionality nearest each trigger
/// (see the module docs), sorted for the registry, and where each
/// functionality's walk stopped.
fn nearest(
    types: &[TypeAnalysis],
    ix: &Index<'_>,
    rows: Vec<TriggerRow>,
) -> (Vec<TriggerRow>, BTreeMap<FunctionalityRef, Narrowed>) {
    // Every core functionality by type and key, and the one of each member.
    let mut table: Vec<FunctionalityRef> = types
        .iter()
        .flat_map(|t| {
            t.functionalities.iter().map(|f| FunctionalityRef {
                ty: t.key.clone(),
                key: f.key.clone(),
            })
        })
        .collect();
    table.sort();
    let at: BTreeMap<&FunctionalityRef, usize> =
        table.iter().enumerate().map(|(i, r)| (r, i)).collect();
    let mut of: BTreeMap<&NodeKey, usize> = BTreeMap::new();
    for t in types {
        for f in &t.functionalities {
            let r = FunctionalityRef {
                ty: t.key.clone(),
                key: f.key.clone(),
            };
            let i = at[&r];
            for m in &f.members {
                // The least by type and key, should a member be in several.
                let j = of.entry(m).or_insert(i);
                *j = (*j).min(i);
            }
        }
    }

    let mut cut: BTreeSet<(usize, usize)> = BTreeSet::new();
    let mut kept: Vec<(TriggerRow, bool)> = Vec::with_capacity(rows.len());
    for mut row in rows {
        let own_ty = ix.owner_of(&row.trigger.member);
        let stop = row.path.iter().position(|hop| {
            of.get(&hop.member)
                .is_some_and(|&g| Some(&table[g].ty) != own_ty && table[g].ty != row.target.ty)
        });
        let Some(k) = stop else {
            kept.push((row, false));
            continue;
        };
        let g = of[&row.path[k].member];
        if let Some(&from) = at.get(&row.target) {
            cut.insert((from, g));
        }
        row.entries = vec![row.path[k].member.clone()];
        row.path.truncate(k);
        row.path.shrink_to_fit();
        row.grade = weakest(row.path.iter().filter_map(|hop| hop.grade));
        row.own = false;
        row.target = table[g].clone();
        kept.push((row, true));
    }
    // A row of the nearest functionality's own walk first, then one moved
    // there; the sort is stable, so equal rows keep the order of the walk.
    kept.sort_by(|a, b| registry_order(&a.0, &b.0).then(a.1.cmp(&b.1)));
    kept.dedup_by(|b, a| a.0.trigger.same(&b.0.trigger) && a.0.target == b.0.target);

    let mut narrowed: BTreeMap<FunctionalityRef, Narrowed> = BTreeMap::new();
    for (from, g) in cut {
        let n = narrowed.entry(table[from].clone()).or_insert(Narrowed {
            at: Vec::new(),
            total: 0,
        });
        if n.at.len() < NAMED_IN_SENTENCE {
            n.at.push(table[g].clone());
        }
        n.total += 1;
    }
    (kept.into_iter().map(|(r, _)| r).collect(), narrowed)
}
