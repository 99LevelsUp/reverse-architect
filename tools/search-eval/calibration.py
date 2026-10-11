"""Calibration of the judge against a frozen set of sentences (U9, KTD12).

The owner writes a small set of sentences, each with the label he expects the
judge to give, in four classes:

* `vyjadruje` - the sentence states the fact;
* `pravdiva_neodpovida` - a true sentence that does not answer the question;
* `parafraze_nepravdy` - a paraphrase of a forbidden claim;
* `popreni` - a denial of the fact.

Every sentence is judged with `judge.assess` (the same three-run protocol and
cache as a measurement); the sentence is the only block of the unit. The
result is a confusion matrix by class, the plain agreement, two dangerous
cells reported separately, and the coverage (units without `NELZE`).

No kappa is computed: with a small sample and unequal classes it misleads.
The result names the judge, its reported model, `PROMPT_VERSION` and the
fingerprint of the set (SHA-256 of its canonical JSON), so a report can find
the calibration for a pair judge + prompt version and refuse one made for
another set. The judge's command line is never part of the result.
"""
from __future__ import annotations

import hashlib
import json
import os
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Sequence

from judge import (KIND_FACT, KIND_FORBIDDEN, LABEL_UNKNOWN, LABELS_FACT, LABELS_FORBIDDEN,
                   PROMPT_VERSION, Assessment, JudgeCache, JudgeRunner, JudgeSelection, JudgeUnit,
                   assess)
from prepare import canonical_path, ensure_outside, write_json_atomic

SET_FORMAT_VERSION = 1
RESULT_FORMAT_VERSION = 1

CLASS_EXPRESSES = "vyjadruje"
CLASS_TRUE_IRRELEVANT = "pravdiva_neodpovida"
CLASS_FORBIDDEN_PARAPHRASE = "parafraze_nepravdy"
CLASS_DENIAL = "popreni"
CLASSES = (CLASS_EXPRESSES, CLASS_TRUE_IRRELEVANT, CLASS_FORBIDDEN_PARAPHRASE, CLASS_DENIAL)
ALL_LABELS = ("VYJADRUJE", "NEVYJADRUJE", "OBSAHUJE", "NEOBSAHUJE", LABEL_UNKNOWN)

DRUH_FACT = "fakt"
DRUH_FORBIDDEN = "zakazane"
_DRUH_TO_KIND = {DRUH_FACT: KIND_FACT, DRUH_FORBIDDEN: KIND_FORBIDDEN}
# a paraphrase of a forbidden claim is judged against a forbidden claim; the rest against a fact
_CLASS_KIND = {CLASS_EXPRESSES: KIND_FACT, CLASS_TRUE_IRRELEVANT: KIND_FACT,
               CLASS_DENIAL: KIND_FACT, CLASS_FORBIDDEN_PARAPHRASE: KIND_FORBIDDEN}

NOT_CLAIMED = (
    "Kalibrace měří shodu hodnotitele s očekáváním jednoho člověka na malé zmrazené sadě vět. "
    "Neměří spolehlivost štítků hodnotitele na skutečných dokumentech a nezaručuje, že jiná sada "
    "nebo jiný vlastník by dopadl stejně.",
    "Nepočítá se kappa: při malém vzorku a nerovnoměrných třídách by zavádělo.",
)

_ITEM_KEYS = {"otazka", "tvrzeni", "veta", "trida", "ocekavany_stitek"}
_CLAIM_KEYS = {"id", "text", "druh"}


class CalibrationSetError(ValueError):
    """The calibration set is malformed, or has an unknown class or label (Czech message)."""


class CalibrationResultError(ValueError):
    """A calibration result file is unreadable or of another format (Czech message)."""


# --- the set -----------------------------------------------------------------

@dataclass(frozen=True)
class CalibrationItem:
    """One sentence with the label the owner expects."""

    item_id: str
    question: str
    claim_id: str
    claim_text: str
    kind: str
    sentence: str
    klass: str
    expected: str

    def unit(self) -> JudgeUnit:
        return JudgeUnit(kind=self.kind, claim_id=self.claim_id, claim_text=self.claim_text,
                         question=self.question, blocks=(self.sentence,))


@dataclass(frozen=True)
class CalibrationSet:
    """A frozen set; `fingerprint` is the SHA-256 of its canonical JSON."""

    items: tuple[CalibrationItem, ...]
    fingerprint: str


def fingerprint_of(data: Any) -> str:
    """SHA-256 of the canonical JSON (sorted keys, no ASCII escaping) of the parsed set."""
    encoded = json.dumps(data, ensure_ascii=False, sort_keys=True,
                         separators=(",", ":")).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


