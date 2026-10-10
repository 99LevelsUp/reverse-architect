//! The type of an access by its receiver chain (R17, KTD4) — one function for
//! every access the analyses read; step 2b generalizes it, it does not grow a
//! second one.
//!
//! An access is resolved name by name from what the syntax roots it at: `this`
//! and a bare name at the enclosing type, `base` at its base type, a local at
//! the type the body declares it with (`ViewA view`, `is not ViewA view`). A
//! name is looked up among the members of the type in hand, then among those
//! of its base type — one level of inheritance and no more — and the next
//! name is looked up in the type the member is declared with. A member of a
//! generic base declared with one of the base's type parameters
//! (`ViewBase<T>.viewModel: T`) takes the type the deriving type's base list
//! gives it (`EditorView : ViewBase<EditorViewModel>`): the base-list entry is
//! paired with the base the first layer's `Extends` edge names, and the
//! parameter is substituted here.
//!
//! What cannot be told is admitted, never guessed ([`Unresolved`]): an access
//! rooted at an expression (`Lookup().Command`), a name the type in hand does
//! not declare (KTD4: `CheckCommand` is never looked up across the repository
//! by its name alone), a member two levels of inheritance up, a nested generic
//! argument, a base list read on no part of the type, a type the repository
//! does not declare or declares twice.
//!
//! Limits: a written type name is matched by its last segment against the
//! types the first layer declares, without `using`s or namespaces, so a name
//! two types share stays unresolved; an overload is chosen by the number of
//! arguments only, optional parameters not counted.

use crate::extract::bodies::{
    AccessPath, BodyKind, BodyLayer, DeclarationKind, MemberBody, PairedBy, PathRoot,
};
use crate::graph::{EdgeKind, FactKind, FactValue, Graph, NodeKey, NodeKind};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// The symbol kinds of a type.
const TYPE_KINDS: &[&str] = &["class", "struct", "record", "interface", "enum"];

/// How many levels of `Extends` the ancestor walk follows before it gives up
/// — a guard against a cycle the first layer would never produce.
const MAX_DEPTH: usize = 16;

/// Why an access, a type or a binding could not be told (KTD4: admitted, never
/// guessed).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Unresolved {
    /// The access starts at an expression the syntax cannot name
    /// (`Lookup().CheckCommand`).
    NoReceiverChain,
    /// The type in hand and its base declare no member of this name.
    NotAMember(String),
    /// The name is declared only further up than one level of inheritance.
    BeyondOneLevel(String),
    /// Several members of this name, and the arguments do not tell them apart.
    AmbiguousMember(String),
    /// A local the body declares without a type (`var`, a lambda parameter).
    UntypedLocal(String),
    /// A local declared more than once with different types.
    AmbiguousLocal(String),
    /// The member states no type, or its parts state different ones.
    UntypedMember(NodeKey),
    /// The repository declares no type of this name.
    ExternalType(String),
    /// The repository declares several types of this name.
    AmbiguousType(String),
    /// A type parameter no base list binds (`T` read inside `ViewBase<T>`).
    UnboundTypeParameter(String),
    /// A generic argument that is itself generic (`ViewBase<List<M>>`), or a
    /// member declared with a generic type over a type parameter
    /// (`List<T> items`).
    NestedGeneric(String),
    /// The type has more than one base type in the repository.
    AmbiguousBase(NodeKey),
    /// No part of the type read gives the base list entry of its base, or the
    /// parts give different ones.
    BaseListNotRead(NodeKey),
    /// The base list gives a different number of type arguments than the base
    /// declares type parameters.
    TypeArgumentsMismatch(NodeKey),
    /// No constructor of the class takes this many arguments.
    NoConstructor(u32),
    /// More than one constructor takes this many arguments.
    AmbiguousConstructor(u32),
    /// Every argument of a construction that ends in a field ends in one
    /// written from more than one argument.
    AmbiguousFields(Vec<NodeKey>),
    /// The value is not a lambda or a method group.
    NotADelegate,
}

