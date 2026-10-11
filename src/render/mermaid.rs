//! Deterministic mermaid emission (U11, KTD-C3).
//!
//! A diagram is built from a set of graph edges by **construction**, never by an
//! LLM validate-and-retry loop (R8). Two properties make the output always valid
//! and always stable:
//!
//! * **Injective node ids.** Each node is given a positional id (`n0`, `n1`, …)
//!   from its index in the sorted node set of the sub-diagram. Sanitizing a key
//!   would be many-to-one (`a/b`, `a.b`, `a-b` all collapse to `a_b`, and a dir
//!   named `end/` collapses to the mermaid reserved word `end`); a positional id
//!   is injective by construction and never a reserved word. The human-readable
//!   key lives only in the quoted label.
//! * **Escaped labels.** Labels are wrapped in `"…"` with every metacharacter
//!   neutralised to an HTML entity, so no raw `"`, `<`, `>` (hence no raw `-->`),
//!   backslash, or newline can leak into a label and break the block.
//!
//! When a diagram would exceed [`SPLIT_THRESHOLD`] nodes it is auto-split along a
//! structural seam (the top-level path component of each edge's source), and if a
//! single seam segment still exceeds the threshold the split recurses into deeper
//! path components, falling back to a fixed-size partition when a segment can no
//! longer be subdivided — so *every* emitted sub-diagram is ≤ threshold.

use super::split::ChunkWeight;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

/// The node-count ceiling for a single mermaid block (a tunable seed, KTD-C3).
/// A diagram with more distinct nodes than this is auto-split so every emitted
/// block stays at or below it.
pub const SPLIT_THRESHOLD: usize = 30;

/// Build the `## Diagram` body (one or more fenced mermaid blocks) from a set of
/// edges given as `(from_key, to_key)` pairs. Returns `None` when there are no
/// edges to draw, so the caller emits no empty section. Deterministic: nodes and
/// edges are emitted in sorted order and sub-diagrams appear in seam order.
pub fn diagram(edges: &[(&str, &str)]) -> Option<String> {
    if edges.is_empty() {
        return None;
    }
    let blocks: Vec<String> = split(edges).iter().map(|c| render_block(c)).collect();
    // A blank line between blocks — matches the leaf's section spacing and keeps
    // adjacent fences apart for stricter Markdown parsers.
    Some(blocks.join("\n\n"))
}

/// The distinct nodes touched by a set of edges.
fn distinct_count(edges: &[(&str, &str)]) -> usize {
    edges
        .iter()
        .flat_map(|&(f, t)| [f, t])
        .collect::<BTreeSet<&str>>()
        .len()
}

/// The first `depth` `/`-separated components of a key, its structural seam.
fn prefix_key(key: &str, depth: usize) -> String {
    key.split('/').take(depth).collect::<Vec<_>>().join("/")
}

/// Split edges into sub-diagrams that each hold ≤ [`SPLIT_THRESHOLD`] nodes.
fn split<'a>(edges: &[(&'a str, &'a str)]) -> Vec<Vec<(&'a str, &'a str)>> {
    if distinct_count(edges) <= SPLIT_THRESHOLD {
        return vec![edges.to_vec()];
    }
    split_by_prefix(edges, 1)
}

/// Group edges by the `depth`-component seam of their source, recursing into a
/// deeper seam for any segment that still exceeds the threshold and falling back
/// to a fixed-size partition once a segment can no longer be subdivided.
fn split_by_prefix<'a>(edges: &[(&'a str, &'a str)], depth: usize) -> Vec<Vec<(&'a str, &'a str)>> {
    let mut groups: BTreeMap<String, Vec<(&'a str, &'a str)>> = BTreeMap::new();
    for &(f, t) in edges {
        groups.entry(prefix_key(f, depth)).or_default().push((f, t));
    }

    // A single group means the seam no longer subdivides this segment — the
    // structural recursion is exhausted, so fall back to a fixed-size partition.
    if groups.len() <= 1 {
        return fixed_partition(edges);
    }

    let mut out = Vec::new();
    for (_, g) in groups {
        if distinct_count(&g) <= SPLIT_THRESHOLD {
            out.push(g);
        } else {
            out.extend(split_by_prefix(&g, depth + 1));
        }
    }
    out
}

