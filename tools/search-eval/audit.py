"""Audit of one run's normalized events: did the agent try anything forbidden? (U4)

This is isolation layer 5 (KTD3). Layers 1-4 are meant to make forbidden
access impossible; the audit does not trust them and reads what the agent
actually *tried*, whether or not the CLI let it through, because R5 counts an
attempt as spoiling the run. A spoiled run is replaced (R6), so a false
positive costs a rerun while a missed finding would put a leaked hint into
the measurement — the checks therefore lean strict.

What spoils a run (one `AuditFinding` per problem):

* `forbidden_tool` — any tool outside Read/Grep/Glob, in either variant. That
  includes every MCP tool: no variant has an MCP server, so a call to one
  means a server leaked past `--strict-mcp-config`.
* `config_dir_access` — a path inside the agent's isolated configuration.
* `git` — a path into a `.git` directory, or a git command in a command-like
  argument.
* `path_outside` — a path outside the work tree (and, when the variant has a
  corpus, outside the corpus). Relative paths resolve against the work tree.

Every path is judged at two locations, and a finding at either counts: its
literal prefix up to the first glob character (`../../x/**` leaves the tree)
and the whole path with `..` collapsed lexically. The second one exists
because a glob character before `..` hides the climb from the prefix:
`{x}/../../catalog.json` or `src/**/../../../x` have a prefix inside the tree
but name a location outside it.
"""
from __future__ import annotations

import os
import re
from dataclasses import dataclass
from typing import Iterable

from agents.base import RunRecord, ToolCall
from agents.claude_code import ALLOWED_BUILTIN_TOOLS
from prepare import canonical_path

FORBIDDEN_TOOL = "forbidden_tool"
CONFIG_DIR_ACCESS = "config_dir_access"
GIT = "git"
PATH_OUTSIDE = "path_outside"

_COMMAND_KEYS = ("command", "cmd", "script", "code", "args")
_GIT_COMMAND = re.compile(r"(?:^|[\s;&|(`'\"])git(?:\.exe)?(?=\s|$)", re.IGNORECASE)
_GLOB_CHARS = set("*?[{")


@dataclass(frozen=True)
class AuditFinding:
    """One forbidden attempt: its kind, the tool call it came from, and a detail."""

    kind: str
    tool_call_id: str
    detail: str


def audit_run(record: RunRecord) -> tuple[AuditFinding, ...]:
    """Every forbidden attempt in the run; empty means the run is clean."""
    workdir = canonical_path(record.workdir)
    roots = [workdir]
    if record.corpus_dir:
        roots.append(canonical_path(record.corpus_dir))
    config_dir = canonical_path(record.config_dir)
    findings: list[AuditFinding] = []
    for call in _tool_calls(record):
        if call.name not in ALLOWED_BUILTIN_TOOLS:
            findings.append(AuditFinding(FORBIDDEN_TOOL, call.id, call.name))
        findings.extend(_git_command_findings(call))
        for raw in call.paths:
            finding = _path_finding(call, raw, record.workdir, roots, config_dir)
            if finding is not None:
                findings.append(finding)
    return tuple(findings)


def _tool_calls(record: RunRecord) -> Iterable[ToolCall]:
    for event in record.events:
        yield from event.tool_calls


def _git_command_findings(call: ToolCall) -> list[AuditFinding]:
    found = []
    for key in _COMMAND_KEYS:
        value = call.args.get(key)
        values = value if isinstance(value, list) else [value]
        text = " ".join(v for v in values if isinstance(v, str))
        if text and _GIT_COMMAND.search(text):
            found.append(AuditFinding(GIT, call.id, f"{call.name}: {text[:200]}"))
    return found


def _path_finding(call: ToolCall, raw: str, workdir: str, roots: list[str],
                  config_dir: str) -> AuditFinding | None:
    locations = (canonical_path(_literal_location(raw, workdir)),
                 canonical_path(_full_location(raw, workdir)))
    if any(_within(location, config_dir) for location in locations):
        return AuditFinding(CONFIG_DIR_ACCESS, call.id, f"{call.name}: {raw}")
    if ".git" in (part.lower() for part in re.split(r"[\\/]", raw)):
        return AuditFinding(GIT, call.id, f"{call.name}: {raw}")
    for location in locations:
        if not any(_within(location, root) for root in roots):
            return AuditFinding(PATH_OUTSIDE, call.id, f"{call.name}: {raw}")
    return None


def _full_location(raw: str, workdir: str) -> str:
    """The whole path or pattern as an absolute path (`..` collapsed by `canonical_path`)."""
    text = os.path.expanduser(raw)
    return text if os.path.isabs(text) else os.path.join(workdir, text)


def _literal_location(raw: str, workdir: str) -> str:
    """The directory or file a path or glob pattern names, as an absolute path."""
    parts = re.split(r"([\\/])", os.path.expanduser(raw))
    literal: list[str] = []
    for part in parts:
        if _GLOB_CHARS & set(part):
            break
        literal.append(part)
    text = "".join(literal) or "."
    return text if os.path.isabs(text) else os.path.join(workdir, text)


def _within(path: str, root: str) -> bool:
    try:
        return os.path.commonpath([path, root]) == root
    except ValueError:  # different drives
        return False
