"""Claude Code adapter: stream parsing, tokens, command, environment, watchdog (U3).

`fixtures/claude_stream_capture.jsonl` is a real transcript, captured once
with Claude Code 2.1.287 (subscription login, model alias `sonnet`):

    claude -p --model sonnet --restricted --tools Read,Grep,Glob
           --strict-mcp-config --setting-sources "" --no-session-persistence
           --output-format stream-json --verbose   (prompt on stdin)

in a temporary directory with one `notes.txt`. Only the working-directory
path and two machine paths were replaced with placeholders; nothing else was
edited. Process tests run `fixtures/fake_agent.py`, never the real CLI.
"""
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path
from unittest import mock

from agents import claude_code
from agents.base import (CAP_STEPS, CAP_TIME, CAP_TOKENS, CONFIG_DIR_NAME, AgentEvent,
                         AgentRunError, Cap, IsolationLeftover, RunRequest, TokenUsage,
                         last_assistant_text)
from agents.claude_code import (ClaudeCodeAdapter, ClaudeCodeSettings, build_command,
                                build_env, parse_lines)

FIXTURES = Path(__file__).resolve().parent / "fixtures"
CAPTURE = FIXTURES / "claude_stream_capture.jsonl"
FAKE_AGENT = str(FIXTURES / "fake_agent.py")
SYNTHETIC_KEY = "SYNTHETIC-SECRET-0000"
# An unrelated secret in the parent environment; it must never reach the agent.
SECRET_ENV = "SEARCH_EVAL_TEST_SECRET"
BIG_CAP = Cap(seconds=60, steps=1000, tokens=10**9)


def line(obj):
    return json.dumps(obj)


def assistant(message_id, content, usage=None, parent=None):
    message = {"id": message_id, "role": "assistant", "content": content}
    if usage is not None:
        message["usage"] = usage
    return line({"type": "assistant", "message": message, "parent_tool_use_id": parent})


def usage(inp, write, read, out):
    return {"input_tokens": inp, "cache_creation_input_tokens": write,
            "cache_read_input_tokens": read, "output_tokens": out}


def tool(tool_id, name="Read", **args):
    return {"type": "tool_use", "id": tool_id, "name": name, "input": args}


def text(value):
    return {"type": "text", "text": value}


def pid_alive(pid):
    """True while the process exists and has not exited (zombies count as gone)."""
    if os.name == "nt":
        import ctypes
        kernel32 = ctypes.windll.kernel32
        handle = kernel32.OpenProcess(0x1000, False, pid)  # QUERY_LIMITED_INFORMATION
        if not handle:
            return False
        code = ctypes.c_ulong()
        kernel32.GetExitCodeProcess(handle, ctypes.byref(code))
        kernel32.CloseHandle(handle)
        return code.value == 259  # STILL_ACTIVE
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    try:
        with open(f"/proc/{pid}/stat", encoding="utf-8") as fh:
            return fh.read().split(")")[-1].split()[0] != "Z"
    except OSError:
        return True


def wait_gone(pid, seconds=10.0):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if not pid_alive(pid):
            return True
        time.sleep(0.1)
    return False


class CapturedTranscriptTest(unittest.TestCase):
    def test_capture_counts_tools_and_tokens(self):
        parsed = parse_lines(CAPTURE.read_text(encoding="utf-8").splitlines())
        self.assertEqual(parsed.steps, 2)
        calls = [c for e in parsed.events for c in e.tool_calls]
        self.assertEqual([c.name for c in calls], ["Glob", "Read"])
        self.assertEqual(calls[1].paths, ("C:\\WORKDIR\\notes.txt",))
        self.assertEqual([c.is_error for c in calls], [False, False])
        # The result event is authoritative: per-message output snapshots sum to 89.
        self.assertEqual(parsed.tokens, TokenUsage(input_uncached=6, cache_write=5573,
                                                   cache_read=10643, output=252))
        self.assertIn('"member": "OrderService.Recalculate"', parsed.final_message)
        self.assertIsNone(parsed.result_error)
        self.assertEqual(parsed.permission_denials, ())
        self.assertEqual({e.role for e in parsed.events}, {"system", "assistant", "user", "result"})


