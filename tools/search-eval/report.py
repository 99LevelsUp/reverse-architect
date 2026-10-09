"""The experiment report: one data dict, written as JSON and rendered as Markdown (U6).

The report is built once as a plain dict (`build_report`); `report.json` is
that dict and `report.md` is rendered from it (`render_markdown`). Numbers
are rounded while the dict is built, never while rendering, so both files
carry the same numbers by construction (U6 verification) rather than by two
computations that happen to agree.

What the numbers mean (R8, KTD5, KTD7, KTD11):

* **Unit of comparison is the case.** The first variant is the reference,
  the second the compared one; "výhra" means the compared variant had more
  successful runs out of three clean ones, "prohra" fewer, "remíza" the same
  (the cost difference is shown without a verdict). A case without three
  clean runs in either variant is incomplete and counts in no tally.
* **Clean runs** are successes and failures. Spoiled attempts (audit) are
  listed with their reasons and replaced by the runner; runs stopped by an
  infrastructure error are listed and count nowhere.
* **Cap convention.** In the main medians a run stopped by any cap carries
  the cap's value for steps, seconds and total tokens: a comparison
  convention ("the run cost the whole cap"), not its real consumption. The
  cap is defined on the token *sum* (KTD4), so the four token categories
  cannot carry a cap value; they are the observed values for every run, a
  lower bound for capped runs. The success-only medians use observed values
  throughout (successful runs never hit a cap).
* **Unknown is not zero.** A token category the agent did not report is
  unknown; a median over a set with any unknown value is unknown, and so is
  a total or a price that depends on it.
* **Money** only with a recorded tariff (price per million tokens and
  category); it follows the observed token categories.
* **Cache reads by run order** are listed because the provider's cache is
  not cleared between runs (KTD11): later runs may profit from earlier ones.

Case flags (R1, R13): cases used for tuning the cards and cases whose prompt
names the location are listed in their own table, and the tallies are given
both for all cases and for the cases with neither flag.
"""
from __future__ import annotations

import json
import os
import statistics
import tempfile
from dataclasses import dataclass, field, replace
from pathlib import Path
from typing import Any, Mapping, Sequence

from agents.base import Cap, TokenUsage
from score import FAILURE, SPOILED, SUCCESS

INFRASTRUCTURE = "infrastructure"  # a run state the runner adds to score's three
CLEAN_STATES = (SUCCESS, FAILURE)

STATUS_COMPLETED = "completed"
STATUS_INFRASTRUCTURE = "infrastructure_error"
STATUS_INTERRUPTED = "interrupted"

WIN, LOSS, TIE, INCOMPLETE = "win", "loss", "tie", "incomplete"
UNKNOWN = "unknown"
RUNS_PER_VARIANT = 3

TOKEN_CATEGORIES = ("input_uncached", "cache_write", "cache_read", "output")
METRICS = (*TOKEN_CATEGORIES, "tokens_total", "steps", "seconds")

REPORT_FORMAT_VERSION = 1


# --- inputs ------------------------------------------------------------------

@dataclass(frozen=True)
class RunResult:
    """One executed attempt as the report reads it.

    `state` is success, failure, spoiled or infrastructure. `reason` is the
    failure reason (or the infrastructure message); `spoil_reasons` are the
    audit findings as "kind: detail". `cap_hit` is the cap record as a dict
    (`kind`, `limit`, `observed`, `overshoot`) or None.
    """

    order: int
    case_id: str
    variant: str
    attempt: int
    state: str
    reason: str | None = None
    replaces: int | None = None
    spoil_reasons: tuple[str, ...] = ()
    answer: dict[str, str] | None = None
    tokens: TokenUsage = field(default_factory=TokenUsage)
    steps: int = 0
    wall_seconds: float = 0.0
    cap_hit: dict[str, Any] | None = None
    result_error: str | None = None
    transcript: str | None = None

    def to_json(self) -> dict[str, Any]:
        return {
            "order": self.order, "case_id": self.case_id, "variant": self.variant,
            "attempt": self.attempt, "replaces": self.replaces, "state": self.state,
            "reason": self.reason, "spoil_reasons": list(self.spoil_reasons),
            "answer": self.answer, "tokens": _tokens_json(self.tokens), "steps": self.steps,
            "wall_seconds": round(self.wall_seconds, 3), "cap_hit": self.cap_hit,
            "result_error": self.result_error, "transcript": self.transcript,
        }


