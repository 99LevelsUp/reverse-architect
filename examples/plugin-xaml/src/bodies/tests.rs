use super::*;
use crate::declare::declarations;
use crate::ra::plugin::types::{FileDeclarations, Op};

const GENERIC: &str = include_str!("../testdata/Generic.xaml");
const EDITOR: &str = include_str!("../testdata/EditorView.xaml");
const VIEW: &str = include_str!("../testdata/ViewE.xaml");

fn read(source: &str) -> FileBodies {
    bodies(source.as_bytes()).expect("valid XAML")
}

/// Line and column (1-based, in characters) of the first `needle`.
fn at(source: &str, needle: &str) -> Position {
    let offset = source.find(needle).expect("needle in the fixture");
    let before = &source[..offset];
    let line = before.matches('\n').count() + 1;
    let start = before.rfind('\n').map_or(0, |i| i + 1);
    let column = before[start..].chars().count() + 1;
    Position {
        line: line as u32,
        column: column as u32,
    }
}

/// The body whose declaration's name is written at the first `needle`.
fn body<'f>(file: &'f FileBodies, source: &str, needle: &str) -> &'f MemberBody {
    let position = at(source, needle);
    file.members
        .iter()
        .find(|m| m.name_position == position)
        .unwrap_or_else(|| panic!("no body at {position:?} ({needle})"))
}

/// The node of the element that starts at the first `needle` (`<TextBox`).
fn node<'b>(body: &'b MemberBody, source: &str, needle: &str) -> &'b BodyNode {
    let position = at(source, needle);
    body.nodes
        .iter()
        .find(|n| n.place.start == position)
        .unwrap_or_else(|| panic!("no node at {position:?} ({needle})"))
}

fn tags<'n>(node: &'n BodyNode, name: &str) -> Vec<(Option<&'n str>, Vec<&'n str>)> {
    node.tags
        .iter()
        .filter(|t| t.name == name)
        .map(|t| {
            (
                t.value.as_deref(),
                t.args.iter().map(String::as_str).collect(),
            )
        })
        .collect()
}

/// The accesses of one node: segments and mode.
fn accesses(body: &MemberBody, node: u32) -> Vec<(Vec<&str>, AccessMode)> {
    body.accesses
        .iter()
        .filter(|a| a.node == Some(node))
        .map(|a| {
            assert_eq!(a.target.root, PathRoot::Name);
            (
                a.target.segments.iter().map(String::as_str).collect(),
                a.mode,
            )
        })
        .collect()
}

fn names(body: &MemberBody) -> Vec<String> {
    body.nodes
        .iter()
        .map(|n| match &n.kind {
            NodeKind::Body => "body".to_string(),
            NodeKind::Element(e) => e.clone(),
            other => panic!("unexpected node kind {other:?}"),
        })
        .collect()
}

fn symbol_names(file: &FileDeclarations) -> Vec<(Option<&str>, Position)> {
    file.ops
        .iter()
        .filter_map(|op| match op {
            Op::Symbol(s) => {
                let name = s
                    .facts
                    .iter()
                    .find(|f| f.kind == "symbol_name")
                    .and_then(|f| f.place.name)?;
                Some((s.node_kind.as_deref(), name))
            }
            _ => None,
        })
        .collect()
}

