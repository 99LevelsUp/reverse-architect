//! The reverse index of callers (plan 2b, U1: R4, R13; KTD2).
//!
//! Who calls, reads, writes or otherwise names a member, and how far that can
//! be trusted — asked of every member by one deterministic query. Built once a
//! run, beside the model of bodies, never in the graph: the second layer lives
//! beside the graph (plan 2a, KTD6) and `crate::graph` knows nothing of
//! `card`.
//!
//! # What makes a caller
//!
//! - **a relation of the first layer** — `Calls`, `References`,
//!   `Instantiates` — at the rung it was read at
//!   ([`crate::graph::Provenance::rung`]); a heuristic relation that carries no
//!   rung is [`Confidence::Ambiguous`] when it was one of several candidates,
//!   else [`Confidence::NameOnly`];
//! - **a name over the ceiling**: an
//!   [`FactKind::AmbiguousReference`] says the first layer matched more
//!   candidates than the ambiguity ceiling allowed and named none. Every member
//!   (and every type) of the repository's C# types of that name gets the node
//!   the fact is on as a weak caller ([`Confidence::Ambiguous`]) — for its class
//!   alone: a candidate is never followed (R4), but nor is it taken for a member
//!   nothing calls;
//! - **every link of every access and call of a body**, resolved by U2
//!   ([`Links::sites_of`]): each member an access names — the ones it goes on
//!   through as much as the one it ends in (`this.viewModel.CheckCommand`
//!   reaches `CheckCommand`) — at the weakest grade of the links up to it. An
//!   access that stopped at several candidates ([`Outcome::Ambiguous`]) makes
//!   each of them, past the ceiling too, a weak caller's; an access that could
//!   not be told at all ([`Outcome::Unresolved`]) makes none — a name alone
//!   makes no edge (R4), and the first layer's own name match is already here
//!   as a relation;
//! - **the delegate binding** of U3 ([`bindings_of`]): the method of the class
//!   that calls the field a construction's argument is written to calls the
//!   argument's target (`RelayCommand.Execute` → `Check`), at the pair's grade;
//! - **a subscription** `source.Event += Handler` (a `-=` is the same
//!   subscription): the member that writes it calls the method group, resolved
//!   by U2 in the body it is written in. A lambda subscribed so is the
//!   subscribing member's own code, already among its sites.
//!
//! A member is never its own caller (recursion calls nothing new).
//!
//! # The class of a member
//!
//! From the strongest grade among its callers ([`CallerClass`]): a caller at
//! [`Confidence::ResolvedByIndex`] or [`Confidence::RulesBacked`] is strong
//! (KTD2), one at [`Confidence::NameOnly`] or [`Confidence::Ambiguous`] weak.
//!
//! # The subscribers of an event
//!
//! The same pass resolves the source of every `+=` with the event's name
//! (`viewModel.CheckCommand.Executed`) and keeps the subscriptions whose event
//! it told, by the event member: the count of subscribers U4 gives a raised
//! event (KTD6) is taken from the whole corpus, on the resolved type of the
//! source.
//!
//! Every list is ordered — the callees and each callee's callers by key in
//! code points, what makes a caller by kind, then place — and nothing iterates
//! a hash map (KTD13).

use super::binding::bindings_of;
use super::entry::{bodies, Place};
use super::link::{weakest, Links, Outcome, Resolution, Site};
use super::resolve::Scope;
use crate::extract::bodies::{AccessMode, AccessPath, Handler, PathRoot, SubscriptionMode};
use crate::graph::{Confidence, EdgeKind, FactKind, FactValue, Graph, NodeKey, SiteId, Span};
use serde::Serialize;
use std::collections::BTreeMap;

/// The relations of the first layer a caller is read from.
const RELATION_KINDS: &[EdgeKind] = &[
    EdgeKind::Calls,
    EdgeKind::References,
    EdgeKind::Instantiates,
];

/// Whether a grade is strong (KTD2): told by an external index or by rules.
pub fn is_strong(grade: Confidence) -> bool {
    matches!(grade, Confidence::ResolvedByIndex | Confidence::RulesBacked)
}

