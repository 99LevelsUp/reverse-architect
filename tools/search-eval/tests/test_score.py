"""Scoring of a run: answer only from the last message, matching, one state (U4)."""
import json
import shutil
import tempfile
import unittest
from pathlib import Path

from agents.base import CAP_STEPS, CAP_TIME, CAP_TOKENS, Cap, CapHit, RunRecord, TokenUsage
from agents.claude_code import parse_lines
from audit import AuditFinding, audit_run
from catalog import load_catalog
from score import (FAILURE, SPOILED, SUCCESS, evaluate_run, extract_answer, member_matches)

EXAMPLE = Path(__file__).resolve().parents[1] / "examples" / "catalog.example.json"


def assistant(message_id, *content):
    return json.dumps({"type": "assistant", "parent_tool_use_id": None,
                       "message": {"id": message_id, "role": "assistant",
                                   "content": list(content)}})


def text(value):
    return {"type": "text", "text": value}


def read_call(tool_id, path):
    return {"type": "tool_use", "id": tool_id, "name": "Read", "input": {"file_path": path}}


def tool_result(tool_id, output):
    return json.dumps({"type": "user", "parent_tool_use_id": None,
                       "message": {"role": "user", "content": [
                           {"type": "tool_result", "tool_use_id": tool_id, "content": output}]}})


def answer_block(file, member, hypothesis="Příčina je v podmínce."):
    body = json.dumps({"file": file, "member": member, "hypothesis": hypothesis},
                      ensure_ascii=False)
    return f"Závěr:\n```json\n{body}\n```"


