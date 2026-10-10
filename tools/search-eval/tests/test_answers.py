"""The `answers.py` command line: `kontrola-karet`, its exit codes and its structure (U5).

The corpus, the catalog and the judge are synthetic (`tests/test_card_check.py`
builds them); the judge is a scripted runner handed to `main`, so no process is
started.
"""
import argparse
import contextlib
import hashlib
import io
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import answers
import calibration
import card_check
import prepare
import report
import review
import run
from agents.judge_cli import CliReply, JudgeInfrastructureError, run_judge_cli
from catalog import parse_catalog
from tests.test_calibration import SentenceRunner, expected_rule
from tests.test_card_check import (EVMN, EXT, OTHER_REV, REV, Rule, ScriptedJudge, a1_types,
                                   build_corpus, catalog_json, claim, fact, place,
                                   question_json)
from tests.test_prepare import commit_all, git, write

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

        extra = answers.Subcommand("zkusebni", "zkušební podpříkaz", configure, command)
        with mock.patch.object(answers, "SUBCOMMANDS", (*answers.SUBCOMMANDS, extra)):
            self.assertEqual(self.run_main(["zkusebni", "--source-repo", "x", "--navic", "1"])[0],
                             0)
            self.assertEqual(self.run_main(["zkusebni", "--navic", "1"])[0], 2)  # common argument
        self.assertEqual(seen, [("1", "x")])
        self.assertEqual([c.name for c in answers.SUBCOMMANDS],
                         ["kontrola-karet", "kontrola", "hodnot-odpovedi", "zprava", "kalibrace"])

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

    def test_two_corpora_of_one_revision_are_two_measurements_and_none_is_overwritten(self):
        other = build_corpus(self.tmp / "other", a1_types())
        self.assertEqual(self.run_main(self.args(), ScriptedJudge(RULES))[0], 0)
        first = self.measurement()
        code, out, err = self.run_main(self.args(**{"--korpus": str(other)}), ScriptedJudge(RULES))
        self.assertEqual(code, 0, err)
        second = self.measurement()
        self.assertEqual(first["corpus"]["revision"], second["corpus"]["revision"])
        self.assertNotEqual(first["id"], second["id"])
        archived = self.state / f"measurement.{first['id']}.json"
        self.assertEqual(json.loads(archived.read_text(encoding="utf-8"))["id"], first["id"])
        self.assertIn("Dřívější měření", out)
        self.assertEqual(second["corpus"]["corpus_path"], os.path.realpath(other))

    def test_a_judge_that_cannot_be_started_during_the_run_is_code_3_and_incomplete(self):
        scripted = ScriptedJudge(RULES)
        unstartable = str(self.tmp / "neni" / "cli.exe")

        def runner(spec, prompt, siblings):
            if prompt.startswith("KONTROLA HODNOTITELE"):
                return scripted(spec, prompt, siblings)  # the smoke test passes
            return run_judge_cli(command=(unstartable,), family="openai",
                                 output_shape="claude-json", credentials=[], variables=[],
                                 prompt=prompt, isolation_root=self.tmp / "izolace", seconds=10)

        code, _, err = self.run_main(self.args(), runner)
        self.assertEqual(code, 3)
        self.assertIn("nedokončené", err)
        self.assertIn("Hodnotitele nejde spustit", err)
        data = self.measurement()
        self.assertEqual((data["status"], data["results"]), ("nedokonceno", None))
        self.assertEqual(list((self.tmp / "izolace").iterdir()), [])


