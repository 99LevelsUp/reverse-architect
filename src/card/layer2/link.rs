//! The links of an access, graded (plan 2b, U2: R4, R8; KTD3, KTD9).
//!
//! Step 2b follows a functionality across types: every member access and
//! every call of a body is resolved name by name by the one walk 2a resolves
//! the targets of its entry points with ([`Index::walk`], behind
//! [`Index::resolve`]) — the declared type of a local, a parameter, a field or
//! a property, a type parameter substituted from the deriving type's base
//! list, the type of a declaration pattern, the one overload of the call's
//! arity. What this module adds is what 2a does not need:
//!
//! - **the grade of each link** (KTD3): a link a rule told is
//!   [`Confidence::RulesBacked`], one the external index pinned is
//!   [`Confidence::ResolvedByIndex`], and the access's grade is the weakest
//!   of its links. Several candidates where the rules expected one — two
//!   overloads of one arity, two members of one name — are
//!   [`Confidence::Ambiguous`]. No new grade exists;
//! - **the index first**: a relation of the first layer an external index
//!   pinned from the member the access is written in to a member of the name,
//!   read at a line the access spans, is taken before the rules, and also
//!   where they could not tell the type in hand (`Lookup().CheckCommand`) —
//!   the one such target, never one of several (KTD3);
//! - **the candidates** of an access that stopped (KTD9): the members of the
//!   name in the type in hand when it was known, else every member of the
//!   repository's C# types of that name — by key, cut at the ambiguity
//!   ceiling, with the number left out. Candidates are shown, never followed:
//!   no edge is made from a name alone (R4).
//!
//! The limits of 2a hold unchanged, because the walk is 2a's: one inherited
//! generic level only; a nested generic argument, a base list no part of the
//! type read, an ancestor outside the repository stay unresolved — and the
//! entry points of the same card admit them for the same reason.

use super::entry::{bodies, Place};
use super::resolve::{ByArity, Chain, Index, Link, Scope, Stop, Unresolved};
use crate::extract::bodies::{AccessMode, AccessPath, CallKind, PathRoot};
use crate::graph::{Confidence, NodeKey};
use serde::Serialize;

/// The rule a link was told by (KTD3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Basis {
    /// A member of the enclosing type or its base: `this.x`, `base.x`, a
    /// bare `x`.
    Enclosing,
    /// A member of the type a local, a parameter, a field or a property is
    /// declared with.
    DeclaredType,
    /// A member of the type a base list gives a type parameter
    /// (`ViewBase<EditorViewModel>.viewModel` → `EditorViewModel`).
    BaseTypeArgument,
    /// A member of the type a declaration pattern names (`is not X x`).
    Pattern,
    /// The one overload of the call's argument count among several.
    Overload,
    /// The member an external index pinned.
    Index,
}

impl Basis {
    /// The grade a link told by this rule stands on.
    pub fn grade(self) -> Confidence {
        match self {
            Basis::Index => Confidence::ResolvedByIndex,
            _ => Confidence::RulesBacked,
        }
    }
}

/// One resolved name of an access, with its rule and grade.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GradedLink {
    pub link: Link,
    pub basis: Basis,
    pub grade: Confidence,
}

/// The members an access that stopped could have meant (KTD9): by key, at
/// most the ceiling of them, and how many more there are.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Candidates {
    pub shown: Vec<NodeKey>,
    /// The candidates past the ceiling, not named.
    pub more: usize,
}

impl Candidates {
    /// `all`, sorted by key, cut at `ceiling`.
    pub fn capped(mut all: Vec<NodeKey>, ceiling: usize) -> Candidates {
        all.sort();
        all.dedup();
        let more = all.len().saturating_sub(ceiling);
        all.truncate(ceiling);
        Candidates { shown: all, more }
    }

    /// How many candidates there are, shown or not.
    pub fn total(&self) -> usize {
        self.shown.len() + self.more
    }

    pub fn is_empty(&self) -> bool {
        self.total() == 0
    }
}

