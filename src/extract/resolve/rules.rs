//! The rule catalog (plan A, KTD6): every rule with a stable id, a revision,
//! and whether it may ever hold a right to delete.
//!
//! A rule reads one usage site — what the plugin said about it (contract 0.2)
//! and what the symbol table knows — and gives each candidate a verdict: back,
//! dispute, exclude, or abstain ([`Verdict`]). It never decides a rung; the
//! resolver does that from the verdicts of all of them (`super::resolve_site`).
//!
//! Without context — a 0.1 plugin, a language whose plugin reads none — no rule
//! here runs: holding `foo()` to the caller's type needs the plugin to have
//! SAID there is no receiver, since a bare `foo()` and an `obj.foo()` are the
//! same site to a plugin that reports neither. `base-spelling` is the one rule
//! that needs no site context; it rates type relations at the run-level flush
//! (`flush_type_relations`), and is listed here for its id and revision.
//!
//! **Change a rule's behavior → raise its revision.** The revision is part of
//! the run fingerprint (KTD15), so a cached record a rule decided under its old
//! behavior is never served beside the new one, and it is what a right to
//! delete is granted against (KTD11): a right measured on revision 1 does not
//! carry over to revision 2.

use super::project::ProjectWide;
use super::rights::RightsTable;
use super::table::{simple_type, strip_generics, SymbolTable};
use crate::extract::treesitter::{lang_family, Role, UsageKind};
use crate::graph::{NodeKey, Rule, Verdict};
use crate::plugin::host::{Receiver, SiteContext, WireScope};
use std::collections::{HashMap, HashSet};

/// One catalog entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RuleSpec {
    pub rule: Rule,
    pub revision: u32,
    /// Whether measurement can ever earn this rule a right to delete. The
    /// rules that say "never" give no proof by their negative claim — a using
    /// a source generator emits, an optional parameter the header did not
    /// show — so they only ever dispute (KTD6).
    pub may_delete: bool,
}

/// The catalog, in the order verdicts are listed in a trace and the order that
/// picks the rule an edge names.
pub(crate) const CATALOG: &[RuleSpec] = &[
    RuleSpec {
        rule: Rule::RoleMismatch,
        revision: 1,
        may_delete: true,
    },
    // 2: the innermost of two aliases of one name decides what the declared
    // type spells; `ref this`, `in this` and `scoped this` mark an extension
    // method as `this` alone did.
    // 3: an extension method whose `this` type the repository does not
    // declare is never backed, only left alone. Its receiver is then an
    // external or keyword type (`string`), whose own instance members RA
    // cannot see, and C# picks an applicable instance method over any
    // extension: `xsrc.IndexOfAny(chars, p1)` on a `string` is
    // `string.IndexOfAny(char[], int)`, not the repository's `this string`
    // overload, which revision 2 backed. Its measurement on the reference C# repository
    // (`docs/measurements/2026-10-03-csharp-reference.json`) counts 8 499 sites where
    // the rule backed a candidate the index did not confirm.
    RuleSpec {
        rule: Rule::DeclaredReceiver,
        revision: 3,
        may_delete: true,
    },
    // 2: never runs on a `value` receiver — a declared local, parameter or
    // field named like a type — and the innermost alias decides.
    RuleSpec {
        rule: Rule::StaticReceiver,
        revision: 2,
        may_delete: true,
    },
    RuleSpec {
        rule: Rule::ImplicitThis,
        revision: 1,
        may_delete: false,
    },
    // 2: an extension method written with `ref this` (and the like) admits
    // its receiver as an argument too.
    RuleSpec {
        rule: Rule::Arity,
        revision: 2,
        may_delete: false,
    },
    // 2: the project's global usings, `<Using>` items and implicit usings are
    // visible in every file; a project whose items cannot be read abstains.
    RuleSpec {
        rule: Rule::NamespaceVisibility,
        revision: 2,
        may_delete: false,
    },
    // A type position or construction (or a `Type.Member()` receiver) whose
    // simple name can reach a nested candidate only from inside its declaring
    // type, a type nested in it or deriving from it, or under a `using
    // static` of it: C#'s own lookup, so it may earn a right — by
    // measurement, which it has not had.
    RuleSpec {
        rule: Rule::NestedTypeVisibility,
        revision: 1,
        may_delete: true,
    },
    RuleSpec {
        rule: Rule::BaseSpelling,
        revision: 1,
        may_delete: false,
    },
];

