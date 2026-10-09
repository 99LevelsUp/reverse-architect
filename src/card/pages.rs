//! Architecture pages: the corpus-wide documents a reader opens first.
//!
//! Views ([`crate::taxonomy`]) answer one question each about the file tree.
//! These pages answer the questions a hand-written architecture document
//! answers — how the code is layered, which modules depend on which, where the
//! weight sits, which contracts the code is extended through — and every name
//! on them links to its card.
//!
//! Everything is computed from [`Insight`], the same aggregation the cards
//! show, so a number on a page and the same number on a card cannot disagree.
//! Layers and cycles are derived from dependency direction alone and say so;
//! nothing here is named or grouped by guesswork.

use super::explain::{grouped, module_label};
use super::insight::{Counts, Insight};
use super::links::{mention, CardIndex};
use super::{Card, CardKind};
use crate::cli::LinkStyle;
use crate::render::RenderedFile;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

/// The page file names, in the order the index lists them.
pub const PAGE_FILENAMES: [&str; 4] = [
    "architecture.md",
    "hotspots.md",
    "hierarchy.md",
    "integrations.md",
];

/// Most modules the architecture diagram draws at one grain.
const MAP_MODULES: usize = 24;
/// Most edges the architecture diagram draws.
const MAP_EDGES: usize = 60;
/// Rows per ranked table.
const TOP: usize = 15;

struct Ctx<'a> {
    insight: &'a Insight,
    index: &'a CardIndex,
    links: LinkStyle,
    from: &'static str,
}

impl Ctx<'_> {
    fn link(&self, identity: &str, label: &str) -> String {
        mention(self.from, identity, label, self.links, self.index)
    }

    fn unit(&self, unit: &str) -> String {
        self.link(unit, self.insight.label(unit))
    }

    fn module(&self, module: &str) -> String {
        self.link(module, module_label(module))
    }
}

fn cell(text: &str) -> String {
    text.replace('|', "\\|").replace(['\n', '\r'], " ")
}

/// Every page, ready to write beside the views.
pub fn build_pages(
    insight: &Insight,
    cards: &[Card],
    index: &CardIndex,
    links: LinkStyle,
) -> Vec<RenderedFile> {
    let cx = |from: &'static str| Ctx {
        insight,
        index,
        links,
        from,
    };
    vec![
        RenderedFile {
            path: "architecture.md".into(),
            contents: architecture(&cx("architecture.md"), cards),
        },
        RenderedFile {
            path: "hotspots.md".into(),
            contents: hotspots(&cx("hotspots.md")),
        },
        RenderedFile {
            path: "hierarchy.md".into(),
            contents: hierarchy(&cx("hierarchy.md")),
        },
        RenderedFile {
            path: "integrations.md".into(),
            contents: integrations(&cx("integrations.md")),
        },
    ]
}

/// A module whose every direct file is a test.
fn is_test_module(insight: &Insight, module: &str) -> bool {
    let files = insight.module_files.get(module);
    files.is_some_and(|f| !f.is_empty() && f.iter().all(|f| insight.is_test_file(f)))
}

/// Modules that hold code: at least one unit with a declared kind, or a
/// counted relation in either direction.
fn code_modules(insight: &Insight) -> BTreeSet<String> {
    let mut set: BTreeSet<String> = BTreeSet::new();
    for (module, units) in &insight.module_units {
        if units.iter().any(|u| insight.kind(u).is_some()) {
            set.insert(module.clone());
        }
    }
    set.extend(insight.module_out.keys().cloned());
    set.extend(insight.module_in.keys().cloned());
    set
}

/// A module key cut to its first `depth` path components.
fn roll(module: &str, depth: usize) -> String {
    if module == "." {
        return ".".into();
    }
    module.split('/').take(depth).collect::<Vec<_>>().join("/")
}

