//! The manifest-deps extractor (U8): dependency edges and entry-point
//! declarations parsed from ecosystem manifests — the highest-trust *scalar-fact*
//! source on the ladder (E3). Its `DependsOn` edges and tree-sitter's `Imports`
//! edges are distinct `EdgeKind`s that coexist; trust-rank governs which claim
//! wins a *scalar fact* conflict, not which edge survives (edges only dedup on
//! byte-identical duplicates — see `graph::merge::dedup_edges`).
//!
//! Parsed manifests (tracked-only, KTD10): `Cargo.toml` (the `toml` crate),
//! `package.json` (`serde_json`), `go.mod` (plain text), `*.csproj`
//! (`quick-xml`). Each yields a shared shape — a set of `(name, scope)`
//! dependencies plus entry-point declarations — that a single emitter turns
//! into graph nodes/edges/facts.
//!
//! Key-space (KTD-B1 discipline): a dependency node key is
//! `dep:<ecosystem>:<name>` (e.g. `dep:cargo:serde`, `dep:npm:react`). The
//! `dep:` prefix keeps these keys disjoint from file/module/symbol keys, so a
//! dependency named the same as a source file never trips the `ensure_node`
//! kind-guard.
//!
//! Trust-rank 40 — the highest on the ladder (KTD-B8). Rank matters only for
//! *scalar-fact* precedence in U9's merge (E6); manifest-declared scalar facts
//! win over lower-ranked claims. It does not resolve edges: a `DependsOn` edge
//! and a tree-sitter `Imports` edge are different `EdgeKind`s and never conflict
//! — both coexist in the graph (E3).
//!
//! Honest degradation (E4): a missing, unreadable, or malformed manifest is
//! skipped with a terse note and never a panic; every other ecosystem in the
//! same repo is parsed independently, so one broken file cannot suppress the
//! rest. Determinism (KTD7): dependency names and entry declarations are
//! funneled through `BTreeMap`/`BTreeSet` before emission, so output is
//! byte-identical across runs regardless of a manifest's own key order.

use super::ids;
use super::{ExtractError, Extractor, Repo};
use crate::graph::{
    Edge, EdgeKind, ExtractorId, Fact, FactKind, FactValue, Graph, NodeKey, NodeKind, Provenance,
    Tier,
};
use std::collections::{BTreeMap, BTreeSet};

/// Trust-rank 40 (KTD-B8): the highest — manifest-declared scalar facts win a
/// scalar conflict in merge (E6). Not an edge-precedence rank: `DependsOn` and
/// tree-sitter `Imports` are distinct edge kinds that coexist (E3).
const TRUST_RANK: u8 = 40;

/// Scope of a dependency within its manifest.
const SCOPE_NORMAL: &str = "normal";
const SCOPE_DEV: &str = "dev";

pub struct ManifestDeps;

/// A parsed manifest, in the shared shape the emitter consumes. `deps` maps a
/// dependency name to the set of scopes it was declared under (a dep in both a
/// normal and a dev table carries both — additive, truthful). `entries` is the
/// set of entry-point declarations (`[[bin]]`/`[lib]`, `main`/`bin`,
/// `OutputType`). Both are sorted containers, so emission is deterministic.
struct Parsed {
    ecosystem: &'static str,
    deps: BTreeMap<String, BTreeSet<&'static str>>,
    entries: BTreeSet<String>,
}

impl Extractor for ManifestDeps {
    fn id(&self) -> ExtractorId {
        ids::manifest_deps()
    }

