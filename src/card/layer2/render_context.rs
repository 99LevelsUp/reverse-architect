use super::{anchors, dead_what, last_name, path_state_phrase, phrase, short, unresolved, Cx};
use crate::card::explain::{code, cs_n, plural, series};
use crate::card::layer2::{
    flow::{constant_text, written},
    CommonPart, ContextPrecondition, FlowResult, FlowStep, FlowStepKind, FunctionalityContext,
    Narrowed, PathState, Place, ResultKind, Source, StateContext, StopCause, Trigger, TriggerFlow,
    TriggerKind,
};
use crate::card::SourceRef;
use crate::extract::bodies::PathRoot;
use crate::extract::external::{redact_secret_block, SecretPrefixes};
use crate::graph::NodeKey;
use crate::render::mermaid::{sequence_block, SPLIT_THRESHOLD};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

// ---- The context (plan 2b, U6: R1, R2, R10, R16, R18; KTD8) ----

/// A text of the code a trigger carries — its key, its condition — through
/// the filter the first layer gives every text, once more: the host redacted
/// the model before anything read it, so this is normally a no-op, and it
/// stays because a trigger's text is the one the card quotes whole. Its
/// marker is admitted by [`Cx::left_out`] like any other.
fn scrubbed(text: &str) -> String {
    redact_secret_block(text, SecretPrefixes::builtin()).0
}

/// A text for one line: its whitespace collapsed.
fn flat(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The key of the type that declares the member `key`: the key cut at the
/// last `.` of its name, before the parameters.
fn owner_key(key: &str) -> &str {
    let head = key.split('(').next().unwrap_or(key);
    let start = head.rfind('#').map_or(0, |i| i + 1);
    match head[start..].rfind('.') {
        Some(i) => &key[..start + i],
        None => key,
    }
}

/// The short name of the type that declares the member `key`.
fn owner_name(key: &str) -> &str {
    short(owner_key(key))
}

/// The heading word of a kind of trigger (an English constant, R13).
fn kind_heading(kind: TriggerKind) -> &'static str {
    match kind {
        TriggerKind::Event => "Event",
        TriggerKind::Key => "Key",
        TriggerKind::MenuItem => "Menu item",
        TriggerKind::External => "Entry from another component",
    }
}

/// `<kind>: <key>`, and the type the trigger is written in after ` — `: the
/// heading of a trigger's flow (KTD8).
fn flow_heading(t: &Trigger) -> String {
    let key = flat(&scrubbed(short(&t.key)));
    match t.kind {
        TriggerKind::External => format!("{}: {key}", kind_heading(t.kind)),
        _ => format!(
            "{}: {key} — {}",
            kind_heading(t.kind),
            owner_name(t.member.as_str())
        ),
    }
}

/// ` — `anchor``, or nothing for no place.
fn anchor_of(place: Option<&Place>) -> String {
    anchors(place)
}