/// The diagram's chunk weight: how many distinct nodes a chunk's edges touch —
/// [`distinct_count`] carried incrementally, so an edge whose endpoints are both
/// already in the chunk adds nothing to it.
struct DistinctNodes<'a> {
    seen: BTreeSet<&'a str>,
}

impl<'a> ChunkWeight<(&'a str, &'a str)> for DistinctNodes<'a> {
    fn peek(&self, &(f, t): &(&'a str, &'a str)) -> usize {
        self.seen.len() + [f, t].iter().filter(|n| !self.seen.contains(**n)).count()
    }

    fn add(&mut self, &(f, t): &(&'a str, &'a str)) {
        self.seen.insert(f);
        self.seen.insert(t);
    }

    fn reset(&mut self) {
        self.seen.clear();
    }
}

/// Greedily pack edges into chunks, opening a new chunk whenever the next edge
/// would push the distinct-node count over the threshold. Guarantees each chunk
/// is ≤ threshold as long as the threshold admits a single edge (two nodes).
///
/// The greedy pass itself lives in [`crate::render::split`], shared with the
/// prose module tree (U1/R2); what stays here is the diagram's own weight and
/// threshold.
fn fixed_partition<'a>(edges: &[(&'a str, &'a str)]) -> Vec<Vec<(&'a str, &'a str)>> {
    const { assert!(SPLIT_THRESHOLD >= 2, "a single edge contributes two nodes") };
    super::split::fixed_partition(
        edges.iter().copied(),
        SPLIT_THRESHOLD,
        &mut DistinctNodes {
            seen: BTreeSet::new(),
        },
    )
}

/// Render one sub-diagram as a fenced mermaid `flowchart` block. Node ids are
/// positional over the sorted node set (injective); edges are sorted and deduped.
fn render_block(edges: &[(&str, &str)]) -> String {
    let nodes: Vec<&str> = edges
        .iter()
        .flat_map(|&(f, t)| [f, t])
        .collect::<BTreeSet<&str>>()
        .into_iter()
        .collect();
    let id_of: BTreeMap<&str, usize> = nodes.iter().enumerate().map(|(i, &n)| (n, i)).collect();

    let mut s = String::from("```mermaid\nflowchart TD\n");
    for (i, &n) in nodes.iter().enumerate() {
        let _ = writeln!(s, "    n{i}[\"{}\"]", escape_label(n));
    }

    let mut resolved: Vec<(usize, usize)> =
        edges.iter().map(|&(f, t)| (id_of[f], id_of[t])).collect();
    resolved.sort_unstable();
    resolved.dedup();
    for (a, b) in resolved {
        let _ = writeln!(s, "    n{a} --> n{b}");
    }
    s.push_str("```");
    s
}

/// One fenced `sequenceDiagram` block (plan 2b, U6: KTD8): the participants
/// in the order given, each under a positional id (`p0`, `p1`, …) with its
/// name as the alias, and the messages `(from, to, text)` between them in
/// order. The caller caps how many it draws. Every name and text goes through
/// [`escape_message`].
pub(crate) fn sequence_block(
    participants: &[String],
    messages: &[(usize, usize, String)],
) -> String {
    let mut s = String::from("```mermaid\nsequenceDiagram\n");
    for (i, p) in participants.iter().enumerate() {
        let _ = writeln!(s, "    participant p{i} as {}", escape_message(p));
    }
    for (from, to, text) in messages {
        let _ = writeln!(s, "    p{from}->>p{to}: {}", escape_message(text));
    }
    s.push_str("```\n");
    s
}

