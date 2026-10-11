use super::{Builder, GenericMark, GenericMarks};
use crate::graph::{EdgeKind, NodeKey};
use std::collections::{BTreeMap, BTreeSet};

/// The parameter types a member's key spells (`OnSave(object,EventArgs)`),
/// split at the commas outside generic arguments; `None` for a key with no
/// parameter list.
fn parameters(key: &str) -> Option<Vec<&str>> {
    let inner = key.strip_suffix(')')?;
    let open = inner.rfind('(')?;
    let list = &inner[open + 1..];
    if list.trim().is_empty() {
        return Some(Vec::new());
    }
    let mut out = Vec::new();
    let (mut depth, mut start) = (0usize, 0usize);
    for (i, c) in list.char_indices() {
        match c {
            '<' | '[' | '(' => depth += 1,
            '>' | ']' | ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                out.push(list[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(list[start..].trim());
    Some(out)
}

/// An event handler's parameters: `object` (or `object?`), then a type whose
/// last name ends in `EventArgs`.
fn is_handler_signature(key: &str) -> bool {
    match parameters(key).as_deref() {
        Some([sender, args]) => {
            let sender = sender.trim_end_matches('?');
            let args = args.split('<').next().unwrap_or(args);
            let last = args.rsplit('.').next().unwrap_or(args);
            matches!(sender, "object" | "Object" | "System.Object") && last.ends_with("EventArgs")
        }
        _ => false,
    }
}

impl<'b, 'i, 'a> Builder<'b, 'i, 'a> {
    // ---- The words of a declaration (U8: R17) ----

    /// Every member of the run's C# types whose declaration gives a marker.
    pub(super) fn generic(&mut self) {
        let ix = self.index();
        // The interfaces each type implements, by the relation the first
        // layer read — gathered into the set of faces below, whose order is
        // its own, so in no order of the graph's.
        let mut implements: BTreeMap<&NodeKey, Vec<&NodeKey>> = BTreeMap::new();
        for e in self.graph.edges_unordered() {
            if e.edge.kind == EdgeKind::Implements {
                implements.entry(e.from).or_default().push(e.to);
            }
        }
        let mut out = Vec::new();
        for ty in ix.csharp_types() {
            // The interfaces of the type and of its line of bases, and the
            // interfaces those extend, each once.
            let mut faces: BTreeSet<&NodeKey> = BTreeSet::new();
            let mut todo: Vec<&NodeKey> = std::iter::once(ty)
                .chain(ix.ancestors(ty))
                .flat_map(|t| implements.get(t).into_iter().flatten().copied())
                .collect();
            while let Some(face) = todo.pop() {
                if faces.insert(face) {
                    todo.extend(implements.get(face).into_iter().flatten().copied());
                }
            }
            let declared: BTreeSet<(&str, &str)> = faces
                .iter()
                .flat_map(|f| ix.members_of(f))
                .filter_map(|m| ix.symbol(m))
                .map(|s| (s.name.as_str(), s.kind.as_str()))
                .collect();
            for m in ix.members_of(ty) {
                let Some(s) = ix.symbol(m) else { continue };
                if s.is_type() {
                    continue;
                }
                let mut marks = Vec::new();
                if declared.contains(&(s.name.as_str(), s.kind.as_str())) {
                    marks.push(GenericMark::Interface);
                }
                let metas = ix.layer.metas_of(m);
                let has = |word: &str| {
                    metas
                        .iter()
                        .any(|(_, meta)| meta.modifiers.iter().any(|w| w == word))
                };
                if has("virtual") || has("abstract") {
                    marks.push(GenericMark::Virtual);
                }
                if has("override") {
                    marks.push(GenericMark::Override);
                }
                if s.is_method() && is_handler_signature(m.as_str()) {
                    marks.push(GenericMark::Handler);
                }
                if !marks.is_empty() {
                    out.push(GenericMarks {
                        member: m.clone(),
                        owner: ty.clone(),
                        marks,
                    });
                }
            }
        }
        self.generic = out;
    }
}