    fn emitted_kinds(&self) -> &'static [FactKind] {
        &[
            FactKind::DependencyName,
            FactKind::DependencyScope,
            FactKind::EntryPointDecl,
        ]
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
        for tf in repo.tracked_files_with_paths()? {
            let key = tf.key.as_str();
            let base = key.rsplit('/').next().unwrap_or(key);
            // Dispatch by manifest kind. `.csproj` is matched by suffix (the
            // project name varies); the rest by exact basename.
            let dispatch = if base == "Cargo.toml" {
                Some(parse_cargo as fn(&str) -> Option<Parsed>)
            } else if base == "package.json" {
                Some(parse_package_json as fn(&str) -> Option<Parsed>)
            } else if base == "go.mod" {
                Some(parse_go_mod as fn(&str) -> Option<Parsed>)
            } else if key.ends_with(".csproj") {
                Some(parse_csproj as fn(&str) -> Option<Parsed>)
            } else {
                None
            };
            let Some(parse) = dispatch else {
                continue;
            };
            // Read from the git INDEX BLOB, not the working tree (KTD10
            // determinism); a symlink (blob = target path text) or unreadable blob
            // is a skip, not fatal (E4). Manifests are text: decode UTF-8 lossily
            // so an odd byte does not drop a whole valid manifest. A parse failure
            // degrades to `None`. Facts key off `tf.key`.
            if tf.is_symlink {
                continue;
            }
            let Ok(bytes) = repo.blob_bytes(&tf) else {
                eprintln!("manifest-deps: skipping unreadable manifest {key}");
                continue;
            };
            let content = String::from_utf8_lossy(&bytes);
            match parse(&content) {
                Some(parsed) => emit(graph, key, &parsed),
                None => eprintln!("manifest-deps: skipping malformed manifest {key}"),
            }
        }
        Ok(())
    }
}

/// Turn a parsed manifest into graph nodes/edges/facts. The manifest's own File
/// node is the edge source (filetree created it); if it is somehow absent the
/// manifest is skipped rather than fabricated.
fn emit(graph: &mut Graph, manifest_file: &str, parsed: &Parsed) {
    let manifest_key = NodeKey::new(manifest_file.to_string());
    match graph.node(&manifest_key) {
        Some(node) if node.kind == NodeKind::File => {}
        _ => return,
    }
    let prov = || Provenance::new(manifest_file, None, ids::manifest_deps(), Tier::Measured);

    for (name, scopes) in &parsed.deps {
        let dep_key = NodeKey::new(format!("dep:{}:{}", parsed.ecosystem, name));
        // The name fact mirrors the opaque key's identity; emit it once, when
        // the node is first created, so two manifests naming the same dep do
        // not stack duplicate name facts (KTD-B1 opaque-key discipline).
        let is_new = graph.node(&dep_key).is_none();
        // The `dep:` prefix keeps this disjoint from file/module/symbol keys, so
        // a collision is unreachable in practice; if the invariant ever broke,
        // skip this dependency rather than corrupt a wrong-kind node (E4 idiom).
        if let Err(e) = graph.ensure_node(dep_key.clone(), NodeKind::Dependency) {
            eprintln!("manifest-deps: {e}");
            continue;
        }
        if is_new {
            graph.add_fact(
                &dep_key,
                Fact::new(
                    FactKind::DependencyName,
                    FactValue::Text(name.clone()),
                    prov(),
                ),
            );
        }
        for scope in scopes {
            graph.add_fact(
                &dep_key,
                Fact::new(
                    FactKind::DependencyScope,
                    FactValue::Text((*scope).to_string()),
                    prov(),
                ),
            );
        }
        // manifest --DependsOn--> dependency. Deduped per (manifest, dep) by
        // construction: `deps` holds one entry per name.
        graph.add_edge(
            &manifest_key,
            &dep_key,
            Edge::new(EdgeKind::DependsOn, prov()),
        );
    }

    for entry in &parsed.entries {
        graph.add_fact(
            &manifest_key,
            Fact::new(
                FactKind::EntryPointDecl,
                FactValue::Text(entry.clone()),
                prov(),
            ),
        );
    }
}

/// Insert a `(name, scope)` pair into a deps map, unioning scopes per name.
fn add_dep(deps: &mut BTreeMap<String, BTreeSet<&'static str>>, name: &str, scope: &'static str) {
    let name = name.trim();
    if name.is_empty() {
        return;
    }
    deps.entry(name.to_string()).or_default().insert(scope);
}

// ---- Cargo.toml (toml crate) ----

