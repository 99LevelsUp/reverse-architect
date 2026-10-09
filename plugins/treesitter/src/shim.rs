//! The host's graph vocabulary, reduced to what the walkers write.
//!
//! The walkers moved here from the host (`src/extract/treesitter.rs`) with
//! their code intact: they still build `Fact::new(FactKind::…, …)`, push onto a
//! `FileOutput`, and anchor everything with a `Provenance`. This module gives
//! those names the same meaning they had in the host — same normalization of
//! keys and spans — so the move changed where the code runs and nothing about
//! what it emits. `crate::convert` then turns the result into the wire records
//! of `wit/extractor.wit`.

/// A graph key, `/`-normalized exactly like the host's `NodeKey::new`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeKey(String);

impl NodeKey {
    pub fn new(value: impl Into<String>) -> Self {
        NodeKey(value.into().replace('\\', "/"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The fact kinds a language walker emits, spelled on the wire by
/// [`FactKind::word`] — the host's snake_case names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FactKind {
    SymbolName,
    SymbolKind,
    SymbolContainer,
    SymbolSignature,
    SymbolVisibility,
    DocComment,
    EntryPoint,
}

impl FactKind {
    pub fn word(self) -> &'static str {
        match self {
            FactKind::SymbolName => "symbol_name",
            FactKind::SymbolKind => "symbol_kind",
            FactKind::SymbolContainer => "symbol_container",
            FactKind::SymbolSignature => "symbol_signature",
            FactKind::SymbolVisibility => "symbol_visibility",
            FactKind::DocComment => "doc_comment",
            FactKind::EntryPoint => "entry_point",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FactValue {
    Text(String),
    #[allow(dead_code)]
    Int(i64),
    Bool(bool),
}

/// The trust tier. A walker only ever measures; the host assigns the tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Measured,
}

/// Stand-in for the host's extractor id: the host stamps the plugin's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtractorId;

pub mod ids {
    pub fn treesitter() -> super::ExtractorId {
        super::ExtractorId
    }
}

/// Where in the file — the host's `Span`, with the same constructors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Span {
    Whole,
    Line(u32),
    Range { start: u32, end: u32 },
}

impl Span {
    pub fn at(line: Option<u32>) -> Span {
        match line {
            Some(line) if line > 0 => Span::Line(line),
            _ => Span::Whole,
        }
    }

    pub fn range(start: u32, end: u32) -> Span {
        let (lo, hi) = if start <= end {
            (start, end)
        } else {
            (end, start)
        };
        match (lo, hi) {
            (0, 0) => Span::Whole,
            (0, hi) => Span::Line(hi),
            (lo, hi) if lo == hi => Span::Line(lo),
            (lo, hi) => Span::Range { start: lo, end: hi },
        }
    }
}

/// Where a statement came from. The file is always the file being read; the
/// host re-attaches its own extractor id, tier and resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provenance {
    pub file: String,
    pub span: Span,
    /// `(line, column)` of the name of the declaration this statement is
    /// about, both 1-based, the column in characters — what contract 0.2's
    /// `place.name` carries. The host has no such field; it lives here only
    /// to cross the boundary.
    pub name: Option<(u32, u32)>,
}

impl Provenance {
    pub fn new(
        file: impl Into<String>,
        line: Option<u32>,
        _extractor: ExtractorId,
        _tier: Tier,
    ) -> Self {
        Provenance {
            file: file.into().replace('\\', "/"),
            span: Span::at(line),
            name: None,
        }
    }

    #[must_use]
    pub fn with_range(mut self, line: u32, end_line: u32) -> Self {
        self.span = Span::range(line, end_line);
        self
    }

    #[must_use]
    pub fn with_name(mut self, name: Option<(u32, u32)>) -> Self {
        self.name = name;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fact {
    pub kind: FactKind,
    pub value: FactValue,
    pub provenance: Provenance,
}

impl Fact {
    pub fn new(kind: FactKind, value: FactValue, provenance: Provenance) -> Self {
        Fact {
            kind,
            value,
            provenance,
        }
    }
}

/// The edge kinds a language walker emits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeKind {
    Declares,
    HasMember,
    Imports,
}

impl EdgeKind {
    pub fn word(self) -> &'static str {
        match self {
            EdgeKind::Declares => "declares",
            EdgeKind::HasMember => "has_member",
            EdgeKind::Imports => "imports",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edge {
    pub kind: EdgeKind,
    pub provenance: Provenance,
}

impl Edge {
    pub fn new(kind: EdgeKind, provenance: Provenance) -> Self {
        Edge { kind, provenance }
    }
}

/// One buffered change — the host's `GraphOp`, minus the variant only the
/// external-reference extractor uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphOp {
    Fact {
        key: NodeKey,
        fact: Fact,
    },
    Symbol {
        key: NodeKey,
        facts: Vec<Fact>,
        edges: Vec<(NodeKey, NodeKey, Edge)>,
    },
    Edge {
        from: NodeKey,
        to: NodeKey,
        edge: Edge,
    },
}

impl GraphOp {
    pub fn push_fact(&mut self, fact: Fact) {
        if let GraphOp::Symbol { facts, .. } = self {
            facts.push(fact);
        }
    }

    pub fn push_edge(&mut self, from: NodeKey, to: NodeKey, edge: Edge) {
        if let GraphOp::Symbol { edges, .. } = self {
            edges.push((from, to, edge));
        }
    }
}

/// Everything one file's declaration walk produces.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FileOutput {
    pub ops: Vec<GraphOp>,
    pub relations: Vec<crate::walkers::TypeRelation>,
}

impl FileOutput {
    pub fn fact(&mut self, key: NodeKey, fact: Fact) {
        self.ops.push(GraphOp::Fact { key, fact });
    }

    pub fn edge(&mut self, from: NodeKey, to: NodeKey, edge: Edge) {
        self.ops.push(GraphOp::Edge { from, to, edge });
    }

    pub fn symbol(&mut self, key: NodeKey) -> &mut GraphOp {
        self.ops.push(GraphOp::Symbol {
            key,
            facts: Vec::new(),
            edges: Vec::new(),
        });
        self.ops.last_mut().expect("just pushed")
    }
}

/// Which repository paths exist — answered by the host (`path-exists`), from
/// the snapshot it froze before the walk. Natively (the crate's own tests) it
/// is backed by a set the test fills in.
#[derive(Default)]
pub struct PathKeys {
    #[cfg(not(target_arch = "wasm32"))]
    pub known: std::collections::HashSet<String>,
}

impl PathKeys {
    pub fn contains(&self, key: &str) -> bool {
        #[cfg(target_arch = "wasm32")]
        {
            crate::ra::plugin::host::path_exists(key)
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.known.contains(key)
        }
    }

    /// The first candidate that exists, normalized the way a node key is
    /// before it is looked up.
    pub fn first_existing(&self, candidates: &[String]) -> Option<NodeKey> {
        candidates
            .iter()
            .map(|c| NodeKey::new(c.clone()))
            .find(|k| self.contains(k.as_str()))
    }
}