#[test]
fn every_body_pairs_with_a_declaration_and_every_style_template_and_view_has_one() {
    let extra = "<ResourceDictionary xmlns:x=\"http://schemas.microsoft.com/winfx/2006/xaml\" xmlns:local=\"clr-namespace:Acme.Controls\">\n<Style TargetType=\"{x:Type local:Gauge}\"><Setter Property=\"Template\"><Setter.Value><ControlTemplate TargetType=\"local:Gauge\"><Border x:Name=\"PART_Track\"/></ControlTemplate></Setter.Value></Setter></Style>\n</ResourceDictionary>\n";
    for source in [GENERIC, EDITOR, VIEW, extra] {
        let declared = declarations("F.xaml", source.as_bytes(), &|_| false).expect("valid XAML");
        let declared = symbol_names(&declared);
        let file = read(source);
        for member in &file.members {
            assert_eq!(member.kind, BodyKind::Markup);
            assert_eq!(member.root, 0);
            let kind = declared
                .iter()
                .find(|(_, p)| *p == member.name_position)
                .map(|(k, _)| *k)
                .unwrap_or_else(|| panic!("body at {:?} pairs with nothing", member.name_position));
            assert!(
                matches!(kind, Some("xaml:style" | "xaml:template" | "xaml:view")),
                "{kind:?}"
            );
            // Ids equal indexes, and a parent comes before its child.
            for (i, n) in member.nodes.iter().enumerate() {
                assert_eq!(n.id as usize, i);
                assert_eq!(n.parent.is_none(), i == 0);
                assert!(n.parent.is_none_or(|p| (p as usize) < i));
            }
            for (i, l) in member.literals.iter().enumerate() {
                assert_eq!(l.id as usize, i);
            }
        }
        let wanted = declared
            .iter()
            .filter(|(k, _)| matches!(k, Some("xaml:style" | "xaml:template" | "xaml:view")))
            .count();
        assert_eq!(file.members.len(), wanted);
    }
}

#[test]
fn a_template_for_a_view_reads_its_binding_from_the_view() {
    let file = read(VIEW);
    let frame = body(&file, VIEW, "Frame\"");
    let text = node(frame, VIEW, "<TextBlock Text=\"{Binding P}\" />");
    assert_eq!(tags(text, "data-source"), [(Some("view"), vec![])]);
    assert_eq!(accesses(frame, text.id), [(vec!["P"], AccessMode::Read)]);
    // A template binding reads from the templated parent.
    let border = node(frame, VIEW, "<Border Background");
    assert_eq!(
        tags(border, "data-source"),
        [(Some("templated-parent"), vec![])]
    );
    assert_eq!(
        accesses(frame, border.id),
        [(vec!["Background"], AccessMode::Read)]
    );
}

#[test]
fn an_item_template_without_a_data_type_reads_from_the_item() {
    let file = read(VIEW);
    let frame = body(&file, VIEW, "Frame\"");
    let caption = node(frame, VIEW, "<TextBlock Text=\"{Binding Caption}\"");
    assert_eq!(tags(caption, "data-source"), [(Some("item"), vec![])]);
    assert_eq!(
        accesses(frame, caption.id),
        [(vec!["Caption"], AccessMode::Read)]
    );
    // The inline template is part of the body, not a body of its own.
    assert!(names(frame).contains(&"DataTemplate".to_string()));
    let list = node(frame, VIEW, "<ListBox ");
    assert_eq!(tags(list, "data-source"), [(Some("view"), vec![])]);
}

#[test]
fn a_template_carries_its_target_type_qualified_by_its_namespace() {
    let file = read(VIEW);
    let frame = body(&file, VIEW, "Frame\"");
    assert_eq!(
        frame.tags,
        [tag("target-type", Some("Acme.Views.ViewA".into()), vec![])]
    );
    assert_eq!(frame.kind, BodyKind::Markup);
    let root = &frame.nodes[0];
    assert_eq!(root.kind, NodeKind::Body);
    let first = at(VIEW, "<ControlTemplate");
    let last = at(VIEW, "</ControlTemplate>");
    assert_eq!(root.place.start, first);
    assert_eq!(
        root.place.end,
        Position {
            line: last.line,
            column: last.column + "</ControlTemplate>".len() as u32 - 1
        }
    );
    // A dictionary's template under a URI-mapped type keeps its plain name.
    let generic = read(GENERIC);
    let k = body(&generic, GENERIC, "K\" TargetType");
    assert_eq!(k.tags, [tag("target-type", Some("Button".into()), vec![])]);
}

