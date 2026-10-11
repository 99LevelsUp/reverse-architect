//! The second layer on the card of a C# type (plan 2a, U6: R4–R11, R15–R18;
//! KTD9, KTD10).
//!
//! What [`super::layer2::analyze`] read of a type becomes sections of its
//! card, after `## Keywords` and before the notes:
//!
//! - `## Functionality: <the representative's key without its `sym:` prefix>`
//!   with `- Key:` (the whole key) and `- Words:` (R16), one sentence of what
//!   it is from the code, and `### Entry points`, `### Preconditions`,
//!   `### Dead paths`, `### Steps`, `### Flowchart` and `### Members`;
//! - `## State: <the member>` with `### Values`, `### Read in conditions`,
//!   `### Changed by` and `### State diagram`.
//!
//! Each ends in its `### Context` (plan 2b, U6: KTD8), when it has one: at a
//! functionality the flow of every trigger that leads to it under
//! `#### <kind>: <key>` — the trigger with its condition, its path when it is
//! not active, the preconditions and the results, and from level 2 the steps
//! and a `sequenceDiagram` — the part flows share written once, the command
//! class's one flow with the number of its triggers (AE9), and where the flows
//! of its own triggers are drawn (R9); at a state who reads and who writes it
//! from outside (R10). `###` and `####` are no chunks of the manifest, so the
//! context stays in the chunk of its section. A heading
//! carries the representative's whole identity and never the words of R16,
//! so the id of its chunk in the manifest depends on neither the order of
//! the functionalities nor the words their members use.
//!
//! After them, on every level, `## Dynamic access` (plan 2c, U8: KTD11,
//! [`dynamic_access`]): one line per member another place may read, write or
//! call without a static call — the words of its declaration (R17), its
//! marks with a place by place, each with its kind, use, grade and anchor
//! (R16), its matches by name alone counted, with no sentence of R15 — and
//! one sentence on the run's state of XAML coverage (R19).
//!
//! The level of detail (R11): 1 — the functionalities and the states, their
//! entry points, preconditions and dead paths, and the members by name, with
//! no diagram; 2 — the steps, the flowchart of a functionality whose code
//! branches or loops, the state diagram, and one sentence for every helper
//! member; 3 — every member with what it is and what it does, its steps
//! included, fields and properties too. The keywords do not depend on the
//! level: [`words`] reads the whole analysis.
//!
//! Every statement that has a place ends in its anchor — `path:N` or
//! `path:N-M`, never a column — in backticks after ` — `. The sentences are
//! templates in English and Czech (R15); the headings and the bullet keys are
//! English constants. Every value in a diagram goes through
//! [`escape_label`]; the literals were redacted before the model was cached.
//!
//! **A withheld value is admitted, never hidden** (R14, KTD16, AE5). A line
//! whose text holds the marker the secret filter left
//! (`[redacted: <shape> — see the source anchor]`) keeps it as it is and says
//! once, in a template sentence after the text and before the anchor, that a
//! value was left out as a likely secret; the quote of a label's comment is no
//! exception. A flowchart whose drawn node holds the marker says so under the
//! diagram, and the label passes through [`escape_label`] like any other. The
//! marker is no search word: [`words`] leaves it out.

use super::explain::{code, cs_n, plural, series, NAMED_IN_SENTENCE};
use super::keywords::UnitWords;
use super::layer2::{
    flow::{constant_text, written, Cond, Op},
    AdmittedPrecondition, ArmStep, DeadKind, DeadPath, EffectKind, EntryKind, EntryPoint,
    Functionality, FunctionalityFlow, Gap, Guarding, Label, PathState, Place, Precondition, Source,
    State, Step, StepKind, Target, TypeAnalysis, Unresolved, Written,
};
use super::links::{mention, CardIndex};
use super::render::Lang;
use super::{Card, SourceRef};
use crate::cli::LinkStyle;
use crate::extract::bodies::{PathRoot, SecondLayer, ValueRef};
use crate::extract::external::REDACTION_MARKER;
use crate::graph::Span;
use crate::render::mermaid::{escape_label, SPLIT_THRESHOLD};
use std::collections::BTreeSet;
use std::fmt::Write;

mod dynamic_render;
mod render_context;
pub use dynamic_render::{
    dynamic_access, grade_word, markup_document_sentence, markup_sentence, DYNAMIC_ACCESS,
};
use render_context::{functionality_context, state_context};

/// The second layer of one type card: the run's level, whether the type has
/// its second layer, and the analysis when it was analyzed.
#[derive(Debug, Clone, Copy)]
pub struct Layer2<'a> {
    /// `--detail-level`: 1, 2 or 3.
    pub level: u8,
    /// Whether every file of the type was read ([`SecondLayer`]).
    pub state: SecondLayer,
    /// `None` for a type the second layer did not analyze — another
    /// language's, one whose part failed, an interface or an enum.
    pub analysis: Option<&'a TypeAnalysis>,
}

impl<'a> Layer2<'a> {
    /// An analyzed type at `level`.
    pub fn present(level: u8, analysis: &'a TypeAnalysis) -> Layer2<'a> {
        Layer2 {
            level,
            state: SecondLayer::Present,
            analysis: Some(analysis),
        }
    }

    /// The front matter's `layer2:` word.
    pub fn word(&self) -> &'static str {
        match self.state {
            SecondLayer::Present => "present",
            SecondLayer::Failed => "failed",
            SecondLayer::None => "none",
        }
    }
}

/// The search words the second layer brings to the card of the analyzed
/// type (KTD9 point 4): the words of every functionality's name (R16), what
/// it mentions — configuration keys and the names of other types (R9) — the
/// resource keys and the quoted comments of the menu items' labels (R18),
/// and the names of the states. Read from the whole analysis, never from
/// what a level renders, so the keywords are the same on every level (R11).
pub fn words(analysis: &TypeAnalysis) -> UnitWords {
    let mut w = UnitWords::default();
    let label = |w: &mut UnitWords, l: &Option<Label>| match l {
        Some(Label::ResourceKey { key, quote, .. }) => {
            w.identifier(key);
            if let Some(q) = quote.as_ref().filter(|q| !redacted(&q.text)) {
                w.text(&q.text);
            }
        }
        Some(Label::Literal { value, .. }) if !redacted(value) => w.text(value),
        _ => {}
    };
    for f in &analysis.functionalities {
        for word in &f.name.words {
            w.identifier(word);
        }
        for e in &f.entry_points {
            if let Some(menu) = &e.menu {
                label(&mut w, &menu.label);
                if let Some(sub) = &menu.submenu {
                    label(&mut w, &sub.label);
                }
            }
        }
    }
    for flow in &analysis.flow.functionalities {
        for k in &flow.mentions.keywords {
            w.identifier(k);
        }
    }
    for st in &analysis.flow.states {
        w.identifier(last_name(st.member.as_str()));
    }
    w
}

/// A text the secret filter replaced: its marker is no word to search by.
fn redacted(text: &str) -> bool {
    text.contains(REDACTION_MARKER)
}

