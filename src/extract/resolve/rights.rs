//! The rights to delete (plan A, KTD11): which rule may remove a candidate
//! outright rather than only dispute it, and for which inputs.
//!
//! A right is earned by measurement against an index (R10, R11), and the
//! measurement vouches for the rule TOGETHER with the data a plugin fed it. A
//! record therefore names the rule and its revision, the language, the plugin
//! component (id and SHA-256) and the contract version it ran under, and the
//! committed report it came from. At run time a right applies only to a site
//! of the same language from the same component, with the rule at the same
//! revision; anywhere else the rule only disputes. A rebuilt `csharp.wasm`, or
//! context arriving for another language, therefore never inherits a right
//! without a new measurement.
//!
//! The table is code, not a data file beside the target repository: output
//! must not depend on a file that can go stale, and a change of rights is
//! reviewed as a change of code. The report's own digest is not checked at
//! run time — the report does not ship beside the binary — but by a test over
//! `docs/measurements/` ([`crate::measure::check_rights`], run by
//! `tests/measure.rs`): every record names a committed report, the report
//! hashes to the recorded digest, and in it the rule removed nothing the index
//! confirmed and was proposed (U11, AE5).
//!
//! ## Granting a right
//!
//! `reverse-architect measure` writes a report and proposes rights; it never
//! writes this table. Granting is a manual change to [`granted_records`]: copy
//! the report into `docs/measurements/`, add one [`DeleteRight`] per proposed
//! rule with the report's language, component, component digest, contract and
//! the rule's revision, and the report's path and SHA-256 — committed together,
//! so review sees the measurement beside the right it earns
//! (`docs/measurements/README.md` walks through it).

use super::rules::{may_delete, revision_of};
use crate::graph::Rule;
use crate::plugin::host::Contract;
use serde::Serialize;

/// One granted right to delete (KTD11).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct DeleteRight {
    /// The rule the right is granted to.
    pub rule: Rule,
    /// The rule's revision the measurement ran against.
    pub revision: u32,
    /// The language the measurement covered, as coverage names it (`csharp`).
    pub language: String,
    /// The id of the plugin component that supplied the site context.
    pub component: String,
    /// That component's SHA-256, hex.
    pub component_sha256: String,
    /// The plugin contract version it ran under (`0.2`).
    pub contract: String,
    /// The committed measurement report, relative to the repository root
    /// (`docs/measurements/…`).
    pub report: String,
    /// The report's SHA-256, hex.
    pub report_sha256: String,
}

/// The rights a run applies (KTD11).
///
/// [`RightsTable::granted`] is the table this build ships, and what every CLI
/// run uses. A library caller can hand a run another table through
/// [`crate::config::Config::rights`] — the regression fixtures need a right
/// granted to see a deletion happen — but no flag reaches it: an operator
/// cannot grant a right without a measurement and a code change.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RightsTable {
    rights: Vec<DeleteRight>,
}

/// The rights this build grants (U14).
///
/// A function rather than a `const` slice: a record's fields are owned
/// strings, which a constant cannot hold with any content, and a record must be
/// addable by writing it here and nothing else.
///
/// - `role-mismatch`, C#: measured on the reference C# repository (commit <ref-revision>, 149
///   projects indexed by `ra index`) — 9 310 sites judged and refuted by the
///   index, none of them confirmed, 2.5 % unmapped
///   (`docs/measurements/2026-10-03-csharp-reference.json`). Re-measured with the
///   components rebuilt from current sources, with the same counts
///   (`docs/measurements/2026-10-09-csharp-reference.json`). No other rule is
///   proposed: `declared-receiver`, `static-receiver` and `nested-type-visibility`
///   would remove edges the index confirmed.
fn granted_records() -> Vec<DeleteRight> {
    vec![DeleteRight {
        rule: Rule::RoleMismatch,
        revision: 1,
        language: "csharp".into(),
        component: "csharp".into(),
        component_sha256: "1198ab3537ba5d97c9cecc2bf0598676643fab56a58a76224334ee6aa3643ae9".into(),
        contract: "0.2".into(),
        report: "docs/measurements/2026-10-09-csharp-reference.json".into(),
        report_sha256: "ece880e73576555bfff508fb8081d8b3f5d8fe65bdc3c766ac41c1709790b5fb".into(),
    }]
}

impl RightsTable {
    /// The table this build ships.
    pub fn granted() -> RightsTable {
        RightsTable::new(granted_records())
    }

    /// A table of exactly these rights, in a canonical order so two tables
    /// listing the same rights are one table.
    pub fn new(mut rights: Vec<DeleteRight>) -> RightsTable {
        rights.sort();
        rights.dedup();
        RightsTable { rights }
    }

    pub fn rights(&self) -> &[DeleteRight] {
        &self.rights
    }

