"""Claude Code adapter: isolated `claude -p` run with a cap and normalized events (U3).

Command (KTD2, KTD3), checked against Claude Code 2.1.287 (`claude --help`;
all flags below exist there):

    claude -p --model <model> --restricted --tools Read,Grep,Glob
           --setting-sources "" --strict-mcp-config --no-session-persistence
           --permission-prompts none
           [--add-dir <corpus>]                                  (with cards)
           --output-format stream-json --verbose

* `--restricted` removes the shell and code-running tools and confines file
  tools to the working directories; `--tools Read,Grep,Glob` leaves no
  subagent tool and no other built-in tool.
* `--strict-mcp-config` without `--mcp-config` means "no MCP server at all"
  in both variants, so no server configured for the user or the machine
  leaks into a run. With cards the corpus is read as files (`--add-dir`).
* `--permission-prompts none` denies anything that would still need an
  approval instead of leaving the run waiting for one.
* The prompt goes through stdin.
* `--bare` is deliberately not used: it disables OAuth, and the agent logs in
  with a copy of the subscription login (Key Decisions).

Stream-json shape (verified on a captured transcript,
`tests/fixtures/claude_stream_capture.jsonl`): one `system/init` event, then
`assistant` events (`message.id`, `message.content` blocks of type `text` /
`tool_use`, `message.usage`, `parent_tool_use_id`), `user` events with
`tool_result` blocks, and a final `result` event with the run's `usage`,
`permission_denials`, `is_error` and `subtype`.

Token accounting: an `assistant` event carries the usage of its whole model
message and is repeated for every content block, so per-message usage is
de-duplicated by `message.id`. Its `output_tokens` is a mid-stream snapshot
(the capture: 89 summed over messages vs. 252 in `result`), so the `result`
event's `usage` is authoritative. Only a run killed before `result` falls back
to the de-duplicated sum, whose output count is then a lower bound.
"""
from __future__ import annotations

import json
import os
import re
import shutil
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Mapping, Sequence

from agents.base import (AgentAdapter, AgentEvent, AgentInit, AgentRunError, RunRecord,
                         RunRequest, TokenUsage, ToolCall, ToolResult, build_isolated_env,
                         generic_infrastructure_problem, isolated_config_dir,
                         last_assistant_text, resolve_executable, run_watched,
                         seed_config_files, sum_usages)

ALLOWED_BUILTIN_TOOLS = ("Read", "Grep", "Glob")
CREDENTIALS_FILE = ".credentials.json"

_PATH_ARG_KEYS = ("file_path", "path", "notebook_path")


# --- tokens ------------------------------------------------------------------

def normalize_claude_usage(usage: Mapping[str, Any] | None) -> TokenUsage:
    """Map one Claude `usage` object to the KTD5 categories; absent key = unknown.

    Claude reports the categories separately, so this is a rename: no
    category contains another and nothing is subtracted.
    """
    if not isinstance(usage, Mapping):
        return TokenUsage()

    def get(key: str) -> int | None:
        value = usage.get(key)
        return value if isinstance(value, int) and not isinstance(value, bool) else None

    return TokenUsage(input_uncached=get("input_tokens"),
                      cache_write=get("cache_creation_input_tokens"),
                      cache_read=get("cache_read_input_tokens"),
                      output=get("output_tokens"))


# --- stream parsing ----------------------------------------------------------

@dataclass
class ParsedRun:
    """What the stream yields once the process is gone."""

    events: tuple[AgentEvent, ...]
    tokens: TokenUsage
    steps: int
    final_message: str | None
    result_error: str | None
    permission_denials: tuple[dict[str, Any], ...]
    init: AgentInit | None = None
    result_text: str | None = None
    api_error: str | None = None