@dataclass(frozen=True)
class Tariff:
    """Recorded prices: currency, price per million tokens of each category, source."""

    currency: str
    per_million: Mapping[str, float]
    source: str = ""

    def cost(self, tokens: TokenUsage) -> float | str:
        values = [getattr(tokens, c) for c in TOKEN_CATEGORIES]
        if any(v is None for v in values):
            return UNKNOWN
        return sum(v * float(self.per_million[c]) / 1_000_000
                   for v, c in zip(values, TOKEN_CATEGORIES))


@dataclass(frozen=True)
class PreparationCost:
    """A one-off cost of the experiment (KTD11); never part of a run's numbers."""

    step: str
    seconds: float | None
    tokens: TokenUsage | None = None
    note: str = ""


@dataclass(frozen=True)
class VariantInfo:
    """A variant as the report describes it (R9)."""

    name: str
    corpus_dir: str | None = None
    corpus_receipt: dict[str, Any] | None = None
    manifest_schema_version: int | None = None
    card_format: str | None = None
    prompt_template: str = ""
    prompt_sha256: Mapping[str, str] = field(default_factory=dict)


@dataclass(frozen=True)
class CaseInfo:
    """A case's identity and the two flags that set it apart (R1, R13).

    `druh` is the catalog item kind. This navigation report describes only
    items of kind `chyba` (the only kind that existed before catalog format 2,
    hence the default); `build_report` leaves out any other kind.
    """

    id: str
    used_for_card_tuning: bool
    prompt_names_location: bool
    druh: str = "chyba"

    @property
    def flagged(self) -> bool:
        return self.used_for_card_tuning or self.prompt_names_location


@dataclass(frozen=True)
class ExperimentInfo:
    """Everything the report states besides the runs (R9)."""

    experiment_id: str
    started_at: str
    status: str
    model: str
    cap: Cap
    seed: int
    base_revision: str
    tree_fingerprint: str
    variants: tuple[VariantInfo, VariantInfo]
    cases: tuple[CaseInfo, ...]
    adapter: str = ""
    agent_version: str | None = None
    status_detail: str | None = None
    finished_at: str | None = None
    preparation: tuple[PreparationCost, ...] = ()
    tariff: Tariff | None = None
    concurrency: int = 1
    max_replacements: int = 2


# --- numbers -----------------------------------------------------------------

def _tokens_json(tokens: TokenUsage) -> dict[str, int | None]:
    return {c: getattr(tokens, c) for c in TOKEN_CATEGORIES}


def _tokens_total(tokens: TokenUsage) -> int | None:
    values = [getattr(tokens, c) for c in TOKEN_CATEGORIES]
    return None if any(v is None for v in values) else sum(values)


def _run_values(run: RunResult, cap: Cap, convention: bool) -> dict[str, Any]:
    """One run's metrics; with `convention` a capped run carries the cap (module docstring)."""
    capped = convention and run.cap_hit is not None
    values: dict[str, Any] = {c: getattr(run.tokens, c) for c in TOKEN_CATEGORIES}
    values["tokens_total"] = cap.tokens if capped else _tokens_total(run.tokens)
    values["steps"] = cap.steps if capped else run.steps
    values["seconds"] = float(cap.seconds) if capped else run.wall_seconds
    return values


def _median(values: Sequence[Any], digits: int | None = None) -> Any:
    """Median; None when there are no values, UNKNOWN when any value is unknown."""
    if not values:
        return None
    if any(v is None or v == UNKNOWN for v in values):
        return UNKNOWN
    result = statistics.median(values)
    if digits is not None:
        return round(float(result), digits)
    return int(result) if float(result).is_integer() else float(result)


def _medians(runs: Sequence[RunResult], cap: Cap, convention: bool) -> dict[str, Any]:
    rows = [_run_values(r, cap, convention) for r in runs]
    return {m: _median([row[m] for row in rows], 1 if m == "seconds" else None)
            for m in METRICS}


def _cost_median(runs: Sequence[RunResult], tariff: Tariff | None) -> Any:
    if tariff is None:
        return None
    return _median([tariff.cost(r.tokens) for r in runs], 6)


