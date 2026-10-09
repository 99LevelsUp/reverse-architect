//! The shared resolver (plan A, U7): one host module sifts the name candidates
//! of a usage site for every language plugin alike (R4, KTD5).
//!
//! A plugin supplies facts about the site (contract 0.2, KTD3); the host
//! decides. For each site the resolver takes the candidates its name matched in
//! the `table::SymbolTable`, lets every applicable rule of the
//! `rules::CATALOG` give each candidate a verdict, applies the deletions a
//! valid right allows ([`rights`]), checks the ambiguity ceiling, and gives
//! every remaining candidate its own rung — following the per-candidate
//! flowchart of the plan:
//!
//! - an exclusion removes the candidate only when its rule holds a right to
//!   delete for this site (KTD11); otherwise it counts as a dispute;
//! - a disputed candidate is [`Confidence::Ambiguous`];
//! - [`Confidence::RulesBacked`] needs the backing to PICK the target: the
//!   candidate is the only one of the site's remaining candidates that a rule
//!   backed and none disputed. Two backed (`Run(int)` and `Run(string)` on the
//!   declared type) are both ambiguous until another rule tells them apart,
//!   and their edges name `name-match`, not the rule that backed them: an
//!   ambiguous edge names a rule only when that rule disputed it;
//! - a candidate no rule decided is ambiguous when the site has several left,
//!   and [`Confidence::NameOnly`] when it is the only one.
//!
//! The ceiling is compared with the count AFTER exclusion, so without rights
//! the volume of edges stays what it was. When no rule with a right applies to
//! the site at all, the ceiling is checked on the raw count BEFORE any rule
//! runs: a name like `Get` with three hundred candidates is settled by one
//! fact, with no work proportional to its candidates. Except under
//! measurement (`ra measure`): a right granted later makes the rules run at
//! exactly those sites, so a measurement that skipped them would vouch for
//! deletions it never looked at. There every site's rules run, and the site
//! still ends over the ceiling with the same one fact.
//!
//! Candidates are judged as references into the shared table, never copied,
//! until a [`crate::graph::SiteRecord`] is built for a run that keeps them.

pub(crate) mod project;
pub mod rights;
pub(crate) mod rules;
pub(crate) mod table;

use crate::graph::{Confidence, NodeKey, Rule, Verdict};
use rules::{Judge, Site};
use table::SymbolTable;

/// What became of one candidate of a site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Fate {
    /// An edge with this rung, named by this rule.
    Kept { rung: Confidence, rule: Rule },
    /// Removed by this rule's valid right to delete.
    Deleted(Rule),
    /// Survived exclusion, but the site is over the ceiling.
    OverCap,
}

/// One candidate as the resolver left it.
#[derive(Debug)]
pub(crate) struct Judged<'t> {
    pub target: &'t NodeKey,
    /// Every applicable rule's verdict, in catalog order (KTD7).
    pub trace: Vec<(Rule, Verdict)>,
    pub fate: Fate,
}

/// The resolver's answer for one site.
#[derive(Debug)]
pub(crate) struct Resolved<'t> {
    /// Every candidate, in table order — empty when the raw count was over the
    /// ceiling and no rule could have thinned it.
    pub candidates: Vec<Judged<'t>>,
    /// The candidate count the site ended with: the raw count when it was
    /// settled before any rule ran, the count after exclusion otherwise.
    pub count: usize,
    /// The site is over the ceiling: one ambiguity fact, no edges.
    pub over_cap: bool,
}

