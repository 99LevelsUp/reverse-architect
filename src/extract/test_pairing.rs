//! The test-pairing extractor (U7): a deterministic name/tree heuristic that
//! pairs source files with their tests by convention, plus an honest per-
//! language coverage fact.
//!
//! Honesty discipline (E9/R18): the heuristic can *confirm* a convention-
//! matching test exists, but it can never *prove* one absent — a test could
//! live under a name or layout no convention covers. So every source carries
//! either [`HAS_TEST`] or [`NO_TEST`], and the words "untested" / "has no test"
//! appear nowhere. `NO_TEST` reads "test by convention not found", not a
//! false claim of absence.
//!
//! The pairing is purely name/tree based (KTD7 determinism): a source and a
//! test are paired when they share a language and a *stem* — the base name with
//! the language's test affix stripped (`foo_test.rs`→`foo`, `test_foo.py`→`foo`,
//! `Foo.test.ts`→`Foo`, `FooTests.cs`→`Foo`). No file contents are read.
//!
//! Coverage answers E9's question — of the sources of each grammar-supported
//! language, how many carry tree-sitter structural facts vs. only metrics+filetree —
//! by checking which source files a U6 Symbol node is attributed to. It is a
//! per-language displayed fact on the root module node. Scope is the five
//! grammar-supported languages: only those *can* have structural facts, so the
//! structural-vs-not contrast is only meaningful there.

use super::ids;
use super::languages::Languages;
use super::{ExtractError, Extractor, Repo};
use crate::graph::{
    ExtractorId, Fact, FactKind, FactValue, Graph, NodeKey, NodeKind, Provenance, Tier,
};
use std::collections::{BTreeMap, BTreeSet};

/// Exact wording for a source with a convention-matching test. Asserted in
/// tests — the phrasing is a contract (E9/R18), not cosmetic.
pub const HAS_TEST: &str = "has a test by convention";
/// Exact wording for a source with no convention-matching test. Never claims
/// absence ("untested"/"has no test") — only that the convention did not match.
pub const NO_TEST: &str = "test by convention not found";

/// The paired-test statement for a source with one or more matching tests.
///
/// Starts with [`HAS_TEST`] verbatim, then names the test the reader should
/// open. Naming it is the point: "a test exists somewhere" leaves the reader
/// with the question they arrived with, and the pairing already knows the
/// answer.
///
/// Several tests can share one stem — `FooTests.cs` beside `FooTest.cs`, or the
/// same name under two directories. The first in sorted order is named and the
/// rest are counted, so the statement stays one line at any repository size
/// (R17) and never grows without saying that it was cut.
fn pairing_wording(tests: &BTreeSet<String>) -> String {
    let mut it = tests.iter();
    let Some(first) = it.next() else {
        // Unreachable: an entry exists only because a test was inserted into it.
        return HAS_TEST.to_string();
    };
    match tests.len() - 1 {
        0 => format!("{HAS_TEST}: `{first}`"),
        1 => format!("{HAS_TEST}: `{first}`, and 1 more sharing its name"),
        more => format!("{HAS_TEST}: `{first}`, and {more} more sharing its name"),
    }
}

/// The root module node key — where the coverage facts live.
const ROOT: &str = ".";

/// Trust-rank 25 (KTD-B8): same tier as metrics — both are file-level Tier 0
/// sources above filetree.
const TRUST_RANK: u8 = 25;

pub struct TestPairing {
    /// The run's language registry (U4) — see [`super::metrics::Metrics`] for
    /// why it rides on the extractor rather than on `run`.
    pub languages: Languages,
}

impl Extractor for TestPairing {
    fn id(&self) -> ExtractorId {
        ids::test_pairing()
    }