/// One first-layer symbol, as the analysis reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    /// `SymbolKind`: `method`, `constructor`, `property`, `field`, `event`,
    /// `class`…
    pub kind: String,
    /// The type it is a member of (`HasMember`).
    pub owner: Option<NodeKey>,
    /// It states `public`.
    pub public: bool,
}

impl Symbol {
    pub fn is_type(&self) -> bool {
        TYPE_KINDS.contains(&self.kind.as_str())
    }

    pub fn is_method(&self) -> bool {
        self.kind == "method"
    }

    /// A field, a property or an event: a value a path goes on through.
    pub fn is_value(&self) -> bool {
        matches!(self.kind.as_str(), "field" | "property" | "event")
    }
}

/// The first layer, indexed for the analyses: symbols by key, members by
/// type, types by name, bases by type; and the second layer beside it.
pub struct Index<'a> {
    pub layer: &'a BodyLayer,
    symbols: BTreeMap<NodeKey, Symbol>,
    members: BTreeMap<NodeKey, Vec<NodeKey>>,
    types_named: BTreeMap<String, Vec<NodeKey>>,
    bases: BTreeMap<NodeKey, Vec<NodeKey>>,
}

fn text_fact(facts: &[crate::graph::Fact], kind: FactKind) -> Option<String> {
    facts
        .iter()
        .find(|f| f.kind == kind)
        .and_then(|f| match &f.value {
            FactValue::Text(t) => Some(t.clone()),
            _ => None,
        })
}

