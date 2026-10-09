"""The card check: does the corpus state the facts of a question, and not its falsehoods (U5).

For every fact and forbidden claim of a catalog question the check finds the
documents of the fact's places in the corpus (`corpus.CorpusIndex.locate`),
gives the judge (`judge.assess`) the right blocks of each document, and
combines the verdicts into one result per fact and role. No agent runs here;
the only model is the judge. The check does not call `prepare.load_corpus`: it
reads the corpus through its index and takes the revision from the receipt or
from `coverage.json` (KTD2), so it works on a corpus without a receipt.

Decisions this module implements (the plan's R6-R9, KTD3-KTD5):

* **Revision first.** `build_plan` refuses (`CorpusError`) a corpus whose
  revision differs from an item's commit before anything else; the judge is
  never called for a mismatching corpus.
* **Places are verified.** A place the corpus cannot confirm gives the fact
  (or claim) the result `místo nenalezeno v korpusu`; the judge is not called
  for it.
* **One unit per fact (claim), role and document.** A fact of role `ramcova`
  is checked for the roles `podpora` and `vyvojar` on the cards and maps of its
  places; a fact of role `misto` only for `vyvojar`, on every document of its
  places (cards, details, maps, others). A forbidden claim is judged on every
  document of its own places (it has no role).
* **Blocks.** A document for which `anchors.needs_preselection` holds (a
  detail, a document without a level, anything over the size threshold) gives
  the judge only the blocks bound to the places (`anchors.bound_blocks`,
  merged in document order for a fact with several places). Any other document
  goes whole; once the judge accepts a sentence (`VYJADRUJE`, `OBSAHUJE`) it
  is checked that the quoted block binds to a place of the fact
  (`anchors.citation_binds`), and a sentence that does not is a disputed unit
  with the state `vyjadřuje, ale bez vazby`. A preselected document without a
  bound block is not judged (`bez vázaného bloku`).
* **Whole flow.** A `cely_tok` fact is judged only on a document that has bound
  blocks for *all* places of the flow; blocks of different documents are never
  joined.
* **Results** (`derive_results`, a pure function of the plan and the units, so
  a later owner's decision that overwrites a unit's `final_label` can be
  applied and the results derived again): a fact is `v rozporu` when a
  forbidden claim that refers to it got the final label `OBSAHUJE` on a
  document selected for the fact and its role (this beats `přítomen` and
  `chybí`; `other_present` says whether the fact is present elsewhere);
  otherwise `přítomen` when a document got the final `VYJADRUJE`; otherwise
  `sporné` while a unit is still disputed; otherwise `chybí, hodnotitel
  nenašel` when the judge read a document and found nothing, and
  `chybí, bez vázaného bloku` when there was nothing to give the judge.

`measurement.json` (`write_measurement`) records the plan, every unit with its
verdicts and, once complete, the results; an interrupted or failed measurement
is saved with the status `nedokonceno` and continues from the judge's cache on
the next run.

The same machinery judges an agent's answer to a question (KTD10,
`build_answer_plan`, `run_answer_check`): the answer of one run is a document
of its own (`odpoved:<run>`, path "odpověď agenta"), read as a map
(`anchors.parse_answer`: a block with a cited anchor binds by it, a block
without one by the name of the type or member) and handed to the judge whole
(an answer is small; one over the size threshold goes as bound blocks). For
the experiment's role (`podpora` / `vyvojar`) only the facts that role reads
are judged (`FACT_ROLE_AUDIENCES`: `ramcova` facts for both, `misto` facts for
the developer) and every forbidden claim. A run without an answer is not judged:
its facts are `chybí, bez odpovědi` (counted as missing, kept apart) and its
claims `bez odpovědi`. Units, fact plans and results carry the `variant` and the
`run` (the order in the experiment) of the answer, so a later report counts the
presence `k/3` per variant; they are left out of the JSON when unset, so the
card check's output is unchanged. A unit of a corpus document also records the size in bytes of
the document's file (`document_bytes`) and whether it is the card that owns one of the unit's
places (`owner_card`), and a measurement over a corpus with a receipt records the receipt's
RA build (`ucenka`): the report of the answers (`answers_report.py`) states them.
"""
from __future__ import annotations

import hashlib
import json
import os
import tempfile
from dataclasses import dataclass, field, replace
from pathlib import Path
from typing import Any, Callable, Iterable, Sequence

from agents.judge_cli import JudgeInfrastructureError
from anchors import (LEVEL_ANSWER, PRESELECT_THRESHOLD_BYTES, BoundBlock, ParsedDocument,
                     bound_blocks, citation_binds, needs_preselection, parse_answer,
                     parse_document)
from catalog import Catalog, Fact, ForbiddenClaim, Place, Question
from corpus import IDENTITY_FROM_RECEIPT, CorpusDoc, CorpusIndex, check_commit
from prepare import receipt_path
from judge import (KIND_FACT, KIND_FORBIDDEN, PROMPT_VERSION, Assessment, JudgeCache,
                   JudgeCacheError, JudgeRunner, JudgeSelection, JudgeUnit, assess)