/// The deepest grain at which the code modules still fit one readable
/// diagram.
fn grain(modules: &BTreeSet<String>) -> usize {
    let deepest = modules
        .iter()
        .map(|m| if m == "." { 0 } else { m.split('/').count() })
        .max()
        .unwrap_or(1)
        .max(1);
    for depth in (1..=deepest).rev() {
        let rolled: BTreeSet<String> = modules.iter().map(|m| roll(m, depth)).collect();
        if rolled.len() <= MAP_MODULES {
            return depth;
        }
    }
    1
}

/// Module-to-module counts at a grain.
fn rolled_edges(insight: &Insight, depth: usize) -> BTreeMap<(String, String), Counts> {
    let mut out: BTreeMap<(String, String), Counts> = BTreeMap::new();
    for (from, targets) in &insight.module_out {
        for (to, counts) in targets {
            let (a, b) = (roll(from, depth), roll(to, depth));
            if a == b {
                continue;
            }
            out.entry((a, b)).or_default().merge(counts);
        }
    }
    out
}

/// Strongly connected components of a small directed graph, each sorted, in a
/// deterministic order (Tarjan over nodes in sorted order).
fn components(nodes: &[String], edges: &BTreeMap<String, BTreeSet<String>>) -> Vec<Vec<String>> {
    struct T<'a> {
        edges: &'a BTreeMap<String, BTreeSet<String>>,
        index: BTreeMap<&'a str, usize>,
        low: BTreeMap<&'a str, usize>,
        on: BTreeSet<&'a str>,
        stack: Vec<&'a str>,
        next: usize,
        out: Vec<Vec<String>>,
    }
    fn visit<'a>(t: &mut T<'a>, v: &'a str) {
        t.index.insert(v, t.next);
        t.low.insert(v, t.next);
        t.next += 1;
        t.stack.push(v);
        t.on.insert(v);
        if let Some(targets) = t.edges.get(v) {
            for w in targets {
                let w = w.as_str();
                if !t.index.contains_key(w) {
                    visit(t, w);
                    let lw = t.low[w];
                    let lv = t.low[v];
                    t.low.insert(v, lv.min(lw));
                } else if t.on.contains(w) {
                    let iw = t.index[w];
                    let lv = t.low[v];
                    t.low.insert(v, lv.min(iw));
                }
            }
        }
        if t.low[v] == t.index[v] {
            let mut comp = Vec::new();
            while let Some(w) = t.stack.pop() {
                t.on.remove(w);
                comp.push(w.to_string());
                if w == v {
                    break;
                }
            }
            comp.sort();
            t.out.push(comp);
        }
    }
    let mut t = T {
        edges,
        index: BTreeMap::new(),
        low: BTreeMap::new(),
        on: BTreeSet::new(),
        stack: Vec::new(),
        next: 0,
        out: Vec::new(),
    };
    for n in nodes {
        if !t.index.contains_key(n.as_str()) {
            visit(&mut t, n.as_str());
        }
    }
    let mut out = t.out;
    out.sort();
    out
}