impl<'a> Index<'a> {
    pub fn new(graph: &Graph, layer: &'a BodyLayer) -> Index<'a> {
        let mut symbols = BTreeMap::new();
        for node in graph.nodes_sorted() {
            if node.kind != NodeKind::Symbol {
                continue;
            }
            let (Some(name), Some(kind)) = (
                text_fact(&node.facts, FactKind::SymbolName),
                text_fact(&node.facts, FactKind::SymbolKind),
            ) else {
                continue;
            };
            let public =
                text_fact(&node.facts, FactKind::SymbolVisibility).as_deref() == Some("public");
            symbols.insert(
                node.key.clone(),
                Symbol {
                    name,
                    kind,
                    owner: None,
                    public,
                },
            );
        }
        let mut members: BTreeMap<NodeKey, Vec<NodeKey>> = BTreeMap::new();
        let mut bases: BTreeMap<NodeKey, Vec<NodeKey>> = BTreeMap::new();
        for e in graph.edges_sorted() {
            match e.edge.kind {
                EdgeKind::HasMember => {
                    members
                        .entry(e.from.clone())
                        .or_default()
                        .push(e.to.clone());
                    if let Some(s) = symbols.get_mut(e.to) {
                        s.owner.get_or_insert_with(|| e.from.clone());
                    }
                }
                EdgeKind::Extends => bases.entry(e.from.clone()).or_default().push(e.to.clone()),
                _ => {}
            }
        }
        for list in members.values_mut().chain(bases.values_mut()) {
            list.sort();
            list.dedup();
        }
        let mut types_named: BTreeMap<String, Vec<NodeKey>> = BTreeMap::new();
        for (key, s) in &symbols {
            if s.is_type() && is_csharp(layer, key) {
                types_named
                    .entry(s.name.clone())
                    .or_default()
                    .push(key.clone());
            }
        }
        Index {
            layer,
            symbols,
            members,
            types_named,
            bases,
        }
    }

    pub fn symbol(&self, key: &NodeKey) -> Option<&Symbol> {
        self.symbols.get(key)
    }

    /// The name a symbol is declared with, or the key when it has none.
    pub fn name_of<'k>(&'k self, key: &'k NodeKey) -> &'k str {
        self.symbol(key).map_or(key.as_str(), |s| s.name.as_str())
    }

    /// The members of a type, sorted by key.
    pub fn members_of(&self, ty: &NodeKey) -> &[NodeKey] {
        self.members.get(ty).map_or(&[], Vec::as_slice)
    }

    /// Every C# type the analysis reads: one whose parts were read by the
    /// second layer, sorted by key.
    pub fn csharp_types(&self) -> Vec<&NodeKey> {
        self.types_named
            .values()
            .flatten()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    /// The base type of `ty` in the repository: `None` when it has none
    /// there.
    pub fn base_of(&self, ty: &NodeKey) -> Result<Option<&NodeKey>, Unresolved> {
        match self.bases.get(ty).map(Vec::as_slice) {
            None | Some([]) => Ok(None),
            Some([one]) => Ok(Some(one)),
            Some(_) => Err(Unresolved::AmbiguousBase(ty.clone())),
        }
    }

    /// Every type `ty` derives from, nearest first, as far as each has one
    /// base.
    pub fn ancestors(&self, ty: &NodeKey) -> Vec<&NodeKey> {
        let mut out: Vec<&NodeKey> = Vec::new();
        let mut at = ty;
        while let Ok(Some(base)) = self.base_of(at) {
            if out.len() >= MAX_DEPTH || base == ty || out.contains(&base) {
                break;
            }
            out.push(base);
            at = base;
        }
        out
    }

    /// The type a written name names: the one type of the repository with
    /// that last segment.
    pub fn type_named(&self, written: &str) -> Result<NodeKey, Unresolved> {
        let last = written.rsplit('.').next().unwrap_or(written);
        match self.types_named.get(last).map(Vec::as_slice) {
            None | Some([]) => Err(Unresolved::ExternalType(written.to_string())),
            Some([one]) => Ok(one.clone()),
            Some(_) => Err(Unresolved::AmbiguousType(written.to_string())),
        }
    }

    /// The number of parameters a method or constructor declares: the
    /// parameters its own body is typed with (`typed-name` of kind
    /// `parameter` outside every lambda). `None` without such a body.
    pub fn arity_of(&self, member: &NodeKey) -> Option<u32> {
        self.layer
            .bodies_of(member)
            .into_iter()
            .find(|b| {
                b.by == PairedBy::Name
                    && matches!(b.body.kind, BodyKind::Method | BodyKind::Constructor)
            })
            .map(|b| {
                b.body
                    .typed_names
                    .iter()
                    .filter(|t| t.kind == DeclarationKind::Parameter && t.scope.is_none())
                    .count() as u32
            })
    }

    /// The type parameters of `ty`, the same on every part.
    pub fn type_params(&self, ty: &NodeKey) -> Result<Vec<String>, Unresolved> {
        let lists: BTreeSet<&Vec<String>> = self
            .layer
            .type_shapes_of(ty)
            .into_iter()
            .map(|(_, s)| &s.type_params)
            .filter(|p| !p.is_empty())
            .collect();
        match lists.len() {
            0 => Ok(Vec::new()),
            1 => Ok(lists.into_iter().next().cloned().unwrap_or_default()),
            _ => Err(Unresolved::BaseListNotRead(ty.clone())),
        }
    }

    /// The type arguments the base list of `derived` gives `base`: the entry
    /// whose last segment is the base's name, the same on every part that
    /// writes one.
    pub fn base_arguments(
        &self,
        derived: &NodeKey,
        base: &NodeKey,
    ) -> Result<Vec<String>, Unresolved> {
        let name = self.name_of(base);
        let entries: BTreeSet<&Vec<String>> = self
            .layer
            .type_shapes_of(derived)
            .into_iter()
            .flat_map(|(_, s)| s.bases.iter())
            .filter(|b| b.base.last().is_some_and(|l| l == name))
            .map(|b| &b.type_args)
            .collect();
        match entries.len() {
            1 => Ok(entries.into_iter().next().cloned().unwrap_or_default()),
            _ => Err(Unresolved::BaseListNotRead(derived.clone())),
        }
    }

    /// The type a field, property or event is declared with — its name and
    /// generic arguments as written, the same on every part.
    pub fn declared_type(&self, member: &NodeKey) -> Result<(String, Vec<String>), Unresolved> {
        let types: BTreeSet<(&String, &Vec<String>)> = self
            .layer
            .typed_members_of(member)
            .into_iter()
            .map(|(_, t)| (&t.type_name, &t.type_args))
            .collect();
        match types.into_iter().collect::<Vec<_>>().as_slice() {
            [(name, args)] => Ok(((*name).clone(), (*args).clone())),
            _ => Err(Unresolved::UntypedMember(member.clone())),
        }
    }

    /// The member `name` of `ty` or of its base, chosen among overloads by
    /// `call`'s argument count. `skip_own` looks in the base only (`base.x`).
    fn lookup(
        &self,
        ty: &NodeKey,
        name: &str,
        call: Option<u32>,
        skip_own: bool,
    ) -> Result<Found, Unresolved> {
        if !skip_own {
            if let Some(member) = self.pick(ty, name, call)? {
                return Ok(Found {
                    member,
                    owner: ty.clone(),
                    through: None,
                });
            }
        }
        let Some(base) = self.base_of(ty)? else {
            return Err(Unresolved::NotAMember(name.to_string()));
        };
        match self.pick(base, name, call)? {
            Some(member) => Ok(Found {
                member,
                owner: base.clone(),
                through: Some(ty.clone()),
            }),
            None if self.ancestors(base).is_empty() => {
                Err(Unresolved::NotAMember(name.to_string()))
            }
            None => Err(Unresolved::BeyondOneLevel(name.to_string())),
        }
    }

    /// The one member of `ty` named `name`: for a call, a method by its
    /// argument count, else a delegate-valued member; for any other access a
    /// value, else a single method (a method group).
    fn pick(
        &self,
        ty: &NodeKey,
        name: &str,
        call: Option<u32>,
    ) -> Result<Option<NodeKey>, Unresolved> {
        let named: Vec<(&NodeKey, &Symbol)> = self
            .members_of(ty)
            .iter()
            .filter_map(|k| self.symbol(k).map(|s| (k, s)))
            .filter(|(_, s)| s.name == name)
            .collect();
        if named.is_empty() {
            return Ok(None);
        }
        let ambiguous = || Unresolved::AmbiguousMember(name.to_string());
        let methods: Vec<&NodeKey> = named
            .iter()
            .filter(|(_, s)| s.is_method())
            .map(|(k, _)| *k)
            .collect();
        let values: Vec<&NodeKey> = named
            .iter()
            .filter(|(_, s)| s.is_value())
            .map(|(k, _)| *k)
            .collect();
        let one = |list: &[&NodeKey]| match list {
            [only] => Ok(Some((*only).clone())),
            _ => Err(ambiguous()),
        };
        match call {
            Some(arity) if !methods.is_empty() => self.overload(name, &methods, arity).map(Some),
            _ if !values.is_empty() => one(&values),
            _ if !methods.is_empty() => one(&methods),
            _ => one(&named.iter().map(|(k, _)| *k).collect::<Vec<_>>()),
        }
    }

    /// The overload of `name` a call with `arity` arguments picks among
    /// `methods`: the only one, else the one that declares `arity`
    /// parameters ([`Index::arity_of`]). None or several of that arity is
    /// [`Unresolved::AmbiguousMember`] — admitted, never guessed. The one
    /// choice of an overload the analyses make (`Check()` → `Check(bool)`);
    /// step 2b extends it to the members of other types instead of writing a
    /// second one.
    pub fn overload(
        &self,
        name: &str,
        methods: &[&NodeKey],
        arity: u32,
    ) -> Result<NodeKey, Unresolved> {
        if let [only] = methods {
            return Ok((*only).clone());
        }
        let by_arity: Vec<&NodeKey> = methods
            .iter()
            .copied()
            .filter(|m| self.arity_of(m) == Some(arity))
            .collect();
        match by_arity.as_slice() {
            [one] => Ok((*one).clone()),
            _ => Err(Unresolved::AmbiguousMember(name.to_string())),
        }
    }

    /// The type a found member's value is of, its base's type parameter
    /// substituted from the deriving type's base list (R17).
    fn type_of(&self, found: &Found) -> Result<NodeKey, Unresolved> {
        let (name, args) = self.declared_type(&found.member)?;
        let params = self.type_params(&found.owner)?;
        if args.iter().any(|a| names_any(a, &params)) {
            return Err(Unresolved::NestedGeneric(format!(
                "{name}<{}>",
                args.join(",")
            )));
        }
        let Some(index) = params.iter().position(|p| *p == name) else {
            return self.type_named(&name);
        };
        let Some(derived) = &found.through else {
            return Err(Unresolved::UnboundTypeParameter(name));
        };
        let given = self.base_arguments(derived, &found.owner)?;
        if given.len() != params.len() {
            return Err(Unresolved::TypeArgumentsMismatch(derived.clone()));
        }
        let arg = &given[index];
        if arg.contains('<') {
            return Err(Unresolved::NestedGeneric(arg.clone()));
        }
        self.type_named(arg)
    }

    /// The type a local of `scope`'s body is declared with: the declaration
    /// whose scope holds the access, else the one declaration of that name in
    /// the body (a pattern's variable is used after the `if` that declares
    /// it).
    fn local_type(&self, scope: &Scope<'_>, name: &str) -> Result<NodeKey, Unresolved> {
        let Some(body) = scope.body else {
            return Err(Unresolved::UntypedLocal(name.to_string()));
        };
        let decls: Vec<_> = body
            .typed_names
            .iter()
            .filter(|t| {
                t.name == name
                    && matches!(
                        t.kind,
                        DeclarationKind::Local
                            | DeclarationKind::Parameter
                            | DeclarationKind::Pattern
                    )
            })
            .collect();
        let holding: Vec<_> = decls
            .iter()
            .filter(|t| match (t.scope, scope.node) {
                (None, _) => true,
                (Some(s), Some(n)) => within(body, n, s),
                (Some(_), None) => false,
            })
            .collect();
        let pick = if holding.len() == 1 {
            holding[0]
        } else if decls.len() == 1 {
            &decls[0]
        } else if decls.is_empty() {
            return Err(Unresolved::UntypedLocal(name.to_string()));
        } else {
            return Err(Unresolved::AmbiguousLocal(name.to_string()));
        };
        let params = self.type_params(scope.ty).unwrap_or_default();
        if pick.type_args.iter().any(|a| names_any(a, &params)) {
            return Err(Unresolved::NestedGeneric(pick.type_name.clone()));
        }
        if params.contains(&pick.type_name) {
            return Err(Unresolved::UnboundTypeParameter(pick.type_name.clone()));
        }
        self.type_named(&pick.type_name)
    }

    /// The members an access names, link by link, as far as they can be told
    /// (see the module docs). `call` is the argument count when the access is
    /// a call: the last name is then chosen among overloads by it.
    pub fn resolve(&self, scope: &Scope<'_>, path: &AccessPath, call: Option<u32>) -> Chain {
        let mut chain = Chain {
            links: Vec::new(),
            stopped: None,
        };
        let segments = &path.segments;
        let (mut ty, start, mut skip_own) = match path.root {
            PathRoot::Expression => {
                chain.stopped = Some(Unresolved::NoReceiverChain);
                return chain;
            }
            PathRoot::This | PathRoot::Name => (scope.ty.clone(), 0, false),
            PathRoot::Base => (scope.ty.clone(), 0, true),
            PathRoot::Local => {
                let Some(first) = segments.first() else {
                    chain.stopped = Some(Unresolved::NoReceiverChain);
                    return chain;
                };
                match self.local_type(scope, first) {
                    Ok(ty) => (ty, 1, false),
                    Err(why) => {
                        chain.stopped = Some(why);
                        return chain;
                    }
                }
            }
        };
        for (i, name) in segments.iter().enumerate().skip(start) {
            let last = i + 1 == segments.len();
            let found = match self.lookup(&ty, name, if last { call } else { None }, skip_own) {
                Ok(found) => found,
                Err(why) => {
                    chain.stopped = Some(why);
                    return chain;
                }
            };
            skip_own = false;
            chain.links.push(Link {
                member: found.member.clone(),
                owner: found.owner.clone(),
            });
            if !last {
                match self.type_of(&found) {
                    Ok(next) => ty = next,
                    Err(why) => {
                        chain.stopped = Some(why);
                        return chain;
                    }
                }
            }
        }
        chain
    }
}

/// Whether a written type (`List<T>`, `Dictionary<string,T>`) names one of
/// `params` as a whole word.
fn names_any(written: &str, params: &[String]) -> bool {
    written
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .any(|word| params.iter().any(|p| p == word))
}

/// Whether a type's parts are C#: a part the second layer read from a `.cs`
/// file (KTD6: the analyses read only C# bodies).
pub fn is_csharp(layer: &BodyLayer, ty: &NodeKey) -> bool {
    layer
        .type_shapes_of(ty)
        .iter()
        .any(|(path, _)| path.ends_with(".cs"))
}

/// Whether `node` is `ancestor` or below it in `body`'s tree.
pub fn within(body: &MemberBody, node: u32, ancestor: u32) -> bool {
    let mut at = Some(node);
    let mut steps = 0;
    while let Some(n) = at {
        if n == ancestor {
            return true;
        }
        steps += 1;
        if steps > body.nodes.len() {
            return false;
        }
        at = body.nodes.get(n as usize).and_then(|x| x.parent);
    }
    false
}

/// A member found by name, and where.
struct Found {
    member: NodeKey,
    /// The type that declares it.
    owner: NodeKey,
    /// The deriving type it was found through, when it was found in a base.
    through: Option<NodeKey>,
}

/// Where an access is written: the enclosing type, and the body and its node
/// when the access is in one (for its locals).
#[derive(Clone, Copy)]
pub struct Scope<'b> {
    pub ty: &'b NodeKey,
    pub body: Option<&'b MemberBody>,
    pub node: Option<u32>,
}

/// One resolved name of an access.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Link {
    pub member: NodeKey,
    /// The type that declares it.
    pub owner: NodeKey,
}

