//! Preconditions, dead paths, states, steps and mentions of a type's core
//! functionalities (plan 2a, U11: KTD13, KTD5, R6, R7, R9). Nothing here
//! renders; U6 puts it on the card. Every fact carries its anchor — the file
//! and the lines ([`Place`]).
//!
//! # Preconditions (KTD13 1–3)
//!
//! A **guard** is an `if` with one arm and no `else` at the top level of a
//! body, whose arm is a `return` without a value, a `throw` or a
//! `yield break`. The top-level statements before it may only be **writes of
//! fields** (any path not rooted at a local) and **calls written as a
//! statement whose result goes nowhere**, events raised (`X?.Invoke(…)`)
//! included; they run even when the guard stops the rest, so they are the
//! guard's **effects**. A local written a construction or a constant
//! (`var evt = new CancelEventArgs();`, `var n = 0;`) has no effect beyond
//! the member: it is neither an effect nor the main work. The first top-level
//! statement that is none of these — a local written anything else
//! (`var x = Load();`), a loop, a choice that is no guard, a `return` with a
//! value — is the **main work**, and no `if` after it is a precondition.
//!
//! A body is followed into a callee only through its main work: when the
//! statements after its last guard are one call of a member of the type,
//! resolved by name and arity ([`Index::resolve`], [`Index::overload`]:
//! `Check()` → `Check(bool)`), the callee's preconditions are the caller's,
//! the caller's effects before the call first. A call after the main work
//! passes nothing. An ambiguous overload, a callee in an ancestor, a callee
//! two levels up are admitted ([`AdmittedPrecondition`]), never followed.
//!
//! The **CanExecute** delegate of a command (the bound delegate a method named
//! `CanExecute` calls, or one declared `Predicate<…>`/`Func<…, bool>`,
//! [`super::bind_construction`]) gives preconditions too: an `if (c) return
//! false;` at its top level gives `c`, and the predicate it returns is split
//! into its top-level conjuncts, each turned by De Morgan into “does not run
//! when” (`(a || b) && c` → `!a && !b`, `!c`). The predicate is read from
//! the [`Condition`] a component gives the `return` node of a returned
//! expression that is no constant (`return …;`, a member's `=> …`, a
//! lambda's `p => …`), from a lone read (`p => canErrors`), or by following
//! a lone call into a member of the type (`p => CanCheck()`, `return
//! CanCheck();`); anything else is admitted as [`Gap::PredicateNotInModel`].
//!
//! Equal conditions are **one precondition** ([`Cond`]): the trees are
//! compared after negations are pushed to the leaves and the operands of
//! `==`, `!=`, `&&` and `||` are put in one order. Each precondition keeps
//! every place it was found at ([`Occurrence`]).
//!
//! # Dead paths (KTD13 4)
//!
//! A constant `return` as the whole body (`=> false`, `{ return false; }`),
//! an empty method body, or an arm under `if (false)` — only in a member that
//! **guards** something ([`Guarding`]): one read in a condition of the type,
//! the target of a CanExecute delegate (or the delegate's own lambda), or one
//! an entry point calls. Constant members that guard nothing are not listed,
//! and neither is the constant or empty body of a member a deriving type may
//! replace (`virtual`, `abstract`, an `override` that is not `sealed`).
//!
//! # States (KTD13 5, R6)
//!
//! A field or property of the type — a property standing for the one field
//! its setter writes `value` to — that is written constants or toggled
//! (`IsFullscreen = !IsFullscreen`, a `bool` written from an expression that
//! reads only itself) from two places or more, and read in a condition. Only
//! an access whose first name is the type's own member counts, so a static
//! read of another type (`Keyboard.Modifiers`) is never a state of this one.
//! The setter of a property tagged `notifies-change` is absorbed: its guard
//! (`if (field == value) return;`) is neither a read of the state nor a step.
//!
//! # Steps (KTD5, R7)
//!
//! From the idiom tags (`full-traversal`, `find-first`, `accumulate`,
//! `guard-return`, `swallow-exception`, `lazy-init`, `notifies-change`) and
//! from the nodes: a `choice` (with “the parameter chooses …” when its arms
//! are told apart by a parameter of the member), a `loop` with no idiom, a
//! call or construction of another type's member, a write of a state. A step
//! that changes no answer about the method is left out (R7): a local
//! declared or written, a call of the type's own member (its own steps say
//! what it does), a call that only logs, a `return`, `break` or `continue`,
//! a key branch (an entry point of its own). Every step has its anchor.
//!
//! # Mentions (R9)
//!
//! A string literal shaped like a configuration key — capital letters,
//! digits and `_`, three or more, at least one letter — written as an argument
//! of a call (`GetConfigValue("SQLPREFIX")`), and the names of other types the
//! functionality constructs, declares locals of or calls into, become its
//! keywords and the data of one sentence “works with …”.
//!
//! Everything is ordered by key, by place, or by the order of the source; no
//! hash map is iterated (R3).

use super::cluster::Functionality;
use super::entry::{bodies, BoundDelegate, EntryKind, EntryPoint, Place};
use super::resolve::{within, Index, Scope, Unresolved};
use crate::card::keywords::{is_word, split_identifier};
use crate::extract::bodies::{
    AccessMode, AccessPath, BodyKind, BodyNode, CallKind, CompareOp, Condition, Constant,
    DeclarationKind, Expr, FlowTarget, Invocation, LiteralContext, MemberAccess, MemberBody,
    NodeKind, PairedBy, PathRoot, StringLiteral, ValueRef,
};
use crate::graph::NodeKey;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// An expression tree longer than this is not compared structurally: it is
/// kept as its text (and bounds the depth every walk over a [`Cond`] goes).
const MAX_EXPR: usize = 256;

/// The idiom tags a loop or a catch carries that are steps of their own.
const NODE_IDIOMS: &[&str] = &[
    "full-traversal",
    "find-first",
    "accumulate",
    "swallow-exception",
];

// ---- Conditions ----

/// Where a path of a [`Cond`] starts. `this.x` and `x` are one member.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Root {
    Member,
    Base,
    Local,
    Expression,
}

impl Root {
    fn of(root: PathRoot) -> Root {
        match root {
            PathRoot::This | PathRoot::Name => Root::Member,
            PathRoot::Base => Root::Base,
            PathRoot::Local => Root::Local,
            PathRoot::Expression => Root::Expression,
        }
    }
}

/// A comparison operator of a [`Cond`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Op {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl Op {
    fn of(op: CompareOp) -> Op {
        match op {
            CompareOp::Eq => Op::Eq,
            CompareOp::Ne => Op::Ne,
            CompareOp::Lt => Op::Lt,
            CompareOp::Le => Op::Le,
            CompareOp::Gt => Op::Gt,
            CompareOp::Ge => Op::Ge,
        }
    }

    /// The operator of the negated comparison: `!(x == y)` is `x != y`.
    fn negated(self) -> Op {
        match self {
            Op::Eq => Op::Ne,
            Op::Ne => Op::Eq,
            Op::Lt => Op::Ge,
            Op::Le => Op::Gt,
            Op::Gt => Op::Le,
            Op::Ge => Op::Lt,
        }
    }

    fn symbol(self) -> &'static str {
        match self {
            Op::Eq => "==",
            Op::Ne => "!=",
            Op::Lt => "<",
            Op::Le => "<=",
            Op::Gt => ">",
            Op::Ge => ">=",
        }
    }
}

/// A condition as a tree — the contract's `expr` with its operands nested —
/// that can be normalized and compared structurally (KTD13 3).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Cond {
    And(Vec<Cond>),
    Or(Vec<Cond>),
    Not(Box<Cond>),
    Compare(Op, Box<Cond>, Box<Cond>),
    Path(Root, Vec<String>),
    EnumMember(String),
    /// `true`, `false`, `null`, a number as written.
    Constant(String),
    /// A string literal's value, redacted.
    Literal(String),
    Call(Root, Vec<String>, Vec<Cond>),
    Binary(String, Box<Cond>, Box<Cond>),
    /// Redacted.
    Other(String),
}

pub(in crate::card) fn constant_text(c: &Constant) -> String {
    match c {
        Constant::Boolean(b) => b.to_string(),
        Constant::Null => "null".to_string(),
        Constant::Number(n) => n.clone(),
    }
}

fn take(built: &mut [Option<Cond>], index: u32) -> Option<Cond> {
    built.get_mut(index as usize)?.take()
}

fn boxed(built: &mut [Option<Cond>], index: u32) -> Option<Box<Cond>> {
    take(built, index).map(Box::new)
}

impl Cond {
    /// The tree of `condition`, when it has one; its literals are `literals`'
    /// (the body's).
    pub fn of(condition: &Condition, literals: &[StringLiteral]) -> Option<Cond> {
        Cond::from_expr(&condition.expr, literals)
    }

    /// The tree of a contract `expr` list: operands before the node that uses
    /// them, the root last. `None` for an empty, malformed or overlong list.
    pub fn from_expr(expr: &[Expr], literals: &[StringLiteral]) -> Option<Cond> {
        if expr.is_empty() || expr.len() > MAX_EXPR {
            return None;
        }
        let mut built: Vec<Option<Cond>> = Vec::with_capacity(expr.len());
        for e in expr {
            let b = &mut built;
            let c = match e {
                Expr::And(xs) => Cond::And(xs.iter().map(|&j| take(b, j)).collect::<Option<_>>()?),
                Expr::Or(xs) => Cond::Or(xs.iter().map(|&j| take(b, j)).collect::<Option<_>>()?),
                Expr::Not(x) => Cond::Not(boxed(b, *x)?),
                Expr::Compare(op, x, y) => {
                    let x = boxed(b, *x)?;
                    Cond::Compare(Op::of(*op), x, boxed(b, *y)?)
                }
                Expr::Path(p) => Cond::Path(Root::of(p.root), p.segments.clone()),
                Expr::EnumMember(m) => Cond::EnumMember(m.clone()),
                Expr::Constant(c) => Cond::Constant(constant_text(c)),
                Expr::Literal(id) => Cond::Literal(
                    literals
                        .get(*id as usize)
                        .map_or_else(String::new, |l| l.value.clone()),
                ),
                Expr::Call(p, args) => Cond::Call(
                    Root::of(p.root),
                    p.segments.clone(),
                    args.iter().map(|&j| take(b, j)).collect::<Option<_>>()?,
                ),
                Expr::Binary(op, x, y) => {
                    let x = boxed(b, *x)?;
                    Cond::Binary(op.clone(), x, boxed(b, *y)?)
                }
                Expr::Other(text) => Cond::Other(text.clone()),
            };
            built.push(Some(c));
        }
        built.pop().flatten()
    }