class TokenNormalizationTest(unittest.TestCase):
    def test_cache_write_only_with_repeated_message_counted_once(self):
        u = usage(3, 500, 0, 20)
        parsed = parse_lines([
            assistant("m1", [text("a")], u),
            assistant("m1", [tool("t1")], u),  # second block of the same message
            assistant("m2", [text("done")], usage(1, 100, 0, 5)),
        ])
        self.assertEqual(parsed.tokens, TokenUsage(4, 600, 0, 25))

    def test_cache_read_only(self):
        parsed = parse_lines([assistant("m1", [text("a")], usage(2, 0, 900, 7)),
                              assistant("m2", [text("b")], usage(2, 0, 950, 8))])
        self.assertEqual(parsed.tokens, TokenUsage(4, 0, 1850, 15))

    def test_mixed_with_result_takes_result_once(self):
        parsed = parse_lines([
            assistant("m1", [tool("t1")], usage(2, 5000, 0, 10)),
            assistant("m2", [text("done")], usage(2, 100, 5000, 10)),
            line({"type": "result", "subtype": "success", "is_error": False,
                  "usage": usage(4, 5100, 5000, 80)}),
        ])
        self.assertEqual(parsed.tokens, TokenUsage(4, 5100, 5000, 80))

    def test_no_usage_is_unknown_not_zero(self):
        parsed = parse_lines([assistant("m1", [text("a")])])
        self.assertEqual(parsed.tokens, TokenUsage(None, None, None, None))

    def test_partially_reported_category_is_unknown(self):
        parsed = parse_lines([assistant("m1", [text("a")], {"input_tokens": 5, "output_tokens": 1}),
                              assistant("m2", [text("b")], usage(5, 10, 20, 1))])
        self.assertEqual(parsed.tokens, TokenUsage(10, None, None, 2))


class StepsAndFinalMessageTest(unittest.TestCase):
    def test_subagent_tool_calls_count_as_steps(self):
        parsed = parse_lines([
            assistant("m1", [tool("t1", "Agent", prompt="x")]),
            assistant("s1", [tool("t2", file_path="a.cs")], parent="t1"),
            assistant("s1", [tool("t2", file_path="a.cs")], parent="t1"),  # repeated event
            assistant("s2", [tool("t3", "Grep", pattern="x")], parent="t1"),
            assistant("m2", [text("done")]),
        ])
        self.assertEqual(parsed.steps, 3)

    def test_final_message_is_last_top_level_message(self):
        parsed = parse_lines([
            assistant("m1", [text("průběžná zpráva")]),
            assistant("m1", [tool("t1")]),
            assistant("m2", [text("část 1 "), text("část 2")]),
            assistant("s1", [text("zpráva subagenta")], parent="t1"),
        ])
        self.assertEqual(parsed.final_message, "část 1 část 2")

    def test_last_message_calling_a_tool_is_no_answer(self):
        events = [AgentEvent(role="assistant", message_id="m1", text="mezivýsledek"),
                  AgentEvent(role="assistant", message_id="m2", text="{}",
                             tool_calls=(mock.sentinel.call,))]
        self.assertIsNone(last_assistant_text(events))

    def test_glob_pattern_is_joined_onto_its_path(self):
        parsed = parse_lines([assistant("m1", [
            tool("t1", "Glob", pattern="**/*.cs", path="src"),
        ])])
        glob, = parsed.events[0].tool_calls
        self.assertEqual(glob.paths, ("src", os.path.join("src", "**/*.cs")))


class CommandTest(unittest.TestCase):
    def options(self, argv):
        return {a: argv[i + 1] for i, a in enumerate(argv[:-1]) if a.startswith("--")}

    def test_without_cards_no_shell_no_subagent_no_mcp(self):
        argv = build_command(["claude"], "claude-sonnet-5-5")
        opts = self.options(argv)
        self.assertEqual(argv[:2], ["claude", "-p"])
        self.assertIn("--restricted", argv)
        self.assertEqual(opts["--tools"].split(","), ["Read", "Grep", "Glob"])
        for forbidden in ("Bash", "PowerShell", "Agent", "Task", "WebFetch"):
            self.assertNotIn(forbidden, opts["--tools"])
        self.assertNotIn("--mcp-config", argv)
        self.assertNotIn("--allowedTools", argv)
        self.assertNotIn("--add-dir", argv)
        self.assertIn("--strict-mcp-config", argv)
        self.assertNotIn("--dangerously-skip-permissions", argv)
        self.assertEqual(opts["--setting-sources"], "")
        self.assertEqual(opts["--output-format"], "stream-json")
        self.assertEqual(opts["--model"], "claude-sonnet-5-5")
        self.assertIn("--verbose", argv)

    def test_with_corpus_only_the_corpus_directory_is_added(self):
        argv = build_command(["claude"], "m", corpus_dir="/corpus")
        opts = self.options(argv)
        self.assertEqual(opts["--add-dir"], "/corpus")
        self.assertEqual(opts["--tools"], "Read,Grep,Glob")
        self.assertIn("--strict-mcp-config", argv)
        self.assertNotIn("--mcp-config", argv)
        self.assertNotIn("--allowedTools", argv)