/// The members an access names, as far as they could be told.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Chain {
    pub links: Vec<Link>,
    /// Why it stopped before its last name; `None` when every name resolved.
    pub stopped: Option<Unresolved>,
}

impl Chain {
    /// The member the access uses, seen from `ty` (KTD12): its first name
    /// when that is `ty`'s own member (`menu.Items.Add` uses `menu`),
    /// otherwise the first member of a type that is neither `ty` nor one of
    /// its bases — an inherited member gone through (`viewModel`) is not what
    /// is used — and otherwise, when every name was told, the last one
    /// (`GetResourceString` of the base). `None` when the access stops before
    /// it reaches anything but inherited members.
    pub fn used(&self, ix: &Index<'_>, ty: &NodeKey) -> Option<&NodeKey> {
        let first = self.links.first()?;
        if &first.owner == ty {
            return Some(&first.member);
        }
        let ancestors = ix.ancestors(ty);
        if let Some(l) = self
            .links
            .iter()
            .find(|l| &l.owner != ty && !ancestors.contains(&&l.owner))
        {
            return Some(&l.member);
        }
        match self.stopped {
            None => self.links.last().map(|l| &l.member),
            Some(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::bodies::{BodyNode, NodeKind as BodyNodeKind, Position, Range};

    fn node(id: u32, parent: Option<u32>) -> BodyNode {
        BodyNode {
            id,
            kind: BodyNodeKind::Statement,
            place: Range::from([1, 1, 1, 1]),
            parent,
            condition: None,
            returns: None,
            tags: Vec::new(),
        }
    }

    #[test]
    fn a_node_is_within_itself_and_its_ancestors_only() {
        let body = MemberBody {
            name_position: Position { line: 1, column: 1 },
            kind: BodyKind::Method,
            root: 0,
            nodes: vec![
                node(0, None),
                node(1, Some(0)),
                node(2, Some(1)),
                node(3, Some(0)),
            ],
            accesses: Vec::new(),
            invocations: Vec::new(),
            lambdas: Vec::new(),
            subscriptions: Vec::new(),
            literals: Vec::new(),
            name_ofs: Vec::new(),
            typed_names: Vec::new(),
            tags: Vec::new(),
        };
        assert!(within(&body, 2, 2));
        assert!(within(&body, 2, 1));
        assert!(within(&body, 2, 0));
        assert!(!within(&body, 3, 1));
        assert!(!within(&body, 1, 2));
        assert!(!within(&body, 9, 0));
    }
}
