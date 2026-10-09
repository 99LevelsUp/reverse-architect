"""The card check (U5): planning, judging, results and `measurement.json`.

Every corpus is synthetic and built here (`build_corpus`, in either manifest
shape); the judge is a scripted runner that never starts a process. All names
are generic (`EditorView`, `EditorViewNew`, `ExternalCheck`, ...).
"""
import json
import re
import shutil
import tempfile
import unittest
from dataclasses import dataclass
from pathlib import Path
from unittest import mock

import card_check
from agents.judge_cli import CliReply, JudgeInfrastructureError
from card_check import (FACT_CONFLICT, FACT_DISPUTED, FACT_MISSING, FACT_MISSING_UNBOUND,
                        FACT_PLACE_UNKNOWN, FACT_PRESENT, MODE_BOUND, MODE_WHOLE,
                        STATE_NO_BOUND_BLOCK, STATE_UNBOUND_FACT, STATUS_DONE,
                        STATUS_INCOMPLETE, build_plan, derive_results, run_card_check,
                        write_measurement)
from catalog import confirmation_fingerprint, parse_catalog
from corpus import CorpusError, open_corpus
from judge import JudgeCache, JudgeSelection, JudgeSpec

REV = "a" * 40
OTHER_REV = "b" * 40
NS = "sym:csharp:Acme#"
COMMAND_MARKER = "--tajny-prepinac-hodnotitele"

EV, EVN = "src/Acme/EditorView.cs", "src/Acme/EditorViewNew.cs"
EVM, EVMN = "src/Acme/EditorViewModel.cs", "src/Acme/EditorViewModelNew.cs"
EXT = "src/Acme/ExternalCheck.cs"


# --- a synthetic corpus -------------------------------------------------------------------

@dataclass
class TypeSpec:
    name: str
    file: str
    start: int
    end: int
    members: tuple = ()  # (name with signature, first line, last line)
    card: str = ""
    detail: str | None = None

    @property
    def key(self):
        return NS + self.name

    @property
    def card_path(self):
        return f"cards/type/{self.name}.md"

    @property
    def detail_path(self):
        return f"cards/detail/{self.name}.md"


@dataclass
class ExtraDoc:
    record: dict
    text: str = ""


def _declaration(file_id, start, end, name):
    prov = {"file_id": file_id, "line": start, "extractor": "csharp", "tier": "structural",
            "resolution": "heuristic"}
    if end != start:
        prov["end_line"] = end
    return [{"kind": kind, "value": name, "provenance": dict(prov), "dissent": []}
            for kind in ("symbol_name", "symbol_kind", "symbol_signature")]


def build_corpus(root, types, *, lines=False, revision=REV, extra=()):
    """Write a corpus of `types` under `root`: manifest (one file, or by lines), docs, coverage."""
    root = Path(root)
    strings, nodes, documents = [], [], []

    def intern(text):
        if text not in strings:
            strings.append(text)
        return strings.index(text)

    for spec in types:
        file_id = intern(spec.file)
        nodes.append({"key_id": intern(spec.key), "kind": "symbol",
                      "facts": _declaration(file_id, spec.start, spec.end, spec.name)})
        for member, first, last in spec.members:
            nodes.append({"key_id": intern(f"{spec.key}.{member}"), "kind": "symbol",
                          "facts": _declaration(file_id, first, last, member)})
        documents.append({"id": spec.key, "kind": "card:type", "level": "card",
                          "parent": None, "header": {"path": spec.file}, "title": spec.name,
                          "path": spec.card_path})
        (root / spec.card_path).parent.mkdir(parents=True, exist_ok=True)
        (root / spec.card_path).write_text(spec.card, encoding="utf-8")
        if spec.detail is not None:
            documents.append({"id": f"detail:{spec.key}", "kind": "card:detail",
                              "level": "detail", "parent": spec.key,
                              "header": {"path": spec.file}, "title": f"Detail of {spec.name}",
                              "path": spec.detail_path})
            (root / spec.detail_path).parent.mkdir(parents=True, exist_ok=True)
            (root / spec.detail_path).write_text(spec.detail, encoding="utf-8")
    for doc in extra:
        documents.append(doc.record)
        target = root / doc.record["path"]
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(doc.text, encoding="utf-8")
    if lines:
        (root / "manifest-strings.json").write_text(
            json.dumps({"schema_version": 10, "strings": strings}), encoding="utf-8")
        for name, records in (("manifest-nodes.jsonl", nodes),
                              ("manifest-documents.jsonl", documents)):
            (root / name).write_text("".join(json.dumps(r) + "\n" for r in records),
                                     encoding="utf-8")
    else:
        (root / "manifest.json").write_text(json.dumps(
            {"schema_version": 11, "strings": strings, "documents": documents, "nodes": nodes}),
            encoding="utf-8")
    (root / "coverage.json").write_text(json.dumps(
        {"schema_version": 6, "repo_revision": revision,
         "files": [{"path": spec.file} for spec in types]}), encoding="utf-8")
    return root


