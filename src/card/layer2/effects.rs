//! The results of a functionality and the state touched from outside (plan
//! 2b, U4: R5, R6, R10; KTD6). Nothing here renders; U5 composes the flows
//! from it and U6 draws them.
//!
//! # What a result is (R5, in the order of how sure it is)
//!
//! An effect seen outside the functionality, at the member and the place of
//! the code that has it:
//!
//! 1. **a write of a property that notifies its change** (`notifies-change`
//!    on the property, from the language's plugin) — and, as sure, **an event
//!    raised** (U3's [`raised_events`]: a method of the type calling a value
//!    member no constructor writes from a parameter, `Executed?.Invoke(…)`),
//!    with how many subscriptions the whole corpus makes to that event on the
//!    resolved type of their source ([`Callers::subscribers_of`]). The flow
//!    stops there; the subscribers are triggers of their own (R6);
//! 2. **a write of state another functionality or another type reads** — a
//!    field or property written (or changed through a call on it,
//!    `items.Add(x)`) that a member outside the functionality's cluster
//!    (plan 2a, KTD12) reads. A read counts unless, at the top level of the
//!    reading body, an unconditional write of the same member comes before it
//!    — the conservative approximation of "on every path" (KTD6): a read in
//!    one branch of an `if` after a write in another counts. A getter that
//!    returns the member stands for it: its readers are the member's;
//! 3. **an access to the database**: a `Uses` relation of the first layer
//!    from a member of the functionality to a database object;
//! 4. **an exception out of the functionality** — a `throw` not inside a `try`
//!    with a `catch` of the same member — and **the value returned** by a
//!    method that is an entry point of the functionality (what a trigger
//!    calls);
//! 5. **nothing happens**: a [`StoppedPath`] — a precondition or a dead path
//!    of U11 stops the main work. The results that still happen on that path
//!    are the writes and the events raised before the guard (KTD13 1 of plan
//!    2a) and the guard's own `throw`.
//!
//! # What is no result (R6)
//!
//! - a call of another type, whatever is done with its result — a step of
//!   the flow;
//! - a write into an object the functionality constructed itself and hands on
//!   (`var a = new Args(); a.Text = x; sink.Take(a);`);
//! - a write of a field tagged `lazy-init` (resetting a lazily built value);
//! - the setter's own write of its backing field (`showErrors = value;`):
//!   the write of the property is the result.
//!
//! # The state from outside (R10)
//!
//! For each state of the type (U11), the reads — by the rule of 2 — and the
//! writes of it by members outside the cluster of the functionality that holds
//! it, each with its place, for the `### Context` of the state.
//!
//! Granularity: a [`FunctionalityResults`] holds every result of the cluster's
//! code with the member it is in, so a flow (U5) takes the ones its trigger
//! reaches; the stopped paths are one per precondition occurrence and per dead
//! path. Everything is ordered by kind, member and place; nothing iterates a
//! hash map (KTD13).

use super::binding::raised_events;
use super::callers::{BodySite, Callers};
use super::cluster::Functionality;
use super::entry::{bodies, Place, Region};
use super::flow::{children, is_bare_return, DeadKind, EffectKind, Source};
use super::link::Links;
use super::resolve::{within, Index};
use super::TypeAnalysis;
use crate::extract::bodies::{
    AccessMode, BodyKind, Constant, MemberBody, NodeKind, PairedBy, PathRoot, Range, ValueRef,
};
use crate::graph::{EdgeKind, FactKind, FactValue, Graph, NodeKey, Span};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// The `ExternalKind` of a database object (`crate::extract::external`).
const DB_OBJECT: &str = "db_object";

/// Whether `member` carries the declaration tag `tag`.
fn tagged(ix: &Index<'_>, member: &NodeKey, tag: &str) -> bool {
    ix.layer
        .metas_of(member)
        .iter()
        .any(|(_, meta)| meta.tags.iter().any(|t| t.name == tag))
}

/// A member that touches a state, and where.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Touch {
    pub member: NodeKey,
    pub place: Place,
}

