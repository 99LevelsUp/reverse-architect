"""Schedule, replacements, paths, tool check, cleanup and dry run of the runner (U5).

Most experiments here use the scripted adapter (`agents/scripted.py`), which
replays Claude Code stream-json through the real parser; the cleanup tests
and the tool-check stop use the real adapter with `fixtures/fake_agent.py`.
"""
import io
import json
import os
import subprocess
import sys
import unittest
from collections import Counter
from contextlib import redirect_stderr, redirect_stdout
from datetime import datetime
from pathlib import Path
from unittest import mock

import prepare
import run
from agents import claude_code
from agents.base import (AgentEvent, Cap, CapHit, RunRecord, TokenUsage)
from agents.claude_code import (ClaudeCodeAdapter, ClaudeCodeSettings,
                                claude_infrastructure_problem)
from agents.scripted import ScriptedRun
from catalog import confirmation_fingerprint
from run import (ConfigError, ExperimentStopped, VariantSpec, build_schedule, run_experiment)
from tests.experiment_fixture import (FAKE_AGENT, FREE_ANSWER, QUESTION_ID, QUESTION_TEXT,
                                      Experiment, answer_run, assistant, auth_error_run,
                                      correct_answer, fake_agent_env, free_answer_run, init,
                                      is_preflight, preflight_run, result)
from tests.test_prepare import _make_dir_link, _remove_dir_link, commit_all, git, write

CLOCK = datetime(2026, 10, 2, 12, 0, 0)


def fixed_now():
    return CLOCK


class ExperimentTestCase(unittest.TestCase):
    cases = 2

    def setUp(self):
        self.world = Experiment(cases=self.cases)
        self.addCleanup(self.world.cleanup)

    def run_scripted(self, script, config=None, **kwargs):
        adapter = self.world.scripted(script)
        outcome = run_experiment(config or self.world.config(),
                                 adapter_factory=lambda c, p: adapter, now=fixed_now, **kwargs)
        return adapter, outcome

    def honest(self, request):
        """Tool check passes; every run answers its case correctly, a question in free text."""
        if is_preflight(request):
            return preflight_run(request)
        if self.world.is_question(request):
            return free_answer_run()
        return answer_run(*correct_answer(self.world.case_of(request)))

    def experiment_dir(self):
        dirs = list(self.world.output.iterdir())
        self.assertEqual(len(dirs), 1)
        return dirs[0]


class ScheduleTest(ExperimentTestCase):
    def test_two_cases_give_twelve_runs_same_seed_same_order(self):
        first = build_schedule(["a", "b"], ["bez karet", "s kartami"], 7)
        self.assertEqual(len(first), 12)
        self.assertEqual(Counter((e.case_id, e.variant) for e in first),
                         {(c, v): 3 for c in "ab" for v in ("bez karet", "s kartami")})
        self.assertEqual(first, build_schedule(["a", "b"], ["bez karet", "s kartami"], 7))
        others = [build_schedule(["a", "b"], ["bez karet", "s kartami"], s) for s in range(8, 12)]
        self.assertTrue(any(o != first for o in others))

    def test_runs_follow_the_seeded_order_one_at_a_time(self):
        adapter, outcome = self.run_scripted(self.honest)
        self.assertEqual(adapter.max_active, 1)
        schedule = json.loads((outcome.directory / "schedule.json").read_text(encoding="utf-8"))
        self.assertEqual(schedule["seed"], 7)
        self.assertEqual(schedule["concurrency"], 1)
        planned = [(p["case_id"], p["variant"]) for p in schedule["planned"]]
        expected = [(e.case_id, e.variant) for e in
                    build_schedule(["ukazka-a", "ukazka-b"], ["bez karet", "s kartami"], 7)]
        self.assertEqual(planned, expected)
        executed = [(e["case_id"], e["variant"]) for e in schedule["executed"]]
        self.assertEqual(executed, expected)
        self.assertEqual(len(adapter.requests), 2 + 12)  # tool check per variant, then runs
        self.assertTrue(all(is_preflight(r) for r in adapter.requests[:2]))
        self.assertEqual(outcome.report["summary"]["all_cases"],
                         {"win": 0, "loss": 0, "tie": 2, "incomplete": 0})

    def test_experiment_directory_holds_every_artifact(self):
        _, outcome = self.run_scripted(self.honest)
        d = outcome.directory
        for name in ("schedule.json", "preflight.json", "preparation.json", "report.md",
                     "report.json"):
            with self.subTest(name=name):
                self.assertTrue((d / name).is_file())
        self.assertEqual(len(list((d / "runs").glob("*.jsonl"))), 12)
        self.assertEqual(len(list((d / "runs").glob("*.result.json"))), 12)
        self.assertEqual(len(list((d / "prompts").glob("*.txt"))), 4)
        self.assertTrue(json.loads((d / "preflight.json").read_text(encoding="utf-8"))["ok"])
        self.assertFalse(self.world.output.joinpath("..", "iso", "agent-config").exists())

    def test_prompts_differ_only_in_the_sources_paragraph(self):
        _, outcome = self.run_scripted(self.honest)
        prompts = outcome.directory / "prompts"
        without = (prompts / "ukazka-a--bez-karet.txt").read_text(encoding="utf-8")
        with_cards = (prompts / "ukazka-a--s-kartami.txt").read_text(encoding="utf-8")

        def split(text):
            head, rest = text.split("Dostupné zdroje:\n", 1)
            sources, tail = rest.split("\n\n", 1)
            return head, sources, tail

        a, b = split(without), split(with_cards)
        self.assertEqual((a[0], a[2]), (b[0], b[2]))
        self.assertNotEqual(a[1], b[1])
        self.assertIn("Zákazník A hlásí", a[0])
        # The runner hands on the real path (macOS temp dirs sit behind a symlink).
        corpus = os.path.realpath(self.world.root / "corpus")
        self.assertIn(corpus.lower(), b[1].lower())

    def test_preparation_costs_are_recorded_apart_from_runs(self):
        _, outcome = self.run_scripted(self.honest)
        steps = [p["step"] for p in outcome.report["preparation"]]
        for expected in ("Export stromu základní revize", "Kontrola případů",
                         "Korpus varianty s kartami: generování",
                         "Kontrola nástroje (bez karet)", "Kontrola nástroje (s kartami)"):
            self.assertIn(expected, steps)


