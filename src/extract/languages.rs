//! Which language a file belongs to, resolved at run time (U4, R26).
//!
//! Language identity was a closed `enum` written out four times over:
//! `test_pairing::Lang` names languages for the coverage artifact,
//! `treesitter::Language` picks the grammar, `external::Lang` tags `ext:` and
//! `sym:` keys, and tokei's `LanguageType` decides how lines are counted. Each
//! kept its own `ext → language` match, so a language nobody compiled in was
//! `None` in all four: coverage reported `language: null` and "no grammar covers
//! this file type" about a file a plugin had just parsed — a false statement
//! rather than a gap (R26).
//!
//! This is the table those consumers ask. It is keyed by extension and answers
//! two questions:
//!
//! - **What is this language called** in the coverage artifact.
//! - **How is its LOC counted**, so a registered language is never silently
//!   zero-LOC (the fourth table, which the plan nearly missed).
//!
//! # `.ts` and `.tsx`
//!
//! Two entries, one reporting name — and that is the whole resolution of a
//! disagreement between the existing tables. `test_pairing` folded `.tsx` into
//! TypeScript because they share test conventions; `treesitter` keeps them apart
//! because they need different grammars (`LANGUAGE_TSX` vs
//! `LANGUAGE_TYPESCRIPT`). Both were right about different things. One entry per
//! extension carrying both facts gives `.tsx` exactly one answer and leaves
//! nothing to drift — rather than picking a winner and making the other consumer
//! wrong.
//!
//! # Extension matching is case-sensitive, deliberately
//!
//! All four existing tables match the raw extension exactly, and `metrics`
//! classifies straight off `Path::extension()`. Folding case here would give
//! `A.RS` a `Loc` fact and a language it does not have today — a behavior change
//! smuggled into a unit whose gate is byte-identity. Whether `.RS` should be
//! Rust is a real question; it is not this unit's question.
//!
//! # What this does NOT carry
//!
//! Test-pairing conventions. `test_pairing::classify` is a per-language
//! *behavior* — Rust's `_test.rs`, Python's `test_` prefix, C#'s `.Tests.cs` —
//! not a datum, so it cannot become a field here. A registered language the core
//! has no convention for is left unpaired, which reports honestly rather than
//! guessing. Letting a plugin state its own convention is a later unit's
//! problem, not a gap this one papers over.

use std::collections::{BTreeMap, BTreeSet};
use tokei::LanguageType;

/// One language, as the artifacts see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageEntry {
    /// The name coverage reports. Two extensions may share one — see the module
    /// doc on `.ts`/`.tsx`.
    pub name: String,
    /// How `metrics` counts this language's lines.
    ///
    /// `None` means "a known language whose lines this build cannot count", and
    /// `metrics` emits no `Loc` fact **and does not fall back to tokei** — the
    /// entry is the answer. That is distinct from the extension being absent
    /// from the table, which means "no language at all" and leaves tokei's own
    /// broad extension table in charge, so `.md` and `.json` keep the LOC facts
    /// they have today.
    pub loc: Option<LanguageType>,
}

/// Why a language could not be registered.
///
/// `register` refuses rather than returning a flag, because both cases end in
/// the coverage artifact making a false statement and neither has an answer the
/// registry can pick on its own. A malformed key produces an entry no lookup can
/// ever reach; a bundled collision is an operator's precedence decision (R28),
/// not the registry's.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegisterError {
    /// The key is not a shape [`Languages::for_extension`] can ever be handed.
    #[error("`{extension}` is not a usable extension key: {reason}")]
    MalformedKey {
        extension: String,
        reason: &'static str,
    },
    /// A language this build compiled in already claims the extension.
    #[error(
        "`{extension}` is already claimed by the bundled language `{bundled}`;          overriding it is the operator's decision to declare, not a plugin's to take"
    )]
    ClaimedByBundled { extension: String, bundled: String },
}