#[test]
fn the_x_class_root_is_a_body_named_by_its_class_with_its_content() {
    let file = read(VIEW);
    let view = body(&file, VIEW, "Acme.Views.ViewE");
    assert_eq!(
        view.tags,
        [tag("target-type", Some("Acme.Views.ViewE".into()), vec![])]
    );
    let text = node(view, VIEW, "<TextBlock Text=\"{Binding P}\" ToolTip");
    assert_eq!(tags(text, "data-source"), [(Some("view"), vec![])]);
    assert_eq!(accesses(view, text.id), [(vec!["P"], AccessMode::Read)]);
    // Declared resources are bodies of their own (or none), never part of
    // the view's.
    let elements = names(view);
    assert!(!elements.iter().any(|n| n == "ControlTemplate"
        || n == "Style"
        || n == "SolidColorBrush"
        || n == "Setter"));
    assert!(elements.contains(&"UserControl.Resources".to_string()));
    assert_eq!(elements[0], "body");
    assert_eq!(file.members.len(), 4);
}

#[test]
fn a_data_template_with_a_data_type_reads_from_that_type() {
    let file = read(VIEW);
    let typed = body(&file, VIEW, "{x:Type m:ItemModel}");
    let text = node(typed, VIEW, "<TextBlock Text=\"{Binding Name}\"");
    assert_eq!(
        tags(text, "data-source"),
        [(Some("typed"), vec!["Acme.Models.ItemModel"])]
    );
    assert_eq!(accesses(typed, text.id), [(vec!["Name"], AccessMode::Read)]);
    // A data template has no target type.
    assert!(typed.tags.is_empty());
}

#[test]
fn a_key_binding_carries_its_gesture_and_command_and_is_no_control() {
    let file = read(VIEW);
    let view = body(&file, VIEW, "Acme.Views.ViewE");
    let save = node(view, VIEW, "<KeyBinding Gesture");
    assert_eq!(tags(save, "key-gesture"), [(Some("Ctrl+S"), vec!["Save"])]);
    assert_eq!(accesses(view, save.id), [(vec!["Save"], AccessMode::Read)]);
    assert!(tags(save, "trigger").is_empty());
    assert!(tags(save, "control-key").is_empty());
    let refresh = node(view, VIEW, "<KeyBinding Key");
    assert_eq!(
        tags(refresh, "key-gesture"),
        [(Some("Shift+F5"), vec!["ApplicationCommands.Refresh"])]
    );
    assert_eq!(
        accesses(view, refresh.id),
        [(vec!["ApplicationCommands", "Refresh"], AccessMode::Read)]
    );
    // The literal rule is syntactic: the gesture's text is a literal too.
    let texts: Vec<(&str, &LiteralContext)> = view
        .literals
        .iter()
        .filter(|l| l.node == Some(save.id) || l.node == Some(refresh.id))
        .map(|l| (l.value.as_str(), &l.context))
        .collect();
    let arg = |name: &str| LiteralContext::AttributeArg((name.into(), None, None));
    assert_eq!(
        texts,
        [
            ("Ctrl+S", &arg("Gesture")),
            ("Shift", &arg("Modifiers")),
            ("ApplicationCommands.Refresh", &arg("Command")),
        ]
    );
}

#[test]
fn an_explicit_two_way_binding_also_writes() {
    let file = read(VIEW);
    let view = body(&file, VIEW, "Acme.Views.ViewE");
    let two_way = node(view, VIEW, "<TextBox x:Name");
    assert_eq!(
        accesses(view, two_way.id),
        [
            (vec!["Name"], AccessMode::Read),
            (vec!["Name"], AccessMode::Write)
        ]
    );
    let one_way = node(view, VIEW, "<TextBox Text");
    assert_eq!(
        accesses(view, one_way.id),
        [(vec!["Name"], AccessMode::Read)]
    );
    // The same in a `MultiBinding` written as elements.
    let first = node(view, VIEW, "<Binding Path=\"First\"");
    assert_eq!(
        accesses(view, first.id),
        [(vec!["First"], AccessMode::Read)]
    );
    assert_eq!(tags(first, "data-source"), [(Some("view"), vec![])]);
    let last = node(view, VIEW, "<Binding Path=\"Last\"");
    assert_eq!(
        accesses(view, last.id),
        [
            (vec!["Last"], AccessMode::Read),
            (vec!["Last"], AccessMode::Write)
        ]
    );
    let place = view
        .accesses
        .iter()
        .find(|a| a.node == Some(two_way.id))
        .expect("an access")
        .place;
    let start = at(VIEW, "Name, Mode=TwoWay");
    assert_eq!(place.start, start);
    assert_eq!(
        place.end,
        Position {
            line: start.line,
            column: start.column + 3
        }
    );
}

