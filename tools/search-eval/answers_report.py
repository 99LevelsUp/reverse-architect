"""The report of the answers: facts and forbidden claims by question, path and corpus (U8, KTD11).

`answers.py zprava` reads finished measurements of two paths and puts them side by side:

* the **card check** (`kontrola-karet`): does the corpus state the facts of a question;
* the **run with an agent** (`hodnot-odpovedi`): does the agent's free answer state them,
  per variant of the experiment and over its (three) runs.

The report is built once as a plain dict (`build_report`); `answers-report.json` is that
dict and `answers-report.md` is rendered from it (`render_markdown`). Every number is
counted while the dict is built, never while rendering (as in `report.py`), so both files
carry the same numbers by construction.

What the command refuses (before anything is written):

1. **A unit still waiting for the owner.** `review.review_state(...).missing`: a disputed
   unit without a stored decision, or a unit drawn into the owner's sample
   (`review.record_sampled`) without one. The error names the units.
2. **Inputs that cannot be compared.** Corpora over different revisions, or over a revision
   other than the one an item carries; items with different confirmation fingerprints;
   inputs judged by another judge, model or prompt version (the advice is to judge the
   older corpus again: the judge's cache makes that cheap).

What the report states, besides the tables:

* the assessment of a unit is the **model's** unless the owner decided it (then it says so);
  a judge of the agent's own family (the fallback) is marked on every row;
* the card check's **document sizes** next to each fact (the documents that carry the fact
  or own its place) and, per corpus, the cards that own the facts' places: the presence of
  a fact grows with the length of a card, so a reader must see the length;
* the **identity of every corpus** (fingerprint, revision of the sources, RA build from the
  runner's receipt, or "sestavení RA neznámé" when there is none);
* the agreement of the owner with the model on the undisputed sample, per measurement;
* the **calibration** of the judge for the pair judge + prompt version, or that there is none;
* what the measurement does not claim.
"""
from __future__ import annotations

import json
import os
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Mapping, Sequence

import calibration
import card_check
import review
from corpus import same_revision
from report import ROLE_DEVELOPER, ROLE_SUPPORT, write_text_atomic

REPORT_FORMAT_VERSION = 1
REPORT_KIND = "odpovedi"
JSON_FILE = "answers-report.json"
MARKDOWN_FILE = "answers-report.md"

PATH_CARDS = review.PATH_CARD_CHECK  # "kontrola karet"
PATH_AGENT = "běh s agentem"
_PATH_ORDER = {PATH_CARDS: 0, PATH_AGENT: 1}
_ROLE_ORDER = {ROLE_SUPPORT: 0, ROLE_DEVELOPER: 1}

SECTION_ACCEPTANCE = "akceptacni_polozka"
SECTION_CONTROL = "kontrolni_otazky"

ASSESSED_OWNER = "vlastnik"
ASSESSED_MODEL = "model"
ASSESSED_MIXED = "smisene"
ASSESSED_NONE = "zadne"

FACT_STATUSES = (card_check.FACT_PRESENT, card_check.FACT_MISSING, card_check.FACT_MISSING_UNBOUND,
                 card_check.FACT_PLACE_UNKNOWN, card_check.FACT_CONFLICT, card_check.FACT_DISPUTED,
                 card_check.FACT_NO_ANSWER)
CLAIM_STATUSES = (card_check.CLAIM_FOUND, card_check.CLAIM_ABSENT, card_check.CLAIM_DISPUTED,
                  card_check.CLAIM_UNBOUND, card_check.CLAIM_PLACE_UNKNOWN,
                  card_check.CLAIM_NO_ANSWER)

RECEIPT_UNKNOWN = "sestavení RA neznámé"
NO_CORPUS = "varianta bez korpusu"
REVIEW_ADVICE = ("Přehodnoť starší korpus aktuálním hodnotitelem (`answers.py kontrola-karet` "
                 "nebo `hodnot-odpovedi` znovu); díky cache hodnotitele je to levné.")
MAX_NAMED_UNITS = 10

NOT_CLAIMED = (
    "Přítomnost faktu v kartě roste s délkou karty: delší karta má víc příležitostí fakt "
    "uvést. Velikosti dokumentů jsou proto uvedeny u každého faktu a korpusu; tabulka je "
    "neváží a nenormalizuje.",
    "Karty vrstvy 2 se hodnotiteli prozradí tvarem (odlišují se od ostatních dokumentů), "
    "takže hodnotitel nečte všechny dokumenty naslepo stejně.",
    "Shoda vlastníka s modelem je shoda s jedním člověkem a jen na vzorku bez sporu; neříká, "
    "že jiný vlastník by rozhodl stejně, ani nic o jednotkách mimo vzorek.",
    "Posouzení bez sporu mimo vzorek je posouzení modelu, které člověk neviděl.",
    "Práh úspěchu se nestanoví: tabulky ukazují počty a stavy, ne verdikt, že korpus nebo "
    "agent obstál.",
    "Měří, zda odpověď nebo karta uvádí fakt (nebo zakázané tvrzení), ne správnost celé "
    "odpovědi ani to, zda by se podle ní dalo konat.",
    "Běh s agentem se čte ze tří běhů na variantu: „k/3 přítomen“ je počet běhů, ne "
    "pravděpodobnost.",
)


