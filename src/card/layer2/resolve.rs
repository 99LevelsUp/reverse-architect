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
//! types of a profiled language the first layer declares — every one, a type whose bodies the
//! second layer could not read too — without `using`s, so a name two types
//! share stays unresolved; a qualified name (`System.Timers.Timer`) takes
//! only a type whose namespace and enclosing types end with its qualifier,
//! and is external otherwise. An overload is chosen by the number of
//! arguments only, optional parameters not counted.

use super::link::Basis;
use super::profile;
use crate::extract::bodies::{
    AccessPath, BodyKind, BodyLayer, DeclarationKind, MemberBody, PairedBy, PathRoot,
};
use crate::graph::{Confidence, EdgeKind, FactKind, FactValue, Graph, NodeKey, NodeKind, SiteId};
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
    /// Neither the type in hand nor any type it derives from in the
    /// repository declares a member of this name.
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
    /// The repository declares no type of this name — a written type, or
    /// the base a type's base list names outside the repository, which a
    /// name no type of the repository declares may be a member of.
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
    /// Every type of a profiled language of the first layer by name — a type
    /// in a file whose bodies were not read too, so that a name it shares
    /// stays ambiguous.
    types_named: BTreeMap<String, Vec<NodeKey>>,
    /// The types the second layer read a part of in their profile's
    /// language, sorted by key.
    read_types: Vec<NodeKey>,
    bases: BTreeMap<NodeKey, Vec<NodeKey>>,
    /// The members of the read types by name, each list sorted by key: the
    /// candidates an access that stopped at a name is shown with (step 2b,
    /// KTD9) — never what it resolves to.
    members_named: BTreeMap<String, Vec<NodeKey>>,
    /// The first layer's relations an external index pinned, by the member
    /// they leave: the target and the sites it was read at (step 2b, KTD3).
    indexed: BTreeMap<NodeKey, Vec<(NodeKey, Vec<SiteId>)>>,
}