    fn emitted_kinds(&self) -> &'static [FactKind] {
        &[FactKind::TestByConvention, FactKind::Coverage]
    }

    fn trust_rank(&self) -> u8 {
        TRUST_RANK
    }

    fn run(
        &self,
        repo: &Repo,
        graph: &mut Graph,
        _progress: &dyn crate::progress::Progress,
    ) -> Result<(), ExtractError> {
        let tracked = repo.tracked_files()?;

        // Classify every tracked file. Test files contribute their stem to a
        // per-language index; non-test source files await pairing.
        // stem -> the test files that carry it. A map rather than a set because
        // the card has to be able to say WHICH test, not merely that one exists:
        // "has a test by convention" without a path leaves the reader with the
        // same question they arrived with.
        let mut test_stems: BTreeMap<(Lang, String), BTreeSet<String>> = BTreeMap::new();
        let mut sources: Vec<(String, Lang, String)> = Vec::new(); // (key, lang, stem)
        for file in &tracked {
            // The registry decides whether this is a language at all; `Lang`
            // then decides whether the core knows a TEST CONVENTION for it. A
            // registered language with no convention falls out here and is left
            // unpaired, which reports honestly rather than guessing at a naming
            // scheme the core has never seen (U4).
            let Some(lang) = self.languages.name_for(file).and_then(Lang::from_name) else {
                continue;
            };
            match classify(file, lang) {
                Some(Role::Test(stem)) => {
                    test_stems
                        .entry((lang, stem))
                        .or_default()
                        .insert(file.clone());
                }
                Some(Role::Source(stem)) => sources.push((file.clone(), lang, stem)),
                // Module plumbing (`mod.rs`, `__init__.py`) — neither a test nor
                // a meaningful pairing target; skip entirely.
                None => {}
            }
        }

        // Which source files carry a U6 structural (tree-sitter) fact: a Symbol
        // node is attributed, via its provenance, to the source file it came
        // from. Collect those source paths so coverage reflects real extraction.
        let structural = structural_source_files(graph);

        // Per-source pairing fact + per-language coverage tallies. `sources` is
        // built in tracked (sorted) order, so fact emission order is stable.
        let mut totals: BTreeMap<Lang, (usize, usize)> = BTreeMap::new(); // lang -> (total, structural)
        for (file, lang, stem) in &sources {
            let paired = test_stems.get(&(*lang, stem.clone()));
            let key = NodeKey::new(file.clone());
            // Land only on the File node filetree created (mirrors metrics).
            match graph.node(&key) {
                Some(node) if node.kind == NodeKind::File => {}
                _ => continue,
            }
            let wording = match paired {
                Some(tests) => pairing_wording(tests),
                None => NO_TEST.to_string(),
            };
            graph.add_fact(
                &key,
                Fact::new(
                    FactKind::TestByConvention,
                    FactValue::Text(wording),
                    Provenance::new(file.clone(), None, ids::test_pairing(), Tier::Derived),
                ),
            );

            let entry = totals.entry(*lang).or_insert((0, 0));
            entry.0 += 1;
            if structural.contains(file.as_str()) {
                entry.1 += 1;
            }
        }

        // Coverage: one displayed fact per language that has sources, on the
        // root module. `totals` iterates in Lang order (BTreeMap) → deterministic.
        let root = NodeKey::new(ROOT);
        if graph.node(&root).is_some() {
            for (lang, (total, structural_count)) in &totals {
                let only_basic = total - structural_count;
                let text = format!(
                    "{}: {} of {} source file{} carry tree-sitter structural facts; \
                     {} {} only metrics + file tree",
                    lang.name(),
                    structural_count,
                    total,
                    plural(*total),
                    only_basic,
                    if only_basic == 1 { "has" } else { "have" },
                );
                graph.add_fact(
                    &root,
                    Fact::new(
                        FactKind::Coverage,
                        FactValue::Text(text),
                        Provenance::new(ROOT, None, ids::test_pairing(), Tier::Derived),
                    ),
                );
            }
        }

        Ok(())
    }
}

/// Source files that carry a tree-sitter structural fact — the set of files a
/// U6 Symbol node is attributed to (via the symbol's fact provenance). Keyed by
/// the same `/`-normalized path metrics and filetree use.
fn structural_source_files(graph: &Graph) -> BTreeSet<String> {
    let mut set = BTreeSet::new();
    for node in graph.nodes_sorted() {
        if node.kind != NodeKind::Symbol {
            continue;
        }
        // All facts on a Symbol share its source file in their provenance.
        if let Some(fact) = node.facts.first() {
            set.insert(fact.provenance.file.clone());
        }
    }
    set
}

/// The grammar-supported languages, aligned with the tree-sitter extractor.
/// `.ts` and `.tsx` fold into one TypeScript bucket (shared test conventions).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Lang {
    Rust,
    TypeScript,
    Python,
    CSharp,
    Go,
    JavaScript,
    Java,
    Sql,
}

impl Lang {
    /// The convention set for a registry language name, or `None` when the core
    /// knows no test-naming convention for it.
    ///
    /// This replaced a second `ext -> language` table (U4). It keys on the
    /// registry's NAME rather than on the extension, which is what folds `.ts`
    /// and `.tsx` into one convention set: the registry already answers
    /// "typescript" for both, so this side never has to know they are two
    /// extensions. `name()` maps back, and
    /// `every_registry_name_round_trips_through_the_convention_table` holds the
    /// two directions together.
    pub(crate) fn from_name(name: &str) -> Option<Lang> {
        match name {
            "rust" => Some(Lang::Rust),
            "typescript" => Some(Lang::TypeScript),
            "python" => Some(Lang::Python),
            "csharp" => Some(Lang::CSharp),
            "go" => Some(Lang::Go),
            "javascript" => Some(Lang::JavaScript),
            "java" => Some(Lang::Java),
            "sql" => Some(Lang::Sql),
            _ => None,
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Lang::Rust => "rust",
            Lang::TypeScript => "typescript",
            Lang::Python => "python",
            Lang::CSharp => "csharp",
            Lang::Go => "go",
            Lang::JavaScript => "javascript",
            Lang::Java => "java",
            Lang::Sql => "sql",
        }
    }
}

