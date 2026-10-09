//! Walker output → wire records of `wit-v2/extractor.wit` (`ra:plugin@0.2.0`).
//!
//! A re-spelling and nothing else: every op keeps its position, every fact its
//! value and place. The host reverses this exactly, so a fact crosses the
//! boundary and arrives as the fact the in-process extractor used to emit.

use crate::ra::plugin::types::{
    DeclaredType as WDeclaredType, Edge as WEdge, Fact as WFact, FileUsages, KeyedFact, Op, Place,
    Position, Receiver as WReceiver, Relation, RelationKind as WRelationKind, Scope,
    SiteContext as WSiteContext, Span as WSpan, Symbol, Usage, UsageKind as WUsageKind,
    UsingDirective as WUsingDirective, Value,
};
use crate::shim::{Edge, Fact, FactValue, FileOutput, GraphOp, PathKeys, Provenance, Span};
use crate::walkers::{
    collect_csharp_usages, collect_usages, construction_reads_as_a_call, extract_declarations,
    parse, Language, LexicalScope, Receiver, RelationKind, SiteContext, UsageKind,
};
use ra_redact::SecretPrefixes;

fn place(prov: &Provenance) -> Place {
    Place {
        span: match prov.span {
            Span::Whole => WSpan::Whole,
            Span::Line(line) => WSpan::Line(line),
            Span::Range { start, end } => WSpan::Range((start, end)),
        },
        name: prov.name.map(|(line, column)| Position { line, column }),
    }
}

fn fact(f: Fact) -> WFact {
    WFact {
        kind: f.kind.word().into(),
        value: match f.value {
            FactValue::Text(t) => Value::Text(t),
            FactValue::Int(i) => Value::Int(i),
            FactValue::Bool(b) => Value::Flag(b),
        },
        place: place(&f.provenance),
    }
}

fn edge(from: &str, to: &str, e: Edge) -> WEdge {
    WEdge {
        source: from.into(),
        target: to.into(),
        kind: e.kind.word().into(),
        place: place(&e.provenance),
    }
}

/// Pass 1 over one file.
pub fn declarations(path: &str, source: &[u8], extra_prefixes: &[String]) -> Vec<Op> {
    let Some(lang) = Language::enabled(path) else {
        return Vec::new();
    };
    let prefixes = SecretPrefixes::new(extra_prefixes);
    let mut out = FileOutput::default();
    extract_declarations(
        lang,
        path,
        source,
        &mut out,
        &PathKeys::default(),
        &prefixes,
    );
    let FileOutput { ops, relations } = out;
    let mut wire: Vec<Op> = Vec::with_capacity(ops.len() + relations.len());
    for op in ops {
        wire.push(match op {
            GraphOp::Fact { key, fact: f } => Op::Fact(KeyedFact {
                node: key.as_str().into(),
                fact: fact(f),
            }),
            GraphOp::Symbol { key, facts, edges } => Op::Symbol(Symbol {
                key: key.as_str().into(),
                facts: facts.into_iter().map(fact).collect(),
                edges: edges
                    .into_iter()
                    .map(|(from, to, e)| edge(from.as_str(), to.as_str(), e))
                    .collect(),
            }),
            GraphOp::Edge { from, to, edge: e } => Op::Edge(edge(from.as_str(), to.as_str(), e)),
        });
    }
    for r in relations {
        wire.push(Op::Relation(Relation {
            sources: r.from.iter().map(|k| k.as_str().to_string()).collect(),
            targets: r.to.iter().map(|k| k.as_str().to_string()).collect(),
            kind: match r.kind {
                RelationKind::Extends => WRelationKind::Extends,
                RelationKind::Implements => WRelationKind::Implements,
                RelationKind::ByTargetKind => WRelationKind::ByTargetKind,
            },
            place: place(&r.prov),
        }));
    }
    wire
}

/// Pass 2 over one file: every usage site, for the host to resolve, and —
/// for C#, the one language that reads context so far — the scopes the
/// sites' context points into. One parse serves both.
pub fn usages(path: &str, source: &[u8]) -> FileUsages {
    let empty = || FileUsages {
        scopes: Vec::new(),
        usages: Vec::new(),
    };
    let Some(lang) = Language::enabled(path) else {
        return empty();
    };
    let Some(grammar) = lang.grammar() else {
        return empty();
    };
    let Some(tree) = parse(source, grammar) else {
        return empty();
    };
    let (scopes, sites) = match lang {
        Language::CSharp => collect_csharp_usages(tree.root_node(), source),
        _ => {
            let mut sites = Vec::new();
            collect_usages(lang, tree.root_node(), source, &mut sites);
            (Vec::new(), sites)
        }
    };
    let call = if construction_reads_as_a_call(lang) {
        WUsageKind::CallOrConstruct
    } else {
        WUsageKind::Call
    };
    FileUsages {
        scopes: scopes.into_iter().map(scope).collect(),
        usages: sites
            .into_iter()
            .map(|s| Usage {
                name: s.name,
                kind: match s.kind {
                    UsageKind::Call => call,
                    UsageKind::Instantiate => WUsageKind::Construct,
                    UsageKind::Reference => WUsageKind::Reference,
                },
                line: s.line,
                column: s.column,
                context: s.context.map(context),
            })
            .collect(),
    }
}

fn scope(s: LexicalScope) -> Scope {
    Scope {
        namespace: s.namespace,
        lines: s.lines,
        parent: s.parent,
        usings: s
            .usings
            .into_iter()
            .map(|u| WUsingDirective {
                target: u.target,
                alias: u.alias,
                is_static: u.is_static,
                is_global: u.is_global,
            })
            .collect(),
    }
}

fn context(c: SiteContext) -> WSiteContext {
    WSiteContext {
        scope: c.scope,
        receiver: match c.receiver {
            Receiver::None => WReceiver::None,
            Receiver::Name(n) => WReceiver::Name(n),
            Receiver::Value(n) => WReceiver::Value(n),
            Receiver::Keyword(k) => WReceiver::Keyword(k),
            Receiver::Expression => WReceiver::Expression,
        },
        declared_type: c.declared_type.map(|t| WDeclaredType {
            name: t.name,
            keyword: t.keyword,
        }),
        argument_count: c.argument_count,
        qualifier: c.qualifier,
    }
}
