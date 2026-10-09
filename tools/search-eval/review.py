"""The owner's review of the card check (U7): disputed units plus a random sample, then the decisions.

The judge marks some verdicts as disputed. Before a report is made the owner
goes through all of them and through a random sample of the undisputed ones.
Two steps, both in `answers.py kontrola`:

* `build_review` / `write_review` lay out the file to check: every disputed
  unit and a sample of the others, stratified by the judge's label (per label
  10 %, rounded up, at least 5, at most all of them), drawn with
  `random.Random(seed)`; the seed is written into the file. Units the owner
  has already decided (a stored decision under the unit's key) are skipped.
  Next to the JSON (filled in by the owner) an `.md` for reading is written
  with the same row numbers.
* `ingest_review` reads the filled file back: validates the values by the kind
  of the row, refuses rows whose key is not in the measurement, stores the
  decisions (atomically) in `rozhodnuti.json` of the state directory and the
  owner's suggestions of forbidden claims (field `nepravda`) in a file beside
  the review file. The catalog is never touched. A partly filled file can be
  loaded again and again; a decision is stored once per key.

Decision key (KTD7): the normalized text of everything the review showed the
owner (the whole document for a card, a map or an agent's answer; the bound
blocks for a detail), the document id, the kind, id and text of the fact or
the claim, and the question. The citation is not part of it, and neither are
the judge or the prompt version: the owner's judgement does not depend on who
asked the question first. A changed text of the corpus or wording of a fact
changes the key, and the unit is shown again.

`review_state` is what the report reads: the measurement's units with the
owner's decisions applied, the results derived from them again and the units
that are still disputed and have no decision.
"""
from __future__ import annotations

import hashlib
import json
import math
import os
import random
import tempfile
import unicodedata
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Iterable, Sequence

from card_check import (LABEL_ABSENT, LABEL_CONTAINS, LABEL_NOT_CONTAINS, LABEL_PRESENT,
                        LABEL_UNKNOWN, MEASUREMENT_FORMAT_VERSION, MODE_BOUND, MODE_WHOLE,
                        STATE_NO_BOUND_BLOCK, STATUS_DONE, ClaimPlan, FactPlan, Plan, Results,
                        UnitResult, derive_results, FACT_PLACE_UNKNOWN, CLAIM_PLACE_UNKNOWN,
                        FACT_NO_ANSWER, CLAIM_NO_ANSWER)
from judge import KIND_FACT, KIND_FORBIDDEN

REVIEW_FORMAT_VERSION = 1
DECISIONS_FORMAT_VERSION = 1
DECISIONS_FILE = "rozhodnuti.json"
PROPOSALS_FORMAT_VERSION = 1
PROPOSALS_SUFFIX = ".navrhy.json"
SAMPLED_FORMAT_VERSION = 1
SAMPLED_FILE = "vylosovano.json"

DECIDED_BY_OWNER = "vlastník"

PATH_CARD_CHECK = "kontrola karet"
PATH_AGENT_ANSWER = "odpověď agenta"  # not produced yet; the field is here for the later unit

ROW_FACT = "fakt"
ROW_CLAIM = "zakazane_tvrzeni"

SAMPLE_SHARE = 0.10
SAMPLE_MINIMUM = 5

# What the owner may write, by the kind of the row (normalized spelling -> label).
FACT_VALUES = {"vyjadruje": LABEL_PRESENT, "nevyjadruje": LABEL_ABSENT,
               "nelze posoudit": LABEL_UNKNOWN}
CLAIM_VALUES = {"obsahuje": LABEL_CONTAINS, "neobsahuje": LABEL_NOT_CONTAINS,
                "nelze posoudit": LABEL_UNKNOWN}


class ReviewError(ValueError):
    """The review cannot be made or loaded (Czech message for the operator)."""


# --- text, keys -------------------------------------------------------------------------