class ReviewCommandTest(AnswersCase):
    """`answers.py kontrola`: the file to check and loading it back."""

    def setUp(self):
        super().setUp()
        self.measurement_file = None

    def finished_measurement(self):
        code, _, _ = self.run_main(self.args(), ScriptedJudge(RULES))
        self.assertEqual(code, 0)
        return card_check.measurement_path(self.state)

    def review_args(self, *extra, **replace):
        values = {"--source-repo": str(REPO), "--stav": str(self.state)}
        values.update(replace)
        out = ["kontrola"]
        for key, value in values.items():
            out += [key, value]
        return out + list(extra)

    def test_the_review_file_and_the_decisions_round_trip(self):
        measurement = self.finished_measurement()
        target = self.tmp / "kontrola.json"
        code, out, err = self.run_main(self.review_args(
            "--mereni", str(measurement), "--vystup", str(target), "--seminko", "5"))
        self.assertEqual(code, 0, err)
        self.assertTrue(target.with_suffix(".md").is_file())
        data = json.loads(target.read_text(encoding="utf-8"))
        self.assertEqual(data["seminko"], 5)
        self.assertTrue(data["radky"])
        for row in data["radky"]:
            row["rozhodnuti"] = "vyjadruje" if row["druh"] == "fakt" else "neobsahuje"
        target.write_text(json.dumps(data, ensure_ascii=False), encoding="utf-8")
        code, out, err = self.run_main(self.review_args("--nacti", str(target)))
        self.assertEqual(code, 0, err)
        self.assertEqual(len(review.load_decisions(self.state)) > 0, True)
        state = review.review_state(measurement, self.state)
        self.assertEqual(state.missing, ())
        # decided units are not offered again
        again = self.tmp / "dalsi.json"
        code, out, err = self.run_main(self.review_args(
            "--mereni", str(measurement), "--vystup", str(again)))
        self.assertEqual(code, 0, err)
        self.assertEqual(json.loads(again.read_text(encoding="utf-8"))["radky"], [])

    def test_the_drawn_sample_is_remembered_and_waits_for_the_owner(self):
        measurement = self.finished_measurement()
        self.assertEqual(review.review_state(measurement, self.state).missing, ())
        target = self.tmp / "kontrola.json"
        code, _, err = self.run_main(self.review_args(
            "--mereni", str(measurement), "--vystup", str(target), "--seminko", "5"))
        self.assertEqual(code, 0, err)
        drawn = review.load_sampled(self.state)
        self.assertTrue(drawn)
        self.assertEqual({r["klic"] for r in json.loads(target.read_text("utf-8"))["radky"]
                          if not r["sporne"]}, set(drawn))
        waiting = review.review_state(measurement, self.state).missing
        self.assertTrue(waiting)
        data = json.loads(target.read_text(encoding="utf-8"))
        for row in data["radky"]:
            row["rozhodnuti"] = "vyjadruje" if row["druh"] == "fakt" else "neobsahuje"
        target.write_text(json.dumps(data, ensure_ascii=False), encoding="utf-8")
        self.assertEqual(self.run_main(self.review_args("--nacti", str(target)))[0], 0)
        self.assertEqual(review.review_state(measurement, self.state).missing, ())

    def test_an_unfinished_measurement_is_code_2(self):
        measurement = self.finished_measurement()
        data = json.loads(measurement.read_text(encoding="utf-8"))
        data["status"] = card_check.STATUS_INCOMPLETE
        measurement.write_text(json.dumps(data, ensure_ascii=False), encoding="utf-8")
        target = self.tmp / "kontrola.json"
        code, _, err = self.run_main(self.review_args(
            "--mereni", str(measurement), "--vystup", str(target)))
        self.assertEqual(code, 2)
        self.assertIn("není dokončené", err)
        self.assertFalse(target.exists())

    def test_a_wrong_decision_is_code_2_and_names_the_row(self):
        measurement = self.finished_measurement()
        target = self.tmp / "kontrola.json"
        self.run_main(self.review_args("--mereni", str(measurement), "--vystup", str(target)))
        data = json.loads(target.read_text(encoding="utf-8"))
        data["radky"][0]["rozhodnuti"] = "překlep"
        target.write_text(json.dumps(data, ensure_ascii=False), encoding="utf-8")
        code, _, err = self.run_main(self.review_args("--nacti", str(target)))
        self.assertEqual(code, 2)
        self.assertIn("řádek 1", err)

    def test_the_modes_need_their_arguments(self):
        self.assertEqual(self.run_main(self.review_args())[0], 2)
        self.assertEqual(self.run_main(self.review_args("--mereni", "x.json"))[0], 2)
        code, _, err = self.run_main(self.review_args("--nacti", "x.json", "--vystup", "y.json"))
        self.assertEqual(code, 2)
        self.assertEqual(self.run_main(["kontrola", "--source-repo", str(REPO)])[0], 2)

    def test_the_state_directory_inside_a_repository_is_refused(self):
        code, _, err = self.run_main(self.review_args(
            "--nacti", "x.json", **{"--stav": str(REPO / "tmp-stav")}))
        self.assertEqual(code, 2)
        self.assertIn("Stavový adresář", err)

    def test_one_owner_review_over_two_measurements_of_one_revision_loads_back(self):
        first_path = self.finished_measurement()
        first_id = self.measurement()["id"]
        other = build_corpus(self.tmp / "other", a1_types())
        self.assertEqual(self.run_main(self.args(**{"--korpus": str(other)}),
                                       ScriptedJudge(RULES))[0], 0)
        current = card_check.measurement_path(self.state)
        archived = self.state / f"measurement.{first_id}.json"
        self.assertTrue(archived.is_file())
        self.assertEqual(first_path, current)  # the file the first run wrote was moved aside
        target = self.tmp / "kontrola.json"
        code, _, err = self.run_main(self.review_args(
            "--mereni", str(archived), "--mereni", str(current), "--vystup", str(target),
            "--seminko", "5"))
        self.assertEqual(code, 0, err)
        data = json.loads(target.read_text(encoding="utf-8"))
        self.assertEqual({row["mereni"] for row in data["radky"]},
                         {first_id, self.measurement()["id"]})
        for row in data["radky"]:
            row["rozhodnuti"] = "vyjadruje" if row["druh"] == "fakt" else "neobsahuje"
        target.write_text(json.dumps(data, ensure_ascii=False), encoding="utf-8")
        code, _, err = self.run_main(self.review_args("--nacti", str(target)))
        self.assertEqual(code, 0, err)
        for path in (archived, current):
            self.assertEqual(review.review_state(path, self.state).missing, ())

    def test_a_sample_that_cannot_be_recorded_leaves_no_review_file(self):
        measurement = self.finished_measurement()
        target = self.tmp / "kontrola.json"
        # the record is a directory: the write of the sampled units fails with an OS error
        review.sampled_path(self.state).mkdir(parents=True)
        code, _, err = self.run_main(self.review_args(
            "--mereni", str(measurement), "--vystup", str(target)))
        self.assertEqual(code, 2)
        self.assertIn("nejde uložit", err)
        self.assertEqual((target.exists(), target.with_suffix(".md").exists()), (False, False))

    def test_the_sample_is_known_before_the_review_file_is_published(self):
        measurement = self.finished_measurement()
        target = self.tmp / "kontrola.json"
        order = []
        real = review._write_json_atomic

        def spy(path, data):
            order.append(path.name)
            return real(path, data)

        with mock.patch.object(review, "_write_json_atomic", spy):
            code, _, err = self.run_main(self.review_args(
                "--mereni", str(measurement), "--vystup", str(target)))
        self.assertEqual(code, 0, err)
        self.assertEqual(order, [review.SAMPLED_FILE, "kontrola.json"])


# --- hodnot-odpovedi (KTD10) --------------------------------------------------------------------

EVMN_TEXT = "src/Acme/EditorViewModelNew.cs"
SILENT_WITH_TYPE = "Metoda `EditorViewModelNew.RunNew()` se při chybějícím dokumentu tiše vrátí."
SILENT_WITHOUT_TYPE = "Při chybějícím dokumentu se editor tiše vrátí a nic neuloží."
SILENT_WITH_ANCHOR = f"Při chybějícím dokumentu se editor tiše vrátí (`{EVMN_TEXT}:10-45`)."
SILENT_WRONG_ANCHOR = "Při chybějícím dokumentu se editor tiše vrátí (`src/Acme/Other.cs:10-45`)."
EXTERNAL = "Externí kontrola zavolá externí registr (`ExternalCheck.Run()`)."
GRID = "Změna řádku gridu se ukládá okamžitě, píše `EditorViewNew.OnCellChanged()`."


def answer_question():
    """A question with a framework fact (`ramcova`), a place fact (`misto`) and a claim."""
    return question_json(
        [fact("f1", "Tichý návrat, když chybí dokument",
              [place(EVMN, "EditorViewModelNew.RunNew()", radky=20)]),
         fact("f2", "Externí kontrola ověří dokument",
              [place(EXT, "ExternalCheck.Run()", radky="30-35")], role="misto")],
        [claim("z1", "Změna řádku gridu se ukládá okamžitě",
               [place("src/Acme/EditorViewNew.cs", "EditorViewNew.OnCellChanged()",
                      radky="42-60")], fakty=["f1"])], qid="Q1")


ANSWER_RULES = [Rule("Tichý návrat", "tiše vrátí", "VYJADRUJE"),
                Rule("Externí kontrola", "externí registr", "VYJADRUJE"),
                Rule("řádku gridu", "Změna řádku gridu", "OBSAHUJE")]

# The attempt directory's name decides where the measurement goes.
ATTEMPT = "pokus-20261002-120000-seed7"


def receipt(revision=REV):
    return {"base_revision": revision, "tree_fingerprint": "t" * 8, "ra_version": "0.0.0",
            "options": [], "corpus_fingerprint": "c" * 8, "generation_seconds": 1.5}


