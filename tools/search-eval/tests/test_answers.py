"""The `answers.py` command line: `kontrola-karet`, its exit codes and its structure (U5).

The corpus, the catalog and the judge are synthetic (`tests/test_card_check.py`
builds them); the judge is a scripted runner handed to `main`, so no process is
started.
"""
import argparse
import contextlib
import io
import json
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import answers
import card_check
from agents.judge_cli import JudgeInfrastructureError
from tests.test_card_check import (EVMN, EXT, OTHER_REV, REV, Rule, ScriptedJudge, a1_types,
                                   build_corpus, catalog_json, claim, fact, place,
                                   question_json)

REPO = Path(__file__).resolve().parents[3]
EVN = "src/Acme/EditorViewNew.cs"


def item():
    return question_json(
        [fact("f1", "Tichý návrat, když chybí dokument",
              [place(EVMN, "EditorViewModelNew.RunNew()", radky=20)]),
         fact("f3", "Externí kontrola ověří dokument",
              [place(EXT, "ExternalCheck.Run()", radky="30-35")])],
        [claim("z1", "Změna řádku gridu se ukládá okamžitě",
               [place(EVN, "EditorViewNew.OnCellChanged()", radky="42-60")])])


RULES = [Rule("Externí kontrola", "Zavolá externí registr", "VYJADRUJE"),
         Rule("řádku gridu", "Změna řádku gridu", "OBSAHUJE")]


