"""Judge CLI runner: isolation, output shapes, infrastructure failures (U4).

Process tests run `fixtures/fake_judge.py`, never a real CLI. The two output
shapes below were captured once from the real CLIs with a synthetic prompt
(Claude Code 2.1.296 `--output-format json`, codex-cli 0.159.2 `--json`); only
ids and the answer text were shortened.
"""
import json
import os
import sys
import tempfile
import unittest
from pathlib import Path

from agents.base import INHERITED_ENV, build_isolated_env
from agents.judge_cli import (SHAPE_CLAUDE, SHAPE_CODEX, SIBLING_PLACEHOLDER,
                              JudgeInfrastructureError, _read_claude, _read_codex,
                              run_judge_cli)

FIXTURES = Path(__file__).resolve().parent / "fixtures"
FAKE_JUDGE = str(FIXTURES / "fake_judge.py")
SCRIPT = FIXTURES / "judge" / "three-agree.json"
SYNTHETIC_LOGIN = "SYNTHETIC-LOGIN-0000"
SECRET_ENV = "SEARCH_EVAL_TEST_SECRET"
ALLOWED_ENV = "SEARCH_EVAL_TEST_ALLOWED"

CLAUDE_OUTPUT = json.dumps({
    "type": "result", "subtype": "success", "is_error": False,
    "result": "{\"a\":1}", "modelUsage": {"claude-sonnet-5-5": {"inputTokens": 2}}})
CODEX_OUTPUT = "\n".join(json.dumps(e) for e in (
    {"type": "thread.started", "thread_id": "t"},
    {"type": "turn.started"},
    {"type": "item.completed", "item": {"id": "item_2", "type": "agent_message",
                                        "text": "{\"a\":1}"}},
    {"type": "turn.completed", "usage": {"input_tokens": 1}}))


class OutputShapeTest(unittest.TestCase):
    def test_claude_json_gives_text_and_model(self):
        reply = _read_claude(CLAUDE_OUTPUT, 0, "")
        self.assertEqual((reply.text, reply.model), ("{\"a\":1}", "claude-sonnet-5-5"))

    def test_claude_error_result_is_an_infrastructure_failure(self):
        error = json.dumps({"type": "result", "subtype": "error_during_execution",
                            "is_error": True, "result": "Not logged in"})
        with self.assertRaisesRegex(JudgeInfrastructureError, "Not logged in"):
            _read_claude(error, 0, "")

    def test_claude_nonzero_exit_and_empty_output_fail(self):
        with self.assertRaisesRegex(JudgeInfrastructureError, "kódem 2"):
            _read_claude(CLAUDE_OUTPUT, 2, "boom")
        with self.assertRaisesRegex(JudgeInfrastructureError, "nevypsal JSON"):
            _read_claude("hello", 0, "")

    def test_codex_events_give_the_last_agent_message(self):
        reply = _read_codex(CODEX_OUTPUT, 0, "", "gpt-x")
        self.assertEqual((reply.text, reply.model), ("{\"a\":1}", "gpt-x"))

    def test_codex_shape_decides_not_the_exit_code(self):
        self.assertEqual(_read_codex(CODEX_OUTPUT, 1, "", "gpt-x").text, "{\"a\":1}")
        with self.assertRaises(JudgeInfrastructureError):
            _read_codex("", 0, "", "gpt-x")  # exit 0 without a result
        failed = "\n".join(json.dumps(e) for e in (
            {"type": "turn.started"},
            {"type": "turn.failed", "error": {"message": "model not supported"}}))
        with self.assertRaisesRegex(JudgeInfrastructureError, "model not supported"):
            _read_codex(failed, 1, "", None)


class EnvTest(unittest.TestCase):
    def test_only_allowed_names_pass_and_isolation_variables_win(self):
        parent = {"PATH": "p", "SystemRoot": "r", ALLOWED_ENV: "a", SECRET_ENV: "s",
                  "CLAUDECODE": "1", "HOME": "elsewhere", "TMPDIR": "elsewhere"}
        env = build_isolated_env(parent, "/cfg", passthrough=[ALLOWED_ENV, "HOME"],
                                 fixed={"CODEX_HOME": "/cfg"})
        self.assertNotIn(SECRET_ENV, env)
        self.assertNotIn("CLAUDECODE", env)
        self.assertEqual(env[ALLOWED_ENV.upper() if os.name == "nt" else ALLOWED_ENV], "a")
        self.assertEqual(Path(env["HOME"]), Path("/cfg"))
        self.assertEqual(Path(env["TMPDIR"]), Path("/cfg/tmp"))
        self.assertEqual(env["CODEX_HOME"], "/cfg")
        self.assertIn("PATH", env)