/// Parse a `Cargo.toml`: `[dependencies]`/`[dev-dependencies]` (scope-tagged)
/// and `[[bin]]`/`[lib]` as entry-point declarations. A parse error → `None`.
fn parse_cargo(content: &str) -> Option<Parsed> {
    let table: toml::Table = content.parse().ok()?;
    let mut deps = BTreeMap::new();
    for name in table_keys(table.get("dependencies")) {
        add_dep(&mut deps, &name, SCOPE_NORMAL);
    }
    for name in table_keys(table.get("dev-dependencies")) {
        add_dep(&mut deps, &name, SCOPE_DEV);
    }

    let mut entries = BTreeSet::new();
    // `[[bin]]` — an array of tables, each with a `name` and/or `path`.
    if let Some(toml::Value::Array(bins)) = table.get("bin") {
        for bin in bins {
            if let Some(decl) = bin
                .get("name")
                .or_else(|| bin.get("path"))
                .and_then(|v| v.as_str())
            {
                entries.insert(format!("bin:{decl}"));
            }
        }
    }
    // `[lib]` — a single table declaring a library crate target.
    if let Some(lib) = table.get("lib") {
        let decl = lib.get("name").and_then(|v| v.as_str()).unwrap_or("lib");
        entries.insert(format!("lib:{decl}"));
    }

    Some(Parsed {
        ecosystem: "cargo",
        deps,
        entries,
    })
}

/// The keys of a `toml` value when it is a table; empty otherwise.
fn table_keys(value: Option<&toml::Value>) -> Vec<String> {
    match value {
        Some(toml::Value::Table(t)) => t.keys().cloned().collect(),
        _ => Vec::new(),
    }
}

// ---- package.json (serde_json) ----

/// Parse a `package.json`: `dependencies`/`devDependencies` (scope-tagged) and
/// `main`/`bin` as entry-point declarations. A parse error → `None`.
fn parse_package_json(content: &str) -> Option<Parsed> {
    let root: serde_json::Value = serde_json::from_str(content).ok()?;
    let mut deps = BTreeMap::new();
    for name in json_object_keys(root.get("dependencies")) {
        add_dep(&mut deps, &name, SCOPE_NORMAL);
    }
    for name in json_object_keys(root.get("devDependencies")) {
        add_dep(&mut deps, &name, SCOPE_DEV);
    }

    let mut entries = BTreeSet::new();
    // `main` — the package entry module (a string).
    if let Some(main) = root.get("main").and_then(|v| v.as_str()) {
        entries.insert(format!("main:{main}"));
    }
    // `bin` — either a string (single binary) or an object of name→path.
    match root.get("bin") {
        Some(serde_json::Value::String(path)) => {
            entries.insert(format!("bin:{path}"));
        }
        Some(serde_json::Value::Object(map)) => {
            for (name, path) in map {
                let path = path.as_str().unwrap_or("");
                entries.insert(format!("bin:{name}={path}"));
            }
        }
        _ => {}
    }

    Some(Parsed {
        ecosystem: "npm",
        deps,
        entries,
    })
}

/// The keys of a JSON value when it is an object; empty otherwise.
fn json_object_keys(value: Option<&serde_json::Value>) -> Vec<String> {
    match value {
        Some(serde_json::Value::Object(map)) => map.keys().cloned().collect(),
        _ => Vec::new(),
    }
}

// ---- go.mod (plain text) ----

/// Parse a `go.mod`'s `require` directives — both the block form
/// (`require ( … )`) and single-line form (`require path v1.2.3`). Only
/// `require` yields dependencies; the module's own path (`module …`), the `go`
/// version line, and `replace`/`exclude`/`retract` are never dependencies. Go
/// has no dev/normal split, so every dependency is `normal`. This is text
/// parsing — always `Some`; a garbled file simply yields fewer deps (E4).
fn parse_go_mod(content: &str) -> Option<Parsed> {
    let mut deps = BTreeMap::new();
    let mut in_require_block = false;
    for raw in content.lines() {
        // Strip a line comment (`// indirect`, etc.) before tokenizing.
        let line = match raw.split_once("//") {
            Some((code, _)) => code,
            None => raw,
        }
        .trim();
        if line.is_empty() {
            continue;
        }
        if in_require_block {
            if line == ")" {
                in_require_block = false;
                continue;
            }
            if let Some(path) = line.split_whitespace().next() {
                add_dep(&mut deps, path, SCOPE_NORMAL);
            }
            continue;
        }
        if line == "require (" || line == "require(" {
            in_require_block = true;
            continue;
        }
        if let Some(rest) = line.strip_prefix("require ") {
            // Single-line require: `require <path> <version>`.
            if let Some(path) = rest.split_whitespace().next() {
                add_dep(&mut deps, path, SCOPE_NORMAL);
            }
        }
        // module/go/replace/exclude/retract lines are intentionally ignored.
    }

    Some(Parsed {
        ecosystem: "go",
        deps,
        entries: BTreeSet::new(),
    })
}