/// The class of a member by its callers (KTD2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CallerClass {
    /// Nothing calls it.
    Uncalled,
    /// Only weak callers: [`Confidence::NameOnly`] or
    /// [`Confidence::Ambiguous`].
    WeakOnly,
    /// At least one caller at [`Confidence::ResolvedByIndex`] or
    /// [`Confidence::RulesBacked`].
    Strong,
}

/// A place of a body that names the member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BodySite {
    pub place: Place,
    /// The index of the body in its file's `members`.
    pub body: usize,
    /// The node of the body it is under.
    pub node: Option<u32>,
    /// The access's mode; `None` for a call.
    pub mode: Option<AccessMode>,
    /// The argument count, for a call.
    pub call: Option<u32>,
    /// The member is the access's last name — what is read, written or
    /// called — not one it goes on through.
    pub last: bool,
}

/// What makes a caller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Via {
    /// A relation of the first layer, with the sites it was read at.
    Relation { kind: EdgeKind, sites: Vec<SiteId> },
    /// A name the first layer matched more candidates of than the ceiling
    /// allowed (`Execute: 412 candidates`), and where it said so.
    OverCeiling {
        name: String,
        file: String,
        span: Span,
    },
    /// A link of an access or a call of a body.
    Link(BodySite),
    /// A candidate of an access that stopped at several.
    Candidate(BodySite),
    /// The delegate binding: the caller is the method of the class that calls
    /// the field; `construction` is the `new C(…)` written in `built_in`.
    Delegate {
        construction: Place,
        built_in: NodeKey,
        field: NodeKey,
    },
    /// A `+=` of the member as the handler of `event`.
    Subscription {
        event: String,
        place: Place,
        body: usize,
        node: Option<u32>,
    },
}

impl Via {
    fn rank(&self) -> u8 {
        match self {
            Via::Relation { .. } => 0,
            Via::OverCeiling { .. } => 1,
            Via::Link(_) => 2,
            Via::Candidate(_) => 3,
            Via::Delegate { .. } => 4,
            Via::Subscription { .. } => 5,
        }
    }

    /// The place of a body it stands at, when it has one.
    pub fn place(&self) -> Option<&Place> {
        match self {
            Via::Link(s) | Via::Candidate(s) => Some(&s.place),
            Via::Delegate { construction, .. } => Some(construction),
            Via::Subscription { place, .. } => Some(place),
            Via::Relation { .. } | Via::OverCeiling { .. } => None,
        }
    }
}

/// One thing that makes a caller, and its grade.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Evidence {
    pub via: Via,
    pub grade: Confidence,
}

/// One caller of a member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Caller {
    /// The member — or the file, for code no declaration encloses.
    pub member: NodeKey,
    /// The strongest grade of its evidence.
    pub grade: Confidence,
    /// By kind, then place; one kind and place in the order it was found.
    pub evidence: Vec<Evidence>,
}

impl Caller {
    pub fn is_strong(&self) -> bool {
        is_strong(self.grade)
    }

    /// The sites of bodies where the caller names the member as one of an
    /// access's links.
    pub fn links(&self) -> impl Iterator<Item = &BodySite> {
        self.evidence.iter().filter_map(|e| match &e.via {
            Via::Link(s) => Some(s),
            _ => None,
        })
    }
}

/// One `+=` of an event whose source was told.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Subscriber {
    /// The member the subscription is written in.
    pub member: NodeKey,
    pub place: Place,
    /// The weakest grade of the links to the event.
    pub grade: Confidence,
    /// The handler, when it is a method group that was told.
    pub handler: Option<NodeKey>,
}

/// The reverse index of callers of a run (see the module docs).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Callers {
    /// By callee, then caller.
    of: BTreeMap<NodeKey, Vec<Caller>>,
    /// By event, then place and member.
    subscribers: BTreeMap<NodeKey, Vec<Subscriber>>,
}

/// The callers gathered so far: by callee, by caller, in the order found.
#[derive(Default)]
struct Gather {
    of: BTreeMap<NodeKey, BTreeMap<NodeKey, Vec<Evidence>>>,
    subscribers: BTreeMap<NodeKey, Vec<Subscriber>>,
}

impl Gather {
    fn add(&mut self, callee: &NodeKey, caller: &NodeKey, via: Via, grade: Confidence) {
        if callee == caller {
            return;
        }
        self.of
            .entry(callee.clone())
            .or_default()
            .entry(caller.clone())
            .or_default()
            .push(Evidence { via, grade });
    }