fn architecture(cx: &Ctx, cards: &[Card]) -> String {
    let insight = cx.insight;
    let mut s = String::from("# Architecture\n\n");
    s.push_str(
        "**Guiding question:** How is the code partitioned, and which parts depend on which?\n\n",
    );
    s.push_str(
        "Every relation the extractors read between two declarations — a call, a type used in a \
         signature or a field, a construction, an inheritance, an import — is lifted to the \
         modules the two sides live in and counted. The layers and cycles below are derived \
         from that direction alone: nothing is grouped by name or guessed from convention. \
         A relation resting only on an ambiguous name match is left out of the map, the \
         layers and the table below; the cards list it under Maybe also.\n",
    );

    let files = cards.iter().filter(|c| c.kind == CardKind::File).count();
    let types = cards.iter().filter(|c| c.kind == CardKind::Type).count();
    let modules = code_modules(insight);
    let total_loc = insight.module_loc(".");
    let _ = write!(s, "\n## At a glance\n\n");
    let all_modules = insight.module_files.len();
    let _ = writeln!(
        s,
        "- {} files in {} directories; {} of them hold code the extractors could read",
        grouped(files as i64),
        all_modules,
        modules.len()
    );
    let _ = writeln!(s, "- {} declared types", grouped(types as i64));
    if let Some(loc) = total_loc {
        let _ = writeln!(s, "- {} lines", grouped(loc));
    }

    if modules.is_empty() {
        s.push_str("\nNo module holds declarations the extractors could read, so there is no dependency structure to show.\n");
        return s;
    }

    let depth = grain(&modules);
    let rolled: BTreeSet<String> = modules.iter().map(|m| roll(m, depth)).collect();
    let edges = rolled_edges(insight, depth);

    // Diagram. At most MAP_MODULES modules are drawn — the most connected
    // ones — so a solution with eighty projects still yields a readable map;
    // the table below lists every one of them.
    let mut weight: BTreeMap<&str, u32> = BTreeMap::new();
    for ((a, b), c) in &edges {
        *weight.entry(a.as_str()).or_default() += c.undisputed();
        *weight.entry(b.as_str()).or_default() += c.undisputed();
    }
    let mut by_weight: Vec<&str> = rolled.iter().map(String::as_str).collect();
    by_weight.sort_by(|a, b| {
        weight
            .get(b)
            .copied()
            .unwrap_or(0)
            .cmp(&weight.get(a).copied().unwrap_or(0))
            .then(
                insight
                    .module_loc(b)
                    .unwrap_or(0)
                    .cmp(&insight.module_loc(a).unwrap_or(0)),
            )
            .then(a.cmp(b))
    });
    let drawn: BTreeSet<&str> = by_weight.iter().take(MAP_MODULES).copied().collect();
    let mut ranked: Vec<(&(String, String), &Counts)> = edges
        .iter()
        .filter(|((a, b), c)| {
            c.undisputed() > 0 && drawn.contains(a.as_str()) && drawn.contains(b.as_str())
        })
        .collect();
    ranked.sort_by(|a, b| b.1.undisputed().cmp(&a.1.undisputed()).then(a.0.cmp(b.0)));
    let shown_edges: Vec<(&(String, String), &Counts)> =
        ranked.into_iter().take(MAP_EDGES).collect();
    let id: BTreeMap<&str, usize> = drawn.iter().enumerate().map(|(i, m)| (*m, i)).collect();
    let esc = crate::render::mermaid::escape_label;
    let _ = write!(
        s,
        "\n## Module map\n\nModules cut to {} path level{}; an arrow points from the module that \
         depends to the module it depends on, labelled with the number of relations.\n\n```mermaid\nflowchart LR\n",
        depth,
        if depth == 1 { "" } else { "s" }
    );
    // Grouping: by top-level directory when the grain is deeper than one
    // level, and — at one level, where a .NET solution puts every project side
    // by side — by the first two segments of a dotted project name.
    let group_of = |m: &str| -> (String, String) {
        if m == "." {
            return (".".into(), module_label(m).into());
        }
        if depth > 1 {
            let top = roll(m, 1);
            let label = m.strip_prefix(&format!("{top}/")).unwrap_or(m).to_string();
            return (top, label);
        }
        let parts: Vec<&str> = m.split('.').collect();
        if parts.len() >= 3 {
            let prefix = parts[..2].join(".");
            let label = parts[2..].join(".");
            return (prefix, label);
        }
        (m.to_string(), m.to_string())
    };
    let mut groups: BTreeMap<String, Vec<(&str, String)>> = BTreeMap::new();
    for m in &drawn {
        let (group, label) = group_of(m);
        groups.entry(group).or_default().push((m, label));
    }
    for (group, members) in &groups {
        let nest = members.len() > 1;
        if nest {
            let _ = writeln!(
                s,
                "    subgraph g{}[\"{}\"]",
                id[members[0].0],
                esc(module_label(group))
            );
            for (m, label) in members {
                let _ = writeln!(s, "    m{}[\"{}\"]", id[m], esc(label));
            }
            s.push_str("    end\n");
        } else {
            for (m, _) in members {
                let _ = writeln!(s, "    m{}[\"{}\"]", id[m], esc(module_label(m)));
            }
        }
    }
    for ((a, b), c) in &shown_edges {
        let _ = writeln!(
            s,
            "    m{} -->|{}| m{}",
            id[a.as_str()],
            c.undisputed(),
            id[b.as_str()]
        );
    }
    s.push_str("```\n");
    let hidden_modules = rolled.len().saturating_sub(drawn.len());
    let undisputed_edges = edges.values().filter(|c| c.undisputed() > 0).count();
    if hidden_modules > 0 || undisputed_edges > shown_edges.len() {
        let _ = writeln!(
            s,
            "\nThe diagram draws the {} most connected of {} modules and the {} strongest of {} relations between them; the table below lists every module.",
            drawn.len(),
            rolled.len(),
            shown_edges.len(),
            undisputed_edges
        );
    }

    // Layers over the condensation of the undisputed-edge graph.
    let node_list: Vec<String> = rolled.iter().cloned().collect();
    let mut adj: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for ((a, b), c) in &edges {
        if c.undisputed() > 0 && rolled.contains(a) && rolled.contains(b) {
            adj.entry(a.clone()).or_default().insert(b.clone());
        }
    }
    let comps = components(&node_list, &adj);
    let comp_of: BTreeMap<&str, usize> = comps
        .iter()
        .enumerate()
        .flat_map(|(i, c)| c.iter().map(move |m| (m.as_str(), i)))
        .collect();
    let mut layer: BTreeMap<usize, usize> = BTreeMap::new();
    fn depth_of(
        c: usize,
        comps: &[Vec<String>],
        adj: &BTreeMap<String, BTreeSet<String>>,
        comp_of: &BTreeMap<&str, usize>,
        memo: &mut BTreeMap<usize, usize>,
    ) -> usize {
        if let Some(d) = memo.get(&c) {
            return *d;
        }
        memo.insert(c, 0);
        let mut best = 0;
        for m in &comps[c] {
            for t in adj.get(m).into_iter().flatten() {
                let tc = comp_of[t.as_str()];
                if tc != c {
                    best = best.max(1 + depth_of(tc, comps, adj, comp_of, memo));
                }
            }
        }
        memo.insert(c, best);
        best
    }
    for c in 0..comps.len() {
        depth_of(c, &comps, &adj, &comp_of, &mut layer);
    }
    let mut by_layer: BTreeMap<usize, Vec<&str>> = BTreeMap::new();
    for (i, comp) in comps.iter().enumerate() {
        for m in comp {
            by_layer.entry(layer[&i]).or_default().push(m);
        }
    }
    s.push_str("\n## Layers\n\nDerived from dependency direction: layer 0 depends on no other module shown here, and every module sits one layer above the highest layer it depends on. Test modules are marked; they normally sit on top.\n\n| Layer | Modules |\n|---|---|\n");
    for (l, members) in by_layer.iter().rev() {
        let names: Vec<String> = members
            .iter()
            .map(|m| {
                let mut n = cx.module(m);
                if is_test_module(insight, m) {
                    n.push_str(" (tests)");
                }
                n
            })
            .collect();
        let _ = writeln!(s, "| {l} | {} |", names.join(", "));
    }

    let cycles: Vec<&Vec<String>> = comps.iter().filter(|c| c.len() > 1).collect();
    s.push_str("\n## Dependency cycles\n\n");
    if cycles.is_empty() {
        s.push_str("No cycle between the modules shown was seen: the certain relations form a layered order.\n");
    } else {
        let _ = writeln!(
            s,
            "{} group{} of modules depend on each other in a circle. A cycle means none of them can be understood, tested or released without the others:\n",
            cycles.len(),
            if cycles.len() == 1 { "" } else { "s" }
        );
        for c in cycles {
            let names: Vec<String> = c.iter().map(|m| cx.module(m)).collect();
            let _ = writeln!(s, "- {}", names.join(" ⇄ "));
        }
    }

    // Module table at the diagram's grain: each row sums the directories cut
    // to it, so the table and the map describe the same modules. Every module
    // card lists its own sub-modules below that.
    let under = |prefix: &str, m: &str| {
        prefix == "." || m == prefix || m.starts_with(&format!("{prefix}/"))
    };
    let mut out_deg: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    let mut in_deg: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    // The same undisputed cut as the map: a module reached only through
    // ambiguous name matches is not one it depends on (plan A, R7).
    for (a, b) in edges
        .iter()
        .filter(|(_, c)| c.undisputed() > 0)
        .map(|(k, _)| k)
    {
        out_deg.entry(a.as_str()).or_default().insert(b.as_str());
        in_deg.entry(b.as_str()).or_default().insert(a.as_str());
    }
    let _ = write!(
        s,
        "\n## Modules\n\nAt the same grain as the map ({} path level{}); files, lines and types count everything beneath each one.\n\n| Module | Files | Lines | Types | Depends on | Used by | Summary |\n|---|---|---|---|---|---|---|\n",
        depth,
        if depth == 1 { "" } else { "s" }
    );
    for r in &rolled {
        let dirs: Vec<&String> = insight
            .module_files
            .keys()
            .filter(|m| if r == "." { *m == "." } else { under(r, m) })
            .collect();
        let files: usize = dirs
            .iter()
            .filter_map(|m| insight.module_files.get(*m))
            .map(BTreeSet::len)
            .sum();
        let types: usize = dirs
            .iter()
            .filter_map(|m| insight.module_units.get(*m))
            .map(|u| u.iter().filter(|u| insight.kind(u).is_some()).count())
            .sum();
        let all_tests = !dirs.is_empty()
            && dirs.iter().all(|m| {
                insight
                    .module_files
                    .get(*m)
                    .is_none_or(|f| f.iter().all(|f| insight.is_test_file(f)))
            });
        let lines = if r == "." {
            insight.module_own_loc(r)
        } else {
            insight.module_loc(r)
        };
        let mut name = cx.module(r);
        if all_tests {
            name.push_str(" (tests)");
        }
        let _ = writeln!(
            s,
            "| {} | {} | {} | {} | {} | {} | {} |",
            name,
            files,
            lines.map(grouped).unwrap_or_default(),
            types,
            out_deg.get(r.as_str()).map_or(0, BTreeSet::len),
            in_deg.get(r.as_str()).map_or(0, BTreeSet::len),
            cell(insight.summary(r).unwrap_or(""))
        );
    }

    // Extension points: contracts the code is extended through.
    let mut points: Vec<(&str, Vec<&str>)> = insight
        .type_units()
        .filter_map(|t| {
            let derived = insight.derived_of(t);
            (derived.len() >= 2).then_some((t, derived))
        })
        .collect();
    points.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(b.0)));
    if !points.is_empty() {
        s.push_str("\n## Extension points\n\nTypes the code is extended through: each has at least two types in this repository that extend or implement it.\n\n| Contract | Kind | Module | Implementations | Where they live |\n|---|---|---|---|---|\n");
        for (t, derived) in points.iter().take(40) {
            let names: Vec<String> = derived.iter().take(6).map(|d| cx.unit(d)).collect();
            let rest = derived.len().saturating_sub(6);
            let home = insight.module_of(t).unwrap_or("");
            let elsewhere = derived
                .iter()
                .filter(|d| insight.module_of(d) != Some(home))
                .count();
            let whereabouts = if elsewhere == 0 {
                "all in its own module".to_string()
            } else if elsewhere == derived.len() {
                "all outside its module".to_string()
            } else {
                format!("{elsewhere} of {} outside its module", derived.len())
            };
            let _ = writeln!(
                s,
                "| {} | {} | {} | {}{} | {} |",
                cx.unit(t),
                insight.kind(t).unwrap_or(""),
                cx.module(home),
                names.join(", "),
                if rest > 0 {
                    format!(" and {rest} more")
                } else {
                    String::new()
                },
                whereabouts
            );
        }
        if points.len() > 40 {
            let _ = writeln!(
                s,
                "\n{} more are listed in the type hierarchy page.",
                points.len() - 40
            );
        }
    }
    s
}

