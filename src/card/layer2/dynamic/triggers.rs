use super::{
    tag_value, tags, Builder, MarkupSite, MarkupTrigger, MarkupTriggerKind, SiteOutcome,
    CONTROL_KEY, KEY_GESTURE, TARGET_TYPE, TRIGGER, X_CLASS_KEY,
};
use crate::card::layer2::entry::Place;
use crate::extract::bodies::{BodyKind, NodeKind as BodyNodeKind};
use crate::graph::NodeKey;
use std::collections::BTreeMap;

/// The last name of an element's type as markup spells it: `Acme.Ui.Item`,
/// `http://…|MenuItem`, `MenuItem`.
fn last_type_name(name: &str) -> &str {
    name.rsplit(['.', '|']).next().unwrap_or(name)
}

/// The name WPF's menu item has; a type is a menu item when it is one or
/// derives from one (plan 2c, KTD15).
const MENU_ITEM: &str = "MenuItem";

impl<'b, 'i, 'a> Builder<'b, 'i, 'a> {
    // ---- The triggers of markup (U9: R9, KTD15) ----

    /// Every element with a command (`trigger: control`) and every key
    /// binding (`key-gesture`) of the markup bodies.
    pub(super) fn markup_triggers(&mut self) {
        let layer = self.index().layer;
        let mut out = Vec::new();
        // The sites the walk told, by the element they are written at, built
        // once: a large dictionary has many elements and many sites.
        let mut told_at: BTreeMap<(&str, usize, u32), Vec<&MarkupSite>> = BTreeMap::new();
        for s in &self.sites {
            if let (Some(node), SiteOutcome::Resolved { .. }, false) =
                (s.node, &s.outcome, s.links.is_empty())
            {
                told_at
                    .entry((s.place.path.as_str(), s.body, node))
                    .or_default()
                    .push(s);
            }
        }
        for (path, file) in layer.files() {
            for (index, body) in file.model.members.iter().enumerate() {
                if body.kind != BodyKind::Markup {
                    continue;
                }
                let member = file
                    .owners
                    .get(index)
                    .and_then(Option::as_ref)
                    .map_or_else(|| NodeKey::new(path), |(k, _)| k.clone());
                // The root of a view with code-behind is written in its
                // class: the full name `x:Class` gives (the body's
                // `target-type`), as a template names the type it is for.
                let member = match tag_value(&body.tags, TARGET_TYPE) {
                    Some(class) if member.as_str().ends_with(X_CLASS_KEY) => NodeKey::new(class),
                    _ => member,
                };
                for node in &body.nodes {
                    let BodyNodeKind::Element(element) = &node.kind else {
                        continue;
                    };
                    let control = tag_value(&node.tags, TRIGGER) == Some("control");
                    if !control && !node.tags.iter().any(|t| t.name == KEY_GESTURE) {
                        continue;
                    }
                    let place = Place {
                        path: path.to_string(),
                        range: node.place,
                    };
                    let told: Vec<&MarkupSite> = told_at
                        .get(&(path, index, node.id))
                        .cloned()
                        .unwrap_or_default();
                    let last = |sites: &[&MarkupSite]| -> Vec<NodeKey> {
                        let mut v: Vec<NodeKey> = Vec::new();
                        for s in sites {
                            if let Some(l) = s.links.last() {
                                if !v.contains(l) {
                                    v.push(l.clone());
                                }
                            }
                        }
                        v
                    };
                    if control {
                        let key = tag_value(&node.tags, CONTROL_KEY)
                            .unwrap_or_else(|| last_type_name(element));
                        let kind = if self.is_menu_item(element) {
                            MarkupTriggerKind::MenuItem
                        } else {
                            MarkupTriggerKind::Control
                        };
                        out.push(MarkupTrigger {
                            kind,
                            key: key.to_string(),
                            member: member.clone(),
                            place: place.clone(),
                            targets: last(&told),
                        });
                    }
                    for gesture in tags(&node.tags, KEY_GESTURE) {
                        let Some(value) = &gesture.value else {
                            continue;
                        };
                        // The command as written, when an access spells it.
                        let named: Vec<&MarkupSite> = told
                            .iter()
                            .copied()
                            .filter(|s| gesture.args.first() == Some(&s.path.join(".")))
                            .collect();
                        let targets = if named.is_empty() {
                            last(&told)
                        } else {
                            last(&named)
                        };
                        out.push(MarkupTrigger {
                            kind: MarkupTriggerKind::Key,
                            key: value.clone(),
                            member: member.clone(),
                            place: place.clone(),
                            targets,
                        });
                    }
                }
            }
        }
        self.triggers = out;
    }

    /// Whether markup's element type `element` is a menu item: WPF's own by
    /// its name, or a C# type of exactly that full name whose base list, or
    /// that of a type it derives from, names one (KTD15: by the type, never
    /// by a list of controls).
    fn is_menu_item(&self, element: &str) -> bool {
        if last_type_name(element) == MENU_ITEM {
            return true;
        }
        let ix = self.index();
        let Ok(ty) = ix.type_exactly(element) else {
            return false;
        };
        std::iter::once(&ty).chain(ix.ancestors(&ty)).any(|t| {
            ix.layer.type_shapes_of(t).iter().any(|(_, shape)| {
                shape
                    .bases
                    .iter()
                    .any(|b| b.base.last().is_some_and(|l| l == MENU_ITEM))
            })
        })
    }
}
