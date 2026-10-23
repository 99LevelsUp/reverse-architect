//! The second layer of a run: the bodies of the members (plan 2a, U4).
//!
//! A run with `--detail-level 1|2|3` makes a third pass over every file a
//! plugin reads bodies of — the `bodies` pass of contract 0.3 — and keeps
//! what comes back as a **model of bodies per file** beside the graph, never
//! as facts of it (KTD6). Later units read the model (analyses, cards); this
//! one reads it in, stores it, pairs it with the first layer and hands it on
//! as a [`BodyLayer`].
//!
//! # What the pass does, in order
//!
//! 1. **Routes** each file to the first plugin, in the run's precedence
//!    order, that claims its extension and serves `bodies`, then to the
//!    bundled body components ([`crate::plugin::bundled::BODY_COMPONENTS`]) — the
//!    component only a run with the second layer loads (KTD7), kept out of
//!    the run's plugin set so it is in no cache fingerprint ([`BodyPass`]).
//! 2. **Reads** the file through the plugin's sandbox, one session per worker
//!    from the same pool the first layer uses.
//! 3. **Redacts** every text of the model ([`redact_bodies`]): the first
//!    layer's decisions about the file's literals, carried to the model's
//!    literals by place, and the run's secret prefixes over every text —
//!    before anything else sees it, so no unredacted text reaches the cache
//!    or an output (KTD16). The block comment written right after a call
//!    argument is attached to its literal, filtered the same way
//!    ([`quote_comments`], U7: R18).
//! 4. **Stores** the redacted model per file in its own cache phase,
//!    [`PHASE_BODIES`] (KTD8). Each record is tagged with the digest of the
//!    component that made it and with [`BODY_REDACTION_VERSION`]
//!    ([`CacheStore::derived`]), so a rebuilt component or a changed
//!    redaction misses exactly these records and the first layer's stay
//!    warm. A file that fails is never stored.
//! 5. **Records a failure** as [`FactKind::BodyExtractionFailed`] on the file
//!    — detail-only, the first layer's `ExtractionFailed` is never used —
//!    and the file loses its second layer alone (KTD8).
//! 6. **Pairs** every member body, type shape, typed member and declaration
//!    meta with the first layer's declarations by file and by the position of
//!    the declared name ([`pair`]). A body whose name no declaration has —
//!    an accessor's, a local function's or a static constructor's, when the
//!    first layer has no symbol for it — goes to the nearest declaration that
//!    encloses it. A body neither finds is kept and counted, never dropped.
//!
//! # Where the pass runs, and why there
//!
//! In [`crate::build_graph`], after every first-layer extractor and before
//! the merge — not as a third phase inside the languages extractor. The
//! model has to leave the extractor chain, which an [`super::Extractor`]
//! cannot hand anything out of, and the failure facts must be in the graph
//! before the merge sorts its facts. The declaration-name positions the
//! pairing needs, which the languages extractor used to drop after its usage
//! pass, reach it through a [`super::treesitter::NamesHandoff`], the way the
//! project-wide usings reach the external extractor.
//!
//! # The model's types
//!
//! The host's own serde types, named after the records of `wit-v3`, compact
//! on disk: positions are pairs, ranges quadruples, and every empty list and
//! absent option is left out. The `wasmtime` bindings
//! ([`crate::plugin::host`]) are converted at the boundary and go no further.

use super::cache::CacheStore;
use super::external::{
    comments_after_literals, is_long_token, named_secret, redact_secret_block, secret_pattern,
    without_long_tokens, SecretPrefixes, SourceSecrets, REDACTION_MARKER,
};
use super::resolve::table::NamePositions;
use super::treesitter::{Lease, PathKeys, Pool};
use super::walk::{walk_files, WalkOutput};
use super::{ExtractError, Repo};
use crate::graph::{ExtractorId, Fact, FactKind, FactValue, Graph, NodeKey, Provenance, Tier};
use crate::plugin::host as w;
use crate::plugin::{Pass, Plugin, PluginSet};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

/// The cache phase of the `bodies` pass (KTD8).
pub const PHASE_BODIES: &str = "languages-bodies";

/// The version of what the host does to a model before it stores it — today
/// [`redact_bodies`]. Part of every `languages-bodies` record's tag
/// ([`CacheStore::derived`]), so raising it misses every stored model and
/// nothing of the first layer. Any change to the redaction or to the
/// conversion of a model raises it: 7 decides a key-shaped markup handler by
/// its digits rather than by its last segment ([`key_shaped_handler`]), 6
/// withholds a key-shaped handler of a markup body that does not end in a
/// word, 5
/// decides the handler a markup body
/// subscribes like a literal and filters every subscription's texts (plan 2c,
/// review), 4 cuts key-shaped words out of a quoted
/// comment and out of a literal no lexed one pairs with, 3 adds the comments
/// after call arguments ([`quote_comments`], U7), 2 is the literal-aware
/// filter (U8, KTD16), 1 the first layer's filter alone.
pub const BODY_REDACTION_VERSION: u32 = 7;

/// The digits a key-shaped markup handler needs before it is taken for a
/// token ([`key_shaped_handler`]).
const HANDLER_TOKEN_DIGITS: usize = 3;

/// Whether a markup handler's name is a random token the plugin took for a
/// method's name: key-shaped as a whole ([`is_long_token`]) and holding at
/// least [`HANDLER_TOKEN_DIGITS`] digits. A designer's names — a control's
/// name and its event (`comboBox1_SelectedIndexChanged`), a duplicate
/// (`textBox1_TextChanged_1`), a long camel-case name (`OnTab2SelectionChanged`)
/// — are words with a digit or two and stay; a random token has digits
/// spread through it wherever its underscores fall.
fn key_shaped_handler(name: &str) -> bool {
    is_long_token(name) && name.bytes().filter(u8::is_ascii_digit).count() >= HANDLER_TOKEN_DIGITS
}

/// The detail levels `--detail-level` accepts. The level is a choice of
/// rendering (KTD7): it decides what later units show, not what this pass
/// reads, so it is in no cache record and no fingerprint.
pub const DETAIL_LEVELS: std::ops::RangeInclusive<u8> = 1..=3;

// ---- The model ----

/// A point in a file (`position`): 1-based line, and 1-based column counted
/// in characters, the unit the first layer's name positions use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(from = "(u32, u32)", into = "(u32, u32)")]
pub struct Position {
    pub line: u32,
    pub column: u32,
}

impl From<(u32, u32)> for Position {
    fn from((line, column): (u32, u32)) -> Position {
        Position { line, column }
    }
}

impl From<Position> for (u32, u32) {
    fn from(p: Position) -> (u32, u32) {
        (p.line, p.column)
    }
}

/// A stretch of source, both ends inclusive (`range`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(from = "[u32; 4]", into = "[u32; 4]")]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

impl From<[u32; 4]> for Range {
    fn from([l1, c1, l2, c2]: [u32; 4]) -> Range {
        Range {
            start: Position {
                line: l1,
                column: c1,
            },
            end: Position {
                line: l2,
                column: c2,
            },
        }
    }
}

impl From<Range> for [u32; 4] {
    fn from(r: Range) -> [u32; 4] {
        [r.start.line, r.start.column, r.end.line, r.end.column]
    }
}

/// The first name of an access path (`path-root`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PathRoot {
    This,
    Base,
    Local,
    Name,
    Expression,
}

/// A dotted path of names, as written (`access-path`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AccessPath {
    pub root: PathRoot,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub segments: Vec<String>,
}

/// A value that is not a string literal (`constant`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Constant {
    Boolean(bool),
    Null,
    Number(String),
}

/// Where a value comes from (`value-ref`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ValueRef {
    Param(u32),
    Access(AccessPath),
    Lambda(u32),
    Literal(u32),
    Construction(u32),
    Constant(Constant),
    Other,
}

/// `access-mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AccessMode {
    Read,
    Write,
    Invoke,
    Return,
    Mutate,
}

/// One read, write, call or return of a member, a local or a parameter
/// (`member-access`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberAccess {
    pub target: AccessPath,
    pub mode: AccessMode,
    pub place: Range,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rhs: Option<ValueRef>,
}

/// `call-kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CallKind {
    Call,
    Construct,
    ThisInit,
    BaseInit,
}

/// One argument written at a call (`argument`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Argument {
    pub position: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub value: ValueRef,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub enum_refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reads: Vec<AccessPath>,
}

/// One call or construction (`invocation`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invocation {
    pub callee: AccessPath,
    pub kind: CallKind,
    pub arity: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<Argument>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub conditional: bool,
    pub place: Range,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow: Option<FlowTarget>,
}

/// Where a value flows (`flow-target`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FlowTarget {
    Member(String),
    MemberReturn,
    Local(String),
    Path(AccessPath),
    /// The invocation's id and the argument position.
    Argument(u32, u32),
}

/// A lambda, anonymous method or local function (`lambda`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lambda {
    pub id: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub argument_of: Option<(u32, u32)>,
    pub arity: u32,
    pub place: Range,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow: Option<FlowTarget>,
    pub body: u32,
}

/// `subscription-mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SubscriptionMode {
    Add,
    Remove,
}

/// `handler`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Handler {
    MethodGroup(AccessPath),
    Lambda(u32),
    Other,
}

/// `source.Event += handler` and `-=` (`subscription`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subscription {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<AccessPath>,
    pub event: String,
    pub handler: Handler,
    pub mode: SubscriptionMode,
    pub place: Range,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<u32>,
}

/// Where a string literal is written (`literal-context`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LiteralContext {
    /// The callee path and the argument position.
    CallArg(AccessPath, u32),
    /// The attribute's name, the argument position, the parameter name.
    AttributeArg(String, Option<u32>, Option<String>),
    Compared(AccessPath),
    Assigned(AccessPath),
    Returned,
    Other,
}

/// `string-literal`. `value` is redacted ([`redact_bodies`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StringLiteral {
    pub id: u32,
    pub value: String,
    pub context: LiteralContext,
    pub place: Range,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<u32>,
    /// The block comment written right after the literal or the call it
    /// closes (plan 2a, U7: R18) — the host's, read from the lexer
    /// ([`quote_comments`]); no plugin reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<LiteralComment>,
}

/// The block comment after a call argument
/// (`GetResourceString("EditorView_checkErrors") /*"Kontrola chyb"*/`): its
/// text without the comment's framing and quotes, redacted, and where the
/// comment is written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiteralComment {
    pub text: String,
    pub place: Range,
}

/// `nameof(x)` (`name-of`). `text` is redacted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NameOf {
    pub text: String,
    pub context: LiteralContext,
    pub place: Range,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<u32>,
}

/// A tag on a node, a body or a declaration (`tag`). `value` and `args` are
/// redacted.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Tag {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
}

/// `compare-op`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CompareOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// One node of a condition's expression tree (`expr`); operands are indices
/// into the same list, before the node that uses them.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Expr {
    And(Vec<u32>),
    Or(Vec<u32>),
    Not(u32),
    Compare(CompareOp, u32, u32),
    Path(AccessPath),
    EnumMember(String),
    Constant(Constant),
    Literal(u32),
    Call(AccessPath, Vec<u32>),
    Binary(String, u32, u32),
    /// Redacted.
    Other(String),
}