/// A tracked file is either a test (contributing its stem) or a source awaiting
/// pairing. The carried `String` is the shared stem used to match the two.
enum Role {
    Test(String),
    Source(String),
}

/// Classify a file within its language by naming/tree convention. The returned
/// stem is what a source and its test share once the test affix is stripped.
/// `None` means "skip" — module plumbing whose bare name (`mod.rs`,
/// `__init__.py`) is not a meaningful test target, so a `tests/` copy of it must
/// not register as a test stem and produce a false "has a test" on every
/// same-named module elsewhere.
fn classify(path: &str, lang: Lang) -> Option<Role> {
    let base = basename(path);
    let in_tests_dir = path.split('/').any(|c| c == "tests");
    Some(match lang {
        Lang::Rust => {
            if let Some(stem) = base.strip_suffix("_test.rs") {
                Role::Test(stem.to_string())
            } else if base == "mod.rs" {
                return None; // module plumbing, never a pairing target
            } else if in_tests_dir {
                Role::Test(strip_ext(base, "rs").to_string())
            } else {
                Role::Source(strip_ext(base, "rs").to_string())
            }
        }
        Lang::TypeScript => {
            // `foo.test.ts` / `foo.spec.ts` (and .tsx) are tests of `foo`.
            if let Some(stem) =
                strip_double(base, &[".test.ts", ".test.tsx", ".spec.ts", ".spec.tsx"])
            {
                Role::Test(stem)
            } else {
                let stem = base
                    .strip_suffix(".tsx")
                    .or_else(|| base.strip_suffix(".ts"))
                    .unwrap_or(base);
                Role::Source(stem.to_string())
            }
        }
        Lang::Python => {
            if let Some(stem) = base
                .strip_prefix("test_")
                .and_then(|s| s.strip_suffix(".py"))
            {
                Role::Test(stem.to_string())
            } else if let Some(stem) = base.strip_suffix("_test.py") {
                Role::Test(stem.to_string())
            } else if base == "__init__.py" {
                return None; // package plumbing, never a pairing target
            } else if in_tests_dir {
                Role::Test(strip_ext(base, "py").to_string())
            } else {
                Role::Source(strip_ext(base, "py").to_string())
            }
        }
        Lang::CSharp => {
            // `Foo.Tests.cs` or `FooTests.cs` are tests of `Foo`.
            if let Some(stem) = base.strip_suffix(".Tests.cs") {
                Role::Test(stem.to_string())
            } else if let Some(stem) = base.strip_suffix("Tests.cs") {
                Role::Test(stem.to_string())
            } else {
                Role::Source(strip_ext(base, "cs").to_string())
            }
        }
        Lang::JavaScript => {
            // `foo.test.js` / `foo.spec.jsx` (and `.mjs`/`.cjs`) test `foo`.
            let exts = ["js", "jsx", "mjs", "cjs"];
            let test = exts.iter().find_map(|ext| {
                base.strip_suffix(&format!(".test.{ext}"))
                    .or_else(|| base.strip_suffix(&format!(".spec.{ext}")))
            });
            match test {
                Some(stem) => Role::Test(stem.to_string()),
                None => {
                    let stem = exts
                        .iter()
                        .find_map(|ext| base.strip_suffix(&format!(".{ext}")))
                        .unwrap_or(base);
                    Role::Source(stem.to_string())
                }
            }
        }
        Lang::Java => {
            // `FooTest.java`, `FooTests.java`, `FooIT.java` test `Foo`.
            let stem = strip_ext(base, "java");
            match ["Tests", "Test", "IT"]
                .iter()
                .find_map(|suffix| stem.strip_suffix(suffix).filter(|s| !s.is_empty()))
            {
                Some(tested) => Role::Test(tested.to_string()),
                None => Role::Source(stem.to_string()),
            }
        }
        Lang::Sql => {
            // tSQLt and pgTAP: `test_foo.sql`, `foo_test.sql`, or a script
            // under `tests/`.
            if let Some(stem) = base
                .strip_prefix("test_")
                .and_then(|s| s.strip_suffix(".sql"))
            {
                Role::Test(stem.to_string())
            } else if let Some(stem) = base.strip_suffix("_test.sql") {
                Role::Test(stem.to_string())
            } else if in_tests_dir {
                Role::Test(strip_ext(base, "sql").to_string())
            } else {
                Role::Source(strip_ext(base, "sql").to_string())
            }
        }
        Lang::Go => {
            if let Some(stem) = base.strip_suffix("_test.go") {
                Role::Test(stem.to_string())
            } else {
                Role::Source(strip_ext(base, "go").to_string())
            }
        }
    })
}