def _difference(compared: Any, reference: Any) -> Any:
    if compared is None or reference is None:
        return None
    if compared == UNKNOWN or reference == UNKNOWN:
        return UNKNOWN
    diff = compared - reference
    return round(diff, 4) if isinstance(diff, float) else diff


# --- report data -------------------------------------------------------------

def build_report(info: ExperimentInfo, results: Sequence[RunResult]) -> dict[str, Any]:
    """The whole report as a JSON-ready dict (see the module docstring)."""
    reference, compared = (v.name for v in info.variants)
    info = replace(info, cases=tuple(c for c in info.cases if c.druh == "chyba"))
    cases = [_case_entry(case, info, results) for case in info.cases]
    by_id = {c["case_id"]: c for c in cases}
    unflagged = [c for c in info.cases if not c.flagged]
    return {
        "format_version": REPORT_FORMAT_VERSION,
        "experiment": _experiment_json(info),
        "variants": {"reference": reference, "compared": compared},
        "preparation": [_preparation_json(p) for p in info.preparation],
        "summary": {
            "all_cases": _tally(cases),
            "unflagged_cases": _tally([by_id[c.id] for c in unflagged]),
            "used_for_card_tuning": [c.id for c in info.cases if c.used_for_card_tuning],
            "prompt_names_location": [c.id for c in info.cases if c.prompt_names_location],
        },
        "cases": cases,
        "spoiled_attempts": [
            {"order": r.order, "case_id": r.case_id, "variant": r.variant,
             "attempt": r.attempt, "reasons": list(r.spoil_reasons)}
            for r in results if r.state == SPOILED],
        "infrastructure_errors": [
            {"order": r.order, "case_id": r.case_id, "variant": r.variant, "reason": r.reason}
            for r in results if r.state == INFRASTRUCTURE],
        "cache_read_by_order": [
            {"order": r.order, "case_id": r.case_id, "variant": r.variant, "state": r.state,
             "cache_read": r.tokens.cache_read}
            for r in sorted(results, key=lambda r: r.order)],
        "money": _money(info, results),
        "not_claimed": not_claimed(info),
    }


def _experiment_json(info: ExperimentInfo) -> dict[str, Any]:
    return {
        "id": info.experiment_id,
        "started_at": info.started_at,
        "finished_at": info.finished_at,
        "status": info.status,
        "status_detail": info.status_detail,
        "model": info.model,
        "adapter": info.adapter,
        "agent_version": info.agent_version,
        "cap": {"seconds": info.cap.seconds, "steps": info.cap.steps, "tokens": info.cap.tokens},
        "seed": info.seed,
        "concurrency": info.concurrency,
        "runs_per_variant": RUNS_PER_VARIANT,
        "max_replacements": info.max_replacements,
        "base_revision": info.base_revision,
        "tree_fingerprint": info.tree_fingerprint,
        "variants": [_variant_json(v) for v in info.variants],
        "cases": [{"id": c.id, "used_for_card_tuning": c.used_for_card_tuning,
                   "prompt_names_location": c.prompt_names_location} for c in info.cases],
    }


def _variant_json(v: VariantInfo) -> dict[str, Any]:
    return {
        "name": v.name, "corpus_dir": v.corpus_dir, "corpus_receipt": v.corpus_receipt,
        "manifest_schema_version": v.manifest_schema_version, "card_format": v.card_format,
        "prompt_template": v.prompt_template, "prompt_sha256": dict(v.prompt_sha256),
    }


def _preparation_json(p: PreparationCost) -> dict[str, Any]:
    return {"step": p.step,
            "seconds": round(p.seconds, 1) if p.seconds is not None else UNKNOWN,
            "tokens": _tokens_json(p.tokens) if p.tokens is not None else None,
            "note": p.note}


def _case_entry(case: CaseInfo, info: ExperimentInfo,
                results: Sequence[RunResult]) -> dict[str, Any]:
    variants = [_variant_entry(case.id, v.name, info, results) for v in info.variants]
    reference, compared = variants
    if not (reference["complete"] and compared["complete"]):
        verdict = INCOMPLETE
    elif compared["successes"] > reference["successes"]:
        verdict = WIN
    elif compared["successes"] < reference["successes"]:
        verdict = LOSS
    else:
        verdict = TIE
    difference = {m: _difference(compared["medians_all"][m], reference["medians_all"][m])
                  for m in METRICS}
    return {
        "case_id": case.id,
        "used_for_card_tuning": case.used_for_card_tuning,
        "prompt_names_location": case.prompt_names_location,
        "result": verdict,
        "variants": variants,
        "difference_compared_minus_reference": difference,
    }


