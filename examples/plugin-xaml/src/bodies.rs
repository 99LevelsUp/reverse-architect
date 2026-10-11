//! Pass 3: the bodies of a XAML file's styles, templates and `x:Class` root.
//!
//! **Which bodies.** Every style and template that pass 1 declares, and the
//! content of the root of a file with `x:Class`, is one `member-body` of
//! kind `markup`. Its `name-position` is the one pass 1 gives the
//! declaration (the same function computes both), and its root node is a
//! `body` node spanning the declaring element. Every element under it is an
//! `element(<name>)` node, the name qualified by the CLR namespace its
//! prefix maps (`Acme.Views.ViewA`; a URI-mapped `Button` stays `Button`).
//! An element that pass 1 declares as a resource of its own (a style, a
//! template or any other resource in a `*.Resources` block) is not part of
//! the body around it: a style or template has its own body, any other
//! resource none — unless it holds a markup extension this pass could not
//! take apart (below). An inline template (a `ListBox.ItemTemplate` with no
//! key) is part of the body it is written in.
//!
//! **Tags on a body.** `target-type` on a style or template with
//! `TargetType`, the type qualified by the CLR namespace of its `xmlns`
//! prefix; on the `x:Class` root, the class's full name from `x:Class`.
//!
//! **Bindings.** `{Binding P}` (or `Path=P`, or a `<Binding Path="P"/>`
//! element) is a `read` of the path's segments, root `name`; with an explicit
//! `Mode=TwoWay` or `Mode=OneWayToSource` also a `write`. `{TemplateBinding
//! P}` reads `P`. `{Binding}` and `{Binding .}` read nothing. An indexer is
//! dropped (`Items[0].Name` reads `[Items, Name]`), a parenthesized attached
//! property reads its owner type and property (`(Validation.Errors)`), and
//! `/` separates segments like `.`. Each element with a binding carries one
//! `data-source` tag per distinct source of its bindings:
//!
//! - `element` (args: the name) for `ElementName=`;
//! - `self` and `templated-parent` for `RelativeSource Self` /
//!   `TemplatedParent`, and `templated-parent` for `{TemplateBinding}`;
//! - `explicit` for `Source=` (args: `[T, M]` when it is `{x:Static T.M}`),
//!   for any other `RelativeSource` (args: the `AncestorType`), and for a
//!   binding under an element that sets `DataContext` (itself included, for
//!   its other attributes): the source is set, but not one the file names;
//! - otherwise what the nearest data template around it says: `typed`
//!   (args: the `DataType`, fully qualified) under a template with
//!   `DataType`, `item` under one without (an `ItemTemplate`, a keyed row
//!   template); with no data template up to the body's root, `view`: the
//!   data context of the view, of the control a template is applied to, or
//!   of the element a style is set on.
//!
//! **Other accesses.** `{x:Static T.M}` is a `read` of `[T, M]`, `T`
//! qualified like a type name above, and so is a plain `Command="T.M"`. An
//! attached property `Owner.Property="…"` is a `write` of `[Owner,
//! Property]`, `Setter Property="P"` a `write` of `[T, P]` where `T` is the
//! type of the element `TargetName` names in the body or else the
//! `TargetType` of the nearest style or template around the setter (`[P]`
//! when neither says). A write's `rhs` is the value written when the syntax
//! says it (`True` → a boolean, a number, `{x:Null}`, a string literal).
//!
//! **Triggers.** An element with `Command=` carries `trigger: control` and
//! `control-key`: its identity, the first of `x:Name`/`Name` (args `name`),
//! `x:Key` (`key`), the resource key its `Content`, `Header` or `ToolTip`
//! names (`resource`), or the literal text of one of them (`literal`). A
//! `KeyBinding` (or `MouseBinding`, any element whose name ends in `Binding`)
//! is an input binding, not a control: it carries no `trigger`, and a
//! `KeyBinding` carries `key-gesture`, its gesture as the value (`Gesture`,
//! or `Modifiers+Key`) and the command as written as the one arg.
//!
//! **Event handlers.** XAML does not say which attribute is an event, so this
//! is a heuristic: in a file with `x:Class`, a plain attribute whose value is
//! a C# identifier (not `True`/`False`), whose name does not start with `Is`
//! or `Has` and is not one this pass reads as a name (below), is a handler
//! when its name ends like an event (`Click`, `…Changed`, `Loaded`, `…Down`,
//! … see [`EVENT_ENDINGS`]) or its value looks like a handler (`OnSave`,
//! `Save_Click`). It is a `subscription` with a method-group handler `[value]`;
//! its source is `none` on the view root itself, `[name]` for an element with
//! a name and an `expression` otherwise. An `EventSetter` is a subscription
//! whatever the file.
//!
//! **Literals.** A plain attribute value is a `string-literal` (context
//! `attribute-arg(<attribute>, none, none)`, entities expanded) when it does
//! not start with `{`, holds two letters in a row, and is not a colour
//! (`#AARRGGBB`, `sc#…`) or path data (only path commands, digits and
//! separators). The rule is syntactic, with no list of attribute names: a
//! value the pass also reads as something else (`Setter Property="IsBusy"`,
//! `Gesture="Ctrl+S"`, `Value="True"`) is a literal as well. Not literals:
//! `xmlns` declarations, attributes of the XAML language namespace (`x:Key`,
//! `x:Name`…) and a value taken for an event handler. The host redacts every
//! text; this pass filters nothing for secrets.
//!
//! **Unparsed markup.** A value written as a markup extension (it opens with
//! `{`, not with the `{}` escape) that the parser cannot take apart
//! (`Text="{Binding Title"`) gives no access and no literal; its element
//! carries `unparsed-markup` with the attribute's name as the value, so the
//! loss is said rather than silent. So that no such value goes unsaid in a
//! resource that is neither a style nor a template (`<SolidColorBrush
//! x:Key="Accent" Color="{DynamicResource"/>`), one that holds any is a body
//! of its own, paired with its declaration like a style's, with its element
//! nodes and those tags alone: no access, subscription, literal or other tag.
//! A value on an element of no body — in a file without `x:Class`, its root
//! and what lies outside every resource, such as a merged dictionary — is
//! still not read.