fn hotspots(cx: &Ctx) -> String {
    let insight = cx.insight;
    let mut s = String::from("# Hotspots\n\n**Guiding question:** Where does the weight of this code sit?\n\nThe largest and most connected parts of the code — where a change travels furthest and where a newcomer should read first. Only files the extractors read declarations from count as code here; generated files are left out of every list.\n");

    // Largest files.
    let mut files: Vec<(&str, i64)> = insight
        .module_files
        .values()
        .flatten()
        .filter(|f| !is_generated_path(f) && insight.declares_code(f))
        .filter_map(|f| insight.file_loc(f).map(|l| (f.as_str(), l)))
        .collect();
    files.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    if !files.is_empty() {
        s.push_str("\n## Largest source files\n\n| File | Lines |\n|---|---|\n");
        for (f, l) in files.iter().take(TOP) {
            let test = if insight.is_test_file(f) {
                " (test)"
            } else {
                ""
            };
            let _ = writeln!(s, "| {}{test} | {} |", cx.link(f, f), grouped(*l));
        }
    }

    let types: Vec<&str> = insight
        .type_units()
        .filter(|t| !insight.is_test_unit(t))
        .filter(|t| !insight.file_of(t).is_some_and(is_generated_path))
        .collect();

    let mut largest: Vec<(&str, u32)> = types
        .iter()
        .filter_map(|t| insight.type_lines(t).map(|l| (*t, l)))
        .collect();
    largest.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    if !largest.is_empty() {
        s.push_str("\n## Largest types\n\n| Type | Kind | Module | Lines |\n|---|---|---|---|\n");
        for (t, l) in largest.iter().take(TOP) {
            let _ = writeln!(
                s,
                "| {} | {} | {} | {} |",
                cx.unit(t),
                insight.kind(t).unwrap_or(""),
                cx.module(insight.module_of(t).unwrap_or(".")),
                grouped(i64::from(*l))
            );
        }
    }

    let undisputed =
        |list: Vec<(&str, Counts)>| list.iter().filter(|(_, c)| c.undisputed() > 0).count();
    let mut used: Vec<(&str, usize)> = types
        .iter()
        .map(|t| (*t, undisputed(insight.users(t))))
        .collect();
    used.retain(|(_, n)| *n > 0);
    used.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    if !used.is_empty() {
        s.push_str("\n## Most used types\n\nTypes the most other types and files reach, on certain relations. A change to one of these travels furthest.\n\n| Type | Kind | Module | Used by | Summary |\n|---|---|---|---|---|\n");
        for (t, n) in used.iter().take(TOP) {
            let _ = writeln!(
                s,
                "| {} | {} | {} | {} | {} |",
                cx.unit(t),
                insight.kind(t).unwrap_or(""),
                cx.module(insight.module_of(t).unwrap_or(".")),
                n,
                cell(insight.summary(t).unwrap_or(""))
            );
        }
    }

    let mut reaching: Vec<(&str, usize)> = types
        .iter()
        .map(|t| (*t, undisputed(insight.uses(t))))
        .collect();
    reaching.retain(|(_, n)| *n > 0);
    reaching.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    if !reaching.is_empty() {
        s.push_str("\n## Most dependent types\n\nTypes that reach the most others. High numbers mark coordinators — or classes doing too much.\n\n| Type | Kind | Module | Works with |\n|---|---|---|---|\n");
        for (t, n) in reaching.iter().take(TOP) {
            let _ = writeln!(
                s,
                "| {} | {} | {} | {} |",
                cx.unit(t),
                insight.kind(t).unwrap_or(""),
                cx.module(insight.module_of(t).unwrap_or(".")),
                n
            );
        }
    }

    let undocumented = types
        .iter()
        .filter(|t| insight.summary(t).is_none())
        .count();
    if !types.is_empty() {
        let _ = write!(
            s,
            "\n## Documentation\n\n{} of {} types outside tests carry a doc comment the cards can quote as their summary; {} carry none.\n",
            types.len() - undocumented,
            types.len(),
            undocumented
        );
    }

    let generated: Vec<&str> = insight
        .module_files
        .values()
        .flatten()
        .filter(|f| is_generated_path(f))
        .map(String::as_str)
        .collect();
    if !generated.is_empty() {
        let _ = write!(
            s,
            "\n## Generated files\n\n{} files look generated by their name or folder and are left out of the lists above.\n",
            generated.len()
        );
    }
    s
}

