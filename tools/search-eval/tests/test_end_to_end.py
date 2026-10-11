"""The whole runner on a small test repo, without the reference repository or an agent (U8).

R12: the runner must be verifiable offline on a small repo with its own
catalog and scripted transcripts whose metrics are known. The world comes
from files in `fixtures/e2e/`:

* `repo/base`, `repo/fix` — the base revision and the later fix commit of a
  three-file C# repo, committed with a fixed identity and dates;
* `catalog.template.json` — three cases over it (placeholders for the two
  commits and for the markers below);
* `markers.json` — a synthetic secret and a customer placeholder name. This
  is the only file that writes them down; the test reads them from here;
* `transcripts/*.jsonl` — stored Claude Code stream-json for each kind of
  run: success (with and without a read in the corpus), wrong answer, invalid
  answer, answer only in a tool output, cap hit, forbidden access (a path
  outside the tree, a tool outside Read/Grep/Glob) and missing token
  telemetry.

AE6 (`ExperimentOverTestRepoTest`) replays a fixed plan of those transcripts
through the scripted adapter and compares the report with numbers computed
by hand below, never with numbers taken from the code under test.

AE4 (`NoLeakTest`) runs one case through the real Claude Code adapter with
`fixtures/fake_agent.py`, which copies the prompt into the agent's temp,
cache and configuration directories, and then looks for the two markers
everywhere they must not be.
"""
import contextlib
import io
import json
import os
import shutil
import stat
import subprocess
import sys
import tempfile
import unittest
from collections import Counter
from datetime import datetime
from pathlib import Path
from unittest import mock

import answers
import card_check
import run
from agents import claude_code, judge_cli
from agents.base import CAP_STEPS, CONFIG_DIR_NAME, Cap, RunRequest
from agents.claude_code import ClaudeCodeAdapter, ClaudeCodeSettings, parse_lines
from agents.scripted import ScriptedAdapter, load_transcript
from run import run_experiment
from tests.experiment_fixture import (FAKE_AGENT, QUESTION_ID, Experiment, fake_agent_env,
                                      free_answer_run, is_preflight, preflight_run)
from tests.test_calibration import make_set
from tests.test_card_check import (EVMN, EXT, Rule, ScriptedJudge, a1_types, build_corpus,
                                   catalog_json, claim, fact, place, question_json)

TESTS = Path(__file__).resolve().parent
FAKE_JUDGE = str(TESTS / "fixtures" / "fake_judge.py")
E2E = TESTS / "fixtures" / "e2e"
TRANSCRIPTS = E2E / "transcripts"
MARKERS_FILE = E2E / "markers.json"
MARKERS = json.loads(MARKERS_FILE.read_text(encoding="utf-8"))
FORBIDDEN_TOOL = "Bash"
CAP = Cap(seconds=600, steps=6, tokens=100_000)
CLOCK = datetime(2026, 10, 2, 12, 0, 0)

# What a correct answer names for each case (the catalog's place, as an agent writes it).
ANSWERS = {
    "ukazka-a": ("Src/Placeholder/LoadAction.cs", "LoadAction.SetScheduledTasksInvalid"),
    "ukazka-b": ("Src/Placeholder/Rounding.cs", "Rounding.Apply"),
    "ukazka-c": ("Src/Placeholder/Scheduler.cs", "Scheduler.Plan"),
}

# Transcript per attempt, in the order the runner makes the attempts of a cell.
PLAN = {
    ("ukazka-a", "bez karet"): ["success", "wrong_answer", "cap_hit"],
    ("ukazka-a", "s kartami"): ["success_corpus", "forbidden_tool", "success_corpus", "success"],
    ("ukazka-b", "bez karet"): ["invalid_answer", "answer_in_tool_output", "success"],
    ("ukazka-b", "s kartami"): ["wrong_answer", "missing_telemetry", "invalid_answer"],
    ("ukazka-c", "bez karet"): ["success", "success", "success"],
    ("ukazka-c", "s kartami"): ["forbidden_access", "success_corpus", "forbidden_access",
                                "forbidden_tool", "success_corpus"],
}