MEASUREMENT_FORMAT_VERSION = 1
MEASUREMENT_FILE = "measurement.json"
JUDGE_CACHE_DIR = "judge-cache"

STATUS_DONE = "hotovo"
STATUS_INCOMPLETE = "nedokonceno"

ROLE_SUPPORT = "podpora"
ROLE_DEVELOPER = "vyvojar"
# Whom a fact of each catalog role is checked for.
FACT_ROLE_AUDIENCES = {"ramcova": (ROLE_SUPPORT, ROLE_DEVELOPER), "misto": (ROLE_DEVELOPER,)}

MODE_BOUND = "vázané bloky"
MODE_WHOLE = "celý dokument"

STATE_PENDING = "čeká"
STATE_ASSESSED = "posouzeno"
STATE_UNBOUND_FACT = "vyjadřuje, ale bez vazby"
STATE_UNBOUND_CLAIM = "obsahuje, ale bez vazby"
STATE_NO_BOUND_BLOCK = "bez vázaného bloku"
UNBOUND_STATES = (STATE_UNBOUND_FACT, STATE_UNBOUND_CLAIM)

LABEL_PRESENT = "VYJADRUJE"
LABEL_ABSENT = "NEVYJADRUJE"
LABEL_CONTAINS = "OBSAHUJE"
LABEL_NOT_CONTAINS = "NEOBSAHUJE"
LABEL_UNKNOWN = "NELZE"

DECIDED_BY_MODEL = None  # `UnitResult.decided_by` of a label nobody overwrote

FACT_PRESENT = "přítomen"
FACT_MISSING = "chybí, hodnotitel nenašel"
FACT_MISSING_UNBOUND = "chybí, bez vázaného bloku"
FACT_CONFLICT = "v rozporu"
FACT_DISPUTED = "sporné"
FACT_PLACE_UNKNOWN = "místo nenalezeno v korpusu"
FACT_NO_ANSWER = "chybí, bez odpovědi"

CLAIM_FOUND = "nalezeno"
CLAIM_ABSENT = "nenalezeno"
CLAIM_DISPUTED = "sporné"
CLAIM_UNBOUND = "bez vázaného bloku"
CLAIM_PLACE_UNKNOWN = "místo nenalezeno v korpusu"
CLAIM_NO_ANSWER = "bez odpovědi"

ANSWER_PATH = "odpověď agenta"
ANSWER_TITLE = "Odpověď agenta"
ANSWER_SLOT = "odpověď"

FLOW_NOTE = "dokument nemá vázaný blok ke všem místům toku"


# --- the model -----------------------------------------------------------------------

@dataclass(frozen=True)
class UnitResult:
    """One unit (a fact or a claim, a role and a document) and what became of it.

    `model_label` is the judge's label, `final_label` the one the results use:
    equal to it unless the unit is disputed (then `NELZE`) or an owner's
    decision overwrote it (`with_decision`; `decided_by` names who). `blocks`
    holds the preselected blocks, or None when the whole document went to the
    judge (`block_count` blocks).
    """

    id: str
    question_id: str
    kind: str
    claim_id: str
    role: str | None
    document: str
    slot: str
    title: str
    mode: str
    blocks: tuple[dict[str, Any], ...] | None
    block_count: int
    state: str = STATE_PENDING
    note: str | None = None
    run_labels: tuple[str, ...] = ()
    quote: str | None = None
    partial: bool | None = None
    binding: dict[str, Any] | None = None
    disputed: bool = False
    dispute_reason: str | None = None
    model_label: str | None = None
    final_label: str | None = None
    decided_by: str | None = DECIDED_BY_MODEL
    # What the owner's review shows (U7): the texts and places of the unit.
    question_text: str | None = None
    claim_text: str | None = None
    document_path: str | None = None
    document_text: str | None = None  # the whole document; only when `mode` is MODE_WHOLE
    places: tuple[dict[str, Any], ...] = ()
    # An agent's answer (not a corpus document): the variant and the run it came from.
    variant: str | None = None
    run: int | None = None
    # The report's sizes (U8), only for a corpus document: the size in bytes of its file, and
    # whether it is the card that owns one of the unit's places (`corpus.PlaceDocuments.owner_card`).
    document_bytes: int | None = None
    owner_card: bool | None = None

    @property
    def open_dispute(self) -> bool:
        """Disputed and not yet decided by anyone."""
        return self.disputed and self.decided_by is None

    def with_decision(self, label: str, decided_by: str) -> "UnitResult":
        """The unit with its final label overwritten by a decision (the model's stays)."""
        if not decided_by:
            raise ValueError("Rozhodnutí potřebuje, kdo rozhodl.")
        return replace(self, final_label=label, decided_by=decided_by)

    def to_json(self) -> dict[str, Any]:
        out: dict[str, Any] = {
            "id": self.id,
            "question_id": self.question_id,
            "kind": self.kind,
            "claim_id": self.claim_id,
            "role": self.role,
            "document": self.document,
            "slot": self.slot,
            "title": self.title,
            "mode": self.mode,
            "blocks": None if self.blocks is None else list(self.blocks),
            "block_count": self.block_count,
            "state": self.state,
            "note": self.note,
            "run_labels": list(self.run_labels),
            "quote": self.quote,
            "partial": self.partial,
            "binding": self.binding,
            "disputed": self.disputed,
            "dispute_reason": self.dispute_reason,
            "model_label": self.model_label,
            "final_label": self.final_label,
            "decided_by": self.decided_by,
            "question_text": self.question_text,
            "claim_text": self.claim_text,
            "document_path": self.document_path,
            "document_text": self.document_text,
            "places": list(self.places),
        }
        out.update(_origin_json(self.variant, self.run))
        if self.document_bytes is not None:
            out["document_bytes"] = self.document_bytes
        if self.owner_card is not None:
            out["owner_card"] = self.owner_card
        return out