    /// Every link of `resolution`, an access of `caller` at `site`, and the
    /// candidates it stopped at when they were several.
    fn resolution(
        &mut self,
        caller: &NodeKey,
        path: &AccessPath,
        resolution: &Resolution,
        site: impl Fn(bool) -> BodySite,
        wrap: impl Fn(BodySite) -> Via,
    ) {
        let resolved = resolution.target().is_some();
        let count = resolution.links.len();
        let mut grade: Option<Confidence> = None;
        for (i, l) in resolution.links.iter().enumerate() {
            grade = weakest(grade.into_iter().chain([l.grade]));
            let Some(g) = grade else { continue };
            let last = resolved && i + 1 == count;
            self.add(&l.link.member, caller, wrap(site(last)), g);
        }
        if let Outcome::Ambiguous { .. } = resolution.outcome {
            // The name it stopped at: the one after its told links.
            let start = usize::from(path.root == PathRoot::Local);
            let last = start + count + 1 == path.segments.len();
            for c in resolution.candidates.all() {
                self.add(c, caller, Via::Candidate(site(last)), Confidence::Ambiguous);
            }
        }
    }

    fn site(&mut self, caller: &NodeKey, s: &Site) {
        let body_site = |last: bool| BodySite {
            place: s.place.clone(),
            body: s.body,
            node: s.node,
            mode: s.mode,
            call: s.call,
            last,
        };
        self.resolution(caller, &s.path, &s.resolution, body_site, Via::Link);
    }
}

/// The name of an [`FactKind::AmbiguousReference`] text (`Execute: 412
/// candidates`).
fn over_ceiling_name(text: &str) -> Option<&str> {
    let (name, rest) = text.rsplit_once(": ")?;
    rest.ends_with("candidates").then_some(name)
}

