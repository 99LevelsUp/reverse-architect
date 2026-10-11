"""Calibration of the judge on a frozen set (U9, KTD12).

A scripted runner stands in for the judge; no process is started. All texts
are synthetic.
"""
import json
import shutil
import tempfile
import unittest
from pathlib import Path

import calibration
import judge
from agents.judge_cli import CliReply, JudgeInfrastructureError
from calibration import (CalibrationSetError, find_calibration, load_set, parse_set,
                         run_calibration, write_result)
from judge import JudgeCache, JudgeSelection, JudgeSpec
from prepare import PrepareError, canonical_path

EXAMPLE = Path(__file__).resolve().parent.parent / "examples" / "calibration.example.json"
COMMAND_MARKER = "--tajny-prepinac-hodnotitele"


def make_set(extra_sentence=""):
    q = "Co zobrazuje ViewA?"
    fact = {"id": "F1", "text": "ViewA zobrazuje seznam.", "druh": "fakt"}
    bad = {"id": "X1", "text": "ViewA zobrazuje jen jednu položku.", "druh": "zakazane"}
    items = [
        {"otazka": q, "tvrzeni": fact, "veta": "ViewA zobrazuje seznam položek.",
         "trida": "vyjadruje", "ocekavany_stitek": "VYJADRUJE"},
        {"otazka": q, "tvrzeni": fact, "veta": "ViewA leží v adresáři Views." + extra_sentence,
         "trida": "pravdiva_neodpovida", "ocekavany_stitek": "NEVYJADRUJE"},
        {"otazka": q, "tvrzeni": fact, "veta": "ViewB leží vedle ViewA.",
         "trida": "pravdiva_neodpovida", "ocekavany_stitek": "NEVYJADRUJE"},
        {"otazka": q, "tvrzeni": bad, "veta": "ViewA ukazuje vždy jediný záznam.",
         "trida": "parafraze_nepravdy", "ocekavany_stitek": "OBSAHUJE"},
        {"otazka": q, "tvrzeni": fact, "veta": "ViewA žádný seznam nezobrazuje.",
         "trida": "popreni", "ocekavany_stitek": "NEVYJADRUJE"},
    ]
    return {"format_version": 1, "polozky": items}


class SentenceRunner:
    """Answers each run from `rule(sentence, kind)`; remembers every prompt."""

    def __init__(self, rule, model="model-1"):
        self.rule, self.model, self.prompts = rule, model, []

    def __call__(self, spec, prompt, siblings):
        self.prompts.append(prompt)
        sentence = self._block(prompt)
        kind = "forbidden" if "Obsahuje blok" in prompt else "fact"
        label = self.rule(sentence, kind)
        quote = sentence if label in judge.POSITIVE_LABELS else None
        text = json.dumps({"stitek": label, "citace": quote, "castecne": False},
                          ensure_ascii=False)
        return CliReply(text=text, model=self.model, raw=text)

    @staticmethod
    def _block(prompt):
        lines = prompt.splitlines()
        index = next(i for i, line in enumerate(lines) if line.endswith(" BLOK 1"))
        return lines[index + 1]


def expected_rule(items):
    by_sentence = {i["veta"]: i["ocekavany_stitek"] for i in items}
    return lambda sentence, kind: by_sentence[sentence]


class TempCase(unittest.TestCase):
    def setUp(self):
        self.base = Path(tempfile.mkdtemp(prefix="calib-test-"))
        self.addCleanup(shutil.rmtree, self.base, True)
        self.cache = JudgeCache(self.base / "cache")

    def selection(self, judge_id="j1", model="model-1"):
        spec = JudgeSpec(id=judge_id, family="openai", command=("cli", COMMAND_MARKER),
                         model="alias", output_shape="claude-json")
        return JudgeSelection(spec=spec, smoke_model=model, same_family=False,
                              agent_family="claude")

    def run_with(self, data, rule, **kw):
        runner = SentenceRunner(rule)
        result = run_calibration(parse_set(data), self.selection(**kw), runner, self.cache)
        return result, runner


class MatrixTest(TempCase):
    def test_judge_that_accepts_every_irrelevant_sentence_fills_the_danger_cell(self):
        data = make_set()

        def rule(sentence, kind):
            if kind == "forbidden":
                return "OBSAHUJE"
            return "VYJADRUJE" if "adresáři" in sentence or "vedle" in sentence \
                or "zobrazuje seznam položek" in sentence else "NEVYJADRUJE"

        result, _ = self.run_with(data, rule)
        self.assertEqual(result.unacknowledged_true["pocet"], 2)
        self.assertEqual(result.unacknowledged_true["jednotky"], ["p2", "p3"])
        self.assertEqual(result.missed_paraphrase["pocet"], 0)
        self.assertEqual(result.matrix["pravdiva_neodpovida"]["VYJADRUJE"], 2)
        self.assertLess(result.agreement, 1)

    def test_agreeing_judge_has_full_agreement_and_empty_danger_cells(self):
        data = make_set()
        result, _ = self.run_with(data, expected_rule(data["polozky"]))
        self.assertEqual(result.agreement, 1.0)
        self.assertEqual(result.coverage, 1.0)
        self.assertEqual(result.unacknowledged_true["pocet"], 0)
        self.assertEqual(result.missed_paraphrase["pocet"], 0)
        self.assertEqual(result.matrix["vyjadruje"]["VYJADRUJE"], 1)
        self.assertEqual(result.matrix["popreni"]["NEVYJADRUJE"], 1)

    def test_overlooked_paraphrase_and_coverage(self):
        data = make_set()

        def rule(sentence, kind):
            if kind == "forbidden":
                return "NEOBSAHUJE"
            return "NELZE" if "vedle" in sentence else expected_rule(data["polozky"])(sentence, kind)

        result, _ = self.run_with(data, rule)
        self.assertEqual(result.missed_paraphrase["jednotky"], ["p4"])
        self.assertEqual(result.coverage, 4 / 5)
        self.assertEqual(result.disputed, 1)

    def test_cache_is_used_on_a_second_calibration(self):
        data = make_set()
        rule = expected_rule(data["polozky"])
        _, first = self.run_with(data, rule)
        _, second = self.run_with(data, rule)
        self.assertEqual(len(first.prompts), 15)
        self.assertEqual(second.prompts, [])

    def test_infrastructure_failure_propagates(self):
        def boom(spec, prompt, siblings):
            raise JudgeInfrastructureError("spadlo")

        with self.assertRaises(JudgeInfrastructureError):
            run_calibration(parse_set(make_set()), self.selection(), boom, self.cache)