def card(name, file, start, end, body):
    return (f"# {name}\n\n**public class** · `{file}:{start}-{end}`\n\n{body}\n")


def a1_types():
    """Five types shaped like the acceptance case A1 (two editors, view models, a check)."""
    return [
        TypeSpec("EditorView", EV, 3, 80, (("Check(bool)", 10, 30), ("Save()", 32, 40)),
                 card("EditorView", EV, 3, 80, (
                     "## Check\n\n"
                     f"- Uloží dokument bez kontroly — `{EV}:32-40`\n"
                     f"- Ověří mřížku před uložením — `{EV}:10-30`\n"))),
        TypeSpec("EditorViewNew", EVN, 3, 90, (("Load()", 10, 40), ("OnCellChanged()", 42, 60)),
                 card("EditorViewNew", EVN, 3, 90, (
                     "## Events\n\n"
                     f"- Změna řádku gridu se ukládá okamžitě — `{EVN}:42-60`\n"
                     f"- Nový editor uloží dokument vždy po kontrole — `{EVN}:10-40`\n"))),
        TypeSpec("EditorViewModel", EVM, 3, 50, (("Run()", 10, 30),),
                 card("EditorViewModel", EVM, 3, 50, (
                     "## Run\n\n"
                     f"- Krok 2: spustí kontrolu — `{EVM}:15`\n"))),
        TypeSpec("EditorViewModelNew", EVMN, 3, 60, (("RunNew()", 10, 45),),
                 card("EditorViewModelNew", EVMN, 3, 60, (
                     "## RunNew\n\n"
                     f"- Spustí tok nového editoru — `{EVMN}:10-45`\n"))),
        TypeSpec("ExternalCheck", EXT, 3, 120, (("Run()", 20, 60),),
                 card("ExternalCheck", EXT, 3, 120, (
                     "## Run\n\n"
                     f"- Zavolá externí registr a uloží verdikt — `{EXT}:30-35`\n"
                     f"- Odešle dokument — `{EXT}:90-95`\n"
                     f"- Krok 3: přečte verdikt — `{EXT}:30`\n"))),
    ]


def doc_id(name):
    return NS + name


# --- a synthetic catalog --------------------------------------------------------------------

def place(soubor, clen=None, typ=None, radky=1):
    out = {"soubor": soubor, "radky": radky}
    if clen is not None:
        out["clen"] = clen
    if typ is not None:
        out["typ"] = typ
    return out


def fact(fid, text, mista, role="ramcova", tok=None):
    out = {"id": fid, "text": text, "role": role, "mista": list(mista)}
    if tok is not None:
        out["tok"] = tok
    return out


def claim(cid, text, mista, fakty=()):
    return {"id": cid, "text": text, "mista": list(mista), "fakty": list(fakty)}


def question_json(facts, claims=(), qid="Q1", commit=REV, kontrolni=False):
    facts, claims = list(facts), list(claims)
    return {"druh": "otazka", "id": qid, "dotaz": "Jak editor ukládá dokument?",
            "ramcova_odpoved": "Ukládá po kontrole.", "commit": commit, "fakta_potvrzena": True,
            "pseudonymizace_potvrzena": True, "kontrolni": kontrolni,
            "otisk_potvrzeni": confirmation_fingerprint(facts, claims), "fakta": facts,
            "zakazana_tvrzeni": claims}


def catalog_json(*questions, base=REV):
    return {"format_version": 2, "base_revision": base, "cases": list(questions)}


def make_catalog(*questions, base=REV):
    return parse_catalog(catalog_json(*questions, base=base))


# --- the scripted judge ---------------------------------------------------------------------

@dataclass
class Rule:
    claim: str   # substring of the claim text
    block: str   # substring of a block; also the quotation
    label: str


@dataclass
class Call:
    claim: str
    blocks: list
    forbidden: bool


class ScriptedJudge:
    """A `JudgeRunner` that answers from rules and records every non-smoke call.

    `fail_after`: the call with that number (0-based, counted over non-smoke
    calls) raises `fail` instead of answering and is not recorded.
    """

    def __init__(self, rules=(), model="model-1", fail_after=None, fail=None):
        self.rules = list(rules)
        self.model = model
        self.calls = []
        self.smoke_calls = 0
        self.fail_after = fail_after
        self.fail = fail or JudgeInfrastructureError("scénář: hodnotitel spadl")

    def __call__(self, spec, prompt, siblings):
        if prompt.startswith("KONTROLA HODNOTITELE"):
            self.smoke_calls += 1
            return CliReply(text=json.dumps({"stav": "OK", "obsah": ""}), model=self.model,
                            raw="{}")
        if self.fail_after is not None and len(self.calls) == self.fail_after:
            raise self.fail
        claim_text = re.search(r"DATA-\w+ TVRZENÍ\n(.*?)\nDATA-\w+ KONEC TVRZENÍ", prompt,
                               re.S).group(1)
        blocks = re.findall(r"DATA-\w+ BLOK \d+\n(.*?)\nDATA-\w+ KONEC BLOKU \d+", prompt, re.S)
        forbidden = "Obsahuje blok" in prompt
        self.calls.append(Call(claim_text, blocks, forbidden))
        for rule in self.rules:
            if rule.claim not in claim_text:
                continue
            if any(rule.block in block for block in blocks):
                return self._answer(rule.label, rule.block)
        return self._answer("NEOBSAHUJE" if forbidden else "NEVYJADRUJE", None)

    def _answer(self, label, quote):
        text = json.dumps({"stitek": label, "citace": quote, "castecne": False},
                          ensure_ascii=False)
        return CliReply(text=text, model=self.model, raw=text)

    def calls_for(self, claim_substring):
        return [c for c in self.calls if claim_substring in c.claim]