class MixedCatalogTest(ExperimentTestCase):
    """Questions are part of the experiment: scheduled, prompted and stored like cases, not scored."""

    def write_mixed(self, keep_bugs=True):
        w = self.world
        data = json.loads(w.catalog.read_text(encoding="utf-8"))
        bugs = [dict(c, druh="chyba") for c in data["cases"]] if keep_bugs else []
        facts = [{"id": "f1", "text": "Ukázkový fakt.", "role": "misto",
                  "mista": [{"soubor": "Src/Placeholder/LoadAction.cs",
                             "clen": "LoadAction.Reload", "radky": 4}]}]
        question = {"druh": "otazka", "id": "ukazka-otazka", "dotaz": "Ukázková otázka?",
                    "ramcova_odpoved": "Ukázková odpověď.", "commit": w.base,
                    "fakta_potvrzena": True, "pseudonymizace_potvrzena": True,
                    "kontrolni": False, "fakta": facts, "zakazana_tvrzeni": [],
                    "otisk_potvrzeni": confirmation_fingerprint(facts, [])}
        data.update(format_version=2, cases=[*bugs[:1], question, *bugs[1:]])
        w.catalog.write_text(json.dumps(data, ensure_ascii=False), encoding="utf-8")
        w.question_prompts = {QUESTION_ID: QUESTION_TEXT}

    def test_schedule_includes_questions_like_cases(self):
        self.write_mixed()
        catalog = run._load_catalog(self.world.config())
        self.assertEqual([c.druh for c in catalog.items], ["chyba", "chyba", "otazka"])
        self.assertEqual([c.id for c in run.measured_cases(catalog, "vyvojar")],
                         ["ukazka-a", "ukazka-b", "ukazka-otazka"])
        self.assertEqual([c.id for c in run.measured_cases(catalog, "podpora")],
                         ["ukazka-otazka"])
        planned = build_schedule(catalog.items, ["bez karet", "s kartami"], 7)
        self.assertEqual(Counter((e.case_id, e.variant) for e in planned),
                         {(c, v): 3 for c in ("ukazka-a", "ukazka-b", "ukazka-otazka")
                          for v in ("bez karet", "s kartami")})
        self.assertEqual(planned, build_schedule(["ukazka-a", "ukazka-b", "ukazka-otazka"],
                                                 ["bez karet", "s kartami"], 7))

    def test_experiment_over_a_mixed_catalog_runs_bugs_and_questions(self):
        self.write_mixed()
        adapter, outcome = self.run_scripted(self.honest)
        self.assertEqual(len(adapter.requests), 2 + 18)
        report = json.loads((outcome.directory / "report.json").read_text(encoding="utf-8"))
        # The navigation tallies still read only the bugs, with the same numbers as without a question.
        self.assertEqual([c["case_id"] for c in report["cases"]], ["ukazka-a", "ukazka-b"])
        self.assertEqual([c["id"] for c in report["experiment"]["cases"]],
                         ["ukazka-a", "ukazka-b"])
        self.assertEqual(report["summary"]["all_cases"],
                         {"win": 0, "loss": 0, "tie": 2, "incomplete": 0})
        self.assertEqual(len(list((outcome.directory / "prompts").glob("*.txt"))), 6)
        question = report["questions"][0]
        self.assertEqual(question["question_id"], "ukazka-otazka")
        self.assertEqual([(v["variant"], v["stored_runs"], v["answered"], v["unanswered"])
                          for v in question["variants"]],
                         [("bez karet", 3, 3, 0), ("s kartami", 3, 3, 0)])
        markdown = (outcome.directory / "report.md").read_text(encoding="utf-8")
        self.assertIn("## Otázky (volná odpověď)", markdown)
        self.assertNotIn("Případ ukazka-otazka", markdown)

    def test_question_with_a_missing_place_stops_the_preparation(self):
        self.write_mixed()
        data = json.loads(self.world.catalog.read_text(encoding="utf-8"))
        question = next(c for c in data["cases"] if c["druh"] == "otazka")
        question["fakta"][0]["mista"][0]["soubor"] = "Src/Placeholder/Missing.cs"
        question["otisk_potvrzeni"] = confirmation_fingerprint(question["fakta"], [])
        self.world.catalog.write_text(json.dumps(data, ensure_ascii=False), encoding="utf-8")
        with self.assertRaises(ExperimentStopped) as ctx:
            self.run_scripted(self.honest)
        self.assertEqual(ctx.exception.exit_code, 2)
        self.assertIn("ukazka-otazka", str(ctx.exception))

    def test_catalog_with_only_questions_is_measured_in_the_developer_role_too(self):
        self.write_mixed(keep_bugs=False)
        catalog = run._load_catalog(self.world.config())
        self.assertEqual(catalog.cases, ())
        adapter, outcome = self.run_scripted(self.honest)
        self.assertEqual(len(adapter.requests), 2 + 6)
        self.assertEqual(outcome.report["cases"], [])
        self.assertEqual(outcome.report["questions"][0]["variants"][0]["answered"], 3)