def _origin_json(variant: str | None, run: int | None) -> dict[str, Any]:
    """`variant` and `run` of an agent's answer, only when set (the card check has none)."""
    out: dict[str, Any] = {}
    if variant is not None:
        out["variant"] = variant
    if run is not None:
        out["run"] = run
    return out


@dataclass(frozen=True)
class PlannedUnit:
    """A unit before the judge: its record, and what the judge and the checks need."""

    record: UnitResult
    claim_text: str
    question_text: str
    blocks: tuple[str, ...]  # what the judge gets; empty when the unit is not judged
    places: tuple[Place, ...]
    parsed: ParsedDocument | None = field(default=None, compare=False, repr=False)

    @property
    def judged(self) -> bool:
        return self.record.state == STATE_PENDING


@dataclass(frozen=True)
class FactPlan:
    """The documents and units of one fact and one role (or why there are none)."""

    question_id: str
    fact_id: str
    role: str
    unverified: tuple[str, ...]
    documents: tuple[str, ...]
    unit_ids: tuple[str, ...]
    variant: str | None = None
    run: int | None = None
    no_answer: str | None = None  # why the run has no answer (an answer plan only)


@dataclass(frozen=True)
class ClaimPlan:
    """The documents and units of one forbidden claim (or why there are none)."""

    question_id: str
    claim_id: str
    fact_ids: tuple[str, ...]
    unverified: tuple[str, ...]
    documents: tuple[str, ...]
    unit_ids: tuple[str, ...]
    variant: str | None = None
    run: int | None = None
    no_answer: str | None = None


@dataclass(frozen=True)
class Plan:
    """Everything the check will ask the judge, in order, before the first call."""

    questions: tuple[Question, ...]
    facts: tuple[FactPlan, ...]
    claims: tuple[ClaimPlan, ...]
    units: tuple[PlannedUnit, ...]


@dataclass(frozen=True)
class FactResult:
    question_id: str
    fact_id: str
    role: str
    status: str
    other_present: bool | None
    documents: tuple[str, ...]
    present_in: tuple[str, ...]
    conflicts: tuple[dict[str, Any], ...]
    disputed_units: tuple[str, ...]
    unit_ids: tuple[str, ...]
    reasons: tuple[str, ...]
    variant: str | None = None
    run: int | None = None

    def to_json(self) -> dict[str, Any]:
        out: dict[str, Any] = {
            "question_id": self.question_id,
            "fact_id": self.fact_id,
            "role": self.role,
            "status": self.status,
            "other_present": self.other_present,
            "documents": list(self.documents),
            "present_in": list(self.present_in),
            "conflicts": list(self.conflicts),
            "disputed_units": list(self.disputed_units),
            "unit_ids": list(self.unit_ids),
            "reasons": list(self.reasons),
        }
        out.update(_origin_json(self.variant, self.run))
        return out


@dataclass(frozen=True)
class ClaimResult:
    question_id: str
    claim_id: str
    status: str
    found_in: tuple[dict[str, Any], ...]
    disputed_units: tuple[str, ...]
    unit_ids: tuple[str, ...]
    reasons: tuple[str, ...]
    fact_ids: tuple[str, ...] = ()
    variant: str | None = None
    run: int | None = None

    def to_json(self) -> dict[str, Any]:
        out: dict[str, Any] = {
            "question_id": self.question_id,
            "claim_id": self.claim_id,
            "status": self.status,
            "found_in": list(self.found_in),
            "disputed_units": list(self.disputed_units),
            "unit_ids": list(self.unit_ids),
            "reasons": list(self.reasons),
            "fact_ids": list(self.fact_ids),
        }
        out.update(_origin_json(self.variant, self.run))
        return out


@dataclass(frozen=True)
class Results:
    facts: tuple[FactResult, ...]
    claims: tuple[ClaimResult, ...]


