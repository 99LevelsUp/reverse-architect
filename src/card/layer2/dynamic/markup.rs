use super::{
    tag_value, tags, Builder, Mark, MarkKind, MarkUse, MarkupSite, SiteOutcome, Unpaired,
    DATA_SOURCE, TARGET_TYPE, UNPARSED_MARKUP, VIEW_MODEL_FIELD,
};
use crate::card::layer2::entry::Place;
use crate::card::layer2::link::Outcome;
use crate::card::layer2::resolve::{Scope, Unresolved};
use crate::extract::bodies::{
    AccessMode, AccessPath, BodyKind, Handler, MemberAccess, MemberBody, NodeKind as BodyNodeKind,
    PathRoot, Range, Subscription, Tag,
};
use crate::graph::{Confidence, NodeKey};

/// One markup body being read.
struct At<'l> {
    path: &'l str,
    index: usize,
    body: &'l MemberBody,
    from: Option<&'l NodeKey>,
}

/// One place being resolved: where, what, how.
struct Head<'h> {
    at: &'h At<'h>,
    range: Range,
    node: Option<u32>,
    kind: MarkKind,
    mode: MarkUse,
    path: Vec<String>,
}

impl Head<'_> {
    fn place(&self) -> Place {
        Place {
            path: self.at.path.to_string(),
            range: self.range,
        }
    }
}

/// A name another file writes qualified — a type, not a member.
fn qualified(name: &str) -> bool {
    name.contains('.') || name.contains('|')
}

fn is_setter(kind: &BodyNodeKind) -> bool {
    matches!(kind, BodyNodeKind::Element(n) if n == "Setter" || n.ends_with(".Setter"))
}

impl<'b, 'i, 'a> Builder<'b, 'i, 'a> {
    // ---- Markup (U6) ----

    pub(super) fn markup(&mut self) {
        let layer = self.index().layer;
        for (path, file) in layer.files() {
            let bodies = &file.model.members;
            for (index, body) in bodies.iter().enumerate() {
                if body.kind != BodyKind::Markup {
                    continue;
                }
                let at = At {
                    path,
                    index,
                    body,
                    from: file
                        .owners
                        .get(index)
                        .and_then(Option::as_ref)
                        .map(|(k, _)| k),
                };
                for access in &body.accesses {
                    self.access(&at, access);
                }
                for s in &body.subscriptions {
                    self.handler(&at, bodies, s);
                }
                let unparsed = body
                    .nodes
                    .iter()
                    .flat_map(|n| &n.tags)
                    .filter(|t| t.name == UNPARSED_MARKUP)
                    .count() as u64;
                if unparsed > 0 {
                    *self.unparsed.entry(path.to_string()).or_default() += unparsed;
                }
            }
        }
    }

    fn access(&mut self, at: &At<'_>, a: &MemberAccess) {
        let segments = &a.target.segments;
        if segments.is_empty() || !matches!(a.mode, AccessMode::Read | AccessMode::Write) {
            return;
        }
        let node = a.node.and_then(|n| at.body.nodes.get(n as usize));
        let sources = node.map_or_else(Vec::new, |n| tags(&n.tags, DATA_SOURCE));
        let read_beside = at
            .body
            .accesses
            .iter()
            .any(|b| b.mode == AccessMode::Read && b.target == a.target && b.place == a.place);
        let head = |kind, mode| Head {
            at,
            range: a.place,
            node: a.node,
            kind,
            mode,
            path: segments.clone(),
        };
        if a.mode == AccessMode::Write && !read_beside {
            let kind = if node.is_some_and(|n| is_setter(&n.kind)) {
                MarkKind::Setter
            } else {
                MarkKind::Attached
            };
            self.type_rooted(head(kind, MarkUse::Write));
        } else if qualified(&segments[0]) || sources.is_empty() {
            self.type_rooted(head(MarkKind::Static, MarkUse::Read));
        } else {
            let mode = if a.mode == AccessMode::Write {
                MarkUse::Write
            } else {
                MarkUse::Read
            };
            let node_kind = node.map(|n| &n.kind);
            self.binding(head(MarkKind::Binding, mode), &sources, node_kind);
        }
    }