class AnswerCheckCase(AnswersCase):
    """An experiment directory written by hand, the way `run.py` writes it, and the command."""

    def setUp(self):
        super().setUp()
        self.catalog = self.write_json("catalog.json", catalog_json(answer_question()))
        self.question = parse_catalog(catalog_json(answer_question())).questions[0]

    def attempt(self, runs, *, status="completed", role="podpora", adapter="claude-code",
                revision=REV, variants=("A", "B"), question=None):
        """Write the attempt; `runs` are (order, variant, state, answer_text, reason) rows."""
        directory = self.tmp / "out" / ATTEMPT
        (directory / "runs").mkdir(parents=True, exist_ok=True)
        question = question or self.question
        template = run.prompt_template("Zdroje.", role, "otazka")
        digest = hashlib.sha256(run.build_prompt(template, question).encode("utf-8")).hexdigest()
        data = {"format_version": report.REPORT_FORMAT_VERSION,
                "experiment": {"id": ATTEMPT, "status": status, "role": role, "adapter": adapter,
                               "base_revision": revision, "tree_fingerprint": "t" * 8,
                               "variants": [{"name": name, "corpus_dir": None,
                                             "corpus_receipt": receipt(revision),
                                             "prompt_template": "",
                                             "question_prompt_template": template,
                                             "prompt_sha256": {question.id: digest}}
                                            for name in variants]},
                "questions": [{"question_id": question.id, "variants": []}]}
        (directory / "report.json").write_text(json.dumps(data), encoding="utf-8")
        for order, variant, state, text, reason in runs:
            row = {"order": order, "case_id": question.id, "variant": variant, "attempt": 1,
                   "state": state, "reason": reason, "answer_text": text}
            (directory / "runs" / f"{order:03d}.result.json").write_text(
                json.dumps(row, ensure_ascii=False), encoding="utf-8")
        return directory

    def command(self, directory, *extra, **replace):
        values = {"--source-repo": str(REPO), "--katalog": str(self.catalog),
                  "--hodnotitele": str(self.judges), "--stav": str(self.state)}
        values.update(replace)
        argv = ["hodnot-odpovedi", str(directory)]
        for key, value in values.items():
            argv += [key, value]
        return argv + list(extra)

    def measurement_file(self):
        return card_check.measurement_path(self.state / "odpovedi" / ATTEMPT)

    def load(self):
        return json.loads(self.measurement_file().read_text(encoding="utf-8"))

    @staticmethod
    def answered(order, variant, text):
        return (order, variant, "answered", text, None)

    @staticmethod
    def unanswered(order, variant, reason="no_answer"):
        return (order, variant, "unanswered", None, reason)

    @staticmethod
    def facts(data, fact_id):
        return [r for r in data["results"]["facts"] if r["fact_id"] == fact_id]


class AnswerBindingTest(AnswerCheckCase):
    def test_an_answer_without_a_citation_that_names_the_type_binds(self):
        directory = self.attempt([self.answered(1, "A", SILENT_WITH_TYPE)])
        code, out, err = self.run_main(self.command(directory), ScriptedJudge(ANSWER_RULES))
        self.assertEqual(code, 0, err)
        data = self.load()
        [f1] = self.facts(data, "f1")
        self.assertEqual((f1["status"], f1["role"], f1["variant"], f1["run"]),
                         (card_check.FACT_PRESENT, "podpora", "A", 1))
        unit = next(u for u in data["units"] if u["claim_id"] == "f1")
        self.assertEqual((unit["state"], unit["mode"], unit["binding"]["via"]),
                         (card_check.STATE_ASSESSED, card_check.MODE_WHOLE, "jmeno"))
        self.assertEqual((unit["document_path"], unit["document"]), ("odpověď agenta",
                                                                      "odpoved:001"))
        self.assertEqual(unit["document_text"], SILENT_WITH_TYPE)

    def test_an_answer_that_does_not_name_it_and_states_the_fact_is_disputed_without_binding(self):
        directory = self.attempt([self.answered(1, "A", SILENT_WITHOUT_TYPE)])
        code, _, err = self.run_main(self.command(directory), ScriptedJudge(ANSWER_RULES))
        self.assertEqual(code, 0, err)
        data = self.load()
        unit = next(u for u in data["units"] if u["claim_id"] == "f1")
        self.assertEqual(unit["state"], card_check.STATE_UNBOUND_FACT)
        self.assertTrue(unit["disputed"])
        self.assertEqual((unit["model_label"], unit["final_label"]),
                         (card_check.LABEL_PRESENT, card_check.LABEL_UNKNOWN))
        [f1] = self.facts(data, "f1")
        self.assertEqual(f1["status"], card_check.FACT_DISPUTED)
        self.assertEqual(f1["disputed_units"], [unit["id"]])

    def test_a_cited_anchor_binds_by_the_anchor_and_a_wrong_one_does_not(self):
        directory = self.attempt([self.answered(1, "A", SILENT_WITH_ANCHOR),
                                  self.answered(2, "B", SILENT_WRONG_ANCHOR)])
        code, _, err = self.run_main(self.command(directory), ScriptedJudge(ANSWER_RULES))
        self.assertEqual(code, 0, err)
        data = self.load()
        by_run = {r["run"]: r for r in self.facts(data, "f1")}
        self.assertEqual(by_run[1]["status"], card_check.FACT_PRESENT)
        self.assertEqual(by_run[2]["status"], card_check.FACT_DISPUTED)
        unit = next(u for u in data["units"] if u["claim_id"] == "f1" and u["run"] == 1)
        self.assertEqual((unit["binding"]["via"], unit["binding"]["anchor"]),
                         ("kotva", f"{EVMN_TEXT}:10-45"))

    def test_a_forbidden_claim_in_the_answer_puts_the_fact_in_conflict(self):
        directory = self.attempt([self.answered(1, "A", f"{SILENT_WITH_TYPE}\n\n{GRID}")])
        code, _, err = self.run_main(self.command(directory), ScriptedJudge(ANSWER_RULES))
        self.assertEqual(code, 0, err)
        data = self.load()
        [f1] = self.facts(data, "f1")
        self.assertEqual(f1["status"], card_check.FACT_CONFLICT)
        [claim_result] = data["results"]["claims"]
        self.assertEqual((claim_result["status"], claim_result["variant"], claim_result["run"]),
                         (card_check.CLAIM_FOUND, "A", 1))