class ReportError(ValueError):
    """The report cannot be made from these inputs (Czech message for the operator)."""


# --- the inputs --------------------------------------------------------------------------

@dataclass(frozen=True)
class Source:
    """One finished measurement as the report reads it."""

    number: int
    path: Path
    loaded: review.LoadedMeasurement
    reviewed: review.ReviewedMeasurement
    agent: bool
    corpora: tuple[dict[str, Any], ...]

    @property
    def header(self) -> dict[str, Any]:
        return self.loaded.header

    @property
    def path_label(self) -> str:
        return PATH_AGENT if self.agent else PATH_CARDS

    def corpus_id(self, variant: str | None) -> str:
        if not self.agent:
            return self.corpora[0]["id"]
        for corpus in self.corpora:
            if corpus["varianta"] == variant:
                return corpus["id"]
        raise ReportError(f"Měření {self.path} má výsledek varianty „{variant}“, kterou "
                          "neuvádí mezi variantami pokusu.")


def _receipt_json(receipt: Mapping[str, Any] | None) -> dict[str, Any] | None:
    if not isinstance(receipt, Mapping):
        return None
    return {"verze": receipt.get("ra_version"), "volby": list(receipt.get("options") or ())}


def _corpora_of(number: int, loaded: review.LoadedMeasurement) -> tuple[dict[str, Any], ...]:
    """The corpus identities of a measurement: one for a card check, one per variant otherwise."""
    corpus, header = loaded.corpus, loaded.header
    pokus = header.get("pokus")
    if not isinstance(pokus, dict):
        receipt = header.get("ucenka")
        known = isinstance(receipt, dict)
        return ({
            "id": f"korpus-{number}", "cesta": PATH_CARDS, "mereni": loaded.id, "varianta": None,
            "bez_korpusu": False, "repo_revision": corpus.get("revision"),
            "zdroj_identity": corpus.get("label"),
            "otisk_korpusu": receipt.get("corpus_fingerprint") if known else None,
            "otisk_stromu": (receipt.get("tree_fingerprint") if known
                             else corpus.get("tree_fingerprint")),
            "sestaveni_ra": _receipt_json(receipt) if known else None},)
    out = []
    for variant in corpus.get("varianty") or ():
        receipt = variant.get("korpus")
        name = variant.get("nazev")
        out.append({
            "id": f"{pokus.get('id')}:{name}", "cesta": PATH_AGENT, "mereni": loaded.id,
            "varianta": name, "bez_korpusu": receipt is None,
            "repo_revision": (receipt or {}).get("base_revision", corpus.get("revision")),
            "zdroj_identity": corpus.get("label"),
            "otisk_korpusu": (receipt or {}).get("corpus_fingerprint"),
            "otisk_stromu": (receipt or {}).get("tree_fingerprint"),
            "sestaveni_ra": _receipt_json(receipt)})
    return tuple(out)


def load_sources(paths: Sequence[str | os.PathLike[str]], state_dir: str | os.PathLike[str]
                 ) -> list[Source]:
    """Read the measurements and apply the owner's decisions; refuse what cannot be compared."""
    if not paths:
        raise ReportError("Chybí měření, ze kterých se zpráva sestaví (--mereni).")
    seen: dict[str, Path] = {}
    sources: list[Source] = []
    for number, path in enumerate(paths, start=1):
        resolved = os.path.normcase(os.path.realpath(path))
        if resolved in seen:
            raise ReportError(f"Měření {path} je zadané víckrát.")
        seen[resolved] = Path(path)
        loaded = review.load_measurement(path)
        reviewed = review.review_state(loaded, state_dir)
        sources.append(Source(number=number, path=Path(path), loaded=loaded, reviewed=reviewed,
                              agent=isinstance(loaded.header.get("pokus"), dict),
                              corpora=_corpora_of(number, loaded)))
    _refuse_incomparable(sources)
    _refuse_undecided(sources)
    return sources


def _judge_key(source: Source) -> tuple[Any, Any, Any, Any]:
    """Who judged: the judge's id, its family, the model it reported and the prompt version."""
    judge = source.header.get("judge") or {}
    return (judge.get("id"), judge.get("rodina"), judge.get("model"),
            source.header.get("prompt_version"))


def _judge_text(key: tuple[Any, Any, Any, Any]) -> str:
    return f"hodnotitel {key[0]} (rodina {key[1]}), model {key[2]}, verze promptu {key[3]}"