/// What a result is (see the module docs).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResultKind {
    /// 1. A write of a property tagged `notifies-change`.
    NotifiesChange {
        target: NodeKey,
        /// The constant written, when it is one.
        value: Option<Constant>,
    },
    /// 1. An event raised, with the subscriptions the corpus makes to it.
    RaisesEvent { event: NodeKey, subscribers: usize },
    /// Kind 2: a write — or a change through a call on it (`mutation`) — of
    /// state read outside the cluster.
    ReadElsewhere {
        target: NodeKey,
        value: Option<Constant>,
        mutation: bool,
        /// The reads that count, by member and place.
        readers: Vec<Touch>,
    },
    /// 3. A database object the member uses, where the first layer read it.
    Database {
        object: NodeKey,
        name: String,
        file: String,
        span: Span,
    },
    /// 4. A `throw` that leaves the member.
    Throws,
    /// 4. A value returned to the trigger.
    Returns,
}

impl ResultKind {
    /// The kind's place in the order of how sure it is (R5): 1 to 4; a
    /// stopped path is 5.
    pub fn rank(&self) -> u8 {
        match self {
            ResultKind::NotifiesChange { .. } | ResultKind::RaisesEvent { .. } => 1,
            ResultKind::ReadElsewhere { .. } => 2,
            ResultKind::Database { .. } => 3,
            ResultKind::Throws | ResultKind::Returns => 4,
        }
    }

    fn order(&self) -> u8 {
        match self {
            ResultKind::NotifiesChange { .. } => 0,
            ResultKind::RaisesEvent { .. } => 1,
            ResultKind::ReadElsewhere { .. } => 2,
            ResultKind::Database { .. } => 3,
            ResultKind::Throws => 4,
            ResultKind::Returns => 5,
        }
    }
}

/// One result of a functionality's code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FlowResult {
    /// The member whose code has it.
    pub member: NodeKey,
    /// Where; `None` for a database object, which the first layer anchors
    /// by its lines alone ([`ResultKind::Database`]).
    pub place: Option<Place>,
    pub kind: ResultKind,
}

impl FlowResult {
    fn order(&self) -> (u8, u8, &NodeKey, Option<&Place>) {
        (
            self.kind.rank(),
            self.kind.order(),
            &self.member,
            self.place.as_ref(),
        )
    }
}

/// Why a path stops before the main work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum StopCause {
    /// A precondition of U11: its text, and whether a guard or a CanExecute
    /// found it.
    Precondition { text: String, source: Source },
    /// A dead path of U11.
    Dead(DeadKind),
}

/// 5. A path on which nothing of the main work happens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StoppedPath {
    /// The member the guard or the dead path is in.
    pub member: NodeKey,
    /// The guard's arm, the predicate, the dead path.
    pub place: Place,
    pub cause: StopCause,
    /// What still happens on it, of the functionality's results.
    pub results: Vec<FlowResult>,
}

/// The results of one functionality.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FunctionalityResults {
    /// The functionality's key.
    pub key: String,
    /// Every result of its code, by kind, member and place.
    pub results: Vec<FlowResult>,
    /// By place, then member.
    pub stops: Vec<StoppedPath>,
}

impl FunctionalityResults {
    /// The results of the code of `member`.
    pub fn of_member(&self, member: &NodeKey) -> Vec<&FlowResult> {
        self.results
            .iter()
            .filter(|r| &r.member == member)
            .collect()
    }
}

/// A state of the type touched from outside its functionality (R10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StateContext {
    /// The state's member (U11).
    pub state: NodeKey,
    /// The reads that count, by member and place.
    pub reads: Vec<Touch>,
    /// By member and place.
    pub writes: Vec<Touch>,
}

/// The results and the state context of one type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TypeResults {
    pub key: NodeKey,
    /// One per functionality, by key.
    pub functionalities: Vec<FunctionalityResults>,
    /// One per state of the type, by member.
    pub states: Vec<StateContext>,
}

impl TypeResults {
    /// The results of the functionality `key`.
    pub fn of(&self, key: &str) -> Option<&FunctionalityResults> {
        self.functionalities
            .binary_search_by(|f| f.key.as_str().cmp(key))
            .ok()
            .map(|i| &self.functionalities[i])
    }

    /// The context of the state `member`.
    pub fn state(&self, member: &NodeKey) -> Option<&StateContext> {
        self.states.iter().find(|s| &s.state == member)
    }
}