/// A condition (`condition`). `text` is redacted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Condition {
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expr: Vec<Expr>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub enum_refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reads: Vec<AccessPath>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub constant: Option<bool>,
}

/// The kind of a body node, normalized by logic (`node-kind`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NodeKind {
    Body,
    Block,
    Choice,
    Arm,
    Loop,
    Try,
    Catch,
    Finally,
    Return,
    Throw,
    YieldBreak,
    YieldReturn,
    Break,
    Continue,
    Statement,
    Lambda,
    Element(String),
    Other(String),
}

/// `body-node`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BodyNode {
    pub id: u32,
    pub kind: NodeKind,
    pub place: Range,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<Condition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub returns: Option<Constant>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<Tag>,
}

/// The kind of declaration a body belongs to (`body-kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BodyKind {
    Method,
    Constructor,
    TypeInitializer,
    Getter,
    Setter,
    Initializer,
    Adder,
    Remover,
    ExpressionProperty,
    MemberInitializer,
    LocalFunction,
    Markup,
    Other,
}

impl BodyKind {
    /// The word the contract spells the kind with (`type-initializer`).
    pub fn word(self) -> &'static str {
        match self {
            BodyKind::Method => "method",
            BodyKind::Constructor => "constructor",
            BodyKind::TypeInitializer => "type-initializer",
            BodyKind::Getter => "getter",
            BodyKind::Setter => "setter",
            BodyKind::Initializer => "initializer",
            BodyKind::Adder => "adder",
            BodyKind::Remover => "remover",
            BodyKind::ExpressionProperty => "expression-property",
            BodyKind::MemberInitializer => "member-initializer",
            BodyKind::LocalFunction => "local-function",
            BodyKind::Markup => "markup",
            BodyKind::Other => "other",
        }
    }
}

/// `declaration-kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeclarationKind {
    Field,
    Property,
    Local,
    Parameter,
    Pattern,
    Event,
}

/// A name declared with a type (`typed-name`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypedName {
    pub name: String,
    pub type_name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub type_args: Vec<String>,
    pub kind: DeclarationKind,
    pub position: Position,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<u32>,
}

/// The body of one member (`member-body`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberBody {
    /// Where the name of the declaration it belongs to is written — what it
    /// is paired by.
    pub name_position: Position,
    pub kind: BodyKind,
    pub root: u32,
    /// A node before its children, ids equal to their index.
    pub nodes: Vec<BodyNode>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accesses: Vec<MemberAccess>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub invocations: Vec<Invocation>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lambdas: Vec<Lambda>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subscriptions: Vec<Subscription>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub literals: Vec<StringLiteral>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub name_ofs: Vec<NameOf>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub typed_names: Vec<TypedName>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<Tag>,
}

/// One entry of a type's base list (`base-type`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BaseType {
    pub base: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub type_args: Vec<String>,
}

/// A type declared in the file (`type-shape`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeShape {
    pub name_position: Position,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub type_params: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bases: Vec<BaseType>,
}

/// `attribute`. `args` are redacted.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Attribute {
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
}

/// What a declaration says beside its type and name (`declaration-meta`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeclarationMeta {
    pub name_position: Position,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modifiers: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attributes: Vec<Attribute>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<Tag>,
}

/// The model of one file's bodies (`file-bodies`), redacted.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileBodies {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub types: Vec<TypeShape>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<MemberBody>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub typed_members: Vec<TypedName>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub metas: Vec<DeclarationMeta>,
}

// ---- From the bindings ----

impl From<w::BodyPosition> for Position {
    fn from(p: w::BodyPosition) -> Position {
        Position {
            line: p.line,
            column: p.column,
        }
    }
}

impl From<w::BodyRange> for Range {
    fn from(r: w::BodyRange) -> Range {
        Range {
            start: r.start.into(),
            end: r.end.into(),
        }
    }
}

impl From<w::AccessPath> for AccessPath {
    fn from(p: w::AccessPath) -> AccessPath {
        AccessPath {
            root: match p.root {
                w::PathRoot::This => PathRoot::This,
                w::PathRoot::Base => PathRoot::Base,
                w::PathRoot::Local => PathRoot::Local,
                w::PathRoot::Name => PathRoot::Name,
                w::PathRoot::Expression => PathRoot::Expression,
            },
            segments: p.segments,
        }
    }
}

fn paths(paths: Vec<w::AccessPath>) -> Vec<AccessPath> {
    paths.into_iter().map(Into::into).collect()
}

impl From<w::BodyConstant> for Constant {
    fn from(c: w::BodyConstant) -> Constant {
        match c {
            w::BodyConstant::Boolean(b) => Constant::Boolean(b),
            w::BodyConstant::Null => Constant::Null,
            w::BodyConstant::Number(n) => Constant::Number(n),
        }
    }
}

impl From<w::ValueRef> for ValueRef {
    fn from(v: w::ValueRef) -> ValueRef {
        match v {
            w::ValueRef::Param(n) => ValueRef::Param(n),
            w::ValueRef::Access(p) => ValueRef::Access(p.into()),
            w::ValueRef::Lambda(n) => ValueRef::Lambda(n),
            w::ValueRef::Literal(n) => ValueRef::Literal(n),
            w::ValueRef::Construction(n) => ValueRef::Construction(n),
            w::ValueRef::Constant(c) => ValueRef::Constant(c.into()),
            w::ValueRef::Other => ValueRef::Other,
        }
    }
}

impl From<w::FlowTarget> for FlowTarget {
    fn from(f: w::FlowTarget) -> FlowTarget {
        match f {
            w::FlowTarget::Member(m) => FlowTarget::Member(m),
            w::FlowTarget::MemberReturn => FlowTarget::MemberReturn,
            w::FlowTarget::Local(l) => FlowTarget::Local(l),
            w::FlowTarget::Path(p) => FlowTarget::Path(p.into()),
            w::FlowTarget::Argument((invocation, position)) => {
                FlowTarget::Argument(invocation, position)
            }
        }
    }
}

impl From<w::LiteralContext> for LiteralContext {
    fn from(c: w::LiteralContext) -> LiteralContext {
        match c {
            w::LiteralContext::CallArg((callee, position)) => {
                LiteralContext::CallArg(callee.into(), position)
            }
            w::LiteralContext::AttributeArg((name, position, param)) => {
                LiteralContext::AttributeArg(name, position, param)
            }
            w::LiteralContext::Compared(p) => LiteralContext::Compared(p.into()),
            w::LiteralContext::Assigned(p) => LiteralContext::Assigned(p.into()),
            w::LiteralContext::Returned => LiteralContext::Returned,
            w::LiteralContext::Other => LiteralContext::Other,
        }
    }
}

impl From<w::BodyTag> for Tag {
    fn from(t: w::BodyTag) -> Tag {
        Tag {
            name: t.name,
            value: t.value,
            args: t.args,
        }
    }
}

fn tags(tags: Vec<w::BodyTag>) -> Vec<Tag> {
    tags.into_iter().map(Into::into).collect()
}

impl From<w::BodyExpr> for Expr {
    fn from(e: w::BodyExpr) -> Expr {
        match e {
            w::BodyExpr::And(operands) => Expr::And(operands),
            w::BodyExpr::Or(operands) => Expr::Or(operands),
            w::BodyExpr::Not(operand) => Expr::Not(operand),
            w::BodyExpr::Compare((op, a, b)) => Expr::Compare(
                match op {
                    w::CompareOp::Eq => CompareOp::Eq,
                    w::CompareOp::Ne => CompareOp::Ne,
                    w::CompareOp::Lt => CompareOp::Lt,
                    w::CompareOp::Le => CompareOp::Le,
                    w::CompareOp::Gt => CompareOp::Gt,
                    w::CompareOp::Ge => CompareOp::Ge,
                },
                a,
                b,
            ),
            w::BodyExpr::Path(p) => Expr::Path(p.into()),
            w::BodyExpr::EnumMember(m) => Expr::EnumMember(m),
            w::BodyExpr::Constant(c) => Expr::Constant(c.into()),
            w::BodyExpr::Literal(n) => Expr::Literal(n),
            w::BodyExpr::Call((callee, args)) => Expr::Call(callee.into(), args),
            w::BodyExpr::Binary((op, a, b)) => Expr::Binary(op, a, b),
            w::BodyExpr::Other(text) => Expr::Other(text),
        }
    }
}

impl From<w::BodyCondition> for Condition {
    fn from(c: w::BodyCondition) -> Condition {
        Condition {
            text: c.text,
            expr: c.expr.into_iter().map(Into::into).collect(),
            enum_refs: c.enum_refs,
            reads: paths(c.reads),
            constant: c.constant,
        }
    }
}

impl From<w::BodyNodeKind> for NodeKind {
    fn from(k: w::BodyNodeKind) -> NodeKind {
        match k {
            w::BodyNodeKind::Body => NodeKind::Body,
            w::BodyNodeKind::Block => NodeKind::Block,
            w::BodyNodeKind::Choice => NodeKind::Choice,
            w::BodyNodeKind::Arm => NodeKind::Arm,
            w::BodyNodeKind::Loop => NodeKind::Loop,
            w::BodyNodeKind::Try => NodeKind::Try,
            w::BodyNodeKind::Catch => NodeKind::Catch,
            w::BodyNodeKind::Finally => NodeKind::Finally,
            w::BodyNodeKind::Return => NodeKind::Return,
            w::BodyNodeKind::Throw => NodeKind::Throw,
            w::BodyNodeKind::YieldBreak => NodeKind::YieldBreak,
            w::BodyNodeKind::YieldReturn => NodeKind::YieldReturn,
            w::BodyNodeKind::Break => NodeKind::Break,
            w::BodyNodeKind::Continue => NodeKind::Continue,
            w::BodyNodeKind::Statement => NodeKind::Statement,
            w::BodyNodeKind::Lambda => NodeKind::Lambda,
            w::BodyNodeKind::Element(name) => NodeKind::Element(name),
            w::BodyNodeKind::Other(name) => NodeKind::Other(name),
        }
    }
}

impl From<w::BodyNode> for BodyNode {
    fn from(n: w::BodyNode) -> BodyNode {
        BodyNode {
            id: n.id,
            kind: n.kind.into(),
            place: n.place.into(),
            parent: n.parent,
            condition: n.condition.map(Into::into),
            returns: n.returns.map(Into::into),
            tags: tags(n.tags),
        }
    }
}

impl From<w::BodyKind> for BodyKind {
    fn from(k: w::BodyKind) -> BodyKind {
        match k {
            w::BodyKind::Method => BodyKind::Method,
            w::BodyKind::Constructor => BodyKind::Constructor,
            w::BodyKind::TypeInitializer => BodyKind::TypeInitializer,
            w::BodyKind::Getter => BodyKind::Getter,
            w::BodyKind::Setter => BodyKind::Setter,
            w::BodyKind::Initializer => BodyKind::Initializer,
            w::BodyKind::Adder => BodyKind::Adder,
            w::BodyKind::Remover => BodyKind::Remover,
            w::BodyKind::ExpressionProperty => BodyKind::ExpressionProperty,
            w::BodyKind::MemberInitializer => BodyKind::MemberInitializer,
            w::BodyKind::LocalFunction => BodyKind::LocalFunction,
            w::BodyKind::Markup => BodyKind::Markup,
            w::BodyKind::Other => BodyKind::Other,
        }
    }
}