def _refuse_incomparable(sources: Sequence[Source]) -> None:
    """Revisions, item fingerprints, judge: one of them different means a different measure."""
    revisions: list[tuple[str, str]] = []
    items: dict[str, tuple[tuple[Any, ...], str]] = {}
    changed: str | None = None
    for source in sources:
        name = source.path.name
        for corpus in source.corpora:
            revision = corpus.get("repo_revision")
            if not isinstance(revision, str) or not revision.strip():
                raise ReportError(f"Měření {name}: korpus {corpus['id']} nemá revizi zdrojáků.")
            revisions.append((f"korpus {corpus['id']} ({name})", revision))
        for item in source.header.get("items") or ():
            revisions.append((f"položka {item.get('id')} ({name})", str(item.get("commit"))))
            fingerprint = (item.get("commit"), item.get("otisk_potvrzeni"), item.get("kontrolni"),
                           item.get("dotaz"))
            known = items.setdefault(item.get("id"), (fingerprint, name))
            if known[0] != fingerprint and changed is None:
                changed = (
                    f"Položka {item.get('id')} má v měřeních {known[1]} a {name} různý otisk "
                    "(commit, otisk potvrzení, příznak kontrolní nebo text otázky); katalog se "
                    "mezi měřeními změnil, porovnání by měřilo různé položky.")
    first_what, first = revisions[0]
    for what, revision in revisions[1:]:
        if not same_revision(first, revision):
            raise ReportError(
                f"Zpráva se odmítá: {first_what} je nad revizí {first}, ale {what} nad "
                f"revizí {revision}. Korpusy nad jinou revizí nebo nad jinou revizí, než nese "
                "položka, se neporovnávají.")
    if changed is not None:
        raise ReportError(changed)
    keys = {_judge_key(s) for s in sources}
    if len(keys) > 1:
        described = "; ".join(f"{s.path.name}: {_judge_text(_judge_key(s))}" for s in sources)
        raise ReportError("Zpráva se odmítá: vstupy posoudil jiný hodnotitel, model nebo jiná "
                          f"verze promptu ({described}). {REVIEW_ADVICE}")


def _refuse_undecided(sources: Sequence[Source]) -> None:
    lines: list[str] = []
    total = 0
    for source in sources:
        for unit_id in source.reviewed.missing:
            total += 1
            if len(lines) < MAX_NAMED_UNITS:
                lines.append(f"{source.path.name}: jednotka {unit_id}")
    if total:
        more = f"\n  … a dalších {total - len(lines)}" if total > len(lines) else ""
        raise ReportError(
            f"Zpráva se nevytvoří: {total} sporných nebo vylosovaných jednotek čeká na "
            "rozhodnutí vlastníka (`answers.py kontrola`, vyplnit a načíst přes --nacti):\n  "
            + "\n  ".join(lines) + more)


# --- rows --------------------------------------------------------------------------------

def _texts(sources: Sequence[Source]) -> tuple[dict[str, str], dict[tuple[str, str], str]]:
    """Question texts and the texts of facts and claims, as the units carry them."""
    questions: dict[str, str] = {}
    claims: dict[tuple[str, str], str] = {}
    for source in sources:
        for unit in source.reviewed.units:
            if unit.question_text is not None:
                questions.setdefault(unit.question_id, unit.question_text)
            if unit.claim_text is not None:
                claims.setdefault((unit.question_id, unit.claim_id), unit.claim_text)
    return questions, claims


def _assessment(units: Sequence[card_check.UnitResult], same_family: bool) -> dict[str, Any]:
    """Who decided the units a result stands on: the model, or the owner (where one decided)."""
    judged = [u for u in units if u.model_label is not None]
    owner = sum(1 for u in judged if u.decided_by is not None)
    if not judged:
        kind = ASSESSED_NONE
    elif owner == len(judged):
        kind = ASSESSED_OWNER
    elif owner == 0:
        kind = ASSESSED_MODEL
    else:
        kind = ASSESSED_MIXED
    return {"druh": kind, "jednotek": len(judged), "vlastnik": owner,
            "zalozni_hodnotitel": same_family}


def _count(statuses: Sequence[str]) -> dict[str, int]:
    out: dict[str, int] = {}
    for status in statuses:
        out[status] = out.get(status, 0) + 1
    return dict(sorted(out.items()))