/// The sections of the analyzed type's card at `level` (see the module docs).
/// `index` and `links` write the links of `### Context` to other cards.
pub fn sections(
    card: &Card,
    lang: Lang,
    level: u8,
    analysis: &TypeAnalysis,
    index: &CardIndex,
    links: LinkStyle,
) -> String {
    let cx = Cx {
        card,
        lang,
        level,
        ty: short(analysis.key.as_str()),
        index,
        links,
    };
    let mut s = String::new();
    for f in &analysis.functionalities {
        functionality(&cx, &mut s, f, analysis.flow_of(&f.key));
        if let Some(c) = analysis.context.of(&f.key) {
            functionality_context(&cx, &mut s, c);
        }
    }
    for st in &analysis.flow.states {
        state(&cx, &mut s, st);
        if let Some(c) = analysis.context.results.state(&st.member) {
            state_context(&cx, &mut s, c);
        }
    }
    s
}

struct Cx<'a> {
    card: &'a Card,
    lang: Lang,
    level: u8,
    /// The type's key without its `sym:` prefix (`EditorView`).
    ty: &'a str,
    index: &'a CardIndex,
    links: LinkStyle,
}

impl Cx<'_> {
    fn cs(&self) -> bool {
        self.lang == Lang::Cs
    }

    /// `en` or `cs`, by the card's language.
    fn t<'s>(&self, en: &'s str, cs: &'s str) -> &'s str {
        if self.cs() {
            cs
        } else {
            en
        }
    }

    /// A member of the type by its own name (`Check(bool)`), anything else
    /// by its type and name (`EditorViewModel.CheckCommand`).
    fn name<'k>(&self, key: &'k str) -> &'k str {
        let s = short(key);
        s.strip_prefix(self.ty)
            .and_then(|rest| rest.strip_prefix('.'))
            .unwrap_or(s)
    }

    /// The first layer's anchor of a member of the type, when it has one.
    fn member_anchor(&self, key: &str) -> Option<String> {
        self.card
            .members
            .iter()
            .find(|m| m.identity == key)
            .and_then(|m| m.source.as_ref())
            .map(SourceRef::anchor)
    }

    fn member_kind(&self, key: &str) -> Option<&str> {
        self.card
            .members
            .iter()
            .find(|m| m.identity == key)
            .and_then(|m| m.member_kind.as_deref())
    }

    /// ` (a value was left out as a likely secret; see the source)` when
    /// `text` holds the marker of a withheld value, and nothing otherwise
    /// (R14): one sentence for a line, however many markers it holds.
    fn left_out(&self, text: &str) -> String {
        left_out(self.lang, text)
    }

    /// `A, B and C`, or `A, B, C and 2 more`.
    fn series(&self, items: &[String]) -> String {
        let shown: Vec<String> = items.iter().take(NAMED_IN_SENTENCE).cloned().collect();
        series(self.lang, &shown, items.len())
    }

    /// A link to the card of the type `ty`, named by its short name; the name
    /// in code when the type has no card.
    fn card_of(&self, ty: &str) -> String {
        mention(&self.card.path, ty, short(ty), self.links, self.index)
    }
}

/// ` (a value was left out as a likely secret; see the source)` in `lang`
/// when `text` holds the marker of a withheld value, and nothing otherwise
/// (R14, R18) — the one wording of the card and the registry of triggers.
pub(super) fn left_out(lang: Lang, text: &str) -> String {
    if !redacted(text) {
        return String::new();
    }
    format!(
        " ({})",
        match lang {
            Lang::En => "a value was left out as a likely secret; see the source",
            Lang::Cs => "hodnota vynechána jako pravděpodobné tajemství; viz zdroj",
        }
    )
}

/// The words a path that is not active opens with (R12), in `lang` — the
/// one wording of the card's `### Context` and the registry of triggers:
/// `dead, turned off at` before the member that turns it off, `unresolved,
/// only callers matched by name alone reach it:` before the weak callers.
/// `None` for an active path.
pub(super) fn path_state_phrase(lang: Lang, state: &PathState) -> Option<&'static str> {
    Some(match (state, lang) {
        (PathState::Active, _) => return None,
        (PathState::Dead { .. }, Lang::En) => "dead, turned off at",
        (PathState::Dead { .. }, Lang::Cs) => "mrtvá, vypíná ji",
        (PathState::Unresolved { .. }, Lang::En) => {
            "unresolved, only callers matched by name alone reach it:"
        }
        (PathState::Unresolved { .. }, Lang::Cs) => {
            "nerozřešená, vedou k ní jen volající podle samotného jména:"
        }
    })
}

/// A key without its `sym:<language>:<qualifier>#` prefix.
pub fn short(key: &str) -> &str {
    match key.strip_prefix("sym:") {
        Some(rest) => rest.split_once('#').map_or(key, |(_, path)| path),
        None => key,
    }
}

/// The last name of a member's key (`isFullscreen` of `AcmeScreen.isFullscreen`).
fn last_name(key: &str) -> &str {
    let s = short(key);
    let name = s.split('(').next().unwrap_or(s);
    match name.rfind('.') {
        Some(i) => &s[i + 1..],
        None => s,
    }
}

/// `path:N` or `path:N-M` — the lines, never a column.
pub fn anchor(place: &Place) -> String {
    SourceRef {
        file: place.path.clone(),
        span: Span::range(place.range.start.line, place.range.end.line),
    }
    .anchor()
}

/// Every anchor of `places`, each once, in their order, after ` — `.
fn anchors<'p>(places: impl IntoIterator<Item = &'p Place>) -> String {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let list: Vec<String> = places
        .into_iter()
        .map(anchor)
        .filter(|a| seen.insert(a.clone()))
        .map(|a| format!("`{a}`"))
        .collect();
    if list.is_empty() {
        String::new()
    } else {
        format!(" — {}", list.join(", "))
    }
}

/// How a text is quoted: as inline code on the card, as it is in a diagram.
type Quote<'q> = &'q dyn Fn(&str) -> String;

fn plain(text: &str) -> String {
    text.to_string()
}

// ---- A functionality ----

fn functionality(cx: &Cx<'_>, s: &mut String, f: &Functionality, flow: Option<&FunctionalityFlow>) {
    let _ = write!(s, "\n## Functionality: {}\n\n", short(&f.key));
    let _ = writeln!(s, "- Key: `{}`", f.key);
    if !f.name.words.is_empty() {
        let _ = writeln!(s, "- Words: {}", f.name.words.join(", "));
    }
    let _ = writeln!(s, "\n{}", what(cx, f, flow));

    s.push_str("\n### Entry points\n\n");
    for e in &f.entry_points {
        let _ = writeln!(s, "- {}", entry_line(cx, e));
    }

    if let Some(flow) = flow {
        if !flow.preconditions.is_empty() || !flow.admitted.is_empty() {
            s.push_str("\n### Preconditions\n\n");
            for p in &flow.preconditions {
                let _ = writeln!(s, "- {}", precondition_line(cx, p));
            }
            for a in &flow.admitted {
                let _ = writeln!(s, "- {}", admitted_line(cx, a));
            }
        }
        if !flow.dead_paths.is_empty() {
            s.push_str("\n### Dead paths\n\n");
            for d in &flow.dead_paths {
                let _ = writeln!(s, "- {}", dead_line(cx, d));
            }
        }
        if cx.level >= 2 && !flow.steps.is_empty() {
            s.push_str("\n### Steps\n\n");
            for st in &flow.steps {
                step_lines(cx, s, st, true);
            }
        }
        if cx.level >= 2 {
            if let Some(chart) = flowchart(cx, f, &flow.steps) {
                s.push_str("\n### Flowchart\n\n");
                s.push_str(&chart);
            }
        }
    }

    if !f.members.is_empty() {
        s.push_str("\n### Members\n\n");
        members(cx, s, f, flow);
    }
}

