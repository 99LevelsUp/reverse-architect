//! The binding of a delegate from the code of the class constructed (plan
//! 2b, U3: R3, R6; KTD4).
//!
//! `new C(args)` is bound by 2a's command binding, not by a second one: the
//! constructor of `C` taking that many arguments (chosen by arity as every
//! overload is, U2), every `: this(…)` and `: base(…)` its parameters are
//! passed on by, the field each parameter is written to, and the methods of
//! `C` and of its bases that call that field — directly, through `Invoke` or
//! `?.Invoke` ([`super::entry::bind_construction`]). What this module adds:
//!
//! - **pairs** (method of the class, target): the method that calls the field
//!   is the delegate's entry, the target is what the lambda of the argument
//!   calls, or the method group — chosen by its name and by the argument
//!   count the class calls the field with, so `new RelayCommand(Check)` picks
//!   the `Check` of that arity. Each target is resolved and graded by U2
//!   ([`super::link`]); a pair stands on the weakest grade of its chain;
//! - **every class of the shape**: nothing here knows a command class by its
//!   name, a class of any name that writes a constructor parameter to a field
//!   and calls it is bound the same way;
//! - **a field written from two arguments** is [`Confidence::Ambiguous`] and
//!   bound to neither;
//! - **the methods left idle**: a method calling a field some constructor
//!   writes from a parameter that this construction does not pass
//!   (`: this(execute, null)` leaves `CanExecute` without a target);
//! - **events raised** for the results of a flow (KTD6): a method calling a
//!   field, a property or an event that no constructor of the class or of its
//!   bases writes from a parameter raises it (`Executed?.Invoke(this, e)`).
//!   Being called proves it is a delegate; no list of delegate type names is
//!   read.
//!
//! A parameter stored behind a null guard (`_execute = execute ?? throw …`)
//! is written from that parameter: the body model gives the write the
//! parameter as its value, so the field is bound and its calls are no
//! raised events.
//!
//! Limits: a field written from a parameter through anything else — a
//! fallback (`execute ?? Noop`), a conversion, a call — is not seen as
//! written from it, so its calls are taken for raised events and the
//! construction does not bind it.

use super::entry::{
    bodies, construction_class, delegate_sites, field_named, field_writes, flows_out_of, invokers,
    is_field_path, At, Place, Region,
};
use super::link::{weakest, Links, Resolution};
use super::resolve::{Index, Scope, Unresolved};
use crate::extract::bodies::{AccessMode, BodyKind, CallKind, Invocation, PathRoot, ValueRef};
use crate::graph::{Confidence, NodeKey};
use serde::Serialize;
use std::collections::BTreeSet;

/// One argument of a construction bound to the field it is written to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoundSlot {
    pub position: u32,
    pub field: NodeKey,
    /// The methods of the class that call the field, by key.
    pub invoked_by: Vec<NodeKey>,
    /// The argument as written: a lambda, a method group, anything else.
    pub value: ValueRef,
    /// A lambda's body: the delegate's target itself when it calls nothing.
    pub lambda: Option<Region>,
    /// What the lambda calls, or the method group, resolved (U2).
    pub targets: Vec<Resolution>,
}

/// A field written from more than one argument: admitted, bound to neither
/// ([`Confidence::Ambiguous`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AmbiguousField {
    pub field: NodeKey,
    pub positions: Vec<u32>,
    pub invoked_by: Vec<NodeKey>,
}

/// A method of the class calling a delegate field this construction passes
/// nothing to: it has no target.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct IdleInvoker {
    pub method: NodeKey,
    pub field: NodeKey,
}

/// A method of the class raising an event: it calls a value member no
/// constructor writes from a parameter (KTD6).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct RaisedEvent {
    pub method: NodeKey,
    /// The field, property or event called.
    pub field: NodeKey,
    pub place: Place,
}

/// The pair a bound delegate makes (KTD4): the method of the class that
/// calls the field, and what it leads to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DelegatePair {
    pub method: NodeKey,
    pub field: NodeKey,
    pub position: u32,
    pub target: Resolution,
    /// The weakest grade of the chain: the construction and the field bound
    /// by rules, then the target's; `None` when the target was not told.
    pub grade: Option<Confidence>,
}

/// A construction `new C(…)` bound from `C`'s code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DelegateBinding {
    pub class: NodeKey,
    pub constructor: NodeKey,
    pub place: Place,
    /// The body the construction is in: a [`ValueRef::Lambda`] is an index
    /// into its `lambdas`.
    pub region: Region,
    /// By position, then field.
    pub slots: Vec<BoundSlot>,
    /// By field.
    pub ambiguous: Vec<AmbiguousField>,
    /// By method, then field.
    pub idle: Vec<IdleInvoker>,
    /// What the class raises, by method and place.
    pub raises: Vec<RaisedEvent>,
}