def _fact_rows(source: Source, kontrolni: Mapping[str, bool], claim_texts: Mapping[Any, str],
               same_family: bool) -> list[dict[str, Any]]:
    by_id = {u.id: u for u in source.reviewed.units}
    groups: dict[tuple[Any, ...], list[Any]] = {}
    for result in source.reviewed.results.facts:
        key = (result.question_id, result.fact_id, result.role,
               result.variant if source.agent else None)
        groups.setdefault(key, []).append(result)
    rows = []
    for (question_id, fact_id, role, variant), results in groups.items():
        results.sort(key=lambda r: (r.run is None, r.run))
        unit_ids = [i for r in results for i in (*r.unit_ids, *(c["unit_id"] for c in r.conflicts))]
        units = [by_id[i] for i in dict.fromkeys(unit_ids)]
        statuses = [r.status for r in results]
        row: dict[str, Any] = {
            "otazka": question_id, "kontrolni": kontrolni.get(question_id, False),
            "fakt": fact_id, "text": claim_texts.get((question_id, fact_id)), "role": role,
            "cesta": source.path_label, "korpus": source.corpus_id(variant),
            "stavy": _count(statuses), "posouzeni": _assessment(units, same_family),
            "behy": None, "behu": None, "pritomen": None, "bez_odpovedi": None, "stav": None}
        if source.agent:
            row.update(
                behy=[{"run": r.run, "stav": r.status} for r in results], behu=len(results),
                pritomen=statuses.count(card_check.FACT_PRESENT),
                bez_odpovedi=statuses.count(card_check.FACT_NO_ANSWER))
        else:
            row["stav"] = statuses[0]
        rows.append(row)
    return rows


def _claim_rows(source: Source, kontrolni: Mapping[str, bool], claim_texts: Mapping[Any, str],
                same_family: bool) -> list[dict[str, Any]]:
    by_id = {u.id: u for u in source.reviewed.units}
    groups: dict[tuple[Any, ...], list[Any]] = {}
    for result in source.reviewed.results.claims:
        key = (result.question_id, result.claim_id, result.variant if source.agent else None)
        groups.setdefault(key, []).append(result)
    rows = []
    for (question_id, claim_id, variant), results in groups.items():
        results.sort(key=lambda r: (r.run is None, r.run))
        units = [by_id[i] for i in dict.fromkeys(i for r in results for i in r.unit_ids)]
        statuses = [r.status for r in results]
        row: dict[str, Any] = {
            "otazka": question_id, "kontrolni": kontrolni.get(question_id, False),
            "tvrzeni": claim_id, "text": claim_texts.get((question_id, claim_id)),
            "fakty": list(results[0].fact_ids), "cesta": source.path_label,
            "korpus": source.corpus_id(variant), "stavy": _count(statuses),
            "posouzeni": _assessment(units, same_family),
            "behy": None, "behu": None, "nalezeno": None, "bez_odpovedi": None, "stav": None}
        if source.agent:
            row.update(
                behy=[{"run": r.run, "stav": r.status} for r in results], behu=len(results),
                nalezeno=statuses.count(card_check.CLAIM_FOUND),
                bez_odpovedi=statuses.count(card_check.CLAIM_NO_ANSWER))
        else:
            row["stav"] = statuses[0]
        rows.append(row)
    return rows


def _corpus_order(sources: Sequence[Source]) -> dict[str, int]:
    return {c["id"]: n for n, c in enumerate(c for s in sources for c in s.corpora)}


def _sorted_rows(rows: list[dict[str, Any]], question_order: Mapping[str, int],
                 corpus_order: Mapping[str, int], subject: str) -> list[dict[str, Any]]:
    return sorted(rows, key=lambda r: (
        question_order.get(r["otazka"], len(question_order)), r[subject],
        _ROLE_ORDER.get(r.get("role"), 9), _PATH_ORDER[r["cesta"]], corpus_order[r["korpus"]]))


def _section(rows: Sequence[dict[str, Any]], kontrolni: bool) -> list[dict[str, Any]]:
    return [r for r in rows if r["kontrolni"] is kontrolni]


def _summary(section: str, facts: Sequence[dict[str, Any]], corpus_order: Mapping[str, int]
             ) -> list[dict[str, Any]]:
    """Counts of each fact status per path and corpus (an agent's run counts once)."""
    totals: dict[tuple[str, str], dict[str, int]] = {}
    for row in facts:
        counts = totals.setdefault((row["cesta"], row["korpus"]), {s: 0 for s in FACT_STATUSES})
        for status, number in row["stavy"].items():
            counts[status] = counts.get(status, 0) + number
    return [{"sekce": section, "cesta": path, "korpus": corpus, "stavy": counts}
            for (path, corpus), counts in sorted(
                totals.items(), key=lambda t: (_PATH_ORDER[t[0][0]], corpus_order[t[0][1]]))]


