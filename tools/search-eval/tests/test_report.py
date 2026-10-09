"""The report: per-case comparison, medians, unknowns, tallies, metadata (U6).

Every expected number below is computed by hand from the scripted results in
`results()`, not taken from the code under test.
"""
import json
import shutil
import tempfile
import unittest
from pathlib import Path

from agents.base import Cap, TokenUsage
from report import (CaseInfo, ExperimentInfo, PreparationCost, RunResult, Tariff, VariantInfo,
                    build_report, render_markdown, write_report)

CAP = Cap(seconds=600, steps=50, tokens=100_000)
REF, CMP = "bez karet", "s kartami"


def run(order, case, variant, state, tokens, steps, seconds, reason=None, cap_hit=None,
        spoil=(), attempt=1):
    return RunResult(order=order, case_id=case, variant=variant, attempt=attempt, state=state,
                     reason=reason, tokens=TokenUsage(*tokens), steps=steps,
                     wall_seconds=seconds, cap_hit=cap_hit, spoil_reasons=tuple(spoil))


STEP_CAP = {"kind": "steps", "limit": 50.0, "observed": 51.0, "overshoot": 1.0}
TIME_CAP = {"kind": "time", "limit": 600.0, "observed": 601.0, "overshoot": 1.0}


def results():
    return [
        # Case A (no flags): reference 1 success of 3, two runs capped; compared 3 of 3.
        run(1, "A", REF, "success", (10, 100, 1000, 50), 10, 100.0),
        run(2, "A", REF, "failure", (20, 200, 2000, 60), 20, 200.0, reason="cap_time",
            cap_hit=TIME_CAP),
        run(3, "A", REF, "failure", (30, 300, 3000, 70), 51, 300.0, reason="cap_steps",
            cap_hit=STEP_CAP),
        run(4, "A", CMP, "success", (5, 50, 500, 25), 5, 50.0),
        run(5, "A", CMP, "success", (7, 70, 700, 35), 7, 70.0),
        run(6, "A", CMP, "success", (9, 90, 900, 45), 9, 90.0),
        # Case B (used for tuning): 2 of 3 each -> tie; one compared run without cache-write.
        run(7, "B", REF, "success", (1, 1, 1, 1), 4, 10.0),
        run(8, "B", REF, "success", (1, 1, 1, 1), 7, 20.0),
        run(9, "B", REF, "failure", (1, 1, 1, 1), 9, 30.0, reason="invalid_answer"),
        run(10, "B", CMP, "spoiled", (1, 1, 1, 1), 2, 5.0,
            spoil=("path_outside: Read: ../x",)),
        run(11, "B", CMP, "success", (2, None, 2, 2), 3, 15.0, attempt=2),
        run(12, "B", CMP, "success", (2, 2, 2, 2), 3, 15.0, attempt=3),
        run(13, "B", CMP, "failure", (2, 2, 2, 2), 3, 15.0, reason="no_answer", attempt=4),
        # Case C (prompt names the location): compared has only 2 clean runs -> incomplete.
        run(14, "C", REF, "success", (1, 1, 1, 1), 1, 1.0),
        run(15, "C", REF, "success", (1, 1, 1, 1), 1, 1.0),
        run(16, "C", REF, "success", (1, 1, 1, 1), 1, 1.0),
        run(17, "C", CMP, "success", (1, 1, 1, 1), 1, 1.0),
        run(18, "C", CMP, "success", (1, 1, 1, 1), 1, 1.0),
        run(19, "C", CMP, "infrastructure", (None, None, None, None), 0, 0.0,
            reason="API vrátilo chybu rate_limit"),
    ]


def info(tariff=None, preparation=()):
    variants = (
        VariantInfo(REF, prompt_template="Zadání {zadani_pripadu}\nJen zdrojáky.",
                    prompt_sha256={"A": "a" * 64}),
        VariantInfo(CMP, corpus_dir="D:/mimo-repo/korpus",
                    corpus_receipt={"corpus_fingerprint": "c" * 64, "ra_version": "ra 1.2.3",
                                    "options": ["--lang", "cs"], "tree_fingerprint": "t" * 64,
                                    "base_revision": "b" * 40, "generation_seconds": 12.5},
                    manifest_schema_version=9, card_format="hlavička chunku",
                    prompt_template="Zadání {zadani_pripadu}\nZdrojáky a korpus.",
                    prompt_sha256={"A": "d" * 64}),
    )
    return ExperimentInfo(
        experiment_id="pokus-test", started_at="02.10.2026 10:00:00", status="completed",
        model="model-test", cap=CAP, seed=7, base_revision="b" * 40, tree_fingerprint="t" * 64,
        variants=variants, role="vyvojar",
        cases=(CaseInfo("A", False, False), CaseInfo("B", True, False),
               CaseInfo("C", False, True)),
        adapter="claude-code", agent_version="2.1.287", tariff=tariff,
        preparation=tuple(preparation))