@dataclass
class ClaudeStreamParser:
    """Incremental reader of Claude Code stream-json (see the module docstring)."""

    raw: list[dict[str, Any]] = field(default_factory=list)
    _tool_ids: set[str] = field(default_factory=set)
    _usage_by_message: dict[str, TokenUsage] = field(default_factory=dict)
    _anonymous: int = 0
    _result: dict[str, Any] | None = None
    _init: dict[str, Any] | None = None
    _api_error: str | None = None

    def feed(self, obj: dict[str, Any]) -> None:
        self.raw.append(obj)
        kind = obj.get("type")
        if kind == "system" and obj.get("subtype") == "init" and self._init is None:
            self._init = obj
        elif kind == "assistant":
            if isinstance(obj.get("error"), str):
                self._api_error = obj["error"]
            message = obj.get("message") or {}
            message_id = message.get("id")
            if not isinstance(message_id, str):
                self._anonymous += 1
                message_id = f"<anonymous {self._anonymous}>"
            if "usage" in message:
                self._usage_by_message[message_id] = normalize_claude_usage(message.get("usage"))
            for block in _blocks(message):
                if block.get("type") == "tool_use":
                    self._tool_ids.add(str(block.get("id") or f"<tool {len(self._tool_ids)}>"))
        elif kind == "result":
            self._result = obj

    @property
    def steps(self) -> int:
        return len(self._tool_ids)

    def tokens(self) -> TokenUsage:
        if self._result is not None and isinstance(self._result.get("usage"), Mapping):
            return normalize_claude_usage(self._result["usage"])
        return sum_usages(list(self._usage_by_message.values()))

    def tokens_total(self) -> int:
        return self.tokens().total_known()

    def finish(self) -> ParsedRun:
        """Normalize every event read so far."""
        results: dict[str, bool] = {}
        for obj in self.raw:
            if obj.get("type") == "user":
                for block in _blocks(obj.get("message") or {}):
                    if block.get("type") == "tool_result":
                        results[str(block.get("tool_use_id"))] = bool(block.get("is_error"))
        events = tuple(e for e in (_event(obj, results) for obj in self.raw) if e is not None)
        result_error = None
        result_text = None
        denials: tuple[dict[str, Any], ...] = ()
        if self._result is not None:
            if self._result.get("is_error"):
                result_error = str(self._result.get("subtype") or "error")
            if isinstance(self._result.get("result"), str):
                result_text = self._result["result"]
            raw_denials = self._result.get("permission_denials")
            if isinstance(raw_denials, list):
                denials = tuple(d for d in raw_denials if isinstance(d, dict))
        return ParsedRun(events=events, tokens=self.tokens(), steps=self.steps,
                         final_message=last_assistant_text(events), result_error=result_error,
                         permission_denials=denials, init=_init(self._init),
                         result_text=result_text, api_error=self._api_error)


def _init(obj: Mapping[str, Any] | None) -> AgentInit | None:
    """Normalize the `system/init` event; None when the stream had none."""
    if obj is None:
        return None
    tools = obj.get("tools") if isinstance(obj.get("tools"), list) else []
    servers = obj.get("mcp_servers") if isinstance(obj.get("mcp_servers"), list) else []
    version = obj.get("claude_code_version")
    model = obj.get("model")
    return AgentInit(
        version=version if isinstance(version, str) else None,
        model=model if isinstance(model, str) else None,
        tools=tuple(str(t) for t in tools),
        mcp_servers=tuple((str(s.get("name")), str(s.get("status")))
                          for s in servers if isinstance(s, dict)))


def parse_lines(lines: Sequence[str]) -> ParsedRun:
    """Parse a whole stream-json transcript (one JSON object per line)."""
    parser = ClaudeStreamParser()
    for line in lines:
        line = line.strip()
        if line:
            obj = json.loads(line)
            if isinstance(obj, dict):
                parser.feed(obj)
    return parser.finish()


def _blocks(message: Mapping[str, Any]) -> list[dict[str, Any]]:
    content = message.get("content")
    if isinstance(content, list):
        return [b for b in content if isinstance(b, dict)]
    return []


def _event(obj: dict[str, Any], results: Mapping[str, bool]) -> AgentEvent | None:
    kind = obj.get("type")
    parent = obj.get("parent_tool_use_id")
    parent = parent if isinstance(parent, str) else None
    if kind == "assistant":
        message = obj.get("message") or {}
        texts, calls = [], []
        for block in _blocks(message):
            if block.get("type") == "text" and isinstance(block.get("text"), str):
                texts.append(block["text"])
            elif block.get("type") == "tool_use":
                calls.append(_tool_call(block, parent, results))
        message_id = message.get("id") if isinstance(message.get("id"), str) else None
        return AgentEvent(role="assistant", message_id=message_id, parent_tool_use_id=parent,
                          text="".join(texts), tool_calls=tuple(calls))
    if kind == "user":
        tool_results = tuple(
            ToolResult(tool_use_id=str(b.get("tool_use_id")), is_error=bool(b.get("is_error")),
                       text=_result_text(b.get("content")))
            for b in _blocks(obj.get("message") or {}) if b.get("type") == "tool_result")
        return AgentEvent(role="user", parent_tool_use_id=parent, tool_results=tool_results)
    if kind in ("system", "result"):
        return AgentEvent(role=kind)
    return None