impl DelegateBinding {
    /// Every (method, target) pair, by method, then position, then the
    /// target's place in the model's order.
    pub fn pairs(&self) -> Vec<DelegatePair> {
        let mut out = Vec::new();
        for slot in &self.slots {
            for method in &slot.invoked_by {
                for target in &slot.targets {
                    // The construction and the field are told by rules.
                    let grade = target
                        .grade()
                        .and_then(|g| weakest([Confidence::RulesBacked, g]));
                    out.push(DelegatePair {
                        method: method.clone(),
                        field: slot.field.clone(),
                        position: slot.position,
                        target: target.clone(),
                        grade,
                    });
                }
            }
        }
        out.sort_by(|a, b| (&a.method, a.position).cmp(&(&b.method, b.position)));
        out
    }

    /// The binding's grade: [`Confidence::Ambiguous`] when a field is written
    /// from two arguments, else the weakest of its pairs that were told.
    pub fn grade(&self) -> Option<Confidence> {
        if !self.ambiguous.is_empty() {
            return Some(Confidence::Ambiguous);
        }
        weakest(self.pairs().iter().filter_map(|p| p.grade))
    }
}

/// A construction of a type, bound or why not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SiteBinding {
    /// The member the construction is written in.
    pub member: NodeKey,
    pub place: Place,
    pub class: NodeKey,
    pub binding: Result<DelegateBinding, Unresolved>,
}

/// The argument count `class` calls `field` with, the same at every call:
/// what a method group passed for the field is chosen among overloads by.
pub fn field_call_arity(ix: &Index<'_>, class: &NodeKey, field: &NodeKey) -> Option<u32> {
    let name = ix.name_of(field);
    let arities: BTreeSet<u32> = invokers(ix, class, field)
        .iter()
        .flat_map(|m| ix.layer.bodies_of(m))
        .flat_map(|pb| pb.body.invocations.iter())
        .filter(|inv| {
            inv.kind == CallKind::Call
                && matches!(inv.callee.root, PathRoot::This | PathRoot::Name)
                && match inv.callee.segments.as_slice() {
                    [f] => f == name,
                    [f, invoke] => f == name && invoke == "Invoke",
                    _ => false,
                }
        })
        .map(|inv| inv.arity)
        .collect();
    match arities.len() {
        1 => arities.into_iter().next(),
        _ => None,
    }
}

/// The value members of `class` and of its bases that a constructor of one
/// of them writes from one of its parameters.
fn written_from_parameters(ix: &Index<'_>, class: &NodeKey) -> BTreeSet<NodeKey> {
    let mut out = BTreeSet::new();
    for ty in std::iter::once(class).chain(ix.ancestors(class)) {
        for m in ix.members_of(ty) {
            if ix.symbol(m).is_none_or(|s| s.kind != "constructor") {
                continue;
            }
            for pb in ix.layer.bodies_of(m) {
                if pb.body.kind != BodyKind::Constructor {
                    continue;
                }
                for a in &pb.body.accesses {
                    let (AccessMode::Write, Some(ValueRef::Param(_)), Some(name)) =
                        (a.mode, &a.rhs, is_field_path(&a.target))
                    else {
                        continue;
                    };
                    out.extend(field_named(ix, ty, name));
                }
            }
        }
    }
    out
}

/// The events `class` raises (see the module docs): every call of a value
/// member of the class or of its bases, in a method of either, that no
/// constructor writes from a parameter — by method and place.
pub fn raised_events(ix: &Index<'_>, class: &NodeKey) -> Vec<RaisedEvent> {
    let bound = written_from_parameters(ix, class);
    let mut out = BTreeSet::new();
    for ty in std::iter::once(class).chain(ix.ancestors(class)) {
        for m in ix.members_of(ty) {
            if !ix.symbol(m).is_some_and(|s| s.is_method()) {
                continue;
            }
            for at in bodies(ix, m) {
                for a in &at.body.accesses {
                    if a.mode != AccessMode::Invoke {
                        continue;
                    }
                    let Some(name) = is_field_path(&a.target) else {
                        continue;
                    };
                    let Some(field) = field_named(ix, class, name) else {
                        continue;
                    };
                    if bound.contains(&field) {
                        continue;
                    }
                    out.insert(RaisedEvent {
                        method: m.clone(),
                        field,
                        place: Place {
                            path: at.path.to_string(),
                            range: a.place,
                        },
                    });
                }
            }
        }
    }
    out.into_iter().collect()
}

