//! The external extractor (U5): what the code reaches for OUTSIDE the repository
//! — types from foreign libraries, database objects, network endpoints and
//! configuration keys (R3).
//!
//! # The one rule that governs everything here (KTD8)
//!
//! An external reference is emitted ONLY from an unambiguous pattern, and is
//! ALWAYS `Resolution::Heuristic`. When in doubt, nothing is emitted. A pattern
//! that fires on a plausible-looking string is worse than no pattern at all: one
//! invented database table or endpoint discredits every true one in the corpus,
//! and a reader who has caught the index lying once cannot trust the rest of it.
//! So every family below is anchored on positive evidence, and the deliberate
//! non-matches are as much a part of the contract as the matches (see the module
//! tests, which assert the decoys stay out).
//!
//! # Foreign symbols: the manifests are the authority
//!
//! Whether an import leaves the repository is decided by the DEPENDENCIES the
//! manifests declare — the `dep:` nodes manifest-deps already put in the graph —
//! never by "the repo does not seem to contain it". That keeps the claim positive
//! (a declared package really is outside this repo) and avoids the whole class of
//! edition/resolver guessing: a Rust 2015 `use mymod::X`, a TS path alias, a Go
//! GOPATH package would each need the resolver's own knowledge to classify, and
//! guessing wrong invents a foreign library that does not exist.
//!
//! Consequences, stated rather than hidden:
//! - Python contributes NO foreign symbols — no Python manifest is parsed yet, so
//!   there is no authority to match `import django` against. A later unit adding
//!   `pyproject.toml`/`requirements.txt` to manifest-deps turns this on with no
//!   change here.
//! - Neither does a repo without manifests, nor the standard library of any
//!   language (nothing declares it).
//!
//! Two shapes carry a foreign symbol into the graph:
//! 1. **An import that BINDS a name** — a Rust `use serde::Serialize`, a TS
//!    `import { useState } from "react"`, a Go `import "github.com/pkg/errors"`, a
//!    C# alias `using J = Newtonsoft.Json.Linq.JObject`. Each site that uses the
//!    bound name yields a `Uses` edge from the member the site sits in.
//! 2. **A fully qualified usage** — a C# `Newtonsoft.Json.JsonConvert.Serialize…`
//!    or a Rust `serde::Serialize` written out in place, whose leading segments
//!    are a declared dependency. The path is taken exactly as the source writes
//!    it; deciding which segment stops being a namespace and starts being a type
//!    would need type information (`Resolved` territory), so nothing here guesses.
//!
//! A C# `using Newtonsoft.Json;` binds no name at all, so no member-level
//! attribution is possible without type resolution. It still states something
//! true — this FILE reaches into that namespace — so the edge starts at the file.
//! The same fallback covers an import that is never used: every External node is
//! the target of at least one `Uses` edge, never an orphan.
//!
//! # Resources: patterns with two anchors, never one
//!
//! - **Database object** (`res:db:<object>`) — a QUALIFIED name (`dbo.Orders`,
//!   `db.dbo.Orders`) in a position a SQL statement defines, inside a literal that
//!   also carries the statement's companion keyword: SELECT with FROM, INSERT
//!   INTO, UPDATE with SET, DELETE with FROM, MERGE INTO, EXEC. Both anchors are
//!   required, which is what keeps prose like "Update config.value from
//!   settings.json" out. An UNQUALIFIED `FROM Orders` is never emitted: it is
//!   indistinguishable from a variable, a LINQ clause or a sentence.
//! - **Network endpoint** (`res:url:<host/path>`) — an absolute `http(s)`/`ftp`
//!   URL in a non-interpolated literal. A relative path names nothing outside the
//!   repo on its own, and an interpolated fragment names a different endpoint per
//!   call, so neither is emitted.
//! - **Configuration key** (`res:cfg:<key>`) — a literal read through a NAMED
//!   configuration API (`ConfigurationManager.AppSettings[…]`, `GetSection(…)`,
//!   `os.getenv(…)`, `env::var(…)`, `process.env.…`). The API call is the second
//!   anchor: a free string constant that merely looks like a key is not evidence
//!   that anything reads it.
//!
//! # Secrets are never quoted (R22)
//!
//! A literal whose SHAPE matches a secret pattern — a password/token assignment, a
//! connection string, a PEM private-key header, a URL carrying userinfo — yields
//! only a [`FactKind::RedactedLiteral`] reference to its place in the code, and is
//! then excluded from every other pattern here. The redaction check runs FIRST and
//! runs on interpolated literals too: emission is fail-closed, redaction is
//! fail-safe. That asymmetry is deliberate — a redaction on a coincidental shape
//! costs a reader one glance, whereas a leaked credential cannot be recalled.
//!
//! # Anchors (plan B, U3 and U4)
//!
//! Beside the references above, every string literal and every line of an
//! ordinary comment is a text a reader may search for — an error message, a
//! query, a note — and becomes a [`FactKind::Anchor`] on the innermost
//! declaration around it, with its first line (KTD1). Nothing here needs a
//! plugin, so C and C++ files, which have none, get anchors and database
//! accesses owned by the file. The secret filter runs on all of it first
//! (KTD11): a comment block goes through [`redact_secret_block`] with adjacent
//! line comments joined, a literal is read with the code on its line, a
//! key-shaped token is never an anchor, and what is withheld leaves its place
//! and reason, never its text. A C# raw literal (`"""…"""`) is read whole and
//! an interpolated one by its text with the holes blanked, for anchors and SQL
//! alike. Anchors never reach the manifest; their only projection is the
//! detail.
//!
//! # Decisions by an enum's value (plan B, U5)
//!
//! In a C# file the code is read once more for `case E.V` labels, switch
//! expression arms `E.V =>` and comparisons `== E.V`, `!= E.V`, `is E.V`,
//! where `E` names an enum the repository declares (KTD3). No plugin reports
//! an access to an enum member and the host has no syntax tree, so each place
//! is checked against the graph instead: `V` must be a `variant` of the enum,
//! and a label or an arm is verified only when the enum is the one type named
//! `E` the place can see — through the types around it and their bases, its
//! namespace and the ones above it, its file's usings and its project's. A
//! comparison is never verified. Each place is an
//! [`FactKind::EnumDecision`] on the declaration around it; a place no single
//! enum can be named for is only counted, on its file.
//!
//! # Cost (R17)
//!
//! Per source file: ONE hand-rolled lexer pass that splits the text into code,
//! comment and string-literal segments; ONE identifier/dotted-path scan over the
//! code segments only (borrowed slices, no allocation per token); a handful of
//! small regexes per string literal; and the language's own import regex over the
//! lines. Nothing is quadratic in file size, every pattern is compiled once into a
//! `OnceLock`, and the whole pattern set is deliberately small. The lexer is what
//! buys the precision: an identifier inside a comment or a bound name inside a
//! string can never become a `Uses` edge.

use super::cache::CacheStore;
use super::ids;
use super::resolve::project::{ProjectUsings, ProjectWide, UsingsHandoff};
use super::resolve::table::namespace_of;
use super::walk::{walk_files, GraphOps};
use super::{ExtractError, Extractor, Repo};
use crate::graph::{
    AnchorSource, AnchorText, DbAccessKind, DecisionShape, Edge, EdgeKind, EnumDecision,
    ExtractorId, Fact, FactKind, FactValue, Graph, NodeKey, NodeKind, Provenance, Tier, ANCHOR_CAP,
    ANCHOR_TEXT_CAP,
};
use rayon::prelude::*;
use regex::Regex;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::OnceLock;

/// Trust-rank 25 (KTD-B8): beside metrics and test-pairing. Rank only decides
/// SCALAR-fact precedence in merge (E6), and no other extractor emits this one's
/// kinds, so nothing here ever competes for a value.
const TRUST_RANK: u8 = 25;

/// The [`FactKind::ExternalKind`] vocabulary — the whole of it.
const KIND_SYMBOL: &str = "symbol";
const KIND_DB: &str = "db_object";
const KIND_URL: &str = "url";
const KIND_CFG: &str = "config_key";

pub struct External {
    /// Token prefixes this run recognizes with no keyword nearby (R22), from
    /// [`crate::extract::ExtractOptions`]. A company token shape is added on the
    /// command line rather than compiled in.
    pub secret_prefixes: SecretPrefixes,
    /// The cross-run cache this extractor's one `walk_files` pass reads and
    /// writes (U4). Set at construction by
    /// [`super::default_extractors_with`] from whatever `generate_reporting`
    /// (U5) decided — `Some` only once `cache::activate` found a matching
    /// fingerprint, `None` otherwise or for any caller that never passes a
    /// cache at all. Owned rather than borrowed so `External` needs no
    /// lifetime parameter, the same choice `treesitter::Languages` makes
    /// for the same reason.
    pub(crate) cache: Option<CacheStore>,
    /// The project-wide C# usings the languages extractor read before its
    /// usage pass, which a decision by an enum's value is checked against
    /// (plan B, U5) — handed over rather than read from every `.cs` blob a
    /// second time.
    pub(crate) project_usings: UsingsHandoff,
}

impl Extractor for External {
    fn id(&self) -> ExtractorId {
        ids::external()
    }

    fn emitted_kinds(&self) -> &'static [FactKind] {
        &[
            FactKind::ExternalName,
            FactKind::ExternalKind,
            FactKind::RedactedLiteral,
            FactKind::DbAccess,
            FactKind::Anchor,
            FactKind::AnchorSkipped,
            FactKind::AnchorsDropped,
            FactKind::EnumDecision,
            FactKind::EnumDecisionsUnmatched,
        ]
    }

    fn trust_rank(&self) -> u8 {
        TRUST_RANK
    }

    fn run(
        &self,
        repo: &Repo,
        graph: &mut Graph,
        progress: &dyn crate::progress::Progress,
    ) -> Result<(), ExtractError> {
        // Every index is read out of the graph ONCE, before any file is
        // scanned: the declared dependencies (manifest-deps), the declaration
        // ranges and the C# types with their enum values (tree-sitter). Both
        // extractors run before this one. The project-wide usings come from
        // the languages extractor, which read them for its own usage pass.
        let run = RunIndexes {
            deps: Deps::build(graph),
            owners: OwnerIndex::build(graph),
            enums: EnumIndex::build(graph),
            project: self.project_usings.get(),
        };

        let tracked = repo.tracked_files_with_paths()?;
        // Counted over every tracked file rather than over the ones this
        // extractor can read, so the operator watches a walk that advances
        // steadily instead of one that stalls and then leaps.
        progress.total(tracked.len());
        walk_files(
            repo,
            &tracked,
            progress,
            "external",
            self.cache.as_ref(),
            |key| Lang::from_path(key).is_some(),
            |batch| {
                batch
                    .par_iter()
                    .map(|(key, bytes)| {
                        let mut ops = GraphOps::default();
                        if let Some(lang) = Lang::from_path(key) {
                            // Lossy is safe for offsets: an invalid byte becomes
                            // U+FFFD, and only `\n` positions decide a line
                            // number.
                            let src = String::from_utf8_lossy(bytes);
                            emit_file(&mut ops, &run, lang, key, &src, &self.secret_prefixes);
                        }
                        ops
                    })
                    .collect()
            },
            |ops: GraphOps| ops.apply(graph),
        )?;
        Ok(())
    }
}

// ---- Languages ----

/// The languages this extractor lexes. Deliberately the same set tree-sitter
/// parses, and the same `<lang>` tags its `sym:` keys use, so an `ext:` key and a
/// `sym:` key never disagree about what "csharp" means.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Lang {
    Rust,
    /// `.ts` and `.tsx` — one lexer, one `ts` tag, exactly as tree-sitter does.
    Ts,
    Python,
    CSharp,
    Go,
    /// Lexed like C#'s plain strings (a `"""` text block is a triple-quoted
    /// literal). No Maven or Gradle manifest is parsed yet, so — like Python —
    /// Java has no dependency authority and its imports stay unmatched; its
    /// database, URL and configuration literals are read all the same.
    Java,
    /// C and C++ (plan B, U4). No plugin reads their declarations, so the
    /// lexer is all this build knows about them: their literals and comments
    /// give anchors and database accesses, owned by the file (KTD1). No import
    /// is read — an `#include` names a header, not a dependency a manifest
    /// declares. `.h` is C's: the two lex alike, and a header says nothing
    /// that would tell them apart.
    C,
    Cpp,
}

impl Lang {
    fn from_path(path: &str) -> Option<Lang> {
        match path.rsplit_once('.').map(|(_, ext)| ext)? {
            "rs" => Some(Lang::Rust),
            // JavaScript imports the same way and resolves against the same
            // npm dependencies, so it shares the TypeScript lexer and tag.
            "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" => Some(Lang::Ts),
            "py" => Some(Lang::Python),
            "cs" => Some(Lang::CSharp),
            "go" => Some(Lang::Go),
            "java" => Some(Lang::Java),
            "c" | "h" => Some(Lang::C),
            "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" => Some(Lang::Cpp),
            _ => None,
        }
    }

    fn tag(self) -> &'static str {
        match self {
            Lang::Rust => "rust",
            Lang::Ts => "ts",
            Lang::Python => "python",
            Lang::CSharp => "csharp",
            Lang::Go => "go",
            Lang::Java => "java",
            Lang::C => "c",
            Lang::Cpp => "cpp",
        }
    }

    /// The separator this language joins a qualified path with.
    fn sep(self) -> &'static str {
        match self {
            Lang::Rust | Lang::Cpp => "::",
            _ => ".",
        }
    }

    /// Which ecosystem's declared dependencies decide whether this language's
    /// imports leave the repo. Python has none yet — see the module doc.
    fn ecosystem(self) -> Option<Ecosystem> {
        match self {
            Lang::Rust => Some(Ecosystem::Cargo),
            Lang::Ts => Some(Ecosystem::Npm),
            Lang::CSharp => Some(Ecosystem::Nuget),
            Lang::Go => Some(Ecosystem::Go),
            Lang::Python | Lang::Java | Lang::C | Lang::Cpp => None,
        }
    }
}

// ---- The dependency authority ----

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Ecosystem {
    Cargo,
    Npm,
    Nuget,
    Go,
}

impl Ecosystem {
    /// Which ecosystem a manifest path belongs to. The manifest comes from the
    /// `DependencyName` fact's PROVENANCE, never from parsing the `dep:` key apart
    /// (KTD-B1) — provenance exists precisely so a consumer never has to.
    fn of_manifest(path: &str) -> Option<Ecosystem> {
        let base = path.rsplit('/').next().unwrap_or(path);
        if base == "Cargo.toml" {
            Some(Ecosystem::Cargo)
        } else if base == "package.json" {
            Some(Ecosystem::Npm)
        } else if base == "go.mod" {
            Some(Ecosystem::Go)
        } else if base.ends_with(".csproj") {
            Some(Ecosystem::Nuget)
        } else {
            None
        }
    }
}

/// The dependency names every manifest in the repo declares, per ecosystem.
#[derive(Default)]
struct Deps {
    by_eco: BTreeMap<Ecosystem, BTreeSet<String>>,
}

impl Deps {
    fn build(graph: &Graph) -> Deps {
        let mut deps = Deps::default();
        for node in graph.nodes_sorted() {
            if node.kind != NodeKind::Dependency {
                continue;
            }
            for fact in &node.facts {
                if fact.kind != FactKind::DependencyName {
                    continue;
                }
                let FactValue::Text(name) = &fact.value else {
                    continue;
                };
                if let Some(eco) = Ecosystem::of_manifest(&fact.provenance.file) {
                    deps.by_eco.entry(eco).or_default().insert(name.clone());
                }
            }
        }
        deps
    }

    fn names(&self, eco: Ecosystem) -> Option<&BTreeSet<String>> {
        self.by_eco.get(&eco)
    }

    /// Whether an IMPORT's module spelling belongs to a declared dependency.
    ///
    /// Matching is segment-aware in every ecosystem, because a prefix test alone
    /// is wrong: `Newtonsoft.JsonSchema` starts with `Newtonsoft.Json` and is a
    /// different package. Cargo additionally normalizes `-`/`_`, since a crate
    /// declared as `tree-sitter-rust` is imported as `tree_sitter_rust`.
    fn declares_import(&self, lang: Lang, spelling: &str) -> bool {
        let Some(eco) = lang.ecosystem() else {
            return false;
        };
        let Some(names) = self.names(eco) else {
            return false;
        };
        match eco {
            Ecosystem::Cargo => {
                let root = spelling.split("::").next().unwrap_or(spelling).trim();
                let root = crate_name(root);
                names.iter().any(|d| crate_name(d) == root)
            }
            Ecosystem::Npm => names
                .iter()
                .any(|d| spelling == d || spelling.starts_with(&format!("{d}/"))),
            Ecosystem::Go => names
                .iter()
                .any(|d| spelling == d || spelling.starts_with(&format!("{d}/"))),
            Ecosystem::Nuget => names
                .iter()
                .any(|d| spelling == d || spelling.starts_with(&format!("{d}."))),
        }
    }

    /// Whether a qualified path WRITTEN IN CODE reaches into a declared
    /// dependency's own namespace. Only Rust and C# spell foreign symbols this way
    /// (`serde::Serialize`, `Newtonsoft.Json.JsonConvert`); a TS or Go dotted path
    /// is a member access on a locally bound name, which the binding map handles.
    ///
    /// The path must reach INSIDE the dependency (`dep::x` / `dep.x`), never merely
    /// equal it: a bare identifier that happens to match a package name is far more
    /// likely a local variable.
    fn qualifies_usage(&self, lang: Lang, path: &str) -> bool {
        let eco = match lang {
            Lang::Rust => Ecosystem::Cargo,
            Lang::CSharp => Ecosystem::Nuget,
            _ => return false,
        };
        let Some(names) = self.names(eco) else {
            return false;
        };
        let sep = lang.sep();
        match eco {
            Ecosystem::Cargo => {
                let Some((root, _)) = path.split_once(sep) else {
                    return false;
                };
                let root = crate_name(root);
                names.iter().any(|d| crate_name(d) == root)
            }
            _ => names.iter().any(|d| path.starts_with(&format!("{d}{sep}"))),
        }
    }
}

/// A cargo dependency name and the identifier a `use` writes it as differ by
/// `-` vs `_` (`tree-sitter-rust` → `tree_sitter_rust`), so both sides normalize.
fn crate_name(name: &str) -> String {
    name.replace('-', "_")
}

// ---- Where a usage site sits ----

/// One declaration — or one part of a partial type — and the source lines it
/// spans.
struct Decl {
    key: NodeKey,
    start: u32,
    end: u32,
    /// Its `SymbolName`, to find where the name is written when lines alone
    /// cannot tell two declarations apart.
    name: String,
}

/// File → declaration ranges, so a reference can be attributed to the MEMBER it
/// sits inside rather than only to its file (KTD5).
///
/// Deliberately a mirror of the shared resolver's `SymbolTable::by_file`/`owner`
/// carrying only the half this extractor needs: the ranges, not the repo-wide
/// name maps a call resolution requires. The two indexes answer different
/// questions and are built from the same public facts.
///
/// Plan B, U3 (KTD1) reworked it in three ways, because anchors ask it far
/// more often than resources ever did:
///
/// - **Every part.** A `partial` type declared in two files has one node with
///   a `SymbolName` fact per part, each anchored in its own file (the facts are
///   merged only after every extractor ran). Taking the first fact only lost the
///   other parts' ranges, so a literal in a field initializer of the second file
///   fell to that file.
/// - **No scan back.** Each declaration knows its parent — the nearest one that
///   contains it on more lines than it spans — so the innermost declaration
///   around a line is the last one starting before it, or the first of its
///   ancestors still open. A query costs a binary search and the nesting depth;
///   the old backwards scan walked every sibling declared before the line, which
///   made a file of a thousand members with a thousand type-level texts after
///   them quadratic.
/// - **Lines that two declarations share.** Where more than one declaration
///   starts or ends on the text's line, the line cannot say which one holds the
///   text, so the column does: the text belongs to the declaration whose name is
///   written last before it. The graph has no columns, so the name's position is
///   read from the source — the one place on the declaration's first line where
///   its name stands as a whole identifier in code. A name the line does not show
///   exactly once decides nothing, and the text then falls to the file rather
///   than to whichever declaration sorts last.
#[derive(Default)]
struct OwnerIndex {
    by_file: BTreeMap<String, FileDecls>,
}

/// One file's declarations.
#[derive(Default)]
struct FileDecls {
    /// Sorted by start ascending, end DESCENDING, then key — an enclosing
    /// declaration before what it encloses.
    decls: Vec<Decl>,
    /// The index of each declaration's parent: the nearest one before it that
    /// contains its lines and spans more of them. Equal ranges are siblings,
    /// not parent and child — lines cannot tell which encloses which.
    parent: Vec<Option<usize>>,
    /// `(end, index)`, sorted — the declarations ending on a given line.
    by_end: Vec<(u32, usize)>,
}

impl OwnerIndex {
    fn build(graph: &Graph) -> OwnerIndex {
        let mut by_file: BTreeMap<String, Vec<Decl>> = BTreeMap::new();
        for node in graph.nodes_sorted() {
            if node.kind != NodeKind::Symbol {
                continue;
            }
            // The declaration's file and line range are the `SymbolName` fact's
            // own provenance (U1) — one fact per part of a partial type.
            for fact in node.facts.iter().filter(|f| f.kind == FactKind::SymbolName) {
                let prov = &fact.provenance;
                let (Some(start), Some(end)) = (prov.span.start(), prov.span.end()) else {
                    continue;
                };
                let name = match &fact.value {
                    FactValue::Text(name) => name.clone(),
                    _ => String::new(),
                };
                by_file.entry(prov.file.clone()).or_default().push(Decl {
                    key: node.key.clone(),
                    start,
                    end,
                    name,
                });
            }
        }
        let by_file = by_file
            .into_iter()
            .map(|(file, decls)| (file, FileDecls::new(decls)))
            .collect();
        OwnerIndex { by_file }
    }

    fn file(&self, file: &str) -> Option<&FileDecls> {
        self.by_file.get(file)
    }
}

impl FileDecls {
    fn new(mut decls: Vec<Decl>) -> FileDecls {
        decls.sort_by(|a, b| {
            (a.start, std::cmp::Reverse(a.end), &a.key).cmp(&(
                b.start,
                std::cmp::Reverse(b.end),
                &b.key,
            ))
        });
        // Ranges come from a syntax tree, so they nest; two only meet on a line
        // one ends and the other starts on. One pass with a stack of the open
        // ancestors finds every parent.
        let mut parent = Vec::with_capacity(decls.len());
        let mut open: Vec<usize> = Vec::new();
        for (i, d) in decls.iter().enumerate() {
            while let Some(&top) = open.last() {
                let t = &decls[top];
                let contains = t.start <= d.start && t.end >= d.end;
                if contains && (t.start, t.end) != (d.start, d.end) {
                    break;
                }
                open.pop();
            }
            parent.push(open.last().copied());
            open.push(i);
        }
        let mut by_end: Vec<(u32, usize)> =
            decls.iter().enumerate().map(|(i, d)| (d.end, i)).collect();
        by_end.sort_unstable();
        FileDecls {
            decls,
            parent,
            by_end,
        }
    }

    /// The declarations that start on `line`.
    fn starting_on(&self, line: u32) -> &[Decl] {
        let lo = self.decls.partition_point(|d| d.start < line);
        let hi = self.decls.partition_point(|d| d.start <= line);
        &self.decls[lo..hi]
    }

    /// The innermost declaration around a text at byte `at` on `line`, or
    /// `None` when no declaration encloses it or the line cannot say which
    /// one does (see [`OwnerIndex`]). `name_at` says at which byte a
    /// declaration's name is written; it is asked only where two
    /// declarations may share the line, and what it says about such a line
    /// is kept in `meetings` — so the many texts of one shared line, a
    /// minified file's only line, read its declarations once, not once each.
    fn owner(
        &self,
        line: u32,
        at: usize,
        name_at: impl Fn(&Decl) -> Option<usize>,
        meetings: &RefCell<HashMap<u32, Meeting>>,
    ) -> Option<&NodeKey> {
        let starting = self.decls.partition_point(|d| d.start < line)
            ..self.decls.partition_point(|d| d.start <= line);
        let ending = self.by_end.partition_point(|(end, _)| *end < line)
            ..self.by_end.partition_point(|(end, _)| *end <= line);
        let i = if starting.len() + ending.len() < 2 {
            // At most one declaration starts or ends here: no name is asked.
            self.meeting(line, starting, ending, &name_at).pick(at)
        } else {
            meetings
                .borrow_mut()
                .entry(line)
                .or_insert_with(|| self.meeting(line, starting, ending, &name_at))
                .pick(at)
        };
        i.map(|i| &self.decls[i].key)
    }

    /// What `line` says about the texts on it, given the declarations
    /// starting on it (`starting`, into `decls`) and ending on it
    /// (`ending`, into `by_end`).
    fn meeting(
        &self,
        line: u32,
        starting: std::ops::Range<usize>,
        ending: std::ops::Range<usize>,
        name_at: impl Fn(&Decl) -> Option<usize>,
    ) -> Meeting {
        let hi = starting.end;
        let ending = self.by_end[ending]
            .iter()
            .map(|(_, i)| *i)
            .filter(move |&i| self.decls[i].start < line);
        match starting.len() + ending.clone().count() {
            // No declaration starts or ends here: the innermost one open on
            // this line is the last one started, or its nearest ancestor still
            // open.
            0 => {
                let mut at = hi.checked_sub(1);
                while let Some(i) = at {
                    if self.decls[i].end >= line {
                        return Meeting::Decided(Some(i));
                    }
                    at = self.parent[i];
                }
                Meeting::Decided(None)
            }
            // One starts or ends here, and every other declaration open on the
            // line spans it whole: that one is the innermost.
            1 => Meeting::Decided(starting.chain(ending).next()),
            // Several meet on this line: the name written last before a text
            // decides — and a name that cannot be placed decides nothing for
            // any text of the line.
            _ => {
                let mut named = Vec::new();
                for i in starting.chain(ending) {
                    match name_at(&self.decls[i]) {
                        Some(offset) => named.push((offset, i)),
                        None => return Meeting::Undecided,
                    }
                }
                named.sort_unstable();
                Meeting::ByName(named)
            }
        }
    }
}

