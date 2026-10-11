use super::{anchor, short};
use crate::card::explain::{code, cs_n, plural};
use crate::card::layer2::{GenericMark, MarkKind, MarkUse, MemberLine, TypeAnalysis};
use crate::card::render::Lang;
use crate::coverage::MarkupState;
use crate::graph::Confidence;
use std::fmt::Write;

// ---- Dynamic access (plan 2c, U8: R15–R19; KTD11) ----

/// The heading of the section, an English constant: the id of its chunk in
/// the manifest, `<card identity>#dynamic-access`, depends on nothing else.
pub const DYNAMIC_ACCESS: &str = "Dynamic access";

/// The one sentence on the run's state of XAML coverage (R19), which ends the
/// `## Dynamic access` of every analyzed type's card and stands under the
/// header of every XAML document of a run with the second layer: whether a
/// missing marker from markup is evidence or not.
pub fn markup_sentence(state: MarkupState, lang: Lang) -> &'static str {
    match (state, lang) {
        (MarkupState::NotRead, Lang::En) => {
            "XAML was not read in this run — the corpus holds no XAML file, or no plugin of \
             the run reads it (coverage.json, `markup`: `files`, `plugins`) — so a member \
             without a marker from markup may still be bound, commanded or handled from XAML."
        }
        (MarkupState::NotRead, Lang::Cs) => {
            "XAML se v tomto běhu nečetl — korpus nemá žádný soubor XAML, nebo ho žádný plugin \
             běhu nečte (coverage.json, `markup`: `files`, `plugins`) —, takže člen bez značky \
             z markupu může být z XAML přesto vázaný, spouštěný příkazem nebo obsluhovaný."
        }
        (MarkupState::ReadWithUnsupportedConstructs, Lang::En) => {
            "XAML was read with unsupported constructs in this run — a file failed, a component \
             read no bodies, or a markup extension could not be taken apart; coverage.json \
             lists them under `markup`, and the XAML plugin's findings its limits — so a \
             missing marker is no evidence that no way from XAML exists."
        }
        (MarkupState::ReadWithUnsupportedConstructs, Lang::Cs) => {
            "XAML se v tomto běhu přečetl s nepodporovanými konstrukty — soubor selhal, \
             komponenta nečetla těla nebo se rozšíření markupu nepodařilo rozebrat; \
             coverage.json je vypisuje v oddílu `markup` a kniha nálezů pluginu pro XAML jeho \
             meze —, takže chybějící značka není doklad, že cesta z XAML neexistuje."
        }
        (MarkupState::ReadInFull, Lang::En) => {
            "XAML was read in full in this run, so no binding, command or handler of XAML the \
             plugin reads leads to a member without a marker from markup (coverage.json, \
             `markup`)."
        }
        (MarkupState::ReadInFull, Lang::Cs) => {
            "XAML se v tomto běhu přečetl úplně, takže k členu bez značky z markupu nevede z \
             XAML žádný binding, příkaz ani obsluha, které plugin čte (coverage.json, \
             `markup`)."
        }
        (MarkupState::SecondLayerOff, Lang::En) => {
            "The second layer is off in this run, so no marker from markup was made \
             (coverage.json, `markup`)."
        }
        (MarkupState::SecondLayerOff, Lang::Cs) => {
            "Druhá vrstva je v tomto běhu vypnutá, takže žádná značka z markupu nevznikla \
             (coverage.json, `markup`)."
        }
    }
}