def _sizes(source: Source) -> dict[str, Any]:
    """The sizes of the documents of each fact and the cards that own the places (card check)."""
    by_id = {u.id: u for u in source.reviewed.units}
    facts: dict[tuple[str, str], dict[str, dict[str, Any]]] = {}
    owners: dict[str, dict[str, Any]] = {}
    for result in source.reviewed.results.facts:
        docs = facts.setdefault((result.question_id, result.fact_id), {})
        for unit_id in result.unit_ids:
            unit = by_id[unit_id]
            entry = docs.setdefault(unit.document, {
                "id": unit.document, "slot": unit.slot, "cesta": unit.document_path,
                "bajtu": unit.document_bytes, "karta_vlastnika": bool(unit.owner_card)})
            if unit.owner_card:
                entry["karta_vlastnika"] = True
                owners.setdefault(unit.document, {"id": unit.document, "cesta": unit.document_path,
                                                  "bajtu": unit.document_bytes})
    fact_entries = []
    for (question_id, fact_id), docs in facts.items():
        listed = list(docs.values())
        known = bool(listed) and all(d["bajtu"] is not None for d in listed)
        fact_entries.append({"otazka": question_id, "fakt": fact_id, "dokumenty": listed,
                             "bajtu_celkem": sum(d["bajtu"] for d in listed) if known else None})
    cards = list(owners.values())
    known_cards = bool(cards) and all(c["bajtu"] is not None for c in cards)
    return {"korpus": source.corpora[0]["id"], "fakta": fact_entries,
            "karty_vlastniku_mist": {
                "pocet": len(cards), "karty": cards,
                "bajtu": sum(c["bajtu"] for c in cards) if known_cards else None}}


def _agreement(source: Source) -> dict[str, Any]:
    """The owner's agreement with the model on the undisputed units he checked (the sample)."""
    groups: dict[str, list[card_check.UnitResult]] = {}
    for unit in source.reviewed.units:
        if unit.decided_by is not None and unit.model_label is not None:
            groups.setdefault(review.decision_key(unit), []).append(unit)
    sample = agreed = disputed = 0
    for units in groups.values():
        if any(u.disputed for u in units):
            disputed += 1
            continue
        sample += 1
        agreed += all(u.final_label == u.model_label for u in units)
    return {"mereni": source.loaded.id, "soubor": source.path.name, "cesta": source.path_label,
            "vzorek": sample, "shodne": agreed, "rozhodnuti_sporne": disputed}


def _calibration(directory: str | os.PathLike[str] | None, judge: Mapping[str, Any],
                 prompt_version: str, set_fingerprint: str | None) -> dict[str, Any]:
    base: dict[str, Any] = {"nalezena": False, "hodnotitel": judge.get("id"),
                            "prompt_version": prompt_version,
                            "otisk_sady_pozadovany": set_fingerprint}
    if directory is None:
        return base
    found = _find_calibration(directory, str(judge.get("id")), prompt_version, set_fingerprint)
    if found is None:
        return base
    path, data = found
    return {**base, "nalezena": True, "soubor": path.name, "otisk_sady": data.get("otisk_sady"),
            "jednotek": data.get("jednotek"), "shoda": data.get("shoda"),
            "pokryti": data.get("pokryti"), "sporne": data.get("sporne"),
            "nebezpecne": data.get("nebezpecne")}


def _find_calibration(directory: str | os.PathLike[str], judge_id: str, prompt_version: str,
                      set_fingerprint: str | None) -> tuple[Path, dict[str, Any]] | None:
    """`calibration.find_calibration`; without a set to check, any set of the pair will do."""
    if set_fingerprint is not None:
        return calibration.find_calibration(directory, judge_id, prompt_version, set_fingerprint)
    root = Path(directory)
    if not root.is_dir():
        return None
    found = []
    for path in root.glob("*.json"):
        try:
            data = calibration.load_result(path)
        except calibration.CalibrationResultError:
            continue
        if calibration.matches(data, judge_id, prompt_version, data.get("otisk_sady")):
            found.append((path.stat().st_mtime, path.name, path, data))
    if not found:
        return None
    _, _, path, data = max(found, key=lambda f: (f[0], f[1]))
    return path, data


# --- the report --------------------------------------------------------------------------