/// Resolve one site's `candidates`.
///
/// `site` is `None` for a site without context, where no rule runs. `rights`
/// are the rules holding a valid right to delete for this site's origin
/// ([`rights::RightsTable::active_for`]). `measuring` runs the rules at a site
/// over the ceiling even when none of them may delete (module docs); what the
/// site becomes is the same either way.
#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_site<'t>(
    table: &'t SymbolTable,
    site: Option<&Site<'_>>,
    owner: Option<&'t NodeKey>,
    family: Option<&str>,
    candidates: &'t [NodeKey],
    rights: &[Rule],
    ceiling: usize,
    measuring: bool,
) -> Resolved<'t> {
    let judging = site.is_some() && (!rights.is_empty() || measuring);
    if !judging && candidates.len() > ceiling {
        return Resolved {
            candidates: Vec::new(),
            count: candidates.len(),
            over_cap: true,
        };
    }
    let judge = site.map(|s| Judge::new(table, s, owner, family));
    let mut judged: Vec<Judged<'t>> = candidates
        .iter()
        .map(|target| {
            let trace = judge.as_ref().map(|j| j.judge(target)).unwrap_or_default();
            let deleted = trace
                .iter()
                .find(|(rule, verdict)| *verdict == Verdict::Exclude && rights.contains(rule))
                .map(|(rule, _)| *rule);
            Judged {
                target,
                trace,
                fate: match deleted {
                    Some(rule) => Fate::Deleted(rule),
                    None => Fate::OverCap,
                },
            }
        })
        .collect();

    let remaining = judged
        .iter()
        .filter(|j| !matches!(j.fate, Fate::Deleted(_)))
        .count();
    if remaining > ceiling {
        return Resolved {
            candidates: judged,
            count: remaining,
            over_cap: true,
        };
    }

    // An exclusion that was not applied weighs as a dispute (KTD5).
    let disputed_by = |j: &Judged<'_>| {
        j.trace
            .iter()
            .find(|(_, v)| matches!(v, Verdict::Dispute | Verdict::Exclude))
            .map(|(rule, _)| *rule)
    };
    let backed_by = |j: &Judged<'_>| {
        j.trace
            .iter()
            .find(|(_, v)| *v == Verdict::Back)
            .map(|(rule, _)| *rule)
    };
    let settled = judged
        .iter()
        .filter(|j| !matches!(j.fate, Fate::Deleted(_)))
        .filter(|j| disputed_by(j).is_none() && backed_by(j).is_some())
        .count();
    for j in judged
        .iter_mut()
        .filter(|j| !matches!(j.fate, Fate::Deleted(_)))
    {
        j.fate = match (disputed_by(j), backed_by(j)) {
            (Some(rule), _) => Fate::Kept {
                rung: Confidence::Ambiguous,
                rule,
            },
            (None, Some(rule)) if settled == 1 => Fate::Kept {
                rung: Confidence::RulesBacked,
                rule,
            },
            // A tie: the backing picked no one target, so the edge does not
            // name the backing rule — a reader of the edge alone could not
            // tell it from the rule's dispute. The trace keeps the `Back`
            // verdicts for measurement.
            (None, Some(_)) => Fate::Kept {
                rung: Confidence::Ambiguous,
                rule: Rule::NameMatch,
            },
            (None, None) => Fate::Kept {
                rung: if remaining > 1 {
                    Confidence::Ambiguous
                } else {
                    Confidence::NameOnly
                },
                rule: Rule::NameMatch,
            },
        };
    }
    Resolved {
        candidates: judged,
        count: remaining,
        over_cap: false,
    }
}

#[cfg(test)]
mod tests {
    //! The resolver over hand-built sites (plan A, U7 execution note): a
    //! symbol table built from a graph written by hand, and site context
    //! written as a 0.2 plugin would send it — no component involved.

    use super::*;
    use crate::extract::treesitter::UsageKind;
    use crate::graph::{
        Edge, EdgeKind, Fact, FactKind, FactValue, Graph, NodeKind, Provenance, Tier,
    };
    use crate::plugin::host::{DeclaredType, Receiver, SiteContext, UsingDirective, WireScope};
    use project::{Imports, ProjectWide};
    use std::collections::HashMap;
    use std::sync::LazyLock;

    /// A site's project imports nothing beyond its own scopes.
    static NO_PROJECT: LazyLock<ProjectWide> =
        LazyLock::new(|| ProjectWide::Known(Imports::default()));

    /// Declare `key` (`sym:csharp:N#…`) as a `kind` named `name` on lines
    /// `start..=end` of `B.cs`, with `signature`, a member of `owner`.
    fn declare(
        g: &mut Graph,
        key: &str,
        kind: &str,
        name: &str,
        lines: (u32, u32),
        signature: &str,
        owner: Option<&str>,
    ) {
        let key = NodeKey::new(key);
        g.ensure_node(key.clone(), NodeKind::Symbol).unwrap();
        let prov = Provenance::new("src/B.cs", None, "csharp".into(), Tier::Measured)
            .with_range(lines.0, lines.1);
        for (k, v) in [
            (FactKind::SymbolName, name),
            (FactKind::SymbolKind, kind),
            (FactKind::SymbolSignature, signature),
        ] {
            g.add_fact(&key, Fact::new(k, FactValue::Text(v.into()), prov.clone()));
        }
        if let Some(owner) = owner {
            g.add_edge(
                &NodeKey::new(owner),
                &key,
                Edge::new(EdgeKind::HasMember, prov),
            );
        }
    }