#[test]
fn a_command_binding_reads_each_segment_and_marks_a_control_with_its_key() {
    let file = read(VIEW);
    let view = body(&file, VIEW, "Acme.Views.ViewE");
    let save = node(view, VIEW, "<Button x:Name=\"SaveButton\"");
    assert_eq!(
        accesses(view, save.id),
        [(vec!["Holder", "Command"], AccessMode::Read)]
    );
    assert_eq!(tags(save, "trigger"), [(Some("control"), vec![])]);
    assert_eq!(
        tags(save, "control-key"),
        [(Some("SaveButton"), vec!["name"])]
    );
    let close = node(view, VIEW, "<Button Command=\"{Binding Close}\"");
    assert_eq!(
        tags(close, "control-key"),
        [(Some("CloseText"), vec!["resource"])]
    );
    let literal = "<Grid xmlns:x=\"http://schemas.microsoft.com/winfx/2006/xaml\" x:Class=\"Acme.V\"><MenuItem Command=\"{Binding Open}\" Header=\"Otevřít &amp; číst\"/></Grid>";
    let file = read(literal);
    let item = &file.members[0].nodes[1];
    assert_eq!(
        tags(item, "control-key"),
        [(Some("Otevřít & číst"), vec!["literal"])]
    );
}

#[test]
fn a_static_member_an_attached_property_and_a_setter_are_read_written_and_written() {
    let file = read(VIEW);
    let view = body(&file, VIEW, "Acme.Views.ViewE");
    let text = node(view, VIEW, "<TextBlock Text=\"{x:Static");
    let found: Vec<(Vec<&str>, AccessMode, Option<&ValueRef>)> = view
        .accesses
        .iter()
        .filter(|a| a.node == Some(text.id))
        .map(|a| {
            (
                a.target.segments.iter().map(String::as_str).collect(),
                a.mode,
                a.rhs.as_ref(),
            )
        })
        .collect();
    assert_eq!(
        found,
        [
            (vec!["Acme.Settings", "Default"], AccessMode::Read, None),
            (
                vec!["Acme.Behaviors.Focus", "IsInitial"],
                AccessMode::Write,
                Some(&ValueRef::Constant(Constant::Boolean(true)))
            ),
        ]
    );
    let busy = body(&file, VIEW, "Busy\"");
    let setter = node(busy, VIEW, "<Setter Property=\"IsBusy\"");
    let access = busy
        .accesses
        .iter()
        .find(|a| a.node == Some(setter.id))
        .expect("the setter's write");
    assert_eq!(access.target.segments, ["Acme.Views.ViewA", "IsBusy"]);
    assert_eq!(access.mode, AccessMode::Write);
    assert_eq!(
        access.rhs,
        Some(ValueRef::Constant(Constant::Boolean(true)))
    );
    assert_eq!(access.place.start, at(VIEW, "IsBusy\""));
    assert_eq!(
        busy.tags,
        [tag("target-type", Some("Acme.Views.ViewA".into()), vec![])]
    );
}

