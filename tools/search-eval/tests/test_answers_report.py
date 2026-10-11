"""The report of the answers (U8): `answers.py zprava` and `answers_report.py`.

The measurements are real: corpora are synthetic (`tests/test_card_check.py`), the judge is a
scripted runner, the agent's answers are written here. Every expected number is counted by
hand in the test. All names are generic.
"""
import contextlib
import io
import json
import tempfile
import unittest
from dataclasses import replace
from pathlib import Path

import answers
import answers_report
import card_check
import review
from card_check import AnswerRun, run_answer_check, write_measurement
from corpus import open_corpus
from judge import PROMPT_VERSION, JudgeSelection, JudgeSpec
from prepare import receipt_path
from tests.test_card_check import (EVM, EVMN, EVN, EXT, OTHER_REV, REV, CheckCase, Rule,
                                   ScriptedJudge, a1_types, card, claim, fact, make_catalog,
                                   place, question_json, selection)

REPO = Path(__file__).resolve().parents[3]

F1 = "Tichý návrat, když chybí dokument"
F2 = "Externí kontrola ověří dokument"
F3 = "Nový editor uloží dokument vždy po kontrole"
Z1 = "Změna řádku gridu se ukládá okamžitě"

RULES = [Rule("Externí kontrola", "Zavolá externí registr", "VYJADRUJE"),
         Rule("Nový editor", "uloží dokument vždy", "VYJADRUJE"),
         Rule("Krok 2", "spustí kontrolu", "VYJADRUJE"),
         Rule("řádku gridu", "Změna řádku gridu", "OBSAHUJE"),
         Rule("Tichý návrat", "tiše vrátí", "VYJADRUJE")]

PRESENT = "Při chybějícím dokumentu se `EditorViewModelNew.RunNew()` tiše vrátí."
ABSENT = "Dokument se zpracuje normálně v `EditorViewModelNew.RunNew()`."

RECEIPT = {"format_version": 1, "fingerprint_algorithm": "sha256-files-v1",
           "base_revision": REV, "tree_fingerprint": "t" * 8, "ra_version": "9.9.9",
           "options": ["--cards"], "corpus_fingerprint": "f" * 8, "generation_seconds": 1.0}


def q3(qid="Q1", kontrolni=False):
    """One question: two framework facts, a place fact and a forbidden claim."""
    return question_json(
        [fact("f1", F1, [place(EVMN, "EditorViewModelNew.RunNew()", radky=20)]),
         fact("f2", F2, [place(EXT, "ExternalCheck.Run()", radky="30-35")]),
         fact("f3", F3, [place(EVN, "EditorViewNew.Load()", radky="10-40")], role="misto")],
        [claim("z1", Z1, [place(EVN, "EditorViewNew.OnCellChanged()", radky="42-60")],
               fakty=("f1",))], qid=qid, kontrolni=kontrolni)


def control_question():
    return question_json(
        [fact("g1", "Krok 2: spustí kontrolu", [place(EVM, "EditorViewModel.Run()", radky=15)])],
        [], qid="Q2", kontrolni=True)


def corpus_one():
    """The A1-shaped corpus plus a detail of the new editor (it carries f3 beside its card)."""
    types = a1_types()
    for spec in types:
        if spec.name == "EditorViewNew":
            spec.detail = f"# Detail\n\n- Načtení dokumentu — `{EVN}:10-40`\n"
    return types


def corpus_two():
    """The same types, but the cards no longer state f2, f3 and the claim."""
    types = corpus_one()
    out = []
    for spec in types:
        if spec.name == "ExternalCheck":
            spec = replace(spec, card=card("ExternalCheck", EXT, 3, 120,
                                           f"## Run\n\n- Odešle dokument — `{EXT}:90-95`\n"))
        if spec.name == "EditorViewNew":
            spec = replace(spec, card=card("EditorViewNew", EVN, 3, 90,
                                           f"## Events\n\n- Načte dokument — `{EVN}:10-40`\n"))
        out.append(spec)
    return out


def family_selection(model="model-1"):
    spec = JudgeSpec(id="j1", family="claude", command=("cli",), model="alias",
                     output_shape="claude-json")
    return JudgeSelection(spec=spec, smoke_model=model, same_family=True, agent_family="claude")


def disputed_copy(measured):
    units = tuple(replace(u, disputed=True, final_label=card_check.LABEL_UNKNOWN)
                  if u.state == card_check.STATE_ASSESSED else u for u in measured.units)
    return replace(measured, units=units)