impl From<w::TypedName> for TypedName {
    fn from(t: w::TypedName) -> TypedName {
        TypedName {
            name: t.name,
            type_name: t.type_name,
            type_args: t.type_args,
            kind: match t.kind {
                w::DeclarationKind::Field => DeclarationKind::Field,
                w::DeclarationKind::Property => DeclarationKind::Property,
                w::DeclarationKind::Local => DeclarationKind::Local,
                w::DeclarationKind::Parameter => DeclarationKind::Parameter,
                w::DeclarationKind::Pattern => DeclarationKind::Pattern,
                w::DeclarationKind::Event => DeclarationKind::Event,
            },
            position: t.position.into(),
            scope: t.scope,
        }
    }
}

fn typed_names(names: Vec<w::TypedName>) -> Vec<TypedName> {
    names.into_iter().map(Into::into).collect()
}

impl From<w::MemberAccess> for MemberAccess {
    fn from(a: w::MemberAccess) -> MemberAccess {
        MemberAccess {
            target: a.target.into(),
            mode: match a.mode {
                w::AccessMode::Read => AccessMode::Read,
                w::AccessMode::Write => AccessMode::Write,
                w::AccessMode::Invoke => AccessMode::Invoke,
                w::AccessMode::Return => AccessMode::Return,
                w::AccessMode::Mutate => AccessMode::Mutate,
            },
            place: a.place.into(),
            node: a.node,
            rhs: a.rhs.map(Into::into),
        }
    }
}

impl From<w::CallArgument> for Argument {
    fn from(a: w::CallArgument) -> Argument {
        Argument {
            position: a.position,
            name: a.name,
            value: a.value.into(),
            enum_refs: a.enum_refs,
            reads: paths(a.reads),
        }
    }
}

impl From<w::Invocation> for Invocation {
    fn from(i: w::Invocation) -> Invocation {
        Invocation {
            callee: i.callee.into(),
            kind: match i.kind {
                w::CallKind::Call => CallKind::Call,
                w::CallKind::Construct => CallKind::Construct,
                w::CallKind::ThisInit => CallKind::ThisInit,
                w::CallKind::BaseInit => CallKind::BaseInit,
            },
            arity: i.arity,
            args: i.args.into_iter().map(Into::into).collect(),
            conditional: i.conditional,
            place: i.place.into(),
            node: i.node,
            flow: i.flow.map(Into::into),
        }
    }
}

impl From<w::Lambda> for Lambda {
    fn from(l: w::Lambda) -> Lambda {
        Lambda {
            id: l.id,
            argument_of: l.argument_of,
            arity: l.arity,
            place: l.place.into(),
            flow: l.flow.map(Into::into),
            body: l.body,
        }
    }
}

impl From<w::Subscription> for Subscription {
    fn from(s: w::Subscription) -> Subscription {
        Subscription {
            source: s.source.map(Into::into),
            event: s.event,
            handler: match s.handler {
                w::Handler::MethodGroup(p) => Handler::MethodGroup(p.into()),
                w::Handler::Lambda(n) => Handler::Lambda(n),
                w::Handler::Other => Handler::Other,
            },
            mode: match s.mode {
                w::SubscriptionMode::Add => SubscriptionMode::Add,
                w::SubscriptionMode::Remove => SubscriptionMode::Remove,
            },
            place: s.place.into(),
            node: s.node,
        }
    }
}

impl From<w::StringLiteral> for StringLiteral {
    fn from(l: w::StringLiteral) -> StringLiteral {
        StringLiteral {
            id: l.id,
            value: l.value,
            context: l.context.into(),
            place: l.place.into(),
            node: l.node,
            comment: None,
        }
    }
}

impl From<w::NameOf> for NameOf {
    fn from(n: w::NameOf) -> NameOf {
        NameOf {
            text: n.text,
            context: n.context.into(),
            place: n.place.into(),
            node: n.node,
        }
    }
}

impl From<w::MemberBody> for MemberBody {
    fn from(m: w::MemberBody) -> MemberBody {
        MemberBody {
            name_position: m.name_position.into(),
            kind: m.kind.into(),
            root: m.root,
            nodes: m.nodes.into_iter().map(Into::into).collect(),
            accesses: m.accesses.into_iter().map(Into::into).collect(),
            invocations: m.invocations.into_iter().map(Into::into).collect(),
            lambdas: m.lambdas.into_iter().map(Into::into).collect(),
            subscriptions: m.subscriptions.into_iter().map(Into::into).collect(),
            literals: m.literals.into_iter().map(Into::into).collect(),
            name_ofs: m.name_ofs.into_iter().map(Into::into).collect(),
            typed_names: typed_names(m.typed_names),
            tags: tags(m.tags),
        }
    }
}

impl From<w::TypeShape> for TypeShape {
    fn from(t: w::TypeShape) -> TypeShape {
        TypeShape {
            name_position: t.name_position.into(),
            type_params: t.type_params,
            bases: t
                .bases
                .into_iter()
                .map(|b| BaseType {
                    base: b.base,
                    type_args: b.type_args,
                })
                .collect(),
        }
    }
}

impl From<w::DeclarationMeta> for DeclarationMeta {
    fn from(m: w::DeclarationMeta) -> DeclarationMeta {
        DeclarationMeta {
            name_position: m.name_position.into(),
            modifiers: m.modifiers,
            attributes: m
                .attributes
                .into_iter()
                .map(|a| Attribute {
                    name: a.name,
                    args: a.args,
                })
                .collect(),
            tags: tags(m.tags),
        }
    }
}

impl From<w::FileBodies> for FileBodies {
    fn from(f: w::FileBodies) -> FileBodies {
        FileBodies {
            types: f.types.into_iter().map(Into::into).collect(),
            members: f.members.into_iter().map(Into::into).collect(),
            typed_members: typed_names(f.typed_members),
            metas: f.metas.into_iter().map(Into::into).collect(),
        }
    }
}

// ---- Redaction ----

/// What a withheld value is replaced by: the marker the first layer leaves
/// (`[redacted: password — see the source anchor]`), which names the shape
/// and sends a reader to the source. It holds nothing a Mermaid label
/// escapes (`crate::render::mermaid::escape_label`), so a card that quotes
/// it in a diagram shows it as it is.
pub(crate) fn withheld(pattern: &str) -> String {
    format!("{REDACTION_MARKER}{pattern} — see the source anchor]")
}

/// What one file's model withholds: the spellings to replace inside any
/// text, longest first, and the values a text that is nothing else is
/// replaced for whole.
struct Withheld<'p> {
    prefixes: &'p SecretPrefixes,
    spellings: Vec<(String, &'static str)>,
    values: BTreeMap<String, &'static str>,
}

impl<'p> Withheld<'p> {
    /// `spellings` as they are written and with their whitespace collapsed,
    /// as the plugin collapses a text it quotes; longest first, so a
    /// spelling inside another is never replaced before it.
    fn new(
        prefixes: &'p SecretPrefixes,
        spellings: Vec<(String, &'static str)>,
        values: BTreeMap<String, &'static str>,
    ) -> Withheld<'p> {
        let mut all: Vec<(String, &'static str)> = Vec::new();
        for (spelling, pattern) in spellings {
            let collapsed = spelling.split_whitespace().collect::<Vec<_>>().join(" ");
            if collapsed != spelling && !collapsed.is_empty() {
                all.push((collapsed, pattern));
            }
            if !spelling.trim().is_empty() {
                all.push((spelling, pattern));
            }
        }
        all.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then_with(|| a.cmp(b)));
        all.dedup_by(|a, b| a.0 == b.0);
        Withheld {
            prefixes,
            spellings: all,
            values,
        }
    }

    /// Redact one text in place: every withheld spelling in it, the text
    /// whole when it is a withheld value or its whole shape is a secret's
    /// ([`secret_pattern`]), and otherwise every line of it a secret's shape
    /// is on ([`redact_secret_block`]). A text the filter leaves alone stays
    /// byte for byte as the plugin returned it — a trailing newline too.
    fn text(&self, text: &mut String) {
        for (spelling, pattern) in &self.spellings {
            if text.contains(spelling.as_str()) {
                *text = text.replace(spelling.as_str(), &withheld(pattern));
            }
        }
        if let Some(pattern) = self
            .values
            .get(text.as_str())
            .copied()
            .or_else(|| secret_pattern(text, self.prefixes))
        {
            *text = withheld(pattern);
            return;
        }
        let (redacted, changed) = redact_secret_block(text, self.prefixes);
        if changed {
            *text = redacted;
        }
    }

    fn tags(&self, tags: &mut [Tag]) {
        for tag in tags {
            if let Some(value) = &mut tag.value {
                self.text(value);
            }
            for arg in &mut tag.args {
                self.text(arg);
            }
        }
    }
}

/// The pattern a literal is withheld under by the name its own record gives
/// it: the attribute's parameter or the attribute it is an argument of, the
/// path it is compared with or assigned to (KTD16 rule 5).
fn context_secret(context: &LiteralContext, value: &str) -> Option<&'static str> {
    let path = |p: &AccessPath| p.segments.join(".");
    match context {
        LiteralContext::AttributeArg(attribute, _, param) => param
            .as_deref()
            .and_then(|param| named_secret(param, value))
            .or_else(|| named_secret(attribute, value)),
        LiteralContext::Compared(p) | LiteralContext::Assigned(p) => named_secret(&path(p), value),
        LiteralContext::CallArg(..) | LiteralContext::Returned | LiteralContext::Other => None,
    }
}

/// The literals of `member` a write or a named argument gives a name, by
/// literal id: the target of a write whose value is the literal, the
/// parameter a named argument passes it to.
fn written_names(member: &MemberBody) -> BTreeMap<u32, String> {
    let mut names = BTreeMap::new();
    for access in &member.accesses {
        if let (AccessMode::Write, Some(ValueRef::Literal(id))) = (access.mode, &access.rhs) {
            names.insert(*id, access.target.segments.join("."));
        }
    }
    for invocation in &member.invocations {
        for arg in &invocation.args {
            if let (Some(name), ValueRef::Literal(id)) = (&arg.name, &arg.value) {
                names.insert(*id, name.clone());
            }
        }
    }
    names
}