/// The `### Context` of a functionality (KTD8): the functionalities its own
/// triggers' flows are drawn at (R9), the command class's one flow (AE9),
/// and a flow under `#### <kind>: <key>` for every trigger that leads to it.
/// The part a flow shares with an earlier one is written once, under the
/// first, and referred to (R1). Level 1 has the triggers, their state, the
/// preconditions and the results; levels 2 and 3 add the steps and a
/// sequence diagram (R16).
pub(super) fn functionality_context(cx: &Cx<'_>, s: &mut String, c: &FunctionalityContext) {
    if c.flows.is_empty()
        && c.command_class.is_none()
        && c.refers.is_empty()
        && c.narrowed.is_none()
    {
        return;
    }
    s.push_str("\n### Context\n\n");
    for r in &c.refers {
        let _ = writeln!(
            s,
            "- {} {} {} {}.",
            cx.t(
                "The flows of its triggers are drawn at the functionality",
                "Toky jejích spouštěčů jsou vykreslené u funkčnosti"
            ),
            code(short(&r.key)),
            cx.t("of", "typu"),
            cx.card_of(r.ty.as_str())
        );
    }
    if let Some(n) = &c.narrowed {
        let _ = writeln!(s, "{}", narrowed_line(cx, n));
    }
    let block = |s: &mut String, heading: &str| {
        if !s.ends_with("\n\n") {
            s.push('\n');
        }
        let _ = write!(s, "#### {heading}\n\n");
    };
    if let Some(class) = &c.command_class {
        block(s, "Execute → target");
        let n = class.triggers;
        let count = if cx.cs() {
            format!(
                "Vede na ni {}: jeden tok pro každý příkaz sestavený z této třídy; každý spouštěč \
                 uvádí rejstřík spouštěčů ({}).",
                cs_n(n, ("spouštěč", "spouštěče", "spouštěčů")),
                cx.registry()
            )
        } else {
            format!(
                "{} {} to it: one flow for every command built of this class; the registry of \
                 triggers ({}) lists every one.",
                plural(n, "trigger", "triggers"),
                if n == 1 { "leads" } else { "lead" },
                cx.registry()
            )
        };
        let _ = writeln!(s, "- {count}");
        if let Some(common) = c.common.get(class.common) {
            common_lines(cx, s, common);
            if cx.level >= 2 {
                if let Some(d) = sequence(cx, &[], common) {
                    let _ = write!(s, "\n{d}");
                }
            }
        }
    }
    // The heading of the flow each shared part was written under.
    let mut written_at: BTreeMap<usize, String> = BTreeMap::new();
    for flow in &c.flows {
        let heading = flow_heading(&flow.trigger);
        block(s, &heading);
        let _ = writeln!(s, "{}", trigger_line(cx, flow));
        if let Some(line) = path_line(cx, flow) {
            let _ = writeln!(s, "{line}");
        }
        let Some(common) = c.common.get(flow.common) else {
            continue;
        };
        if cx.level >= 2 {
            for (i, hop) in flow.path.iter().enumerate() {
                let next: Vec<&NodeKey> = match flow.path.get(i + 1) {
                    Some(h) => vec![&h.member],
                    None => common.entries.iter().collect(),
                };
                let _ = writeln!(s, "{}", hop_line(cx, hop, &next));
            }
        }
        match written_at.get(&flow.common) {
            Some(first) => {
                let entries: Vec<String> = common
                    .entries
                    .iter()
                    .map(|k| code(cx.name(k.as_str())))
                    .collect();
                let _ = if cx.cs() {
                    writeln!(
                        s,
                        "- Od {} dál: jako u „{first}“ výše.",
                        cx.series(&entries)
                    )
                } else {
                    writeln!(
                        s,
                        "- From {} on: as under “{first}” above.",
                        cx.series(&entries)
                    )
                };
            }
            None => {
                common_lines(cx, s, common);
                written_at.insert(flow.common, heading.clone());
            }
        }
        if cx.level >= 2 {
            if let Some(d) = sequence(cx, &flow.path, common) {
                let _ = write!(s, "\n{d}");
            }
        }
    }
}

/// Where the walk back from a functionality stopped when the run kept only
/// the nearest functionality (plan 2b, risk table of KTD5): the card claims
/// no trigger reached past those functionalities.
fn narrowed_line(cx: &Cx<'_>, n: &Narrowed) -> String {
    let at: Vec<String> =
        n.at.iter()
            .map(|r| {
                format!(
                    "{} {} {}",
                    code(short(&r.key)),
                    cx.t("of", "typu"),
                    cx.card_of(r.ty.as_str())
                )
            })
            .collect();
    let at = series(cx.lang, &at, n.total);
    if cx.cs() {
        format!(
            "- V tomto běhu se zpětný průchod zastavuje u funkčnosti nejbližší spouštěči ({}): \
             spouštěč, jehož cesta sem vede přes člena funkčnosti {at} — funkčnosti jiného typu \
             mimo typ spouštěče —, vede jen tam a jeho tok je vykreslený tam, ne tady.",
            cx.registry()
        )
    } else {
        format!(
            "- In this run the walk back stops at the functionality nearest the trigger ({}): a \
             trigger whose way here passes through a member of the functionality {at} — another \
             type's, outside the trigger's own — leads only there, and its flow is drawn there, \
             not here.",
            cx.registry()
        )
    }
}

/// The path of `cards/triggers.md`, the registry of triggers (U7), which the
/// card links — every run with the second layer writes it.
const REGISTRY: &str = crate::card::triggers::REGISTRY_PATH;

impl Cx<'_> {
    /// The link to the registry of triggers from the card, named by its path.
    fn registry(&self) -> String {
        crate::card::links::link(&self.card.path, REGISTRY, REGISTRY, self.links)
    }
}