// ---- *.csproj (quick-xml) ----

/// Parse a `.csproj`: `<PackageReference Include="…"/>` dependencies and
/// `<OutputType>…</OutputType>` as an entry-point declaration. `.csproj` has no
/// dev/normal split, so every package reference is `normal`. A reader error →
/// `None` (E4). `PackageReference` is typically a self-closing element, so both
/// `Start` and `Empty` events must be handled — the classic quick-xml trap.
fn parse_csproj(content: &str) -> Option<Parsed> {
    use quick_xml::events::Event;
    use quick_xml::Reader;

    let mut reader = Reader::from_str(content);
    let mut deps = BTreeMap::new();
    let mut entries = BTreeSet::new();
    let mut in_output_type = false;

    loop {
        match reader.read_event() {
            Ok(Event::Eof) => break,
            Ok(Event::Start(ref e)) => {
                if e.name().as_ref() == b"PackageReference" {
                    if let Some(name) = package_reference_name(e) {
                        add_dep(&mut deps, &name, SCOPE_NORMAL);
                    }
                }
                if e.name().as_ref() == b"OutputType" {
                    in_output_type = true;
                }
            }
            Ok(Event::Empty(ref e)) => {
                // Self-closing `<PackageReference … />` — the common form.
                if e.name().as_ref() == b"PackageReference" {
                    if let Some(name) = package_reference_name(e) {
                        add_dep(&mut deps, &name, SCOPE_NORMAL);
                    }
                }
            }
            Ok(Event::Text(e)) if in_output_type => {
                if let Ok(text) = e.decode() {
                    let text = text.trim();
                    if !text.is_empty() {
                        entries.insert(format!("output_type:{text}"));
                    }
                }
                in_output_type = false;
            }
            Ok(Event::End(ref e)) if e.name().as_ref() == b"OutputType" => {
                in_output_type = false;
            }
            Ok(_) => {}
            Err(_) => return None, // malformed XML → skip the whole manifest (E4)
        }
    }

    Some(Parsed {
        ecosystem: "nuget",
        deps,
        entries,
    })
}