impl Callers {
    /// The index of every caller in `graph` and in the bodies `links`
    /// resolves (see the module docs).
    pub fn new(graph: &Graph, links: &Links<'_, '_>) -> Callers {
        let ix = links.index();
        let mut g = Gather::default();

        for e in graph.edges_sorted() {
            if !RELATION_KINDS.contains(&e.edge.kind) {
                continue;
            }
            let p = &e.edge.provenance;
            let grade = p.rung().unwrap_or(if p.ambiguous_candidates.is_some() {
                Confidence::Ambiguous
            } else {
                Confidence::NameOnly
            });
            let via = Via::Relation {
                kind: e.edge.kind,
                sites: e.edge.sites.clone(),
            };
            g.add(e.to, e.from, via, grade);
        }

        for node in graph.nodes_sorted() {
            for f in node
                .facts
                .iter()
                .filter(|f| f.kind == FactKind::AmbiguousReference)
            {
                let FactValue::Text(text) = &f.value else {
                    continue;
                };
                let Some(name) = over_ceiling_name(text) else {
                    continue;
                };
                for c in ix.members_named(name).iter().chain(ix.types_named(name)) {
                    let via = Via::OverCeiling {
                        name: name.to_string(),
                        file: f.provenance.file.clone(),
                        span: f.provenance.span,
                    };
                    g.add(c, &node.key, via, Confidence::Ambiguous);
                }
            }
        }

        for ty in ix.csharp_types() {
            for m in ix.members_of(ty) {
                if ix.symbol(m).is_none_or(|s| s.is_type()) {
                    continue;
                }
                for s in links.sites_of(m) {
                    g.site(m, &s);
                }
                for at in bodies(ix, m) {
                    let place = |range| Place {
                        path: at.path.to_string(),
                        range,
                    };
                    for s in at
                        .body
                        .subscriptions
                        .iter()
                        .filter(|s| s.mode == SubscriptionMode::Add)
                    {
                        let scope = Scope {
                            ty,
                            body: Some(at.body),
                            node: s.node,
                        };
                        let place = place(s.place);
                        let handler = match &s.handler {
                            Handler::MethodGroup(path) => {
                                let r = links.resolve(m, &scope, path, None, &place);
                                let site = |last| BodySite {
                                    place: place.clone(),
                                    body: at.index,
                                    node: s.node,
                                    mode: None,
                                    call: None,
                                    last,
                                };
                                // The handler itself is what the event
                                // calls; the names the path goes on through
                                // are read by the subscribing member.
                                let wrap = |bs: BodySite| {
                                    if bs.last {
                                        Via::Subscription {
                                            event: s.event.clone(),
                                            place: bs.place,
                                            body: bs.body,
                                            node: bs.node,
                                        }
                                    } else {
                                        Via::Link(bs)
                                    }
                                };
                                g.resolution(m, path, &r, site, wrap);
                                r.target().cloned()
                            }
                            Handler::Lambda(_) | Handler::Other => None,
                        };
                        let event = match &s.source {
                            Some(source) => AccessPath {
                                root: source.root,
                                segments: source
                                    .segments
                                    .iter()
                                    .cloned()
                                    .chain([s.event.clone()])
                                    .collect(),
                            },
                            None => AccessPath {
                                root: PathRoot::This,
                                segments: vec![s.event.clone()],
                            },
                        };
                        let r = links.resolve(m, &scope, &event, None, &place);
                        if let (Some(ev), Some(grade)) = (r.target(), r.grade()) {
                            g.subscribers
                                .entry(ev.clone())
                                .or_default()
                                .push(Subscriber {
                                    member: m.clone(),
                                    place,
                                    grade,
                                    handler,
                                });
                        }
                    }
                }
            }
            for sb in bindings_of(links, ty) {
                let Ok(binding) = &sb.binding else {
                    continue;
                };
                for p in binding.pairs() {
                    let (Some(target), Some(grade)) = (p.target.target(), p.grade) else {
                        continue;
                    };
                    let via = Via::Delegate {
                        construction: binding.place.clone(),
                        built_in: sb.member.clone(),
                        field: p.field.clone(),
                    };
                    g.add(target, &p.method, via, grade);
                }
            }
        }

        let of =
            g.of.into_iter()
                .map(|(callee, callers)| {
                    let callers = callers
                        .into_iter()
                        .filter_map(|(member, mut evidence)| {
                            evidence.sort_by(|a, b| {
                                (a.via.rank(), a.via.place()).cmp(&(b.via.rank(), b.via.place()))
                            });
                            let grade = evidence.iter().map(|e| e.grade).min()?;
                            Some(Caller {
                                member,
                                grade,
                                evidence,
                            })
                        })
                        .collect();
                    (callee, callers)
                })
                .collect();
        let mut subscribers = g.subscribers;
        for list in subscribers.values_mut() {
            list.sort_by(|a, b| (&a.place, &a.member).cmp(&(&b.place, &b.member)));
        }
        Callers { of, subscribers }
    }

    /// The callers of `member`, by key; empty when nothing calls it.
    pub fn of(&self, member: &NodeKey) -> &[Caller] {
        self.of.get(member).map_or(&[], Vec::as_slice)
    }

    /// The class of `member` by its strongest caller (KTD2).
    pub fn class_of(&self, member: &NodeKey) -> CallerClass {
        let callers = self.of(member);
        if callers.is_empty() {
            CallerClass::Uncalled
        } else if callers.iter().any(Caller::is_strong) {
            CallerClass::Strong
        } else {
            CallerClass::WeakOnly
        }
    }

    /// Every member something calls, by key.
    pub fn members(&self) -> impl Iterator<Item = &NodeKey> {
        self.of.keys()
    }

    /// The subscriptions to `event` whose source was told, by place.
    pub fn subscribers_of(&self, event: &NodeKey) -> &[Subscriber] {
        self.subscribers.get(event).map_or(&[], Vec::as_slice)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_name_over_the_ceiling_is_read_from_the_facts_text() {
        assert_eq!(
            over_ceiling_name("Execute: 412 candidates"),
            Some("Execute")
        );
        assert_eq!(over_ceiling_name("a: b: 9 candidates"), Some("a: b"));
        assert_eq!(over_ceiling_name("Execute"), None);
        assert_eq!(over_ceiling_name("Execute: soon"), None);
    }

    #[test]
    fn only_a_grade_an_index_or_a_rule_told_is_strong() {
        assert!(is_strong(Confidence::ResolvedByIndex));
        assert!(is_strong(Confidence::RulesBacked));
        assert!(!is_strong(Confidence::NameOnly));
        assert!(!is_strong(Confidence::Ambiguous));
    }
}