    /// The tree with every negation pushed to the leaves (De Morgan; a
    /// negated comparison flips its operator, `!true` is `false`), nested
    /// `&&` and `||` flattened, and the operands of `==`, `!=`, `&&` and `||`
    /// in one order: equal conditions are equal trees.
    pub fn normalized(&self) -> Cond {
        self.norm(false)
    }

    /// The normalized negation: “does not run when” of a condition that must
    /// hold.
    pub fn negated(&self) -> Cond {
        self.norm(true)
    }

    fn norm(&self, negate: bool) -> Cond {
        match self {
            Cond::And(xs) | Cond::Or(xs) => {
                let and = matches!(self, Cond::And(_)) != negate;
                let mut parts: Vec<Cond> = Vec::new();
                for x in xs {
                    match x.norm(negate) {
                        Cond::And(inner) if and => parts.extend(inner),
                        Cond::Or(inner) if !and => parts.extend(inner),
                        other => parts.push(other),
                    }
                }
                parts.sort();
                parts.dedup();
                if parts.len() == 1 {
                    parts.remove(0)
                } else if and {
                    Cond::And(parts)
                } else {
                    Cond::Or(parts)
                }
            }
            Cond::Not(x) => x.norm(!negate),
            Cond::Compare(op, a, b) => {
                let op = if negate { op.negated() } else { *op };
                let (mut a, mut b) = (a.norm(false), b.norm(false));
                if matches!(op, Op::Eq | Op::Ne) && a > b {
                    std::mem::swap(&mut a, &mut b);
                }
                Cond::Compare(op, Box::new(a), Box::new(b))
            }
            Cond::Constant(t) if negate && (t == "true" || t == "false") => {
                Cond::Constant(if t == "true" { "false" } else { "true" }.to_string())
            }
            leaf => {
                let n = match leaf {
                    Cond::Call(r, s, args) => {
                        Cond::Call(*r, s.clone(), args.iter().map(|a| a.norm(false)).collect())
                    }
                    Cond::Binary(op, a, b) => {
                        Cond::Binary(op.clone(), Box::new(a.norm(false)), Box::new(b.norm(false)))
                    }
                    other => other.clone(),
                };
                if negate {
                    Cond::Not(Box::new(n))
                } else {
                    n
                }
            }
        }
    }

    /// The top-level conjuncts of the normalized tree.
    pub fn conjuncts(&self) -> Vec<Cond> {
        match self.normalized() {
            Cond::And(xs) => xs,
            c => vec![c],
        }
    }

    /// The guards “does not run when” a predicate that must hold gives (KTD13
    /// 2): each top-level conjunct, negated.
    pub fn guards_of_predicate(&self) -> Vec<Cond> {
        self.conjuncts().iter().map(Cond::negated).collect()
    }

    /// The tree written as C# (`syntax != SyntaxKind.Sql && !canErrors`).
    pub fn render(&self) -> String {
        let mut out = String::new();
        self.write(&mut out, 0);
        out
    }

    fn precedence(&self) -> u8 {
        match self {
            Cond::Or(_) => 1,
            Cond::And(_) => 2,
            Cond::Compare(..) => 3,
            Cond::Binary(..) => 4,
            _ => 5,
        }
    }

    fn write(&self, out: &mut String, parent: u8) {
        let own = self.precedence();
        let paren = own < parent;
        if paren {
            out.push('(');
        }
        let join = |out: &mut String, xs: &[Cond], sep: &str, at: u8| {
            for (i, x) in xs.iter().enumerate() {
                if i > 0 {
                    out.push_str(sep);
                }
                x.write(out, at);
            }
        };
        match self {
            Cond::And(xs) => join(out, xs, " && ", own),
            Cond::Or(xs) => join(out, xs, " || ", own),
            Cond::Not(x) => {
                out.push('!');
                x.write(out, 5);
            }
            Cond::Compare(op, a, b) => {
                a.write(out, 4);
                out.push_str(&format!(" {} ", op.symbol()));
                b.write(out, 4);
            }
            Cond::Binary(op, a, b) => {
                a.write(out, 5);
                out.push_str(&format!(" {op} "));
                b.write(out, 5);
            }
            Cond::Path(root, segments) => out.push_str(&path_text(*root, segments)),
            Cond::Call(root, segments, args) => {
                out.push_str(&path_text(*root, segments));
                out.push('(');
                join(out, args, ", ", 0);
                out.push(')');
            }
            Cond::EnumMember(t) | Cond::Constant(t) | Cond::Other(t) => out.push_str(t),
            Cond::Literal(v) => out.push_str(&format!("\"{v}\"")),
        }
        if paren {
            out.push(')');
        }
    }
}

fn path_text(root: Root, segments: &[String]) -> String {
    let joined = segments.join(".");
    match root {
        Root::Member | Root::Local => joined,
        Root::Base => format!("base.{joined}"),
        Root::Expression if joined.is_empty() => "…".to_string(),
        Root::Expression => format!("….{joined}"),
    }
}

/// An access path as written (`this.viewModel.Check`).
pub(in crate::card) fn written(p: &AccessPath) -> String {
    let joined = p.segments.join(".");
    match p.root {
        PathRoot::This if joined.is_empty() => "this".to_string(),
        PathRoot::This => format!("this.{joined}"),
        PathRoot::Base if joined.is_empty() => "base".to_string(),
        PathRoot::Base => format!("base.{joined}"),
        PathRoot::Expression => path_text(Root::Expression, &p.segments),
        PathRoot::Local | PathRoot::Name => joined,
    }
}

// ---- The data model ----

/// What runs before a guard and still runs when it stops the rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EffectKind {
    /// A field or property written.
    Write,
    /// A call written as a statement.
    Call,
    /// An event or delegate raised: `X?.Invoke(…)`; the path is `X`.
    Raise,
}

/// One effect of a guarded path (KTD13 1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Effect {
    pub kind: EffectKind,
    pub path: AccessPath,
    pub place: Place,
}

/// Where a precondition was read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Source {
    /// A guard in a body.
    Guard,
    /// The CanExecute delegate of a command.
    CanExecute,
}

/// One place a precondition was found at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Occurrence {
    pub source: Source,
    /// The member whose body holds the guard (the command property for a
    /// CanExecute lambda).
    pub member: NodeKey,
    /// The guard's arm, or the CanExecute predicate.
    pub place: Place,
    /// The members of the type the entry point's code was followed into to
    /// reach it, in call order, the guard's member last; empty when it is in
    /// the entry point's own code.
    pub via: Vec<NodeKey>,
    /// What runs even when this guard stops the rest, in source order, the
    /// caller's first. Always empty for a CanExecute predicate.
    pub effects: Vec<Effect>,
}

/// One precondition of a functionality: when it does not run (R5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Precondition {
    /// The condition under which it does not run, normalized.
    pub when: Cond,
    /// The guard's text as written, or the tree rendered when it was read
    /// only from a predicate.
    pub text: String,
    /// By place.
    pub occurrences: Vec<Occurrence>,
}

/// Why a precondition could not be read (admitted, never guessed).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Gap {
    /// The call of the main work could not be told: an ambiguous overload,
    /// a member two levels up.
    Call(Unresolved),
    /// The main work calls a member of an ancestor; its preconditions are
    /// not followed.
    Ancestor(NodeKey),
    /// A CanExecute predicate the model gives no tree of: the component gave
    /// its returned expression none, or it is a call of nothing the type
    /// declares.
    PredicateNotInModel,
}

/// A precondition that may exist but could not be read.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct AdmittedPrecondition {
    pub place: Place,
    pub member: NodeKey,
    pub gap: Gap,
    pub via: Vec<NodeKey>,
}

/// Why a member counts as guarding something (KTD13 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Guarding {
    /// It is read or called in a condition of the type.
    ReadInCondition,
    /// It is a command's CanExecute delegate, or what the delegate calls.
    CanExecute,
    /// An entry point calls it.
    CalledByEntry,
}

/// What makes a path dead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeadKind {
    /// The whole body returns this constant.
    ConstantReturn(Constant),
    /// The body is empty.
    EmptyBody,
    /// `if (false)`.
    ConstantCondition,
}

/// A dead path of a guarding member (KTD13 4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeadPath {
    /// The member; for a CanExecute lambda, the command property it is
    /// written in.
    pub member: NodeKey,
    pub kind: DeadKind,
    pub place: Place,
    pub guarding: Vec<Guarding>,
}

/// What a write gives a state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Written {
    Constant(Constant),
    /// `x = !x`.
    Toggle,
    /// Anything else.
    Other,
}

impl Written {
    fn order(&self) -> (u8, String) {
        match self {
            Written::Constant(c) => (0, constant_text(c)),
            Written::Toggle => (1, String::new()),
            Written::Other => (2, String::new()),
        }
    }
}

/// One value a state is given, and where.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StateValue {
    pub value: Written,
    /// By place.
    pub places: Vec<Place>,
}

/// One write of a state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StateChange {
    /// The member whose body writes it.
    pub member: NodeKey,
    pub place: Place,
    pub value: Written,
}

/// A state of the type (KTD13 5, R6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct State {
    /// The field, or the property standing for its field.
    pub member: NodeKey,
    /// A property's backing field.
    pub field: Option<NodeKey>,
    /// The constants and toggles it is given, by value.
    pub values: Vec<StateValue>,
    /// The conditions that read it, by place.
    pub reads: Vec<Place>,
    /// Every write of it, by place.
    pub changes: Vec<StateChange>,
}

/// One arm of a choice step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArmStep {
    /// The arm's condition as written; `None` for `else`.
    pub condition: Option<String>,
    pub place: Place,
    /// What this arm calls or passes that no other arm does: callees, enum
    /// members, constants and literals of its arguments (`CheckKind.Syntax`
    /// against `CheckKind.Check`), sorted.
    pub selects: Vec<String>,
}

/// What a step does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum StepKind {
    /// A tag of a loop, an arm or a catch: `full-traversal`, `find-first`,
    /// `accumulate`, `guard-return`, `swallow-exception`.
    Idiom(String),
    /// The getter initializes `field` on first read.
    LazyInit { field: NodeKey },
    /// The setter raises a change notification.
    NotifiesChange,
    /// A loop no idiom names.
    Loop,
    /// A choice of arms.
    Choice { arms: Vec<ArmStep> },
    /// A choice whose arms a parameter of the member tells apart: “the
    /// parameter chooses …”.
    ParameterChoice {
        parameter: String,
        arms: Vec<ArmStep>,
    },
    /// A call or construction of another type's member.
    Call {
        callee: AccessPath,
        /// What it uses, when told ([`super::Chain::used`]).
        member: Option<NodeKey>,
        construct: bool,
    },
    /// A write of a state of the type.
    StateChange { state: NodeKey, value: Written },
}