fn hierarchy(cx: &Ctx) -> String {
    let insight = cx.insight;
    let mut s = String::from("# Type hierarchy\n\n**Guiding question:** Which types build on which?\n\nEvery `extends` and `implements` relation between two types this repository declares. A base outside the repository — a framework class, a library interface — has no card and is named only in the deriving type's signature.\n");
    let mut roots: Vec<(&str, Vec<&str>)> = insight
        .type_units()
        .filter_map(|t| {
            let d = insight.derived_of(t);
            (!d.is_empty()).then_some((t, d))
        })
        .collect();
    roots.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(b.0)));
    if roots.is_empty() {
        s.push_str("\nNo type in this repository was seen extending or implementing another.\n");
        return s;
    }
    let esc = crate::render::mermaid::escape_label;
    for (base, derived) in roots.iter().take(60) {
        let _ = write!(s, "\n## {}\n\n", insight.label(base));
        let _ = writeln!(
            s,
            "{} {} in {} — {} {}.",
            cx.unit(base),
            insight.kind(base).unwrap_or("type"),
            cx.module(insight.module_of(base).unwrap_or(".")),
            derived.len(),
            if derived.len() == 1 {
                "type builds on it"
            } else {
                "types build on it"
            }
        );
        if let Some(summary) = insight.summary(base) {
            let _ = writeln!(s, "\n> {summary}");
        }
        s.push_str("\n```mermaid\nflowchart BT\n");
        let _ = writeln!(s, "    b[\"{}\"]", esc(insight.label(base)));
        for (i, d) in derived.iter().take(16).enumerate() {
            let _ = writeln!(s, "    d{i}[\"{}\"] --> b", esc(insight.label(d)));
        }
        s.push_str("```\n\n");
        for d in derived.iter().take(40) {
            let summary = insight
                .summary(d)
                .map(|x| format!(" — {x}"))
                .unwrap_or_default();
            let _ = writeln!(s, "- {}{}", cx.unit(d), summary);
        }
        if derived.len() > 40 {
            let _ = writeln!(
                s,
                "\n{} more are listed on the card of {}.",
                derived.len() - 40,
                cx.unit(base)
            );
        }
    }
    if roots.len() > 60 {
        let _ = writeln!(
            s,
            "\n{} more base types are not drawn here; each one's card lists what builds on it.",
            roots.len() - 60
        );
    }
    s
}