/// The sentence of what a functionality is, from the code (R5, R9): how many
/// entry points start it, what of other types it leads to, and what it works
/// with.
fn what(cx: &Cx<'_>, f: &Functionality, flow: Option<&FunctionalityFlow>) -> String {
    let n = f.entry_points.len();
    let mut s = if cx.cs() {
        cs_n(n, ("vstupní bod", "vstupní body", "vstupních bodů"))
    } else {
        plural(n, "entry point", "entry points")
    };
    let targets: Vec<String> = f
        .targets
        .iter()
        .map(|t| code(cx.name(t.as_str())))
        .collect();
    if !targets.is_empty() {
        let _ = write!(
            s,
            "; {} {}",
            cx.t("leads to", "vede na"),
            cx.series(&targets)
        );
    }
    s.push('.');
    if let Some(flow) = flow {
        let with: Vec<String> = flow.mentions.works_with().into_iter().map(code).collect();
        if !with.is_empty() {
            let _ = write!(
                s,
                " {} {}.",
                cx.t("It works with", "Pracuje s"),
                cx.series(&with)
            );
        }
    }
    s
}

// ---- Entry points ----

fn entry_line(cx: &Cx<'_>, e: &EntryPoint) -> String {
    let mut s = String::new();
    let member = code(cx.name(e.member.as_str()));
    let mut anchor_at: Option<String> = e.place.as_ref().map(anchor);
    match e.kind {
        EntryKind::Command => {
            let _ = write!(s, "{} {member}", cx.t("Command", "Příkaz"));
            if let Some(site) = e.commands.first() {
                let _ = write!(s, " ({})", code(cx.name(site.class.as_str())));
                anchor_at = Some(anchor(&site.place));
            }
            let roles: Vec<String> = e
                .commands
                .iter()
                .flat_map(|c| &c.delegates)
                .filter_map(|d| {
                    let role = d
                        .invoked_by
                        .iter()
                        .map(|k| code(last_name(k.as_str())))
                        .collect::<Vec<_>>()
                        .join(", ");
                    let runs = targets_text(cx, &d.targets, false);
                    let runs = if runs.is_empty() {
                        match d.value {
                            ValueRef::Lambda(_) => cx.t("a lambda", "lambdu").to_string(),
                            _ => return None,
                        }
                    } else {
                        runs
                    };
                    let role = if role.is_empty() {
                        code(last_name(d.field.as_str()))
                    } else {
                        role
                    };
                    Some(format!("{role} {} {runs}", cx.t("runs", "spouští")))
                })
                .collect();
            if !roles.is_empty() {
                let _ = write!(s, ": {}", roles.join("; "));
            }
            let ambiguous: Vec<String> = e
                .commands
                .iter()
                .flat_map(|c| &c.ambiguous)
                .map(|k| code(cx.name(k.as_str())))
                .collect();
            if !ambiguous.is_empty() {
                let _ = write!(
                    s,
                    "; {} {}",
                    cx.t(
                        "not bound, written from more than one argument:",
                        "nesvázané, zapsané z víc argumentů:"
                    ),
                    ambiguous.join(", ")
                );
            }
        }
        EntryKind::Key => {
            let key = e.discriminator.as_deref().unwrap_or("");
            let _ = write!(
                s,
                "{} {} {} {member}",
                cx.t("Key", "Klávesa"),
                code(key),
                cx.t("in", "v")
            );
            if let Some(when) = key_condition(e) {
                let _ = write!(s, " {} {}", cx.t("when", "když"), code(&when));
            }
        }
        EntryKind::MenuItem => {
            let variable = e.menu.as_ref().map_or_else(
                || e.discriminator.clone().unwrap_or_default(),
                |m| m.variable.clone(),
            );
            let _ = write!(
                s,
                "{} {}",
                cx.t("Menu item", "Položka menu"),
                code(&variable)
            );
            if let Some(menu) = &e.menu {
                if let Some(t) = &menu.type_name {
                    let _ = write!(s, " ({})", code(t));
                }
                if let Some(l) = label_text(cx, &menu.label) {
                    let _ = write!(s, ", {l}");
                }
                if let Some(sub) = &menu.submenu {
                    let _ = write!(
                        s,
                        ", {} {}",
                        cx.t("in the submenu", "v podmenu"),
                        code(&sub.variable)
                    );
                    if let Some(l) = label_text(cx, &sub.label) {
                        let _ = write!(s, " ({l})");
                    }
                }
            }
            let _ = write!(s, ", {} {member}", cx.t("built in", "sestavená v"));
        }
        EntryKind::Handler => {
            let events: Vec<String> = e
                .subscriptions
                .iter()
                .map(|sub| match &sub.source {
                    Some(src) if !src.segments.is_empty() || src.root != PathRoot::This => {
                        code(&format!("{}.{}", written(src), sub.event))
                    }
                    _ => code(&sub.event),
                })
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            let events = if events.is_empty() {
                e.discriminator.iter().map(|d| code(d)).collect()
            } else {
                events
            };
            // A subscribed lambda is told apart by its event; a member
            // handler is the member itself.
            match &e.discriminator {
                Some(_) => {
                    let _ = write!(s, "{} {member}", cx.t("Lambda in", "Lambda v"));
                }
                None => {
                    let _ = write!(s, "{} {member}", cx.t("Handler", "Obsluha"));
                }
            }
            if !events.is_empty() {
                let _ = write!(
                    s,
                    " {} {}",
                    cx.t("handling", "obsluhuje"),
                    events.join(", ")
                );
            }
            if anchor_at.is_none() {
                anchor_at = e.subscriptions.first().map(|sub| anchor(&sub.place));
            }
        }
        EntryKind::PublicMethod => {
            let _ = write!(s, "{} {member}", cx.t("Public method", "Veřejná metoda"));
        }
    }
    if e.kind != EntryKind::Command {
        let targets = targets_text(cx, &e.targets, true);
        if !targets.is_empty() {
            let _ = write!(s, ": {targets}");
        }
    }
    if anchor_at.is_none() {
        anchor_at = cx.member_anchor(e.member.as_str());
    }
    let note = cx.left_out(&s);
    s.push_str(&note);
    if let Some(a) = anchor_at {
        let _ = write!(s, " — `{a}`");
    }
    s
}

/// A key branch's condition without the comparison of the key itself and of
/// its modifiers (`viewModel.ShowErrors || viewModel.CanErrors` of
/// `e.Key == Key.F4 && (…)`), in the order it is written; the whole text when
/// the condition has no tree, or one holding a literal. `None` when nothing
/// but the key is compared.
fn key_condition(e: &EntryPoint) -> Option<String> {
    let condition = e.condition.as_ref()?;
    let Some(tree) = Cond::of(condition, &[]) else {
        return Some(condition.text.clone());
    };
    if holds_literal(&tree) {
        return Some(condition.text.clone());
    }
    let compared: BTreeSet<&str> = e
        .discriminator
        .iter()
        .flat_map(|d| d.split('+'))
        .chain(e.modifiers.iter().map(String::as_str))
        .collect();
    let is_key = |c: &Cond| match c {
        Cond::Compare(Op::Eq, a, b) => [a, b]
            .iter()
            .any(|x| matches!(x.as_ref(), Cond::EnumMember(m) if compared.contains(m.as_str()))),
        _ => false,
    };
    let rest: Vec<Cond> = match tree {
        Cond::And(xs) => xs.into_iter().filter(|x| !is_key(x)).collect(),
        c if is_key(&c) => Vec::new(),
        c => vec![c],
    };
    match rest.len() {
        0 => None,
        1 => Some(rest[0].render()),
        _ => Some(Cond::And(rest).render()),
    }
}