# The transcripts' `result` usage, steps and wall time, as written in the files:
#   success              100/1000/2000/50 = 3150, 1 step, 20 s
#   success_corpus        60/ 800/1200/40 = 2100, 2 steps, 12 s
#   wrong_answer         120/1100/2500/70 = 3790, 1 step, 25 s
#   invalid_answer        90/ 900/1800/30 = 2820, 1 step, 18 s
#   answer_in_tool_output 110/1000/2200/60 = 3370, 1 step, 22 s
#   missing_telemetry    no usage at all,         1 step, 15 s
#   cap_hit              killed on the 7th of 8 tool calls: 7 messages of
#                        10/100/200/5 = 70/700/1400/35 observed; the cap
#                        convention puts 100000 tokens, 6 steps, 600 s
U = "unknown"
EXPECTED_MEDIANS = {
    ("ukazka-a", "bez karet"): {  # success, wrong_answer, cap_hit
        "input_uncached": 100, "cache_write": 1000, "cache_read": 2000, "output": 50,
        "tokens_total": 3790, "steps": 1, "seconds": 25.0},
    ("ukazka-a", "s kartami"): {  # success_corpus, success_corpus, success
        "input_uncached": 60, "cache_write": 800, "cache_read": 1200, "output": 40,
        "tokens_total": 2100, "steps": 2, "seconds": 12.0},
    ("ukazka-b", "bez karet"): {  # invalid_answer, answer_in_tool_output, success
        "input_uncached": 100, "cache_write": 1000, "cache_read": 2000, "output": 50,
        "tokens_total": 3150, "steps": 1, "seconds": 20.0},
    ("ukazka-b", "s kartami"): {  # wrong_answer, missing_telemetry, invalid_answer
        "input_uncached": U, "cache_write": U, "cache_read": U, "output": U,
        "tokens_total": U, "steps": 1, "seconds": 18.0},
}
EXPECTED_SUCCESS_MEDIANS = {
    ("ukazka-a", "bez karet"): {
        "input_uncached": 100, "cache_write": 1000, "cache_read": 2000, "output": 50,
        "tokens_total": 3150, "steps": 1, "seconds": 20.0},
    ("ukazka-b", "s kartami"): {  # only the run without telemetry succeeded
        "input_uncached": U, "cache_write": U, "cache_read": U, "output": U,
        "tokens_total": U, "steps": 1, "seconds": 15.0},
}
EXPECTED_DIFFERENCE = {
    "ukazka-a": {"input_uncached": -40, "cache_write": -200, "cache_read": -800, "output": -10,
                 "tokens_total": -1690, "steps": 1, "seconds": -13.0},
    "ukazka-b": {"input_uncached": U, "cache_write": U, "cache_read": U, "output": U,
                 "tokens_total": U, "steps": 0, "seconds": -2.0},
}


# A free answer to the question the mixed catalog adds: it cites the place of its one fact
# (`LoadAction.Reload`, line 13 of the base revision).
QUESTION_ANSWER = ("Metoda `LoadAction.Reload` jen zavolá `Src/Placeholder/LoadAction.cs:13` "
                   "a nic dalšího nedělá.")


def values_for(case_id, corpus="/korpus"):
    file, member = ANSWERS[case_id]
    return {"__FILE__": file, "__MEMBER__": member, "__CORPUS__": str(corpus)}