/// The kinds of relation an external index pins from a member to a member.
const INDEXED_KINDS: &[EdgeKind] = &[EdgeKind::Calls, EdgeKind::References];

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
        let mut indexed: BTreeMap<NodeKey, Vec<(NodeKey, Vec<SiteId>)>> = BTreeMap::new();
        for e in graph.edges_sorted() {
            if INDEXED_KINDS.contains(&e.edge.kind)
                && e.edge.provenance.rung() == Some(Confidence::ResolvedByIndex)
            {
                indexed
                    .entry(e.from.clone())
                    .or_default()
                    .push((e.to.clone(), e.edge.sites.clone()));
            }
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
        let mut read_types: Vec<NodeKey> = Vec::new();
        for (key, s) in &symbols {
            let Some(profile) = profile::of_key(key) else {
                continue;
            };
            if s.is_type() {
                types_named
                    .entry(s.name.clone())
                    .or_default()
                    .push(key.clone());
                if profile.reads(layer, key) {
                    read_types.push(key.clone());
                }
            }
        }
        let read: BTreeSet<&NodeKey> = read_types.iter().collect();
        let mut members_named: BTreeMap<String, Vec<NodeKey>> = BTreeMap::new();
        for (key, s) in &symbols {
            if !s.is_type() && s.owner.as_ref().is_some_and(|o| read.contains(o)) {
                members_named
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
            read_types,
            bases,
            members_named,
            indexed,
        }
    }

    /// The members of the repository's C# types named `name`, by key — the
    /// candidates of an access stopped at that name, never its target.
    pub fn members_named(&self, name: &str) -> &[NodeKey] {
        self.members_named.get(name).map_or(&[], Vec::as_slice)
    }

    /// The repository's C# types named `name`, by key — the candidates a
    /// name over the ambiguity ceiling may have meant (step 2b, U1), never
    /// what a written type resolves to ([`Index::type_named`]).
    pub fn types_named(&self, name: &str) -> &[NodeKey] {
        self.types_named.get(name).map_or(&[], Vec::as_slice)
    }

    /// The members an external index pinned as the targets of `caller`'s
    /// relations, with the sites each was read at, by target.
    pub fn indexed_from(&self, caller: &NodeKey) -> &[(NodeKey, Vec<SiteId>)] {
        self.indexed.get(caller).map_or(&[], Vec::as_slice)
    }

    pub fn symbol(&self, key: &NodeKey) -> Option<&Symbol> {
        self.symbols.get(key)
    }

    /// The body at `index` of the file `path`'s model of bodies, when the
    /// second layer read the file — what a site's `body` points at, and the
    /// tree [`within`] walks.
    pub(super) fn body_at(&self, path: &str, index: usize) -> Option<&'a MemberBody> {
        self.layer.file(path)?.model.members.get(index)
    }

    /// The type that declares `member` (`HasMember`).
    pub fn owner_of(&self, member: &NodeKey) -> Option<&NodeKey> {
        self.symbol(member).and_then(|s| s.owner.as_ref())
    }

    /// Whether `member` is declared by `ty` or by one of `ancestors` —
    /// `ty`'s line of bases ([`Index::ancestors`]): a member `ty` has, its
    /// own or inherited.
    pub fn declared_along(&self, member: &NodeKey, ty: &NodeKey, ancestors: &[&NodeKey]) -> bool {
        self.owner_of(member)
            .is_some_and(|o| o == ty || ancestors.contains(&o))
    }

    /// The name a symbol is declared with, or the key when it has none.
    pub fn name_of<'k>(&'k self, key: &'k NodeKey) -> &'k str {
        self.symbol(key).map_or(key.as_str(), |s| s.name.as_str())
    }

    /// The members of a type, sorted by key.
    pub fn members_of(&self, ty: &NodeKey) -> &[NodeKey] {
        self.members.get(ty).map_or(&[], Vec::as_slice)
    }

    /// Every type the analysis reads: one of a profiled language whose parts
    /// the second layer read, sorted by key.
    pub fn read_types(&self) -> Vec<&NodeKey> {
        self.read_types.iter().collect()
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

    /// The type a written name names: the one C# type of the repository
    /// with that last segment — for a qualified name (`System.Timers.Timer`)
    /// the one whose namespace and enclosing types end with the qualifier.
    pub fn type_named(&self, written: &str) -> Result<NodeKey, Unresolved> {
        let bare = written.strip_prefix("global::").unwrap_or(written);
        let (qualifier, last) = match bare.rsplit_once('.') {
            Some((qualifier, last)) => (Some(qualifier), last),
            None => (None, bare),
        };
        let named: Vec<&NodeKey> = self
            .types_named
            .get(last)
            .into_iter()
            .flatten()
            .filter(|key| match qualifier {
                Some(qualifier) => qualified_by(key, qualifier),
                None => true,
            })
            .collect();
        match named.as_slice() {
            [] => Err(Unresolved::ExternalType(written.to_string())),
            [one] => Ok((*one).clone()),
            _ => Err(Unresolved::AmbiguousType(written.to_string())),
        }
    }

    /// The one C# type of the repository whose full name — namespace,
    /// enclosing types and name, dotted — is exactly `full` (plan 2c, KTD8:
    /// a type another language names fully qualified, `Acme.Views.ViewA`).
    /// Unlike [`Index::type_named`], whose qualifier may be any trailing part
    /// of the namespace, this is equality: a name the other language wrote
    /// fully qualified is backed only by the type of exactly that name.
    pub fn type_exactly(&self, full: &str) -> Result<NodeKey, Unresolved> {
        let last = full.rsplit_once('.').map_or(full, |(_, last)| last);
        let named: Vec<&NodeKey> = self
            .types_named
            .get(last)
            .into_iter()
            .flatten()
            .filter(|key| full_name(key).as_deref() == Some(full))
            .collect();
        match named.as_slice() {
            [] => Err(Unresolved::ExternalType(full.to_string())),
            [one] => Ok((*one).clone()),
            _ => Err(Unresolved::AmbiguousType(full.to_string())),
        }
    }

    /// Why a name no type of `ty`'s line of bases declares cannot be said
    /// to be no member: `ty`'s base is ambiguous, or the first entry of a
    /// base list of it — the base class, when it has one — names a type the
    /// repository does not declare (`MainView : Window`), whose members are
    /// not known.
    fn beyond_repository(&self, ty: &NodeKey) -> Option<Unresolved> {
        if let Err(why) = self.base_of(ty) {
            return Some(why);
        }
        self.layer
            .type_shapes_of(ty)
            .into_iter()
            .filter_map(|(_, shape)| shape.bases.first())
            .find_map(|entry| match self.type_named(&entry.base.join(".")) {
                Err(why @ (Unresolved::ExternalType(_) | Unresolved::AmbiguousType(_))) => {
                    Some(why)
                }
                _ => None,
            })
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
            if let Some((member, overloaded)) = self.pick(ty, name, call)? {
                return Ok(Found {
                    member,
                    owner: ty.clone(),
                    through: None,
                    overloaded,
                });
            }
        }
        let not_a_member = |last: &NodeKey| {
            self.beyond_repository(last)
                .unwrap_or_else(|| Unresolved::NotAMember(name.to_string()))
        };
        let Some(base) = self.base_of(ty)? else {
            return Err(not_a_member(ty));
        };
        if let Some((member, overloaded)) = self.pick(base, name, call)? {
            return Ok(Found {
                member,
                owner: base.clone(),
                through: Some(ty.clone()),
                overloaded,
            });
        }
        // Only a name a type further up declares — once or more — is
        // declared beyond one level.
        let ancestors = self.ancestors(base);
        if ancestors
            .iter()
            .any(|a| !matches!(self.pick(a, name, call), Ok(None)))
        {
            return Err(Unresolved::BeyondOneLevel(name.to_string()));
        }
        Err(not_a_member(ancestors.last().copied().unwrap_or(base)))
    }

    /// The one member of `ty` named `name`: for a call, a method by its
    /// argument count, else a delegate-valued member; for any other access a
    /// value, else a single method (a method group). The flag says whether
    /// the call's arity chose it among several methods.
    fn pick(
        &self,
        ty: &NodeKey,
        name: &str,
        call: Option<u32>,
    ) -> Result<Option<(NodeKey, bool)>, Unresolved> {
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
            [only] => Ok(Some(((*only).clone(), false))),
            _ => Err(ambiguous()),
        };
        match call {
            Some(arity) if !methods.is_empty() => self
                .overload(name, &methods, arity)
                .map(|m| Some((m, methods.len() > 1))),
            _ if !values.is_empty() => one(&values),
            _ if !methods.is_empty() => one(&methods),
            _ => one(&named.iter().map(|(k, _)| *k).collect::<Vec<_>>()),
        }
    }

    /// The overload of `name` a call with `arity` arguments picks among
    /// `methods`: the only one, else the one that declares `arity`
    /// parameters ([`Index::by_arity`]). None or several of that arity is
    /// [`Unresolved::AmbiguousMember`] — admitted, never guessed. The one
    /// choice of an overload the analyses make (`Check()` → `Check(bool)`),
    /// for the members of the type in hand and of every type an access goes
    /// on through alike.
    pub fn overload(
        &self,
        name: &str,
        methods: &[&NodeKey],
        arity: u32,
    ) -> Result<NodeKey, Unresolved> {
        if let [only] = methods {
            return Ok((*only).clone());
        }
        match self.by_arity(methods, arity) {
            ByArity::One(one) => Ok(one),
            ByArity::NoneOf | ByArity::Several(_) => {
                Err(Unresolved::AmbiguousMember(name.to_string()))
            }
        }
    }

    /// The members among `candidates` that declare `arity` parameters
    /// ([`Index::arity_of`]) — the one selection by arity under
    /// [`Index::overload`] and the choice of a constructor
    /// (`entry::constructor`). Strict: a single candidate of another arity
    /// is [`ByArity::NoneOf`]; `overload` alone takes a lone method whatever
    /// its arity (optional parameters are not counted).
    pub fn by_arity(&self, candidates: &[&NodeKey], arity: u32) -> ByArity {
        let mut of: Vec<NodeKey> = candidates
            .iter()
            .filter(|m| self.arity_of(m) == Some(arity))
            .map(|m| (*m).clone())
            .collect();
        match of.len() {
            0 => ByArity::NoneOf,
            1 => ByArity::One(of.remove(0)),
            _ => {
                of.sort();
                ByArity::Several(of)
            }
        }
    }

    /// The type a found member's value is of, its base's type parameter
    /// substituted from the deriving type's base list (R17); the flag says
    /// whether it was substituted.
    fn type_of(&self, found: &Found) -> Result<(NodeKey, bool), Unresolved> {
        let (name, args) = self.declared_type(&found.member)?;
        let params = self.type_params(&found.owner)?;
        if args.iter().any(|a| names_any(a, &params)) {
            return Err(Unresolved::NestedGeneric(format!(
                "{name}<{}>",
                args.join(",")
            )));
        }
        let Some(index) = params.iter().position(|p| *p == name) else {
            return self.type_named(&name).map(|t| (t, false));
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
        self.type_named(arg).map(|t| (t, true))
    }

    /// The type a local of `scope`'s body is declared with: the declaration
    /// whose scope holds the access, else the one declaration of that name in
    /// the body (a pattern's variable is used after the `if` that declares
    /// it). With the kind of the declaration.
    fn local_type(
        &self,
        scope: &Scope<'_>,
        name: &str,
    ) -> Result<(NodeKey, DeclarationKind), Unresolved> {
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
        self.type_named(&pick.type_name).map(|t| (t, pick.kind))
    }

    /// The members an access names, link by link, as far as they can be told
    /// (see the module docs). `call` is the argument count when the access is
    /// a call: the last name is then chosen among overloads by it.
    pub fn resolve(&self, scope: &Scope<'_>, path: &AccessPath, call: Option<u32>) -> Chain {
        self.walk(scope, path, call, &|_, _, _| None).chain
    }

    /// [`Index::resolve`] with what step 2b grades its links by: the rule
    /// each link was told by and, when the access stopped, the name it
    /// stopped at and the type that name was looked up in. `pin` may name the
    /// member a name is (the type in hand when it is known, the name, the
    /// call's argument count for the last name): an external index's target,
    /// taken before the rules and even where they could not tell the type in
    /// hand (`link`, KTD3). Without a pin the walk is the rules' alone. The
    /// one walk every analysis resolves an access by.
    pub(crate) fn walk(
        &self,
        scope: &Scope<'_>,
        path: &AccessPath,
        call: Option<u32>,
        pin: Pin<'_>,
    ) -> Walk {
        let mut walk = Walk {
            chain: Chain {
                links: Vec::new(),
                stopped: None,
            },
            bases: Vec::new(),
            stop: None,
        };
        let segments = &path.segments;
        // The type in hand, or why it is not known.
        let (mut ty, start, mut skip_own, mut basis): (Result<NodeKey, Unresolved>, _, _, _) =
            match path.root {
                PathRoot::Expression => {
                    (Err(Unresolved::NoReceiverChain), 0, false, Basis::Enclosing)
                }
                PathRoot::This | PathRoot::Name => {
                    (Ok(scope.ty.clone()), 0, false, Basis::Enclosing)
                }
                PathRoot::Base => (Ok(scope.ty.clone()), 0, true, Basis::Enclosing),
                PathRoot::Local => {
                    let Some(first) = segments.first() else {
                        return walk.stopped(Unresolved::NoReceiverChain, None, None);
                    };
                    match self.local_type(scope, first) {
                        Ok((ty, DeclarationKind::Pattern)) => (Ok(ty), 1, false, Basis::Pattern),
                        Ok((ty, _)) => (Ok(ty), 1, false, Basis::DeclaredType),
                        Err(why) => (Err(why), 1, false, Basis::DeclaredType),
                    }
                }
            };
        if segments.len() <= start {
            if let Err(why) = ty {
                return walk.stopped(why, None, None);
            }
        }
        for (i, name) in segments.iter().enumerate().skip(start) {
            let last = i + 1 == segments.len();
            let arity = if last { call } else { None };
            let (found, link_basis) = match (pin(ty.as_ref().ok(), name, arity), &ty) {
                (Some(member), _) => {
                    let owner = match (self.symbol(&member).and_then(|s| s.owner.clone()), &ty) {
                        (Some(owner), _) => owner,
                        (None, Ok(ty)) => ty.clone(),
                        (None, Err(_)) => member.clone(),
                    };
                    let through = ty.as_ref().ok().filter(|t| **t != owner).cloned();
                    (
                        Found {
                            member,
                            owner,
                            through,
                            overloaded: false,
                        },
                        Basis::Index,
                    )
                }
                (None, Err(why)) => {
                    let why = why.clone();
                    return walk.stopped(why, Some(name), None);
                }
                (None, Ok(in_hand)) => match self.lookup(in_hand, name, arity, skip_own) {
                    Ok(found) => {
                        let b = if found.overloaded {
                            Basis::Overload
                        } else {
                            basis
                        };
                        (found, b)
                    }
                    Err(why) => {
                        let in_hand = in_hand.clone();
                        return walk.stopped(why, Some(name), Some(in_hand));
                    }
                },
            };
            skip_own = false;
            walk.chain.links.push(Link {
                member: found.member.clone(),
                owner: found.owner.clone(),
            });
            walk.bases.push(link_basis);
            if !last {
                ty = self.type_of(&found).map(|(next, substituted)| {
                    basis = if substituted {
                        Basis::BaseTypeArgument
                    } else {
                        Basis::DeclaredType
                    };
                    next
                });
            }
        }
        walk
    }
}

