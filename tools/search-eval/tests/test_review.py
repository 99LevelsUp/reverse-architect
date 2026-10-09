"""The owner's review (U7): the file to check, the sample, the decisions and the key (KTD7, KTD8).

Measurements are built here from synthetic units (generic names only); one
test runs the real card check over a synthetic corpus to see that the final
labels and results follow the owner's decisions.
"""
import json
import tempfile
import unittest
from dataclasses import replace
from pathlib import Path

import card_check
import review
from card_check import (LABEL_ABSENT, LABEL_CONTAINS, LABEL_PRESENT, LABEL_UNKNOWN, MODE_WHOLE,
                        Measurement, Plan, UnitResult, write_measurement)
from judge import KIND_FACT, KIND_FORBIDDEN
from tests.test_card_check import (EV, EVMN, EVN, REV, CheckCase, Rule, a1_types, claim,
                                   fact, make_catalog, place, question_json)

PLACE = {"soubor": "src/Acme/ViewA.cs", "clen": "ViewA.Run()", "typ": None, "radky": [10, 20]}


def make_unit(n, label=LABEL_PRESENT, disputed=False, kind=KIND_FACT, text=None, **extra):
    text = text if text is not None else f"Věta karty {n}."
    fields = dict(
        id=f"Q1::f{n}::podpora::sym:Acme#View{n}", question_id="Q1", kind=kind,
        claim_id=f"f{n}", role="podpora", document=f"sym:Acme#View{n}", slot="karta",
        title=f"View{n}", mode=MODE_WHOLE, blocks=None, block_count=1,
        state=card_check.STATE_ASSESSED, run_labels=(label, label), quote=text,
        disputed=disputed, dispute_reason="běhy se liší" if disputed else None,
        model_label=label, final_label=LABEL_UNKNOWN if disputed else label,
        question_text="Co se stane, když chybí dokument?", claim_text=f"Fakt {n}",
        document_path=f"cards/type/View{n}.md", document_text=text, places=(PLACE,))
    fields.update(extra)
    return UnitResult(**fields)


def write_units(directory, units, name="measurement.json", mid="m1", status=card_check.STATUS_DONE):
    measurement = Measurement(
        id=mid, status=status, corpus={"revision": REV, "source": "test"}, items=(), judge={},
        reported_model="model-1", agent_family="claude", prompt_version="p1",
        plan=Plan(questions=(), facts=(), claims=(), units=()), units=tuple(units))
    path = Path(directory) / name
    write_measurement(path, measurement)
    return path