def build_report(paths: Sequence[str | os.PathLike[str]], state_dir: str | os.PathLike[str],
                 calibration_dir: str | os.PathLike[str] | None = None, *,
                 set_fingerprint: str | None = None) -> dict[str, Any]:
    """The whole report as a JSON-ready dict; raises `ReportError` when it must not be made."""
    sources = load_sources(paths, state_dir)
    judge = dict(sources[0].header.get("judge") or {})
    prompt_version = sources[0].header.get("prompt_version")
    same_family = bool(judge.get("stejna_rodina"))
    question_texts, claim_texts = _texts(sources)
    kontrolni: dict[str, bool] = {}
    question_order: dict[str, int] = {}
    items: dict[str, dict[str, Any]] = {}
    for source in sources:
        for item in source.header.get("items") or ():
            kontrolni[item["id"]] = bool(item.get("kontrolni"))
            question_order.setdefault(item["id"], len(question_order))
            items.setdefault(item["id"], dict(item))
    corpus_order = _corpus_order(sources)

    facts: list[dict[str, Any]] = []
    claims: list[dict[str, Any]] = []
    for source in sources:
        facts += _fact_rows(source, kontrolni, claim_texts, same_family)
        claims += _claim_rows(source, kontrolni, claim_texts, same_family)
    facts = _sorted_rows(facts, question_order, corpus_order, "fakt")
    claims = _sorted_rows(claims, question_order, corpus_order, "tvrzeni")

    sections = {}
    summary: list[dict[str, Any]] = []
    for name, flag in ((SECTION_ACCEPTANCE, False), (SECTION_CONTROL, True)):
        fact_rows, claim_rows = _section(facts, flag), _section(claims, flag)
        sections[name] = {
            "otazky": [{"id": q, "text": question_texts.get(q)} for q in question_order
                       if kontrolni.get(q) is flag],
            "fakta": fact_rows, "tvrzeni": claim_rows}
        summary += _summary(name, fact_rows, corpus_order)

    no_answer = [{"mereni": s.loaded.id, "pokus": s.header["pokus"].get("id"), **{
        "otazka": r.get("otazka"), "run": r.get("run"), "varianta": r.get("varianta"),
        "duvod": r.get("duvod")}}
        for s in sources if s.agent for r in s.header["pokus"].get("bez_odpovedi") or ()]

    return {
        "kind": REPORT_KIND,
        "hodnotitel": judge,
        "prompt_version": prompt_version,
        "zalozni_hodnotitel": same_family,
        "revize": sources[0].corpora[0]["repo_revision"],
        "polozky": [items[q] for q in question_order],
        "vstupy": [_input_json(s) for s in sources],
        "korpusy": [c for s in sources for c in s.corpora],
        SECTION_ACCEPTANCE: sections[SECTION_ACCEPTANCE],
        SECTION_CONTROL: sections[SECTION_CONTROL],
        "souhrn": summary,
        "behy_bez_odpovedi": no_answer,
        "velikosti": [_sizes(s) for s in sources if not s.agent],
        "shoda_vlastnika": [_agreement(s) for s in sources],
        "kalibrace": _calibration(calibration_dir, judge, str(prompt_version), set_fingerprint),
        "netvrdi": list(NOT_CLAIMED),
    }


def _input_json(source: Source) -> dict[str, Any]:
    out: dict[str, Any] = {"mereni": source.loaded.id, "soubor": source.path.name,
                           "cesta": source.path_label,
                           "korpusy": [c["id"] for c in source.corpora],
                           "rozhodnuti_vlastnika": len(source.reviewed.decided)}
    pokus = source.header.get("pokus")
    if isinstance(pokus, dict):
        out["pokus"] = {"id": pokus.get("id"), "stav": pokus.get("stav"),
                        "nedokonceny": bool(pokus.get("nedokonceny")),
                        "role": pokus.get("role"), "rodina_agenta": pokus.get("rodina_agenta")}
    return out


def _sorted(value: Any) -> Any:
    if isinstance(value, dict):
        return {k: _sorted(value[k]) for k in sorted(value)}
    if isinstance(value, (list, tuple)):
        return [_sorted(v) for v in value]
    return value


def report_json_text(data: Mapping[str, Any]) -> str:
    """The JSON text: `format_version` first, every other key sorted, at every level."""
    ordered = {"format_version": REPORT_FORMAT_VERSION, **_sorted(dict(data))}
    return json.dumps(ordered, ensure_ascii=False, indent=2) + "\n"


def write_report(directory: str | os.PathLike[str], data: Mapping[str, Any]) -> tuple[Path, Path]:
    """Write both files atomically into `directory` (created when missing)."""
    target = Path(directory)
    target.mkdir(parents=True, exist_ok=True)
    json_path, md_path = target / JSON_FILE, target / MARKDOWN_FILE
    write_text_atomic(json_path, report_json_text(data))
    write_text_atomic(md_path, render_markdown(data))
    return json_path, md_path


# --- Markdown ----------------------------------------------------------------------------

_ASSESSED_TEXT = {ASSESSED_OWNER: "rozhodl vlastník", ASSESSED_MODEL: "posouzení modelu",
                  ASSESSED_NONE: "—"}
_SAME_FAMILY_MARK = "záložní hodnotitel stejné rodiny"


def _cell(text: Any) -> str:
    return str(text).replace("|", "\\|").replace("\n", " ")


def _table(header: Sequence[str], rows: Sequence[Sequence[Any]]) -> list[str]:
    lines = ["| " + " | ".join(header) + " |", "|" + "---|" * len(header)]
    lines += ["| " + " | ".join(_cell(c) for c in row) + " |" for row in rows]
    return lines


def _assessed(posouzeni: Mapping[str, Any]) -> str:
    if posouzeni["druh"] == ASSESSED_MIXED:
        text = (f"vlastník {posouzeni['vlastnik']} z {posouzeni['jednotek']} jednotek, "
                "zbytek posouzení modelu")
    else:
        text = _ASSESSED_TEXT[posouzeni["druh"]]
    if posouzeni["zalozni_hodnotitel"] and posouzeni["druh"] != ASSESSED_NONE:
        text += f" ({_SAME_FAMILY_MARK})"
    return text