@dataclass(frozen=True)
class Measurement:
    """One card-check measurement: inputs, judge, units and (when done) results."""

    id: str
    status: str
    corpus: dict[str, Any]
    items: tuple[dict[str, Any], ...]
    judge: dict[str, Any]
    reported_model: str
    agent_family: str
    prompt_version: str
    plan: Plan
    units: tuple[UnitResult, ...]
    extra: dict[str, Any] = field(default_factory=dict)  # more top-level keys (answer check)

    def results(self) -> Results:
        return derive_results(self.plan, self.units)

    def to_json(self) -> dict[str, Any]:
        out: dict[str, Any] = {
            "format_version": MEASUREMENT_FORMAT_VERSION,
            "id": self.id,
            "status": self.status,
            "corpus": self.corpus,
            "items": list(self.items),
            "judge": self.judge,
            "reported_model": self.reported_model,
            "agent_family": self.agent_family,
            "prompt_version": self.prompt_version,
            "units": [u.to_json() for u in self.units],
        }
        if self.status == STATUS_DONE:
            results = self.results()
            out["results"] = {"facts": [r.to_json() for r in results.facts],
                              "claims": [r.to_json() for r in results.claims]}
        else:
            out["results"] = None
        out.update(self.extra)
        return out


# --- the plan --------------------------------------------------------------------------

def unit_id(question_id: str, claim_id: str, role: str | None, doc_id: str) -> str:
    return f"{question_id}::{claim_id}::{role or '-'}::{doc_id}"


def _merge_in_order(parsed: ParsedDocument, lists: Sequence[Sequence[BoundBlock]]
                    ) -> list[BoundBlock]:
    """The union of bound blocks of several places, in document order, each block once.

    Every list is a subsequence of the document's blocks (same texts), so one
    walk over the blocks with a pointer per list restores the order.
    """
    pointers = [0] * len(lists)
    out: list[BoundBlock] = []
    for block in parsed.blocks:
        taken = False
        for k, items in enumerate(lists):
            if pointers[k] < len(items) and items[pointers[k]].text == block.text:
                if not taken:
                    out.append(items[pointers[k]])
                    taken = True
                pointers[k] += 1
    return out


def _block_json(block: BoundBlock) -> dict[str, Any]:
    return {"text": block.text, "section": block.section, "anchor": block.anchor,
            "via": block.via}


def _unique(items: Iterable[str]) -> tuple[str, ...]:
    return tuple(dict.fromkeys(items))


class _Planner:
    def __init__(self, index: CorpusIndex) -> None:
        self.index = index
        self._located: dict[Place, Any] = {}
        self._parsed: dict[str, ParsedDocument] = {}
        self._texts: dict[str, str] = {}

    def locate(self, place: Place):
        found = self._located.get(place)
        if found is None:
            found = self._located[place] = self.index.locate(place)
        return found

    def parsed(self, doc: CorpusDoc) -> ParsedDocument:
        found = self._parsed.get(doc.id)
        if found is None:
            text = self._texts[doc.id] = self.index.read_document(doc)
            found = self._parsed[doc.id] = parse_document(
                text, doc_id=doc.id, title=doc.title, level=doc.level,
                known_files=self.index.files)
        return found

    def preselects(self, parsed: ParsedDocument) -> bool:
        """Whether the document goes to the judge as bound blocks (`anchors.needs_preselection`)."""
        return needs_preselection(parsed.level, parsed.size_bytes)

    def file_bytes(self, doc: CorpusDoc, parsed: ParsedDocument) -> int | None:
        """The size of the document's file in bytes (what the report counts), None if unknown."""
        try:
            return (self.index.corpus / doc.path).stat().st_size
        except OSError:
            return parsed.size_bytes

    def owns(self, doc: CorpusDoc, places: Sequence[Place]) -> bool | None:
        """Whether `doc` is the card that owns one of the places."""
        return any(self.locate(p).owner_card == doc.id for p in places)

    def unit(self, question: Question, kind: str, claim_id: str, claim_text: str,
             role: str | None, doc: CorpusDoc, places: Sequence[Place],
             whole_flow: bool) -> PlannedUnit:
        parsed = self.parsed(doc)
        per_place = [bound_blocks(parsed, p) for p in places]
        uncovered = [p for p, found in zip(places, per_place) if not found]
        preselect = self.preselects(parsed)
        mode = MODE_BOUND if preselect else MODE_WHOLE
        blocks: tuple[str, ...] = ()
        shown: tuple[dict[str, Any], ...] | None = None
        count = 0
        note: str | None = None
        if whole_flow and uncovered:
            note = f"{FLOW_NOTE} (bez vazby: {_place_label(uncovered[0])})"
        elif preselect:
            bound = _merge_in_order(parsed, per_place)
            if bound:
                blocks = tuple(b.text for b in bound)
                shown = tuple(_block_json(b) for b in bound)
                count = len(bound)
            else:
                note = "dokument nemá žádný blok vázaný k místu"
        elif parsed.blocks:
            blocks = tuple(b.text for b in parsed.blocks)
            count = len(blocks)
        else:
            note = "dokument nemá žádný blok"
        record = UnitResult(
            id=unit_id(question.id, claim_id, role, doc.id), question_id=question.id, kind=kind,
            claim_id=claim_id, role=role, document=doc.id, slot=doc.slot, title=doc.title,
            mode=mode, blocks=shown, block_count=count,
            state=STATE_PENDING if blocks else STATE_NO_BOUND_BLOCK, note=note,
            question_text=question.dotaz, claim_text=claim_text, document_path=doc.path,
            document_text=None if preselect else self._texts.get(doc.id),
            places=tuple(_place_json(p) for p in places),
            document_bytes=self.file_bytes(doc, parsed), owner_card=self.owns(doc, places))
        return PlannedUnit(record=record, claim_text=claim_text, question_text=question.dotaz,
                           blocks=blocks, places=tuple(places), parsed=parsed)


