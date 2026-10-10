//! The second layer's analyses over the model of C# bodies (plan 2a).
//!
//! U5 builds the first: the **core functionalities** of every C# type the
//! second layer read in full ([`SecondLayer::Present`]) — each with its key,
//! its name and its entry points, the targets of commands and of members
//! inherited from a generic base included. Nothing here renders; the type's
//! card draws it (`card::layer2_render`, U6).
//!
//! - [`resolve`] — the type of an access by its receiver chain (R17, KTD4),
//!   [`Index::resolve`]; one implementation step 2b generalizes.
//! - [`entry`] — the entry points and the command binding,
//!   [`bind_construction`]; one implementation step 2b generalizes to
//!   delegates and events.
//! - [`link`] — step 2b, U2: every access of a body resolved by the same
//!   walk, each link graded, the external index first, the candidates of a
//!   stopped access cut at the ambiguity ceiling (KTD3, KTD9), [`Links`].
//! - [`binding`] — step 2b, U3: the delegate binding as (method of the
//!   class, target) pairs, for every class of the shape, and the events a
//!   class raises (KTD4, KTD6), [`bindings_of`].
//! - [`callers`] — step 2b, U1: the reverse index of callers of every member
//!   and its class (none, weak only, strong), built once a run beside the
//!   model of bodies (KTD2), [`Callers`].
//! - [`effects`] — step 2b, U4: the results of every functionality in the
//!   order of how sure they are, the paths that stop before the main work,
//!   and the state read and written from outside (KTD6, R5, R6, R10),
//!   [`FlowResults`].
//! - [`cluster`] — clusters, hubs, the representative and the name (KTD11,
//!   KTD12, R16).
//! - [`flow`] — U11: the preconditions and their effects, the dead paths,
//!   the steps and the mentions of every functionality, and the states of the
//!   type (KTD13, KTD5, R6, R7, R9), [`TypeAnalysis::flow`].
//! - U7: what the code says the type is for, and whether its doc comment is
//!   probably another type's (R12, R13, KTD15), [`TypeAnalysis::purpose`] —
//!   see [`crate::card::purpose`].
//!
//! The analysis is deterministic (R3): it reads the graph and the model in
//! key and path order, keeps every collection ordered, and never iterates a
//! hash map; how many threads the run that built the model had changes no
//! byte of it.

pub mod binding;
pub mod callers;
pub mod cluster;
pub mod effects;
pub mod entry;
pub mod flow;
pub mod link;
pub mod resolve;

pub use binding::{
    bindings_of, field_call_arity, raised_events, AmbiguousField, BoundSlot, DelegateBinding,
    DelegatePair, IdleInvoker, RaisedEvent, SiteBinding,
};
pub use callers::{BodySite, Caller, CallerClass, Callers, Evidence, Subscriber, Via};
pub use effects::{
    FlowResult, FlowResults, FunctionalityResults, ResultKind, StateContext, StopCause,
    StoppedPath, Touch, TypeResults,
};
pub use link::{Basis, Candidates, GradedLink, Links, Outcome, Resolution, Site};

pub use cluster::{Functionality, FunctionalityName};
pub use entry::{
    bind_construction, Binding, BoundDelegate, CommandSite, EntryKind, EntryPoint, Label,
    MenuEntry, Place, Region, Slot, SubMenu, SubscriptionSite, Target, UnboundCommand,
};
pub use flow::{
    AdmittedPrecondition, ArmStep, Cond, ConfigKey, DeadKind, DeadPath, Effect, EffectKind,
    FunctionalityFlow, Gap, Guarding, Mentions, Occurrence, Precondition, Source, State,
    StateChange, StateValue, Step, StepKind, TypeFlow, Written,
};
pub use resolve::{ByArity, Chain, Index, Link, Scope, Unresolved};

use crate::card::purpose::{copied_comments, CodePurpose};
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
    /// The preconditions, dead paths, steps and mentions of each
    /// functionality, and the type's states (U11).
    pub flow: TypeFlow,
    /// What the code says the type is for (U7) — read only by the card with
    /// the second layer, never by the first layer's purpose (KTD9).
    pub purpose: CodePurpose,
}

impl TypeAnalysis {
    /// The functionality with `key`.
    pub fn functionality(&self, key: &str) -> Option<&Functionality> {
        self.functionalities
            .binary_search_by(|f| f.key.as_str().cmp(key))
            .ok()
            .map(|i| &self.functionalities[i])
    }

    /// What U11 read of the functionality `key`.
    pub fn flow_of(&self, key: &str) -> Option<&FunctionalityFlow> {
        self.flow.of(key)
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
    // Over the whole graph, every language's types: a comment is copied from
    // wherever it was written first.
    let copied = copied_comments(graph);
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
            let flow = flow::analyze(&ix, ty, &functionalities);
            let purpose = CodePurpose::of(&functionalities, &flow.states, copied.contains(ty));
            TypeAnalysis {
                key: ty.clone(),
                functionalities,
                hubs,
                unbound_commands,
                flow,
                purpose,
            }
        })
        .collect();
    Analysis { types }
}