/// How an access ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Outcome {
    /// Every name was told: the member the access ends in, and the weakest
    /// grade of its links.
    Resolved { target: NodeKey, grade: Confidence },
    /// Several candidates where the rules wanted one: the access stops there,
    /// [`Confidence::Ambiguous`].
    Ambiguous { reason: Unresolved },
    /// It could not be told: no edge, the reason admitted.
    Unresolved { reason: Unresolved },
}

/// An access resolved and graded (see the module docs).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Resolution {
    /// Every name told, in order.
    pub links: Vec<GradedLink>,
    pub outcome: Outcome,
    /// What a stopped access could have meant; empty when it resolved.
    pub candidates: Candidates,
}

impl Resolution {
    /// The member the access ends in, when every name was told.
    pub fn target(&self) -> Option<&NodeKey> {
        match &self.outcome {
            Outcome::Resolved { target, .. } => Some(target),
            _ => None,
        }
    }

    /// The access's grade: the weakest of its links when it resolved,
    /// [`Confidence::Ambiguous`] when it stopped at several candidates,
    /// `None` when it could not be told — no edge.
    pub fn grade(&self) -> Option<Confidence> {
        match &self.outcome {
            Outcome::Resolved { grade, .. } => Some(*grade),
            Outcome::Ambiguous { .. } => Some(Confidence::Ambiguous),
            Outcome::Unresolved { .. } => None,
        }
    }

    /// Why it stopped, when it did.
    pub fn reason(&self) -> Option<&Unresolved> {
        match &self.outcome {
            Outcome::Resolved { .. } => None,
            Outcome::Ambiguous { reason } | Outcome::Unresolved { reason } => Some(reason),
        }
    }

    /// The links as 2a's [`Chain`] holds them.
    pub fn chain(&self) -> Chain {
        Chain {
            links: self.links.iter().map(|l| l.link.clone()).collect(),
            stopped: self.reason().cloned(),
        }
    }
}

/// The weakest of grades (the order of [`Confidence`] is strongest first);
/// `None` for none.
pub fn weakest(grades: impl IntoIterator<Item = Confidence>) -> Option<Confidence> {
    grades.into_iter().max()
}

/// Whether a stop is several candidates rather than none.
fn is_ambiguity(why: &Unresolved) -> bool {
    matches!(
        why,
        Unresolved::AmbiguousMember(_)
            | Unresolved::AmbiguousType(_)
            | Unresolved::AmbiguousLocal(_)
            | Unresolved::AmbiguousBase(_)
            | Unresolved::AmbiguousConstructor(_)
            | Unresolved::AmbiguousFields(_)
    )
}

/// Whether a path names a member: one name after `this`, `base` or an
/// expression, or one after the local it starts at.
fn names_a_member(path: &AccessPath) -> bool {
    let local = usize::from(path.root == PathRoot::Local);
    path.segments.len() > local
}

/// One access or call of a body, resolved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Site {
    pub place: Place,
    /// The node of the body it is under.
    pub node: Option<u32>,
    pub path: AccessPath,
    /// The access's mode; `None` for a call.
    pub mode: Option<AccessMode>,
    /// The argument count, for a call.
    pub call: Option<u32>,
    pub resolution: Resolution,
}

/// The resolver of step 2b over an [`Index`], with the ambiguity ceiling of
/// the run.
pub struct Links<'i, 'a> {
    ix: &'i Index<'a>,
    ceiling: usize,
}

impl<'i, 'a> Links<'i, 'a> {
    /// `ceiling` is the run's `ambiguity_ceiling`
    /// ([`crate::config::DEFAULT_AMBIGUITY_CEILING`] by default).
    pub fn new(ix: &'i Index<'a>, ceiling: usize) -> Links<'i, 'a> {
        Links { ix, ceiling }
    }

    pub fn index(&self) -> &'i Index<'a> {
        self.ix
    }

    pub fn ceiling(&self) -> usize {
        self.ceiling
    }