class ReportCase(CheckCase):
    def setUp(self):
        super().setUp()
        self.out = self.tmp / "out"
        self.cal = self.tmp / "kalibrace"

    # -- inputs --

    def card_measurement(self, name, types, catalog, *, receipt=None, judge=None,
                         selection_=None, transform=None):
        root = self.corpus(types, name=name)
        if receipt is not None:
            receipt_path(root).write_text(json.dumps(receipt), encoding="utf-8")
        index = open_corpus(root, self.state)
        measured = card_check.run_card_check(
            catalog, index, selection_ or selection(), judge or ScriptedJudge(RULES), self.cache)
        if transform is not None:
            measured = transform(measured)
        path = self.tmp / name / "measurement.json"
        write_measurement(path, measured)
        return path

    def answer_corpus(self):
        receipt = {"base_revision": REV, "tree_fingerprint": "t" * 8, "ra_version": "1.2.3",
                   "options": ["--cards"], "corpus_fingerprint": "c" * 8}
        return {"revision": REV, "source": card_check.ANSWER_PATH, "label": "odpovědi pokusu p1",
                "tree_fingerprint": "t" * 8, "pokus": "p1", "role": "podpora", "odpovedi": "x",
                "varianty": [{"nazev": "A", "korpus": receipt},
                             {"nazev": "B", "korpus": None}]}

    def answers_measurement(self, name, catalog, runs, *, selection_=None):
        no_answer = [{"run": r.run, "otazka": r.question_id, "varianta": r.variant,
                      "duvod": r.reason} for r in runs if r.text is None]
        extra = {"pokus": {"id": "p1", "stav": "completed", "nedokonceny": False,
                           "role": "podpora", "adapter": "claude-code", "rodina_agenta": "claude",
                           "behy": [], "bez_odpovedi": no_answer}}
        measured = run_answer_check(catalog, runs, "podpora", self.answer_corpus(), extra,
                                    selection_ or selection(), ScriptedJudge(RULES), self.cache)
        path = self.tmp / name / "measurement.json"
        write_measurement(path, measured)
        return path

    def three_runs(self, third_text=ABSENT, third_reason=None):
        return [AnswerRun("Q1", "A", 1, PRESENT), AnswerRun("Q1", "A", 2, PRESENT),
                AnswerRun("Q1", "A", 3, third_text, third_reason),
                AnswerRun("Q1", "B", 4, ABSENT), AnswerRun("Q1", "B", 5, ABSENT),
                AnswerRun("Q1", "B", 6, ABSENT)]

    # -- the report --

    def report(self, paths, **kwargs):
        return answers_report.build_report(paths, self.state, kwargs.pop("cal", self.cal),
                                           **kwargs)

    @staticmethod
    def facts_of(data, section=answers_report.SECTION_ACCEPTANCE):
        return data[section]["fakta"]

    @staticmethod
    def cells(rows):
        return [(r["fakt"], r["role"], r["korpus"], r["stav"]) for r in rows]

    # -- the command --

    def zprava(self, paths, *extra, out=None, **replace_args):
        values = {"--source-repo": str(REPO), "--vystup": str(out or self.out),
                  "--stav": str(self.state), "--kalibrace": str(self.cal)}
        values.update(replace_args)
        argv = ["zprava"]
        for key, value in values.items():
            argv += [key, value]
        for path in paths:
            argv += ["--mereni", str(path)]
        return self.main(argv + list(extra))

    @staticmethod
    def main(argv):
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = answers.main(argv)
        return code, out.getvalue(), err.getvalue()

    def review_args(self, *extra):
        return ["kontrola", "--source-repo", str(REPO), "--stav", str(self.state), *extra]