    /// `Worker { Go(); Run(); static Make(); Run(int); }` and `Other { Run(); }`.
    fn table() -> SymbolTable {
        let mut g = Graph::new();
        let w = "sym:csharp:N#Worker";
        let o = "sym:csharp:N#Other";
        declare(
            &mut g,
            w,
            "class",
            "Worker",
            (1, 10),
            "public class Worker",
            None,
        );
        declare(
            &mut g,
            "sym:csharp:N#Worker.Go()",
            "method",
            "Go",
            (2, 5),
            "public void Go()",
            Some(w),
        );
        declare(
            &mut g,
            "sym:csharp:N#Worker.Run()",
            "method",
            "Run",
            (6, 6),
            "public void Run()",
            Some(w),
        );
        declare(
            &mut g,
            "sym:csharp:N#Worker.Make()",
            "method",
            "Make",
            (7, 7),
            "public static void Make()",
            Some(w),
        );
        declare(
            &mut g,
            "sym:csharp:N#Worker.Run(int)",
            "method",
            "Run",
            (8, 8),
            "public void Run(int n)",
            Some(w),
        );
        declare(
            &mut g,
            o,
            "class",
            "Other",
            (11, 13),
            "public class Other",
            None,
        );
        declare(
            &mut g,
            "sym:csharp:N#Other.Run()",
            "method",
            "Run",
            (12, 12),
            "public void Run()",
            Some(o),
        );
        SymbolTable::build(&g, &HashMap::new())
    }

    fn context(receiver: Receiver) -> SiteContext {
        SiteContext {
            scope: 0,
            receiver,
            declared_type: None,
            argument_count: Some(0),
            qualifier: None,
        }
    }

    fn scopes() -> Vec<WireScope> {
        vec![WireScope {
            namespace: "N".into(),
            lines: (1, 20),
            parent: None,
            usings: Vec::new(),
        }]
    }

    fn rungs(resolved: &Resolved<'_>) -> Vec<(String, Fate)> {
        resolved
            .candidates
            .iter()
            .map(|j| (j.target.as_str().to_string(), j.fate))
            .collect()
    }

    /// A 0.1 plugin cannot tell `Run()` from `obj.Run()`, so no rule runs —
    /// `implicit-this` included — and the rung follows the count alone. The
    /// same bare call from a 0.2 plugin that SAYS there is no receiver is
    /// backed by `implicit-this`.
    #[test]
    fn implicit_this_runs_only_where_the_plugin_said_there_is_no_receiver() {
        let t = table();
        let owner = NodeKey::new("sym:csharp:N#Worker.Go()");
        let one = [NodeKey::new("sym:csharp:N#Worker.Run()")];

        let without = resolve_site(&t, None, Some(&owner), Some("csharp"), &one, &[], 8, false);
        assert_eq!(
            rungs(&without),
            vec![(
                "sym:csharp:N#Worker.Run()".to_string(),
                Fate::Kept {
                    rung: Confidence::NameOnly,
                    rule: Rule::NameMatch
                }
            )]
        );
        assert!(without.candidates[0].trace.is_empty());

        let ctx = context(Receiver::None);
        let scopes = scopes();
        let site = Site {
            kind: UsageKind::Call,
            context: &ctx,
            scopes: &scopes,
            project: &NO_PROJECT,
        };
        let with = resolve_site(
            &t,
            Some(&site),
            Some(&owner),
            Some("csharp"),
            &one,
            &[],
            8,
            false,
        );
        assert_eq!(
            with.candidates[0].fate,
            Fate::Kept {
                rung: Confidence::RulesBacked,
                rule: Rule::ImplicitThis
            }
        );
    }

