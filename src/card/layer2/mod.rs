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
//! - [`triggers`] — step 2b, U5: the triggers of every functionality by the
//!   walk back over the callers, each row with its kind, key, place and state
//!   (KTD5, R11–R13), [`Analysis::triggers`].
//! - [`context`] — step 2b, U5: the flow of every row at the functionality
//!   it leads to, the part every flow entering at the same members shares
//!   written once, the command class's one flow (KTD7, R1, R2, R7, R9),
//!   [`TypeAnalysis::context`].
//! - [`dynamic`] — step 2c, U6 and U7: the dynamic-call markers of the run —
//!   every member a binding, a static member, an attached property, a setter
//!   or a handler of markup, a literal or a `nameof` of C# reads, writes,
//!   calls or names, with place and grade, the matches by name alone, and the
//!   references from C# to markup (KTD7, KTD8, KTD10), [`Analysis::dynamic`];
//!   [`literal_refs`] reads the C# half. Step 2c, U8 and U9: the words of a
//!   declaration (R17), the lines of every analyzed type's `## Dynamic
//!   access` with the run's state of XAML coverage ([`TypeAnalysis::dynamic`])
//!   and the triggers of markup for the registry (R9, KTD15).
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
pub mod context;
pub mod dynamic;
pub mod effects;
pub mod entry;
pub mod flow;
pub mod link;
pub mod literal_refs;
pub mod resolve;
pub mod triggers;

pub use binding::{
    bindings_of, field_call_arity, raised_events, AmbiguousField, BoundSlot, DelegateBinding,
    DelegatePair, IdleInvoker, RaisedEvent, SiteBinding,
};
pub use callers::{BodySite, Caller, CallerClass, Callers, Evidence, Subscriber, Via};
pub use context::{
    CommandClass, CommonPart, ContextPrecondition, FlowStep, FlowStepKind, FunctionalityContext,
    TriggerCondition, TriggerFlow, TypeContext,
};
pub use dynamic::{
    DynamicAccess, GenericMark, GenericMarks, Mark, MarkKind, MarkUse, MarkupSite, MarkupTrigger,
    MarkupTriggerKind, MemberLine, MemberMarks, SiteOutcome, TypeDynamic, Unpaired, XamlRef,
    XamlRefKind, VIEW_MODEL_FIELD,
};
pub use effects::{
    FlowResult, FlowResults, FunctionalityResults, ResultKind, StateContext, StopCause,
    StoppedPath, Touch, TypeResults,
};
pub use link::{Basis, Candidates, GradedLink, Links, Outcome, Resolution, Site};
pub use triggers::{
    DynamicMarks, FunctionalityRef, Hop, Narrowed, NoMarks, PathState, Trigger, TriggerKind,
    TriggerRow, WalkCounts, NEAREST_RATIO,
};

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
use crate::config::DEFAULT_AMBIGUITY_CEILING;
use crate::coverage::MarkupState;
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
    /// Step 2b: the results and the state from outside (U4) and the flow of
    /// every trigger at each functionality (U5) — for the `### Context` of
    /// the card (U6).
    pub context: TypeContext,
    /// Step 2c: what the `## Dynamic access` of the card draws (U8) — the
    /// lines of the type's members and the run's state of XAML coverage.
    /// Empty from [`analyze`] and [`analyze_with`]; [`analyze_run`] fills
    /// the lines, and [`Analysis::set_markup_state`] the state.
    pub dynamic: TypeDynamic,
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
    /// Step 2b: the registry of triggers — every trigger and functionality
    /// it leads to, by kind, key, place and functionality (R11, U5); what
    /// `cards/triggers.md` lists (U7). A [`TriggerFlow::row`] is an index
    /// into it.
    pub triggers: Vec<TriggerRow>,
    /// What the walk back gave the run, and whether it was cut at the
    /// nearest functionality ([`triggers::NEAREST_RATIO`]): what the registry
    /// and coverage admit. Not serialized: a count, not a finding.
    #[serde(skip)]
    pub walk: WalkCounts,
    /// Step 2c: the dynamic-call markers of the run (U6, U7) — what the
    /// `## Dynamic access` of a card (U8) and the registry's rows from markup
    /// (U9) read. Empty from [`analyze`] and [`analyze_with`]; [`analyze_run`]
    /// builds it.
    pub dynamic: DynamicAccess,
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

    /// The run's state of XAML coverage (plan 2c, U8: R19), which every
    /// analyzed type's `## Dynamic access` ends with: known only once the
    /// run's plugins and second layer are, after the analysis.
    pub fn set_markup_state(&mut self, state: MarkupState) {
        for t in &mut self.types {
            t.dynamic.state = state;
        }
    }

    /// The rows of the registry that lead to the functionality `key` of the
    /// type `ty`, in the registry's order.
    pub fn triggers_of<'s>(
        &'s self,
        ty: &'s NodeKey,
        key: &'s str,
    ) -> impl Iterator<Item = &'s TriggerRow> {
        self.triggers
            .iter()
            .filter(move |r| &r.target.ty == ty && r.target.key == key)
    }
}