fn integrations(cx: &Ctx) -> String {
    let insight = cx.insight;
    let mut s = String::from("# Integrations\n\n**Guiding question:** What does this code reach outside itself?\n\nForeign library symbols matched against declared dependencies, database objects named in SQL, absolute URLs, and configuration keys read through a named API — each recognised by an unambiguous pattern, never guessed. Secrets are never carried.\n");
    // External → users, by kind.
    let mut by_kind: BTreeMap<String, BTreeMap<String, BTreeSet<String>>> = BTreeMap::new();
    for (unit, externals) in &insight.unit_externals {
        for ext in externals.keys() {
            let kind = insight
                .external(ext)
                .and_then(|(_, k)| k)
                .unwrap_or("external")
                .to_string();
            by_kind
                .entry(kind)
                .or_default()
                .entry(ext.clone())
                .or_default()
                .insert(unit.clone());
        }
    }
    if by_kind.is_empty() {
        s.push_str("\nNothing outside the repository was recognised. That is a statement about the patterns, not a proof the code reaches nothing.\n");
        return s;
    }
    for (kind, externals) in &by_kind {
        let _ = write!(
            s,
            "\n## {kind}\n\n| External | Used by | Modules |\n|---|---|---|\n"
        );
        let mut rows: Vec<(&String, &BTreeSet<String>)> = externals.iter().collect();
        rows.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(b.0)));
        for (ext, users) in rows.iter().take(50) {
            let label = insight.external(ext).map(|(l, _)| l).unwrap_or(ext);
            let named: Vec<String> = users.iter().take(4).map(|u| cx.unit(u)).collect();
            let more = users.len().saturating_sub(4);
            let modules: BTreeSet<&str> =
                users.iter().filter_map(|u| insight.module_of(u)).collect();
            let _ = writeln!(
                s,
                "| `{}` | {}{} | {} |",
                cell(label),
                named.join(", "),
                if more > 0 {
                    format!(" and {more} more")
                } else {
                    String::new()
                },
                modules.len()
            );
        }
        if rows.len() > 50 {
            let _ = writeln!(
                s,
                "\n{} more are named on the cards that use them.",
                rows.len() - 50
            );
        }
    }
    s
}