/// What the declarations starting and ending on one line say about the
/// texts on it (see [`OwnerIndex`]).
enum Meeting {
    /// The lines alone decide: this declaration, or none, holds every text.
    Decided(Option<usize>),
    /// Several meet and every name is placed: `(name offset, declaration)`,
    /// sorted. A text belongs to the one whose name is written last before
    /// it — to none when that place holds two names, or none is before it.
    ByName(Vec<(usize, usize)>),
    /// Several meet and some name cannot be placed: nothing is told.
    Undecided,
}

impl Meeting {
    /// The declaration holding a text at byte `at`.
    fn pick(&self, at: usize) -> Option<usize> {
        match self {
            Meeting::Decided(i) => *i,
            Meeting::Undecided => None,
            Meeting::ByName(named) => {
                let before = named.partition_point(|&(offset, _)| offset <= at);
                let (best, i) = *named.get(before.checked_sub(1)?)?;
                let tied = before >= 2 && named[before - 2].0 == best;
                (!tied).then_some(i)
            }
        }
    }
}

/// Byte offset → 1-based line number.
struct LineIndex {
    starts: Vec<usize>,
}

impl LineIndex {
    fn new(src: &str) -> LineIndex {
        let mut starts = vec![0usize];
        starts.extend(
            src.bytes()
                .enumerate()
                .filter(|(_, b)| *b == b'\n')
                .map(|(i, _)| i + 1),
        );
        LineIndex { starts }
    }

    fn line(&self, offset: usize) -> u32 {
        self.starts.partition_point(|s| *s <= offset) as u32
    }

    /// Where a 1-based line starts, as a byte offset.
    fn start(&self, line: u32) -> usize {
        let i = (line.max(1) as usize - 1).min(self.starts.len() - 1);
        self.starts[i]
    }

    /// The bytes of a 1-based line, without its line break, in a text of
    /// `len` bytes.
    fn range(&self, line: u32, len: usize) -> (usize, usize) {
        let start = self.start(line);
        let end = self.starts.get(line as usize).map_or(len, |next| next - 1);
        (start, end.max(start))
    }
}

/// A byte of an identifier. A byte of a multi-byte character is part of an
/// identifier as surely as an ASCII letter is.
fn is_name_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c >= 0x80
}

/// How a declaration's name is written: `Foo<T>` and `Foo(int)` are written
/// `Foo`; `N.Foo` is written `Foo`.
fn written_name(decl: &Decl) -> &str {
    let name = decl.name.split(['<', '(']).next().unwrap_or("");
    name.rsplit(['.', ':']).next().unwrap_or(name).trim()
}

/// The stretches of code on a 1-based line, as byte ranges clipped to it.
fn code_on_line<'a>(
    src: &'a str,
    segs: &'a [Segment],
    lines: &LineIndex,
    line: u32,
) -> impl Iterator<Item = (usize, usize)> + 'a {
    let (line_start, line_end) = lines.range(line, src.len());
    let first = segs.partition_point(|s| s.end <= line_start);
    segs[first..]
        .iter()
        .take_while(move |s| s.start < line_end)
        .filter(|s| s.kind == SegKind::Code)
        .map(move |s| (s.start.max(line_start), s.end.min(line_end)))
        .filter(|&(from, to)| src.is_char_boundary(from) && src.is_char_boundary(to))
}

/// One line's names: written name → (how often it stands there, where last).
type LineNames<'a> = HashMap<&'a str, (u32, usize)>;

/// Where the names of a file's declarations are written (see [`OwnerIndex`]):
/// the one place on a declaration's first line where its name stands as a
/// whole identifier in code, as a byte offset — `None` when the line shows
/// it anywhere but exactly once, and then the position decides nothing.
///
/// Asked only for lines that declarations share, and read one line at a
/// time: the first name asked on a line has the line's code read once for
/// the names of every declaration starting there. One scan of the line per
/// declaration, as before, made a minified file — one line of thousands of
/// declarations — cubic in its length.
struct NamePlaces<'a> {
    src: &'a str,
    segs: &'a [Segment],
    lines: &'a LineIndex,
    decls: &'a FileDecls,
    /// Line → written name → (how often it stands there, where last).
    by_line: RefCell<HashMap<u32, LineNames<'a>>>,
}

impl<'a> NamePlaces<'a> {
    fn new(
        src: &'a str,
        segs: &'a [Segment],
        lines: &'a LineIndex,
        decls: &'a FileDecls,
    ) -> NamePlaces<'a> {
        NamePlaces {
            src,
            segs,
            lines,
            decls,
            by_line: RefCell::new(HashMap::new()),
        }
    }

    fn of(&self, decl: &Decl) -> Option<usize> {
        let name = written_name(decl);
        if name.is_empty() {
            return None;
        }
        if !name.bytes().all(is_name_byte) {
            // `~Foo`, `this[]`, `operator +`: no identifier, so read alone.
            return self.scanned(decl.start, name);
        }
        let mut by_line = self.by_line.borrow_mut();
        let names = by_line
            .entry(decl.start)
            .or_insert_with(|| self.names_on(decl.start));
        match names.get(name) {
            Some(&(1, at)) => Some(at),
            _ => None,
        }
    }

    /// Every identifier-shaped name of a declaration starting on `line`,
    /// with how often and where last it stands there as a whole identifier
    /// in code.
    fn names_on(&self, line: u32) -> LineNames<'a> {
        let mut names: LineNames<'a> = self
            .decls
            .starting_on(line)
            .iter()
            .map(written_name)
            .filter(|n| !n.is_empty() && n.bytes().all(is_name_byte))
            .map(|n| (n, (0, 0)))
            .collect();
        let b = self.src.as_bytes();
        for (from, to) in code_on_line(self.src, self.segs, self.lines, line) {
            let mut i = from;
            while i < to {
                if !is_name_byte(b[i]) {
                    i += 1;
                    continue;
                }
                let s = i;
                while i < to && is_name_byte(b[i]) {
                    i += 1;
                }
                // A whole identifier of the source, not only of this
                // stretch: what borders it outside counts as well.
                let whole = (s == 0 || !is_name_byte(b[s - 1]))
                    && b.get(i).is_none_or(|c| !is_name_byte(*c));
                if !whole {
                    continue;
                }
                if let Some(seen) = names.get_mut(&self.src[s..i]) {
                    seen.0 += 1;
                    seen.1 = s;
                }
            }
        }
        names
    }

    /// Where `name` — no identifier on its own — stands exactly once in the
    /// code of `line`, bordered by no identifier byte.
    fn scanned(&self, line: u32, name: &str) -> Option<usize> {
        let b = self.src.as_bytes();
        let mut found = None;
        let mut count = 0;
        for (from, to) in code_on_line(self.src, self.segs, self.lines, line) {
            for (at, _) in self.src[from..to].match_indices(name) {
                let (s, e) = (from + at, from + at + name.len());
                if (s == 0 || !is_name_byte(b[s - 1])) && b.get(e).is_none_or(|c| !is_name_byte(*c))
                {
                    count += 1;
                    found = Some(s);
                }
            }
        }
        found.filter(|_| count == 1)
    }
}

// ---- The lexer ----

/// What a stretch of source is. The whole point of lexing before matching: an
/// identifier in a comment is not a usage, and a bound name inside a string is not
/// a reference.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SegKind {
    Code,
    Comment,
    /// A string literal. `interpolated` marks the forms whose value differs per
    /// call — a C# `$"…"`, a TS template with a `${…}` hole, a Python f-string —
    /// which may be redacted but never emitted as a URL or a configuration key.
    Str {
        interpolated: bool,
    },
}

/// A segment of the source. For a string the range is its CONTENT, without the
/// quotes or any prefix; for code and comments it is the text itself.
struct Segment {
    kind: SegKind,
    start: usize,
    end: usize,
    /// The interpolation holes inside a C# interpolated literal, as absolute
    /// byte ranges — braces, code and format part — so the literal's TEXT can
    /// be read without them (plan B, U3). Empty for every other segment, and
    /// for an interpolated literal of a language whose holes this lexer does
    /// not read (a TS template with a hole, a Python f-string), which is
    /// therefore never read as text at all.
    holes: Vec<(usize, usize)>,
}

impl Segment {
    fn new(kind: SegKind, start: usize, end: usize) -> Segment {
        Segment {
            kind,
            start,
            end,
            holes: Vec::new(),
        }
    }
}

/// Split a source file into code, comment and string segments.
///
/// Hand-rolled rather than one giant alternation regex: the string flavors that
/// matter here (a C# `@"…"` holding multi-line SQL, a C# raw `"""…"""`, a Rust
/// `r#"…"#`, a C++ `R"(…)"`, a Go backtick block, a Python triple quote) each
/// have their own terminator rule, and a scanner states them plainly instead of
/// hiding them in a pattern nobody can read. An unterminated literal ends at the
/// newline (or EOF) rather than running away over the rest of the file.
fn segments(lang: Lang, src: &str) -> Vec<Segment> {
    let b = src.as_bytes();
    let n = b.len();
    let mut out: Vec<Segment> = Vec::new();
    let mut code_start = 0usize;
    let mut i = 0usize;

    let push_code = |out: &mut Vec<Segment>, start: usize, end: usize| {
        if end > start {
            out.push(Segment::new(SegKind::Code, start, end));
        }
    };

    while i < n {
        let c = b[i];

        // Line comment: `#` in Python, `//` everywhere else.
        let line_comment = (lang == Lang::Python && c == b'#')
            || (lang != Lang::Python && c == b'/' && i + 1 < n && b[i + 1] == b'/');
        if line_comment {
            let j = find_byte(b, i, b'\n').unwrap_or(n);
            push_code(&mut out, code_start, i);
            out.push(Segment::new(SegKind::Comment, i, j));
            i = j;
            code_start = i;
            continue;
        }
        // Block comment `/* … */` — every language here but Python.
        if lang != Lang::Python && c == b'/' && i + 1 < n && b[i + 1] == b'*' {
            let j = find_seq(b, i + 2, b"*/").map(|p| p + 2).unwrap_or(n);
            push_code(&mut out, code_start, i);
            out.push(Segment::new(SegKind::Comment, i, j));
            i = j;
            code_start = i;
            continue;
        }
        // A character literal is code, but `'"'` must not open a string: the
        // quote inside would pair with the next real one and turn the literal
        // after it into code.
        if c == b'\'' && matches!(lang, Lang::CSharp | Lang::C | Lang::Cpp) {
            if let Some(len) = char_literal_len(b, i) {
                i += len;
                continue;
            }
        }
        if c == b'"' || c == b'\'' || c == b'`' {
            if let Some(lit) = string_at(lang, b, i) {
                push_code(&mut out, code_start, lit.prefix_start);
                out.push(Segment {
                    kind: SegKind::Str {
                        interpolated: lit.interpolated,
                    },
                    start: lit.content_start,
                    end: lit.content_end,
                    holes: lit.holes,
                });
                i = lit.after;
                code_start = i;
                continue;
            }
        }
        i += 1;
    }
    push_code(&mut out, code_start, n);
    out
}

/// One lexed string literal.
struct Lit {
    /// Start of the literal INCLUDING its prefix (`@`, `$`, `r#`, `f`, `u8R`),
    /// so the code segment before it ends exactly here.
    prefix_start: usize,
    content_start: usize,
    content_end: usize,
    /// First offset after the closing delimiter.
    after: usize,
    interpolated: bool,
    /// See [`Segment::holes`].
    holes: Vec<(usize, usize)>,
}

/// Lex the string literal opening at `q`, or `None` if that quote does not open
/// one in this language (a Rust lifetime `'a`, a C# char literal, a stray
/// backtick).
fn string_at(lang: Lang, b: &[u8], q: usize) -> Option<Lit> {
    let quote = b[q];
    match lang {
        Lang::CSharp => return (quote == b'"').then(|| csharp_literal_at(b, q)).flatten(),
        Lang::C | Lang::Cpp => return (quote == b'"').then(|| c_literal_at(b, q)),
        _ => {}
    }
    let single_quotes_strings = matches!(lang, Lang::Python | Lang::Ts);
    let backtick_strings = matches!(lang, Lang::Ts | Lang::Go);
    match quote {
        // A Rust lifetime and a Go char literal must NOT open a string: hunting
        // for a closing `'` would swallow whole lines of code.
        b'\'' if !single_quotes_strings => return None,
        b'`' if !backtick_strings => return None,
        _ => {}
    }

    // The literal's prefix — the run of prefix-shaped bytes immediately before the
    // quote (`r`, `f`, `#`).
    let mut prefix_start = q;
    while prefix_start > 0
        && matches!(b[prefix_start - 1], b'@' | b'$' | b'#' | b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_')
    {
        prefix_start -= 1;
    }
    let prefix = std::str::from_utf8(&b[prefix_start..q]).unwrap_or("");

    let mut interpolated = false;
    let mut raw_hashes: Option<usize> = None;
    let mut triple = false;

    match lang {
        Lang::Rust => {
            let hashes = prefix.chars().rev().take_while(|c| *c == '#').count();
            let head = &prefix[..prefix.len() - hashes];
            if head.ends_with('r') || head.ends_with("br") {
                raw_hashes = Some(hashes);
            }
        }
        Lang::Python => {
            let low = prefix.to_ascii_lowercase();
            interpolated = low.ends_with('f') || low.ends_with("rf") || low.ends_with("fr");
            triple = b[q..].starts_with(&[quote, quote, quote]);
        }
        // A TS template's holes are known only once its end is: below.
        Lang::Ts | Lang::Go => {}
        Lang::Java => triple = b[q..].starts_with(b"\"\"\""),
        // Lexed above.
        Lang::CSharp | Lang::C | Lang::Cpp => {}
    }

    let (content_start, content_end, after) = if let Some(hashes) = raw_hashes {
        // Rust raw string: the terminator is `"` plus the same number of `#`.
        let cs = q + 1;
        let mut j = cs;
        loop {
            match find_byte(b, j, b'"') {
                None => break (cs, b.len(), b.len()),
                Some(k) => {
                    if b[k + 1..]
                        .iter()
                        .take(hashes)
                        .filter(|c| **c == b'#')
                        .count()
                        == hashes
                    {
                        break (cs, k, k + 1 + hashes);
                    }
                    j = k + 1;
                }
            }
        }
    } else if triple {
        let cs = q + 3;
        match find_seq(b, cs, &[quote, quote, quote]) {
            Some(k) => (cs, k, k + 3),
            None => (cs, b.len(), b.len()),
        }
    } else if quote == b'`' && lang == Lang::Ts {
        // A TS template reads backslash escapes — `\${` opens no hole and
        // `` \` `` closes nothing — and is interpolated by its holes, not by
        // its quote: one without an unescaped `${` is one fixed text, read
        // like any other string.
        let cs = q + 1;
        let mut j = cs;
        loop {
            match b.get(j) {
                None => break (cs, j, j),
                Some(b'\\') => j += 2,
                Some(b'`') => break (cs, j, j + 1),
                Some(b'$') if b.get(j + 1) == Some(&b'{') => {
                    interpolated = true;
                    j += 2;
                }
                Some(_) => j += 1,
            }
        }
    } else if quote == b'`' {
        // A Go raw string: no escapes, the next backtick ends it.
        let cs = q + 1;
        match find_byte(b, cs, b'`') {
            Some(k) => (cs, k, k + 1),
            None => (cs, b.len(), b.len()),
        }
    } else {
        // The ordinary form: backslash escapes, and a newline means the literal was
        // never closed — stop there so the damage cannot spread past one line.
        let cs = q + 1;
        let mut j = cs;
        loop {
            match b.get(j) {
                None => break (cs, j, j),
                Some(b'\\') => j += 2,
                Some(b'\n') => break (cs, j, j),
                Some(c) if *c == quote => break (cs, j, j + 1),
                Some(_) => j += 1,
            }
        }
    };

    Some(Lit {
        prefix_start,
        content_start,
        content_end: content_end.min(b.len()),
        after: after.min(b.len()),
        interpolated,
        holes: Vec::new(),
    })
}

/// The length of a character literal at `i` (`'x'`, `'"'`, `'\''`, `'\x41'`,
/// `'é'`, `'\101'`, `'č'`), read by its exact shape, or `None` when the
/// apostrophe opens none — a C++14 digit separator, an apostrophe in a
/// preprocessor line, prose in a directive.
fn char_literal_len(b: &[u8], i: usize) -> Option<usize> {
    match *b.get(i + 1)? {
        b'\'' | b'\n' | b'\r' => None,
        // An escape: the escaped byte, then up to eight more (hex, octal or a
        // `\U` code point) before the closing apostrophe, never past a line.
        b'\\' => {
            let escaped = *b.get(i + 2)?;
            if escaped == b'\n' || escaped == b'\r' {
                return None;
            }
            let rest = b.get(i + 3..)?;
            let close = rest.iter().take(9).position(|c| *c == b'\'')?;
            rest[..close]
                .iter()
                .all(|c| c.is_ascii_alphanumeric())
                .then_some(close + 4)
        }
        // One character, however many bytes it takes.
        lead => {
            let width = match lead {
                0x00..=0x7f => 1,
                0xc0..=0xdf => 2,
                0xe0..=0xef => 3,
                _ => 4,
            };
            (b.get(i + 1 + width) == Some(&b'\'')).then_some(width + 2)
        }
    }
}

/// How a C# literal's text reads.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CsKind {
    /// `"…"`: `\` escapes, closed by a line break.
    Regular,
    /// `@"…"`: `""` escapes, across lines.
    Verbatim,
    /// `"""…"""` with this many quotes (three or more): no escapes, across
    /// lines, closed by a run of at least as many.
    Raw(usize),
}

/// A C# literal opening at `p` — its `$`/`@` prefix included: where its quotes
/// start, how it reads, and how many `$` it has (0 when not interpolated; a
/// raw literal's hole opens with that many braces).
fn cs_opening(b: &[u8], p: usize) -> Option<(usize, CsKind, usize)> {
    let prefix = b
        .get(p..)?
        .iter()
        .take_while(|c| matches!(c, b'$' | b'@'))
        .count();
    let q = p + prefix;
    if b.get(q) != Some(&b'"') {
        return None;
    }
    let ats = b[p..q].iter().filter(|c| **c == b'@').count();
    if ats > 1 {
        return None;
    }
    let quotes = b[q..].iter().take_while(|c| **c == b'"').count();
    let kind = if ats == 1 {
        CsKind::Verbatim
    } else if quotes >= 3 {
        CsKind::Raw(quotes)
    } else {
        CsKind::Regular
    };
    Some((q, kind, prefix - ats))
}

/// Lex the C# literal whose (first) quote is at `q`: regular, verbatim
/// (`@"…"`), raw (`"""…"""`), and each of them interpolated (`$"…"`,
/// `$@"…"`, `@$"…"`, `$$"""…"""`) with its holes recorded.
///
/// The grammar is the one `verify::mask_string_literals` documents, read here
/// by bytes and kept to what this extractor needs: a hole is code, so a
/// comment, a character literal, a nested literal of any kind and bracket
/// depth are tokens of it, and a brace or a quote inside one never closes the
/// hole or the literal; a `:` at depth 0 starts the format part, which is text
/// by the outer literal's rules. A raw literal's hole opens with as many braces
/// as it has `$` — fewer are content. Its limits are that masker's: the
/// conditions of `#if` are not evaluated, and `global::` at a hole's depth 0
/// reads as a format part.
fn csharp_literal_at(b: &[u8], q: usize) -> Option<Lit> {
    let mut p = q;
    while p > 0 && matches!(b[p - 1], b'$' | b'@') {
        p -= 1;
    }
    let (q, kind, dollars) = cs_opening(b, p)?;
    let content_start = match kind {
        CsKind::Raw(quotes) => q + quotes,
        CsKind::Regular | CsKind::Verbatim => q + 1,
    };
    let body = cs_body(b, content_start, kind, dollars);
    Some(Lit {
        prefix_start: p,
        content_start,
        content_end: body.end,
        after: body.after,
        interpolated: dollars > 0,
        holes: body.holes,
    })
}

/// A C# literal's body as read: where its text ends, where the literal does,
/// and its holes.
struct CsBody {
    end: usize,
    after: usize,
    holes: Vec<(usize, usize)>,
}

/// How an interpolation hole ended.
enum HoleEnd {
    /// By its closing brace(s), the first offset after them; the literal's
    /// text resumes.
    Closed(usize),
    /// Its format part reached the literal's closing quotes: `(start, after)`.
    LiteralClosed(usize, usize),
    /// The literal stops here unclosed — a line break in a regular literal's
    /// format, or the end of the text.
    Open(usize),
}

fn run_of(b: &[u8], i: usize, c: u8) -> usize {
    b[i..].iter().take_while(|x| **x == c).count()
}

fn is_line_break(c: u8) -> bool {
    c == b'\n' || c == b'\r'
}

/// Read a C# literal's body from `start`, just after its opening quotes.
fn cs_body(b: &[u8], start: usize, kind: CsKind, dollars: usize) -> CsBody {
    let n = b.len();
    let mut holes = Vec::new();
    let mut i = start;
    loop {
        let Some(&c) = b.get(i) else {
            return CsBody {
                end: n,
                after: n,
                holes,
            };
        };
        if dollars > 0 && c == b'{' {
            let braces = run_of(b, i, b'{');
            let (opens, opener) = match kind {
                // Braces beyond the `$` count are content.
                CsKind::Raw(_) => (braces >= dollars, dollars),
                // `{{` is an escaped brace.
                CsKind::Regular | CsKind::Verbatim => (braces % 2 == 1, 1),
            };
            i += braces;
            if !opens {
                continue;
            }
            let hole_start = i - opener;
            match cs_hole(b, i, kind, dollars) {
                HoleEnd::Closed(after) => {
                    holes.push((hole_start, after));
                    i = after;
                }
                HoleEnd::LiteralClosed(end, after) => {
                    holes.push((hole_start, end));
                    return CsBody { end, after, holes };
                }
                HoleEnd::Open(at) => {
                    holes.push((hole_start, at));
                    return CsBody {
                        end: at,
                        after: at,
                        holes,
                    };
                }
            }
            continue;
        }
        match (kind, c) {
            (CsKind::Raw(quotes), b'"') => {
                let run = run_of(b, i, b'"');
                if run >= quotes {
                    return CsBody {
                        end: i,
                        after: i + run,
                        holes,
                    };
                }
                i += run;
            }
            (CsKind::Verbatim, b'"') if b.get(i + 1) == Some(&b'"') => i += 2,
            (_, b'"') => {
                return CsBody {
                    end: i,
                    after: i + 1,
                    holes,
                }
            }
            (CsKind::Regular, b'\\') if b.get(i + 1).is_some_and(|x| !is_line_break(*x)) => i += 2,
            // A regular literal a line ends inside was never closed: stop
            // there, so the damage cannot spread past one line.
            (CsKind::Regular, x) if is_line_break(x) => {
                return CsBody {
                    end: i,
                    after: i,
                    holes,
                }
            }
            _ => i += 1,
        }
    }
}

/// Read an interpolation hole from `i`, just after its opening brace(s): code,
/// until the brace that closes it at depth 0, or its format part.
fn cs_hole(b: &[u8], mut i: usize, kind: CsKind, dollars: usize) -> HoleEnd {
    let mut depth = 0usize;
    loop {
        let Some(&c) = b.get(i) else {
            return HoleEnd::Open(b.len());
        };
        if c == b'/' && b.get(i + 1) == Some(&b'/') {
            i = find_byte(b, i, b'\n').unwrap_or(b.len());
            continue;
        }
        if c == b'/' && b.get(i + 1) == Some(&b'*') {
            i = find_seq(b, i + 2, b"*/").map_or(b.len(), |p| p + 2);
            continue;
        }
        if c == b'\'' {
            if let Some(len) = char_literal_len(b, i) {
                i += len;
                continue;
            }
        }
        if matches!(c, b'"' | b'$' | b'@') {
            if let Some((q, inner, inner_dollars)) = cs_opening(b, i) {
                let start = match inner {
                    CsKind::Raw(quotes) => q + quotes,
                    CsKind::Regular | CsKind::Verbatim => q + 1,
                };
                i = cs_body(b, start, inner, inner_dollars).after;
                continue;
            }
        }
        match c {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' => depth = depth.saturating_sub(1),
            b'}' if depth > 0 => depth -= 1,
            b'}' => {
                let closers = match kind {
                    CsKind::Raw(_) => dollars,
                    CsKind::Regular | CsKind::Verbatim => 1,
                };
                return HoleEnd::Closed(i + run_of(b, i, b'}').min(closers).max(1));
            }
            b':' if depth == 0 => return cs_format(b, i + 1, kind, dollars),
            _ => {}
        }
        i += 1;
    }
}

/// Read a hole's format part from `i`, just after its `:` — text by the outer
/// literal's rules, until the hole's closing brace(s).
fn cs_format(b: &[u8], mut i: usize, kind: CsKind, dollars: usize) -> HoleEnd {
    loop {
        let Some(&c) = b.get(i) else {
            return HoleEnd::Open(b.len());
        };
        match (kind, c) {
            (CsKind::Raw(quotes), b'"') => {
                let run = run_of(b, i, b'"');
                if run >= quotes {
                    return HoleEnd::LiteralClosed(i, i + run);
                }
                i += run;
            }
            (CsKind::Verbatim, b'"') if b.get(i + 1) == Some(&b'"') => i += 2,
            (_, b'"') => return HoleEnd::LiteralClosed(i, i + 1),
            (CsKind::Raw(_), b'}') => {
                let run = run_of(b, i, b'}');
                if run >= dollars {
                    return HoleEnd::Closed(i + dollars);
                }
                i += run;
            }
            (_, b'}') => return HoleEnd::Closed(i + 1),
            (CsKind::Regular, b'\\') if b.get(i + 1).is_some_and(|x| !is_line_break(*x)) => i += 2,
            (CsKind::Regular, x) if is_line_break(x) => return HoleEnd::Open(i),
            _ => i += 1,
        }
    }
}