/// Every text of a model, redacted — the one place a model is redacted,
/// between the `bodies` call and everything that stores or reads it (KTD16;
/// contract 0.3 hands the plugin no configuration for this pass, so the host
/// filters for it). `path` and `source` are the file the model was read
/// from.
///
/// **Literals, by place.** For a file the host has a lexer for, the
/// decisions the first layer makes about its literals are read from the
/// source ([`SourceSecrets`]) and carried to the model's literals by where
/// they are written: a model literal's place starts at its opening token,
/// the lexer's literal at its content, so a model literal takes the lexer's
/// literal whose content starts inside its place. Beside that — and alone,
/// for a file the host has no lexer for (a `.xaml`), or a literal no lexed
/// one pairs with — the record names the value: the attribute's argument it
/// is, the path it is compared with or written to, the parameter a named
/// argument passes it to; and so does its own shape ([`secret_pattern`], and
/// a key-shaped word in it, [`without_long_tokens`]). A
/// withheld literal's value is replaced by the marker ([`withheld`]).
///
/// **Handlers of markup.** A markup body's subscription names its handler
/// by an attribute's value the plugin took for a method's name, so that name
/// is decided like a literal of the attribute: by the event's name
/// ([`named_secret`]) and by its own shape ([`secret_pattern`]) — not by a
/// key-shaped word in it, which a designer's handler name has. A withheld
/// name is replaced by the marker, as a literal's value is.
///
/// **Every text.** The texts the contract marks "redacted by the host" — a
/// string literal's value, a condition's text and every `other` node of its
/// expression, a `nameof`'s text, an attribute's arguments, a subscription's
/// event, source and handler, and the value and arguments of every tag —
/// lose every withheld literal as written and every
/// comment the source names a secret in, and then go through the filter the
/// first layer gives every text. What is left is names the syntax spells
/// (paths, types, enum members), which the first layer stores unredacted too.
///
/// Every change here raises [`BODY_REDACTION_VERSION`].
pub fn redact_bodies(model: &mut FileBodies, path: &str, source: &[u8], prefixes: &SecretPrefixes) {
    let secrets = std::str::from_utf8(source)
        .ok()
        .and_then(|src| SourceSecrets::read(path, src, prefixes));
    let mut spellings: Vec<(String, &'static str)> = secrets
        .as_ref()
        .map(|s| s.spellings().to_vec())
        .unwrap_or_default();
    let mut values: BTreeMap<String, &'static str> = BTreeMap::new();
    for member in &mut model.members {
        let names = written_names(member);
        for literal in &mut member.literals {
            let lexed = secrets
                .as_ref()
                .and_then(|s| s.literal(literal.place.start.into(), literal.place.end.into()));
            let value = &literal.value;
            let pattern = lexed.and_then(|(pattern, _)| pattern).or_else(|| {
                context_secret(&literal.context, value)
                    .or_else(|| {
                        names
                            .get(&literal.id)
                            .and_then(|name| named_secret(name, value))
                    })
                    .or_else(|| secret_pattern(value, prefixes))
                    .or_else(|| without_long_tokens(value).1.then_some("long token"))
            });
            let Some(pattern) = pattern else {
                continue;
            };
            if !value.is_empty() {
                match lexed {
                    Some((_, spelling)) => spellings.push((spelling.to_string(), pattern)),
                    None => {
                        spellings.push((format!("\"{value}\""), pattern));
                        spellings.push((format!("'{value}'"), pattern));
                    }
                }
                values.insert(value.clone(), pattern);
            }
            literal.value = withheld(pattern);
        }
        // A markup body's handler is an attribute's value the plugin took for
        // a method's name (XAML does not say which attribute is an event), so
        // it is decided like a literal of that attribute: by the event's name
        // and by its own shape. A key-shaped word withholds it only when the
        // part after its last `_` is not letters alone: a designer's handler
        // name (`comboBox1_SelectedIndexChanged`) is shaped like a key but
        // ends in the event's name, a random token does not.
        // A C# handler is a name the syntax spells and goes through the filter
        // below alone.
        if member.kind != BodyKind::Markup {
            continue;
        }
        for subscription in &mut member.subscriptions {
            let Handler::MethodGroup(handler) = &mut subscription.handler else {
                continue;
            };
            let name = handler.segments.join(".");
            let Some(pattern) = named_secret(&subscription.event, &name)
                .or_else(|| secret_pattern(&name, prefixes))
                .or_else(|| key_shaped_handler(&name).then_some("long token"))
            else {
                continue;
            };
            if !name.is_empty() {
                spellings.push((format!("\"{name}\""), pattern));
                spellings.push((format!("'{name}'"), pattern));
                values.insert(name, pattern);
            }
            handler.segments = vec![withheld(pattern)];
        }
    }
    let filter = Withheld::new(prefixes, spellings, values);
    for member in &mut model.members {
        for node in &mut member.nodes {
            if let Some(condition) = &mut node.condition {
                filter.text(&mut condition.text);
                for expr in &mut condition.expr {
                    if let Expr::Other(text) = expr {
                        filter.text(text);
                    }
                }
            }
            filter.tags(&mut node.tags);
        }
        for literal in &mut member.literals {
            filter.text(&mut literal.value);
        }
        for name_of in &mut member.name_ofs {
            filter.text(&mut name_of.text);
        }
        for subscription in &mut member.subscriptions {
            filter.text(&mut subscription.event);
            if let Some(source) = &mut subscription.source {
                source.segments.iter_mut().for_each(|s| filter.text(s));
            }
            if let Handler::MethodGroup(handler) = &mut subscription.handler {
                handler.segments.iter_mut().for_each(|s| filter.text(s));
            }
        }
        filter.tags(&mut member.tags);
    }
    for meta in &mut model.metas {
        for attribute in &mut meta.attributes {
            for arg in &mut attribute.args {
                filter.text(arg);
            }
        }
        filter.tags(&mut meta.tags);
    }
}

/// The comment written right after each call argument of a model, attached
/// to the literal (plan 2a, U7: R18) — the one ordinary comment the second
/// layer reads: a menu item's label is a resource key, and the text it
/// stands for is often spelled beside it
/// (`GetResourceString("EditorView_checkErrors") /*"Kontrola chyb"*/`).
///
/// The comments are the lexer's segments as they are
/// ([`comments_after_literals`]), paired with a literal by place the way
/// [`redact_bodies`] pairs the lexer's literals: the one whose content starts
/// inside the model literal's place. A literal already withheld gets none,
/// and every text goes through [`redact_secret_block`] with the run's
/// prefixes; a text that still holds a key-shaped word
/// ([`without_long_tokens`]) is withheld whole, as `long token`. A file the
/// host has no lexer for gets none.
///
/// What this adds to a stored model is part of [`BODY_REDACTION_VERSION`].
pub fn quote_comments(
    model: &mut FileBodies,
    path: &str,
    source: &[u8],
    prefixes: &SecretPrefixes,
) {
    let Some(comments) = std::str::from_utf8(source)
        .ok()
        .and_then(|src| comments_after_literals(path, src))
    else {
        return;
    };
    if comments.is_empty() {
        return;
    }
    for member in &mut model.members {
        for literal in &mut member.literals {
            if !matches!(literal.context, LiteralContext::CallArg(..))
                || literal.value.contains(REDACTION_MARKER)
            {
                continue;
            }
            let start: (u32, u32) = literal.place.start.into();
            let end: (u32, u32) = literal.place.end.into();
            let first = comments.partition_point(|c| c.literal < start);
            let Some(comment) = comments.get(first).filter(|c| c.literal <= end) else {
                continue;
            };
            // The cut the first layer makes in a comment (`SourceSecrets`):
            // a key-shaped word withholds the whole quote, which no reader
            // could tell from a label once part of it is gone.
            let (mut text, _) = redact_secret_block(&comment.text, prefixes);
            if without_long_tokens(&text).1 {
                text = withheld("long token");
            }
            literal.comment = Some(LiteralComment {
                text,
                place: Range {
                    start: comment.start.into(),
                    end: comment.end.into(),
                },
            });
        }
    }
}

// ---- The pass ----

/// What a run with the second layer reads bodies with (KTD7): the run's own
/// plugins, then the bundled body components.
///
/// The components are held here and not in the run's [`PluginSet`], which the
/// cache fingerprint names plugin by plugin: switching the layer on and off
/// must leave the first layer's records valid, and only a run with the layer
/// loads the components at all. A declared plugin with a component's id
/// replaces it, as a declared plugin replaces a bundled one.
#[derive(Clone)]
pub struct BodyPass {
    level: u8,
    plugins: PluginSet,
    components: Vec<&'static Plugin>,
}

impl BodyPass {
    /// The pass of a run at `level` over `plugins`, with every bundled body
    /// component none of `plugins` has the id of.
    pub fn new(level: u8, plugins: &PluginSet) -> BodyPass {
        let components = crate::plugin::bundled::BODY_COMPONENTS
            .iter()
            .map(|(id, _)| *id)
            .filter(|id| !plugins.iter().any(|p| p.id() == *id))
            .map(crate::plugin::bundled::body_component)
            .collect();
        BodyPass {
            level,
            plugins: plugins.clone(),
            components,
        }
    }

    /// `--detail-level`.
    pub fn level(&self) -> u8 {
        self.level
    }

    /// The bundled body components the run adds, those no plugin of the run
    /// replaced — for the manifest and the language registry.
    pub fn components(&self) -> &[&'static Plugin] {
        &self.components
    }

    /// The plugin that reads the bodies of `path`: the first of the run's
    /// plugins that claims its extension and serves `bodies`, else the first
    /// component that does.
    fn plugin_for(&self, path: &str) -> Option<&'static Plugin> {
        let ext = path.rsplit_once('.').map(|(_, e)| e)?;
        self.plugins.for_pass(ext, Pass::Bodies).or_else(|| {
            self.components
                .iter()
                .copied()
                .find(|c| c.manifest().serves_on(ext, Pass::Bodies))
        })
    }

    /// Every plugin that may read bodies in this run, in routing order.
    fn readers(&self) -> Vec<&'static Plugin> {
        self.plugins
            .iter()
            .filter(|p| p.manifest().serves(Pass::Bodies))
            .chain(self.components.iter().copied())
            .collect()
    }
}

/// One file's output of the `bodies` pass.
pub(crate) enum BodiesOutput {
    Read(BodiesWire),
    Failed {
        path: String,
        plugin: String,
        /// Redacted.
        reason: String,
    },
}

/// A `languages-bodies` record: the redacted model and who read it. Which
/// component and which redaction made it is the record's tag
/// ([`CacheStore::derived`]); the model is stored unpaired, since the pairing
/// reads the first layer of the run that serves the record.
#[derive(Serialize, Deserialize)]
pub(crate) struct BodiesWire {
    path: String,
    plugin: String,
    /// Shared with the run's model, so writing the record copies nothing.
    model: Arc<FileBodies>,
}

impl WalkOutput for BodiesOutput {
    type Wire = BodiesWire;

    fn to_wire(&self, _vocabulary: &crate::graph::Vocabulary) -> Option<BodiesWire> {
        match self {
            BodiesOutput::Read(wire) => Some(BodiesWire {
                path: wire.path.clone(),
                plugin: wire.plugin.clone(),
                model: Arc::clone(&wire.model),
            }),
            BodiesOutput::Failed { .. } => None,
        }
    }

    fn from_wire(wire: BodiesWire, _vocabulary: &crate::graph::Vocabulary) -> Option<Self> {
        Some(BodiesOutput::Read(wire))
    }

    /// None: a model names no extractor and no kind.
    fn provenances_mut(&mut self) -> Vec<&mut Provenance> {
        Vec::new()
    }

    fn is_failure(&self) -> bool {
        matches!(self, BodiesOutput::Failed { .. })
    }
}

/// The tag of the records `plugin` writes: the redaction and the component.
fn record_tag(plugin: &Plugin) -> String {
    format!(
        "bodies-r{BODY_REDACTION_VERSION}+{}@{}",
        plugin.id(),
        plugin.digest()
    )
}