class TableTest(ReportCase):
    """Two corpora, one question, three facts, two roles: the table is the hand-counted one."""

    def setUp(self):
        super().setUp()
        catalog = make_catalog(q3())
        self.one = self.card_measurement("c1", corpus_one(), catalog, receipt=RECEIPT)
        self.two = self.card_measurement("c2", corpus_two(), catalog)
        self.data = self.report([self.one, self.two])

    def test_every_fact_role_path_and_corpus_has_its_row_with_the_hand_counted_result(self):
        missing, present = card_check.FACT_MISSING, card_check.FACT_PRESENT
        expected = [
            ("f1", "podpora", "korpus-1", missing), ("f1", "podpora", "korpus-2", missing),
            ("f1", "vyvojar", "korpus-1", missing), ("f1", "vyvojar", "korpus-2", missing),
            ("f2", "podpora", "korpus-1", present), ("f2", "podpora", "korpus-2", missing),
            ("f2", "vyvojar", "korpus-1", present), ("f2", "vyvojar", "korpus-2", missing),
            ("f3", "vyvojar", "korpus-1", present), ("f3", "vyvojar", "korpus-2", missing)]
        rows = self.facts_of(self.data)
        self.assertEqual(self.cells(rows), expected)
        self.assertEqual({r["cesta"] for r in rows}, {"kontrola karet"})
        self.assertEqual({r["otazka"] for r in rows}, {"Q1"})
        self.assertEqual(rows[0]["text"], F1)

    def test_the_forbidden_claim_has_its_own_rows_found_and_not_found(self):
        rows = self.data[answers_report.SECTION_ACCEPTANCE]["tvrzeni"]
        self.assertEqual([(r["tvrzeni"], r["korpus"], r["stav"]) for r in rows],
                         [("z1", "korpus-1", card_check.CLAIM_FOUND),
                          ("z1", "korpus-2", card_check.CLAIM_ABSENT)])
        text = answers_report.render_markdown(self.data)
        self.assertIn("### Zakázaná tvrzení (nalezeno / nenalezeno)", text)
        self.assertIn("| nenalezeno |", text)

    def test_the_summary_counts_every_status_apart(self):
        by_corpus = {r["korpus"]: r["stavy"] for r in self.data["souhrn"]}
        self.assertEqual(by_corpus["korpus-1"][card_check.FACT_PRESENT], 3)
        self.assertEqual(by_corpus["korpus-1"][card_check.FACT_MISSING], 2)
        self.assertEqual(by_corpus["korpus-2"][card_check.FACT_MISSING], 5)
        for counts in by_corpus.values():
            self.assertEqual(set(counts), set(answers_report.FACT_STATUSES))
            self.assertEqual(counts[card_check.FACT_MISSING_UNBOUND], 0)
            self.assertEqual(counts[card_check.FACT_PLACE_UNKNOWN], 0)

    def test_a_place_the_corpus_does_not_know_is_its_own_status(self):
        catalog = make_catalog(question_json(
            [fact("f1", F1, [place(EVMN, "EditorViewModelNew.RunNew()", radky=20)]),
             fact("f9", "Fakt o místě, které v korpusu není",
                  [place(EVMN, "EditorViewModelNew.Gone()", radky=20)])]))
        path = self.card_measurement("c3", corpus_one(), catalog)
        data = self.report([path])
        stavy = data["souhrn"][0]["stavy"]
        self.assertEqual((stavy[card_check.FACT_PLACE_UNKNOWN], stavy[card_check.FACT_MISSING],
                          stavy[card_check.FACT_MISSING_UNBOUND]), (2, 2, 0))

    def test_the_corpus_identity_names_the_receipt_or_says_the_build_is_unknown(self):
        first, second = self.data["korpusy"]
        self.assertEqual(first["sestaveni_ra"], {"verze": "9.9.9", "volby": ["--cards"]})
        self.assertEqual((first["otisk_korpusu"], first["otisk_stromu"], first["repo_revision"]),
                         ("f" * 8, "t" * 8, REV))
        self.assertIsNone(second["sestaveni_ra"])
        self.assertIsNone(second["otisk_korpusu"])
        text = answers_report.render_markdown(self.data)
        self.assertIn("RA 9.9.9; --cards", text)
        self.assertIn(answers_report.RECEIPT_UNKNOWN, text)

    def test_the_assessment_is_the_models_unless_the_owner_decided(self):
        for row in self.facts_of(self.data):
            self.assertEqual(row["posouzeni"]["druh"], answers_report.ASSESSED_MODEL)
            self.assertFalse(row["posouzeni"]["zalozni_hodnotitel"])
        text = answers_report.render_markdown(self.data)
        self.assertIn("posouzení modelu", text)
        self.assertNotIn("záložní hodnotitel", text)

    def test_the_markdown_renders_the_numbers_it_is_given_and_counts_nothing(self):
        data = json.loads(json.dumps(self.data))
        data["velikosti"][0]["karty_vlastniku_mist"]["bajtu"] = 12345
        data["shoda_vlastnika"][0].update(vzorek=7, shodne=5)
        text = answers_report.render_markdown(data)
        self.assertIn("dohromady 12345 bajtů", text)
        self.assertIn("5/7", text)


class SectionsTest(ReportCase):
    def test_the_control_question_is_in_another_section_than_the_acceptance_item(self):
        catalog = make_catalog(q3(), control_question())
        path = self.card_measurement("c1", corpus_one(), catalog)
        data = self.report([path])
        acceptance = data[answers_report.SECTION_ACCEPTANCE]
        control = data[answers_report.SECTION_CONTROL]
        self.assertEqual([q["id"] for q in acceptance["otazky"]], ["Q1"])
        self.assertEqual([q["id"] for q in control["otazky"]], ["Q2"])
        self.assertEqual({r["otazka"] for r in acceptance["fakta"]}, {"Q1"})
        self.assertEqual([(r["otazka"], r["fakt"], r["stav"]) for r in control["fakta"]],
                         [("Q2", "g1", card_check.FACT_PRESENT)] * 2)
        self.assertTrue(all(r["kontrolni"] for r in control["fakta"]))
        text = answers_report.render_markdown(data)
        self.assertLess(text.index("## Akceptační položka"), text.index("## Kontrolní otázky"))
        control_part = text[text.index("## Kontrolní otázky"):]
        self.assertIn("| Q2 |", control_part)
        self.assertNotIn("| Q1 |", control_part.split("## Souhrn")[0])
        self.assertNotIn("| Q2 |", text[:text.index("## Kontrolní otázky")])