    /// `implicit-this` only ever backs: the other type's `Run` is left
    /// undecided — ambiguous among two — even with every right granted.
    #[test]
    fn implicit_this_never_disputes_or_deletes() {
        let t = table();
        let owner = NodeKey::new("sym:csharp:N#Worker.Go()");
        let both = [
            NodeKey::new("sym:csharp:N#Other.Run()"),
            NodeKey::new("sym:csharp:N#Worker.Run()"),
        ];
        let ctx = context(Receiver::None);
        let scopes = scopes();
        let site = Site {
            kind: UsageKind::Call,
            context: &ctx,
            scopes: &scopes,
            project: &NO_PROJECT,
        };
        let all_rights: Vec<Rule> = rules::CATALOG.iter().map(|s| s.rule).collect();
        let r = resolve_site(
            &t,
            Some(&site),
            Some(&owner),
            Some("csharp"),
            &both,
            &all_rights,
            8,
            false,
        );
        assert_eq!(
            rungs(&r),
            vec![
                (
                    "sym:csharp:N#Other.Run()".to_string(),
                    Fate::Kept {
                        rung: Confidence::Ambiguous,
                        rule: Rule::NameMatch
                    }
                ),
                (
                    "sym:csharp:N#Worker.Run()".to_string(),
                    Fate::Kept {
                        rung: Confidence::RulesBacked,
                        rule: Rule::ImplicitThis
                    }
                ),
            ]
        );
    }

    /// A receiver declared as an external type disputes every repository
    /// member (AE2): one candidate, disputed, is ambiguous — never name-only.
    #[test]
    fn a_disputed_single_candidate_is_ambiguous() {
        let t = table();
        let owner = NodeKey::new("sym:csharp:N#Worker.Go()");
        let one = [NodeKey::new("sym:csharp:N#Other.Run()")];
        let mut ctx = context(Receiver::Name("sb".into()));
        ctx.declared_type = Some(DeclaredType {
            name: "StringBuilder".into(),
            keyword: false,
        });
        let scopes = scopes();
        let site = Site {
            kind: UsageKind::Call,
            context: &ctx,
            scopes: &scopes,
            project: &NO_PROJECT,
        };
        let r = resolve_site(
            &t,
            Some(&site),
            Some(&owner),
            Some("csharp"),
            &one,
            &[],
            8,
            false,
        );
        assert_eq!(
            r.candidates[0].fate,
            Fate::Kept {
                rung: Confidence::Ambiguous,
                rule: Rule::DeclaredReceiver
            }
        );
        assert_eq!(r.count, 1);
    }

    /// Without a right the raw count is checked first: over the ceiling, no
    /// candidate is judged at all.
    #[test]
    fn without_rights_the_ceiling_is_checked_before_any_rule() {
        let t = table();
        let both = [
            NodeKey::new("sym:csharp:N#Other.Run()"),
            NodeKey::new("sym:csharp:N#Worker.Run()"),
        ];
        let ctx = context(Receiver::None);
        let scopes = scopes();
        let site = Site {
            kind: UsageKind::Call,
            context: &ctx,
            scopes: &scopes,
            project: &NO_PROJECT,
        };
        let r = resolve_site(&t, Some(&site), None, Some("csharp"), &both, &[], 1, false);
        assert!(r.over_cap);
        assert!(r.candidates.is_empty());
        assert_eq!(r.count, 2);

        // Measuring, the same site ends the same way — over the ceiling, with
        // the same count — but every candidate carries what the rules said.
        let measured = resolve_site(&t, Some(&site), None, Some("csharp"), &both, &[], 1, true);
        assert!(measured.over_cap);
        assert_eq!(measured.count, 2);
        assert_eq!(measured.candidates.len(), 2);
        assert!(measured
            .candidates
            .iter()
            .all(|j| j.fate == Fate::OverCap && !j.trace.is_empty()));
    }

    /// `Worker.Make()` on a `name` is a call on the type: `static-receiver`
    /// backs its static member. The same spelling on a `value` — a local
    /// or parameter the file declares, named like the type, whose type it
    /// does not state — is a call on that value, and the rule never runs.
    #[test]
    fn static_receiver_never_reads_a_value_as_a_type() {
        let t = table();
        let owner = NodeKey::new("sym:csharp:N#Other.Run()");
        let one = [NodeKey::new("sym:csharp:N#Worker.Make()")];
        let scopes = scopes();
        let judged = |receiver: Receiver| {
            let ctx = context(receiver);
            let site = Site {
                kind: UsageKind::Call,
                context: &ctx,
                scopes: &scopes,
                project: &NO_PROJECT,
            };
            let r = resolve_site(
                &t,
                Some(&site),
                Some(&owner),
                Some("csharp"),
                &one,
                &[],
                8,
                false,
            );
            (r.candidates[0].fate, r.candidates[0].trace.clone())
        };

        let (fate, _) = judged(Receiver::Name("Worker".into()));
        assert_eq!(
            fate,
            Fate::Kept {
                rung: Confidence::RulesBacked,
                rule: Rule::StaticReceiver
            }
        );
        let (fate, trace) = judged(Receiver::Value("Worker".into()));
        assert_eq!(
            fate,
            Fate::Kept {
                rung: Confidence::NameOnly,
                rule: Rule::NameMatch
            }
        );
        assert!(
            trace.iter().all(|(rule, _)| *rule != Rule::StaticReceiver),
            "{trace:?}"
        );
    }