def _place_json(place: Place) -> dict[str, Any]:
    return {"soubor": place.soubor, "clen": place.clen, "typ": place.typ,
            "radky": list(place.radky)}


def _place_label(place: Place) -> str:
    return f"{place.soubor} {place.clen or place.typ} {place.radky[0]}-{place.radky[1]}"


def build_plan(catalog: Catalog, index: CorpusIndex) -> Plan:
    """Verify the corpus's revision and places and lay out every unit; nothing is judged.

    Raises `corpus.CorpusError` when the corpus's revision is not the commit of
    any question (before anything else is done).
    """
    for question in catalog.questions:
        check_commit(index.identity, question.commit)
    planner = _Planner(index)
    facts: list[FactPlan] = []
    claims: list[ClaimPlan] = []
    units: list[PlannedUnit] = []
    for question in catalog.questions:
        for fact in question.fakta:
            for role in FACT_ROLE_AUDIENCES[fact.role]:
                plan, planned = _plan_fact(planner, question, fact, role)
                facts.append(plan)
                units.extend(planned)
    for question in catalog.questions:
        for claim in question.zakazana_tvrzeni:
            plan, planned = _plan_claim(planner, question, claim)
            claims.append(plan)
            units.extend(planned)
    return Plan(questions=tuple(catalog.questions), facts=tuple(facts), claims=tuple(claims),
                units=tuple(units))


def _unverified(planner: _Planner, places: Sequence[Place]) -> tuple[list[Any], list[str]]:
    located = [planner.locate(p) for p in places]
    return located, [pd.reason or "místo nenalezeno v korpusu" for pd in located if not pd.found]


def _plan_fact(planner: _Planner, question: Question, fact: Fact, role: str
               ) -> tuple[FactPlan, list[PlannedUnit]]:
    located, problems = _unverified(planner, fact.mista)
    if problems:
        return FactPlan(question.id, fact.id, role, tuple(problems), (), ()), []
    docs: dict[str, CorpusDoc] = {}
    for pd in located:
        for hit in pd.for_role(fact.role):
            docs.setdefault(hit.doc.id, hit.doc)
    whole_flow = fact.tok is not None and fact.tok.whole
    planned = [planner.unit(question, KIND_FACT, fact.id, fact.text, role, doc, fact.mista,
                            whole_flow) for doc in docs.values()]
    return (FactPlan(question.id, fact.id, role, (), tuple(docs),
                     tuple(p.record.id for p in planned)), planned)


def _plan_claim(planner: _Planner, question: Question, claim: ForbiddenClaim
                ) -> tuple[ClaimPlan, list[PlannedUnit]]:
    located, problems = _unverified(planner, claim.mista)
    if problems:
        return ClaimPlan(question.id, claim.id, claim.fakty, tuple(problems), (), ()), []
    docs: dict[str, CorpusDoc] = {}
    for pd in located:
        for hit in pd.hits:
            docs.setdefault(hit.doc.id, hit.doc)
    planned = [planner.unit(question, KIND_FORBIDDEN, claim.id, claim.text, None, doc,
                            claim.mista, False) for doc in docs.values()]
    return (ClaimPlan(question.id, claim.id, claim.fakty, (), tuple(docs),
                      tuple(p.record.id for p in planned)), planned)


# --- the plan of an agent's answers (KTD10) --------------------------------------------

@dataclass(frozen=True)
class AnswerRun:
    """One stored run of a question: where it came from and its answer (None = no answer).

    `run` is the run's order in the experiment; `reason` says why a run has no answer."""

    question_id: str
    variant: str
    run: int
    text: str | None
    reason: str | None = None


def answer_doc_id(run: int) -> str:
    return f"odpoved:{run:03d}"


class _AnswerPlanner(_Planner):
    """The planner over answers: no corpus, an answer is read as a map (`parse_answer`)."""

    def __init__(self) -> None:
        super().__init__(None)  # type: ignore[arg-type]  # no index: documents are the answers

    def add(self, doc: CorpusDoc, text: str) -> None:
        self._texts[doc.id] = text
        self._parsed[doc.id] = parse_answer(text, doc_id=doc.id, title=doc.title)

    def parsed(self, doc: CorpusDoc) -> ParsedDocument:
        return self._parsed[doc.id]

    def preselects(self, parsed: ParsedDocument) -> bool:
        # An answer is small and goes whole, like a map; an oversized one as bound blocks.
        return parsed.size_bytes > PRESELECT_THRESHOLD_BYTES

    def file_bytes(self, doc: CorpusDoc, parsed: ParsedDocument) -> int | None:
        return None  # an answer is not a corpus document

    def owns(self, doc: CorpusDoc, places: Sequence[Place]) -> bool | None:
        return None