class AnswerRoleTest(AnswerCheckCase):
    def test_support_judges_only_framework_facts_and_the_claims(self):
        directory = self.attempt([self.answered(1, "A", f"{SILENT_WITH_TYPE}\n\n{EXTERNAL}")])
        runner = ScriptedJudge(ANSWER_RULES)
        code, _, err = self.run_main(self.command(directory), runner)
        self.assertEqual(code, 0, err)
        data = self.load()
        self.assertEqual({(r["fact_id"], r["role"]) for r in data["results"]["facts"]},
                         {("f1", "podpora")})
        self.assertEqual(len(runner.calls_for("Externí kontrola")), 0)
        self.assertEqual(len(data["results"]["claims"]), 1)
        self.assertEqual(data["pokus"]["role"], "podpora")

    def test_developer_judges_every_fact(self):
        directory = self.attempt([self.answered(1, "A", f"{SILENT_WITH_TYPE}\n\n{EXTERNAL}")],
                                 role="vyvojar")
        runner = ScriptedJudge(ANSWER_RULES)
        code, _, err = self.run_main(self.command(directory), runner)
        self.assertEqual(code, 0, err)
        data = self.load()
        self.assertEqual({(r["fact_id"], r["role"]): r["status"] for r in data["results"]["facts"]},
                         {("f1", "vyvojar"): card_check.FACT_PRESENT,
                          ("f2", "vyvojar"): card_check.FACT_PRESENT})


class AnswerWithoutAnswerTest(AnswerCheckCase):
    def test_runs_without_an_answer_count_as_missing_and_are_listed_apart(self):
        directory = self.attempt([self.answered(1, "A", SILENT_WITH_TYPE),
                                  self.unanswered(2, "A", "cap_steps"),
                                  self.unanswered(3, "B")])
        runner = ScriptedJudge(ANSWER_RULES)
        code, out, err = self.run_main(self.command(directory), runner)
        self.assertEqual(code, 0, err)
        data = self.load()
        by_run = {r["run"]: r for r in self.facts(data, "f1")}
        self.assertEqual({k: v["status"] for k, v in by_run.items()},
                         {1: card_check.FACT_PRESENT, 2: card_check.FACT_NO_ANSWER,
                          3: card_check.FACT_NO_ANSWER})
        self.assertEqual(by_run[2]["reasons"], ["běh zastavil strop kroků"])
        self.assertEqual(by_run[2]["unit_ids"], [])
        self.assertEqual(data["pokus"]["bez_odpovedi"],
                         [{"run": 2, "otazka": "Q1", "varianta": "A",
                           "duvod": "běh zastavil strop kroků"},
                          {"run": 3, "otazka": "Q1", "varianta": "B",
                           "duvod": "běh skončil bez závěrečné zprávy"}])
        self.assertEqual([c["status"] for c in data["results"]["claims"]],
                         [card_check.CLAIM_ABSENT, card_check.CLAIM_NO_ANSWER,
                          card_check.CLAIM_NO_ANSWER])
        # Nothing was asked of the judge for the runs without an answer.
        self.assertEqual({u["run"] for u in data["units"]}, {1})
        self.assertIn("2 bez odpovědi", out)
        self.assertIn(card_check.FACT_NO_ANSWER, out)

    def test_only_the_first_three_stored_runs_of_a_variant_count(self):
        runs = [self.answered(n, "A", SILENT_WITH_TYPE) for n in (1, 2, 3, 4)]
        directory = self.attempt(runs)
        code, _, err = self.run_main(self.command(directory), ScriptedJudge(ANSWER_RULES))
        self.assertEqual(code, 0, err)
        self.assertEqual(sorted(r["run"] for r in self.facts(self.load(), "f1")), [1, 2, 3])

    def test_spoiled_runs_are_not_judged(self):
        directory = self.attempt([(1, "A", "spoiled", "zbytek", None),
                                  self.answered(2, "A", SILENT_WITH_TYPE)])
        code, _, err = self.run_main(self.command(directory), ScriptedJudge(ANSWER_RULES))
        self.assertEqual(code, 0, err)
        self.assertEqual([r["run"] for r in self.facts(self.load(), "f1")], [2])


class AnswerAttemptStateTest(AnswerCheckCase):
    def test_an_interrupted_attempt_is_code_2_without_the_switch(self):
        directory = self.attempt([self.answered(1, "A", SILENT_WITH_TYPE)], status="interrupted")
        runner = ScriptedJudge(ANSWER_RULES)
        code, _, err = self.run_main(self.command(directory), runner)
        self.assertEqual(code, 2)
        self.assertIn("interrupted", err)
        self.assertIn("--i-nedokonceny", err)
        self.assertEqual((runner.calls, runner.smoke_calls), ([], 0))
        self.assertFalse(self.measurement_file().exists())

    def test_the_switch_takes_it_and_the_measurement_says_so(self):
        for status in ("interrupted", "infrastructure_error"):
            with self.subTest(status=status):
                directory = self.attempt([self.answered(1, "A", SILENT_WITH_TYPE)], status=status)
                code, out, err = self.run_main(self.command(directory, "--i-nedokonceny"),
                                               ScriptedJudge(ANSWER_RULES))
                self.assertEqual(code, 0, err)
                data = self.load()
                self.assertEqual((data["pokus"]["nedokonceny"], data["pokus"]["stav"]),
                                 (True, status))
                self.assertIn(status, out)

    def test_a_completed_attempt_is_not_marked(self):
        directory = self.attempt([self.answered(1, "A", SILENT_WITH_TYPE)])
        self.assertEqual(self.run_main(self.command(directory), ScriptedJudge(ANSWER_RULES))[0], 0)
        data = self.load()
        self.assertFalse(data["pokus"]["nedokonceny"])
        self.assertEqual(data["pokus"]["rodina_agenta"], "claude")
        self.assertEqual(data["agent_family"], "claude")


class AnswerShapeTest(AnswerCheckCase):
    def test_the_measurement_has_the_shape_of_the_card_check(self):
        directory = self.attempt([self.answered(1, "A", SILENT_WITH_TYPE),
                                  self.answered(2, "B", SILENT_WITHOUT_TYPE)])
        runner = ScriptedJudge(ANSWER_RULES)
        code, _, err = self.run_main(self.command(directory), runner)
        self.assertEqual(code, 0, err)
        data = self.load()
        self.assertEqual(next(iter(data)), "format_version")
        self.assertEqual(data["status"], card_check.STATUS_DONE)
        for key in ("id", "corpus", "items", "judge", "reported_model", "agent_family",
                    "prompt_version", "units", "results"):
            self.assertIn(key, data)
        self.assertEqual(data["items"], [{"id": "Q1", "commit": REV,
                                          "otisk_potvrzeni": self.question.otisk_potvrzeni,
                                          "kontrolni": False, "dotaz": self.question.dotaz}])
        self.assertEqual(data["corpus"]["source"], "odpověď agenta")
        self.assertEqual([v["korpus"]["corpus_fingerprint"] for v in data["corpus"]["varianty"]],
                         ["c" * 8, "c" * 8])
        self.assertNotIn("corpus_dir", json.dumps(data))
        self.assertEqual(data["pokus"]["korpusy_variant"], [])  # the test attempt has no corpus
        self.assertNotIn("cli-ktery-se-nespusti", json.dumps(data))
        for unit in data["units"]:
            self.assertEqual(unit["document_path"], "odpověď agenta")
            self.assertIn(unit["variant"], ("A", "B"))
            self.assertEqual(unit["document"], f"odpoved:{unit['run']:03d}")
        self.assertEqual([r["status"] for r in data["results"]["facts"]],
                         [card_check.FACT_PRESENT, card_check.FACT_DISPUTED])
        # the review of the owner reads the measurement as it reads a card check
        loaded = review.load_measurement(self.measurement_file())
        self.assertEqual(len(loaded.units), len(data["units"]))

    def test_the_card_check_json_is_unchanged_by_the_new_fields(self):
        self.assertEqual(self.run_main(self.args(), ScriptedJudge(ANSWER_RULES))[0], 0)
        data = self.measurement()
        for unit in data["units"]:
            self.assertNotIn("variant", unit)
            self.assertNotIn("run", unit)
        for result in (*data["results"]["facts"], *data["results"]["claims"]):
            self.assertNotIn("variant", result)
            self.assertNotIn("run", result)

    def test_the_measurement_does_not_overwrite_the_card_check_of_the_state_directory(self):
        self.assertEqual(self.run_main(self.args(), ScriptedJudge(ANSWER_RULES))[0], 0)
        card_id = self.measurement()["id"]
        directory = self.attempt([self.answered(1, "A", SILENT_WITH_TYPE)])
        self.assertEqual(self.run_main(self.command(directory),
                                       ScriptedJudge(ANSWER_RULES))[0], 0)
        self.assertEqual(self.measurement()["id"], card_id)
        self.assertNotEqual(self.load()["id"], card_id)


