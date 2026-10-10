//! Entry points of a type and the binding of a command (plan 2a, U5).
//!
//! # Entry points
//!
//! What starts a functionality of a type, each split by the branches of its
//! dispatcher (KTD12 step 1):
//!
//! - a **command property** — a property whose getter returns, or lazily
//!   initializes and returns, a construction the command binding leads from
//!   ([`bind_construction`]);
//! - a **key branch** — an arm tagged `key-compare`, one per key it compares
//!   (`#Key.F4`, with the modifiers of the condition: `#Key.F4+ModifierKeys.Shift`);
//! - a **menu item** — an object tagged `menu-item` that is given a command or
//!   a click handler, told apart by its variable (`#f4`); an item given
//!   neither is only a submenu other items are added to;
//! - an **event handler** — a member subscribed with `+=` (a `-=` of the
//!   same handler is the same subscription), or a lambda subscribed so, told
//!   apart by the event (`#Loaded`); a handler whose body dispatches keys or
//!   menu items is no entry point of its own, its branches are;
//! - a **public method**.
//!
//! A branch reaches what its own arm reaches and a menu item what its command
//! and click handler do: the member they are written in is not part of them,
//! or every branch of one handler would be one functionality.
//!
//! # The command binding
//!
//! [`bind_construction`] reads, from the code of the command class alone and
//! with no list of command classes, which delegate an argument of `new C(…)`
//! becomes: the constructor of `C` taking that many arguments, through every
//! `: this(…)` and `: base(…)` its parameters are passed on by, to the field
//! a parameter is written to, and to the methods of the class that call that
//! field — `Execute`, `CanExecute`, whatever they are named. One function for
//! every class of the shape; step 2b generalizes it to delegates and events.

use super::resolve::{within, Chain, Index, Scope, Unresolved};
use crate::extract::bodies::{
    AccessMode, AccessPath, Argument, BodyKind, CallKind, Condition, FlowTarget, Handler,
    Invocation, MemberBody, PathRoot, Range, SubscriptionMode, ValueRef,
};
use crate::graph::NodeKey;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// How deeply constructor initializers are followed.
const MAX_INITIALIZERS: usize = 8;

/// What starts a functionality, in the order a representative is chosen in
/// (KTD11) — see [`EntryKind::rank`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EntryKind {
    Command,
    Key,
    MenuItem,
    Handler,
    PublicMethod,
}

impl EntryKind {
    /// The rank of the kind among representatives (KTD11): a command
    /// property, then a key branch or a menu item, then an event handler,
    /// then a public method.
    pub fn rank(self) -> u8 {
        match self {
            EntryKind::Command => 0,
            EntryKind::Key | EntryKind::MenuItem => 1,
            EntryKind::Handler => 2,
            EntryKind::PublicMethod => 3,
        }
    }
}

/// A stretch of a file.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Place {
    pub path: String,
    pub range: Range,
}

/// Where an entry point's code is: a body of the model (`path`, the index of
/// the body in the file's `members`) and, for a branch or a lambda, the node
/// its code is under; `None` for the whole body.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Region {
    pub path: String,
    pub body: usize,
    pub node: Option<u32>,
}

/// What a call or a command leads to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Target {
    /// The callee or the value, as written.
    pub path: AccessPath,
    pub place: Range,
    /// The member it uses, seen from the type ([`Chain::used`]); `None` when
    /// not even its first name could be told.
    pub member: Option<NodeKey>,
    /// Every member told, and why the rest could not be.
    pub chain: Chain,
}

impl Target {
    /// Why the target is not known, when it is not.
    pub fn unresolved(&self) -> Option<&Unresolved> {
        match self.member {
            Some(_) => None,
            None => self.chain.stopped.as_ref(),
        }
    }
}

/// The label of a menu item (R18).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Label {
    /// A string literal passed to the call the label is assigned from
    /// (`GetResourceString("EditorView_checkErrors")`): a resource key.
    /// `quote` is for the adjacent comment that spells the text, which U7
    /// supplies.
    ResourceKey {
        key: String,
        callee: AccessPath,
        place: Range,
        quote: Option<String>,
    },
    /// A string literal assigned as it is.
    Literal {
        value: String,
        place: Range,
    },
    /// A path assigned as it is (`viewModel.ShowHeaderText`).
    Expression {
        path: AccessPath,
    },
    /// A call given no string literal.
    Call {
        callee: AccessPath,
    },
    Other,
}

/// The submenu a menu item is added to (`hotKeys.Items.Add(f4)`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SubMenu {
    pub variable: String,
    pub label: Option<Label>,
}

/// A menu item, as its body builds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MenuEntry {
    pub variable: String,
    /// The type constructed, as written.
    pub type_name: Option<String>,
    pub label: Option<Label>,
    pub submenu: Option<SubMenu>,
    /// The path written to its `Command`.
    pub command: Option<AccessPath>,
}