class SetTest(unittest.TestCase):
    def test_unknown_class_is_rejected(self):
        data = make_set()
        data["polozky"][0]["trida"] = "neznama"
        with self.assertRaisesRegex(CalibrationSetError, "neznámá třída"):
            parse_set(data)

    def test_unknown_label_and_wrong_format_are_rejected(self):
        data = make_set()
        data["polozky"][0]["ocekavany_stitek"] = "ANO"
        with self.assertRaisesRegex(CalibrationSetError, "neznámý štítek"):
            parse_set(data)
        with self.assertRaisesRegex(CalibrationSetError, "verzi formátu"):
            parse_set({"format_version": 2, "polozky": []})
        with self.assertRaisesRegex(CalibrationSetError, "neprázdný seznam"):
            parse_set({"format_version": 1, "polozky": []})

    def test_class_must_fit_the_kind(self):
        data = make_set()
        data["polozky"][3]["tvrzeni"]["druh"] = "fakt"
        with self.assertRaisesRegex(CalibrationSetError, "posuzuje proti druhu"):
            parse_set(data)

    def test_example_set_loads(self):
        loaded = load_set(EXAMPLE)
        self.assertEqual(len(loaded.items), 4)
        self.assertEqual({i.klass for i in loaded.items}, set(calibration.CLASSES))
        self.assertIn("Ukázková sada, nejde o skutečný případ.",
                      EXAMPLE.read_text(encoding="utf-8"))

    def test_fingerprint_follows_the_content_not_the_key_order(self):
        data = make_set()
        reordered = {"polozky": data["polozky"], "format_version": 1}
        self.assertEqual(parse_set(data).fingerprint, parse_set(reordered).fingerprint)
        self.assertNotEqual(parse_set(data).fingerprint, parse_set(make_set("!")).fingerprint)


class OutputTest(TempCase):
    def test_result_carries_identity_and_is_found_for_judge_and_prompt_version(self):
        data = make_set()
        result, _ = self.run_with(data, expected_rule(data["polozky"]))
        out = write_result(result, self.base / "out" / "kal.json")
        text = out.read_text(encoding="utf-8")
        parsed = json.loads(text)
        self.assertEqual(next(iter(parsed)), "format_version")
        self.assertEqual(parsed["hodnotitel"]["id"], "j1")
        self.assertEqual(parsed["hodnotitel"]["model"], "model-1")
        self.assertEqual(parsed["prompt_version"], judge.PROMPT_VERSION)
        self.assertEqual(parsed["otisk_sady"], parse_set(data).fingerprint)
        self.assertNotIn("kappa", [k.lower() for k in parsed])
        self.assertTrue(parsed["netvrdi"])
        self.assertNotIn(COMMAND_MARKER, text)
        self.assertIn("ě", text)  # ensure_ascii=False
        found = find_calibration(self.base / "out", "j1", judge.PROMPT_VERSION,
                                 parse_set(data).fingerprint)
        # write_result returns the canonical path (realpath, case-folded on Windows),
        # find_calibration the path as found under the given directory: compare canonically
        self.assertEqual(canonical_path(found[0]), canonical_path(out))
        self.assertEqual(found[1]["shoda"], 1.0)
        self.assertIsNone(find_calibration(self.base / "out", "j2", judge.PROMPT_VERSION,
                                           parse_set(data).fingerprint))
        self.assertIsNone(find_calibration(self.base / "out", "j1", "999",
                                           parse_set(data).fingerprint))

    def test_changed_set_gives_another_fingerprint_and_old_calibration_is_not_used(self):
        data = make_set()
        result, _ = self.run_with(data, expected_rule(data["polozky"]))
        write_result(result, self.base / "out" / "kal.json")
        changed = parse_set(make_set(" Změna."))
        self.assertNotEqual(changed.fingerprint, result.set_fingerprint)
        self.assertIsNone(find_calibration(self.base / "out", "j1", judge.PROMPT_VERSION,
                                           changed.fingerprint))

    def test_protected_root_is_refused_and_junk_files_are_skipped(self):
        data = make_set()
        result, _ = self.run_with(data, expected_rule(data["polozky"]))
        with self.assertRaises(PrepareError):
            write_result(result, self.base / "kal.json", protected=[str(self.base)])
        with self.assertRaises(CalibrationSetError):
            write_result(result, "  ")
        (self.base / "out").mkdir()
        (self.base / "out" / "junk.json").write_text("{nope", encoding="utf-8")
        self.assertIsNone(find_calibration(self.base / "out", "j1", "1", "x"))


if __name__ == "__main__":
    unittest.main()