class EnvironmentTest(unittest.TestCase):
    PARENT = {
        "Path": "/usr/bin", "SystemRoot": "C:\\Windows", "CLAUDECODE": "1",
        "CLAUDE_CODE_ENTRYPOINT": "cli", "ClaudeCode_Extra": "x", "CLAUDE_CONFIG_DIR": "/home/u",
        "ANTHROPIC_BASE_URL": "http://proxy.invalid", "ANTHROPIC_API_KEY": SYNTHETIC_KEY,
        "HOME": "/home/u", "SOME_OTHER": "y", SECRET_ENV: SYNTHETIC_KEY,
    }

    def test_only_allow_listed(self):
        env = build_env(self.PARENT, "/iso/cfg")
        self.assertEqual(env["PATH"], "/usr/bin")
        self.assertEqual(env["SYSTEMROOT"], "C:\\Windows")
        claude_vars = {k: v for k, v in env.items() if k.upper().startswith("CLAUDE")}
        self.assertEqual(claude_vars, {"CLAUDE_CONFIG_DIR": str(Path("/iso/cfg")),
                                       "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1"})
        self.assertFalse([k for k in env if k.upper().startswith("ANTHROPIC")])
        self.assertNotIn("SOME_OTHER", env)
        self.assertNotIn(SECRET_ENV, env)
        self.assertNotIn(SYNTHETIC_KEY, env.values())
        self.assertEqual(env["HOME"], str(Path("/iso/cfg")))
        self.assertEqual(env["TEMP"], str(Path("/iso/cfg/tmp")))


