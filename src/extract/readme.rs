//! The readme extractor: a directory's own statement of what it is for.
//!
//! A module card has no declaration to quote — a directory has no doc comment.
//! What most repositories do have is a README beside the code, and its first
//! paragraph is the author's one-line answer to "what is this folder". That
//! paragraph lands on the directory's Module node as a [`FactKind::DocComment`],
//! anchored to the README's lines, so the module card can quote it the way a
//! type card quotes a doc comment.
//!
//! Read from the git index blob like every other extractor (KTD7), never from
//! the working tree. Only prose is taken: headings, badges, HTML, front matter,
//! tables, lists and code are skipped, and link markup is reduced to its text.

use super::ids;
use super::{parent_key, ExtractError, Extractor, Repo};
use crate::graph::{
    ExtractorId, Fact, FactKind, FactValue, Graph, NodeKey, NodeKind, Provenance, Tier,
};
use std::collections::BTreeMap;

/// Below every code extractor: a README is the author's word about a folder,
/// and a conflicting doc comment on the same node — there is none today —
/// should win.
const TRUST_RANK: u8 = 15;

/// README file names, most preferred first.
const NAMES: [&str; 6] = [
    "readme.md",
    "readme.markdown",
    "readme.rst",
    "readme.txt",
    "readme",
    "index.md",
];

pub struct Readme;

impl Extractor for Readme {
    fn id(&self) -> ExtractorId {
        ids::readme()
    }

    fn emitted_kinds(&self) -> &'static [FactKind] {
        &[FactKind::DocComment, FactKind::AnchorSkipped]
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
        // module → (preference, file, paragraph, first line, last line)
        let mut best: BTreeMap<String, (usize, String, String, u32, u32)> = BTreeMap::new();
        for tf in repo.tracked_files_with_paths()? {
            if tf.is_symlink {
                continue;
            }
            let base = tf
                .key
                .rsplit('/')
                .next()
                .unwrap_or(&tf.key)
                .to_ascii_lowercase();
            let Some(rank) = NAMES.iter().position(|n| *n == base) else {
                continue;
            };
            let module = parent_key(&tf.key);
            if best.get(&module).is_some_and(|(r, ..)| *r <= rank) {
                continue;
            }
            let Ok(bytes) = repo.blob_bytes(&tf) else {
                continue;
            };
            let text = String::from_utf8_lossy(&bytes);
            if let Some((paragraph, start, end)) = first_paragraph(&text) {
                best.insert(module, (rank, tf.key.clone(), paragraph, start, end));
            }
        }
        for (module, (_, file, paragraph, start, end)) in best {
            let key = NodeKey::new(module);
            match graph.node(&key) {
                Some(node) if node.kind == NodeKind::Module => {}
                _ => continue,
            }
            let (text, _) = super::external::redact_secret_block(
                &paragraph,
                super::external::SecretPrefixes::builtin(),
            );
            let mut doc = Fact::new(
                FactKind::DocComment,
                FactValue::Text(text),
                Provenance::new(file, None, ids::readme(), Tier::Measured).with_range(start, end),
            );
            // KTD11 rule 3, as for a declaration's doc comment.
            let skips = super::external::cut_doc_long_tokens(&mut doc);
            graph.add_fact(&key, doc);
            for skip in skips {
                graph.add_fact(&key, skip);
            }
        }
        Ok(())
    }
}

/// The first prose paragraph of a README, with the lines it spans (1-based).
pub(crate) fn first_paragraph(text: &str) -> Option<(String, u32, u32)> {
    let mut lines = text.lines().enumerate().peekable();
    // YAML front matter.
    if lines.peek().is_some_and(|(_, l)| l.trim() == "---") {
        lines.next();
        for (_, l) in lines.by_ref() {
            if l.trim() == "---" {
                break;
            }
        }
    }
    let mut para: Vec<String> = Vec::new();
    let (mut start, mut end) = (0u32, 0u32);
    let mut in_code = false;
    for (i, raw) in lines {
        let line = raw.trim();
        if line.starts_with("```") || line.starts_with("~~~") {
            in_code = !in_code;
            if !para.is_empty() {
                break;
            }
            continue;
        }
        if in_code {
            continue;
        }
        if line.is_empty() {
            if !para.is_empty() {
                break;
            }
            continue;
        }
        let skip = line.starts_with('#')
            || line.starts_with('<')
            || line.starts_with("![")
            || line.starts_with("[![")
            || line.starts_with('|')
            || line.starts_with("- ")
            || line.starts_with("* ")
            || line.starts_with('>')
            || line.starts_with("===")
            || line.starts_with("---")
            || line.starts_with(".. ")
            || line.chars().all(|c| "=-~^*".contains(c));
        if skip {
            if !para.is_empty() {
                break;
            }
            continue;
        }
        if para.is_empty() {
            start = i as u32 + 1;
        }
        end = i as u32 + 1;
        para.push(strip_markup(line));
    }
    let joined = para.join(" ");
    let joined = joined.trim();
    if joined.chars().filter(|c| c.is_alphabetic()).count() < 12 {
        return None;
    }
    Some((joined.to_string(), start, end))
}

/// `[text](url)` → `text`, and `**`/`__`/`*`/`_`/backtick emphasis dropped.
fn strip_markup(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(open) = rest.find('[') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find("](") {
            Some(mid) if !after[..mid].contains('[') => {
                out.push_str(&after[..mid]);
                let tail = &after[mid + 2..];
                rest = match tail.find(')') {
                    Some(close) => &tail[close + 1..],
                    None => "",
                };
            }
            _ => {
                out.push('[');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out.replace("**", "").replace("__", "").replace('`', "")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_prose_paragraph_is_taken_past_headings_and_badges() {
        let text = "# Title\n\n[![ci](x.svg)](y)\n\nThe **runtime** drives [sessions](a.md) and\nheartbeats.\n\nSecond paragraph.\n";
        let (p, start, end) = first_paragraph(text).unwrap();
        assert_eq!(p, "The runtime drives sessions and heartbeats.");
        assert_eq!((start, end), (5, 6));
    }

    #[test]
    fn a_readme_of_only_headings_and_code_has_no_paragraph() {
        assert_eq!(first_paragraph("# Title\n\n```\ncode here\n```\n"), None);
    }

    #[test]
    fn front_matter_is_skipped() {
        let (p, ..) =
            first_paragraph("---\ntitle: x\n---\nA module that parses frames.\n").unwrap();
        assert_eq!(p, "A module that parses frames.");
    }
}