class AgentTest(ReportCase):
    def setUp(self):
        super().setUp()
        self.catalog = make_catalog(q3())

    def test_a_fact_present_in_two_of_three_runs_is_two_of_three(self):
        path = self.answers_measurement("a1", self.catalog, self.three_runs())
        data = self.report([path])
        rows = {(r["fakt"], r["korpus"]): r for r in self.facts_of(data)}
        a1 = rows[("f1", "p1:A")]
        self.assertEqual((a1["pritomen"], a1["behu"], a1["cesta"]), (2, 3, "běh s agentem"))
        self.assertEqual([b["run"] for b in a1["behy"]], [1, 2, 3])
        self.assertEqual(rows[("f1", "p1:B")]["pritomen"], 0)
        self.assertEqual(rows[("f2", "p1:A")]["pritomen"], 0)
        self.assertEqual(len(rows), 4)  # the role podpora: f1 and f2, two variants
        self.assertIn("2/3 přítomen", answers_report.render_markdown(data))

    def test_runs_without_an_answer_are_counted_apart(self):
        path = self.answers_measurement(
            "a1", self.catalog, self.three_runs(None, "běh skončil bez závěrečné zprávy"))
        data = self.report([path])
        row = next(r for r in self.facts_of(data) if (r["fakt"], r["korpus"]) == ("f1", "p1:A"))
        self.assertEqual((row["pritomen"], row["behu"], row["bez_odpovedi"]), (2, 3, 1))
        self.assertEqual(row["stavy"], {card_check.FACT_NO_ANSWER: 1, card_check.FACT_PRESENT: 2})
        self.assertEqual(data["behy_bez_odpovedi"], [{
            "mereni": data["vstupy"][0]["mereni"], "pokus": "p1", "otazka": "Q1", "run": 3,
            "varianta": "A", "duvod": "běh skončil bez závěrečné zprávy"}])
        claim_row = next(r for r in data[answers_report.SECTION_ACCEPTANCE]["tvrzeni"]
                         if r["korpus"] == "p1:A")
        self.assertEqual((claim_row["bez_odpovedi"], claim_row["behu"]), (1, 3))
        text = answers_report.render_markdown(data)
        self.assertIn("2/3 přítomen; chybí, bez odpovědi 1", text)
        self.assertIn("## Běhy agenta bez odpovědi", text)
        self.assertEqual(data["souhrn"][0]["stavy"][card_check.FACT_NO_ANSWER], 2)

    def test_the_agent_path_and_the_card_path_sit_side_by_side(self):
        cards = self.card_measurement("c1", corpus_one(), self.catalog, receipt=RECEIPT)
        agent = self.answers_measurement("a1", self.catalog, self.three_runs())
        data = self.report([cards, agent])
        f1 = [r for r in self.facts_of(data) if r["fakt"] == "f1" and r["role"] == "podpora"]
        self.assertEqual([(r["cesta"], r["korpus"]) for r in f1],
                         [("kontrola karet", "korpus-1"), ("běh s agentem", "p1:A"),
                          ("běh s agentem", "p1:B")])
        corpora = {c["id"]: c for c in data["korpusy"]}
        self.assertEqual(corpora["p1:A"]["sestaveni_ra"], {"verze": "1.2.3", "volby": ["--cards"]})
        self.assertTrue(corpora["p1:B"]["bez_korpusu"])
        self.assertIn(answers_report.NO_CORPUS, answers_report.render_markdown(data))
        self.assertEqual(len(data["velikosti"]), 1)  # sizes come from the card check only

    def test_an_unfinished_experiment_is_marked(self):
        path = self.answers_measurement("a1", self.catalog, self.three_runs())
        raw = json.loads(path.read_text("utf-8"))
        raw["pokus"].update(stav="interrupted", nedokonceny=True)
        path.write_text(json.dumps(raw, ensure_ascii=False), encoding="utf-8")
        self.assertIn("je nedokončený", answers_report.render_markdown(self.report([path])))


