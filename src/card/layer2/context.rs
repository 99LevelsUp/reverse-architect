//! The flows of the core functionalities: one for every trigger that leads to
//! one (plan 2b, U5: R1, R2, R7, R9; KTD7). Nothing here renders; the
//! `### Context` of a card (U6) draws it.
//!
//! # A flow (KTD7)
//!
//! A flow has the trigger's own part — its precondition (the key branch's
//! condition, R7) and the members it goes through on its way to the
//! functionality, each one step at the place it calls on (a type the flow
//! only passes is one step, never expanded, R6) — and the part from where it
//! enters the functionality on, which every flow entering at the same members
//! shares and which is written once ([`CommonPart`]; the flows refer to it by
//! index):
//!
//! - the preconditions found in the members it reaches from there (a guard,
//!   a CanExecute predicate of a command it enters through — U11 of 2a);
//! - the steps: the command class as one step `Execute → target` per bound
//!   delegate (KTD4, R3), an argument of a call of the functionality's own
//!   member that chooses the callee's mode (an expression with an enum member,
//!   or one given to a `bool` parameter: `Check((Keyboard.Modifiers &
//!   ModifierKeys.Shift) == ModifierKeys.Shift)`, R7), a call of another
//!   type — resolved and graded by U2, one step with its card — or the
//!   unresolved continuation with its candidates (R4), and the other steps of
//!   2a;
//! - the results of U4 the members it reaches have, and the paths that stop
//!   before the main work.
//!
//! What the flow reaches inside the functionality is what its entry members
//! call, bind as a delegate or pass as a method group, by strong links only:
//! an entry from another component that calls `Check(bool)` gets neither the
//! command's CanExecute nor the branch on Shift that `Check()` holds (AE4).
//!
//! # The exceptions
//!
//! - **a command class** (a functionality holding a method that calls a
//!   delegate field bound from a construction, U3): one flow from those
//!   methods on, with its guard and its results, and only the number of
//!   triggers that lead to it — a flow per trigger would put every key and
//!   menu item of the corpus on its card (AE9);
//! - **an entry point that is a trigger and leads to another type's
//!   functionality** (the key branch of a view): its flow is drawn at the
//!   functionality it leads to, and it refers there (R9). A trigger whose
//!   code never leaves its type keeps its flow.
//!
//! # The state of a row
//!
//! A row whose shared part holds a dead path that turns the whole way off —
//! a CanExecute delegate's target that returns `false`, an empty body an
//! entry point calls — is dead, with that place (R12, AE2). A row whose way
//! passes through a functionality the same trigger's row into is dead is dead
//! too, with that row's place: a command that never runs leads nowhere
//! further. A trigger's own row that refers elsewhere is dead when every row
//! it refers to is.

use super::callers::{is_strong, Callers, Via};
use super::cluster::Functionality;
use super::effects::{FlowResult, StopCause, StoppedPath, TypeResults};
use super::entry::{bodies, EntryKind, Place};
use super::flow::{is_can_execute, Cond, DeadKind, Guarding, Occurrence, StepKind};
use super::link::{Candidates, Links, Outcome};
use super::resolve::{Scope, Unresolved};
use super::triggers::{delegate_hosts, FunctionalityRef, PathState, Trigger, TriggerRow};
use super::TypeAnalysis;
use crate::extract::bodies::{
    AccessPath, BodyKind, CallKind, Constant, DeclarationKind, PairedBy, ValueRef,
};
use crate::graph::{Confidence, EdgeKind, NodeKey};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// A precondition of a flow: one place a precondition of U11 was found at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContextPrecondition {
    /// The guard's text as written, or the predicate's tree rendered.
    pub text: String,
    pub when: Cond,
    pub occurrence: Occurrence,
}

/// The trigger's own precondition (R7): a key branch's condition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TriggerCondition {
    /// As written.
    pub text: String,
    pub place: Option<Place>,
}