class SupportRoleTest(ExperimentTestCase):
    """The role of the reader (U12, KTD9): `podpora` has no sources, only a corpus.

    `chyba` items are set aside in the support role and the questions are its only
    measured items; the tests of what a measured *navigation* run looks like force
    the runs of the bugs with `measurable`.
    """

    cases = 1

    def setUp(self):
        super().setUp()
        self.world.add_questions()

    def support_config(self, **overrides):
        return self.world.config(variants=self.world.support_variants(), role="podpora",
                                 **overrides)

    def measurable(self):
        """Pretend a later unit made the bugs measurable in the support role."""
        return mock.patch.object(run, "measured_cases", lambda catalog, role: catalog.cases)

    def argv(self, *extra, role="podpora"):
        w = self.world
        argv = ["--catalog", str(w.catalog), "--source-repo", str(w.repo),
                "--output-root", str(w.output), "--export-dir", str(w.export),
                "--isolation-root", str(w.isolation), "--ra-binary", "ra", "--model", "m",
                "--cap-seconds", "600", "--cap-steps", "50", "--cap-tokens", "100000",
                "--seed", "7", *extra]
        return argv if role is None else [*argv, "--role", role]

    def variants_file(self, variants):
        path = self.world.root / "variants.json"
        path.write_text(json.dumps({"format_version": 1, "variants": variants}),
                        encoding="utf-8")
        return str(path)

    def main(self, argv):
        err = io.StringIO()
        with mock.patch.object(run, "run_experiment") as started, redirect_stderr(err):
            code = run.main(argv)
        return code, err.getvalue(), started

    # -- the work directory ------------------------------------------------------

    def test_agent_works_in_an_empty_directory_and_reads_the_corpus_through_add_dir(self):
        w = self.world
        sources_read = []

        def script(request):
            if is_preflight(request):
                return preflight_run(request)
            if not sources_read:  # the first measured run strays into the sources
                sources_read.append(str(w.export / "Src" / "Placeholder" / "LoadAction.cs"))
                return answer_run(*correct_answer("ukazka-a"), reads=sources_read)
            return answer_run(*correct_answer("ukazka-a"),
                              reads=[os.path.join(os.path.realpath(request.corpus_dir),
                                                  "index.md")])

        with self.measurable():
            adapter, outcome = self.run_scripted(script, self.support_config())
        runs = [r for r in adapter.requests if not is_preflight(r)]
        self.assertEqual(len(runs), 6 + 1)  # one spoiled attempt is replaced
        workdirs = {os.path.realpath(r.workdir) for r in runs}
        self.assertEqual(workdirs, {os.path.realpath(w.support_workdir)})
        self.assertNotEqual(workdirs, {os.path.realpath(w.export)})
        self.assertEqual(list(w.support_workdir.iterdir()), [])
        self.assertTrue(any(w.export.rglob("*.cs")))  # the sources exist, elsewhere
        self.assertEqual({r.corpus_dir.name for r in runs}, {"corpus-a", "corpus-b"})
        results = [json.loads(p.read_text(encoding="utf-8"))
                   for p in sorted((outcome.directory / "runs").glob("*.result.json"))]
        spoiled = [r for r in results if r["state"] == "spoiled"]
        self.assertEqual(len(spoiled), 1)
        self.assertTrue(spoiled[0]["spoil_reasons"][0].startswith("path_outside"))
        self.assertIn("LoadAction.cs", spoiled[0]["spoil_reasons"][0])
        self.assertEqual([r["state"] for r in results if r is not spoiled[0]],
                         ["success"] * 6)

    def test_developer_role_still_works_in_the_exported_tree(self):
        adapter, outcome = self.run_scripted(self.honest, self.world.config(role="vyvojar"))
        runs = [r for r in adapter.requests if not is_preflight(r)]
        self.assertEqual({os.path.realpath(r.workdir) for r in runs},
                         {os.path.realpath(self.world.export)})
        self.assertFalse(self.world.support_workdir.exists())
        self.assertEqual(outcome.report["experiment"]["role"], "vyvojar")
        self.assertEqual(outcome.report["experiment"]["not_measured"], [])

    def test_tool_check_creates_no_work_directory_with_sources(self):
        adapter, outcome = self.run_scripted(self.honest, self.support_config())
        checks = [r for r in adapter.requests if is_preflight(r)]
        self.assertEqual(len(checks), 2)
        self.assertTrue(all(is_preflight(r) for r in adapter.requests[:2]))
        for request in checks:
            with self.subTest(variant=request.workdir.parent.name):
                self.assertEqual(request.workdir.name, "empty-work")
                self.assertEqual(list(request.workdir.iterdir()), [])
                self.assertFalse((request.workdir.parent / "work").exists())
                self.assertIn("index.md", request.prompt)
        self.assertTrue(json.loads((outcome.directory / "preflight.json")
                                   .read_text(encoding="utf-8"))["ok"])

    # -- what is measured ------------------------------------------------------------

    def test_mixed_catalog_sets_the_bugs_aside_and_runs_the_questions(self):
        adapter, outcome = self.run_scripted(self.honest, self.support_config())
        self.assertEqual(outcome.status, "completed")
        self.assertEqual(len(adapter.requests), 2 + 6)  # the tool check, then the question
        report = json.loads((outcome.directory / "report.json").read_text(encoding="utf-8"))
        experiment = report["experiment"]
        self.assertEqual(experiment["role"], "podpora")
        self.assertEqual(experiment["not_measured"],
                         [{"id": "ukazka-a", "reason": "v roli podpora se neměří"}])
        self.assertEqual((report["cases"], experiment["cases"]), ([], []))
        self.assertEqual([q["question_id"] for q in report["questions"]], [QUESTION_ID])
        results = [json.loads(p.read_text(encoding="utf-8"))
                   for p in sorted((outcome.directory / "runs").glob("*.result.json"))]
        self.assertEqual([(r["case_id"], r["state"], r["answer_text"]) for r in results],
                         [(QUESTION_ID, "answered", FREE_ANSWER)] * 6)
        self.assertEqual(len(list((outcome.directory / "prompts").iterdir())), 2)
        schedule = json.loads((outcome.directory / "schedule.json").read_text(encoding="utf-8"))
        self.assertEqual({p["case_id"] for p in schedule["planned"]}, {QUESTION_ID})
        self.assertEqual(len(schedule["planned"]), 6)
        markdown = (outcome.directory / "report.md").read_text(encoding="utf-8")
        self.assertIn("ukazka-a: v roli podpora se neměří", markdown)
        self.assertIn("- Role: podpora", markdown)

    def test_catalog_with_only_bugs_is_refused_in_the_support_role(self):
        data = json.loads(self.world.catalog.read_text(encoding="utf-8"))
        data.update(format_version=2, cases=[c for c in data["cases"] if c["druh"] == "chyba"])
        self.world.catalog.write_text(json.dumps(data, ensure_ascii=False), encoding="utf-8")
        with self.assertRaises(ConfigError) as ctx:
            run_experiment(self.support_config(),
                           adapter_factory=lambda c, p: self.fail("agent nesmí startovat"))
        self.assertIn("neměří", str(ctx.exception))
        self.assertFalse(self.world.output.exists())
        code, err, _ = self.main(self.argv(
            "--variants", self.variants_file([{"name": "A", "corpus_dir": "corpus-a"},
                                               {"name": "B", "corpus_dir": "corpus-b"}]),
            "--dry-run"))
        self.assertEqual(code, 2)
        self.assertIn("neměří", err)

    def test_variant_without_a_corpus_is_refused_in_the_support_role(self):
        variants = (VariantSpec("bez karet"), self.world.corpus_variant("s kartami", "corpus"))
        with self.assertRaises(ConfigError) as ctx:
            run_experiment(self.world.config(variants=variants, role="podpora"),
                           adapter_factory=lambda c, p: self.fail("agent nesmí startovat"))
        self.assertIn("bez karet", str(ctx.exception))
        self.assertIn("nemá korpus", str(ctx.exception))
        self.assertFalse(self.world.output.exists())
        code, err, started = self.main(self.argv(
            "--variants", self.variants_file([{"name": "bez karet"},
                                               {"name": "s kartami", "corpus_dir": "corpus"}])))
        self.assertEqual(code, 2)
        self.assertIn("nemá korpus", err)
        started.assert_not_called()

    def test_default_variants_end_with_code_2_and_the_advice(self):
        code, err, started = self.main(self.argv("--corpus-dir", str(self.world.root / "corpus")))
        self.assertEqual(code, 2)
        self.assertIn("--variants", err)
        self.assertIn("dva korpusy", err)
        started.assert_not_called()
        self.assertFalse(self.world.output.exists())

    def test_dry_run_in_the_support_role_names_the_set_aside_bugs(self):
        out = io.StringIO()
        argv = self.argv("--variants", self.variants_file(
            [{"name": "A", "corpus_dir": "corpus-a"}, {"name": "B", "corpus_dir": "corpus-b"}]),
            "--dry-run")
        with mock.patch.object(run, "run_experiment") as started, redirect_stdout(out):
            code = run.main(argv)
        self.assertEqual(code, 0)
        started.assert_not_called()
        self.assertIn("ukazka-a", out.getvalue())
        self.assertIn("v roli podpora se neměří", out.getvalue())
        self.assertIn("Otázky v katalogu: 1", out.getvalue())
        self.assertIn("Běhů v rozvrhu: 6", out.getvalue())
        self.assertFalse(self.world.output.exists())
        self.assertFalse(self.world.support_workdir.exists())

    # -- the role itself ---------------------------------------------------------------

    def test_role_has_no_silent_default(self):
        for argv in (self.argv(role=None), self.argv(role="ctenar")):
            with self.subTest(argv=argv[-2:]), redirect_stderr(io.StringIO()) as err:
                with self.assertRaises(SystemExit) as ctx:
                    run.main(argv)
                self.assertEqual(ctx.exception.code, 2)
                self.assertIn("--role", err.getvalue())
        with self.assertRaises(TypeError):  # nor does the configuration
            run.ExperimentConfig(**{k: None for k in ("catalog_path", "source_repo",
                                                      "output_root", "export_dir",
                                                      "isolation_root", "ra_command", "model",
                                                      "cap", "seed", "variants")})

    def test_unknown_role_in_the_configuration_is_refused(self):
        with self.assertRaises(ConfigError):
            run_experiment(self.world.config(role="ctenar"),
                           adapter_factory=lambda c, p: self.fail("agent nesmí startovat"))

    def test_prompts_of_the_two_variants_differ_only_in_the_sources_paragraph(self):
        _, outcome = self.run_scripted(self.honest, self.support_config())
        meta = outcome.report["experiment"]["variants"]

        def split(text):
            head, rest = text.split("Dostupné zdroje:\n", 1)
            sources, tail = rest.split("\n\n", 1)
            return head, sources, tail

        a, b = (split(v["prompt_template"]) for v in meta)
        self.assertEqual((a[0], a[2]), (b[0], b[2]))
        corpus_a, corpus_b = (os.path.realpath(self.world.root / n)
                              for n in ("corpus-a", "corpus-b"))
        self.assertIn(corpus_a.lower(), a[1].lower())
        self.assertIn(corpus_b.lower(), b[1].lower())
        for sources in (a[1], b[1]):
            self.assertIn("Zdrojáky systému nemáš", sources)
            self.assertNotIn("jsou v pracovním adresáři", sources)
        self.assertEqual(a[1].replace(corpus_a, "<korpus>"), b[1].replace(corpus_b, "<korpus>"))
        self.assertIn("relativně ke kořeni zdrojáků systému", a[2] + a[0])

    # -- the location checks -------------------------------------------------------------

    def test_the_support_work_directory_gets_the_same_overlap_checks_as_the_export(self):
        w = self.world
        for what, overrides in (
                ("kořen izolace", {"isolation_root": w.support_workdir}),
                ("kořen výstupu", {"output_root": w.support_workdir / "out"})):
            with self.subTest(what=what):
                with self.assertRaises(ConfigError) as ctx:
                    run.resolve_paths(self.support_config(**overrides))
                self.assertIn("pracovní adresář role podpora", str(ctx.exception))
        w.support_workdir.mkdir()
        (w.support_workdir / "zbytek.txt").write_text("x", encoding="utf-8")
        with self.assertRaises(ConfigError) as ctx:
            run.resolve_paths(self.support_config())
        self.assertIn("už existuje", str(ctx.exception))