/// The trigger of a flow, with its condition and where it is wired.
fn trigger_line(cx: &Cx<'_>, flow: &TriggerFlow) -> String {
    let t = &flow.trigger;
    let member = code(cx.name(t.member.as_str()));
    let key = code(&scrubbed(&t.key));
    let mut line = match t.kind {
        TriggerKind::Key => format!(
            "{} {key} {} {member}",
            cx.t("key", "klávesa"),
            cx.t("in", "v")
        ),
        TriggerKind::MenuItem => format!(
            "{} {key} {} {member}",
            cx.t("menu item", "položka menu"),
            cx.t("built in", "sestavená v")
        ),
        TriggerKind::Event => {
            let event = match &t.source {
                Some(src) if !src.segments.is_empty() || src.root != PathRoot::This => {
                    format!("{}.{}", written(src), t.key)
                }
                _ => t.key.clone(),
            };
            format!(
                "{} {} {} {member}",
                cx.t("event", "událost"),
                code(&scrubbed(&event)),
                cx.t("subscribed in", "přihlášená v")
            )
        }
        TriggerKind::External => {
            let mut l = format!(
                "{} {member}",
                cx.t("entry from another component", "vstup z jiné komponenty")
            );
            if flow.state == PathState::Active {
                l.push_str(cx.t(
                    ", which nothing in the corpus calls",
                    ", který nic v korpusu nevolá",
                ));
            }
            l
        }
    };
    if let Some(c) = &flow.condition {
        let _ = write!(
            line,
            ", {} {}",
            cx.t("when", "když"),
            code(&scrubbed(&c.text))
        );
    }
    let note = cx.left_out(&line);
    let place = t
        .place
        .as_ref()
        .or_else(|| flow.condition.as_ref().and_then(|c| c.place.as_ref()));
    format!(
        "- {} {line}{note}{}",
        cx.t("Trigger:", "Spouštěč:"),
        anchor_of(place)
    )
}

/// The state of a flow's path that is not active (R12): dead, with the place
/// that turns it off, or unresolved, with the weak callers.
fn path_line(cx: &Cx<'_>, flow: &TriggerFlow) -> Option<String> {
    let lead = cx.t("Path:", "Cesta:");
    let phrase = path_state_phrase(cx.lang, &flow.state)?;
    match &flow.state {
        PathState::Active => None,
        PathState::Dead { member, place } => Some(format!(
            "- {lead} {phrase} {}; {}{}",
            code(cx.name(member.as_str())),
            cx.t("nothing happens", "nic se nestane"),
            anchor_of(Some(place))
        )),
        PathState::Unresolved { callers } => {
            let names: Vec<String> = callers.iter().map(|k| code(cx.name(k.as_str()))).collect();
            Some(format!(
                "- {lead} {phrase} {}{}",
                cx.series(&names),
                anchor_of(flow.trigger.place.as_ref())
            ))
        }
    }
}

/// A member on the trigger's way to the functionality, and what it calls on
/// (R6: a type the flow only passes is one step, with its card).
fn hop_line(cx: &Cx<'_>, hop: &FlowStep, next: &[&NodeKey]) -> String {
    let names: Vec<String> = next.iter().map(|k| code(cx.name(k.as_str()))).collect();
    let mut line = format!(
        "{} → {}",
        code(cx.name(hop.member.as_str())),
        cx.series(&names)
    );
    let owner = owner_key(hop.member.as_str());
    if short(owner) != cx.ty {
        let _ = write!(line, " ({} {})", cx.t("card", "karta"), cx.card_of(owner));
    }
    format!(
        "- {} {line}{}",
        cx.t("Step:", "Krok:"),
        anchor_of(hop.place.as_ref())
    )
}