/// Extension → language. Resolved once per run and threaded (KTD17).
///
/// [`Default`] gives an EMPTY registry, which is not the same thing as
/// [`Languages::bundled`]: a run threading the derived default reports every
/// file as `language: null`. `ExtractOptions::default` supplies `bundled()` for
/// exactly that reason.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Languages {
    by_extension: BTreeMap<String, LanguageEntry>,
    /// The keys `bundled()` installed. Kept beside the table rather than as a
    /// field on [`LanguageEntry`], so a plugin cannot construct an entry that
    /// claims to be bundled.
    bundled: BTreeSet<String>,
    /// Extension → id of the plugin that parses its files in this run: the
    /// first plugin, in precedence order, that claims it. Kept beside the
    /// table for the same reason as `bundled`, and because the registry
    /// names a language whether or not a plugin parses it — a `bundled()`
    /// registry no plugin set was recorded into says nothing here.
    parsed_by: BTreeMap<String, String>,
    /// Plugin id → the contract version it runs under (`0.1`, `0.2`), for
    /// every plugin of the run — one whose every extension another plugin
    /// took first included, since it was loaded all the same (plan A, U5).
    plugin_contracts: BTreeMap<String, String>,
    /// The bundled extensions no bundled plugin parses — C and C++ (plan B,
    /// U4). The host lexer reads their literals and comments, so they are
    /// languages with a name and a line count, but nothing reads their
    /// declarations, and coverage must not say a grammar found none.
    lexed_only: BTreeSet<String>,
}

impl Languages {
    /// The languages this build has grammars for.
    ///
    /// The extensions the bundled plugins parse, which
    /// `bundled_languages_match_the_plugins_that_actually_parse` in
    /// `treesitter.rs` holds together with this table — the same treatment
    /// `ids::all()` gets from
    /// `the_bundled_ids_match_the_extractors_that_actually_run` (U3) — plus C
    /// and C++, which only the host lexer reads (plan B, U4; see
    /// [`Languages::is_lexed_only`]).
    pub fn bundled() -> Self {
        let mut by_extension = BTreeMap::new();
        let mut add = |ext: &str, name: &str, loc: LanguageType| {
            by_extension.insert(
                ext.to_string(),
                LanguageEntry {
                    name: name.to_string(),
                    loc: Some(loc),
                },
            );
        };
        add("rs", "rust", LanguageType::Rust);
        add("ts", "typescript", LanguageType::TypeScript);
        add("tsx", "typescript", LanguageType::Tsx);
        add("py", "python", LanguageType::Python);
        add("cs", "csharp", LanguageType::CSharp);
        add("go", "go", LanguageType::Go);
        add("js", "javascript", LanguageType::JavaScript);
        add("mjs", "javascript", LanguageType::JavaScript);
        add("cjs", "javascript", LanguageType::JavaScript);
        add("jsx", "javascript", LanguageType::Jsx);
        add("java", "java", LanguageType::Java);
        add("sql", "sql", LanguageType::Sql);
        add("xaml", "xaml", LanguageType::Xaml);
        // C and C++ (plan B, U4): no plugin, only the host lexer. Their line
        // counter is whichever tokei already used for the extension — `.h` is
        // `CHeader`, `.hpp` `CppHeader` — so registering them changes no count.
        let lexed_only: BTreeSet<String> = [
            ("c", "c"),
            ("h", "c"),
            ("cpp", "cpp"),
            ("cc", "cpp"),
            ("cxx", "cpp"),
            ("hpp", "cpp"),
            ("hh", "cpp"),
            ("hxx", "cpp"),
        ]
        .into_iter()
        .filter_map(|(ext, name)| {
            let loc = LanguageType::from_file_extension(ext)?;
            add(ext, name, loc);
            Some(ext.to_string())
        })
        .collect();
        let bundled = by_extension.keys().cloned().collect();
        Languages {
            by_extension,
            bundled,
            parsed_by: BTreeMap::new(),
            plugin_contracts: BTreeMap::new(),
            lexed_only,
        }
    }