class SizesTest(ReportCase):
    def test_sizes_match_the_files_of_the_corpus(self):
        catalog = make_catalog(q3())
        root = self.corpus(corpus_one(), name="c1")
        index = open_corpus(root, self.state)
        measured = card_check.run_card_check(catalog, index, selection(), ScriptedJudge(RULES),
                                             self.cache)
        path = self.tmp / "m1" / "measurement.json"
        write_measurement(path, measured)
        data = self.report([path])

        def size(relative):
            return len((root / relative).read_bytes())

        evmn, ext = "cards/type/EditorViewModelNew.md", "cards/type/ExternalCheck.md"
        evn, evn_detail = "cards/type/EditorViewNew.md", "cards/detail/EditorViewNew.md"
        [corpus] = data["velikosti"]
        facts = {f["fakt"]: f for f in corpus["fakta"]}
        self.assertEqual([(d["cesta"], d["bajtu"], d["karta_vlastnika"]) for d in
                          facts["f1"]["dokumenty"]], [(evmn, size(evmn), True)])
        self.assertEqual(facts["f1"]["bajtu_celkem"], size(evmn))
        self.assertEqual(facts["f2"]["bajtu_celkem"], size(ext))
        self.assertEqual([(d["cesta"], d["bajtu"], d["karta_vlastnika"]) for d in
                          facts["f3"]["dokumenty"]],
                         [(evn, size(evn), True), (evn_detail, size(evn_detail), False)])
        self.assertEqual(facts["f3"]["bajtu_celkem"], size(evn) + size(evn_detail))
        owners = corpus["karty_vlastniku_mist"]
        self.assertEqual(owners["pocet"], 3)
        self.assertEqual(sorted(c["cesta"] for c in owners["karty"]), sorted([evmn, ext, evn]))
        self.assertEqual(owners["bajtu"], size(evmn) + size(ext) + size(evn))
        text = answers_report.render_markdown(data)
        self.assertIn(f"dohromady {size(evmn) + size(ext) + size(evn)} bajtů", text)
        self.assertIn(f"{size(evn) + size(evn_detail)} |", text)

    def test_a_measurement_without_sizes_says_so_instead_of_inventing_them(self):
        path = self.card_measurement("c1", corpus_one(), make_catalog(q3()))
        raw = json.loads(path.read_text("utf-8"))
        for unit in raw["units"]:
            unit.pop("document_bytes", None)
        path.write_text(json.dumps(raw, ensure_ascii=False), encoding="utf-8")
        data = self.report([path])
        self.assertTrue(all(f["bajtu_celkem"] is None for f in data["velikosti"][0]["fakta"]))
        self.assertIsNone(data["velikosti"][0]["karty_vlastniku_mist"]["bajtu"])
        self.assertIn("neuchováno", answers_report.render_markdown(data))