fn holds_literal(c: &Cond) -> bool {
    match c {
        Cond::Literal(_) => true,
        Cond::And(xs) | Cond::Or(xs) => xs.iter().any(holds_literal),
        Cond::Not(x) => holds_literal(x),
        Cond::Compare(_, a, b) | Cond::Binary(_, a, b) => holds_literal(a) || holds_literal(b),
        Cond::Call(_, _, args) => args.iter().any(holds_literal),
        _ => false,
    }
}

/// A menu item's or a submenu's label (R18): the resource key and the quoted
/// comment beside it, a literal, or what it is read from.
fn label_text(cx: &Cx<'_>, label: &Option<Label>) -> Option<String> {
    Some(match label.as_ref()? {
        Label::ResourceKey { key, quote, .. } => {
            let mut s = format!(
                "{} {}",
                cx.t("label resource key", "klíč zdroje popisku"),
                code(key)
            );
            if let Some(q) = quote {
                let (open, close) = if cx.cs() {
                    ("„", "“")
                } else {
                    ("“", "”")
                };
                let _ = write!(
                    s,
                    ", {} {open}{}{close} {}",
                    cx.t("quoted", "citace"),
                    q.text.split_whitespace().collect::<Vec<_>>().join(" "),
                    cx.t(
                        "(from the comment beside it, not read from the code)",
                        "(z komentáře u něj, není přečteno z kódu)"
                    )
                );
            }
            s
        }
        Label::Literal { value, .. } => {
            format!(
                "{} {}",
                cx.t("label", "popisek"),
                code(&format!("\"{value}\""))
            )
        }
        Label::Expression { path } => format!(
            "{} {}",
            cx.t("label from", "popisek z"),
            code(&written(path))
        ),
        Label::Call { callee } => format!(
            "{} {}",
            cx.t("label from a call of", "popisek z volání"),
            code(&written(callee))
        ),
        Label::Other => return None,
    })
}

/// What the targets lead to: `leads to A (through B), B` and every one not
/// resolved with why (R17: admitted, never guessed). `lead` puts the verb in
/// front of the resolved ones.
fn targets_text(cx: &Cx<'_>, targets: &[Target], lead: bool) -> String {
    let mut resolved: Vec<String> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut open: Vec<String> = Vec::new();
    for t in targets {
        match &t.member {
            Some(m) => {
                let mut text = code(cx.name(m.as_str()));
                let at = t.chain.links.iter().position(|l| &l.member == m);
                if let Some(at) = at.filter(|&i| i > 0) {
                    let through: Vec<String> = t.chain.links[..at]
                        .iter()
                        .map(|l| code(cx.name(l.member.as_str())))
                        .collect();
                    let _ = write!(
                        text,
                        " ({} {})",
                        cx.t("through", "přes"),
                        through.join(" → ")
                    );
                }
                if seen.insert(text.clone()) {
                    resolved.push(text);
                }
            }
            None => {
                let why = t
                    .chain
                    .stopped
                    .as_ref()
                    .map_or_else(String::new, |u| format!(": {}", unresolved(cx, u)));
                let text = format!(
                    "{} {}{why}",
                    code(&written(&t.path)),
                    cx.t("is not resolved", "není rozřešené")
                );
                if seen.insert(text.clone()) {
                    open.push(text);
                }
            }
        }
    }
    let mut parts: Vec<String> = Vec::new();
    if !resolved.is_empty() {
        let list = resolved.join(", ");
        parts.push(if lead {
            format!("{} {list}", cx.t("leads to", "vede na"))
        } else {
            list
        });
    }
    parts.extend(open);
    parts.join("; ")
}

/// Why an access, a type or a binding could not be told, in a sentence.
fn unresolved(cx: &Cx<'_>, u: &Unresolved) -> String {
    let en = cx.lang == Lang::En;
    let name = |k: &str| code(cx.name(k));
    match u {
        Unresolved::NoReceiverChain => cx
            .t(
                "the access starts at an expression",
                "přístup začíná výrazem",
            )
            .to_string(),
        Unresolved::NotAMember(n) if en => {
            format!("{} is no member of the type or of its base", code(n))
        }
        Unresolved::NotAMember(n) => format!("{} není členem typu ani jeho předka", code(n)),
        Unresolved::BeyondOneLevel(n) if en => format!(
            "{} is declared more than one level of inheritance up",
            code(n)
        ),
        Unresolved::BeyondOneLevel(n) => format!(
            "{} je deklarovaný víc než o jednu úroveň dědičnosti výš",
            code(n)
        ),
        Unresolved::AmbiguousMember(n) if en => format!("several members are named {}", code(n)),
        Unresolved::AmbiguousMember(n) => format!("jméno {} nese víc členů", code(n)),
        Unresolved::UntypedLocal(n) if en => {
            format!("the local {} is declared without a type", code(n))
        }
        Unresolved::UntypedLocal(n) => {
            format!("lokální proměnná {} je deklarovaná bez typu", code(n))
        }
        Unresolved::AmbiguousLocal(n) if en => {
            format!("the local {} is declared with different types", code(n))
        }
        Unresolved::AmbiguousLocal(n) => {
            format!("lokální proměnná {} je deklarovaná s různými typy", code(n))
        }
        Unresolved::UntypedMember(k) if en => format!("{} states no type", name(k.as_str())),
        Unresolved::UntypedMember(k) => format!("{} neuvádí typ", name(k.as_str())),
        Unresolved::ExternalType(n) if en => {
            format!("the repository declares no type {}", code(n))
        }
        Unresolved::ExternalType(n) => format!("repozitář nedeklaruje typ {}", code(n)),
        Unresolved::AmbiguousType(n) if en => {
            format!("the repository declares several types {}", code(n))
        }
        Unresolved::AmbiguousType(n) => format!("repozitář deklaruje víc typů {}", code(n)),
        Unresolved::UnboundTypeParameter(n) if en => {
            format!("no base list binds the type parameter {}", code(n))
        }
        Unresolved::UnboundTypeParameter(n) => {
            format!("typový parametr {} neváže žádný seznam předků", code(n))
        }
        Unresolved::NestedGeneric(n) if en => {
            format!("the generic argument {} is generic itself", code(n))
        }
        Unresolved::NestedGeneric(n) => format!("generický argument {} je sám generický", code(n)),
        Unresolved::AmbiguousBase(k) if en => {
            format!("{} has more than one base type", name(k.as_str()))
        }
        Unresolved::AmbiguousBase(k) => format!("{} má víc předků", name(k.as_str())),
        Unresolved::BaseListNotRead(k) if en => {
            format!("the base list of {} was not read", name(k.as_str()))
        }
        Unresolved::BaseListNotRead(k) => {
            format!("seznam předků {} nebyl přečten", name(k.as_str()))
        }
        Unresolved::TypeArgumentsMismatch(k) if en => format!(
            "the base list of {} gives another number of type arguments",
            name(k.as_str())
        ),
        Unresolved::TypeArgumentsMismatch(k) => format!(
            "seznam předků {} dává jiný počet typových argumentů",
            name(k.as_str())
        ),
        Unresolved::NoConstructor(n) if en => format!("no constructor takes {n} arguments"),
        Unresolved::NoConstructor(n) => format!("žádný konstruktor nebere {n} argumentů"),
        Unresolved::AmbiguousConstructor(n) if en => {
            format!("more than one constructor takes {n} arguments")
        }
        Unresolved::AmbiguousConstructor(n) => format!("{n} argumentů bere víc konstruktorů"),
        Unresolved::AmbiguousFields(_) => cx
            .t(
                "every argument ends in a field written from more than one argument",
                "každý argument končí v poli zapsaném z víc argumentů",
            )
            .to_string(),
        Unresolved::NotADelegate => cx
            .t(
                "the value is no lambda and no method group",
                "hodnota není lambda ani skupina metod",
            )
            .to_string(),
    }
}