/// Lex the C or C++ literal whose quote is at `q` (plan B, U4): an ordinary
/// one with its encoding prefix (`L`, `u8`, `u`, `U`), or a raw one
/// (`R"delim(…)delim"`, with any of those prefixes before the `R`).
///
/// An identifier glued to the quote that is no prefix — a macro pasted
/// against a literal — is code, and the literal starts at the quote. A raw
/// delimiter is at most sixteen characters with no space, parenthesis or
/// backslash; one that breaks those rules is no raw literal, and the quote is
/// read as an ordinary one.
fn c_literal_at(b: &[u8], q: usize) -> Lit {
    let mut p = q;
    while p > 0 && (b[p - 1].is_ascii_alphanumeric() || b[p - 1] == b'_') {
        p -= 1;
    }
    let (prefix_start, raw) = match &b[p..q] {
        b"" | b"L" | b"u8" | b"u" | b"U" => (p, false),
        b"R" | b"LR" | b"u8R" | b"uR" | b"UR" => (p, true),
        _ => (q, false),
    };
    if raw {
        let delim_len = b[q + 1..].iter().take(17).position(|c| *c == b'(');
        if let Some(d) = delim_len {
            let delim = &b[q + 1..q + 1 + d];
            if delim
                .iter()
                .all(|c| !matches!(c, b' ' | b')' | b'\\' | b'\t' | b'\n' | b'\r' | b'"'))
            {
                let content_start = q + 1 + d + 1;
                let mut close = Vec::with_capacity(d + 2);
                close.push(b')');
                close.extend_from_slice(delim);
                close.push(b'"');
                let (content_end, after) = match find_seq(b, content_start, &close) {
                    Some(k) => (k, k + close.len()),
                    None => (b.len(), b.len()),
                };
                return Lit {
                    prefix_start,
                    content_start,
                    content_end,
                    after,
                    interpolated: false,
                    holes: Vec::new(),
                };
            }
        }
    }
    // The ordinary form: backslash escapes (a backslash before the line break
    // continues the literal on the next line), closed by a quote or ended,
    // unclosed, by a line break.
    let content_start = q + 1;
    let mut j = content_start;
    let (content_end, after) = loop {
        match b.get(j) {
            None => break (b.len(), b.len()),
            // A CRLF break is one line break: stepping over only its `\r`
            // would leave the `\n` to end the literal a line early.
            Some(b'\\') if b.get(j + 1) == Some(&b'\r') && b.get(j + 2) == Some(&b'\n') => j += 3,
            Some(b'\\') => j += 2,
            Some(b'\n') => break (j, j),
            Some(b'"') => break (j, j + 1),
            Some(_) => j += 1,
        }
    };
    Lit {
        prefix_start,
        content_start,
        content_end: content_end.min(b.len()),
        after: after.min(b.len()),
        interpolated: false,
        holes: Vec::new(),
    }
}

fn find_byte(b: &[u8], from: usize, needle: u8) -> Option<usize> {
    b.get(from..)?
        .iter()
        .position(|c| *c == needle)
        .map(|p| p + from)
}

fn find_seq(b: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    let hay = b.get(from..)?;
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    (0..=hay.len() - needle.len())
        .find(|i| &hay[*i..*i + needle.len()] == needle)
        .map(|p| p + from)
}

// ---- The identifier / qualified-path scan ----

/// Every identifier and qualified path in one code segment, as borrowed slices —
/// no allocation per token, only per match downstream.
fn paths_in(lang: Lang, src: &str, start: usize, end: usize) -> Vec<(usize, &str)> {
    let b = src.as_bytes();
    let sep = lang.sep().as_bytes();
    let mut out = Vec::new();
    let mut i = start;
    while i < end {
        if !is_ident_start(b[i]) {
            i += 1;
            continue;
        }
        let path_start = i;
        loop {
            while i < end && is_ident_continue(b[i]) {
                i += 1;
            }
            // Extend over `.`/`::` only when a further identifier really follows.
            let next = i + sep.len();
            if next < end && &b[i..next] == sep && is_ident_start(b[next]) {
                i = next;
                continue;
            }
            break;
        }
        out.push((path_start, &src[path_start..i]));
    }
    out
}

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_'
}

fn is_ident_continue(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

// ---- Imports ----

/// One import statement that a manifest-declared dependency backs.
struct Import {
    /// The name it binds in this file, when it binds one. `None` for a C#
    /// namespace/static `using`, a Rust glob, a side-effect TS import — those name
    /// nothing a usage site could be attributed to.
    local: Option<String>,
    /// The external identity, spelled as the source spells it.
    fqname: String,
    /// Byte offset of the statement, for the file-level fallback edge.
    offset: usize,
}

/// Every import in `src` whose module a manifest declares as a dependency.
fn imports(lang: Lang, src: &str, deps: &Deps) -> Vec<Import> {
    match lang {
        Lang::Rust => rust_imports(src, deps),
        Lang::CSharp => csharp_imports(src, deps),
        Lang::Ts => ts_imports(src, deps),
        Lang::Go => go_imports(src, deps),
        // No Python manifest is parsed yet, so there is no authority to match an
        // `import django` against — see the module doc.
        Lang::Python => Vec::new(),
        // Same reason: no Maven/Gradle manifest is read.
        Lang::Java => Vec::new(),
        // An `#include` names a header, never a declared dependency.
        Lang::C | Lang::Cpp => Vec::new(),
    }
}

/// Whether a line is commented out — the cheap guard that keeps a commented
/// import from becoming a live one. (Full lexing is reserved for literals and
/// identifiers, where a comment can hide anything; an import statement is a
/// one-line shape at the top of a file.)
fn is_comment_line(trimmed: &str) -> bool {
    trimmed.starts_with("//") || trimmed.starts_with('#') || trimmed.starts_with('*')
}

/// Statements of the form `<keyword> … ;`, joined across lines, with the byte
/// offset each one starts at.
fn joined_statements(src: &str, keywords: &[&str]) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    let mut pending: Option<(usize, String)> = None;
    for raw in src.split_inclusive('\n') {
        let line_offset = offset;
        offset += raw.len();
        let trimmed = raw.trim();
        if let Some((start, mut acc)) = pending.take() {
            acc.push(' ');
            acc.push_str(trimmed);
            if trimmed.contains(';') {
                out.push((start, acc));
            } else if acc.len() < 4096 {
                pending = Some((start, acc));
            }
            continue;
        }
        if is_comment_line(trimmed) {
            continue;
        }
        if !keywords.iter().any(|k| trimmed.starts_with(k)) {
            continue;
        }
        if trimmed.contains(';') {
            out.push((line_offset, trimmed.to_string()));
        } else {
            pending = Some((line_offset, trimmed.to_string()));
        }
    }
    out
}

/// Rust `use` paths: `use serde::Serialize;`, `use serde::{Serialize, de::Error};`,
/// `use serde_json as json;`, `use serde::*;`. A NESTED brace group is skipped
/// whole rather than half-parsed.
fn rust_imports(src: &str, deps: &Deps) -> Vec<Import> {
    let mut out = Vec::new();
    for (offset, stmt) in joined_statements(src, &["use ", "pub use ", "pub(crate) use "]) {
        let Some(body) = stmt.split_once("use ").map(|(_, rest)| rest) else {
            continue;
        };
        let body = body.split(';').next().unwrap_or(body).trim();
        if !deps.declares_import(Lang::Rust, body) {
            continue;
        }
        match body.split_once('{') {
            None => out.extend(rust_import_leaf(body, "", offset)),
            Some((prefix, rest)) => {
                let Some(inner) = rest.rsplit_once('}').map(|(inner, _)| inner) else {
                    continue;
                };
                if inner.contains('{') {
                    continue; // nested groups: emit nothing rather than guess
                }
                let prefix = prefix.trim().trim_end_matches(':');
                for item in inner.split(',') {
                    out.extend(rust_import_leaf(item.trim(), prefix, offset));
                }
            }
        }
    }
    out
}

/// One leaf of a Rust `use`, given the group prefix it hangs under.
fn rust_import_leaf(item: &str, prefix: &str, offset: usize) -> Option<Import> {
    let item = item.trim();
    if item.is_empty() {
        return None;
    }
    let join = |tail: &str| {
        if prefix.is_empty() {
            tail.to_string()
        } else if tail.is_empty() {
            prefix.to_string()
        } else {
            format!("{prefix}::{tail}")
        }
    };
    // `x as y` binds `y`; a glob or `self` binds nothing nameable.
    if let Some((path, alias)) = item.split_once(" as ") {
        let alias = alias.trim();
        return Some(Import {
            local: (!alias.is_empty() && alias != "_").then(|| alias.to_string()),
            fqname: join(path.trim()),
            offset,
        });
    }
    if item == "*" || item == "self" {
        return Some(Import {
            local: None,
            fqname: join(""),
            offset,
        });
    }
    if item.ends_with("::*") {
        return Some(Import {
            local: None,
            fqname: join(item.trim_end_matches("::*")),
            offset,
        });
    }
    let last = item.rsplit("::").next().unwrap_or(item).trim();
    Some(Import {
        local: (!last.is_empty()).then(|| last.to_string()),
        fqname: join(item),
        offset,
    })
}

/// C# `using` directives. A `using (…)` statement and a `using var x = …;` are
/// excluded by the shape itself: the pattern demands a dotted name then `;`.
fn csharp_imports(src: &str, deps: &Deps) -> Vec<Import> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(
            r"^\s*(?:global\s+)?using\s+(?:static\s+)?(?:([A-Za-z_]\w*)\s*=\s*)?([A-Za-z_][\w.]*)\s*;",
        )
        .unwrap()
    });
    let mut out = Vec::new();
    let mut offset = 0usize;
    for raw in src.split_inclusive('\n') {
        let line_offset = offset;
        offset += raw.len();
        if is_comment_line(raw.trim()) {
            continue;
        }
        let Some(caps) = re.captures(raw) else {
            continue;
        };
        let path = caps.get(2).map(|m| m.as_str()).unwrap_or_default();
        if !deps.declares_import(Lang::CSharp, path) {
            continue;
        }
        out.push(Import {
            local: caps.get(1).map(|m| m.as_str().to_string()),
            fqname: path.to_string(),
            offset: line_offset,
        });
    }
    out
}

/// TS/TSX imports. A bare specifier is only accepted when a `package.json`
/// declares it; the specifier is the module identity, and a named import hangs off
/// it as `<spec>.<name>`.
fn ts_imports(src: &str, deps: &Deps) -> Vec<Import> {
    static RE_FROM: OnceLock<Regex> = OnceLock::new();
    static RE_BARE: OnceLock<Regex> = OnceLock::new();
    static RE_NS: OnceLock<Regex> = OnceLock::new();
    let re_from = RE_FROM.get_or_init(|| {
        Regex::new(r#"^\s*import\s+(?:type\s+)?(.+?)\s+from\s*['"]([^'"]+)['"]"#).unwrap()
    });
    let re_bare = RE_BARE.get_or_init(|| Regex::new(r#"^\s*import\s*['"]([^'"]+)['"]"#).unwrap());
    let re_ns = RE_NS.get_or_init(|| Regex::new(r"\*\s*as\s+([A-Za-z_$][\w$]*)").unwrap());

    let mut out = Vec::new();
    let mut offset = 0usize;
    for raw in src.split_inclusive('\n') {
        let line_offset = offset;
        offset += raw.len();
        if is_comment_line(raw.trim()) {
            continue;
        }
        if let Some(caps) = re_from.captures(raw) {
            let spec = caps.get(2).map(|m| m.as_str()).unwrap_or_default();
            if !deps.declares_import(Lang::Ts, spec) {
                continue;
            }
            let clause = caps.get(1).map(|m| m.as_str()).unwrap_or_default();
            // `* as ns` binds the module itself.
            if let Some(ns) = re_ns.captures(clause) {
                out.push(Import {
                    local: Some(ns[1].to_string()),
                    fqname: spec.to_string(),
                    offset: line_offset,
                });
            }
            // A default import binds the module's default export.
            let head = clause.split(['{', ',']).next().unwrap_or("").trim();
            if !head.is_empty() && !head.contains('*') {
                out.push(Import {
                    local: Some(head.to_string()),
                    fqname: format!("{spec}.default"),
                    offset: line_offset,
                });
            }
            // `{ a, b as c }` — each name is a member of the module.
            if let Some((_, rest)) = clause.split_once('{') {
                let inner = rest.split_once('}').map(|(i, _)| i).unwrap_or(rest);
                for item in inner.split(',') {
                    let item = item.trim().trim_start_matches("type ").trim();
                    if item.is_empty() {
                        continue;
                    }
                    let (name, local) = match item.split_once(" as ") {
                        Some((n, a)) => (n.trim(), a.trim()),
                        None => (item, item),
                    };
                    out.push(Import {
                        local: Some(local.to_string()),
                        fqname: format!("{spec}.{name}"),
                        offset: line_offset,
                    });
                }
            }
            continue;
        }
        if let Some(caps) = re_bare.captures(raw) {
            let spec = caps.get(1).map(|m| m.as_str()).unwrap_or_default();
            if deps.declares_import(Lang::Ts, spec) {
                out.push(Import {
                    local: None,
                    fqname: spec.to_string(),
                    offset: line_offset,
                });
            }
        }
    }
    out
}

/// Go imports, both the single-line and the parenthesized block form. An import
/// binds the path's last segment unless it is aliased; `_` and `.` bind nothing.
fn go_imports(src: &str, deps: &Deps) -> Vec<Import> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re =
        RE.get_or_init(|| Regex::new(r#"^\s*(?:import\s+)?(?:([\w.]+)\s+)?"([^"]+)"#).unwrap());
    let mut out = Vec::new();
    let mut in_block = false;
    let mut offset = 0usize;
    for raw in src.split_inclusive('\n') {
        let line_offset = offset;
        offset += raw.len();
        let trimmed = raw.trim();
        if is_comment_line(trimmed) {
            continue;
        }
        if trimmed.starts_with("import") && trimmed.contains('(') {
            in_block = true;
            continue;
        }
        if in_block && trimmed.starts_with(')') {
            in_block = false;
            continue;
        }
        if !in_block && !trimmed.starts_with("import") {
            continue;
        }
        let Some(caps) = re.captures(trimmed) else {
            continue;
        };
        let path = caps.get(2).map(|m| m.as_str()).unwrap_or_default();
        if !deps.declares_import(Lang::Go, path) {
            continue;
        }
        let alias = caps.get(1).map(|m| m.as_str());
        let local = match alias {
            Some("_") | Some(".") => None,
            Some(a) => Some(a.to_string()),
            None => path.rsplit('/').next().map(|s| s.to_string()),
        };
        out.push(Import {
            local,
            fqname: path.to_string(),
            offset: line_offset,
        });
    }
    out
}

// ---- Literal patterns ----

/// A qualified SQL object name: one or two dots, optional `[…]` quoting.
const QUAL: &str = r"((?:\[?[A-Za-z_][A-Za-z0-9_]*\]?\.){1,2}\[?[A-Za-z_][A-Za-z0-9_]*\]?)";

/// Which SQL keywords a literal carries — the gate that decides whether it is a
/// statement at all, in ONE pass.
#[derive(Default)]
struct SqlGate {
    select: bool,
    delete: bool,
    from: bool,
    set: bool,
    update: bool,
    insert_into: bool,
    merge_into: bool,
    exec: bool,
}

impl SqlGate {
    /// Whether any arm of [`sql_objects`] could fire — the literal is a
    /// statement at all. The same test decides whether [`sql_accesses`] reads
    /// it, so a sentence that merely says "update" and "from" stays prose for
    /// both.
    fn admits(&self) -> bool {
        ((self.select || self.delete) && self.from)
            || self.insert_into
            || self.merge_into
            || (self.update && self.set)
            || self.exec
    }
}

fn sql_gate(lit: &str) -> SqlGate {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(
            r"(?i)\b(select|delete|from|set|update|insert\s+into|merge\s+into|exec|execute)\b",
        )
        .unwrap()
    });
    let mut gate = SqlGate::default();
    for m in re.find_iter(lit) {
        let word: String = m
            .as_str()
            .to_ascii_lowercase()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        match word.as_str() {
            "select" => gate.select = true,
            "delete" => gate.delete = true,
            "from" => gate.from = true,
            "set" => gate.set = true,
            "update" => gate.update = true,
            "insert into" => gate.insert_into = true,
            "merge into" => gate.merge_into = true,
            "exec" | "execute" => gate.exec = true,
            _ => {}
        }
    }
    gate
}

/// Qualified database objects a literal names, with their offset inside it.
///
/// Every arm needs the statement's VERB and its companion keyword, and extracts
/// only at the position that arm defines. That pairing is what keeps prose out:
/// "Update config.value from settings.json" carries `update` and `from` but no
/// `set` and no `select`, so no arm fires.
fn sql_objects(lit: &str, gate: &SqlGate) -> Vec<(usize, String)> {
    static RE_FROM: OnceLock<Regex> = OnceLock::new();
    static RE_INTO: OnceLock<Regex> = OnceLock::new();
    static RE_UPDATE: OnceLock<Regex> = OnceLock::new();
    static RE_EXEC: OnceLock<Regex> = OnceLock::new();
    let compile = |head: &str| Regex::new(&format!(r"(?i)\b{head}\s+{QUAL}")).unwrap();

    let mut out: Vec<(usize, String)> = Vec::new();
    let mut collect = |re: &Regex| {
        for caps in re.captures_iter(lit) {
            let Some(m) = caps.get(1) else { continue };
            let name = normalize_object(m.as_str());
            // A dotted token whose tail is a file extension is a path, not a table
            // — the commonest way a "qualified name" turns out to be prose.
            if name.is_empty() || looks_like_a_file(&name) {
                continue;
            }
            out.push((m.start(), name));
        }
    };

    if (gate.select || gate.delete) && gate.from {
        collect(RE_FROM.get_or_init(|| compile("(?:from|join)")));
    }
    if gate.insert_into || gate.merge_into {
        collect(RE_INTO.get_or_init(|| compile("(?:insert|merge)\\s+into")));
    }
    if gate.update && gate.set {
        collect(RE_UPDATE.get_or_init(|| compile("update")));
    }
    if gate.exec {
        collect(RE_EXEC.get_or_init(|| compile("(?:exec|execute)")));
    }
    out
}

/// The verbs that start a statement. A `WITH` prefix is not among them: its
/// common-table expressions sit in parentheses ahead of the verb, outside any
/// statement — what their own `SELECT` names is read
/// ([`DbAccessKind::Reads`]), and the statement after them keeps its target.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Verb {
    Select,
    Insert,
    Update,
    Delete,
    Merge,
    Exec,
}

/// Words that end a table reference rather than alias it — what may follow
/// `FROM dba.t` without being its alias — and that never name a target.
const NOT_AN_ALIAS: &[&str] = &[
    "where",
    "on",
    "inner",
    "left",
    "right",
    "full",
    "outer",
    "cross",
    "join",
    "with",
    "group",
    "order",
    "having",
    "union",
    "except",
    "intersect",
    "set",
    "select",
    "from",
    "when",
    "then",
    "using",
    "option",
    "for",
    "into",
    "values",
    "output",
    "and",
    "or",
    "not",
    "exists",
    "in",
    "is",
    "apply",
    "pivot",
    "unpivot",
    "natural",
    "update",
    "delete",
    "insert",
    "merge",
    "exec",
    "execute",
    "as",
    "top",
];

/// Every qualified database object a SQL literal names, with what its
/// statement does with it, at the object's offset inside the literal
/// (plan B, U2: R11, KTD2). Nothing unless `gate` admits the literal at all.
///
/// The state is read per STATEMENT, never from the word right before the
/// object. SQL comments and `'…'` strings are blanked first — with spaces, so
/// every offset still points into the literal — which also keeps a template
/// directive passed as a string (`'#SELECT;'`) out of the statement. The
/// statements are then cut at `;` and at every verb outside parentheses: a
/// subquery stays in the statement around it, while `INSERT … SELECT` and two
/// statements on two lines with no `;` between them come apart.
///
/// In each statement the target of a change is found and resolved: a
/// qualified name directly, an unqualified one through the aliases `FROM`,
/// `JOIN` and `USING` declare (or, failing that, as the last segment of
/// exactly one object), so `UPDATE t SET … FROM dba.t t` and
/// `DELETE t FROM dba.t t` write `dba.t`. Every other object the statement
/// names after `FROM`/`JOIN` (and `USING` in a `MERGE`) is read; `EXEC` calls
/// its procedure. A statement whose target cannot be resolved — or whose verb
/// lacks its companion (`UPDATE` without `SET`, `DELETE` without `FROM`,
/// `INSERT` without `INTO`) — mentions its objects and claims nothing more.
fn sql_accesses(lit: &str, gate: &SqlGate) -> Vec<(usize, String, DbAccessKind)> {
    static RE_CUT: OnceLock<Regex> = OnceLock::new();
    static RE_SOURCE: OnceLock<Regex> = OnceLock::new();
    static RE_QUAL: OnceLock<Regex> = OnceLock::new();
    static RE_SELECT: OnceLock<Regex> = OnceLock::new();
    let mut out = Vec::new();
    if !gate.admits() {
        return out;
    }
    let clean = blank_sql_noise(lit);
    let cut = RE_CUT.get_or_init(|| {
        Regex::new(r"(?i)[();]|\b(?:select|insert|update|delete|merge|execute|exec)\b").unwrap()
    });

    // Statement boundaries as (start, verb). Text before the first verb is a
    // statement without one.
    let mut statements: Vec<(usize, Option<Verb>)> = vec![(0, None)];
    let mut depth = 0usize;
    for m in cut.find_iter(&clean) {
        match m.as_str() {
            "(" => depth += 1,
            ")" => depth = depth.saturating_sub(1),
            ";" => {
                depth = 0;
                statements.push((m.end(), None));
            }
            word if depth == 0 => {
                let verb = match word.to_ascii_lowercase().as_str() {
                    "select" => Verb::Select,
                    "insert" => Verb::Insert,
                    "update" => Verb::Update,
                    "delete" => Verb::Delete,
                    "merge" => Verb::Merge,
                    _ => Verb::Exec,
                };
                statements.push((m.start(), Some(verb)));
            }
            _ => {}
        }
    }

    let source = RE_SOURCE
        .get_or_init(|| Regex::new(&format!(r"(?i)\b(from|join|using)\s+{QUAL}")).unwrap());
    let qual = RE_QUAL.get_or_init(|| Regex::new(&format!("^{QUAL}$")).unwrap());
    let select_word = RE_SELECT.get_or_init(|| Regex::new(r"(?i)\bselect\b").unwrap());
    let object = |raw: &str| {
        let name = normalize_object(raw);
        (!name.is_empty() && !looks_like_a_file(&name)).then_some(name)
    };

    for (i, &(start, verb)) in statements.iter().enumerate() {
        let end = statements.get(i + 1).map_or(clean.len(), |s| s.0);
        let text = &clean[start..end];

        // The objects named after FROM / JOIN (/ USING in a MERGE), each with
        // the alias it declares and how deep in parentheses it stands — the
        // depth counted as the matches come, in order, so the statement is
        // walked once.
        let mut sources: Vec<SqlSource> = Vec::new();
        let (mut walked, mut depth) = (0usize, 0usize);
        for caps in source.captures_iter(text) {
            let (Some(kw), Some(m)) = (caps.get(1), caps.get(2)) else {
                continue;
            };
            for c in text[walked..m.start()].bytes() {
                match c {
                    b'(' => depth += 1,
                    b')' => depth = depth.saturating_sub(1),
                    _ => {}
                }
            }
            walked = m.start();
            if kw.as_str().eq_ignore_ascii_case("using") && verb != Some(Verb::Merge) {
                continue;
            }
            let Some(name) = object(m.as_str()) else {
                continue;
            };
            sources.push(SqlSource {
                at: start + m.start(),
                name,
                alias: alias_after(&text[m.end()..]),
                depth,
            });
        }
        let all = |sources: Vec<SqlSource>, access: DbAccessKind| {
            sources.into_iter().map(move |s| (s.at, s.name, access))
        };

        let Some(verb) = verb else {
            // Ahead of every verb: a common-table expression's `SELECT`, in
            // parentheses that cut no statement, reads what it names. The
            // rest is named outside any statement and only mentioned.
            let select = select_word
                .find(text)
                .map_or(usize::MAX, |m| start + m.start());
            out.extend(sources.into_iter().map(|s| {
                let access = if s.at > select {
                    DbAccessKind::Reads
                } else {
                    DbAccessKind::Mentions
                };
                (s.at, s.name, access)
            }));
            continue;
        };
        // The word after the verb spells the target — after `INTO` or `FROM`
        // where the form has one, and after `@rc =` where an `EXEC` keeps a
        // return code.
        let verb_end = verb_len(text);
        let after_verb = &text[verb_end..];
        let (target_at, target, companion) = match verb {
            Verb::Select => {
                out.extend(all(sources, DbAccessKind::Reads));
                continue;
            }
            Verb::Exec => {
                let (at, word) = next_word(after_verb, None);
                if let Some(name) = qual.is_match(word).then(|| object(word)).flatten() {
                    out.push((start + verb_end + at, name, DbAccessKind::Calls));
                }
                out.extend(all(sources, DbAccessKind::Reads));
                continue;
            }
            Verb::Insert => {
                let (at, word) = next_word(after_verb, Some("into"));
                (at, word, has_word(&after_verb[..at], "into"))
            }
            Verb::Merge => {
                let (at, word) = next_word(after_verb, Some("into"));
                (at, word, true)
            }
            Verb::Update => {
                let (at, word) = next_word(after_verb, None);
                (at, word, has_word(text, "set"))
            }
            Verb::Delete => {
                let (at, word) = next_word(after_verb, Some("from"));
                (at, word, has_word(text, "from"))
            }
        };
        let target_at = start + verb_end + target_at;
        let qualified = qual.is_match(target);
        // The target is the occurrence the verb names — the name after it,
        // or the source its alias binds — and the statement's own FROM
        // naming that same table with no alias of its own (T-SQL's `UPDATE
        // dba.t SET … FROM dba.t`). Not every occurrence of its name: `UPDATE
        // dba.t SET x = (SELECT MAX(x) FROM dba.t)` reads the table it
        // writes, and a self-join's other alias is a read.
        let resolved = match (companion, qualified) {
            (false, _) => None,
            (true, true) => object(target).map(|name| {
                let mut written: Vec<usize> = sources
                    .iter()
                    .filter(|s| s.depth == 0 && s.alias.is_none() && s.name == name)
                    .map(|s| s.at)
                    .collect();
                written.push(target_at);
                (name, written)
            }),
            (true, false) => resolve_alias(target, &sources),
        };
        let Some((name, written)) = resolved else {
            out.extend(all(sources, DbAccessKind::Mentions));
            continue;
        };
        // `DELETE FROM dba.t` names its target after `FROM`: one occurrence,
        // already among the sources.
        if qualified && !sources.iter().any(|s| s.at == target_at) {
            out.push((target_at, name, DbAccessKind::Writes));
        }
        for s in sources {
            let access = if written.contains(&s.at) {
                DbAccessKind::Writes
            } else {
                DbAccessKind::Reads
            };
            out.push((s.at, s.name, access));
        }
    }
    out
}

/// The literal with its SQL comments and `'…'` strings overwritten by spaces,
/// byte for byte, so an offset into it is an offset into the literal. A
/// doubled `''` stays inside its string, as SQL reads it.
fn blank_sql_noise(lit: &str) -> String {
    let mut b = lit.as_bytes().to_vec();
    let n = b.len();
    let mut i = 0;
    while i < n {
        let end = match b[i] {
            b'-' if b.get(i + 1) == Some(&b'-') => find_byte(&b, i, b'\n').unwrap_or(n),
            b'/' if b.get(i + 1) == Some(&b'*') => find_seq(&b, i + 2, b"*/").map_or(n, |p| p + 2),
            b'\'' => {
                let mut j = i + 1;
                loop {
                    match find_byte(&b, j, b'\'') {
                        Some(k) if b.get(k + 1) == Some(&b'\'') => j = k + 2,
                        Some(k) => break k + 1,
                        None => break n,
                    }
                }
            }
            _ => {
                i += 1;
                continue;
            }
        };
        // Only ASCII delimiters open and close a range, so a range never
        // splits a character and the bytes stay valid UTF-8.
        b[i..end].fill(b' ');
        i = end;
    }
    String::from_utf8(b).unwrap_or_default()
}

/// The length of the verb a statement opens with.
fn verb_len(text: &str) -> usize {
    text.bytes()
        .position(|c| !c.is_ascii_alphabetic())
        .unwrap_or(text.len())
}

/// The next name-shaped word in `text` and its offset — past `optional` when
/// that word comes first (case-insensitive), and past an `@var =` assignment.
/// An empty word when there is none.
fn next_word<'t>(text: &'t str, optional: Option<&str>) -> (usize, &'t str) {
    let word_at = |from: usize| -> (usize, &'t str) {
        let rest = &text[from..];
        let start = from + (rest.len() - rest.trim_start().len());
        let len = text[start..]
            .bytes()
            .position(|c| {
                !(c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'[' | b']' | b'@' | b'#'))
            })
            .unwrap_or(text.len() - start);
        (start, &text[start..start + len])
    };
    let (mut at, mut word) = word_at(0);
    if optional.is_some_and(|o| word.eq_ignore_ascii_case(o)) {
        (at, word) = word_at(at + word.len());
    }
    if word.starts_with('@') {
        let rest = &text[at + word.len()..];
        if let Some(value) = rest.trim_start().strip_prefix('=') {
            (at, word) = word_at(text.len() - value.len());
        }
    }
    (at, word)
}