/// The construction `inv`, written in `at`, a body of `caller` of the type
/// `ty`, bound from its class's code (see the module docs). `declared` is
/// the member a target-typed `new()` flows into. `None` for no construction
/// of a class of the repository; `Err` with the class when no single
/// constructor takes that many arguments.
pub(crate) fn bind(
    links: &Links<'_, '_>,
    ty: &NodeKey,
    caller: &NodeKey,
    at: At<'_>,
    inv: &Invocation,
    declared: Option<&NodeKey>,
) -> Option<(NodeKey, Result<DelegateBinding, Unresolved>)> {
    let ix = links.index();
    let class = construction_class(ix, inv, declared)?;
    let (constructor, writes) = match field_writes(ix, &class, inv.arity) {
        Ok(found) => found,
        Err(why) => return Some((class, Err(why))),
    };
    let place = |range| Place {
        path: at.path.to_string(),
        range,
    };
    let mut slots = Vec::new();
    let mut ambiguous = Vec::new();
    let mut fields: BTreeSet<NodeKey> = BTreeSet::new();
    for (field, positions) in &writes {
        fields.insert(field.clone());
        let invoked_by = invokers(ix, &class, field);
        let positions: Vec<u32> = positions.iter().copied().collect();
        let &[position] = positions.as_slice() else {
            ambiguous.push(AmbiguousField {
                field: field.clone(),
                positions,
                invoked_by,
            });
            continue;
        };
        let Some(arg) = inv.args.iter().find(|a| a.position == position) else {
            continue;
        };
        let lambda = match arg.value {
            ValueRef::Lambda(id) => at.body.lambdas.get(id as usize).map(|l| Region {
                path: at.path.to_string(),
                body: at.index,
                node: Some(l.body),
            }),
            _ => None,
        };
        let targets = delegate_sites(at, arg, field_call_arity(ix, &class, field))
            .into_iter()
            .map(|site| {
                let scope = Scope {
                    ty,
                    body: Some(at.body),
                    node: site.node,
                };
                links.resolve(caller, &scope, site.path, site.call, &place(site.place))
            })
            .collect();
        slots.push(BoundSlot {
            position,
            field: field.clone(),
            invoked_by,
            value: arg.value.clone(),
            lambda,
            targets,
        });
    }
    slots.sort_by(|a, b| (a.position, &a.field).cmp(&(b.position, &b.field)));
    let mut idle: Vec<IdleInvoker> = written_from_parameters(ix, &class)
        .into_iter()
        .filter(|f| !fields.contains(f))
        .flat_map(|field| {
            invokers(ix, &class, &field)
                .into_iter()
                .map(move |method| IdleInvoker {
                    method,
                    field: field.clone(),
                })
        })
        .collect();
    idle.sort();
    Some((
        class.clone(),
        Ok(DelegateBinding {
            raises: raised_events(ix, &class),
            class,
            constructor,
            place: place(inv.place),
            region: Region {
                path: at.path.to_string(),
                body: at.index,
                node: inv.node,
            },
            slots,
            ambiguous,
            idle,
        }),
    ))
}

/// Every construction of a repository class in the bodies of `ty`'s own
/// members whose binding leads somewhere — an argument ends in a field the
/// class calls, or a field is written from two arguments — or cannot be
/// told; by member, then place. A construction of a class that calls no
/// field written from it (a plain object) is left out.
pub fn bindings_of(links: &Links<'_, '_>, ty: &NodeKey) -> Vec<SiteBinding> {
    let ix = links.index();
    let mut out = Vec::new();
    for m in ix.members_of(ty) {
        if ix.symbol(m).is_none_or(|s| s.is_type()) {
            continue;
        }
        for at in bodies(ix, m) {
            for inv in &at.body.invocations {
                // A target-typed `new()` takes its class from the property
                // it flows out of, as for 2a's command properties.
                let declared = flows_out_of(ix, m, at.body, inv).then_some(m);
                let Some((class, binding)) = bind(links, ty, m, at, inv, declared) else {
                    continue;
                };
                let leads = match &binding {
                    Ok(b) => {
                        b.slots.iter().any(|s| !s.invoked_by.is_empty())
                            || b.ambiguous.iter().any(|f| !f.invoked_by.is_empty())
                    }
                    Err(Unresolved::NoConstructor(_)) => false,
                    Err(_) => true,
                };
                if leads {
                    out.push(SiteBinding {
                        member: m.clone(),
                        place: Place {
                            path: at.path.to_string(),
                            range: inv.place,
                        },
                        class,
                        binding,
                    });
                }
            }
        }
    }
    out.sort_by(|a, b| (&a.member, &a.place).cmp(&(&b.member, &b.place)));
    out
}