class QuestionRunTest(ExperimentTestCase):
    """A run of a question (KTD10): a free answer, stored whole and not scored."""

    cases = 1

    def setUp(self):
        super().setUp()
        self.world.add_questions()

    def question_config(self, **overrides):
        return self.world.config(**overrides)

    def results(self, outcome, case_id=QUESTION_ID):
        return [r for r in (json.loads(p.read_text(encoding="utf-8"))
                            for p in sorted((outcome.directory / "runs").glob("*.result.json")))
                if r["case_id"] == case_id]

    def play(self, question_run):
        def play(request):
            if is_preflight(request):
                return preflight_run(request)
            if self.world.is_question(request):
                return question_run(request)
            return answer_run(*correct_answer("ukazka-a"))
        return play

    def test_the_answer_is_stored_whole_and_the_run_is_not_invalid(self):
        text = ("Dlouhá odpověď.\n\n```json\n{\"file\": \"a.cs\"}\n```\n\n"
                "Vychází z `Src/Placeholder/LoadAction.cs:4`.")
        _, outcome = self.run_scripted(self.play(lambda request: free_answer_run(text)))
        runs = self.results(outcome)
        self.assertEqual(len(runs), 6)
        for result in runs:
            with self.subTest(order=result["order"]):
                self.assertEqual((result["state"], result["reason"]), ("answered", None))
                self.assertEqual(result["answer_text"], text)
                self.assertIsNone(result["answer"])
        bug = self.results(outcome, "ukazka-a")[0]
        self.assertEqual(bug["state"], "success")
        self.assertIsNone(bug["answer_text"])

    def test_a_run_stopped_by_a_cap_is_stored_without_an_answer(self):
        def too_long(request):
            return free_answer_run(reads=["a.txt", "b.txt", "c.txt", "d.txt", "e.txt"])

        _, outcome = self.run_scripted(
            self.play(too_long), self.question_config(
                cap=Cap(seconds=600, steps=3, tokens=100_000)))
        runs = self.results(outcome)
        self.assertEqual(len(runs), 6)
        for result in runs:
            self.assertEqual((result["state"], result["reason"], result["answer_text"]),
                             ("unanswered", "cap_steps", None))
            self.assertIsNotNone(result["cap_hit"])
        question = outcome.report["questions"][0]
        self.assertEqual([(v["answered"], v["unanswered"], v["unanswered_reasons"])
                          for v in question["variants"]], [(0, 3, ["cap_steps"] * 3)] * 2)
        markdown = (outcome.directory / "report.md").read_text(encoding="utf-8")
        self.assertIn("3 (strop kroků, strop kroků, strop kroků)", markdown)

    def test_an_empty_final_message_is_without_an_answer(self):
        def empty(request):
            return ScriptedRun((init(["Glob", "Grep", "Read"]),
                                assistant("final", [{"type": "text", "text": ""}]), result()))

        _, outcome = self.run_scripted(self.play(empty))
        self.assertEqual({(r["state"], r["reason"], r["answer_text"])
                          for r in self.results(outcome)}, {("unanswered", "no_answer", None)})

    def test_a_spoiled_question_run_is_replaced_like_a_case(self):
        w = self.world
        calls = Counter()

        def stray(request):
            calls[request.corpus_dir is not None] += 1
            if calls[request.corpus_dir is not None] == 1:
                return free_answer_run(reads=[str(w.root / "catalog" / "catalog.json")])
            return free_answer_run()

        _, outcome = self.run_scripted(self.play(stray))
        runs = self.results(outcome)
        self.assertEqual(len(runs), 6 + 2)
        spoiled = [r for r in runs if r["state"] == "spoiled"]
        self.assertEqual(len(spoiled), 2)
        self.assertTrue(all(r["answer_text"] is None for r in spoiled))
        self.assertEqual([r["replaces"] for r in runs if r["replaces"] is not None],
                         [r["order"] for r in spoiled])

    def test_prompts_of_the_two_variants_differ_only_in_the_sources_paragraph(self):
        _, outcome = self.run_scripted(self.honest)
        prompts = outcome.directory / "prompts"
        without = (prompts / f"{QUESTION_ID}--bez-karet.txt").read_text(encoding="utf-8")
        with_cards = (prompts / f"{QUESTION_ID}--s-kartami.txt").read_text(encoding="utf-8")

        def split(text):
            head, rest = text.split("Dostupné zdroje:\n", 1)
            sources, tail = rest.split("\n\n", 1)
            return head, sources, tail

        a, b = split(without), split(with_cards)
        self.assertEqual((a[0], a[2]), (b[0], b[2]))
        self.assertNotEqual(a[1], b[1])
        self.assertIn(QUESTION_TEXT, a[0])
        self.assertIn("`cesta:N` nebo `cesta:N-M`", a[2])
        self.assertIn("relativně k pracovnímu adresáři", a[2])
        self.assertIn("JSON nepiš", a[2])
        self.assertNotIn("```json", without)
        corpus = os.path.realpath(self.world.root / "corpus")
        self.assertIn(corpus.lower(), b[1].lower())
        # The case prompts are the same texts as before a question was in the catalog.
        case_prompt = (prompts / "ukazka-a--bez-karet.txt").read_text(encoding="utf-8")
        self.assertIn("```json", case_prompt)
        self.assertNotEqual(case_prompt.split("Zadání případu:")[0], a[0])

    def test_the_report_records_both_prompt_templates_and_every_prompt_digest(self):
        _, outcome = self.run_scripted(self.honest)
        for variant in outcome.report["experiment"]["variants"]:
            self.assertIn("Zadání případu:", variant["prompt_template"])
            self.assertIn("Otázka:", variant["question_prompt_template"])
            self.assertEqual(sorted(variant["prompt_sha256"]), ["ukazka-a", QUESTION_ID])