/// One step of a functionality (R7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Step {
    /// The member whose body it is in.
    pub member: NodeKey,
    pub place: Place,
    pub kind: StepKind,
}

/// A configuration key a functionality passes to a call (R9).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConfigKey {
    pub key: String,
    pub callee: AccessPath,
    pub place: Place,
}

/// What a functionality mentions that leads further (R9).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Mentions {
    /// By key, then place.
    pub config_keys: Vec<ConfigKey>,
    /// The names of other types it works with, sorted.
    pub types: Vec<String>,
    /// The search keywords: the keys and the type names that hold a word,
    /// as written, sorted.
    pub keywords: Vec<String>,
}

impl Mentions {
    /// What the sentence “works with …” lists: the types, then the keys.
    pub fn works_with(&self) -> Vec<&str> {
        let mut keys: Vec<&str> = self.config_keys.iter().map(|k| k.key.as_str()).collect();
        keys.dedup();
        self.types.iter().map(String::as_str).chain(keys).collect()
    }
}

/// What U11 reads of one functionality.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FunctionalityFlow {
    /// The functionality's key.
    pub key: String,
    /// By first place, then condition.
    pub preconditions: Vec<Precondition>,
    /// By place.
    pub admitted: Vec<AdmittedPrecondition>,
    /// Of its members, by member and place.
    pub dead_paths: Vec<DeadPath>,
    /// By member, then place.
    pub steps: Vec<Step>,
    pub mentions: Mentions,
}

/// What U11 reads of one type.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct TypeFlow {
    /// One per functionality, by key.
    pub functionalities: Vec<FunctionalityFlow>,
    /// By member.
    pub states: Vec<State>,
    /// Dead paths of guarding members no functionality holds (a hub), by
    /// member and place.
    pub dead_paths: Vec<DeadPath>,
}

impl TypeFlow {
    /// The flow of the functionality `key`.
    pub fn of(&self, key: &str) -> Option<&FunctionalityFlow> {
        self.functionalities
            .binary_search_by(|f| f.key.as_str().cmp(key))
            .ok()
            .map(|i| &self.functionalities[i])
    }
}

// ---- Reading a body ----

/// A body and the file it is in.
#[derive(Clone, Copy)]
struct Loc<'l> {
    path: &'l str,
    body: &'l MemberBody,
}

impl Loc<'_> {
    fn place(&self, range: crate::extract::bodies::Range) -> Place {
        Place {
            path: self.path.to_string(),
            range,
        }
    }
}

/// The children of `node`, in source order.
pub(crate) fn children(body: &MemberBody, node: u32) -> Vec<&BodyNode> {
    body.nodes
        .iter()
        .skip(node as usize + 1)
        .filter(|n| n.parent == Some(node))
        .collect()
}

fn accesses_at(body: &MemberBody, node: u32) -> impl Iterator<Item = &MemberAccess> {
    body.accesses.iter().filter(move |a| a.node == Some(node))
}

fn invocations_at(body: &MemberBody, node: u32) -> impl Iterator<Item = &Invocation> {
    body.invocations
        .iter()
        .filter(move |i| i.node == Some(node))
}

/// A node whose `condition` decides a branch: an arm, a `switch` subject, a
/// loop, a catch filter. The `condition` a component gives a `return` node
/// is the returned value ([`returned`]), not a test the states and the
/// guarding members are read from.
fn carries_condition(n: &BodyNode) -> bool {
    matches!(
        n.kind,
        NodeKind::Arm | NodeKind::Choice | NodeKind::Loop | NodeKind::Catch
    )
}

/// `return;`: nothing of the model hangs on the node.
pub(crate) fn is_bare_return(body: &MemberBody, r: &BodyNode) -> bool {
    r.kind == NodeKind::Return
        && r.returns.is_none()
        && children(body, r.id).is_empty()
        && accesses_at(body, r.id).next().is_none()
        && invocations_at(body, r.id).next().is_none()
        && !body.literals.iter().any(|l| l.node == Some(r.id))
        && !body.name_ofs.iter().any(|l| l.node == Some(r.id))
}

/// The arm of `choice` when it is an `if` of one arm whose body is one node
/// that `stops` accepts.
fn single_arm<'b>(
    body: &'b MemberBody,
    choice: &BodyNode,
    stops: impl Fn(&BodyNode) -> bool,
) -> Option<&'b BodyNode> {
    // A `switch` carries its subject on the choice: never a guard.
    if choice.kind != NodeKind::Choice || choice.condition.is_some() {
        return None;
    }
    let [arm] = children(body, choice.id)[..] else {
        return None;
    };
    if arm.kind != NodeKind::Arm || arm.condition.is_none() {
        return None;
    }
    let [only] = children(body, arm.id)[..] else {
        return None;
    };
    stops(only).then_some(arm)
}

/// The arm of a guard (see the module docs).
fn guard_arm<'b>(body: &'b MemberBody, choice: &BodyNode) -> Option<&'b BodyNode> {
    single_arm(body, choice, |n| match n.kind {
        NodeKind::Return => is_bare_return(body, n),
        NodeKind::Throw | NodeKind::YieldBreak => true,
        _ => false,
    })
}

/// The arm of `if (c) return false;` in a predicate.
fn false_arm<'b>(body: &'b MemberBody, choice: &BodyNode) -> Option<&'b BodyNode> {
    single_arm(body, choice, |n| {
        n.kind == NodeKind::Return && n.returns == Some(Constant::Boolean(false))
    })
}

