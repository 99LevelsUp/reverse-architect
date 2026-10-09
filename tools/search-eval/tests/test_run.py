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
from run import (ConfigError, ExperimentStopped, VariantSpec, build_schedule, run_experiment)
from tests.experiment_fixture import (FAKE_AGENT, Experiment, answer_run, auth_error_run,
                                      correct_answer, fake_agent_env, is_preflight,
                                      preflight_run)
from tests.test_prepare import _make_dir_link, _remove_dir_link

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
        """Tool check passes; every run answers its case correctly."""
        if is_preflight(request):
            return preflight_run(request)
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


class DryRunTest(ExperimentTestCase):
    def test_dry_run_prints_counts_and_cap_without_starting_the_agent(self):
        w = self.world
        argv = ["--catalog", str(w.catalog), "--source-repo", str(w.repo),
                "--output-root", str(w.output), "--export-dir", str(w.export),
                "--isolation-root", str(w.isolation), "--ra-binary", "ra", "--model", "m",
                "--cap-seconds", "600", "--cap-steps", "50", "--cap-tokens", "100000",
                "--seed", "7", "--corpus-dir", str(w.root / "corpus"), "--dry-run"]
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
                "--seed", "7", "--corpus-dir", str(w.root / "ws" / "corpus"),
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


if __name__ == "__main__":
    unittest.main()