/// What [`Index::walk`] may pin a name to before the rules: the type in
/// hand when it is known, the name, and the call's argument count for the
/// last name.
pub(crate) type Pin<'p> = &'p dyn Fn(Option<&NodeKey>, &str, Option<u32>) -> Option<NodeKey>;

/// The result of [`Index::by_arity`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ByArity {
    One(NodeKey),
    /// No candidate declares that many parameters.
    NoneOf,
    /// Several do, by key.
    Several(Vec<NodeKey>),
}

/// An access walked by [`Index::walk`].
pub(crate) struct Walk {
    pub chain: Chain,
    /// The rule each link of `chain` was told by, link by link.
    pub bases: Vec<Basis>,
    /// Where the walk stopped, when it did.
    pub stop: Option<Stop>,
}

/// The name an access stopped at, and the type it was looked up in when the
/// walk knew it (a name the type in hand does not declare, or several
/// members of it); `None` when the type itself could not be told.
pub(crate) struct Stop {
    pub name: Option<String>,
    pub ty: Option<NodeKey>,
}

impl Walk {
    fn stopped(mut self, why: Unresolved, name: Option<&String>, ty: Option<NodeKey>) -> Walk {
        self.chain.stopped = Some(why);
        self.stop = Some(Stop {
            name: name.cloned(),
            ty,
        });
        self
    }
}