/// A database object a member uses.
#[derive(Debug, Clone)]
struct DbUse {
    object: NodeKey,
    name: String,
    file: String,
    span: Span,
}

/// Where a functionality's code is: its members whole, and the regions of
/// its entry points written in other members (a key branch, a menu item).
struct Cluster<'f> {
    members: BTreeSet<&'f NodeKey>,
    regions: Vec<(&'f NodeKey, &'f Region)>,
}

impl<'f> Cluster<'f> {
    fn of(f: &'f Functionality) -> Cluster<'f> {
        let members: BTreeSet<&NodeKey> = f.members.iter().collect();
        let mut regions = Vec::new();
        for e in &f.entry_points {
            if members.contains(&e.member) {
                continue;
            }
            for r in &e.regions {
                regions.push((&e.member, r));
            }
        }
        Cluster { members, regions }
    }

    fn none() -> Cluster<'f> {
        Cluster {
            members: BTreeSet::new(),
            regions: Vec::new(),
        }
    }

    /// Whether the code of `member` at `path`, body `body`, node `node` is
    /// the cluster's.
    fn holds(
        &self,
        ix: &Index<'_>,
        member: &NodeKey,
        path: &str,
        body: usize,
        node: Option<u32>,
    ) -> bool {
        self.members.contains(member)
            || self.regions.iter().any(|(m, r)| {
                *m == member
                    && r.path == path
                    && r.body == body
                    && match r.node {
                        None => true,
                        Some(root) => node.is_some_and(|n| {
                            ix.body_at(path, body).is_some_and(|b| within(b, n, root))
                        }),
                    }
            })
    }
}

/// The statement at the top level of `body` that holds `node`; `None` for
/// the root.
fn top_level(body: &MemberBody, node: u32) -> Option<u32> {
    let mut at = node;
    for _ in 0..=body.nodes.len() {
        let parent = body.nodes.get(at as usize)?.parent?;
        if parent == body.root {
            return Some(at);
        }
        at = parent;
    }
    None
}

/// Whether the `throw` at `node` is caught in its own body: inside the
/// block of a `try` that has a `catch`.
fn caught(body: &MemberBody, node: u32) -> bool {
    let mut at = node;
    for _ in 0..=body.nodes.len() {
        let Some(parent) = body.nodes.get(at as usize).and_then(|n| n.parent) else {
            return false;
        };
        let Some(p) = body.nodes.get(parent as usize) else {
            return false;
        };
        let in_block = body
            .nodes
            .get(at as usize)
            .is_some_and(|n| !matches!(n.kind, NodeKind::Catch | NodeKind::Finally));
        if p.kind == NodeKind::Try
            && in_block
            && children(body, parent)
                .iter()
                .any(|c| c.kind == NodeKind::Catch)
        {
            return true;
        }
        at = parent;
    }
    false
}

/// Whether `node` is under a lambda of `body`.
fn in_lambda(body: &MemberBody, node: u32) -> bool {
    let mut at = body.nodes.get(node as usize).and_then(|n| n.parent);
    for _ in 0..=body.nodes.len() {
        let Some(n) = at.and_then(|i| body.nodes.get(i as usize)) else {
            return false;
        };
        if n.kind == NodeKind::Lambda {
            return true;
        }
        at = n.parent;
    }
    false
}

fn contains(outer: &Range, inner: &Range) -> bool {
    outer.start <= inner.start && inner.end <= outer.end
}

/// The results of the functionalities of a run's types (see the module
/// docs): built once over the whole corpus, asked type by type.
pub struct FlowResults<'a, 'i, 'l> {
    links: &'a Links<'i, 'l>,
    callers: &'a Callers,
    /// The database objects of each member, by member.
    db: BTreeMap<NodeKey, Vec<DbUse>>,
}