class StoredTranscriptTest(unittest.TestCase):
    """`load_transcript` and the scripted adapter on the stored transcripts."""

    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="se-u8-"))
        self.addCleanup(shutil.rmtree, self.tmp, ignore_errors=True)

    def test_every_stored_transcript_parses_without_its_header(self):
        names = sorted(p.stem for p in TRANSCRIPTS.glob("*.jsonl"))
        self.assertEqual(names, sorted({kind for plan in PLAN.values() for kind in plan}))
        for path in TRANSCRIPTS.glob("*.jsonl"):
            with self.subTest(transcript=path.name):
                scripted = load_transcript(path, values_for("ukazka-a"))
                self.assertGreater(scripted.wall_seconds, 1.0)
                self.assertFalse(any('"scripted"' in line for line in scripted.lines))
                parsed = parse_lines(scripted.lines)
                self.assertIsNotNone(parsed.init)

    def test_values_are_json_escaped(self):
        path = self.tmp / "t.jsonl"
        path.write_text('{"type": "assistant", "message": {"id": "m", "content": [{"type": '
                        '"tool_use", "id": "t", "name": "Read", "input": {"file_path": '
                        '"__FILE__"}}]}}\n', encoding="utf-8")
        tricky = 'C:\\strom\\"uvozovky".cs'
        parsed = parse_lines(load_transcript(path, {"__FILE__": tricky}).lines)
        self.assertEqual(parsed.events[0].tool_calls[0].paths, (tricky,))

    def test_placeholder_without_value_is_an_error(self):
        with self.assertRaises(ValueError) as ctx:
            load_transcript(TRANSCRIPTS / "success.jsonl", {"__FILE__": "a.cs"})
        self.assertIn("__MEMBER__", str(ctx.exception))

    def test_step_cap_cuts_the_replay_where_the_watchdog_would(self):
        work = self.tmp / "work"
        work.mkdir()
        transcript = self.tmp / "out" / "run.jsonl"
        transcript.parent.mkdir()
        scripted = load_transcript(TRANSCRIPTS / "cap_hit.jsonl", values_for("ukazka-a"))
        adapter = ScriptedAdapter(lambda request: scripted, self.tmp / "iso")
        record = adapter.run(RunRequest(work, "p", CAP, transcript_path=transcript))
        self.assertEqual((record.cap_hit.kind, record.cap_hit.limit, record.cap_hit.observed,
                          record.cap_hit.overshoot), (CAP_STEPS, 6.0, 7.0, 1.0))
        self.assertEqual(record.steps, 7)
        self.assertFalse(record.has_result)
        self.assertIsNone(record.final_message)
        self.assertEqual((record.tokens.input_uncached, record.tokens.cache_write,
                          record.tokens.cache_read, record.tokens.output), (70, 700, 1400, 35))
        # init + 6 answered calls (2 lines each) + the 7th call, then nothing.
        self.assertEqual(len(transcript.read_text(encoding="utf-8").splitlines()), 1 + 12 + 1)
        self.assertEqual(record.wall_seconds, 40.0)
        self.assertFalse((self.tmp / "iso" / CONFIG_DIR_NAME).exists())