class RefusalTest(ReportCase):
    def setUp(self):
        super().setUp()
        self.catalog = make_catalog(q3())
        self.one = self.card_measurement("c1", corpus_one(), self.catalog)

    def edit(self, path, mutate, name):
        raw = json.loads(path.read_text("utf-8"))
        mutate(raw)
        target = self.tmp / name / "measurement.json"
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(json.dumps(raw, ensure_ascii=False), encoding="utf-8")
        return target

    def test_a_disputed_unit_without_a_decision_stops_the_report_and_is_named(self):
        path = self.card_measurement("c2", corpus_one(), self.catalog,
                                     transform=disputed_copy)
        with self.assertRaises(answers_report.ReportError) as caught:
            self.report([path])
        text = str(caught.exception)
        self.assertIn("jednotka Q1::f2::podpora::", text)
        self.assertIn("čeká na rozhodnutí vlastníka", text)
        code, _, err = self.zprava([path])
        self.assertEqual(code, 2)
        self.assertIn("Q1::f1::podpora::", err)
        self.assertFalse(self.out.exists())

    def test_a_judge_other_than_the_first_is_refused_with_the_advice_to_judge_again(self):
        other = self.card_measurement("c2", corpus_two(), self.catalog,
                                      selection_=selection(model="model-2"),
                                      judge=ScriptedJudge(RULES, model="model-2"))
        with self.assertRaisesRegex(answers_report.ReportError,
                                    "jiný hodnotitel, model.*Přehodnoť starší korpus"):
            self.report([self.one, other])
        code, _, err = self.zprava([self.one, other])
        self.assertEqual((code, self.out.exists()), (2, False))
        self.assertIn("Přehodnoť", err)

    def test_another_prompt_version_is_refused(self):
        other = self.edit(self.one, lambda raw: raw.update(prompt_version="99"), "c2")
        with self.assertRaisesRegex(answers_report.ReportError,
                                    "verze promptu 99.*Přehodnoť"):
            self.report([self.one, other])

    def test_another_judge_id_is_refused(self):
        other = self.edit(self.one, lambda raw: raw["judge"].update(id="j2"), "c2")
        with self.assertRaisesRegex(answers_report.ReportError, "hodnotitel j2"):
            self.report([self.one, other])

    def test_another_judge_family_is_refused_even_when_id_model_and_prompt_agree(self):
        other = self.edit(self.one, lambda raw: raw["judge"].update(rodina="jina"), "c2")
        with self.assertRaisesRegex(answers_report.ReportError,
                                    "rodina openai.*rodina jina.*Přehodnoť"):
            self.report([self.one, other])

    def test_items_with_a_different_question_text_are_refused(self):
        def change(raw):
            raw["items"][0]["dotaz"] = "Jak jinak editor ukládá dokument?"

        other = self.edit(self.one, change, "c2")
        fingerprints = [json.loads(p.read_text("utf-8"))["items"][0]["otisk_potvrzeni"]
                        for p in (self.one, other)]
        self.assertEqual(fingerprints[0], fingerprints[1])  # the confirmation fingerprint is unchanged
        with self.assertRaisesRegex(answers_report.ReportError, "Položka Q1.*text otázky"):
            self.report([self.one, other])

    def test_corpora_over_different_revisions_are_refused(self):
        catalog = make_catalog(question_json(
            [fact("f1", F1, [place(EVMN, "EditorViewModelNew.RunNew()", radky=20)])],
            commit=OTHER_REV), base=OTHER_REV)
        root = self.corpus(corpus_one(), name="o", revision=OTHER_REV)
        index = open_corpus(root, self.state)
        measured = card_check.run_card_check(catalog, index, selection(), ScriptedJudge(RULES),
                                             self.cache)
        other = self.tmp / "o-m" / "measurement.json"
        write_measurement(other, measured)
        with self.assertRaisesRegex(answers_report.ReportError, "nad revizí"):
            self.report([self.one, other])
        code, _, err = self.zprava([self.one, other])
        self.assertEqual((code, self.out.exists()), (2, False))
        self.assertIn("revizí", err)

    def test_a_corpus_over_another_revision_than_the_item_carries_is_refused(self):
        other = self.edit(self.one, lambda raw: raw["corpus"].update(revision=OTHER_REV), "c2")
        with self.assertRaisesRegex(answers_report.ReportError,
                                    "položka Q1.*nad revizí|nese položka"):
            self.report([other])

    def test_items_with_different_fingerprints_are_refused(self):
        def change(raw):
            raw["items"][0]["otisk_potvrzeni"] = "jiny-otisk"

        other = self.edit(self.one, change, "c2")
        with self.assertRaisesRegex(answers_report.ReportError, "Položka Q1.*různý otisk"):
            self.report([self.one, other])

    def test_the_same_measurement_twice_is_refused(self):
        with self.assertRaisesRegex(answers_report.ReportError, "víckrát"):
            self.report([self.one, self.one])

    def test_an_unfinished_measurement_is_refused(self):
        other = self.edit(self.one, lambda raw: raw.update(status="nedokonceno"), "c2")
        with self.assertRaises(review.ReviewError):
            self.report([other])
        self.assertEqual(self.zprava([other])[0], 2)

    def test_an_output_path_inside_a_repository_is_refused_before_anything_is_written(self):
        inside = REPO / "tmp-zprava-vystup"
        code, _, err = self.zprava([self.tmp / "neexistuje.json"], out=inside)
        self.assertEqual(code, 2)
        self.assertIn("Výstupní adresář", err)
        self.assertFalse(inside.exists())
        code, _, err = self.zprava([self.one], out=inside)
        self.assertEqual((code, inside.exists()), (2, False))

    def test_an_output_that_is_a_file_is_refused(self):
        target = self.tmp / "soubor"
        target.write_text("x", encoding="utf-8")
        code, _, err = self.zprava([self.one], out=target)
        self.assertEqual(code, 2)
        self.assertIn("není adresář", err)

    def test_the_arguments_are_required(self):
        for name in ("--vystup", "--stav", "--kalibrace", "--source-repo"):
            with self.subTest(name):
                values = {"--source-repo": str(REPO), "--vystup": str(self.out),
                          "--stav": str(self.state), "--kalibrace": str(self.cal)}
                del values[name]
                argv = ["zprava", "--mereni", str(self.one)]
                for key, value in values.items():
                    argv += [key, value]
                code, _, err = self.main(argv)
                self.assertEqual(code, 2)
                self.assertIn(name, err)
        code, _, err = self.main(["zprava", "--source-repo", str(REPO), "--vystup", "x",
                                  "--stav", "y", "--kalibrace", "z"])
        self.assertEqual(code, 2)
        self.assertIn("--mereni", err)