    /// The member of `name` the external index pinned from `caller` at a
    /// line `place` spans: the one such target, never one of several.
    fn pinned(&self, caller: &NodeKey, place: &Place, name: &str) -> Option<NodeKey> {
        let lines = place.range.start.line..=place.range.end.line;
        let mut hits = self
            .ix
            .indexed_from(caller)
            .iter()
            .filter(|(target, sites)| {
                self.ix.symbol(target).is_some_and(|s| s.name == name)
                    && sites
                        .iter()
                        .any(|s| s.file == place.path && lines.contains(&s.line))
            })
            .map(|(target, _)| target);
        let first = hits.next()?;
        match hits.find(|t| *t != first) {
            None => Some(first.clone()),
            Some(_) => None,
        }
    }

    /// The access `path` written at `place` in a body of `caller`, with
    /// `scope` its enclosing type and node, resolved link by link and graded.
    /// `call` is the argument count when the access is a call.
    pub fn resolve(
        &self,
        caller: &NodeKey,
        scope: &Scope<'_>,
        path: &AccessPath,
        call: Option<u32>,
        place: &Place,
    ) -> Resolution {
        let pin =
            |_: Option<&NodeKey>, name: &str, _: Option<u32>| self.pinned(caller, place, name);
        let walk = self.ix.walk(scope, path, call, &pin);
        let links: Vec<GradedLink> = walk
            .chain
            .links
            .into_iter()
            .zip(walk.bases)
            .map(|(link, basis)| GradedLink {
                link,
                basis,
                grade: basis.grade(),
            })
            .collect();
        let (outcome, candidates) = match (walk.chain.stopped, walk.stop) {
            (None, _) => match (links.last(), weakest(links.iter().map(|l| l.grade))) {
                (Some(last), Some(grade)) => (
                    Outcome::Resolved {
                        target: last.link.member.clone(),
                        grade,
                    },
                    Candidates::default(),
                ),
                // A path of no names: nothing to tell.
                _ => (
                    Outcome::Unresolved {
                        reason: Unresolved::NoReceiverChain,
                    },
                    Candidates::default(),
                ),
            },
            (Some(reason), stop) => {
                let candidates =
                    Candidates::capped(self.candidates(stop.as_ref(), &reason, call), self.ceiling);
                if is_ambiguity(&reason) {
                    (Outcome::Ambiguous { reason }, candidates)
                } else {
                    (Outcome::Unresolved { reason }, candidates)
                }
            }
        };
        Resolution {
            links,
            outcome,
            candidates,
        }
    }

    /// What an access stopped at `stop` could have meant: the members of the
    /// name in the type in hand or its base when the walk knew that type and
    /// found several (narrowed to the call's arity when that leaves several),
    /// else every member of the name.
    fn candidates(&self, stop: Option<&Stop>, why: &Unresolved, call: Option<u32>) -> Vec<NodeKey> {
        let Some(Stop {
            name: Some(name),
            ty,
        }) = stop
        else {
            return Vec::new();
        };
        if let (Unresolved::AmbiguousMember(_), Some(ty)) = (why, ty) {
            let named = |t: &NodeKey| -> Vec<&NodeKey> {
                self.ix
                    .members_of(t)
                    .iter()
                    .filter(|k| self.ix.symbol(k).is_some_and(|s| s.name == *name))
                    .collect()
            };
            let mut list = named(ty);
            if list.is_empty() {
                if let Ok(Some(base)) = self.ix.base_of(ty) {
                    list = named(base);
                }
            }
            if let Some(arity) = call {
                if let ByArity::Several(of) = self.ix.by_arity(&list, arity) {
                    return of;
                }
            }
            return list.into_iter().cloned().collect();
        }
        self.ix.members_named(name).to_vec()
    }

