"""Tool check before the experiment (KTD10), run through the real adapter (U5).

Every case runs `fixtures/fake_agent.py` as the agent process via
`ClaudeCodeAdapter`, so the check sees what it would see from Claude Code:
argv, the isolated configuration directory, stream-json on stdout.
"""
import re
import shutil
import sys
import tempfile
import unittest
from pathlib import Path

import preflight
from agents.base import Cap
from agents.claude_code import ClaudeCodeAdapter, ClaudeCodeSettings
from agents.scripted import ScriptedAdapter
from preflight import (CHECK_LOGIN, CHECK_OUTSIDE, CHECK_TELEMETRY, CHECK_TOOLS, CHECK_USER,
                       PreflightFailed, PreflightTarget, require_ok, run_preflight)
from tests.experiment_fixture import FAKE_AGENT, SYNTHETIC_KEY, fake_agent_env, preflight_run

CAP = Cap(seconds=60, steps=50, tokens=10**6)
ALL_CHECKS = {CHECK_OUTSIDE, CHECK_USER, CHECK_TOOLS, CHECK_TELEMETRY, CHECK_LOGIN}


class PreflightTest(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="se-u5-pre-"))
        self.addCleanup(shutil.rmtree, self.tmp, ignore_errors=True)
        self.creds = self.tmp / "login" / ".credentials.json"
        self.creds.parent.mkdir()
        self.creds.write_text('{"token": "%s"}' % SYNTHETIC_KEY, encoding="utf-8")
        self.count = 0

    def check(self, behaviour, with_corpus=False):
        """Tool check against the fake agent playing `behaviour`."""
        self.count += 1
        settings = ClaudeCodeSettings(model="m", isolation_root=self.tmp / "iso",
                                      executable=(sys.executable, FAKE_AGENT,
                                                  "preflight", behaviour),
                                      credentials_source=self.creds)
        adapter = ClaudeCodeAdapter(settings, parent_env=fake_agent_env())
        target = PreflightTarget(name="s kartami" if with_corpus else "bez karet",
                                 slug=f"v{self.count}", with_corpus=with_corpus)
        result = run_preflight(adapter, target, self.tmp / "preflight", CAP, token="T0KEN")
        self.assertFalse((self.tmp / "iso" / "agent-config").exists())
        return result

    def failed(self, result):
        return sorted(c.name for c in result.checks if not c.ok)

    def test_honest_agent_passes_without_and_with_corpus(self):
        for with_corpus in (False, True):
            with self.subTest(with_corpus=with_corpus):
                result = self.check("good", with_corpus)
                self.assertTrue(result.ok, [c for c in result.checks if not c.ok])
                self.assertEqual({c.name for c in result.checks}, ALL_CHECKS)
                self.assertEqual(result.agent_version, "0.0.0-fake")
                require_ok([result])

    def test_agent_allowed_to_read_outside_the_tree_fails(self):
        result = self.check("leak-outside")
        self.assertEqual(self.failed(result), [CHECK_OUTSIDE])
        with self.assertRaises(PreflightFailed) as ctx:
            require_ok([result])
        self.assertIn("chyba infrastruktury", str(ctx.exception))
        self.assertIn(CHECK_OUTSIDE, str(ctx.exception))

    def test_outside_read_never_attempted_is_not_a_pass(self):
        self.assertEqual(self.failed(self.check("skip-outside")), [CHECK_OUTSIDE])

    def test_outside_read_without_a_tool_result_is_not_a_pass(self):
        result = self.check("unresolved-outside")
        self.assertEqual(self.failed(result), [CHECK_OUTSIDE])
        detail = next(c.detail for c in result.checks if c.name == CHECK_OUTSIDE)
        self.assertIn("bez výsledku", detail)

    def test_successful_empty_outside_read_is_not_a_pass(self):
        result = self.check("empty-read-outside")
        self.assertEqual(self.failed(result), [CHECK_OUTSIDE])
        detail = next(c.detail for c in result.checks if c.name == CHECK_OUTSIDE)
        self.assertIn("uspěl", detail)

    def test_outside_read_failing_on_its_arguments_is_not_a_pass(self):
        result = self.check("argerror-outside")
        self.assertEqual(self.failed(result), [CHECK_OUTSIDE])
        detail = next(c.detail for c in result.checks if c.name == CHECK_OUTSIDE)
        self.assertIn("ne odepřením", detail)

    def test_other_tool_succeeding_on_the_outside_file_is_not_a_pass(self):
        result = self.check("grep-outside")
        self.assertEqual(self.failed(result), [CHECK_OUTSIDE])
        detail = next(c.detail for c in result.checks if c.name == CHECK_OUTSIDE)
        self.assertIn("Grep", detail)

    def test_tool_check_stopped_by_the_cap_after_progress_fails_as_infrastructure(self):
        result = self.check("cap-after-progress")
        self.assertEqual(self.failed(result), [CHECK_LOGIN])
        detail = next(c.detail for c in result.checks if c.name == CHECK_LOGIN)
        self.assertIn("strop", detail)
        with self.assertRaises(PreflightFailed) as ctx:
            require_ok([result])
        self.assertIn("chyba infrastruktury", str(ctx.exception))

    def test_user_instructions_followed_fail(self):
        self.assertEqual(self.failed(self.check("follow-user")), [CHECK_USER])

    def test_extra_tool_or_mcp_server_fails(self):
        extra_tool = self.check("extra-tool")
        self.assertEqual(self.failed(extra_tool), [CHECK_TOOLS])
        detail = next(c.detail for c in extra_tool.checks if c.name == CHECK_TOOLS)
        self.assertIn("Bash", detail)
        # No variant has an MCP server, the one with a corpus neither.
        extra_mcp = self.check("extra-mcp", with_corpus=True)
        self.assertEqual(self.failed(extra_mcp), [CHECK_TOOLS])
        detail = next(c.detail for c in extra_mcp.checks if c.name == CHECK_TOOLS)
        self.assertIn("mcp__other__search", detail)
        self.assertIn("servery MCP: other, očekáváno: žádné", detail)

    def test_missing_token_telemetry_fails(self):
        self.assertEqual(self.failed(self.check("no-usage")), [CHECK_TELEMETRY])

    def test_login_failure_fails(self):
        result = self.check("auth-error")
        self.assertIn(CHECK_LOGIN, self.failed(result))
        detail = next(c.detail for c in result.checks if c.name == CHECK_LOGIN)
        self.assertIn("/login", detail)

    def test_unsupported_flag_fails_as_infrastructure(self):
        result = self.check("bad-flag")
        self.assertIn(CHECK_LOGIN, self.failed(result))
        self.assertIn(CHECK_TELEMETRY, self.failed(result))
        detail = next(c.detail for c in result.checks if c.name == CHECK_LOGIN)
        self.assertIn("unknown option", detail)

    def test_marker_files_are_planted_outside_the_work_tree(self):
        self.check("good")
        root = self.tmp / "preflight" / "v1"
        marker = (root / "outside" / preflight.MARKER_FILE).read_text(encoding="utf-8")
        self.assertIn("PREFLIGHT-OUTSIDE-T0KEN", marker)
        self.assertEqual(sorted(p.name for p in (root / "work").iterdir()), ["README.txt"])