#[test]
fn a_setter_by_target_name_writes_the_named_element_and_an_owned_property_its_owner() {
    let source = "<ResourceDictionary xmlns:x=\"http://schemas.microsoft.com/winfx/2006/xaml\" xmlns:local=\"clr-namespace:Acme.Controls\">\n<ControlTemplate x:Key=\"T\" TargetType=\"local:Gauge\"><local:Dial x:Name=\"PART_Dial\"/><ControlTemplate.Triggers><Trigger Property=\"IsEnabled\" Value=\"False\"><Setter TargetName=\"PART_Dial\" Property=\"Opacity\" Value=\"0.5\"/><Setter Property=\"local:Gauge.Mode\" Value=\"Compact\"/><Setter Property=\"Tag\"><Setter.Value>x</Setter.Value></Setter></Trigger></ControlTemplate.Triggers></ControlTemplate>\n</ResourceDictionary>\n";
    let file = read(source);
    let t = &file.members[0];
    let writes: Vec<(Vec<&str>, Option<&ValueRef>)> = t
        .accesses
        .iter()
        .map(|a| {
            assert_eq!(a.mode, AccessMode::Write);
            (
                a.target.segments.iter().map(String::as_str).collect(),
                a.rhs.as_ref(),
            )
        })
        .collect();
    let compact = t
        .literals
        .iter()
        .find(|l| l.value == "Compact")
        .expect("the value is a literal");
    assert_eq!(
        writes,
        [
            (
                vec!["Acme.Controls.Dial", "Opacity"],
                Some(&ValueRef::Constant(Constant::Number("0.5".into())))
            ),
            (
                vec!["Acme.Controls.Gauge", "Mode"],
                Some(&ValueRef::Literal(compact.id))
            ),
            (vec!["Acme.Controls.Gauge", "Tag"], Some(&ValueRef::Other)),
        ]
    );
}

#[test]
fn a_static_resource_is_no_access_path_data_and_colours_are_no_literals_but_a_tooltip_is() {
    let file = read(VIEW);
    let view = body(&file, VIEW, "Acme.Views.ViewE");
    let path = node(view, VIEW, "<Path Fill");
    assert!(view.literals.iter().all(|l| l.node != Some(path.id)));
    let text = node(view, VIEW, "<TextBlock Text=\"{Binding P}\" ToolTip");
    let literals: Vec<(&str, &LiteralContext)> = view
        .literals
        .iter()
        .filter(|l| l.node == Some(text.id))
        .map(|l| (l.value.as_str(), &l.context))
        .collect();
    assert_eq!(
        literals,
        [(
            "Uložit dokument",
            &LiteralContext::AttributeArg(("ToolTip".into(), None, None))
        )]
    );
    let literal = view
        .literals
        .iter()
        .find(|l| l.node == Some(text.id))
        .expect("the tooltip");
    let start = at(VIEW, "Uložit dokument");
    assert_eq!(literal.place.start, start);
    assert_eq!(
        literal.place.end,
        Position {
            line: start.line,
            column: start.column + "Uložit dokument".chars().count() as u32 - 1
        }
    );
    // A handler name and an element's name are not literals; a content text is.
    let values: Vec<&str> = view.literals.iter().map(|l| l.value.as_str()).collect();
    assert!(!values
        .iter()
        .any(|v| *v == "OnSave" || *v == "OnLoaded" || *v == "SaveButton" || *v == "TitleBox"));
    assert!(values.contains(&"Uložit"));
    let close = node(view, VIEW, "<Button Command=\"{Binding Close}\"");
    assert_eq!(
        accesses(view, close.id),
        [(vec!["Close"], AccessMode::Read)]
    );
}

#[test]
fn the_literal_rule_keeps_words_and_drops_numbers_colours_and_geometry() {
    for word in ["Uložit dokument", "Save", "Auto", "Ctrl+S"] {
        assert!(is_literal(word), "{word}");
    }
    for not in [
        "{Binding A}",
        "12",
        "2,0,2,0",
        "#FF2050A0",
        "#FFF",
        "sc#1,0.5,0.5,0.5",
        "M0,0 L10,10 Z",
        "F1 M 0,0 L 10,10 Z M5,5 H1 V2 ZM1,1",
        "a",
        "",
    ] {
        assert!(!is_literal(not), "{not}");
    }
}