    /// Whether `path` is a language the host lexer reads and no plugin of the
    /// run parses — a C or C++ file (plan B, U4). Its literals and comments
    /// are read; its declarations are not.
    pub fn is_lexed_only(&self, path: &str) -> bool {
        extension(path)
            .is_some_and(|ext| self.lexed_only.contains(ext) && !self.parsed_by.contains_key(ext))
    }

    /// The bundled extensions no bundled plugin parses, ordered. For the
    /// tie-down tests that hold the registry against the plugins.
    pub fn lexed_only_extensions(&self) -> impl Iterator<Item = &str> {
        self.lexed_only.iter().map(String::as_str)
    }

    /// Record one plugin of the run: the contract it runs under, and the
    /// extensions it claims. Called in precedence order, so an extension two
    /// plugins claim stays with the first — the one that parses its files.
    pub fn record_plugin<'e>(
        &mut self,
        id: &str,
        contract: &str,
        extensions: impl IntoIterator<Item = &'e str>,
    ) {
        self.plugin_contracts
            .insert(id.to_string(), contract.to_string());
        for ext in extensions {
            self.parsed_by
                .entry(ext.to_string())
                .or_insert_with(|| id.to_string());
        }
    }

    /// The contract version of the plugin that parses `path`, or `None` when
    /// no recorded plugin does.
    pub fn contract_for(&self, path: &str) -> Option<&str> {
        let plugin = self.parsed_by.get(extension(path)?)?;
        self.plugin_contracts.get(plugin).map(String::as_str)
    }

    /// How many recorded plugins run under each contract version.
    pub fn plugins_by_contract(&self) -> BTreeMap<String, u64> {
        let mut counts = BTreeMap::new();
        for contract in self.plugin_contracts.values() {
            *counts.entry(contract.clone()).or_insert(0) += 1;
        }
        counts
    }

    /// Register a language this build was not compiled with (R26).
    ///
    /// On success, returns the entry it replaced — `None` for a clean insert,
    /// `Some` when one registered language displaced another, which is the
    /// collision R28 resolves by declared precedence.
    ///
    /// # The key is a bare extension
    ///
    /// It is compared against the text after a path's LAST dot, so a key
    /// containing a dot, a path separator, or nothing at all can never match any
    /// file. Accepting one silently produced an entry that was simply
    /// unreachable, and a file the plugin had parsed still reported
    /// `language: null` and "no grammar covers this file type" — the exact R26
    /// statement the registry exists to remove. A plugin manifest writing
    /// `.ktd`, the way manifests usually do, hit precisely that.
    ///
    /// Case is NOT normalized and NOT rejected. Matching is case-sensitive
    /// throughout (see the module doc), so `KTD` is a coherent key that claims
    /// `*.KTD` and nothing else — surprising to a manifest author, but
    /// consistent, and folding it here would silently change which files a
    /// bundled language claims.
    ///
    /// # A bundled extension is refused
    ///
    /// The registry is authoritative for coverage naming, `metrics` LOC and
    /// `test_pairing` conventions, but NOT for `treesitter` or `external`, which
    /// still match on their own tables (U11 and U12 migrate them). Overriding
    /// `rs` therefore left tree-sitter parsing every Rust file and emitting
    /// symbols while every `Loc` and `TestByConvention` fact for those files
    /// disappeared — a repository reported under the plugin's language name with
    /// no lines at all. Refusing keeps that from happening by accident; U8 owns
    /// the path where an operator declares the override deliberately (its step
    /// 1c: precedence is set only in the operator's declaration surface, never
    /// carried in a plugin manifest).
    pub fn register(
        &mut self,
        extension: &str,
        entry: LanguageEntry,
    ) -> Result<Option<LanguageEntry>, RegisterError> {
        let malformed = |reason| {
            Err(RegisterError::MalformedKey {
                extension: extension.to_string(),
                reason,
            })
        };
        if extension.is_empty() {
            return malformed("it is empty");
        }
        if extension.contains('.') {
            return malformed("an extension is the text after the last dot and cannot contain one");
        }
        if extension.contains('/') || extension.contains('\\') {
            return malformed("it contains a path separator");
        }
        if let Some(bundled) = self.bundled.get(extension) {
            return Err(RegisterError::ClaimedByBundled {
                extension: extension.to_string(),
                bundled: self.by_extension[bundled].name.clone(),
            });
        }
        Ok(self.by_extension.insert(extension.to_string(), entry))
    }

    /// The entry for a path's extension, or `None` when no language claims it.
    pub fn for_path(&self, path: &str) -> Option<&LanguageEntry> {
        self.for_extension(extension(path)?)
    }

    /// The entry for a bare extension, without the leading dot.
    ///
    /// `metrics` classifies straight off `Path::extension()` and never holds a
    /// path string, so making it build one would be inventing a round trip for
    /// the lookup to undo.
    pub fn for_extension(&self, extension: &str) -> Option<&LanguageEntry> {
        self.by_extension.get(extension)
    }

    /// The reporting name for a path's language.
    pub fn name_for(&self, path: &str) -> Option<&str> {
        self.for_path(path).map(|e| e.name.as_str())
    }

    /// Every extension this table claims, ordered. Exists for the tie-down tests
    /// that hold the remaining closed tables against this one.
    pub fn extensions(&self) -> impl Iterator<Item = &str> {
        self.by_extension.keys().map(String::as_str)
    }
}