/// What a step of a flow is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FlowStepKind {
    /// A member on the trigger's way to the functionality, at the place it
    /// calls on; a member of another type is one step with its card.
    Through,
    /// A command class as one step: the method of the class that calls the
    /// bound field, and what the argument leads to (KTD4, R3).
    Delegate {
        class: NodeKey,
        invoker: NodeKey,
        /// By key.
        targets: Vec<NodeKey>,
        /// The delegate is the command's CanExecute.
        can_execute: bool,
    },
    /// An argument that chooses the mode of the functionality's own callee
    /// (R7).
    ArgumentChoice {
        callee: NodeKey,
        position: u32,
        /// The callee's parameter at that position.
        parameter: Option<String>,
        enum_refs: Vec<String>,
        reads: Vec<AccessPath>,
    },
    /// A call into another type: one step, with its card (R6).
    Call {
        target: NodeKey,
        owner: NodeKey,
        grade: Confidence,
    },
    /// A call the rules could not follow: the flow stops there, with the
    /// candidates (R4, KTD9).
    UnresolvedContinuation {
        path: AccessPath,
        reason: Unresolved,
        candidates: Candidates,
    },
    /// Another step of the functionality's code (2a, U11).
    Step(StepKind),
}

/// One step of a flow, with its anchor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FlowStep {
    /// The member whose code it is.
    pub member: NodeKey,
    /// `None` only for a hop of a first-layer relation, which has no place
    /// of a body.
    pub place: Option<Place>,
    /// How sure the link is, where the step is one.
    pub grade: Option<Confidence>,
    pub kind: FlowStepKind,
}

/// The part of the flows from where they enter the functionality on (KTD7):
/// written once, every flow entering at the same members refers to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommonPart {
    /// The functionality's members the flows enter at, by key.
    pub entries: Vec<NodeKey>,
    /// By the order of U11: first place, then condition; each occurrence once.
    pub preconditions: Vec<ContextPrecondition>,
    /// The delegate steps first, then by member and place.
    pub steps: Vec<FlowStep>,
    /// By kind, member and place (U4).
    pub results: Vec<FlowResult>,
    /// By place, then member (U4).
    pub stops: Vec<StoppedPath>,
    /// The dead path that turns the whole way off, when one does: its member
    /// and place.
    pub dead: Option<(NodeKey, Place)>,
}

/// The flow of one row of the registry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TriggerFlow {
    /// The row: an index into [`super::Analysis::triggers`].
    pub row: usize,
    /// The row's trigger, as the registry holds it — what the card's
    /// `### Context` draws without the registry (U6).
    pub trigger: Trigger,
    /// The row's state once the composition told every dead row (U6).
    pub state: PathState,
    pub condition: Option<TriggerCondition>,
    /// The trigger's way to the functionality, one [`FlowStepKind::Through`]
    /// per member.
    pub path: Vec<FlowStep>,
    /// The part it shares: an index into [`FunctionalityContext::common`].
    pub common: usize,
}

/// A command class's one flow and how many triggers lead to it (AE9).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommandClass {
    /// An index into [`FunctionalityContext::common`].
    pub common: usize,
    /// The rows of the registry that lead to it.
    pub triggers: usize,
}

/// The `### Context` of one functionality.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FunctionalityContext {
    pub key: String,
    pub common: Vec<CommonPart>,
    /// By the order of the registry.
    pub flows: Vec<TriggerFlow>,
    pub command_class: Option<CommandClass>,
    /// The functionalities of other types its own triggers' flows are drawn
    /// at (R9), by type and key.
    pub refers: Vec<FunctionalityRef>,
}

/// What step 2b gives a type: the results and state of U4 and the context
/// of every functionality.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TypeContext {
    pub results: TypeResults,
    /// One per functionality, by key.
    pub functionalities: Vec<FunctionalityContext>,
}

impl TypeContext {
    /// The context of a type before step 2b composed it.
    pub(crate) fn empty(key: &NodeKey) -> TypeContext {
        TypeContext {
            results: TypeResults {
                key: key.clone(),
                functionalities: Vec::new(),
                states: Vec::new(),
            },
            functionalities: Vec::new(),
        }
    }

    /// The context of the functionality `key`.
    pub fn of(&self, key: &str) -> Option<&FunctionalityContext> {
        self.functionalities
            .binary_search_by(|f| f.key.as_str().cmp(key))
            .ok()
            .map(|i| &self.functionalities[i])
    }
}

/// What composes the flows of one functionality.
struct Composer<'c, 'i, 'l> {
    links: &'c Links<'i, 'l>,
    t: &'c TypeAnalysis,
    f: &'c Functionality,
    results: Option<&'c super::effects::FunctionalityResults>,
    /// What each member of the functionality calls of it, binds or passes.
    inner: BTreeMap<NodeKey, BTreeSet<NodeKey>>,
}