def build_answer_plan(catalog: Catalog, runs: Sequence[AnswerRun], role: str) -> Plan:
    """Lay out every unit for the stored runs of the questions, for the experiment's `role`.

    Facts are planned for the role only (`FACT_ROLE_AUDIENCES`); every forbidden claim is
    planned. A run without an answer gets plans without units (`no_answer`). Nothing is judged.
    """
    questions = {q.id: q for q in catalog.questions}
    planner = _AnswerPlanner()
    facts: list[FactPlan] = []
    claims: list[ClaimPlan] = []
    units: list[PlannedUnit] = []
    used: dict[str, Question] = {}
    for item in runs:
        question = questions[item.question_id]
        used[question.id] = question
        doc = CorpusDoc(id=answer_doc_id(item.run), kind=None, level=LEVEL_ANSWER, parent=None,
                        title=ANSWER_TITLE, path=ANSWER_PATH, header_path=None)
        answered = item.text is not None
        if answered:
            planner.add(doc, item.text)
        reason = None if answered else (item.reason or "bez odpovědi")
        origin = dict(variant=item.variant, run=item.run, no_answer=reason)

        def planned_unit(kind: str, claim_id: str, text: str, unit_role: str | None,
                         places: Sequence[Place], whole_flow: bool) -> PlannedUnit:
            planned = planner.unit(question, kind, claim_id, text, unit_role, doc, places,
                                   whole_flow)
            record = replace(planned.record, slot=ANSWER_SLOT, variant=item.variant, run=item.run)
            return replace(planned, record=record)

        for fact in question.fakta:
            if role not in FACT_ROLE_AUDIENCES[fact.role]:
                continue
            if not answered:
                facts.append(FactPlan(question.id, fact.id, role, (), (), (), **origin))
                continue
            whole_flow = fact.tok is not None and fact.tok.whole
            planned = planned_unit(KIND_FACT, fact.id, fact.text, role, fact.mista, whole_flow)
            facts.append(FactPlan(question.id, fact.id, role, (), (doc.id,), (planned.record.id,),
                                  **origin))
            units.append(planned)
        for claim in question.zakazana_tvrzeni:
            if not answered:
                claims.append(ClaimPlan(question.id, claim.id, claim.fakty, (), (), (), **origin))
                continue
            planned = planned_unit(KIND_FORBIDDEN, claim.id, claim.text, None, claim.mista, False)
            claims.append(ClaimPlan(question.id, claim.id, claim.fakty, (), (doc.id,),
                                    (planned.record.id,), **origin))
            units.append(planned)
    return Plan(questions=tuple(used.values()), facts=tuple(facts), claims=tuple(claims),
                units=tuple(units))


# --- judging a unit --------------------------------------------------------------------

def evaluate_unit(planned: PlannedUnit, selection: JudgeSelection, runner: JudgeRunner,
                  cache: JudgeCache) -> UnitResult:
    """Judge one unit and apply the citation check; a unit with no blocks is returned as it is."""
    record = planned.record
    if not planned.judged:
        return record
    judge_unit = JudgeUnit(kind=record.kind, claim_id=record.claim_id,
                           claim_text=planned.claim_text, question=planned.question_text,
                           blocks=planned.blocks)
    assessment = assess(judge_unit, selection, runner, cache)
    return _with_assessment(planned, assessment)


def _with_assessment(planned: PlannedUnit, assessment: Assessment) -> UnitResult:
    record = planned.record
    positive = LABEL_PRESENT if record.kind == KIND_FACT else LABEL_CONTAINS
    disputed, reason = assessment.disputed, assessment.dispute_reason
    state, final = STATE_ASSESSED, assessment.label
    binding: dict[str, Any] | None = None
    if assessment.label == positive and not disputed and planned.parsed is not None:
        for place in planned.places:
            found = citation_binds(planned.parsed, place, assessment.quote or "")
            if found is not None:
                binding = {"anchor": found.anchor, "section": found.section, "via": found.via,
                           "place": _place_label(place)}
                break
        if binding is None and record.mode == MODE_WHOLE:
            state = STATE_UNBOUND_FACT if record.kind == KIND_FACT else STATE_UNBOUND_CLAIM
            disputed = True
            reason = "citovaná věta se k místu neváže (kotva jejího bloku míjí řádky místa)"
    if disputed:
        final = LABEL_UNKNOWN
    return replace(record, state=state, run_labels=assessment.run_labels, quote=assessment.quote,
                   partial=assessment.partial, binding=binding, disputed=disputed,
                   dispute_reason=reason, model_label=assessment.label, final_label=final)