def _variant_entry(case_id: str, variant: str, info: ExperimentInfo,
                   results: Sequence[RunResult]) -> dict[str, Any]:
    mine = [r for r in results if r.case_id == case_id and r.variant == variant]
    clean = [r for r in mine if r.state in CLEAN_STATES][:RUNS_PER_VARIANT]
    successes = [r for r in clean if r.state == SUCCESS]
    return {
        "variant": variant,
        "clean_runs": len(clean),
        "successes": len(successes),
        "complete": len(clean) == RUNS_PER_VARIANT,
        "capped_runs": sum(1 for r in clean if r.cap_hit is not None),
        "spoiled_attempts": sum(1 for r in mine if r.state == SPOILED),
        "failure_reasons": [r.reason for r in clean if r.state == FAILURE],
        "medians_all": _medians(clean, info.cap, convention=True),
        "medians_success": _medians(successes, info.cap, convention=False),
        "cost_median_all": _cost_median(clean, info.tariff),
        "cost_median_success": _cost_median(successes, info.tariff),
    }


def _tally(cases: Sequence[Mapping[str, Any]]) -> dict[str, int]:
    tally = {WIN: 0, LOSS: 0, TIE: 0, INCOMPLETE: 0}
    for case in cases:
        tally[case["result"]] += 1
    return tally


def _money(info: ExperimentInfo, results: Sequence[RunResult]) -> dict[str, Any] | None:
    if info.tariff is None:
        return None
    costs = [info.tariff.cost(r.tokens) for r in results]
    total: Any = UNKNOWN if any(c == UNKNOWN for c in costs) else round(sum(costs), 6)
    return {"currency": info.tariff.currency,
            "per_million": {c: float(info.tariff.per_million[c]) for c in TOKEN_CATEGORIES},
            "source": info.tariff.source,
            "total_all_runs": total}


def not_claimed(info: ExperimentInfo) -> list[str]:
    """The "co toto měření netvrdí" section (R15), as plain sentences."""
    return [
        f"Měří agenta (model {info.model}) v pracovním postupu se zdrojáky a korpusem RA "
        f"čteným jako soubory na těchto {len(info.cases)} případech, ne lidi (podporu, "
        "analytiky ani dokumentaristy).",
        "Neměří příjem případu, ověření verze ani ověření v kódu a návrh opravy: běh končí "
        "odevzdáním místa příčiny.",
        "Nepřipisuje rozdíl samotnému obsahu karet bez vlivu agentova hledání: varianta "
        "s kartami měří korpus a způsob, jak v něm agent hledá, společně.",
        "Nestanoví práh úspěchu. Na několika případech rozliší jen velký rozdíl a výsledek smí "
        "být nerozhodný.",
        "Hlavní mediány s hodnotou stropu u běhů ukončených stropem jsou srovnávací konvence, "
        "ne skutečná spotřeba.",
    ]


# --- Markdown ----------------------------------------------------------------

_RESULT_CS = {WIN: "výhra", LOSS: "prohra", TIE: "remíza", INCOMPLETE: "neúplný"}
_STATE_CS = {SUCCESS: "úspěch", FAILURE: "neúspěch", SPOILED: "zkažený",
             INFRASTRUCTURE: "chyba infrastruktury"}
_STATUS_CS = {
    STATUS_COMPLETED: "dokončen",
    STATUS_INFRASTRUCTURE: "přerušen chybou infrastruktury — výsledky jsou neúplné",
    STATUS_INTERRUPTED: "přerušen operátorem — výsledky jsou neúplné",
}
_METRIC_CS = {
    "input_uncached": "vstup bez cache", "cache_write": "zápis do cache",
    "cache_read": "čtení z cache", "output": "výstup", "tokens_total": "tokeny celkem",
    "steps": "kroky", "seconds": "čas (s)",
}
_REASON_CS = {
    "wrong_location": "jiné místo", "invalid_answer": "neplatná odpověď",
    "no_answer": "bez odpovědi", "cap_steps": "strop kroků", "cap_tokens": "strop tokenů",
    "cap_time": "strop času",
}