impl<'a, 'i, 'l> FlowResults<'a, 'i, 'l> {
    pub fn new(graph: &Graph, links: &'a Links<'i, 'l>, callers: &'a Callers) -> Self {
        let text = |key: &NodeKey, kind: FactKind| -> Option<String> {
            graph.node(key)?.facts.iter().find_map(|f| match &f.value {
                FactValue::Text(t) if f.kind == kind => Some(t.clone()),
                _ => None,
            })
        };
        let mut db: BTreeMap<NodeKey, Vec<DbUse>> = BTreeMap::new();
        for e in graph.edges_sorted() {
            if e.edge.kind != EdgeKind::Uses
                || text(e.to, FactKind::ExternalKind).as_deref() != Some(DB_OBJECT)
            {
                continue;
            }
            db.entry(e.from.clone()).or_default().push(DbUse {
                object: e.to.clone(),
                name: text(e.to, FactKind::ExternalName).unwrap_or_default(),
                file: e.edge.provenance.file.clone(),
                span: e.edge.provenance.span,
            });
        }
        FlowResults { links, callers, db }
    }

    fn ix(&self) -> &'i Index<'l> {
        self.links.index()
    }

    /// The results of `analysis`'s functionalities and the context of its
    /// states.
    pub fn of_type(&self, analysis: &TypeAnalysis) -> TypeResults {
        let ix = self.ix();
        let raised = raised_events(ix, &analysis.key);
        let mut functionalities: Vec<FunctionalityResults> = analysis
            .functionalities
            .iter()
            .map(|f| self.functionality(analysis, f, &raised))
            .collect();
        functionalities.sort_by(|a, b| a.key.cmp(&b.key));
        let states = analysis
            .flow
            .states
            .iter()
            .map(|s| {
                let holder = analysis.functionalities.iter().find(|f| {
                    f.members
                        .iter()
                        .any(|m| m == &s.member || Some(m) == s.field.as_ref())
                });
                let cluster = holder.map_or_else(Cluster::none, Cluster::of);
                let state: Vec<NodeKey> = std::iter::once(s.member.clone())
                    .chain(s.field.clone())
                    .collect();
                StateContext {
                    state: s.member.clone(),
                    reads: self.readers(&state, &cluster),
                    writes: self.writers(&state, &cluster),
                }
            })
            .collect();
        TypeResults {
            key: analysis.key.clone(),
            functionalities,
            states,
        }
    }