/// The members of `f` each member of it leads to by a strong call, a
/// delegate bound in it, or a method group it passes.
fn inner_calls(
    callers: &Callers,
    ix: &super::Index<'_>,
    f: &Functionality,
) -> BTreeMap<NodeKey, BTreeSet<NodeKey>> {
    let members: BTreeSet<&NodeKey> = f.members.iter().collect();
    let mut out: BTreeMap<NodeKey, BTreeSet<NodeKey>> = BTreeMap::new();
    for y in &f.members {
        let method = ix
            .symbol(y)
            .is_some_and(|s| matches!(s.kind.as_str(), "method" | "constructor"));
        for c in callers.of(y) {
            for e in c.evidence.iter().filter(|e| is_strong(e.grade)) {
                let from = match &e.via {
                    Via::Delegate { built_in, .. } => built_in,
                    Via::Link(s) if s.call.is_some() || method => &c.member,
                    Via::Relation {
                        kind: EdgeKind::Calls | EdgeKind::Instantiates,
                        ..
                    } => &c.member,
                    _ => continue,
                };
                if from != y && members.contains(from) {
                    out.entry(from.clone()).or_default().insert(y.clone());
                }
            }
        }
    }
    out
}

/// The parameters a method declares, in order: name and type as written.
fn parameters(ix: &super::Index<'_>, method: &NodeKey) -> Vec<(String, String)> {
    let Some(pb) = ix
        .layer
        .bodies_of(method)
        .into_iter()
        .find(|pb| pb.by == PairedBy::Name && pb.body.kind == BodyKind::Method)
    else {
        return Vec::new();
    };
    let mut params: Vec<_> = pb
        .body
        .typed_names
        .iter()
        .filter(|t| t.kind == DeclarationKind::Parameter && t.scope.is_none())
        .collect();
    params.sort_by_key(|t| t.position);
    params
        .into_iter()
        .map(|t| (t.name.clone(), t.type_name.clone()))
        .collect()
}