/// One `+=` of a handler.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SubscriptionSite {
    pub source: Option<AccessPath>,
    pub event: String,
    pub place: Place,
}

/// One argument of a command construction bound to a delegate field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoundDelegate {
    pub position: u32,
    pub field: NodeKey,
    /// The methods of the command class that call the field (`Execute`,
    /// `CanExecute`).
    pub invoked_by: Vec<NodeKey>,
    /// The argument as written: a lambda, a method group, anything else.
    pub value: ValueRef,
    /// What the lambda calls, or the method group.
    pub targets: Vec<Target>,
}

/// A construction of a command class (`new RelayCommand(p => Check(), …)`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommandSite {
    pub class: NodeKey,
    pub constructor: NodeKey,
    pub place: Place,
    /// The body the construction is in: a delegate's [`ValueRef::Lambda`]
    /// is an index into that body's `lambdas`.
    pub region: Region,
    pub delegates: Vec<BoundDelegate>,
    /// Fields written from more than one argument: admitted, not bound.
    pub ambiguous: Vec<NodeKey>,
}

/// A construction that would be a command but whose binding cannot be told:
/// no single constructor takes that many arguments, or every argument ends in
/// a field written from more than one of them. Admitted, never guessed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnboundCommand {
    /// The property it flows out of, or the member that builds the menu item
    /// it is the command of.
    pub member: NodeKey,
    pub class: NodeKey,
    pub place: Place,
    pub reason: Unresolved,
}

/// One entry point of a type (see the module docs).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EntryPoint {
    /// The key (KTD11): the member's first-layer key, and for a branch, a
    /// menu item or a subscribed lambda `#<discriminator>`.
    pub key: String,
    pub kind: EntryKind,
    /// The member it is written in, or is.
    pub member: NodeKey,
    pub discriminator: Option<String>,
    pub regions: Vec<Region>,
    /// The arm, the menu item's construction, the subscription; `None` for
    /// a member.
    pub place: Option<Place>,
    /// A key branch's condition.
    pub condition: Option<Condition>,
    /// A key branch's modifiers (`ModifierKeys.Shift`).
    pub modifiers: Vec<String>,
    pub menu: Option<MenuEntry>,
    /// Where a handler is subscribed; for a key branch, where its
    /// dispatcher is.
    pub subscriptions: Vec<SubscriptionSite>,
    pub commands: Vec<CommandSite>,
    /// What it calls and leads to, in source order.
    pub targets: Vec<Target>,
}

/// The result of [`bind_construction`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Binding {
    pub class: NodeKey,
    pub constructor: NodeKey,
    /// By argument position, then field.
    pub slots: Vec<Slot>,
    /// Fields written from more than one argument position.
    pub ambiguous: Vec<NodeKey>,
}

/// One argument position that ends in a field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Slot {
    pub position: u32,
    pub field: NodeKey,
    /// The methods that call the field.
    pub invoked_by: Vec<NodeKey>,
}

/// The constructor of `class` that takes `arity` arguments.
fn constructor(ix: &Index<'_>, class: &NodeKey, arity: u32) -> Result<NodeKey, Unresolved> {
    let ctors: Vec<&NodeKey> = ix
        .members_of(class)
        .iter()
        .filter(|k| ix.symbol(k).is_some_and(|s| s.kind == "constructor"))
        .filter(|k| ix.arity_of(k) == Some(arity))
        .collect();
    match ctors.as_slice() {
        [] => Err(Unresolved::NoConstructor(arity)),
        [one] => Ok((*one).clone()),
        _ => Err(Unresolved::AmbiguousConstructor(arity)),
    }
}

fn is_field_path(p: &AccessPath) -> Option<&str> {
    match (p.root, p.segments.as_slice()) {
        (PathRoot::This | PathRoot::Name, [name]) => Some(name),
        _ => None,
    }
}

/// The value member `name` of `class` or of one of its bases.
fn field_named(ix: &Index<'_>, class: &NodeKey, name: &str) -> Option<NodeKey> {
    std::iter::once(class)
        .chain(ix.ancestors(class))
        .find_map(|ty| {
            ix.members_of(ty)
                .iter()
                .find(|k| ix.symbol(k).is_some_and(|s| s.is_value() && s.name == name))
        })
        .cloned()
}