def normalize_text(text: str) -> str:
    """Unicode NFC, runs of whitespace collapsed to one space, trimmed."""
    return " ".join(unicodedata.normalize("NFC", text).split())


def _fold(value: str) -> str:
    """A typed decision for comparison: no diacritics, lower case, single spaces."""
    decomposed = unicodedata.normalize("NFD", value)
    stripped = "".join(c for c in decomposed if not unicodedata.combining(c))
    return " ".join(stripped.casefold().split())


def shown_text(unit: UnitResult) -> str:
    """What the review shows the owner of the unit: the whole document, or the bound blocks."""
    if unit.mode == MODE_WHOLE:
        if unit.document_text is None:
            raise ReviewError(f"Jednotka {unit.id}: měření neuchovalo text dokumentu; "
                              "kontrolu karet je třeba spustit znovu.")
        return unit.document_text
    if unit.mode == MODE_BOUND:
        if unit.blocks is None:
            raise ReviewError(f"Jednotka {unit.id}: měření neuchovalo vázané bloky.")
        return "\n\n".join(block["text"] for block in unit.blocks)
    raise ReviewError(f"Jednotka {unit.id}: neznámý režim „{unit.mode}“.")


def decision_key(unit: UnitResult) -> str:
    """KTD7: the key of the owner's decision about a unit (see the module docstring)."""
    if unit.question_text is None or unit.claim_text is None:
        raise ReviewError(f"Jednotka {unit.id}: měření neuchovalo text otázky a tvrzení; "
                          "kontrolu karet je třeba spustit znovu.")
    payload = [normalize_text(shown_text(unit)), unit.document, unit.kind, unit.claim_id,
               normalize_text(unit.claim_text), normalize_text(unit.question_text)]
    encoded = json.dumps(payload, ensure_ascii=False).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()[:32]


def _reviewable(unit: UnitResult) -> bool:
    """A unit the judge assessed; a unit without a block was never judged."""
    return unit.state != STATE_NO_BOUND_BLOCK and unit.model_label is not None


def _row_kind(unit: UnitResult) -> str:
    return ROW_FACT if unit.kind == KIND_FACT else ROW_CLAIM


def _allowed(kind: str) -> dict[str, str]:
    return FACT_VALUES if kind == KIND_FACT else CLAIM_VALUES


# --- reading a measurement ---------------------------------------------------------------

@dataclass(frozen=True)
class LoadedMeasurement:
    id: str
    path: Path
    status: str
    corpus: dict[str, Any]
    units: tuple[UnitResult, ...]
    results: dict[str, Any] | None
    # Every top-level key of the file except `units` and `results` (the judge, the prompt
    # version, the items and an answer check's `pokus`), for the report that compares inputs.
    header: dict[str, Any] = field(default_factory=dict)


def unit_from_json(data: dict[str, Any]) -> UnitResult:
    try:
        blocks = data["blocks"]
        return UnitResult(
            id=data["id"], question_id=data["question_id"], kind=data["kind"],
            claim_id=data["claim_id"], role=data["role"], document=data["document"],
            slot=data["slot"], title=data["title"], mode=data["mode"],
            blocks=None if blocks is None else tuple(blocks), block_count=data["block_count"],
            state=data["state"], note=data["note"], run_labels=tuple(data["run_labels"]),
            quote=data["quote"], partial=data["partial"], binding=data["binding"],
            disputed=data["disputed"], dispute_reason=data["dispute_reason"],
            model_label=data["model_label"], final_label=data["final_label"],
            decided_by=data["decided_by"], question_text=data.get("question_text"),
            claim_text=data.get("claim_text"), document_path=data.get("document_path"),
            document_text=data.get("document_text"),
            places=tuple(data.get("places") or ()), variant=data.get("variant"),
            run=data.get("run"), document_bytes=data.get("document_bytes"),
            owner_card=data.get("owner_card"))
    except (KeyError, TypeError) as exc:
        raise ReviewError(f"Jednotka v měření je neúplná (chybí {exc}).") from None