/// Whether `text` holds `word` as a whole word, ignoring case.
fn has_word(text: &str, word: &str) -> bool {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .any(|w| w.eq_ignore_ascii_case(word))
}

/// The alias a table reference declares right after its name — `dba.t t`,
/// `dba.t AS t` — or `None` when the next word ends the reference instead.
fn alias_after(rest: &str) -> Option<String> {
    let rest = rest.trim_start();
    let rest = match rest.get(..3) {
        Some(head) if head.eq_ignore_ascii_case("as ") => rest[3..].trim_start(),
        _ => rest,
    };
    let len = rest
        .bytes()
        .position(|c| !(c.is_ascii_alphanumeric() || c == b'_'))
        .unwrap_or(rest.len());
    let word = &rest[..len];
    let name_shaped = word
        .bytes()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == b'_');
    (name_shaped && !NOT_AN_ALIAS.iter().any(|k| word.eq_ignore_ascii_case(k)))
        .then(|| word.to_string())
}

/// An object a statement names after FROM / JOIN (/ USING in a MERGE).
struct SqlSource {
    /// Its offset in the literal.
    at: usize,
    name: String,
    /// The alias it declares.
    alias: Option<String>,
    /// How many parentheses it stands in within its statement: 0 in the
    /// statement's own FROM, more in a subquery.
    depth: usize,
}

/// The object an unqualified target stands for, and the offsets of the
/// sources that are the target itself: the sources declaring it as their
/// alias, or else the sources whose last segment it is — those without an
/// alias of their own when there are any, since an aliased source answers
/// to its alias — and of those the statement's own (depth 0) when there are
/// any, since a subquery's are reads. `None` when the object is not one —
/// no claim beats a wrong one.
fn resolve_alias(target: &str, sources: &[SqlSource]) -> Option<(String, Vec<usize>)> {
    let target = target.trim_matches(['[', ']']);
    if target.is_empty() || NOT_AN_ALIAS.iter().any(|k| target.eq_ignore_ascii_case(k)) {
        return None;
    }
    let by_alias: Vec<&SqlSource> = sources
        .iter()
        .filter(|s| {
            s.alias
                .as_deref()
                .is_some_and(|a| a.eq_ignore_ascii_case(target))
        })
        .collect();
    let mut candidates = if by_alias.is_empty() {
        sources
            .iter()
            .filter(|s| {
                s.name
                    .rsplit('.')
                    .next()
                    .is_some_and(|last| last.eq_ignore_ascii_case(target))
            })
            .collect()
    } else {
        by_alias
    };
    let names: BTreeSet<&str> = candidates.iter().map(|s| s.name.as_str()).collect();
    let mut it = names.into_iter();
    let (Some(only), None) = (it.next(), it.next()) else {
        return None;
    };
    if candidates.iter().any(|s| s.alias.is_none()) {
        candidates.retain(|s| s.alias.is_none());
    }
    if candidates.iter().any(|s| s.depth == 0) {
        candidates.retain(|s| s.depth == 0);
    }
    Some((only.to_string(), candidates.iter().map(|s| s.at).collect()))
}

/// `[db].[dbo].[T]` → `db.dbo.T`. Case is left exactly as the source writes it:
/// folding it would be a claim about the server's collation this cannot make.
fn normalize_object(raw: &str) -> String {
    raw.replace(['[', ']'], "").trim().to_string()
}

/// Whether a "qualified name" is really a file path — the commonest way prose or a
/// path slips past the keyword gate (`from settings.json`, `FROM app.config`).
/// Only extensions no one would name a table or procedure after are listed:
/// `Log`, `Tmp` and `Ini` are all plausible object names and deliberately absent.
fn looks_like_a_file(name: &str) -> bool {
    const EXTS: &[&str] = &[
        "json", "xml", "txt", "cs", "md", "config", "dll", "exe", "csv", "sql", "html", "htm",
        "js", "ts", "py", "rs", "go", "yaml", "yml", "zip", "pdf", "xlsx", "docx",
    ];
    name.rsplit('.')
        .next()
        .map(|e| EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// What an absolute URL in a literal turned out to be.
enum UrlHit {
    /// An endpoint: `(offset, host/path)`.
    Endpoint(usize, String),
    /// A URL carrying userinfo — `https://user:pass@host/…`. The credential is
    /// part of the value, so the whole URL is redacted and no endpoint is emitted.
    Credentials(usize),
}

fn urls(lit: &str) -> Vec<UrlHit> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re =
        RE.get_or_init(|| Regex::new(r#"(?i)\b(?:https?|ftp)://([^\s"'<>\\)\]},;]+)"#).unwrap());
    let mut out = Vec::new();
    for caps in re.captures_iter(lit) {
        let Some(m) = caps.get(1) else { continue };
        let rest = m.as_str();
        // A placeholder makes the endpoint a different one per call — not a fact
        // about one external resource.
        if rest.contains(['{', '}', '$']) {
            continue;
        }
        let host_path = rest
            .split(['?', '#'])
            .next()
            .unwrap_or(rest)
            .trim_end_matches(['/', '.', ',']);
        if host_path.is_empty() {
            continue;
        }
        let host = host_path.split('/').next().unwrap_or(host_path);
        if host.contains('@') {
            out.push(UrlHit::Credentials(m.start()));
            continue;
        }
        // An IP-literal host (`http://[::1]:8080`) is skipped: its brackets are
        // exactly the characters the match stops at, so the host would be recorded
        // truncated — and an address literal names no endpoint a reader could look
        // up anyway. Better nothing than a mangled key (KTD8).
        if host.contains(['[', ']']) {
            continue;
        }
        out.push(UrlHit::Endpoint(m.start(), host_path.to_string()));
    }
    out
}

/// Whether the code immediately before a literal is a NAMED configuration read.
/// The API call is the second anchor a config key needs: without it, a string that
/// merely looks like a key is no evidence that anything reads it.
fn is_config_read(code_tail: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        // Verbose mode, so the API list stays one readable name per line.
        Regex::new(
            r"(?x)
             (?:
                 ConfigurationManager \s* \. \s* (?: AppSettings | ConnectionStrings ) \s* \[
               | \. \s* GetConnectionString \s* \(
               | \. \s* GetSection \s* \(
               | \. \s* GetValue \s* (?: < [^<>()]* > )? \s* \(
               | os \s* \. \s* environ \s* \[
               | os \s* \. \s* environ \s* \. \s* get \s* \(
               | os \s* \. \s* getenv \s* \(
               | os \s* \. \s* Getenv \s* \(
               | process \s* \. \s* env \s* \[
               | (?: std \s* :: \s* )? env \s* :: \s* var (?: _os )? \s* \(
             )
             \s* $",
        )
        .unwrap()
    });
    // Only the tail can match an anchored pattern, and bounding it keeps the cost
    // per literal flat regardless of how long the enclosing statement is. The cut
    // is an arithmetic offset, not a token position, so it must be walked forward
    // onto a character boundary: a Czech identifier before the literal would
    // otherwise split a multi-byte character and panic, and an extractor never
    // panics (E4).
    let mut start = code_tail.len().saturating_sub(160);
    while start < code_tail.len() && !code_tail.is_char_boundary(start) {
        start += 1;
    }
    re.is_match(&code_tail[start..])
}

/// The redaction filter lives in `crates/ra-redact`, shared with every
// language plugin so the host and the guests agree on what a secret looks
// like. Re-exported here under the names the rest of the crate already uses.
pub use ra_redact::SecretPrefixes;
pub(crate) use ra_redact::{redact_secret_block, secret_pattern, REDACTION_MARKER};
use ra_redact::{secret_shapes, SecretShapes};

// ---- The secret filter around a literal (plan B, KTD11) ----

/// Which secret a literal is, read together with the code on its line (plan
/// B, KTD11 rule 2) — `None` when it is none.
///
/// The literal alone is what [`secret_pattern`] always read. The line adds
/// the two shapes a value carries no keyword of its own in: an assignment the
/// table knows (`password = "…"`, `apiKey: "…"`, `heslo = "…"`), read from
/// the start of the line through the literal; and an assignment to a name
/// whose last word says what the value is (`secret = "…"`, `_password =
/// "…"`, `SmtpPassword = "…"`, `GoogleApiKey: "…"`), which a keyword at a word
/// boundary never sees in C#'s spelling. Over-reading is the intended failure
/// direction: a withheld literal costs a reader one look at its line.
///
/// A literal of several words is a message more often than a value, and the
/// text support searches for (`lblPassword: "Zadejte heslo"`) must survive
/// (plan B stop condition) — so the line's patterns never speak for it. The
/// name it is assigned to still does, with evidence from the whole name: a
/// passphrase (`password = "correct horse battery staple"`) is withheld,
/// unless some word of the assigned name says the value is a message
/// (`lblPassword`, `Errors.Password`, see [`names_a_message`]).
///
/// The assignment is read on the literal's own line; a literal that starts
/// its line (`password =` then `"…"` below it) is read with the end of the
/// previous non-blank line instead ([`assigned_name_before`]).
///
/// Answered for every segment of the file at once, indexed like `segs`
/// (`None` for a segment that is no literal). One literal at a time, the
/// line would be read from its start once per literal on it — and a
/// minified bundle is one line holding thousands of them: 579 s for one
/// 11 MB vendor script of the reference C# repository, against a whole run of four minutes.
/// So the literals of a line are read together through
/// [`line_secret_patterns`], which reads the line a bounded number of times
/// and answers exactly what reading it per literal would.
fn literal_secrets(
    src: &str,
    segs: &[Segment],
    lines: &LineIndex,
    prefixes: &SecretPrefixes,
) -> Vec<Option<&'static str>> {
    let mut out = vec![None; segs.len()];
    // The literals the line must speak for: `(line start, literal end, index)`,
    // in source order — so one line's are adjacent, their ends ascending.
    let mut by_line: Vec<(usize, usize, usize)> = Vec::new();
    for (i, seg) in segs.iter().enumerate() {
        if !matches!(seg.kind, SegKind::Str { .. }) {
            continue;
        }
        let literal = &src[seg.start..seg.end];
        if let Some(pattern) = secret_pattern(literal, prefixes) {
            out[i] = Some(pattern);
            continue;
        }
        let line_start = lines.start(lines.line(seg.start));
        // The line only speaks for a VALUE-shaped literal. A literal with words
        // separated by whitespace is a message (`lblPassword: "Zadejte heslo"`,
        // `Errors.Password = "Heslo je krátké"`) — exactly the text support
        // searches for, which the plan's stop condition forbids losing — or a
        // passphrase, which only the name it is assigned to tells apart. A
        // secret inside such a text is still caught by its own pattern above.
        if literal
            .trim_matches(|c: char| c == '"' || c == '@' || c == '$' || c == '\'')
            .trim()
            .contains(char::is_whitespace)
        {
            out[i] = assigned_name_before(src, segs, i, line_start).and_then(|name| {
                secret_name(last_segment(name)).filter(|_| !names_a_message(name))
            });
            continue;
        }
        by_line.push((line_start, seg.end, i));
    }
    for line in by_line.chunk_by(|a, b| a.0 == b.0) {
        let line_start = line[0].0;
        let ends: Vec<usize> = line.iter().map(|&(_, end, _)| end).collect();
        let patterns = line_secret_patterns(src, line_start, &ends, prefixes);
        for (&(_, _, i), pattern) in line.iter().zip(patterns) {
            out[i] = pattern.or_else(|| {
                secret_name(last_segment(assigned_name_before(
                    src, segs, i, line_start,
                )?))
            });
        }
    }
    out
}

/// The name a literal is assigned to ([`assigned_name`]), read from the
/// code segment right before it: what precedes the literal on its line. A
/// literal that starts its line — nothing but indentation before it, or
/// nothing at all, `password =` on the line above — is read with the end of
/// the previous non-blank line of that same segment instead. One line back
/// and no further, so the cost per literal stays the code between it and
/// the literal before it.
///
/// A key written as a literal itself — `{ "password": "…" }` in JS, JSON or
/// a Python dict, `cfg["password"] = "…"` — leaves only its separator
/// between the two literals; the key's text is then the name.
fn assigned_name_before<'s>(
    src: &'s str,
    segs: &[Segment],
    i: usize,
    line_start: usize,
) -> Option<&'s str> {
    // A literal at column zero has its code segment end exactly at its line.
    let prev = segs
        .get(i.wrapping_sub(1))
        .filter(|prev| prev.kind == SegKind::Code && prev.end >= line_start)?;
    let between = src[prev.start..prev.end].trim();
    let separator = between.strip_prefix(']').map_or(between, str::trim_start);
    if matches!(separator, ":" | "=") {
        if let Some(key) = segs
            .get(i.wrapping_sub(2))
            .filter(|key| matches!(key.kind, SegKind::Str { .. }))
        {
            return Some(src[key.start..key.end].trim());
        }
    }
    let on_line = &src[prev.start.max(line_start)..prev.end];
    // A property whose accessors take lines of their own (`Password` / `{` /
    // `get;` / `set;` / `} = "…"`, the value on that line or below it) ends
    // the code before its value in `} =`: its name is read from the line that
    // opens the accessor block, wherever the value itself starts. Bounded, so
    // code between two literals is not searched past one accessor block.
    let code = &src[prev.start..prev.end];
    if code
        .trim_end()
        .strip_suffix('=')
        .is_some_and(|rest| rest.trim_end().ends_with('}'))
    {
        const ACCESSORS: usize = 256;
        let close = code.rfind('}')?;
        let from = close.saturating_sub(ACCESSORS);
        if let Some(open) = code
            .get(from..close)
            .and_then(|window| window.rfind('{'))
            .map(|at| from + at)
        {
            let before = code[..open].trim_end();
            let name_line = before.rfind('\n').map_or(0, |at| at + 1);
            if let Some(name) = assigned_name(&code[name_line..]) {
                return Some(name);
            }
        }
    }
    if prev.start >= line_start || !on_line.trim().is_empty() {
        return assigned_name(on_line);
    }
    let above = src[prev.start..line_start].trim_end();
    assigned_name(above.rfind('\n').map_or(above, |at| &above[at + 1..]))
}

/// [`secret_pattern`] of `src[line_start..end]` for each of `ends`
/// (ascending) — what the line says from its start through each literal.
///
/// Every rule's finding only grows as the text is read further (see
/// [`ra_redact::SecretShapes`]), so where two ends read the same shapes,
/// every end between them does too. Halving between ends that differ finds
/// each point where the shapes change, and a rule can change them only a
/// few times — a line with no secret on it is read twice, however many
/// literals it holds.
fn line_secret_patterns(
    src: &str,
    line_start: usize,
    ends: &[usize],
    prefixes: &SecretPrefixes,
) -> Vec<Option<&'static str>> {
    let shapes_at = |k: usize| secret_shapes(&src[line_start..ends[k]], prefixes);
    let mut shapes: Vec<Option<SecretShapes>> = vec![None; ends.len()];
    let last = ends.len() - 1;
    shapes[0] = Some(shapes_at(0));
    shapes[last] = Some(shapes_at(last));
    // Spans whose two ends are read and whose inside is not yet.
    let mut open = vec![(0, last)];
    while let Some((lo, hi)) = open.pop() {
        if hi - lo < 2 {
            continue;
        }
        if shapes[lo] == shapes[hi] {
            let same = shapes[lo];
            shapes[lo + 1..hi].fill(same);
            continue;
        }
        let mid = lo + (hi - lo) / 2;
        shapes[mid] = Some(shapes_at(mid));
        open.push((lo, mid));
        open.push((mid, hi));
    }
    shapes
        .into_iter()
        .map(|s| s.and_then(|s| s.pattern()))
        .collect()
}

/// The name `code` — what precedes a literal — ends by assigning to, whole:
/// every segment of a member path (`Errors.Password`, `this.lblPassword`)
/// before `=`, `==` or `:`, or the name a `#define` gives the literal. A C
/// or C++ macro definition (`#define DB_PASSWORD "…"`) is the same
/// assignment written without a separator: the most common way that code
/// hard-codes a credential.
///
/// A C# property gives its value the same way past its accessors — an
/// initializer `Password { get; set; } = "…"` or an expression body
/// `Password => "…"` — and is the other common place a credential sits.
fn assigned_name(code: &str) -> Option<&str> {
    static RE: OnceLock<Regex> = OnceLock::new();
    static PROPERTY: OnceLock<Regex> = OnceLock::new();
    static DEFINE: OnceLock<Regex> = OnceLock::new();
    // Every shape ends in a separator or starts with `#`; anything else is
    // turned away before a pattern runs — most literals are not assigned.
    let end = code.trim_end();
    let ends_assigning = end.ends_with(['=', ':']);
    let ends_property = end.ends_with("=>") || (end.ends_with('=') && end.contains('}'));
    if !ends_assigning && !ends_property && !code.trim_start().starts_with('#') {
        return None;
    }
    if ends_property {
        let property = PROPERTY.get_or_init(|| {
            Regex::new(
                r"([\p{L}_][\p{L}\p{N}_]*)\s*(?:\{\s*(?:(?:public|protected|internal|private|get|set|init)\s*;?\s*)*\}\s*=|=>)\s*$",
            )
            .unwrap()
        });
        if let Some(caps) = property.captures(code) {
            return Some(caps.get(1)?.as_str());
        }
    }
    let re = RE.get_or_init(|| {
        Regex::new(r"((?:[\p{L}_][\p{L}\p{N}_]*\s*\.\s*)*[\p{L}_][\p{L}\p{N}_]*)\s*(?:==|=|:)\s*$")
            .unwrap()
    });
    let define = DEFINE
        .get_or_init(|| Regex::new(r"^\s*#\s*define\s+([A-Za-z_][A-Za-z0-9_]*)\s+$").unwrap());
    let caps = if ends_assigning {
        re.captures(code)?
    } else {
        define.captures(code)?
    };
    Some(caps.get(1)?.as_str())
}

/// The last segment of a member path: `Password` of `Errors.Password`.
fn last_segment(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name).trim()
}

/// Whether some word of an assigned name — of any of its segments — says
/// its value is a text shown to a person rather than a value: `lblPassword`,
/// `Errors.Password`, `PasswordPrompt`, `chybaHesla`. A word written as two
/// (`ToolTip`, `PlaceHolder`) counts as well.
fn names_a_message(name: &str) -> bool {
    const MESSAGE_WORDS: &[&str] = &[
        "lbl",
        "label",
        "msg",
        "message",
        "text",
        "caption",
        "title",
        "prompt",
        "hint",
        "tooltip",
        "error",
        "errors",
        "warning",
        "info",
        "placeholder",
        "watermark",
        "description",
        "popis",
        "zprava",
        "zpráva",
        "chyba",
        "hlaska",
        "hláška",
        "napoveda",
        "nápověda",
        "titulek",
    ];
    name.split('.').any(|segment| {
        let words = name_words(segment.trim());
        words.iter().any(|w| MESSAGE_WORDS.contains(&w.as_str()))
            || words
                .windows(2)
                .any(|pair| MESSAGE_WORDS.contains(&format!("{}{}", pair[0], pair[1]).as_str()))
    })
}

/// Which secret a name says its value is, by its last word or its last two
/// (`SmtpPassword`, `DB_PASSWORD`, `GoogleApiKey`) — `None` for any other.
fn secret_name(name: &str) -> Option<&'static str> {
    let words = name_words(name);
    let last = words.last()?.as_str();
    let pair = words
        .len()
        .checked_sub(2)
        .map(|at| format!("{} {}", words[at], words[at + 1]));
    match last {
        "password" | "passwd" | "pwd" | "heslo" | "hesla" => Some("password"),
        "secret" | "token" | "klíč" => Some("token"),
        _ if matches!(
            pair.as_deref(),
            Some("api key") | Some("secret key") | Some("access key")
        ) =>
        {
            Some("token")
        }
        _ => None,
    }
}

/// The words of an identifier, lowercased: split at `_` and where a lower
/// case letter or a digit meets an upper case one (`SmtpPassword` →
/// `smtp password`, `api_key` → `api key`).
fn name_words(name: &str) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut prev_lower = false;
    for c in name.chars() {
        if c == '_' {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
            prev_lower = false;
            continue;
        }
        if c.is_uppercase() && prev_lower && !current.is_empty() {
            words.push(std::mem::take(&mut current));
        }
        prev_lower = c.is_lowercase() || c.is_numeric();
        current.extend(c.to_lowercase());
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

// ---- The secret filter of the second layer (plan 2a, KTD16) ----

/// Which secret a value is by the name it is given, compared with or
/// written to (`SmtpPassword`, `token`, `Errors.Password`) — the rule
/// [`literal_secrets`] reads the assigned name with, for a name the second
/// layer's model gives instead of the code on the line. A value of several
/// words is a message more often than a secret, so a name with a word of a
/// message (`lblPassword`, `errorToken`) speaks for none of those.
pub(crate) fn named_secret(name: &str, value: &str) -> Option<&'static str> {
    let pattern = secret_name(last_segment(name))?;
    let words = value
        .trim_matches(|c: char| c == '"' || c == '@' || c == '$' || c == '\'')
        .trim()
        .contains(char::is_whitespace);
    (!words || !names_a_message(name)).then_some(pattern)
}

/// One string literal of a source file, as the second layer pairs it.
struct SourceLiteral {
    /// Where its content starts: 1-based line, 1-based column counted in
    /// characters — the units of the model's positions.
    at: (u32, u32),
    /// The pattern it is withheld under, when it is.
    pattern: Option<&'static str>,
    /// How it is written: its content with the quotes around it, when they
    /// are quotes; a text the model quotes from the source holds it so.
    spelling: String,
}

/// What a source file withholds, read for the second layer's model of its
/// bodies (plan 2a, KTD16) — the decisions the first layer makes about its
/// literals, extended by what only the second layer quotes.
///
/// Per literal, in source order: what [`literal_secrets`] decides, which
/// stays the first layer's own and unchanged; the literals of a key written
/// one per line through the one holding `-----END`, as [`emit_file`]
/// withholds them; a literal holding a key-shaped word ([`is_key_word`]),
/// which the first layer only keeps out of its anchors; and a literal
/// compared with a name that says what it is, on either side of `==` or
/// `!=` (`token == "…"`, `"…" != token`) — the partner the first layer reads
/// in one direction and for `==` alone.
///
/// Beside them, the spellings a text of the model may quote: each withheld
/// literal as written, and each comment that names a secret, which a
/// condition's text quotes with the code around it.
pub(crate) struct SourceSecrets {
    literals: Vec<SourceLiteral>,
    spellings: Vec<(String, &'static str)>,
}

impl SourceSecrets {
    /// The file at `path` read for the second layer, or `None` when the host
    /// has no lexer for its language.
    pub(crate) fn read(path: &str, src: &str, prefixes: &SecretPrefixes) -> Option<SourceSecrets> {
        let lang = Lang::from_path(path)?;
        let segs = segments(lang, src);
        let lines = LineIndex::new(src);
        let named = literal_secrets(src, &segs, &lines, prefixes);
        // Columns are counted on from the literal before on the same line, so
        // a minified line of many literals is counted once, not per literal.
        let mut last: (u32, usize, u32) = (0, 0, 0);
        let mut at = |offset: usize| {
            let line = lines.line(offset);
            let (from, column) = if last.0 == line && last.1 <= offset {
                (last.1, last.2)
            } else {
                (lines.start(line), 1)
            };
            let column = column + src[from..offset].chars().count() as u32;
            last = (line, offset, column);
            (line, column)
        };
        let mut literals = Vec::new();
        let mut spellings = Vec::new();
        let mut in_key = false;
        for (i, seg) in segs.iter().enumerate() {
            let text = &src[seg.start..seg.end];
            match seg.kind {
                SegKind::Code => {}
                SegKind::Comment => {
                    let (_, redacted) = redact_secret_block(text, prefixes);
                    let pattern = if redacted {
                        Some(secret_pattern(text, prefixes).unwrap_or("secret value"))
                    } else {
                        without_long_tokens(text).1.then_some("long token")
                    };
                    if let Some(pattern) = pattern {
                        spellings.push((text.to_string(), pattern));
                    }
                }
                SegKind::Str { .. } => {
                    let pattern = if in_key {
                        in_key = !text.contains("-----END");
                        Some("private key")
                    } else if let Some(pattern) = named[i] {
                        in_key = pattern == "private key"
                            && text.contains("-----BEGIN")
                            && !text.contains("-----END");
                        Some(pattern)
                    } else {
                        compared_name(src, &segs, i)
                            .and_then(|name| named_secret(name, text))
                            .or_else(|| {
                                without_long_tokens(&literal_text(src, seg))
                                    .1
                                    .then_some("long token")
                            })
                    };
                    let b = src.as_bytes();
                    let quoted = seg.start > 0
                        && seg.end < b.len()
                        && matches!(b[seg.start - 1], b'"' | b'\'' | b'`')
                        && b[seg.end] == b[seg.start - 1];
                    let spelling = if quoted {
                        &src[seg.start - 1..seg.end + 1]
                    } else {
                        text
                    };
                    if let Some(pattern) = pattern.filter(|_| !text.is_empty()) {
                        spellings.push((spelling.to_string(), pattern));
                    }
                    literals.push(SourceLiteral {
                        at: at(seg.start),
                        pattern,
                        spelling: spelling.to_string(),
                    });
                }
            }
        }
        Some(SourceSecrets {
            literals,
            spellings,
        })
    }

    /// The literal of the file whose content starts within `start..=end`
    /// (line, column in characters) — the place the model gives a literal,
    /// which starts at its opening token (`@`, `$`, the quotes) — as the
    /// pattern it is withheld under and its spelling; `None` when no literal
    /// of the file starts there.
    pub(crate) fn literal(
        &self,
        start: (u32, u32),
        end: (u32, u32),
    ) -> Option<(Option<&'static str>, &str)> {
        let first = self.literals.partition_point(|l| l.at < start);
        self.literals
            .get(first)
            .filter(|l| l.at <= end)
            .map(|l| (l.pattern, l.spelling.as_str()))
    }

    /// Every spelling the file withholds, with its pattern, in source order.
    pub(crate) fn spellings(&self) -> &[(String, &'static str)] {
        &self.spellings
    }
}

/// A block comment written right after a string literal, or right after the
/// call the literal closes the arguments of —
/// `GetResourceString("EditorView_checkErrors") /*"Kontrola chyb"*/` — as the
/// lexer split it (plan 2a, U7: R18). One lexer segment as it is: never
/// joined with the comments around it and never under the anchors' merging or
/// their cap ([`crate::graph::ANCHOR_CAP`]), which a type with many comments
/// reaches long before its menu is read.
pub(crate) struct CommentAfterLiteral {
    /// Where the literal's content starts: 1-based line, 1-based column in
    /// characters — the point [`SourceSecrets::literal`] pairs a model's
    /// literal by.
    pub(crate) literal: (u32, u32),
    /// Where the comment's `/*` is, in the same units.
    pub(crate) start: (u32, u32),
    /// Where the comment's last character is.
    pub(crate) end: (u32, u32),
    /// The comment without `/*` and `*/`, its whitespace collapsed, and
    /// without the quotes around it when it is quoted whole. Unredacted: the
    /// caller filters it.
    pub(crate) text: String,
}

/// Every [`CommentAfterLiteral`] of the file at `path`, in source order, or
/// `None` when the host has no lexer for its language.
///
/// Only what stands between the literal and the comment decides: the
/// literal's closing quotes, closing parentheses and spaces, on one line. A
/// comma, an operator or a line break between them makes the comment
/// something else, and so does a doc comment, a line comment or an empty
/// one. No other comment of the file is read.
pub(crate) fn comments_after_literals(path: &str, src: &str) -> Option<Vec<CommentAfterLiteral>> {
    let lang = Lang::from_path(path)?;
    let segs = segments(lang, src);
    let lines = LineIndex::new(src);
    let at = |offset: usize| {
        let line = lines.line(offset);
        let column = 1 + src[lines.start(line)..offset].chars().count() as u32;
        (line, column)
    };
    let mut out = Vec::new();
    for (i, seg) in segs.iter().enumerate() {
        if !matches!(seg.kind, SegKind::Str { .. }) {
            continue;
        }
        let Some(comment) = segs[i + 1..]
            .iter()
            .take(2)
            .take_while(|s| !matches!(s.kind, SegKind::Str { .. }))
            .find(|s| s.kind == SegKind::Comment)
        else {
            continue;
        };
        let text = &src[comment.start..comment.end];
        let gap = &src[seg.end..comment.start];
        if !text.starts_with("/*")
            || is_doc_comment(lang, text)
            || !gap
                .chars()
                .all(|c| matches!(c, '"' | '\'' | ')' | ' ' | '\t'))
        {
            continue;
        }
        let inner = text.strip_prefix("/*").unwrap_or(text);
        let inner = inner.strip_suffix("*/").unwrap_or(inner);
        let inner = inner.split_whitespace().collect::<Vec<_>>().join(" ");
        let inner = match inner.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
            Some(quoted) => quoted.trim().to_string(),
            None => inner,
        };
        if inner.is_empty() {
            continue;
        }
        out.push(CommentAfterLiteral {
            literal: at(seg.start),
            start: at(comment.start),
            end: at(comment.end - 1),
            text: inner,
        });
    }
    Some(out)
}

/// The name a literal is compared with by `==` or `!=`: the path written
/// right before the operator, or right after it when the literal comes
/// first — not a call's name, which names no value.
fn compared_name<'s>(src: &'s str, segs: &[Segment], i: usize) -> Option<&'s str> {
    static AFTER: OnceLock<Regex> = OnceLock::new();
    static BEFORE: OnceLock<Regex> = OnceLock::new();
    let code = |at: usize| {
        segs.get(at)
            .filter(|seg| seg.kind == SegKind::Code)
            .map(|seg| &src[seg.start..seg.end])
    };
    let after = AFTER.get_or_init(|| {
        Regex::new(
            r"^\s*[!=]=\s*((?:[\p{L}_][\p{L}\p{N}_]*\s*\.\s*)*[\p{L}_][\p{L}\p{N}_]*)\s*(\()?",
        )
        .unwrap()
    });
    if let Some(caps) = code(i + 1).and_then(|c| after.captures(c)) {
        if caps.get(2).is_none() {
            return Some(caps.get(1)?.as_str());
        }
    }
    let before = BEFORE.get_or_init(|| {
        Regex::new(r"((?:[\p{L}_][\p{L}\p{N}_]*\s*\.\s*)*[\p{L}_][\p{L}\p{N}_]*)\s*[!=]=\s*$")
            .unwrap()
    });
    // The literal's own line of the code before it: a minified line is read
    // once per literal on it, never the whole file before it.
    code(i.wrapping_sub(1))
        .and_then(|c| c.rsplit('\n').next())
        .and_then(|c| before.captures(c))
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str())
}

/// Whether a word is one long token shaped like a key or a hash (plan B,
/// KTD11 rule 3): at least twenty characters with no space, every one of them
/// from the hex or base64 alphabet, a digit among them, and either nothing but
/// hex digits or both cases of letters. That last pair is what keeps a long
/// identifier out of it — `SetScheduledTasksInvalid` has no digit, and
/// `ERR_CODE_2024_INVALID_STATE` has no lower case letter.
pub(crate) fn is_long_token(word: &str) -> bool {
    let b = word.as_bytes();
    b.len() >= 20
        && b.iter()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'+' | b'/' | b'=' | b'_' | b'-'))
        && b.iter().any(u8::is_ascii_digit)
        && (b.iter().all(u8::is_ascii_hexdigit)
            || (b.iter().any(u8::is_ascii_uppercase) && b.iter().any(u8::is_ascii_lowercase)))
}

/// A literal's text with every key-shaped word cut out (KTD11 rule 3), and
/// whether one was. A word is tested with its escape sequences removed — a
/// key line written as `"MIIE…\n"` carries `\` and `n` in its source text,
/// and no key alphabet holds a backslash — and trimmed of the punctuation
/// around it, as a comment's word is.
pub(crate) fn without_long_tokens(text: &str) -> (std::borrow::Cow<'_, str>, bool) {
    use std::borrow::Cow;
    let trimmed = text.trim();
    // One word — most literals — is tested whole, with no split and no copy.
    if !trimmed.contains(char::is_whitespace) {
        return if is_key_word(trimmed) {
            (Cow::Borrowed(""), true)
        } else {
            (Cow::Borrowed(text), false)
        };
    }
    let mut long = false;
    let kept: Vec<&str> = text
        .split_whitespace()
        .filter(|w| {
            let key = is_key_word(w);
            long |= key;
            !key
        })
        .collect();
    if long {
        (Cow::Owned(kept.join(" ")), true)
    } else {
        (Cow::Borrowed(text), false)
    }
}

/// Whether a word, with its escape sequences removed and, failing that, the
/// punctuation around it trimmed, is key-shaped ([`is_long_token`]).
fn is_key_word(word: &str) -> bool {
    let word = unescaped(word);
    is_long_token(&word) || is_long_token(word.trim_matches(|c: char| !c.is_ascii_alphanumeric()))
}

/// A doc comment's text with every key-shaped word cut out (KTD11 rule 3),
/// line by line — a line that loses one keeps its indentation and the words
/// around it — and the 0-based index of each line that lost one. `None` when
/// no word was cut.
///
/// A doc comment is no anchor, so [`comment_anchors`] never reads it; it is
/// cut where it enters the graph instead, so that every reader of
/// `DocComment` — the card's quote and purpose, the map row, the manifest
/// header, the model's input — inherits the cut.
pub(crate) fn doc_without_long_tokens(text: &str) -> Option<(String, Vec<usize>)> {
    let mut cut: Vec<usize> = Vec::new();
    let mut lines: Vec<std::borrow::Cow<'_, str>> = Vec::new();
    for (n, line) in text.split('\n').enumerate() {
        let (kept, long) = if line.contains(['<', '>']) {
            without_long_tokens_in_markup(line)
        } else {
            without_long_tokens(line)
        };
        if long {
            cut.push(n);
            let indent = &line[..line.len() - line.trim_start().len()];
            lines.push(format!("{indent}{}", kept.trim()).into());
        } else {
            lines.push(std::borrow::Cow::Borrowed(line));
        }
    }
    (!cut.is_empty()).then(|| (lines.join("\n"), cut))
}

/// [`without_long_tokens`] for a doc comment's line that holds markup. A
/// key written right between two tags (`<c>KEY</c>`, `<summary>KEY</summary>`)
/// is one whitespace word with the tags glued to it, and no key alphabet
/// holds `<` or `>` — so each word is also read run by run between them, and
/// a key-shaped run is cut out with the markup around it kept: `<c></c>`.
fn without_long_tokens_in_markup(line: &str) -> (std::borrow::Cow<'_, str>, bool) {
    let (whole, long) = without_long_tokens(line);
    let mut cut = long;
    let words: Vec<String> = whole
        .split_whitespace()
        .map(|word| {
            if !word.contains(['<', '>']) {
                return word.to_string();
            }
            let mut kept = String::with_capacity(word.len());
            for run in word.split_inclusive(['<', '>']) {
                let text = run.trim_end_matches(['<', '>']);
                if is_key_word(text) {
                    cut = true;
                    kept.push_str(&run[text.len()..]);
                } else {
                    kept.push_str(run);
                }
            }
            kept
        })
        .collect();
    if cut {
        (std::borrow::Cow::Owned(words.join(" ")), true)
    } else {
        (std::borrow::Cow::Borrowed(line), false)
    }
}

/// [`doc_without_long_tokens`] applied to a `DocComment` fact in place, and
/// the skips it leaves: one `AnchorSkipped("long token")` per line that lost
/// a word, at that line — the reason and the place, never the text, as
/// [`comment_anchors`] records a comment line's (R10). For the caller to put
/// on the same node; empty, and nothing allocated, for every other fact.
pub(crate) fn cut_doc_long_tokens(fact: &mut Fact) -> Vec<Fact> {
    if fact.kind != FactKind::DocComment {
        return Vec::new();
    }
    let FactValue::Text(text) = &fact.value else {
        return Vec::new();
    };
    let Some((kept, cut)) = doc_without_long_tokens(text) else {
        return Vec::new();
    };
    fact.value = FactValue::Text(kept);
    let (first, last) = (fact.provenance.span.start(), fact.provenance.span.end());
    cut.into_iter()
        .map(|n| {
            // A doc comment's text holds one line per source line; where the
            // two disagree the skip stays inside the comment's own range.
            let line = first.map(|f| {
                let at = f.saturating_add(u32::try_from(n).unwrap_or(u32::MAX));
                last.map_or(at, |l| at.min(l))
            });
            let mut prov = fact.provenance.clone();
            prov.span = crate::graph::Span::at(line);
            Fact::new(
                FactKind::AnchorSkipped,
                FactValue::Text("long token".to_string()),
                prov,
            )
        })
        .collect()
}

/// `word` without the escape sequences a key's line is written with in a
/// literal: `\n`, `\r`, `\t`, `\\` and `\"`.
fn unescaped(word: &str) -> std::borrow::Cow<'_, str> {
    if !word.contains('\\') {
        return std::borrow::Cow::Borrowed(word);
    }
    let mut out = String::with_capacity(word.len());
    let mut chars = word.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' && matches!(chars.peek(), Some('n' | 'r' | 't' | '\\' | '"')) {
            chars.next();
            continue;
        }
        out.push(c);
    }
    std::borrow::Cow::Owned(out)
}

