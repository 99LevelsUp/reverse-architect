//! `sql` — a reverse-architect language plugin for SQL scripts.
//!
//! Built only against the published contract (`wit/extractor.wit`): it knows
//! nothing about the host beyond that file, which is the point of it. Read it
//! as the smallest complete plugin — both passes, symbols with members, doc
//! comments, the `declares` edges a file owes its declarations — and copy it.
//!
//! What it reports lives in [`sql`]; this file only turns that into the wire
//! form. The key of a declaration is `sym:sql:<schema>#<name>` (`_` when the
//! script names no schema), a column's `sym:sql:<schema>#<table>.<column>`.

wit_bindgen::generate!({
    path: "../../wit",
    world: "language-plugin",
});

pub mod sql;

use exports::ra::plugin::extractor::Guest;
use ra::plugin::types::{
    Config, Edge, Fact, Language, Manifest, Op, Place, Symbol, Usage, UsageKind, Value, Vocabulary,
};
use std::collections::BTreeMap;

fn text_fact(kind: &str, value: &str, place: Place) -> Fact {
    Fact {
        kind: kind.into(),
        value: Value::Text(value.into()),
        place,
    }
}

/// The wire operations for one script.
pub fn declarations(path: &str, source: &[u8]) -> Vec<Op> {
    let src = String::from_utf8_lossy(source);
    let (decls, _) = sql::scan(&src);
    let mut ops = Vec::new();
    // The same object created twice in one script (a drop-and-recreate) gets
    // an ordinal, as the bundled plugins do, so its two ranges stay apart.
    let mut seen: BTreeMap<String, u32> = BTreeMap::new();
    let mut unique = |identity: String| {
        let n = seen.entry(identity.clone()).or_insert(0);
        *n += 1;
        if *n == 1 {
            format!("sym:{identity}")
        } else {
            format!("sym:{identity}~{n}")
        }
    };
    for decl in decls {
        let schema = decl.schema.clone().unwrap_or_else(|| "_".into());
        let key = unique(format!("sql:{schema}#{}", decl.name));
        let range = Place::Range((decl.start, decl.end));
        let mut facts = vec![
            text_fact("symbol_name", &decl.name, range),
            text_fact("symbol_kind", decl.kind, range),
            text_fact("symbol_signature", &decl.signature, range),
        ];
        if let Some((doc, start, end)) = &decl.doc {
            facts.push(text_fact("doc_comment", doc, Place::Range((*start, *end))));
        }
        ops.push(Op::Symbol(Symbol {
            key: key.clone(),
            facts,
            // A file declares what it creates. The host never infers this:
            // without it the file reads as one that declares nothing.
            edges: vec![Edge {
                source: path.into(),
                target: key.clone(),
                kind: "declares".into(),
                place: range,
            }],
        }));
        for column in &decl.columns {
            let member = unique(format!("sql:{schema}#{}.{}", decl.name, column.name));
            let at = Place::Range((column.line, column.line));
            ops.push(Op::Symbol(Symbol {
                key: member.clone(),
                facts: vec![
                    text_fact("symbol_name", &column.name, at),
                    text_fact("symbol_kind", "field", at),
                    text_fact("symbol_signature", &column.signature, at),
                    text_fact("symbol_container", &decl.name, at),
                ],
                edges: vec![Edge {
                    source: path.into(),
                    target: member.clone(),
                    kind: "declares".into(),
                    place: at,
                }],
            }));
            // After both ends exist: an edge to a missing node is dropped.
            ops.push(Op::Edge(Edge {
                source: key.clone(),
                target: member,
                kind: "has_member".into(),
                place: at,
            }));
        }
    }
    ops
}

/// Every name the script uses, for the host to resolve.
pub fn usages(source: &[u8]) -> Vec<Usage> {
    let src = String::from_utf8_lossy(source);
    let (_, uses) = sql::scan(&src);
    uses.into_iter()
        .map(|u| Usage {
            name: u.name,
            kind: if u.call {
                UsageKind::Call
            } else {
                UsageKind::Reference
            },
            line: u.line,
        })
        .collect()
}

// Constructed only by the export glue, which exists only in the component.
#[cfg_attr(not(target_family = "wasm"), allow(dead_code))]
struct Plugin;

impl Guest for Plugin {
    fn describe() -> Manifest {
        Manifest {
            id: "sql".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            languages: vec![Language {
                name: "sql".into(),
                extensions: vec!["sql".into()],
            }],
            trust_rank: 30,
            depends_on: Vec::new(),
        }
    }

    fn declare() -> Option<Vocabulary> {
        // Everything here is a core kind: tables and views are types,
        // procedures and functions are callables, columns are fields.
        None
    }

    fn declarations(path: String, source: Vec<u8>, _config: Config) -> Vec<Op> {
        // Text this plugin quotes — a signature line, a comment — is redacted
        // by the host with the operator's prefixes, so the plugin does not
        // need the list itself.
        declarations(&path, &source)
    }

    fn usages(_path: String, source: Vec<u8>) -> Vec<Usage> {
        usages(&source)
    }
}

// Only the component exports the interface; a native build is for the tests.
#[cfg(target_family = "wasm")]
export!(Plugin);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_table_comes_with_its_declares_edge_and_its_columns() {
        let ops = declarations("db/schema.sql", b"CREATE TABLE t (a int, b int);\n");
        let keys: Vec<String> = ops
            .iter()
            .filter_map(|op| match op {
                Op::Symbol(s) => Some(s.key.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(keys, ["sym:sql:_#t", "sym:sql:_#t.a", "sym:sql:_#t.b"]);
        let Op::Symbol(table) = &ops[0] else {
            panic!("the table comes first")
        };
        assert_eq!(table.edges[0].source, "db/schema.sql");
        assert_eq!(table.edges[0].kind, "declares");
        assert!(ops
            .iter()
            .any(|op| matches!(op, Op::Edge(e) if e.kind == "has_member" && e.target == "sym:sql:_#t.a")));
    }

    #[test]
    fn a_recreated_object_gets_an_ordinal() {
        let ops = declarations(
            "x.sql",
            b"create view v as select 1;\ndrop view v;\ncreate view v as select 2;\n",
        );
        let keys: Vec<String> = ops
            .iter()
            .filter_map(|op| match op {
                Op::Symbol(s) => Some(s.key.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(keys, ["sym:sql:_#v", "sym:sql:_#v~2"]);
    }
}