// ---- Preconditions and dead paths ----

fn precondition_line(cx: &Cx<'_>, p: &Precondition) -> String {
    let mut s = format!(
        "{} {}",
        cx.t("Does not run when", "Nespustí se, když"),
        code(&p.text)
    );
    // Where each occurrence was read: a guard of a member or the CanExecute
    // of a command, and the members followed to reach it.
    let mut wheres: Vec<String> = Vec::new();
    for o in &p.occurrences {
        let mut w = match o.source {
            Source::Guard => format!(
                "{} {}",
                cx.t("a guard in", "stráž v"),
                code(cx.name(o.member.as_str()))
            ),
            Source::CanExecute => format!(
                "{} {}",
                cx.t("the CanExecute predicate in", "podmínka CanExecute v"),
                code(cx.name(o.member.as_str()))
            ),
        };
        // The guard's own member closes `via`; it is named already.
        let via: Vec<&crate::graph::NodeKey> = o.via.iter().filter(|k| **k != o.member).collect();
        if !via.is_empty() {
            let path: Vec<String> = via.iter().map(|k| code(cx.name(k.as_str()))).collect();
            let _ = write!(w, " {} {}", cx.t("through", "přes"), path.join(" → "));
        }
        if !wheres.contains(&w) {
            wheres.push(w);
        }
    }
    if !wheres.is_empty() {
        let _ = write!(s, " ({})", wheres.join("; "));
    }
    let mut effects: Vec<String> = Vec::new();
    for e in p.occurrences.iter().flat_map(|o| &o.effects) {
        let verb = match (e.kind, cx.cs()) {
            (EffectKind::Write, false) => "writes",
            (EffectKind::Write, true) => "zapíše",
            (EffectKind::Call, false) => "calls",
            (EffectKind::Call, true) => "volá",
            (EffectKind::Raise, false) => "raises",
            (EffectKind::Raise, true) => "vyvolá",
        };
        let text = format!("{verb} {}", code(&written(&e.path)));
        if !effects.contains(&text) {
            effects.push(text);
        }
    }
    if !effects.is_empty() {
        let _ = write!(
            s,
            "; {} {}",
            cx.t(
                "what runs before it still runs:",
                "co mu předchází, proběhne i tak:"
            ),
            effects.join(", ")
        );
    }
    let note = cx.left_out(&s);
    s.push_str(&note);
    s.push_str(&anchors(p.occurrences.iter().map(|o| &o.place)));
    s
}

fn admitted_line(cx: &Cx<'_>, a: &AdmittedPrecondition) -> String {
    let gap = match &a.gap {
        Gap::Call(u) => format!(
            "{} {}",
            cx.t(
                "the call of its main work is not resolved:",
                "volání hlavní práce není rozřešené:"
            ),
            unresolved(cx, u)
        ),
        Gap::Ancestor(k) => format!(
            "{} {}",
            cx.t(
                "its main work calls a member of a base type, not followed:",
                "hlavní práce volá člena předka, který se nesleduje:"
            ),
            code(cx.name(k.as_str()))
        ),
        Gap::PredicateNotInModel => cx
            .t(
                "the CanExecute predicate is not in the model",
                "podmínka CanExecute v modelu není",
            )
            .to_string(),
    };
    let line = format!(
        "{} {} {gap}",
        code(cx.name(a.member.as_str())),
        cx.t(
            "may have a precondition that was not read —",
            "může mít předpoklad, který se nepřečetl —"
        ),
    );
    format!("{line}{}{}", cx.left_out(&line), anchors([&a.place]))
}

/// What makes a path dead, in a phrase.
fn dead_what(cx: &Cx<'_>, kind: &DeadKind) -> String {
    match kind {
        DeadKind::ConstantReturn(c) => format!(
            "{} {}",
            cx.t("always returns", "vrací vždy"),
            code(&constant_text(c))
        ),
        DeadKind::EmptyBody => cx.t("has an empty body", "má prázdné tělo").to_string(),
        DeadKind::ConstantCondition => cx
            .t("has an arm under `if (false)`", "má větev pod `if (false)`")
            .to_string(),
    }
}

fn dead_line(cx: &Cx<'_>, d: &DeadPath) -> String {
    let what = dead_what(cx, &d.kind);
    let guarding: Vec<&str> = d
        .guarding
        .iter()
        .map(|g| match (g, cx.cs()) {
            (Guarding::ReadInCondition, false) => "read in a condition",
            (Guarding::ReadInCondition, true) => "čte se v podmínce",
            (Guarding::CanExecute, false) => "the CanExecute of a command",
            (Guarding::CanExecute, true) => "CanExecute příkazu",
            (Guarding::CalledByEntry, false) => "called by an entry point",
            (Guarding::CalledByEntry, true) => "volá ho vstupní bod",
        })
        .collect();
    let mut s = format!("{} {what}", code(cx.name(d.member.as_str())));
    if !guarding.is_empty() {
        let _ = write!(s, " ({})", guarding.join(", "));
    }
    let note = cx.left_out(&s);
    s.push_str(&note);
    s.push_str(&anchors([&d.place]));
    s
}

// ---- Steps ----

