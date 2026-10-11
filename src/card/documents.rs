//! The manifest's documents for the card corpus (plan B, U11: R4, R16,
//! KTD9): one per card, one per module map, one per detail.
//!
//! The three levels are three kinds of document, so each channel picks what
//! it indexes: a vector index takes the maps and the cards and leaves out
//! every detail (`embed: false`, the owner's decision — a detail's anchors
//! would flood the vectors of the cards), while an agent or a pipeline that
//! reads files follows `parent` from a card down to its detail. Every one of
//! them carries the full context header of its entity as structured fields,
//! which the files themselves state in one line (R4).
//!
//! A card is paired with its rendered file by order, as it always was —
//! [`super::render_card_files`] renders one file per card, in card order. A
//! map and a detail are not: only some cards have one, so each is found by
//! the path its card was assigned ([`super::Card::detail`],
//! [`super::Card::map`]), and a path nothing rendered gets no document.

use super::render::Lang;
use super::{map, Card, CardKind};
use crate::manifest::{rendered_document, ManifestDoc, ManifestHeader};
use crate::render::RenderedFile;
use std::collections::{HashMap, HashSet};

/// The `level` of a card's document; a map's is `map`, a detail's `detail`.
const LEVEL_CARD: &str = "card";
const LEVEL_MAP: &str = "map";
const LEVEL_DETAIL: &str = "detail";

/// The document kind of a module map.
pub const MAP_KIND: &str = "map";
/// The document kind of a detail: a card's own level below it, so it keeps
/// the `card:` prefix, and `level` is what tells it from a card.
pub const DETAIL_KIND: &str = "card:detail";

/// A map's document id: `map:<module key>`.
pub fn map_id(module: &str) -> String {
    format!("map:{module}")
}

/// A detail's document id: `detail:<card identity>`. Not
/// `<identity>#detail`, which is the shape of the card's own chunk ids.
pub fn detail_id(identity: &str) -> String {
    format!("detail:{identity}")
}

/// The documents of every card, map and detail: the cards in card order,
/// then the maps, then the details, each in card order. `files` are the
/// cards' rendered files, one per card in the same order; `maps` and
/// `details` are what [`map::render_map_files`] and
/// [`super::detail::render_detail_files`] rendered.
pub fn manifest_documents(
    cards: &[Card],
    files: &[RenderedFile],
    maps: &[RenderedFile],
    details: &[RenderedFile],
    lang: Lang,
) -> Vec<ManifestDoc> {
    let maps: HashMap<&str, &RenderedFile> = maps.iter().map(|f| (f.path.as_str(), f)).collect();
    let details: HashMap<&str, &RenderedFile> =
        details.iter().map(|f| (f.path.as_str(), f)).collect();
    // The modules whose map was rendered: a type card's parent is its
    // module's map only where that map is a document too.
    let mapped: HashSet<&str> = cards
        .iter()
        .filter(|c| c.kind == CardKind::Module)
        .filter(|c| c.map.as_deref().is_some_and(|p| maps.contains_key(p)))
        .map(|c| c.identity.as_str())
        .collect();

    let mut out: Vec<ManifestDoc> = Vec::with_capacity(cards.len() + maps.len() + details.len());
    for (card, file) in cards.iter().zip(files) {
        let (kind, question) = card_kind_and_question(card);
        let mut doc = rendered_document(
            &card.identity,
            &kind,
            &card.title,
            &question,
            &file.path,
            &file.contents,
        );
        doc.level = Some(LEVEL_CARD.into());
        doc.parent = match card.kind {
            CardKind::Type => card
                .story
                .module
                .as_deref()
                .filter(|m| mapped.contains(m))
                .map(map_id),
            _ => None,
        };
        doc.header = Some(header(card, lang));
        out.push(doc);
    }
    for module in cards.iter().filter(|c| c.kind == CardKind::Module) {
        let Some(file) = module.map.as_deref().and_then(|p| maps.get(p)) else {
            continue;
        };
        let mut doc = rendered_document(
            &map_id(&module.identity),
            MAP_KIND,
            &format!("Map of {}", module.title),
            &format!(
                "Which types does `{}` hold, and what is each of them for?",
                module.title
            ),
            &file.path,
            &file.contents,
        );
        doc.level = Some(LEVEL_MAP.into());
        doc.parent = Some(module.identity.clone());
        doc.header = Some(header(module, lang));
        out.push(doc);
    }
    for card in cards {
        let Some(file) = card.detail.as_deref().and_then(|p| details.get(p)) else {
            continue;
        };
        let mut doc = rendered_document(
            &detail_id(&card.identity),
            DETAIL_KIND,
            &format!("Detail of {}", card.title),
            &format!(
                "Which line of `{}` holds a given text, database access or decision, and what \
                 does it relate to?",
                card.title
            ),
            &file.path,
            &file.contents,
        );
        doc.level = Some(LEVEL_DETAIL.into());
        doc.parent = Some(card.identity.clone());
        doc.embed = Some(false);
        doc.header = Some(header(card, lang));
        out.push(doc);
    }
    out
}