/// The catalog revision of `rule`, or `None` for a rule the catalog does not
/// list (`name-match`, which is no sifting rule).
pub(crate) fn revision_of(rule: Rule) -> Option<u32> {
    CATALOG.iter().find(|s| s.rule == rule).map(|s| s.revision)
}

/// Whether `rule` may ever delete (see [`RuleSpec::may_delete`]).
pub(crate) fn may_delete(rule: Rule) -> bool {
    CATALOG.iter().any(|s| s.rule == rule && s.may_delete)
}

/// The resolver's revision (KTD15): every rule's id and revision, and the
/// rights table's fingerprint. A pure function of its inputs, so a changed
/// rule or a granted right changes it and nothing else does.
pub(crate) fn resolver_revision(catalog: &[RuleSpec], rights: &RightsTable) -> String {
    let rules: Vec<String> = catalog
        .iter()
        .map(|s| format!("{}@{}", s.rule.id(), s.revision))
        .collect();
    format!("{};rights={}", rules.join(","), rights.fingerprint())
}

/// One usage site as the rules read it.
pub(crate) struct Site<'s> {
    pub kind: UsageKind,
    pub context: &'s SiteContext,
    /// The file's scopes, which `context.scope` indexes.
    pub scopes: &'s [WireScope],
    /// What the usings of the file's project make visible beyond its own
    /// scopes ([`super::project`]).
    pub project: &'s ProjectWide,
}

/// The rules, ready to judge the candidates of one site. Everything a rule
/// works out once per site — the declared type's repository types and their
/// ancestors, the caller's own type, the visible namespaces — is worked out
/// here, so judging a candidate is a lookup.
pub(crate) struct Judge<'t, 's> {
    table: &'t SymbolTable,
    kind: UsageKind,
    argument_count: Option<u32>,
    declared: Option<Declared<'t>>,
    receiver_types: Option<HashSet<&'t NodeKey>>,
    own: Option<HashSet<&'t NodeKey>>,
    /// The namespaces the site's own scopes make visible, and those its
    /// project makes visible in every file — `None` where the rule does not
    /// run.
    visible: Option<(HashSet<String>, &'s HashSet<String>)>,
    /// What `nested-type-visibility` reads — `None` where it does not run.
    nested: Option<Nested<'t, 's>>,
}

/// Where a nested type's simple name reaches at one site (C# name lookup):
/// from inside the types enclosing the site and the types they derive from,
/// whose nested types it sees, and through a `using static` or an alias.
struct Nested<'t, 's> {
    /// The receiver of a static-receiver call, spelled as one simple name,
    /// generic arguments dropped. `None` at a type position or construction,
    /// whose candidate is the type itself.
    receiver: Option<String>,
    /// The types enclosing the site, innermost out, and every repository type
    /// they extend or implement: a type nested in any of these is visible.
    enclosing: HashSet<&'t NodeKey>,
    /// The simple names the enclosing types' base lists spell — a base the
    /// repository has more than one type of carries no `Extends` edge, and
    /// such a base is the declaring type as far as this rule can tell.
    spelled_bases: HashSet<&'t str>,
    /// The repository types a `using static` in force at the site names, in
    /// its own scopes or project-wide, with their ancestors.
    statics: HashSet<&'t NodeKey>,
    /// The names an alias in force at the site introduces.
    aliases: HashSet<&'s str>,
}

/// What `declared-receiver` knows about the receiver's declared type.
struct Declared<'t> {
    /// Its simple name, as the file spells it.
    simple: String,
    /// The repository types it can mean — empty for a type the repository does
    /// not declare (`StringBuilder`) and for a keyword type (`string`).
    types: Vec<&'t NodeKey>,
    /// Those and every repository type they extend or implement.
    with_ancestors: HashSet<&'t NodeKey>,
    /// `object`: every class descends from it, so every override is reachable.
    is_root: bool,
}