#[test]
fn event_handlers_and_event_setters_are_subscriptions() {
    let file = read(VIEW);
    let view = body(&file, VIEW, "Acme.Views.ViewE");
    type Seen<'s> = (Option<(PathRoot, Vec<&'s str>)>, &'s str, &'s Handler);
    let found: Vec<Seen> = view
        .subscriptions
        .iter()
        .map(|s| {
            assert_eq!(s.mode, SubscriptionMode::Add);
            (
                s.source
                    .as_ref()
                    .map(|p| (p.root, p.segments.iter().map(String::as_str).collect())),
                s.event.as_str(),
                &s.handler,
            )
        })
        .collect();
    assert_eq!(
        found,
        [
            (
                None,
                "Loaded",
                &Handler::MethodGroup(name_path(vec!["OnLoaded".into()]))
            ),
            (
                Some((PathRoot::Name, vec!["SaveButton"])),
                "Click",
                &Handler::MethodGroup(name_path(vec!["OnSave".into()]))
            ),
        ]
    );
    let root = &view.nodes[0];
    assert_eq!(view.subscriptions[0].node, Some(root.id));
    assert_eq!(view.subscriptions[1].place.start, at(VIEW, "OnSave"));
    let busy = body(&file, VIEW, "Busy\"");
    assert_eq!(busy.subscriptions.len(), 1);
    let setter = &busy.subscriptions[0];
    assert_eq!(setter.event, "Click");
    assert_eq!(
        setter.handler,
        Handler::MethodGroup(name_path(vec!["OnItemClick".into()]))
    );
    assert_eq!(
        setter.source,
        Some(AccessPath {
            root: PathRoot::Expression,
            segments: Vec::new()
        })
    );
    // Without `x:Class` no attribute is taken for a handler.
    let dictionary = read("<ResourceDictionary xmlns:x=\"http://schemas.microsoft.com/winfx/2006/xaml\"><Style x:Key=\"S\"><Setter Property=\"Tag\" Value=\"OnSave\"/></Style><ControlTemplate x:Key=\"T\"><Button Click=\"OnSave\"/></ControlTemplate></ResourceDictionary>");
    assert!(dictionary
        .members
        .iter()
        .all(|m| m.subscriptions.is_empty()));
    // A property whose name ends like an event is not one.
    let flags = read("<Grid xmlns:x=\"http://schemas.microsoft.com/winfx/2006/xaml\" x:Class=\"Acme.V\"><CheckBox IsChecked=\"Checked\" AllowDrop=\"True\" Visibility=\"Collapsed\" SelectionChanged=\"Selection_Changed\" AutomationProperties.AutomationId=\"Btn_Save\"/></Grid>");
    let events: Vec<&str> = flags.members[0]
        .subscriptions
        .iter()
        .map(|s| s.event.as_str())
        .collect();
    assert_eq!(events, ["SelectionChanged"]);
    // An attached property whose value is named like a handler stays a write.
    let writes: Vec<&[String]> = flags.members[0]
        .accesses
        .iter()
        .map(|a| a.target.segments.as_slice())
        .collect();
    assert_eq!(
        writes,
        [[
            "AutomationProperties".to_string(),
            "AutomationId".to_string()
        ]]
    );
}

#[test]
fn a_binding_names_its_own_source() {
    let file = read(VIEW);
    let view = body(&file, VIEW, "Acme.Views.ViewE");
    let by_element = node(view, VIEW, "<TextBlock Text=\"{Binding Text, ElementName");
    assert_eq!(
        tags(by_element, "data-source"),
        [(Some("element"), vec!["TitleBox"])]
    );
    let by_self = node(view, VIEW, "<TextBlock Text=\"{Binding Tag, RelativeSource");
    assert_eq!(tags(by_self, "data-source"), [(Some("self"), vec![])]);
    let source = "<Grid xmlns:x=\"http://schemas.microsoft.com/winfx/2006/xaml\" xmlns:v=\"clr-namespace:Acme.Vm\" x:Class=\"Acme.V\">\n<TextBlock Text=\"{Binding Title, Source={x:Static v:Shell.Current}}\" Tag=\"{Binding Name, RelativeSource={RelativeSource AncestorType={x:Type v:Host}}}\"/>\n<StackPanel DataContext=\"{Binding Detail}\" Tag=\"{Binding Id}\"><TextBlock Text=\"{Binding Note}\"/></StackPanel>\n</Grid>";
    let file = read(source);
    let view = &file.members[0];
    let text = &view.nodes[1];
    assert_eq!(
        tags(text, "data-source"),
        [
            (Some("explicit"), vec!["Acme.Vm.Shell", "Current"]),
            (Some("explicit"), vec!["Acme.Vm.Host"]),
        ]
    );
    assert_eq!(
        accesses(view, text.id),
        [
            (vec!["Title"], AccessMode::Read),
            (vec!["Acme.Vm.Shell", "Current"], AccessMode::Read),
            (vec!["Name"], AccessMode::Read),
        ]
    );
    // `DataContext` reads from the context around it; the element's other
    // bindings and those under it from what it set.
    let panel = &view.nodes[2];
    assert_eq!(
        tags(panel, "data-source"),
        [(Some("view"), vec![]), (Some("explicit"), vec![])]
    );
    let note = &view.nodes[3];
    assert_eq!(tags(note, "data-source"), [(Some("explicit"), vec![])]);
}