/// The state of XAML coverage as the header of a XAML document states it
/// (R19): the run's, never the file's.
pub fn markup_document_sentence(state: MarkupState, lang: Lang) -> &'static str {
    match (state, lang) {
        (MarkupState::NotRead, Lang::En) => {
            "XAML coverage of this run: not read — no plugin of the run reads XAML \
             (coverage.json, `markup`)."
        }
        (MarkupState::NotRead, Lang::Cs) => {
            "Pokrytí XAML v tomto běhu: nepřečteno — žádný plugin běhu XAML nečte \
             (coverage.json, `markup`)."
        }
        (MarkupState::ReadWithUnsupportedConstructs, Lang::En) => {
            "XAML coverage of this run: read with unsupported constructs — a file failed, a \
             component read no bodies, or a markup extension could not be taken apart; \
             coverage.json lists them under `markup`, and the XAML plugin's findings its \
             limits."
        }
        (MarkupState::ReadWithUnsupportedConstructs, Lang::Cs) => {
            "Pokrytí XAML v tomto běhu: přečteno s nepodporovanými konstrukty — soubor selhal, \
             komponenta nečetla těla nebo se rozšíření markupu nepodařilo rozebrat; \
             coverage.json je vypisuje v oddílu `markup` a kniha nálezů pluginu pro XAML jeho \
             meze."
        }
        (MarkupState::ReadInFull, Lang::En) => {
            "XAML coverage of this run: read in full — every XAML file was read by both layers \
             (coverage.json, `markup`)."
        }
        (MarkupState::ReadInFull, Lang::Cs) => {
            "Pokrytí XAML v tomto běhu: přečteno úplně — každý soubor XAML přečetly obě vrstvy \
             (coverage.json, `markup`)."
        }
        (MarkupState::SecondLayerOff, Lang::En) => {
            "XAML coverage of this run: the second layer is off (coverage.json, `markup`)."
        }
        (MarkupState::SecondLayerOff, Lang::Cs) => {
            "Pokrytí XAML v tomto běhu: druhá vrstva je vypnutá (coverage.json, `markup`)."
        }
    }
}

/// A word of a declaration (R17).
fn generic_word(mark: GenericMark, lang: Lang) -> &'static str {
    match (mark, lang) {
        (GenericMark::Interface, Lang::En) => "interface",
        (GenericMark::Interface, Lang::Cs) => "rozhraní",
        (GenericMark::Virtual, Lang::En) => "virtual",
        (GenericMark::Virtual, Lang::Cs) => "virtuální",
        (GenericMark::Override, Lang::En) => "override",
        (GenericMark::Override, Lang::Cs) => "přepis",
        (GenericMark::Handler, Lang::En) => "handler",
        (GenericMark::Handler, Lang::Cs) => "obsluha",
    }
}

fn mark_kind_word(kind: MarkKind, lang: Lang) -> &'static str {
    match (kind, lang) {
        (MarkKind::Binding, _) => "binding",
        (MarkKind::Static, Lang::En) => "static member",
        (MarkKind::Static, Lang::Cs) => "statický člen",
        (MarkKind::Attached, Lang::En) => "attached property",
        (MarkKind::Attached, Lang::Cs) => "připojená vlastnost",
        (MarkKind::Setter, _) => "setter",
        (MarkKind::Handler, Lang::En) => "handler",
        (MarkKind::Handler, Lang::Cs) => "obsluha",
        (MarkKind::Literal, Lang::En) => "literal",
        (MarkKind::Literal, Lang::Cs) => "literál",
        (MarkKind::NameOf, _) => "nameof",
    }
}

fn mark_use_word(mode: MarkUse, lang: Lang) -> &'static str {
    match (mode, lang) {
        (MarkUse::Read, Lang::En) => "read",
        (MarkUse::Read, Lang::Cs) => "čtení",
        (MarkUse::Write, Lang::En) => "write",
        (MarkUse::Write, Lang::Cs) => "zápis",
        (MarkUse::Call, Lang::En) => "call",
        (MarkUse::Call, Lang::Cs) => "volání",
        (MarkUse::Named, Lang::En) => "named",
        (MarkUse::Named, Lang::Cs) => "jméno",
    }
}

/// The verb of the sentence of R15 for one use: `read`, `čten`.
fn mark_use_verb(mode: MarkUse, lang: Lang) -> &'static str {
    match (mode, lang) {
        (MarkUse::Read, Lang::En) => "read",
        (MarkUse::Read, Lang::Cs) => "čten",
        (MarkUse::Write, Lang::En) => "written",
        (MarkUse::Write, Lang::Cs) => "zapisován",
        (MarkUse::Call, Lang::En) => "called",
        (MarkUse::Call, Lang::Cs) => "volán",
        (MarkUse::Named, Lang::En) => "named",
        (MarkUse::Named, Lang::Cs) => "jmenován",
    }
}

/// How sure a mark is (KTD8), in the card's words.
pub fn grade_word(grade: Confidence, lang: Lang) -> &'static str {
    match (grade, lang) {
        (Confidence::ResolvedByIndex, Lang::En) => "resolved by an index",
        (Confidence::ResolvedByIndex, Lang::Cs) => "podle indexu",
        (Confidence::RulesBacked, Lang::En) => "rules-backed",
        (Confidence::RulesBacked, Lang::Cs) => "podle pravidla",
        (Confidence::NameOnly, Lang::En) => "by name only",
        (Confidence::NameOnly, Lang::Cs) => "jen podle jména",
        (Confidence::Ambiguous, Lang::En) => "ambiguous",
        (Confidence::Ambiguous, Lang::Cs) => "nejednoznačně",
    }
}