class AdapterProcessTest(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="se-u3-"))
        self.addCleanup(shutil.rmtree, self.tmp, ignore_errors=True)
        self.work = self.tmp / "work"
        self.work.mkdir()
        self.corpus = self.tmp / "corpus"
        self.corpus.mkdir()
        self.iso = self.tmp / "iso"
        self.creds = self.tmp / "login" / ".credentials.json"
        self.creds.parent.mkdir()
        self.creds.write_text('{"token": "%s"}' % SYNTHETIC_KEY, encoding="utf-8")

    def adapter(self, *mode):
        settings = ClaudeCodeSettings(model="m", isolation_root=self.iso,
                                      executable=(sys.executable, FAKE_AGENT, *mode),
                                      credentials_source=self.creds)
        return ClaudeCodeAdapter(settings, parent_env={**os.environ, SECRET_ENV: SYNTHETIC_KEY,
                                                       "CLAUDECODE": "1"})

    def config_dir(self):
        return self.iso / CONFIG_DIR_NAME

    def test_child_gets_allow_listed_env_prompt_on_stdin_and_cleanup(self):
        dump = self.tmp / "dump.json"
        prompt = "Najdi příčinu chyby v účtování."
        record = self.adapter("env", str(dump)).run(RunRequest(self.work, prompt, BIG_CAP))
        seen = json.loads(dump.read_text(encoding="utf-8"))
        self.assertEqual(seen["stdin"], prompt)
        self.assertEqual(os.path.normcase(os.path.realpath(seen["cwd"])),
                         os.path.normcase(os.path.realpath(self.work)))
        names = {k.upper() for k in seen["env"]}
        self.assertNotIn("CLAUDECODE", names)
        self.assertNotIn(SECRET_ENV.upper(), names)
        self.assertEqual({n for n in names if n.startswith("CLAUDE")},
                         {"CLAUDE_CONFIG_DIR", "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"})
        allowed = {"PATH", "PATHEXT", "SYSTEMROOT", "SYSTEMDRIVE", "WINDIR", "COMSPEC", "LANG",
                   "LC_ALL", "LC_CTYPE", "TZ", "HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA",
                   "XDG_CONFIG_HOME", "XDG_CACHE_HOME", "XDG_DATA_HOME", "TEMP", "TMP",
                   "TMPDIR", "CLAUDE_CONFIG_DIR", "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"}
        # The OS may add a few variables of its own to every process.
        self.assertLessEqual(names - allowed, {"__CF_USER_TEXT_ENCODING", "LC_CTYPE"})
        self.assertIn(".credentials.json", seen["config_files"])
        self.assertNotIn("mcp-config.json", seen["config_files"])
        self.assertNotIn("--mcp-config", seen["argv"])
        self.assertEqual(record.final_message, "hotovo")
        self.assertIsNone(record.cap_hit)
        self.assertEqual(record.exit_status, 0)
        self.assertFalse(self.config_dir().exists())

    def test_with_corpus_the_corpus_is_added_and_no_mcp_server_configured(self):
        dump = self.tmp / "dump.json"
        record = self.adapter("env", str(dump)).run(
            RunRequest(self.work, "p", BIG_CAP, corpus_dir=self.corpus))
        seen = json.loads(dump.read_text(encoding="utf-8"))
        argv = seen["argv"]
        self.assertEqual(argv[argv.index("--add-dir") + 1], str(self.corpus))
        self.assertIn("--strict-mcp-config", argv)
        self.assertNotIn("--mcp-config", argv)
        self.assertNotIn(SECRET_ENV.upper(), {k.upper() for k in seen["env"]})
        self.assertEqual(record.corpus_dir, str(self.corpus))
        self.assertFalse(self.config_dir().exists())

    def test_silent_agent_with_child_is_killed_on_time_cap(self):
        pidfile = self.tmp / "child.pid"
        cap = Cap(seconds=3, steps=1000, tokens=10**9)
        started = time.monotonic()
        record = self.adapter("spew", "3", str(pidfile)).run(RunRequest(self.work, "p", cap))
        self.assertLess(time.monotonic() - started, 30)
        self.assertEqual(record.cap_hit.kind, CAP_TIME)
        self.assertEqual(record.cap_hit.limit, 3)
        self.assertGreaterEqual(record.cap_hit.observed, 3)
        self.assertAlmostEqual(record.cap_hit.overshoot, record.cap_hit.observed - 3)
        self.assertEqual(record.steps, 3)
        child = int(pidfile.read_text(encoding="utf-8"))
        self.assertTrue(wait_gone(child), f"potomek {child} po ukončení stromu běží dál")
        self.assertFalse(self.config_dir().exists())

    def test_agent_hanging_without_progress_until_time_cap_is_infrastructure(self):
        # A CLI stuck at login or on an unreachable API: init, then nothing.
        adapter = self.adapter("hang")
        record = adapter.run(RunRequest(self.work, "p", Cap(seconds=3, steps=1000,
                                                            tokens=10**9)))
        self.assertEqual(record.cap_hit.kind, CAP_TIME)
        self.assertEqual(record.steps, 0)
        self.assertEqual([e.role for e in record.events], ["system"])
        problem = adapter.infrastructure_problem(record)
        self.assertIsNotNone(problem)
        self.assertIn("časov", problem)
        self.assertFalse(self.config_dir().exists())

    def test_step_cap_kills_and_records_overshoot(self):
        record = self.adapter("spew", "200").run(
            RunRequest(self.work, "p", Cap(seconds=60, steps=5, tokens=10**9)))
        self.assertEqual(record.cap_hit.kind, CAP_STEPS)
        self.assertEqual(record.cap_hit.limit, 5)
        self.assertGreater(record.steps, 5)
        self.assertEqual(record.cap_hit.observed, record.steps)
        self.assertEqual(record.cap_hit.overshoot, record.steps - 5)
        self.assertLess(record.wall_seconds, 30)

    def test_token_cap_kills(self):
        record = self.adapter("tokens", "50").run(
            RunRequest(self.work, "p", Cap(seconds=60, steps=1000, tokens=5000)))
        self.assertEqual(record.cap_hit.kind, CAP_TOKENS)
        self.assertGreater(record.cap_hit.observed, 5000)
        self.assertEqual(record.cap_hit.observed % 1000, 0)

    def test_replayed_capture_through_process(self):
        transcript = self.tmp / "out" / "t.jsonl"
        transcript.parent.mkdir()
        record = self.adapter("replay", str(CAPTURE)).run(
            RunRequest(self.work, "p", BIG_CAP, transcript_path=transcript))
        self.assertEqual(record.steps, 2)
        self.assertEqual(record.tokens.output, 252)
        self.assertIsNone(record.cap_hit)
        self.assertEqual(transcript.read_text(encoding="utf-8"),
                         CAPTURE.read_text(encoding="utf-8"))

    def test_leftover_config_dir_refuses_start(self):
        self.config_dir().mkdir(parents=True)
        dump = self.tmp / "dump.json"
        with self.assertRaises(IsolationLeftover):
            self.adapter("env", str(dump)).run(RunRequest(self.work, "p", BIG_CAP))
        self.assertFalse(dump.exists())
        self.assertTrue(self.config_dir().exists())

    def test_isolation_root_inside_work_tree_refused(self):
        settings = ClaudeCodeSettings(model="m", isolation_root=self.work / "iso",
                                      executable=(sys.executable, FAKE_AGENT, "env", "x"),
                                      credentials_source=self.creds)
        with self.assertRaises(AgentRunError):
            ClaudeCodeAdapter(settings).run(RunRequest(self.work, "p", BIG_CAP))

    def test_config_dir_removed_on_interrupt(self):
        seen = {}

        def interrupted(*_args, **kwargs):
            seen["cfg"] = kwargs["env"]["CLAUDE_CONFIG_DIR"]
            self.assertTrue(Path(seen["cfg"], ".credentials.json").exists())
            raise KeyboardInterrupt

        with mock.patch.object(claude_code, "run_watched", interrupted):
            with self.assertRaises(KeyboardInterrupt):
                self.adapter("env", "x").run(RunRequest(self.work, "p", BIG_CAP))
        self.assertFalse(Path(seen["cfg"]).exists())


if __name__ == "__main__":
    unittest.main()