class ReportNumbersTest(unittest.TestCase):
    def setUp(self):
        self.data = build_report(info(), results())
        self.cases = {c["case_id"]: c for c in self.data["cases"]}

    def test_main_medians_use_cap_convention_success_medians_do_not(self):
        ref, cmp_ = self.cases["A"]["variants"]
        self.assertEqual(ref["medians_all"], {
            # categories are observed for every run: (10, 20, 30) etc.
            "input_uncached": 20, "cache_write": 200, "cache_read": 2000, "output": 60,
            # total 1160 and both capped runs at the token cap 100000
            "tokens_total": 100000,
            # steps 10 and both capped runs at the step cap 50
            "steps": 50,
            # 100 s and both capped runs at the time cap 600 s
            "seconds": 600.0})
        self.assertEqual(ref["medians_success"], {
            "input_uncached": 10, "cache_write": 100, "cache_read": 1000, "output": 50,
            "tokens_total": 1160, "steps": 10, "seconds": 100.0})
        self.assertEqual(cmp_["medians_all"], {
            "input_uncached": 7, "cache_write": 70, "cache_read": 700, "output": 35,
            "tokens_total": 812, "steps": 7, "seconds": 70.0})
        self.assertEqual((ref["successes"], ref["clean_runs"], ref["capped_runs"]), (1, 3, 2))
        self.assertEqual(self.cases["A"]["difference_compared_minus_reference"], {
            "input_uncached": -13, "cache_write": -130, "cache_read": -1300, "output": -25,
            "tokens_total": -99188, "steps": -43, "seconds": -530.0})

    def test_even_count_success_median_is_the_mean_of_the_middle(self):
        ref, _ = self.cases["B"]["variants"]
        self.assertEqual(ref["medians_success"]["steps"], 5.5)  # successes with 4 and 7 steps
        self.assertEqual(ref["medians_success"]["seconds"], 15.0)

    def test_unknown_tokens_stay_unknown_never_zero(self):
        _, cmp_ = self.cases["B"]["variants"]
        self.assertEqual(cmp_["medians_all"]["cache_write"], "unknown")
        self.assertEqual(cmp_["medians_all"]["tokens_total"], "unknown")
        self.assertEqual(cmp_["medians_all"]["input_uncached"], 2)
        self.assertEqual(self.cases["B"]["difference_compared_minus_reference"]["cache_write"],
                         "unknown")
        markdown = render_markdown(self.data)
        self.assertIn("| zápis do cache | 1 | neznámý | neznámý |", markdown)
        self.assertIn("| tokeny celkem | 4 | neznámý | neznámý |", markdown)

    def test_win_tie_and_incomplete(self):
        self.assertEqual(self.cases["A"]["result"], "win")
        self.assertEqual(self.cases["B"]["result"], "tie")
        self.assertEqual(self.cases["C"]["result"], "incomplete")
        self.assertEqual(self.data["summary"]["all_cases"],
                         {"win": 1, "loss": 0, "tie": 1, "incomplete": 1})
        self.assertEqual(self.data["summary"]["unflagged_cases"],
                         {"win": 1, "loss": 0, "tie": 0, "incomplete": 0})

    def test_items_of_another_kind_stay_out_of_the_navigation_report(self):
        base = info()
        mixed = ExperimentInfo(**{**base.__dict__, "cases": (*base.cases,
                                                             CaseInfo("Q", False, False, "otazka"))})
        data = build_report(mixed, results())
        self.assertEqual([c["case_id"] for c in data["cases"]], ["A", "B", "C"])
        self.assertEqual([c["id"] for c in data["experiment"]["cases"]], ["A", "B", "C"])
        self.assertEqual(data["summary"], build_report(base, results())["summary"])
        self.assertNotIn("Případ Q", render_markdown(data))
        self.assertEqual(data["not_claimed"], build_report(base, results())["not_claimed"])

    def test_loss_when_compared_has_fewer_successes(self):
        swapped = [RunResult(**{**r.__dict__, "variant": CMP if r.variant == REF else REF})
                   for r in results() if r.case_id == "A"]
        data = build_report(info(), swapped)
        self.assertEqual(data["cases"][0]["result"], "loss")

    def test_spoiled_and_infrastructure_runs_are_listed_but_not_counted(self):
        _, cmp_b = self.cases["B"]["variants"]
        self.assertEqual((cmp_b["clean_runs"], cmp_b["successes"], cmp_b["spoiled_attempts"]),
                         (3, 2, 1))
        self.assertEqual(self.data["spoiled_attempts"],
                         [{"order": 10, "case_id": "B", "variant": CMP, "attempt": 1,
                           "reasons": ["path_outside: Read: ../x"]}])
        _, cmp_c = self.cases["C"]["variants"]
        self.assertEqual(cmp_c["clean_runs"], 2)
        self.assertEqual([e["order"] for e in self.data["infrastructure_errors"]], [19])

    def test_flagged_cases_are_listed_separately(self):
        summary = self.data["summary"]
        self.assertEqual(summary["used_for_card_tuning"], ["B"])
        self.assertEqual(summary["prompt_names_location"], ["C"])
        markdown = render_markdown(self.data)
        plain, flagged = markdown.split("### Označené případy", 1)
        plain = plain.split("### Případy bez označení", 1)[1]
        self.assertIn("| A |", plain)
        self.assertNotIn("| B |", plain)
        self.assertIn("| B | ladění |", flagged.split("####")[0])
        self.assertIn("| C | jmenuje místo |", flagged.split("####")[0])

    def test_cache_read_is_listed_in_run_order(self):
        rows = self.data["cache_read_by_order"]
        self.assertEqual([r["order"] for r in rows], list(range(1, 20)))
        self.assertEqual([r["cache_read"] for r in rows[:6]], [1000, 2000, 3000, 500, 700, 900])
        self.assertIsNone(rows[-1]["cache_read"])
        self.assertIn("| 19 | C | s kartami | chyba infrastruktury | neznámý |",
                      render_markdown(self.data))