/// Follow `mapping` (argument position of the construction → parameter of
/// `ctor`) through `ctor`'s body into the fields it writes.
fn follow(
    ix: &Index<'_>,
    class: &NodeKey,
    ctor: &NodeKey,
    mapping: &[(u32, u32)],
    depth: usize,
    writes: &mut BTreeMap<NodeKey, BTreeSet<u32>>,
) {
    if depth > MAX_INITIALIZERS || mapping.is_empty() {
        return;
    }
    for pb in ix.layer.bodies_of(ctor) {
        if pb.body.kind != BodyKind::Constructor {
            continue;
        }
        for a in &pb.body.accesses {
            let (AccessMode::Write, Some(ValueRef::Param(k)), Some(name)) =
                (a.mode, &a.rhs, is_field_path(&a.target))
            else {
                continue;
            };
            let Some(field) = field_named(ix, class, name) else {
                continue;
            };
            for (outer, _) in mapping.iter().filter(|(_, inner)| inner == k) {
                writes.entry(field.clone()).or_default().insert(*outer);
            }
        }
        for inv in &pb.body.invocations {
            let next_class = match inv.kind {
                CallKind::ThisInit => class.clone(),
                CallKind::BaseInit => match ix.base_of(class) {
                    Ok(Some(base)) => base.clone(),
                    _ => continue,
                },
                _ => continue,
            };
            let Ok(next) = constructor(ix, &next_class, inv.arity) else {
                continue;
            };
            let next_mapping: Vec<(u32, u32)> = inv
                .args
                .iter()
                .filter_map(|arg| match arg.value {
                    ValueRef::Param(k) => Some((k, arg.position)),
                    _ => None,
                })
                .flat_map(|(k, position)| {
                    mapping
                        .iter()
                        .filter(move |(_, inner)| *inner == k)
                        .map(move |(outer, _)| (*outer, position))
                })
                .collect();
            follow(ix, &next_class, &next, &next_mapping, depth + 1, writes);
        }
    }
}

/// The methods of `class` and of its bases that call `field`
/// (`_execute(p)`, `_execute.Invoke(p)`, `_execute?.Invoke(p)`).
fn invokers(ix: &Index<'_>, class: &NodeKey, field: &NodeKey) -> Vec<NodeKey> {
    let name = ix.name_of(field).to_string();
    let mut out: BTreeSet<NodeKey> = BTreeSet::new();
    for ty in std::iter::once(class).chain(ix.ancestors(class)) {
        for m in ix.members_of(ty) {
            if !ix.symbol(m).is_some_and(|s| s.is_method()) {
                continue;
            }
            let calls = ix.layer.bodies_of(m).into_iter().any(|pb| {
                pb.body.accesses.iter().any(|a| {
                    a.mode == AccessMode::Invoke && is_field_path(&a.target) == Some(name.as_str())
                })
            });
            if calls {
                out.insert(m.clone());
            }
        }
    }
    out.into_iter().collect()
}

/// The command binding of `new class(…)` with `arity` arguments (see the
/// module docs): which argument position ends in which field, and which
/// methods call that field. A constructor no single one of which takes
/// `arity` arguments is admitted; a field written from two argument
/// positions is listed in [`Binding::ambiguous`] and bound to neither.
pub fn bind_construction(
    ix: &Index<'_>,
    class: &NodeKey,
    arity: u32,
) -> Result<Binding, Unresolved> {
    let ctor = constructor(ix, class, arity)?;
    let mapping: Vec<(u32, u32)> = (0..arity).map(|p| (p, p)).collect();
    let mut writes: BTreeMap<NodeKey, BTreeSet<u32>> = BTreeMap::new();
    follow(ix, class, &ctor, &mapping, 0, &mut writes);
    let mut slots = Vec::new();
    let mut ambiguous = Vec::new();
    for (field, positions) in writes {
        match positions.into_iter().collect::<Vec<_>>().as_slice() {
            [position] => slots.push(Slot {
                position: *position,
                invoked_by: invokers(ix, class, &field),
                field,
            }),
            _ => ambiguous.push(field),
        }
    }
    slots.sort_by(|a, b| (a.position, &a.field).cmp(&(b.position, &b.field)));
    Ok(Binding {
        class: class.clone(),
        constructor: ctor,
        slots,
        ambiguous,
    })
}

/// A body of the model with where it is.
#[derive(Clone, Copy)]
pub(crate) struct At<'l> {
    pub path: &'l str,
    pub index: usize,
    pub body: &'l MemberBody,
}

/// Every body of `member`, with its index in its file.
pub(crate) fn bodies<'l>(ix: &Index<'l>, member: &NodeKey) -> Vec<At<'l>> {
    ix.layer
        .bodies_of(member)
        .into_iter()
        .filter_map(|pb| {
            let file = ix.layer.file(pb.path)?;
            let index = file
                .model
                .members
                .iter()
                .position(|b| std::ptr::eq(b, pb.body))?;
            Some(At {
                path: pb.path,
                index,
                body: pb.body,
            })
        })
        .collect()
}

fn target(
    ix: &Index<'_>,
    ty: &NodeKey,
    scope: &Scope<'_>,
    path: &AccessPath,
    place: Range,
    call: Option<u32>,
) -> Target {
    let chain = ix.resolve(scope, path, call);
    Target {
        path: path.clone(),
        place,
        member: chain.used(ix, ty).cloned(),
        chain,
    }
}

