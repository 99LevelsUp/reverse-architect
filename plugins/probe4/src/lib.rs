//! A test component for contract 0.4 (`wit-v4`).
//!
//! It serves the `bodies` pass alone, as a body component does, and claims
//! `.probe4`: a host test can route two body components by extension, and
//! read what 0.4 says beside the model — a comment of prose, a comment that is
//! commented-out code, and the role of a literal.
//!
//! Not a language plugin anyone would load.

wit_bindgen::generate!({
    path: "../../wit-v4",
    world: "language-plugin",
});

use exports::ra::plugin::extractor::Guest;
use ra::plugin::notes::{Comment, CommentKind, FileNotes, LiteralRole, RoleKind};
use ra::plugin::types::{
    BodyKind, BodyNode, Config, FileBodies, FileDeclarations, FileUsages, Language,
    LiteralContext, Manifest, MemberBody, NodeKind, Pass, Position, Range, StringLiteral,
    Vocabulary,
};

struct Probe4;

/// The one path `bodies` refuses.
const REFUSED: &str = "refuse.probe4";

fn range(line: u32, from: u32, to: u32) -> Range {
    Range {
        start: Position { line, column: from },
        end: Position { line, column: to },
    }
}

impl Guest for Probe4 {
    fn describe() -> Manifest {
        Manifest {
            id: "probe4".into(),
            version: "0.1.0".into(),
            languages: vec![Language {
                name: "probe4".into(),
                extensions: vec!["probe4".into()],
            }],
            trust_rank: 1,
            depends_on: Vec::new(),
            passes: vec![Pass::Bodies],
            emits: vec!["comment".into(), "literal-role".into()],
        }
    }

    fn declare() -> Option<Vocabulary> {
        None
    }

    fn declarations(
        _path: String,
        _source: Vec<u8>,
        _config: Config,
    ) -> Result<FileDeclarations, String> {
        Err("probe4 reads bodies only".into())
    }

    fn usages(_path: String, _source: Vec<u8>) -> Result<FileUsages, String> {
        Err("probe4 reads bodies only".into())
    }

    fn bodies(path: String, source: Vec<u8>) -> Result<FileNotes, String> {
        if path == REFUSED {
            return Err("bodies: probe4 cannot read this file".into());
        }
        let mut notes = FileNotes {
            bodies: FileBodies {
                types: Vec::new(),
                members: Vec::new(),
                typed_members: Vec::new(),
                metas: Vec::new(),
            },
            comments: Vec::new(),
            roles: Vec::new(),
        };
        if source.is_empty() {
            return Ok(notes);
        }
        notes.bodies.members.push(MemberBody {
            name_position: Position { line: 2, column: 5 },
            kind: BodyKind::Method,
            root: 0,
            nodes: vec![BodyNode {
                id: 0,
                kind: NodeKind::Body,
                place: range(2, 1, 9),
                parent: None,
                condition: None,
                returns: None,
                tags: Vec::new(),
            }],
            accesses: Vec::new(),
            invocations: Vec::new(),
            lambdas: Vec::new(),
            subscriptions: Vec::new(),
            literals: vec![StringLiteral {
                id: 0,
                value: "KEY".into(),
                context: LiteralContext::Other,
                place: range(3, 9, 14),
                node: Some(0),
            }],
            name_ofs: Vec::new(),
            typed_names: Vec::new(),
            tags: Vec::new(),
        });
        notes.comments = vec![
            Comment {
                place: range(1, 1, 30),
                text: "disabled on purpose, see the ticket".into(),
                kind: CommentKind::Prose,
            },
            Comment {
                place: range(4, 5, 22),
                text: "var x = Load(\"a\");".into(),
                kind: CommentKind::Code,
            },
        ];
        notes.roles = vec![LiteralRole {
            member: 0,
            literal: 0,
            role: RoleKind::Key,
        }];
        // A role past the model: the host keeps none of it and counts it.
        if path == "roles.probe4" {
            notes.roles.push(LiteralRole {
                member: 9,
                literal: 0,
                role: RoleKind::Compared,
            });
        }
        Ok(notes)
    }
}

export!(Probe4);