class OwnerTest(ReportCase):
    def setUp(self):
        super().setUp()
        self.catalog = make_catalog(q3())

    def fill(self, review_file, mutate=None):
        data = json.loads(review_file.read_text("utf-8"))
        for row in data["radky"]:
            if row["druh"] == "fakt":
                value = "vyjadruje" if row["stitek_hodnotitele"] == "VYJADRUJE" else "nevyjadruje"
            else:
                value = "obsahuje" if row["stitek_hodnotitele"] == "OBSAHUJE" else "neobsahuje"
            row["rozhodnuti"] = value
        if mutate is not None:
            mutate(data)
        review_file.write_text(json.dumps(data, ensure_ascii=False), encoding="utf-8")
        return data

    def test_the_decisions_of_the_owner_then_let_the_report_be_made_and_are_marked(self):
        path = self.card_measurement("c1", corpus_one(), self.catalog, transform=disputed_copy)
        self.assertEqual(self.zprava([path])[0], 2)
        review_file = self.tmp / "kontrola.json"
        code, _, err = self.main(self.review_args("--mereni", str(path), "--vystup",
                                                  str(review_file), "--seminko", "3"))
        self.assertEqual(code, 0, err)
        data = json.loads(review_file.read_text("utf-8"))
        for row in data["radky"]:
            row["rozhodnuti"] = "vyjadruje" if row["druh"] == "fakt" else "obsahuje"
        review_file.write_text(json.dumps(data, ensure_ascii=False), encoding="utf-8")
        self.assertEqual(self.main(self.review_args("--nacti", str(review_file)))[0], 0)
        code, out, err = self.zprava([path])
        self.assertEqual(code, 0, err)
        report = json.loads((self.out / answers_report.JSON_FILE).read_text("utf-8"))
        rows = report[answers_report.SECTION_ACCEPTANCE]["fakta"]
        self.assertTrue(all(r["posouzeni"]["druh"] == answers_report.ASSESSED_OWNER
                            for r in rows if r["posouzeni"]["jednotek"]))
        text = (self.out / answers_report.MARKDOWN_FILE).read_text("utf-8")
        self.assertIn("rozhodl vlastník", text)
        table = [ln for ln in text.splitlines() if ln.startswith("| Q1 |")]
        self.assertTrue(table)
        self.assertFalse([ln for ln in table if "posouzení modelu" in ln])

    def test_the_drawn_sample_must_be_decided_and_the_agreement_is_counted(self):
        path = self.card_measurement("c1", corpus_one(), self.catalog)
        self.assertEqual(self.zprava([path])[0], 0)  # nothing drawn yet: nothing waits
        review_file = self.tmp / "kontrola.json"
        self.assertEqual(self.main(self.review_args("--mereni", str(path), "--vystup",
                                                    str(review_file), "--seminko", "3"))[0], 0)
        rows = json.loads(review_file.read_text("utf-8"))["radky"]
        self.assertTrue(rows and not any(r["sporne"] for r in rows))
        code, _, err = self.zprava([path], out=self.tmp / "out2")
        self.assertEqual(code, 2)
        self.assertIn("čeká na rozhodnutí vlastníka", err)
        self.assertFalse((self.tmp / "out2").exists())

        def flip_first_fact(data):
            row = next(r for r in data["radky"] if r["druh"] == "fakt")
            row["rozhodnuti"] = "nevyjadruje" if row["stitek_hodnotitele"] == "VYJADRUJE" \
                else "vyjadruje"

        self.fill(review_file, flip_first_fact)
        self.assertEqual(self.main(self.review_args("--nacti", str(review_file)))[0], 0)
        code, _, err = self.zprava([path], out=self.tmp / "out2")
        self.assertEqual(code, 0, err)
        report = json.loads((self.tmp / "out2" / answers_report.JSON_FILE).read_text("utf-8"))
        [agreement] = report["shoda_vlastnika"]
        self.assertEqual((agreement["vzorek"], agreement["shodne"],
                          agreement["rozhodnuti_sporne"]), (len(rows), len(rows) - 1, 0))
        text = (self.tmp / "out2" / answers_report.MARKDOWN_FILE).read_text("utf-8")
        self.assertIn(f"{len(rows) - 1}/{len(rows)}", text)
        self.assertIn("rozhodl vlastník", text)


class JudgeMarkTest(ReportCase):
    def test_an_assessment_by_the_fallback_judge_of_the_same_family_is_marked_everywhere(self):
        catalog = make_catalog(q3())
        path = self.card_measurement("c1", corpus_one(), catalog, selection_=family_selection())
        data = self.report([path])
        self.assertTrue(data["zalozni_hodnotitel"])
        self.assertTrue(data["hodnotitel"]["stejna_rodina"])
        rows = self.facts_of(data)
        self.assertTrue(all(r["posouzeni"]["zalozni_hodnotitel"] for r in rows))
        text = answers_report.render_markdown(data)
        self.assertIn("> Pozor: posouzení udělal záložní hodnotitel stejné rodiny", text)
        marked = [ln for ln in text.splitlines()
                  if ln.startswith("| Q1 |") and "záložní hodnotitel stejné rodiny" in ln]
        self.assertEqual(len(marked), len(rows) + len(data[answers_report.SECTION_ACCEPTANCE][
            "tvrzeni"]))
        code, out, err = self.zprava([path])
        self.assertEqual(code, 0, err)
        self.assertIn("záložní hodnotitel", out)


