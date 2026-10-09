//! The corpus signpost (U10/R12): a rozcestník into the card corpus, appended
//! to `index.md` alongside the existing view list. `render_map`'s own list
//! covers views only — with tens of thousands of cards in a large repository,
//! nothing else points a reader or agent at a module or a type without
//! knowing the `cards/` directory layout already.

use super::links::{mention, CardIndex};
use super::{Card, CardKind};
use crate::cli::LinkStyle;
use std::collections::BTreeMap;
use std::fmt::Write;

/// How many types the "Notable types" section names, at most. A short,
/// curated highlight — the module list already gives exhaustive structural
/// navigation, so this section stays a highlight reel, not a second index.
const NOTABLE_TYPES_LIMIT: usize = 20;

/// How many cards each declared-kind section names, at most. The same argument
/// as [`NOTABLE_TYPES_LIMIT`], kept as its own constant because the two sections
/// answer about different corpora: a repository has one type population, and a
/// plugin that wants a longer list of its own kind should be able to get one
/// without lengthening the type highlight reel.
const DECLARED_KIND_LIMIT: usize = 20;

/// Append the rendered signpost to `index.md`'s own [`crate::render::RenderedFile`]
/// in `corpus`. `index.md` is always present (`render::build_corpus` writes it
/// unconditionally), so a missing entry here would be a broken invariant
/// elsewhere, not a case this function needs to handle.
///
/// Takes an already-built `CardIndex` rather than building its own: the
/// caller (`generate_reporting`) also needs one to render the card files
/// themselves, and `CardIndex::build` is an O(n) clone-into-a-map over the
/// full card set — worth sharing, not repeating, at real-repository scale.
pub fn append_signpost(
    corpus: &mut crate::render::Corpus,
    cards: &[Card],
    index: &CardIndex,
    links: LinkStyle,
) {
    let section = render_signpost(cards, index, links);
    if let Some(file) = corpus.files.iter_mut().find(|f| f.path == "index.md") {
        file.contents.push_str(&section);
    }
}

/// Put a `## Start here` list of the architecture pages above the view list in
/// `index.md` — the pages are what a first-time reader should open, and the
/// views are the narrower follow-up questions.
pub fn prepend_start_here(corpus: &mut crate::render::Corpus, pages: &[(&str, &str, &str)]) {
    if pages.is_empty() {
        return;
    }
    let mut block = String::from("## Start here\n\n");
    for (path, title, blurb) in pages {
        let _ = writeln!(block, "- [{title}]({path}) — {blurb}");
    }
    block.push('\n');
    if let Some(file) = corpus.files.iter_mut().find(|f| f.path == "index.md") {
        match file.contents.find("## Views") {
            Some(at) => file.contents.insert_str(at, &block),
            None => file.contents.push_str(&format!("\n{block}")),
        }
    }
}

/// Render the `## Modules` and `## Notable types` sections appended to
/// `index.md`, plus one section per declared card kind (R1, U5). `from_card` for
/// every link is `"index.md"` itself, since this content lives at the corpus
/// root.
///
/// Ranking is `Card.incoming.len()` (existing backlink data, no new graph
/// traversal), descending, with identity as the deterministic tiebreak.
pub fn render_signpost(cards: &[Card], index: &CardIndex, links: LinkStyle) -> String {
    let mut s = String::new();
    render_modules(&mut s, cards, index, links);
    render_notable_types(&mut s, cards, index, links);
    render_declared(&mut s, cards, index, links);
    s
}

fn render_modules(out: &mut String, cards: &[Card], index: &CardIndex, links: LinkStyle) {
    let _ = write!(out, "\n## Modules\n\n");
    let mut modules: Vec<&Card> = cards
        .iter()
        // Core card kinds only. A declared card kind belongs in neither this
        // section nor the types one below: `Modules` and `Notable types` are the
        // core's own words for the core's own tree, and a declared kind that
        // borrowed one of them would be claiming to be something it is not.
        // It gets its own section instead — see `render_declared`.
        .filter(|c| c.kind == CardKind::Module)
        .collect();
    modules.sort_by(|a, b| a.identity.cmp(&b.identity));

    if modules.is_empty() {
        let _ = writeln!(out, "No modules were extracted from this repository.");
        return;
    }
    let _ = writeln!(
        out,
        "| Module | Files | Lines | Types | Summary |\n|---|---|---|---|---|"
    );
    for m in modules {
        let (files, types) = m
            .story
            .module_story
            .as_ref()
            .map_or((0, 0), |ms| (ms.files.len(), ms.types));
        let label = super::explain::module_label(&m.title);
        // The count of types leads to the module's map, which lists them one
        // row each (plan B, U10).
        let types = match &m.map {
            Some(map) => super::links::link("index.md", map, &types.to_string(), links),
            None => types.to_string(),
        };
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} | {} |",
            mention("index.md", &m.identity, label, links, index),
            files,
            m.story
                .lines
                .map(super::explain::grouped)
                .unwrap_or_default(),
            types,
            m.story.summary.as_deref().unwrap_or("").replace('|', "\\|")
        );
    }
}

