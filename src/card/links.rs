//! Intra-corpus links and backlinks (U9).
//!
//! R11 asks for three different things, and the distinctions are the whole
//! substance of this module:
//!
//! - a mention of a **symbol or module** becomes a link to its card,
//! - a mention of a **source file that declares a type** points at the file in
//!   the repository rather than at a card, because such a file owns none (R8).
//!   A file that declares nothing DOES own a card, and a mention of it links
//!   there like any other carded entity — the rule is "link what has a card",
//!   and `CardIndex` is the single answer to which those are,
//! - a mention of something with **no card at all** (an external type, a database
//!   object) stays text with its anchor, rather than becoming a link into nothing.
//!
//! ## Why a source-file mention is not a hyperlink
//!
//! It is rendered as the repo-relative path in code style. A hyperlink would have
//! to resolve somewhere, and there is no somewhere that is correct: the corpus is
//! written to a sidecar directory beside the repository AND, with `--mirror`, to a
//! byte-identical copy inside it (R14). One relative path cannot be right in both
//! places, and a forge URL would mean assuming a remote — RA is offline by
//! design. A path the reader can open is honest; a link that resolves in one of
//! the two layouts is a dead link in the other, which is exactly what this unit's
//! own verification forbids.

use super::Card;
use crate::cli::LinkStyle;
use crate::graph::EdgeKind;
use std::borrow::Cow;
use std::collections::BTreeMap;

/// How many entries of ONE relation kind a backlink section renders before it
/// says how many more of that kind there are.
///
/// A widely used type in a large repository is referenced thousands of times; a
/// card listing all of them stops being readable and stops being retrievable
/// (R17). The cap is never silent: the section names the cap and, for each kind
/// it thinned, how many of that kind it left out. It does NOT print one grand
/// total — a single number would say "some of everything" about a list that is
/// complete for most kinds and cut for one.
pub const BACKLINK_CAP: usize = 50;

/// Which identities own a card, and where that card lives.
///
/// Built from the finished card set, so "has a card" is answered by the same list
/// that gets written to disk — a link can never point at a card that was never
/// rendered.
#[derive(Debug, Default, Clone)]
pub struct CardIndex {
    paths: BTreeMap<String, String>,
}

impl CardIndex {
    pub fn build(cards: &[Card]) -> CardIndex {
        CardIndex {
            paths: cards
                .iter()
                .map(|c| (c.identity.clone(), c.path.clone()))
                .collect(),
        }
    }

    pub fn path_of(&self, identity: &str) -> Option<&str> {
        self.paths.get(identity).map(String::as_str)
    }

    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    pub fn len(&self) -> usize {
        self.paths.len()
    }
}

/// Render a mention of `identity` from the card at `from_card`.
///
/// A target that owns a card becomes a link in the configured style. Everything
/// else — a source file, an external symbol, a database object — becomes the
/// label in code style, which is a reference a reader can act on and a link
/// checker will never flag.
pub fn mention(
    from_card: &str,
    identity: &str,
    label: &str,
    links: LinkStyle,
    index: &CardIndex,
) -> String {
    match index.path_of(identity) {
        Some(target) => link(from_card, target, label, links),
        None => format!("`{label}`"),
    }
}

/// A link from the document at `from_doc` to the document at `target`, both
/// corpus-relative, in the configured style — what [`mention`] writes for a
/// carded target, and what a card writes to its own detail (plan B, U8).
pub fn link(from_doc: &str, target: &str, label: &str, links: LinkStyle) -> String {
    match links {
        LinkStyle::Portable => {
            format!("[{}]({})", escape_label(label), relative(from_doc, target))
        }
        // Obsidian resolves a wikilink across the whole vault by name, so the
        // target is spelled from the corpus root and without `.md`: two cards
        // can legitimately share a file name in different trees, and a bare
        // name would resolve to whichever one the vault found first.
        LinkStyle::Obsidian => {
            format!(
                "[[{}|{}]]",
                target.strip_suffix(".md").unwrap_or(target),
                escape_label(label)
            )
        }
    }
}

/// A `[` or `]` inside link text ends the text early and produces a broken link.
/// Backslash-escaped rather than replaced, so the label still reads as itself.
fn escape_label(label: &str) -> String {
    label.replace('[', "\\[").replace(']', "\\]")
}