/// Read one file's bodies with `plugin` and redact them. `refuse` is a hook a
/// test fails a chosen file through; a run never refuses anything itself.
fn read_file(
    plugin: &'static Plugin,
    sessions: &mut Lease<'_>,
    path: &str,
    source: &[u8],
    prefixes: &SecretPrefixes,
    refuse: &(dyn Fn(&str) -> Option<String> + Sync),
) -> BodiesOutput {
    let failed = |reason: &str| BodiesOutput::Failed {
        path: path.to_string(),
        plugin: plugin.id().to_string(),
        reason: redact_secret_block(reason, prefixes).0,
    };
    if let Some(reason) = refuse(path) {
        return failed(&reason);
    }
    let Some(session) = sessions.get(plugin) else {
        return failed("the plugin could not be started");
    };
    match session.bodies(path, source) {
        Ok(file) => {
            let crate::plugin::host::Bodies { model, .. } = file;
            let mut model = FileBodies::from(model);
            redact_bodies(&mut model, path, source, prefixes);
            quote_comments(&mut model, path, source, prefixes);
            BodiesOutput::Read(BodiesWire {
                path: path.to_string(),
                plugin: plugin.id().to_string(),
                model: Arc::new(model),
            })
        }
        Err(e) => {
            // A 0.3 plugin's own reason is in the text: redacted, as the
            // fact and coverage store it.
            eprintln!("{}", redact_secret_block(&e.to_string(), prefixes).0);
            failed(&e.reason())
        }
    }
}

/// Run the `bodies` pass over `repo` and pair it with the first layer in
/// `graph` (see the module docs). `names` are the declaration-name positions
/// the languages extractor collected; `cache` is the run's store, which this
/// pass tags per component.
pub(crate) fn run(
    pass: &BodyPass,
    repo: &Repo,
    graph: &mut Graph,
    names: &NamePositions,
    prefixes: &SecretPrefixes,
    cache: Option<&CacheStore>,
    progress: &dyn crate::progress::Progress,
) -> Result<BodyLayer, ExtractError> {
    run_with(pass, repo, graph, names, prefixes, cache, progress, &|_| {
        None
    })
}

// The pass's inputs, plus the one test hook; a struct would only rename them.
#[allow(clippy::too_many_arguments)]
fn run_with(
    pass: &BodyPass,
    repo: &Repo,
    graph: &mut Graph,
    names: &NamePositions,
    prefixes: &SecretPrefixes,
    cache: Option<&CacheStore>,
    progress: &dyn crate::progress::Progress,
    refuse: &(dyn Fn(&str) -> Option<String> + Sync),
) -> Result<BodyLayer, ExtractError> {
    let tracked = repo.tracked_files_with_paths()?;
    let pool = Pool::new(Arc::new(PathKeys::snapshot(graph).into_set()));
    let mut read: BTreeMap<String, (String, Arc<FileBodies>)> = BTreeMap::new();
    let mut failed: BTreeMap<String, BodyFailure> = BTreeMap::new();
    let readers = pass.readers();
    progress.phase("languages bodies", Some(tracked.len()));
    // One walk per component, so each record is tagged with the digest of the
    // one component that made it: a rebuilt component misses its own records
    // and no other's.
    for &plugin in &readers {
        let wants = |key: &str| pass.plugin_for(key).is_some_and(|p| p.id() == plugin.id());
        if !tracked.iter().any(|tf| wants(&tf.key)) {
            continue;
        }
        let store = cache.map(|c| c.derived(&record_tag(plugin)));
        walk_files(
            repo,
            &tracked,
            progress,
            PHASE_BODIES,
            store.as_ref(),
            wants,
            |batch| {
                batch
                    .par_iter()
                    .map_init(
                        || pool.lease(),
                        |sessions, (key, src)| {
                            read_file(plugin, sessions, key, src, prefixes, refuse)
                        },
                    )
                    .collect()
            },
            |out| match out {
                BodiesOutput::Read(wire) => {
                    read.insert(wire.path, (wire.plugin, wire.model));
                }
                BodiesOutput::Failed {
                    path,
                    plugin,
                    reason,
                } => {
                    let fact = Fact::new(
                        FactKind::BodyExtractionFailed,
                        FactValue::Text(format!("bodies: {reason}")),
                        Provenance::new(
                            path.as_str(),
                            None,
                            ExtractorId::from(plugin.as_str()),
                            Tier::Measured,
                        ),
                    );
                    graph.add_fact(&NodeKey::new(path.as_str()), fact);
                    failed.insert(path, BodyFailure { plugin, reason });
                }
            },
        )?;
    }
    progress.phase_done();
    Ok(pair(
        pass.level,
        readers
            .iter()
            .map(|p| BodyReader {
                id: p.id().to_string(),
                version: p.manifest().version.clone(),
                sha256: p.digest().to_string(),
                contract: p.contract().word().to_string(),
                languages: p
                    .manifest()
                    .languages
                    .iter()
                    .map(|l| l.name.clone())
                    .collect(),
                component: pass.components.iter().any(|c| c.id() == p.id()),
            })
            .collect(),
        graph,
        names,
        read,
        failed,
    ))
}

// ---- Pairing ----

/// How a body found the first-layer declaration it belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PairedBy {
    /// Its name position is the declaration's.
    Name,
    /// No declaration has its name position; this is the nearest one that
    /// encloses it (an accessor's, a local function's, a static
    /// constructor's body when the first layer has no symbol of its own).
    Enclosing,
}

/// One first-layer declaration that can take a body: one whose declaration
/// carried the position of its name — of any language and any kind, a
/// declared node kind of a 0.3 component included.
struct Decl {
    key: NodeKey,
    name_at: Position,
    /// The lines its declaration spans in this file, when known.
    span: Option<(u32, u32)>,
}

/// The lines `key`'s declaration spans in `file`: the provenance of its name
/// fact there, or of any fact of it read from that file.
fn declaration_span(graph: &Graph, key: &NodeKey, file: &str) -> Option<(u32, u32)> {
    let node = graph.node(key)?;
    let in_file = || node.facts.iter().filter(|f| f.provenance.file == file);
    let fact = in_file()
        .find(|f| f.kind == FactKind::SymbolName)
        .or_else(|| in_file().next())?;
    Some((fact.provenance.span.start()?, fact.provenance.span.end()?))
}

/// The declaration nearest around `at`, among those of one file: the
/// innermost whose lines hold `at`'s line. Several with the same lines — two
/// members on one line inside a type that is all on that line too — are told
/// apart by the name written last at or before `at`; one alone takes the body
/// whatever its column, since nothing else on those lines could. The rule of
/// the usage pass's owner lookup, over the declarations that carry a name
/// position.
fn enclosing(decls: &[Decl], at: Position) -> Option<&Decl> {
    let holds = |d: &&Decl| d.span.is_some_and(|(s, e)| s <= at.line && at.line <= e);
    let inner = decls
        .iter()
        .filter(holds)
        .max_by_key(|d| d.span.map(|(s, e)| (s, std::cmp::Reverse(e))))?;
    let tied: Vec<&Decl> = decls
        .iter()
        .filter(holds)
        .filter(|d| d.span == inner.span)
        .collect();
    if let [only] = tied.as_slice() {
        return Some(only);
    }
    tied.into_iter()
        .filter(|d| d.name_at <= at)
        .max_by_key(|d| d.name_at)
}

/// A body that paired with no declaration and has none around it. Kept and
/// counted, never dropped (KTD6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnpairedBody {
    pub path: String,
    pub position: Position,
    pub kind: BodyKind,
}

/// A file the `bodies` pass failed on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BodyFailure {
    pub plugin: String,
    /// The pass's reason, redacted.
    pub reason: String,
}

/// A plugin that may read bodies in the run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BodyReader {
    pub id: String,
    pub version: String,
    pub sha256: String,
    pub contract: String,
    /// The languages it claims, by name.
    pub languages: Vec<String>,
    /// The bundled body component the second layer adds to the run — not one
    /// of the run's own plugins, so not yet in the manifest's list of them.
    pub component: bool,
}

/// One file's model, as the run read it.
#[derive(Debug, Clone, PartialEq)]
pub struct FileLayer {
    /// The plugin that read it.
    pub plugin: String,
    pub model: Arc<FileBodies>,
    /// By member index: the first-layer declaration the body belongs to, and
    /// how it was found; `None` for an unpaired body.
    pub owners: Vec<Option<(NodeKey, PairedBy)>>,
}

/// Whether a declaration has its second layer (KTD8): for a later unit to
/// decide whether to render it with or without.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecondLayer {
    /// Every file that declares it — every part of a partial type — was read.
    Present,
    /// A file that declares it failed in the `bodies` pass: it is rendered
    /// without its second layer, whichever parts were read, and coverage
    /// says so.
    Failed,
    /// No file of it was read: no plugin of the run reads its bodies, or the
    /// first layer gave it no name position.
    None,
}

/// A member body paired with a declaration, and where it was read.
#[derive(Debug, Clone, Copy)]
pub struct PairedBody<'l> {
    pub path: &'l str,
    pub body: &'l MemberBody,
    pub by: PairedBy,
}

/// How the run's bodies paired, counted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PairingCounts {
    /// Member bodies read.
    pub members: u64,
    pub paired_by_name: u64,
    pub paired_by_enclosing: u64,
    /// Type shapes, typed members and declaration metas whose name position
    /// is no declaration's.
    pub unpaired_declarations: u64,
}

/// Where in a file a paired record is: `(path, index)`.
type At = (String, usize);

/// The second layer of a run (KTD6): the model of bodies per file, paired
/// with the first layer, beside the graph.
///
/// Read a first-layer declaration's second layer by its key:
/// [`BodyLayer::layer_of`] first (a type with a failed part renders without
/// it), then [`BodyLayer::bodies_of`] for its bodies — a method's body, a
/// property's accessors, the local functions declared in it — and
/// [`BodyLayer::type_shapes_of`], [`BodyLayer::typed_members_of`] and
/// [`BodyLayer::metas_of`] for what a type, a field or property and any
/// declaration say beside their bodies. Every list is ordered by path, then
/// by position in the file — never by the order the workers finished in —
/// so the parts of a partial type come in one order on every run.
#[derive(Debug, Clone, PartialEq)]
pub struct BodyLayer {
    level: u8,
    readers: Vec<BodyReader>,
    files: BTreeMap<String, FileLayer>,
    failed: BTreeMap<String, BodyFailure>,
    /// Every declaration that carried a name position, with the files that
    /// declare it — more than one for a partial type.
    declared_in: BTreeMap<NodeKey, BTreeSet<String>>,
    bodies: BTreeMap<NodeKey, Vec<At>>,
    types: BTreeMap<NodeKey, Vec<At>>,
    typed_members: BTreeMap<NodeKey, Vec<At>>,
    metas: BTreeMap<NodeKey, Vec<At>>,
    unpaired: Vec<UnpairedBody>,
    counts: PairingCounts,
}

