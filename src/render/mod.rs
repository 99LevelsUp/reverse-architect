//! Rendering (U4): projects the fact graph into a human corpus — a top-level
//! map plus one self-contained leaf per view.

pub mod markdown;
pub mod mermaid;
pub mod split;

use crate::cli::LinkStyle;
use crate::graph::Graph;
use crate::lens;
use crate::taxonomy;
use markdown::MapEntry;

/// A single rendered corpus file (path relative to the corpus directory).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedFile {
    pub path: String,
    pub contents: String,
}

/// The complete rendered corpus: the map (`index.md`) followed by the leaves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Corpus {
    pub files: Vec<RenderedFile>,
}

/// Build the corpus from the graph, in deterministic order (map first, then
/// leaves in catalog order).
///
/// `profile` narrows the view set to a named taxonomy override (R18); `None`
/// renders the automatic taxonomy. The same `profile` MUST be passed to
/// [`crate::manifest::build_manifest`] or the two projections diverge (KTD-C5).
pub fn build_corpus(
    graph: &Graph,
    links: LinkStyle,
    profile: Option<&str>,
) -> Result<Corpus, crate::taxonomy::profile::UnknownProfile> {
    let views = taxonomy::select_views(graph, profile)?;

    let entries: Vec<MapEntry> = views
        .iter()
        .map(|v| MapEntry {
            id: v.id.to_string(),
            title: v.title.to_string(),
        })
        .collect();

    let mut files = vec![RenderedFile {
        path: "index.md".to_string(),
        contents: markdown::render_map(&entries, links),
    }];

    for view in &views {
        let lens = lens::compose(view, graph);
        files.push(RenderedFile {
            path: format!("{}.md", view.id),
            contents: markdown::render_leaf(&lens),
        });
    }

    Ok(Corpus { files })
}