def _others(stavy: Mapping[str, int], leading: str) -> str:
    return ", ".join(f"{s} {n}" for s, n in stavy.items() if s != leading)


def _fact_result(row: Mapping[str, Any]) -> str:
    if row["behu"] is None:
        return row["stav"]
    text = f"{row['pritomen']}/{row['behu']} přítomen"
    rest = _others(row["stavy"], card_check.FACT_PRESENT)
    return f"{text}; {rest}" if rest else text


def _claim_result(row: Mapping[str, Any]) -> str:
    if row["behu"] is None:
        return row["stav"]
    text = f"nalezeno {row['nalezeno']}/{row['behu']}"
    rest = _others(row["stavy"], card_check.CLAIM_FOUND)
    return f"{text}; {rest}" if rest else text


def render_markdown(data: Mapping[str, Any]) -> str:
    """Render the report dict (never counting anything)."""
    judge = data["hodnotitel"]
    out: list[str] = ["# Zpráva odpovědí", ""]
    out += [f"Revize zdrojáků: {data['revize']}. Hodnotitel: {judge.get('id')} (model "
            f"{judge.get('model')}, rodina {judge.get('rodina')}), verze promptu "
            f"{data['prompt_version']}.", ""]
    if data["zalozni_hodnotitel"]:
        out += [f"> Pozor: posouzení udělal {_SAME_FAMILY_MARK}: model stejné rodiny jako "
                "agent. Není to nezávislý pohled a každý řádek je tak označený.", ""]
    out += _render_inputs(data)
    out += _render_section("Akceptační položka", data[SECTION_ACCEPTANCE])
    out += _render_section("Kontrolní otázky", data[SECTION_CONTROL])
    out += _render_summary(data)
    out += _render_no_answer(data)
    out += _render_sizes(data)
    out += _render_agreement(data)
    out += _render_calibration(data)
    out += ["## Co toto měření netvrdí", ""] + [f"- {s}" for s in data["netvrdi"]] + [""]
    return "\n".join(out)


def _render_inputs(data: Mapping[str, Any]) -> list[str]:
    out = ["## Vstupy a identita korpusů", ""]
    out += _table(["měření", "soubor", "cesta", "korpusy", "rozhodnutí vlastníka"],
                  [[i["mereni"], i["soubor"], i["cesta"], ", ".join(i["korpusy"]),
                    i["rozhodnuti_vlastnika"]] for i in data["vstupy"]])
    out += ["", "Položky: " + "; ".join(
        f"{p['id']} ({'kontrolní' if p['kontrolni'] else 'akceptační'}, commit {p['commit']})"
        for p in data["polozky"]), ""]
    for i in data["vstupy"]:
        pokus = i.get("pokus")
        if pokus and pokus["nedokonceny"]:
            out += [f"> Pokus {pokus['id']} je nedokončený (stav „{pokus['stav']}“); odpovědi "
                    "jsou jen z běhů, které stihl.", ""]
    rows = []
    for c in data["korpusy"]:
        if c["bez_korpusu"]:
            build = NO_CORPUS
        elif c["sestaveni_ra"] is None:
            build = RECEIPT_UNKNOWN
        else:
            options = " ".join(c["sestaveni_ra"]["volby"]) or "bez voleb"
            build = f"RA {c['sestaveni_ra']['verze']}; {options}"
        rows.append([c["id"], c["cesta"], c["otisk_korpusu"] or "neznámý",
                     c["otisk_stromu"] or "neznámý", c["repo_revision"], build])
    out += _table(["korpus", "cesta", "otisk korpusu", "otisk stromu", "repo_revision",
                   "sestavení RA"], rows)
    return out + [""]


def _render_section(title: str, section: Mapping[str, Any]) -> list[str]:
    out = [f"## {title}", ""]
    if not section["otazky"]:
        return out + ["Žádná otázka.", ""]
    out += ["Otázky: " + "; ".join(f"{q['id']}: {q['text']}" for q in section["otazky"]), ""]
    out += ["### Fakta (otázka × fakt × role × cesta × korpus)", ""]
    out += _table(["otázka", "fakt", "role", "cesta", "korpus", "výsledek", "posouzení"],
                  [[r["otazka"], f"{r['fakt']}: {r['text'] or ''}", r["role"], r["cesta"],
                    r["korpus"], _fact_result(r), _assessed(r["posouzeni"])]
                   for r in section["fakta"]])
    out += ["", "### Zakázaná tvrzení (nalezeno / nenalezeno)", ""]
    if section["tvrzeni"]:
        out += _table(["otázka", "tvrzení", "cesta", "korpus", "výsledek", "posouzení"],
                      [[r["otazka"], f"{r['tvrzeni']}: {r['text'] or ''}", r["cesta"],
                        r["korpus"], _claim_result(r), _assessed(r["posouzeni"])]
                       for r in section["tvrzeni"]])
    else:
        out += ["Žádná zakázaná tvrzení."]
    return out + [""]


