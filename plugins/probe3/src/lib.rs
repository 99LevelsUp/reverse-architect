//! A test component for contract 0.3 (`wit-v3`).
//!
//! Each input path stands for one situation the host has to handle: a symbol
//! of a node kind the component declared, one of a kind it did not, a
//! reference by name inside a lookup block, and the `bodies` pass — an empty
//! model, a small one, and a guest that spins until its fuel runs out — and,
//! for every pass, a file the component refuses with `err` and a reason.
//!
//! Not a language plugin anyone would load. It claims `.probe3`, and `.probe`
//! as well, which the 0.1 probe also claims — so a host test can put two
//! components on one extension and watch each pass go to the one that serves
//! it.

wit_bindgen::generate!({
    path: "../../wit-v3",
    world: "language-plugin",
});

use exports::ra::plugin::extractor::Guest;
use ra::plugin::types::{
    BodyKind, BodyNode, Config, Edge, Fact, FileBodies, FileDeclarations, FileUsages, Language,
    LookupBlock, Manifest, MemberBody, NameRef, NodeKind, NodeKindDecl, Op, Pass, Place, Position,
    Range, Span, StructuralRole, Symbol, Tag, Usage, UsageKind, Value, Vocabulary,
};

struct Probe3;

/// The one path every pass refuses.
const REFUSED: &str = "refuse.probe3";

/// The reason a pass gives for refusing [`REFUSED`].
fn refusal(pass: &str) -> String {
    format!("{pass}: probe3 cannot read this file")
}

fn place(line: u32, name: Option<(u32, u32)>) -> Place {
    Place {
        span: Span::Line(line),
        name: name.map(|(line, column)| Position { line, column }),
    }
}

/// A symbol named `name` in `path`, of the node kind `node_kind` when given,
/// declared by its file.
fn symbol(path: &str, name: &str, node_kind: Option<&str>, block: Option<u32>) -> Op {
    let key = format!("sym:probe3:{path}#{name}");
    let fact = |kind: &str, value: Value| Fact {
        kind: kind.into(),
        value,
        place: place(1, Some((1, 3))),
    };
    Op::Symbol(Symbol {
        key: key.clone(),
        facts: vec![
            fact("symbol_name", Value::Text(name.into())),
            fact("symbol_kind", Value::Text("class".into())),
        ],
        edges: vec![Edge {
            source: path.into(),
            target: key,
            kind: "declares".into(),
            place: place(1, None),
        }],
        node_kind: node_kind.map(Into::into),
        block,
    })
}

fn position(line: u32, column: u32) -> Position {
    Position { line, column }
}

impl Guest for Probe3 {
    fn describe() -> Manifest {
        Manifest {
            id: "probe3".into(),
            version: "0.1.0".into(),
            languages: vec![
                Language {
                    name: "probe3".into(),
                    extensions: vec!["probe3".into()],
                },
                Language {
                    name: "probe".into(),
                    extensions: vec!["probe".into()],
                },
            ],
            trust_rank: 1,
            depends_on: Vec::new(),
            passes: vec![Pass::Declarations, Pass::Usages, Pass::Bodies],
            emits: vec!["probe3-tag".into()],
        }
    }

    fn declare() -> Option<Vocabulary> {
        Some(Vocabulary {
            namespace: "probe3".into(),
            card_kinds: vec!["widget".into()],
            node_kinds: vec![NodeKindDecl {
                name: "widget".into(),
                role: StructuralRole::Definition,
                card: Some("widget".into()),
            }],
            edge_kinds: Vec::new(),
            fact_kinds: Vec::new(),
        })
    }

    fn declarations(
        path: String,
        _source: Vec<u8>,
        config: Config,
    ) -> Result<FileDeclarations, String> {
        if path == REFUSED {
            return Err(refusal("declarations"));
        }
        let file = |ops: Vec<Op>| FileDeclarations {
            ops,
            name_refs: Vec::new(),
            blocks: Vec::new(),
        };
        Ok(match path.as_str() {
            "trap.probe3" => panic!("probe3 traps on purpose"),
            // A symbol of the kind the vocabulary declares, in a lookup
            // block, referring to another by name from inside it.
            "widget.probe3" => FileDeclarations {
                ops: vec![symbol(&path, "Main", Some("probe3:widget"), Some(0))],
                name_refs: vec![NameRef {
                    source: format!("sym:probe3:{path}#Main"),
                    name: "Other".into(),
                    target_kind: "probe3:widget".into(),
                    relation: None,
                    block: Some(0),
                    place: place(2, Some((2, 5))),
                }],
                blocks: vec![LookupBlock {
                    lines: (1, 3),
                    parent: None,
                }],
            },
            // A kind the vocabulary does not declare, and a kind of another
            // namespace, beside an ordinary symbol.
            "stray.probe3" => file(vec![
                symbol(&path, "Stray", Some("probe3:gadget"), None),
                symbol(&path, "Foreign", Some("probe:widget"), None),
                symbol(&path, "Plain", None, None),
            ]),
            _ => {
                let mut op = symbol(&path, "Thing", None, None);
                if let Op::Symbol(sym) = &mut op {
                    sym.facts.push(Fact {
                        kind: "symbol_signature".into(),
                        value: Value::Text(format!("prefixes:{}", config.secret_prefixes.len())),
                        place: place(1, None),
                    });
                }
                file(vec![op])
            }
        })
    }

    fn usages(path: String, _source: Vec<u8>) -> Result<FileUsages, String> {
        if path == REFUSED {
            return Err(refusal("usages"));
        }
        Ok(FileUsages {
            scopes: Vec::new(),
            usages: vec![Usage {
                name: "Thing".into(),
                kind: UsageKind::Call,
                line: 1,
                column: Some(1),
                context: None,
            }],
        })
    }

    fn bodies(path: String, source: Vec<u8>) -> Result<FileBodies, String> {
        if path == REFUSED {
            return Err(refusal("bodies"));
        }
        if path == "loop.probe3" {
            let mut n: u64 = 0;
            loop {
                n = n.wrapping_add(1);
                std::hint::black_box(n);
            }
        }
        let mut file = FileBodies {
            types: Vec::new(),
            members: Vec::new(),
            typed_members: Vec::new(),
            metas: Vec::new(),
        };
        if source.is_empty() {
            return Ok(file);
        }
        let range = Range {
            start: position(1, 1),
            end: position(1, 2),
        };
        file.members.push(MemberBody {
            name_position: position(1, 1),
            kind: BodyKind::Method,
            root: 0,
            nodes: vec![BodyNode {
                id: 0,
                kind: NodeKind::Body,
                place: range,
                parent: None,
                condition: None,
                returns: None,
                tags: vec![Tag {
                    name: "probe3-tag".into(),
                    value: None,
                    args: Vec::new(),
                }],
            }],
            accesses: Vec::new(),
            invocations: Vec::new(),
            lambdas: Vec::new(),
            subscriptions: Vec::new(),
            literals: Vec::new(),
            name_ofs: Vec::new(),
            typed_names: Vec::new(),
            tags: Vec::new(),
        });
        Ok(file)
    }
}

export!(Probe3);