    /// Every member access and every call of `member`'s bodies, resolved: the
    /// calls with their argument counts, the other accesses (a read, a write,
    /// a return, a mutation) by their paths; the access a call is also
    /// recorded as is not repeated, and a path that names no member (`this`,
    /// a local or a parameter alone) is none. In the order of the bodies and
    /// of the model. Empty for a member of no type.
    pub fn sites_of(&self, member: &NodeKey) -> Vec<Site> {
        let Some(ty) = self.ix.symbol(member).and_then(|s| s.owner.clone()) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for at in bodies(self.ix, member) {
            let place = |range| Place {
                path: at.path.to_string(),
                range,
            };
            for inv in at
                .body
                .invocations
                .iter()
                .filter(|i| i.kind == CallKind::Call)
            {
                let scope = Scope {
                    ty: &ty,
                    body: Some(at.body),
                    node: inv.node,
                };
                let place = place(inv.place);
                let resolution = self.resolve(member, &scope, &inv.callee, Some(inv.arity), &place);
                out.push(Site {
                    place,
                    node: inv.node,
                    path: inv.callee.clone(),
                    mode: None,
                    call: Some(inv.arity),
                    resolution,
                });
            }
            for a in at
                .body
                .accesses
                .iter()
                .filter(|a| a.mode != AccessMode::Invoke && names_a_member(&a.target))
            {
                let scope = Scope {
                    ty: &ty,
                    body: Some(at.body),
                    node: a.node,
                };
                let place = place(a.place);
                let resolution = self.resolve(member, &scope, &a.target, None, &place);
                out.push(Site {
                    place,
                    node: a.node,
                    path: a.target.clone(),
                    mode: Some(a.mode),
                    call: None,
                    resolution,
                });
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(k: &str) -> NodeKey {
        NodeKey::new(k)
    }

    #[test]
    fn candidates_are_sorted_deduplicated_and_cut_at_the_ceiling() {
        let all = vec![key("c"), key("a"), key("b"), key("a"), key("d")];
        let cut = Candidates::capped(all.clone(), 2);
        assert_eq!(cut.shown, [key("a"), key("b")]);
        assert_eq!(cut.more, 2);
        assert_eq!(cut.total(), 4);
        let whole = Candidates::capped(all, 8);
        assert_eq!(whole.shown.len(), 4);
        assert_eq!(whole.more, 0);
        assert!(Candidates::capped(Vec::new(), 8).is_empty());
        let none_shown = Candidates::capped(vec![key("a")], 0);
        assert!(none_shown.shown.is_empty());
        assert_eq!(none_shown.more, 1);
    }

    #[test]
    fn the_weakest_grade_is_the_greatest_and_an_index_link_is_strongest() {
        assert_eq!(
            weakest([Confidence::ResolvedByIndex, Confidence::RulesBacked]),
            Some(Confidence::RulesBacked)
        );
        assert_eq!(
            weakest([Confidence::RulesBacked, Confidence::Ambiguous]),
            Some(Confidence::Ambiguous)
        );
        assert_eq!(weakest([]), None);
        assert_eq!(Basis::Index.grade(), Confidence::ResolvedByIndex);
        for b in [
            Basis::Enclosing,
            Basis::DeclaredType,
            Basis::BaseTypeArgument,
            Basis::Pattern,
            Basis::Overload,
        ] {
            assert_eq!(b.grade(), Confidence::RulesBacked, "{b:?}");
        }
    }

    #[test]
    fn a_path_names_a_member_past_its_local() {
        let path = |root, segments: &[&str]| AccessPath {
            root,
            segments: segments.iter().map(|s| s.to_string()).collect(),
        };
        assert!(names_a_member(&path(PathRoot::This, &["x"])));
        assert!(names_a_member(&path(PathRoot::Expression, &["Ping"])));
        assert!(names_a_member(&path(PathRoot::Local, &["view", "Save"])));
        assert!(!names_a_member(&path(PathRoot::This, &[])));
        assert!(!names_a_member(&path(PathRoot::Local, &["view"])));
    }

    #[test]
    fn a_stop_at_several_candidates_is_an_ambiguity_and_one_at_none_is_not() {
        assert!(is_ambiguity(&Unresolved::AmbiguousMember("Check".into())));
        assert!(is_ambiguity(&Unresolved::AmbiguousType("View".into())));
        assert!(!is_ambiguity(&Unresolved::NotAMember("Check".into())));
        assert!(!is_ambiguity(&Unresolved::NestedGeneric("List<M>".into())));
        assert!(!is_ambiguity(&Unresolved::UnboundTypeParameter("T".into())));
    }
}