/// Whether a written type (`List<T>`, `Dictionary<string,T>`) names one of
/// `params` as a whole word.
fn names_any(written: &str, params: &[String]) -> bool {
    written
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .any(|word| params.iter().any(|p| p == word))
}

/// Whether the type `key` sits in a container and enclosing types whose
/// dotted path ends with `qualifier` (`Acme.Views#Editor` by `Views` and by
/// `Acme.Views`); a generic arity or a duplicate's suffix is not part of a
/// type's name here.
fn qualified_by(key: &NodeKey, qualifier: &str) -> bool {
    let Some(profile) = profile::of_key(key) else {
        return false;
    };
    let Some((namespace, path)) = profile.split(key) else {
        return false;
    };
    let enclosing = path.rsplit_once('.').map_or("", |(outer, _)| outer);
    let container: Vec<&str> = profile
        .container_segments(namespace)
        .chain(enclosing.split('.'))
        .filter(|segment| !segment.is_empty())
        .map(|segment| segment.split(['<', '`', '~']).next().unwrap_or(segment))
        .collect();
    let container = container.join(".");
    container == qualifier || container.ends_with(&format!(".{qualifier}"))
}

/// The full name of the type `key` — its container, enclosing types and
/// name, dotted (`Acme.Views#Outer.Inner` is `Acme.Views.Outer.Inner`) — a
/// generic arity or a duplicate's suffix left out, as [`qualified_by`] reads
/// them. `None` for a key of no profiled language.
pub fn full_name(key: &NodeKey) -> Option<String> {
    let profile = profile::of_key(key)?;
    let (namespace, path) = profile.split(key)?;
    let parts: Vec<&str> = profile
        .container_segments(namespace)
        .chain(path.split('.'))
        .filter(|segment| !segment.is_empty())
        .map(|segment| segment.split(['<', '`', '~']).next().unwrap_or(segment))
        .collect();
    (!parts.is_empty()).then(|| parts.join("."))
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
    /// The call's argument count chose it among several methods.
    overloaded: bool,
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
    fn a_full_name_is_the_namespace_and_the_type_path_dotted() {
        let full = |k: &str| full_name(&NodeKey::new(k));
        assert_eq!(
            full("sym:csharp:Acme.Views#ViewA").as_deref(),
            Some("Acme.Views.ViewA")
        );
        assert_eq!(
            full("sym:csharp:Acme.Views#Outer.Inner").as_deref(),
            Some("Acme.Views.Outer.Inner")
        );
        assert_eq!(full("sym:csharp:#Global").as_deref(), Some("Global"));
        assert_eq!(full("sym:csharp:Acme#Base~2").as_deref(), Some("Acme.Base"));
        assert_eq!(full("sym:xaml:a.xaml#K"), None);
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