# --- the results -----------------------------------------------------------------------

def derive_results(plan: Plan, units: Iterable[UnitResult]) -> Results:
    """Fold the units' final labels into a result per fact and role and per claim."""
    by_id = {u.id: u for u in units}
    fact_results = [_fact_result(fp, plan, by_id) for fp in plan.facts]
    claim_results = [_claim_result(cp, by_id) for cp in plan.claims]
    return Results(facts=tuple(fact_results), claims=tuple(claim_results))


def _fact_result(fp: FactPlan, plan: Plan, by_id: dict[str, UnitResult]) -> FactResult:
    def result(status: str, **kwargs: Any) -> FactResult:
        base: dict[str, Any] = dict(other_present=None, documents=fp.documents, present_in=(),
                                    conflicts=(), disputed_units=(), unit_ids=fp.unit_ids,
                                    reasons=(), variant=fp.variant, run=fp.run)
        base.update(kwargs)
        return FactResult(fp.question_id, fp.fact_id, fp.role, status, **base)

    if fp.no_answer:
        return result(FACT_NO_ANSWER, reasons=(fp.no_answer,))
    if fp.unverified:
        return result(FACT_PLACE_UNKNOWN, reasons=fp.unverified)
    units = [by_id[i] for i in fp.unit_ids]
    present = tuple(u.document for u in units if u.final_label == LABEL_PRESENT)
    conflicts = []
    disputed = [u.id for u in units if u.open_dispute]
    for cp in plan.claims:
        if cp.question_id != fp.question_id or fp.fact_id not in cp.fact_ids:
            continue
        for uid in cp.unit_ids:
            claim_unit = by_id[uid]
            if claim_unit.document not in fp.documents:
                continue
            if claim_unit.final_label == LABEL_CONTAINS:
                conflicts.append({"claim_id": cp.claim_id, "unit_id": uid,
                                  "document": claim_unit.document, "quote": claim_unit.quote})
            elif claim_unit.open_dispute:
                disputed.append(uid)
    if conflicts:
        return result(FACT_CONFLICT, other_present=bool(present), present_in=present,
                      conflicts=tuple(conflicts), disputed_units=tuple(disputed))
    if present:
        return result(FACT_PRESENT, present_in=present, disputed_units=tuple(disputed))
    if disputed:
        return result(FACT_DISPUTED, disputed_units=tuple(disputed))
    if any(u.final_label == LABEL_ABSENT for u in units):
        return result(FACT_MISSING)
    reasons = tuple(f"{u.document}: {u.note}" for u in units if u.note) \
        or ("pro místa faktu není žádný dokument",)
    return result(FACT_MISSING_UNBOUND, reasons=reasons)


def _claim_result(cp: ClaimPlan, by_id: dict[str, UnitResult]) -> ClaimResult:
    units = [by_id[i] for i in cp.unit_ids]
    found = tuple({"document": u.document, "unit_id": u.id, "quote": u.quote}
                  for u in units if u.final_label == LABEL_CONTAINS)
    disputed = tuple(u.id for u in units if u.open_dispute)

    def result(status: str, reasons: tuple[str, ...] = ()) -> ClaimResult:
        return ClaimResult(cp.question_id, cp.claim_id, status, found, disputed, cp.unit_ids,
                           reasons, cp.fact_ids, cp.variant, cp.run)

    if cp.no_answer:
        return result(CLAIM_NO_ANSWER, (cp.no_answer,))
    if cp.unverified:
        return result(CLAIM_PLACE_UNKNOWN, cp.unverified)
    if found:
        return result(CLAIM_FOUND)
    if disputed:
        return result(CLAIM_DISPUTED)
    if any(u.final_label == LABEL_NOT_CONTAINS for u in units):
        return result(CLAIM_ABSENT)
    return result(CLAIM_UNBOUND, tuple(f"{u.document}: {u.note}" for u in units if u.note))


# --- running and saving ----------------------------------------------------------------

def corpus_identity_json(index: CorpusIndex) -> dict[str, Any]:
    identity = index.identity
    return {"revision": identity.revision, "source": identity.source, "label": identity.label,
            "tree_fingerprint": identity.tree_fingerprint}