def fmt(value: Any) -> str:
    """A number as the Markdown shows it: Czech decimal comma, unknown and missing named."""
    if value is None:
        return "—"
    if value == UNKNOWN:
        return "neznámý"
    if isinstance(value, bool):
        return "ano" if value else "ne"
    if isinstance(value, float):
        if value.is_integer():
            return str(int(value))
        # Values are rounded to at most 6 decimals when the dict is built.
        return f"{value:.6f}".rstrip("0").replace(".", ",")
    return str(value)


def _fmt_diff(value: Any) -> str:
    text = fmt(value)
    if isinstance(value, (int, float)) and not isinstance(value, bool) and value > 0:
        return "+" + text
    return text


def _cell(text: Any) -> str:
    return str(text).replace("|", "\\|").replace("\n", " ")


def _table(header: Sequence[str], rows: Sequence[Sequence[Any]]) -> list[str]:
    lines = ["| " + " | ".join(header) + " |", "|" + "---|" * len(header)]
    lines += ["| " + " | ".join(_cell(c) for c in row) + " |" for row in rows]
    return lines


def render_markdown(data: Mapping[str, Any]) -> str:
    """Render the report dict (never recomputing a number)."""
    exp = data["experiment"]
    ref, cmp_ = data["variants"]["reference"], data["variants"]["compared"]
    out: list[str] = [f"# Měření vyhledávání — {exp['id']}", ""]
    out += [f"**Stav pokusu:** {_STATUS_CS.get(exp['status'], exp['status'])}"]
    if exp["status_detail"]:
        out += ["", f"> {_cell(exp['status_detail'])}"]
    out += ["", f"Srovnávaná varianta „{cmp_}“ proti referenční „{ref}“. Výhra, prohra a "
            "remíza jsou z pohledu srovnávané varianty. Zpráva nestanoví práh úspěchu.", ""]
    out += _render_summary(data, ref, cmp_)
    out += _render_cases(data, ref, cmp_)
    out += _render_spoiled(data)
    out += _render_cache(data)
    out += _render_money(data, ref, cmp_)
    out += _render_preparation(data)
    out += _render_metadata(data)
    out += ["## Co toto měření netvrdí", ""] + [f"- {s}" for s in data["not_claimed"]] + [""]
    return "\n".join(out)


def _render_summary(data: Mapping[str, Any], ref: str, cmp_: str) -> list[str]:
    s = data["summary"]
    rows = [[label, t[WIN], t[LOSS], t[TIE], t[INCOMPLETE]]
            for label, t in (("všechny případy", s["all_cases"]),
                             ("bez označených případů", s["unflagged_cases"]))]
    out = ["## Souhrn", ""]
    out += _table(["případy", "výhry", "prohry", "remízy", "neúplné"], rows)
    out += ["",
            "Označené případy (do řádku „bez označených případů“ se nepočítají): "
            f"použité při ladění karet: {', '.join(s['used_for_card_tuning']) or 'žádné'}; "
            f"zadání jmenuje místo: {', '.join(s['prompt_names_location']) or 'žádné'}.", ""]
    return out


def _render_cases(data: Mapping[str, Any], ref: str, cmp_: str) -> list[str]:
    plain = [c for c in data["cases"] if not (c["used_for_card_tuning"]
                                              or c["prompt_names_location"])]
    flagged = [c for c in data["cases"] if c not in plain]
    out = ["## Po případech", "",
           "Úspěchy jsou ze tří čistých běhů. Hlavní mediány jdou přes všechny čisté běhy; běh "
           "ukončený stropem v nich nese hodnotu stropu u kroků, času a tokenů celkem "
           "(srovnávací konvence, ne skutečná spotřeba). Kategorie tokenů jsou u všech běhů "
           "naměřené, u běhů ukončených stropem jde o dolní odhad, protože strop se vztahuje "
           "k součtu. Mediány jen přes úspěšné běhy jsou naměřené hodnoty.", ""]
    out += _case_table(plain, "### Případy bez označení")
    out += _case_table(flagged, "### Označené případy (použité při ladění karet nebo zadání "
                                "jmenuje místo)")
    for case in data["cases"]:
        out += _case_detail(case)
    return out