class ExperimentOverTestRepoTest(unittest.TestCase):
    """AE6: six clean runs per complete case, known metrics, failures, replacements."""

    with_question = False  # a subclass adds a question to the same catalog

    @classmethod
    def setUpClass(cls):
        cls.world = Experiment(cases=3, fixture=E2E, values=MARKERS)
        if cls.with_question:
            cls.world.add_questions(line=13)
        cls.addClassCleanup(cls.world.cleanup)
        attempts = Counter()

        def play(request):
            if is_preflight(request):
                return preflight_run(request)
            if cls.world.is_question(request):
                return free_answer_run(QUESTION_ANSWER)
            case = cls.world.case_of(request)
            variant = "s kartami" if request.with_cards else "bez karet"
            attempts[(case, variant)] += 1
            kind = PLAN[(case, variant)][attempts[(case, variant)] - 1]
            return load_transcript(TRANSCRIPTS / f"{kind}.jsonl",
                                   values_for(case, request.corpus_dir))

        cls.adapter = cls.world.scripted(play)
        cls.outcome = run_experiment(cls.world.config(cap=CAP),
                                     adapter_factory=lambda c, p: cls.adapter,
                                     now=lambda: CLOCK)
        cls.report = cls.outcome.report
        cls.cases = {c["case_id"]: c for c in cls.report["cases"]}
        runs = cls.outcome.directory / "runs"
        everything = [json.loads(p.read_text(encoding="utf-8"))
                      for p in sorted(runs.glob("*.result.json"))]
        cls.results = [r for r in everything if r["case_id"] in ANSWERS]
        cls.question_results = [r for r in everything if r["case_id"] not in ANSWERS]

    def variant(self, case_id, name):
        return next(v for v in self.cases[case_id]["variants"] if v["variant"] == name)

    def cell(self, case_id, name):
        return [r for r in self.results if r["case_id"] == case_id and r["variant"] == name]

    def test_runs_and_states_per_case(self):
        self.assertEqual(self.outcome.status, "completed")
        self.assertEqual(len(self.results), sum(len(kinds) for kinds in PLAN.values()))
        expected = {  # clean, successes, spoiled, failure reasons
            ("ukazka-a", "bez karet"): (3, 1, 0, ["wrong_location", "cap_steps"]),
            ("ukazka-a", "s kartami"): (3, 3, 1, []),
            ("ukazka-b", "bez karet"): (3, 1, 0, ["invalid_answer", "invalid_answer"]),
            ("ukazka-b", "s kartami"): (3, 1, 0, ["wrong_location", "invalid_answer"]),
            ("ukazka-c", "bez karet"): (3, 3, 0, []),
            ("ukazka-c", "s kartami"): (2, 2, 3, []),
        }
        for (case_id, name), (clean, successes, spoiled, reasons) in expected.items():
            with self.subTest(case=case_id, variant=name):
                v = self.variant(case_id, name)
                self.assertEqual((v["clean_runs"], v["successes"], v["spoiled_attempts"],
                                  v["failure_reasons"]), (clean, successes, spoiled, reasons))
        self.assertEqual(self.variant("ukazka-a", "bez karet")["capped_runs"], 1)
        for case_id in ("ukazka-a", "ukazka-b"):
            clean = sum(v["clean_runs"] for v in self.cases[case_id]["variants"])
            self.assertEqual(clean, 6, case_id)

    def test_verdicts_and_tallies(self):
        self.assertEqual({c: self.cases[c]["result"] for c in self.cases},
                         {"ukazka-a": "win", "ukazka-b": "tie", "ukazka-c": "incomplete"})
        summary = self.report["summary"]
        self.assertEqual(summary["all_cases"], {"win": 1, "loss": 0, "tie": 1, "incomplete": 1})
        # ukazka-a is flagged (used for tuning, prompt names the location).
        self.assertEqual(summary["unflagged_cases"],
                         {"win": 0, "loss": 0, "tie": 1, "incomplete": 1})

    def test_medians_and_differences_match_the_hand_computed_values(self):
        for (case_id, name), medians in EXPECTED_MEDIANS.items():
            with self.subTest(case=case_id, variant=name):
                self.assertEqual(self.variant(case_id, name)["medians_all"], medians)
        for (case_id, name), medians in EXPECTED_SUCCESS_MEDIANS.items():
            with self.subTest(case=case_id, variant=name, only="success"):
                self.assertEqual(self.variant(case_id, name)["medians_success"], medians)
        for case_id, difference in EXPECTED_DIFFERENCE.items():
            with self.subTest(case=case_id, only="difference"):
                self.assertEqual(self.cases[case_id]["difference_compared_minus_reference"],
                                 difference)

    def test_spoiled_attempts_are_replaced_at_most_twice(self):
        for case_id, expected in (("ukazka-a", ["success", "spoiled", "success", "success"]),
                                  ("ukazka-c", ["spoiled", "success", "spoiled", "spoiled",
                                                "success"])):
            with self.subTest(case=case_id):
                cell = self.cell(case_id, "s kartami")
                self.assertEqual([r["attempt"] for r in cell], list(range(1, len(expected) + 1)))
                self.assertEqual([r["state"] for r in cell], expected)
                spoiled = [r["order"] for r in cell if r["state"] == "spoiled"]
                replacements = [r["replaces"] for r in cell if r["replaces"] is not None]
                self.assertEqual(replacements, spoiled[:run.MAX_REPLACEMENTS])
        reasons = [(s["case_id"], s["reasons"]) for s in self.report["spoiled_attempts"]]
        self.assertEqual(sorted(reasons), [
            ("ukazka-a", [f"forbidden_tool: {FORBIDDEN_TOOL}"]),
            ("ukazka-c", [f"forbidden_tool: {FORBIDDEN_TOOL}"]),
            ("ukazka-c", ["path_outside: Read: ../../mimo-strom/reseni.txt"]),
            ("ukazka-c", ["path_outside: Read: ../../mimo-strom/reseni.txt"]),
        ])

    def test_single_runs_keep_what_the_transcript_showed(self):
        capped = next(r for r in self.cell("ukazka-a", "bez karet") if r["cap_hit"])
        self.assertEqual((capped["state"], capped["reason"]), ("failure", "cap_steps"))
        self.assertEqual(capped["cap_hit"], {"kind": "steps", "limit": 6.0, "observed": 7.0,
                                             "overshoot": 1.0})
        self.assertEqual(capped["tokens"], {"input_uncached": 70, "cache_write": 700,
                                            "cache_read": 1400, "output": 35})
        in_tool_output = self.cell("ukazka-b", "bez karet")[1]
        self.assertEqual((in_tool_output["state"], in_tool_output["reason"],
                          in_tool_output["answer"]), ("failure", "invalid_answer", None))
        no_telemetry = self.cell("ukazka-b", "s kartami")[1]
        self.assertEqual(no_telemetry["state"], "success")
        self.assertEqual(no_telemetry["tokens"], {"input_uncached": None, "cache_write": None,
                                                  "cache_read": None, "output": None})
        read_corpus = self.cell("ukazka-a", "s kartami")[0]
        self.assertEqual((read_corpus["state"], read_corpus["steps"]), ("success", 2))

    def test_only_runs_with_cards_get_the_corpus(self):
        checks = [r for r in self.adapter.requests if is_preflight(r)]
        runs = [r for r in self.adapter.requests
                if not is_preflight(r) and not self.world.is_question(r)]
        self.assertEqual(len(checks), 2)
        corpus = os.path.realpath(self.world.root / "corpus")
        self.assertEqual({str(r.corpus_dir) for r in runs if r.with_cards}, {corpus})
        self.assertEqual(sum(1 for r in runs if not r.with_cards),
                         sum(len(kinds) for (_, v), kinds in PLAN.items() if v == "bez karet"))
        self.assertEqual(self.adapter.max_active, 1)

    def test_markdown_names_unknown_values_and_failure_reasons(self):
        text = (self.outcome.directory / "report.md").read_text(encoding="utf-8")
        for expected in ("neznámý", "strop kroků", "neplatná odpověď", "jiné místo"):
            self.assertIn(expected, text)