use crate::declare::{local_name, resource, resource_ref, view_class, STYLE, TEMPLATE};
use crate::parse::{markup, parse, split_prefix, ArgValue, Doc, Element, Error, Markup};
use crate::ra::plugin::types::{
    AccessMode, AccessPath, BodyKind, BodyNode, Constant, FileBodies, Handler, LiteralContext,
    MemberAccess, MemberBody, NodeKind, PathRoot, Position, Range as TextRange, StringLiteral,
    Subscription, SubscriptionMode, Tag, ValueRef,
};
use crate::xmlns::{is_xaml, type_of_value, TypeRef};
use std::borrow::Cow;

/// The record kinds and tags this component reports (`manifest.emits`).
pub const EMITS: &[&str] = &[
    "member-access",
    "subscription",
    "string-literal",
    "data-source",
    "target-type",
    "trigger",
    "control-key",
    "key-gesture",
    UNPARSED_MARKUP,
];

/// The tag on an element with a value written as a markup extension that
/// this pass could not take apart (`Text="{Binding Title"`); its value is the
/// attribute's name. What the extension would have said is lost, so a host
/// that sees it knows the file was not read in full.
pub const UNPARSED_MARKUP: &str = "unparsed-markup";

/// How an attribute name ends when it names an event (`Click`,
/// `SelectionChanged`, `PreviewKeyDown`, `Loaded`).
pub const EVENT_ENDINGS: &[&str] = &[
    "Click",
    "Changed",
    "Loaded",
    "Unloaded",
    "Initialized",
    "Down",
    "Up",
    "Enter",
    "Leave",
    "Move",
    "Wheel",
    "Opened",
    "Opening",
    "Closed",
    "Closing",
    "Checked",
    "Unchecked",
    "Executed",
    "CanExecute",
    "Drop",
    "Over",
    "Expanded",
    "Collapsed",
    "Selected",
    "Unselected",
    "Activated",
    "Deactivated",
    "Focus",
    "Scroll",
    "Input",
    "Completed",
    "Error",
    "Updated",
    "Rendered",
    "Navigated",
    "Navigating",
];

/// The bodies of one file, in document order.
pub fn bodies(source: &[u8]) -> Result<FileBodies, Error> {
    let doc = parse(source)?;
    let class = view_class(&doc);
    let mut members = Vec::new();
    for index in 0..doc.elements.len() {
        // The name position, the target type and whether the element is a
        // resource other than a style or a template.
        let opened = if index == 0 && class.is_some() {
            class
                .clone()
                .map(|c| (c.start, doc.text(&c).trim().to_string(), false))
        } else {
            match resource(&doc, index) {
                Some((kind, _, name_at)) if kind == STYLE || kind == TEMPLATE => {
                    let element = &doc.elements[index];
                    let target = doc
                        .attr(element, "TargetType")
                        .and_then(|a| type_of_value(&doc, index, &a.value))
                        .map(|t| t.clr_name());
                    Some((name_at, target.unwrap_or_default(), false))
                }
                Some((_, _, name_at)) => Some((name_at, String::new(), true)),
                None => None,
            }
        };
        let Some((name_at, target, other)) = opened else {
            continue;
        };
        let mut body = Body {
            doc: &doc,
            has_class: class.is_some(),
            root: index,
            nodes: Vec::new(),
            accesses: Vec::new(),
            subscriptions: Vec::new(),
            literals: Vec::new(),
        };
        body.walk();
        if other && !body.unparsed_only() {
            continue;
        }
        let (line, column) = doc.position(name_at);
        let tags = if target.is_empty() {
            Vec::new()
        } else {
            vec![tag("target-type", Some(target), Vec::new())]
        };
        members.push(MemberBody {
            name_position: Position { line, column },
            kind: BodyKind::Markup,
            root: 0,
            nodes: body.nodes,
            accesses: body.accesses,
            invocations: Vec::new(),
            lambdas: Vec::new(),
            subscriptions: body.subscriptions,
            literals: body.literals,
            name_ofs: Vec::new(),
            typed_names: Vec::new(),
            tags,
        });
    }
    Ok(FileBodies {
        types: Vec::new(),
        members,
        typed_members: Vec::new(),
        metas: Vec::new(),
    })
}