def load_set(path: str | Path) -> CalibrationSet:
    """Read and validate the set (`format_version` 1)."""
    try:
        data = json.loads(Path(path).read_text(encoding="utf-8"))
    except OSError as exc:
        raise CalibrationSetError(
            f"Sada kalibrace {path} nejde přečíst: {exc.strerror or exc}.") from exc
    except json.JSONDecodeError as exc:
        raise CalibrationSetError(f"Sada kalibrace {path} není platný JSON: {exc}.") from exc
    return parse_set(data)


def parse_set(data: Any) -> CalibrationSet:
    if not isinstance(data, dict):
        raise CalibrationSetError("Sada kalibrace musí být JSON objekt.")
    if data.get("format_version") != SET_FORMAT_VERSION:
        raise CalibrationSetError(
            f"Sada kalibrace má verzi formátu {data.get('format_version')!r}; "
            f"podporována je jen {SET_FORMAT_VERSION}.")
    entries = data.get("polozky")
    if not isinstance(entries, list) or not entries:
        raise CalibrationSetError("Pole `polozky` musí být neprázdný seznam.")
    items = tuple(_parse_item(entry, number) for number, entry in enumerate(entries, start=1))
    return CalibrationSet(items=items, fingerprint=fingerprint_of(data))


def _parse_item(entry: Any, number: int) -> CalibrationItem:
    where = f"Položka kalibrace č. {number}"
    if not isinstance(entry, dict):
        raise CalibrationSetError(f"{where} musí být objekt.")
    unknown = sorted(set(entry) - _ITEM_KEYS)
    if unknown:
        raise CalibrationSetError(f"{where} má neznámé pole {', '.join(unknown)}.")

    def text(source: dict[str, Any], key: str) -> str:
        value = source.get(key)
        if not isinstance(value, str) or not value.strip():
            raise CalibrationSetError(f"{where}: pole `{key}` musí být neprázdný text.")
        return value

    claim = entry.get("tvrzeni")
    if not isinstance(claim, dict):
        raise CalibrationSetError(f"{where}: pole `tvrzeni` musí být objekt (id, text, druh).")
    unknown_claim = sorted(set(claim) - _CLAIM_KEYS)
    if unknown_claim:
        raise CalibrationSetError(f"{where}: `tvrzeni` má neznámé pole {', '.join(unknown_claim)}.")
    druh = text(claim, "druh")
    if druh not in _DRUH_TO_KIND:
        raise CalibrationSetError(
            f"{where}: neznámý druh {druh!r} (povoleno: {', '.join(_DRUH_TO_KIND)}).")
    klass = text(entry, "trida")
    if klass not in CLASSES:
        raise CalibrationSetError(
            f"{where}: neznámá třída {klass!r} (povoleno: {', '.join(CLASSES)}).")
    kind = _DRUH_TO_KIND[druh]
    if _CLASS_KIND[klass] != kind:
        needed = DRUH_FORBIDDEN if _CLASS_KIND[klass] == KIND_FORBIDDEN else DRUH_FACT
        raise CalibrationSetError(
            f"{where}: třída {klass} se posuzuje proti druhu {needed!r}, ne {druh!r}.")
    expected = text(entry, "ocekavany_stitek")
    labels = LABELS_FACT if kind == KIND_FACT else LABELS_FORBIDDEN
    if expected not in labels:
        raise CalibrationSetError(
            f"{where}: neznámý štítek {expected!r} pro druh {druh!r} "
            f"(povoleno: {', '.join(labels)}).")
    return CalibrationItem(
        item_id=f"p{number}", question=text(entry, "otazka"), claim_id=text(claim, "id"),
        claim_text=text(claim, "text"), kind=kind, sentence=text(entry, "veta"), klass=klass,
        expected=expected)


# --- the run -----------------------------------------------------------------

@dataclass(frozen=True)
class CalibrationResult:
    """What a calibration records; `to_json` never carries the judge's command."""

    judge: dict[str, Any]
    prompt_version: str
    set_fingerprint: str
    total: int
    matrix: dict[str, dict[str, int]]
    agreement: float
    unacknowledged_true: dict[str, Any]
    missed_paraphrase: dict[str, Any]
    coverage: float
    disputed: int
    units: tuple[dict[str, Any], ...]

    def to_json(self) -> dict[str, Any]:
        return {
            "format_version": RESULT_FORMAT_VERSION,
            "hodnotitel": dict(self.judge),
            "prompt_version": self.prompt_version,
            "otisk_sady": self.set_fingerprint,
            "jednotek": self.total,
            "matice": {k: dict(v) for k, v in self.matrix.items()},
            "shoda": self.agreement,
            "nebezpecne": {
                "neodpovidajici_veta_uznana": dict(self.unacknowledged_true),
                "parafraze_nepravdy_prehlednuta": dict(self.missed_paraphrase),
            },
            "pokryti": self.coverage,
            "sporne": self.disputed,
            "jednotky": [dict(u) for u in self.units],
            "netvrdi": list(NOT_CLAIMED),
        }