/// The file name after the last `/`.
fn basename(path: &str) -> &str {
    path.rsplit_once('/').map(|(_, b)| b).unwrap_or(path)
}

/// Strip a single-dot extension, returning the stem (`foo.rs`→`foo`).
fn strip_ext<'a>(base: &'a str, ext: &str) -> &'a str {
    base.strip_suffix(&format!(".{ext}")).unwrap_or(base)
}

/// The stem if `base` ends with one of the multi-part `suffixes`.
fn strip_double(base: &str, suffixes: &[&str]) -> Option<String> {
    suffixes
        .iter()
        .find_map(|s| base.strip_suffix(s))
        .map(str::to_string)
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

#[cfg(test)]
mod tests {
    /// `Lang::from_name` and `Lang::name` are the two directions of one mapping,
    /// and coverage reads one while pairing reads the other. If they drift, a
    /// language silently stops being paired while still being reported — the
    /// exact failure the registry was introduced to make impossible.
    #[test]
    fn every_registry_name_round_trips_through_the_convention_table() {
        let registry = crate::extract::languages::Languages::bundled();
        // C and C++ (plan B, U4) have no convention set on purpose: a language
        // the core knows no test naming for is left unpaired rather than
        // guessed at, and only the host lexer reads them.
        for ext in registry
            .extensions()
            .filter(|ext| !registry.is_lexed_only(&format!("a.{ext}")))
        {
            let name = registry.name_for(&format!("a.{ext}")).unwrap();
            let lang = super::Lang::from_name(name)
                .unwrap_or_else(|| panic!("`{name}` is reported but has no convention set"));
            assert_eq!(
                lang.name(),
                name,
                "`{name}` maps to a convention set that calls itself something else"
            );
        }
        assert_eq!(super::Lang::from_name("otevrenost"), None);
    }

    use super::*;

    fn stem_of(role: Option<Role>) -> (bool, String) {
        match role.expect("expected a classification, got skip") {
            Role::Test(s) => (true, s),
            Role::Source(s) => (false, s),
        }
    }

    #[test]
    fn rust_conventions() {
        assert_eq!(
            stem_of(classify("src/calc.rs", Lang::Rust)),
            (false, "calc".into())
        );
        assert_eq!(
            stem_of(classify("src/calc_test.rs", Lang::Rust)),
            (true, "calc".into())
        );
        assert_eq!(
            stem_of(classify("tests/calc.rs", Lang::Rust)),
            (true, "calc".into())
        );
    }

    #[test]
    fn module_plumbing_is_skipped_not_a_test_stem() {
        // `tests/common/mod.rs` must NOT register test-stem `mod` — otherwise
        // every `src/**/mod.rs` would falsely read "has a test by convention".
        assert!(classify("tests/common/mod.rs", Lang::Rust).is_none());
        assert!(classify("src/extract/mod.rs", Lang::Rust).is_none());
        assert!(classify("pkg/tests/__init__.py", Lang::Python).is_none());
    }

    #[test]
    fn typescript_conventions() {
        assert_eq!(
            stem_of(classify("src/a.ts", Lang::TypeScript)),
            (false, "a".into())
        );
        assert_eq!(
            stem_of(classify("src/a.test.ts", Lang::TypeScript)),
            (true, "a".into())
        );
        assert_eq!(
            stem_of(classify("src/a.spec.tsx", Lang::TypeScript)),
            (true, "a".into())
        );
    }

    #[test]
    fn python_csharp_go_conventions() {
        assert_eq!(
            stem_of(classify("pkg/m.py", Lang::Python)),
            (false, "m".into())
        );
        assert_eq!(
            stem_of(classify("pkg/test_m.py", Lang::Python)),
            (true, "m".into())
        );
        assert_eq!(
            stem_of(classify("A/Foo.cs", Lang::CSharp)),
            (false, "Foo".into())
        );
        assert_eq!(
            stem_of(classify("A/FooTests.cs", Lang::CSharp)),
            (true, "Foo".into())
        );
        assert_eq!(
            stem_of(classify("A/Foo.Tests.cs", Lang::CSharp)),
            (true, "Foo".into())
        );
        assert_eq!(stem_of(classify("x/s.go", Lang::Go)), (false, "s".into()));
        assert_eq!(
            stem_of(classify("x/s_test.go", Lang::Go)),
            (true, "s".into())
        );
    }

    #[test]
    fn wording_never_claims_absence() {
        // The contract: the "no test" wording must not read as a proof of
        // absence (E9/R18).
        assert!(!NO_TEST.contains("untested"));
        assert!(!NO_TEST.contains("no test"));
        assert!(NO_TEST.contains("not found"));
    }
}