def _result_text(content: Any) -> str:
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return "".join(p.get("text", "") for p in content
                       if isinstance(p, dict) and isinstance(p.get("text"), str))
    return ""


def _tool_call(block: Mapping[str, Any], parent: str | None,
               results: Mapping[str, bool]) -> ToolCall:
    call_id = str(block.get("id") or "")
    name = str(block.get("name") or "")
    args = block.get("input") if isinstance(block.get("input"), dict) else {}
    return ToolCall(id=call_id, name=name, args=dict(args), parent_tool_use_id=parent,
                    paths=_paths(name, args), is_error=results.get(call_id))


def _paths(name: str, args: Mapping[str, Any]) -> tuple[str, ...]:
    """Path strings a file tool would touch, relative ones left relative.

    Glob's `pattern` and Grep's `glob` are locations too (`../../x/**` leaves
    the tree), so they are joined onto the tool's `path` argument when given.
    """
    paths = [args[k] for k in _PATH_ARG_KEYS if isinstance(args.get(k), str) and args[k]]
    base = args.get("path") if isinstance(args.get("path"), str) else None
    pattern_key = {"Glob": "pattern", "Grep": "glob"}.get(name)
    pattern = args.get(pattern_key) if pattern_key else None
    if isinstance(pattern, str) and pattern:
        paths.append(os.path.join(base, pattern) if base and not os.path.isabs(pattern)
                     else pattern)
    return tuple(paths)


# --- command and environment -------------------------------------------------

def build_command(executable: Sequence[str], model: str, *,
                  corpus_dir: str | None = None) -> list[str]:
    """The `claude -p` command line; the corpus directory only with cards."""
    argv = [*executable, "-p", "--model", model, "--restricted",
            "--tools", ",".join(ALLOWED_BUILTIN_TOOLS),
            "--setting-sources", "", "--strict-mcp-config", "--no-session-persistence",
            "--permission-prompts", "none"]
    if corpus_dir is not None:
        argv += ["--add-dir", corpus_dir]
    argv += ["--output-format", "stream-json", "--verbose"]
    return argv


def build_env(parent: Mapping[str, str], config_dir: str | os.PathLike[str]) -> dict[str, str]:
    """The child's whole environment, built from an allow-list (KTD3 layer 4).

    Nothing `CLAUDE*`/`ANTHROPIC*` is inherited (no parent session markers,
    no endpoint redirection); home, temp and configuration all point into the
    isolated directory. The only `CLAUDE*` variables are the two set here.
    Non-essential traffic (telemetry, error reporting, update checks) is off
    so the model stays the only destination of the run's data (R14).
    """
    return build_isolated_env(parent, config_dir, fixed={
        "CLAUDE_CONFIG_DIR": str(Path(config_dir)),
        "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1",
    })


# --- infrastructure failures -------------------------------------------------

# Error classes Claude Code may attach to an assistant message (`error`).
# Recalled from the Agent SDK types, not seen in the captured transcript; only
# consulted when present. `max_output_tokens` and `invalid_request` are left
# out on purpose: they can follow from what the agent itself did.
_INFRA_API_ERRORS = frozenset({"authentication_failed", "billing_error", "rate_limit",
                               "server_error"})
# Texts that name a cause outside the agent: login, quota, API outage, a CLI
# that refused its options. Matched only on a run the CLI itself ended as an
# error, so an answer that merely talks about rate limits is not affected.
_INFRA_TEXT = re.compile(
    r"invalid api key|/login\b|not logged in|oauth|authenticat|"
    r"token (?:has )?expired|credit balance|billing|"
    r"rate.?limit|usage limit|limit reached|quota|\b429\b|"
    r"overloaded|\b5(?:00|02|03|04|29)\b|api error|internal server error|"
    r"service unavailable|connection (?:error|refused|reset)|"
    r"econnrefused|econnreset|enotfound|etimedout|"
    r"unknown option|unrecognized option|error: option",
    re.IGNORECASE)


