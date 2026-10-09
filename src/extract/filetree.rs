//! The filetree extractor (U3): the universal, grammar-free structural source.
//!
//! Enumerates tracked files from git (KTD10) and maps the directory hierarchy
//! into the graph — directories become [`NodeKind::Module`] nodes, files become
//! [`NodeKind::File`] nodes, and parent→child relations become
//! [`EdgeKind::Contains`] edges. A language with nothing else degrades to this
//! plus git (E4).

use super::ids;
use super::{parent_key, ExtractError, Extractor, Repo};
use crate::graph::{
    Edge, EdgeKind, ExtractorId, FactKind, Graph, NodeKey, NodeKind, Provenance, Tier,
};
use std::collections::BTreeSet;

/// The root module key — the parent of all top-level entries.
const ROOT: &str = ".";

pub struct FileTree;

impl Extractor for FileTree {
    fn id(&self) -> ExtractorId {
        ids::filetree()
    }

    fn emitted_kinds(&self) -> &'static [FactKind] {
        // filetree emits structure (nodes + Contains edges), not facts.
        &[]
    }

    fn trust_rank(&self) -> u8 {
        10
    }

    fn run(
        &self,
        repo: &Repo,
        graph: &mut Graph,
        _progress: &dyn crate::progress::Progress,
    ) -> Result<(), ExtractError> {
        // Gitlinks are already excluded by the enumeration (the single choke
        // point in tracked_files_with_paths), so a submodule never surfaces as a
        // File node — the contract is pinned by tests/gitlink.rs. Symlinks DO
        // become File nodes (they are real tracked paths with git history); only
        // their content extraction is skipped downstream.
        let files: Vec<String> = repo.tracked_files()?;

        // Collect every ancestor directory of every file (the module set). A
        // directory whose only entry is a gitlink is therefore never created —
        // no phantom empty module.
        let mut modules: BTreeSet<String> = BTreeSet::new();
        modules.insert(ROOT.to_string());
        for file in &files {
            for dir in ancestor_dirs(file) {
                modules.insert(dir);
            }
        }

        // Modules first, then their Contains edges — each emitted exactly once.
        // filetree owns the file/module key-space, so a kind collision here is
        // structurally impossible; still handled (skip + note) rather than
        // ignored, so the IR is never corrupted (E4 idiom).
        for module in &modules {
            if let Err(e) = graph.ensure_node(NodeKey::new(module.clone()), NodeKind::Module) {
                eprintln!("filetree: {e}");
            }
        }
        for module in &modules {
            if module == ROOT {
                continue;
            }
            let parent = NodeKey::new(parent_key(module));
            let child = NodeKey::new(module.clone());
            graph.add_edge(&parent, &child, contains(module));
        }

        // Files and their Contains edges.
        for file in &files {
            let key = NodeKey::new(file.clone());
            if let Err(e) = graph.ensure_node(key.clone(), NodeKind::File) {
                eprintln!("filetree: {e}");
                continue;
            }
            let parent = NodeKey::new(parent_key(file));
            graph.add_edge(&parent, &key, contains(file));
        }

        Ok(())
    }
}

/// Every ancestor directory of a `/`-normalized file path, outermost first
/// (the file name itself is excluded). `README.md` → none; `a/b/c.rs` → `a`, `a/b`.
fn ancestor_dirs(file: &str) -> Vec<String> {
    let parts: Vec<&str> = file.split('/').collect();
    (1..parts.len()).map(|i| parts[..i].join("/")).collect()
}

fn contains(entity: &str) -> Edge {
    Edge::new(
        EdgeKind::Contains,
        Provenance::new(entity, None, ids::filetree(), Tier::Measured),
    )
}