def _case_table(cases: Sequence[Mapping[str, Any]], title: str) -> list[str]:
    out = [title, ""]
    if not cases:
        return out + ["Žádné.", ""]
    rows = []
    for c in cases:
        r, k = c["variants"]
        flags = ", ".join(f for f, on in (("ladění", c["used_for_card_tuning"]),
                                          ("jmenuje místo", c["prompt_names_location"])) if on)
        rows.append([c["case_id"], flags or "—", _successes(r), _successes(k),
                     _RESULT_CS[c["result"]]])
    out += _table(["případ", "označení", f"úspěchy {r['variant']}", f"úspěchy {k['variant']}",
                   "výsledek"], rows)
    return out + [""]


def _successes(variant: Mapping[str, Any]) -> str:
    return f"{variant['successes']}/{variant['clean_runs']}"


def _case_detail(case: Mapping[str, Any]) -> list[str]:
    ref, cmp_ = case["variants"]
    out = [f"#### Případ {case['case_id']} — {_RESULT_CS[case['result']]}", ""]
    header = ["metrika", f"{ref['variant']}: medián", f"{cmp_['variant']}: medián",
              "rozdíl", f"{ref['variant']}: jen úspěšné", f"{cmp_['variant']}: jen úspěšné"]
    rows = [[_METRIC_CS[m], fmt(ref["medians_all"][m]), fmt(cmp_["medians_all"][m]),
             _fmt_diff(case["difference_compared_minus_reference"][m]),
             fmt(ref["medians_success"][m]), fmt(cmp_["medians_success"][m])] for m in METRICS]
    out += _table(header, rows)
    out += [""]
    for v in (ref, cmp_):
        reasons = ", ".join(_REASON_CS.get(r, r or "?") for r in v["failure_reasons"]) or "žádné"
        out += [f"- {v['variant']}: čisté běhy {v['clean_runs']}, úspěchy {v['successes']}, "
                f"ukončené stropem {v['capped_runs']}, zkažené pokusy {v['spoiled_attempts']}, "
                f"neúspěchy: {reasons}" + ("" if v["complete"] else " — neúplné")]
    if case["result"] == TIE:
        out += ["- Remíza: rozdíl nákladů je uvedený bez rozhodnutí."]
    return out + [""]


def _render_spoiled(data: Mapping[str, Any]) -> list[str]:
    out = ["## Zkažené pokusy", ""]
    if data["spoiled_attempts"]:
        out += _table(["pořadí", "případ", "varianta", "pokus", "důvody"],
                      [[s["order"], s["case_id"], s["variant"], s["attempt"],
                        "; ".join(s["reasons"])] for s in data["spoiled_attempts"]])
    else:
        out += ["Žádné."]
    out += ["", "## Chyby infrastruktury", ""]
    if data["infrastructure_errors"]:
        out += ["Tyto běhy se nikam nepočítají; pokus se při první z nich zastavil.", ""]
        out += _table(["pořadí", "případ", "varianta", "důvod"],
                      [[e["order"], e["case_id"], e["variant"], e["reason"]]
                       for e in data["infrastructure_errors"]])
    else:
        out += ["Žádné."]
    return out + [""]


def _render_cache(data: Mapping[str, Any]) -> list[str]:
    out = ["## Čtení z cache po pořadí běhu", "",
           "Cache poskytovatele se mezi běhy nemaže; pořadí ukazuje, jestli pozdější běhy "
           "těží z dřívějších.", ""]
    out += _table(["pořadí", "případ", "varianta", "stav", "čtení z cache"],
                  [[r["order"], r["case_id"], r["variant"], _STATE_CS.get(r["state"], r["state"]),
                    fmt(r["cache_read"]) if r["cache_read"] is not None else "neznámý"]
                   for r in data["cache_read_by_order"]])
    return out + [""]


def _render_money(data: Mapping[str, Any], ref: str, cmp_: str) -> list[str]:
    money = data["money"]
    out = ["## Peníze", ""]
    if money is None:
        return out + ["Peníze nejsou uvedené: tarif není zapsaný.", ""]
    cur = money["currency"]
    out += [f"Tarif ({cur} za milion tokenů): "
            + ", ".join(f"{_METRIC_CS[c]} {fmt(money['per_million'][c])}"
                        for c in TOKEN_CATEGORIES)
            + (f"; zdroj: {money['source']}" if money["source"] else "") + ".",
            "Cena vychází z naměřených kategorií tokenů (u běhů ukončených stropem dolní odhad).",
            ""]
    rows = []
    for case in data["cases"]:
        r, k = case["variants"]
        rows.append([case["case_id"], fmt(r["cost_median_all"]), fmt(k["cost_median_all"]),
                     fmt(r["cost_median_success"]), fmt(k["cost_median_success"])])
    out += _table(["případ", f"{ref}: medián", f"{cmp_}: medián", f"{ref}: jen úspěšné",
                   f"{cmp_}: jen úspěšné"], rows)
    out += ["", f"Celkem za všechny běhy pokusu: {fmt(money['total_all_runs'])} {cur}.", ""]
    return out