def claude_infrastructure_problem(record: RunRecord) -> str | None:
    """Claude Code's non-agent failures, on top of `generic_infrastructure_problem`.

    Conservative by design: a run the CLI ended as an error (`is_error`)
    counts as infrastructure only when the API error class or the CLI's text
    names a clearly external cause; any other error falls through to scoring
    as the agent's failure, with the error kept in the run's record.
    """
    generic = generic_infrastructure_problem(record)
    if generic is not None or record.cap_hit is not None:
        return generic
    if record.result_error is None:
        return None  # the CLI completed the run, whatever happened on the way
    if record.api_error in _INFRA_API_ERRORS:
        return f"API vrátilo chybu {record.api_error}: {_error_text(record)}"
    text = "\n".join(t for t in (record.result_text, record.stderr_tail, record.final_message)
                     if t)
    if _INFRA_TEXT.search(text):
        return f"CLI skončilo chybou mimo agenta ({record.result_error}): {_error_text(record)}"
    return None


def _error_text(record: RunRecord) -> str:
    text = (record.result_text or record.stderr_tail or record.final_message or "").strip()
    return text[:500] or "bez popisu"


# --- adapter -----------------------------------------------------------------

@dataclass(frozen=True)
class ClaudeCodeSettings:
    """Fixed for the whole experiment (R6): model, CLI, login copy, isolation root.

    `keep_apart` names further directories the isolated configuration must
    stay out of (the output root, the catalog's directory).
    """

    model: str
    isolation_root: Path
    executable: tuple[str, ...] = ("claude",)
    credentials_source: Path = field(
        default_factory=lambda: Path.home() / ".claude" / CREDENTIALS_FILE)
    keep_apart: tuple[Path, ...] = ()


class ClaudeCodeAdapter(AgentAdapter):
    """Claude Code in `--restricted` print mode with an isolated configuration."""

    name = "claude-code"

    def __init__(self, settings: ClaudeCodeSettings,
                 parent_env: Mapping[str, str] | None = None) -> None:
        self.settings = settings
        self._parent_env = dict(os.environ if parent_env is None else parent_env)

    def run(self, request: RunRequest) -> RunRecord:
        s = self.settings
        executable = resolve_executable(s.executable)
        if not s.credentials_source.is_file():
            raise AgentRunError(f"Přihlášení agenta {s.credentials_source} neexistuje.")
        keep_apart = [request.workdir, *s.keep_apart]
        if request.corpus_dir is not None:
            keep_apart.append(request.corpus_dir)
        parser = ClaudeStreamParser()
        with isolated_config_dir(s.isolation_root, keep_apart) as cfg:
            for sub in ("tmp", "appdata", "localappdata"):
                (cfg / sub).mkdir()
            credentials = cfg / CREDENTIALS_FILE
            shutil.copyfile(s.credentials_source, credentials)
            os.chmod(credentials, 0o600)
            seed_config_files(cfg, request.config_files)
            env = build_env(self._parent_env, cfg)
            argv = build_command(
                executable, s.model,
                corpus_dir=str(request.corpus_dir) if request.corpus_dir is not None else None)
            outcome = run_watched(argv, cwd=request.workdir, env=env, stdin_text=request.prompt,
                                  cap=request.cap, parser=parser,
                                  transcript_path=request.transcript_path)
            config_dir = str(cfg)
        parsed = parser.finish()
        return RunRecord(
            adapter=self.name,
            workdir=str(request.workdir),
            corpus_dir=str(request.corpus_dir) if request.corpus_dir is not None else None,
            config_dir=config_dir,
            events=parsed.events,
            tokens=parsed.tokens,
            steps=parsed.steps,
            wall_seconds=outcome.wall_seconds,
            cap=request.cap,
            cap_hit=outcome.cap_hit,
            exit_status=outcome.exit_status,
            final_message=parsed.final_message,
            result_error=parsed.result_error,
            permission_denials=parsed.permission_denials,
            stderr_tail=outcome.stderr_tail,
            init=parsed.init,
            result_text=parsed.result_text,
            api_error=parsed.api_error,
        )

    def infrastructure_problem(self, record: RunRecord) -> str | None:
        return claude_infrastructure_problem(record)