// ---- Anchors (plan B, U3: KTD1) ----

/// A literal's text as anchors and SQL read it: its interpolation holes
/// overwritten by spaces, byte for byte, so an offset into it is still an
/// offset into the literal and no code of a hole is ever quoted.
fn literal_text<'s>(src: &'s str, seg: &Segment) -> std::borrow::Cow<'s, str> {
    let text = &src[seg.start..seg.end];
    if seg.holes.is_empty() {
        return std::borrow::Cow::Borrowed(text);
    }
    let mut bytes = text.as_bytes().to_vec();
    for &(from, to) in &seg.holes {
        let (from, to) = (from.max(seg.start) - seg.start, to.min(seg.end) - seg.start);
        if from < to {
            // A hole is replaced whole, so no character is ever split.
            bytes[from..to].fill(b' ');
        }
    }
    std::borrow::Cow::Owned(String::from_utf8(bytes).unwrap_or_default())
}

/// Whether the qualified name a SQL pattern found at `offset` in a literal's
/// text is glued to an interpolation hole — `dba.t{suffix}` names a different
/// table per call, and `dba.t` is none of them.
fn touches_hole(seg: &Segment, text: &str, offset: usize) -> bool {
    if seg.holes.is_empty() {
        return false;
    }
    let len = text[offset..]
        .bytes()
        .position(|c| !(c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'[' | b']')))
        .unwrap_or(text.len() - offset);
    let (start, end) = (seg.start + offset, seg.start + offset + len);
    seg.holes.iter().any(|&(h0, h1)| h1 == start || h0 == end)
}

/// A text worth an anchor: whitespace collapsed, and at least two letters —
/// a separator, a placeholder or a number leads nowhere.
fn anchor_text(raw: &str) -> Option<String> {
    let text = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    (text.chars().filter(|c| c.is_alphabetic()).take(2).count() == 2).then_some(text)
}

/// Whether a comment is a doc comment of its language — `DocComment` carries
/// those. Go and Python write theirs as ordinary comments or as a string, so
/// none of their comments is one.
fn is_doc_comment(lang: Lang, text: &str) -> bool {
    match lang {
        Lang::Python | Lang::Go => false,
        _ => {
            (text.starts_with("///") && !text.starts_with("////"))
                || text.starts_with("//!")
                || (text.starts_with("/**") && !text.starts_with("/**/"))
                || text.starts_with("/*!")
        }
    }
}

/// Whether a Python literal is a docstring — a triple-quoted string standing
/// as a statement where a body begins: at the top of the file, or right after
/// the `:` that opens a `def` or a `class`. A dictionary value written with
/// triple quotes after a `:` reads the same way and is skipped too; that is
/// the rarer shape by far.
fn is_python_docstring(src: &str, segs: &[Segment], i: usize) -> bool {
    let seg = &segs[i];
    let b = src.as_bytes();
    let triple = seg.start >= 3 && matches!(&b[seg.start - 3..seg.start], b"\"\"\"" | b"'''");
    if !triple {
        return false;
    }
    match segs.get(i.wrapping_sub(1)) {
        None => true,
        Some(prev) if prev.kind == SegKind::Code => {
            let code = src[prev.start..prev.end].trim_end();
            // The prefix of the literal itself (`r`, `u`) is not in the code.
            code.is_empty() || code.ends_with(':')
        }
        Some(_) => true,
    }
}

/// One text found in the file, before it is given an owner.
struct AnchorHit {
    offset: usize,
    source: AnchorSource,
    text: String,
}

/// The lines of one comment segment without its framing (`//`, `#`, `/*`,
/// `*/`, a leading `*`), each with the byte offset of its source line.
fn comment_lines<'s>(src: &'s str, seg: &Segment) -> Vec<(usize, &'s str)> {
    let text = &src[seg.start..seg.end];
    let block = text.starts_with("/*");
    let mut out = Vec::new();
    let mut offset = seg.start;
    for raw in text.split('\n') {
        let line_offset = offset;
        offset += raw.len() + 1;
        let mut line = raw.trim_end_matches('\r').trim();
        if block {
            line = line.strip_prefix("/*").unwrap_or(line);
            line = line.strip_suffix("*/").unwrap_or(line);
            line = line.trim_start().strip_prefix('*').unwrap_or(line);
        } else {
            line = line
                .strip_prefix("//")
                .or_else(|| line.strip_prefix('#'))
                .unwrap_or(line);
        }
        out.push((line_offset, line.trim()));
    }
    out
}

/// Every ordinary comment of the file read for anchors, after the secret
/// filter (plan B, KTD11 rule 1): adjacent line comments are joined into one
/// logical block first, so a value on the line below its label (`// password:`
/// then `// hunter2`) and a key block written as `//` lines are read whole.
/// A line the filter replaced leaves its place and the reason; a long token
/// inside a kept line is cut out of it (rule 3).
fn comment_anchors(
    lang: Lang,
    src: &str,
    segs: &[Segment],
    prefixes: &SecretPrefixes,
    hits: &mut Vec<AnchorHit>,
    skips: &mut Vec<(usize, String)>,
) {
    let comments: Vec<&Segment> = segs
        .iter()
        .filter(|s| s.kind == SegKind::Comment && !is_doc_comment(lang, &src[s.start..s.end]))
        .collect();
    let is_line = |s: &Segment| !src[s.start..s.end].starts_with("/*");
    let mut i = 0;
    while i < comments.len() {
        // One logical block: a block comment alone, or a run of line comments
        // with nothing but one line break and indentation between them.
        let mut j = i + 1;
        if is_line(comments[i]) {
            while j < comments.len() && is_line(comments[j]) && {
                let gap = &src[comments[j - 1].end..comments[j].start];
                gap.trim().is_empty() && gap.matches('\n').count() == 1
            } {
                j += 1;
            }
        }
        let lines: Vec<(usize, &str)> = comments[i..j]
            .iter()
            .flat_map(|s| comment_lines(src, s))
            .collect();
        let block = lines.iter().map(|(_, l)| *l).collect::<Vec<_>>().join("\n");
        let (filtered, _) = redact_secret_block(&block, prefixes);
        for ((offset, _), line) in lines.iter().zip(filtered.split('\n')) {
            if let Some(rest) = line.strip_prefix(REDACTION_MARKER) {
                let reason = rest
                    .split(" — ")
                    .next()
                    .unwrap_or(rest)
                    .trim_end_matches(']');
                skips.push((*offset, reason.to_string()));
                continue;
            }
            let mut long = false;
            let kept: Vec<&str> = line
                .split_whitespace()
                .filter(|w| {
                    let token = is_long_token(w.trim_matches(|c: char| !c.is_ascii_alphanumeric()));
                    long |= token;
                    !token
                })
                .collect();
            if long {
                skips.push((*offset, "long token".to_string()));
            }
            if let Some(text) = anchor_text(&kept.join(" ")) {
                hits.push(AnchorHit {
                    offset: *offset,
                    source: AnchorSource::Comment,
                    text,
                });
            }
        }
        i = j;
    }
}

/// The anchors of one owner, merged and capped (KTD1): one per distinct text,
/// with its count and first line; multi-word texts before single words, then
/// the lower line, then the text; past [`ANCHOR_CAP`] only counted.
fn select_anchors(hits: Vec<(u32, AnchorSource, String)>) -> (Vec<(u32, AnchorText)>, usize) {
    // Source order in, so the first entry of a text is its first line.
    let mut merged: Vec<(u32, AnchorSource, String, u32)> = Vec::new();
    let mut at: HashMap<String, usize> = HashMap::new();
    for (line, source, text) in hits {
        match at.get(&text) {
            Some(&k) => merged[k].3 += 1,
            None => {
                at.insert(text.clone(), merged.len());
                merged.push((line, source, text, 1));
            }
        }
    }
    merged.sort_by(|a, b| (!a.2.contains(' '), a.0, &a.2).cmp(&(!b.2.contains(' '), b.0, &b.2)));
    let dropped = merged.len().saturating_sub(ANCHOR_CAP);
    merged.truncate(ANCHOR_CAP);
    let kept = merged
        .into_iter()
        .map(|(line, source, text, occurrences)| {
            let length = text.chars().count();
            let text = match text.char_indices().nth(ANCHOR_TEXT_CAP) {
                Some((cut, _)) => text[..cut].to_string(),
                None => text,
            };
            (
                line,
                AnchorText {
                    source,
                    occurrences,
                    length: length as u32,
                    text,
                },
            )
        })
        .collect();
    (kept, dropped)
}

// ---- Decisions by an enum's value (plan B, U5) ----

/// The `SymbolKind` values of a C# type — every kind a nested type or a
/// qualifier `X` of `X.Y` can be.
const CSHARP_TYPE_KINDS: &[&str] = &["class", "struct", "record", "interface", "enum"];

/// One C# type the graph declares.
struct TypeInfo {
    name: String,
    is_enum: bool,
    /// The namespace its key names, `""` for the global namespace — a C#
    /// file with no namespace declaration is keyed by its path stem, which
    /// names no namespace at all.
    namespace: String,
}

/// What a decision on an enum's value is checked against (plan B, U5: KTD3):
/// the C# types the graph declares, where each is declared, which are enums
/// and which values they have. Read out of the graph once, before any file is
/// scanned, like [`OwnerIndex`] — the tree-sitter extractor has put every
/// type, member, `HasMember` and `Extends` edge there by then.
///
/// The cache needs nothing more for this: a record is served only under a run
/// fingerprint equal to the one it was written under, and that fingerprint
/// holds every tracked blob, so a place verified against an enum in another
/// file is re-read whenever that file changes.
#[derive(Default)]
struct EnumIndex {
    /// Simple name → the enums of that name. The filter every candidate
    /// `X.Y` passes first: a qualifier no enum is named costs one lookup.
    enums_named: HashMap<String, Vec<NodeKey>>,
    /// Enum → the names of its `variant` members.
    values: HashMap<NodeKey, HashSet<String>>,
    types: HashMap<NodeKey, TypeInfo>,
    /// Member → the type that declares it (`HasMember`), for every member of
    /// a C# type — how a place's method finds the types around it.
    declaring: HashMap<NodeKey, NodeKey>,
    /// Type → the types nested in it.
    nested: HashMap<NodeKey, Vec<NodeKey>>,
    /// Type → the repository types it extends (`Extends`).
    bases: HashMap<NodeKey, Vec<NodeKey>>,
    /// Namespace → simple name → the top-level types declared there.
    top: HashMap<String, HashMap<String, Vec<NodeKey>>>,
    /// `Namespace.Outer.Inner` → the types of that full name, for the target
    /// of an alias or a `using static`.
    by_full_name: HashMap<String, Vec<NodeKey>>,
}

