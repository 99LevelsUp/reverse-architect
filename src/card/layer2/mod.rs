//! The second layer's analyses over the model of C# bodies (plan 2a).
//!
//! U5 builds the first: the **core functionalities** of every C# type the
//! second layer read in full ([`SecondLayer::Present`]) — each with its key,
//! its name and its entry points, the targets of commands and of members
//! inherited from a generic base included. Nothing here renders; a later
//! unit puts it on the card.
//!
//! - [`resolve`] — the type of an access by its receiver chain (R17, KTD4),
//!   [`Index::resolve`]; one implementation step 2b generalizes.
//! - [`entry`] — the entry points and the command binding,
//!   [`bind_construction`]; one implementation step 2b generalizes to
//!   delegates and events.
//! - [`cluster`] — clusters, hubs, the representative and the name (KTD11,
//!   KTD12, R16).
//!
//! The analysis is deterministic (R3): it reads the graph and the model in
//! key and path order, keeps every collection ordered, and never iterates a
//! hash map; how many threads the run that built the model had changes no
//! byte of it.

pub mod cluster;
pub mod entry;
pub mod resolve;

pub use cluster::{Functionality, FunctionalityName};
pub use entry::{
    bind_construction, Binding, BoundDelegate, CommandSite, EntryKind, EntryPoint, Label,
    MenuEntry, Place, Region, Slot, SubMenu, SubscriptionSite, Target, UnboundCommand,
};
pub use resolve::{Chain, Index, Link, Scope, Unresolved};

use crate::extract::bodies::{BodyLayer, SecondLayer};
use crate::graph::{Graph, NodeKey};
use serde::Serialize;

/// The symbol kinds the analysis reads functionalities of.
const ANALYZED_KINDS: &[&str] = &["class", "struct", "record"];

/// The analysis of one type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TypeAnalysis {
    pub key: NodeKey,
    /// By key.
    pub functionalities: Vec<Functionality>,
    /// The members no cluster is joined through (KTD12 step 4), by key.
    pub hubs: Vec<NodeKey>,
    /// Constructions that would be commands but whose binding cannot be
    /// told (admitted), by place.
    pub unbound_commands: Vec<UnboundCommand>,
}

impl TypeAnalysis {
    /// The functionality with `key`.
    pub fn functionality(&self, key: &str) -> Option<&Functionality> {
        self.functionalities
            .binary_search_by(|f| f.key.as_str().cmp(key))
            .ok()
            .map(|i| &self.functionalities[i])
    }

    /// The functionality an entry point with `key` belongs to.
    pub fn functionality_of(&self, entry: &str) -> Option<&Functionality> {
        self.functionalities
            .iter()
            .find(|f| f.entry_points.iter().any(|e| e.key == entry))
    }
}

/// The analysis of a run's second layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Analysis {
    /// Every C# class, struct and record with its second layer, by key.
    pub types: Vec<TypeAnalysis>,
}

impl Analysis {
    /// The analysis of the type `key`; `None` for a type without its second
    /// layer or of another language.
    pub fn of(&self, key: &NodeKey) -> Option<&TypeAnalysis> {
        self.types
            .binary_search_by(|t| t.key.cmp(key))
            .ok()
            .map(|i| &self.types[i])
    }
}

/// Analyze every C# type of `graph` whose second layer `layer` holds in full.
pub fn analyze(graph: &Graph, layer: &BodyLayer) -> Analysis {
    let ix = Index::new(graph, layer);
    let types = ix
        .csharp_types()
        .into_iter()
        .filter(|ty| {
            ix.symbol(ty)
                .is_some_and(|s| ANALYZED_KINDS.contains(&s.kind.as_str()))
                && layer.layer_of(ty) == SecondLayer::Present
        })
        .map(|ty| {
            let mut seeds = entry::seeds(&ix, ty);
            let unbound_commands = std::mem::take(&mut seeds.unbound);
            let (functionalities, hubs) = cluster::cluster(&ix, ty, seeds);
            TypeAnalysis {
                key: ty.clone(),
                functionalities,
                hubs,
                unbound_commands,
            }
        })
        .collect();
    Analysis { types }
}