/// What a step does, in a phrase.
fn phrase(cx: &Cx<'_>, kind: &StepKind, q: Quote<'_>) -> String {
    let cs = cx.cs();
    match kind {
        StepKind::Idiom(tag) => match (tag.as_str(), cs) {
            ("full-traversal", false) => "goes through the whole collection".to_string(),
            ("full-traversal", true) => "projde celou kolekci".to_string(),
            ("find-first", false) => "finds the first match".to_string(),
            ("find-first", true) => "najde první shodu".to_string(),
            ("accumulate", false) => "accumulates a result".to_string(),
            ("accumulate", true) => "střádá výsledek".to_string(),
            ("guard-return", false) => "returns early".to_string(),
            ("guard-return", true) => "vrátí se předčasně".to_string(),
            ("swallow-exception", false) => "swallows an exception".to_string(),
            ("swallow-exception", true) => "spolkne výjimku".to_string(),
            (other, _) => q(other),
        },
        StepKind::LazyInit { field } => format!(
            "{} {} {}",
            cx.t("creates", "vytvoří"),
            q(cx.name(field.as_str())),
            cx.t("on its first read", "při prvním čtení")
        ),
        StepKind::NotifiesChange => cx
            .t("raises a change notification", "ohlásí změnu")
            .to_string(),
        StepKind::Loop => cx.t("repeats a loop", "opakuje smyčku").to_string(),
        StepKind::Choice { arms } => format!(
            "{} {}",
            cx.t("chooses:", "volí:"),
            if cs {
                cs_n(arms.len(), ("větev", "větve", "větví"))
            } else {
                plural(arms.len(), "arm", "arms")
            }
        ),
        StepKind::ParameterChoice { parameter, arms } => {
            let picks: Vec<String> = arms
                .iter()
                .filter(|a| !a.selects.is_empty())
                .map(|a| {
                    a.selects
                        .iter()
                        .map(|x| q(x))
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .collect();
            let mut s = format!(
                "{} {} {}",
                cx.t("the parameter", "parametr"),
                q(parameter),
                cx.t("chooses", "volí")
            );
            if picks.len() == arms.len() && !picks.is_empty() {
                let _ = write!(s, ": {}", picks.join(cx.t(" or ", ", nebo ")));
            } else if cs {
                let _ = write!(s, ": {}", cs_n(arms.len(), ("větev", "větve", "větví")));
            } else {
                let _ = write!(s, ": {}", plural(arms.len(), "arm", "arms"));
            }
            s
        }
        StepKind::Call {
            callee,
            member,
            construct,
        } => {
            let target = match (construct, member) {
                (false, Some(m)) => q(cx.name(m.as_str())),
                _ => q(&written(callee)),
            };
            if *construct {
                format!("{} {target}", cx.t("constructs", "vytvoří"))
            } else {
                format!("{} {target}", cx.t("calls", "volá"))
            }
        }
        StepKind::StateChange { state, value } => {
            let st = q(cx.name(state.as_str()));
            match value {
                Written::Constant(c) => format!(
                    "{} {st} {} {}",
                    cx.t("sets", "nastaví"),
                    cx.t("to", "na"),
                    q(&constant_text(c))
                ),
                Written::Toggle => format!("{} {st}", cx.t("toggles", "přepne")),
                Written::Other => format!("{} {st}", cx.t("changes", "změní")),
            }
        }
    }
}

fn arms_of(kind: &StepKind) -> Option<&[ArmStep]> {
    match kind {
        StepKind::Choice { arms } | StepKind::ParameterChoice { arms, .. } => Some(arms),
        _ => None,
    }
}

/// The variable every arm but an `else` compares with `==` — the chain of
/// `else if` over one variable a table shows (R2).
fn uniform_variable(arms: &[ArmStep]) -> Option<&str> {
    let mut conditioned = arms.iter().filter_map(|a| a.condition.as_deref());
    let first = conditioned.next()?.split_once(" == ")?.0;
    let mut n = 1;
    for c in conditioned {
        if c.split_once(" == ")?.0 != first {
            return None;
        }
        n += 1;
    }
    // At most one `else`, and it comes last.
    let else_last = arms
        .iter()
        .position(|a| a.condition.is_none())
        .is_none_or(|i| i + 1 == arms.len());
    (n >= 2 && else_last).then_some(first)
}

/// A table cell: a pipe would end it.
fn cell(text: &str) -> String {
    text.replace('|', "\\|")
}

/// The lines of one step: the step itself, prefixed by its member when
/// `with_member`, and the arms of a choice under it — a table when they are
/// an `else if` chain over one variable.
fn step_lines(cx: &Cx<'_>, s: &mut String, st: &Step, with_member: bool) {
    let indent = if with_member { "" } else { "  " };
    let lead = if with_member {
        format!("{}: ", code(cx.name(st.member.as_str())))
    } else {
        String::new()
    };
    let text = phrase(cx, &st.kind, &code);
    let _ = writeln!(
        s,
        "{indent}- {lead}{text}{}{}",
        cx.left_out(&text),
        anchors([&st.place])
    );
    let Some(arms) = arms_of(&st.kind) else {
        return;
    };
    if uniform_variable(arms).is_some() {
        let _ = writeln!(s, "\n{indent}  | When | Selects | Anchor |");
        let _ = writeln!(s, "{indent}  |---|---|---|");
        for a in arms {
            let when = a
                .condition
                .as_deref()
                .map_or_else(|| cx.t("otherwise", "jinak").to_string(), code);
            let selects: Vec<String> = a.selects.iter().map(|x| code(x)).collect();
            let selects = selects.join(", ");
            let note = cx.left_out(&format!("{when} {selects}"));
            let _ = writeln!(
                s,
                "{indent}  | {} | {} | `{}` |",
                cell(&format!("{when}{note}")),
                cell(&selects),
                anchor(&a.place)
            );
        }
        s.push('\n');
        return;
    }
    for a in arms {
        let when = match &a.condition {
            Some(c) => format!("{} {}", cx.t("when", "když"), code(c)),
            None => cx.t("otherwise", "jinak").to_string(),
        };
        let selects = if a.selects.is_empty() {
            String::new()
        } else {
            let list: Vec<String> = a.selects.iter().map(|x| code(x)).collect();
            format!(": {} {}", cx.t("selects", "vybere"), list.join(", "))
        };
        let note = cx.left_out(&format!("{when}{selects}"));
        let _ = writeln!(
            s,
            "{indent}  - {when}{selects}{note}{}",
            anchors([&a.place])
        );
    }
}

// ---- The flowchart ----

fn is_loop(kind: &StepKind) -> bool {
    match kind {
        StepKind::Loop => true,
        StepKind::Idiom(t) => matches!(t.as_str(), "full-traversal" | "find-first" | "accumulate"),
        _ => false,
    }
}

fn branches(kind: &StepKind) -> bool {
    arms_of(kind).is_some() || is_loop(kind)
}

/// Whether `outer` holds `inner` in one file.
fn holds(outer: &Place, inner: &Place) -> bool {
    outer.path == inner.path
        && outer.range.start <= inner.range.start
        && inner.range.end <= outer.range.end
}

/// A step with the steps inside it.
struct Tree<'s> {
    step: &'s Step,
    children: Vec<Tree<'s>>,
}

/// The steps of one member as trees: a choice or a loop holds the steps
/// written inside it. `steps` are by start, the outer first.
fn trees<'s>(steps: &[&'s Step]) -> Vec<Tree<'s>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < steps.len() {
        let st = steps[i];
        let mut j = i + 1;
        if branches(&st.kind) {
            while j < steps.len() && holds(&st.place, &steps[j].place) {
                j += 1;
            }
        }
        out.push(Tree {
            step: st,
            children: trees(&steps[i + 1..j]),
        });
        i = j;
    }
    out
}

#[derive(Clone, Copy)]
enum Shape {
    Start,
    Member,
    Step,
    Decision,
    Arm,
    Loop,
}

#[derive(Default)]
struct Chart {
    nodes: Vec<(Shape, String)>,
    /// `(from, to, back)`: a back edge closes a loop.
    edges: Vec<(usize, usize, bool)>,
}

impl Chart {
    fn node(&mut self, shape: Shape, label: String) -> usize {
        self.nodes.push((shape, label));
        self.nodes.len() - 1
    }

    fn link_all(&mut self, prev: &[usize], to: usize) {
        for &p in prev {
            self.edges.push((p, to, false));
        }
    }