def load_measurement(path: str | os.PathLike[str]) -> LoadedMeasurement:
    """Read `measurement.json`; anything but a finished measurement is refused."""
    file = Path(path)
    try:
        data = json.loads(file.read_text(encoding="utf-8"))
    except OSError as exc:
        raise ReviewError(f"Měření {file} nejde načíst: {exc}") from None
    except ValueError as exc:
        raise ReviewError(f"Měření {file} není platný JSON: {exc}") from None
    if not isinstance(data, dict) or data.get("format_version") != MEASUREMENT_FORMAT_VERSION:
        raise ReviewError(f"Měření {file} nemá podporovaný formát "
                          f"(čekám format_version {MEASUREMENT_FORMAT_VERSION}).")
    if data.get("status") != STATUS_DONE:
        raise ReviewError(f"Měření {file} není dokončené (stav „{data.get('status')}“); "
                          "kontrola vlastníka čeká na hotové měření.")
    if not isinstance(data.get("id"), str) or not isinstance(data.get("units"), list):
        raise ReviewError(f"Měření {file} je poškozené (chybí id nebo jednotky).")
    units = tuple(unit_from_json(u) for u in data["units"])
    corpus = data.get("corpus") if isinstance(data.get("corpus"), dict) else {}
    header = {k: v for k, v in data.items() if k not in ("units", "results")}
    return LoadedMeasurement(id=data["id"], path=file, status=data["status"], corpus=corpus,
                             units=units, results=data.get("results"), header=header)


# --- stored decisions --------------------------------------------------------------------

def decisions_path(state_dir: str | os.PathLike[str]) -> Path:
    return Path(state_dir) / DECISIONS_FILE


def load_decisions(state_dir: str | os.PathLike[str]) -> dict[str, dict[str, Any]]:
    """The stored decisions by key (empty when there are none yet)."""
    path = decisions_path(state_dir)
    if not path.is_file():
        return {}
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as exc:
        raise ReviewError(f"Rozhodnutí {path} nejdou načíst: {exc}") from None
    if (not isinstance(data, dict) or data.get("format_version") != DECISIONS_FORMAT_VERSION
            or not isinstance(data.get("rozhodnuti"), dict)):
        raise ReviewError(f"Rozhodnutí {path} nemají podporovaný formát.")
    return data["rozhodnuti"]