/// What the code of `at` under `include` calls (as targets) and uses (calls,
/// and reads and writes of `ty`'s own members).
fn region_uses(
    ix: &Index<'_>,
    ty: &NodeKey,
    at: At<'_>,
    include: &dyn Fn(Option<u32>) -> bool,
    uses: &mut BTreeSet<NodeKey>,
    targets: &mut Vec<Target>,
) {
    for inv in &at.body.invocations {
        if inv.kind != CallKind::Call || !include(inv.node) {
            continue;
        }
        let scope = Scope {
            ty,
            body: Some(at.body),
            node: inv.node,
        };
        let t = target(ix, ty, &scope, &inv.callee, inv.place, Some(inv.arity));
        if let Some(m) = &t.member {
            uses.insert(m.clone());
        }
        targets.push(t);
    }
    for a in &at.body.accesses {
        if a.mode == AccessMode::Invoke || !include(a.node) {
            continue;
        }
        let scope = Scope {
            ty,
            body: Some(at.body),
            node: a.node,
        };
        let chain = ix.resolve(&scope, &a.target, None);
        if let Some(m) = chain.used(ix, ty) {
            if ix.symbol(m).and_then(|s| s.owner.as_ref()) == Some(ty) {
                uses.insert(m.clone());
            }
        }
    }
}

/// What a delegate argument leads to: the calls under a lambda's body, or
/// the method group.
fn delegate_targets(ix: &Index<'_>, ty: &NodeKey, at: At<'_>, arg: &Argument) -> Vec<Target> {
    match &arg.value {
        ValueRef::Lambda(id) => {
            let Some(lambda) = at.body.lambdas.get(*id as usize) else {
                return Vec::new();
            };
            let mut uses = BTreeSet::new();
            let mut targets = Vec::new();
            let root = lambda.body;
            region_uses(
                ix,
                ty,
                at,
                &|n| n.is_some_and(|n| within(at.body, n, root)),
                &mut uses,
                &mut targets,
            );
            targets
        }
        ValueRef::Access(path) => {
            // The model gives an argument no place of its own; the read of
            // the method group has one.
            let read = at.body.accesses.iter().find(|a| a.target == *path);
            let scope = Scope {
                ty,
                body: Some(at.body),
                node: read.and_then(|a| a.node),
            };
            let place = read.map(|a| a.place).unwrap_or_else(|| {
                at.body
                    .nodes
                    .first()
                    .map_or(Range::from([0, 0, 0, 0]), |n| n.place)
            });
            vec![target(ix, ty, &scope, path, place, None)]
        }
        _ => Vec::new(),
    }
}

/// The command construction `inv` in `at`, when it is one: its class
/// resolved from the callee (or, for a target-typed `new()`, from `declared`),
/// bound by [`bind_construction`], with at least one argument that ends in a
/// field some method calls. A construction of a class of the repository
/// whose binding is ambiguous is `Err`: the class and why.
fn command_site(
    ix: &Index<'_>,
    ty: &NodeKey,
    at: At<'_>,
    inv: &Invocation,
    declared: Option<&NodeKey>,
) -> Option<Result<CommandSite, (NodeKey, Unresolved)>> {
    if inv.kind != CallKind::Construct {
        return None;
    }
    let class = if inv.callee.root == PathRoot::Expression && inv.callee.segments.is_empty() {
        let (name, _) = ix.declared_type(declared?).ok()?;
        ix.type_named(&name).ok()?
    } else {
        ix.type_named(&inv.callee.segments.join(".")).ok()?
    };
    let binding = match bind_construction(ix, &class, inv.arity) {
        Ok(binding) => binding,
        Err(why @ Unresolved::AmbiguousConstructor(_)) => return Some(Err((class, why))),
        Err(_) => return None,
    };
    if binding.slots.iter().all(|s| s.invoked_by.is_empty()) {
        if binding.ambiguous.is_empty() {
            return None;
        }
        return Some(Err((class, Unresolved::AmbiguousFields(binding.ambiguous))));
    }
    let delegates = binding
        .slots
        .iter()
        .filter_map(|slot| {
            let arg = inv.args.iter().find(|a| a.position == slot.position)?;
            Some(BoundDelegate {
                position: slot.position,
                field: slot.field.clone(),
                invoked_by: slot.invoked_by.clone(),
                value: arg.value.clone(),
                targets: delegate_targets(ix, ty, at, arg),
            })
        })
        .collect();
    Some(Ok(CommandSite {
        class,
        constructor: binding.constructor,
        place: Place {
            path: at.path.to_string(),
            range: inv.place,
        },
        region: Region {
            path: at.path.to_string(),
            body: at.index,
            node: inv.node,
        },
        delegates,
        ambiguous: binding.ambiguous,
    }))
}

