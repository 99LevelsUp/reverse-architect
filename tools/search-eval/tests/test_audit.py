"""Audit of normalized run events: forbidden paths, git, config dir, tools (U4)."""
import json
import os
import shutil
import tempfile
import unittest
from pathlib import Path

from agents.base import Cap, RunRecord, TokenUsage
from agents.claude_code import parse_lines
from audit import CONFIG_DIR_ACCESS, FORBIDDEN_TOOL, GIT, PATH_OUTSIDE, audit_run


def tool_line(*calls):
    content = [{"type": "tool_use", "id": f"t{i}", "name": name, "input": args}
               for i, (name, args) in enumerate(calls)]
    return json.dumps({"type": "assistant", "parent_tool_use_id": None,
                       "message": {"id": "m1", "role": "assistant", "content": content}})


class AuditTest(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="se-u4-audit-"))
        self.addCleanup(shutil.rmtree, self.tmp, ignore_errors=True)
        self.work = self.tmp / "work"
        self.corpus = self.tmp / "corpus"
        self.config = self.tmp / "iso" / "agent-config"
        for path in (self.work / "src", self.corpus, self.config):
            path.mkdir(parents=True)

    def record(self, *calls, corpus=False):
        parsed = parse_lines([tool_line(*calls)])
        return RunRecord(
            adapter="claude-code", workdir=str(self.work),
            corpus_dir=str(self.corpus) if corpus else None, config_dir=str(self.config),
            events=parsed.events,
            tokens=TokenUsage(), steps=parsed.steps, wall_seconds=1.0,
            cap=Cap(60, 100, 10**6), cap_hit=None, exit_status=0, final_message=None)

    def kinds(self, record):
        return sorted(f.kind for f in audit_run(record))

    def test_reads_inside_work_tree_are_clean(self):
        record = self.record(
            ("Read", {"file_path": str(self.work / "src" / "A.cs")}),
            ("Glob", {"pattern": "**/*.cs"}),
            ("Glob", {"pattern": "src/**/*.cs", "path": str(self.work)}),
            ("Grep", {"pattern": "git log", "path": "src", "glob": "*.cs"}),
        )
        self.assertEqual(self.kinds(record), [])

    def test_read_outside_tree_spoils(self):
        outside = self.tmp / "elsewhere" / "notes.txt"
        self.assertEqual(self.kinds(self.record(("Read", {"file_path": str(outside)}))),
                         [PATH_OUTSIDE])

    def test_glob_climbing_out_of_tree_spoils(self):
        record = self.record(("Glob", {"pattern": "../../**/*.json"}))
        self.assertEqual(self.kinds(record), [PATH_OUTSIDE])

    def test_glob_chars_with_dot_dot_cannot_hide_a_read_outside(self):
        # The literal prefix of these paths stops at the glob character and
        # lies inside the tree; the path itself climbs out of it.
        for call, kind in (
                (("Read", {"file_path": str(self.work / "{x}" / ".." / ".." / "catalog.json")}),
                 PATH_OUTSIDE),
                (("Read", {"file_path": os.path.join(str(self.work), "a*b", "..", "..", "iso",
                                                     "agent-config", ".credentials.json")}),
                 CONFIG_DIR_ACCESS),
                (("Glob", {"pattern": "src/**/../../../x/*.json"}), PATH_OUTSIDE),
                (("Glob", {"pattern": "src/*/../../../iso/agent-config/*"}), CONFIG_DIR_ACCESS),
                (("Grep", {"pattern": "x", "path": "src/[ab]/../../.."}), PATH_OUTSIDE)):
            with self.subTest(call=call):
                self.assertEqual(self.kinds(self.record(call)), [kind])

    def test_glob_chars_with_dot_dot_inside_the_tree_stay_clean(self):
        record = self.record(("Glob", {"pattern": "src/**/../*.cs"}),
                             ("Read", {"file_path": str(self.work / "src" / "{x}" / ".." / "A.cs")}))
        self.assertEqual(self.kinds(record), [])

    def test_corpus_is_allowed_only_when_the_variant_has_one(self):
        card = str(self.corpus / "cards" / "A.md")
        self.assertEqual(self.kinds(self.record(("Read", {"file_path": card}))), [PATH_OUTSIDE])
        self.assertEqual(self.kinds(self.record(("Read", {"file_path": card}),
                                                corpus=True)), [])

    def test_reads_and_globs_inside_the_corpus_are_clean(self):
        record = self.record(("Read", {"file_path": str(self.corpus / "index.md")}),
                             ("Glob", {"pattern": str(self.corpus / "cards" / "*.md")}),
                             corpus=True)
        self.assertEqual(self.kinds(record), [])

    def test_git_log_and_git_directory_spoil(self):
        record = self.record(("Bash", {"command": "git log --oneline -5"}))
        self.assertEqual(self.kinds(record), [FORBIDDEN_TOOL, GIT])
        record = self.record(("Read", {"file_path": str(self.work / ".git" / "HEAD")}))
        self.assertEqual(self.kinds(record), [GIT])

    def test_config_dir_access_spoils(self):
        record = self.record(("Read", {"file_path": str(self.config / ".credentials.json")}))
        self.assertEqual(self.kinds(record), [CONFIG_DIR_ACCESS])

    def test_tools_outside_allowed_set_spoil(self):
        # No variant has an MCP server, so any MCP tool is forbidden too.
        record = self.record(("Agent", {"prompt": "x"}),
                             ("mcp__docs__create_document", {"title": "x"}),
                             ("mcp__other__search", {"query": "x"}),
                             corpus=True)
        findings = audit_run(record)
        self.assertEqual([f.kind for f in findings], [FORBIDDEN_TOOL] * 3)
        self.assertEqual([f.tool_call_id for f in findings], ["t0", "t1", "t2"])

    @unittest.skipUnless(os.name == "nt", "case-insensitive paths are a Windows property")
    def test_work_tree_path_in_other_case_is_inside(self):
        upper = str(self.work / "src" / "A.cs").upper()
        self.assertEqual(self.kinds(self.record(("Read", {"file_path": upper}))), [])


if __name__ == "__main__":
    unittest.main()
