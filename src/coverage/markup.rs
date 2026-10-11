use crate::extract::languages::Languages;
use crate::graph::{FactKind, FactValue, Graph, NodeKind};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// The language whose coverage [`Markup`] states.
pub const MARKUP_LANGUAGE: &str = "xaml";

/// How far XAML was read in a run (plan 2c, R19): a property of the run,
/// never of one card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MarkupState {
    /// The run has the second layer and no plugin reads XAML: a member
    /// without a marker may still be bound from XAML.
    NotRead,
    /// XAML was read, but a file failed, a component claiming XAML read no
    /// bodies (contract 0.2) or a value written as a markup extension could
    /// not be taken apart: a missing marker proves nothing either.
    ReadWithUnsupportedConstructs,
    /// Every XAML file was read in both layers: a missing marker is evidence
    /// that no way from XAML exists.
    ReadInFull,
    /// A plugin reads XAML, but the run has no second layer: no marker was
    /// made at all.
    SecondLayerOff,
}

/// A XAML file one pass of the run failed on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailedMarkup {
    pub path: String,
    pub plugin: String,
    /// `first-layer` (declarations or usages) or `bodies`.
    pub pass: String,
    /// Redacted, as the plugin's every text.
    pub reason: String,
}

/// A XAML file with values written as markup extensions its plugin could not
/// take apart (`Text="{Binding Title"`, the tag `unparsed-markup`): the file
/// was read, but what those values say was lost (plan 2c, R19).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnparsedMarkup {
    pub path: String,
    /// How many such values the file has.
    pub count: u64,
}

/// The markup places the second layer read, by how they ended (plan 2c,
/// U6): counted, never a reason to lower the state — an unpaired binding is
/// a known limit, not an unread file.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarkupSites {
    pub resolved: u64,
    pub ambiguous: u64,
    pub unresolved: u64,
    pub unpaired: u64,
}

/// XAML coverage of a run (plan 2c, U8: R19, KTD12).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Markup {
    pub state: MarkupState,
    /// The plugins of the run that claim XAML, by id.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plugins: Vec<String>,
    /// Tracked files of the language XAML.
    pub files: u64,
    /// Every XAML file a pass failed on, by path and pass.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failed: Vec<FailedMarkup>,
    /// The plugins that claim XAML and read no bodies in a run with the
    /// second layer — a component on contract 0.2 serves no `bodies` pass.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub without_bodies: Vec<String>,
    /// The XAML files whose bodies hold a markup extension the plugin could
    /// not take apart, by path; only in a run with the second layer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unparsed: Vec<UnparsedMarkup>,
    /// The markup places of the second layer, by outcome; absent without it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sites: Option<MarkupSites>,
}

/// The state the facts of a run give (R19): only a failed file, a component
/// that served no bodies and a markup extension the plugin could not take
/// apart lower it — never a known limit.
pub fn markup_state(
    second_layer: bool,
    claimed: bool,
    failed: bool,
    without_bodies: bool,
    unparsed: bool,
) -> MarkupState {
    match (second_layer, claimed) {
        (false, _) => MarkupState::SecondLayerOff,
        (true, false) => MarkupState::NotRead,
        (true, true) if failed || without_bodies || unparsed => {
            MarkupState::ReadWithUnsupportedConstructs
        }
        (true, true) => MarkupState::ReadInFull,
    }
}

/// The XAML section of coverage (plan 2c, U8): `plugins` are the ids of the
/// run's plugins that claim XAML, `bodies` the second layer when the run has
/// one, `sites` how the second layer's markup places ended and `unparsed` the
/// files with a markup extension the plugin could not take apart (both read
/// only with the second layer). `symlinks` are the tracked symlinks: File
/// nodes no pass reads, which are no XAML file read whatever their name.
/// `None` for a run with neither a plugin for XAML nor the second layer.
pub fn markup(
    graph: &Graph,
    languages: &Languages,
    plugins: &[String],
    symlinks: &BTreeSet<String>,
    bodies: Option<&crate::extract::bodies::BodyLayer>,
    sites: Option<MarkupSites>,
    unparsed: &[UnparsedMarkup],
) -> Option<Markup> {
    if plugins.is_empty() && bodies.is_none() {
        return None;
    }
    let mut files = 0;
    let mut failed: Vec<FailedMarkup> = Vec::new();
    // Counted, and the failed files sorted below: no order of the graph's.
    for node in graph.nodes_unordered() {
        if node.kind != NodeKind::File {
            continue;
        }
        let path = node.key.as_str();
        if languages.name_for(path) != Some(MARKUP_LANGUAGE) || symlinks.contains(path) {
            continue;
        }
        files += 1;
        if let Some(f) = node
            .facts
            .iter()
            .find(|f| f.kind == FactKind::ExtractionFailed)
        {
            failed.push(FailedMarkup {
                path: path.to_string(),
                plugin: f.provenance.extractor.to_string(),
                pass: "first-layer".to_string(),
                reason: match &f.value {
                    FactValue::Text(t) => t.clone(),
                    other => format!("{other:?}"),
                },
            });
        }
    }
    let mut without_bodies = Vec::new();
    if let Some(layer) = bodies {
        for (path, f) in layer.failures() {
            if languages.name_for(path) == Some(MARKUP_LANGUAGE) {
                failed.push(FailedMarkup {
                    path: path.to_string(),
                    plugin: f.plugin.clone(),
                    pass: "bodies".to_string(),
                    reason: f.reason.clone(),
                });
            }
        }
        let readers: BTreeSet<&str> = layer
            .readers()
            .iter()
            .filter(|r| r.languages.iter().any(|l| l == MARKUP_LANGUAGE))
            .map(|r| r.id.as_str())
            .collect();
        without_bodies = plugins
            .iter()
            .filter(|p| !readers.contains(p.as_str()))
            .cloned()
            .collect();
    }
    failed.sort_by(|a, b| {
        (&a.path, &a.pass, &a.plugin, &a.reason).cmp(&(&b.path, &b.pass, &b.plugin, &b.reason))
    });
    let mut unparsed: Vec<UnparsedMarkup> = if bodies.is_some() {
        unparsed.to_vec()
    } else {
        Vec::new()
    };
    unparsed.sort_by(|a, b| a.path.cmp(&b.path));
    Some(Markup {
        state: markup_state(
            bodies.is_some(),
            !plugins.is_empty(),
            !failed.is_empty(),
            !without_bodies.is_empty(),
            !unparsed.is_empty(),
        ),
        plugins: plugins.to_vec(),
        files,
        failed,
        without_bodies,
        unparsed,
        sites: bodies.and(sites),
    })
}