/// The document of the registry of triggers (plan 2b, U7: R14, KTD10): id
/// `triggers:index`, kind `triggers`, level `map`, its chunks its `## Part N`
/// or its one `## Triggers`. It has no `parent` and no `header`: it is about
/// no one entity, so there is no context header to give it.
pub fn triggers_document(file: &RenderedFile) -> ManifestDoc {
    let mut doc = rendered_document(
        super::triggers::TRIGGERS_ID,
        super::triggers::TRIGGERS_KIND,
        "Registry of triggers",
        "Which triggers lead to which functionality, and is each way open?",
        &file.path,
        &file.contents,
    );
    doc.level = Some(LEVEL_MAP.into());
    doc
}

/// A card's document kind and guiding question (v8).
fn card_kind_and_question(card: &Card) -> (String, String) {
    match card.kind {
        CardKind::Type => (
            "card:type".to_string(),
            format!("What is `{}` for, and what does it work with?", card.title),
        ),
        CardKind::File => (
            "card:file".to_string(),
            format!("What does `{}` declare, and what does it use?", card.title),
        ),
        CardKind::Module => (
            "card:module".to_string(),
            format!(
                "What does `{}` hold, and what does it depend on?",
                card.title
            ),
        ),
        CardKind::Declared(_) => (
            format!("card:{}", card.kind_word),
            format!("What is `{}`?", card.title),
        ),
    }
}

/// The context header of `card`'s entity (R4). A map's header is its
/// module card's: the map is about the module.
fn header(card: &Card, lang: Lang) -> ManifestHeader {
    let module = match card.kind {
        CardKind::Module => Some(card.identity.clone()),
        _ => card.story.module.clone(),
    };
    ManifestHeader {
        path: card
            .source
            .as_ref()
            .map_or_else(|| card.identity.clone(), |s| s.file.clone()),
        module,
        namespace: namespace(&card.identity).map(str::to_string),
        kind: card.kind_phrase(),
        name: card.title.clone(),
        purpose: map::brief_purpose(card).map(|s| s.text(lang)),
    }
}

/// The qualifier a `sym:<lang>:<qualifier>#<path>` key holds — a C#
/// namespace, a Java package, a Rust module path, a JS/TS file — or `None`
/// for any other key and an empty qualifier.
///
/// Read from the key, the second exception to "no projection parses a key
/// for readable content" beside the parameter list of a label
/// (`params_of`): the qualifier is what the plugin keyed the symbol under,
/// and no fact carries it — `SymbolContainer` names the enclosing TYPE, and
/// a top-level type has none.
fn namespace(identity: &str) -> Option<&str> {
    let (_, rest) = identity.strip_prefix("sym:")?.split_once(':')?;
    let (qualifier, _) = rest.split_once('#')?;
    (!qualifier.is_empty()).then_some(qualifier)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_namespace_is_the_qualifier_of_a_symbol_key_and_nothing_else() {
        assert_eq!(
            namespace("sym:csharp:Orbit.Geo#GeoSolver"),
            Some("Orbit.Geo")
        );
        assert_eq!(
            namespace("sym:rust:crate::helper#Widget"),
            Some("crate::helper")
        );
        assert_eq!(
            namespace("sym:ts:ts/model.ts#TsInvoice"),
            Some("ts/model.ts")
        );
        assert_eq!(namespace("src/helper.rs"), None);
        assert_eq!(namespace("src"), None);
        assert_eq!(namespace("sym:csharp:#Global"), None);
    }
}