    fn sequence(&mut self, cx: &Cx<'_>, trees: &[Tree<'_>], mut prev: Vec<usize>) -> Vec<usize> {
        for t in trees {
            prev = self.tree(cx, t, prev);
        }
        prev
    }

    fn tree(&mut self, cx: &Cx<'_>, t: &Tree<'_>, prev: Vec<usize>) -> Vec<usize> {
        let label = phrase(cx, &t.step.kind, &plain);
        if let Some(arms) = arms_of(&t.step.kind) {
            let n = self.node(Shape::Decision, label);
            self.link_all(&prev, n);
            let mut ends = Vec::new();
            for a in arms {
                let mut text = match &a.condition {
                    Some(c) => format!("{} {c}", cx.t("when", "když")),
                    None => cx.t("otherwise", "jinak").to_string(),
                };
                if !a.selects.is_empty() {
                    let _ = write!(text, " → {}", a.selects.join(", "));
                }
                let arm = self.node(Shape::Arm, text);
                self.edges.push((n, arm, false));
                let inside: Vec<&Tree<'_>> = t
                    .children
                    .iter()
                    .filter(|c| holds(&a.place, &c.step.place))
                    .collect();
                let mut at = vec![arm];
                for c in inside {
                    at = self.tree(cx, c, at);
                }
                ends.extend(at);
            }
            if arms.iter().all(|a| a.condition.is_some()) {
                ends.push(n);
            }
            ends
        } else if is_loop(&t.step.kind) {
            let n = self.node(Shape::Loop, label);
            self.link_all(&prev, n);
            let ends = self.sequence(cx, &t.children, vec![n]);
            for e in ends.into_iter().filter(|&e| e != n) {
                self.edges.push((e, n, true));
            }
            vec![n]
        } else {
            let n = self.node(Shape::Step, label);
            self.link_all(&prev, n);
            vec![n]
        }
    }
}

/// The flowchart of a functionality whose code branches or loops (R8): from
/// the functionality to each member with steps, and through that member's
/// steps in the order of the source — a choice as one decision with an arm
/// per branch, so an `else if` chain over one variable is one decision and
/// a table of arms; a loop with the steps inside it and an edge back. At
/// most [`SPLIT_THRESHOLD`] nodes are drawn; the sentence under the diagram
/// says how many there are, and the steps above list them all.
fn flowchart(cx: &Cx<'_>, f: &Functionality, steps: &[Step]) -> Option<String> {
    if !steps.iter().any(|s| branches(&s.kind)) {
        return None;
    }
    let mut chart = Chart::default();
    let start = chart.node(Shape::Start, short(&f.key).to_string());
    let mut members: Vec<&str> = steps.iter().map(|s| s.member.as_str()).collect();
    members.dedup();
    for m in members {
        let mut own: Vec<&Step> = steps.iter().filter(|s| s.member.as_str() == m).collect();
        // By start, the outer of two that start together first.
        own.sort_by(|a, b| {
            (
                &a.place.path,
                a.place.range.start,
                std::cmp::Reverse(a.place.range.end),
            )
                .cmp(&(
                    &b.place.path,
                    b.place.range.start,
                    std::cmp::Reverse(b.place.range.end),
                ))
        });
        let node = chart.node(Shape::Member, cx.name(m).to_string());
        chart.edges.push((start, node, false));
        let forest = trees(&own);
        chart.sequence(cx, &forest, vec![node]);
    }

    let total = chart.nodes.len();
    let drawn = total.min(SPLIT_THRESHOLD);
    let mut s = String::from("```mermaid\nflowchart TD\n");
    for (i, (shape, label)) in chart.nodes.iter().take(drawn).enumerate() {
        let l = escape_label(label);
        let _ = match shape {
            Shape::Start => writeln!(s, "    n{i}([\"{l}\"])"),
            Shape::Member => writeln!(s, "    n{i}[[\"{l}\"]]"),
            Shape::Step => writeln!(s, "    n{i}[\"{l}\"]"),
            Shape::Decision => writeln!(s, "    n{i}{{\"{l}\"}}"),
            Shape::Arm => writeln!(s, "    n{i}(\"{l}\")"),
            Shape::Loop => writeln!(s, "    n{i}{{{{\"{l}\"}}}}"),
        };
    }
    for &(from, to, back) in &chart.edges {
        if from < drawn && to < drawn {
            let arrow = if back { "-.->" } else { "-->" };
            let _ = writeln!(s, "    n{from} {arrow} n{to}");
        }
    }
    s.push_str("```\n");
    if chart
        .nodes
        .iter()
        .take(drawn)
        .any(|(_, label)| redacted(label))
    {
        let _ = writeln!(
            s,
            "\n{}",
            cx.t(
                "A value in the diagram was left out as a likely secret; see the source.",
                "Hodnota v diagramu vynechána jako pravděpodobné tajemství; viz zdroj."
            )
        );
    }
    if total > drawn {
        let _ = if cx.cs() {
            writeln!(
                s,
                "\nDiagram kreslí {drawn} uzlů z celkem {total}; zbytek uvádějí kroky výše."
            )
        } else {
            writeln!(
                s,
                "\nThe flowchart draws {drawn} of {total} nodes; the steps above list the rest."
            )
        };
    }
    Some(s)
}

// ---- Members ----

fn kind_word<'k>(cx: &Cx<'_>, kind: &'k str) -> &'k str {
    if !cx.cs() {
        return kind;
    }
    match kind {
        "method" => "metoda",
        "field" => "pole",
        "property" => "vlastnost",
        "constructor" => "konstruktor",
        "event" => "událost",
        "indexer" => "indexer",
        other => other,
    }
}

/// `### Members` (R11): level 1 names them; level 2 gives every helper — a
/// member no entry point is written in — one sentence; level 3 gives every
/// member its sentence and its steps.
fn members(cx: &Cx<'_>, s: &mut String, f: &Functionality, flow: Option<&FunctionalityFlow>) {
    if cx.level < 2 {
        let names: Vec<String> = f
            .members
            .iter()
            .map(|m| code(cx.name(m.as_str())))
            .collect();
        let _ = writeln!(s, "{}", names.join(", "));
        return;
    }
    let entries: BTreeSet<&str> = f.entry_points.iter().map(|e| e.member.as_str()).collect();
    let no_steps: Vec<Step> = Vec::new();
    let steps = flow.map_or(&no_steps, |fl| &fl.steps);
    for m in &f.members {
        let key = m.as_str();
        let name = code(cx.name(key));
        let own: Vec<&Step> = steps.iter().filter(|st| st.member == *m).collect();
        if cx.level < 3 && entries.contains(key) {
            let _ = writeln!(s, "- {name} ({})", cx.t("an entry point", "vstupní bod"));
            continue;
        }
        let mut line = name;
        if let Some(kind) = cx.member_kind(key) {
            let _ = write!(line, " ({})", kind_word(cx, kind));
        }
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let phrases: Vec<String> = own
            .iter()
            .map(|st| phrase(cx, &st.kind, &code))
            .filter(|p| seen.insert(p.clone()))
            .collect();
        if phrases.is_empty() {
            let _ = write!(
                line,
                ": {}.",
                cx.t("no step of its own", "žádný vlastní krok")
            );
        } else {
            let sentence = cx.series(&phrases);
            let _ = write!(line, ": {sentence}{}.", cx.left_out(&sentence));
        }
        if let Some(a) = cx.member_anchor(key) {
            let _ = write!(line, " — `{a}`");
        }
        let _ = writeln!(s, "- {line}");
        if cx.level >= 3 {
            for st in own {
                step_lines(cx, s, st, false);
            }
        }
    }
}