impl<'t, 's> Judge<'t, 's> {
    /// Prepare the rules for one site. `owner` is the declaration enclosing it,
    /// only when the site is mapped to one (KTD7); `family` is the language
    /// family of the file, which the types a spelling can mean are drawn from.
    pub(crate) fn new(
        table: &'t SymbolTable,
        site: &Site<'s>,
        owner: Option<&'t NodeKey>,
        family: Option<&str>,
    ) -> Judge<'t, 's> {
        let ctx = site.context;
        let call = site.kind == UsageKind::Call;
        let chain = scope_chain(site.scopes, ctx.scope);
        // Outermost first, so an alias of an inner scope overwrites the outer
        // one it shadows, as C# resolves it.
        let aliases: HashMap<&str, &str> = chain
            .iter()
            .rev()
            .flat_map(|s| s.usings.iter())
            .filter_map(|u| Some((u.alias.as_deref()?, u.target.as_str())))
            .collect();
        let unalias = |spelled: &str| -> String {
            let (first, rest) = spelled.split_once('.').unwrap_or((spelled, ""));
            match aliases.get(first) {
                Some(target) if rest.is_empty() => target.to_string(),
                Some(target) => format!("{target}.{rest}"),
                None => spelled.to_string(),
            }
        };
        // The type parameters in scope at the site: a receiver declared as one
        // can hold anything its constraints allow, which no header shows.
        let mut generic: HashSet<&str> = HashSet::new();
        if let Some(owner) = owner {
            let mut key = Some(owner);
            while let Some(k) = key {
                let Some(info) = table.info(k) else { break };
                generic.extend(info.generic_params.iter().map(String::as_str));
                key = info.declaring_type.as_ref();
            }
        }

        let declared = match (&ctx.declared_type, family) {
            (Some(dt), Some(family))
                if call && dt.name != "dynamic" && !generic.contains(dt.name.as_str()) =>
            {
                let spelled = unalias(&dt.name);
                let types = if dt.keyword {
                    Vec::new()
                } else {
                    table.types_named(family, &spelled)
                };
                let simple = simple_type(&spelled);
                Some(Declared {
                    with_ancestors: table.with_ancestors(types.iter().copied()),
                    is_root: matches!(simple.as_str(), "object" | "Object"),
                    simple,
                    types,
                })
            }
            _ => None,
        };

        // Only a `name` can be a type. A `value` is a binding the file
        // declares — `var Worker = Load(); Worker.Run()` — which shadows any
        // type of that name even when its own type is unknown.
        let receiver_types = match (&ctx.receiver, &ctx.declared_type, family) {
            (Receiver::Name(name), None, Some(family))
                if call
                    && name != "this"
                    && name != "base"
                    && !name.starts_with("this.")
                    && !name.starts_with("base.") =>
            {
                let spelled = table.types_named(family, &unalias(name));
                (!spelled.is_empty()).then(|| table.with_ancestors(spelled))
            }
            _ => None,
        };

        let own = match &ctx.receiver {
            Receiver::None if call => owner
                .and_then(|o| table.own_type(o))
                .map(|t| table.with_ancestors([t])),
            Receiver::Name(name) if call && name == "this" => owner
                .and_then(|o| table.own_type(o))
                .map(|t| table.with_ancestors([t])),
            Receiver::Name(name) if call && name == "base" => {
                owner.and_then(|o| table.own_type(o)).map(|t| {
                    let mut set = table.with_ancestors([t]);
                    set.remove(t);
                    set
                })
            }
            _ => None,
        };

        let visible = match site.project {
            ProjectWide::Known(project)
                if matches!(site.kind, UsageKind::Instantiate | UsageKind::Reference)
                    && ctx.qualifier.is_none() =>
            {
                Some((visible_namespaces(&chain), &project.namespaces))
            }
            // A project file imports what this host cannot read: the visible
            // set is known to be incomplete, so the rule says nothing.
            _ => None,
        };

        // A simple name only: a qualified spelling (`Outer.List`, an
        // `Alias.List`) names its container, and the rule has nothing to say.
        let simple_receiver = match (&ctx.receiver, &ctx.declared_type) {
            (Receiver::Name(name), None) if call => {
                let name = strip_generics(name);
                (!name.contains('.') && !name.contains("::")).then_some(name)
            }
            _ => None,
        };
        let type_site = matches!(site.kind, UsageKind::Instantiate | UsageKind::Reference)
            && ctx.qualifier.is_none();
        let nested = match (site.project, owner, family) {
            (ProjectWide::Known(project), Some(owner), Some(family))
                if type_site || simple_receiver.is_some() =>
            {
                let mut enclosing = HashSet::new();
                let mut spelled_bases = HashSet::new();
                let mut at = table.own_type(owner);
                while let Some(t) = at {
                    // A cycle — only broken code has one — ends the walk.
                    if !enclosing.insert(t) {
                        break;
                    }
                    at = table.info(t).and_then(|i| i.declaring_type.as_ref());
                }
                let enclosing = table.with_ancestors(enclosing);
                for t in &enclosing {
                    if let Some(info) = table.info(t) {
                        spelled_bases.extend(info.spelled_bases.iter().map(String::as_str));
                    }
                }
                let static_targets = chain
                    .iter()
                    .flat_map(|s| s.usings.iter())
                    .filter(|u| u.is_static)
                    .map(|u| unalias(&u.target))
                    .chain(project.statics.iter().cloned());
                let static_types: Vec<&NodeKey> = static_targets
                    .flat_map(|t| table.types_named(family, &t))
                    .collect();
                Some(Nested {
                    receiver: simple_receiver,
                    enclosing,
                    spelled_bases,
                    statics: table.with_ancestors(static_types),
                    aliases: aliases
                        .keys()
                        .copied()
                        .chain(project.aliases.iter().map(String::as_str))
                        .collect(),
                })
            }
            // No owner (an unmapped site), no family, or a project whose
            // usings cannot be read: the data to decide is missing.
            _ => None,
        };

        Judge {
            table,
            kind: site.kind,
            argument_count: ctx.argument_count.filter(|_| call),
            declared,
            receiver_types,
            own,
            visible,
            nested,
        }
    }