def _write_text_atomic(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(dir=path.parent, prefix=f".{path.name}.", suffix=".tmp")
    try:
        with os.fdopen(fd, "w", encoding="utf-8", newline="\n") as handle:
            handle.write(text)
        os.replace(tmp, path)
    except BaseException:
        if os.path.exists(tmp):
            os.unlink(tmp)
        raise


def _write_json_atomic(path: Path, data: dict[str, Any]) -> None:
    _write_text_atomic(path, json.dumps(data, ensure_ascii=False, indent=2) + "\n")


def save_decisions(state_dir: str | os.PathLike[str], decisions: dict[str, dict[str, Any]]) -> None:
    _write_json_atomic(decisions_path(state_dir),
                       {"format_version": DECISIONS_FORMAT_VERSION, "rozhodnuti": decisions})


# --- laying out the review ---------------------------------------------------------------

@dataclass(frozen=True)
class ReviewRow:
    number: int
    key: str
    disputed: bool
    kind: str  # unit.kind of the units under the key
    measurement_id: str
    unit_ids: tuple[str, ...]
    data: dict[str, Any]  # the row as it is written (without `radek`)

    def to_json(self) -> dict[str, Any]:
        return {"radek": self.number, **self.data}


@dataclass(frozen=True)
class Review:
    seed: int
    measurements: tuple[tuple[str, str], ...]  # (id, path)
    rows: tuple[ReviewRow, ...]
    sample_sizes: dict[str, tuple[int, int]]  # label -> (population, drawn)

    @property
    def disputed_count(self) -> int:
        return sum(1 for row in self.rows if row.disputed)

    def to_json(self) -> dict[str, Any]:
        return {
            "format_version": REVIEW_FORMAT_VERSION,
            "seminko": self.seed,
            "mereni": [{"id": mid, "cesta": path} for mid, path in self.measurements],
            "vzorek": {label: {"bezsporne": total, "ve_vzorku": drawn}
                       for label, (total, drawn) in sorted(self.sample_sizes.items())},
            "radky": [row.to_json() for row in self.rows],
        }


def default_seed(measurement_ids: Iterable[str]) -> int:
    """A deterministic seed from the measurements (the same measurements, the same sample)."""
    digest = hashlib.sha256("\n".join(sorted(measurement_ids)).encode("utf-8")).hexdigest()
    return int(digest[:8], 16)


def sample_size(population: int) -> int:
    """10 % of the undisputed units of a label, rounded up, at least 5, at most all."""
    return min(population, max(SAMPLE_MINIMUM, math.ceil(population * SAMPLE_SHARE)))


def _row_data(measurement: LoadedMeasurement, units: Sequence[UnitResult], key: str,
              disputed: bool) -> dict[str, Any]:
    unit = units[0]
    allowed = list(_allowed(unit.kind))
    whole = unit.mode == MODE_WHOLE
    reasons = [u.dispute_reason for u in units if u.disputed and u.dispute_reason]
    return {
        "klic": key,
        "druh": _row_kind(unit),
        "sporne": disputed,
        "duvod_sporu": reasons[0] if reasons else None,
        "otazka": {"id": unit.question_id, "text": unit.question_text},
        "predmet": {"id": unit.claim_id, "text": unit.claim_text},
        "role": unit.role,
        "cesta": PATH_CARD_CHECK,
        "korpus": measurement.corpus,
        "dokument": {"id": unit.document, "titul": unit.title, "slot": unit.slot,
                     "cesta": unit.document_path},
        "rezim": unit.mode,
        "bloky": None if whole else [
            {"text": b["text"], "sekce": b.get("section"), "kotva": b.get("anchor")}
            for b in (unit.blocks or ())],
        "text_dokumentu": unit.document_text if whole else None,
        "citace": unit.quote,
        "stitek_hodnotitele": unit.model_label,
        "stitky_behu": list(unit.run_labels),
        "mereni": measurement.id,
        "jednotky": [u.id for u in units],
        "povolene": allowed,
        "rozhodnuti": "",
        "nepravda": "",
    }


def build_review(measurements: Sequence[LoadedMeasurement],
                 decisions: dict[str, dict[str, Any]], seed: int | None = None) -> Review:
    """All disputed units and a stratified random sample of the others, decided ones skipped.

    Units with the same key (the same shown text, document, fact and question,
    e.g. the roles `podpora` and `vyvojar` of one card) are one row.
    """
    if not measurements:
        raise ReviewError("Chybí měření, ze kterého se má kontrola sestavit.")
    if seed is None:
        seed = default_seed(m.id for m in measurements)
    groups: dict[tuple[str, str], list[UnitResult]] = {}
    owners: dict[tuple[str, str], LoadedMeasurement] = {}
    for measurement in measurements:
        for unit in measurement.units:
            if not _reviewable(unit):
                continue
            key = decision_key(unit)
            if key in decisions:
                continue
            slot = (measurement.id, key)
            groups.setdefault(slot, []).append(unit)
            owners[slot] = measurement
    disputed_rows: list[ReviewRow] = []
    by_label: dict[str, list[tuple[str, str]]] = {}
    for slot, units in groups.items():
        if any(u.disputed for u in units):
            disputed_rows.append(_make_row(owners[slot], units, slot[1], True))
        else:
            by_label.setdefault(units[0].model_label or "", []).append(slot)
    rng = random.Random(seed)
    sizes: dict[str, tuple[int, int]] = {}
    sampled: list[ReviewRow] = []
    for label in sorted(by_label):
        population = by_label[label]  # in measurement order: the draw depends on seed only
        drawn = sample_size(len(population))
        sizes[label] = (len(population), drawn)
        chosen = sorted(rng.sample(range(len(population)), drawn))
        sampled += [_make_row(owners[population[i]], groups[population[i]], population[i][1],
                              False) for i in chosen]
    rows = [ReviewRow(number=n, key=r.key, disputed=r.disputed, kind=r.kind,
                      measurement_id=r.measurement_id, unit_ids=r.unit_ids, data=r.data)
            for n, r in enumerate(disputed_rows + sampled, start=1)]
    return Review(seed=seed, measurements=tuple((m.id, str(m.path)) for m in measurements),
                  rows=tuple(rows), sample_sizes=sizes)


def _make_row(measurement: LoadedMeasurement, units: Sequence[UnitResult], key: str,
              disputed: bool) -> ReviewRow:
    return ReviewRow(number=0, key=key, disputed=disputed, kind=units[0].kind,
                     measurement_id=measurement.id, unit_ids=tuple(u.id for u in units),
                     data=_row_data(measurement, units, key, disputed))


def render_markdown(review: Review) -> str:
    """The review for reading; the row numbers are those of the JSON."""
    lines = ["# Kontrola vlastníka", "",
             f"Semínko vzorku: {review.seed}. Řádků: {len(review.rows)} "
             f"(sporných {review.disputed_count}). Rozhodnutí se vyplňuje v JSON, "
             "tento soubor je jen ke čtení.", ""]
    for row in review.rows:
        data = row.data
        kind = "fakt" if data["druh"] == ROW_FACT else "zakázané tvrzení"
        lines += [f"## Řádek {row.number}: {kind} {data['predmet']['id']} "
                  f"({'sporné' if row.disputed else 'vzorek'})", "",
                  f"- Otázka {data['otazka']['id']}: {data['otazka']['text']}",
                  f"- {kind.capitalize()}: {data['predmet']['text']}",
                  f"- Role: {data['role'] or '-'}; cesta: {data['cesta']}",
                  f"- Dokument: {data['dokument']['id']} ({data['dokument']['cesta']})",
                  f"- Štítek hodnotitele: {data['stitek_hodnotitele']}; "
                  f"štítky běhů: {', '.join(data['stitky_behu']) or '-'}"]
        if data["duvod_sporu"]:
            lines.append(f"- Důvod sporu: {data['duvod_sporu']}")
        if data["citace"]:
            lines.append(f"- Citace hodnotitele: {data['citace']}")
        lines.append(f"- Povolená rozhodnutí: {', '.join(data['povolene'])}")
        lines.append("")
        if data["text_dokumentu"] is not None:
            lines += ["Text dokumentu:", ""] + [f"> {ln}" for ln in
                                                 data["text_dokumentu"].splitlines()] + [""]
        else:
            for number, block in enumerate(data["bloky"] or (), start=1):
                lines += [f"Vázaný blok {number} (sekce {block['sekce'] or '-'}, "
                          f"kotva {block['kotva'] or '-'}):", ""]
                lines += [f"> {ln}" for ln in block["text"].splitlines()] + [""]
    return "\n".join(lines).rstrip("\n") + "\n"


def markdown_path(json_path: Path) -> Path:
    if json_path.suffix.lower() == ".md":
        raise ReviewError(f"Výstup {json_path} nesmí mít příponu .md; vedle JSON se "
                          "čitelná verze zapíše s touto příponou.")
    return json_path.with_suffix(".md")


def write_review(review: Review, json_path: Path) -> tuple[Path, Path]:
    """Write the JSON to fill in and the `.md` to read; an existing file is never overwritten."""
    md = markdown_path(json_path)
    for target in (json_path, md):
        if target.exists():
            raise ReviewError(f"Soubor {target} už existuje; mohl by obsahovat vyplněná "
                              "rozhodnutí. Načti ho (--nacti) nebo zvol jiný název.")
    _write_json_atomic(json_path, review.to_json())
    _write_text_atomic(md, render_markdown(review))
    return json_path, md


# --- loading the filled file -------------------------------------------------------------

@dataclass(frozen=True)
class IngestResult:
    decided: int       # rows with a decision in the file
    new: int           # of them stored for the first time or changed
    empty: int         # rows left empty
    proposals: int     # suggestions of forbidden claims in the proposals file
    proposals_path: Path


def proposals_path(review_path: Path) -> Path:
    return review_path.with_name(review_path.stem + PROPOSALS_SUFFIX)


def ingest_review(review_path: str | os.PathLike[str],
                  state_dir: str | os.PathLike[str]) -> IngestResult:
    """Validate the filled review and store its decisions; nothing is stored when a row is wrong."""
    file = Path(review_path)
    try:
        data = json.loads(file.read_text(encoding="utf-8"))
    except OSError as exc:
        raise ReviewError(f"Soubor ke kontrole {file} nejde načíst: {exc}") from None
    except ValueError as exc:
        raise ReviewError(f"Soubor ke kontrole {file} není platný JSON: {exc}") from None
    if (not isinstance(data, dict) or data.get("format_version") != REVIEW_FORMAT_VERSION
            or not isinstance(data.get("radky"), list) or not isinstance(data.get("mereni"), list)):
        raise ReviewError(f"Soubor {file} není soubor ke kontrole vlastníka (formát "
                          f"{REVIEW_FORMAT_VERSION}).")
    measured: dict[str, dict[str, UnitResult]] = {}  # measurement id -> key -> first unit
    for entry in data["mereni"]:
        if not isinstance(entry, dict) or "id" not in entry or "cesta" not in entry:
            raise ReviewError(f"Soubor {file}: záznam měření je neúplný.")
        loaded = load_measurement(entry["cesta"])
        if loaded.id != entry["id"]:
            raise ReviewError(f"Měření {entry['cesta']} má id {loaded.id}, soubor ke kontrole "
                              f"čeká {entry['id']}; měření se mezitím změnilo.")
        keys: dict[str, UnitResult] = {}
        for unit in loaded.units:
            if _reviewable(unit):
                keys.setdefault(decision_key(unit), unit)
        measured[loaded.id] = keys

    errors: list[str] = []
    accepted: dict[str, dict[str, Any]] = {}
    new_proposals: dict[str, dict[str, Any]] = {}
    empty = 0
    for position, row in enumerate(data["radky"], start=1):
        if not isinstance(row, dict):
            errors.append(f"řádek na pozici {position}: není objekt")
            continue
        number = row.get("radek", position)
        value = row.get("rozhodnuti")
        wrong = row.get("nepravda")
        text = wrong.strip() if isinstance(wrong, str) else ""
        if value is None or (isinstance(value, str) and not value.strip()):
            empty += 1
            if text:
                errors.append(f"řádek {number}: připsaná nepravda bez rozhodnutí")
            continue
        keys = measured.get(row.get("mereni"))
        key = row.get("klic")
        if keys is None or not isinstance(key, str) or key not in keys:
            errors.append(f"řádek {number}: klíč není v měření {row.get('mereni')} "
                          "(text korpusu, formulace nebo měření se změnily)")
            continue
        unit = keys[key]
        allowed = _allowed(unit.kind)
        folded = _fold(value) if isinstance(value, str) else ""
        if folded not in allowed:
            what = "fakt" if unit.kind == KIND_FACT else "zakázané tvrzení"
            errors.append(f"řádek {number} ({what} {unit.claim_id}): rozhodnutí „{value}“ není "
                          f"přípustné; platí: {', '.join(allowed)}")
            continue
        if wrong is not None and not isinstance(wrong, str):
            errors.append(f"řádek {number}: „nepravda“ musí být text")
            continue
        previous = accepted.get(key)
        if previous is not None and previous["rozhodnuti"] != folded:
            errors.append(f"řádek {number}: stejná jednotka je v souboru rozhodnutá jinak "
                          f"(řádek {previous['radek']})")
            continue
        accepted[key] = {"rozhodnuti": folded, "stitek": allowed[folded], "druh": unit.kind,
                         "otazka": unit.question_id, "predmet": unit.claim_id,
                         "dokument": unit.document, "nepravda": text or None,
                         "rozhodl": DECIDED_BY_OWNER, "radek": number}
        if text:
            new_proposals[key] = {"klic": key, "otazka": unit.question_id, "druh": unit.kind,
                                  "predmet": unit.claim_id, "text": text,
                                  "misto": list(unit.places), "dokument": unit.document}
    if errors:
        raise ReviewError("Soubor ke kontrole se nenačetl, protože obsahuje chyby:\n  "
                          + "\n  ".join(errors))

    stored = load_decisions(state_dir)
    changed = 0
    for key, record in accepted.items():
        record = {k: v for k, v in record.items() if k != "radek"}
        if stored.get(key) != record:
            changed += 1
        stored[key] = record
    if accepted:
        save_decisions(state_dir, stored)
    target = proposals_path(file)
    proposals = _read_proposals(target)
    proposals.update(new_proposals)
    if new_proposals:
        _write_json_atomic(target, {"format_version": PROPOSALS_FORMAT_VERSION,
                                    "navrhy": list(proposals.values())})
    return IngestResult(decided=len(accepted), new=changed, empty=empty,
                        proposals=len(proposals), proposals_path=target)


def _read_proposals(path: Path) -> dict[str, dict[str, Any]]:
    if not path.is_file():
        return {}
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
        return {item["klic"]: item for item in data["navrhy"]}
    except (OSError, ValueError, KeyError, TypeError):
        raise ReviewError(f"Soubor návrhů {path} nejde načíst; oprav ho nebo ho odstraň.") from None


# --- what the report reads ---------------------------------------------------------------

@dataclass(frozen=True)
class ReviewedMeasurement:
    """A measurement with the owner's decisions applied.

    `units` carry the final labels (`decided_by` names the owner where a
    decision overwrote the model's label), `results` are derived from them,
    `missing` are the ids of the units that still wait for the owner: disputed
    without a decision, or drawn into a sample (`record_sampled`) without one;
    while there are any, the report must not be made.
    """

    id: str
    units: tuple[UnitResult, ...]
    results: Results
    missing: tuple[str, ...]
    decided: tuple[str, ...]


def _plan_from_results(results: dict[str, Any], units: dict[str, UnitResult]) -> Plan:
    """The plan the results were derived from, as far as `derive_results` needs it.

    An agent's answers keep their `variant` and `run`, and a run without an answer keeps
    `no_answer` (its status `chybí, bez odpovědi` carries the reason), so deriving the
    results again changes nothing but the labels a decision overwrote.
    """
    facts = tuple(FactPlan(
        question_id=r["question_id"], fact_id=r["fact_id"], role=r["role"],
        unverified=tuple(r["reasons"]) if r["status"] == FACT_PLACE_UNKNOWN else (),
        documents=tuple(r["documents"]), unit_ids=tuple(r["unit_ids"]),
        variant=r.get("variant"), run=r.get("run"),
        no_answer=_no_answer_reason(r, FACT_NO_ANSWER))
        for r in results["facts"])
    claims = tuple(ClaimPlan(
        question_id=r["question_id"], claim_id=r["claim_id"],
        fact_ids=tuple(r.get("fact_ids") or ()),
        unverified=tuple(r["reasons"]) if r["status"] == CLAIM_PLACE_UNKNOWN else (),
        documents=tuple(dict.fromkeys(units[u].document for u in r["unit_ids"])),
        unit_ids=tuple(r["unit_ids"]), variant=r.get("variant"), run=r.get("run"),
        no_answer=_no_answer_reason(r, CLAIM_NO_ANSWER)) for r in results["claims"])
    return Plan(questions=(), facts=facts, claims=claims, units=())


def _no_answer_reason(result: dict[str, Any], status: str) -> str | None:
    """Why a run has no answer: the reason a result of that status carries, or None."""
    if result["status"] != status:
        return None
    reasons = result.get("reasons") or ()
    return reasons[0] if reasons else "bez odpovědi"


# --- the sampled units -------------------------------------------------------------------

def sampled_path(state_dir: str | os.PathLike[str]) -> Path:
    return Path(state_dir) / SAMPLED_FILE


def load_sampled(state_dir: str | os.PathLike[str]) -> dict[str, dict[str, Any]]:
    """The keys of the undisputed units the owner was asked to check (empty when none yet)."""
    path = sampled_path(state_dir)
    if not path.is_file():
        return {}
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as exc:
        raise ReviewError(f"Seznam vylosovaných jednotek {path} nejde načíst: {exc}") from None
    if (not isinstance(data, dict) or data.get("format_version") != SAMPLED_FORMAT_VERSION
            or not isinstance(data.get("vylosovano"), dict)):
        raise ReviewError(f"Seznam vylosovaných jednotek {path} nemá podporovaný formát.")
    return data["vylosovano"]


def record_sampled(state_dir: str | os.PathLike[str], plan: Review) -> int:
    """Remember the sampled rows of a review file, so a report can tell what was drawn.

    The sample is drawn at random from the units still undecided, so it cannot be drawn
    again afterwards: the keys of the drawn rows are stored (added to those of earlier
    review files; a key is never removed). Returns how many rows were recorded.
    """
    stored = load_sampled(state_dir)
    drawn = [row for row in plan.rows if not row.disputed]
    for row in drawn:
        stored[row.key] = {"mereni": row.measurement_id, "jednotky": list(row.unit_ids)}
    if drawn:
        _write_json_atomic(sampled_path(state_dir), {
            "format_version": SAMPLED_FORMAT_VERSION, "seminko": plan.seed,
            "vylosovano": stored})
    return len(drawn)


def review_state(measurement: str | os.PathLike[str] | LoadedMeasurement,
                 state_dir: str | os.PathLike[str]) -> ReviewedMeasurement:
    """The final labels of a finished measurement with the owner's decisions, and what lacks one."""
    loaded = measurement if isinstance(measurement, LoadedMeasurement) \
        else load_measurement(measurement)
    if loaded.results is None:
        raise ReviewError(f"Měření {loaded.path} nemá výsledky.")
    decisions = load_decisions(state_dir)
    sampled = load_sampled(state_dir)
    final: list[UnitResult] = []
    decided: list[str] = []
    undecided_sample: set[str] = set()
    for unit in loaded.units:
        if _reviewable(unit):
            key = decision_key(unit)
            record = decisions.get(key)
            if record is None and key in sampled:
                undecided_sample.add(unit.id)
            if record is not None:
                if record.get("druh") != unit.kind or record.get("stitek") not in _allowed(
                        unit.kind).values():
                    raise ReviewError(f"Rozhodnutí jednotky {unit.id} nesedí k jejímu druhu; "
                                      f"{DECISIONS_FILE} je poškozený.")
                unit = unit.with_decision(record["stitek"], record.get("rozhodl")
                                          or DECIDED_BY_OWNER)
                decided.append(unit.id)
        final.append(unit)
    missing = tuple(u.id for u in final if u.open_dispute or u.id in undecided_sample)
    results = derive_results(_plan_from_results(loaded.results, {u.id: u for u in final}), final)
    return ReviewedMeasurement(id=loaded.id, units=tuple(final), results=results,
                               missing=missing, decided=tuple(decided))