/// Analyze every C# type of `graph` whose second layer `layer` holds in full,
/// at the default ambiguity ceiling and without the markers of step 2c.
pub fn analyze(graph: &Graph, layer: &BodyLayer) -> Analysis {
    analyze_with(graph, layer, DEFAULT_AMBIGUITY_CEILING, &NoMarks)
}

/// [`analyze`] with the run's ambiguity ceiling (the candidates of a link
/// that stopped, KTD9) and the dynamic-call markers of step 2c (`marks`,
/// [`NoMarks`] without it). Step 2a reads every type first; then the index
/// of callers, the results and the triggers are built once over the whole
/// run, because a trigger in one type leads to a functionality of another.
pub fn analyze_with(
    graph: &Graph,
    layer: &BodyLayer,
    ceiling: usize,
    marks: &dyn DynamicMarks,
) -> Analysis {
    let ix = Index::new(graph, layer);
    // One walk for 2a's entry points and 2b's links: the external index's
    // relation at an access first (R-25).
    let links = Links::new(&ix, ceiling);
    let (types, triggers, walk) = analyze_in(graph, layer, &links, marks);
    Analysis {
        types,
        triggers,
        walk,
        dynamic: DynamicAccess::default(),
    }
}

/// The analysis of a run with step 2c: the dynamic-call markers
/// ([`DynamicAccess`]) are built first over the same index and walk as the
/// rest, and are the markers the walk of triggers reads (a member marked
/// from markup is no entry from another component) — then kept on the
/// analysis for the card and the registry. What the run renders from.
pub fn analyze_run(graph: &Graph, layer: &BodyLayer, ceiling: usize) -> Analysis {
    let ix = Index::new(graph, layer);
    let links = Links::new(&ix, ceiling);
    let dynamic = DynamicAccess::build(graph, &links);
    let (mut types, triggers, walk) = analyze_in(graph, layer, &links, &dynamic);
    for t in &mut types {
        t.dynamic.lines = dynamic.lines_of(&t.key);
    }
    Analysis {
        types,
        triggers,
        walk,
        dynamic,
    }
}

/// Step 2a's analyses of every type, then step 2b's once over the run, over
/// one index and walk.
fn analyze_in(
    graph: &Graph,
    layer: &BodyLayer,
    links: &Links<'_, '_>,
    marks: &dyn DynamicMarks,
) -> (Vec<TypeAnalysis>, Vec<TriggerRow>, WalkCounts) {
    let ix = links.index();
    // Over the whole graph, every language's types: a comment is copied from
    // wherever it was written first.
    let copied = copied_comments(graph);
    let mut types: Vec<TypeAnalysis> = ix
        .csharp_types()
        .into_iter()
        .filter(|ty| {
            ix.symbol(ty)
                .is_some_and(|s| ANALYZED_KINDS.contains(&s.kind.as_str()))
                && layer.layer_of(ty) == SecondLayer::Present
        })
        .map(|ty| {
            let mut seeds = entry::seeds(links, ty);
            let unbound_commands = std::mem::take(&mut seeds.unbound);
            let (functionalities, hubs) = cluster::cluster(ix, ty, seeds);
            let flow = flow::analyze(ix, ty, &functionalities);
            let purpose = CodePurpose::of(&functionalities, &flow.states, copied.contains(ty));
            TypeAnalysis {
                key: ty.clone(),
                functionalities,
                hubs,
                unbound_commands,
                flow,
                purpose,
                context: TypeContext::empty(ty),
                dynamic: TypeDynamic::default(),
            }
        })
        .collect();

    // Step 2b, once a run.
    let callers = Callers::new(graph, links);
    let results: Vec<TypeResults> = {
        let flow_results = FlowResults::new(graph, links, &callers);
        types.iter().map(|t| flow_results.of_type(t)).collect()
    };
    let triggers::Found {
        rows: mut triggers,
        narrowed,
        walk,
    } = triggers::find(&types, links, &callers, marks);
    // The index of callers is the largest structure of step 2b: what the
    // composition still reads of it is taken, and it is dropped before the
    // flows are composed.
    let facts = context::CallerFacts::new(&types, &callers, ix);
    drop(callers);
    let contexts = context::compose(&types, results, &mut triggers, links, facts, narrowed);
    for (t, c) in types.iter_mut().zip(contexts) {
        t.context = c;
    }
    (types, triggers, walk)
}