    /// Every applicable rule's verdict on `candidate`, in catalog order.
    pub(crate) fn judge(&self, candidate: &NodeKey) -> Vec<(Rule, Verdict)> {
        let info = self.table.info(candidate);
        let mut out = Vec::new();
        if self.kind == UsageKind::Call {
            let constructor = info.is_some_and(|i| i.kind == "constructor");
            out.push((
                Rule::RoleMismatch,
                if constructor {
                    Verdict::Exclude
                } else {
                    Verdict::Abstain
                },
            ));
        }
        if let Some(declared) = &self.declared {
            out.push((
                Rule::DeclaredReceiver,
                self.declared_receiver(declared, candidate),
            ));
        }
        if let Some(types) = &self.receiver_types {
            out.push((Rule::StaticReceiver, self.static_receiver(types, candidate)));
        }
        if let Some(own) = &self.own {
            let backed = info
                .and_then(|i| i.declaring_type.as_ref())
                .is_some_and(|t| own.contains(t));
            out.push((
                Rule::ImplicitThis,
                if backed {
                    Verdict::Back
                } else {
                    Verdict::Abstain
                },
            ));
        }
        if let Some(n) = self.argument_count {
            let verdict = match info.and_then(|i| i.arity.map(|a| (a, i.extends.is_some()))) {
                // An extension method is called with its `this` argument
                // either before the dot or inside the list.
                Some((arity, extension))
                    if !(arity.admits(n) || extension && arity.admits(n + 1)) =>
                {
                    Verdict::Dispute
                }
                _ => Verdict::Abstain,
            };
            out.push((Rule::Arity, verdict));
        }
        if let Some((visible, project)) = &self.visible {
            let verdict = match info {
                Some(i) if i.role == Some(Role::Type) && !is_nested(candidate) => {
                    match &i.namespace {
                        Some(ns) if !visible.contains(ns) && !project.contains(ns) => {
                            Verdict::Dispute
                        }
                        _ => Verdict::Abstain,
                    }
                }
                _ => Verdict::Abstain,
            };
            out.push((Rule::NamespaceVisibility, verdict));
        }
        if let Some(nested) = &self.nested {
            out.push((
                Rule::NestedTypeVisibility,
                self.nested_type_visibility(nested, candidate),
            ));
        }
        out
    }