impl Composer<'_, '_, '_> {
    fn ix(&self) -> &super::Index<'_> {
        self.links.index()
    }

    /// Everything `entries` lead to inside the functionality.
    fn reach(&self, entries: &BTreeSet<NodeKey>) -> BTreeSet<NodeKey> {
        let mut seen: BTreeSet<NodeKey> = BTreeSet::new();
        let mut stack: Vec<NodeKey> = entries.iter().rev().cloned().collect();
        while let Some(x) = stack.pop() {
            if !seen.insert(x.clone()) {
                continue;
            }
            if let Some(next) = self.inner.get(&x) {
                stack.extend(next.iter().rev().cloned());
            }
        }
        seen
    }

    /// The part of every flow entering at `entries` (see the module docs).
    fn common(&self, entries: &BTreeSet<NodeKey>) -> CommonPart {
        let reach = self.reach(entries);
        let flow = self.t.flow_of(&self.f.key);
        let preconditions: Vec<ContextPrecondition> = flow
            .into_iter()
            .flat_map(|fl| &fl.preconditions)
            .flat_map(|p| {
                p.occurrences
                    .iter()
                    .filter(|o| reach.contains(&o.member))
                    .map(|o| ContextPrecondition {
                        text: p.text.clone(),
                        when: p.when.clone(),
                        occurrence: o.clone(),
                    })
            })
            .collect();
        let results: Vec<FlowResult> = self
            .results
            .into_iter()
            .flat_map(|r| &r.results)
            .filter(|r| reach.contains(&r.member))
            .cloned()
            .collect();
        let stops: Vec<StoppedPath> = self
            .results
            .into_iter()
            .flat_map(|r| &r.stops)
            .filter(|s| reach.contains(&s.member))
            .cloned()
            .collect();
        let dead = stops.iter().find_map(|s| {
            let StopCause::Dead(kind) = &s.cause else {
                return None;
            };
            let guarding = flow
                .into_iter()
                .flat_map(|fl| &fl.dead_paths)
                .find(|d| d.member == s.member && d.place == s.place)
                .map(|d| d.guarding.as_slice())
                .unwrap_or_default();
            let off = match kind {
                DeadKind::ConstantReturn(Constant::Boolean(false)) => {
                    guarding.contains(&Guarding::CanExecute)
                }
                DeadKind::EmptyBody => {
                    guarding.contains(&Guarding::CalledByEntry)
                        || guarding.contains(&Guarding::CanExecute)
                }
                _ => false,
            };
            off.then(|| (s.member.clone(), s.place.clone()))
        });

        let mut steps = self.delegate_steps(&reach);
        let mut rest = self.argument_choices(&reach);
        rest.extend(self.own_steps(&reach));
        rest.sort_by(|a, b| (&a.member, &a.place).cmp(&(&b.member, &b.place)));
        steps.extend(rest);
        CommonPart {
            entries: entries.iter().cloned().collect(),
            preconditions,
            steps,
            results,
            stops,
            dead,
        }
    }

    /// The command class as one step per bound delegate of the command
    /// properties reached (KTD4).
    fn delegate_steps(&self, reach: &BTreeSet<NodeKey>) -> Vec<FlowStep> {
        let ix = self.ix();
        let mut out = Vec::new();
        for e in &self.f.entry_points {
            if e.kind != EntryKind::Command || !reach.contains(&e.member) {
                continue;
            }
            for site in &e.commands {
                for d in &site.delegates {
                    let mut targets: Vec<NodeKey> =
                        d.targets.iter().filter_map(|t| t.member.clone()).collect();
                    targets.sort();
                    targets.dedup();
                    for invoker in &d.invoked_by {
                        out.push(FlowStep {
                            member: e.member.clone(),
                            place: Some(site.place.clone()),
                            grade: Some(Confidence::RulesBacked),
                            kind: FlowStepKind::Delegate {
                                class: site.class.clone(),
                                invoker: invoker.clone(),
                                targets: targets.clone(),
                                can_execute: is_can_execute(ix, d),
                            },
                        });
                    }
                }
            }
        }
        out
    }

    /// The arguments that choose the mode of an own callee (R7).
    fn argument_choices(&self, reach: &BTreeSet<NodeKey>) -> Vec<FlowStep> {
        let ix = self.ix();
        let ty = &self.t.key;
        let mut out = Vec::new();
        for x in reach {
            for at in bodies(ix, x) {
                for inv in at
                    .body
                    .invocations
                    .iter()
                    .filter(|i| i.kind == CallKind::Call)
                {
                    let choosing: Vec<_> = inv
                        .args
                        .iter()
                        .filter(|a| a.value == ValueRef::Other)
                        .collect();
                    if choosing.is_empty() {
                        continue;
                    }
                    let place = Place {
                        path: at.path.to_string(),
                        range: inv.place,
                    };
                    let scope = Scope {
                        ty,
                        body: Some(at.body),
                        node: inv.node,
                    };
                    let chain = self
                        .links
                        .chain(x, &scope, &inv.callee, Some(inv.arity), &place);
                    let (None, Some(last)) = (&chain.stopped, chain.links.last()) else {
                        continue;
                    };
                    if !self.f.members.contains(&last.member) {
                        continue;
                    }
                    let params = parameters(ix, &last.member);
                    for a in choosing {
                        let param = params.get(a.position as usize);
                        let boolean = param.is_some_and(|(_, t)| t == "bool" || t == "Boolean");
                        if a.enum_refs.is_empty() && !boolean {
                            continue;
                        }
                        let mut enum_refs = a.enum_refs.clone();
                        enum_refs.sort();
                        enum_refs.dedup();
                        out.push(FlowStep {
                            member: x.clone(),
                            place: Some(place.clone()),
                            grade: None,
                            kind: FlowStepKind::ArgumentChoice {
                                callee: last.member.clone(),
                                position: a.position,
                                parameter: param.map(|(n, _)| n.clone()),
                                enum_refs,
                                reads: a.reads.clone(),
                            },
                        });
                    }
                }
            }
        }
        out
    }

    /// The steps of the members reached: every call of a body resolved and
    /// graded by U2 — a call of another type one step with its card, one the
    /// rules stopped at with candidates the unresolved continuation — and
    /// the other steps of 2a. A call 2a lists that U2 tells nothing of (out
    /// of the repository) stays 2a's step.
    fn own_steps(&self, reach: &BTreeSet<NodeKey>) -> Vec<FlowStep> {
        let ix = self.ix();
        let own: BTreeSet<&NodeKey> = std::iter::once(&self.t.key)
            .chain(ix.ancestors(&self.t.key))
            .collect();
        let mut out = Vec::new();
        let mut told: BTreeSet<(&NodeKey, Place)> = BTreeSet::new();
        for x in reach {
            for site in self.links.sites_of(x) {
                if site.call.is_none() {
                    continue;
                }
                let r = &site.resolution;
                let kind = match &r.outcome {
                    Outcome::Resolved { target, grade } => {
                        let Some(owner) = ix.owner_of(target) else {
                            continue;
                        };
                        if own.contains(owner) {
                            continue;
                        }
                        FlowStepKind::Call {
                            target: target.clone(),
                            owner: owner.clone(),
                            grade: *grade,
                        }
                    }
                    Outcome::Ambiguous { reason } => FlowStepKind::UnresolvedContinuation {
                        path: site.path.clone(),
                        reason: reason.clone(),
                        candidates: r.candidates.clone(),
                    },
                    Outcome::Unresolved { reason } if !r.candidates.is_empty() => {
                        FlowStepKind::UnresolvedContinuation {
                            path: site.path.clone(),
                            reason: reason.clone(),
                            candidates: r.candidates.clone(),
                        }
                    }
                    Outcome::Unresolved { .. } => continue,
                };
                told.insert((x, site.place.clone()));
                out.push(FlowStep {
                    member: x.clone(),
                    place: Some(site.place.clone()),
                    grade: r.grade(),
                    kind,
                });
            }
        }
        for s in self
            .t
            .flow_of(&self.f.key)
            .into_iter()
            .flat_map(|fl| &fl.steps)
            .filter(|s| reach.contains(&s.member))
        {
            let call = matches!(
                s.kind,
                StepKind::Call {
                    construct: false,
                    ..
                }
            );
            if call && told.contains(&(&s.member, s.place.clone())) {
                continue;
            }
            out.push(FlowStep {
                member: s.member.clone(),
                place: Some(s.place.clone()),
                grade: None,
                kind: FlowStepKind::Step(s.kind.clone()),
            });
        }
        out
    }
}