    fn functionality(
        &self,
        analysis: &TypeAnalysis,
        f: &Functionality,
        raised: &[super::RaisedEvent],
    ) -> FunctionalityResults {
        let ix = self.ix();
        let cluster = Cluster::of(f);
        let mut results: Vec<FlowResult> = Vec::new();

        // Writes and changes (kinds 1, 2), throws (4).
        let stretches: Vec<(&NodeKey, Option<&Region>)> = f
            .members
            .iter()
            .map(|m| (m, None))
            .chain(cluster.regions.iter().map(|(m, r)| (*m, Some(*r))))
            .collect();
        for (member, region) in &stretches {
            let in_stretch = |path: &str, body: usize, node: Option<u32>| match region {
                None => true,
                Some(r) => {
                    r.path == path
                        && r.body == body
                        && match r.node {
                            None => true,
                            Some(root) => node.is_some_and(|n| {
                                ix.body_at(path, body).is_some_and(|b| within(b, n, root))
                            }),
                        }
                }
            };
            for s in self.links.sites_of(member) {
                if !in_stretch(&s.place.path, s.body, s.node) {
                    continue;
                }
                let mutation = match s.mode {
                    Some(AccessMode::Write) => false,
                    Some(AccessMode::Mutate) => true,
                    _ => continue,
                };
                let Some(target) = s.resolution.target() else {
                    continue;
                };
                let Some(body) = ix.body_at(&s.place.path, s.body) else {
                    continue;
                };
                let rhs = (!mutation)
                    .then(|| {
                        body.accesses.iter().find(|a| {
                            a.mode == AccessMode::Write
                                && a.place == s.place.range
                                && a.target == s.path
                        })
                    })
                    .flatten()
                    .and_then(|a| a.rhs.as_ref());
                if self.not_a_result(member, body, &s.path, rhs, target) {
                    continue;
                }
                let value = match rhs {
                    Some(ValueRef::Constant(c)) => Some(c.clone()),
                    _ => None,
                };
                let kind = if !mutation && tagged(ix, target, "notifies-change") {
                    ResultKind::NotifiesChange {
                        target: target.clone(),
                        value,
                    }
                } else {
                    let readers = self.readers(&self.aliases(target), &cluster);
                    if readers.is_empty() {
                        continue;
                    }
                    ResultKind::ReadElsewhere {
                        target: target.clone(),
                        value,
                        mutation,
                        readers,
                    }
                };
                results.push(FlowResult {
                    member: (*member).clone(),
                    place: Some(s.place.clone()),
                    kind,
                });
            }
            for at in bodies(ix, member) {
                for n in at.body.nodes.iter().filter(|n| n.kind == NodeKind::Throw) {
                    if !in_stretch(at.path, at.index, Some(n.id)) || caught(at.body, n.id) {
                        continue;
                    }
                    results.push(FlowResult {
                        member: (*member).clone(),
                        place: Some(Place {
                            path: at.path.to_string(),
                            range: n.place,
                        }),
                        kind: ResultKind::Throws,
                    });
                }
            }
        }

        // Events raised (1).
        for r in raised
            .iter()
            .filter(|r| cluster.members.contains(&r.method))
        {
            results.push(FlowResult {
                member: r.method.clone(),
                place: Some(r.place.clone()),
                kind: ResultKind::RaisesEvent {
                    event: r.field.clone(),
                    subscribers: self.callers.subscribers_of(&r.field).len(),
                },
            });
        }

        // The database (3).
        for m in &f.members {
            for d in self.db.get(m).into_iter().flatten() {
                results.push(FlowResult {
                    member: m.clone(),
                    place: None,
                    kind: ResultKind::Database {
                        object: d.object.clone(),
                        name: d.name.clone(),
                        file: d.file.clone(),
                        span: d.span,
                    },
                });
            }
        }

        // The value returned to the trigger (4).
        let entries: BTreeSet<&NodeKey> = f
            .entry_points
            .iter()
            .filter(|e| e.discriminator.is_none())
            .map(|e| &e.member)
            .filter(|m| ix.symbol(m).is_some_and(|s| s.is_method()))
            .collect();
        for m in entries {
            for pb in ix.layer.bodies_of(m) {
                if pb.by != PairedBy::Name || pb.body.kind != BodyKind::Method {
                    continue;
                }
                for n in &pb.body.nodes {
                    if n.kind == NodeKind::Return
                        && !is_bare_return(pb.body, n)
                        && !in_lambda(pb.body, n.id)
                    {
                        results.push(FlowResult {
                            member: m.clone(),
                            place: Some(Place {
                                path: pb.path.to_string(),
                                range: n.place,
                            }),
                            kind: ResultKind::Returns,
                        });
                    }
                }
            }
        }

        results.sort_by(|a, b| a.order().cmp(&b.order()));
        results.dedup();
        let stops = analysis
            .flow_of(&f.key)
            .map(|flow| stops(flow, &results))
            .unwrap_or_default();
        FunctionalityResults {
            key: f.key.clone(),
            results,
            stops,
        }
    }

    /// Whether a write of `target` by `path` in `body`, a body of `member`,
    /// is no result (R6): into an object the body constructed, of a
    /// `lazy-init` field, the setter's own write of its field.
    fn not_a_result(
        &self,
        member: &NodeKey,
        body: &MemberBody,
        path: &crate::extract::bodies::AccessPath,
        rhs: Option<&ValueRef>,
        target: &NodeKey,
    ) -> bool {
        let ix = self.ix();
        let constructed = path.root == PathRoot::Local
            && path.segments.first().is_some_and(|local| {
                body.accesses.iter().any(|a| {
                    a.mode == AccessMode::Write
                        && a.target.root == PathRoot::Local
                        && a.target.segments.as_slice() == std::slice::from_ref(local)
                        && matches!(a.rhs, Some(ValueRef::Construction(_)))
                })
            });
        let own_field = body.kind == BodyKind::Setter
            && matches!(rhs, Some(ValueRef::Access(p))
                if p.root == PathRoot::Local && p.segments == ["value"])
            && ix.symbol(member).is_some_and(|s| s.kind == "property");
        constructed || own_field || tagged(ix, target, "lazy-init")
    }