class MixedExperimentOverTestRepoTest(ExperimentOverTestRepoTest):
    """The same experiment with a question in the catalog (KTD10).

    Every inherited test runs again on the bugs and must see the same hand-computed
    numbers: a question changes the schedule, not how a case is scored. The new tests
    cover what the question adds, up to `answers.py hodnot-odpovedi` over the finished
    experiment directory."""

    with_question = True

    def test_the_question_runs_are_stored_whole_and_not_scored(self):
        self.assertEqual(len(self.question_results), 6)
        for result in self.question_results:
            with self.subTest(order=result["order"]):
                self.assertEqual((result["case_id"], result["state"], result["reason"],
                                  result["answer_text"], result["answer"]),
                                 (QUESTION_ID, "answered", None, QUESTION_ANSWER, None))
        self.assertEqual({r["variant"] for r in self.question_results},
                         {"bez karet", "s kartami"})
        self.assertEqual([c["case_id"] for c in self.report["cases"]],
                         ["ukazka-a", "ukazka-b", "ukazka-c"])
        question = self.report["questions"][0]
        self.assertEqual([(v["stored_runs"], v["answered"]) for v in question["variants"]],
                         [(3, 3), (3, 3)])

    def test_the_answers_of_the_experiment_are_judged(self):
        state = self.world.root / "state"
        judges = self.world.root / "judges.json"
        judges.write_text(json.dumps({"format_version": 1, "hodnotitele": [
            {"id": "j1", "rodina": "openai", "model": "alias", "prikaz": ["cli-neni"],
             "vystup": "claude-json"}]}), encoding="utf-8")
        runner = ScriptedJudge([Rule("Ukázkový fakt", "jen zavolá", "VYJADRUJE")])
        # The scripted adapter replays Claude Code transcripts; production does not know it.
        families = mock.patch.dict(answers.AGENT_FAMILIES, {"scripted": "claude"})
        with contextlib.redirect_stdout(io.StringIO()), families:
            code = answers.main(["hodnot-odpovedi", str(self.outcome.directory),
                                 "--source-repo", str(self.world.repo),
                                 "--katalog", str(self.world.catalog),
                                 "--hodnotitele", str(judges), "--stav", str(state)],
                                runner=runner)
        self.assertEqual(code, 0)
        path = card_check.measurement_path(state / "odpovedi" / self.outcome.directory.name)
        data = json.loads(path.read_text(encoding="utf-8"))
        self.assertEqual((data["status"], data["agent_family"], data["pokus"]["role"]),
                         ("hotovo", "claude", "vyvojar"))
        self.assertFalse(data["pokus"]["nedokonceny"])
        facts = data["results"]["facts"]
        self.assertEqual({(f["variant"], f["status"]) for f in facts},
                         {("bez karet", card_check.FACT_PRESENT),
                          ("s kartami", card_check.FACT_PRESENT)})
        self.assertEqual(sorted(f["run"] for f in facts),
                         sorted(r["order"] for r in self.question_results))
        self.assertEqual([v["korpus"] is not None for v in data["corpus"]["varianty"]],
                         [False, True])
        # the same answer text is judged once, the other runs come from the cache
        self.assertEqual(len(runner.calls), 3)