    /// `nested-type-visibility` (KTD6): a type nested in another is reachable
    /// by its simple name only from inside its declaring type, a type nested
    /// in that one, or a type deriving from it — or where a `using static` of
    /// it or an alias of the name is in force. Anywhere else the simple name
    /// means something else (`List<…>` in a foreign file is the BCL's, not a
    /// `Catalog.List`), and the candidate is excluded.
    ///
    /// At a type position or construction the candidate is the type; at a
    /// static-receiver call it is a member of the type the receiver spells,
    /// which the same lookup must reach first.
    fn nested_type_visibility(&self, nested: &Nested<'t, 's>, candidate: &NodeKey) -> Verdict {
        let Some(info) = self.table.info(candidate) else {
            return Verdict::Abstain;
        };
        let ty = match &nested.receiver {
            None if info.role == Some(Role::Type) => candidate,
            Some(receiver) => match info.declaring_type.as_ref() {
                Some(t) if type_simple_name(t) == *receiver => t,
                _ => return Verdict::Abstain,
            },
            None => return Verdict::Abstain,
        };
        let Some(declaring) = self
            .table
            .info(ty)
            .and_then(|i| i.declaring_type.as_ref())
            .filter(|d| {
                self.table
                    .info(d)
                    .is_some_and(|i| i.role == Some(Role::Type))
            })
        else {
            // A top-level type: nothing to say.
            return Verdict::Abstain;
        };
        let visible = nested.enclosing.contains(declaring)
            || nested
                .spelled_bases
                .contains(type_simple_name(declaring).as_str())
            || nested.statics.contains(declaring)
            || nested.aliases.contains(type_simple_name(ty).as_str());
        if visible {
            Verdict::Abstain
        } else {
            Verdict::Exclude
        }
    }

    /// `declared-receiver` (KTD6): members of the declared type and its
    /// ancestors are backed; overrides and implementations in its descendants
    /// are left alone, since dynamic dispatch reaches them; an extension method
    /// stands or falls by its `this` type; anything else is excluded.
    fn declared_receiver(&self, declared: &Declared<'t>, candidate: &NodeKey) -> Verdict {
        let Some(info) = self.table.info(candidate) else {
            return Verdict::Abstain;
        };
        let family = lang_family(candidate.as_str()).unwrap_or("");
        if let Some(this) = &info.extends {
            let Some(this) = this else {
                // `this T` for the method's own `T`: any receiver fits.
                return Verdict::Abstain;
            };
            let this_types = self.table.types_named(family, this);
            if this_types.is_empty() {
                // An external `this` type: whether the declared type derives
                // from it is not something the repository says. Nor is it
                // backed when the two are spelled alike (`this string` on a
                // `string`): the receiver's type is then external too, and an
                // applicable instance member of it — which no repository
                // shows — wins over any extension (`string.IndexOfAny`).
                return Verdict::Abstain;
            }
            return if this_types
                .iter()
                .any(|t| declared.with_ancestors.contains(t))
            {
                Verdict::Back
            } else {
                Verdict::Exclude
            };
        }
        let Some(owner) = &info.declaring_type else {
            return Verdict::Abstain;
        };
        if declared.with_ancestors.contains(owner) {
            return Verdict::Back;
        }
        let lineage = self.table.with_ancestors([owner]);
        let descends = if declared.types.is_empty() {
            // A declared type outside the repository is reached only through
            // the bases a repository type spells.
            declared.is_root
                || lineage.iter().any(|t| {
                    self.table
                        .info(t)
                        .is_some_and(|i| i.spelled_bases.contains(&declared.simple))
                })
        } else {
            declared.types.iter().any(|d| lineage.contains(d))
        };
        if descends {
            Verdict::Abstain
        } else {
            Verdict::Exclude
        }
    }