fn tag(name: &str, value: Option<String>, args: Vec<String>) -> Tag {
    Tag {
        name: name.into(),
        value,
        args,
    }
}

fn push_tag(tags: &mut Vec<Tag>, t: Tag) {
    if !tags.contains(&t) {
        tags.push(t);
    }
}

fn to_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

fn name_path(segments: Vec<String>) -> AccessPath {
    AccessPath {
        root: PathRoot::Name,
        segments,
    }
}

/// Whether `s` is spelled like a C# identifier.
fn is_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_alphabetic() || c == '_')
        && chars.all(|c| c.is_alphanumeric() || c == '_')
}

/// `IsEnabled`, `HasItems`: a property, whatever its name ends with.
fn is_flag_property(name: &str) -> bool {
    ["Is", "Has"].iter().any(|p| {
        name.strip_prefix(p)
            .and_then(|rest| rest.chars().next())
            .is_some_and(char::is_uppercase)
    })
}

/// `OnSave`, `Save_Click`: a value named like a handler.
fn looks_like_handler(value: &str) -> bool {
    let on = value
        .strip_prefix("On")
        .and_then(|rest| rest.chars().next())
        .is_some_and(char::is_uppercase);
    let underscore = value.rsplit_once('_').is_some_and(|(before, after)| {
        !before.is_empty() && after.chars().next().is_some_and(char::is_uppercase)
    });
    on || underscore
}

/// The value as text, its character and entity references expanded.
fn unescaped(raw: &str) -> String {
    quick_xml::escape::unescape(raw)
        .unwrap_or(Cow::Borrowed(raw))
        .into_owned()
}

/// Whether an attribute value is written as a markup extension — it opens
/// with `{`, and not with the `{}` escape — that [`markup`] could not take
/// apart (`{Binding Title`): it is called only for a value `markup` refused.
fn is_unparsed_markup(raw: &str) -> bool {
    let value = raw.trim_start();
    value.starts_with('{') && !value.starts_with("{}")
}

/// Whether an attribute value is a literal worth reporting: not a markup
/// extension, a word in it, not a colour and not path data.
pub fn is_literal(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.starts_with('{') {
        return false;
    }
    let chars: Vec<char> = trimmed.chars().collect();
    if !chars
        .windows(2)
        .any(|w| w[0].is_alphabetic() && w[1].is_alphabetic())
    {
        return false;
    }
    let colour = trimmed
        .strip_prefix('#')
        .is_some_and(|hex| !hex.is_empty() && hex.chars().all(|c| c.is_ascii_hexdigit()))
        || trimmed.starts_with("sc#");
    const PATH: &str = "MmLlHhVvCcSsQqTtAaZzFf0123456789 ,.+-eE\t\r\n";
    let geometry =
        trimmed.chars().any(|c| c.is_ascii_digit()) && trimmed.chars().all(|c| PATH.contains(c));
    !colour && !geometry
}

/// What a plain value written to a property is, as the contract names it.
fn value_ref(text: &str, literal: Option<u32>) -> ValueRef {
    let t = text.trim();
    if t.eq_ignore_ascii_case("true") {
        ValueRef::Constant(Constant::Boolean(true))
    } else if t.eq_ignore_ascii_case("false") {
        ValueRef::Constant(Constant::Boolean(false))
    } else if !t.is_empty() && t.chars().any(|c| c.is_ascii_digit()) && t.parse::<f64>().is_ok() {
        ValueRef::Constant(Constant::Number(t.to_string()))
    } else if let Some(id) = literal {
        ValueRef::Literal(id)
    } else {
        ValueRef::Other
    }
}