/// The lines of a shared part: its preconditions with what still happens
/// when each stops the rest, its steps from level 2, its results, and the
/// paths on which nothing happens that no precondition stands for.
fn common_lines(cx: &Cx<'_>, s: &mut String, c: &CommonPart) {
    let stop_at = |member: &NodeKey, place: &Place| {
        c.stops
            .iter()
            .find(|st| &st.member == member && &st.place == place)
    };
    for p in &c.preconditions {
        let still = stop_at(&p.occurrence.member, &p.occurrence.place)
            .map(|st| st.results.as_slice())
            .unwrap_or_default();
        let _ = writeln!(s, "{}", precondition_context_line(cx, p, still));
    }
    if cx.level >= 2 {
        for st in &c.steps {
            let _ = writeln!(s, "{}", step_context_line(cx, st));
        }
    }
    for r in &c.results {
        let text = format!(
            "{} {}",
            code(cx.name(r.member.as_str())),
            result_phrase(cx, r)
        );
        let _ = writeln!(
            s,
            "- {} {text}{}{}",
            cx.t("Result:", "Výsledek:"),
            cx.left_out(&text),
            result_anchor(r)
        );
    }
    for st in &c.stops {
        let text = match &st.cause {
            StopCause::Dead(kind) => format!(
                "{}: {} {}",
                cx.t("nothing happens", "nic se nestane"),
                code(cx.name(st.member.as_str())),
                dead_what(cx, kind)
            ),
            StopCause::Precondition { text, .. } => {
                let told = c
                    .preconditions
                    .iter()
                    .any(|p| p.occurrence.member == st.member && p.occurrence.place == st.place);
                if told {
                    continue;
                }
                let mut t = format!(
                    "{} {}",
                    cx.t("nothing happens when", "nic se nestane, když"),
                    code(&scrubbed(text))
                );
                t.push_str(&still_happens(cx, &st.results));
                t
            }
        };
        let _ = writeln!(
            s,
            "- {} {text}{}{}",
            cx.t("Result:", "Výsledek:"),
            cx.left_out(&text),
            anchor_of(Some(&st.place))
        );
    }
}

/// `; still happens: …` — the results on a path that stops (R5, 5).
fn still_happens(cx: &Cx<'_>, results: &[FlowResult]) -> String {
    if results.is_empty() {
        return String::new();
    }
    let list: Vec<String> = results
        .iter()
        .map(|r| {
            format!(
                "{} {}",
                code(cx.name(r.member.as_str())),
                result_phrase(cx, r)
            )
        })
        .collect();
    format!(
        "; {} {}",
        cx.t("still happens:", "i tak proběhne:"),
        list.join("; ")
    )
}

fn precondition_context_line(cx: &Cx<'_>, p: &ContextPrecondition, still: &[FlowResult]) -> String {
    let o = &p.occurrence;
    let at = match o.source {
        Source::Guard => cx.t("a guard in", "stráž v"),
        Source::CanExecute => cx.t("the CanExecute predicate in", "podmínka CanExecute v"),
    };
    let mut text = format!(
        "{} {} ({at} {})",
        cx.t("does not run when", "nespustí se, když"),
        code(&scrubbed(&p.text)),
        code(cx.name(o.member.as_str()))
    );
    text.push_str(&still_happens(cx, still));
    format!(
        "- {} {text}{}{}",
        cx.t("Precondition:", "Předpoklad:"),
        cx.left_out(&text),
        anchor_of(Some(&o.place))
    )
}

/// Where a result is: its place, or a database object's lines.
fn result_anchor(r: &FlowResult) -> String {
    match (&r.place, &r.kind) {
        (Some(p), _) => anchor_of(Some(p)),
        (None, ResultKind::Database { file, span, .. }) => format!(
            " — `{}`",
            SourceRef {
                file: file.clone(),
                span: *span,
            }
            .anchor()
        ),
        (None, _) => String::new(),
    }
}

/// What a result is, in a phrase after its member (R5, R6).
fn result_phrase(cx: &Cx<'_>, r: &FlowResult) -> String {
    let value = |v: &Option<crate::extract::bodies::Constant>| {
        v.as_ref()
            .map_or_else(String::new, |c| format!(" = {}", code(&constant_text(c))))
    };
    match &r.kind {
        ResultKind::NotifiesChange { target, value: v } => format!(
            "{} {}{}{}",
            cx.t("writes", "zapíše"),
            code(cx.name(target.as_str())),
            value(v),
            cx.t(", which notifies its change", ", vlastnost hlásí změnu")
        ),
        ResultKind::RaisesEvent { event, subscribers } => {
            let n = *subscribers;
            if cx.cs() {
                format!(
                    "vyvolá událost {}; v korpusu {} k ní, uvádí je rejstřík spouštěčů ({})",
                    code(cx.name(event.as_str())),
                    cs_n(n, ("přihlášení", "přihlášení", "přihlášení")),
                    cx.registry()
                )
            } else {
                format!(
                    "raises the event {}; {} to it in the corpus, listed in the registry of \
                     triggers ({})",
                    code(cx.name(event.as_str())),
                    plural(n, "subscription", "subscriptions"),
                    cx.registry()
                )
            }
        }
        ResultKind::ReadElsewhere {
            target,
            value: v,
            mutation,
            readers,
        } => {
            let target = code(cx.name(target.as_str()));
            let mut t = if *mutation {
                format!(
                    "{} {target} {}",
                    cx.t("changes", "změní"),
                    cx.t("through a call on it", "voláním na něm")
                )
            } else {
                format!("{} {target}{}", cx.t("writes", "zapíše"), value(v))
            };
            let mut seen: BTreeSet<String> = BTreeSet::new();
            let names: Vec<String> = readers
                .iter()
                .map(|r| code(cx.name(r.member.as_str())))
                .filter(|n| seen.insert(n.clone()))
                .collect();
            if !names.is_empty() {
                let _ = write!(t, ", {} {}", cx.t("read by", "čte ho"), cx.series(&names));
            }
            t
        }
        ResultKind::Database { name, .. } => format!(
            "{} {}",
            cx.t("uses the database object", "používá databázový objekt"),
            code(name)
        ),
        ResultKind::Throws => cx
            .t(
                "throws an exception out of the functionality",
                "vyhodí výjimku ven z funkčnosti",
            )
            .to_string(),
        ResultKind::Returns => cx
            .t("returns a value to the trigger", "vrátí hodnotu spouštěči")
            .to_string(),
    }
}