def _receipt_extra(index: CorpusIndex) -> dict[str, Any]:
    """The build of the corpus from the runner's receipt (`ucenka`), when one lies beside it.

    The report names which RA build made the corpus; a corpus without a receipt has no
    such key and the report says the build is unknown. It is not part of the measurement's id.
    """
    if index.identity.source != IDENTITY_FROM_RECEIPT:
        return {}
    try:
        data = json.loads(receipt_path(index.corpus).read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return {}
    if not isinstance(data, dict):
        return {}
    keep = ("base_revision", "tree_fingerprint", "ra_version", "options", "corpus_fingerprint")
    return {"ucenka": {k: data.get(k) for k in keep}}


def _items_json(plan: Plan) -> tuple[dict[str, Any], ...]:
    return tuple({"id": q.id, "commit": q.commit, "otisk_potvrzeni": q.otisk_potvrzeni,
                  "kontrolni": q.kontrolni} for q in plan.questions)


def measurement_id(corpus: dict[str, Any], items: Sequence[dict[str, Any]],
                   judge_identity: dict[str, Any], agent_family: str) -> str:
    """Stable id of what is measured, so a repeated command continues the same measurement."""
    payload = {"agent_family": agent_family, "corpus": corpus, "items": list(items),
               "judge": judge_identity, "prompt_version": PROMPT_VERSION}
    encoded = json.dumps(payload, ensure_ascii=False, sort_keys=True).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()[:16]


def measurement_path(state_dir: str | os.PathLike[str]) -> Path:
    return Path(state_dir) / MEASUREMENT_FILE


def write_measurement(path: Path, measurement: Measurement) -> None:
    """Atomic write (temp file + `os.replace`); `format_version` is the first key."""
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(dir=path.parent, prefix=f".{path.name}.", suffix=".tmp")
    try:
        with os.fdopen(fd, "w", encoding="utf-8", newline="\n") as handle:
            json.dump(measurement.to_json(), handle, ensure_ascii=False, indent=2)
            handle.write("\n")
        os.replace(tmp, path)
    except BaseException:
        if os.path.exists(tmp):
            os.unlink(tmp)
        raise


def archive_foreign_measurement(path: Path, new_id: str) -> Path | None:
    """Move an existing `measurement.json` of another measurement aside; return where it went."""
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
        old_id = data.get("id") if isinstance(data, dict) else None
    except (OSError, ValueError):
        old_id = None
    if old_id == new_id and old_id is not None:
        return None
    if not path.is_file():
        return None
    target = path.with_name(f"measurement.{old_id or 'neznamy'}.json")
    counter = 1
    while target.exists():
        counter += 1
        target = path.with_name(f"measurement.{old_id or 'neznamy'}.{counter}.json")
    os.replace(path, target)
    return target


def run_card_check(catalog: Catalog, index: CorpusIndex, selection: JudgeSelection,
                   runner: JudgeRunner, cache: JudgeCache, *,
                   save: Callable[[Measurement], None] | None = None,
                   progress: Callable[[int, int, UnitResult], None] | None = None,
                   ) -> Measurement:
    """Run the whole card check and return the finished measurement.

    The corpus's revision is checked and the plan built before the first judge
    call. `save` receives the measurement before the first call, and again
    (status `nedokonceno`, units not yet judged still `čeká`) when a judge
    infrastructure or cache failure or a keyboard interrupt ends the run - the
    exception then propagates - and finally when the run completes.
    """
    plan = build_plan(catalog, index)
    return _run_plan(plan, corpus_identity_json(index), selection, runner, cache, save=save,
                     progress=progress, extra=_receipt_extra(index))


def run_answer_check(catalog: Catalog, runs: Sequence[AnswerRun], role: str,
                     corpus: dict[str, Any], extra: dict[str, Any], selection: JudgeSelection,
                     runner: JudgeRunner, cache: JudgeCache, *,
                     save: Callable[[Measurement], None] | None = None,
                     progress: Callable[[int, int, UnitResult], None] | None = None,
                     ) -> Measurement:
    """Judge the stored answers of the questions and return the finished measurement.

    `corpus` identifies what was measured (it goes into the measurement's id and its
    `corpus` key: the experiment and its variants' corpora), `extra` is recorded as more
    top-level keys. Saving, interruption and the cache work as in `run_card_check`.
    """
    plan = build_answer_plan(catalog, runs, role)
    return _run_plan(plan, corpus, selection, runner, cache, save=save, progress=progress,
                     extra=extra)


def _run_plan(plan: Plan, corpus: dict[str, Any], selection: JudgeSelection,
              runner: JudgeRunner, cache: JudgeCache, *,
              save: Callable[[Measurement], None] | None,
              progress: Callable[[int, int, UnitResult], None] | None,
              extra: dict[str, Any] | None = None) -> Measurement:
    items = _items_json(plan)
    identity = selection.identity()
    base = dict(id=measurement_id(corpus, items, identity, selection.agent_family),
                corpus=corpus, items=items, judge=identity, reported_model=selection.smoke_model,
                agent_family=selection.agent_family, prompt_version=PROMPT_VERSION, plan=plan,
                extra=extra or {})
    results: list[UnitResult] = [p.record for p in plan.units]

    def snapshot(status: str) -> Measurement:
        return Measurement(status=status, units=tuple(results), **base)

    if save is not None:
        save(snapshot(STATUS_INCOMPLETE))
    try:
        for number, planned in enumerate(plan.units):
            results[number] = evaluate_unit(planned, selection, runner, cache)
            if progress is not None:
                progress(number + 1, len(plan.units), results[number])
    except (JudgeInfrastructureError, JudgeCacheError, KeyboardInterrupt):
        if save is not None:
            save(snapshot(STATUS_INCOMPLETE))
        raise
    finished = snapshot(STATUS_DONE)
    if save is not None:
        save(finished)
    return finished