class PrepareOnlyTest(ExperimentTestCase):
    cases = 1

    def test_prepare_only_writes_a_receipted_corpus_and_runs_nothing(self):
        outcome = run_experiment(self.world.config(), prepare_only=True,
                                 adapter_factory=lambda c, p: self.fail("agent nesmí startovat"),
                                 now=fixed_now)
        self.assertEqual(outcome.status, "prepared")
        self.assertTrue(prepare.receipt_path(self.world.root / "corpus").is_file())
        preparation = json.loads((outcome.directory / "preparation.json")
                                 .read_text(encoding="utf-8"))
        self.assertIsNone(preparation["error"])
        self.assertEqual(list(preparation["corpora"]), ["s kartami"])
        self.assertEqual(sorted(p.name for p in outcome.directory.iterdir()),
                         ["preparation.json"])

    def test_next_run_reuses_the_corpus_by_its_receipt(self):
        run_experiment(self.world.config(), prepare_only=True, now=fixed_now)
        receipt = prepare.receipt_path(self.world.root / "corpus").read_text(encoding="utf-8")
        adapter = self.world.scripted(lambda r: preflight_run(r) if is_preflight(r)
                                      else answer_run(*correct_answer("ukazka-a")))
        later = datetime(2026, 10, 2, 13, 0, 0)
        outcome = run_experiment(self.world.config(export_dir=self.world.root / "export-2"),
                                 adapter_factory=lambda c, p: adapter, now=lambda: later)
        self.assertEqual(outcome.status, "completed")
        self.assertEqual(prepare.receipt_path(self.world.root / "corpus")
                         .read_text(encoding="utf-8"), receipt)
        corpus_cost = next(p for p in outcome.report["preparation"]
                           if p["step"].startswith("Korpus"))
        self.assertEqual(corpus_cost["note"], "korpus už existoval, čas generování z účtenky")


class ReplacementTest(ExperimentTestCase):
    cases = 1

    def script(self, plan):
        """`plan[(variant, attempt)]` = "spoil" or "answer" (default answer)."""
        attempts = Counter()

        def play(request):
            if is_preflight(request):
                return preflight_run(request)
            variant = "s kartami" if request.corpus_dir is not None else "bez karet"
            attempts[variant] += 1
            if plan.get((variant, attempts[variant])) == "spoil":
                outside = str(self.world.root / "catalog" / "catalog.json")
                return answer_run(*correct_answer("ukazka-a"), reads=[outside])
            return answer_run(*correct_answer("ukazka-a"))
        return play

    def results(self, outcome):
        return [json.loads(p.read_text(encoding="utf-8"))
                for p in sorted((outcome.directory / "runs").glob("*.result.json"))]

    def test_spoiled_run_is_replaced_at_the_end_of_the_queue(self):
        _, outcome = self.run_scripted(self.script({("bez karet", 1): "spoil"}))
        results = self.results(outcome)
        self.assertEqual(len(results), 7)
        spoiled = [r for r in results if r["state"] == "spoiled"]
        self.assertEqual(len(spoiled), 1)
        self.assertTrue(spoiled[0]["spoil_reasons"][0].startswith("path_outside"))
        self.assertEqual(results[-1]["replaces"], spoiled[0]["order"])
        self.assertEqual(results[-1]["variant"], "bez karet")
        case = outcome.report["cases"][0]
        self.assertEqual(case["result"], "tie")
        self.assertEqual(case["variants"][0]["clean_runs"], 3)
        self.assertEqual(case["variants"][0]["spoiled_attempts"], 1)

    def test_after_two_spoiled_replacements_the_case_is_incomplete(self):
        plan = {("s kartami", 1): "spoil", ("s kartami", 4): "spoil", ("s kartami", 5): "spoil"}
        _, outcome = self.run_scripted(self.script(plan))
        results = self.results(outcome)
        with_cards = [r for r in results if r["variant"] == "s kartami"]
        self.assertEqual([r["state"] for r in with_cards],
                         ["spoiled", "success", "success", "spoiled", "spoiled"])
        self.assertEqual(len(results), 3 + 5)
        case = outcome.report["cases"][0]
        self.assertEqual(case["result"], "incomplete")
        self.assertEqual(case["variants"][1]["clean_runs"], 2)
        self.assertEqual(outcome.report["summary"]["all_cases"]["incomplete"], 1)


class InfrastructureStopTest(ExperimentTestCase):
    cases = 1

    def test_login_failure_during_runs_stops_and_is_not_counted(self):
        calls = Counter()

        def play(request):
            if is_preflight(request):
                return preflight_run(request)
            calls["run"] += 1
            if calls["run"] == 3:
                return auth_error_run()
            return answer_run(*correct_answer("ukazka-a"))

        with self.assertRaises(ExperimentStopped) as ctx:
            self.run_scripted(play)
        self.assertEqual(ctx.exception.exit_code, 3)
        self.assertIn("chybou infrastruktury", str(ctx.exception))
        self.assertIn("/login", str(ctx.exception))
        directory = ctx.exception.directory
        data = json.loads((directory / "report.json").read_text(encoding="utf-8"))
        self.assertEqual(data["experiment"]["status"], "infrastructure_error")
        self.assertEqual([e["order"] for e in data["infrastructure_errors"]], [3])
        self.assertEqual(len(list((directory / "runs").glob("*.result.json"))), 3)
        clean = sum(v["clean_runs"] for c in data["cases"] for v in c["variants"])
        self.assertEqual(clean, 2)
        self.assertIn("přerušen chybou infrastruktury",
                      (directory / "report.md").read_text(encoding="utf-8"))

    def test_scripted_tool_check_that_leaks_stops_before_any_run(self):
        def play(request):
            if is_preflight(request):
                return preflight_run(request, leak=True)
            raise AssertionError("po neúspěšné kontrole nástroje nesmí běžet žádný běh")

        with self.assertRaises(ExperimentStopped) as ctx:
            self.run_scripted(play)
        self.assertEqual(ctx.exception.exit_code, 3)
        self.assertFalse((ctx.exception.directory / "runs").exists())