#[test]
fn a_binding_path_is_split_into_segments() {
    let doc = parse(b"<a xmlns:v=\"clr-namespace:Acme.Views\"/>").expect("valid XML");
    let cases: [(&str, &[&str]); 6] = [
        ("A.B.C", &["A", "B", "C"]),
        ("Items[0].Name", &["Items", "Name"]),
        (
            "(Validation.Errors)[0].ErrorContent",
            &["Validation", "Errors", "ErrorContent"],
        ),
        ("(v:Focus.IsInitial)", &["Acme.Views.Focus", "IsInitial"]),
        ("Items/Name", &["Items", "Name"]),
        (".", &[]),
    ];
    for (path, segments) in cases {
        assert_eq!(path_segments(&doc, 0, path), segments, "{path}");
    }
}

#[test]
fn a_binding_without_a_path_reads_nothing() {
    let file = read("<Grid xmlns:x=\"http://schemas.microsoft.com/winfx/2006/xaml\" x:Class=\"Acme.V\"><ContentControl Content=\"{Binding}\" Tag=\"{Binding .}\"/></Grid>");
    let view = &file.members[0];
    assert!(view.accesses.is_empty());
    assert_eq!(
        tags(&view.nodes[1], "data-source"),
        [(Some("view"), vec![])]
    );
}

#[test]
fn a_dictionary_has_a_body_per_style_and_template_and_none_for_other_resources() {
    let file = read(GENERIC);
    assert_eq!(file.members.len(), 5);
    let data = body(&file, GENERIC, "{x:Type local:EditorViewModel}");
    let text = node(data, GENERIC, "<TextBlock Text=\"{Binding Title}\"");
    assert_eq!(
        tags(text, "data-source"),
        [(Some("typed"), vec!["Acme.Views.EditorViewModel"])]
    );
    assert_eq!(
        accesses(data, text.id),
        [
            (vec!["Title"], AccessMode::Read),
            (vec!["Acme.Views.Palette", "Ink"], AccessMode::Read),
        ]
    );
    assert!(file.types.is_empty() && file.typed_members.is_empty() && file.metas.is_empty());
}

/// R19, U8: a value written as a markup extension the parser cannot take
/// apart is no access and no literal, and its element says so with
/// `unparsed-markup`, the attribute's name as the value; the `{}` escape and
/// a well-formed extension beside it carry none.
#[test]
fn an_extension_that_cannot_be_parsed_is_tagged_on_its_element() {
    let source = "<Grid xmlns:x=\"http://schemas.microsoft.com/winfx/2006/xaml\" x:Class=\"Acme.V\">\n<TextBlock Text=\"{Binding Title\" Tag=\"{Binding Note}\"/>\n<TextBlock Text=\"{}{Plain}\" ToolTip=\"{ }\"/>\n</Grid>";
    let file = read(source);
    let view = &file.members[0];
    let broken = &view.nodes[1];
    assert_eq!(tags(broken, UNPARSED_MARKUP), [(Some("Text"), vec![])]);
    assert_eq!(
        accesses(view, broken.id),
        [(vec!["Note"], AccessMode::Read)]
    );
    assert!(view.literals.iter().all(|l| !l.value.contains("Title")));
    let other = &view.nodes[2];
    assert_eq!(tags(other, UNPARSED_MARKUP), [(Some("ToolTip"), vec![])]);
    assert!(EMITS.contains(&UNPARSED_MARKUP));
}