def _contains(path, markers):
    """Whether the file's bytes hold a marker.

    Only regular files are read (a FIFO in a shared temp dir would block the
    read); unreadable and huge files are skipped.
    """
    try:
        info = os.lstat(path)
        if not stat.S_ISREG(info.st_mode) or info.st_size > 50_000_000:
            return False
        data = Path(path).read_bytes()
    except OSError:
        return False
    return any(m.encode("utf-8") in data for m in markers)


def _files(root, skip=()):
    """Every file below `root` (or `root` itself if it is a file), minus `skip` dirs."""
    if os.path.isfile(root):
        yield str(root)
        return
    skip = [os.path.realpath(s) for s in skip]
    for dirpath, dirnames, filenames in os.walk(root):
        real = os.path.realpath(dirpath)
        dirnames[:] = [d for d in dirnames
                       if os.path.realpath(os.path.join(real, d)) not in skip]
        for name in filenames:
            yield os.path.join(dirpath, name)


def _repo_files():
    """Tracked and untracked (not ignored) files of this runner, from the repository root."""
    root = Path(run.__file__).resolve().parents[2]
    try:
        listed = subprocess.run(
            ["git", "-C", str(root), "ls-files", "-z", "--cached", "--others",
             "--exclude-standard", "--", "tools/search-eval"],
            capture_output=True, check=True).stdout.decode("utf-8")
        return [root / p for p in listed.split("\0") if p]
    except (OSError, subprocess.CalledProcessError):  # not a git checkout
        tool = Path(run.__file__).resolve().parent
        return [Path(p) for p in _files(tool) if "__pycache__" not in Path(p).parts]


