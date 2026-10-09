"""Comparing two corpus variants instead of without/with cards (U7, AE5, R11).

Two corpora are generated from the same tree with different RA options; the
fake RA writes the options into `options.txt`, so each corpus carries a
marker the other one lacks. Each variant reads only its own corpus, as files.
"""
import io
import json
import re
import unittest
from collections import Counter
from contextlib import redirect_stderr
from dataclasses import replace
from datetime import datetime
from pathlib import Path

import run
from agents.base import RunRequest
from run import ExperimentStopped, run_experiment
from tests.experiment_fixture import (FAKE_RA, Experiment, answer_run, correct_answer,
                                      is_preflight, preflight_run)

MARKER_A, MARKER_B = "--marker-a", "--marker-b"


def now():
    return datetime(2026, 10, 2, 12, 0, 0)


class TwoCorporaTest(unittest.TestCase):
    def setUp(self):
        self.world = Experiment(cases=2)
        self.addCleanup(self.world.cleanup)
        self.variants = (
            self.world.corpus_variant("korpus A", "corpus-a", [MARKER_A]),
            self.world.corpus_variant("korpus B", "corpus-b", [MARKER_B]),
        )
        self.attempts = Counter()

    def corpus(self, name):
        return self.world.root / ("corpus-a" if name == "a" else "corpus-b")

    def script(self, request: RunRequest):
        """Each run reads its own corpus marker; the very first run of A reads B's."""
        if is_preflight(request):
            return preflight_run(request)
        mine = "a" if Path(request.corpus_dir).name == "corpus-a" else "b"
        self.attempts[mine] += 1
        target = "b" if (mine, self.attempts[mine]) == ("a", 1) else mine
        options = self.corpus(target) / "options.txt"
        return answer_run(*correct_answer(self.world.case_of(request)), reads=[str(options)],
                          read_text=options.read_text(encoding="utf-8"))

    def run_two_corpora(self):
        adapter = self.world.scripted(self.script)
        outcome = run_experiment(self.world.config(variants=self.variants),
                                 adapter_factory=lambda c, p: adapter, now=now)
        return adapter, outcome

    def test_each_variant_reads_only_its_own_corpus(self):
        adapter, outcome = self.run_two_corpora()
        self.assertIn(MARKER_A, (self.corpus("a") / "options.txt").read_text(encoding="utf-8"))
        self.assertNotIn(MARKER_A, (self.corpus("b") / "options.txt").read_text(encoding="utf-8"))
        results = [json.loads(p.read_text(encoding="utf-8"))
                   for p in sorted((outcome.directory / "runs").glob("*.result.json"))]
        spoiled = [r for r in results if r["state"] == "spoiled"]
        self.assertEqual(len(spoiled), 1)
        self.assertEqual(spoiled[0]["variant"], "korpus A")
        self.assertTrue(spoiled[0]["spoil_reasons"][0].startswith("path_outside"))
        self.assertIn("corpus-b", spoiled[0]["spoil_reasons"][0])
        for result in results:
            if result["state"] == "spoiled":
                continue
            transcript = (outcome.directory / result["transcript"]).read_text(encoding="utf-8")
            own, other = (MARKER_A, MARKER_B) if result["variant"] == "korpus A" \
                else (MARKER_B, MARKER_A)
            with self.subTest(order=result["order"]):
                self.assertEqual(result["state"], "success")
                self.assertIn(own, transcript)
                self.assertNotIn(other, transcript)
        corpora = {r.corpus_dir for r in adapter.requests if not is_preflight(r)}
        self.assertEqual({p.name for p in corpora}, {"corpus-a", "corpus-b"})
        self.assertEqual(outcome.report["summary"]["all_cases"],
                         {"win": 0, "loss": 0, "tie": 2, "incomplete": 0})
        meta = {v["name"]: v for v in outcome.report["experiment"]["variants"]}
        self.assertEqual(meta["korpus A"]["corpus_receipt"]["options"], [MARKER_A])
        self.assertEqual(meta["korpus B"]["corpus_receipt"]["options"], [MARKER_B])
        self.assertNotEqual(meta["korpus A"]["corpus_receipt"]["corpus_fingerprint"],
                            meta["korpus B"]["corpus_receipt"]["corpus_fingerprint"])

    def test_report_has_the_same_structure_as_without_and_with_cards(self):
        _, two_corpora = self.run_two_corpora()
        pair_world = Experiment(cases=2)
        self.addCleanup(pair_world.cleanup)

        def honest(request):
            if is_preflight(request):
                return preflight_run(request)
            return answer_run(*correct_answer(pair_world.case_of(request)))

        adapter = pair_world.scripted(honest)
        pair = run_experiment(pair_world.config(), adapter_factory=lambda c, p: adapter, now=now)
        self.assertEqual(shape(two_corpora.report), shape(pair.report))
        self.assertEqual(headings(two_corpora.directory, ("korpus A", "korpus B")),
                         headings(pair.directory, ("bez karet", "s kartami")))