/// Whether a path is generated code by the conventions of the common
/// toolchains: designer and generator suffixes, protobuf output, build
/// output and vendored folders.
pub fn is_generated_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let base = lower.rsplit('/').next().unwrap_or(&lower);
    let dirs: Vec<&str> = lower.split('/').collect();
    dirs[..dirs.len().saturating_sub(1)].iter().any(|d| {
        matches!(
            *d,
            "node_modules" | "dist" | "obj" | "bin" | "generated" | "vendor" | "third_party"
        )
    }) || base.ends_with(".designer.cs")
        || base.ends_with(".g.cs")
        || base.ends_with(".g.i.cs")
        || base.contains(".generated.")
        || base.ends_with("_pb2.py")
        || base.ends_with(".pb.go")
        || base.ends_with(".min.js")
        || base == "reference.cs"
        || base.ends_with(".d.ts") && dirs.contains(&"types")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_grain_is_the_deepest_that_still_fits() {
        let modules: BTreeSet<String> = ["src/a/x", "src/a/y", "src/b"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(grain(&modules), 3);
        assert_eq!(roll("src/a/x", 2), "src/a");
        assert_eq!(roll(".", 2), ".");
    }

    #[test]
    fn a_two_module_cycle_is_one_component() {
        let nodes = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let mut adj: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        adj.entry("a".into()).or_default().insert("b".into());
        adj.entry("b".into()).or_default().insert("a".into());
        adj.entry("b".into()).or_default().insert("c".into());
        let comps = components(&nodes, &adj);
        assert!(comps.contains(&vec!["a".to_string(), "b".to_string()]));
        assert!(comps.contains(&vec!["c".to_string()]));
    }

    #[test]
    fn generated_paths_are_recognised() {
        for p in [
            "Forms/Main.Designer.cs",
            "api/x_pb2.py",
            "web/dist/app.js",
            "Svc/Reference.cs",
        ] {
            assert!(is_generated_path(p), "{p}");
        }
        assert!(!is_generated_path("src/runtime/queue.ts"));
    }
}