// ---- A state ----

fn state(cx: &Cx<'_>, s: &mut String, st: &State) {
    let _ = write!(s, "\n## State: {}\n\n", short(st.member.as_str()));
    let _ = writeln!(s, "- Key: `{}`", st.member.as_str());
    if let Some(field) = &st.field {
        let _ = writeln!(s, "- Field: {}", code(cx.name(field.as_str())));
    }
    if !st.values.is_empty() {
        s.push_str("\n### Values\n\n");
        for v in &st.values {
            let value = match &v.value {
                Written::Constant(c) => code(&constant_text(c)),
                Written::Toggle => cx.t("toggled", "přepíná se").to_string(),
                Written::Other => cx.t("another value", "jiná hodnota").to_string(),
            };
            let _ = writeln!(s, "- {value}{}", anchors(&v.places));
        }
    }
    if !st.reads.is_empty() {
        s.push_str("\n### Read in conditions\n\n");
        let n = st.reads.len();
        let count = if cx.cs() {
            format!(
                "Čte se v {}",
                cs_n(n, ("podmínce", "podmínkách", "podmínkách"))
            )
        } else {
            format!("Read in {}", plural(n, "condition", "conditions"))
        };
        let _ = writeln!(s, "{count}{}", anchors(&st.reads));
    }
    if !st.changes.is_empty() {
        s.push_str("\n### Changed by\n\n");
        for c in &st.changes {
            let how = match &c.value {
                Written::Constant(v) => format!(
                    "{} {}",
                    cx.t("sets it to", "ho nastaví na"),
                    code(&constant_text(v))
                ),
                Written::Toggle => cx.t("toggles it", "ho přepne").to_string(),
                Written::Other => cx.t("changes it otherwise", "ho změní jinak").to_string(),
            };
            let _ = writeln!(
                s,
                "- {} {how}{}",
                code(cx.name(c.member.as_str())),
                anchors([&c.place])
            );
        }
    }
    if cx.level >= 2 {
        if let Some(d) = state_diagram(cx, st) {
            s.push_str("\n### State diagram\n\n");
            s.push_str(&d);
        }
    }
}

/// The state diagram (R6): a state per value the state is given — both of a
/// toggled `bool` — and one for any other value, and a transition into each
/// value from every other, labelled by the member that writes it. At most
/// [`SPLIT_THRESHOLD`] states are drawn.
fn state_diagram(cx: &Cx<'_>, st: &State) -> Option<String> {
    let mut values: Vec<String> = Vec::new();
    let add = |values: &mut Vec<String>, v: String| {
        if !values.contains(&v) {
            values.push(v);
        }
    };
    let other = cx.t("another value", "jiná hodnota").to_string();
    for v in &st.values {
        match &v.value {
            Written::Constant(c) => add(&mut values, constant_text(c)),
            Written::Toggle => {
                add(&mut values, "true".to_string());
                add(&mut values, "false".to_string());
            }
            Written::Other => {}
        }
    }
    if st.changes.iter().any(|c| c.value == Written::Other) {
        add(&mut values, other.clone());
    }
    if values.len() < 2 {
        return None;
    }
    let index = |v: &str| values.iter().position(|x| x == v);
    let mut transitions: BTreeSet<(usize, usize, String)> = BTreeSet::new();
    for c in &st.changes {
        let label = cx.name(c.member.as_str()).to_string();
        match &c.value {
            Written::Toggle => {
                if let (Some(t), Some(f)) = (index("true"), index("false")) {
                    transitions.insert((t, f, label.clone()));
                    transitions.insert((f, t, label));
                }
            }
            value => {
                let target = match value {
                    Written::Constant(v) => constant_text(v),
                    _ => other.clone(),
                };
                let Some(to) = index(&target) else {
                    continue;
                };
                for from in (0..values.len()).filter(|&i| i != to) {
                    transitions.insert((from, to, label.clone()));
                }
            }
        }
    }
    let total = values.len();
    let drawn = total.min(SPLIT_THRESHOLD);
    let mut s = String::from("```mermaid\nstateDiagram-v2\n");
    for (i, v) in values.iter().take(drawn).enumerate() {
        let _ = writeln!(s, "    state \"{}\" as s{i}", escape_label(v));
    }
    for (from, to, label) in &transitions {
        if *from < drawn && *to < drawn {
            let _ = writeln!(s, "    s{from} --> s{to} : {}", escape_label(label));
        }
    }
    s.push_str("```\n");
    if total > drawn {
        let _ = if cx.cs() {
            writeln!(
                s,
                "\nDiagram kreslí {drawn} hodnot z celkem {total}; všechny uvádějí hodnoty výše."
            )
        } else {
            writeln!(
                s,
                "\nThe diagram draws {drawn} of {total} values; the values above list them all."
            )
        };
    }
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::bodies::Range;

    fn place(path: &str, range: [u32; 4]) -> Place {
        Place {
            path: path.to_string(),
            range: Range::from(range),
        }
    }

    #[test]
    fn an_anchor_is_the_lines_and_never_a_column() {
        assert_eq!(anchor(&place("src/A.cs", [12, 5, 12, 40])), "src/A.cs:12");
        assert_eq!(anchor(&place("src/A.cs", [12, 5, 15, 2])), "src/A.cs:12-15");
    }

    #[test]
    fn a_key_loses_its_symbol_prefix_and_keeps_its_discriminator() {
        assert_eq!(
            short("sym:csharp:Acme.Views#EditorView.Handler(object,KeyEventArgs)#Key.F4"),
            "EditorView.Handler(object,KeyEventArgs)#Key.F4"
        );
        assert_eq!(short("sym:csharp:#Global.Run()"), "Global.Run()");
        assert_eq!(short("src/a.cs"), "src/a.cs");
        assert_eq!(
            last_name("sym:csharp:Acme#Screen.isFullscreen"),
            "isFullscreen"
        );
        assert_eq!(
            last_name("sym:csharp:Acme#Screen.Run(Acme.Kind)"),
            "Run(Acme.Kind)"
        );
    }

    fn arm(condition: Option<&str>) -> ArmStep {
        ArmStep {
            condition: condition.map(str::to_string),
            place: place("a.cs", [1, 1, 1, 2]),
            selects: Vec::new(),
        }
    }

    #[test]
    fn an_else_if_chain_over_one_variable_is_uniform_and_a_mixed_one_is_not() {
        let chain = [
            arm(Some("syntax == SyntaxKind.Sql")),
            arm(Some("syntax == SyntaxKind.Script")),
            arm(None),
        ];
        assert_eq!(uniform_variable(&chain), Some("syntax"));
        let mixed = [
            arm(Some("syntax == SyntaxKind.Sql")),
            arm(Some("mode == 1")),
        ];
        assert_eq!(uniform_variable(&mixed), None);
        assert_eq!(uniform_variable(&[arm(Some("a == 1")), arm(None)]), None);
        assert_eq!(cell("a || b"), "a \\|\\| b");
    }
}