/// How a top-level statement takes part in the preconditions.
enum Class<'b> {
    Guard(&'b BodyNode),
    /// Field writes and calls whose result goes nowhere: their effects, and
    /// the call when the statement is one call alone.
    Precursor(Vec<Effect>, Option<&'b Invocation>),
    /// Locals written a construction or a constant: no effect beyond the
    /// member, no main work.
    Local,
    Work,
}

fn is_raise(callee: &AccessPath) -> bool {
    callee.segments.len() >= 2 && callee.segments.last().is_some_and(|s| s == "Invoke")
}

/// Whether a statement only writes locals a construction or a constant
/// (`var evt = new CancelEventArgs();`, `var n = 0;`) and calls nothing but
/// those constructions. A parameter of the member is no local here: an `out`
/// or `ref` one reaches the caller.
fn writes_locals_only(body: &MemberBody, writes: &[&MemberAccess], invs: &[&Invocation]) -> bool {
    let parameter = |name: &str| {
        body.typed_names
            .iter()
            .any(|t| t.kind == DeclarationKind::Parameter && t.scope.is_none() && t.name == name)
    };
    let locals: Vec<&str> = writes
        .iter()
        .filter_map(
            |w| match (w.target.root, w.target.segments.as_slice(), &w.rhs) {
                (
                    PathRoot::Local,
                    [name],
                    Some(ValueRef::Construction(_) | ValueRef::Constant(_)),
                ) if !parameter(name) => Some(name.as_str()),
                _ => None,
            },
        )
        .collect();
    !writes.is_empty()
        && locals.len() == writes.len()
        && invs.iter().all(|i| {
            i.kind == CallKind::Construct
                && matches!(&i.flow, Some(FlowTarget::Local(l)) if locals.contains(&l.as_str()))
        })
}

fn classify<'b>(at: Loc<'b>, n: &'b BodyNode) -> Class<'b> {
    let body = at.body;
    match n.kind {
        NodeKind::Choice => match guard_arm(body, n) {
            Some(arm) => Class::Guard(arm),
            None => Class::Work,
        },
        NodeKind::Statement => {
            let writes: Vec<&MemberAccess> = accesses_at(body, n.id)
                .filter(|a| a.mode == AccessMode::Write)
                .collect();
            let invs: Vec<&Invocation> = invocations_at(body, n.id).collect();
            if writes_locals_only(body, &writes, &invs) {
                return Class::Local;
            }
            if writes
                .iter()
                .any(|w| matches!(w.target.root, PathRoot::Local | PathRoot::Expression))
                || invs.iter().any(|i| {
                    matches!(
                        i.flow,
                        Some(FlowTarget::Local(_) | FlowTarget::MemberReturn)
                    )
                })
            {
                return Class::Work;
            }
            let standalone: Vec<&Invocation> = invs
                .into_iter()
                .filter(|i| i.flow.is_none() && i.kind == CallKind::Call)
                .collect();
            if writes.is_empty() && standalone.is_empty() {
                return Class::Work;
            }
            let mut effects: Vec<Effect> = writes
                .iter()
                .map(|w| Effect {
                    kind: EffectKind::Write,
                    path: w.target.clone(),
                    place: at.place(w.place),
                })
                .collect();
            for inv in &standalone {
                let (kind, path) = if is_raise(&inv.callee) {
                    let mut receiver = inv.callee.clone();
                    receiver.segments.pop();
                    (EffectKind::Raise, receiver)
                } else {
                    (EffectKind::Call, inv.callee.clone())
                };
                effects.push(Effect {
                    kind,
                    path,
                    place: at.place(inv.place),
                });
            }
            effects.sort_by(|a, b| a.place.cmp(&b.place));
            let tail = match (writes.is_empty(), standalone.as_slice()) {
                (true, [one]) => Some(*one),
                _ => None,
            };
            Class::Precursor(effects, tail)
        }
        _ => Class::Work,
    }
}

/// The top level of `node` read for preconditions.
struct Scan<'b> {
    /// Each guard's arm and the effects before it.
    guards: Vec<(&'b BodyNode, Vec<Effect>)>,
    /// The main work when it is one call alone after the last guard, and the
    /// effects before it.
    tail: Option<(&'b Invocation, u32, Vec<Effect>)>,
}

fn scan<'b>(at: Loc<'b>, node: u32) -> Scan<'b> {
    let mut effects: Vec<Effect> = Vec::new();
    let mut guards = Vec::new();
    // The statements after the last guard: the call alone, and the effects
    // before it.
    let mut after: Vec<(Option<(&'b Invocation, u32)>, usize)> = Vec::new();
    let mut work = false;
    for k in children(at.body, node) {
        match classify(at, k) {
            Class::Guard(arm) => {
                guards.push((arm, effects.clone()));
                after.clear();
            }
            Class::Precursor(e, tail) => {
                after.push((tail.map(|t| (t, k.id)), effects.len()));
                effects.extend(e);
            }
            Class::Local => {}
            Class::Work => {
                work = true;
                break;
            }
        }
    }
    let tail = match (work, after.as_slice()) {
        (false, [(Some((inv, node)), mark)]) => Some((*inv, *node, effects[..*mark].to_vec())),
        _ => None,
    };
    Scan { guards, tail }
}

/// What the returned value of a predicate is, as the model gives it.
enum Returned<'b> {
    /// A tree: the `condition` a component gave the `return` node.
    Tree(Cond),
    /// A constant (a dead path, if anything).
    Constant,
    /// One read alone (`p => canErrors`).
    Read(&'b AccessPath),
    /// One call alone (`p => CanCheck()`).
    Call(&'b Invocation),
    /// Anything else: the component gave the returned expression no tree.
    NotInModel,
}

fn returned<'b>(body: &'b MemberBody, r: &BodyNode) -> Returned<'b> {
    let tree = r
        .condition
        .as_ref()
        .and_then(|c| Cond::of(c, &body.literals));
    // A lone call is followed into the member it calls, not read as a tree.
    let tree = match tree {
        Some(Cond::Call(..)) | None => tree,
        Some(c) => return Returned::Tree(c),
    };
    if r.returns.is_some() {
        return Returned::Constant;
    }
    let invs: Vec<&Invocation> = invocations_at(body, r.id).collect();
    let accesses: Vec<&MemberAccess> = accesses_at(body, r.id).collect();
    match (invs.as_slice(), accesses.as_slice()) {
        // `return x;` reads `x` in mode `return` only when it is the whole
        // value; in a lambda the read spans the whole expression.
        ([], [a]) if a.mode == AccessMode::Return || a.place == r.place => {
            Returned::Read(&a.target)
        }
        ([inv], _)
            if inv.kind == CallKind::Call
                && (inv.flow == Some(FlowTarget::MemberReturn) || inv.place == r.place) =>
        {
            Returned::Call(inv)
        }
        _ => tree.map_or(Returned::NotInModel, Returned::Tree),
    }
}

/// A guard or an admitted gap, as found.
#[derive(Debug, Clone)]
enum Found {
    Guard {
        when: Cond,
        text: String,
        occurrence: Occurrence,
    },
    Admitted(AdmittedPrecondition),
}

/// `found` reached through a call of `via`, the caller's `effects` first.
fn prefixed(found: Vec<Found>, via: &NodeKey, effects: &[Effect]) -> Vec<Found> {
    found
        .into_iter()
        .map(|f| match f {
            Found::Guard {
                when,
                text,
                mut occurrence,
            } => {
                occurrence.via.insert(0, via.clone());
                if occurrence.source == Source::Guard {
                    let mut all = effects.to_vec();
                    all.append(&mut occurrence.effects);
                    occurrence.effects = all;
                }
                Found::Guard {
                    when,
                    text,
                    occurrence,
                }
            }
            Found::Admitted(mut a) => {
                a.via.insert(0, via.clone());
                Found::Admitted(a)
            }
        })
        .collect()
}

/// A guard found at `place` in `member`'s body.
fn guard(
    source: Source,
    member: &NodeKey,
    place: Place,
    when: Cond,
    text: String,
    effects: Vec<Effect>,
) -> Found {
    Found::Guard {
        when,
        text,
        occurrence: Occurrence {
            source,
            member: member.clone(),
            place,
            via: Vec::new(),
            effects,
        },
    }
}

/// Whether a deriving type may replace `member`'s body: it is `virtual` or
/// `abstract`, or an `override` that is not `sealed`, on some part.
fn overridable(ix: &Index<'_>, member: &NodeKey) -> bool {
    ix.layer.metas_of(member).iter().any(|(_, meta)| {
        let has = |word: &str| meta.modifiers.iter().any(|m| m == word);
        has("virtual") || has("abstract") || (has("override") && !has("sealed"))
    })
}

/// Whether a bound delegate is the command's CanExecute (see the module
/// docs).
pub(crate) fn is_can_execute(ix: &Index<'_>, d: &BoundDelegate) -> bool {
    d.invoked_by.iter().any(|m| ix.name_of(m) == "CanExecute")
        || ix.declared_type(&d.field).is_ok_and(|(name, args)| {
            let last = name.rsplit('.').next().unwrap_or(&name);
            last == "Predicate"
                || (last == "Func" && args.last().is_some_and(|a| a == "bool" || a == "Boolean"))
        })
}

// ---- The analysis of one type ----

struct Flow<'a, 'l> {
    ix: &'a Index<'l>,
    ty: &'a NodeKey,
    ancestors: Vec<&'a NodeKey>,
    guards_memo: BTreeMap<NodeKey, Vec<Found>>,
    predicate_memo: BTreeMap<NodeKey, Vec<Found>>,
    visiting: BTreeSet<NodeKey>,
}

/// What a call of the main work leads to.
enum Callee {
    Own(NodeKey),
    Gap(Gap),
}

impl<'a, 'l> Flow<'a, 'l> {
    fn own(&self, m: &NodeKey) -> bool {
        self.ix.owner_of(m) == Some(self.ty)
    }

    fn body_at<'p>(&self, path: &'p str, index: usize) -> Option<Loc<'p>>
    where
        'l: 'p,
    {
        let file = self.ix.layer.file(path)?;
        Some(Loc {
            path,
            body: file.model.members.get(index)?,
        })
    }

    /// The one member of the type a call written by name calls, resolved by
    /// name and arity; `None` for a call of another object or of nothing the
    /// type declares.
    fn callee(&self, at: Loc<'_>, node: u32, inv: &Invocation) -> Option<Callee> {
        if inv.kind != CallKind::Call
            || !matches!(
                inv.callee.root,
                PathRoot::This | PathRoot::Name | PathRoot::Base
            )
            || inv.callee.segments.len() != 1
        {
            return None;
        }
        let scope = Scope {
            ty: self.ty,
            body: Some(at.body),
            node: Some(node),
        };
        let chain = self.ix.resolve(&scope, &inv.callee, Some(inv.arity));
        match (&chain.stopped, chain.links.as_slice()) {
            (None, [link]) if self.ix.symbol(&link.member).is_some_and(|s| s.is_method()) => {
                Some(if &link.owner == self.ty {
                    Callee::Own(link.member.clone())
                } else {
                    Callee::Gap(Gap::Ancestor(link.member.clone()))
                })
            }
            (Some(why @ (Unresolved::AmbiguousMember(_) | Unresolved::BeyondOneLevel(_))), _) => {
                Some(Callee::Gap(Gap::Call(why.clone())))
            }
            _ => None,
        }
    }

    fn admitted(member: &NodeKey, place: Place, gap: Gap) -> Found {
        Found::Admitted(AdmittedPrecondition {
            place,
            member: member.clone(),
            gap,
            via: Vec::new(),
        })
    }

    /// The preconditions of the code at the top level of `node` in `m`'s
    /// body (see the module docs).
    fn guards_in(&mut self, m: &NodeKey, at: Loc<'_>, node: u32) -> Vec<Found> {
        let scan = scan(at, node);
        let mut out = Vec::new();
        for (arm, effects) in scan.guards {
            let Some(condition) = &arm.condition else {
                continue;
            };
            let when = Cond::of(condition, &at.body.literals)
                .map_or_else(|| Cond::Other(condition.text.clone()), |c| c.normalized());
            out.push(guard(
                Source::Guard,
                m,
                at.place(arm.place),
                when,
                condition.text.clone(),
                effects,
            ));
        }
        if let Some((inv, statement, effects)) = scan.tail {
            match self.callee(at, statement, inv) {
                Some(Callee::Own(callee)) => {
                    let found = self.member_guards(&callee);
                    out.extend(prefixed(found, &callee, &effects));
                }
                Some(Callee::Gap(gap)) => out.push(Self::admitted(m, at.place(inv.place), gap)),
                None => {}
            }
        }
        out
    }

    /// The preconditions of a method of the type, as its own code gives
    /// them (relative: `via` empty, its own effects only).
    fn member_guards(&mut self, m: &NodeKey) -> Vec<Found> {
        if let Some(found) = self.guards_memo.get(m) {
            return found.clone();
        }
        if !self.visiting.insert(m.clone()) {
            return Vec::new();
        }
        let ix = self.ix;
        let mut out = Vec::new();
        for pb in ix.layer.bodies_of(m) {
            if pb.by != PairedBy::Name || pb.body.kind != BodyKind::Method {
                continue;
            }
            let at = Loc {
                path: pb.path,
                body: pb.body,
            };
            out.extend(self.guards_in(m, at, pb.body.root));
        }
        self.visiting.remove(m);
        self.guards_memo.insert(m.clone(), out.clone());
        out
    }

    /// The preconditions a CanExecute predicate at the top level of `node`
    /// gives (see the module docs).
    fn predicate_in(&mut self, m: &NodeKey, at: Loc<'_>, node: u32) -> Vec<Found> {
        let mut out = Vec::new();
        for k in children(at.body, node) {
            if let Some(arm) = false_arm(at.body, k) {
                if let Some(condition) = &arm.condition {
                    let when = Cond::of(condition, &at.body.literals)
                        .map_or_else(|| Cond::Other(condition.text.clone()), |c| c.normalized());
                    out.push(guard(
                        Source::CanExecute,
                        m,
                        at.place(arm.place),
                        when,
                        condition.text.clone(),
                        Vec::new(),
                    ));
                }
                continue;
            }
            // A lambda's expression body that is a call is a `statement`
            // node; it is the returned value all the same.
            let expression_body = k.kind == NodeKind::Statement
                && at
                    .body
                    .nodes
                    .get(node as usize)
                    .is_some_and(|n| n.kind == NodeKind::Lambda);
            if k.kind != NodeKind::Return && !expression_body {
                out.push(Self::admitted(
                    m,
                    at.place(k.place),
                    Gap::PredicateNotInModel,
                ));
                return out;
            }
            let from_tree = |tree: Cond, out: &mut Vec<Found>| {
                for g in tree.guards_of_predicate() {
                    let text = g.render();
                    out.push(guard(
                        Source::CanExecute,
                        m,
                        at.place(k.place),
                        g,
                        text,
                        Vec::new(),
                    ));
                }
            };
            match returned(at.body, k) {
                Returned::Tree(tree) => from_tree(tree, &mut out),
                Returned::Constant => {}
                Returned::Read(path) => from_tree(
                    Cond::Path(Root::of(path.root), path.segments.clone()),
                    &mut out,
                ),
                Returned::Call(inv) => match self.callee(at, k.id, inv) {
                    Some(Callee::Own(callee)) => {
                        let found = self.predicate_member(&callee);
                        out.extend(prefixed(found, &callee, &[]));
                    }
                    Some(Callee::Gap(gap)) => out.push(Self::admitted(m, at.place(inv.place), gap)),
                    None => out.push(Self::admitted(
                        m,
                        at.place(k.place),
                        Gap::PredicateNotInModel,
                    )),
                },
                Returned::NotInModel => out.push(Self::admitted(
                    m,
                    at.place(k.place),
                    Gap::PredicateNotInModel,
                )),
            }
            return out;
        }
        out
    }

    fn predicate_member(&mut self, m: &NodeKey) -> Vec<Found> {
        if let Some(found) = self.predicate_memo.get(m) {
            return found.clone();
        }
        if !self.visiting.insert(m.clone()) {
            return Vec::new();
        }
        let ix = self.ix;
        let mut out = Vec::new();
        for pb in ix.layer.bodies_of(m) {
            let fits = match pb.body.kind {
                BodyKind::Method => pb.by == PairedBy::Name,
                BodyKind::Getter | BodyKind::ExpressionProperty => true,
                _ => false,
            };
            if !fits {
                continue;
            }
            let at = Loc {
                path: pb.path,
                body: pb.body,
            };
            out.extend(self.predicate_in(m, at, pb.body.root));
        }
        self.visiting.remove(m);
        self.predicate_memo.insert(m.clone(), out.clone());
        out
    }

    /// Everything an entry point's code gives as preconditions.
    fn entry_found(&mut self, e: &EntryPoint) -> Vec<Found> {
        let mut out = Vec::new();
        if e.kind == EntryKind::Command {
            for site in &e.commands {
                let Some(at) = self.body_at(&site.region.path, site.region.body) else {
                    continue;
                };
                for d in &site.delegates {
                    let predicate = is_can_execute(self.ix, d);
                    match &d.value {
                        ValueRef::Lambda(id) => {
                            let Some(l) = at.body.lambdas.get(*id as usize) else {
                                continue;
                            };
                            out.extend(if predicate {
                                self.predicate_in(&e.member, at, l.body)
                            } else {
                                self.guards_in(&e.member, at, l.body)
                            });
                        }
                        ValueRef::Access(_) => {
                            for t in &d.targets {
                                let Some(m) = t.member.as_ref() else {
                                    continue;
                                };
                                if !self.ix.symbol(m).is_some_and(|s| s.is_method()) {
                                    continue;
                                }
                                if !self.own(m) {
                                    let gap = Gap::Ancestor(m.clone());
                                    out.push(Self::admitted(&e.member, at.place(t.place), gap));
                                    continue;
                                }
                                let found = if predicate {
                                    self.predicate_member(m)
                                } else {
                                    self.member_guards(m)
                                };
                                out.extend(prefixed(found, m, &[]));
                            }
                        }
                        _ => {}
                    }
                }
            }
            return out;
        }
        for r in &e.regions {
            let Some(at) = self.body_at(&r.path, r.body) else {
                continue;
            };
            if at.body.kind == BodyKind::LocalFunction {
                continue;
            }
            let node = r.node.unwrap_or(at.body.root);
            out.extend(self.guards_in(&e.member, at, node));
        }
        out
    }
}