/// One step of a shared part (R2, R3, R4, R7).
fn step_context_line(cx: &Cx<'_>, st: &FlowStep) -> String {
    let m = code(cx.name(st.member.as_str()));
    let text = match &st.kind {
        FlowStepKind::Through => m,
        FlowStepKind::Delegate {
            class,
            invoker,
            targets,
            can_execute,
        } => {
            let to = if targets.is_empty() {
                cx.t("a lambda", "lambda").to_string()
            } else {
                targets
                    .iter()
                    .map(|k| code(cx.name(k.as_str())))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let role = if *can_execute {
                cx.t(
                    "the CanExecute of the command class",
                    "CanExecute třídy příkazu",
                )
            } else {
                cx.t("the command class", "třída příkazu")
            };
            format!(
                "{m}: {} → {to} ({role} {}, {})",
                code(cx.name(invoker.as_str())),
                cx.card_of(class.as_str()),
                cx.t("one step", "jeden krok")
            )
        }
        FlowStepKind::ArgumentChoice {
            callee,
            parameter,
            enum_refs,
            reads,
            ..
        } => {
            let mut by: Vec<String> = enum_refs.iter().map(|e| code(e)).collect();
            if by.is_empty() {
                by = reads.iter().map(|r| code(&written(r))).collect();
            }
            let argument = match parameter {
                Some(p) => format!("{} {}", cx.t("the argument for", "argument pro"), code(p)),
                None => cx.t("an argument", "argument").to_string(),
            };
            let on = if by.is_empty() {
                cx.t("branches on its value", "větví podle své hodnoty")
                    .to_string()
            } else {
                format!("{} {}", cx.t("branches on", "větví podle"), by.join(", "))
            };
            format!(
                "{m} {} {}, {argument} {on}",
                cx.t("calls", "volá"),
                code(cx.name(callee.as_str()))
            )
        }
        FlowStepKind::Call { target, owner, .. } => format!(
            "{m} {} {}, {} ({} {})",
            cx.t("calls", "volá"),
            code(cx.name(target.as_str())),
            cx.t("one step", "jeden krok"),
            cx.t("card", "karta"),
            cx.card_of(owner.as_str())
        ),
        FlowStepKind::UnresolvedContinuation {
            path,
            reason,
            candidates,
        } => {
            let mut t = format!(
                "{m} {} {}: {}, {}",
                cx.t("calls", "volá"),
                code(&written(path)),
                cx.t("unresolved continuation", "nerozřešené pokračování"),
                unresolved(cx, reason)
            );
            let names: Vec<String> = candidates
                .shown
                .iter()
                .map(|k| code(cx.name(k.as_str())))
                .collect();
            if !names.is_empty() || candidates.more > 0 {
                let list = series(cx.lang, &names, names.len() + candidates.more);
                let _ = write!(t, "; {} {list}", cx.t("candidates", "kandidáti"));
            }
            t
        }
        FlowStepKind::Step(kind) => format!("{m}: {}", phrase(cx, kind, &code)),
    };
    format!(
        "- {} {text}{}{}",
        cx.t("Step:", "Krok:"),
        cx.left_out(&text),
        anchor_of(st.place.as_ref())
    )
}

/// The sequence diagram of a flow (KTD8): the types in the order the flow
/// meets them, and a message for every hop and for every step that goes to
/// another member — the command class, the argument that chooses a mode, a
/// call of another type, an unresolved continuation. Names only, so no value
/// of the code is drawn. At most [`SPLIT_THRESHOLD`] messages; the sentence
/// under it says how many there are, and the steps above list them all.
fn sequence(cx: &Cx<'_>, path: &[FlowStep], c: &CommonPart) -> Option<String> {
    let mut participants: Vec<String> = Vec::new();
    let mut messages: Vec<(usize, usize, String)> = Vec::new();
    let at = |name: &str, participants: &mut Vec<String>| match participants
        .iter()
        .position(|p| p == name)
    {
        Some(i) => i,
        None => {
            participants.push(name.to_string());
            participants.len() - 1
        }
    };
    let mut push = |from: &str, to: &str, text: String, participants: &mut Vec<String>| {
        let f = at(from, participants);
        let t = at(to, participants);
        messages.push((f, t, text));
    };
    for (i, hop) in path.iter().enumerate() {
        let next = path
            .get(i + 1)
            .map(|h| &h.member)
            .or_else(|| c.entries.first());
        if let Some(next) = next {
            push(
                owner_name(hop.member.as_str()),
                owner_name(next.as_str()),
                last_name(next.as_str()).to_string(),
                &mut participants,
            );
        }
    }
    for st in &c.steps {
        let from = owner_name(st.member.as_str());
        match &st.kind {
            FlowStepKind::Delegate {
                class,
                invoker,
                targets,
                ..
            } => {
                let class = short(class.as_str());
                push(
                    from,
                    class,
                    last_name(invoker.as_str()).to_string(),
                    &mut participants,
                );
                for t in targets {
                    push(
                        class,
                        owner_name(t.as_str()),
                        last_name(t.as_str()).to_string(),
                        &mut participants,
                    );
                }
            }
            FlowStepKind::ArgumentChoice { callee, .. } => push(
                from,
                owner_name(callee.as_str()),
                last_name(callee.as_str()).to_string(),
                &mut participants,
            ),
            FlowStepKind::Call { target, owner, .. } => push(
                from,
                short(owner.as_str()),
                last_name(target.as_str()).to_string(),
                &mut participants,
            ),
            FlowStepKind::UnresolvedContinuation { path, .. } => push(
                from,
                from,
                format!("? {}", written(path)),
                &mut participants,
            ),
            FlowStepKind::Through | FlowStepKind::Step(_) => {}
        }
    }
    if messages.is_empty() {
        return None;
    }
    let total = messages.len();
    let drawn = total.min(SPLIT_THRESHOLD);
    messages.truncate(drawn);
    // Only the participants a drawn message names.
    let used: BTreeSet<usize> = messages.iter().flat_map(|&(f, t, _)| [f, t]).collect();
    let keep: Vec<usize> = (0..participants.len())
        .filter(|i| used.contains(i))
        .collect();
    let renumber = |i: usize| keep.iter().position(|&k| k == i).unwrap_or(i);
    let names: Vec<String> = keep.iter().map(|&i| participants[i].clone()).collect();
    let messages: Vec<(usize, usize, String)> = messages
        .into_iter()
        .map(|(f, t, text)| (renumber(f), renumber(t), text))
        .collect();
    let mut s = sequence_block(&names, &messages);
    if total > drawn {
        let _ = if cx.cs() {
            writeln!(
                s,
                "\nSekvenční diagram kreslí {} z celkem {total}; všechny uvádějí kroky výše.",
                cs_n(drawn, ("zprávu", "zprávy", "zpráv"))
            )
        } else {
            writeln!(
                s,
                "\nThe sequence diagram draws {drawn} of {total} messages; the steps above list \
                 them all."
            )
        };
    }
    Some(s)
}

/// The `### Context` of a state (R10): who reads it and who writes it from
/// outside the functionality that holds it, each with its anchor.
pub(super) fn state_context(cx: &Cx<'_>, s: &mut String, c: &StateContext) {
    if c.reads.is_empty() && c.writes.is_empty() {
        return;
    }
    s.push_str("\n### Context\n\n");
    for (touches, verb) in [
        (&c.reads, cx.t("Read by", "Čte")),
        (&c.writes, cx.t("Written by", "Zapisuje")),
    ] {
        for t in touches {
            let _ = writeln!(
                s,
                "- {verb} {}{}",
                code(cx.name(t.member.as_str())),
                anchor_of(Some(&t.place))
            );
        }
    }
}