class ScoreTest(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="se-u4-score-"))
        self.addCleanup(shutil.rmtree, self.tmp, ignore_errors=True)
        self.work = self.tmp / "work"
        (self.work / "Src" / "Placeholder").mkdir(parents=True)
        self.catalog = load_catalog(EXAMPLE)
        self.case1, self.case2 = self.catalog.cases

    def record(self, *lines, cap_hit=None):
        parsed = parse_lines(list(lines))
        return RunRecord(
            adapter="claude-code", workdir=str(self.work), corpus_dir=None,
            config_dir=str(self.tmp / "iso" / "agent-config"),
            events=parsed.events, tokens=TokenUsage(), steps=parsed.steps,
            wall_seconds=1.0, cap=Cap(60, 10, 1000), cap_hit=cap_hit, exit_status=0,
            final_message=parsed.final_message)

    def outcome(self, final_text, case=None, **kwargs):
        record = self.record(assistant("m-final", text(final_text)), **kwargs)
        return evaluate_run(record, case or self.case1, audit_run(record))

    def assert_failure(self, outcome, reason):
        self.assertEqual((outcome.state, outcome.failure_reason), (FAILURE, reason))

    # AE2
    def test_example_member_is_success_other_member_failure(self):
        good = self.outcome(answer_block("Src/Placeholder/LoadAction.cs",
                                         "LoadAction.SetScheduledTasksInvalid"))
        self.assertEqual(good.state, SUCCESS)
        self.assertIsNone(good.failure_reason)
        self.assertEqual(good.matched, self.case1.acceptable_locations[0])
        self.assert_failure(self.outcome(answer_block("Src/Placeholder/LoadAction.cs",
                                                      "LoadAction.Reload")), "wrong_location")

    def test_path_separator_and_case_are_normalized(self):
        for path in ("src\\placeholder\\loadaction.cs", "./SRC/Placeholder/LoadAction.cs",
                     str(self.work / "Src" / "Placeholder" / "LoadAction.cs")):
            with self.subTest(path=path):
                outcome = self.outcome(answer_block(path, "LoadAction.SetScheduledTasksInvalid"))
                self.assertEqual(outcome.state, SUCCESS)

    def test_path_outside_tree_or_bare_name_does_not_match(self):
        for path in (str(self.tmp / "Src" / "Placeholder" / "LoadAction.cs"), "LoadAction.cs",
                     "../work/Src/Placeholder/LoadAction.cs"):
            with self.subTest(path=path):
                self.assert_failure(
                    self.outcome(answer_block(path, "LoadAction.SetScheduledTasksInvalid")),
                    "wrong_location")

    def test_member_qualifier_must_agree(self):
        file = "Src/Placeholder/LoadAction.cs"
        for member in ("SetScheduledTasksInvalid", "Placeholder.LoadAction.SetScheduledTasksInvalid",
                       "LoadAction.SetScheduledTasksInvalid()"):
            with self.subTest(member=member):
                self.assertEqual(self.outcome(answer_block(file, member)).state, SUCCESS)
        self.assert_failure(self.outcome(answer_block(file, "OtherAction.SetScheduledTasksInvalid")),
                            "wrong_location")

    def test_overload_marker_is_ignored_members_match_by_qualified_name(self):
        # The catalog's `Rounding.Apply(+1)` is SCIP notation an agent without
        # cards cannot produce; the plain name and any suffix match by name.
        rounding = "Src/Placeholder/Rounding.cs"
        for member in ("Rounding.Apply", "Rounding.Apply(+1)", "Rounding.Apply(+2)",
                       "Rounding.Apply(decimal, int)", "Apply"):
            with self.subTest(member=member):
                self.assertEqual(self.outcome(answer_block(rounding, member),
                                              case=self.case2).state, SUCCESS)
        for member in ("Rounding.Truncate(+1)", "Other.Apply(+1)"):
            with self.subTest(member=member):
                self.assert_failure(self.outcome(answer_block(rounding, member), case=self.case2),
                                    "wrong_location")
        summary = self.outcome(answer_block("Src/Placeholder/SummaryBuilder.cs",
                                            "SummaryBuilder.AddItem(int)"), case=self.case2)
        self.assertEqual(summary.state, SUCCESS)

    def test_answer_in_tool_output_is_not_accepted(self):
        block = answer_block("Src/Placeholder/LoadAction.cs", "LoadAction.SetScheduledTasksInvalid")
        record = self.record(
            assistant("m1", read_call("t1", str(self.work / "notes.txt"))),
            tool_result("t1", block),
            assistant("m2", text("Příčinu jsem nenašel.")))
        self.assert_failure(evaluate_run(record, self.case1, audit_run(record)), "invalid_answer")

    def test_answer_in_intermediate_message_is_not_accepted(self):
        block = answer_block("Src/Placeholder/LoadAction.cs", "LoadAction.SetScheduledTasksInvalid")
        record = self.record(
            assistant("m1", text(block)),
            assistant("m2", read_call("t1", str(self.work / "a.cs"))),
            tool_result("t1", "obsah"),
            assistant("m3", text("Nakonec nevím.")))
        self.assert_failure(evaluate_run(record, self.case1, audit_run(record)), "invalid_answer")
        unfinished = self.record(assistant("m1", text(block)),
                                 assistant("m2", read_call("t1", str(self.work / "a.cs"))))
        self.assert_failure(evaluate_run(unfinished, self.case1, audit_run(unfinished)),
                            "no_answer")

    def test_missing_or_invalid_json_is_invalid_answer(self):
        for message in ("Příčina je v LoadAction.SetScheduledTasksInvalid.",
                        "```json\n{\"file\": \"Src/Placeholder/LoadAction.cs\", \"member\": }\n```",
                        "```json\n{\"file\": \"Src/Placeholder/LoadAction.cs\", "
                        "\"member\": \"LoadAction.SetScheduledTasksInvalid\"}\n```",
                        "```json\n[\"Src/Placeholder/LoadAction.cs\"]\n```"):
            with self.subTest(message=message):
                self.assert_failure(self.outcome(message), "invalid_answer")

    def test_bare_json_object_is_accepted(self):
        message = ('Odpověď: {"file": "Src/Placeholder/LoadAction.cs", '
                   '"member": "LoadAction.SetScheduledTasksInvalid", "hypothesis": "h"}')
        self.assertEqual(self.outcome(message).state, SUCCESS)
        self.assertEqual(extract_answer(message).hypothesis, "h")

    def test_no_assistant_message_is_no_answer(self):
        record = self.record(json.dumps({"type": "system", "subtype": "init"}))
        self.assert_failure(evaluate_run(record, self.case1, ()), "no_answer")

    # AE7
    def test_cap_hit_is_failure_even_with_correct_answer(self):
        block = answer_block("Src/Placeholder/LoadAction.cs", "LoadAction.SetScheduledTasksInvalid")
        for kind, reason in ((CAP_STEPS, "cap_steps"), (CAP_TOKENS, "cap_tokens"),
                             (CAP_TIME, "cap_time")):
            with self.subTest(kind=kind):
                hit = CapHit(kind=kind, limit=10, observed=12, overshoot=2)
                self.assert_failure(self.outcome(block, cap_hit=hit), reason)

    # AE1 end to end
    def test_forbidden_attempt_makes_run_spoiled(self):
        block = answer_block("Src/Placeholder/LoadAction.cs", "LoadAction.SetScheduledTasksInvalid")
        git_call = {"type": "tool_use", "id": "t1", "name": "Bash",
                    "input": {"command": "git log -5"}}
        record = self.record(assistant("m1", git_call),
                             tool_result("t1", "Tool not available"),
                             assistant("m2", text(block)),
                             cap_hit=CapHit(CAP_STEPS, 10, 11, 1))
        outcome = evaluate_run(record, self.case1, audit_run(record))
        self.assertEqual(outcome.state, SPOILED)
        self.assertIsNone(outcome.failure_reason)
        self.assertEqual(sorted(f.kind for f in outcome.spoil_reasons), ["forbidden_tool", "git"])

    def test_every_outcome_has_exactly_one_state(self):
        block = answer_block("Src/Placeholder/LoadAction.cs", "LoadAction.SetScheduledTasksInvalid")
        outcomes = [
            self.outcome(block),
            self.outcome("nic"),
            self.outcome(block, cap_hit=CapHit(CAP_TIME, 10, 11, 1)),
            evaluate_run(self.record(assistant("m", text(block))), self.case1,
                         (AuditFinding("git", "t1", "x"),)),
        ]
        self.assertEqual([o.state for o in outcomes], [SUCCESS, FAILURE, FAILURE, SPOILED])
        for outcome in outcomes:
            self.assertEqual(outcome.failure_reason is not None, outcome.state == FAILURE)
            self.assertEqual(bool(outcome.spoil_reasons), outcome.state == SPOILED)


class GenericMemberMatchTest(unittest.TestCase):
    """Type arguments on the qualifier, written on one side only, do not matter."""

    def test_generic_qualifier_on_either_side_matches(self):
        for answer, catalog_member in (("Repository<T>.Save", "Repository.Save"),
                                       ("Repository.Save", "Repository<T>.Save"),
                                       ("Save", "Repository<T>.Save"),
                                       ("Repository<T>.Save", "Save"),
                                       ("Map<K, List<V>>.Put", "Map.Put"),
                                       ("Ns.Map<K, V>.Put(int)", "Map<K, List<V>>.Put(+1)")):
            with self.subTest(answer=answer, catalog=catalog_member):
                self.assertTrue(member_matches(answer, catalog_member))

    def test_generic_qualifier_still_checks_names(self):
        for answer, catalog_member in (("Other<T>.Save", "Repository<T>.Save"),
                                       ("Repository<T>.Load", "Repository.Save"),
                                       ("Repository<T>", "Repository<T>.Save")):
            with self.subTest(answer=answer, catalog=catalog_member):
                self.assertFalse(member_matches(answer, catalog_member))


if __name__ == "__main__":
    unittest.main()