/// Equal conditions are one precondition; each keeps its places.
fn merge(found: Vec<Found>) -> (Vec<Precondition>, Vec<AdmittedPrecondition>) {
    let mut by: BTreeMap<Cond, Vec<(Occurrence, String)>> = BTreeMap::new();
    let mut admitted = Vec::new();
    for f in found {
        match f {
            Found::Guard {
                when,
                text,
                occurrence,
            } => by.entry(when).or_default().push((occurrence, text)),
            Found::Admitted(a) => admitted.push(a),
        }
    }
    let mut preconditions: Vec<Precondition> = by
        .into_iter()
        .map(|(when, mut list)| {
            // The shortest way to a place is the one kept.
            list.sort_by(|(a, _), (b, _)| {
                (&a.place, a.source, &a.member, a.via.len(), &a.via).cmp(&(
                    &b.place,
                    b.source,
                    &b.member,
                    b.via.len(),
                    &b.via,
                ))
            });
            list.dedup_by(|(a, _), (b, _)| {
                a.place == b.place && a.source == b.source && a.member == b.member
            });
            let text = list
                .iter()
                .find(|(o, _)| o.source == Source::Guard)
                .or(list.first())
                .map(|(_, t)| t.clone())
                .unwrap_or_else(|| when.render());
            Precondition {
                text,
                occurrences: list.into_iter().map(|(o, _)| o).collect(),
                when,
            }
        })
        .collect();
    preconditions.sort_by(|a, b| {
        let first = |p: &Precondition| p.occurrences.first().map(|o| o.place.clone());
        (first(a), &a.when).cmp(&(first(b), &b.when))
    });
    admitted.sort();
    admitted.dedup_by(|a, b| a.place == b.place && a.member == b.member && a.gap == b.gap);
    (preconditions, admitted)
}

/// A stretch of a functionality's code: a whole body of a member, or the
/// code under a node of it.
struct Stretch<'l> {
    member: NodeKey,
    at: Loc<'l>,
    index: usize,
    node: Option<u32>,
}

impl Stretch<'_> {
    fn holds(&self, n: Option<u32>) -> bool {
        match (self.node, n) {
            (None, _) => true,
            (Some(root), Some(n)) => n != root && within(self.at.body, n, root),
            (Some(_), None) => false,
        }
    }
}

/// The state index: what each own value member stands for.
struct States {
    /// The state a field or property is, by member.
    of: BTreeMap<NodeKey, NodeKey>,
    states: Vec<State>,
}

impl<'a, 'l> Flow<'a, 'l> {
    fn local(&self, m: &NodeKey) -> bool {
        self.ix.declared_along(m, self.ty, &self.ancestors)
    }

    /// The own value member an access names directly (`x`, `this.x`).
    fn own_value(&self, at: Loc<'_>, node: Option<u32>, path: &AccessPath) -> Option<NodeKey> {
        if !matches!(path.root, PathRoot::This | PathRoot::Name) || path.segments.len() != 1 {
            return None;
        }
        let scope = Scope {
            ty: self.ty,
            body: Some(at.body),
            node,
        };
        let chain = self.ix.resolve(&scope, path, None);
        let [link] = chain.links.as_slice() else {
            return None;
        };
        (&link.owner == self.ty && self.ix.symbol(&link.member).is_some_and(|s| s.is_value()))
            .then(|| link.member.clone())
    }