class ReportContentTest(unittest.TestCase):
    def test_metadata_needed_to_repeat_the_experiment(self):
        markdown = render_markdown(build_report(info(), results()))
        for expected in ("model-test", "verze CLI agenta: 2.1.287",
                         "čas 600 s, kroky 50, tokeny 100000", "semínko 7, souběh 1",
                         "Základní revize: " + "b" * 40, "otisk stromu: " + "t" * 64,
                         "otisk korpusu: " + "c" * 64, "verze RA: ra 1.2.3; volby: --lang cs",
                         "verze schématu manifestu: 9; formát karet: hlavička chunku",
                         "Korpus: žádný", "Korpus: D:/mimo-repo/korpus",
                         "Zadání {zadani_pripadu}\nJen zdrojáky.",
                         "Zadání {zadani_pripadu}\nZdrojáky a korpus.",
                         "| A | " + "d" * 64 + " |",
                         "## Co toto měření netvrdí", "ne lidi",
                         "bez vlivu agentova hledání"):
            with self.subTest(expected=expected):
                self.assertIn(expected, markdown)

    def test_preparation_costs_are_separate_and_not_in_run_medians(self):
        huge = TokenUsage(10**6, 10**6, 10**6, 10**6)
        data = build_report(info(preparation=(
            PreparationCost("Export stromu základní revize", 3.25),
            PreparationCost("Korpus varianty s kartami: generování", None,
                            note="účtenka čas neuvádí"),
            PreparationCost("Kontrola nástroje (s kartami)", 8.0, huge))), results())
        self.assertEqual(data["preparation"][0], {"step": "Export stromu základní revize",
                                                  "seconds": 3.2, "tokens": None, "note": ""})
        self.assertEqual(data["preparation"][1]["seconds"], "unknown")
        self.assertEqual(data["cases"][0]["variants"][1]["medians_all"]["tokens_total"], 812)
        markdown = render_markdown(data)
        section = markdown.split("## Jednorázové náklady přípravy", 1)[1].split("##", 1)[0]
        self.assertIn("| Export stromu základní revize | 3,2 | — | — |", section)
        self.assertIn("| Korpus varianty s kartami: generování | neznámý | — | účtenka čas "
                      "neuvádí |", section)
        self.assertIn("| Kontrola nástroje (s kartami) | 8 | 4000000 | — |", section)

    def test_money_only_with_a_recorded_tariff(self):
        without = build_report(info(), results())
        self.assertIsNone(without["money"])
        self.assertIn("tarif není zapsaný", render_markdown(without))
        tariff = Tariff("USD", {"input_uncached": 3, "cache_write": 3.75, "cache_read": 0.3,
                                "output": 15}, source="ceník 10/2026")
        runs = [run(i, "A", v, "success", (1000, 10000, 100000, 1000), 1, 1.0)
                for i, v in ((1, REF), (2, REF), (3, REF), (4, CMP), (5, CMP))]
        runs.append(run(6, "A", CMP, "success", (1000, None, 100000, 1000), 1, 1.0))
        data = build_report(info(tariff=tariff), runs)
        ref, cmp_ = data["cases"][0]["variants"]
        # 1000*3 + 10000*3.75 + 100000*0.3 + 1000*15 = 85500 per million
        self.assertEqual(ref["cost_median_all"], 0.0855)
        self.assertEqual(cmp_["cost_median_all"], "unknown")
        self.assertEqual(data["money"]["total_all_runs"], "unknown")
        self.assertIn("| A | 0,0855 | neznámý | 0,0855 | neznámý |", render_markdown(data))