impl EnumIndex {
    fn build(graph: &Graph) -> EnumIndex {
        let mut index = EnumIndex::default();
        let mut variants: HashMap<NodeKey, String> = HashMap::new();
        for node in graph.nodes_unordered() {
            if node.kind != NodeKind::Symbol || !node.key.as_str().starts_with("sym:csharp:") {
                continue;
            }
            let mut name: Option<(&str, &str)> = None;
            let mut kind: Option<&str> = None;
            for fact in &node.facts {
                match (fact.kind, &fact.value) {
                    // The first part of a partial type is as good as any: the
                    // parts share the key, and so the name and namespace.
                    (FactKind::SymbolName, FactValue::Text(v)) if name.is_none() => {
                        name = Some((v.as_str(), fact.provenance.file.as_str()));
                    }
                    (FactKind::SymbolKind, FactValue::Text(v)) => kind = Some(v.as_str()),
                    _ => {}
                }
            }
            let (Some((name, file)), Some(kind)) = (name, kind) else {
                continue;
            };
            if kind == "variant" {
                variants.insert(node.key.clone(), name.to_string());
            } else if CSHARP_TYPE_KINDS.contains(&kind) {
                index.types.insert(
                    node.key.clone(),
                    TypeInfo {
                        name: name.to_string(),
                        is_enum: kind == "enum",
                        namespace: namespace_of(node.key.as_str(), file).unwrap_or_default(),
                    },
                );
            }
        }
        for edge in graph.edges_unordered() {
            if !index.types.contains_key(edge.from) {
                continue;
            }
            match edge.edge.kind {
                EdgeKind::HasMember => {
                    index.declaring.insert(edge.to.clone(), edge.from.clone());
                }
                EdgeKind::Extends => index
                    .bases
                    .entry(edge.from.clone())
                    .or_default()
                    .push(edge.to.clone()),
                _ => {}
            }
        }
        for (member, owner) in &index.declaring {
            if index.types.contains_key(member) {
                index
                    .nested
                    .entry(owner.clone())
                    .or_default()
                    .push(member.clone());
            } else if let Some(value) = variants.get(member) {
                if index.types.get(owner).is_some_and(|t| t.is_enum) {
                    index
                        .values
                        .entry(owner.clone())
                        .or_default()
                        .insert(value.clone());
                }
            }
        }
        for (key, info) in &index.types {
            if info.is_enum {
                index
                    .enums_named
                    .entry(info.name.clone())
                    .or_default()
                    .push(key.clone());
            }
            if !index.declaring.contains_key(key) {
                index
                    .top
                    .entry(info.namespace.clone())
                    .or_default()
                    .entry(info.name.clone())
                    .or_default()
                    .push(key.clone());
            }
            let mut path = vec![info.name.as_str()];
            let mut at = key;
            // A cycle only broken code has; the depth bound ends it.
            while let Some(outer) = index.declaring.get(at).filter(|_| path.len() < 64) {
                let Some(outer_info) = index.types.get(outer) else {
                    break;
                };
                path.push(outer_info.name.as_str());
                at = outer;
            }
            path.reverse();
            let path = path.join(".");
            let full = if info.namespace.is_empty() {
                path
            } else {
                format!("{}.{path}", info.namespace)
            };
            index
                .by_full_name
                .entry(full)
                .or_default()
                .push(key.clone());
        }
        index
    }

    /// Whether a qualifier names any enum the repository declares.
    fn names_an_enum(&self, name: &str) -> bool {
        self.enums_named.contains_key(name)
    }

    /// The types around a place's owner, innermost first: the owner itself
    /// when it is a type, else the type declaring it, then each type that one
    /// is nested in. Empty when the owner is no C# declaration of a type or a
    /// member — a file, or code no type encloses.
    fn enclosing_types<'i>(&'i self, owner: &'i NodeKey) -> Vec<&'i NodeKey> {
        let mut chain = Vec::new();
        let mut at = if self.types.contains_key(owner) {
            Some(owner)
        } else {
            self.declaring.get(owner)
        };
        while let Some(t) = at.filter(|t| self.types.contains_key(*t) && chain.len() < 64) {
            chain.push(t);
            at = self.declaring.get(t);
        }
        chain
    }

    /// `types` and every repository type they extend, however far up.
    fn with_ancestors<'i>(&'i self, types: &[&'i NodeKey]) -> Vec<&'i NodeKey> {
        let mut seen: HashSet<&NodeKey> = HashSet::new();
        let mut out = Vec::new();
        let mut stack: Vec<&NodeKey> = types.to_vec();
        while let Some(t) = stack.pop() {
            if seen.insert(t) {
                out.push(t);
                if let Some(bases) = self.bases.get(t) {
                    stack.extend(bases);
                }
            }
        }
        out
    }

    /// The types nested in `outer` under the simple name `name`.
    fn nested_named<'i>(
        &'i self,
        outer: &NodeKey,
        name: &'i str,
    ) -> impl Iterator<Item = &'i NodeKey> + 'i {
        self.nested
            .get(outer)
            .into_iter()
            .flatten()
            .filter(move |k| self.types.get(*k).is_some_and(|t| t.name == name))
    }

    /// The top-level types of `namespace` under the simple name `name`.
    fn top_named(&self, namespace: &str, name: &str) -> &[NodeKey] {
        self.top
            .get(namespace)
            .and_then(|by_name| by_name.get(name))
            .map_or(&[], Vec::as_slice)
    }

    /// The repository types a written type name means, by full name —
    /// `global::` and generic arguments left out.
    fn by_written_name(&self, written: &str) -> &[NodeKey] {
        let written = written.trim();
        let written = written.strip_prefix("global::").unwrap_or(written);
        let written = written.split('<').next().unwrap_or(written).trim();
        self.by_full_name.get(written).map_or(&[], Vec::as_slice)
    }
}

/// The `using` directives one C# file writes, read from its code (plan B,
/// U5: KTD3). Read file-wide: a `using` inside a block-bodied namespace is
/// taken to reach the whole file, which can only show a place MORE types of
/// its qualifier's name — a verification lost, never one gained.
#[derive(Default)]
struct FileUsings {
    namespaces: Vec<String>,
    statics: Vec<String>,
    /// Alias → what it is written to stand for.
    aliases: HashMap<String, String>,
}

impl FileUsings {
    fn read(src: &str, segs: &[Segment]) -> FileUsings {
        static RE: OnceLock<Regex> = OnceLock::new();
        let re = RE.get_or_init(|| {
            Regex::new(
                r"^\s*(?:global\s+)?using\s+(static\s+)?(?:([A-Za-z_]\w*)\s*=\s*)?((?:global::)?[A-Za-z_][\w.]*(?:<[^;]*>)?)\s*;",
            )
            .unwrap()
        });
        let mut out = FileUsings::default();
        let mut offset = 0usize;
        for raw in src.split_inclusive('\n') {
            let line_offset = offset;
            offset += raw.len();
            // Read for every C# file, so a line that cannot open a directive
            // is passed over before the pattern runs.
            let head = raw.trim_start();
            if !head.starts_with("using") && !head.starts_with("global") {
                continue;
            }
            let Some(caps) = re.captures(raw) else {
                continue;
            };
            // A directive in a comment or a string is no directive.
            let at = line_offset + caps.get(3).map_or(0, |m| m.start());
            let seg = segs.partition_point(|s| s.end <= at);
            if segs
                .get(seg)
                .is_none_or(|s| s.kind != SegKind::Code || s.start > at)
            {
                continue;
            }
            let target = caps.get(3).map_or("", |m| m.as_str()).to_string();
            match (caps.get(1), caps.get(2)) {
                (_, Some(alias)) => {
                    out.aliases.insert(alias.as_str().to_string(), target);
                }
                (Some(_), None) => out.statics.push(target),
                (None, None) => out
                    .namespaces
                    .push(target.trim_start_matches("global::").to_string()),
            }
        }
        out
    }
}

/// One `X.Y` written in a shape that decides by its value.
struct DecisionSite<'s> {
    /// Byte offset of `X` — its line and owner.
    offset: usize,
    shape: DecisionShape,
    qualifier: &'s str,
    value: &'s str,
}

/// Every `X.Y` in the code of a C# file written as a `case` label, a switch
/// expression arm or a comparison, whose `X` names an enum the repository
/// declares (plan B, U5: KTD3).
///
/// Read without a syntax tree, so the shapes are kept narrow:
/// - a label is `case X.Y` followed by `:` or `when` — `goto case X.Y;` is a
///   jump, not a decision;
/// - an arm is `X.Y` right after `{` or `,`, followed by `=>` or `when` —
///   which keeps `int IFoo.Count => 1;`, an explicit interface member, out;
/// - a comparison has `==` or `!=` on either side of `X.Y`, or `is` / `is
///   not` before it.
///
/// `X.Y` must be the whole path: `a.X.Y`, `X.Y.Z` and a call `X.Y(…)` are
/// something else. A shape broken by a comment between its parts is not
/// read. A longer qualifier (`Ns.X.Y`, `Outer.X.Y`) is not read either —
/// which of its segments is the enum is a question this reading cannot
/// settle, and KTD3 counts the qualified `X.Y` labels as nearly all of them.
///
/// `aliased` holds the file's aliases of another name that stand for one
/// enum (`using BLT = …BackupLoadType;`): `BLT.Full` is read as surely as
/// `BackupLoadType.Full` is.
fn decision_sites<'s>(
    src: &'s str,
    segs: &[Segment],
    index: &EnumIndex,
    aliased: &HashMap<&str, &NodeKey>,
) -> Vec<DecisionSite<'s>> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    for (n, seg) in segs.iter().enumerate() {
        if seg.kind != SegKind::Code {
            continue;
        }
        let (start, end) = (seg.start, seg.end);
        let mut i = start;
        while i < end {
            if !is_ident_start(b[i]) {
                i += 1;
                continue;
            }
            // The tail of an identifier that starts with a digit or a
            // non-ASCII letter is no identifier of its own.
            if i > 0 && (is_ident_continue(b[i - 1]) || b[i - 1] >= 0x80) {
                i = ident_end(b, i, end);
                continue;
            }
            let x_end = ident_end(b, i, end);
            // The whole dotted path from here, so `X.Y.Z` is skipped as one.
            // No allocation per identifier: only the first two segments of
            // the path are kept, and how many there were.
            let (xs, xe) = (i, x_end);
            let (mut ys, mut ye) = (0, 0);
            let mut segments = 1;
            let mut j = x_end;
            while j + 1 < end && b[j] == b'.' && is_ident_start(b[j + 1]) {
                let k = ident_end(b, j + 1, end);
                if segments == 1 {
                    (ys, ye) = (j + 1, k);
                }
                segments += 1;
                j = k;
            }
            i = j.max(i + 1);
            if segments != 2 {
                continue;
            }
            let qualifier = &src[xs..xe];
            if !index.names_an_enum(qualifier) && !aliased.contains_key(qualifier) {
                continue;
            }
            // Preceded by a member access (`a.X.Y`) is not the type `X`. A
            // place first in its code segment looks back over the comments
            // before it — an arm under a comment line is still an arm.
            let before = prev_non_space(b, start, xs).or_else(|| code_before(b, segs, n));
            if before.is_some_and(|p| b[p] == b'.') {
                continue;
            }
            let after = next_non_space(b, ye, end);
            if after.is_some_and(|p| matches!(b[p], b'.' | b'(' | b'<' | b'[')) {
                continue;
            }
            let Some(shape) = decision_shape(b, start, end, before, after) else {
                continue;
            };
            out.push(DecisionSite {
                offset: xs,
                shape,
                qualifier,
                value: &src[ys..ye],
            });
        }
    }
    out
}

/// The shape of a place given the code around `X.Y`: `before` is the last
/// non-space byte ahead of `X`, `after` the first behind `Y`, both within
/// the code segment `start..end`.
fn decision_shape(
    b: &[u8],
    start: usize,
    end: usize,
    before: Option<usize>,
    after: Option<usize>,
) -> Option<DecisionShape> {
    let at =
        |p: Option<usize>, text: &[u8]| p.is_some_and(|p| b[p..end.min(b.len())].starts_with(text));
    let word_after = |p: Option<usize>, word: &str| {
        p.is_some_and(|p| {
            let e = ident_end(b, p, end);
            &b[p..e] == word.as_bytes()
        })
    };
    let word_before = |p: Option<usize>| -> Option<(usize, &[u8])> {
        let p = p?;
        if !is_ident_continue(b[p]) {
            return None;
        }
        let mut s = p;
        while s > start && is_ident_continue(b[s - 1]) {
            s -= 1;
        }
        Some((s, &b[s..=p]))
    };
    let colon = at(after, b":") && !at(after, b"::");
    if let Some((_, b"case")) = word_before(before) {
        return (colon || word_after(after, "when")).then_some(DecisionShape::Label);
    }
    let opens_arm = before.is_some_and(|p| matches!(b[p], b'{' | b','));
    if opens_arm && (at(after, b"=>") || word_after(after, "when")) {
        return Some(DecisionShape::Arm);
    }
    let operator_before = before.is_some_and(|p| {
        p > start
            && b[p] == b'='
            && matches!(b[p - 1], b'=' | b'!')
            && (p < start + 2 || b[p - 2] != b'=')
    });
    let operator_after = (at(after, b"==") || at(after, b"!=")) && !at(after, b"===");
    let is_before = match word_before(before) {
        Some((_, b"is")) => true,
        Some((s, b"not")) => matches!(word_before(prev_non_space(b, start, s)), Some((_, b"is"))),
        _ => false,
    };
    (operator_before || operator_after || is_before).then_some(DecisionShape::Comparison)
}

/// The last non-space byte of code before segment `n`, across the comments
/// right before it — `None` when a string or the start of the file comes
/// first.
fn code_before(b: &[u8], segs: &[Segment], n: usize) -> Option<usize> {
    segs[..n]
        .iter()
        .rev()
        .find(|s| s.kind != SegKind::Comment)
        .filter(|s| s.kind == SegKind::Code)
        .and_then(|s| prev_non_space(b, s.start, s.end))
}

/// Where the identifier starting at `i` ends, within `..end`.
fn ident_end(b: &[u8], mut i: usize, end: usize) -> usize {
    while i < end && (is_ident_continue(b[i]) || b[i] >= 0x80) {
        i += 1;
    }
    i
}

/// The last non-space byte before `i`, not before `start`.
fn prev_non_space(b: &[u8], start: usize, i: usize) -> Option<usize> {
    (start..i).rev().find(|&p| !b[p].is_ascii_whitespace())
}

/// The first non-space byte at or after `i`, before `end`.
fn next_non_space(b: &[u8], i: usize, end: usize) -> Option<usize> {
    (i..end).find(|&p| !b[p].is_ascii_whitespace())
}

/// What the types named `X` visible at a place are, as far as the host can
/// read them (KTD3).
struct Visible<'i> {
    types: BTreeSet<&'i NodeKey>,
    /// An alias of the name points at something that is not one repository
    /// type, or a project-wide alias of the name exists whose target the
    /// reading of the project files does not keep.
    elsewhere: bool,
    /// What is visible could not be read: no type encloses the place, or a
    /// project file imports what the reading cannot follow.
    unknown: bool,
}

/// The types named `name` a place inside `owner` can see (KTD3): those
/// nested in the types around it and in the types they extend; those of its
/// namespace and every namespace above it; those of the namespaces its file
/// and its project import; those nested in a `using static` target; and an
/// alias of the name. The union of all of them — not the one C# would pick —
/// because a place is verified only when there is nothing to pick between.
fn visible_types<'i>(
    index: &'i EnumIndex,
    owner: &'i NodeKey,
    name: &'i str,
    usings: &FileUsings,
    project: &ProjectWide,
) -> Visible<'i> {
    let mut seen = Visible {
        types: BTreeSet::new(),
        elsewhere: false,
        unknown: false,
    };
    let chain = index.enclosing_types(owner);
    let Some(outermost) = chain.last() else {
        seen.unknown = true;
        return seen;
    };
    for t in index.with_ancestors(&chain) {
        seen.types.extend(index.nested_named(t, name));
    }
    let mut namespace = index.types[*outermost].namespace.as_str();
    loop {
        seen.types.extend(index.top_named(namespace, name));
        if namespace.is_empty() {
            break;
        }
        namespace = namespace.rsplit_once('.').map_or("", |(parent, _)| parent);
    }
    let (project_namespaces, project_statics) = match project {
        ProjectWide::Known(imports) => {
            seen.elsewhere |= imports.aliases.contains(name);
            (Some(&imports.namespaces), Some(&imports.statics))
        }
        ProjectWide::Unknown => {
            seen.unknown = true;
            (None, None)
        }
    };
    for ns in usings
        .namespaces
        .iter()
        .chain(project_namespaces.into_iter().flatten())
    {
        seen.types.extend(index.top_named(ns, name));
    }
    for target in usings
        .statics
        .iter()
        .chain(project_statics.into_iter().flatten())
    {
        for t in index.by_written_name(target) {
            seen.types.extend(index.nested_named(t, name));
        }
    }
    if let Some(target) = usings.aliases.get(name) {
        match index.by_written_name(target) {
            [one] => {
                seen.types.insert(one);
            }
            _ => seen.elsewhere = true,
        }
    }
    seen
}

/// Tie each place to an enum and say whether it is verified (KTD3), or
/// count it unmatched when no single enum can be named for it.
///
/// The enum named for a place is the one enum called `X` that has the value
/// `Y` — and where several have it, the one of them the place can see. A
/// label or an arm is verified when that enum is the ONLY type named `X`
/// the place can see and no alias of the name points elsewhere; a
/// comparison never is. A place written through an alias of another name
/// (`BLT.Full`, `using BLT = …BackupLoadType;`) has the alias's enum as its
/// one candidate.
fn emit_enum_decisions(
    ops: &mut GraphOps,
    run: &RunIndexes,
    file: &str,
    src: &str,
    segs: &[Segment],
    lines: &LineIndex,
    owner_of: &dyn Fn(usize) -> NodeKey,
) {
    let index = &run.enums;
    if index.enums_named.is_empty() {
        return;
    }
    let usings = FileUsings::read(src, segs);
    // The aliases of another name that stand for one enum. One named like
    // an enum keeps that name's own reading: there the alias is a reason the
    // place is unverified, as `visible_types` says.
    let aliased: HashMap<&str, &NodeKey> = usings
        .aliases
        .iter()
        .filter(|(alias, _)| !index.names_an_enum(alias))
        .filter_map(|(alias, target)| match index.by_written_name(target) {
            [one] if index.types.get(one).is_some_and(|t| t.is_enum) => Some((alias.as_str(), one)),
            _ => None,
        })
        .collect();
    let sites = decision_sites(src, segs, index, &aliased);
    if sites.is_empty() {
        return;
    }
    let project = run.project.for_file(file);
    let mut facts: BTreeSet<(NodeKey, u32, String)> = BTreeSet::new();
    let mut unmatched = 0i64;
    for site in sites {
        let owner = owner_of(site.offset);
        let visible = visible_types(index, &owner, site.qualifier, &usings, project);
        let named: &[NodeKey] = match index.enums_named.get(site.qualifier) {
            Some(enums) => enums,
            None => aliased
                .get(site.qualifier)
                .map_or(&[], |e| std::slice::from_ref(*e)),
        };
        let candidates: Vec<&NodeKey> = named
            .iter()
            .filter(|e| index.values.get(*e).is_some_and(|v| v.contains(site.value)))
            .collect();
        let target = match candidates[..] {
            [one] => Some(one),
            [] => None,
            _ => {
                let seen: Vec<&NodeKey> = candidates
                    .iter()
                    .copied()
                    .filter(|e| visible.types.contains(e))
                    .collect();
                match seen[..] {
                    [one] => Some(one),
                    _ => None,
                }
            }
        };
        let Some(target) = target else {
            unmatched += 1;
            continue;
        };
        let verified = site.shape != DecisionShape::Comparison
            && !visible.elsewhere
            && !visible.unknown
            && visible.types.len() == 1
            && visible.types.contains(target);
        let decision = EnumDecision {
            verified,
            shape: site.shape,
            enum_key: target.clone(),
            member: site.value.to_string(),
        };
        facts.insert((owner, lines.line(site.offset), decision.encode()));
    }
    for (owner, line, value) in facts {
        ops.fact(
            owner,
            Fact::new(
                FactKind::EnumDecision,
                FactValue::Text(value),
                Provenance::new(file, Some(line), ids::external(), Tier::Measured),
            ),
        );
    }
    if unmatched > 0 {
        ops.fact(
            NodeKey::new(file.to_string()),
            Fact::new(
                FactKind::EnumDecisionsUnmatched,
                FactValue::Int(unmatched),
                Provenance::new(file, None, ids::external(), Tier::Measured),
            ),
        );
    }
}

// ---- Emission ----

/// One external reference found in a file, before it is turned into graph shape.
struct Hit {
    /// Byte offset the reference is anchored at — which decides both its line and
    /// which declaration owns it.
    offset: usize,
    key: String,
    name: String,
    kind: &'static str,
}

/// What one run reads out of the graph — and, for the project-wide usings,
/// out of the tracked files — before any file is scanned.
struct RunIndexes {
    owners: OwnerIndex,
    deps: Deps,
    enums: EnumIndex,
    project: std::sync::Arc<ProjectUsings>,
}