    fn own_members(&self) -> Vec<&'a NodeKey> {
        let ix = self.ix;
        ix.members_of(self.ty)
            .iter()
            .filter(|k| ix.symbol(k).is_some_and(|s| !s.is_type()))
            .collect()
    }

    fn has_tag(&self, m: &NodeKey, tag: &str) -> bool {
        self.ix
            .layer
            .metas_of(m)
            .iter()
            .any(|(_, meta)| meta.tags.iter().any(|t| t.name == tag))
    }

    fn is_bool(&self, m: &NodeKey) -> bool {
        self.ix
            .declared_type(m)
            .is_ok_and(|(name, _)| name == "bool" || name == "Boolean")
    }

    /// The states of the type (see the module docs).
    fn states(&self) -> States {
        let ix = self.ix;
        let members = self.own_members();
        // A property standing for the one field its setter writes `value` to.
        let mut field_of: BTreeMap<NodeKey, NodeKey> = BTreeMap::new();
        let mut property_of: BTreeMap<NodeKey, NodeKey> = BTreeMap::new();
        for &m in &members {
            if ix.symbol(m).is_none_or(|s| s.kind != "property") {
                continue;
            }
            let mut fields: BTreeSet<NodeKey> = BTreeSet::new();
            for pb in ix.layer.bodies_of(m) {
                if pb.body.kind != BodyKind::Setter {
                    continue;
                }
                let at = Loc {
                    path: pb.path,
                    body: pb.body,
                };
                for a in &pb.body.accesses {
                    let from_value = matches!(&a.rhs, Some(ValueRef::Access(p))
                        if p.root == PathRoot::Local && p.segments == ["value"]);
                    if a.mode == AccessMode::Write && from_value {
                        fields.extend(self.own_value(at, a.node, &a.target));
                    }
                }
            }
            if let [field] = fields.into_iter().collect::<Vec<_>>().as_slice() {
                if field != m && !property_of.contains_key(field) {
                    field_of.insert(m.clone(), field.clone());
                    property_of.insert(field.clone(), m.clone());
                }
            }
        }
        let state_of = |x: NodeKey| property_of.get(&x).cloned().unwrap_or(x);

        let mut changes: BTreeMap<NodeKey, Vec<StateChange>> = BTreeMap::new();
        let mut reads: BTreeMap<NodeKey, BTreeSet<Place>> = BTreeMap::new();
        for &m in &members {
            let notifying = self.has_tag(m, "notifies-change");
            for pb in ix.layer.bodies_of(m) {
                let at = Loc {
                    path: pb.path,
                    body: pb.body,
                };
                let setter = pb.body.kind == BodyKind::Setter;
                for a in &pb.body.accesses {
                    if a.mode != AccessMode::Write {
                        continue;
                    }
                    let Some(x) = self.own_value(at, a.node, &a.target) else {
                        continue;
                    };
                    let from_value = matches!(&a.rhs, Some(ValueRef::Access(p))
                        if p.root == PathRoot::Local && p.segments == ["value"]);
                    if setter && from_value && field_of.get(m) == Some(&x) {
                        // The setter's own write of its field.
                        continue;
                    }
                    let state = state_of(x.clone());
                    let value = match &a.rhs {
                        Some(ValueRef::Constant(c)) => Written::Constant(c.clone()),
                        Some(ValueRef::Other) if self.toggles(at, a, &state) => Written::Toggle,
                        _ => Written::Other,
                    };
                    changes.entry(state).or_default().push(StateChange {
                        member: m.clone(),
                        place: at.place(a.place),
                        value,
                    });
                }
                if setter && notifying {
                    // Absorbed by `notifies-change`.
                    continue;
                }
                for n in pb.body.nodes.iter().filter(|n| carries_condition(n)) {
                    let Some(condition) = &n.condition else {
                        continue;
                    };
                    for p in &condition.reads {
                        if let Some(x) = self.own_value(at, Some(n.id), p) {
                            reads
                                .entry(state_of(x))
                                .or_default()
                                .insert(at.place(n.place));
                        }
                    }
                }
            }
        }
        let mut states = Vec::new();
        let mut of = BTreeMap::new();
        for (member, mut list) in changes {
            let Some(read) = reads.remove(&member) else {
                continue;
            };
            let set = list
                .iter()
                .filter(|c| c.value != Written::Other)
                .map(|c| &c.place)
                .collect::<BTreeSet<_>>()
                .len();
            if set < 2 {
                continue;
            }
            list.sort_by(|a, b| (&a.place, &a.member).cmp(&(&b.place, &b.member)));
            let mut values: Vec<StateValue> = Vec::new();
            for c in list.iter().filter(|c| c.value != Written::Other) {
                match values.iter_mut().find(|v| v.value == c.value) {
                    Some(v) => v.places.push(c.place.clone()),
                    None => values.push(StateValue {
                        value: c.value.clone(),
                        places: vec![c.place.clone()],
                    }),
                }
            }
            values.sort_by_key(|v| v.value.order());
            let field = field_of.get(&member).cloned();
            of.insert(member.clone(), member.clone());
            if let Some(f) = &field {
                of.insert(f.clone(), member.clone());
            }
            states.push(State {
                member,
                field,
                values,
                reads: read.into_iter().collect(),
                changes: list,
            });
        }
        States { of, states }
    }

    /// `x = !x`: a `bool` written from an expression that reads only itself.
    fn toggles(&self, at: Loc<'_>, write: &MemberAccess, state: &NodeKey) -> bool {
        let Some(node) = write.node else {
            return false;
        };
        if !self.is_bool(state) || invocations_at(at.body, node).next().is_some() {
            return false;
        }
        let reads: Vec<&MemberAccess> = accesses_at(at.body, node)
            .filter(|a| a.mode == AccessMode::Read)
            .collect();
        !reads.is_empty()
            && reads.iter().all(|r| {
                self.own_value(at, Some(node), &r.target)
                    .is_some_and(|x| &x == state || self.states_field(state, &x))
            })
    }

    /// Whether `x` is the backing field of the property `state`.
    fn states_field(&self, state: &NodeKey, x: &NodeKey) -> bool {
        let ix = self.ix;
        ix.layer.bodies_of(state).iter().any(|pb| {
            pb.body.kind == BodyKind::Setter
                && pb.body.accesses.iter().any(|a| {
                    a.mode == AccessMode::Write
                        && a.target.segments.last().is_some_and(|s| s == ix.name_of(x))
                })
        })
    }

    /// The members that guard something, and why (KTD13 4).
    fn guarding(&self, functionalities: &[Functionality]) -> BTreeMap<NodeKey, BTreeSet<Guarding>> {
        let ix = self.ix;
        let mut out: BTreeMap<NodeKey, BTreeSet<Guarding>> = BTreeMap::new();
        for &m in &self.own_members() {
            for pb in ix.layer.bodies_of(m) {
                let at = Loc {
                    path: pb.path,
                    body: pb.body,
                };
                for n in pb.body.nodes.iter().filter(|n| carries_condition(n)) {
                    let Some(condition) = &n.condition else {
                        continue;
                    };
                    let scope = Scope {
                        ty: self.ty,
                        body: Some(at.body),
                        node: Some(n.id),
                    };
                    let mut named: Vec<(&AccessPath, Option<u32>)> =
                        condition.reads.iter().map(|p| (p, None)).collect();
                    for e in &condition.expr {
                        if let Expr::Call(p, args) = e {
                            named.push((p, Some(args.len() as u32)));
                        }
                    }
                    for (p, call) in named {
                        let chain = ix.resolve(&scope, p, call);
                        if let Some(x) = chain.used(ix, self.ty).filter(|x| self.own(x)) {
                            out.entry(x.clone())
                                .or_default()
                                .insert(Guarding::ReadInCondition);
                        }
                    }
                }
            }
        }
        for f in functionalities {
            for e in &f.entry_points {
                for t in &e.targets {
                    if let Some(x) = t.member.as_ref().filter(|x| self.own(x)) {
                        out.entry(x.clone())
                            .or_default()
                            .insert(Guarding::CalledByEntry);
                    }
                }
                for site in &e.commands {
                    for d in site.delegates.iter().filter(|d| is_can_execute(ix, d)) {
                        for x in d.targets.iter().filter_map(|t| t.member.as_ref()) {
                            if self.own(x) {
                                out.entry(x.clone())
                                    .or_default()
                                    .insert(Guarding::CanExecute);
                            }
                        }
                    }
                }
            }
        }
        out
    }

    /// The dead paths of the guarding members, and of CanExecute lambdas, by
    /// the functionality key that holds them (`None`: none does).
    fn dead_paths(
        &self,
        functionalities: &[Functionality],
    ) -> BTreeMap<Option<String>, Vec<DeadPath>> {
        let ix = self.ix;
        let holder: BTreeMap<&NodeKey, &str> = functionalities
            .iter()
            .flat_map(|f| f.members.iter().map(move |m| (m, f.key.as_str())))
            .collect();
        let mut out: BTreeMap<Option<String>, Vec<DeadPath>> = BTreeMap::new();
        for (m, why) in self.guarding(functionalities) {
            let kind = ix.symbol(&m).map(|s| s.kind.clone()).unwrap_or_default();
            let guarding: Vec<Guarding> = why.into_iter().collect();
            // A deriving type may replace the body: what it returns, or that
            // it does nothing, is not what a call of the member does.
            let replaceable = overridable(ix, &m);
            let mut found: Vec<(DeadKind, Place)> = Vec::new();
            for pb in ix.layer.bodies_of(&m) {
                let fits = match (kind.as_str(), pb.body.kind) {
                    ("method", BodyKind::Method) => pb.by == PairedBy::Name,
                    ("property", BodyKind::Getter | BodyKind::ExpressionProperty) => true,
                    _ => false,
                };
                if !fits {
                    continue;
                }
                let at = Loc {
                    path: pb.path,
                    body: pb.body,
                };
                let root = pb.body.root;
                match children(pb.body, root).as_slice() {
                    _ if replaceable => {}
                    [] if pb.body.kind == BodyKind::Method => {
                        if let Some(r) = pb.body.nodes.get(root as usize) {
                            found.push((DeadKind::EmptyBody, at.place(r.place)));
                        }
                    }
                    [r] if r.kind == NodeKind::Return => {
                        if let Some(c) = &r.returns {
                            found.push((DeadKind::ConstantReturn(c.clone()), at.place(r.place)));
                        }
                    }
                    _ => {}
                }
                for n in &pb.body.nodes {
                    if n.kind == NodeKind::Arm
                        && n.condition
                            .as_ref()
                            .is_some_and(|c| c.constant == Some(false))
                    {
                        found.push((DeadKind::ConstantCondition, at.place(n.place)));
                    }
                }
            }
            let key = holder.get(&m).map(|k| k.to_string());
            for (kind, place) in found {
                out.entry(key.clone()).or_default().push(DeadPath {
                    member: m.clone(),
                    kind,
                    place,
                    guarding: guarding.clone(),
                });
            }
        }
        // A CanExecute lambda that is a constant itself.
        for f in functionalities {
            for e in &f.entry_points {
                for site in &e.commands {
                    let Some(at) = self.body_at(&site.region.path, site.region.body) else {
                        continue;
                    };
                    for d in site.delegates.iter().filter(|d| is_can_execute(ix, d)) {
                        let ValueRef::Lambda(id) = d.value else {
                            continue;
                        };
                        let Some(l) = at.body.lambdas.get(id as usize) else {
                            continue;
                        };
                        if let [r] = children(at.body, l.body).as_slice() {
                            if let (NodeKind::Return, Some(c)) = (&r.kind, &r.returns) {
                                out.entry(Some(f.key.clone())).or_default().push(DeadPath {
                                    member: e.member.clone(),
                                    kind: DeadKind::ConstantReturn(c.clone()),
                                    place: at.place(r.place),
                                    guarding: vec![Guarding::CanExecute],
                                });
                            }
                        }
                    }
                }
            }
        }
        for list in out.values_mut() {
            list.sort_by(|a, b| (&a.member, &a.place).cmp(&(&b.member, &b.place)));
            list.dedup();
        }
        out
    }

    /// The stretches of code a functionality is: the bodies of its members,
    /// and the regions of its entry points not inside one of them.
    fn stretches<'p>(&self, f: &'p Functionality) -> Vec<Stretch<'p>>
    where
        'l: 'p,
    {
        let mut out: Vec<Stretch<'p>> = Vec::new();
        let mut whole: BTreeSet<(&str, usize)> = BTreeSet::new();
        for m in &f.members {
            for at in bodies(self.ix, m) {
                if whole.insert((at.path, at.index)) {
                    out.push(Stretch {
                        member: m.clone(),
                        at: Loc {
                            path: at.path,
                            body: at.body,
                        },
                        index: at.index,
                        node: None,
                    });
                }
            }
        }
        let mut seen: BTreeSet<(&str, usize, Option<u32>)> = BTreeSet::new();
        for e in &f.entry_points {
            for r in &e.regions {
                if whole.contains(&(r.path.as_str(), r.body))
                    || !seen.insert((r.path.as_str(), r.body, r.node))
                {
                    continue;
                }
                let Some(at) = self.body_at(&r.path, r.body) else {
                    continue;
                };
                out.push(Stretch {
                    member: e.member.clone(),
                    at,
                    index: r.body,
                    node: r.node,
                });
            }
        }
        out.sort_by(|a, b| {
            (&a.member, a.at.path, a.index, a.node).cmp(&(&b.member, b.at.path, b.index, b.node))
        });
        out
    }

    /// The steps of a functionality (see the module docs).
    fn steps(&self, stretches: &[Stretch<'_>], states: &States) -> Vec<Step> {
        let ix = self.ix;
        // The getters that initialize a field on first read.
        let mut lazy: BTreeMap<String, NodeKey> = BTreeMap::new();
        for &m in &self.own_members() {
            for (_, meta) in ix.layer.metas_of(m) {
                for t in meta.tags.iter().filter(|t| t.name == "lazy-init") {
                    if let Some(property) = &t.value {
                        lazy.insert(property.clone(), m.clone());
                    }
                }
            }
        }
        let mut out = Vec::new();
        for s in stretches {
            let body = s.at.body;
            let step = |place, kind| Step {
                member: s.member.clone(),
                place: s.at.place(place),
                kind,
            };
            let notifying =
                body.kind == BodyKind::Setter && self.has_tag(&s.member, "notifies-change");
            let lazy_field = lazy
                .get(ix.name_of(&s.member))
                .filter(|_| s.node.is_none())
                .filter(|_| matches!(body.kind, BodyKind::Getter | BodyKind::ExpressionProperty));
            if s.node.is_none() {
                if let Some(root) = body.nodes.get(body.root as usize) {
                    if notifying {
                        out.push(step(root.place, StepKind::NotifiesChange));
                    }
                    if let Some(field) = lazy_field {
                        out.push(step(
                            root.place,
                            StepKind::LazyInit {
                                field: field.clone(),
                            },
                        ));
                    }
                }
            }
            // The null test of a lazy getter is its `lazy-init` step.
            let tests_lazy = |n: &BodyNode| {
                lazy_field.is_some_and(|field| {
                    children(body, n.id).iter().any(|arm| {
                        arm.condition.as_ref().is_some_and(|c| {
                            c.reads.iter().any(|p| {
                                self.own_value(s.at, Some(arm.id), p).as_ref() == Some(field)
                            })
                        })
                    })
                })
            };
            let params: BTreeSet<&str> = body
                .typed_names
                .iter()
                .filter(|t| t.kind == DeclarationKind::Parameter && t.scope.is_none())
                .map(|t| t.name.as_str())
                .collect();
            for n in body.nodes.iter().filter(|n| s.holds(Some(n.id))) {
                let idioms: Vec<&str> = n
                    .tags
                    .iter()
                    .map(|t| t.name.as_str())
                    .filter(|t| NODE_IDIOMS.contains(t))
                    .collect();
                match n.kind {
                    NodeKind::Loop if idioms.is_empty() => out.push(step(n.place, StepKind::Loop)),
                    NodeKind::Loop | NodeKind::Catch => {
                        for i in idioms {
                            out.push(step(n.place, StepKind::Idiom(i.to_string())));
                        }
                    }
                    NodeKind::Arm
                        if !notifying && n.tags.iter().any(|t| t.name == "guard-return") =>
                    {
                        out.push(step(n.place, StepKind::Idiom("guard-return".to_string())));
                    }
                    NodeKind::Choice if !tests_lazy(n) => {
                        if let Some(kind) = self.choice(s.at, n, &params) {
                            out.push(step(n.place, kind));
                        }
                    }
                    _ => {}
                }
            }
            for inv in body.invocations.iter().filter(|i| s.holds(i.node)) {
                if let Some(kind) = self.call_step(s.at, inv) {
                    out.push(step(inv.place, kind));
                }
            }
            for a in body.accesses.iter().filter(|a| s.holds(a.node)) {
                if a.mode != AccessMode::Write {
                    continue;
                }
                let Some(state) = self
                    .own_value(s.at, a.node, &a.target)
                    .and_then(|x| states.of.get(&x))
                else {
                    continue;
                };
                // The setter's own write of its field is the state's setter,
                // not a change.
                if body.kind == BodyKind::Setter && state == &s.member {
                    continue;
                }
                let value = states
                    .states
                    .iter()
                    .find(|st| &st.member == state)
                    .and_then(|st| st.changes.iter().find(|c| c.place == s.at.place(a.place)))
                    .map_or(Written::Other, |c| c.value.clone());
                out.push(step(
                    a.place,
                    StepKind::StateChange {
                        state: state.clone(),
                        value,
                    },
                ));
            }
        }
        out.sort_by(|a, b| (&a.member, &a.place).cmp(&(&b.member, &b.place)));
        out
    }

    /// A choice as a step: none for a guard (a precondition and its
    /// `guard-return` step) or for a key dispatch (its arms are entry
    /// points).
    fn choice(&self, at: Loc<'_>, n: &BodyNode, params: &BTreeSet<&str>) -> Option<StepKind> {
        let body = at.body;
        if guard_arm(body, n).is_some() {
            return None;
        }
        let arms: Vec<&BodyNode> = children(body, n.id)
            .into_iter()
            .filter(|a| a.kind == NodeKind::Arm)
            .collect();
        if arms.is_empty()
            || arms
                .iter()
                .any(|a| a.tags.iter().any(|t| t.name == "key-compare"))
        {
            return None;
        }
        let picks: Vec<BTreeSet<String>> = arms.iter().map(|a| picks(body, a.id)).collect();
        let common: BTreeSet<String> = match picks.split_first() {
            Some((first, rest)) if !rest.is_empty() => first
                .iter()
                .filter(|x| rest.iter().all(|r| r.contains(*x)))
                .cloned()
                .collect(),
            _ => picks.first().cloned().unwrap_or_default(),
        };
        let steps: Vec<ArmStep> = arms
            .iter()
            .zip(&picks)
            .map(|(a, p)| ArmStep {
                condition: a.condition.as_ref().map(|c| c.text.clone()),
                place: at.place(a.place),
                selects: p.difference(&common).cloned().collect(),
            })
            .collect();
        let parameter = std::iter::once(n)
            .chain(arms.iter().copied())
            .filter_map(|x| x.condition.as_ref())
            .flat_map(|c| {
                c.reads.iter().chain(c.expr.iter().filter_map(|e| match e {
                    Expr::Path(p) => Some(p),
                    _ => None,
                }))
            })
            .filter(|p| p.root == PathRoot::Local)
            .filter_map(|p| p.segments.first())
            .find(|name| params.contains(name.as_str()))
            .cloned();
        Some(match parameter {
            Some(parameter) => StepKind::ParameterChoice {
                parameter,
                arms: steps,
            },
            None => StepKind::Choice { arms: steps },
        })
    }

    /// A call or construction as a step: one of another type's member.
    fn call_step(&self, at: Loc<'_>, inv: &Invocation) -> Option<StepKind> {
        let construct = match inv.kind {
            CallKind::Call => false,
            CallKind::Construct => true,
            CallKind::ThisInit | CallKind::BaseInit => return None,
        };
        if is_log_like(&inv.callee) {
            return None;
        }
        if construct {
            if inv.callee.segments.is_empty() {
                return None;
            }
            let own = self
                .ix
                .type_named(&inv.callee.segments.join("."))
                .is_ok_and(|t| &t == self.ty);
            return (!own).then(|| StepKind::Call {
                callee: inv.callee.clone(),
                member: None,
                construct,
            });
        }
        // A local function or a delegate held in a local.
        if inv.callee.root == PathRoot::Local && inv.callee.segments.len() == 1 {
            return None;
        }
        let scope = Scope {
            ty: self.ty,
            body: Some(at.body),
            node: inv.node,
        };
        let chain = self.ix.resolve(&scope, &inv.callee, Some(inv.arity));
        let member = chain.used(self.ix, self.ty).cloned();
        let own_overload =
            chain.links.is_empty() && matches!(chain.stopped, Some(Unresolved::AmbiguousMember(_)));
        if own_overload || member.as_ref().is_some_and(|m| self.own(m)) {
            return None;
        }
        Some(StepKind::Call {
            callee: inv.callee.clone(),
            member,
            construct,
        })
    }

    /// What a functionality mentions (R9).
    fn mentions(&self, stretches: &[Stretch<'_>]) -> Mentions {
        let ix = self.ix;
        let own_names: BTreeSet<&str> = std::iter::once(self.ty)
            .chain(self.ancestors.iter().copied())
            .map(|t| ix.name_of(t))
            .collect();
        let params = ix.type_params(self.ty).unwrap_or_default();
        let mut types: BTreeSet<String> = BTreeSet::new();
        let mut add = |written: &str| {
            let last = written.rsplit('.').next().unwrap_or(written);
            if last.chars().next().is_some_and(char::is_uppercase)
                && !own_names.contains(last)
                && !params.iter().any(|p| p == last)
            {
                types.insert(last.to_string());
            }
        };
        let mut keys: Vec<ConfigKey> = Vec::new();
        for s in stretches {
            let body = s.at.body;
            for l in body.literals.iter().filter(|l| s.holds(l.node)) {
                if let LiteralContext::CallArg(callee, _) = &l.context {
                    if is_config_key(&l.value) {
                        keys.push(ConfigKey {
                            key: l.value.clone(),
                            callee: callee.clone(),
                            place: s.at.place(l.place),
                        });
                    }
                }
            }
            for t in &body.typed_names {
                let held = match t.scope {
                    Some(scope) => s.holds(Some(scope)) || s.node == Some(scope),
                    None => s.node.is_none(),
                };
                if held && matches!(t.kind, DeclarationKind::Local | DeclarationKind::Pattern) {
                    add(&t.type_name);
                }
            }
            for inv in body.invocations.iter().filter(|i| s.holds(i.node)) {
                match inv.kind {
                    CallKind::Construct if !inv.callee.segments.is_empty() => {
                        add(&inv.callee.segments.join("."));
                    }
                    CallKind::Call => {
                        let scope = Scope {
                            ty: self.ty,
                            body: Some(body),
                            node: inv.node,
                        };
                        let chain = ix.resolve(&scope, &inv.callee, Some(inv.arity));
                        for link in &chain.links {
                            if !self.local(&link.member) {
                                add(ix.name_of(&link.owner));
                            }
                        }
                        if let Some(Unresolved::ExternalType(name)) = &chain.stopped {
                            add(name);
                        }
                    }
                    _ => {}
                }
            }
        }
        keys.sort_by(|a, b| (&a.key, &a.place).cmp(&(&b.key, &b.place)));
        keys.dedup_by(|a, b| a.key == b.key && a.place == b.place);
        let types: Vec<String> = types.into_iter().collect();
        let mut keywords: BTreeSet<String> = keys.iter().map(|k| k.key.clone()).collect();
        keywords.extend(
            types
                .iter()
                .filter(|t| split_identifier(t).iter().any(|p| is_word(p)))
                .cloned(),
        );
        Mentions {
            config_keys: keys,
            types,
            keywords: keywords.into_iter().collect(),
        }
    }
}

/// What an arm calls or passes: the callees, and the enum members, constants
/// and literals of the arguments of the calls under it.
fn picks(body: &MemberBody, arm: u32) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for inv in &body.invocations {
        if !inv.node.is_some_and(|n| within(body, n, arm)) {
            continue;
        }
        out.insert(written(&inv.callee));
        for a in &inv.args {
            out.extend(a.enum_refs.iter().cloned());
            match &a.value {
                ValueRef::Constant(c) => {
                    out.insert(constant_text(c));
                }
                ValueRef::Literal(id) => {
                    if let Some(l) = body.literals.get(*id as usize) {
                        out.insert(format!("\"{}\"", l.value));
                    }
                }
                _ => {}
            }
        }
    }
    out
}