/// The `Include` attribute of a `<PackageReference>` element, if present.
fn package_reference_name(e: &quick_xml::events::BytesStart) -> Option<String> {
    for attr in e.attributes().flatten() {
        if attr.key.as_ref() == b"Include" {
            // NuGet package IDs are plain ASCII (letters, digits, `.`, `-`, `_`)
            // with no XML entities, so a lossy UTF-8 decode of the raw attribute
            // value is exact here and avoids quick-xml's version-parameterized
            // unescaping API.
            let value = String::from_utf8_lossy(&attr.value).trim().to_string();
            return if value.is_empty() { None } else { Some(value) };
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scopes recorded for a dependency name, sorted.
    fn scopes_of<'a>(p: &'a Parsed, name: &str) -> Vec<&'a str> {
        p.deps
            .get(name)
            .map(|s| s.iter().copied().collect())
            .unwrap_or_default()
    }

    #[test]
    fn cargo_distinguishes_normal_and_dev_deps() {
        let p = parse_cargo(
            "[dependencies]\nserde = \"1\"\npetgraph = \"0.6\"\n\n[dev-dependencies]\ntempfile = \"3\"\n",
        )
        .unwrap();
        assert_eq!(p.ecosystem, "cargo");
        assert_eq!(scopes_of(&p, "serde"), vec!["normal"]);
        assert_eq!(scopes_of(&p, "petgraph"), vec!["normal"]);
        assert_eq!(scopes_of(&p, "tempfile"), vec!["dev"]);
    }

    #[test]
    fn cargo_dep_in_both_tables_carries_both_scopes() {
        let p = parse_cargo("[dependencies]\nx = \"1\"\n[dev-dependencies]\nx = \"1\"\n").unwrap();
        assert_eq!(scopes_of(&p, "x"), vec!["dev", "normal"]);
    }

    #[test]
    fn cargo_bin_and_lib_become_entry_declarations() {
        let p = parse_cargo(
            "[[bin]]\nname = \"tool\"\npath = \"src/main.rs\"\n\n[lib]\nname = \"mylib\"\n",
        )
        .unwrap();
        assert!(p.entries.contains("bin:tool"));
        assert!(p.entries.contains("lib:mylib"));
    }

    #[test]
    fn cargo_malformed_is_none() {
        assert!(parse_cargo("[dependencies\nthis is not = = toml").is_none());
    }

    #[test]
    fn package_json_distinguishes_normal_and_dev_and_entries() {
        let p = parse_package_json(
            r#"{"dependencies":{"react":"^18"},"devDependencies":{"jest":"^29"},"main":"index.js","bin":{"cli":"bin/cli.js"}}"#,
        )
        .unwrap();
        assert_eq!(p.ecosystem, "npm");
        assert_eq!(scopes_of(&p, "react"), vec!["normal"]);
        assert_eq!(scopes_of(&p, "jest"), vec!["dev"]);
        assert!(p.entries.contains("main:index.js"));
        assert!(p.entries.contains("bin:cli=bin/cli.js"));
    }

    #[test]
    fn package_json_string_bin_is_a_single_entry() {
        let p = parse_package_json(r#"{"bin":"./run.js"}"#).unwrap();
        assert!(p.entries.contains("bin:./run.js"));
    }

    #[test]
    fn package_json_malformed_is_none() {
        assert!(parse_package_json("{not json,,,").is_none());
    }

    #[test]
    fn go_mod_block_and_single_line_requires_are_deps() {
        let p = parse_go_mod(
            "module github.com/me/proj\n\ngo 1.21\n\nrequire (\n\tgithub.com/pkg/errors v0.9.1\n\tgolang.org/x/sync v0.1.0 // indirect\n)\n\nrequire github.com/spf13/cobra v1.8.0\n",
        )
        .unwrap();
        assert_eq!(p.ecosystem, "go");
        assert!(p.deps.contains_key("github.com/pkg/errors"));
        assert!(p.deps.contains_key("golang.org/x/sync"));
        assert!(p.deps.contains_key("github.com/spf13/cobra"));
        // The module's own path and the go-version line are NOT dependencies.
        assert!(!p.deps.contains_key("github.com/me/proj"));
        assert!(!p.deps.contains_key("1.21"));
        assert_eq!(scopes_of(&p, "github.com/pkg/errors"), vec!["normal"]);
    }

    #[test]
    fn csproj_package_references_and_output_type() {
        let p = parse_csproj(
            "<Project Sdk=\"Microsoft.NET.Sdk\">\n  <PropertyGroup>\n    <OutputType>Exe</OutputType>\n  </PropertyGroup>\n  <ItemGroup>\n    <PackageReference Include=\"Newtonsoft.Json\" Version=\"13.0.3\" />\n    <PackageReference Include=\"Serilog\" Version=\"3.0.0\" />\n  </ItemGroup>\n</Project>\n",
        )
        .unwrap();
        assert_eq!(p.ecosystem, "nuget");
        assert!(p.deps.contains_key("Newtonsoft.Json"));
        assert!(p.deps.contains_key("Serilog"));
        assert!(p.entries.contains("output_type:Exe"));
    }

    #[test]
    fn csproj_start_form_package_reference_is_also_a_dep() {
        // A `<PackageReference>…</PackageReference>` with a child (Start form,
        // not self-closing) must still be captured.
        let p = parse_csproj(
            "<Project>\n  <ItemGroup>\n    <PackageReference Include=\"Dapper\">\n      <Version>2.0.0</Version>\n    </PackageReference>\n  </ItemGroup>\n</Project>\n",
        )
        .unwrap();
        assert!(p.deps.contains_key("Dapper"));
    }
}