/// One member's line (KTD11): the words of its declaration, the sentence of
/// R15 with every mark with a place — by place — and the count of its
/// matches by name alone, which no sentence of R15 comes with.
fn dynamic_line(ty: &str, line: &MemberLine, lang: Lang) -> String {
    let cs = lang == Lang::Cs;
    let s = short(line.member.as_str());
    let name = s
        .strip_prefix(ty)
        .and_then(|rest| rest.strip_prefix('.'))
        .unwrap_or(s);
    let mut parts: Vec<String> = Vec::new();
    if !line.generic.is_empty() {
        parts.push(
            line.generic
                .iter()
                .map(|g| generic_word(*g, lang))
                .collect::<Vec<_>>()
                .join(", "),
        );
    }
    if !line.marks.is_empty() {
        let mut uses: Vec<MarkUse> = line.marks.iter().map(|m| m.mode).collect();
        uses.sort();
        uses.dedup();
        let verbs: Vec<&str> = uses.iter().map(|u| mark_use_verb(*u, lang)).collect();
        let verbs = verbs.join(if cs { " nebo " } else { " or " });
        let marks: Vec<String> = line
            .marks
            .iter()
            .map(|m| {
                format!(
                    "{} ({}, {}) — `{}`",
                    mark_kind_word(m.kind, lang),
                    mark_use_word(m.mode, lang),
                    grade_word(m.grade, lang),
                    anchor(&m.place)
                )
            })
            .collect();
        parts.push(if cs {
            format!("může být i dynamicky {verbs}, odkud: {}", marks.join("; "))
        } else {
            format!("may be {verbs} dynamically too, from: {}", marks.join("; "))
        });
    }
    if line.name_matches > 0 {
        let n = line.name_matches;
        parts.push(if cs {
            format!(
                "shoda jen podle jména na {}, ke kterým ho žádné pravidlo nepřiřadilo",
                cs_n(n, ("místě", "místech", "místech"))
            )
        } else {
            format!(
                "matched by name alone at {}, which no rule tied to it",
                plural(n, "place", "places")
            )
        });
    }
    format!("- {} — {}\n", code(name), parts.join("; "))
}

/// The `## Dynamic access` of an analyzed type's card (KTD11): one line per
/// member the run says anything of, then the sentence on the run's state of
/// XAML coverage. On every level of detail, and on every analyzed type's
/// card, a type no member of which has a marker included: the state is what
/// says whether that absence is evidence (R19).
pub fn dynamic_access(lang: Lang, analysis: &TypeAnalysis) -> String {
    let cs = lang == Lang::Cs;
    let ty = short(analysis.key.as_str());
    let mut s = format!("\n## {DYNAMIC_ACCESS}\n\n");
    s.push_str(if cs {
        "Členy, které může jiné místo číst, zapisovat nebo volat bez statického volání, jaké \
         tento korpus ukazuje — binding, statický člen, připojená vlastnost, setter nebo \
         obsluha v markupu, literál nebo `nameof` v C# —, každý na jednom řádku: co o něm \
         jedním slovem říká deklarace (člen rozhraní, virtuální, přepis, signatura obsluhy \
         události), každé místo s druhem, užitím a jistotou pravidla, a na kolika místech ho \
         jmenuje jen jméno, které k němu žádné pravidlo nepřiřadilo.\n\n"
    } else {
        "Members another place may read, write or call without a static call this corpus \
         shows — a binding, a static member, an attached property, a setter or a handler of \
         markup, a literal or a `nameof` of C# — one line each: what its declaration says in a \
         word (an interface's member, virtual, override, an event handler's signature), every \
         place with its kind, its use and how sure the rule is, and at how many places only \
         its name, which no rule tied to it, names it.\n\n"
    });
    if analysis.dynamic.lines.is_empty() {
        s.push_str(if cs {
            "Žádný člen tohoto typu nemá v tomto běhu značku dynamického volání.\n"
        } else {
            "No member of this type has a dynamic-call marker in this run.\n"
        });
    }
    for line in &analysis.dynamic.lines {
        s.push_str(&dynamic_line(ty, line, lang));
    }
    let _ = writeln!(s, "\n{}", markup_sentence(analysis.dynamic.state, lang));
    s
}