def selection(model="model-1"):
    spec = JudgeSpec(id="j1", family="openai", command=("cli", COMMAND_MARKER), model="alias",
                     output_shape="claude-json")
    return JudgeSelection(spec=spec, smoke_model=model, same_family=False,
                          agent_family="claude")


class CheckCase(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.tmp = Path(tmp.name)
        self.state = self.tmp / "state"
        self.cache = JudgeCache(self.state / "judge-cache")

    def corpus(self, types=None, name="corpus", **kwargs):
        return build_corpus(self.tmp / name, types if types is not None else a1_types(),
                            **kwargs)

    def check(self, catalog, rules=(), types=None, runner=None, **kwargs):
        root = self.corpus(types, **kwargs)
        index = open_corpus(root, self.state)
        runner = runner or ScriptedJudge(rules)
        measurement = run_card_check(catalog, index, selection(), runner, self.cache)
        return measurement, runner

    @staticmethod
    def fact_result(measurement, fid, role=None):
        return [r for r in measurement.results().facts
                if r.fact_id == fid and (role is None or r.role == role)]


def unit_of(measurement, claim_id, doc, role=None):
    found = [u for u in measurement.units if u.claim_id == claim_id and u.document == doc
             and (role is None or u.role == role)]
    assert len(found) >= 1, f"jednotka {claim_id} / {doc} není v měření"
    return found[0]


class AcceptanceShapeTest(CheckCase):
    """An item shaped like the acceptance case A1."""

    def a1(self):
        return make_catalog(question_json(
            [fact("f1", "Tichý návrat, když chybí dokument",
                  [place(EVMN, "EditorViewModelNew.RunNew()", radky=20)]),
             fact("f2", "Tichý návrat, když je kontrola vypnutá",
                  [place(EVMN, "EditorViewModelNew.RunNew()", radky=30)]),
             fact("f3", "Externí kontrola ověří dokument",
                  [place(EXT, "ExternalCheck.Run()", radky="30-35")])],
            [claim("z1", "Změna řádku gridu se ukládá okamžitě",
                   [place(EVN, "EditorViewNew.OnCellChanged()", radky="42-60")])]))

    def rules(self):
        return [Rule("Externí kontrola", "Zavolá externí registr", "VYJADRUJE"),
                Rule("řádku gridu", "Změna řádku gridu", "OBSAHUJE")]

    def test_silent_returns_missing_external_check_present_and_claim_found(self):
        measurement, _ = self.check(self.a1(), self.rules())
        self.assertEqual(measurement.status, STATUS_DONE)
        for fid in ("f1", "f2"):
            rows = self.fact_result(measurement, fid)
            self.assertEqual([r.status for r in rows], [FACT_MISSING, FACT_MISSING])
            self.assertEqual({r.role for r in rows}, {"podpora", "vyvojar"})
        [external] = self.fact_result(measurement, "f3", "podpora")
        self.assertEqual(external.status, FACT_PRESENT)
        self.assertEqual(external.present_in, (doc_id("ExternalCheck"),))
        unit = unit_of(measurement, "f3", doc_id("ExternalCheck"), "podpora")
        self.assertEqual(unit.final_label, "VYJADRUJE")
        self.assertIn("Zavolá externí registr", unit.quote)
        self.assertEqual(unit.binding["anchor"], f"{EXT}:30-35")
        self.assertEqual(unit.mode, MODE_WHOLE)
        [grid] = measurement.results().claims
        self.assertEqual(grid.status, card_check.CLAIM_FOUND)
        self.assertEqual([f["document"] for f in grid.found_in], [doc_id("EditorViewNew")])
        self.assertIn("řádku gridu", grid.found_in[0]["quote"])

    def test_the_old_editor_cards_are_not_judged_for_the_new_editor_places(self):
        _, runner = self.check(self.a1(), self.rules())
        for call in runner.calls:
            self.assertFalse(any("Uloží dokument bez kontroly" in b for b in call.blocks))

    def test_claim_bound_only_to_a_type_is_judged_on_the_type_card(self):
        catalog = make_catalog(question_json(
            [fact("f1", "Cokoli", [place(EVMN, "EditorViewModelNew.RunNew()", radky=20)])],
            [claim("z1", "Změna řádku gridu se ukládá okamžitě",
                   [place(EVN, typ="EditorViewNew", radky="3-90")])]))
        measurement, _ = self.check(catalog, self.rules())
        [grid] = measurement.results().claims
        self.assertEqual(grid.status, card_check.CLAIM_FOUND)
        self.assertEqual(unit_of(measurement, "z1", doc_id("EditorViewNew")).final_label,
                         "OBSAHUJE")


class FlowTest(CheckCase):
    def three_card_catalog(self):
        places = [place(EV, "EditorView.Save()", radky=35),
                  place(EVM, "EditorViewModel.Run()", radky=15),
                  place(EXT, "ExternalCheck.Run()", radky=30)]
        return make_catalog(question_json([
            fact("s1", "Krok 1 uloží dokument", [places[0]], tok={"krok": 1, "tok": "save"}),
            fact("s2", "Krok 2 spustí kontrolu", [places[1]], tok={"krok": 2, "tok": "save"}),
            fact("s3", "Krok 3 přečte verdikt", [places[2]], tok={"krok": 3, "tok": "save"}),
            fact("all", "Celý tok uložení", places, tok={"cely_tok": "save"})]))

    def test_whole_flow_over_three_cards_is_missing_while_every_step_is_present(self):
        types = a1_types()
        types[0].card = card("EditorView", EV, 3, 80, (
            "## Save\n\n" f"- Krok 1: uloží dokument — `{EV}:35`\n"))
        rules = [Rule("Krok 1", "Krok 1: uloží", "VYJADRUJE"),
                 Rule("Krok 2", "Krok 2: spustí", "VYJADRUJE"),
                 Rule("Krok 3", "Krok 3: přečte", "VYJADRUJE"),
                 Rule("Celý tok", "Krok", "VYJADRUJE")]
        measurement, runner = self.check(self.three_card_catalog(), rules, types=types)
        for fid in ("s1", "s2", "s3"):
            for row in self.fact_result(measurement, fid):
                self.assertEqual(row.status, FACT_PRESENT, fid)
        for row in self.fact_result(measurement, "all"):
            self.assertEqual(row.status, FACT_MISSING_UNBOUND)
        self.assertEqual(runner.calls_for("Celý tok"), [])  # no card had all three places
        notes = [u.note for u in measurement.units if u.claim_id == "all"]
        self.assertTrue(notes and all(card_check.FLOW_NOTE in n for n in notes))

    def test_whole_flow_chapter_context_of_a_layer_two_card_is_present(self):
        text = (
            "---\nschema: 7\ntool: reverse-architect\nkind: type\ndetail_level: 2\n"
            "layer2: present\n---\n\n# ExternalCheck\n\n"
            f"**public class** · 116 lines · `{EXT}:3-120`\n\n"
            "## Dynamic access\n\n### Context\n\n#### caller: EditorViewModel.Save\n\n"
            f"- enters the check — `{EXT}:20`\n"
            f"- calls the register — `{EXT}:30`\n"
            f"- stores the verdict — `{EXT}:44`\n")
        types = [TypeSpec("ExternalCheck", EXT, 3, 120, (("Run()", 20, 60),), card=text)]
        places = [place(EXT, "ExternalCheck.Run()", radky=n) for n in (20, 30, 44)]
        catalog = make_catalog(question_json([
            fact("s1", "Vstup 1", [places[0]], tok={"krok": 1, "tok": "ctx"}),
            fact("s2", "Vstup 2", [places[1]], tok={"krok": 2, "tok": "ctx"}),
            fact("s3", "Vstup 3", [places[2]], tok={"krok": 3, "tok": "ctx"}),
            fact("all", "Celý tok kontroly", places, tok={"cely_tok": "ctx"})]))
        rules = [Rule("Vstup 1", "enters the check", "VYJADRUJE"),
                 Rule("Vstup 2", "calls the register", "VYJADRUJE"),
                 Rule("Vstup 3", "stores the verdict", "VYJADRUJE"),
                 Rule("Celý tok", "calls the register", "VYJADRUJE")]
        measurement, runner = self.check(catalog, rules, types=types)
        for row in measurement.results().facts:
            self.assertEqual(row.status, FACT_PRESENT, row.fact_id)
        unit = unit_of(measurement, "all", doc_id("ExternalCheck"), "vyvojar")
        self.assertEqual((unit.mode, unit.blocks), (MODE_WHOLE, None))
        calls = runner.calls_for("Celý tok")  # three runs of one unit; the other role is cached
        self.assertEqual(len(calls), 3)
        call = calls[0]
        self.assertEqual(len(call.blocks), unit.block_count)
        self.assertGreater(unit.block_count, 3)  # the whole card, not three blocks

    def test_blocks_of_different_documents_are_not_joined_for_a_whole_flow(self):
        # The card binds one place of the flow, the detail the other: together they would
        # cover it, but a flow is judged on one document that binds every place.
        card_text = card("ExternalCheck", EXT, 3, 120, f"## Run\n\n- Vstup — `{EXT}:20`\n")
        detail = f"# Detail\n\n## API\n\n- Výstup — `{EXT}:44`\n"
        types = [TypeSpec("ExternalCheck", EXT, 3, 120, (("Run()", 20, 60),), card=card_text,
                          detail=detail)]
        places = [place(EXT, "ExternalCheck.Run()", radky=20),
                  place(EXT, "ExternalCheck.Run()", radky=44)]
        catalog = make_catalog(question_json([
            fact("a", "Vstup", [places[0]], role="misto", tok={"krok": 1, "tok": "t"}),
            fact("b", "Výstup", [places[1]], role="misto", tok={"krok": 2, "tok": "t"}),
            fact("all", "Celý tok", places, role="misto", tok={"cely_tok": "t"})]))
        measurement, runner = self.check(catalog, [Rule("Celý tok", "Vstup", "VYJADRUJE"),
                                                   Rule("Celý tok", "Výstup", "VYJADRUJE")],
                                         types=types)
        [row] = self.fact_result(measurement, "all")
        self.assertEqual(row.status, FACT_MISSING_UNBOUND)
        self.assertEqual(runner.calls_for("Celý tok"), [])
        for doc in (doc_id("ExternalCheck"), f"detail:{doc_id('ExternalCheck')}"):
            unit = unit_of(measurement, "all", doc)
            self.assertEqual(unit.state, STATE_NO_BOUND_BLOCK)
            self.assertIn(card_check.FLOW_NOTE, unit.note)


class RolesAndBlocksTest(CheckCase):
    def detail_types(self):
        card_text = card("ExternalCheck", EXT, 3, 120, f"## Run\n\n- Volá registr — `{EXT}:25`\n")
        detail = ("# Detail of ExternalCheck\n\n## API\n\n"
                  f"- `public void Run()` · `{EXT}:20-60`\n"
                  f"  - Odešle dokument; vrací se tiše — `{EXT}:44`\n"
                  f"- `public void Other()` · `{EXT}:100-110`\n"
                  "  - Nesouvisející metoda\n")
        return [TypeSpec("ExternalCheck", EXT, 3, 120, (("Run()", 20, 60),), card=card_text,
                         detail=detail)]

    def test_fact_of_role_misto_found_only_in_a_detail_is_for_the_developer_only(self):
        catalog = make_catalog(question_json([
            fact("fm", "Tichý návrat v kontrole", [place(EXT, "ExternalCheck.Run()", radky=44)],
                 role="misto")]))
        measurement, runner = self.check(catalog, [Rule("Tichý návrat", "vrací se tiše",
                                                        "VYJADRUJE")], types=self.detail_types())
        rows = measurement.results().facts
        self.assertEqual([(r.role, r.status) for r in rows], [("vyvojar", FACT_PRESENT)])
        self.assertEqual(rows[0].present_in, (f"detail:{doc_id('ExternalCheck')}",))
        unit = unit_of(measurement, "fm", f"detail:{doc_id('ExternalCheck')}")
        self.assertEqual(unit.mode, MODE_BOUND)
        texts = [b["text"] for b in unit.blocks]
        self.assertTrue(any("vrací se tiše" in t for t in texts))
        self.assertFalse(any("Nesouvisející" in t or "Other()" in t for t in texts))
        sent = [c for c in runner.calls if any("vrací se tiše" in b for b in c.blocks)]
        self.assertEqual(len(sent), 3)  # three runs of one unit (the card was not sent it)
        self.assertEqual(sent[0].blocks, texts)  # exactly the bound blocks went to the judge

    def test_fact_of_role_ramcova_is_not_checked_on_a_detail(self):
        catalog = make_catalog(question_json([
            fact("fr", "Tichý návrat v kontrole", [place(EXT, "ExternalCheck.Run()", radky=44)])]))
        measurement, _ = self.check(catalog, [Rule("Tichý návrat", "vrací se tiše",
                                                   "VYJADRUJE")], types=self.detail_types())
        self.assertEqual({u.slot for u in measurement.units}, {"karta"})
        self.assertEqual({r.status for r in measurement.results().facts}, {FACT_MISSING})

    def test_anchor_of_the_place_without_the_statement_is_missing_not_unbound(self):
        catalog = make_catalog(question_json([
            fact("f", "Odeslání dokumentu", [place(EXT, "ExternalCheck.Run()", radky="30-35")])]))
        measurement, runner = self.check(catalog)
        self.assertEqual({r.status for r in measurement.results().facts}, {FACT_MISSING})
        self.assertTrue(runner.calls)

    def test_card_over_the_threshold_is_preselected_and_without_a_bound_block_not_judged(self):
        filler = "".join(f"- Vedlejší věta číslo {n} o něčem jiném — `{EXT}:100-110`\n"
                         for n in range(500))
        body = ("## Run\n\n" f"- Zavolá externí registr a uloží verdikt — `{EXT}:30-35`\n"
                + filler)
        types = [TypeSpec("ExternalCheck", EXT, 3, 120, (("Run()", 20, 60),),
                          card=card("ExternalCheck", EXT, 3, 120, body))]
        self.assertGreater(len(body.encode("utf-8")), 16 * 1024)
        catalog = make_catalog(question_json([
            fact("f1", "Externí kontrola", [place(EXT, "ExternalCheck.Run()", radky="30-35")]),
            fact("f2", "Něco jiného", [place(EXT, "ExternalCheck.Run()", radky=50)])]))
        measurement, runner = self.check(catalog, [Rule("Externí kontrola", "Zavolá externí",
                                                        "VYJADRUJE")], types=types)
        first = unit_of(measurement, "f1", doc_id("ExternalCheck"), "vyvojar")
        self.assertEqual((first.mode, first.block_count), (MODE_BOUND, 1))
        self.assertEqual(first.final_label, "VYJADRUJE")
        self.assertEqual(first.state, card_check.STATE_ASSESSED)  # no "without binding" state here
        second = unit_of(measurement, "f2", doc_id("ExternalCheck"), "vyvojar")
        self.assertEqual(second.state, STATE_NO_BOUND_BLOCK)
        self.assertEqual(runner.calls_for("Něco jiného"), [])
        statuses = {r.fact_id: r.status for r in measurement.results().facts}
        self.assertEqual(statuses, {"f1": FACT_PRESENT, "f2": FACT_MISSING_UNBOUND})


class PlacesAndRevisionTest(CheckCase):
    def test_corpus_of_another_revision_is_refused_before_the_judge_is_called(self):
        catalog = make_catalog(question_json([
            fact("f", "Cokoli", [place(EXT, "ExternalCheck.Run()", radky=30)])]))
        root = self.corpus(revision=OTHER_REV)
        index = open_corpus(root, self.state)
        runner = ScriptedJudge()
        with self.assertRaisesRegex(CorpusError, "revize"):
            run_card_check(catalog, index, selection(), runner, self.cache)
        with self.assertRaises(CorpusError):
            build_plan(catalog, index)
        self.assertEqual((runner.calls, runner.smoke_calls), ([], 0))

    def test_a_place_not_in_the_corpus_is_reported_and_not_judged(self):
        catalog = make_catalog(question_json([
            fact("fx", "Chybějící místo", [place("src/Acme/Missing.cs", "Missing.Run()", radky=5)]),
            fact("fy", "Existující místo", [place(EXT, "ExternalCheck.Run()", radky=30)])],
            [claim("zx", "Tvrzení bez místa", [place("src/Acme/Missing.cs", typ="Missing",
                                                     radky=5)])]))
        measurement, runner = self.check(catalog)
        for row in self.fact_result(measurement, "fx"):
            self.assertEqual(row.status, FACT_PLACE_UNKNOWN)
            self.assertIn("místo nenalezeno v korpusu", row.reasons[0])
            self.assertEqual(row.unit_ids, ())
        self.assertEqual(runner.calls_for("Chybějící místo"), [])
        self.assertEqual(runner.calls_for("Tvrzení bez místa"), [])
        self.assertTrue(runner.calls_for("Existující místo"))
        [missing] = measurement.results().claims
        self.assertEqual(missing.status, card_check.CLAIM_PLACE_UNKNOWN)

    def test_load_corpus_is_never_called_and_no_receipt_is_needed(self):
        import prepare
        catalog = make_catalog(question_json([
            fact("f", "Cokoli", [place(EXT, "ExternalCheck.Run()", radky=30)])]))
        self.assertFalse(hasattr(card_check, "load_corpus"))
        with mock.patch.object(prepare, "load_corpus",
                               side_effect=AssertionError("load_corpus se volat nesmí")):
            measurement, _ = self.check(catalog)
        self.assertEqual(measurement.status, STATUS_DONE)
        self.assertEqual(measurement.corpus["label"], "identita z coverage.json")
        self.assertFalse((self.tmp / "corpus.receipt.json").exists())

    def test_a_manifest_by_lines_gives_the_same_result_as_one_file(self):
        catalog = AcceptanceShapeTest.a1(self)
        rules = AcceptanceShapeTest.rules(self)
        results = []
        for name, lines in (("one", False), ("lines", True)):
            index = open_corpus(build_corpus(self.tmp / name, a1_types(), lines=lines),
                                self.state / name)
            measurement = run_card_check(catalog, index, selection(), ScriptedJudge(rules),
                                         JudgeCache(self.state / name / "cache"))
            results.append(measurement.to_json())
        self.assertEqual(results[0], results[1])


class ConflictTest(CheckCase):
    def test_a_claim_found_on_another_card_selected_for_the_fact_makes_it_conflicting(self):
        catalog = make_catalog(question_json(
            [fact("fx", "Uložení proběhne bez kontroly",
                  [place(EV, "EditorView.Save()", radky="32-40"),
                   place(EVN, "EditorViewNew.Load()", radky="10-40")])],
            [claim("zx", "Nový editor uloží dokument vždy po kontrole",
                   [place(EVN, "EditorViewNew.Load()", radky="10-40")], fakty=["fx"])]))
        rules = [Rule("bez kontroly", "Uloží dokument bez kontroly", "VYJADRUJE"),
                 Rule("vždy po kontrole", "uloží dokument vždy po kontrole", "OBSAHUJE")]
        measurement, _ = self.check(catalog, rules)
        for row in self.fact_result(measurement, "fx"):
            self.assertEqual(row.status, FACT_CONFLICT)
            self.assertTrue(row.other_present)
            self.assertEqual(row.present_in, (doc_id("EditorView"),))
            self.assertEqual([c["document"] for c in row.conflicts], [doc_id("EditorViewNew")])
            self.assertEqual(row.conflicts[0]["claim_id"], "zx")

    def test_a_claim_on_a_card_not_selected_for_the_fact_does_not_make_it_conflicting(self):
        catalog = make_catalog(question_json(
            [fact("fx", "Uložení proběhne bez kontroly",
                  [place(EV, "EditorView.Save()", radky="32-40")])],
            [claim("zx", "Nový editor uloží dokument vždy po kontrole",
                   [place(EVN, "EditorViewNew.Load()", radky="10-40")], fakty=["fx"])]))
        rules = [Rule("bez kontroly", "Uloží dokument bez kontroly", "VYJADRUJE"),
                 Rule("vždy po kontrole", "uloží dokument vždy po kontrole", "OBSAHUJE")]
        measurement, _ = self.check(catalog, rules)
        self.assertEqual({r.status for r in measurement.results().facts}, {FACT_PRESENT})
        self.assertEqual(measurement.results().claims[0].status, card_check.CLAIM_FOUND)

    def test_a_conflict_beats_a_missing_fact_and_reports_it_is_not_present_elsewhere(self):
        catalog = make_catalog(question_json(
            [fact("fx", "Uložení proběhne bez kontroly",
                  [place(EVN, "EditorViewNew.Load()", radky="10-40")])],
            [claim("zx", "Nový editor uloží dokument vždy po kontrole",
                   [place(EVN, "EditorViewNew.Load()", radky="10-40")], fakty=["fx"])]))
        measurement, _ = self.check(catalog, [Rule("vždy po kontrole", "uloží dokument vždy",
                                                   "OBSAHUJE")])
        for row in self.fact_result(measurement, "fx"):
            self.assertEqual((row.status, row.other_present), (FACT_CONFLICT, False))


class BindingTest(CheckCase):
    def unbound(self):
        catalog = make_catalog(question_json([
            fact("f", "Odeslání dokumentu", [place(EXT, "ExternalCheck.Run()", radky="30-35")])]))
        return self.check(catalog, [Rule("Odeslání", "Odešle dokument", "VYJADRUJE")])

    def test_a_sentence_without_binding_to_the_place_is_a_disputed_unit(self):
        measurement, _ = self.unbound()
        unit = unit_of(measurement, "f", doc_id("ExternalCheck"), "vyvojar")
        self.assertEqual(unit.state, STATE_UNBOUND_FACT)
        self.assertEqual(STATE_UNBOUND_FACT, "vyjadřuje, ale bez vazby")
        self.assertTrue(unit.disputed)
        self.assertEqual((unit.model_label, unit.final_label), ("VYJADRUJE", "NELZE"))
        self.assertIn("neváže", unit.dispute_reason)
        self.assertEqual({r.status for r in measurement.results().facts}, {FACT_DISPUTED})

    def test_an_owner_decision_overwrites_the_final_label_and_the_results_follow(self):
        measurement, _ = self.unbound()
        decided = tuple(u.with_decision("VYJADRUJE", "vlastník") if u.disputed else u
                        for u in measurement.units)
        results = derive_results(measurement.plan, decided)
        self.assertEqual({r.status for r in results.facts}, {FACT_PRESENT})
        self.assertEqual(decided[0].model_label, "VYJADRUJE")
        self.assertEqual(decided[0].decided_by, "vlastník")
        with self.assertRaises(ValueError):
            decided[0].with_decision("VYJADRUJE", "")

    def test_a_dispute_next_to_a_certain_presence_does_not_change_the_result(self):
        catalog = make_catalog(question_json([
            fact("f", "Odeslání dokumentu", [place(EXT, "ExternalCheck.Run()", radky="30-35"),
                                             place(EV, "EditorView.Save()", radky="32-40")])]))
        rules = [Rule("Odeslání", "Odešle dokument", "VYJADRUJE"),
                 Rule("Odeslání", "Uloží dokument bez kontroly", "VYJADRUJE")]
        measurement, _ = self.check(catalog, rules)
        for row in measurement.results().facts:
            self.assertEqual(row.status, FACT_PRESENT)
            self.assertEqual(row.present_in, (doc_id("EditorView"),))
            self.assertEqual(len(row.disputed_units), 1)


class MeasurementFileTest(CheckCase):
    def test_a_completed_measurement_is_written_atomically_with_format_version_first(self):
        catalog = AcceptanceShapeTest.a1(self)
        measurement, _ = self.check(catalog, AcceptanceShapeTest.rules(self))
        path = card_check.measurement_path(self.state)
        write_measurement(path, measurement)
        raw = path.read_text(encoding="utf-8")
        data = json.loads(raw)
        self.assertEqual(next(iter(data)), "format_version")
        self.assertEqual(data["format_version"], card_check.MEASUREMENT_FORMAT_VERSION)
        self.assertEqual(data["status"], "hotovo")
        self.assertEqual(data["judge"], {"id": "j1", "rodina": "openai", "model": "model-1",
                                         "stejna_rodina": False})
        self.assertEqual(data["reported_model"], "model-1")
        self.assertEqual(data["prompt_version"], card_check.PROMPT_VERSION)
        self.assertEqual(data["agent_family"], "claude")
        self.assertEqual(data["corpus"]["revision"], REV)
        self.assertEqual(data["corpus"]["label"], "identita z coverage.json")
        self.assertEqual(data["items"][0]["id"], "Q1")
        self.assertEqual(len(data["items"][0]["otisk_potvrzeni"]), 64)
        self.assertNotIn(COMMAND_MARKER, raw)
        self.assertIn("řádku gridu", raw)  # ensure_ascii=False
        self.assertIn('\n  "id"', raw)  # indent=2
        self.assertEqual([p.name for p in path.parent.glob(".*.tmp")], [])
        unit = data["units"][0]
        for key in ("id", "question_id", "claim_id", "role", "document", "mode", "state",
                    "run_labels", "quote", "disputed", "dispute_reason", "final_label"):
            self.assertIn(key, unit)
        self.assertEqual(data["results"]["facts"][0]["status"], FACT_MISSING)

    def test_a_failure_in_the_middle_saves_an_incomplete_measurement(self):
        catalog = AcceptanceShapeTest.a1(self)
        saved = []
        root = self.corpus()
        index = open_corpus(root, self.state)
        runner = ScriptedJudge(AcceptanceShapeTest.rules(self), fail_after=4)
        with self.assertRaises(JudgeInfrastructureError):
            run_card_check(catalog, index, selection(), runner, self.cache,
                           save=lambda m: saved.append(m))
        self.assertEqual([m.status for m in saved], [STATUS_INCOMPLETE, STATUS_INCOMPLETE])
        last = saved[-1].to_json()
        self.assertIsNone(last["results"])
        states = [u["state"] for u in last["units"]]
        self.assertIn(card_check.STATE_PENDING, states)
        self.assertIn(card_check.STATE_ASSESSED, states)

    def test_the_second_run_does_not_repeat_the_calls_that_were_stored(self):
        catalog = AcceptanceShapeTest.a1(self)
        rules = AcceptanceShapeTest.rules(self)
        root = self.corpus()
        index = open_corpus(root, self.state)
        clean = ScriptedJudge(rules)
        run_card_check(catalog, index, selection(), clean,
                       JudgeCache(self.tmp / "other-cache"))
        total = len(clean.calls)
        first = ScriptedJudge(rules, fail_after=4)
        with self.assertRaises(JudgeInfrastructureError):
            run_card_check(catalog, index, selection(), first, self.cache)
        second = ScriptedJudge(rules)
        measurement = run_card_check(catalog, index, selection(), second, self.cache)
        self.assertEqual(len(first.calls), 4)
        self.assertEqual(len(second.calls), total - 4)
        self.assertEqual(measurement.status, STATUS_DONE)

    def test_the_measurement_id_follows_the_inputs(self):
        catalog = AcceptanceShapeTest.a1(self)
        one, _ = self.check(catalog, AcceptanceShapeTest.rules(self))
        again, _ = self.check(catalog, AcceptanceShapeTest.rules(self))
        self.assertEqual(one.id, again.id)
        other = card_check.measurement_id(one.corpus, one.items, {"id": "j2"}, "claude")
        self.assertNotEqual(other, one.id)

    def test_a_measurement_of_other_inputs_is_moved_aside(self):
        path = card_check.measurement_path(self.state)
        path.parent.mkdir(parents=True)
        path.write_text(json.dumps({"format_version": 1, "id": "old1"}), encoding="utf-8")
        self.assertIsNone(card_check.archive_foreign_measurement(path, "old1"))
        moved = card_check.archive_foreign_measurement(path, "new2")
        self.assertEqual(moved.name, "measurement.old1.json")
        self.assertFalse(path.exists())
        self.assertTrue(moved.is_file())


if __name__ == "__main__":
    unittest.main()