/// Review of 2c: a resource that is neither a style nor a template has no
/// body — unless it holds an extension the parser cannot take apart, which
/// would otherwise go unsaid. Then it is a body paired with its declaration,
/// its nodes carrying that tag and nothing else; one inside a style's own
/// resources too, apart from the style's body.
#[test]
fn a_resource_with_an_extension_that_cannot_be_parsed_is_a_body_of_that_tag_alone() {
    let source = "<ResourceDictionary xmlns:x=\"http://schemas.microsoft.com/winfx/2006/xaml\">\n\
        <SolidColorBrush x:Key=\"Accent\" Color=\"{DynamicResource\" Opacity=\"{Binding Level}\"/>\n\
        <SolidColorBrush x:Key=\"Ink\" Color=\"{DynamicResource Base}\"/>\n\
        <ContextMenu x:Key=\"Menu\">\n<MenuItem Header=\"Open\" Command=\"{Binding Open\"/>\n</ContextMenu>\n\
        <Style x:Key=\"Plain\" TargetType=\"Button\">\n<Style.Resources>\n\
        <SolidColorBrush x:Key=\"Inner\" Color=\"{StaticResource Base\"/>\n</Style.Resources>\n\
        <Setter Property=\"Background\" Value=\"Red\"/>\n</Style>\n\
        </ResourceDictionary>\n";
    let declared = declarations("F.xaml", source.as_bytes(), &|_| false).expect("valid XAML");
    let declared = symbol_names(&declared);
    let file = read(source);
    let kind_of = |member: &MemberBody| {
        declared
            .iter()
            .find(|(_, p)| *p == member.name_position)
            .and_then(|(k, _)| *k)
    };
    // `Accent`, `Menu`, the style and `Inner`; `Ink` reads in full.
    let kinds: Vec<_> = file.members.iter().map(kind_of).collect();
    assert_eq!(
        kinds,
        [
            Some("xaml:resource"),
            Some("xaml:resource"),
            Some("xaml:style"),
            Some("xaml:resource")
        ]
    );
    let accent = body(&file, source, "Accent");
    assert_eq!(names(accent), ["body"]);
    assert_eq!(
        tags(&accent.nodes[0], UNPARSED_MARKUP),
        [(Some("Color"), vec![])]
    );
    assert!(accent.nodes[0].tags.len() == 1 && accent.tags.is_empty());
    assert!(accent.accesses.is_empty() && accent.literals.is_empty());
    assert!(accent.subscriptions.is_empty());
    let menu = body(&file, source, "Menu\"");
    assert_eq!(names(menu), ["body", "MenuItem"]);
    assert!(menu.nodes[0].tags.is_empty());
    assert_eq!(
        tags(&menu.nodes[1], UNPARSED_MARKUP),
        [(Some("Command"), vec![])]
    );
    assert!(menu.nodes[1].tags.len() == 1 && menu.literals.is_empty());
    let inner = body(&file, source, "Inner");
    assert_eq!(
        tags(&inner.nodes[0], UNPARSED_MARKUP),
        [(Some("Color"), vec![])]
    );
    // The style's body holds neither the inner resource nor its tag.
    let style = body(&file, source, "Plain");
    assert!(style
        .nodes
        .iter()
        .all(|n| n.tags.iter().all(|t| t.name != UNPARSED_MARKUP)));
    assert_eq!(read(source), read(source));
}

#[test]
fn the_same_file_gives_the_same_bodies() {
    assert_eq!(read(VIEW), read(VIEW));
    assert_eq!(read(GENERIC), read(GENERIC));
}

#[test]
fn invalid_xml_is_an_error() {
    assert!(bodies(b"<Grid><Button></Grid>").is_err());
    assert!(bodies(b"\xFF<a/>").is_err());
}