    /// `static-receiver` (KTD6): `Type.Member()` on a repository type backs
    /// the static members of that type and its ancestors, and excludes members
    /// of any other type. Their instance members are left alone: a property
    /// named like its type (`Color Color`) makes `Color.M()` a call on the
    /// instance, which the spelling alone cannot tell apart.
    fn static_receiver(&self, types: &HashSet<&'t NodeKey>, candidate: &NodeKey) -> Verdict {
        let Some(info) = self.table.info(candidate) else {
            return Verdict::Abstain;
        };
        let Some(owner) = &info.declaring_type else {
            return Verdict::Abstain;
        };
        if types.contains(owner) {
            if info.is_static {
                Verdict::Back
            } else {
                Verdict::Abstain
            }
        } else {
            Verdict::Exclude
        }
    }
}

/// A type nested in another (`N#Outer.Inner`): reachable by its simple name
/// from inside its container and its descendants, which the visible
/// namespaces do not show, so `namespace-visibility` leaves it alone and
/// `nested-type-visibility` decides.
fn is_nested(key: &NodeKey) -> bool {
    key.as_str()
        .split_once('#')
        .is_some_and(|(_, path)| path.contains('.'))
}

/// The simple name a type key spells: its last path segment, any ordinal
/// suffix or generic arguments left out (`N#Outer.List` → `List`).
fn type_simple_name(key: &NodeKey) -> String {
    let path = key.as_str().split_once('#').map_or("", |(_, p)| p);
    simple_type(path.split(['~', '(']).next().unwrap_or(path))
}

/// The scopes from the site's innermost one out to the file.
fn scope_chain(scopes: &[WireScope], innermost: u32) -> Vec<&WireScope> {
    let mut out = Vec::new();
    let mut at = Some(innermost);
    while let Some(i) = at {
        let Some(scope) = scopes.get(i as usize) else {
            break;
        };
        // A parent index that does not point outward would loop.
        if out.len() > scopes.len() {
            break;
        }
        out.push(scope);
        at = scope.parent;
    }
    out
}

/// The namespaces a simple type name can reach from the site through its own
/// file: each enclosing namespace and its parents, and every namespace a using
/// directive in an enclosing scope imports. What the project imports into
/// every file — global usings written in other files, `<Using>` items, the
/// SDK's implicit usings — comes beside it from [`super::project`], which
/// still cannot see everything (a source generator's usings), the reason this
/// rule never deletes.
fn visible_namespaces(chain: &[&WireScope]) -> HashSet<String> {
    let mut out = HashSet::new();
    for scope in chain {
        let mut ns = scope.namespace.as_str();
        while !ns.is_empty() {
            out.insert(ns.to_string());
            ns = ns.rsplit_once('.').map_or("", |(parent, _)| parent);
        }
        for using in &scope.usings {
            if using.alias.is_none() && !using.is_static {
                let target = using
                    .target
                    .strip_prefix("global::")
                    .unwrap_or(&using.target);
                out.insert(target.to_string());
            }
        }
    }
    out
}