class JudgeProcessTest(unittest.TestCase):
    def setUp(self):
        self.base = Path(tempfile.mkdtemp(prefix="judge-cli-test-"))
        self.addCleanup(self._cleanup)
        self.isolation = self.base / "isolation"
        self.login = self.base / "login.txt"
        self.login.write_text(SYNTHETIC_LOGIN, encoding="utf-8")
        self.dump = self.base / "dump.json"
        self.parent_env = {**{k: v for k, v in os.environ.items()
                              if k.upper() in INHERITED_ENV},
                           SECRET_ENV: "must-not-leak", ALLOWED_ENV: "pass-me",
                           "CLAUDECODE": "1"}

    def _cleanup(self):
        import shutil
        shutil.rmtree(self.base, ignore_errors=True)

    def call(self, shape, *mode, family="claude", **kwargs):
        params = dict(command=(sys.executable, FAKE_JUDGE, shape, *mode), family=family,
                      output_shape={"claude": SHAPE_CLAUDE, "codex": SHAPE_CODEX}[shape],
                      credentials=[], variables=[], prompt="text k posouzení",
                      isolation_root=self.isolation, parent_env=self.parent_env, seconds=60)
        params.update(kwargs)
        return run_judge_cli(**params)

    def test_both_shapes_return_text_and_reported_model(self):
        script = ("script", str(SCRIPT), str(self.base / "counter"))
        prompt = "KONTROLA HODNOTITELE"
        claude = self.call("claude", *script, prompt=prompt)
        codex = self.call("codex", *script, family="openai", prompt=prompt)
        for reply in (claude, codex):
            self.assertEqual(json.loads(reply.text), {"stav": "OK", "obsah": ""})
            self.assertEqual(reply.model, "fake-model-1")

    def test_environment_cwd_and_login_copy_are_isolated(self):
        self.call("claude", "dump", str(self.dump), variables=[ALLOWED_ENV],
                  credentials=[(str(self.login), "auth/login.txt")])
        seen = json.loads(self.dump.read_text(encoding="utf-8"))
        names = {k.upper() for k in seen["env"]}
        self.assertNotIn(SECRET_ENV, names)
        self.assertNotIn("CLAUDECODE", names)
        self.assertEqual(seen["env"].get(ALLOWED_ENV, seen["env"].get(ALLOWED_ENV.upper())),
                         "pass-me")
        allowed = {n.upper() for n in INHERITED_ENV} | {
            "HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "XDG_CONFIG_HOME",
            "XDG_CACHE_HOME", "XDG_DATA_HOME", "TEMP", "TMP", "TMPDIR", "CLAUDE_CONFIG_DIR",
            "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", ALLOWED_ENV.upper()}
        self.assertEqual(sorted(names - allowed), [])
        self.assertEqual(seen["cwd_listing"], [])           # empty working directory
        self.assertEqual(seen["sibling_listing"], [])       # empty sibling, no canary
        self.assertEqual(seen["config_listing"], ["auth/login.txt"])
        self.assertEqual(seen["env"]["HOME"], seen["config_dir"])
        # afterwards the login copy, the configuration and the work directories are gone
        self.assertEqual(list(self.isolation.iterdir()), [])

    def test_codex_family_keeps_its_state_in_the_isolated_directory(self):
        self.call("codex", "dump", str(self.dump), family="openai")
        seen = json.loads(self.dump.read_text(encoding="utf-8"))
        self.assertEqual(seen["env"]["CODEX_HOME"], seen["config_dir"])
        self.assertNotIn("CLAUDE_CONFIG_DIR", seen["env"])

    def test_prompt_goes_through_stdin_with_the_sibling_path_filled_in(self):
        self.call("claude", "dump", str(self.dump),
                  prompt=f"čti {SIBLING_PLACEHOLDER}/canary.txt")
        prompt = json.loads(self.dump.read_text(encoding="utf-8"))["prompt"]
        self.assertNotIn(SIBLING_PLACEHOLDER, prompt)
        self.assertTrue(prompt.endswith("/canary.txt"))

    def test_sibling_files_are_planted_next_to_the_working_directory(self):
        script = self.base / "leak.json"
        script.write_text(json.dumps({"model": "m", "smoke": "leak", "odpovedi": []}),
                          encoding="utf-8")
        reply = self.call("claude", "script", str(script), str(self.base / "c"),
                          prompt="KONTROLA HODNOTITELE",
                          sibling_files={"canary.txt": "CANARY-123"})
        self.assertIn("CANARY-123", reply.text)
        self.assertEqual(list(self.isolation.iterdir()), [])

    def test_failures_are_infrastructure_errors_and_leave_nothing_behind(self):
        cases = {
            "nonzero exit": (("rc1",), "kódem 1"),
            "exit 0 without JSON": (("nojson",), "nevypsal JSON"),
        }
        for name, (mode, message) in cases.items():
            with self.subTest(name), self.assertRaisesRegex(JudgeInfrastructureError, message):
                self.call("claude", *mode)
            self.assertEqual(list(self.isolation.iterdir()), [], name)
        with self.assertRaisesRegex(JudgeInfrastructureError, "nevrátil dokončenou odpověď"):
            self.call("codex", "nojson", family="openai")

    def test_time_limit_is_an_infrastructure_error(self):
        with self.assertRaisesRegex(JudgeInfrastructureError, "časového limitu"):
            self.call("claude", "hang", seconds=1)
        self.assertEqual(list(self.isolation.iterdir()), [])

    def test_missing_login_source_and_executable_are_reported(self):
        with self.assertRaisesRegex(JudgeInfrastructureError, "přihlášení"):
            self.call("claude", "dump", str(self.dump),
                      credentials=[(str(self.base / "není.txt"), "x")])
        with self.assertRaisesRegex(JudgeInfrastructureError, "nebyl nalezen"):
            self.call("claude", command=("neexistujici-judge-cli-xyz",))
        with self.assertRaisesRegex(JudgeInfrastructureError, "relativní"):
            self.call("claude", "dump", str(self.dump),
                      credentials=[(str(self.login), "../ven.txt")])
        self.assertEqual(list(self.isolation.iterdir()), [])


if __name__ == "__main__":
    unittest.main()