    /// `target` and what stands for it: the field a property's setter writes
    /// `value` to, the property whose setter writes this field.
    fn aliases(&self, target: &NodeKey) -> Vec<NodeKey> {
        let ix = self.ix();
        let mut out = vec![target.clone()];
        let Some(owner) = ix.symbol(target).and_then(|s| s.owner.clone()) else {
            return out;
        };
        let setter_field = |p: &NodeKey| -> Option<NodeKey> {
            ix.layer.bodies_of(p).into_iter().find_map(|pb| {
                if pb.body.kind != BodyKind::Setter {
                    return None;
                }
                pb.body.accesses.iter().find_map(|a| {
                    let from_value = matches!(&a.rhs, Some(ValueRef::Access(v))
                        if v.root == PathRoot::Local && v.segments == ["value"]);
                    let name = super::entry::is_field_path(&a.target)?;
                    (a.mode == AccessMode::Write && from_value)
                        .then(|| super::entry::field_named(ix, &owner, name))
                        .flatten()
                })
            })
        };
        if let Some(f) = setter_field(target).filter(|f| f != target) {
            out.push(f);
        }
        for p in ix.members_of(&owner) {
            if p != target
                && ix.symbol(p).is_some_and(|s| s.kind == "property")
                && setter_field(p).as_ref() == Some(target)
            {
                out.push(p.clone());
            }
        }
        out.sort();
        out.dedup();
        out
    }

    /// Whether `member` writes one of `targets` unconditionally at the top
    /// level of the body `site` is in, before the statement `site` is in.
    fn written_before(&self, member: &NodeKey, site: &BodySite, targets: &[NodeKey]) -> bool {
        let (Some(node), Some(body)) = (site.node, self.ix().body_at(&site.place.path, site.body))
        else {
            return false;
        };
        let Some(read_at) = top_level(body, node) else {
            return false;
        };
        targets.iter().any(|t| {
            self.callers
                .of(t)
                .iter()
                .filter(|c| &c.member == member)
                .flat_map(|c| c.links())
                .any(|w| {
                    w.last
                        && w.mode == Some(AccessMode::Write)
                        && w.place.path == site.place.path
                        && w.body == site.body
                        && w.node.is_some_and(|n| {
                            top_level(body, n) == Some(n)
                                && n < read_at
                                && body
                                    .nodes
                                    .get(n as usize)
                                    .is_some_and(|x| x.kind == NodeKind::Statement)
                        })
                })
        })
    }

    /// The reads of `targets` that count (see the module docs), by members
    /// outside `cluster`; a getter of the same type returning one of them
    /// stands for it.
    fn readers(&self, targets: &[NodeKey], cluster: &Cluster<'_>) -> Vec<Touch> {
        let ix = self.ix();
        let mut all: Vec<NodeKey> = targets.to_vec();
        let mut out: BTreeSet<Touch> = BTreeSet::new();
        let mut i = 0;
        while i < all.len() {
            let target = all[i].clone();
            i += 1;
            let owner = ix.symbol(&target).and_then(|s| s.owner.clone());
            for c in self.callers.of(&target) {
                for s in c.links() {
                    let read = !s.last
                        || matches!(
                            s.mode,
                            Some(AccessMode::Read | AccessMode::Return | AccessMode::Mutate)
                        );
                    if !read {
                        continue;
                    }
                    let getter = s.last
                        && s.mode == Some(AccessMode::Return)
                        && ix.symbol(&c.member).is_some_and(|x| {
                            x.kind == "property" && x.owner.is_some() && x.owner == owner
                        });
                    if getter {
                        if !all.contains(&c.member) {
                            all.push(c.member.clone());
                        }
                        continue;
                    }
                    if cluster.holds(ix, &c.member, &s.place.path, s.body, s.node)
                        || self.written_before(&c.member, s, &all)
                    {
                        continue;
                    }
                    out.insert(Touch {
                        member: c.member.clone(),
                        place: s.place.clone(),
                    });
                }
            }
        }
        out.into_iter().collect()
    }

    /// The writes and changes of `targets` by members outside `cluster`.
    fn writers(&self, targets: &[NodeKey], cluster: &Cluster<'_>) -> Vec<Touch> {
        let ix = self.ix();
        let mut out: BTreeSet<Touch> = BTreeSet::new();
        for t in targets {
            for c in self.callers.of(t) {
                for s in c.links() {
                    if s.last
                        && matches!(s.mode, Some(AccessMode::Write | AccessMode::Mutate))
                        && !cluster.holds(ix, &c.member, &s.place.path, s.body, s.node)
                    {
                        out.insert(Touch {
                            member: c.member.clone(),
                            place: s.place.clone(),
                        });
                    }
                }
            }
        }
        out.into_iter().collect()
    }
}