def run_calibration(calibration_set: CalibrationSet, selection: JudgeSelection,
                    runner: JudgeRunner, cache: JudgeCache) -> CalibrationResult:
    """Judge every sentence (three runs each, cache first) and tally the outcome.

    A `JudgeInfrastructureError` propagates: a calibration is not made from a
    failing judge. Runs already cached are not repeated.
    """
    matrix = {klass: {label: 0 for label in ALL_LABELS} for klass in CLASSES}
    units: list[dict[str, Any]] = []
    agreed = covered = disputed = 0
    unack: list[str] = []
    missed: list[str] = []
    for item in calibration_set.items:
        outcome: Assessment = assess(item.unit(), selection, runner, cache)
        matrix[item.klass][outcome.label] += 1
        agreed += outcome.label == item.expected
        covered += outcome.label != LABEL_UNKNOWN
        disputed += outcome.disputed
        if item.klass == CLASS_TRUE_IRRELEVANT and outcome.label == "VYJADRUJE":
            unack.append(item.item_id)
        if item.klass == CLASS_FORBIDDEN_PARAPHRASE and outcome.label == "NEOBSAHUJE":
            missed.append(item.item_id)
        units.append({"id": item.item_id, "trida": item.klass, "ocekavano": item.expected,
                      "stitek": outcome.label, "sporne": outcome.disputed})
    total = len(calibration_set.items)
    return CalibrationResult(
        judge=selection.identity(), prompt_version=PROMPT_VERSION,
        set_fingerprint=calibration_set.fingerprint, total=total, matrix=matrix,
        agreement=agreed / total,
        unacknowledged_true={"pocet": len(unack), "jednotky": unack},
        missed_paraphrase={"pocet": len(missed), "jednotky": missed},
        coverage=covered / total, disputed=disputed, units=tuple(units))


# --- output and lookup -------------------------------------------------------

def write_result(result: CalibrationResult, output: str | os.PathLike[str], *,
                 protected: Sequence[str] = ()) -> Path:
    """Write the result atomically to `output` (required); refuse a path under `protected` roots."""
    if not str(output).strip():
        raise CalibrationSetError("Výstupní cesta kalibrace je povinná.")
    path = ensure_outside(output, [canonical_path(r) for r in protected], "Výstup kalibrace")
    path.parent.mkdir(parents=True, exist_ok=True)
    write_json_atomic(path, result.to_json())
    return path


def load_result(path: str | Path) -> dict[str, Any]:
    """Read a calibration result file back (as parsed JSON)."""
    try:
        data = json.loads(Path(path).read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise CalibrationResultError(f"Výsledek kalibrace {path} nejde přečíst: {exc}.") from exc
    if not isinstance(data, dict) or data.get("format_version") != RESULT_FORMAT_VERSION:
        raise CalibrationResultError(f"Výsledek kalibrace {path} má neznámý formát.")
    return data


def matches(result: dict[str, Any], judge_id: str, prompt_version: str,
            set_fingerprint: str) -> bool:
    """True when `result` was made by this judge, with this prompt version, on this very set."""
    judge = result.get("hodnotitel")
    return (isinstance(judge, dict) and judge.get("id") == judge_id
            and result.get("prompt_version") == prompt_version
            and result.get("otisk_sady") == set_fingerprint)


def find_calibration(directory: str | Path, judge_id: str, prompt_version: str,
                     set_fingerprint: str) -> tuple[Path, dict[str, Any]] | None:
    """The newest calibration in `directory` for the pair judge + prompt version on this set.

    A calibration of a changed set (other fingerprint) is not found. Files that
    are not calibration results are skipped.
    """
    root = Path(directory)
    if not root.is_dir():
        return None
    found: list[tuple[float, str, Path, dict[str, Any]]] = []
    for path in root.glob("*.json"):
        try:
            data = load_result(path)
        except CalibrationResultError:
            continue
        if matches(data, judge_id, prompt_version, set_fingerprint):
            found.append((path.stat().st_mtime, path.name, path, data))
    if not found:
        return None
    _, _, path, data = max(found, key=lambda f: (f[0], f[1]))
    return path, data