    /// A digest of the whole table, for the run fingerprint (KTD15): granting
    /// or withdrawing a right changes what a cached record would hold, so it
    /// must invalidate the cache like a changed file does.
    pub fn fingerprint(&self) -> String {
        let text =
            serde_json::to_string(&self.rights).expect("a rights table is plain serializable data");
        crate::graph::fnv1a64_hex(&text)
    }

    /// The rules holding a right that applies to sites from this origin, in
    /// catalog order. A right recorded for a rule the catalog says may never
    /// delete is ignored: such a rule's negative claim is no proof, whatever a
    /// measurement found (KTD6).
    pub(crate) fn active_for(&self, origin: &SiteOrigin) -> Vec<Rule> {
        let mut out: Vec<Rule> = self
            .rights
            .iter()
            .filter(|r| {
                r.language == origin.language
                    && r.component == origin.component
                    && r.component_sha256 == origin.component_sha256
                    && r.contract == origin.contract.word()
                    && revision_of(r.rule) == Some(r.revision)
                    && may_delete(r.rule)
            })
            .map(|r| r.rule)
            .collect();
        out.sort();
        out.dedup();
        out
    }
}

/// The revision of `rule` this build's catalog holds — what a right must be
/// recorded against to apply (KTD11). `None` for a rule the catalog does not
/// list (`name-match`).
pub fn current_revision(rule: Rule) -> Option<u32> {
    revision_of(rule)
}

/// Where a file's usage sites came from: what a right is matched against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SiteOrigin {
    pub language: String,
    pub component: String,
    pub component_sha256: String,
    pub contract: Contract,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origin() -> SiteOrigin {
        SiteOrigin {
            language: "csharp".into(),
            component: "csharp".into(),
            component_sha256: "ab".repeat(32),
            contract: Contract::V0_2,
        }
    }

    fn right(rule: Rule, revision: u32) -> DeleteRight {
        DeleteRight {
            rule,
            revision,
            language: "csharp".into(),
            component: "csharp".into(),
            component_sha256: "ab".repeat(32),
            contract: "0.2".into(),
            report: "docs/measurements/x.json".into(),
            report_sha256: "cd".repeat(32),
        }
    }

    /// The shipped table holds exactly the rights U14 granted: `role-mismatch`
    /// for C#, at the revision it was measured at. A new grant changes this
    /// list on purpose, beside its report (the report itself is checked by
    /// `tests/measure.rs`).
    #[test]
    fn the_shipped_table_grants_exactly_the_measured_rights() {
        let table = RightsTable::granted();
        let granted: Vec<(Rule, u32, &str)> = table
            .rights()
            .iter()
            .map(|r| (r.rule, r.revision, r.language.as_str()))
            .collect();
        assert_eq!(granted, vec![(Rule::RoleMismatch, 1, "csharp")]);
        assert_eq!(revision_of(Rule::RoleMismatch), Some(1));
    }

    /// A right holds for the exact inputs it was measured on and nowhere else:
    /// another revision, another component build, another component, another
    /// language or another contract leaves the rule disputing only.
    #[test]
    fn a_right_applies_only_to_its_language_component_contract_and_revision() {
        let revision = revision_of(Rule::DeclaredReceiver).unwrap();
        let table = RightsTable::new(vec![right(Rule::DeclaredReceiver, revision)]);
        assert_eq!(table.active_for(&origin()), vec![Rule::DeclaredReceiver]);

        let stale = RightsTable::new(vec![right(Rule::DeclaredReceiver, revision + 1)]);
        assert!(stale.active_for(&origin()).is_empty());

        let rebuilt = SiteOrigin {
            component_sha256: "ef".repeat(32),
            ..origin()
        };
        assert!(table.active_for(&rebuilt).is_empty());
        let other_component = SiteOrigin {
            component: "my-csharp".into(),
            ..origin()
        };
        assert!(table.active_for(&other_component).is_empty());
        let other_language = SiteOrigin {
            language: "java".into(),
            ..origin()
        };
        assert!(table.active_for(&other_language).is_empty());
        let old_contract = SiteOrigin {
            contract: Contract::V0_1,
            ..origin()
        };
        assert!(table.active_for(&old_contract).is_empty());
    }

    /// The rules the catalog says may never delete get no right from a
    /// record, whatever a measurement found (KTD6): their negative claim is
    /// no proof.
    #[test]
    fn a_right_for_a_rule_that_may_never_delete_is_ignored() {
        for rule in [
            Rule::Arity,
            Rule::NamespaceVisibility,
            Rule::ImplicitThis,
            Rule::BaseSpelling,
        ] {
            let table = RightsTable::new(vec![right(rule, revision_of(rule).unwrap())]);
            assert!(table.active_for(&origin()).is_empty(), "{rule:?}");
        }
    }

    #[test]
    fn granting_a_right_changes_the_fingerprint() {
        let revision = revision_of(Rule::StaticReceiver).unwrap();
        assert_ne!(
            RightsTable::granted().fingerprint(),
            RightsTable::new(vec![right(Rule::StaticReceiver, revision)]).fingerprint()
        );
    }
}