/// Scan one file and write everything it references into the graph.
fn emit_file(
    ops: &mut GraphOps,
    run: &RunIndexes,
    lang: Lang,
    file: &str,
    src: &str,
    prefixes: &SecretPrefixes,
) {
    let (owners, deps) = (&run.owners, &run.deps);
    let segs = segments(lang, src);
    let lines = LineIndex::new(src);
    let file_key = NodeKey::new(file.to_string());
    let decls = owners.file(file);
    let names = decls.map(|d| NamePlaces::new(src, &segs, &lines, d));
    let meetings = RefCell::new(HashMap::new());
    // The innermost declaration around an offset, or the file (KTD1).
    let owner_of = |offset: usize| -> NodeKey {
        let line = lines.line(offset);
        decls
            .zip(names.as_ref())
            .and_then(|(d, names)| d.owner(line, offset, |decl| names.of(decl), &meetings))
            .cloned()
            .unwrap_or_else(|| file_key.clone())
    };
    let mut hits: Vec<Hit> = Vec::new();
    let mut redactions: Vec<(usize, &'static str)> = Vec::new();
    let mut accesses: Vec<(usize, DbAccessKind, String)> = Vec::new();
    let mut anchor_hits: Vec<AnchorHit> = Vec::new();
    let mut skips: Vec<(usize, String)> = Vec::new();

    // ---- Imports, and the names they bind ----
    let statements = imports(lang, src, deps);
    let mut bound: BTreeMap<&str, (String, &str)> = BTreeMap::new();
    for imp in &statements {
        if let Some(local) = &imp.local {
            bound.insert(
                local.as_str(),
                (external_key(lang, &imp.fqname), imp.fqname.as_str()),
            );
        }
    }

    // ---- Code: bound names used, and qualified foreign paths written out ----
    let mut used: BTreeSet<&str> = BTreeSet::new();
    for seg in segs.iter().filter(|s| s.kind == SegKind::Code) {
        for (offset, path) in paths_in(lang, src, seg.start, seg.end) {
            let head = path.split(lang.sep()).next().unwrap_or(path);
            if let Some((key, fqname)) = bound.get(head) {
                used.insert(head);
                hits.push(Hit {
                    offset,
                    key: key.clone(),
                    name: (*fqname).to_string(),
                    kind: KIND_SYMBOL,
                });
                continue;
            }
            if deps.qualifies_usage(lang, path) {
                hits.push(Hit {
                    offset,
                    key: external_key(lang, path),
                    name: path.to_string(),
                    kind: KIND_SYMBOL,
                });
                continue;
            }
            // `process.env.KEY` reads a configuration key without a literal at all
            // — the property access IS the named API.
            if lang == Lang::Ts {
                if let Some(key) = path.strip_prefix("process.env.") {
                    if !key.is_empty() && !key.contains('.') {
                        hits.push(Hit {
                            offset,
                            key: format!("res:cfg:{key}"),
                            name: key.to_string(),
                            kind: KIND_CFG,
                        });
                    }
                }
            }
        }
    }

    // An import whose bound name is never used — and one that binds no name at all
    // — is still a statement the FILE makes, and it is what keeps every External
    // node reachable rather than orphaned.
    for imp in &statements {
        let already = imp
            .local
            .as_deref()
            .map(|l| used.contains(l))
            .unwrap_or(false);
        if already {
            continue;
        }
        hits.push(Hit {
            offset: imp.offset,
            key: external_key(lang, &imp.fqname),
            name: imp.fqname.clone(),
            kind: KIND_SYMBOL,
        });
    }

    // ---- String literals ----
    let secrets = literal_secrets(src, &segs, &lines, prefixes);
    // Set by a literal that opens a key block and does not close it: a key
    // written one literal per line (`"-----BEGIN … PRIVATE KEY-----\n" +`,
    // then `"MIIE…\n" +` …) has its material in literals no rule names
    // alone. They are withheld through the one holding `-----END`, as
    // `redact_secret_block` withholds the lines of a key in a comment.
    let mut in_key = false;
    for (i, seg) in segs.iter().enumerate() {
        let SegKind::Str { interpolated } = seg.kind else {
            continue;
        };
        if in_key {
            redactions.push((seg.start, "private key"));
            in_key = !src[seg.start..seg.end].contains("-----END");
            continue;
        }
        // R22 first, and on interpolated literals too: redaction is fail-safe,
        // emission is fail-closed. A redacted literal contributes nothing else
        // — no resource, no access, no anchor (plan B, KTD11 rules 2 and 4
        // read it with the code on its line).
        if let Some(pattern) = secrets[i] {
            redactions.push((seg.start, pattern));
            // Only the literal holding the header opens the block: a later
            // literal on the header's line is named `private key` by the
            // line, and a key written whole in one literal before it must not
            // make every literal after it a key.
            let lit = &src[seg.start..seg.end];
            in_key =
                pattern == "private key" && lit.contains("-----BEGIN") && !lit.contains("-----END");
            continue;
        }
        // An interpolated literal is read by its text, holes blanked — where
        // this lexer knows the holes, which is C# alone. A TS template with a
        // hole or a Python f-string stays unread.
        if interpolated && lang != Lang::CSharp {
            continue;
        }
        let text = literal_text(src, seg);
        let gate = sql_gate(&text);
        for (offset, object) in sql_objects(&text, &gate) {
            if touches_hole(seg, &text, offset) {
                continue;
            }
            hits.push(Hit {
                offset: seg.start + offset,
                key: format!("res:db:{object}"),
                name: object,
                kind: KIND_DB,
            });
        }
        for (offset, object, access) in sql_accesses(&text, &gate) {
            if !touches_hole(seg, &text, offset) {
                accesses.push((seg.start + offset, access, object));
            }
        }
        // KTD11 rule 3: a key-shaped token is never an anchor, and says so —
        // a word of a message as well as a whole literal (`"Bearer 9f2b…"`),
        // as `comment_anchors` drops one from a comment line.
        if !(lang == Lang::Python && is_python_docstring(src, &segs, i)) {
            let (kept, long) = without_long_tokens(&text);
            if long {
                skips.push((seg.start, "long token".to_string()));
            }
            if let Some(text) = anchor_text(&kept) {
                anchor_hits.push(AnchorHit {
                    offset: seg.start,
                    source: AnchorSource::Literal,
                    text,
                });
            }
        }
        // A URL and a configuration key are one fixed value; an interpolated
        // literal is a different one per call, so it names neither.
        if interpolated {
            continue;
        }
        let lit = &src[seg.start..seg.end];
        for hit in urls(lit) {
            match hit {
                UrlHit::Endpoint(offset, host_path) => hits.push(Hit {
                    offset: seg.start + offset,
                    key: format!("res:url:{host_path}"),
                    name: host_path,
                    kind: KIND_URL,
                }),
                UrlHit::Credentials(offset) => {
                    redactions.push((seg.start + offset, "url credentials"))
                }
            }
        }
        // The named configuration API sits in the code segment right before the
        // literal — which the lexer guarantees is the previous segment.
        let code_before = match segs.get(i.wrapping_sub(1)) {
            Some(prev) if prev.kind == SegKind::Code => &src[prev.start..prev.end],
            _ => "",
        };
        if is_config_read(code_before) {
            let key = lit.trim();
            if !key.is_empty() && !key.contains('\n') {
                hits.push(Hit {
                    offset: seg.start,
                    key: format!("res:cfg:{key}"),
                    name: key.to_string(),
                    kind: KIND_CFG,
                });
            }
        }
    }

    // ---- Comments ----
    comment_anchors(lang, src, &segs, prefixes, &mut anchor_hits, &mut skips);

    // ---- Decisions by an enum's value (plan B, U5) — C# first (R13) ----
    if lang == Lang::CSharp {
        emit_enum_decisions(ops, run, file, src, &segs, &lines, &owner_of);
    }

    // ---- Into the graph ----
    //
    // Source order, so a reference made twice is anchored at its FIRST site — the
    // same discipline tree-sitter's usage flush follows, and what makes the anchor
    // independent of the order the patterns happen to run in.
    hits.sort_by(|a, b| (a.offset, &a.key).cmp(&(b.offset, &b.key)));
    let mut edges_seen: HashSet<(String, String)> = HashSet::new();
    let prov_at = |line: u32| Provenance::new(file, Some(line), ids::external(), Tier::Measured);

    for hit in hits {
        let line = lines.line(hit.offset);
        let owner = owner_of(hit.offset);
        let node_key = NodeKey::new(hit.key.clone());
        let prov = prov_at(line);
        // The identity facts belong to whichever file reaches this external
        // FIRST: two files referencing one external must not stack duplicate
        // scalar claims (the manifest-deps discipline). This file cannot know
        // whether it is first — that is a fact about the graph, not about the
        // file — so it states its claim and the reducer decides whether to keep
        // it (KTD2/KTD3).
        let identity = vec![
            Fact::new(
                FactKind::ExternalName,
                FactValue::Text(hit.name),
                prov.clone(),
            ),
            Fact::new(
                FactKind::ExternalKind,
                FactValue::Text(hit.kind.to_string()),
                prov.clone(),
            ),
        ];
        // Repeated references from one owner are one edge; that much IS decidable
        // here, because the dedup set is this file's own.
        let uses = edges_seen
            .insert((owner.as_str().to_string(), hit.key))
            .then(|| (owner, Edge::new(EdgeKind::Uses, prov)));
        ops.external(node_key, identity, uses);
    }

    // One fact per access, on the declaration around it (plan B, U2) —
    // beside the `Uses` edge above, which stays one per owner and object.
    for (offset, access, object) in accesses {
        ops.fact(
            owner_of(offset),
            Fact::new(
                FactKind::DbAccess,
                FactValue::Text(format!("{} {object}", access.word())),
                prov_at(lines.line(offset)),
            ),
        );
    }

    for (offset, pattern) in redactions {
        ops.fact(
            owner_of(offset),
            Fact::new(
                FactKind::RedactedLiteral,
                // The PATTERN's name, never the literal (R22).
                FactValue::Text(pattern.to_string()),
                prov_at(lines.line(offset)),
            ),
        );
    }

    // ---- Anchors, per owner (plan B, U3) ----
    //
    // Literals and comments were found in two passes; sorting by offset puts
    // every owner's texts back in source order, so a text's first occurrence
    // is the one its line comes from.
    anchor_hits.sort_by_key(|h| h.offset);
    let mut by_owner: BTreeMap<NodeKey, Vec<(u32, AnchorSource, String)>> = BTreeMap::new();
    for hit in anchor_hits {
        by_owner.entry(owner_of(hit.offset)).or_default().push((
            lines.line(hit.offset),
            hit.source,
            hit.text,
        ));
    }
    for (owner, hits) in by_owner {
        let (kept, dropped) = select_anchors(hits);
        for (line, anchor) in kept {
            ops.fact(
                owner.clone(),
                Fact::new(
                    FactKind::Anchor,
                    FactValue::Text(anchor.encode()),
                    prov_at(line),
                ),
            );
        }
        if dropped > 0 {
            ops.fact(
                owner,
                Fact::new(
                    FactKind::AnchorsDropped,
                    FactValue::Int(dropped as i64),
                    Provenance::new(file, None, ids::external(), Tier::Measured),
                ),
            );
        }
    }
    skips.sort();
    for (offset, reason) in skips {
        ops.fact(
            owner_of(offset),
            Fact::new(
                FactKind::AnchorSkipped,
                // The reason, never the text (R10).
                FactValue::Text(reason),
                prov_at(lines.line(offset)),
            ),
        );
    }
}

fn external_key(lang: Lang, fqname: &str) -> String {
    format!("ext:{}:{}", lang.tag(), fqname)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This module's own `ext -> language` table must claim exactly the
    /// extensions the registry does (U4).
    ///
    /// Only the extensions, not the names: `tag()` deliberately answers "ts"
    /// where coverage answers "typescript", because a tag keys `ext:` and
    /// `sym:` nodes and has to match what tree-sitter writes there. Two
    /// vocabularies for two jobs is fine; two answers to "is this file a
    /// language at all" is not, and that is what this pins.
    ///
    /// Both directions, because each produces a different false statement. An
    /// extension the registry claims and this table does not leaves a named
    /// language whose imports go unresolved. One this table claims and the
    /// registry does not is worse: `external` resolves the file's imports and
    /// creates its `ext:`/`sym:` nodes while coverage reports `language: null`
    /// and "no grammar covers this file type" about it — exactly the R26
    /// statement U4 exists to remove.
    ///
    /// What this does and does not catch, stated plainly because an earlier
    /// version of this doc claimed set equality while the body asserted only
    /// one direction. `extensions_of` matches exhaustively on `Lang`, so a new
    /// VARIANT does not compile until its extensions are stated here, and the
    /// `assert_eq!` then forces the registry to know them. An extension added
    /// to `from_path` under an EXISTING variant still slips through, because
    /// `from_path`'s arms cannot be enumerated. That is the same residual gap
    /// `bundled_languages_match_the_grammars_that_actually_parse` concedes,
    /// and closing it properly means making `from_path` read `extensions_of`
    /// rather than its own match — a production change, not a test change.
    #[test]
    fn the_import_language_table_claims_what_the_registry_claims() {
        fn extensions_of(l: Lang) -> &'static [&'static str] {
            match l {
                Lang::Rust => &["rs"],
                Lang::Ts => &["ts", "tsx", "js", "jsx", "mjs", "cjs"],
                Lang::Python => &["py"],
                Lang::CSharp => &["cs"],
                Lang::Go => &["go"],
                Lang::Java => &["java"],
                Lang::C => &["c", "h"],
                Lang::Cpp => &["cpp", "cc", "cxx", "hpp", "hh", "hxx"],
            }
        }
        const ALL: [Lang; 8] = [
            Lang::Rust,
            Lang::Ts,
            Lang::Python,
            Lang::CSharp,
            Lang::Go,
            Lang::Java,
            Lang::C,
            Lang::Cpp,
        ];

        let registry = crate::extract::languages::Languages::bundled();
        // SQL has no import statement: a script names tables, never another
        // file, so there is nothing for this pass to resolve in one.
        const NO_IMPORTS: [&str; 1] = ["sql"];
        let claimed: std::collections::BTreeSet<&str> = registry
            .extensions()
            .filter(|e| !NO_IMPORTS.contains(e))
            .collect();
        let resolved: std::collections::BTreeSet<&str> =
            ALL.into_iter().flat_map(extensions_of).copied().collect();
        assert_eq!(
            claimed, resolved,
            "external resolves imports for a file coverage calls unclassified, \
             or ignores one coverage names"
        );

        // …and every claimed extension must actually route, so the sets can
        // agree while `from_path` refuses one of them.
        for ext in registry.extensions().filter(|e| !NO_IMPORTS.contains(e)) {
            assert!(
                Lang::from_path(&format!("a.{ext}")).is_some(),
                "`.{ext}` is a registered language whose imports go unresolved"
            );
        }
    }

    /// The literals a file holds, with their interpolation flag.
    fn literals(lang: Lang, src: &str) -> Vec<(&str, bool)> {
        segments(lang, src)
            .into_iter()
            .filter_map(|s| match s.kind {
                SegKind::Str { interpolated } => Some((&src[s.start..s.end], interpolated)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn csharp_verbatim_and_interpolated_literals_are_told_apart() {
        let src = "var a = @\"line1\nline2\"\"q\";\nvar b = $\"x{y}\";\nvar c = \"plain\";\n";
        assert_eq!(
            literals(Lang::CSharp, src),
            vec![
                ("line1\nline2\"\"q", false),
                ("x{y}", true),
                ("plain", false)
            ]
        );
    }

    /// A Rust lifetime must not open a string — the failure mode would swallow the
    /// rest of the file into one literal.
    #[test]
    fn a_rust_lifetime_is_not_a_string() {
        let src = "fn f<'a>(x: &'a str) -> &'a str { \"real\" }\n";
        assert_eq!(literals(Lang::Rust, src), vec![("real", false)]);
    }

    #[test]
    fn rust_raw_strings_end_on_their_own_hash_count() {
        let src = "let q = r#\"a \"b\" c\"#;\nlet p = r\"plain\";\n";
        assert_eq!(
            literals(Lang::Rust, src),
            vec![("a \"b\" c", false), ("plain", false)]
        );
    }

    #[test]
    fn comments_are_not_code_and_their_strings_are_not_literals() {
        let src =
            "// var u = \"https://dead.example.com\";\n/* \"also dead\" */\nvar v = \"live\";\n";
        assert_eq!(literals(Lang::CSharp, src), vec![("live", false)]);
        let code: Vec<&str> = segments(Lang::CSharp, src)
            .into_iter()
            .filter(|s| s.kind == SegKind::Code)
            .map(|s| &src[s.start..s.end])
            .collect();
        assert!(
            !code.iter().any(|c| c.contains("dead")),
            "a comment leaked into a code segment: {code:?}"
        );
    }

    #[test]
    fn python_triple_quotes_and_f_strings() {
        let src = "s = \"\"\"a\nb\"\"\"\nt = f\"x{y}\"\n# \"comment\"\n";
        assert_eq!(
            literals(Lang::Python, src),
            vec![("a\nb", false), ("x{y}", true)]
        );
    }

    #[test]
    fn go_backticks_are_raw_and_not_interpolated() {
        assert_eq!(
            literals(Lang::Go, "q := `SELECT 1`\n"),
            vec![("SELECT 1", false)]
        );
        assert_eq!(
            literals(Lang::Ts, "const q = `a${b}`\n"),
            vec![("a${b}", true)]
        );
    }

    /// A TS/JS template is interpolated by its holes, not by its quote: one
    /// without `${` is one fixed text, read like any string.
    #[test]
    fn a_ts_template_without_a_hole_is_an_ordinary_literal() {
        assert_eq!(
            literals(
                Lang::Ts,
                "const m = `Chyba importu`;\nconst q = `select *\n  from dba.t`;\nconst h = `x ${y} z`;\n"
            ),
            vec![
                ("Chyba importu", false),
                ("select *\n  from dba.t", false),
                ("x ${y} z", true)
            ]
        );
    }

    /// A TS/JS template reads its backslash escapes: `\${` opens no hole and
    /// `` \` `` closes nothing. A Go backtick block stays raw — a backslash in
    /// it is a backslash, and the next backtick ends it.
    #[test]
    fn a_ts_template_reads_its_escapes_and_a_go_block_stays_raw() {
        assert_eq!(
            literals(
                Lang::Ts,
                "const m = `Missing \\${name}`;\nconst q = `say \\`hi\\` now`;\nconst z = \"next\";\n"
            ),
            vec![
                ("Missing \\${name}", false),
                ("say \\`hi\\` now", false),
                ("next", false)
            ]
        );
        assert_eq!(
            literals(Lang::Go, "q := `a\\`\nr := \"b\"\n"),
            vec![("a\\", false), ("b", false)]
        );
    }

    fn objects(lit: &str) -> Vec<(usize, String)> {
        sql_objects(lit, &sql_gate(lit))
    }

    #[test]
    fn qualified_sql_objects_need_a_verb_and_its_companion() {
        assert_eq!(
            objects("SELECT Id FROM dbo.Orders WHERE Id = 1")
                .into_iter()
                .map(|(_, o)| o)
                .collect::<Vec<_>>(),
            vec!["dbo.Orders".to_string()]
        );
        assert_eq!(
            objects("UPDATE [db].[dbo].[T] SET x = 1")
                .into_iter()
                .map(|(_, o)| o)
                .collect::<Vec<_>>(),
            vec!["db.dbo.T".to_string()]
        );
        assert_eq!(
            objects("INSERT INTO dbo.Log (a) VALUES (1)")
                .into_iter()
                .map(|(_, o)| o)
                .collect::<Vec<_>>(),
            vec!["dbo.Log".to_string()]
        );
        assert_eq!(
            objects("EXEC dbo.spRecalc @id = 1")
                .into_iter()
                .map(|(_, o)| o)
                .collect::<Vec<_>>(),
            vec!["dbo.spRecalc".to_string()]
        );
    }

    /// The KTD8 half of the contract: the shapes that must NOT fire.
    #[test]
    fn ambiguous_or_prose_sql_is_never_an_object() {
        // Prose: `update` and `from` without `set` or `select`.
        assert!(objects("Update config.value from settings.json before deploy").is_empty());
        // Unqualified: indistinguishable from a variable or a LINQ range.
        assert!(objects("select id from orders").is_empty());
        // A qualified-looking token that is really a file path.
        assert!(objects("SELECT * FROM app.config").is_empty());
        // No verb at all.
        assert!(objects("dbo.Orders").is_empty());
    }

    /// `(word, object)` for every access a literal's statements make, in the
    /// order of the literal.
    fn accesses(lit: &str) -> Vec<(&'static str, String)> {
        let mut out = sql_accesses(lit, &sql_gate(lit));
        out.sort_by_key(|(offset, _, _)| *offset);
        out.into_iter().map(|(_, o, a)| (a.word(), o)).collect()
    }

    fn pairs(expected: &[(&'static str, &str)]) -> Vec<(&'static str, String)> {
        expected.iter().map(|(a, o)| (*a, o.to_string())).collect()
    }

    /// Plan B, U2 (KTD2): the shapes the per-statement reading exists for.
    #[test]
    fn a_statement_resolves_its_target_through_the_aliases_it_declares() {
        // A subquery stays in its statement and is read; the alias, declared
        // after it, still names the target.
        assert_eq!(
            accesses("UPDATE t SET x = (SELECT max(y) FROM dba.u) FROM dba.t AS t"),
            pairs(&[("reads", "dba.u"), ("writes", "dba.t")])
        );
        assert_eq!(
            accesses("delete from dba.t where id in (select id from dba.u)"),
            pairs(&[("writes", "dba.t"), ("reads", "dba.u")])
        );
        assert_eq!(
            accesses(
                "MERGE INTO dba.t AS t USING dba.s AS s ON t.id = s.id \
                 WHEN MATCHED THEN UPDATE SET t.x = s.x"
            ),
            pairs(&[("writes", "dba.t"), ("reads", "dba.s")])
        );
        // An unqualified target matching one object's last segment.
        assert_eq!(
            accesses("UPDATE Orders SET x = 1 FROM [dbo].[Orders] JOIN dbo.Lines l ON 1 = 1"),
            pairs(&[("writes", "dbo.Orders"), ("reads", "dbo.Lines")])
        );
        assert_eq!(
            accesses("EXEC @rc = dba.p_recalc"),
            pairs(&[("calls", "dba.p_recalc")])
        );
    }

    /// A common-table expression reads what its `SELECT` names, ahead of the
    /// statement it serves — which keeps its own target.
    #[test]
    fn a_cte_reads_its_tables_and_the_statement_after_it_keeps_its_target() {
        assert_eq!(
            accesses("WITH c AS (SELECT * FROM dba.a) SELECT * FROM c"),
            pairs(&[("reads", "dba.a")])
        );
        assert_eq!(
            accesses(
                "WITH c AS (SELECT id FROM dba.a JOIN dba.x ON 1 = 1), \
                 d AS (SELECT id FROM dba.d) \
                 UPDATE dba.b SET flag = 1 WHERE id IN (SELECT id FROM c)"
            ),
            pairs(&[
                ("reads", "dba.a"),
                ("reads", "dba.x"),
                ("reads", "dba.d"),
                ("writes", "dba.b")
            ])
        );
        // No `SELECT` before it: a name ahead of every verb still claims
        // nothing more than a mention.
        assert_eq!(
            accesses("from dba.a; select x from dba.b"),
            pairs(&[("mentions", "dba.a"), ("reads", "dba.b")])
        );
    }

    /// The target is one occurrence — the one after the verb, or the source
    /// binding the alias the verb names — not every occurrence of its name:
    /// a statement that reads the table it changes reads it and writes it.
    #[test]
    fn a_statement_reading_its_own_target_reads_and_writes_it() {
        assert_eq!(
            accesses("UPDATE dba.t SET x = (SELECT MAX(x) FROM dba.t)"),
            pairs(&[("writes", "dba.t"), ("reads", "dba.t")])
        );
        assert_eq!(
            accesses("DELETE FROM dba.t WHERE id IN (SELECT id FROM dba.t WHERE y = 1)"),
            pairs(&[("writes", "dba.t"), ("reads", "dba.t")])
        );
        assert_eq!(
            accesses("UPDATE a SET x = b.x FROM dba.t AS a JOIN dba.t AS b ON a.p = b.id"),
            pairs(&[("writes", "dba.t"), ("reads", "dba.t")])
        );
        assert_eq!(
            accesses("UPDATE t SET x = 1 FROM dba.t JOIN dba.t o ON o.id = t.p"),
            pairs(&[("writes", "dba.t"), ("reads", "dba.t")])
        );
        // T-SQL's own FROM naming the target again, with no alias, is the
        // target itself; a second, aliased occurrence beside it is a read.
        assert_eq!(
            accesses("UPDATE dba.t SET x = o.x FROM dba.t JOIN dba.t AS o ON o.id = dba.t.p"),
            pairs(&[("writes", "dba.t"), ("writes", "dba.t"), ("reads", "dba.t")])
        );
    }

    /// What the reading must not claim: an object in a SQL comment or string,
    /// prose the gate refuses, and a change whose companion keyword is missing.
    #[test]
    fn comments_strings_and_prose_make_no_access() {
        assert_eq!(
            accesses("select x from dba.a -- from dba.b\n /* join dba.c */ where y = 'from dba.d'"),
            pairs(&[("reads", "dba.a")])
        );
        assert!(accesses("Update config.value from settings.json before deploy").is_empty());
        assert!(accesses("#SELECT;").is_empty());
        // `INSERT` without `INTO` claims no write; the `SELECT` feeding it is
        // its own statement and is still read.
        assert_eq!(
            accesses("exec dba.p; insert dba.t select a from dba.s"),
            pairs(&[("calls", "dba.p"), ("reads", "dba.s")])
        );
    }

    #[test]
    fn only_absolute_uninterpolated_urls_are_endpoints() {
        let hits: Vec<String> = urls("see https://api.example.com/v1/orders?x=1 now")
            .into_iter()
            .filter_map(|h| match h {
                UrlHit::Endpoint(_, hp) => Some(hp),
                UrlHit::Credentials(_) => None,
            })
            .collect();
        assert_eq!(hits, vec!["api.example.com/v1/orders".to_string()]);
        assert!(urls("/v1/orders").is_empty());
        assert!(urls("https://{host}/v1/orders").is_empty());
        // An IP-literal host would be recorded truncated, so it is not recorded.
        assert!(urls("http://[::1]:11434/api").is_empty());
        assert!(urls("https://\" + host + \"/x").is_empty());
    }

    /// R22: userinfo is part of the value, so the URL is redacted rather than
    /// published as an endpoint.
    #[test]
    fn a_url_with_credentials_is_redacted_not_published() {
        let hits = urls("ftp://user:hunter2@files.example.com/in");
        assert_eq!(hits.len(), 1);
        assert!(matches!(hits[0], UrlHit::Credentials(_)));
    }

    #[test]
    fn secret_shapes_are_named_but_never_quoted() {
        let p = SecretPrefixes::default();
        assert_eq!(
            secret_pattern(
                "Data Source=srv;Initial Catalog=K;User ID=sa;Password=abc",
                &p
            ),
            Some("password")
        );
        assert_eq!(
            secret_pattern(
                "Data Source=srv;Initial Catalog=K;Integrated Security=true",
                &p
            ),
            Some("connection string")
        );
        assert_eq!(secret_pattern("api_key: abc123", &p), Some("token"));
        // A person writing a comment types a space, not an underscore.
        assert_eq!(secret_pattern("api key = abc123", &p), Some("token"));
        assert_eq!(
            secret_pattern("-----BEGIN RSA PRIVATE KEY-----", &p),
            Some("private key")
        );
        // A log-format string carrying ONE canonical key and no `;` pairs is not a
        // connection string — the real-world false positive this guard exists for.
        assert_eq!(
            secret_pattern("(2->3) response Type={1} Server={4} Sql={5} Data=", &p),
            None
        );
        assert_eq!(secret_pattern("SELECT Id FROM dbo.Orders", &p), None);
    }

    #[test]
    fn a_provider_token_is_caught_with_no_keyword_anywhere_near_it() {
        let p = SecretPrefixes::default();
        // The shape that carries no keyword at all: a bare token in prose.
        assert_eq!(
            secret_pattern("Paste ghp_A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5 into the form", &p),
            Some("provider token")
        );
        assert_eq!(
            secret_pattern("aws_access_key AKIAIOSFODNN7EXAMPLE", &p),
            Some("provider token")
        );
        // …and the sentence that merely NAMES a prefix, which must survive: a
        // match replaces the whole line, so this is the expensive mistake.
        assert_eq!(
            secret_pattern("OpenAI keys start with sk- and GitHub PATs with ghp_", &p),
            None
        );
        assert_eq!(secret_pattern("let asia = Region::ApSoutheast1;", &p), None);
    }

    #[test]
    fn a_configured_prefix_adds_to_the_public_ones_and_stays_a_literal() {
        let p = SecretPrefixes::new(&["orbit.tok+".to_string()]);
        assert_eq!(
            secret_pattern("orbit.tok+9f2b7c11ea3d4f5a6b7c8d9e0f", &p),
            Some("provider token")
        );
        // The public list is still in force — configuring your own never turns
        // the built-in ones off.
        assert_eq!(
            secret_pattern("ghp_A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5", &p),
            Some("provider token")
        );
        // And the prefix was escaped: `.` and `+` are literals, not a pattern, so
        // a configured prefix cannot silently widen the filter.
        assert_eq!(
            secret_pattern("orbitXtokkkkkkkkkkkkkkkkkkkkkkkkk", &p),
            None
        );
    }

    #[test]
    fn a_config_key_needs_a_named_api_before_the_literal() {
        assert!(is_config_read("var s = ConfigurationManager.AppSettings["));
        assert!(is_config_read("cfg.GetSection("));
        assert!(is_config_read("var t = cfg.GetValue<int>("));
        assert!(is_config_read("v = os.getenv("));
        assert!(is_config_read("let v = std::env::var("));
        assert!(is_config_read("v = process.env["));
        // A free constant, a dictionary lookup, a plain call: no named API.
        assert!(!is_config_read("var free = "));
        assert!(!is_config_read("map["));
        assert!(!is_config_read("Log("));
    }

    /// The tail is cut at an arithmetic offset, so a multi-byte identifier before
    /// the literal must not split a character. A Czech codebase is the R17 target,
    /// which makes this a real input rather than a theoretical one.
    #[test]
    fn a_non_ascii_identifier_before_the_literal_does_not_split_a_character() {
        // Sweep the padding so the 160-byte cut lands inside a multi-byte
        // character for some of these, whatever the exact byte arithmetic.
        for pad in 100..140 {
            let padding = "x".repeat(pad);
            let tail = format!("var čísloÚčtuÚčetní{padding} = 1; var s = os.getenv(");
            assert!(is_config_read(&tail), "pad {pad}");
            let tail = format!("var čísloÚčtuÚčetní{padding} = 1; var free = ");
            assert!(!is_config_read(&tail), "pad {pad}");
        }
    }

    #[test]
    fn cargo_dependency_names_normalize_dashes_to_underscores() {
        let mut deps = Deps::default();
        deps.by_eco
            .entry(Ecosystem::Cargo)
            .or_default()
            .insert("tree-sitter-rust".to_string());
        assert!(deps.declares_import(Lang::Rust, "tree_sitter_rust::LANGUAGE"));
        assert!(deps.qualifies_usage(Lang::Rust, "tree_sitter_rust::LANGUAGE"));
        // A bare identifier equal to the crate name is far likelier a local
        // binding, so a qualified usage must reach INSIDE the crate.
        assert!(!deps.qualifies_usage(Lang::Rust, "tree_sitter_rust"));
    }

    /// A prefix test alone is wrong: a longer package name that merely starts with
    /// a declared one is a different package.
    #[test]
    fn dependency_matching_respects_segment_boundaries() {
        let mut deps = Deps::default();
        deps.by_eco
            .entry(Ecosystem::Nuget)
            .or_default()
            .insert("Newtonsoft.Json".to_string());
        assert!(deps.declares_import(Lang::CSharp, "Newtonsoft.Json"));
        assert!(deps.declares_import(Lang::CSharp, "Newtonsoft.Json.Linq"));
        assert!(!deps.declares_import(Lang::CSharp, "Newtonsoft.JsonSchema"));

        let mut npm = Deps::default();
        npm.by_eco
            .entry(Ecosystem::Npm)
            .or_default()
            .insert("@scope/pkg".to_string());
        assert!(npm.declares_import(Lang::Ts, "@scope/pkg/sub"));
        assert!(!npm.declares_import(Lang::Ts, "@scope/pkg-other"));
    }

    #[test]
    fn a_rust_use_group_binds_every_leaf() {
        let mut deps = Deps::default();
        deps.by_eco
            .entry(Ecosystem::Cargo)
            .or_default()
            .insert("serde".to_string());
        let found = rust_imports("use serde::{Serialize, de::Error as E};\n", &deps);
        let mut pairs: Vec<(Option<String>, String)> =
            found.into_iter().map(|i| (i.local, i.fqname)).collect();
        pairs.sort();
        assert_eq!(
            pairs,
            vec![
                (Some("E".to_string()), "serde::de::Error".to_string()),
                (
                    Some("Serialize".to_string()),
                    "serde::Serialize".to_string()
                ),
            ]
        );
    }

    /// A `using (…)` statement and a `using var x = …;` are not imports, and the
    /// shape must not mistake them for one.
    #[test]
    fn csharp_using_statements_are_not_imports() {
        let mut deps = Deps::default();
        deps.by_eco
            .entry(Ecosystem::Nuget)
            .or_default()
            .insert("Newtonsoft.Json".to_string());
        let src = "using Newtonsoft.Json;\nusing (var s = new Newtonsoft.Json.X()) { }\nusing var t = Open();\n";
        let found = csharp_imports(src, &deps);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].fqname, "Newtonsoft.Json");
        assert!(found[0].local.is_none());
    }

    #[test]
    fn identifiers_inside_comments_and_strings_are_not_code_paths() {
        let src = "var a = 1; // JsonConvert.Ignore\nvar b = \"JsonConvert.Ignore\";\nJsonConvert.Real();\n";
        let paths: Vec<&str> = segments(Lang::CSharp, src)
            .iter()
            .filter(|s| s.kind == SegKind::Code)
            .flat_map(|s| paths_in(Lang::CSharp, src, s.start, s.end))
            .map(|(_, p)| p)
            .collect();
        assert!(paths.contains(&"JsonConvert.Real"));
        assert!(!paths.contains(&"JsonConvert.Ignore"));
    }

    #[test]
    fn line_index_maps_offsets_to_one_based_lines() {
        let src = "a\nbb\nccc\n";
        let idx = LineIndex::new(src);
        assert_eq!(idx.line(0), 1);
        assert_eq!(idx.line(2), 2);
        assert_eq!(idx.line(5), 3);
    }

    // ---- Plan B, U3 and U4 ----

    /// The text of each literal with its holes, as `(text, [hole])`.
    fn literals_with_holes(lang: Lang, src: &str) -> Vec<(&str, Vec<&str>)> {
        segments(lang, src)
            .into_iter()
            .filter(|s| matches!(s.kind, SegKind::Str { .. }))
            .map(|s| {
                let holes = s.holes.iter().map(|&(a, b)| &src[a..b]).collect();
                (&src[s.start..s.end], holes)
            })
            .collect()
    }

    /// A raw literal spans lines and closes only on a run of its own quote
    /// count; `""` is still the empty string, not a raw opening.
    #[test]
    fn csharp_raw_literals_close_on_their_own_quote_count() {
        let src = "var a = \"\"\"\n  select \"x\" from t\n  \"\"\";\nvar b = \"\"\"\"q \"\"\" r\"\"\"\";\nvar c = \"\";\nvar d = \"live\";\n";
        assert_eq!(
            literals(Lang::CSharp, src),
            vec![
                ("\n  select \"x\" from t\n  ", false),
                ("q \"\"\" r", false),
                ("", false),
                ("live", false),
            ]
        );
    }

    /// A hole is code: a quote, a brace or a char literal inside it closes
    /// neither the hole nor the literal; `{{` is text; a raw literal's hole
    /// opens with as many braces as it has `$`.
    #[test]
    fn csharp_interpolation_holes_are_read_as_code() {
        let src = "var a = $\"x {(a ? \"}\" : '\"')} y {{z}} {n:N2} w\";\nvar b = $$\"\"\"{c} {{d}} {{{e}}}\"\"\";\nvar c = @$\"p {q}\n r\";\nvar after = \"live\";\n";
        assert_eq!(
            literals_with_holes(Lang::CSharp, src),
            vec![
                (
                    "x {(a ? \"}\" : '\"')} y {{z}} {n:N2} w",
                    vec!["{(a ? \"}\" : '\"')}", "{n:N2}"]
                ),
                ("{c} {{d}} {{{e}}}", vec!["{{d}}", "{{e}}"]),
                ("p {q}\n r", vec!["{q}"]),
                ("live", vec![]),
            ]
        );
        assert!(literals(Lang::CSharp, src)[..3].iter().all(|(_, i)| *i));
    }

    /// A char literal holding a quote is code; reading its quote as an opening
    /// would turn the real literal after it into code.
    #[test]
    fn a_char_literal_holding_a_quote_opens_no_string() {
        let src = "if (c == '\"' || c == '\\'') s = \"real\";\n";
        assert_eq!(literals(Lang::CSharp, src), vec![("real", false)]);
        assert_eq!(literals(Lang::C, src), vec![("real", false)]);
        // A digit separator is no char literal and must not eat the line.
        assert_eq!(
            literals(Lang::Cpp, "int n = 1'000'000; auto s = \"x\";\n"),
            vec![("x", false)]
        );
    }

    /// U4: C and C++ literals with an encoding prefix, a raw delimiter, and
    /// an identifier glued to the quote that is no prefix.
    #[test]
    fn c_and_cpp_literals_read_their_prefixes_and_raw_delimiters() {
        let src = "auto a = R\"(select * from dba.a)\";\nauto b = u8R\"sql(x \")\" y)sql\";\nauto c = L\"wide\";\nauto d = MACRO\"glued\";\n// \"not a literal\"\n";
        assert_eq!(
            literals(Lang::Cpp, src),
            vec![
                ("select * from dba.a", false),
                ("x \")\" y", false),
                ("wide", false),
                ("glued", false),
            ]
        );
        let code: String = segments(Lang::Cpp, src)
            .iter()
            .filter(|s| s.kind == SegKind::Code)
            .map(|s| &src[s.start..s.end])
            .collect();
        assert!(code.contains("MACRO"), "{code}");
        assert!(!code.contains("u8R") && !code.contains("L\""), "{code}");
        // A backslash before a CRLF line break continues the literal as one
        // before a bare LF does: the next line's quote closes it.
        let crlf = "auto e = \"x\\\r\ny\";\r\nauto f = \"z\";\r\n";
        assert_eq!(
            literals(Lang::Cpp, crlf),
            vec![("x\\\r\ny", false), ("z", false)]
        );
    }

    fn decl(key: &str, start: u32, end: u32) -> Decl {
        Decl {
            key: NodeKey::new(key),
            start,
            end,
            name: key.to_string(),
        }
    }

    /// KTD1: a thousand members and then a thousand type-level texts must not
    /// cost a thousand steps each. Fifty thousand of each would take the old
    /// backwards scan over a billion steps; the parent walk takes two per
    /// query, so the bound below is generous by orders of magnitude.
    #[test]
    fn the_owner_lookup_stays_linear_over_many_declarations() {
        let n = 50_000u32;
        let mut decls = vec![decl("C", 1, 2 * n + 10)];
        decls.extend((0..n).map(|k| decl(&format!("m{k}"), 2 + k, 2 + k)));
        let file = FileDecls::new(decls);
        let meetings = RefCell::new(HashMap::new());
        let started = std::time::Instant::now();
        for line in n + 3..2 * n + 3 {
            assert_eq!(
                file.owner(line, 1, |_| None, &meetings)
                    .map(NodeKey::as_str),
                Some("C"),
                "line {line}"
            );
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "the owner lookup is no longer linear: {:?}",
            started.elapsed()
        );
        assert_eq!(
            file.owner(5, 1, |_| None, &meetings).map(NodeKey::as_str),
            Some("m3")
        );
        assert_eq!(file.owner(2 * n + 11, 1, |_| None, &meetings), None);
    }

    /// KTD1: two declarations sharing a line are told apart by where their
    /// names are written, and nothing decides when a name cannot be placed.
    #[test]
    fn a_shared_line_is_decided_by_the_column_or_left_to_the_file() {
        let file = FileDecls::new(vec![decl("C", 1, 9), decl("A", 5, 5), decl("B", 5, 5)]);
        // Line 5 starts at byte 500 here: A's name at 510, B's at 530.
        let names = |d: &Decl| match d.key.as_str() {
            "A" => Some(510),
            "B" => Some(530),
            _ => None,
        };
        let meetings = RefCell::new(HashMap::new());
        let owner = |at: usize| file.owner(5, at, names, &meetings).map(NodeKey::as_str);
        assert_eq!(owner(520), Some("A"));
        assert_eq!(owner(540), Some("B"));
        assert_eq!(owner(505), None, "before every name: no owner can be told");
        assert_eq!(
            file.owner(5, 520, |_| None, &RefCell::new(HashMap::new())),
            None
        );
        // Two names written at one place tell nothing apart.
        let tied = |d: &Decl| (d.key.as_str() != "C").then_some(510);
        assert_eq!(
            file.owner(5, 520, tied, &RefCell::new(HashMap::new())),
            None
        );
        // The lines around the shared one need no name at all.
        assert_eq!(
            file.owner(6, 600, |_| None, &meetings).map(NodeKey::as_str),
            Some("C")
        );
    }

    /// KTD1: a minified file is one line of every declaration and every
    /// literal. Each literal's owner is told by the name written last before
    /// it — which must not cost a scan of the whole line per declaration per
    /// literal: that is cubic in the line, minutes for one vendor bundle.
    #[test]
    fn a_minified_line_of_many_declarations_finds_each_owner_in_linear_time() {
        let n = 1_000;
        let file = "min.js";
        let src: String = (0..n)
            .map(|k| format!("function f{k}(){{return \"ab{k}\"}}"))
            .collect::<String>()
            + "\n";
        let mut graph = Graph::new();
        for k in 0..n {
            let key = NodeKey::new(format!("sym:ts:{file}#f{k}()"));
            graph.ensure_node(key.clone(), NodeKind::Symbol).unwrap();
            graph.add_fact(
                &key,
                Fact::new(
                    FactKind::SymbolName,
                    FactValue::Text(format!("f{k}")),
                    Provenance::new(file, None, ids::external(), Tier::Measured).with_range(1, 1),
                ),
            );
        }
        let run = RunIndexes {
            deps: Deps::build(&Graph::new()),
            owners: OwnerIndex::build(&graph),
            enums: EnumIndex::default(),
            project: std::sync::Arc::new(ProjectUsings::default()),
        };
        let mut ops = GraphOps::default();
        let started = std::time::Instant::now();
        emit_file(
            &mut ops,
            &run,
            Lang::Ts,
            file,
            &src,
            &SecretPrefixes::default(),
        );
        let elapsed = started.elapsed();
        let mut owned: Vec<(String, String)> = ops
            .0
            .into_iter()
            .filter_map(|op| match op {
                crate::extract::walk::GraphOp::Fact { key, fact }
                    if fact.kind == FactKind::Anchor =>
                {
                    match fact.value {
                        FactValue::Text(t) => Some((
                            key.as_str().to_string(),
                            AnchorText::decode(&t).expect("decodes").text,
                        )),
                        _ => None,
                    }
                }
                _ => None,
            })
            .collect();
        owned.sort();
        let mut expected: Vec<(String, String)> = (0..n)
            .map(|k| (format!("sym:ts:{file}#f{k}()"), format!("ab{k}")))
            .collect();
        expected.sort();
        assert_eq!(owned, expected);
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "the owners of one shared line are read once per literal again: {elapsed:?}"
        );
    }

    /// Which secret the name `code` ends by assigning to names.
    fn secret_name_assigned(code: &str) -> Option<&'static str> {
        secret_name(last_segment(assigned_name(code)?))
    }

    /// KTD11 rule 2: the name a literal is assigned to says what it is, in
    /// the spellings a keyword at a word boundary never sees.
    #[test]
    fn a_secret_is_known_by_the_name_it_is_assigned_to() {
        assert_eq!(secret_name_assigned("secret = "), Some("token"));
        assert_eq!(secret_name_assigned("    _password = "), Some("password"));
        assert_eq!(
            secret_name_assigned("var SmtpPassword = "),
            Some("password")
        );
        assert_eq!(secret_name_assigned("new { GoogleApiKey: "), Some("token"));
        assert_eq!(secret_name_assigned("if (dbPwd == "), Some("password"));
        assert_eq!(secret_name_assigned("noveHeslo = "), Some("password"));
        assert_eq!(secret_name_assigned("PasswordLabel = "), None);
        assert_eq!(secret_name_assigned("var message = "), None);
        assert_eq!(secret_name_assigned("x => "), None);
        assert_eq!(secret_name_assigned("Show("), None);
    }

    /// KTD11 rule 3: a key or a hash is a long token; a long identifier, an
    /// object name and a sentence are not.
    #[test]
    fn a_long_token_is_shaped_like_a_key_not_like_a_name() {
        assert!(is_long_token("9f2b7c11ea3d40aa9c1f"));
        assert!(is_long_token("QmFzZTY0RW5jb2RlZFNlY3JldDE="));
        assert!(!is_long_token("SetScheduledTasksInvalid"));
        assert!(!is_long_token("dba.ops_schedule_tasks_2024"));
        assert!(!is_long_token("ERR_CODE_2024_INVALID_STATE"));
        assert!(!is_long_token("9f2b7c11ea3d"));
    }

    /// KTD11 rule 4: the Czech words and SQL's spelling, only with a value —
    /// support's "Chybné heslo" and a form's "Heslo:" pass.
    #[test]
    fn czech_and_sql_password_shapes_need_a_value() {
        let p = SecretPrefixes::default();
        assert_eq!(secret_pattern("heslo = \"pivo\"", &p), Some("password"));
        assert_eq!(secret_pattern("Hesla: tajne123", &p), Some("password"));
        assert_eq!(secret_pattern("tajný klíč = abc", &p), Some("token"));
        assert_eq!(
            secret_pattern("create login k with password 'abc'", &p),
            Some("password")
        );
        assert_eq!(
            secret_pattern("ALTER LOGIN k WITH PASSWORD 'abc'", &p),
            Some("password")
        );
        for prose in [
            "Chybné heslo",
            "Chybné heslo:",
            "Heslo:",
            "heslo = \"\"",
            "Zadejte klíč",
            "the password is checked twice",
        ] {
            assert_eq!(secret_pattern(prose, &p), None, "{prose}");
        }
    }

    /// A hole glued to a qualified name makes it a different name per call.
    #[test]
    fn a_name_glued_to_a_hole_names_nothing() {
        let src = "var q = $\"select * from dba.v{suffix} join dba.w w on 1 = 1\";\n";
        let seg = segments(Lang::CSharp, src)
            .into_iter()
            .find(|s| matches!(s.kind, SegKind::Str { .. }))
            .unwrap();
        let text = literal_text(src, &seg);
        assert!(!text.contains("suffix"));
        let names: Vec<String> = sql_objects(&text, &sql_gate(&text))
            .into_iter()
            .filter(|(o, _)| !touches_hole(&seg, &text, *o))
            .map(|(_, n)| n)
            .collect();
        assert_eq!(names, vec!["dba.w".to_string()]);
    }

    /// The patterns of every `RedactedLiteral` one file's pass states, in
    /// the order it states them, read through the file pass itself over an
    /// empty graph.
    fn redacted_in(lang: Lang, file: &str, src: &str) -> Vec<String> {
        let run = RunIndexes {
            deps: Deps::build(&Graph::new()),
            owners: OwnerIndex::default(),
            enums: EnumIndex::default(),
            project: std::sync::Arc::new(ProjectUsings::default()),
        };
        let mut ops = GraphOps::default();
        emit_file(&mut ops, &run, lang, file, src, &SecretPrefixes::default());
        ops.0
            .into_iter()
            .filter_map(|op| match op {
                crate::extract::walk::GraphOp::Fact { fact, .. }
                    if fact.kind == FactKind::RedactedLiteral =>
                {
                    match fact.value {
                        FactValue::Text(t) => Some(t),
                        _ => None,
                    }
                }
                _ => None,
            })
            .collect()
    }

    /// KTD11 rule 2 reads a literal with its line from the line's START, not
    /// from a window before it: a key far back on a long line still names
    /// every later value on it, a literal before the key is not named by it,
    /// and a more specific shape later on the line takes over from there.
    #[test]
    fn a_long_line_names_its_literals_by_everything_before_them() {
        let padding = "f(a, b); ".repeat(200);
        let src = format!(
            "x = \"a\"; pwd = \"b\"; {padding}z = \"far\"; \
             u = \"mongodb://app:hunter2@db01/k\"; w = \"q\";\n"
        );
        assert_eq!(
            redacted_in(Lang::Ts, "a.ts", &src),
            vec![
                "password",
                "password",
                "credentials in a URL",
                "credentials in a URL"
            ]
        );
    }

    /// Reading a line's literals together answers, at every literal, exactly
    /// what reading the line from its start through that literal does — on
    /// a line whose answer changes several times, through every rule.
    #[test]
    fn a_line_read_once_answers_what_each_prefix_does() {
        let p = SecretPrefixes::default();
        let parts = [
            "a",
            "x",
            "klíč=v",
            "y",
            "heslo: w",
            "b",
            "pwd=1",
            "c",
            "server=s;",
            "d",
            "port=1;",
            "e",
            "api_key=z",
            "f",
            "u://a:b@h",
            "g",
            "ghp_A1b2C3d4E5f6G7h8I9j0",
            "h",
            "-----BEGIN",
            "i",
            "PRIVATE KEY",
            "j",
        ];
        let mut src = String::from("    ");
        let mut ends = Vec::new();
        for part in parts {
            src.push_str(part);
            src.push_str(" q ");
            ends.push(src.len());
        }
        let line_start = 2;
        let naive: Vec<Option<&str>> = ends
            .iter()
            .map(|&end| secret_pattern(&src[line_start..end], &p))
            .collect();
        assert_eq!(line_secret_patterns(&src, line_start, &ends, &p), naive);
        assert_eq!(naive.first(), Some(&None));
        assert_eq!(naive.last(), Some(&Some("private key")));
        let distinct: BTreeSet<Option<&str>> = naive.iter().copied().collect();
        assert_eq!(distinct.len(), 5, "{naive:?}");
    }

    /// The quoted-key, markup and setting shapes keep the rule the line
    /// reading relies on: each one only grows with the text, so a line where
    /// each of them in turn changes the answer is read exactly as reading
    /// every prefix would.
    #[test]
    fn a_line_of_quoted_keys_and_markup_answers_what_each_prefix_does() {
        let p = SecretPrefixes::default();
        let parts = [
            "a",
            "{\"kod\": \"x\"}",
            "b",
            "<add key=\"AppToken\" value=",
            "c",
            "<apikey>z",
            "d",
            "<Pwd>v",
            "e",
            "\\\"heslo\\\":\\\"w",
            "f",
            "\"pwd\": 1",
            "g",
            "-----BEGIN",
            "h",
            "PRIVATE KEY",
            "i",
        ];
        let mut src = String::from("    ");
        let mut ends = Vec::new();
        for part in parts {
            src.push_str(part);
            src.push_str(" q ");
            ends.push(src.len());
        }
        let line_start = 2;
        let naive: Vec<Option<&str>> = ends
            .iter()
            .map(|&end| secret_pattern(&src[line_start..end], &p))
            .collect();
        assert_eq!(line_secret_patterns(&src, line_start, &ends, &p), naive);
        let token = Some("token");
        let password = Some("password");
        let key = Some("private key");
        assert_eq!(
            naive,
            [
                None, None, None, token, token, token, token, password, password, password,
                password, password, password, password, password, key, key
            ]
        );
    }

    /// A C or C++ macro that defines a credential is read by its name, as an
    /// assignment is: the name's last words say what the value is.
    #[test]
    fn a_define_of_a_secret_name_is_withheld() {
        let src = "#define DB_PASSWORD \"x1\"\n#  define API_KEY \"x2\"\n\
                   #define GREETING \"hello\"\n#define TIMEOUT_MS \"30\"\n";
        assert_eq!(redacted_in(Lang::C, "k.h", src), vec!["password", "token"]);
    }

    /// A key written as one literal per line is withheld through its
    /// `-----END` literal — a body literal names no secret alone — and the
    /// literal after it is read again.
    #[test]
    fn a_key_in_concatenated_literals_is_withheld_through_its_end() {
        let src = "const k = \"-----BEGIN RSA PRIVATE KEY-----\\n\" +\n  \"abc\\n\" +\n  \
                   \"-----END RSA PRIVATE KEY-----\\n\";\nconst m = \"hello\";\n";
        assert_eq!(
            redacted_in(Lang::Ts, "a.ts", src),
            vec!["private key", "private key", "private key"]
        );
    }

    /// A minified file is one line holding thousands of literals. Reading
    /// that line from its start through every literal on it rescanned the
    /// line once per literal — 579 s for a single 11 MB vendor bundle of
    /// the reference C# repository. The line must be read a bounded number of times, not once
    /// per literal.
    #[test]
    fn a_minified_line_of_many_literals_is_read_in_linear_time() {
        let literals: Vec<String> = (0..8_000).map(|k| format!("\"k{k}\"")).collect();
        let src = format!("var t=[{}];\n", literals.join(","));
        let started = std::time::Instant::now();
        assert!(redacted_in(Lang::Ts, "min.js", &src).is_empty());
        assert!(
            started.elapsed() < std::time::Duration::from_secs(3),
            "a long line is read once per literal again: {:?}",
            started.elapsed()
        );
    }

    /// Plan 2a, U7 (R18): a block comment right after a literal or after the
    /// call it closes is handed out as the lexer split it, by the literal's
    /// content start; a comma between them, a line comment, a doc comment or
    /// a line break makes it no comment of the literal.
    #[test]
    fn a_block_comment_right_after_a_call_argument_is_the_literals() {
        let src = "x = Get(\"a\") /*\"Label A\"*/;\n\
                   y = Get(\"b\", \"c\") /* Label  C */;\n\
                   z = Get(\"d\"), /* no */ w;\n\
                   v = Get(\"e\"); // no\n\
                   u = Get(\"f\") /** no */;\n\
                   t = Get(\"g\")\n    /* no */;\n";
        let found = comments_after_literals("src/A.cs", src).expect("a C# lexer");
        let got: Vec<String> = found
            .iter()
            .map(|c| format!("{:?} {:?} {:?} {}", c.literal, c.start, c.end, c.text))
            .collect();
        assert_eq!(
            got,
            [
                "(1, 10) (1, 14) (1, 26) Label A",
                "(2, 15) (2, 19) (2, 32) Label C",
            ]
        );
        assert!(comments_after_literals("src/A.xaml", src).is_none());
    }
}