/// An entry point with what the clustering needs of it.
#[derive(Debug, Clone)]
pub(crate) struct Seed {
    pub entry: EntryPoint,
    /// The members it reaches directly: the calls and the commands of its
    /// region, the own members its region reads and writes.
    pub uses: BTreeSet<NodeKey>,
    /// The member it is, for a command property, a handler, a public method.
    pub identity: Option<NodeKey>,
}

/// The entry points of a type and the uses of its members.
pub(crate) struct Seeds {
    /// By key.
    pub seeds: Vec<Seed>,
    /// What each own member uses outside the regions of the entry points
    /// written in it.
    pub member_uses: BTreeMap<NodeKey, BTreeSet<NodeKey>>,
    /// What each own member uses anywhere in its bodies.
    pub full_uses: BTreeMap<NodeKey, BTreeSet<NodeKey>>,
    /// Commands whose binding cannot be told, by place.
    pub unbound: Vec<UnboundCommand>,
}

fn local_root(p: &AccessPath) -> Option<&str> {
    match p.root {
        PathRoot::Local => p.segments.first().map(String::as_str),
        _ => None,
    }
}

fn is_local(p: &AccessPath, segments: &[&str]) -> bool {
    p.root == PathRoot::Local
        && p.segments.len() == segments.len()
        && p.segments.iter().zip(segments).all(|(a, b)| a == b)
}

/// The statements of `body` that touch the local `v`.
fn statements_of(body: &MemberBody, v: &str) -> BTreeSet<u32> {
    let mut out = BTreeSet::new();
    let touches = |p: &AccessPath| local_root(p) == Some(v);
    for a in &body.accesses {
        if touches(&a.target) {
            out.extend(a.node);
        }
    }
    for inv in &body.invocations {
        let flows = match &inv.flow {
            Some(FlowTarget::Local(l)) => l == v,
            Some(FlowTarget::Path(p)) => touches(p),
            _ => false,
        };
        let passed = inv.args.iter().any(|arg| {
            matches!(&arg.value, ValueRef::Access(p) if touches(p)) || arg.reads.iter().any(touches)
        });
        if touches(&inv.callee) || flows || passed {
            out.extend(inv.node);
        }
    }
    for s in &body.subscriptions {
        if s.source.as_ref().is_some_and(touches) {
            out.extend(s.node);
        }
    }
    out
}

/// The label assigned to the local `v`'s `Header`.
fn label_of(body: &MemberBody, v: &str) -> Option<Label> {
    let write = body
        .accesses
        .iter()
        .find(|a| a.mode == AccessMode::Write && is_local(&a.target, &[v, "Header"]))?;
    Some(match &write.rhs {
        Some(ValueRef::Access(path)) => Label::Expression { path: path.clone() },
        Some(ValueRef::Literal(id)) => match body.literals.get(*id as usize) {
            Some(l) => Label::Literal {
                value: l.value.clone(),
                place: l.place,
            },
            None => Label::Other,
        },
        _ => {
            let call = body.invocations.iter().find(
                |i| matches!(&i.flow, Some(FlowTarget::Path(p)) if is_local(p, &[v, "Header"])),
            );
            match call {
                Some(call) => {
                    let literal = call.args.iter().find_map(|arg| match arg.value {
                        ValueRef::Literal(id) => body.literals.get(id as usize),
                        _ => None,
                    });
                    match literal {
                        Some(l) => Label::ResourceKey {
                            key: l.value.clone(),
                            callee: call.callee.clone(),
                            place: l.place,
                            quote: None,
                        },
                        None => Label::Call {
                            callee: call.callee.clone(),
                        },
                    }
                }
                None => Label::Other,
            }
        }
    })
}

/// The local `v` is added to: `p.Items.Add(v)`.
fn parent_of(body: &MemberBody, v: &str) -> Option<String> {
    body.invocations.iter().find_map(|inv| {
        let [p, items, add] = inv.callee.segments.as_slice() else {
            return None;
        };
        let added = inv
            .args
            .first()
            .is_some_and(|a| matches!(&a.value, ValueRef::Access(x) if is_local(x, &[v])));
        (inv.callee.root == PathRoot::Local && items == "Items" && add == "Add" && added)
            .then(|| p.clone())
    })
}

/// A seed found in pass one, its key not yet told apart.
struct Found {
    base_key: String,
    seed: Seed,
}