class ReportRoleTest(unittest.TestCase):
    def test_role_is_in_the_data_and_the_markdown(self):
        for role, text in (("vyvojar", "čtenář se zdrojáky"),
                           ("podpora", "čtenář bez zdrojáků, jen korpus")):
            with self.subTest(role=role):
                base = info()
                data = build_report(ExperimentInfo(**{**base.__dict__, "role": role}), results())
                self.assertEqual(data["experiment"]["role"], role)
                self.assertIn(f"- Role: {role} ({text})", render_markdown(data))

    def test_items_set_aside_by_the_role_are_listed_with_the_reason(self):
        base = info()
        support = ExperimentInfo(**{**base.__dict__, "role": "podpora", "cases": (),
                                    "not_measured": ("A", "B")})
        data = build_report(support, [])
        self.assertEqual(data["experiment"]["not_measured"],
                         [{"id": "A", "reason": "v roli podpora se neměří"},
                          {"id": "B", "reason": "v roli podpora se neměří"}])
        self.assertEqual(data["cases"], [])
        markdown = render_markdown(data)
        self.assertIn("- Neměřený případ A: v roli podpora se neměří", markdown)
        self.assertIn("bez zdrojáků, jen s korpusem RA", markdown)
        self.assertEqual(build_report(base, results())["experiment"]["not_measured"], [])


class ReportFilesTest(unittest.TestCase):
    def test_json_and_markdown_carry_the_same_numbers(self):
        tmp = Path(tempfile.mkdtemp(prefix="se-u6-"))
        self.addCleanup(shutil.rmtree, tmp, ignore_errors=True)
        write_report(tmp, info(), results())
        data = json.loads((tmp / "report.json").read_text(encoding="utf-8"))
        markdown = (tmp / "report.md").read_text(encoding="utf-8")
        ref, cmp_ = data["cases"][0]["variants"]
        self.assertEqual((ref["medians_all"]["tokens_total"], cmp_["medians_all"]["tokens_total"],
                          ref["medians_success"]["tokens_total"]), (100000, 812, 1160))
        self.assertIn("| tokeny celkem | 100000 | 812 | -99188 | 1160 | 812 |", markdown)
        self.assertIn("| kroky | 50 | 7 | -43 | 10 | 7 |", markdown)
        self.assertIn("| čas (s) | 600 | 70 | -530 | 100 | 70 |", markdown)
        self.assertIn("ukončené stropem 2", markdown)
        self.assertIn("| všechny případy | 1 | 0 | 1 | 1 |", markdown)
        self.assertIn("| bez označených případů | 1 | 0 | 0 | 0 |", markdown)
        self.assertEqual(data["summary"]["all_cases"],
                         {"win": 1, "loss": 0, "tie": 1, "incomplete": 1})


if __name__ == "__main__":
    unittest.main()