class InfrastructureClassificationTest(unittest.TestCase):
    def record(self, *, events=("system", "assistant", "result"), result_error=None,
               result_text=None, stderr="", final=None, cap_hit=None, api_error=None,
               steps=0):
        return RunRecord(adapter="claude-code", workdir="w", corpus_dir=None, config_dir="c",
                         events=tuple(AgentEvent(role=r) for r in events),
                         tokens=TokenUsage(), steps=steps, wall_seconds=1.0, cap=Cap(60, 10, 100),
                         cap_hit=cap_hit, exit_status=1 if result_error else 0,
                         final_message=final, result_error=result_error,
                         result_text=result_text, stderr_tail=stderr, api_error=api_error)

    def test_external_causes_are_infrastructure(self):
        for kwargs in ({"result_error": "success",
                        "result_text": "Invalid API key · Please run /login"},
                       {"result_error": "success", "result_text": "OAuth token has expired"},
                       {"result_error": "error_during_execution", "stderr": "API Error: 529 "
                        "Overloaded"},
                       {"result_error": "success", "result_text": "Rate limit reached"},
                       {"result_error": "success", "api_error": "rate_limit"},
                       {"events": ()},
                       {"events": ("system", "assistant")}):
            with self.subTest(kwargs=kwargs):
                self.assertIsNotNone(claude_infrastructure_problem(self.record(**kwargs)))

    def test_agent_causes_are_not_infrastructure(self):
        cap = CapHit("steps", 10, 11, 1)
        for kwargs in ({},
                       {"final": "Příčina je v ošetření rate limit na serveru."},
                       {"result_error": "error_during_execution", "result_text": "Tool failed"},
                       {"events": ("system", "assistant"), "cap_hit": cap},
                       {"api_error": "rate_limit"}):
            with self.subTest(kwargs=kwargs):
                self.assertIsNone(claude_infrastructure_problem(self.record(**kwargs)))

    def test_time_cap_without_any_progress_is_infrastructure(self):
        time_cap = CapHit("time", 60, 61, 1)
        for events in ((), ("system",)):
            with self.subTest(events=events):
                self.assertIsNotNone(claude_infrastructure_problem(
                    self.record(events=events, cap_hit=time_cap)))

    def test_cap_with_progress_or_other_cap_stays_the_agents_failure(self):
        time_cap = CapHit("time", 60, 61, 1)
        for kwargs in ({"events": ("system", "assistant"), "cap_hit": time_cap},
                       {"events": ("system",), "cap_hit": time_cap, "steps": 2},
                       {"events": ("system",), "cap_hit": CapHit("steps", 10, 11, 1)},
                       {"events": ("system",), "cap_hit": CapHit("tokens", 10, 11, 1)}):
            with self.subTest(kwargs=kwargs):
                self.assertIsNone(claude_infrastructure_problem(self.record(**kwargs)))


class PathsTest(ExperimentTestCase):
    cases = 1

    def assert_refused(self, **overrides):
        with self.assertRaises(ConfigError):
            run_experiment(self.world.config(**overrides),
                           adapter_factory=lambda c, p: self.fail("agent nesmí startovat"))

    def test_output_root_inside_the_repository_is_refused(self):
        runner_repo = Path(run.__file__).resolve().parents[2]
        target = runner_repo / "se-test-output-should-not-exist"
        self.assert_refused(output_root=target)
        self.assertFalse(target.exists())
        self.assert_refused(output_root=self.world.repo / "out")
        self.assertFalse((self.world.repo / "out").exists())

    def test_output_root_through_a_link_into_the_repository_is_refused(self):
        runner_repo = Path(run.__file__).resolve().parents[2]
        link = self.world.root / "link-to-repo"
        if not _make_dir_link(link, runner_repo):
            self.skipTest("cannot create a directory symlink or junction here")
        self.addCleanup(_remove_dir_link, link)
        self.assert_refused(output_root=link / "se-test-output-should-not-exist")
        self.assertFalse((runner_repo / "se-test-output-should-not-exist").exists())

    def test_output_root_or_catalog_inside_the_work_tree_is_refused(self):
        self.assert_refused(output_root=self.world.export / "out")
        self.assert_refused(export_dir=self.world.catalog.parent)
        corpus_holding_catalog = (VariantSpec("bez karet"),
                                  self.world.corpus_variant("s kartami", "catalog"))
        self.assert_refused(variants=corpus_holding_catalog)
        self.assertFalse(self.world.output.exists())

    def test_leftover_configuration_directory_stops_the_start(self):
        leftover = self.world.isolation / "agent-config"
        leftover.mkdir(parents=True)
        self.assert_refused()
        self.assertFalse(self.world.output.exists())
        self.assertFalse(self.world.export.exists())


class ProtectedRootsTest(ExperimentTestCase):
    """KTD13: the catalog and every other path of the run are refused inside a protected root.

    The roots are the source repository, the tool repository and a worktree of the tool."""

    cases = 1

    def setUp(self):
        super().setUp()
        self.tool = self.world.root / "tool-repo"
        self.tool.mkdir()
        git(self.tool, "init", "-q", "-b", "main")
        write(self.tool, "README.md", "Ukázkové repo.\n")
        commit_all(self.tool, "init")
        self.worktree = self.world.root / "tool-worktree"
        git(self.tool, "worktree", "add", "-q", "--detach", str(self.worktree), "HEAD")
        patcher = mock.patch.object(prepare, "_find_runner_repo", return_value=str(self.tool))
        patcher.start()
        self.addCleanup(patcher.stop)
        self.roots = (("zdrojové repo", self.world.repo), ("repo nástroje", self.tool),
                      ("worktree repa nástroje", self.worktree))

    def overrides(self, what, target):
        if what == "catalog_path":
            return {"catalog_path": target}
        if what == "corpus":
            return {"variants": (VariantSpec("bez karet"),
                                 VariantSpec("s kartami", corpus_dir=target))}
        return {what: target}

    def test_every_path_of_the_run_under_every_protected_root_is_refused(self):
        refused = 0
        for what in ("catalog_path", "output_root", "export_dir", "isolation_root", "corpus"):
            for label, root in self.roots:
                with self.subTest(path=what, root=label):
                    target = root / "uvnitr" / "cesta.json"
                    config = self.world.config(**self.overrides(what, target))
                    with self.assertRaises(ConfigError) as dry:
                        run.dry_run(config)
                    self.assertIn(str(target), str(dry.exception))
                    with self.assertRaises(ConfigError) as full:
                        run_experiment(config, adapter_factory=lambda c, p: self.fail("agent"))
                    self.assertIn(str(target), str(full.exception))
                    self.assertFalse((root / "uvnitr").exists())
                    refused += 1
        self.assertEqual(refused, 15)
        self.assertFalse(self.world.output.exists())

    def test_the_catalog_is_refused_before_it_is_read(self):
        # the file does not exist: the location is what the error names, not a missing file
        for label, root in self.roots:
            with self.subTest(root=label):
                target = root / "katalog-ktery-neexistuje.json"
                with self.assertRaises(ConfigError) as ctx:
                    run.dry_run(self.world.config(catalog_path=target))
                self.assertIn("Katalog", str(ctx.exception))
                self.assertIn("leží uvnitř", str(ctx.exception))

    def test_a_catalog_inside_a_protected_root_is_exit_code_2_on_the_command_line(self):
        w = self.world
        for label, root in self.roots:
            with self.subTest(root=label):
                catalog = root / "katalog.json"
                argv = ["--catalog", str(catalog), "--source-repo", str(w.repo),
                        "--output-root", str(w.output), "--export-dir", str(w.export),
                        "--isolation-root", str(w.isolation), "--ra-binary", "ra", "--model", "m",
                        "--cap-seconds", "600", "--cap-steps", "50", "--cap-tokens", "100000",
                        "--seed", "7", "--role", "vyvojar", "--corpus-dir",
                        str(w.root / "corpus"), "--dry-run"]
                err = io.StringIO()
                with mock.patch.object(run, "run_experiment") as started, redirect_stderr(err):
                    code = run.main(argv)
                self.assertEqual(code, 2)
                started.assert_not_called()
                self.assertIn(str(catalog), err.getvalue())

    def test_a_catalog_outside_every_root_is_accepted(self):
        self.assertIn("Běhů v rozvrhu", run.dry_run(self.world.config()))