def shape(data):
    """Key sets of every part of the report that does not depend on variant names."""
    case = data["cases"][0]
    variant = case["variants"][0]
    return {
        "top": sorted(data),
        "experiment": sorted(data["experiment"]),
        "variant_meta": [sorted(v) for v in data["experiment"]["variants"]],
        "summary": sorted(data["summary"]),
        "tally": sorted(data["summary"]["all_cases"]),
        "case": sorted(case),
        "case_variant": sorted(variant),
        "medians": sorted(variant["medians_all"]),
        "success_medians": sorted(variant["medians_success"]),
        "difference": sorted(case["difference_compared_minus_reference"]),
        "cache_row": sorted(data["cache_read_by_order"][0]),
        "cases": len(data["cases"]),
    }


def headings(directory, names):
    text = (directory / "report.md").read_text(encoding="utf-8")
    for index, name in enumerate(names):
        text = text.replace(name, f"<varianta {index + 1}>")
    return [line for line in text.splitlines() if line.startswith("#")
            and not line.startswith("# ")]


class PerVariantRaCommandTest(unittest.TestCase):
    """A variant may name its own RA command: two versions of RA side by side (U6)."""

    def setUp(self):
        self.world = Experiment(cases=2)
        self.addCleanup(self.world.cleanup)

    def honest(self, request):
        if is_preflight(request):
            return preflight_run(request)
        return answer_run(*correct_answer(self.world.case_of(request)))

    def run_pair(self, variants):
        adapter = self.world.scripted(self.honest)
        return run_experiment(self.world.config(variants=variants),
                              adapter_factory=lambda c, p: adapter, now=now)

    def test_a_variant_with_its_own_command_generates_its_corpus_with_it(self):
        w = self.world
        outcome = self.run_pair((w.corpus_variant("RA 2", "corpus-v2",
                                                  ra_command=w.other_ra("ra-v2")),
                                 w.corpus_variant("RA výchozí", "corpus-v1")))
        own, default = w.root / "corpus-v2", w.root / "corpus-v1"
        self.assertEqual((own / "ra-version.txt").read_text(encoding="utf-8"), "ra-v2\n")
        self.assertFalse((default / "ra-version.txt").exists())
        meta = {v["name"]: v for v in outcome.report["experiment"]["variants"]}
        self.assertEqual(meta["RA 2"]["corpus_receipt"]["ra_version"], "reverse-architect ra-v2")
        self.assertEqual(meta["RA výchozí"]["corpus_receipt"]["ra_version"],
                         "reverse-architect 0.0.0-fake")
        self.assertNotEqual(meta["RA 2"]["corpus_receipt"]["corpus_fingerprint"],
                            meta["RA výchozí"]["corpus_receipt"]["corpus_fingerprint"])

    def test_variants_without_a_command_use_the_experiments_one_as_before(self):
        w = self.world
        outcome = self.run_pair(w.default_variants())
        self.assertFalse((w.root / "corpus" / "ra-version.txt").exists())
        self.assertEqual(outcome.report["summary"]["all_cases"],
                         {"win": 0, "loss": 0, "tie": 2, "incomplete": 0})

    def test_the_experiments_command_is_not_used_for_a_variant_with_its_own(self):
        w = self.world
        broken = replace(w.config(variants=(
            w.corpus_variant("RA 2", "corpus-v2", ra_command=w.other_ra("ra-v2")),
            w.corpus_variant("RA výchozí", "corpus-v1"))), ra_command=(*FAKE_RA, "--fail"))
        with self.assertRaises(ExperimentStopped) as ctx:
            run_experiment(broken, adapter_factory=lambda c, p: self.fail("agent nesmí startovat"),
                           now=now)
        self.assertEqual(ctx.exception.exit_code, 2)
        # The variant with its own command went first and was generated in spite of it.
        self.assertTrue((w.root / "corpus-v2" / "ra-version.txt").is_file())

    def test_variants_file_reads_ra_command(self):
        data = {"format_version": 1, "variants": [
            {"name": "A", "corpus_dir": "a", "ra_command": ["ra-nova", "--profil", "x"]},
            {"name": "B", "corpus_dir": "b"}]}
        first, second = run.parse_variants(data, Path("base"))
        self.assertEqual(first.ra_command, ("ra-nova", "--profil", "x"))
        self.assertIsNone(second.ra_command)

    def test_invalid_ra_command_is_refused(self):
        for bad in ("ra", [], [""], [1], {"a": 1}):
            with self.subTest(bad=bad):
                data = {"format_version": 1, "variants": [
                    {"name": "A", "corpus_dir": "a", "ra_command": bad},
                    {"name": "B", "corpus_dir": "b"}]}
                with self.assertRaisesRegex(run.ConfigError, "ra_command"):
                    run.parse_variants(data, Path("base"))

    def test_a_command_without_a_corpus_is_refused(self):
        with self.assertRaisesRegex(run.ConfigError, "příkaz RA bez korpusu"):
            run.validate_variants([run.VariantSpec("a", ra_command=("ra",)),
                                   run.VariantSpec("b")])

    def test_help_and_readme_describe_the_option(self):
        help_text = " ".join(run.build_parser().format_help().split())
        self.assertIn("ra_command", help_text)
        readme = (Path(run.__file__).parent / "README.md").read_text(encoding="utf-8")
        self.assertIn("ra_command", readme)


class VariantsFileTest(unittest.TestCase):
    def setUp(self):
        self.world = Experiment(cases=1)
        self.addCleanup(self.world.cleanup)

    def test_invalid_variants_file_fails_on_the_command_line(self):
        w = self.world
        variants = w.root / "variants.json"
        variants.write_text(json.dumps({"format_version": 1, "variants": [
            {"name": "korpus A", "corpus_dir": "corpus-a"},
            {"name": "korpus A", "corpus_dir": "corpus-b"}]}), encoding="utf-8")
        err = io.StringIO()
        with redirect_stderr(err):
            code = run.main(["--catalog", str(w.catalog), "--source-repo", str(w.repo),
                             "--output-root", str(w.output), "--export-dir", str(w.export),
                             "--isolation-root", str(w.isolation), "--ra-binary", "ra",
                             "--model", "m", "--cap-seconds", "60", "--cap-steps", "5",
                             "--cap-tokens", "1000", "--seed", "1", "--role", "vyvojar",
                             "--variants", str(variants),
                             "--dry-run"])
        self.assertEqual(code, 2)
        self.assertTrue(re.search("různá jména", err.getvalue()))
        self.assertFalse(w.output.exists())


if __name__ == "__main__":
    unittest.main()