class AnswerFailureTest(AnswerCheckCase):
    def test_a_failing_judge_is_code_3_and_the_next_run_continues_from_the_cache(self):
        directory = self.attempt([self.answered(1, "A", SILENT_WITH_TYPE),
                                  self.answered(2, "B", SILENT_WITHOUT_TYPE)], role="vyvojar")
        first = ScriptedJudge(ANSWER_RULES, fail_after=4)
        code, _, err = self.run_main(self.command(directory), first)
        self.assertEqual(code, 3)
        self.assertIn("nedokončené", err)
        data = self.load()
        self.assertEqual((data["status"], data["results"]), ("nedokonceno", None))
        self.assertEqual(len(first.calls), 4)
        second = ScriptedJudge(ANSWER_RULES)
        code, _, err = self.run_main(self.command(directory), second)
        self.assertEqual(code, 0, err)
        self.assertEqual(self.load()["status"], "hotovo")
        reference = ScriptedJudge(ANSWER_RULES)
        self.run_main(self.command(directory, **{"--stav": str(self.tmp / "reference")}),
                      reference)
        self.assertEqual(len(second.calls), len(reference.calls) - 4)

    def test_no_judge_available_is_code_3(self):
        directory = self.attempt([self.answered(1, "A", SILENT_WITH_TYPE)])

        def failing(spec, prompt, siblings):
            raise JudgeInfrastructureError("scénář: smoke test nejde")

        code, _, err = self.run_main(self.command(directory), failing)
        self.assertEqual(code, 3)
        self.assertIn("Žádný hodnotitel", err)

    def test_interrupt_is_130_and_leaves_the_measurement_incomplete(self):
        directory = self.attempt([self.answered(1, "A", SILENT_WITH_TYPE)])
        runner = ScriptedJudge(ANSWER_RULES, fail_after=1, fail=KeyboardInterrupt())
        code, _, err = self.run_main(self.command(directory), runner)
        self.assertEqual(code, 130)
        self.assertEqual(self.load()["status"], "nedokonceno")


class AnswerInputTest(AnswerCheckCase):
    def refused(self, directory, needle, **replace):
        runner = ScriptedJudge(ANSWER_RULES)
        code, _, err = self.run_main(self.command(directory, **replace), runner)
        self.assertEqual(code, 2, err)
        self.assertTrue(err.startswith("Chyba vstupu"))
        self.assertIn(needle, err)
        self.assertEqual((runner.calls, runner.smoke_calls), ([], 0))
        self.assertFalse(self.measurement_file().exists())

    def test_not_an_experiment_directory(self):
        self.refused(self.tmp / "nic", "report.json")
        broken = self.tmp / "rozbity"
        broken.mkdir()
        (broken / "report.json").write_text("{}", encoding="utf-8")
        self.refused(broken, "není zpráva pokusu")

    def test_an_unknown_adapter_cannot_give_the_agent_family(self):
        directory = self.attempt([self.answered(1, "A", SILENT_WITH_TYPE)], adapter="scripted")
        self.refused(directory, "rodina agenta")

    def test_a_question_missing_from_the_catalog(self):
        directory = self.attempt([self.answered(1, "A", SILENT_WITH_TYPE)])
        other = question_json([fact("f1", "x", [place(EVMN, "EditorViewModelNew.RunNew()")])],
                              qid="Q9")
        path = self.write_json("other.json", catalog_json(other))
        self.refused(directory, "Q1", **{"--katalog": str(path)})

    def test_a_catalog_over_another_revision(self):
        directory = self.attempt([self.answered(1, "A", SILENT_WITH_TYPE)], revision=OTHER_REV)
        self.refused(directory, "commitem")

    def test_a_catalog_whose_question_text_changed_since_the_experiment(self):
        directory = self.attempt([self.answered(1, "A", SILENT_WITH_TYPE)])
        changed = answer_question()
        changed["dotaz"] = "Úplně jiná otázka?"
        path = self.write_json("changed.json", catalog_json(changed))
        self.refused(directory, "neodpovídá zadání", **{"--katalog": str(path)})

    def test_an_attempt_without_a_question_or_a_stored_run(self):
        directory = self.attempt([])
        self.refused(directory, "žádný uložený běh otázky")

    def test_the_state_directory_inside_a_repository_is_refused(self):
        directory = self.attempt([self.answered(1, "A", SILENT_WITH_TYPE)])
        self.refused(directory, "Stavový adresář", **{"--stav": str(REPO / "tmp-stav")})
        self.assertFalse((REPO / "tmp-stav").exists())

    def test_the_arguments_are_required(self):
        directory = self.attempt([self.answered(1, "A", SILENT_WITH_TYPE)])
        for name in ("--katalog", "--hodnotitele", "--stav", "--source-repo"):
            with self.subTest(name):
                argv = self.command(directory)
                at = argv.index(name)
                code, _, err = self.run_main(argv[:at] + argv[at + 2:], ScriptedJudge())
                self.assertEqual(code, 2)
                self.assertIn(name, err)
        self.assertEqual(self.run_main(["hodnot-odpovedi", "--source-repo", str(REPO)])[0], 2)
        self.assertEqual(self.run_main(["hodnot-odpovedi", "--help"])[0], 0)


# --- paths (KTD13) ------------------------------------------------------------------------------