    /// Two candidates a rule backed alike are a tie: both ambiguous, and the
    /// edge names `name-match` — the backing picked no one, and a card must
    /// never read the backing rule as the reason it disputed (R7). The
    /// trace still holds the backing for measurement.
    #[test]
    fn a_tie_between_backed_candidates_is_named_by_the_shared_name() {
        let t = table();
        let owner = NodeKey::new("sym:csharp:N#Worker.Go()");
        // `w.Run()` with no argument count said: both overloads of the
        // declared type are backed alike.
        let both_worker = [
            NodeKey::new("sym:csharp:N#Worker.Run()"),
            NodeKey::new("sym:csharp:N#Worker.Run(int)"),
        ];
        let mut ctx = context(Receiver::Value("w".into()));
        ctx.declared_type = Some(DeclaredType {
            name: "Worker".into(),
            keyword: false,
        });
        ctx.argument_count = None;
        let scopes = scopes();
        let site = Site {
            kind: UsageKind::Call,
            context: &ctx,
            scopes: &scopes,
            project: &NO_PROJECT,
        };
        let r = resolve_site(
            &t,
            Some(&site),
            Some(&owner),
            Some("csharp"),
            &both_worker,
            &[],
            8,
            false,
        );
        for j in &r.candidates {
            assert_eq!(
                j.fate,
                Fate::Kept {
                    rung: Confidence::Ambiguous,
                    rule: Rule::NameMatch
                },
                "{}",
                j.target.as_str()
            );
            assert!(
                j.trace.contains(&(Rule::DeclaredReceiver, Verdict::Back)),
                "{:?}",
                j.trace
            );
        }
    }

    /// An alias of an inner scope shadows the same alias of an outer one:
    /// `W` declared in the namespace means `Worker` there, though the file
    /// says `W = Other`.
    #[test]
    fn the_innermost_alias_decides() {
        let t = table();
        let owner = NodeKey::new("sym:csharp:N#Worker.Go()");
        let both = [
            NodeKey::new("sym:csharp:N#Other.Run()"),
            NodeKey::new("sym:csharp:N#Worker.Run()"),
        ];
        let alias = |target: &str| UsingDirective {
            target: target.into(),
            alias: Some("W".into()),
            is_static: false,
            is_global: false,
        };
        let scopes = vec![
            WireScope {
                namespace: String::new(),
                lines: (1, 20),
                parent: None,
                usings: vec![alias("N.Other")],
            },
            WireScope {
                namespace: "N".into(),
                lines: (1, 20),
                parent: Some(0),
                usings: vec![alias("N.Worker")],
            },
        ];
        let mut ctx = context(Receiver::Value("w".into()));
        ctx.scope = 1;
        ctx.declared_type = Some(DeclaredType {
            name: "W".into(),
            keyword: false,
        });
        let site = Site {
            kind: UsageKind::Call,
            context: &ctx,
            scopes: &scopes,
            project: &NO_PROJECT,
        };
        let r = resolve_site(
            &t,
            Some(&site),
            Some(&owner),
            Some("csharp"),
            &both,
            &[],
            8,
            false,
        );
        assert_eq!(
            rungs(&r),
            vec![
                (
                    "sym:csharp:N#Other.Run()".to_string(),
                    Fate::Kept {
                        rung: Confidence::Ambiguous,
                        rule: Rule::DeclaredReceiver
                    }
                ),
                (
                    "sym:csharp:N#Worker.Run()".to_string(),
                    Fate::Kept {
                        rung: Confidence::RulesBacked,
                        rule: Rule::DeclaredReceiver
                    }
                ),
            ]
        );
    }