fn render_notable_types(out: &mut String, cards: &[Card], index: &CardIndex, links: LinkStyle) {
    let _ = write!(out, "\n## Notable types\n\n");
    let types: Vec<&Card> = cards.iter().filter(|c| c.kind == CardKind::Type).collect();

    if types.is_empty() {
        let _ = writeln!(out, "No type cards exist in this repository.");
        return;
    }
    // Ranked by how many other types and files reach the type on undisputed
    // relations (`users_certain`), then by all incoming evidence, then identity
    // (R18). The list
    // says where it was cut.
    let mut ranked = types;
    ranked.sort_by(|a, b| {
        b.story
            .users_certain
            .cmp(&a.story.users_certain)
            .then_with(|| b.incoming.len().cmp(&a.incoming.len()))
            .then_with(|| a.identity.cmp(&b.identity))
    });
    let total = ranked.len();
    let _ = writeln!(
        out,
        "| Type | Kind | Module | Used by | Summary |\n|---|---|---|---|---|"
    );
    for c in ranked.iter().take(NOTABLE_TYPES_LIMIT) {
        let module = c
            .story
            .module
            .as_deref()
            .map(|m| mention("index.md", m, super::explain::module_label(m), links, index))
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} | {} |",
            mention("index.md", &c.identity, &c.title, links, index),
            c.entity_kind.as_deref().unwrap_or(""),
            module,
            c.story.users_certain,
            c.story.summary.as_deref().unwrap_or("").replace('|', "\\|")
        );
    }
    if total > NOTABLE_TYPES_LIMIT {
        let _ = writeln!(
            out,
            "\nThe {NOTABLE_TYPES_LIMIT} most used of {total} types; every module card lists its own."
        );
    }
}

/// Write one ranked, capped list of cards as `- <link> — referenced N times`.
///
/// The two sections that use it stay separate functions on purpose — what goes
/// into each, and what each says when it is empty, are different decisions that
/// their own doc comments argue for keeping apart. Only the mechanical half is
/// shared: the ranking comparator and the line format were character-identical
/// in both, which is the shape where one gets updated alone (RG3).
///
/// `confess_cap` decides whether a truncated list says so. It is `true` for the
/// declared sections and `false` for `Notable types`, and that asymmetry is
/// deliberate rather than an oversight: adding the line to the core section would
/// change the bytes of every existing corpus with more than
/// [`NOTABLE_TYPES_LIMIT`] types, which R4 forbids. The core section's silence is
/// older than this work and is not this work's to break; the declared one is new,
/// and three independent external reviewers called its silence a violation of
/// R10 — a signpost that drops entries without saying so tells the reader the
/// list is complete.
fn render_ranked(
    out: &mut String,
    mut cards: Vec<&Card>,
    limit: usize,
    confess_cap: bool,
    index: &CardIndex,
    links: LinkStyle,
) {
    // Most-referenced first, identity as the deterministic tiebreak (R18).
    cards.sort_by(|a, b| {
        b.incoming
            .len()
            .cmp(&a.incoming.len())
            .then_with(|| a.identity.cmp(&b.identity))
    });
    let total = cards.len();
    for c in cards.into_iter().take(limit) {
        let count = c.incoming.len();
        let noun = if count == 1 { "time" } else { "times" };
        let _ = writeln!(
            out,
            "- {} — referenced {count} {noun}",
            mention("index.md", &c.identity, &c.title, links, index)
        );
    }
    if confess_cap && total > limit {
        let left = total - limit;
        let noun = if left == 1 { "card" } else { "cards" };
        let _ = writeln!(
            out,
            "- … and {left} further {noun} of this kind, not listed here."
        );
    }
}

/// One section per declared card kind, headed by the word the declaration gave
/// it (R1, KTD4) — never by a core word, and never by a digest.
///
/// Writes **nothing at all** when the corpus declared no cards: not an empty
/// section, not a "none" line. This function appends to a file every corpus has,
/// and a corpus that declares nothing must stay byte-identical to what the tool
/// produced before any of this existed (R4). The two core sections above say
/// "none" because their absence would itself be a finding about a repository;
/// a declared section's absence is just a corpus that declared nothing.
///
/// One section per kind rather than one merged list: two declared kinds are two
/// different questions a reader asks, and merging them would bury the answer to
/// the smaller one under the larger. Each is capped like `Notable types` — a
/// corpus can declare kinds with thousands of nodes, and an uncapped list would
/// stop being a signpost and become a second index of its own — and what the cap
/// leaves out, it says (R10).
fn render_declared(out: &mut String, cards: &[Card], index: &CardIndex, links: LinkStyle) {
    // Keyed by the reader-facing word, which is `namespace:name` and therefore
    // unique per declared card kind — so grouping by it groups by kind, and the
    // `BTreeMap` fixes section order by name rather than by digest (R18).
    let mut by_kind: BTreeMap<&str, Vec<&Card>> = BTreeMap::new();
    for card in cards
        .iter()
        .filter(|c| matches!(c.kind, CardKind::Declared(_)))
    {
        by_kind
            .entry(card.kind_word.as_ref())
            .or_default()
            .push(card);
    }

    for (word, group) in by_kind {
        // `word` is `namespace:name`, and `check_name` refuses a name carrying a
        // control character or a backtick — so a declared kind cannot open a
        // heading this corpus did not declare (RG3).
        let _ = write!(out, "\n## {word}\n\n");
        render_ranked(out, group, DECLARED_KIND_LIMIT, true, index, links);
    }
}