CALIBRATION_EXAMPLE = Path(__file__).resolve().parent.parent / "examples" / "calibration.example.json"


class ProtectedPathsCase(AnswersCase):
    """Three protected roots: a source repository, a tool repository and a worktree of the tool.

    The tool repository is the one `prepare.protected_roots` takes for this runner's own."""

    def setUp(self):
        super().setUp()
        self.source = self.make_repo("source-repo")
        self.tool = self.make_repo("tool-repo")
        self.worktree = self.tmp / "tool-worktree"
        git(self.tool, "worktree", "add", "-q", "--detach", str(self.worktree), "HEAD")
        patcher = mock.patch.object(prepare, "_find_runner_repo", return_value=str(self.tool))
        patcher.start()
        self.addCleanup(patcher.stop)
        self.roots = (("zdrojové repo", self.source), ("repo nástroje", self.tool),
                      ("worktree repa nástroje", self.worktree))

    def make_repo(self, name):
        repo = self.tmp / name
        repo.mkdir()
        git(repo, "init", "-q", "-b", "main")
        write(repo, "README.md", "Ukázkové repo.\n")
        commit_all(repo, "init")
        return repo


def flat(prefix, options):
    out = list(prefix)
    for key, value in options.items():
        out += [key, str(value)]
    return out


class ProtectedPathsTest(ProtectedPathsCase):
    """Every path KTD13 protects, under every protected root: exit 2, the path named, nothing done."""

    def command_lines(self):
        """subcommand -> (words before the options, options in order, the protected ones)."""
        common = {"--source-repo": self.source}
        nothing = self.tmp / "neexistuje.json"
        return {
            "kontrola-karet": (
                ["kontrola-karet"],
                {**common, "--agent-family": "claude", "--katalog": self.catalog,
                 "--korpus": self.corpus, "--hodnotitele": self.judges, "--stav": self.state},
                ("--katalog", "--hodnotitele", "--stav")),
            "hodnot-odpovedi": (
                ["hodnot-odpovedi", str(self.tmp / "neexistuje")],
                {**common, "--katalog": self.catalog, "--hodnotitele": self.judges,
                 "--stav": self.state},
                ("--katalog", "--hodnotitele", "--stav")),
            "kontrola (sestavení)": (
                ["kontrola"],
                {**common, "--stav": self.state, "--mereni": nothing,
                 "--vystup": self.tmp / "kontrola.json"},
                ("--stav", "--vystup", "--mereni")),
            "kontrola (načtení)": (
                ["kontrola"], {**common, "--stav": self.state, "--nacti": nothing},
                ("--stav", "--nacti")),
            "zprava": (
                ["zprava"],
                {**common, "--mereni": nothing, "--vystup": self.tmp / "zprava",
                 "--stav": self.state, "--kalibrace": self.tmp / "kalibrace",
                 "--sada-kalibrace": self.tmp / "sada.json"},
                ("--vystup", "--stav", "--kalibrace", "--sada-kalibrace", "--mereni")),
            "kalibrace": (
                ["kalibrace"],
                {**common, "--agent-family": "claude", "--sada": self.tmp / "sada.json",
                 "--hodnotitele": self.judges, "--stav": self.state,
                 "--vystup": self.tmp / "kalibrace.json"},
                ("--sada", "--hodnotitele", "--stav", "--vystup")),
        }

    def test_every_protected_path_under_every_protected_root_is_refused(self):
        checked = 0
        for command, (prefix, options, protected) in self.command_lines().items():
            for option in protected:
                for what, root in self.roots:
                    with self.subTest(command=command, option=option, root=what):
                        target = root / "uvnitr" / f"{option.lstrip('-')}.json"
                        runner = ScriptedJudge(RULES)
                        code, _, err = self.run_main(
                            flat(prefix, {**options, option: target}), runner)
                        self.assertEqual(code, 2, err)
                        self.assertTrue(err.startswith("Chyba vstupu"), err)
                        self.assertIn(str(target), err)
                        self.assertIn("leží uvnitř", err)
                        self.assertEqual((runner.calls, runner.smoke_calls), ([], 0))
                        self.assertFalse((root / "uvnitr").exists())
                        checked += 1
        self.assertEqual(checked, 3 * (3 + 3 + 3 + 2 + 5 + 4))

    def test_a_link_into_a_protected_root_is_refused_like_the_root(self):
        from tests.test_prepare import _make_dir_link, _remove_dir_link
        link = self.tmp / "odkaz"
        if not _make_dir_link(link, self.source):
            self.skipTest("cannot create a directory symlink or junction here")
        self.addCleanup(_remove_dir_link, link)
        code, _, err = self.run_main(self.args(**{"--source-repo": str(self.source),
                                                  "--stav": str(link / "stav")}),
                                     ScriptedJudge(RULES))
        self.assertEqual(code, 2)
        self.assertIn("Stavový adresář", err)
        self.assertFalse((self.source / "stav").exists())

    def test_paths_outside_every_protected_root_are_accepted(self):
        code, _, err = self.run_main(self.args(**{"--source-repo": str(self.source)}),
                                     ScriptedJudge(RULES))
        self.assertEqual(code, 0, err)


class StateAgainstCorpusTest(AnswersCase):
    """The state directory holds the judge's records; an agent reads corpora, so they stay apart."""

    def test_the_state_directory_inside_the_measured_corpus_is_refused_before_any_judge(self):
        runner = ScriptedJudge(RULES)
        inside = self.corpus / "stav"
        code, _, err = self.run_main(self.args(**{"--stav": str(inside)}), runner)
        self.assertEqual(code, 2)
        self.assertIn("překrývá", err)
        self.assertIn(str(inside), err)
        self.assertEqual((runner.calls, runner.smoke_calls), ([], 0))
        self.assertFalse(inside.exists())

    def test_the_corpus_inside_the_state_directory_is_refused_too(self):
        runner = ScriptedJudge(RULES)
        code, _, err = self.run_main(self.args(**{"--stav": str(self.corpus.parent)}), runner)
        self.assertEqual(code, 2)
        self.assertIn("překrývá", err)
        self.assertEqual((runner.calls, runner.smoke_calls), ([], 0))

    def test_the_same_directory_is_refused(self):
        code, _, err = self.run_main(self.args(**{"--stav": str(self.corpus)}),
                                     ScriptedJudge(RULES))
        self.assertEqual(code, 2)
        self.assertIn("překrývá", err)