/// Pair what was read with the first layer (see [`PairedBy`]).
fn pair(
    level: u8,
    readers: Vec<BodyReader>,
    graph: &Graph,
    names: &NamePositions,
    read: BTreeMap<String, (String, Arc<FileBodies>)>,
    failed: BTreeMap<String, BodyFailure>,
) -> BodyLayer {
    let mut declared_in: BTreeMap<NodeKey, BTreeSet<String>> = BTreeMap::new();
    for (file, key) in names.keys() {
        declared_in
            .entry(key.clone())
            .or_default()
            .insert(file.clone());
    }
    // Only the files that were read need their declarations indexed.
    let mut by_file: HashMap<&str, Vec<Decl>> = HashMap::new();
    for ((file, key), at) in names {
        if read.contains_key(file) {
            by_file.entry(file.as_str()).or_default().push(Decl {
                key: key.clone(),
                name_at: Position::from(*at),
                span: declaration_span(graph, key, file),
            });
        }
    }
    for decls in by_file.values_mut() {
        decls.sort_by(|a, b| (a.name_at, &a.key).cmp(&(b.name_at, &b.key)));
    }
    let mut layer = BodyLayer {
        level,
        readers,
        files: BTreeMap::new(),
        failed,
        declared_in,
        bodies: BTreeMap::new(),
        types: BTreeMap::new(),
        typed_members: BTreeMap::new(),
        metas: BTreeMap::new(),
        unpaired: Vec::new(),
        counts: PairingCounts::default(),
    };
    for (path, (plugin, model)) in read {
        let decls = by_file.get(path.as_str()).map_or(&[][..], Vec::as_slice);
        // The one declaration whose name is written exactly there; a position
        // two declarations claim names neither.
        let named = |at: Position| -> Option<&NodeKey> {
            let mut hits = decls.iter().filter(|d| d.name_at == at);
            let first = hits.next()?;
            hits.next().is_none().then_some(&first.key)
        };
        let mut owners = Vec::with_capacity(model.members.len());
        for (i, member) in model.members.iter().enumerate() {
            layer.counts.members += 1;
            let at = member.name_position;
            let owner = match named(at) {
                Some(key) => {
                    layer.counts.paired_by_name += 1;
                    Some((key.clone(), PairedBy::Name))
                }
                None => match enclosing(decls, at) {
                    Some(d) => {
                        layer.counts.paired_by_enclosing += 1;
                        Some((d.key.clone(), PairedBy::Enclosing))
                    }
                    None => {
                        layer.unpaired.push(UnpairedBody {
                            path: path.clone(),
                            position: at,
                            kind: member.kind,
                        });
                        None
                    }
                },
            };
            if let Some((key, _)) = &owner {
                layer
                    .bodies
                    .entry(key.clone())
                    .or_default()
                    .push((path.clone(), i));
            }
            owners.push(owner);
        }
        let mut by_name = |map: &mut BTreeMap<NodeKey, Vec<At>>, positions: Vec<Position>| {
            for (i, at) in positions.into_iter().enumerate() {
                match named(at) {
                    Some(key) => map.entry(key.clone()).or_default().push((path.clone(), i)),
                    None => layer.counts.unpaired_declarations += 1,
                }
            }
        };
        by_name(
            &mut layer.types,
            model.types.iter().map(|t| t.name_position).collect(),
        );
        by_name(
            &mut layer.typed_members,
            model.typed_members.iter().map(|t| t.position).collect(),
        );
        by_name(
            &mut layer.metas,
            model.metas.iter().map(|m| m.name_position).collect(),
        );
        layer.files.insert(
            path,
            FileLayer {
                plugin,
                model,
                owners,
            },
        );
    }
    // Path, then position in the file: the order a partial type's parts and a
    // property's accessors come in, whoever finished first.
    let files = &layer.files;
    let body_at = |(path, i): &At| {
        (
            path.clone(),
            files[path].model.members[*i].name_position,
            *i,
        )
    };
    for list in layer.bodies.values_mut() {
        list.sort_by_cached_key(body_at);
    }
    layer
}

impl BodyLayer {
    /// `--detail-level`.
    pub fn level(&self) -> u8 {
        self.level
    }

    /// The plugins that could read bodies in the run, in routing order.
    pub fn readers(&self) -> &[BodyReader] {
        &self.readers
    }

    /// The model of every file read, by path.
    pub fn files(&self) -> impl Iterator<Item = (&str, &FileLayer)> {
        self.files.iter().map(|(p, f)| (p.as_str(), f))
    }

    /// One file's model, when it was read.
    pub fn file(&self, path: &str) -> Option<&FileLayer> {
        self.files.get(path)
    }

    /// Every file the pass failed on, by path.
    pub fn failures(&self) -> impl Iterator<Item = (&str, &BodyFailure)> {
        self.failed.iter().map(|(p, f)| (p.as_str(), f))
    }