    /// `[T, M, …]`: `T` a type's full name, the rest its members.
    fn type_rooted(&mut self, head: Head<'_>) {
        let [ty, rest @ ..] = head.path.as_slice() else {
            return;
        };
        if rest.is_empty() {
            return self.unpaired(head, Unpaired::NoTargetType, 0);
        }
        let ty = match self.index().type_exactly(ty) {
            Ok(ty) => ty,
            Err(reason) => return self.stopped(head, reason, 1),
        };
        let names: Vec<String> = rest.to_vec();
        if head.kind == MarkKind::Attached && names.len() == 1 {
            return self.attached(head, &ty, &names[0]);
        }
        self.follow(head, &ty, names, 0);
    }

    /// `Owner.Property="…"`: the owner's accessors `Get<Property>` and
    /// `Set<Property>` — WPF's convention for an attached property — or,
    /// when it has neither, a member of that name.
    fn attached(&mut self, head: Head<'_>, owner: &NodeKey, property: &str) {
        let ix = self.index();
        let accessors: Vec<NodeKey> = [format!("Get{property}"), format!("Set{property}")]
            .iter()
            .flat_map(|name| {
                ix.members_of(owner)
                    .iter()
                    .filter(|m| {
                        ix.symbol(m)
                            .is_some_and(|s| s.name == *name && s.is_method())
                    })
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .collect();
        if accessors.is_empty() {
            return self.follow(head, owner, vec![property.to_string()], 0);
        }
        for m in &accessors {
            self.put(&head, m, Confidence::RulesBacked);
        }
        self.sites.push(MarkupSite {
            place: head.place(),
            body: head.at.index,
            node: head.node,
            from: head.at.from.cloned(),
            kind: head.kind,
            mode: head.mode,
            path: head.path,
            links: accessors,
            outcome: SiteOutcome::Resolved {
                grade: Confidence::RulesBacked,
            },
        });
    }

    fn binding(&mut self, head: Head<'_>, sources: &[&Tag], node: Option<&BodyNodeKind>) {
        let [source] = sources else {
            return self.unpaired(head, Unpaired::SeveralSources, 0);
        };
        match source.value.as_deref() {
            Some("view") => match self.target_type(&head) {
                Ok(view) => self.view_chain(head, &view),
                Err(Err(why)) => self.unpaired(head, why, 0),
                Err(Ok(reason)) => self.stopped(head, reason, 0),
            },
            Some("typed") => match source.args.first() {
                Some(t) => match self.index().type_exactly(t) {
                    Ok(ty) => {
                        let names = head.path.clone();
                        self.follow(head, &ty, names, 0)
                    }
                    Err(reason) => self.stopped(head, reason, 0),
                },
                None => self.unpaired(head, Unpaired::UnknownSource, 0),
            },
            Some("element") => match source.args.first() {
                Some(name) => match self.element_type(head.at, name) {
                    Some(t) => match self.index().type_exactly(&t) {
                        Ok(ty) => {
                            let names = head.path.clone();
                            self.follow(head, &ty, names, 0)
                        }
                        Err(reason) => self.stopped(head, reason, 0),
                    },
                    None => self.unpaired(head, Unpaired::NoElement(name.clone()), 0),
                },
                None => self.unpaired(head, Unpaired::UnknownSource, 0),
            },
            Some("self") => {
                let own = match node {
                    Some(k @ BodyNodeKind::Element(n)) if !is_setter(k) => Some(n.clone()),
                    _ => None,
                };
                let ty = match own {
                    Some(n) => self.index().type_exactly(&n).map_err(Ok),
                    None => self.target_type(&head),
                };
                match ty {
                    Ok(ty) => {
                        let names = head.path.clone();
                        self.follow(head, &ty, names, 0)
                    }
                    Err(Err(why)) => self.unpaired(head, why, 0),
                    Err(Ok(reason)) => self.stopped(head, reason, 0),
                }
            }
            Some("templated-parent") => match self.target_type(&head) {
                Ok(ty) => {
                    let names = head.path.clone();
                    self.follow(head, &ty, names, 0)
                }
                Err(Err(why)) => self.unpaired(head, why, 0),
                Err(Ok(reason)) => self.stopped(head, reason, 0),
            },
            Some("explicit") => match source.args.as_slice() {
                [ty, member] => match self.index().type_exactly(ty) {
                    Ok(ty) => {
                        let mut names = vec![member.clone()];
                        names.extend(head.path.iter().cloned());
                        self.follow(head, &ty, names, 1)
                    }
                    Err(reason) => self.stopped(head, reason, 0),
                },
                _ => self.unpaired(head, Unpaired::UnknownSource, 0),
            },
            Some("item") => self.unpaired(head, Unpaired::ItemSource, 0),
            _ => self.unpaired(head, Unpaired::UnknownSource, 0),
        }
    }

    /// The type the body is about: `Err(Err(_))` when it names none,
    /// `Err(Ok(_))` when the repository declares no C# type of exactly that
    /// name, or several.
    fn target_type(&self, head: &Head<'_>) -> Result<NodeKey, Result<Unresolved, Unpaired>> {
        let Some(full) = tag_value(&head.at.body.tags, TARGET_TYPE) else {
            return Err(Err(Unpaired::NoTargetType));
        };
        self.index().type_exactly(full).map_err(Ok)
    }

    /// From the view to its model, then the binding's path (KTD7).
    fn view_chain(&mut self, head: Head<'_>, view: &NodeKey) {
        if let Some(model) = self.view_model(view) {
            let names = head.path.clone();
            return self.follow(head, &model, names, 0);
        }
        if self.holds_view_model_field(view) {
            let mut names = vec![VIEW_MODEL_FIELD.to_string()];
            names.extend(head.path.iter().cloned());
            return self.follow(head, view, names, 1);
        }
        self.unpaired(head, Unpaired::NoViewModel(view.clone()), 0)
    }

    /// The one type argument the first entry of `view`'s base list gives,
    /// the same on every part that writes one, when it names one type of the
    /// repository.
    fn view_model(&self, view: &NodeKey) -> Option<NodeKey> {
        let ix = self.index();
        let mut given: Vec<&Vec<String>> = ix
            .layer
            .type_shapes_of(view)
            .into_iter()
            .filter_map(|(_, shape)| shape.bases.first())
            .map(|b| &b.type_args)
            .filter(|args| !args.is_empty())
            .collect();
        given.sort();
        given.dedup();
        match given.as_slice() {
            [args] => match args.as_slice() {
                [arg] if !arg.contains('<') => ix.type_named(arg).ok(),
                _ => None,
            },
            _ => None,
        }
    }

    fn holds_view_model_field(&self, view: &NodeKey) -> bool {
        let ix = self.index();
        let holds = |ty: &NodeKey| {
            ix.members_of(ty).iter().any(|m| {
                ix.symbol(m)
                    .is_some_and(|s| s.name == VIEW_MODEL_FIELD && s.is_value())
            })
        };
        holds(view) || matches!(ix.base_of(view), Ok(Some(base)) if holds(base))
    }

    /// The type of the element `name` of the body's declaration: a named
    /// element the declaration holds, at the one element node of its line.
    fn element_type(&self, at: &At<'_>, name: &str) -> Option<String> {
        let from = at.from?;
        let named: Vec<u32> = self
            .index()
            .members_of(from)
            .iter()
            .filter_map(|k| self.declared_name(k))
            .filter(|(n, _)| *n == name)
            .filter_map(|(_, line)| line)
            .collect();
        let [line] = named.as_slice() else {
            return None;
        };
        let mut elements = at.body.nodes.iter().filter_map(|n| match &n.kind {
            BodyNodeKind::Element(t) if n.place.start.line == *line => Some(t),
            _ => None,
        });
        let one = elements.next()?;
        elements.next().is_none().then(|| one.clone())
    }

    /// A handler of an event a markup body subscribes: a method of the type
    /// of the outermost body around it in its file.
    fn handler(&mut self, at: &At<'_>, bodies: &[MemberBody], s: &Subscription) {
        let Handler::MethodGroup(AccessPath { segments, .. }) = &s.handler else {
            return;
        };
        let [name] = segments.as_slice() else {
            return;
        };
        let head = Head {
            at,
            range: s.place,
            node: s.node,
            kind: MarkKind::Handler,
            mode: MarkUse::Call,
            path: vec![name.clone()],
        };
        let around = bodies
            .iter()
            .filter(|b| b.kind == BodyKind::Markup)
            .filter_map(|b| b.nodes.get(b.root as usize).map(|root| (b, root.place)))
            .filter(|(_, r)| r.start <= s.place.start && s.place.end <= r.end)
            .min_by_key(|(_, r)| (r.start, std::cmp::Reverse(r.end)))
            .and_then(|(b, _)| tag_value(&b.tags, TARGET_TYPE));
        let Some(full) = around else {
            return self.unpaired(head, Unpaired::NoTargetType, 0);
        };
        match self.index().type_exactly(full) {
            Ok(ty) => self.follow(head, &ty, vec![name.clone()], 0),
            Err(reason) => self.stopped(head, reason, 0),
        }
    }

    /// Walk `names` from `ty` and mark every link from `skip` on (the names
    /// before it lead to the type the written path starts at); what could
    /// not be told from there on is a match by name alone.
    fn follow(&mut self, head: Head<'_>, ty: &NodeKey, names: Vec<String>, skip: usize) {
        let place = head.place();
        let caller = head.at.from.cloned().unwrap_or_else(|| ty.clone());
        let scope = Scope {
            ty,
            body: None,
            node: None,
        };
        let path = AccessPath {
            root: PathRoot::This,
            segments: names.clone(),
        };
        let res = self.links.resolve(&caller, &scope, &path, None, &place);
        let mut marked = Vec::new();
        for l in res.links.iter().skip(skip) {
            self.put(&head, &l.link.member, l.grade);
            marked.push(l.link.member.clone());
        }
        // The names of `names` from which on nothing was told.
        let told = res.links.len();
        let outcome = match &res.outcome {
            Outcome::Resolved { grade, .. } => SiteOutcome::Resolved { grade: *grade },
            Outcome::Ambiguous { reason } => {
                if told >= skip {
                    for c in &res.candidates.shown {
                        self.put(&head, c, Confidence::Ambiguous);
                    }
                }
                self.names_from(&names, (told + 1).max(skip));
                SiteOutcome::Ambiguous {
                    reason: reason.clone(),
                }
            }
            Outcome::Unresolved { reason } => {
                self.names_from(&names, told.max(skip));
                SiteOutcome::Unresolved {
                    reason: reason.clone(),
                }
            }
        };
        self.sites.push(MarkupSite {
            place,
            body: head.at.index,
            node: head.node,
            from: head.at.from.cloned(),
            kind: head.kind,
            mode: head.mode,
            path: head.path,
            links: marked,
            outcome,
        });
    }

    fn names_from(&mut self, names: &[String], from: usize) {
        for name in names.iter().skip(from) {
            self.name_only(name);
        }
    }

    fn put(&mut self, head: &Head<'_>, member: &NodeKey, grade: Confidence) {
        let mark = Mark {
            place: head.place(),
            kind: head.kind,
            mode: head.mode,
            body: head.at.index,
            node: head.node,
            from: head.at.from.cloned(),
            grade,
        };
        self.mark(member, mark);
    }

    /// The first type could not be told: every written name from `from` on
    /// is a match by name alone.
    fn stopped(&mut self, head: Head<'_>, reason: Unresolved, from: usize) {
        self.names_from(&head.path.clone(), from);
        self.sites.push(MarkupSite {
            place: head.place(),
            body: head.at.index,
            node: head.node,
            from: head.at.from.cloned(),
            kind: head.kind,
            mode: head.mode,
            path: head.path,
            links: Vec::new(),
            outcome: SiteOutcome::Unresolved { reason },
        });
    }

    /// Not walked: every written name from `from` on is a match by name
    /// alone.
    fn unpaired(&mut self, head: Head<'_>, why: Unpaired, from: usize) {
        self.names_from(&head.path.clone(), from);
        self.sites.push(MarkupSite {
            place: head.place(),
            body: head.at.index,
            node: head.node,
            from: head.at.from.cloned(),
            kind: head.kind,
            mode: head.mode,
            path: head.path,
            links: Vec::new(),
            outcome: SiteOutcome::Unpaired { why },
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_qualified_name_is_a_type_and_a_setter_is_told_by_its_element() {
        assert!(qualified("Acme.Settings"));
        assert!(qualified("http://p|Button"));
        assert!(!qualified("Holder"));
        assert!(is_setter(&BodyNodeKind::Element("Setter".into())));
        assert!(is_setter(&BodyNodeKind::Element("Acme.Ui.Setter".into())));
        assert!(!is_setter(&BodyNodeKind::Element("Button".into())));
        assert!(!is_setter(&BodyNodeKind::Body));
    }
}