/// [`escape_label`] for the text of a sequence diagram, which is not quoted:
/// a `;` there ends the statement. An HTML entity (`&lt;`) ends in `;` too and
/// would cut the text in two, so every metacharacter becomes Mermaid's own
/// entity (`#lt;`, `#59;`): Mermaid swaps those for placeholders before it
/// parses and turns them back into HTML entities in the drawing. One pass over
/// the characters, so no entity this introduces is escaped again.
pub(crate) fn escape_message(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("#amp;"),
            '#' => out.push_str("#35;"),
            '\\' => out.push_str("#92;"),
            '"' => out.push_str("#quot;"),
            '<' => out.push_str("#lt;"),
            '>' => out.push_str("#gt;"),
            ';' => out.push_str("#59;"),
            '\n' | '\r' => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

/// Neutralise every mermaid label metacharacter to an HTML entity. `&` is
/// escaped first so the entities we introduce are not re-escaped; `#` follows it
/// (never before — that would double-escape the `&` in `&#35;`) so a key
/// containing `#NN;` cannot be decoded as a mermaid numeric entity; `<`/`>` become
/// entities so a key like `a-->b` can never surface a raw `-->` inside a label;
/// newlines collapse to spaces. The result is safe inside a `"…"` label.
pub(crate) fn escape_label(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('#', "&#35;")
        .replace('\\', "&#92;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace(['\n', '\r'], " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The count of a `char` across a string — used to assert balanced quotes.
    fn count(s: &str, c: char) -> usize {
        s.chars().filter(|&x| x == c).count()
    }

    #[test]
    fn empty_edge_set_yields_no_diagram() {
        assert_eq!(diagram(&[]), None);
    }

    #[test]
    fn labels_with_metacharacters_stay_structurally_valid() {
        // A key carrying `:`, quotes, `-->`, a backslash and a newline.
        let out = diagram(&[("weird:\"k\"-->x", "b\\c\nd")]).unwrap();

        // Quotes are balanced: exactly two per node declaration, none leaked from
        // a label (the inner `"` became `&quot;`).
        assert_eq!(count(&out, '"') % 2, 0, "balanced quotes: {out}");

        // No raw `-->` inside a label — labels have no `>` at all, so the only
        // `-->` is the edge connector on its own line.
        for line in out.lines() {
            if let Some(label) = line.split_once('[').and_then(|(_, r)| r.strip_suffix("]")) {
                assert!(!label.contains("-->"), "raw arrow in label: {line}");
                assert!(!label.contains('>'), "raw `>` in label: {line}");
            }
        }
        assert!(out.contains("&quot;"), "quote escaped to entity: {out}");
        assert!(out.contains("--&gt;"), "arrow neutralised in label: {out}");
    }

    #[test]
    fn hash_in_a_label_is_escaped_to_an_entity() {
        // A key containing `#NN;` must not be decodable as a mermaid numeric
        // entity in the label; `#` becomes `&#35;` (and `&` stays single-escaped).
        let out = diagram(&[("sym:a.rs#35;", "b")]).unwrap();
        for line in out.lines() {
            if let Some(label) = line.split_once('[').and_then(|(_, r)| r.strip_suffix("]")) {
                assert!(!label.contains("\"#"), "raw # left in label: {line}");
                assert!(
                    !label.contains("#35;") || label.contains("&#35;"),
                    "bare #35; not neutralised: {line}"
                );
            }
        }
        assert!(out.contains("&#35;"), "# escaped to entity: {out}");
        // The `&` we introduced is not itself re-escaped to `&amp;#35;`.
        assert!(
            !out.contains("&amp;#35;"),
            "# entity got double-escaped: {out}"
        );
    }

    #[test]
    fn a_withheld_value_marker_in_a_label_is_drawn_as_it_is() {
        // The marker the secret filter leaves holds no character the escape
        // changes, so a card's diagram shows it verbatim (plan 2a, KTD16.4);
        // the quotes around a compared value are escaped as in any label.
        let marker = "[redacted: token — see the source anchor]";
        assert_eq!(escape_label(marker), marker);

        let label = format!("when token == \"{marker}\"");
        let out = diagram(&[(label.as_str(), "b")]).unwrap();
        assert_eq!(count(&out, '"') % 2, 0, "balanced quotes: {out}");
        assert!(
            out.contains(&format!("when token == &quot;{marker}&quot;")),
            "the marker survives the escape: {out}"
        );
        for line in out.lines() {
            if let Some(label) = line.split_once('[').and_then(|(_, r)| r.strip_suffix("]")) {
                assert!(!label.contains('>'), "raw `>` in label: {line}");
            }
        }
    }

    #[test]
    fn a_sequence_diagram_declares_its_participants_and_escapes_its_texts() {
        let out = sequence_block(
            &["EditorView".to_string(), "A<B>".to_string()],
            &[
                (0, 1, "Run(a;b) -> x #1".to_string()),
                (1, 1, "self".to_string()),
            ],
        );
        assert_eq!(
            out,
            "```mermaid\nsequenceDiagram\n    participant p0 as EditorView\n    \
             participant p1 as A#lt;B#gt;\n    p0->>p1: Run(a#59;b) -#gt; x #35;1\n    \
             p1->>p1: self\n```\n"
        );
    }

    /// What Mermaid does to a sequence diagram's text before it parses: each
    /// `#name;` / `#NN;` entity becomes a placeholder without a `;`.
    fn without_mermaid_entities(line: &str) -> String {
        let mut out = String::new();
        let mut rest = line;
        while let Some(at) = rest.find('#') {
            out.push_str(&rest[..at]);
            let tail = &rest[at + 1..];
            let word = tail
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(tail.len());
            if word > 0 && tail[word..].starts_with(';') {
                out.push_str("ENTITY");
                rest = &tail[word + 1..];
            } else {
                out.push('#');
                rest = tail;
            }
        }
        out.push_str(rest);
        out
    }

    #[test]
    fn no_text_of_a_sequence_diagram_ends_its_statement_early() {
        // A generic type, a condition with `&&`, a quote, a hash and a `;`:
        // every one used to leave an HTML entity whose `;` cut the line.
        let out = sequence_block(
            &["Cache<Key, List<Value>>".to_string(), "Solver".to_string()],
            &[(
                0,
                1,
                "Solve(Func<Proxy>) when a && b; \"x\" #1 \\ n".to_string(),
            )],
        );
        for line in out.lines().skip(2).filter(|l| *l != "```") {
            let plain = without_mermaid_entities(line);
            assert!(!plain.contains(';'), "a `;` ends the statement: {line}");
            for raw in ['<', '"', '&', '\\'] {
                assert!(!plain.contains(raw), "raw {raw:?}: {line}");
            }
            assert!(
                !plain.replacen("->>", "", 1).contains('>'),
                "raw `>`: {line}"
            );
            assert!(!line.contains("&lt;") && !line.contains("&#"), "{line}");
        }
    }

    #[test]
    fn keys_that_sanitize_alike_get_distinct_ids() {
        // `a/b`, `a.b`, `a-b` all collapse under naive sanitisation to `a_b`.
        let out = diagram(&[("a/b", "a.b"), ("a.b", "a-b")]).unwrap();
        // Three distinct positional ids for the three distinct keys.
        assert!(out.contains("n0[\""), "id n0 present: {out}");
        assert!(out.contains("n1[\""), "id n1 present: {out}");
        assert!(out.contains("n2[\""), "id n2 present: {out}");
        assert!(!out.contains("n3[\""), "exactly three nodes: {out}");
    }

    #[test]
    fn double_run_is_byte_identical() {
        let edges = [("src", "src/a.rs"), ("src", "src/b.rs"), (".", "src")];
        assert_eq!(diagram(&edges), diagram(&edges));
    }

    /// Every emitted block declares at most `SPLIT_THRESHOLD` positional ids.
    fn assert_all_blocks_within_threshold(out: &str) {
        let mut max_id = 0usize;
        let mut blocks = 0usize;
        for block in out.split("```mermaid") {
            if block.trim().is_empty() {
                continue;
            }
            blocks += 1;
            let ids = block
                .lines()
                .filter_map(|l| l.trim().strip_prefix('n'))
                .filter_map(|r| r.split_once('[').map(|(id, _)| id))
                .filter_map(|id| id.parse::<usize>().ok())
                .count();
            assert!(
                ids <= SPLIT_THRESHOLD,
                "block exceeds threshold: {ids} nodes"
            );
            max_id = max_id.max(ids);
        }
        assert!(blocks > 1, "expected an auto-split into multiple blocks");
        assert!(max_id > 0);
        // Blocks are separated by a blank line — never two adjacent fences.
        assert!(
            !out.contains("```\n```mermaid"),
            "adjacent fences with no blank line between blocks"
        );
    }

    #[test]
    fn single_oversized_module_falls_back_to_fixed_partition() {
        // One top-level seam holding far more than the threshold: the seam does
        // not subdivide (all sources share prefix `big`), so the fixed-size
        // partition must still keep every block ≤ threshold.
        let keys: Vec<String> = (0..50).map(|i| format!("big/f{i:02}.rs")).collect();
        let edges: Vec<(&str, &str)> = keys.iter().map(|k| ("big", k.as_str())).collect();

        let out = diagram(&edges).unwrap();
        assert_all_blocks_within_threshold(&out);
    }

    /// The 50-edge fallback fixture above, pinned at the byte level. The
    /// structural assertion proves every block is within the threshold; this
    /// proves the chunker puts the boundaries in exactly the same places — the
    /// acceptance condition for extracting it into [`crate::render::split`].
    #[test]
    fn fixed_partition_fallback_output_is_byte_stable() {
        let keys: Vec<String> = (0..50).map(|i| format!("big/f{i:02}.rs")).collect();
        let edges: Vec<(&str, &str)> = keys.iter().map(|k| ("big", k.as_str())).collect();

        insta::assert_snapshot!(diagram(&edges).unwrap());
    }

    /// The recursive-seam fixture, pinned at the byte level for the same reason.
    #[test]
    fn nested_seam_split_output_is_byte_stable() {
        let a: Vec<String> = (0..20).map(|i| format!("src/a/f{i:02}.rs")).collect();
        let b: Vec<String> = (0..20).map(|i| format!("src/b/f{i:02}.rs")).collect();
        let mut edges: Vec<(&str, &str)> = vec![(".", "src"), ("src", "src/a"), ("src", "src/b")];
        edges.extend(a.iter().map(|k| ("src/a", k.as_str())));
        edges.extend(b.iter().map(|k| ("src/b", k.as_str())));

        insta::assert_snapshot!(diagram(&edges).unwrap());
    }

    #[test]
    fn nested_oversized_module_recurses_into_submodules() {
        // Root `.` plus a `src` seam that alone exceeds the threshold and holds
        // two sub-modules — the depth-1 seam recurses to depth-2 and splits into
        // `src/a` and `src/b` sub-diagrams, each within the threshold.
        let a: Vec<String> = (0..20).map(|i| format!("src/a/f{i:02}.rs")).collect();
        let b: Vec<String> = (0..20).map(|i| format!("src/b/f{i:02}.rs")).collect();
        let mut edges: Vec<(&str, &str)> = vec![(".", "src"), ("src", "src/a"), ("src", "src/b")];
        edges.extend(a.iter().map(|k| ("src/a", k.as_str())));
        edges.extend(b.iter().map(|k| ("src/b", k.as_str())));

        let out = diagram(&edges).unwrap();
        assert_all_blocks_within_threshold(&out);
    }
}