class NoLeakTest(unittest.TestCase):
    """AE4: the case's secret and customer name stay in the allowed stores only.

    Allowed: the catalog (outside the repo) and the experiment directory's
    verbatim prompts (outside the repo, KTD8 point 5). Not allowed: this
    repository, the report and the other experiment files, the agent's
    temp, cache and configuration directory, and anything else the run
    leaves behind on disk.
    """

    def setUp(self):
        self.markers = tuple(MARKERS.values())
        self.temp_before = set(os.listdir(tempfile.gettempdir()))
        self.world = Experiment(cases=1, fixture=E2E, values=MARKERS)
        self.addCleanup(self.world.cleanup)

    def run_through_the_real_adapter(self):
        """One case, both variants, `fake_agent stash`; returns (outcome, stash evidence)."""
        file, member = ANSWERS["ukazka-a"]
        stashed = []
        real = claude_code.run_watched

        def watched(argv, **kwargs):
            outcome = real(argv, **kwargs)
            env = kwargs["env"]
            places = [Path(env["TMPDIR"]), Path(env["XDG_CACHE_HOME"]) / "fake-cli",
                      Path(env["CLAUDE_CONFIG_DIR"]) / "projects"]
            stashed.append([_contains(p / "stash-prompt.txt", self.markers) for p in places])
            return outcome

        def build(config, paths):
            settings = ClaudeCodeSettings(
                model=config.model, isolation_root=paths.isolation_root,
                executable=(sys.executable, FAKE_AGENT, "auto", "stash", file, member),
                credentials_source=self.world.credentials,
                keep_apart=(paths.output_root, self.world.catalog.parent))
            return ClaudeCodeAdapter(settings, parent_env=fake_agent_env())

        with mock.patch.object(claude_code, "run_watched", watched):
            outcome = run_experiment(self.world.config(), adapter_factory=build,
                                     now=lambda: CLOCK)
        return outcome, stashed

    def test_secret_and_customer_name_appear_only_in_allowed_stores(self):
        outcome, stashed = self.run_through_the_real_adapter()
        self.assertEqual(outcome.status, "completed")
        # The leak was real: every run left the prompt in temp, cache and config dir.
        runs = [s for s in stashed if any(s)]
        self.assertEqual(runs, [[True, True, True]] * 6)
        directory = outcome.directory
        prompts = directory / "prompts"
        self.assertTrue(all(_contains(p, self.markers) for p in prompts.iterdir()))

        isolation = self.world.isolation
        self.assertFalse((isolation / CONFIG_DIR_NAME).exists())
        leaks = [p for p in _files(isolation) if _contains(p, self.markers)]
        self.assertEqual(leaks, [], "adresář izolace")

        for name in ("report.md", "report.json", "preflight.json", "preparation.json",
                     "schedule.json"):
            self.assertFalse(_contains(directory / name, self.markers), name)
        outside_prompts = [p for p in _files(directory, skip=[prompts])
                           if _contains(p, self.markers)]
        self.assertEqual(outside_prompts, [], "adresář pokusu mimo prompts/")

        rest = [p for p in _files(self.world.root, skip=[self.world.catalog.parent, directory])
                if _contains(p, self.markers)]
        self.assertEqual(rest, [], "export, korpus, zdrojové repo, přihlášení")

        # Entries other processes create meanwhile are scanned too; they cannot
        # hold markers that exist only in this test's catalog.
        new_temp = set(os.listdir(tempfile.gettempdir())) - self.temp_before
        new_temp.discard(self.world.root.name)
        temp_leaks = [p for entry in sorted(new_temp)
                      for p in _files(Path(tempfile.gettempdir()) / entry)
                      if _contains(p, self.markers)]
        self.assertEqual(temp_leaks, [], "systémový dočasný adresář")

        repo_files = _repo_files()
        self.assertIn(MARKERS_FILE, [p.resolve() for p in repo_files])  # the scan sees fixtures
        repo_leaks = [str(p) for p in repo_files
                      if p.resolve() != MARKERS_FILE and _contains(p, self.markers)]
        self.assertEqual(repo_leaks, [], "repozitář")

    # -- the answer check: state directory, review files, report, calibration -----------------

    def answer_check_world(self):
        """The files of the answer check under the world's root; the markers are in the question
        (its text and its fact), in the corpus-independent claim and in the calibration set."""
        secret, customer = MARKERS["__SECRET__"], MARKERS["__CUSTOMER__"]
        root = self.world.root / "odpovedi"
        paths = {name: root / name for name in (
            "stav", "kontrola", "zprava", "katalog", "kalibrace-sada", "kalibrace-vystup",
            "hodnotitele")}
        for name in ("kontrola", "katalog", "kalibrace-sada", "hodnotitele"):
            paths[name].mkdir(parents=True)
        corpus = build_corpus(root / "korpus", a1_types())
        item = question_json(
            [fact("f1", f"Tichý návrat, když chybí dokument ({customer})",
                  [place(EVMN, "EditorViewModelNew.RunNew()", radky=20)]),
             fact("f3", "Externí kontrola ověří dokument",
                  [place(EXT, "ExternalCheck.Run()", radky="30-35")])],
            [claim("z1", "Změna řádku gridu se ukládá okamžitě",
                   [place("src/Acme/EditorViewNew.cs", "EditorViewNew.OnCellChanged()",
                          radky="42-60")])])
        item["dotaz"] = f"Jak editor ukládá dokument? Heslo je {secret}."
        catalog = paths["katalog"] / "katalog.json"
        catalog.write_text(json.dumps(catalog_json(item), ensure_ascii=False), encoding="utf-8")
        judges = paths["hodnotitele"] / "judges.json"
        judges.write_text(json.dumps({"format_version": 1, "hodnotitele": [
            {"id": "stash", "rodina": "openai", "model": "alias", "vystup": "claude-json",
             "prikaz": [sys.executable, FAKE_JUDGE, "claude", "stash"]}]}), encoding="utf-8")
        sada = paths["kalibrace-sada"] / "sada.json"
        data = make_set(extra_sentence=f" {secret}")
        data["polozky"][0]["tvrzeni"]["text"] += f" ({customer})"
        sada.write_text(json.dumps(data, ensure_ascii=False), encoding="utf-8")
        return paths, corpus, catalog, judges, sada

    def run_answers(self, argv):
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = answers.main(argv)  # the real judge runner: judge_cli and fake_judge.py stash
        self.assertEqual(code, 0, err.getvalue())
        return out.getvalue()

    def test_answer_check_files_stay_in_allowed_stores(self):
        paths, corpus, catalog, judges, sada = self.answer_check_world()
        repo = str(self.world.repo)
        state, review_dir = paths["stav"], paths["kontrola"]
        stashed = []
        real = judge_cli.run_watched

        def watched(argv, **kwargs):
            outcome = real(argv, **kwargs)
            env = kwargs["env"]
            config = env.get("CLAUDE_CONFIG_DIR") or env["CODEX_HOME"]
            places = [Path(env["TMPDIR"]), Path(env["XDG_CACHE_HOME"]) / "fake-cli",
                      Path(config) / "projects", Path(kwargs["cwd"])]
            stashed.append([_contains(p / "stash-prompt.txt", self.markers) for p in places])
            return outcome

        with mock.patch.object(judge_cli, "run_watched", watched):
            self.run_answers(["kontrola-karet", "--source-repo", repo, "--agent-family", "claude",
                              "--katalog", str(catalog), "--korpus", str(corpus),
                              "--hodnotitele", str(judges), "--stav", str(state)])
            measurement = card_check.measurement_path(state)
            review_file = review_dir / "kontrola.json"
            self.run_answers(["kontrola", "--source-repo", repo, "--stav", str(state),
                              "--mereni", str(measurement), "--vystup", str(review_file)])
            filled = json.loads(review_file.read_text(encoding="utf-8"))
            for row in filled["radky"]:
                row["rozhodnuti"] = "vyjadruje" if row["druh"] == "fakt" else "neobsahuje"
            review_file.write_text(json.dumps(filled, ensure_ascii=False), encoding="utf-8")
            self.run_answers(["kontrola", "--source-repo", repo, "--stav", str(state),
                              "--nacti", str(review_file)])
            self.run_answers(["kalibrace", "--source-repo", repo, "--agent-family", "claude",
                              "--sada", str(sada), "--hodnotitele", str(judges),
                              "--stav", str(state),
                              "--vystup", str(paths["kalibrace-vystup"] / "vysledek.json")])
            self.run_answers(["zprava", "--source-repo", repo, "--mereni", str(measurement),
                              "--vystup", str(paths["zprava"]), "--stav", str(state),
                              "--kalibrace", str(paths["kalibrace-vystup"]),
                              "--sada-kalibrace", str(sada)])

        # The leak was real: a judge that saw the markers left the prompt in its temp, cache,
        # configuration and working directories, and the runner removed all four.
        leaking = [s for s in stashed if any(s)]
        self.assertTrue(leaking)
        self.assertEqual(leaking, [[True, True, True, True]] * len(leaking))

        isolation = state / "izolace"
        self.assertFalse((isolation / CONFIG_DIR_NAME).exists())
        self.assertEqual(sorted(p.name for p in isolation.iterdir()), [])
        # Allowed: the state directory (the measurement, the judge's cache), the files of the
        # review, the report, the catalog, the calibration set and its result.
        in_state = [p for p in _files(state) if _contains(p, self.markers)]
        self.assertTrue(in_state)
        for name in ("kontrola", "zprava"):
            self.assertTrue([p for p in _files(paths[name]) if _contains(p, self.markers)], name)
        self.assertTrue(_contains(card_check.measurement_path(state), self.markers))

        allowed = [state, review_dir, paths["zprava"], paths["katalog"], paths["kalibrace-sada"],
                   paths["kalibrace-vystup"], self.world.catalog.parent]
        rest = [p for p in _files(self.world.root, skip=allowed) if _contains(p, self.markers)]
        self.assertEqual(rest, [], "export, korpus, soubor hodnotitelů, zdrojové repo")

        new_temp = set(os.listdir(tempfile.gettempdir())) - self.temp_before
        new_temp.discard(self.world.root.name)
        temp_leaks = [p for entry in sorted(new_temp)
                      for p in _files(Path(tempfile.gettempdir()) / entry)
                      if _contains(p, self.markers)]
        self.assertEqual(temp_leaks, [], "systémový dočasný adresář")

        repo_leaks = [str(p) for p in _repo_files()
                      if p.resolve() != MARKERS_FILE and _contains(p, self.markers)]
        self.assertEqual(repo_leaks, [], "repozitář")


if __name__ == "__main__":
    unittest.main()