/// Every entry point of `ty` and the uses of its members.
pub(crate) fn seeds(ix: &Index<'_>, ty: &NodeKey) -> Seeds {
    let members: Vec<&NodeKey> = ix
        .members_of(ty)
        .iter()
        .filter(|k| ix.symbol(k).is_some_and(|s| !s.is_type()))
        .collect();
    // Regions of the entry points inside a body, by (path, body index): what
    // the member they are written in does not reach itself.
    let mut excluded: BTreeMap<(String, usize), BTreeSet<u32>> = BTreeMap::new();
    let mut found: Vec<Found> = Vec::new();
    let mut dispatchers: BTreeSet<NodeKey> = BTreeSet::new();
    // Handlers by member, with their subscriptions.
    let mut handlers: BTreeMap<NodeKey, Vec<SubscriptionSite>> = BTreeMap::new();
    let mut commands: BTreeMap<NodeKey, Vec<CommandSite>> = BTreeMap::new();
    let mut unbound: Vec<UnboundCommand> = Vec::new();

    for &m in &members {
        let symbol = ix.symbol(m).expect("filtered above");
        for at in bodies(ix, m) {
            let body = at.body;
            let place = |range: Range| Place {
                path: at.path.to_string(),
                range,
            };
            let region = |node: Option<u32>| Region {
                path: at.path.to_string(),
                body: at.index,
                node,
            };
            let roots = excluded.entry((at.path.to_string(), at.index)).or_default();

            // Key branches.
            for node in &body.nodes {
                for tag in node.tags.iter().filter(|t| t.name == "key-compare") {
                    let Some(value) = &tag.value else { continue };
                    let disc = std::iter::once(value.as_str())
                        .chain(tag.args.iter().map(String::as_str))
                        .collect::<Vec<_>>()
                        .join("+");
                    let mut uses = BTreeSet::new();
                    let mut targets = Vec::new();
                    let arm = node.id;
                    region_uses(
                        ix,
                        ty,
                        at,
                        &|n| n.is_some_and(|n| within(body, n, arm)),
                        &mut uses,
                        &mut targets,
                    );
                    roots.insert(arm);
                    dispatchers.insert(m.clone());
                    found.push(Found {
                        base_key: format!("{}#{disc}", m.as_str()),
                        seed: Seed {
                            entry: EntryPoint {
                                key: String::new(),
                                kind: EntryKind::Key,
                                member: m.clone(),
                                discriminator: Some(disc),
                                regions: vec![region(Some(arm))],
                                place: Some(place(node.place)),
                                condition: node.condition.clone(),
                                modifiers: tag.args.clone(),
                                menu: None,
                                subscriptions: Vec::new(),
                                commands: Vec::new(),
                                targets,
                            },
                            uses,
                            identity: None,
                        },
                    });
                }
            }

            // Menu items.
            let items: Vec<(&crate::extract::bodies::BodyNode, String, Option<String>)> = body
                .nodes
                .iter()
                .flat_map(|n| {
                    n.tags
                        .iter()
                        .filter(|t| t.name == "menu-item")
                        .filter_map(move |t| Some((n, t.value.clone()?, t.args.first().cloned())))
                })
                .collect();
            let labels: BTreeMap<String, Option<Label>> = items
                .iter()
                .map(|(_, v, _)| (v.clone(), label_of(body, v)))
                .collect();
            for (node, v, type_name) in &items {
                roots.extend(statements_of(body, v));
                let mut uses = BTreeSet::new();
                let mut targets = Vec::new();
                let mut sites = Vec::new();
                let mut command = None;
                for a in &body.accesses {
                    if a.mode != AccessMode::Write || !is_local(&a.target, &[v, "Command"]) {
                        continue;
                    }
                    let scope = Scope {
                        ty,
                        body: Some(body),
                        node: a.node,
                    };
                    match &a.rhs {
                        Some(ValueRef::Access(path)) => {
                            command = Some(path.clone());
                            let t = target(ix, ty, &scope, path, a.place, None);
                            uses.extend(t.member.clone());
                            targets.push(t);
                        }
                        Some(ValueRef::Construction(id)) => {
                            let Some(inv) = body.invocations.get(*id as usize) else {
                                continue;
                            };
                            match command_site(ix, ty, at, inv, None) {
                                Some(Ok(site)) => {
                                    for d in &site.delegates {
                                        for t in &d.targets {
                                            uses.extend(t.member.clone());
                                            targets.push(t.clone());
                                        }
                                    }
                                    sites.push(site);
                                }
                                Some(Err((class, reason))) => unbound.push(UnboundCommand {
                                    member: m.clone(),
                                    class,
                                    place: place(inv.place),
                                    reason,
                                }),
                                None => {}
                            }
                        }
                        _ => {}
                    }
                }
                let mut clicks = Vec::new();
                for s in &body.subscriptions {
                    let from_item = s.source.as_ref().is_some_and(|p| is_local(p, &[v]));
                    if !from_item || s.mode != SubscriptionMode::Add {
                        continue;
                    }
                    clicks.push(SubscriptionSite {
                        source: s.source.clone(),
                        event: s.event.clone(),
                        place: place(s.place),
                    });
                    match &s.handler {
                        Handler::MethodGroup(path) => {
                            let scope = Scope {
                                ty,
                                body: Some(body),
                                node: s.node,
                            };
                            let t = target(ix, ty, &scope, path, s.place, None);
                            uses.extend(t.member.clone());
                            targets.push(t);
                        }
                        Handler::Lambda(id) => {
                            if let Some(l) = body.lambdas.get(*id as usize) {
                                let root = l.body;
                                region_uses(
                                    ix,
                                    ty,
                                    at,
                                    &|n| n.is_some_and(|n| within(body, n, root)),
                                    &mut uses,
                                    &mut targets,
                                );
                            }
                        }
                        Handler::Other => {}
                    }
                }
                if command.is_none() && sites.is_empty() && clicks.is_empty() {
                    // A submenu: no entry point of its own.
                    continue;
                }
                let submenu = parent_of(body, v).map(|p| SubMenu {
                    label: labels.get(&p).cloned().flatten(),
                    variable: p,
                });
                dispatchers.insert(m.clone());
                found.push(Found {
                    base_key: format!("{}#{v}", m.as_str()),
                    seed: Seed {
                        entry: EntryPoint {
                            key: String::new(),
                            kind: EntryKind::MenuItem,
                            member: m.clone(),
                            discriminator: Some(v.clone()),
                            regions: vec![region(Some(node.id))],
                            place: Some(place(node.place)),
                            condition: None,
                            modifiers: Vec::new(),
                            menu: Some(MenuEntry {
                                variable: v.clone(),
                                type_name: type_name.clone(),
                                label: labels.get(v).cloned().flatten(),
                                submenu,
                                command,
                            }),
                            subscriptions: clicks,
                            commands: sites,
                            targets,
                        },
                        uses,
                        identity: None,
                    },
                });
            }

            // Subscriptions (`-=` is the same subscription as its `+=`); a
            // menu item's own are its own.
            for s in &body.subscriptions {
                let of_item = s
                    .source
                    .as_ref()
                    .and_then(local_root)
                    .is_some_and(|l| labels.contains_key(l));
                if s.mode != SubscriptionMode::Add || of_item {
                    continue;
                }
                let site = SubscriptionSite {
                    source: s.source.clone(),
                    event: s.event.clone(),
                    place: place(s.place),
                };
                let scope = Scope {
                    ty,
                    body: Some(body),
                    node: s.node,
                };
                let own_handler = match &s.handler {
                    Handler::MethodGroup(path) => {
                        let t = target(ix, ty, &scope, path, s.place, None);
                        match t.member {
                            Some(h)
                                if ix.symbol(&h).is_some_and(|x| {
                                    x.is_method() && x.owner.as_ref() == Some(ty)
                                }) =>
                            {
                                Ok(h)
                            }
                            _ => Err(Some(t)),
                        }
                    }
                    Handler::Lambda(id) => match body.lambdas.get(*id as usize) {
                        Some(l) => {
                            let mut uses = BTreeSet::new();
                            let mut targets = Vec::new();
                            let root = l.body;
                            region_uses(
                                ix,
                                ty,
                                at,
                                &|n| n.is_some_and(|n| within(body, n, root)),
                                &mut uses,
                                &mut targets,
                            );
                            roots.insert(root);
                            found.push(Found {
                                base_key: format!("{}#{}", m.as_str(), s.event),
                                seed: lambda_handler(
                                    m,
                                    s.event.clone(),
                                    region(Some(root)),
                                    site.clone(),
                                    uses,
                                    targets,
                                ),
                            });
                            continue;
                        }
                        None => continue,
                    },
                    Handler::Other => continue,
                };
                match own_handler {
                    Ok(h) => handlers.entry(h).or_default().push(site),
                    Err(t) => {
                        // A handler of another object: the subscription is
                        // the entry point.
                        let mut uses = BTreeSet::new();
                        let targets: Vec<Target> = t.into_iter().collect();
                        for t in &targets {
                            uses.extend(t.member.clone());
                        }
                        found.push(Found {
                            base_key: format!("{}#{}", m.as_str(), s.event),
                            seed: lambda_handler(
                                m,
                                s.event.clone(),
                                region(s.node),
                                site,
                                uses,
                                targets,
                            ),
                        });
                    }
                }
            }

            // Command properties.
            if symbol.kind == "property"
                && matches!(
                    body.kind,
                    BodyKind::Getter | BodyKind::ExpressionProperty | BodyKind::MemberInitializer
                )
            {
                let returned = |name: &str| {
                    body.accesses.iter().any(|a| {
                        a.mode == AccessMode::Return && is_field_path(&a.target) == Some(name)
                    })
                };
                for inv in &body.invocations {
                    let flows_out = match &inv.flow {
                        Some(FlowTarget::MemberReturn) => true,
                        Some(FlowTarget::Member(name)) => *name == symbol.name || returned(name),
                        _ => false,
                    };
                    if !flows_out {
                        continue;
                    }
                    match command_site(ix, ty, at, inv, Some(m)) {
                        Some(Ok(site)) => commands.entry(m.clone()).or_default().push(site),
                        Some(Err((class, reason))) => unbound.push(UnboundCommand {
                            member: m.clone(),
                            class,
                            place: place(inv.place),
                            reason,
                        }),
                        None => {}
                    }
                }
            }
        }
    }

    // What each member uses, outside and inside the entry points' regions.
    let mut member_uses: BTreeMap<NodeKey, BTreeSet<NodeKey>> = BTreeMap::new();
    let mut full_uses: BTreeMap<NodeKey, BTreeSet<NodeKey>> = BTreeMap::new();
    let mut member_targets: BTreeMap<NodeKey, Vec<Target>> = BTreeMap::new();
    for &m in &members {
        let (own, all, targets) = (
            member_uses.entry(m.clone()).or_default(),
            full_uses.entry(m.clone()).or_default(),
            member_targets.entry(m.clone()).or_default(),
        );
        for at in bodies(ix, m) {
            let roots = excluded
                .get(&(at.path.to_string(), at.index))
                .cloned()
                .unwrap_or_default();
            let body = at.body;
            region_uses(
                ix,
                ty,
                at,
                &|n| match n {
                    Some(n) => !roots.iter().any(|&r| within(body, n, r)),
                    None => true,
                },
                own,
                targets,
            );
            region_uses(ix, ty, at, &|_| true, all, &mut Vec::new());
        }
        own.remove(m);
        all.remove(m);
    }

    // Member-level entry points: commands, handlers, public methods.
    for &m in &members {
        if dispatchers.contains(m) {
            continue;
        }
        let symbol = ix.symbol(m).expect("filtered above");
        let kind = if commands.contains_key(m) {
            EntryKind::Command
        } else if handlers.contains_key(m) {
            EntryKind::Handler
        } else if symbol.is_method() && symbol.public {
            EntryKind::PublicMethod
        } else {
            continue;
        };
        let mut uses = member_uses.get(m).cloned().unwrap_or_default();
        uses.insert(m.clone());
        found.push(Found {
            base_key: m.as_str().to_string(),
            seed: Seed {
                entry: EntryPoint {
                    key: String::new(),
                    kind,
                    member: m.clone(),
                    discriminator: None,
                    regions: bodies(ix, m)
                        .into_iter()
                        .map(|at| Region {
                            path: at.path.to_string(),
                            body: at.index,
                            node: None,
                        })
                        .collect(),
                    place: None,
                    condition: None,
                    modifiers: Vec::new(),
                    menu: None,
                    subscriptions: handlers.get(m).cloned().unwrap_or_default(),
                    commands: commands.remove(m).unwrap_or_default(),
                    targets: member_targets.get(m).cloned().unwrap_or_default(),
                },
                uses,
                identity: Some(m.clone()),
            },
        });
    }
    // A key branch names where its dispatcher is subscribed.
    for f in &mut found {
        if f.seed.entry.kind == EntryKind::Key {
            if let Some(sites) = handlers.get(&f.seed.entry.member) {
                f.seed.entry.subscriptions = sites.clone();
            }
        }
    }

    // Keys: a discriminator met twice in one member gets `~2`, `~3`, … in
    // the order the bodies are written in.
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    let mut seeds: Vec<Seed> = found
        .into_iter()
        .map(|mut f| {
            let n = seen.entry(f.base_key.clone()).or_insert(0);
            *n += 1;
            f.seed.entry.key = if *n == 1 {
                f.base_key
            } else {
                format!("{}~{n}", f.base_key)
            };
            f.seed
        })
        .collect();
    seeds.sort_by(|a, b| a.entry.key.cmp(&b.entry.key));
    for s in &mut seeds {
        s.entry
            .subscriptions
            .sort_by(|a, b| (&a.place, &a.event).cmp(&(&b.place, &b.event)));
    }
    unbound.sort_by(|a, b| (&a.place, &a.member).cmp(&(&b.place, &b.member)));
    Seeds {
        seeds,
        member_uses,
        full_uses,
        unbound,
    }
}

/// An entry point that is a subscription of a lambda, or of a handler of
/// another object.
fn lambda_handler(
    member: &NodeKey,
    event: String,
    region: Region,
    site: SubscriptionSite,
    uses: BTreeSet<NodeKey>,
    targets: Vec<Target>,
) -> Seed {
    Seed {
        entry: EntryPoint {
            key: String::new(),
            kind: EntryKind::Handler,
            member: member.clone(),
            discriminator: Some(event),
            regions: vec![region],
            place: Some(site.place.clone()),
            condition: None,
            modifiers: Vec::new(),
            menu: None,
            subscriptions: vec![site],
            commands: Vec::new(),
            targets,
        },
        uses,
        identity: None,
    }
}