/// The relative path from the document at `from_doc` to the document at `to_doc`,
/// both corpus-relative.
///
/// Pure path arithmetic over `/`-separated components: drop the shared directory
/// prefix, climb out of what remains on the left, then descend on the right.
pub fn relative(from_doc: &str, to_doc: &str) -> String {
    let from: Vec<&str> = from_doc.split('/').collect();
    let to: Vec<&str> = to_doc.split('/').collect();
    // The last component is the document itself, not a directory to climb out of.
    let from_dirs = &from[..from.len().saturating_sub(1)];

    let shared = from_dirs
        .iter()
        .zip(to.iter())
        .take_while(|(a, b)| a == b)
        .count();

    let mut parts: Vec<&str> = vec![".."; from_dirs.len() - shared];
    parts.extend(&to[shared..]);
    // Same directory: a bare file name is a valid relative link and reads better
    // than `./name`.
    parts.join("/")
}

/// The backlink entries a card shows, and how many of each kind were left out.
///
/// The cap is applied PER RELATION KIND rather than over the whole list. A card's
/// incoming relations are sorted by kind, so one global cap would spend the entire
/// budget on whichever kind happens to be numerous and leave every other kind at
/// zero: an interface with three thousand implementors would show no callers at
/// all, and a single "2950 more" line would hide the fact that a whole category
/// had vanished rather than been thinned.
///
/// Split out so the renderer never has to decide what to drop: it renders what it
/// is given and states what it was not given alongside.
///
/// It counts `card.incoming` as `build_card` left it (plan A, U3): one row per
/// relation, no row between two members of the card's own unit, no ambiguous
/// row. So the cap is spent on real foreign callers, and neither a duplicate
/// nor a type's own inside can push one of them out.
/// The omitted counts carry the relation's reader-facing *word*, not its
/// kind: the renderer holds no vocabulary, so a declared kind could not be
/// named there (KTD4).
pub fn backlinks(card: &Card) -> (Vec<&super::Relation>, Vec<(Cow<'static, str>, usize)>) {
    // Keyed so a DECLARED kind groups and orders by its NAME, never by its id —
    // the same R2 rule `card::sort_relations` obeys, and broken here in the same
    // way until the external review round read both. Core kinds keep the variant
    // order they always had: they all key under `false` with an empty word, so a
    // corpus that declares nothing does not move (R4).
    type KindKey = (bool, Option<EdgeKind>, String);
    let key = |r: &super::Relation| -> KindKey {
        let declared = matches!(r.kind, EdgeKind::Declared(_));
        (
            declared,
            (!declared).then_some(r.kind),
            if declared {
                r.kind_word.to_string()
            } else {
                String::new()
            },
        )
    };
    let mut by_kind: BTreeMap<KindKey, Vec<&super::Relation>> = BTreeMap::new();
    for relation in &card.incoming {
        by_kind.entry(key(relation)).or_default().push(relation);
    }

    let mut shown = Vec::new();
    let mut omitted = Vec::new();
    for (_key, group) in by_kind {
        let take = group.len().min(BACKLINK_CAP);
        shown.extend(&group[..take]);
        if group.len() > take {
            let word = group[0].kind_word.clone();
            omitted.push((word, group.len() - take));
        }
    }
    (shown, omitted)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::card::{CardKind, Relation};
    use crate::graph::Resolution;

    fn incoming(kinds: &[(EdgeKind, usize)]) -> Card {
        let mut card = Card {
            identity: "sym:cs:Ns#IThing".to_string(),
            path: "cards/type/cs/Ns/IThing.md".to_string(),
            kind: CardKind::Type,
            kind_word: "type".into(),
            title: "IThing".to_string(),
            entity_kind: None,
            visibility: None,
            signature: None,
            header: None,
            source: None,
            doc: None,
            members: Vec::new(),
            outgoing: Vec::new(),
            incoming: Vec::new(),
            externals: Vec::new(),
            maybe_also: Vec::new(),
            test: None,
            caveats: Vec::new(),
            story: Default::default(),
            navigators: Default::default(),
            detail: None,
            map: None,
            generated: false,
        };
        for (kind, count) in kinds {
            for i in 0..*count {
                card.incoming.push(Relation {
                    kind: *kind,
                    // Through the production mapping, not a re-derivation.
                    // Review caught the earlier `format!("{kind:?}").to_lowercase()`
                    // here: it agrees for `Implements` and `Calls` and silently
                    // disagrees for `HasMember` ("hasmember" vs "has_member"),
                    // so the fixture modelled exactly the drift the field can
                    // suffer.
                    kind_word: crate::graph::edge_word(*kind, &crate::graph::Vocabulary::default()),
                    label: format!("{kind:?}{i:04}"),
                    identity: format!("sym:cs:Ns#{kind:?}{i:04}"),
                    resolution: Resolution::Heuristic,
                    source: None,
                    via: None,
                    ambiguous_candidates: None,
                    rung: None,
                    via_identity: None,
                    sites: Vec::new(),
                });
            }
        }
        crate::card::sort_relations(&mut card.incoming);
        card
    }

    #[test]
    fn a_numerous_relation_kind_cannot_starve_a_rare_one() {
        // The realistic shape in a large repository: an interface implemented
        // everywhere and called from a handful of places. A cap over the whole
        // kind-sorted list would spend its budget on `implements` and render the
        // three callers invisible — the card would then be silent about the
        // question a reader most often asks of an interface.
        let card = incoming(&[(EdgeKind::Implements, 3000), (EdgeKind::Calls, 3)]);
        let (shown, omitted) = backlinks(&card);

        let calls = shown.iter().filter(|r| r.kind == EdgeKind::Calls).count();
        assert_eq!(calls, 3, "every caller must survive the cap");
        assert_eq!(
            shown
                .iter()
                .filter(|r| r.kind == EdgeKind::Implements)
                .count(),
            BACKLINK_CAP
        );
        // And what was dropped is named per kind, not summed into one number.
        assert_eq!(
            omitted,
            vec![(Cow::Borrowed("implements"), 3000 - BACKLINK_CAP)]
        );
    }

    #[test]
    fn nothing_is_reported_as_omitted_when_nothing_was() {
        let card = incoming(&[(EdgeKind::Calls, 2), (EdgeKind::References, 1)]);
        let (shown, omitted) = backlinks(&card);
        assert_eq!(shown.len(), 3);
        assert!(omitted.is_empty(), "{omitted:?}");
    }

    #[test]
    fn a_sibling_document_is_a_bare_file_name() {
        assert_eq!(
            relative("cards/type/cs/Ns/Invoice.md", "cards/type/cs/Ns/Money.md"),
            "Money.md"
        );
    }

    #[test]
    fn a_document_in_another_tree_climbs_out_and_back_down() {
        assert_eq!(
            relative("cards/type/cs/Ns/Invoice.md", "cards/module/src.md"),
            "../../../module/src.md"
        );
        assert_eq!(
            relative("cards/module/src.md", "cards/type/cs/Ns/Invoice.md"),
            "../type/cs/Ns/Invoice.md"
        );
    }

    #[test]
    fn a_deeper_target_only_descends() {
        assert_eq!(
            relative("cards/module/_root.md", "cards/module/src/extract.md"),
            "src/extract.md"
        );
    }

    #[test]
    fn a_shallower_target_only_climbs() {
        assert_eq!(
            relative("cards/module/src/extract.md", "cards/module/_root.md"),
            "../_root.md"
        );
    }

    #[test]
    fn a_target_without_a_card_stays_text_not_a_link() {
        let index = CardIndex::default();
        assert_eq!(
            mention(
                "cards/type/cs/Ns/Invoice.md",
                "res:db:dbo.Orders",
                "dbo.Orders",
                LinkStyle::Portable,
                &index
            ),
            "`dbo.Orders`"
        );
    }

    #[test]
    fn both_link_styles_address_the_same_card() {
        let mut index = CardIndex::default();
        index.paths.insert(
            "sym:cs:Ns#Money".to_string(),
            "cards/type/cs/Ns/Money.md".to_string(),
        );

        assert_eq!(
            mention(
                "cards/type/cs/Ns/Invoice.md",
                "sym:cs:Ns#Money",
                "Money",
                LinkStyle::Portable,
                &index
            ),
            "[Money](Money.md)"
        );
        // Obsidian: from the corpus root, without the extension.
        assert_eq!(
            mention(
                "cards/type/cs/Ns/Invoice.md",
                "sym:cs:Ns#Money",
                "Money",
                LinkStyle::Obsidian,
                &index
            ),
            "[[cards/type/cs/Ns/Money|Money]]"
        );
    }

    #[test]
    fn brackets_in_a_label_cannot_break_the_link_text() {
        let mut index = CardIndex::default();
        index.paths.insert(
            "sym:rs:m#Idx".to_string(),
            "cards/type/rs/m/Idx.md".to_string(),
        );
        let rendered = mention(
            "cards/type/rs/m/Other.md",
            "sym:rs:m#Idx",
            "Index[T]",
            LinkStyle::Portable,
            &index,
        );
        assert_eq!(rendered, "[Index\\[T\\]](Idx.md)");
    }
}