/// A call that only logs: a segment one of whose words is `log`, `logger`
/// or `logging` (`LogError`, `_logger.Warn`), or that is `Trace`, `Debug` or
/// `Console`. `Login`, `Logout` and `Logic` are words of their own.
fn is_log_like(callee: &AccessPath) -> bool {
    callee.segments.iter().any(|s| {
        matches!(
            s.trim_start_matches('_').to_lowercase().as_str(),
            "trace" | "debug" | "console"
        ) || split_identifier(s)
            .iter()
            .any(|w| matches!(w.as_str(), "log" | "logger" | "logging"))
    })
}

/// The shape of a configuration key (R9): capital letters, digits and `_`,
/// three characters or more, at least one letter.
pub fn is_config_key(value: &str) -> bool {
    value.chars().count() >= 3
        && value
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        && value.chars().any(|c| c.is_ascii_uppercase())
}

/// The flow of a type's functionalities (see the module docs).
pub(crate) fn analyze(ix: &Index<'_>, ty: &NodeKey, functionalities: &[Functionality]) -> TypeFlow {
    let mut flow = Flow {
        ix,
        ty,
        ancestors: ix.ancestors(ty),
        guards_memo: BTreeMap::new(),
        predicate_memo: BTreeMap::new(),
        visiting: BTreeSet::new(),
    };
    let states = flow.states();
    let mut dead = flow.dead_paths(functionalities);
    let mut out = Vec::new();
    for f in functionalities {
        let mut found = Vec::new();
        for e in &f.entry_points {
            found.extend(flow.entry_found(e));
        }
        let (preconditions, admitted) = merge(found);
        let stretches = flow.stretches(f);
        out.push(FunctionalityFlow {
            key: f.key.clone(),
            preconditions,
            admitted,
            dead_paths: dead.remove(&Some(f.key.clone())).unwrap_or_default(),
            steps: flow.steps(&stretches, &states),
            mentions: flow.mentions(&stretches),
        });
    }
    out.sort_by(|a, b| a.key.cmp(&b.key));
    TypeFlow {
        functionalities: out,
        states: states.states,
        dead_paths: dead.into_values().flatten().collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::bodies::{Position, Range};

    fn path(segments: &[&str]) -> Expr {
        Expr::Path(AccessPath {
            root: PathRoot::Name,
            segments: segments.iter().map(|s| s.to_string()).collect(),
        })
    }

    fn enum_member(m: &str) -> Expr {
        Expr::EnumMember(m.to_string())
    }

    fn tree(expr: Vec<Expr>) -> Cond {
        Cond::from_expr(&expr, &[]).expect("a well-formed tree")
    }

    /// `(syntax == SyntaxKind.Sql || syntax == SyntaxKind.Script) && canErrors`
    fn can_execute() -> Cond {
        tree(vec![
            path(&["syntax"]),
            enum_member("SyntaxKind.Sql"),
            Expr::Compare(CompareOp::Eq, 0, 1),
            path(&["syntax"]),
            enum_member("SyntaxKind.Script"),
            Expr::Compare(CompareOp::Eq, 3, 4),
            Expr::Or(vec![2, 5]),
            path(&["canErrors"]),
            Expr::And(vec![6, 7]),
        ])
    }

    /// `!canErrors`
    fn not_can_errors() -> Cond {
        tree(vec![path(&["canErrors"]), Expr::Not(0)])
    }

    /// `syntax != SyntaxKind.Sql && syntax != SyntaxKind.Script`, written
    /// with `this.` and the operands of the first comparison the other way.
    fn other_syntax() -> Cond {
        tree(vec![
            enum_member("SyntaxKind.Sql"),
            Expr::Path(AccessPath {
                root: PathRoot::This,
                segments: vec!["syntax".into()],
            }),
            Expr::Compare(CompareOp::Ne, 0, 1),
            path(&["syntax"]),
            enum_member("SyntaxKind.Script"),
            Expr::Compare(CompareOp::Ne, 3, 4),
            Expr::And(vec![2, 5]),
        ])
    }

    fn place(line: u32) -> Place {
        Place {
            path: "src/A.cs".into(),
            range: Range::from([line, 1, line, 10]),
        }
    }

    fn found(source: Source, when: Cond, text: &str, line: u32) -> Found {
        guard(
            source,
            &NodeKey::new("sym:csharp:Acme#A.M()"),
            place(line),
            when,
            text.to_string(),
            Vec::new(),
        )
    }

    /// AE1: the conjuncts of the CanExecute predicate, each turned into
    /// "does not run when", are the two guards of `Check(bool)` - so after
    /// merging each precondition is there once, with both places.
    #[test]
    fn the_can_execute_conjuncts_inverted_are_the_guards_and_merge_with_them() {
        let inverted = can_execute().guards_of_predicate();
        assert_eq!(inverted.len(), 2, "{inverted:?}");
        let guards = [not_can_errors().normalized(), other_syntax().normalized()];
        for g in &guards {
            assert!(inverted.contains(g), "{} not in {inverted:?}", g.render());
        }
        let mut all = vec![
            found(Source::Guard, guards[0].clone(), "!canErrors", 3),
            found(
                Source::Guard,
                guards[1].clone(),
                "syntax != SyntaxKind.Sql && syntax != SyntaxKind.Script",
                4,
            ),
        ];
        for g in inverted {
            let text = g.render();
            all.push(found(Source::CanExecute, g, &text, 1));
        }
        let (preconditions, admitted) = merge(all);
        assert!(admitted.is_empty());
        assert_eq!(preconditions.len(), 2, "{preconditions:#?}");
        for p in &preconditions {
            let sources: Vec<Source> = p.occurrences.iter().map(|o| o.source).collect();
            assert_eq!(sources, [Source::CanExecute, Source::Guard], "{}", p.text);
        }
        // A guard's own text wins over the rendered tree.
        let texts: Vec<&str> = preconditions.iter().map(|p| p.text.as_str()).collect();
        assert!(texts.contains(&"!canErrors"), "{texts:?}");
        assert!(texts.contains(&"syntax != SyntaxKind.Sql && syntax != SyntaxKind.Script"));
    }

    #[test]
    fn negations_go_to_the_leaves_and_operands_take_one_order() {
        // `!(a || b)` is `!a && !b`.
        let a_or_b = tree(vec![
            path(&["a"]),
            path(&["b"]),
            Expr::Or(vec![0, 1]),
            Expr::Not(2),
        ]);
        let not_a_and_not_b = tree(vec![
            path(&["b"]),
            Expr::Not(0),
            path(&["a"]),
            Expr::Not(2),
            Expr::And(vec![1, 3]),
        ]);
        assert_eq!(a_or_b.normalized(), not_a_and_not_b.normalized());
        assert_eq!(a_or_b.normalized().render(), "!a && !b");
        // `!(x == y)` is `x != y`, and `y != x` too.
        let not_eq = tree(vec![
            path(&["x"]),
            path(&["y"]),
            Expr::Compare(CompareOp::Eq, 0, 1),
            Expr::Not(2),
        ]);
        let ne = tree(vec![
            path(&["y"]),
            path(&["x"]),
            Expr::Compare(CompareOp::Ne, 0, 1),
        ]);
        assert_eq!(not_eq.normalized(), ne.normalized());
        assert_eq!(ne.normalized().render(), "x != y");
        // A negated order comparison flips its operator, keeping its sides.
        let lt = tree(vec![
            path(&["a"]),
            path(&["b"]),
            Expr::Compare(CompareOp::Lt, 0, 1),
        ]);
        assert_eq!(lt.negated().render(), "a >= b");
        // `!!x` is `x`, `!true` is `false`.
        let twice = tree(vec![path(&["x"]), Expr::Not(0), Expr::Not(1)]);
        assert_eq!(twice.normalized(), tree(vec![path(&["x"])]));
        let t = tree(vec![Expr::Constant(Constant::Boolean(true))]);
        assert_eq!(t.negated().render(), "false");
        // An `||` under an `&&` keeps its parentheses.
        assert_eq!(
            can_execute().normalized().render(),
            "(syntax == SyntaxKind.Script || syntax == SyntaxKind.Sql) && canErrors"
        );
    }

    #[test]
    fn a_malformed_tree_is_no_tree() {
        assert!(Cond::from_expr(&[], &[]).is_none());
        // An operand used twice.
        let twice = [path(&["a"]), Expr::Compare(CompareOp::Eq, 0, 0)];
        assert!(Cond::from_expr(&twice, &[]).is_none());
        // An operand after its user.
        let forward = [Expr::Not(1), path(&["a"])];
        assert!(Cond::from_expr(&forward, &[]).is_none());
    }

    fn node(id: u32, kind: NodeKind, parent: Option<u32>) -> BodyNode {
        BodyNode {
            id,
            kind,
            place: Range::from([1, 1, 1, 30]),
            parent,
            condition: None,
            returns: None,
            tags: Vec::new(),
        }
    }

    fn body(nodes: Vec<BodyNode>) -> MemberBody {
        MemberBody {
            name_position: Position { line: 1, column: 1 },
            kind: BodyKind::Method,
            root: 0,
            nodes,
            accesses: Vec::new(),
            invocations: Vec::new(),
            lambdas: Vec::new(),
            subscriptions: Vec::new(),
            literals: Vec::new(),
            name_ofs: Vec::new(),
            typed_names: Vec::new(),
            tags: Vec::new(),
        }
    }

    /// A component that gives a returned predicate its tree (contract 0.3
    /// does not) is read without a change of the host.
    #[test]
    fn a_returned_predicate_with_a_tree_is_read_from_it() {
        let mut r = node(1, NodeKind::Return, Some(0));
        r.condition = Some(Condition {
            text: "canErrors".into(),
            expr: vec![path(&["canErrors"])],
            enum_refs: Vec::new(),
            reads: Vec::new(),
            constant: None,
        });
        let b = body(vec![node(0, NodeKind::Body, None), r]);
        match returned(&b, &b.nodes[1]) {
            Returned::Tree(t) => assert_eq!(t.guards_of_predicate()[0].render(), "!canErrors"),
            _ => panic!("the tree was not read"),
        }
        // Without one, a returned expression is not in the model.
        let b = body(vec![
            node(0, NodeKind::Body, None),
            node(1, NodeKind::Return, Some(0)),
        ]);
        assert!(matches!(returned(&b, &b.nodes[1]), Returned::NotInModel));
    }

    #[test]
    fn a_log_call_is_told_by_its_words_not_by_a_prefix() {
        let callee = |segments: &[&str]| AccessPath {
            root: PathRoot::Name,
            segments: segments.iter().map(|s| s.to_string()).collect(),
        };
        for log in [
            &["LogError"][..],
            &["_logger", "Warn"],
            &["Debug", "WriteLine"],
            &["Console", "WriteLine"],
        ] {
            assert!(is_log_like(&callee(log)), "{log:?}");
        }
        for other in [
            &["LoginService", "Authenticate"][..],
            &["Logout"],
            &["Logic", "Run"],
        ] {
            assert!(!is_log_like(&callee(other)), "{other:?}");
        }
    }

    #[test]
    fn only_a_capital_key_shape_is_a_config_key() {
        for key in ["SQLPREFIX", "DB_HOST", "A1_"] {
            assert!(is_config_key(key), "{key}");
        }
        for other in [
            "Heslo123",
            "123",
            "AB",
            "sql_prefix",
            "[redacted: x]",
            "DB-HOST",
        ] {
            assert!(!is_config_key(other), "{other}");
        }
    }
}