/// The stopped paths of a functionality (5): one per occurrence of each
/// precondition, with the results before its guard and the guard's own
/// `throw`, and one per dead path.
fn stops(flow: &super::FunctionalityFlow, results: &[FlowResult]) -> Vec<StoppedPath> {
    let mut out = Vec::new();
    for p in &flow.preconditions {
        for o in &p.occurrences {
            let before = |r: &&FlowResult| {
                let Some(place) = &r.place else {
                    return false;
                };
                let effect = o.effects.iter().any(|e| {
                    e.place.path == place.path
                        && match (e.kind, &r.kind) {
                            (
                                EffectKind::Write,
                                ResultKind::NotifiesChange { .. }
                                | ResultKind::ReadElsewhere { .. },
                            ) => e.place.range == place.range,
                            (EffectKind::Raise, ResultKind::RaisesEvent { .. }) => {
                                contains(&e.place.range, &place.range)
                            }
                            _ => false,
                        }
                });
                let thrown = o.source == Source::Guard
                    && matches!(r.kind, ResultKind::Throws)
                    && o.place.path == place.path
                    && contains(&o.place.range, &place.range);
                effect || thrown
            };
            out.push(StoppedPath {
                member: o.member.clone(),
                place: o.place.clone(),
                cause: StopCause::Precondition {
                    text: p.text.clone(),
                    source: o.source,
                },
                results: results.iter().filter(before).cloned().collect(),
            });
        }
    }
    for d in &flow.dead_paths {
        out.push(StoppedPath {
            member: d.member.clone(),
            place: d.place.clone(),
            cause: StopCause::Dead(d.kind.clone()),
            results: Vec::new(),
        });
    }
    out.sort_by(|a, b| (&a.place, &a.member).cmp(&(&b.place, &b.member)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::bodies::{BodyNode, Position};

    fn node(id: u32, kind: NodeKind, parent: Option<u32>) -> BodyNode {
        BodyNode {
            id,
            kind,
            place: Range::from([1, 1, 1, 30]),
            parent,
            condition: None,
            returns: None,
            tags: Vec::new(),
        }
    }

    fn body(nodes: Vec<BodyNode>) -> MemberBody {
        MemberBody {
            name_position: Position { line: 1, column: 1 },
            kind: BodyKind::Method,
            root: 0,
            nodes,
            accesses: Vec::new(),
            invocations: Vec::new(),
            lambdas: Vec::new(),
            subscriptions: Vec::new(),
            literals: Vec::new(),
            name_ofs: Vec::new(),
            typed_names: Vec::new(),
            tags: Vec::new(),
        }
    }

    /// `try { throw; } catch { }` catches; `try { } catch { throw; }` and
    /// `try { throw; } finally { }` do not.
    #[test]
    fn a_throw_is_caught_only_in_the_block_of_a_try_with_a_catch() {
        let b = body(vec![
            node(0, NodeKind::Body, None),
            node(1, NodeKind::Try, Some(0)),
            node(2, NodeKind::Throw, Some(1)),
            node(3, NodeKind::Catch, Some(1)),
            node(4, NodeKind::Throw, Some(3)),
            node(5, NodeKind::Try, Some(0)),
            node(6, NodeKind::Throw, Some(5)),
            node(7, NodeKind::Finally, Some(5)),
            node(8, NodeKind::Throw, Some(0)),
        ]);
        assert!(caught(&b, 2));
        assert!(!caught(&b, 4));
        assert!(!caught(&b, 6));
        assert!(!caught(&b, 8));
    }

    #[test]
    fn the_top_level_statement_holds_its_descendants() {
        let b = body(vec![
            node(0, NodeKind::Body, None),
            node(1, NodeKind::Statement, Some(0)),
            node(2, NodeKind::Choice, Some(0)),
            node(3, NodeKind::Arm, Some(2)),
            node(4, NodeKind::Statement, Some(3)),
        ]);
        assert_eq!(top_level(&b, 0), None);
        assert_eq!(top_level(&b, 1), Some(1));
        assert_eq!(top_level(&b, 4), Some(2));
        assert!(!in_lambda(&b, 4));
    }
}