class AnswersCase(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.tmp = Path(tmp.name)
        self.state = self.tmp / "state"
        self.corpus = build_corpus(self.tmp / "corpus", a1_types())
        self.catalog = self.write_json("catalog.json", catalog_json(item()))
        self.judges = self.write_json("judges.json", {
            "format_version": 1,
            "hodnotitele": [{"id": "j1", "rodina": "openai", "model": "alias",
                             "prikaz": ["cli-ktery-se-nespusti"], "vystup": "claude-json"}]})

    def write_json(self, name, data):
        path = self.tmp / name
        path.write_text(json.dumps(data, ensure_ascii=False), encoding="utf-8")
        return path

    def args(self, drop=(), **replace):
        values = {"--source-repo": str(REPO), "--agent-family": "claude",
                  "--katalog": str(self.catalog), "--korpus": str(self.corpus),
                  "--hodnotitele": str(self.judges), "--stav": str(self.state)}
        values.update(replace)
        out = ["kontrola-karet"]
        for key, value in values.items():
            if key not in drop:
                out += [key, value]
        return out

    def run_main(self, argv, runner=None):
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = answers.main(argv, runner=runner)
        return code, out.getvalue(), err.getvalue()

    def measurement(self):
        return json.loads(card_check.measurement_path(self.state).read_text(encoding="utf-8"))


class ArgumentsTest(AnswersCase):
    def test_missing_source_repo_is_an_input_error(self):
        runner = ScriptedJudge(RULES)
        code, _, err = self.run_main(self.args(drop=("--source-repo",)), runner)
        self.assertEqual(code, 2)
        self.assertIn("--source-repo", err)
        self.assertEqual((runner.calls, runner.smoke_calls), ([], 0))

    def test_missing_agent_family_is_an_input_error(self):
        code, _, err = self.run_main(self.args(drop=("--agent-family",)), ScriptedJudge(RULES))
        self.assertEqual(code, 2)
        self.assertIn("--agent-family", err)

    def test_every_other_required_argument_is_required(self):
        for name in ("--katalog", "--korpus", "--hodnotitele", "--stav"):
            with self.subTest(name):
                code, _, err = self.run_main(self.args(drop=(name,)), ScriptedJudge(RULES))
                self.assertEqual(code, 2)
                self.assertIn(name, err)

    def test_no_subcommand_and_an_unknown_one_are_input_errors(self):
        self.assertEqual(self.run_main([])[0], 2)
        self.assertEqual(self.run_main(["neznamy", "--source-repo", str(REPO)])[0], 2)

    def test_help_is_not_an_error(self):
        self.assertEqual(self.run_main(["kontrola-karet", "--help"])[0], 0)

    def test_unreadable_inputs_are_input_errors(self):
        for name, bad in (("--katalog", self.tmp / "nic.json"),
                          ("--hodnotitele", self.tmp / "nic.json"),
                          ("--korpus", self.tmp / "nic")):
            with self.subTest(name):
                runner = ScriptedJudge(RULES)
                code, _, err = self.run_main(self.args(**{name: str(bad)}), runner)
                self.assertEqual(code, 2)
                self.assertTrue(err.startswith("Chyba vstupu"))
                self.assertEqual(runner.calls, [])

    def test_state_directory_inside_a_repository_is_refused(self):
        code, _, err = self.run_main(self.args(**{"--stav": str(REPO / "tmp-stav")}),
                                     ScriptedJudge(RULES))
        self.assertEqual(code, 2)
        self.assertIn("Stavový adresář", err)
        self.assertFalse((REPO / "tmp-stav").exists())

    def test_the_subcommands_are_a_list_a_later_unit_can_extend(self):
        seen = []

        def configure(parser):
            parser.add_argument("--navic", required=True)

        def command(args, services):
            seen.append((args.navic, str(args.source_repo)))
            return 0

        extra = answers.Subcommand("zprava", "zkušební podpříkaz", configure, command)
        with mock.patch.object(answers, "SUBCOMMANDS", (*answers.SUBCOMMANDS, extra)):
            self.assertEqual(self.run_main(["zprava", "--source-repo", "x", "--navic", "1"])[0],
                             0)
            self.assertEqual(self.run_main(["zprava", "--navic", "1"])[0], 2)  # common argument
        self.assertEqual(seen, [("1", "x")])
        self.assertEqual([c.name for c in answers.SUBCOMMANDS], ["kontrola-karet"])

    def test_the_parser_names_the_required_arguments(self):
        parser = answers.build_parser()
        self.assertIsInstance(parser, argparse.ArgumentParser)
        text = parser.format_help()
        self.assertIn("kontrola-karet", text)


class CardCheckCommandTest(AnswersCase):
    def test_a_complete_run_writes_the_measurement_and_returns_0(self):
        runner = ScriptedJudge(RULES)
        code, out, _ = self.run_main(self.args(), runner)
        self.assertEqual(code, 0, out)
        data = self.measurement()
        self.assertEqual(next(iter(data)), "format_version")
        self.assertEqual(data["status"], "hotovo")
        self.assertEqual(data["agent_family"], "claude")
        self.assertEqual(data["judge"]["id"], "j1")
        self.assertNotIn("cli-ktery-se-nespusti", json.dumps(data))
        statuses = {(r["fact_id"], r["role"]): r["status"] for r in data["results"]["facts"]}
        self.assertEqual(statuses[("f1", "podpora")], card_check.FACT_MISSING)
        self.assertEqual(statuses[("f3", "vyvojar")], card_check.FACT_PRESENT)
        self.assertEqual(data["results"]["claims"][0]["status"], card_check.CLAIM_FOUND)
        self.assertEqual(runner.smoke_calls, 1)
        self.assertTrue((self.state / card_check.JUDGE_CACHE_DIR).is_dir())

    def test_a_corpus_of_another_revision_is_refused_before_any_judge_call(self):
        other = build_corpus(self.tmp / "other", a1_types(), revision=OTHER_REV)
        runner = ScriptedJudge(RULES)
        code, _, err = self.run_main(self.args(**{"--korpus": str(other)}), runner)
        self.assertEqual(code, 2)
        self.assertIn("revize", err)
        self.assertEqual((runner.calls, runner.smoke_calls), ([], 0))
        self.assertFalse(card_check.measurement_path(self.state).exists())

    def test_a_catalog_without_a_question_is_an_input_error(self):
        case = {"id": "c1", "prompt": "p", "bug_confirmed_at_base": True,
                "pseudonymization_confirmed": True, "used_for_card_tuning": False,
                "prompt_names_location": False, "druh": "chyba",
                "acceptable_locations": [{"file": "a.cs", "member": "A.B", "line": 1,
                                          "fix_commit": REV}]}
        path = self.write_json("only-bugs.json", catalog_json(case))
        code, _, err = self.run_main(self.args(**{"--katalog": str(path)}), ScriptedJudge())
        self.assertEqual(code, 2)
        self.assertIn("otazka", err)

    def test_no_judge_available_is_code_3(self):

        def failing(spec, prompt, siblings):
            raise JudgeInfrastructureError("scénář: smoke test nejde")

        code, _, err = self.run_main(self.args(), failing)
        self.assertEqual(code, 3)
        self.assertIn("Žádný hodnotitel", err)

    def test_interrupt_returns_130_and_leaves_the_measurement_incomplete(self):
        runner = ScriptedJudge(RULES, fail_after=2, fail=KeyboardInterrupt())
        code, _, err = self.run_main(self.args(), runner)
        self.assertEqual(code, 130)
        self.assertIn("nedokončené", err)
        data = self.measurement()
        self.assertEqual(data["status"], "nedokonceno")
        self.assertIsNone(data["results"])

    def test_a_failing_judge_gives_3_and_the_second_run_finishes_from_the_cache(self):
        reference = ScriptedJudge(RULES)
        code, _, _ = self.run_main(self.args(**{"--stav": str(self.tmp / "reference")}),
                                   reference)
        self.assertEqual(code, 0)
        total = len(reference.calls)

        first = ScriptedJudge(RULES, fail_after=5)
        code, _, err = self.run_main(self.args(), first)
        self.assertEqual(code, 3)
        self.assertIn("nedokončené", err)
        self.assertEqual(self.measurement()["status"], "nedokonceno")
        self.assertEqual(len(first.calls), 5)

        second = ScriptedJudge(RULES)
        code, _, _ = self.run_main(self.args(), second)
        self.assertEqual(code, 0)
        self.assertEqual(len(second.calls), total - 5)  # nothing stored was asked again
        data = self.measurement()
        self.assertEqual(data["status"], "hotovo")
        self.assertEqual(data["id"], json.loads(
            card_check.measurement_path(self.tmp / "reference").read_text(
                encoding="utf-8"))["id"])

    def test_a_measurement_of_other_inputs_is_kept_under_its_id(self):
        self.assertEqual(self.run_main(self.args(), ScriptedJudge(RULES))[0], 0)
        first_id = self.measurement()["id"]
        # a different agent family changes what is measured
        self.assertEqual(self.run_main(self.args(**{"--agent-family": "openai"}),
                                       ScriptedJudge(RULES))[0], 0)
        self.assertNotEqual(self.measurement()["id"], first_id)
        self.assertTrue((self.state / f"measurement.{first_id}.json").is_file())

    def test_the_command_does_not_load_the_corpus_receipt_path(self):
        import prepare
        with mock.patch.object(prepare, "load_corpus",
                               side_effect=AssertionError("load_corpus se volat nesmí")):
            self.assertEqual(self.run_main(self.args(), ScriptedJudge(RULES))[0], 0)


if __name__ == "__main__":
    unittest.main()