    /// `Catalog { static class List { static All(); } Run(); }`, `Derived :
    /// Catalog { Go(); }` in `Orbit.Mcp`, and `Solver { Find(); }` in
    /// `Orbit.Geo` — the shape of the `List<…>` false edge (plan A, Problem
    /// Frame): the repository's only `List` is nested in another type.
    fn nested_table() -> SymbolTable {
        let mut g = Graph::new();
        let catalog = "sym:csharp:Orbit.Mcp#Catalog";
        let list = "sym:csharp:Orbit.Mcp#Catalog.List";
        let derived = "sym:csharp:Orbit.Mcp#Derived";
        let solver = "sym:csharp:Orbit.Geo#Solver";
        for (key, kind, name, signature, owner) in [
            (catalog, "class", "Catalog", "public class Catalog", None),
            (
                list,
                "class",
                "List",
                "public static class List",
                Some(catalog),
            ),
            (
                "sym:csharp:Orbit.Mcp#Catalog.List.All()",
                "method",
                "All",
                "public static string[] All()",
                Some(list),
            ),
            (
                "sym:csharp:Orbit.Mcp#Catalog.Run()",
                "method",
                "Run",
                "public void Run()",
                Some(catalog),
            ),
            (
                derived,
                "class",
                "Derived",
                "public class Derived : Catalog",
                None,
            ),
            (
                "sym:csharp:Orbit.Mcp#Derived.Go()",
                "method",
                "Go",
                "public void Go()",
                Some(derived),
            ),
            (solver, "class", "Solver", "public class Solver", None),
            (
                "sym:csharp:Orbit.Geo#Solver.Find()",
                "method",
                "Find",
                "public void Find()",
                Some(solver),
            ),
        ] {
            declare(&mut g, key, kind, name, (1, 1), signature, owner);
        }
        let prov = Provenance::new("src/B.cs", None, "csharp".into(), Tier::Measured);
        g.add_edge(
            &NodeKey::new(derived),
            &NodeKey::new(catalog),
            Edge::new(EdgeKind::Extends, prov),
        );
        SymbolTable::build(&g, &HashMap::new())
    }

    /// One `using` directive.
    fn using(target: &str, alias: Option<&str>, is_static: bool) -> UsingDirective {
        UsingDirective {
            target: target.into(),
            alias: alias.map(str::to_string),
            is_static,
            is_global: false,
        }
    }

    /// The fate of the nested `Catalog.List` named by its simple name in a
    /// type position (`out List<KeyValuePair<string, int>> r`) inside
    /// `owner`, under `usings` in the file's namespace `Orbit.Geo`.
    fn nested_reference(
        owner: Option<&str>,
        usings: Vec<UsingDirective>,
        qualifier: Option<&str>,
        project: &ProjectWide,
        rights: &[Rule],
    ) -> (Fate, Vec<(Rule, Verdict)>) {
        let t = nested_table();
        let owner = owner.map(NodeKey::new);
        let one = [NodeKey::new("sym:csharp:Orbit.Mcp#Catalog.List")];
        let ctx = SiteContext {
            scope: 0,
            receiver: Receiver::None,
            declared_type: None,
            argument_count: None,
            qualifier: qualifier.map(str::to_string),
        };
        let scopes = vec![WireScope {
            namespace: "Orbit.Geo".into(),
            lines: (1, 20),
            parent: None,
            usings,
        }];
        let site = Site {
            kind: UsageKind::Reference,
            context: &ctx,
            scopes: &scopes,
            project,
        };
        let r = resolve_site(
            &t,
            Some(&site),
            owner.as_ref(),
            Some("csharp"),
            &one,
            rights,
            8,
            false,
        );
        (r.candidates[0].fate, r.candidates[0].trace.clone())
    }

    const NAME_ONLY: Fate = Fate::Kept {
        rung: Confidence::NameOnly,
        rule: Rule::NameMatch,
    };

    /// `List<…>` in a foreign type is not the nested `Catalog.List`: a nested
    /// type's simple name reaches only into its declaring type, the types
    /// nested in it and deriving from it, or through a `using static` of it
    /// or an alias. Disputed without a right, deleted with one.
    #[test]
    fn a_nested_type_is_disputed_where_its_simple_name_cannot_reach_it() {
        let find = Some("sym:csharp:Orbit.Geo#Solver.Find()");
        let (fate, trace) = nested_reference(find, Vec::new(), None, &NO_PROJECT, &[]);
        assert_eq!(
            fate,
            Fate::Kept {
                rung: Confidence::Ambiguous,
                rule: Rule::NestedTypeVisibility
            }
        );
        assert!(
            trace.contains(&(Rule::NestedTypeVisibility, Verdict::Exclude)),
            "{trace:?}"
        );
        let (fate, _) = nested_reference(
            find,
            Vec::new(),
            None,
            &NO_PROJECT,
            &[Rule::NestedTypeVisibility],
        );
        assert_eq!(fate, Fate::Deleted(Rule::NestedTypeVisibility));
    }