class SupportRolePreflightTest(unittest.TestCase):
    """The tool check of the support role: an empty work directory, no source file (KTD9)."""

    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="se-u12-pre-"))
        self.addCleanup(shutil.rmtree, self.tmp, ignore_errors=True)

    def check(self, with_sources):
        adapter = ScriptedAdapter(preflight_run, self.tmp / "iso")
        target = PreflightTarget(name="korpus", slug="v1", with_corpus=True,
                                 with_sources=with_sources)
        result = run_preflight(adapter, target, self.tmp / "preflight", CAP, token="T0KEN")
        return result, adapter.requests[0]

    def test_no_work_directory_with_sources_is_created(self):
        result, request = self.check(with_sources=False)
        root = self.tmp / "preflight" / "v1"
        self.assertTrue(result.ok, [c for c in result.checks if not c.ok])
        self.assertFalse((root / "work").exists())
        self.assertEqual(list(request.workdir.iterdir()), [])
        self.assertEqual(request.workdir.parent, root)
        self.assertEqual(request.corpus_dir, root / "corpus")
        self.assertNotIn("README.txt", request.prompt.split(preflight.OUTSIDE_LABEL)[0])
        self.assertIn(str(root / "corpus" / "index.md"), request.prompt)

    def test_with_sources_keeps_the_work_directory_with_the_readme(self):
        _, request = self.check(with_sources=True)
        self.assertEqual(sorted(p.name for p in request.workdir.iterdir()), ["README.txt"])
        self.assertEqual(request.workdir.name, "work")

    def test_support_role_needs_a_corpus(self):
        adapter = ScriptedAdapter(preflight_run, self.tmp / "iso")
        target = PreflightTarget(name="x", slug="v1", with_corpus=False, with_sources=False)
        with self.assertRaises(ValueError):
            run_preflight(adapter, target, self.tmp / "preflight", CAP)
        self.assertEqual(adapter.requests, [])


class SharedConstantsTest(unittest.TestCase):
    def test_fake_agent_duplicates_match(self):
        source = Path(FAKE_AGENT).read_text(encoding="utf-8")
        for name in ("PREFLIGHT_TITLE", "OUTSIDE_LABEL"):
            with self.subTest(name=name):
                value = re.search(rf'^{name} = "(.*)"$', source, re.MULTILINE).group(1)
                self.assertEqual(value, getattr(preflight, name))


if __name__ == "__main__":
    unittest.main()