    /// The bodies that belong to `key`, by path and position.
    pub fn bodies_of(&self, key: &NodeKey) -> Vec<PairedBody<'_>> {
        self.bodies.get(key).map_or_else(Vec::new, |list| {
            list.iter()
                .map(|(path, i)| {
                    let file = &self.files[path];
                    PairedBody {
                        path,
                        body: &file.model.members[*i],
                        by: file.owners[*i]
                            .as_ref()
                            .map_or(PairedBy::Name, |(_, by)| *by),
                    }
                })
                .collect()
        })
    }

    fn records<'l, T>(
        &'l self,
        map: &BTreeMap<NodeKey, Vec<At>>,
        key: &NodeKey,
        get: impl Fn(&'l FileBodies, usize) -> &'l T,
    ) -> Vec<(&'l str, &'l T)> {
        map.get(key).map_or_else(Vec::new, |list| {
            list.iter()
                .map(|(path, i)| {
                    let (path, file) = self
                        .files
                        .get_key_value(path)
                        .expect("a paired record's file was read");
                    (path.as_str(), get(&file.model, *i))
                })
                .collect()
        })
    }

    /// What the type `key` declares about itself, one shape per part.
    pub fn type_shapes_of(&self, key: &NodeKey) -> Vec<(&str, &TypeShape)> {
        self.records(&self.types, key, |m, i| &m.types[i])
    }

    /// The type a field, property or event `key` is declared with.
    pub fn typed_members_of(&self, key: &NodeKey) -> Vec<(&str, &TypedName)> {
        self.records(&self.typed_members, key, |m, i| &m.typed_members[i])
    }

    /// The modifiers, attributes and tags of `key`, one per part.
    pub fn metas_of(&self, key: &NodeKey) -> Vec<(&str, &DeclarationMeta)> {
        self.records(&self.metas, key, |m, i| &m.metas[i])
    }

    /// Whether `key` has its second layer — see [`SecondLayer`].
    pub fn layer_of(&self, key: &NodeKey) -> SecondLayer {
        let Some(files) = self.declared_in.get(key) else {
            return SecondLayer::None;
        };
        if files.iter().any(|f| self.failed.contains_key(f)) {
            SecondLayer::Failed
        } else if files.iter().all(|f| self.files.contains_key(f)) {
            SecondLayer::Present
        } else {
            SecondLayer::None
        }
    }

    /// The declarations written in several files — the parts of a partial
    /// type — that lost their second layer to one failed part although
    /// another was read. A declaration of one failed file says so through
    /// its file's failure; these are the ones that would otherwise look read.
    pub fn withheld(&self) -> Vec<&NodeKey> {
        self.declared_in
            .iter()
            .filter(|(_, files)| {
                files.iter().any(|f| self.failed.contains_key(f))
                    && files.iter().any(|f| self.files.contains_key(f))
            })
            .map(|(key, _)| key)
            .collect()
    }

    /// The bodies that paired with nothing, in path and file order.
    pub fn unpaired(&self) -> &[UnpairedBody] {
        &self.unpaired
    }

    pub fn counts(&self) -> PairingCounts {
        self.counts
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::{ExtractOptions, NamesHandoff};
    use git2::{Repository, Signature, Time};
    use std::path::Path;

    const SECRET: &str = "ghp_Q7rT2mX9vB4nL6kP1sD8fH3jW5zC0aE";

    fn prefixes() -> SecretPrefixes {
        SecretPrefixes::new(&["acme_".to_string()])
    }

    fn range() -> Range {
        Range::from([1, 1, 1, 2])
    }

    fn tag(value: &str) -> Tag {
        Tag {
            name: "key-compare".into(),
            value: Some(value.into()),
            args: vec![value.into()],
        }
    }

    fn path() -> AccessPath {
        AccessPath {
            root: PathRoot::Name,
            segments: vec!["Run".into()],
        }
    }

    /// A model that carries `text` in every field the contract marks
    /// "redacted by the host".
    fn model_with(text: &str) -> FileBodies {
        FileBodies {
            types: Vec::new(),
            members: vec![MemberBody {
                name_position: Position { line: 1, column: 1 },
                kind: BodyKind::Method,
                root: 0,
                nodes: vec![BodyNode {
                    id: 0,
                    kind: NodeKind::Choice,
                    place: range(),
                    parent: None,
                    condition: Some(Condition {
                        text: text.into(),
                        expr: vec![Expr::Other(text.into())],
                        enum_refs: Vec::new(),
                        reads: Vec::new(),
                        constant: None,
                    }),
                    returns: None,
                    tags: vec![tag(text)],
                }],
                accesses: Vec::new(),
                invocations: Vec::new(),
                lambdas: Vec::new(),
                subscriptions: vec![Subscription {
                    source: Some(AccessPath {
                        root: PathRoot::Name,
                        segments: vec![text.into()],
                    }),
                    event: text.into(),
                    handler: Handler::MethodGroup(AccessPath {
                        root: PathRoot::Name,
                        segments: vec![text.into()],
                    }),
                    mode: SubscriptionMode::Add,
                    place: range(),
                    node: Some(0),
                }],
                literals: vec![StringLiteral {
                    id: 0,
                    value: text.into(),
                    context: LiteralContext::CallArg(path(), 0),
                    place: range(),
                    node: Some(0),
                    comment: None,
                }],
                name_ofs: vec![NameOf {
                    text: text.into(),
                    context: LiteralContext::Other,
                    place: range(),
                    node: None,
                }],
                typed_names: Vec::new(),
                tags: vec![tag(text)],
            }],
            typed_members: Vec::new(),
            metas: vec![DeclarationMeta {
                name_position: Position { line: 1, column: 1 },
                modifiers: vec!["public".into()],
                attributes: vec![Attribute {
                    name: "Obsolete".into(),
                    args: vec![text.into()],
                }],
                tags: vec![tag(text)],
            }],
        }
    }

    #[test]
    fn every_text_the_contract_names_is_redacted() {
        for secret in [SECRET, "acme_Zx81Lq0Wv3Nd5Rt7Yp2"] {
            let mut model = model_with(secret);
            redact_bodies(&mut model, "src/View.xaml", b"", &prefixes());
            let text = serde_json::to_string(&model).unwrap();
            assert!(!text.contains(secret), "{text}");
            // Fourteen texts: the condition and its `other` node, the value
            // and the argument of three tags, the literal, the `nameof`, the
            // attribute's argument, the subscription's event, source and
            // handler.
            assert_eq!(
                text.matches(ra_redact::REDACTION_MARKER).count(),
                14,
                "{text}"
            );
        }
    }

    /// A subscription of `event` to the method group `handler`, from `source`.
    fn subscription(source: Option<&str>, event: &str, handler: &str) -> Subscription {
        Subscription {
            source: source.map(|s| AccessPath {
                root: PathRoot::Name,
                segments: vec![s.into()],
            }),
            event: event.into(),
            handler: Handler::MethodGroup(AccessPath {
                root: PathRoot::Name,
                segments: vec![handler.into()],
            }),
            mode: SubscriptionMode::Add,
            place: range(),
            node: None,
        }
    }

    /// One body of `kind` with `subscriptions` and nothing else.
    fn subscribing(kind: BodyKind, subscriptions: Vec<Subscription>) -> FileBodies {
        FileBodies {
            types: Vec::new(),
            members: vec![MemberBody {
                name_position: Position { line: 1, column: 1 },
                kind,
                root: 0,
                nodes: Vec::new(),
                accesses: Vec::new(),
                invocations: Vec::new(),
                lambdas: Vec::new(),
                subscriptions,
                literals: Vec::new(),
                name_ofs: Vec::new(),
                typed_names: Vec::new(),
                tags: Vec::new(),
            }],
            typed_members: Vec::new(),
            metas: Vec::new(),
        }
    }

    fn handler_of(model: &FileBodies, i: usize) -> String {
        match &model.members[0].subscriptions[i].handler {
            Handler::MethodGroup(p) => p.segments.join("."),
            other => format!("{other:?}"),
        }
    }

    /// Plan 2c review: an attribute value the XAML plugin took for a handler
    /// (`Token="ghp_…"`, `Password="Admin_Pass2024"`) is withheld like a
    /// literal of that attribute — by its shape, and by the event's name —
    /// and so is every other place of the file that spells it; a handler
    /// named like one stays, a designer's long name and an event that only
    /// mentions a password (`PasswordChanged`) included.
    #[test]
    fn a_markup_handler_is_decided_like_a_literal_of_its_attribute() {
        let mut model = subscribing(
            BodyKind::Markup,
            vec![
                subscription(Some("box"), "Tag", SECRET),
                subscription(None, "Password", "Admin_Pass2024"),
                subscription(None, "Click", "OnSave"),
                subscription(None, "SelectionChanged", "comboBox1_SelectedIndexChanged"),
                subscription(None, "PasswordChanged", "OnPasswordChanged"),
                subscription(None, "Tag", "k3J9xQ8vLm2_Rz7Tp4Wn6Yb1Hc5Dg0Fa"),
                subscription(None, "Loaded", "dataGridView1_CellValueChanged"),
                subscription(None, "Tag", "k3J9xQ8vLm2Rz7Tp4Wn6Yb1Hc5Dg0Fa_Xq"),
                subscription(None, "TextChanged", "textBox1_TextChanged_1"),
                subscription(None, "SelectionChanged", "OnTab2SelectionChanged"),
            ],
        );
        // Another text of the file that quotes a withheld handler loses it.
        model.members[0].tags = vec![tag("Login('Admin_Pass2024')")];
        redact_bodies(&mut model, "src/View.xaml", b"", &prefixes());
        let text = serde_json::to_string(&model).unwrap();
        assert!(!text.contains(SECRET), "{text}");
        assert!(!text.contains("Admin_Pass2024"), "{text}");
        assert_eq!(handler_of(&model, 0), withheld("provider token"));
        assert_eq!(handler_of(&model, 1), withheld("password"));
        assert_eq!(handler_of(&model, 2), "OnSave");
        assert_eq!(handler_of(&model, 3), "comboBox1_SelectedIndexChanged");
        assert_eq!(handler_of(&model, 4), "OnPasswordChanged");
        // A random key-shaped token goes wherever its underscore falls; a
        // designer's name — key-shaped, but words with a digit or two — stays.
        assert!(!text.contains("k3J9xQ8vLm2_Rz7Tp4Wn6Yb1Hc5Dg0Fa"), "{text}");
        assert!(
            !text.contains("k3J9xQ8vLm2Rz7Tp4Wn6Yb1Hc5Dg0Fa_Xq"),
            "{text}"
        );
        assert_eq!(handler_of(&model, 5), withheld("long token"));
        assert_eq!(handler_of(&model, 6), "dataGridView1_CellValueChanged");
        assert_eq!(handler_of(&model, 7), withheld("long token"));
        assert_eq!(handler_of(&model, 8), "textBox1_TextChanged_1");
        assert_eq!(handler_of(&model, 9), "OnTab2SelectionChanged");
        assert_eq!(model.members[0].subscriptions[0].event, "Tag");
    }

    /// A C# handler is a method's name the syntax spells: it is no literal,
    /// so the event's name decides nothing and a designer's name stays —
    /// the model of a C# file keeps every subscription byte for byte.
    #[test]
    fn csharp_handlers_keep_their_method_names() {
        let subscriptions = vec![
            subscription(Some("saveButton"), "Click", "OnSave"),
            subscription(
                Some("dataGridView1"),
                "CellValueChanged",
                "dataGridView1_CellValueChanged",
            ),
            subscription(Some("box"), "PasswordChanged", "OnPasswordChanged"),
            subscription(None, "Token", "Admin_Pass2024"),
        ];
        let mut model = subscribing(BodyKind::Method, subscriptions);
        let before = model.clone();
        redact_bodies(&mut model, "src/View.cs", b"", &prefixes());
        assert_eq!(model, before);
    }

    /// A text the filter leaves alone stays as the plugin returned it — its
    /// trailing line break too.
    #[test]
    fn a_text_without_a_secret_is_kept_byte_for_byte() {
        let mut model = model_with("line one\nline two\n");
        let before = model.clone();
        redact_bodies(&mut model, "src/View.xaml", b"", &prefixes());
        assert_eq!(model, before);
    }

    /// The model goes to disk compact: positions as pairs, ranges as four
    /// numbers, nothing empty written.
    #[test]
    fn a_stored_model_is_compact_and_reads_back() {
        let model = model_with("x");
        let text = serde_json::to_string(&model).unwrap();
        assert!(text.contains("\"name_position\":[1,1]"), "{text}");
        assert!(text.contains("\"place\":[1,1,1,2]"), "{text}");
        assert!(!text.contains("[]") && !text.contains("null"), "{text}");
        let back: FileBodies = serde_json::from_str(&text).unwrap();
        assert_eq!(back, model);
    }

    fn decl(key: &str, at: (u32, u32), span: Option<(u32, u32)>) -> Decl {
        Decl {
            key: NodeKey::new(key),
            name_at: Position::from(at),
            span,
        }
    }

    #[test]
    fn the_nearest_enclosing_declaration_takes_a_body_without_its_own() {
        let decls = [
            decl("sym:T", (2, 14), Some((2, 9))),
            decl("sym:T.A", (4, 20), Some((4, 4))),
            decl("sym:T.B", (4, 40), Some((4, 4))),
            decl("sym:T.M", (6, 17), Some((6, 8))),
        ];
        let at = |line, column| {
            enclosing(&decls, Position { line, column }).map(|d| d.key.as_str().to_string())
        };
        // Innermost by lines.
        assert_eq!(at(7, 13).as_deref(), Some("sym:T.M"));
        assert_eq!(at(3, 5).as_deref(), Some("sym:T"));
        // Two on one line: the name written last before it.
        assert_eq!(at(4, 30).as_deref(), Some("sym:T.A"));
        assert_eq!(at(4, 50).as_deref(), Some("sym:T.B"));
        // Before both names: neither can be told.
        assert_eq!(at(4, 2), None);
        // Outside every declaration.
        assert_eq!(at(12, 1), None);
        // One alone on its lines takes the body whatever its column.
        let alone = [decl("sym:W", (1, 3), Some((1, 1)))];
        assert_eq!(
            enclosing(&alone, Position { line: 1, column: 1 }).map(|d| d.key.as_str()),
            Some("sym:W")
        );
        // A declaration whose lines are unknown encloses nothing.
        let unknown = [decl("sym:U", (1, 3), None)];
        assert!(enclosing(&unknown, Position { line: 1, column: 1 }).is_none());
    }

    /// A record is served only to a store with the tag it was written under:
    /// another component digest or another redaction version is a miss, and
    /// the run's own untagged store does not see it.
    #[test]
    fn a_tagged_record_misses_under_another_tag() {
        let dir = tempfile::tempdir().unwrap();
        let store = CacheStore::new(
            dir.path().to_path_buf(),
            "run".into(),
            crate::graph::Ranks::default(),
            crate::graph::Vocabulary::default(),
        );
        let blob = git2::Oid::hash_object(git2::ObjectType::Blob, b"x").unwrap();
        let tagged = store.derived("bodies-r1+csharp-body@aa");
        tagged.put(PHASE_BODIES, blob, "a.cs", &7u32);
        assert_eq!(tagged.get::<u32>(PHASE_BODIES, blob, "a.cs"), Some(7));
        let rebuilt = store.derived("bodies-r1+csharp-body@bb");
        assert_eq!(rebuilt.get::<u32>(PHASE_BODIES, blob, "a.cs"), None);
        let redacted_anew = store.derived("bodies-r2+csharp-body@aa");
        assert_eq!(redacted_anew.get::<u32>(PHASE_BODIES, blob, "a.cs"), None);
        assert_eq!(store.get::<u32>(PHASE_BODIES, blob, "a.cs"), None);
        // The record says which component made it.
        let raw =
            std::fs::read_to_string(dir.path().join(PHASE_BODIES).join(format!("{blob}.json")))
                .unwrap();
        assert!(raw.contains("csharp-body@aa"), "{raw}");
    }

    fn repo_with_files(dir: &Path, files: &[(&str, &str)]) -> Repo {
        for (rel, contents) in files {
            let full = dir.join(rel);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(&full, contents).unwrap();
        }
        let repository = Repository::init(dir).unwrap();
        let mut index = repository.index().unwrap();
        for (rel, _) in files {
            index.add_path(Path::new(rel)).unwrap();
        }
        index.write().unwrap();
        let tree = repository.find_tree(index.write_tree().unwrap()).unwrap();
        let sig = Signature::new("Ann", "ann@example.test", &Time::new(1_600_000_000, 0)).unwrap();
        repository
            .commit(Some("HEAD"), &sig, &sig, "initial", &tree, &[])
            .unwrap();
        Repo::open(dir).unwrap()
    }

    fn key_named(graph: &Graph, name: &str) -> NodeKey {
        let keys: Vec<NodeKey> = graph
            .nodes_sorted()
            .into_iter()
            .filter(|n| {
                n.facts.iter().any(|f| {
                    f.kind == FactKind::SymbolName && f.value == FactValue::Text(name.into())
                })
            })
            .map(|n| n.key.clone())
            .collect();
        assert_eq!(keys.len(), 1, "{name}: {keys:?}");
        keys.into_iter().next().unwrap()
    }

    /// The first layer and the second over `files`, the second failing every
    /// path `refuse` names.
    fn layers(
        files: &[(&str, &str)],
        refuse: &(dyn Fn(&str) -> Option<String> + Sync),
    ) -> (Graph, BodyLayer) {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo_with_files(dir.path(), files);
        let mut graph = Graph::new();
        let opts = ExtractOptions::default();
        let names = NamesHandoff::default();
        let silent = crate::progress::Silent;
        crate::extract::run_all_handing_names(
            &repo,
            &mut graph,
            &opts,
            &silent,
            None,
            Some(&names),
        )
        .unwrap();
        let pass = BodyPass::new(2, &opts.plugins);
        let layer = run_with(
            &pass,
            &repo,
            &mut graph,
            &names.take(),
            &prefixes(),
            None,
            &silent,
            refuse,
        )
        .unwrap();
        (graph, layer)
    }

    const VIEW: &str = "namespace Acme.Views\n{\n    public partial class EditorView\n    {\n        public void Check() { Save(); }\n        public int Count { get { return 1; } }\n    }\n}\n";
    const KEYS: &str = "namespace Acme.Views\n{\n    public partial class EditorView\n    {\n        public void Save() { }\n    }\n}\n";

    #[test]
    fn a_partial_type_with_one_failed_part_has_no_second_layer_and_says_so() {
        let refuse = |path: &str| (path == "src/EditorView.Keys.cs").then(|| "forced".to_string());
        let (graph, layer) = layers(
            &[
                ("src/EditorView.cs", VIEW),
                ("src/EditorView.Keys.cs", KEYS),
            ],
            &refuse,
        );
        let view = key_named(&graph, "EditorView");
        let check = key_named(&graph, "Check");
        let save = key_named(&graph, "Save");

        assert_eq!(layer.layer_of(&view), SecondLayer::Failed);
        assert_eq!(layer.withheld(), [&view]);
        // A member of the part that was read keeps its layer.
        assert_eq!(layer.layer_of(&check), SecondLayer::Present);
        assert_eq!(layer.layer_of(&save), SecondLayer::Failed);
        let bodies = layer.bodies_of(&check);
        assert_eq!(bodies.len(), 1);
        assert_eq!(bodies[0].by, PairedBy::Name);
        assert_eq!(bodies[0].path, "src/EditorView.cs");
        // The shape of the part that was read is still in the model.
        assert_eq!(layer.type_shapes_of(&view).len(), 1);

        // The failure is the second layer's own fact, on the file only.
        let file = graph.node(&NodeKey::new("src/EditorView.Keys.cs")).unwrap();
        let failures: Vec<&Fact> = file
            .facts
            .iter()
            .filter(|f| f.kind == FactKind::BodyExtractionFailed)
            .collect();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].value, FactValue::Text("bodies: forced".into()));
        assert_eq!(&*failures[0].provenance.extractor, "csharp-body");
        assert!(file
            .facts
            .iter()
            .all(|f| f.kind != FactKind::ExtractionFailed));

        let coverage = crate::coverage::bodies(&layer);
        assert_eq!(coverage.files, 1);
        assert_eq!(coverage.failed.len(), 1);
        assert_eq!(coverage.failed[0].path, "src/EditorView.Keys.cs");
        assert_eq!(coverage.withheld, [view.as_str()]);
        assert_eq!(coverage.plugins, ["xaml", "csharp-body"]);
    }

    /// A property's accessor carries its property's name position, so it
    /// pairs by name; the parts of a partial type come by path.
    #[test]
    fn bodies_pair_by_name_and_come_in_path_order() {
        let (graph, layer) = layers(
            &[
                ("src/EditorView.Keys.cs", KEYS),
                ("src/EditorView.cs", VIEW),
            ],
            &|_| None,
        );
        let view = key_named(&graph, "EditorView");
        assert_eq!(layer.layer_of(&view), SecondLayer::Present);
        assert!(layer.withheld().is_empty());
        let shapes: Vec<&str> = layer
            .type_shapes_of(&view)
            .iter()
            .map(|(p, _)| *p)
            .collect();
        assert_eq!(shapes, ["src/EditorView.Keys.cs", "src/EditorView.cs"]);
        let count = layer.bodies_of(&key_named(&graph, "Count"));
        assert_eq!(count.len(), 1, "{count:?}");
        assert_eq!(count[0].body.kind, BodyKind::Getter);
        assert_eq!(count[0].by, PairedBy::Name);
        let counts = layer.counts();
        assert_eq!(
            counts.members,
            counts.paired_by_name + counts.paired_by_enclosing
        );
        assert!(layer.unpaired().is_empty());
        assert_eq!(layer.layer_of(&NodeKey::new("sym:none")), SecondLayer::None);
    }

    // ---- The literal-aware filter (U8, KTD16) ----

    /// The stored form of the model the run read from `path` of `files`.
    fn stored(files: &[(&str, &str)], path: &str) -> String {
        let (_, layer) = layers(files, &|_| None);
        let file = layer
            .file(path)
            .unwrap_or_else(|| panic!("{path} was read"));
        serde_json::to_string(&*file.model).unwrap()
    }

    /// Secrets a name gives the literal on either side of `==` and `!=`, and
    /// the field it initializes; and a key-shaped literal only the lexer
    /// withholds, behind more characters of two bytes on its line than it is
    /// long, with a plain literal after it — a column counted in bytes would
    /// pair the one with nothing and the other with the key.
    const NAMED: &str = "namespace Acme.Views\n{\n    public class EditorView\n    {\n        private const string SmtpPassword = \"saltmarshowl\";\n        public bool Check(string token, string žeton, string errorToken)\n        {\n            if (žeton == \"plain\" || token == \"brindlecairn\") { return true; }\n            if (\"cobaltfennel\" != token) { return false; }\n            if (token != \"duskmallow\") { return false; }\n            if (errorToken == \"Neplatný token\") { return false; }\n            var žžžžžžžžžžžžžžžžžžžžžžžžžžžžžžžžžžžžžžžžžžžž = 0; var blob = \"QmFzZTY0U2VudGluZWxCbG9i7x\"; var tail = \"lindenword\";\n            return žeton == \"quiet\";\n        }\n    }\n}\n";

    /// The run withholds what a name says a literal is, in the literal and
    /// in the condition that quotes it, and keeps the literals no name speaks
    /// for and a message under a name of one.
    #[test]
    fn a_literal_is_withheld_by_the_name_it_is_compared_with_or_given_to() {
        let text = stored(&[("src/EditorView.cs", NAMED)], "src/EditorView.cs");
        for secret in [
            "saltmarshowl",
            "brindlecairn",
            "cobaltfennel",
            "duskmallow",
            "QmFzZTY0U2VudGluZWxCbG9i7x",
        ] {
            assert!(!text.contains(secret), "{secret}: {text}");
        }
        for kept in [
            "\"value\":\"plain\"",
            "\"value\":\"quiet\"",
            "\"value\":\"lindenword\"",
            "\"value\":\"Neplatný token\"",
        ] {
            assert!(text.contains(kept), "{kept}: {text}");
        }
        // The condition keeps its code and its plain literal; the withheld one
        // leaves the marker in its place.
        assert!(
            text.contains(
                "žeton == \\\"plain\\\" || token == [redacted: token — see the source anchor]"
            ),
            "{text}"
        );
        assert!(
            text.contains("[redacted: password — see the source anchor]"),
            "{text}"
        );
    }

    /// The first layer's decisions about literals come to the second
    /// unchanged: a key written one literal per line, a key-shaped word in a
    /// message, and a comment that names a secret inside a condition.
    #[test]
    fn the_first_layers_literal_decisions_reach_the_model() {
        let src = "namespace Acme.Views\n{\n    public class EditorView\n    {\n        public string Key()\n        {\n            var key = \"-----BEGIN RSA PRIVATE KEY-----\\n\" +\n                \"pemtailwarbler==\\n\" +\n                \"-----END RSA PRIVATE KEY-----\\n\";\n            var auth = \"Bearer c0ffee5eed1234abcd5678ef90abcdef\";\n            if (GetResourceString(\"k\") /* heslo: hollyquartz */ == null) { return auth; }\n            return key;\n        }\n        public string GetResourceString(string k) { return k; }\n    }\n}\n";
        let text = stored(&[("src/EditorView.cs", src)], "src/EditorView.cs");
        for secret in [
            "pemtailwarbler",
            "c0ffee5eed1234abcd5678ef90abcdef",
            "hollyquartz",
        ] {
            assert!(!text.contains(secret), "{secret}: {text}");
        }
        assert!(text.contains("\"value\":\"k\""), "{text}");
        assert!(text.contains("GetResourceString"), "{text}");
    }

    /// A model from a file the host has no lexer for is redacted by its own
    /// records: a literal given to a `Password` attribute, a literal of a
    /// provider token's shape, and every text that quotes either.
    #[test]
    fn a_model_without_a_lexer_is_redacted_by_its_records() {
        let mut model = model_with("Hello");
        let member = &mut model.members[0];
        let literal = |id: u32, value: &str, attribute: &str| StringLiteral {
            id,
            value: value.into(),
            context: LiteralContext::AttributeArg(attribute.into(), None, None),
            place: range(),
            node: Some(0),
            comment: None,
        };
        member.literals = vec![
            literal(0, "quillfrond", "Password"),
            literal(1, SECRET, "Tag"),
            literal(2, "Zadejte heslo", "Text"),
        ];
        member.tags = vec![Tag {
            name: "markup".into(),
            value: Some("quillfrond".into()),
            args: vec!["Password=\"quillfrond\"".into()],
        }];
        model.metas[0].attributes[0].args = vec!["Password=\"quillfrond\"".into()];
        for path in ["src/View.xaml", "src/widget.probe3"] {
            let mut redacted = model.clone();
            redact_bodies(
                &mut redacted,
                path,
                b"<View Password=\"quillfrond\" />",
                &prefixes(),
            );
            let text = serde_json::to_string(&redacted).unwrap();
            for secret in ["quillfrond", SECRET] {
                assert!(!text.contains(secret), "{path}: {secret}: {text}");
            }
            assert!(text.contains("\"value\":\"Zadejte heslo\""), "{text}");
            assert_eq!(
                redacted.members[0].tags[0].value.as_deref(),
                Some(withheld("password").as_str()),
                "{text}"
            );
            assert_eq!(
                redacted.members[0].literals[1].value,
                withheld("provider token")
            );
        }
    }

    /// The marker passes a Mermaid label's escaping as it is.
    #[test]
    fn the_marker_passes_a_mermaid_label_unchanged() {
        for pattern in ["password", "token", "private key", "long token"] {
            let marker = withheld(pattern);
            assert!(marker.starts_with(REDACTION_MARKER));
            assert_eq!(crate::render::mermaid::escape_label(&marker), marker);
        }
    }

    /// A declared plugin with the id of a bundled body component replaces
    /// it: the pass keeps no bundled component of that id, and the files
    /// of its extension go to the declared plugin, as a declared plugin
    /// replaces a bundled one in the first layer.
    #[test]
    fn a_declared_plugin_replaces_the_bundled_body_component_of_its_id() {
        use crate::plugin::bundled::{body_component, CSHARP_BODY_ID};
        use crate::plugin::{PluginSet, PluginSpec};
        let spec = PluginSpec {
            path: Path::new(env!("CARGO_MANIFEST_DIR")).join("plugins/prebuilt/csharp-body.wasm"),
            sha256: None,
        };
        let set = PluginSet::with_declared(&[spec]).unwrap();
        let pass = BodyPass::new(1, &set);
        assert!(
            pass.components().is_empty(),
            "the bundled component stays out"
        );
        let reader = pass
            .plugin_for("src/EditorView.cs")
            .expect("a reader for .cs");
        assert_eq!(reader.id(), CSHARP_BODY_ID);
        assert!(
            !std::ptr::eq(reader, body_component(CSHARP_BODY_ID)),
            "the declared plugin reads, not the bundled one"
        );
        assert_eq!(
            pass.readers()
                .iter()
                .filter(|p| p.id() == CSHARP_BODY_ID)
                .count(),
            1,
            "one reader of C# bodies"
        );

        let bundled = BodyPass::new(1, &PluginSet::bundled());
        assert_eq!(bundled.components().len(), 1);
        assert!(std::ptr::eq(
            bundled.plugin_for("src/EditorView.cs").unwrap(),
            body_component(CSHARP_BODY_ID)
        ));
    }
}