class AnswerStateAgainstCorpusTest(AnswerCheckCase):
    def with_corpus_dir(self, directory, corpus):
        path = directory / "report.json"
        data = json.loads(path.read_text(encoding="utf-8"))
        data["experiment"]["variants"][1]["corpus_dir"] = str(corpus)
        path.write_text(json.dumps(data), encoding="utf-8")

    def test_the_state_directory_inside_the_corpus_of_any_variant_is_refused(self):
        directory = self.attempt([self.answered(1, "A", SILENT_WITH_TYPE)])
        corpus = self.tmp / "korpus-varianty-b"
        self.with_corpus_dir(directory, corpus)
        runner = ScriptedJudge(ANSWER_RULES)
        inside = corpus / "stav"
        code, _, err = self.run_main(self.command(directory, **{"--stav": str(inside)}), runner)
        self.assertEqual(code, 2)
        self.assertIn("překrývá", err)
        self.assertIn("korpus varianty B", err)
        self.assertEqual((runner.calls, runner.smoke_calls), ([], 0))
        self.assertFalse(inside.exists())

    def test_a_state_directory_beside_the_corpus_is_accepted(self):
        directory = self.attempt([self.answered(1, "A", SILENT_WITH_TYPE)])
        self.with_corpus_dir(directory, self.tmp / "korpus-varianty-b")
        code, _, err = self.run_main(self.command(directory), ScriptedJudge(ANSWER_RULES))
        self.assertEqual(code, 0, err)


class MeasurementInputsTest(ProtectedPathsCase):
    """The inputs of a measurement are protected like its outputs: nothing is read from a repository."""

    def test_the_measurements_of_the_review_and_of_the_report_are_refused_inside_a_root(self):
        for what, root in self.roots:
            inside = root / "uvnitr" / "measurement.json"
            lines = {
                "kontrola": flat(["kontrola"], {
                    "--source-repo": self.source, "--stav": self.state, "--mereni": inside,
                    "--vystup": self.tmp / "kontrola.json"}),
                "zprava": flat(["zprava"], {
                    "--source-repo": self.source, "--mereni": inside,
                    "--vystup": self.tmp / "zprava", "--stav": self.state,
                    "--kalibrace": self.tmp / "kalibrace"})}
            for command, argv in lines.items():
                with self.subTest(command=command, root=what):
                    code, _, err = self.run_main(argv)
                    self.assertEqual(code, 2, err)
                    self.assertIn(str(inside), err)
                    self.assertIn("leží uvnitř", err)
                    self.assertFalse((root / "uvnitr").exists())

    def test_a_measurement_named_inside_a_review_file_is_refused(self):
        for number, (what, root) in enumerate(self.roots):
            inside = root / "uvnitr" / "measurement.json"
            listed = self.tmp / f"kontrola-{number}.json"
            listed.write_text(json.dumps({"format_version": 1, "radky": [], "mereni": [
                {"id": "m", "cesta": str(inside)}]}), encoding="utf-8")
            with self.subTest(root=what):
                code, _, err = self.run_main(flat(["kontrola"], {
                    "--source-repo": self.source, "--stav": self.state, "--nacti": listed}))
                self.assertEqual(code, 2, err)
                self.assertIn(str(inside), err)
                self.assertIn("leží uvnitř", err)
                self.assertFalse(review.decisions_path(self.state).exists())

    def test_the_attempt_directory_is_refused_inside_a_root(self):
        for what, root in self.roots:
            inside = root / "uvnitr" / "pokus"
            with self.subTest(root=what):
                runner = ScriptedJudge(RULES)
                code, _, err = self.run_main(flat(["hodnot-odpovedi", str(inside)], {
                    "--source-repo": self.source, "--katalog": self.catalog,
                    "--hodnotitele": self.judges, "--stav": self.state}), runner)
                self.assertEqual(code, 2, err)
                self.assertIn("Adresář pokusu", err)
                self.assertIn(str(inside), err)
                self.assertEqual((runner.calls, runner.smoke_calls), ([], 0))


class ReviewFilesAgainstCorpusTest(AnswersCase):
    """The owner's files stay out of the corpus an agent reads (the card check's corpus)."""

    def setUp(self):
        super().setUp()
        self.assertEqual(self.run_main(self.args(), ScriptedJudge(RULES))[0], 0)
        self.measurement_path = card_check.measurement_path(self.state)

    def review_line(self, *extra):
        return ["kontrola", "--source-repo", str(REPO), "--stav", str(self.state), *extra]

    def test_the_review_files_inside_the_measured_corpus_are_refused(self):
        inside = self.corpus / "kontrola.json"
        code, _, err = self.run_main(self.review_line(
            "--mereni", str(self.measurement_path), "--vystup", str(inside)))
        self.assertEqual(code, 2)
        self.assertIn("překrývá", err)
        self.assertIn("korpus měření", err)
        self.assertEqual((inside.exists(), inside.with_suffix(".md").exists()), (False, False))
        self.assertFalse(review.sampled_path(self.state).exists())
        beside = self.tmp / "kontrola.json"
        self.assertEqual(self.run_main(self.review_line(
            "--mereni", str(self.measurement_path), "--vystup", str(beside)))[0], 0)

    def test_a_filled_review_file_inside_the_corpus_is_not_loaded(self):
        beside = self.tmp / "kontrola.json"
        self.assertEqual(self.run_main(self.review_line(
            "--mereni", str(self.measurement_path), "--vystup", str(beside)))[0], 0)
        data = json.loads(beside.read_text(encoding="utf-8"))
        for row in data["radky"]:
            row["rozhodnuti"] = "vyjadruje" if row["druh"] == "fakt" else "neobsahuje"
        inside = self.corpus / "vyplnena.json"
        inside.write_text(json.dumps(data, ensure_ascii=False), encoding="utf-8")
        code, _, err = self.run_main(self.review_line("--nacti", str(inside)))
        self.assertEqual(code, 2)
        self.assertIn("překrývá", err)
        self.assertFalse(review.decisions_path(self.state).exists())
        beside.write_text(json.dumps(data, ensure_ascii=False), encoding="utf-8")
        self.assertEqual(self.run_main(self.review_line("--nacti", str(beside)))[0], 0)

    def test_the_report_files_and_the_calibration_set_inside_the_corpus_are_refused(self):
        values = {"--vystup": self.tmp / "zprava", "--kalibrace": self.tmp / "kalibrace"}
        for option in ("--vystup", "--kalibrace", "--sada-kalibrace"):
            inside = self.corpus / "uvnitr" / option.lstrip("-")
            options = {"--source-repo": REPO, "--mereni": self.measurement_path,
                       "--stav": self.state, **values}
            options[option] = inside
            with self.subTest(option=option):
                code, _, err = self.run_main(flat(["zprava"], options))
                self.assertEqual(code, 2, err)
                self.assertIn("překrývá", err)
                self.assertIn(str(inside), err)
                self.assertFalse(inside.exists())