/// The context of every type and functionality, and the dead rows told
/// (see the module docs). `results` are U4's, one per type in order.
pub(crate) fn compose(
    types: &[TypeAnalysis],
    results: Vec<TypeResults>,
    rows: &mut [TriggerRow],
    links: &Links<'_, '_>,
    callers: &Callers,
) -> Vec<TypeContext> {
    let ix = links.index();
    let hosts = delegate_hosts(callers);
    let mut classes: BTreeSet<FunctionalityRef> = BTreeSet::new();
    for t in types {
        for f in &t.functionalities {
            if f.members.iter().any(|m| hosts.contains_key(m)) {
                classes.insert(FunctionalityRef {
                    ty: t.key.clone(),
                    key: f.key.clone(),
                });
            }
        }
    }
    let mut by_target: BTreeMap<&FunctionalityRef, Vec<usize>> = BTreeMap::new();
    let mut by_trigger: BTreeMap<_, Vec<usize>> = BTreeMap::new();
    for (i, r) in rows.iter().enumerate() {
        by_target.entry(&r.target).or_default().push(i);
        by_trigger.entry(r.trigger.identity()).or_default().push(i);
    }

    let mut dead: Vec<(usize, NodeKey, Place)> = Vec::new();
    let mut referring: Vec<(usize, Vec<usize>)> = Vec::new();
    let mut out = Vec::new();
    for (t, type_results) in types.iter().zip(results) {
        let mut functionalities = Vec::new();
        for f in &t.functionalities {
            let r = FunctionalityRef {
                ty: t.key.clone(),
                key: f.key.clone(),
            };
            let composer = Composer {
                links,
                t,
                f,
                results: type_results.of(&f.key),
                inner: inner_calls(callers, ix, f),
            };
            let targeting = by_target.get(&r).cloned().unwrap_or_default();
            let mut fc = FunctionalityContext {
                key: f.key.clone(),
                common: Vec::new(),
                flows: Vec::new(),
                command_class: None,
                refers: Vec::new(),
            };
            if classes.contains(&r) {
                let entries: BTreeSet<NodeKey> = f
                    .members
                    .iter()
                    .filter(|m| hosts.contains_key(*m))
                    .cloned()
                    .collect();
                fc.common.push(composer.common(&entries));
                fc.command_class = Some(CommandClass {
                    common: 0,
                    triggers: targeting.len(),
                });
            } else {
                for i in targeting {
                    let row = &rows[i];
                    if row.own {
                        let elsewhere: Vec<usize> = by_trigger
                            .get(&row.trigger.identity())
                            .into_iter()
                            .flatten()
                            .copied()
                            .filter(|&j| {
                                rows[j].target.ty != t.key && !classes.contains(&rows[j].target)
                            })
                            .collect();
                        if !elsewhere.is_empty() {
                            fc.refers
                                .extend(elsewhere.iter().map(|&j| rows[j].target.clone()));
                            referring.push((i, elsewhere));
                            continue;
                        }
                    }
                    let entries: BTreeSet<NodeKey> = row.entries.iter().cloned().collect();
                    let common = match fc
                        .common
                        .iter()
                        .position(|c| c.entries.iter().eq(entries.iter()))
                    {
                        Some(c) => c,
                        None => {
                            fc.common.push(composer.common(&entries));
                            fc.common.len() - 1
                        }
                    };
                    if let (Some((member, place)), PathState::Active) =
                        (&fc.common[common].dead, &row.state)
                    {
                        dead.push((i, member.clone(), place.clone()));
                    }
                    fc.flows.push(TriggerFlow {
                        row: i,
                        trigger: row.trigger.clone(),
                        state: row.state.clone(),
                        condition: row.trigger.condition.as_ref().map(|c| TriggerCondition {
                            text: c.text.clone(),
                            place: row.trigger.place.clone(),
                        }),
                        path: row
                            .path
                            .iter()
                            .map(|hop| FlowStep {
                                member: hop.member.clone(),
                                // A hop of a first-layer relation has no
                                // place of a body: the card anchors it at
                                // the member's own body (U6).
                                place: hop.place.clone().or_else(|| body_place(ix, &hop.member)),
                                grade: hop.grade,
                                kind: FlowStepKind::Through,
                            })
                            .collect(),
                        common,
                    });
                }
            }
            fc.refers.sort();
            fc.refers.dedup();
            functionalities.push(fc);
        }
        functionalities.sort_by(|a, b| a.key.cmp(&b.key));
        out.push(TypeContext {
            results: type_results,
            functionalities,
        });
    }
    for (i, member, place) in dead {
        rows[i].state = PathState::Dead { member, place };
    }
    pass_death_along_paths(types, rows, &by_trigger);
    for (i, elsewhere) in referring {
        if !matches!(rows[i].state, PathState::Active) {
            continue;
        }
        let first = elsewhere.iter().find_map(|&j| match &rows[j].state {
            PathState::Dead { member, place } => Some((member.clone(), place.clone())),
            _ => None,
        });
        let all = elsewhere
            .iter()
            .all(|&j| matches!(rows[j].state, PathState::Dead { .. }));
        if let (true, Some((member, place))) = (all, first) {
            rows[i].state = PathState::Dead { member, place };
        }
    }
    // Every flow carries its row's final state: the dead rows were told
    // only now.
    for flow in out
        .iter_mut()
        .flat_map(|t| &mut t.functionalities)
        .flat_map(|f| &mut f.flows)
    {
        flow.state = rows[flow.row].state.clone();
    }
    out
}