class DryRunTest(ExperimentTestCase):
    def test_dry_run_prints_counts_and_cap_without_starting_the_agent(self):
        w = self.world
        argv = ["--catalog", str(w.catalog), "--source-repo", str(w.repo),
                "--output-root", str(w.output), "--export-dir", str(w.export),
                "--isolation-root", str(w.isolation), "--ra-binary", "ra", "--model", "m",
                "--cap-seconds", "600", "--cap-steps", "50", "--cap-tokens", "100000",
                "--seed", "7", "--role", "vyvojar", "--corpus-dir", str(w.root / "corpus"),
                "--dry-run"]
        out = io.StringIO()
        with mock.patch.object(run, "run_experiment") as started, \
                mock.patch.object(run, "default_adapter") as adapter, redirect_stdout(out):
            code = run.main(argv)
        self.assertEqual(code, 0)
        started.assert_not_called()
        adapter.assert_not_called()
        text = out.getvalue()
        self.assertIn("Běhů v rozvrhu: 12", text)
        self.assertIn("nejvýš 20", text)
        self.assertIn("čas 600 s, kroky 50, tokeny 100000", text)
        self.assertFalse(w.output.exists())
        self.assertFalse(w.export.exists())

    def test_forbid_dir_refuses_a_corpus_under_that_name(self):
        w = self.world
        argv = ["--catalog", str(w.catalog), "--source-repo", str(w.repo),
                "--output-root", str(w.output), "--export-dir", str(w.export),
                "--isolation-root", str(w.isolation), "--ra-binary", "ra", "--model", "m",
                "--cap-seconds", "600", "--cap-steps", "50", "--cap-tokens", "100000",
                "--seed", "7", "--role", "vyvojar", "--corpus-dir", str(w.root / "ws" / "corpus"),
                "--forbid-dir", "other", "--forbid-dir", "WS", "--dry-run"]
        err = io.StringIO()
        with mock.patch.object(run, "run_experiment") as started, redirect_stderr(err):
            code = run.main(argv)
        self.assertEqual(code, 2)
        started.assert_not_called()
        self.assertIn("leží v adresáři WS", err.getvalue())


class CleanupTest(ExperimentTestCase):
    """The isolated configuration directory is gone after a cap, an exception, an interrupt."""

    cases = 1

    def adapter_factory(self, *mode):
        def build(config, paths):
            settings = ClaudeCodeSettings(
                model=config.model, isolation_root=paths.isolation_root,
                executable=(sys.executable, FAKE_AGENT, *mode),
                credentials_source=self.world.credentials,
                keep_apart=(paths.output_root, self.world.catalog.parent))
            return ClaudeCodeAdapter(settings, parent_env=fake_agent_env())
        return build

    def run_real(self, *mode, cap=Cap(seconds=60, steps=50, tokens=10**9)):
        return run_experiment(self.world.config(cap=cap),
                              adapter_factory=self.adapter_factory(*mode), now=fixed_now)

    def config_dir(self):
        return self.world.isolation / "agent-config"

    def test_cap_stopped_runs_leave_no_configuration_directory(self):
        outcome = self.run_real("auto", "spew", "200", cap=Cap(seconds=60, steps=5,
                                                                 tokens=10**9))
        self.assertFalse(self.config_dir().exists())
        reasons = [r for c in outcome.report["cases"] for v in c["variants"]
                   for r in v["failure_reasons"]]
        self.assertEqual(reasons, ["cap_steps"] * 6)
        self.assertEqual(outcome.report["cases"][0]["result"], "tie")

    def interrupting(self, error):
        real = claude_code.run_watched

        def fake(argv, **kwargs):
            if "KONTROLA NÁSTROJE" in kwargs["stdin_text"]:
                return real(argv, **kwargs)
            self.assertTrue(Path(kwargs["env"]["CLAUDE_CONFIG_DIR"]).is_dir())
            raise error
        return mock.patch.object(claude_code, "run_watched", fake)

    def test_exception_in_a_run_leaves_no_configuration_directory(self):
        with self.interrupting(RuntimeError("simulovaná chyba")):
            with self.assertRaises(RuntimeError):
                self.run_real("auto", "spew", "1")
        self.assertFalse(self.config_dir().exists())

    def test_interrupt_leaves_no_configuration_directory_and_a_partial_report(self):
        with self.interrupting(KeyboardInterrupt()):
            with self.assertRaises(KeyboardInterrupt):
                self.run_real("auto", "spew", "1")
        self.assertFalse(self.config_dir().exists())
        report = json.loads((self.experiment_dir() / "report.json").read_text(encoding="utf-8"))
        self.assertEqual(report["experiment"]["status"], "interrupted")

    def assert_partial_report_after(self, error: BaseException) -> None:
        with self.interrupting(error):
            with self.assertRaises(type(error)):
                self.run_real("auto", "spew", "1")
        self.assertFalse(self.config_dir().exists())
        directory = self.experiment_dir()
        report = json.loads((directory / "report.json").read_text(encoding="utf-8"))
        self.assertEqual(report["experiment"]["status"], "infrastructure_error")
        self.assertIn(type(error).__name__, report["experiment"]["status_detail"])
        self.assertTrue((directory / "report.md").is_file())

    def test_unexpected_exception_leaves_a_partial_report_and_propagates(self):
        self.assert_partial_report_after(RuntimeError("simulovaná chyba"))

    def test_uncaught_subprocess_timeout_leaves_a_partial_report_and_propagates(self):
        self.assert_partial_report_after(subprocess.TimeoutExpired(["claude"], 30))

    def test_tool_check_letting_the_agent_read_outside_stops_the_experiment(self):
        with self.assertRaises(ExperimentStopped) as ctx:
            self.run_real("preflight", "leak-outside")
        self.assertEqual(ctx.exception.exit_code, 3)
        self.assertIn("chyba infrastruktury", str(ctx.exception))
        self.assertIn("čtení mimo strom odepřeno", str(ctx.exception))
        directory = ctx.exception.directory
        result = json.loads((directory / "preflight.json").read_text(encoding="utf-8"))
        self.assertFalse(result["ok"])
        self.assertFalse((directory / "runs").exists())
        self.assertFalse(self.config_dir().exists())