    /// Where C# does reach the nested type by its simple name, or where the
    /// data to tell is missing, the rule says nothing and the lone candidate
    /// stays name-only.
    #[test]
    fn a_nested_type_stays_where_its_simple_name_reaches_it() {
        /// What is checked, the owner, the usings, the qualifier, the project.
        type Case = (
            &'static str,
            Option<&'static str>,
            Vec<UsingDirective>,
            Option<&'static str>,
            ProjectWide,
        );
        let find = Some("sym:csharp:Orbit.Geo#Solver.Find()");
        let cases: Vec<Case> = vec![
            (
                "inside the declaring type",
                Some("sym:csharp:Orbit.Mcp#Catalog.Run()"),
                Vec::new(),
                None,
                NO_PROJECT.clone(),
            ),
            (
                "inside the nested type itself",
                Some("sym:csharp:Orbit.Mcp#Catalog.List.All()"),
                Vec::new(),
                None,
                NO_PROJECT.clone(),
            ),
            (
                "in a type deriving from the declaring type",
                Some("sym:csharp:Orbit.Mcp#Derived.Go()"),
                Vec::new(),
                None,
                NO_PROJECT.clone(),
            ),
            (
                "under `using static` of the declaring type",
                find,
                vec![using("Orbit.Mcp.Catalog", None, true)],
                None,
                NO_PROJECT.clone(),
            ),
            (
                "under an alias of the name",
                find,
                vec![using("Orbit.Mcp.Catalog.List", Some("List"), false)],
                None,
                NO_PROJECT.clone(),
            ),
            (
                "under a project-wide `global using static`",
                find,
                Vec::new(),
                None,
                ProjectWide::Known(Imports {
                    statics: ["Orbit.Mcp.Catalog".to_string()].into_iter().collect(),
                    ..Imports::default()
                }),
            ),
            (
                "a qualified spelling",
                find,
                Vec::new(),
                Some("Catalog"),
                NO_PROJECT.clone(),
            ),
            (
                "an unmapped site",
                None,
                Vec::new(),
                None,
                NO_PROJECT.clone(),
            ),
            (
                "a project whose usings cannot be read",
                find,
                Vec::new(),
                None,
                ProjectWide::Unknown,
            ),
        ];
        for (what, owner, usings, qualifier, project) in cases {
            let (fate, trace) = nested_reference(
                owner,
                usings,
                qualifier,
                &project,
                &[Rule::NestedTypeVisibility],
            );
            assert_eq!(fate, NAME_ONLY, "{what}: {trace:?}");
        }
    }

    /// `List.All()` names the nested type as a static receiver: inside the
    /// declaring type `static-receiver` backs its member, in a foreign type
    /// the same lookup cannot reach the receiver, and the member is disputed.
    #[test]
    fn a_static_receiver_spelling_a_nested_type_is_held_to_the_same_lookup() {
        let t = nested_table();
        let one = [NodeKey::new("sym:csharp:Orbit.Mcp#Catalog.List.All()")];
        let ctx = context(Receiver::Name("List".into()));
        let scopes = vec![WireScope {
            namespace: "Orbit.Geo".into(),
            lines: (1, 20),
            parent: None,
            usings: Vec::new(),
        }];
        let site = Site {
            kind: UsageKind::Call,
            context: &ctx,
            scopes: &scopes,
            project: &NO_PROJECT,
        };
        let fate_in = |owner: &str| {
            let owner = NodeKey::new(owner);
            let r = resolve_site(
                &t,
                Some(&site),
                Some(&owner),
                Some("csharp"),
                &one,
                &[],
                8,
                false,
            );
            r.candidates[0].fate
        };
        assert_eq!(
            fate_in("sym:csharp:Orbit.Mcp#Catalog.Run()"),
            Fate::Kept {
                rung: Confidence::RulesBacked,
                rule: Rule::StaticReceiver
            }
        );
        assert_eq!(
            fate_in("sym:csharp:Orbit.Geo#Solver.Find()"),
            Fate::Kept {
                rung: Confidence::Ambiguous,
                rule: Rule::NestedTypeVisibility
            }
        );
    }