class ReviewCase(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.tmp = Path(tmp.name)
        self.state = self.tmp / "state"
        self.state.mkdir()

    def measurement(self, units, **kwargs):
        return review.load_measurement(write_units(self.tmp, units, **kwargs))

    def review_of(self, units, decisions=None, seed=7):
        return review.build_review([self.measurement(units)], decisions or {}, seed)

    def write_review(self, plan, name="kontrola.json"):
        path = self.tmp / name
        review.write_review(plan, path)
        return path

    @staticmethod
    def fill(path, values):
        """values: row number -> decision or (decision, nepravda)."""
        data = json.loads(path.read_text(encoding="utf-8"))
        for row in data["radky"]:
            value = values.get(row["radek"])
            if value is None:
                continue
            if isinstance(value, tuple):
                row["rozhodnuti"], row["nepravda"] = value
            else:
                row["rozhodnuti"] = value
        path.write_text(json.dumps(data, ensure_ascii=False), encoding="utf-8")
        return data


def twenty_three():
    disputed = [make_unit(n, disputed=True) for n in range(3)]
    present = [make_unit(n) for n in range(10, 25)]
    absent = [make_unit(n, label=LABEL_ABSENT) for n in range(30, 35)]
    return disputed + present + absent


class SampleTest(ReviewCase):
    def test_disputed_all_and_a_sample_of_five_and_five_by_label(self):
        plan = self.review_of(twenty_three())
        self.assertEqual(plan.disputed_count, 3)
        undisputed = [r for r in plan.rows if not r.disputed]
        labels = [r.data["stitek_hodnotitele"] for r in undisputed]
        self.assertEqual((labels.count(LABEL_PRESENT), labels.count(LABEL_ABSENT)), (5, 5))
        self.assertEqual(len(plan.rows), 13)
        self.assertEqual([r.number for r in plan.rows], list(range(1, 14)))
        self.assertEqual(plan.sample_sizes[LABEL_PRESENT], (15, 5))
        self.assertEqual(json.loads(self.write_review(plan).read_text("utf-8"))["seminko"], 7)

    def test_the_same_seed_gives_the_same_sample_and_another_seed_another(self):
        units = twenty_three()
        first = [r.key for r in self.review_of(units, seed=1).rows]
        self.assertEqual(first, [r.key for r in self.review_of(units, seed=1).rows])
        others = {tuple(r.key for r in self.review_of(units, seed=s).rows) for s in range(2, 8)}
        self.assertTrue(others - {tuple(first)})

    def test_the_default_seed_is_deterministic(self):
        units = twenty_three()
        a = review.build_review([self.measurement(units)], {})
        b = review.build_review([self.measurement(units)], {})
        self.assertEqual(a.seed, b.seed)
        self.assertEqual([r.key for r in a.rows], [r.key for r in b.rows])

    def test_sample_size_boundaries(self):
        self.assertEqual(review.sample_size(3), 3)
        self.assertEqual(review.sample_size(5), 5)
        self.assertEqual(review.sample_size(100), 10)
        self.assertEqual(review.sample_size(101), 11)
        plan = self.review_of([make_unit(n) for n in range(3)])
        self.assertEqual(len(plan.rows), 3)
        plan = self.review_of([make_unit(n) for n in range(100)])
        self.assertEqual(len(plan.rows), 10)

    def test_units_that_were_never_judged_are_not_reviewed(self):
        unjudged = make_unit(50, state=card_check.STATE_NO_BOUND_BLOCK, model_label=None,
                             final_label=None, run_labels=())
        plan = self.review_of([make_unit(1), unjudged])
        self.assertEqual(len(plan.rows), 1)

    def test_units_with_the_same_key_are_one_row(self):
        same = make_unit(1, id="Q1::f1::vyvojar::sym:Acme#View1", role="vyvojar")
        plan = self.review_of([make_unit(1), same])
        self.assertEqual(len(plan.rows), 1)
        self.assertEqual(len(plan.rows[0].unit_ids), 2)

    def test_the_row_shows_what_the_owner_needs(self):
        bound = make_unit(2, mode=card_check.MODE_BOUND, document_text=None,
                          blocks=({"text": "Blok A", "section": "S", "anchor": "a.cs:1-2",
                                   "via": "kotva"},), run_labels=(LABEL_PRESENT, LABEL_ABSENT))
        plan = self.review_of([make_unit(1), bound])
        whole, blocks = (r.to_json() for r in plan.rows)
        self.assertEqual(whole["text_dokumentu"], "Věta karty 1.")
        self.assertIsNone(whole["bloky"])
        self.assertEqual(blocks["bloky"], [{"text": "Blok A", "sekce": "S", "kotva": "a.cs:1-2"}])
        self.assertEqual(blocks["stitky_behu"], [LABEL_PRESENT, LABEL_ABSENT])
        self.assertEqual(whole["druh"], "fakt")
        self.assertEqual(whole["cesta"], review.PATH_CARD_CHECK)
        self.assertEqual(whole["dokument"]["cesta"], "cards/type/View1.md")
        self.assertEqual(whole["povolene"], ["vyjadruje", "nevyjadruje", "nelze posoudit"])
        self.assertEqual((whole["rozhodnuti"], whole["nepravda"]), ("", ""))
        self.assertEqual(whole["otazka"]["id"], "Q1")

    def test_the_markdown_has_the_same_row_numbers(self):
        plan = self.review_of(twenty_three())
        path = self.write_review(plan)
        text = path.with_suffix(".md").read_text("utf-8")
        for number in range(1, 14):
            self.assertIn(f"## Řádek {number}:", text)
        self.assertIn("Věta karty 0.", text)

    def test_an_existing_review_file_is_never_overwritten(self):
        plan = self.review_of([make_unit(1)])
        self.write_review(plan)
        with self.assertRaisesRegex(review.ReviewError, "už existuje"):
            self.write_review(plan)


class KeyTest(ReviewCase):
    def test_a_changed_card_sentence_changes_the_key_but_the_judge_does_not(self):
        base = make_unit(1)
        self.assertNotEqual(review.decision_key(base),
                            review.decision_key(make_unit(1, text="Věta karty 1, změněná.")))
        other_judge = replace(base, run_labels=(LABEL_ABSENT,), model_label=LABEL_ABSENT,
                              quote="jiná citace", disputed=True)
        self.assertEqual(review.decision_key(base), review.decision_key(other_judge))
        spaced = make_unit(1, text="Věta   karty\n1.")
        self.assertEqual(review.decision_key(base), review.decision_key(spaced))
        self.assertNotEqual(review.decision_key(base),
                            review.decision_key(replace(base, claim_text="Fakt jinak")))

    def test_a_decided_unit_is_not_shown_again_until_its_text_changes(self):
        units = [make_unit(1, disputed=True), make_unit(2, disputed=True)]
        plan = self.review_of(units)
        path = self.write_review(plan)
        self.fill(path, {1: "vyjadruje"})
        review.ingest_review(path, self.state)
        decisions = review.load_decisions(self.state)
        again = review.build_review([self.measurement(units, name="m2.json")], decisions, 7)
        self.assertEqual([r.unit_ids for r in again.rows], [(units[1].id,)])
        changed = [make_unit(1, disputed=True, text="Věta karty 1 přepsaná."), units[1]]
        shown = review.build_review([self.measurement(changed, name="m3.json")], decisions, 7)
        self.assertEqual(len(shown.rows), 2)

    def test_a_new_assessment_by_another_judge_keeps_the_decision(self):
        units = [make_unit(1, disputed=True)]
        path = self.write_review(self.review_of(units))
        self.fill(path, {1: "nevyjadruje"})
        review.ingest_review(path, self.state)
        reassessed = [replace(units[0], quote="jinak", run_labels=(LABEL_PRESENT,) * 3)]
        plan = review.build_review([self.measurement(reassessed, name="m2.json", mid="m2")],
                                   review.load_decisions(self.state), 7)
        self.assertEqual(plan.rows, ())


class IngestTest(ReviewCase):
    def test_decisions_are_stored_with_the_label_and_the_owner(self):
        units = [make_unit(1, disputed=True), make_unit(2, disputed=True)]
        path = self.write_review(self.review_of(units))
        self.fill(path, {1: "vyjadruje", 2: "Nevyjadřuje"})
        result = review.ingest_review(path, self.state)
        self.assertEqual((result.decided, result.new, result.empty), (2, 2, 0))
        stored = review.load_decisions(self.state)
        self.assertEqual({r["stitek"] for r in stored.values()},
                         {LABEL_PRESENT, LABEL_ABSENT})
        self.assertTrue(all(r["rozhodl"] == review.DECIDED_BY_OWNER for r in stored.values()))
        raw = json.loads(review.decisions_path(self.state).read_text("utf-8"))
        self.assertEqual(next(iter(raw)), "format_version")

    def test_a_typo_is_refused_naming_the_row_and_nothing_is_stored(self):
        units = [make_unit(1, disputed=True), make_unit(2, disputed=True)]
        path = self.write_review(self.review_of(units))
        self.fill(path, {1: "vyjadruje", 2: "vyjadryje"})
        with self.assertRaises(review.ReviewError) as caught:
            review.ingest_review(path, self.state)
        self.assertIn("řádek 2", str(caught.exception))
        self.assertNotIn("řádek 1", str(caught.exception))
        self.assertFalse(review.decisions_path(self.state).exists())

    def test_a_forbidden_claim_row_does_not_accept_vyjadruje(self):
        unit = make_unit(1, disputed=True, kind=KIND_FORBIDDEN)
        path = self.write_review(self.review_of([unit]))
        self.assertEqual(json.loads(path.read_text("utf-8"))["radky"][0]["druh"],
                         "zakazane_tvrzeni")
        for wrong in ("vyjadruje", "vyjadřuje", "nevyjadruje"):
            self.fill(path, {1: wrong})
            with self.assertRaisesRegex(review.ReviewError, "řádek 1"):
                review.ingest_review(path, self.state)
        self.fill(path, {1: "obsahuje"})
        review.ingest_review(path, self.state)
        self.assertEqual(next(iter(review.load_decisions(self.state).values()))["stitek"],
                         LABEL_CONTAINS)

    def test_a_fact_row_does_not_accept_obsahuje(self):
        path = self.write_review(self.review_of([make_unit(1, disputed=True)]))
        self.fill(path, {1: "obsahuje"})
        with self.assertRaisesRegex(review.ReviewError, "řádek 1"):
            review.ingest_review(path, self.state)

    def test_a_row_with_a_key_that_is_not_in_the_measurement_is_refused(self):
        path = self.write_review(self.review_of([make_unit(1, disputed=True)]))
        data = self.fill(path, {1: "vyjadruje"})
        data["radky"][0]["klic"] = "0" * 32
        path.write_text(json.dumps(data), encoding="utf-8")
        with self.assertRaisesRegex(review.ReviewError, "řádek 1.*klíč není v měření"):
            review.ingest_review(path, self.state)
        self.assertFalse(review.decisions_path(self.state).exists())

    def test_a_changed_measurement_is_refused(self):
        units = [make_unit(1, disputed=True)]
        path = self.write_review(self.review_of(units))
        self.fill(path, {1: "vyjadruje"})
        write_units(self.tmp, units, mid="jine")
        with self.assertRaisesRegex(review.ReviewError, "změnilo"):
            review.ingest_review(path, self.state)

    def test_a_partly_filled_file_loads_and_loading_again_does_not_duplicate(self):
        units = [make_unit(n, disputed=True) for n in range(4)]
        path = self.write_review(self.review_of(units))
        self.fill(path, {1: "vyjadruje", 2: ("nevyjadruje", "Karta tvrdí opak")})
        first = review.ingest_review(path, self.state)
        self.assertEqual((first.decided, first.empty, first.proposals), (2, 2, 1))
        before = review.decisions_path(self.state).read_text("utf-8")
        again = review.ingest_review(path, self.state)
        self.assertEqual((again.decided, again.new, again.proposals), (2, 0, 1))
        self.assertEqual(review.decisions_path(self.state).read_text("utf-8"), before)
        self.fill(path, {3: "nelze posoudit"})
        third = review.ingest_review(path, self.state)
        self.assertEqual((third.decided, third.new), (3, 1))
        self.assertEqual(len(review.load_decisions(self.state)), 3)
        proposals = json.loads(third.proposals_path.read_text("utf-8"))
        self.assertEqual(len(proposals["navrhy"]), 1)

    def test_a_falsehood_makes_a_proposal_with_the_place_and_leaves_the_catalog(self):
        catalog = self.tmp / "catalog.json"
        catalog.write_text('{"format_version": 1}', encoding="utf-8")
        before = catalog.read_bytes()
        path = self.write_review(self.review_of([make_unit(1, disputed=True)]))
        self.fill(path, {1: ("nevyjadruje", "  Karta tvrdí, že dokument se tiše zahodí  ")})
        result = review.ingest_review(path, self.state)
        data = json.loads(result.proposals_path.read_text("utf-8"))
        self.assertEqual(result.proposals_path.name, "kontrola.navrhy.json")
        self.assertEqual(data["format_version"], 1)
        proposal = data["navrhy"][0]
        self.assertEqual(proposal["text"], "Karta tvrdí, že dokument se tiše zahodí")
        self.assertEqual(proposal["misto"], [PLACE])
        self.assertEqual((proposal["otazka"], proposal["predmet"]), ("Q1", "f1"))
        self.assertEqual(catalog.read_bytes(), before)

    def test_a_falsehood_without_a_decision_is_refused(self):
        path = self.write_review(self.review_of([make_unit(1, disputed=True)]))
        data = json.loads(path.read_text("utf-8"))
        data["radky"][0]["nepravda"] = "něco"
        path.write_text(json.dumps(data), encoding="utf-8")
        with self.assertRaisesRegex(review.ReviewError, "řádek 1"):
            review.ingest_review(path, self.state)


class RealCheckTest(CheckCase):
    """The final labels and results of a real measurement follow the owner's decisions."""

    def setUp(self):
        super().setUp()
        catalog = make_catalog(question_json(
            [fact("f1", "Tichý návrat, když chybí dokument",
                  [place(EVMN, "EditorViewModelNew.RunNew()", radky=20)])],
            [claim("z1", "Změna řádku gridu se ukládá okamžitě",
                   [place(EVN, "EditorViewNew.OnCellChanged()", radky="42-60")], fakty=("f1",))]))
        rules = [Rule("Tichý návrat", "Tiše", "VYJADRUJE")]
        self.measured, _ = self.check(catalog, rules, types=a1_types())
        self.dir = self.tmp / "out"
        self.dir.mkdir()

    def disputed_copy(self):
        units = tuple(replace(u, disputed=True, final_label=LABEL_UNKNOWN)
                      if u.state == card_check.STATE_ASSESSED else u
                      for u in self.measured.units)
        return replace(self.measured, units=units)

    def test_missing_units_shrink_as_the_owner_decides_and_results_follow(self):
        measurement = self.disputed_copy()
        path = self.dir / "measurement.json"
        write_measurement(path, measurement)
        before = review.review_state(path, self.state)
        judged = [u for u in measurement.units if u.state == card_check.STATE_ASSESSED]
        self.assertTrue(judged)
        self.assertEqual(set(before.missing), {u.id for u in judged})
        self.assertEqual(before.decided, ())
        self.assertTrue(any(r.status == card_check.FACT_DISPUTED for r in before.results.facts))

        plan = review.build_review([review.load_measurement(path)], {}, 3)
        review_file = self.dir / "kontrola.json"
        review.write_review(plan, review_file)
        values = {}
        for row in plan.rows:
            values[row.number] = "vyjadruje" if row.kind == KIND_FACT else "obsahuje"
        ReviewCase.fill(review_file, values)
        review.ingest_review(review_file, self.state)

        after = review.review_state(path, self.state)
        self.assertEqual(after.missing, ())
        self.assertEqual(len(after.decided), len(judged))
        decided = [u for u in after.units if u.decided_by == review.DECIDED_BY_OWNER]
        self.assertEqual(len(decided), len(judged))
        self.assertTrue(all(u.model_label is not None for u in decided))
        statuses = {(r.fact_id, r.role): r.status for r in after.results.facts}
        self.assertTrue(any(s == card_check.FACT_CONFLICT for s in statuses.values()) or
                        any(s == card_check.FACT_PRESENT for s in statuses.values()))

    def test_a_card_rewrite_shows_the_unit_again(self):
        path = self.dir / "measurement.json"
        write_measurement(path, self.disputed_copy())
        loaded = review.load_measurement(path)
        review_file = self.dir / "kontrola.json"
        plan = review.build_review([loaded], {}, 3)
        review.write_review(plan, review_file)
        ReviewCase.fill(review_file, {r.number: ("vyjadruje" if r.kind == KIND_FACT
                                                 else "obsahuje") for r in plan.rows})
        review.ingest_review(review_file, self.state)
        decisions = review.load_decisions(self.state)
        self.assertEqual(review.build_review([loaded], decisions, 3).rows, ())
        rewritten = replace(loaded, units=tuple(
            replace(u, document_text=(u.document_text or "") + "\nNová věta.")
            if u.document_text else u for u in loaded.units))
        self.assertTrue(review.build_review([rewritten], decisions, 3).rows)


class LoadMeasurementTest(ReviewCase):
    def test_an_unfinished_measurement_is_refused(self):
        path = write_units(self.tmp, [make_unit(1)], status=card_check.STATUS_INCOMPLETE)
        with self.assertRaisesRegex(review.ReviewError, "není dokončené"):
            review.load_measurement(path)

    def test_a_measurement_without_the_texts_asks_for_a_new_run(self):
        loaded = self.measurement([make_unit(1, question_text=None)])
        with self.assertRaisesRegex(review.ReviewError, "spustit znovu"):
            review.build_review([loaded], {}, 1)


if __name__ == "__main__":
    unittest.main()