class ForbiddenToolTest(ExperimentTestCase):
    """A tool outside Read/Grep/Glob spoils the run in either variant (real adapter)."""

    cases = 1
    TOOL = "mcp__other__create_document"

    def test_call_to_a_forbidden_tool_spoils_every_attempt(self):
        file, member = correct_answer("ukazka-a")

        def build(config, paths):
            settings = ClaudeCodeSettings(
                model=config.model, isolation_root=paths.isolation_root,
                executable=(sys.executable, FAKE_AGENT, "auto", "call", self.TOOL, file, member),
                credentials_source=self.world.credentials,
                keep_apart=(paths.output_root, self.world.catalog.parent))
            return ClaudeCodeAdapter(settings, parent_env=fake_agent_env())

        outcome = run_experiment(self.world.config(), adapter_factory=build, now=fixed_now)
        results = [json.loads(p.read_text(encoding="utf-8"))
                   for p in sorted((outcome.directory / "runs").glob("*.result.json"))]
        for variant in ("bez karet", "s kartami"):
            with self.subTest(variant=variant):
                mine = [r for r in results if r["variant"] == variant]
                self.assertEqual(len(mine), 3 + run.MAX_REPLACEMENTS)
                self.assertTrue(all(r["state"] == "spoiled" for r in mine))
                self.assertTrue(all(r["spoil_reasons"] == [f"forbidden_tool: {self.TOOL}"]
                                    for r in mine))
        self.assertEqual(outcome.report["cases"][0]["result"], "incomplete")


class FreeAnswerFakeAgentTest(ExperimentTestCase):
    """The real adapter and `fake_agent.py free`: a question's answer survives the whole path."""

    cases = 1

    def test_the_free_answer_of_a_process_is_stored_whole(self):
        self.world.add_questions(bugs=False)
        text = "Odpověď podle `Src/Placeholder/LoadAction.cs:4`.\n\nA druhý odstavec."

        def build(config, paths):
            settings = ClaudeCodeSettings(
                model=config.model, isolation_root=paths.isolation_root,
                executable=(sys.executable, FAKE_AGENT, "auto", "free", text),
                credentials_source=self.world.credentials,
                keep_apart=(paths.output_root, self.world.catalog.parent))
            return ClaudeCodeAdapter(settings, parent_env=fake_agent_env())

        outcome = run_experiment(self.world.config(), adapter_factory=build, now=fixed_now)
        results = [json.loads(p.read_text(encoding="utf-8"))
                   for p in sorted((outcome.directory / "runs").glob("*.result.json"))]
        self.assertEqual([(r["case_id"], r["state"], r["answer_text"]) for r in results],
                         [(QUESTION_ID, "answered", text)] * 6)
        self.assertEqual(outcome.report["cases"], [])
        self.assertEqual(outcome.report["questions"][0]["variants"][0]["answered"], 3)


class PromptLayerTest(unittest.TestCase):
    """The prompt layer lives in `prompts.py`; `run.py` re-exports it for its callers."""

    NAMES = ("PATH_BASE", "PROMPT_PLACEHOLDER", "PROMPT_TEMPLATE", "QUESTION_PROMPT_TEMPLATE",
             "build_prompt", "item_text", "prompt_template", "sources_paragraph")

    def test_run_re_exports_every_name_of_the_prompt_layer(self):
        import prompts
        for name in self.NAMES:
            with self.subTest(name):
                self.assertIs(getattr(run, name), getattr(prompts, name))

    def test_the_templates_and_functions_are_defined_in_the_prompt_module(self):
        import prompts
        for name in ("build_prompt", "item_text", "prompt_template", "sources_paragraph"):
            self.assertEqual(getattr(prompts, name).__module__, "prompts")
        self.assertNotIn("PROMPT_TEMPLATE =", Path(run.__file__).read_text(encoding="utf-8"))

    def test_the_prompt_text_is_what_it_was(self):
        # The bytes of a stored prompt (its SHA-256 is in every report) must not move.
        template = run.prompt_template("Zdroje.", "podpora", "otazka")
        self.assertTrue(template.startswith("Odpovídáš na otázku uživatele systému. "))
        self.assertIn("Dostupné zdroje:\nZdroje.\n", template)
        self.assertIn("cesta je relativně ke kořeni zdrojáků systému, jak ji uvádí korpus a N",
                      template)
        case = run.prompt_template("Zdroje.", "vyvojar")
        self.assertIn('{"file": "<cesta k souboru relativně k pracovnímu adresáři>", "member":',
                      case)
        self.assertEqual(case.count("{zadani_pripadu}"), 1)


class VariantConfigTest(unittest.TestCase):
    def test_default_pair_and_validation(self):
        pair = run.default_variants(Path("c"), ["--x"])
        self.assertEqual([v.name for v in pair], ["bez karet", "s kartami"])
        self.assertIsNone(pair[0].corpus_dir)
        self.assertEqual((pair[1].corpus_dir, pair[1].ra_options), (Path("c"), ("--x",)))
        with self.assertRaises(ConfigError):
            run.default_variants(None)
        with self.assertRaises(ConfigError):
            run.validate_variants([VariantSpec("a")])
        with self.assertRaises(ConfigError):
            run.validate_variants([VariantSpec("a"), VariantSpec("a")])
        with self.assertRaises(ConfigError):
            run.validate_variants([VariantSpec("a"), VariantSpec("b", ra_options=("--x",))])

    def test_variants_file_paths_are_relative_to_the_file(self):
        variants = run.parse_variants({"format_version": 1, "variants": [
            {"name": "bez karet", "corpus_dir": None},
            {"name": "s kartami", "corpus_dir": "korpus", "ra_options": ["--x"],
             "card_format": "v2"},
        ]}, Path("/base"))
        self.assertEqual(variants[1].corpus_dir, Path("/base/korpus"))
        self.assertEqual(variants[1].ra_options, ("--x",))
        self.assertEqual(variants[1].card_format, "v2")


class ManifestSchemaVersionTest(unittest.TestCase):
    def setUp(self):
        import tempfile
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.dir = Path(tmp.name)

    def test_single_file_manifest(self):
        (self.dir / "manifest.json").write_text('{"schema_version": 10}', encoding="utf-8")
        self.assertEqual(run.manifest_schema_version(self.dir), 10)

    def test_manifest_by_lines_reads_the_header_document(self):
        (self.dir / "manifest-strings.json").write_text(
            '{"schema_version": 11, "strings": []}', encoding="utf-8")
        (self.dir / "manifest-nodes.jsonl").write_text("not json at all\n", encoding="utf-8")
        self.assertEqual(run.manifest_schema_version(self.dir), 11)

    def test_no_manifest_or_no_version(self):
        self.assertIsNone(run.manifest_schema_version(self.dir))
        (self.dir / "manifest.json").write_text('{"schema_version": true}', encoding="utf-8")
        self.assertIsNone(run.manifest_schema_version(self.dir))


if __name__ == "__main__":
    unittest.main()