def _render_preparation(data: Mapping[str, Any]) -> list[str]:
    out = ["## Jednorázové náklady přípravy", "",
           "Do mediánů běhů se nepočítají.", ""]
    rows = []
    for p in data["preparation"]:
        tokens = p["tokens"]
        total = "—"
        if tokens is not None:
            values = [tokens[c] for c in TOKEN_CATEGORIES]
            total = "neznámý" if any(v is None for v in values) else str(sum(values))
        rows.append([p["step"], fmt(p["seconds"]), total, p["note"] or "—"])
    out += _table(["krok", "čas (s)", "tokeny", "poznámka"], rows) if rows else ["Žádné."]
    return out + [""]


def _render_metadata(data: Mapping[str, Any]) -> list[str]:
    exp = data["experiment"]
    cap = exp["cap"]
    out = ["## Metadata pokusu", ""]
    out += [f"- Zahájení: {exp['started_at']}" + (f", konec: {exp['finished_at']}"
                                                  if exp["finished_at"] else ""),
            f"- Model: {exp['model']}; adaptér: {exp['adapter'] or '—'}; verze CLI agenta: "
            f"{exp['agent_version'] or 'neznámá'}",
            f"- Strop běhu: čas {fmt(cap['seconds'])} s, kroky {cap['steps']}, tokeny {cap['tokens']}",
            f"- Rozvrh: semínko {exp['seed']}, souběh {exp['concurrency']}, "
            f"{exp['runs_per_variant']} čisté běhy na případ a variantu, nejvýš "
            f"{exp['max_replacements']} náhrady zkažených běhů, bez zahřívacích běhů",
            f"- Základní revize: {exp['base_revision']}; otisk stromu: {exp['tree_fingerprint']}",
            f"- Případy: {', '.join(c['id'] for c in exp['cases'])}", ""]
    for v in exp["variants"]:
        out += [f"### Varianta {v['name']}", ""]
        receipt = v["corpus_receipt"]
        if receipt is None:
            out += ["- Korpus: žádný"]
        else:
            out += [f"- Korpus: {v['corpus_dir']}",
                    f"  - otisk korpusu: {receipt['corpus_fingerprint']}",
                    f"  - verze RA: {receipt['ra_version']}; volby: "
                    f"{' '.join(receipt['options']) or 'žádné'}",
                    f"  - verze schématu manifestu: {fmt(v['manifest_schema_version'])}; "
                    f"formát karet: {v['card_format'] or 'neuvedený'}"]
        out += ["- Znění zadání (místo `{zadani_pripadu}` stojí zadání případu):", "",
                "````text", v["prompt_template"], "````", ""]
        out += _table(["případ", "otisk zadání (SHA-256)"],
                      [[case_id, digest] for case_id, digest in sorted(v["prompt_sha256"].items())])
        out += [""]
    return out


# --- files -------------------------------------------------------------------

def write_report(directory: str | os.PathLike[str], info: ExperimentInfo,
                 results: Sequence[RunResult]) -> dict[str, Any]:
    """Write `report.json` and `report.md` into the experiment directory."""
    data = build_report(info, results)
    target = Path(directory)
    write_text_atomic(target / "report.json",
                      json.dumps(data, ensure_ascii=False, indent=2, sort_keys=True) + "\n")
    write_text_atomic(target / "report.md", render_markdown(data))
    return data


def write_text_atomic(path: Path, text: str) -> None:
    """Replace `path` in one step, so a crash never leaves half a file behind."""
    fd, tmp = tempfile.mkstemp(dir=path.parent, prefix=f".{path.name}.", suffix=".tmp")
    try:
        with os.fdopen(fd, "w", encoding="utf-8", newline="\n") as handle:
            handle.write(text)
        os.replace(tmp, path)
    except BaseException:
        if os.path.exists(tmp):
            os.unlink(tmp)
        raise