    /// `StringExtensions.IndexOfAny(this string, char[], int)`,
    /// `TextDocument.IndexOfAny(char[], int)` (an instance method), and
    /// `Node` with `NodeExtensions.Describe(this Node)`.
    fn extension_table() -> SymbolTable {
        let mut g = Graph::new();
        let ext = "sym:csharp:Orbit.Text#StringExtensions";
        let doc = "sym:csharp:Orbit.Editor#TextDocument";
        let node = "sym:csharp:Orbit.Geo#Node";
        let node_ext = "sym:csharp:Orbit.Geo#NodeExtensions";
        for (key, kind, name, signature, owner) in [
            (
                ext,
                "class",
                "StringExtensions",
                "public static class StringExtensions",
                None,
            ),
            (
                "sym:csharp:Orbit.Text#StringExtensions.IndexOfAny(string,char[],int)",
                "method",
                "IndexOfAny",
                "public static int IndexOfAny(this string s, char[] chars, int start)",
                Some(ext),
            ),
            (
                doc,
                "class",
                "TextDocument",
                "public class TextDocument",
                None,
            ),
            (
                "sym:csharp:Orbit.Editor#TextDocument.IndexOfAny(char[],int)",
                "method",
                "IndexOfAny",
                "public int IndexOfAny(char[] chars, int start)",
                Some(doc),
            ),
            (node, "class", "Node", "public class Node", None),
            (
                node_ext,
                "class",
                "NodeExtensions",
                "public static class NodeExtensions",
                None,
            ),
            (
                "sym:csharp:Orbit.Geo#NodeExtensions.Describe(Node)",
                "method",
                "Describe",
                "public static string Describe(this Node n)",
                Some(node_ext),
            ),
        ] {
            declare(&mut g, key, kind, name, (1, 1), signature, owner);
        }
        SymbolTable::build(&g, &HashMap::new())
    }

    /// `declared-receiver` revision 3: `s.IndexOfAny(chars, 1)` on a `string`
    /// is `string.IndexOfAny(char[], int)`, an instance member no repository
    /// shows, which C# prefers to any extension. The repository's `this
    /// string` overload is left alone — a lone candidate is name-only, not
    /// backed; beside another candidate the rule disputed, both stay
    /// ambiguous. An extension on a repository type is still backed.
    #[test]
    fn an_extension_on_an_external_receiver_type_is_never_backed() {
        let t = extension_table();
        let owner = NodeKey::new("sym:csharp:Orbit.Geo#Node");
        let ext =
            NodeKey::new("sym:csharp:Orbit.Text#StringExtensions.IndexOfAny(string,char[],int)");
        let doc = NodeKey::new("sym:csharp:Orbit.Editor#TextDocument.IndexOfAny(char[],int)");
        let scopes = scopes();
        let resolve = |declared: &str, keyword: bool, arguments: u32, candidates: &[NodeKey]| {
            let mut ctx = context(Receiver::Value("s".into()));
            ctx.declared_type = Some(DeclaredType {
                name: declared.into(),
                keyword,
            });
            ctx.argument_count = Some(arguments);
            let site = Site {
                kind: UsageKind::Call,
                context: &ctx,
                scopes: &scopes,
                project: &NO_PROJECT,
            };
            let r = resolve_site(
                &t,
                Some(&site),
                Some(&owner),
                Some("csharp"),
                candidates,
                &[],
                8,
                false,
            );
            rungs(&r)
        };

        for (declared, keyword) in [("string", true), ("String", false)] {
            assert_eq!(
                resolve(declared, keyword, 2, std::slice::from_ref(&ext)),
                vec![(ext.as_str().to_string(), NAME_ONLY)],
                "{declared}"
            );
        }
        assert_eq!(
            resolve("string", true, 2, &[doc.clone(), ext.clone()]),
            vec![
                (
                    doc.as_str().to_string(),
                    Fate::Kept {
                        rung: Confidence::Ambiguous,
                        rule: Rule::DeclaredReceiver
                    }
                ),
                (
                    ext.as_str().to_string(),
                    Fate::Kept {
                        rung: Confidence::Ambiguous,
                        rule: Rule::NameMatch
                    }
                ),
            ]
        );

        let describe = NodeKey::new("sym:csharp:Orbit.Geo#NodeExtensions.Describe(Node)");
        assert_eq!(
            resolve("Node", false, 0, std::slice::from_ref(&describe)),
            vec![(
                describe.as_str().to_string(),
                Fate::Kept {
                    rung: Confidence::RulesBacked,
                    rule: Rule::DeclaredReceiver
                }
            )]
        );
    }
}