/// A row whose way passes through a functionality its trigger's own row into
/// is dead goes no further: it is dead too, at that row's place (R12). A key
/// whose command can never run reaches nothing behind the command's target,
/// whatever functionality of another type that target calls into. To a fixed
/// point, so a chain of functionalities passes the death on.
fn pass_death_along_paths(
    types: &[TypeAnalysis],
    rows: &mut [TriggerRow],
    by_trigger: &BTreeMap<super::triggers::Identity, Vec<usize>>,
) {
    let members: BTreeMap<FunctionalityRef, BTreeSet<&NodeKey>> = types
        .iter()
        .flat_map(|t| {
            t.functionalities.iter().map(move |f| {
                let r = FunctionalityRef {
                    ty: t.key.clone(),
                    key: f.key.clone(),
                };
                (r, f.members.iter().collect())
            })
        })
        .collect();
    loop {
        let mut changed = false;
        for i in 0..rows.len() {
            if !matches!(rows[i].state, PathState::Active) {
                continue;
            }
            let same = by_trigger
                .get(&rows[i].trigger.identity())
                .map_or(&[][..], Vec::as_slice);
            let off = rows[i].path.iter().find_map(|hop| {
                same.iter().find_map(|&j| match &rows[j].state {
                    PathState::Dead { member, place }
                        if members
                            .get(&rows[j].target)
                            .is_some_and(|m| m.contains(&hop.member)) =>
                    {
                        Some((member.clone(), place.clone()))
                    }
                    _ => None,
                })
            });
            if let Some((member, place)) = off {
                rows[i].state = PathState::Dead { member, place };
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
}

/// Where the body of `member` is: the place of its root node.
fn body_place(ix: &super::Index<'_>, member: &NodeKey) -> Option<Place> {
    bodies(ix, member).into_iter().find_map(|at| {
        at.body.nodes.get(at.body.root as usize).map(|n| Place {
            path: at.path.to_string(),
            range: n.place,
        })
    })
}