class CalibrationTest(ReportCase):
    def setUp(self):
        super().setUp()
        self.path = self.card_measurement("c1", corpus_one(), make_catalog(q3()))

    def write_calibration(self, name="kal.json", judge="j1", prompt=PROMPT_VERSION,
                          fingerprint="s" * 8):
        self.cal.mkdir(parents=True, exist_ok=True)
        (self.cal / name).write_text(json.dumps({
            "format_version": 1, "hodnotitel": {"id": judge, "model": "model-1"},
            "prompt_version": prompt, "otisk_sady": fingerprint, "jednotek": 8, "shoda": 0.875,
            "pokryti": 1.0, "sporne": 0,
            "nebezpecne": {"neodpovidajici_veta_uznana": {"pocet": 1, "jednotky": ["p3"]},
                           "parafraze_nepravdy_prehlednuta": {"pocet": 0, "jednotky": []}}}),
            encoding="utf-8")

    def test_a_missing_calibration_is_stated(self):
        data = self.report([self.path])
        self.assertFalse(data["kalibrace"]["nalezena"])
        self.assertEqual((data["kalibrace"]["hodnotitel"], data["kalibrace"]["prompt_version"]),
                         ("j1", PROMPT_VERSION))
        self.assertIn(f"Kalibrace pro hodnotitele j1 a verzi promptu {PROMPT_VERSION} chybí",
                      answers_report.render_markdown(data))
        code, out, _ = self.zprava([self.path])
        self.assertEqual(code, 0)
        self.assertIn("Kalibrace pro tohoto hodnotitele", out)

    def test_a_calibration_of_another_judge_or_prompt_version_does_not_count(self):
        self.write_calibration("a.json", judge="j2")
        self.write_calibration("b.json", prompt="0")
        self.assertFalse(self.report([self.path])["kalibrace"]["nalezena"])

    def test_the_calibration_of_the_pair_is_reported(self):
        self.write_calibration()
        data = self.report([self.path])
        self.assertTrue(data["kalibrace"]["nalezena"])
        self.assertEqual((data["kalibrace"]["soubor"], data["kalibrace"]["shoda"]),
                         ("kal.json", 0.875))
        text = answers_report.render_markdown(data)
        self.assertIn("soubor kal.json", text)
        self.assertIn("neodpovidajici_veta_uznana: 1", text)

    def test_a_given_set_finds_only_a_calibration_made_on_it(self):
        self.write_calibration(fingerprint="s" * 8)
        self.assertTrue(self.report([self.path], set_fingerprint="s" * 8)["kalibrace"]["nalezena"])
        self.assertFalse(self.report([self.path], set_fingerprint="x" * 8)["kalibrace"]["nalezena"])


class FilesTest(ReportCase):
    def test_the_command_writes_both_files_and_the_json_is_ordered(self):
        catalog = make_catalog(q3(), control_question())
        cards = self.card_measurement("c1", corpus_one(), catalog, receipt=RECEIPT)
        code, out, err = self.zprava([cards])
        self.assertEqual(code, 0, err)
        self.assertEqual(sorted(p.name for p in self.out.iterdir()),
                         [answers_report.JSON_FILE, answers_report.MARKDOWN_FILE])
        text = (self.out / answers_report.JSON_FILE).read_text("utf-8")
        data = json.loads(text)
        self.assertEqual(list(data)[0], "format_version")
        self.assertEqual((data["format_version"], data["kind"]), (1, "odpovedi"))
        self.assertEqual(list(data)[1:], sorted(list(data)[1:]))

        def check(value):
            if isinstance(value, dict):
                self.assertEqual(list(value), sorted(value))
                for child in value.values():
                    check(child)
            elif isinstance(value, list):
                for child in value:
                    check(child)

        check({k: v for k, v in data.items() if k != "format_version"})
        self.assertEqual(text, answers_report.report_json_text(data))
        markdown = (self.out / answers_report.MARKDOWN_FILE).read_text("utf-8")
        for heading in ("## Vstupy a identita korpusů", "## Akceptační položka",
                        "## Kontrolní otázky", "## Souhrn stavů faktů",
                        "## Velikosti dokumentů (bajty)", "## Shoda vlastníka s modelem",
                        "## Kalibrace hodnotitele", "## Co toto měření netvrdí"):
            self.assertIn(heading, markdown)
        self.assertIn("Zpráva odpovědí: 1 měření, 1 korpusů.", out)

    def test_what_the_measurement_does_not_claim_is_stated(self):
        path = self.card_measurement("c1", corpus_one(), make_catalog(q3()))
        data = self.report([path])
        joined = " ".join(data["netvrdi"])
        for phrase in ("roste s délkou karty", "Karty vrstvy 2", "s jedním člověkem",
                       "Práh úspěchu se nestanoví"):
            self.assertIn(phrase, joined)
        tail = answers_report.render_markdown(data).split("## Co toto měření netvrdí")[1]
        self.assertEqual(len([ln for ln in tail.splitlines() if ln.startswith("- ")]),
                         len(data["netvrdi"]))

    def test_a_second_run_replaces_the_files_atomically(self):
        path = self.card_measurement("c1", corpus_one(), make_catalog(q3()))
        self.assertEqual(self.zprava([path])[0], 0)
        first = (self.out / answers_report.JSON_FILE).read_text("utf-8")
        self.assertEqual(self.zprava([path])[0], 0)
        self.assertEqual((self.out / answers_report.JSON_FILE).read_text("utf-8"), first)
        self.assertEqual(len(list(self.out.iterdir())), 2)


if __name__ == "__main__":
    unittest.main()