def _render_summary(data: Mapping[str, Any]) -> list[str]:
    out = ["## Souhrn stavů faktů", "",
           "Stavy „chybí, bez vázaného bloku“, „chybí, hodnotitel nenašel“ a „místo nenalezeno "
           "v korpusu“ jsou tři různé věci a počítají se zvlášť. U běhů s agentem se počítá každý "
           "běh.", ""]
    if not data["souhrn"]:
        return out + ["Žádné řádky.", ""]
    names = {SECTION_ACCEPTANCE: "akceptační", SECTION_CONTROL: "kontrolní"}
    out += _table(["sekce", "cesta", "korpus", *FACT_STATUSES],
                  [[names[r["sekce"]], r["cesta"], r["korpus"],
                    *(r["stavy"].get(s, 0) for s in FACT_STATUSES)] for r in data["souhrn"]])
    return out + [""]


def _render_no_answer(data: Mapping[str, Any]) -> list[str]:
    out = ["## Běhy agenta bez odpovědi", ""]
    if not data["behy_bez_odpovedi"]:
        return out + ["Žádné.", ""]
    out += ["Fakta těchto běhů se počítají jako chybějící („chybí, bez odpovědi“), ale jsou "
            "tu zvlášť.", ""]
    out += _table(["pokus", "otázka", "varianta", "běh", "důvod"],
                  [[r["pokus"], r["otazka"], r["varianta"], r["run"], r["duvod"]]
                   for r in data["behy_bez_odpovedi"]])
    return out + [""]


def _bytes(value: Any) -> str:
    return "neuchováno" if value is None else str(value)


def _render_sizes(data: Mapping[str, Any]) -> list[str]:
    out = ["## Velikosti dokumentů (bajty)", "",
           "U každého faktu dokumenty, které ho nesou nebo vlastní jeho místo (karta vlastníka "
           "místa je označená). Přítomnost faktu s délkou karty roste.", ""]
    if not data["velikosti"]:
        return out + ["Žádná kontrola karet na vstupu.", ""]
    for corpus in data["velikosti"]:
        out += [f"### {corpus['korpus']}", ""]
        rows = []
        for fact in corpus["fakta"]:
            docs = "; ".join(
                f"{d['id']} ({d['slot']}{', karta vlastníka' if d['karta_vlastnika'] else ''}): "
                f"{_bytes(d['bajtu'])}" for d in fact["dokumenty"]) or "žádný dokument"
            rows.append([fact["otazka"], fact["fakt"], docs, _bytes(fact["bajtu_celkem"])])
        out += _table(["otázka", "fakt", "dokumenty", "celkem"], rows)
        owners = corpus["karty_vlastniku_mist"]
        out += ["", f"Karty vlastníků míst: {owners['pocet']}, dohromady "
                    f"{_bytes(owners['bajtu'])} bajtů."]
        out += [f"- {c['id']}: {_bytes(c['bajtu'])}" for c in owners["karty"]] + [""]
    return out


def _render_agreement(data: Mapping[str, Any]) -> list[str]:
    out = ["## Shoda vlastníka s modelem", "",
           "Na vzorku bez sporu: kolik z jednotek, které vlastník zkontroloval, rozhodl stejně "
           "jako model. Sporné jednotky rozhodl vlastník jako rozhodčí a do shody se nepočítají. "
           "Je to shoda s jedním člověkem.", ""]
    out += _table(["měření", "cesta", "vzorek bez sporu", "shodně", "sporné rozhodnuté"],
                  [[a["soubor"], a["cesta"], a["vzorek"],
                    f"{a['shodne']}/{a['vzorek']}" if a["vzorek"] else "bez vzorku",
                    a["rozhodnuti_sporne"]] for a in data["shoda_vlastnika"]])
    return out + [""]


def _render_calibration(data: Mapping[str, Any]) -> list[str]:
    cal = data["kalibrace"]
    out = ["## Kalibrace hodnotitele", ""]
    if not cal["nalezena"]:
        return out + [f"Kalibrace pro hodnotitele {cal['hodnotitel']} a verzi promptu "
                      f"{cal['prompt_version']} chybí; štítky hodnotitele nejsou ověřené na "
                      "zmrazené sadě vět.", ""]
    danger = cal.get("nebezpecne") or {}
    out += [f"Kalibrace pro hodnotitele {cal['hodnotitel']} a verzi promptu "
            f"{cal['prompt_version']}: soubor {cal['soubor']}, otisk sady {cal['otisk_sady']}.",
            f"- Jednotek: {cal['jednotek']}; shoda: {cal['shoda']}; pokrytí: {cal['pokryti']}; "
            f"sporných: {cal['sporne']}."]
    for key, value in sorted(danger.items()):
        out += [f"- {key}: {value.get('pocet') if isinstance(value, dict) else value}"]
    return out + [""]