/// The extension of a path, or `None` when it has none. Case-preserving — see
/// the module doc.
fn extension(path: &str) -> Option<&str> {
    path.rsplit_once('.').map(|(_, ext)| ext)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ts_and_tsx_report_one_name_and_count_with_their_own_grammar() {
        let langs = Languages::bundled();
        assert_eq!(langs.name_for("a/b.ts"), Some("typescript"));
        assert_eq!(langs.name_for("a/b.tsx"), Some("typescript"));
        // …while LOC follows the file's actual grammar, which is the half
        // `test_pairing`'s folded enum could not express.
        assert_eq!(
            langs.for_path("a/b.ts").unwrap().loc,
            Some(LanguageType::TypeScript)
        );
        assert_eq!(
            langs.for_path("a/b.tsx").unwrap().loc,
            Some(LanguageType::Tsx)
        );
    }

    /// The registry must not change what `metrics` counts for a bundled
    /// language. Registry-first is only behavior-preserving if every entry
    /// agrees with the tokei lookup it replaces — a belief about `Tsx` is worth
    /// less than an assertion about it.
    #[test]
    fn every_bundled_entry_counts_lines_exactly_as_tokei_would() {
        let langs = Languages::bundled();
        for ext in langs.extensions() {
            let entry = langs.for_path(&format!("a.{ext}")).unwrap();
            assert_eq!(
                entry.loc,
                LanguageType::from_file_extension(ext),
                "`.{ext}` would get a different LOC count than it does today"
            );
        }
    }

    #[test]
    fn an_unclaimed_extension_has_no_language() {
        let langs = Languages::bundled();
        assert_eq!(langs.name_for("README.md"), None);
        assert_eq!(langs.name_for("Makefile"), None);
    }

    /// Guards the module doc: folding case would hand `.RS` a language and a
    /// `Loc` fact it does not have today.
    #[test]
    fn extension_matching_is_case_sensitive() {
        assert_eq!(Languages::bundled().name_for("SRC/LIB.RS"), None);
    }

    /// R26: a language this build was never compiled with reports its own name,
    /// which is the false statement the whole registry exists to stop.
    #[test]
    fn a_registered_language_reports_itself_rather_than_nothing() {
        let mut langs = Languages::bundled();
        assert_eq!(langs.name_for("vzor/a.ktd"), None);
        langs
            .register(
                "ktd",
                LanguageEntry {
                    name: "otevrenost".to_string(),
                    loc: None,
                },
            )
            .expect("an unclaimed, well-formed extension registers");
        assert_eq!(langs.name_for("vzor/a.ktd"), Some("otevrenost"));
    }

    /// A key no lookup can ever be handed is refused at the entry point rather
    /// than stored as an unreachable entry. `.ktd` is the case that matters: it
    /// is how a manifest author naturally writes an extension.
    #[test]
    fn a_key_the_lookup_could_never_match_is_refused() {
        for bad in [".ktd", "tar.gz", "", "a/b", "a\\b"] {
            let mut langs = Languages::bundled();
            let err = langs
                .register(
                    bad,
                    LanguageEntry {
                        name: "otevrenost".to_string(),
                        loc: None,
                    },
                )
                .expect_err("`{bad}` can never match a file and must not be stored");
            assert!(matches!(err, RegisterError::MalformedKey { .. }), "{err}");
            assert_eq!(
                langs.name_for("vzor/a.ktd"),
                None,
                "a refused registration must leave the table untouched"
            );
        }
    }

    /// Case is a coherent key, not a malformed one — matching is case-sensitive
    /// throughout, so `KTD` claims `*.KTD`. Pinned so nobody "fixes" it into a
    /// silent fold that would change which files a bundled language claims.
    #[test]
    fn an_uppercase_key_is_accepted_and_claims_only_that_case() {
        let mut langs = Languages::bundled();
        langs
            .register(
                "KTD",
                LanguageEntry {
                    name: "otevrenost".to_string(),
                    loc: None,
                },
            )
            .expect("an uppercase key is coherent, just narrow");
        assert_eq!(langs.name_for("vzor/a.KTD"), Some("otevrenost"));
        assert_eq!(langs.name_for("vzor/a.ktd"), None);
    }

    /// The `loc: None` half of the entry doc: a registered language tokei DOES
    /// know must still count nothing, because the entry is the answer and there
    /// is no fallback behind it.
    #[test]
    fn a_registered_language_with_no_counter_overrides_tokei() {
        let mut langs = Languages::bundled();
        assert!(
            LanguageType::from_file_extension("json").is_some(),
            "premise: tokei knows this extension on its own"
        );
        langs
            .register(
                "json",
                LanguageEntry {
                    name: "konfigurace".to_string(),
                    loc: None,
                },
            )
            .expect("a registered language that no bundled one claims");
        assert_eq!(langs.for_path("a.json").unwrap().loc, None);
    }

    /// Taking a bundled extension is refused, because `treesitter` and `external`
    /// do not consult this table yet: the override would silently strip every
    /// `Loc` and `TestByConvention` fact while the Rust grammar kept parsing.
    #[test]
    fn taking_a_bundled_extension_is_refused_not_silently_granted() {
        let mut langs = Languages::bundled();
        let err = langs
            .register(
                "rs",
                LanguageEntry {
                    name: "not-rust".to_string(),
                    loc: None,
                },
            )
            .expect_err("a bundled extension is the operator's to reassign, not a plugin's");
        assert!(
            matches!(&err, RegisterError::ClaimedByBundled { bundled, .. } if bundled == "rust"),
            "the error must name what it protects: {err}"
        );
        assert_eq!(
            langs.name_for("src/lib.rs"),
            Some("rust"),
            "a refused override must leave the bundled language in place"
        );
    }

    /// One registered language displacing another IS allowed and reported — that
    /// is the collision R28 resolves by declared precedence, not a bundled one.
    #[test]
    fn one_registered_language_displacing_another_is_reported() {
        let mut langs = Languages::bundled();
        let entry = |name: &str| LanguageEntry {
            name: name.to_string(),
            loc: None,
        };
        assert_eq!(langs.register("ktd", entry("otevrenost")), Ok(None));
        assert_eq!(
            langs.register("ktd", entry("jiny-plugin")),
            Ok(Some(entry("otevrenost"))),
            "a caller must be able to see that it displaced something"
        );
    }
}