/// The segments of a binding path.
fn path_segments(doc: &Doc<'_>, at: usize, path: &str) -> Vec<String> {
    let mut parts: Vec<&str> = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (i, c) in path.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            '.' | '/' if depth == 0 => {
                parts.push(&path[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&path[start..]);
    let mut out = Vec::new();
    for part in parts {
        let part = part.trim();
        let part = part
            .split_once('[')
            .map_or(part, |(before, _)| before)
            .trim();
        if let Some(inner) = part.strip_prefix('(').and_then(|p| p.strip_suffix(')')) {
            // An attached property: `(local:Owner.Property)`.
            match inner.rsplit_once('.') {
                Some((owner, property)) => {
                    if let Some(ty) = TypeRef::read(doc, at, owner, 0) {
                        out.push(ty.clr_name());
                    }
                    out.push(property.trim().to_string());
                }
                None => out.push(split_prefix(inner.trim()).1.to_string()),
            }
        } else if !part.is_empty() {
            out.push(part.to_string());
        }
    }
    out.retain(|s| !s.is_empty());
    out
}

/// `T.M` (`local:Palette.Ink`, `Acme.Settings.Default`) as `[T, M]`, `T`
/// read in the scope of element `at`.
fn static_member(doc: &Doc<'_>, at: usize, text: &str) -> Option<Vec<String>> {
    let (owner, member) = text.trim().rsplit_once('.')?;
    let member = member.trim();
    if member.is_empty() {
        return None;
    }
    let ty = TypeRef::read(doc, at, owner, 0)?;
    Some(vec![ty.clr_name(), member.to_string()])
}

/// The source a binding names for itself, if any: `element`, `self`,
/// `templated-parent` or `explicit`.
fn named_source(
    doc: &Doc<'_>,
    at: usize,
    element_name: Option<&str>,
    relative: Option<&Markup<'_>>,
    source: Option<&ArgValue<'_>>,
) -> Option<(&'static str, Vec<String>)> {
    if let Some(name) = element_name {
        return Some(("element", vec![name.to_string()]));
    }
    if let Some(rel) = relative {
        let mode = match rel.positional().or_else(|| rel.named("Mode")) {
            Some(ArgValue::Text { text, .. }) => *text,
            _ => "",
        };
        return Some(match mode {
            "Self" => ("self", Vec::new()),
            "TemplatedParent" => ("templated-parent", Vec::new()),
            _ => {
                let ancestor = match rel.named("AncestorType") {
                    Some(ArgValue::Text { text, .. }) => {
                        TypeRef::read(doc, at, text, 0).map(|t| t.clr_name())
                    }
                    Some(ArgValue::Markup(m)) => {
                        crate::xmlns::x_type(doc, at, m).map(|t| t.clr_name())
                    }
                    None => None,
                };
                ("explicit", ancestor.into_iter().collect())
            }
        });
    }
    if let Some(source) = source {
        let args = match source {
            ArgValue::Markup(m) if is_static(doc, at, m) => match m.positional() {
                Some(ArgValue::Text { text, .. }) => {
                    static_member(doc, at, text).unwrap_or_default()
                }
                _ => Vec::new(),
            },
            _ => Vec::new(),
        };
        return Some(("explicit", args));
    }
    None
}

fn is_static(doc: &Doc<'_>, at: usize, ext: &Markup<'_>) -> bool {
    let (prefix, name) = split_prefix(ext.name);
    name == "Static" && is_xaml(doc, at, prefix)
}

/// The parts of one binding, written as an extension or as an element.
struct Binding<'a, 'm> {
    path: Option<(&'a str, usize)>,
    mode: Option<&'a str>,
    element_name: Option<&'a str>,
    relative: Option<&'m Markup<'a>>,
    source: Option<&'m ArgValue<'a>>,
    templated: bool,
}

struct Body<'d, 'a> {
    doc: &'d Doc<'a>,
    has_class: bool,
    root: usize,
    nodes: Vec<BodyNode>,
    accesses: Vec<MemberAccess>,
    subscriptions: Vec<Subscription>,
    literals: Vec<StringLiteral>,
}

impl<'a> Body<'_, 'a> {
    fn position(&self, offset: usize) -> Position {
        let (line, column) = self.doc.position(offset);
        Position { line, column }
    }

    /// The text `len` bytes long at `at`, both ends inclusive.
    fn text_range(&self, at: usize, len: usize) -> TextRange {
        TextRange {
            start: self.position(at),
            end: self.position(at + len.max(1) - 1),
        }
    }

    fn element_range(&self, element: &Element) -> TextRange {
        TextRange {
            start: self.position(element.start),
            end: self.position(element.end.saturating_sub(1).max(element.start)),
        }
    }

    /// The elements of the body in document order: the root, then every
    /// element under it that is not a declaration of its own.
    fn walk(&mut self) {
        let mut stack = vec![(self.root, None)];
        let doc = self.doc;
        while let Some((index, parent)) = stack.pop() {
            let id = to_u32(self.nodes.len());
            let element = &doc.elements[index];
            let kind = if index == self.root {
                NodeKind::Body
            } else {
                NodeKind::Element(self.element_name(index))
            };
            self.nodes.push(BodyNode {
                id,
                kind,
                place: self.element_range(element),
                parent,
                condition: None,
                returns: None,
                tags: Vec::new(),
            });
            self.element(index, id);
            for &child in element.children.iter().rev() {
                if resource(doc, child).is_none() {
                    stack.push((child, Some(id)));
                }
            }
        }
    }

    /// Cuts a walked body down to its nodes and their `unparsed-markup`
    /// tags, and says whether one is left: the body of a resource that is
    /// neither a style nor a template, which has none of its own (see the
    /// module documentation).
    fn unparsed_only(&mut self) -> bool {
        self.accesses.clear();
        self.subscriptions.clear();
        self.literals.clear();
        for node in &mut self.nodes {
            node.tags.retain(|t| t.name == UNPARSED_MARKUP);
        }
        self.nodes.iter().any(|n| !n.tags.is_empty())
    }

    /// An element's name as a type name (`Acme.Views.ViewA`, `Button`,
    /// `Grid.Resources`).
    fn element_name(&self, index: usize) -> String {
        let element = &self.doc.elements[index];
        let qname = self.doc.name(element);
        TypeRef::read(self.doc, index, qname, element.name.start)
            .map_or_else(|| qname.to_string(), |t| t.clr_name())
    }

    fn attr_text(&self, index: usize, name: &str) -> Option<&'a str> {
        let doc = self.doc;
        doc.attr(&doc.elements[index], name)
            .map(|a| doc.text(&a.value).trim())
    }

    /// Whether this pass reads the attribute's value as a name of something
    /// else, which is then never taken for an event handler. (Literals do not
    /// look at this: their rule is syntactic.)
    fn read_as_name(&self, index: usize, attr: &str) -> bool {
        let element = local_name(self.doc, index);
        matches!(
            attr,
            "TargetType" | "DataType" | "Property" | "TargetName" | "Name" | "Command"
        ) || (element == "EventSetter" && matches!(attr, "Event" | "Handler"))
            || (element == "KeyBinding" && matches!(attr, "Gesture" | "Key" | "Modifiers"))
            || (element == "ResourceDictionary" && attr == "Source")
            || self.is_binding_element(index)
    }

    fn is_binding_element(&self, index: usize) -> bool {
        let (prefix, name) = split_prefix(self.doc.name(&self.doc.elements[index]));
        name == "Binding" && !is_xaml(self.doc, index, prefix)
    }

    fn is_handler(&self, index: usize, attr: &str, value: &str) -> bool {
        let local = attr.rsplit('.').next().unwrap_or(attr);
        self.has_class
            && is_identifier(value)
            && !value.eq_ignore_ascii_case("true")
            && !value.eq_ignore_ascii_case("false")
            && !is_flag_property(local)
            && !self.read_as_name(index, attr)
            // An attached property named like nothing in particular
            // (`AutomationProperties.AutomationId="Btn_Save"`) stays a write.
            && (EVENT_ENDINGS.iter().any(|e| local.ends_with(e))
                || (!attr.contains('.') && looks_like_handler(value)))
    }

    /// The source a binding without one of its own reads from, at element
    /// `index`. `on_data_context` is true for the binding that sets the
    /// element's own `DataContext`, which reads from the context around it.
    fn inherited_source(&self, index: usize, on_data_context: bool) -> (&'static str, Vec<String>) {
        let doc = self.doc;
        let sets_context = |i: usize| doc.attr(&doc.elements[i], "DataContext").is_some();
        if !on_data_context && sets_context(index) {
            return ("explicit", Vec::new());
        }
        // The elements around it up to the body's root; the root alone for
        // the root itself.
        let mut chain = Vec::new();
        if index == self.root {
            chain.push(index);
        } else {
            for i in doc.ancestors(index) {
                chain.push(i);
                if i == self.root {
                    break;
                }
            }
        }
        for i in chain {
            let name = local_name(doc, i);
            if !name.contains('.') && name.ends_with("DataTemplate") {
                let typed = doc
                    .attr(&doc.elements[i], "DataType")
                    .and_then(|a| type_of_value(doc, i, &a.value));
                return match typed {
                    Some(ty) => ("typed", vec![ty.clr_name()]),
                    None => ("item", Vec::new()),
                };
            }
            // The element's own `DataContext` was decided above.
            if i != index && sets_context(i) {
                return ("explicit", Vec::new());
            }
        }
        ("view", Vec::new())
    }

    fn data_source(&mut self, node: u32, source: (&'static str, Vec<String>)) {
        let t = tag("data-source", Some(source.0.to_string()), source.1);
        push_tag(&mut self.nodes[node as usize].tags, t);
    }

    fn access(
        &mut self,
        segments: Vec<String>,
        mode: AccessMode,
        place: TextRange,
        node: u32,
        rhs: Option<ValueRef>,
    ) {
        self.accesses.push(MemberAccess {
            target: name_path(segments),
            mode,
            place,
            node: Some(node),
            rhs,
        });
    }

    fn binding(&mut self, index: usize, node: u32, attr: &str, b: &Binding<'a, '_>) {
        let source = if b.templated {
            Some(("templated-parent", Vec::new()))
        } else {
            named_source(self.doc, index, b.element_name, b.relative, b.source)
        };
        let source = source.unwrap_or_else(|| self.inherited_source(index, attr == "DataContext"));
        self.data_source(node, source);
        let Some((path, at)) = b.path else {
            return;
        };
        let segments = path_segments(self.doc, index, path);
        if segments.is_empty() {
            return;
        }
        let place = self.text_range(at, path.len());
        let writes = b.mode.is_some_and(|m| {
            m.eq_ignore_ascii_case("TwoWay") || m.eq_ignore_ascii_case("OneWayToSource")
        });
        if writes {
            self.access(segments.clone(), AccessMode::Read, place, node, None);
            self.access(segments, AccessMode::Write, place, node, None);
        } else {
            self.access(segments, AccessMode::Read, place, node, None);
        }
    }

    /// The bindings and static reads of one extension and those nested in it.
    fn extension(&mut self, index: usize, node: u32, attr: &str, ext: &Markup<'a>) {
        let (prefix, name) = split_prefix(ext.name);
        let text = |v: Option<&ArgValue<'a>>| match v {
            Some(ArgValue::Text { text, at }) => Some((*text, *at)),
            _ => None,
        };
        if is_xaml(self.doc, index, prefix) {
            if name == "Static" {
                if let Some((t, at)) = text(ext.positional().or_else(|| ext.named("Member"))) {
                    if let Some(segments) = static_member(self.doc, index, t) {
                        let place = self.text_range(at, t.len());
                        self.access(segments, AccessMode::Read, place, node, None);
                    }
                }
            }
        } else if name == "Binding" || name == "TemplateBinding" {
            let templated = name == "TemplateBinding";
            let path_arg = if templated {
                ext.positional().or_else(|| ext.named("Property"))
            } else {
                ext.positional().or_else(|| ext.named("Path"))
            };
            let relative = match ext.named("RelativeSource") {
                Some(ArgValue::Markup(m)) => Some(m),
                _ => None,
            };
            let b = Binding {
                path: text(path_arg),
                mode: text(ext.named("Mode")).map(|(t, _)| t),
                element_name: text(ext.named("ElementName")).map(|(t, _)| t),
                relative,
                source: ext.named("Source"),
                templated,
            };
            self.binding(index, node, attr, &b);
        }
        for arg in &ext.args {
            if let ArgValue::Markup(inner) = &arg.value {
                self.extension(index, node, attr, inner);
            }
        }
    }

    fn literal(&mut self, node: u32, attr: &str, raw: &str, at: usize) -> u32 {
        let id = to_u32(self.literals.len());
        let place = self.text_range(at, raw.len());
        self.literals.push(StringLiteral {
            id,
            value: unescaped(raw),
            context: LiteralContext::AttributeArg((attr.to_string(), None, None)),
            place,
            node: Some(node),
        });
        id
    }

    fn subscribe(
        &mut self,
        node: u32,
        source: Option<AccessPath>,
        event: &str,
        handler: &str,
        at: usize,
    ) {
        let place = self.text_range(at, handler.len());
        self.subscriptions.push(Subscription {
            source,
            event: event.to_string(),
            handler: Handler::MethodGroup(name_path(vec![handler.to_string()])),
            mode: SubscriptionMode::Add,
            place,
            node: Some(node),
        });
    }

    /// The name an element is known by: `x:Name`, else `Name`.
    fn element_id(&self, index: usize) -> Option<&'a str> {
        let doc = self.doc;
        crate::xmlns::x_value(doc, index, "Name")
            .map(|v| doc.text(&v).trim())
            .or_else(|| self.attr_text(index, "Name"))
            .filter(|n| !n.is_empty())
    }

    /// What raises an event written on element `index`.
    fn event_source(&self, index: usize) -> Option<AccessPath> {
        if index == 0 && self.root == 0 && self.has_class {
            return None;
        }
        Some(match self.element_id(index) {
            Some(name) => name_path(vec![name.to_string()]),
            None => AccessPath {
                root: PathRoot::Expression,
                segments: Vec::new(),
            },
        })
    }

    fn element(&mut self, index: usize, node: u32) {
        let doc = self.doc;
        let element = &doc.elements[index];
        let binding_element = self.is_binding_element(index);
        let mut literal_of: Vec<Option<u32>> = vec![None; element.attrs.len()];
        for (n, attr) in element.attrs.iter().enumerate() {
            let name = doc.text(&attr.name);
            if name == "xmlns" || name.starts_with("xmlns:") {
                continue;
            }
            let (prefix, local) = split_prefix(name);
            if prefix.is_some() && is_xaml(doc, index, prefix) {
                continue;
            }
            let raw = doc.text(&attr.value);
            if let Some(ext) = markup(raw, attr.value.start) {
                self.extension(index, node, local, &ext);
                if local.contains('.') && !binding_element {
                    self.attached(index, node, name, attr.name.start, ValueRef::Other);
                }
                continue;
            }
            if is_unparsed_markup(raw) {
                push_tag(
                    &mut self.nodes[node as usize].tags,
                    tag(UNPARSED_MARKUP, Some(local.to_string()), Vec::new()),
                );
            }
            let value = raw.trim();
            if self.is_handler(index, local, value) {
                let at = attr.value.start + (raw.len() - raw.trim_start().len());
                let source = self.event_source(index);
                self.subscribe(node, source, local, value, at);
                continue;
            }
            if local == "Command" && !binding_element {
                if let Some(segments) = static_member(doc, index, value) {
                    let place = self.text_range(attr.value.start, raw.len());
                    self.access(segments, AccessMode::Read, place, node, None);
                }
            }
            let literal = is_literal(raw).then(|| self.literal(node, name, raw, attr.value.start));
            literal_of[n] = literal;
            if local.contains('.') && !binding_element {
                self.attached(index, node, name, attr.name.start, value_ref(raw, literal));
            }
        }
        if binding_element {
            self.binding_element(index, node);
        }
        match local_name(doc, index) {
            "Setter" => self.setter(index, node, &literal_of),
            "EventSetter" => {
                if let (Some(event), Some(handler)) =
                    (self.attr_text(index, "Event"), doc.attr(element, "Handler"))
                {
                    let raw = doc.text(&handler.value);
                    let value = raw.trim();
                    if is_identifier(value) {
                        let at = handler.value.start + (raw.len() - raw.trim_start().len());
                        let source = AccessPath {
                            root: PathRoot::Expression,
                            segments: Vec::new(),
                        };
                        self.subscribe(node, Some(source), event, value, at);
                    }
                }
            }
            "KeyBinding" => self.key_gesture(index, node),
            _ => {}
        }
        let input_binding = local_name(doc, index).ends_with("Binding");
        if !input_binding && doc.attr(element, "Command").is_some() {
            push_tag(
                &mut self.nodes[node as usize].tags,
                tag("trigger", Some("control".into()), Vec::new()),
            );
            if let Some((identity, how)) = self.control_key(index) {
                push_tag(
                    &mut self.nodes[node as usize].tags,
                    tag("control-key", Some(identity), vec![how.into()]),
                );
            }
        }
    }

    /// `Owner.Property="…"` on element `index`: a write through the owner
    /// type's accessors.
    fn attached(&mut self, index: usize, node: u32, name: &str, at: usize, rhs: ValueRef) {
        let Some((owner, property)) = name.rsplit_once('.') else {
            return;
        };
        let Some(ty) = TypeRef::read(self.doc, index, owner, at) else {
            return;
        };
        let place = self.text_range(at, name.len());
        self.access(
            vec![ty.clr_name(), property.to_string()],
            AccessMode::Write,
            place,
            node,
            Some(rhs),
        );
    }

    /// `<Binding Path="…" Mode="…"/>`, as inside a `MultiBinding`.
    fn binding_element(&mut self, index: usize, node: u32) {
        let doc = self.doc;
        let element = &doc.elements[index];
        let path = doc.attr(element, "Path").map(|a| {
            let raw = doc.text(&a.value);
            let lead = raw.len() - raw.trim_start().len();
            (raw.trim(), a.value.start + lead)
        });
        let relative = doc
            .attr(element, "RelativeSource")
            .and_then(|a| markup(doc.text(&a.value), a.value.start));
        let source_markup = doc
            .attr(element, "Source")
            .and_then(|a| markup(doc.text(&a.value), a.value.start));
        let source_value = source_markup.map(ArgValue::Markup).or_else(|| {
            doc.attr(element, "Source").map(|a| ArgValue::Text {
                text: doc.text(&a.value),
                at: a.value.start,
            })
        });
        let b = Binding {
            path,
            mode: self.attr_text(index, "Mode"),
            element_name: self.attr_text(index, "ElementName"),
            relative: relative.as_ref(),
            source: source_value.as_ref(),
            templated: false,
        };
        self.binding(index, node, "", &b);
    }

    /// `Setter Property="P"`: a write of `P` on the type the setter styles.
    fn setter(&mut self, index: usize, node: u32, literal_of: &[Option<u32>]) {
        let doc = self.doc;
        let element = &doc.elements[index];
        let Some(property) = doc.attr(element, "Property") else {
            return;
        };
        let raw = doc.text(&property.value);
        let text = raw.trim();
        if text.is_empty() || text.starts_with('{') {
            return;
        }
        let lead = raw.len() - raw.trim_start().len();
        let at = property.value.start + lead;
        let segments = match text.rsplit_once('.') {
            Some((owner, name)) => match TypeRef::read(doc, index, owner, at) {
                Some(ty) => vec![ty.clr_name(), name.to_string()],
                None => vec![name.to_string()],
            },
            None => match self.styled_type(index) {
                Some(owner) => vec![owner, text.to_string()],
                None => vec![text.to_string()],
            },
        };
        let rhs = match element
            .attrs
            .iter()
            .position(|a| doc.text(&a.name) == "Value")
        {
            Some(n) => {
                let value = doc.text(&element.attrs[n].value);
                if value.trim() == "{x:Null}" {
                    ValueRef::Constant(Constant::Null)
                } else if value.trim_start().starts_with('{') {
                    ValueRef::Other
                } else {
                    value_ref(value, literal_of[n])
                }
            }
            None => ValueRef::Other,
        };
        let place = self.text_range(at, text.len());
        self.access(segments, AccessMode::Write, place, node, Some(rhs));
    }

    /// The type a setter at element `index` sets a property of: the element
    /// its `TargetName` names in this body, else the `TargetType` of the
    /// nearest style or template around it.
    fn styled_type(&self, index: usize) -> Option<String> {
        let doc = self.doc;
        if let Some(target) = self.attr_text(index, "TargetName") {
            let named = (self.root..doc.elements.len())
                .take_while(|&i| doc.elements[i].start < doc.elements[self.root].end)
                .find(|&i| self.element_id(i) == Some(target));
            return named.map(|i| self.element_name(i));
        }
        for i in doc.ancestors(index) {
            let name = local_name(doc, i);
            if name == "Style" || (!name.contains('.') && name.ends_with("Template")) {
                return doc
                    .attr(&doc.elements[i], "TargetType")
                    .and_then(|a| type_of_value(doc, i, &a.value))
                    .map(|t| t.clr_name());
            }
        }
        None
    }

    /// `KeyBinding`: its gesture and the command it runs.
    fn key_gesture(&mut self, index: usize, node: u32) {
        let gesture = match self.attr_text(index, "Gesture").filter(|g| !g.is_empty()) {
            Some(g) => g.to_string(),
            None => match (
                self.attr_text(index, "Key"),
                self.attr_text(index, "Modifiers"),
            ) {
                (Some(key), Some(m)) if !m.is_empty() => format!("{m}+{key}"),
                (Some(key), _) if !key.is_empty() => key.to_string(),
                _ => return,
            },
        };
        let doc = self.doc;
        let command = doc.attr(&doc.elements[index], "Command").map(|a| {
            let raw = doc.text(&a.value);
            match markup(raw, a.value.start) {
                Some(ext) => match ext.positional().or_else(|| ext.named("Path")) {
                    Some(ArgValue::Text { text, .. }) => text.to_string(),
                    _ => raw.trim().to_string(),
                },
                None => raw.trim().to_string(),
            }
        });
        push_tag(
            &mut self.nodes[node as usize].tags,
            tag("key-gesture", Some(gesture), command.into_iter().collect()),
        );
    }

    /// The identity of a control with a command, and how it was found.
    fn control_key(&self, index: usize) -> Option<(String, &'static str)> {
        let doc = self.doc;
        if let Some(name) = self.element_id(index) {
            return Some((name.to_string(), "name"));
        }
        if let Some(key) = crate::xmlns::x_value(doc, index, "Key") {
            let key = doc.text(&key).trim();
            if !key.is_empty() {
                return Some((key.to_string(), "key"));
            }
        }
        const TEXTS: [&str; 3] = ["Content", "Header", "ToolTip"];
        for attr in TEXTS {
            let Some(a) = doc.attr(&doc.elements[index], attr) else {
                continue;
            };
            if let Some(ext) = markup(doc.text(&a.value), a.value.start) {
                if let Some((key, _)) = resource_ref(doc, index, &ext) {
                    return Some((key, "resource"));
                }
            }
        }
        for attr in TEXTS {
            let Some(a) = doc.attr(&doc.elements[index], attr) else {
                continue;
            };
            let raw = doc.text(&a.value);
            if !raw.trim().is_empty() && !raw.trim_start().starts_with('{') {
                return Some((unescaped(raw.trim()), "literal"));
            }
        }
        None
    }
}

#[cfg(test)]
mod tests;