class ReviewFilesAgainstVariantCorpusTest(AnswerCheckCase):
    """The same for the answer check: the corpus of any variant of the attempt."""

    def test_the_review_files_inside_the_corpus_of_a_variant_are_refused(self):
        directory = self.attempt([self.answered(1, "A", SILENT_WITH_TYPE)])
        corpus = self.tmp / "korpus-varianty-b"
        AnswerStateAgainstCorpusTest.with_corpus_dir(self, directory, corpus)
        self.assertEqual(self.run_main(self.command(directory), ScriptedJudge(ANSWER_RULES))[0], 0)
        data = self.load()
        self.assertEqual(data["pokus"]["korpusy_variant"], [{"nazev": "B", "adresar": str(corpus)}])
        self.assertNotIn(str(corpus), json.dumps(data["corpus"]))  # no path in what the id hashes
        inside = corpus / "kontrola.json"
        code, _, err = self.run_main(["kontrola", "--source-repo", str(REPO), "--stav",
                                      str(self.state), "--mereni", str(self.measurement_file()),
                                      "--vystup", str(inside)])
        self.assertEqual(code, 2)
        self.assertIn("korpus varianty B", err)
        self.assertFalse(inside.exists())
        beside = self.tmp / "kontrola.json"
        self.assertEqual(self.run_main(["kontrola", "--source-repo", str(REPO), "--stav",
                                        str(self.state), "--mereni", str(self.measurement_file()),
                                        "--vystup", str(beside)])[0], 0)


# --- kalibrace ----------------------------------------------------------------------------------

class CalibrationCommandTest(ProtectedPathsCase):
    """`answers.py kalibrace` over the example set with a scripted judge."""

    def setUp(self):
        super().setUp()
        self.sada = self.tmp / "sada.json"
        self.sada.write_text(CALIBRATION_EXAMPLE.read_text(encoding="utf-8"), encoding="utf-8")
        self.items = json.loads(self.sada.read_text(encoding="utf-8"))["polozky"]
        self.output = self.tmp / "vysledky" / "kalibrace.json"

    def judge(self, model="model-1"):
        """A judge that answers every sentence as the owner expects; the smoke test passes."""
        sentences = SentenceRunner(expected_rule(self.items), model=model)

        def runner(spec, prompt, siblings):
            if prompt.startswith("KONTROLA HODNOTITELE"):
                text = json.dumps({"stav": "OK", "obsah": ""})
                return CliReply(text=text, model=model, raw=text)
            return sentences(spec, prompt, siblings)

        runner.sentences = sentences
        return runner

    def line(self, **replace):
        values = {"--source-repo": self.source, "--agent-family": "claude", "--sada": self.sada,
                  "--hodnotitele": self.judges, "--stav": self.state, "--vystup": self.output}
        values.update(replace)
        return flat(["kalibrace"], values)

    def test_it_writes_the_result_of_the_example_set(self):
        code, out, err = self.run_main(self.line(), self.judge())
        self.assertEqual(code, 0, err)
        data = calibration.load_result(self.output)
        self.assertEqual((data["hodnotitel"]["id"], data["jednotek"], data["shoda"]),
                         ("j1", len(self.items), 1.0))
        self.assertEqual(data["otisk_sady"], calibration.load_set(self.sada).fingerprint)
        self.assertNotIn("cli-ktery-se-nespusti", self.output.read_text(encoding="utf-8"))
        self.assertIn("shoda 100%", out)
        self.assertIn(str(self.output), out)
        self.assertTrue((self.state / card_check.JUDGE_CACHE_DIR).is_dir())
        # the report finds it in the directory
        found = calibration.find_calibration(self.output.parent, "j1", data["prompt_version"],
                                             data["otisk_sady"])
        self.assertIsNotNone(found)

    def test_a_second_run_asks_the_judge_nothing_new(self):
        self.assertEqual(self.run_main(self.line(), self.judge())[0], 0)
        second = self.judge()
        self.assertEqual(self.run_main(self.line(), second)[0], 0)
        self.assertEqual(second.sentences.prompts, [])

    def test_an_output_inside_a_repository_is_refused_before_any_judge_call(self):
        for what, root in self.roots:
            with self.subTest(root=what):
                runner = self.judge()
                target = root / "kalibrace.json"
                code, _, err = self.run_main(self.line(**{"--vystup": target}), runner)
                self.assertEqual(code, 2)
                self.assertIn(str(target), err)
                self.assertFalse(target.exists())
                self.assertEqual(runner.sentences.prompts, [])
        self.assertFalse((self.state / card_check.JUDGE_CACHE_DIR).exists())

    def test_the_output_must_be_named_and_be_a_file(self):
        self.assertEqual(self.run_main(self.line(**{"--vystup": ""}), self.judge())[0], 2)
        self.output.mkdir(parents=True)
        code, _, err = self.run_main(self.line(), self.judge())
        self.assertEqual(code, 2)
        self.assertIn("adresář", err)

    def test_the_other_arguments_are_required(self):
        for name in ("--agent-family", "--sada", "--hodnotitele", "--stav", "--vystup",
                     "--source-repo"):
            with self.subTest(name):
                argv = self.line()
                at = argv.index(name)
                code, _, err = self.run_main(argv[:at] + argv[at + 2:], self.judge())
                self.assertEqual(code, 2)
                self.assertIn(name, err)
        self.assertEqual(self.run_main(["kalibrace", "--help"])[0], 0)

    def test_a_broken_set_is_code_2(self):
        self.sada.write_text(json.dumps({"format_version": 1, "polozky": []}), encoding="utf-8")
        runner = self.judge()
        code, _, err = self.run_main(self.line(), runner)
        self.assertEqual(code, 2)
        self.assertIn("neprázdný seznam", err)
        self.assertEqual(runner.sentences.prompts, [])
        self.assertFalse(self.output.exists())

    def test_no_judge_available_is_code_3(self):
        def failing(spec, prompt, siblings):
            raise JudgeInfrastructureError("scénář: smoke test nejde")

        code, _, err = self.run_main(self.line(), failing)
        self.assertEqual(code, 3)
        self.assertIn("Žádný hodnotitel", err)
        self.assertFalse(self.output.exists())

    def test_a_judge_that_fails_midway_is_code_3_and_writes_nothing(self):
        inner = self.judge()
        calls = []

        def failing(spec, prompt, siblings):
            if not prompt.startswith("KONTROLA HODNOTITELE"):
                calls.append(prompt)
                if len(calls) == 2:
                    raise JudgeInfrastructureError("scénář: hodnotitel spadl")
            return inner(spec, prompt, siblings)

        code, _, err = self.run_main(self.line(), failing)
        self.assertEqual(code, 3)
        self.assertIn("nedokončená", err)
        self.assertFalse(self.output.exists())

    def test_interrupt_is_130(self):
        def interrupted(spec, prompt, siblings):
            raise KeyboardInterrupt

        code, _, err = self.run_main(self.line(), interrupted)
        self.assertEqual(code, 130)
        self.assertFalse(self.output.exists())


if __name__ == "__main__":
    unittest.main()
