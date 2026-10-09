"""Scripted adapter: replays stored Claude Code stream-json instead of running an agent.

The runner, the tool check and the report must be testable without a model,
a network or a process (R12). This adapter takes a script — a function from
the run request to the transcript lines that run should "produce" — and
turns those lines into a `RunRecord` through the same `ClaudeStreamParser`
the real adapter uses, so audit, scoring and the report see exactly what they
would see from Claude Code.

What it mirrors from the real adapter, because tests rely on it:

* the isolated configuration directory is created and removed with
  `isolated_config_dir` (leftover refusal, overlap checks, `config_files`);
* the transcript is written to `request.transcript_path`;
* the step and token caps trip on the event that exceeds them and the rest of
  the script is dropped, as the watchdog would kill the agent there; the time
  cap trips when the script's declared wall time exceeds the cap.

It also records every request it got (`requests`) and how many runs were
active at once (`max_active`), so a test can prove what the runner asked
for and that it never runs two attempts concurrently (KTD11).

Stored transcripts (`load_transcript`) are stream-json files as Claude Code
writes them, with two additions for replay:

* An optional header line `{"type": "scripted", "wall_seconds": 12.5,
  "exit_status": 0, "stderr_tail": ""}` carries what the process would have
  reported; it is consumed by the loader and never replayed.
* Placeholders such as `__FILE__` are replaced before parsing, each value
  JSON-escaped, so one transcript serves every case of a catalog.
"""
from __future__ import annotations

import json
import re
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable, Mapping, Sequence, Union

from agents.base import (CAP_STEPS, CAP_TIME, CAP_TOKENS, AgentAdapter, RunRecord,
                         RunRequest, cap_hit_for, isolated_config_dir, seed_config_files)
from agents.claude_code import ClaudeStreamParser, claude_infrastructure_problem


@dataclass(frozen=True)
class ScriptedRun:
    """One scripted attempt: stream-json lines plus what the process would report."""

    lines: tuple[str, ...]
    wall_seconds: float = 1.0
    exit_status: int | None = 0
    stderr_tail: str = ""


Script = Callable[[RunRequest], Union[ScriptedRun, Sequence[str]]]

HEADER_TYPE = "scripted"
_PLACEHOLDER = re.compile(r"__[A-Z][A-Z0-9_]*__")


def load_transcript(path: str | Path, values: Mapping[str, str] | None = None) -> ScriptedRun:
    """Read a stored transcript into a `ScriptedRun` (format in the module docstring).

    Every placeholder of `values` is replaced in the raw text by the
    JSON-escaped value, so a value with quotes or backslashes (a Windows
    path) still yields valid JSON. A placeholder (`__NAME__`, upper case)
    without a value would silently replay the wrong text, so it is an error.
    """
    text = Path(path).read_text(encoding="utf-8")
    values = values or {}
    leftover = sorted(set(_PLACEHOLDER.findall(text)) - set(values))
    if leftover:
        raise ValueError(f"{path}: placeholders without a value: {', '.join(leftover)}")
    for placeholder, value in values.items():
        text = text.replace(placeholder, json.dumps(value, ensure_ascii=False)[1:-1])
    header: dict[str, Any] = {}
    lines: list[str] = []
    for line in text.splitlines():
        if not line.strip():
            continue
        obj = json.loads(line)
        if isinstance(obj, dict) and obj.get("type") == HEADER_TYPE:
            header.update(obj)
        else:
            lines.append(line)
    return ScriptedRun(tuple(lines), wall_seconds=float(header.get("wall_seconds", 1.0)),
                       exit_status=header.get("exit_status", 0),
                       stderr_tail=str(header.get("stderr_tail", "")))


class ScriptedAdapter(AgentAdapter):
    """Replays a script through the Claude stream parser (see the module docstring)."""

    name = "scripted"

    def __init__(self, script: Script, isolation_root: str | Path,
                 keep_apart: Sequence[str | Path] = ()) -> None:
        self._script = script
        self._isolation_root = Path(isolation_root)
        self._keep_apart = tuple(Path(p) for p in keep_apart)
        self.requests: list[RunRequest] = []
        self._active = 0
        self.max_active = 0

    def run(self, request: RunRequest) -> RunRecord:
        self._active += 1
        self.max_active = max(self.max_active, self._active)
        try:
            return self._replay(request)
        finally:
            self._active -= 1

    def infrastructure_problem(self, record: RunRecord) -> str | None:
        return claude_infrastructure_problem(record)

    def _replay(self, request: RunRequest) -> RunRecord:
        self.requests.append(request)
        keep_apart = [request.workdir, *self._keep_apart]
        if request.corpus_dir is not None:
            keep_apart.append(request.corpus_dir)
        parser = ClaudeStreamParser()
        with isolated_config_dir(self._isolation_root, keep_apart) as cfg:
            seed_config_files(cfg, request.config_files)
            planned = self._script(request)
            if not isinstance(planned, ScriptedRun):
                planned = ScriptedRun(tuple(planned))
            tripped = _feed(parser, planned.lines, request)
            if tripped is None and planned.wall_seconds > request.cap.seconds:
                tripped = CAP_TIME
            config_dir = str(cfg)
        wall = planned.wall_seconds
        parsed = parser.finish()
        return RunRecord(
            adapter=self.name,
            workdir=str(request.workdir),
            corpus_dir=str(request.corpus_dir) if request.corpus_dir is not None else None,
            config_dir=config_dir,
            events=parsed.events,
            tokens=parsed.tokens,
            steps=parsed.steps,
            wall_seconds=wall,
            cap=request.cap,
            cap_hit=cap_hit_for(tripped, request.cap, parser, wall),
            exit_status=None if tripped is not None else planned.exit_status,
            final_message=parsed.final_message,
            result_error=parsed.result_error,
            permission_denials=parsed.permission_denials,
            stderr_tail=planned.stderr_tail,
            init=parsed.init,
            result_text=parsed.result_text,
            api_error=parsed.api_error,
        )


def _feed(parser: ClaudeStreamParser, lines: Sequence[str], request: RunRequest) -> str | None:
    """Feed lines like the watchdog's reader thread; return the cap that tripped."""
    transcript = open(request.transcript_path, "w", encoding="utf-8", newline="\n") \
        if request.transcript_path is not None else None
    try:
        for line in lines:
            if transcript is not None:
                transcript.write(line if line.endswith("\n") else line + "\n")
            stripped = line.strip()
            if not stripped:
                continue
            obj = json.loads(stripped)
            if not isinstance(obj, dict):
                continue
            parser.feed(obj)
            if parser.steps > request.cap.steps:
                return CAP_STEPS
            if parser.tokens_total() > request.cap.tokens:
                return CAP_TOKENS
        return None
    finally:
        if transcript is not None:
            transcript.close()
