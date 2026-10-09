"""One call of a judge CLI in isolation: prompt on stdin, JSON reply and model out (KTD6).

The judge is a command line the owner configures (`judges.json`); this module
only knows how to run it safely and how to read what it prints. It does not
know what a verdict is (that is `judge`).

Isolation, in the same terms as the agent's (KTD3 layer 4):

* a private configuration directory (`isolated_config_dir`) that holds the
  copies of the login files the judge entry names (`prihlaseni`) and is
  removed afterwards, whatever happens;
* an environment built from an allow-list (`build_isolated_env`): the OS
  basics, the variables the entry names (`promenne`, names only) and the
  directories of this CLI family; nothing else of the parent's is inherited;
* an empty working directory, with a sibling directory next to it that stays
  empty unless the smoke test plants a canary file there (R17);
* the text goes to standard input; nothing is put on the command line.

Output shapes (`vystup` in the judges file, default by family: `claude` ->
`claude-json`, `openai` -> `codex-jsonl`; any other family must name one):

* `claude-json` - `claude -p ... --output-format json` prints one JSON object
  with `type: "result"`; the answer text is its `result`, the model is the
  key of `modelUsage` (checked on Claude Code 2.1.296).
* `codex-jsonl` - `codex exec --json ...` prints one JSON event per line; the
  answer is the text of the last `item.completed` event whose item is an
  `agent_message`, and `turn.completed` marks a finished turn. The events do
  not name the model, so it is read from the `turn_context` record of the
  session file the CLI writes into its (isolated) `CODEX_HOME` (codex-cli
  0.159.2); the session is therefore not run with `--ephemeral`. The exit
  code of `codex exec` is not trusted (its own sandbox can return 0 without
  a result): the shape of the output decides.

An infrastructure failure (exit code, no JSON, time limit, no model) raises
`JudgeInfrastructureError`; the caller must stop the measurement, not retry
with another judge.
"""
from __future__ import annotations

import json
import os
import shutil
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Mapping, Sequence

from agents.base import (AgentRunError, Cap, build_isolated_env, isolated_config_dir,
                         remove_tree, run_watched)

SHAPE_CLAUDE = "claude-json"
SHAPE_CODEX = "codex-jsonl"
OUTPUT_SHAPES = (SHAPE_CLAUDE, SHAPE_CODEX)
DEFAULT_SHAPE = {"claude": SHAPE_CLAUDE, "openai": SHAPE_CODEX}

# Replaced in the prompt by the path of the sibling directory (smoke test only).
SIBLING_PLACEHOLDER = "@@SIBLING_DIR@@"

_STDOUT_FILE = "stdout.txt"
_NO_LIMIT = 2 ** 62
_TAIL = 400


class JudgeInfrastructureError(AgentRunError):
    """The judge CLI did not run or did not answer (Czech message for the operator)."""


@dataclass(frozen=True)
class CliReply:
    """What one successful CLI call produced.

    `text` is the answer text (expected to hold the verdict JSON); `model` is
    the model the CLI itself reported, `None` when it reported none; `raw` is
    the CLI's whole standard output (the smoke test looks for the canary in it).
    """

    text: str
    model: str | None
    raw: str


class _NullParser:
    """The watchdog's stream parser; the judge's output is read from the file afterwards."""

    steps = 0

    def feed(self, obj: dict[str, Any]) -> None:
        pass

    def tokens_total(self) -> int:
        return 0


def family_environment(family: str, config_dir: Path) -> dict[str, str]:
    """The variables a CLI family needs to keep its state inside `config_dir`."""
    if family == "claude":
        return {"CLAUDE_CONFIG_DIR": str(config_dir),
                "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1"}
    if family == "openai":
        return {"CODEX_HOME": str(config_dir)}
    return {}


def _resolve_executable(command: Sequence[str]) -> list[str]:
    if not command:
        raise JudgeInfrastructureError("Příkaz hodnotitele je prázdný.")
    first = command[0]
    if os.path.dirname(first):
        return list(command)
    found = shutil.which(first)
    if found is None:
        raise JudgeInfrastructureError(f"Spustitelný soubor hodnotitele {first} nebyl nalezen v PATH.")
    return [found, *command[1:]]


def _tail(text: str) -> str:
    text = text.strip()
    return f" Chybový výstup: {text[-_TAIL:]}" if text else ""


def run_judge_cli(*, command: Sequence[str], family: str, output_shape: str,
                  credentials: Sequence[tuple[str, str]], variables: Sequence[str],
                  prompt: str, isolation_root: str | os.PathLike[str],
                  parent_env: Mapping[str, str] | None = None, seconds: float = 300.0,
                  keep_apart: Sequence[str | os.PathLike[str]] = (),
                  sibling_files: Mapping[str, str] | None = None) -> CliReply:
    """Run the judge once and return its reply; raise `JudgeInfrastructureError` on failure.

    `credentials` are (source file, relative target inside the configuration
    directory) pairs; `variables` are the names of the parent's variables to
    pass on. `sibling_files` (name -> text) are written into the sibling
    directory of the working directory before the start (smoke test, R17).
    """
    if output_shape not in OUTPUT_SHAPES:
        raise JudgeInfrastructureError(f"Neznámý tvar výstupu hodnotitele {output_shape}.")
    argv = _resolve_executable(command)
    sources = []
    for source, target in credentials:
        path = Path(source).expanduser()
        if not path.is_file():
            raise JudgeInfrastructureError(f"Soubor přihlášení hodnotitele {source} neexistuje.")
        sources.append((path, target))
    environment = dict(os.environ if parent_env is None else parent_env)

    root = Path(isolation_root)
    root.mkdir(parents=True, exist_ok=True)
    scratch = Path(tempfile.mkdtemp(prefix="judge-run-", dir=root))
    try:
        workdir = scratch / "work"
        sibling = scratch / "sibling"
        workdir.mkdir()
        sibling.mkdir()
        for name, text in (sibling_files or {}).items():
            (sibling / name).write_text(text, encoding="utf-8")
        stdout_file = scratch / _STDOUT_FILE
        with isolated_config_dir(root, [scratch, *keep_apart]) as cfg:
            for sub in ("tmp", "appdata", "localappdata"):
                (cfg / sub).mkdir()
            for source, target in sources:
                _copy_login(source, target, cfg)
            env = build_isolated_env(environment, cfg, passthrough=variables,
                                     fixed=family_environment(family, cfg))
            outcome = run_watched(argv, cwd=workdir, env=env,
                                  stdin_text=prompt.replace(SIBLING_PLACEHOLDER, str(sibling)),
                                  cap=Cap(seconds=seconds, steps=_NO_LIMIT, tokens=_NO_LIMIT),
                                  parser=_NullParser(), transcript_path=stdout_file)
            raw = stdout_file.read_text(encoding="utf-8", errors="replace") \
                if stdout_file.is_file() else ""
            session_model = _codex_session_model(cfg) if output_shape == SHAPE_CODEX else None
    finally:
        remove_tree(scratch)

    if outcome.cap_hit is not None:
        raise JudgeInfrastructureError(
            f"Hodnotitel nedoběhl do časového limitu {seconds:g} s.{_tail(outcome.stderr_tail)}")
    if output_shape == SHAPE_CLAUDE:
        return _read_claude(raw, outcome.exit_status, outcome.stderr_tail)
    return _read_codex(raw, outcome.exit_status, outcome.stderr_tail, session_model)


def _copy_login(source: Path, target: str, config_dir: Path) -> None:
    parts = Path(target).parts
    if not parts or Path(target).is_absolute() or Path(target).anchor or ".." in parts:
        raise JudgeInfrastructureError(
            f"Cíl přihlášení {target} musí být relativní cesta uvnitř adresáře nastavení.")
    destination = config_dir / target
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(source, destination)
    os.chmod(destination, 0o600)


# --- reading the output ------------------------------------------------------

def _read_claude(raw: str, exit_status: int | None, stderr: str) -> CliReply:
    if exit_status != 0:
        raise JudgeInfrastructureError(
            f"Hodnotitel skončil s kódem {exit_status}.{_tail(stderr)}")
    result = _claude_result(raw)
    if result is None:
        raise JudgeInfrastructureError(
            f"Hodnotitel (kód 0) nevypsal JSON s výsledkem.{_tail(stderr)}")
    text = result.get("result")
    if result.get("is_error") or not isinstance(text, str):
        raise JudgeInfrastructureError(
            f"Hodnotitel ohlásil chybu: {str(text or result.get('subtype'))[:_TAIL]}")
    usage = result.get("modelUsage")
    models = sorted(str(name) for name in usage) if isinstance(usage, dict) else []
    return CliReply(text=text, model=",".join(models) if models else None, raw=raw)


def _claude_result(raw: str) -> dict[str, Any] | None:
    stripped = raw.strip()
    candidates = [stripped] + [line.strip() for line in reversed(stripped.splitlines())]
    for candidate in candidates:
        if not candidate:
            continue
        try:
            data = json.loads(candidate)
        except json.JSONDecodeError:
            continue
        items = data if isinstance(data, list) else [data]
        for item in reversed(items):
            if isinstance(item, dict) and item.get("type") == "result":
                return item
    return None


def _read_codex(raw: str, exit_status: int | None, stderr: str,
                session_model: str | None) -> CliReply:
    reply: str | None = None
    completed = False
    failure: str | None = None
    for line in raw.splitlines():
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if not isinstance(event, dict):
            continue
        kind = event.get("type")
        if kind == "item.completed":
            item = event.get("item")
            if isinstance(item, dict) and item.get("type") == "agent_message" \
                    and isinstance(item.get("text"), str):
                reply = item["text"]
        elif kind == "turn.completed":
            completed = True
        elif kind == "turn.failed":
            error = event.get("error")
            failure = str(error.get("message") if isinstance(error, dict) else error)[:_TAIL]
    if reply is None or not completed:
        detail = f" {failure}" if failure else _tail(stderr)
        raise JudgeInfrastructureError(
            f"Hodnotitel nevrátil dokončenou odpověď (kód {exit_status}).{detail}")
    return CliReply(text=reply, model=session_model, raw=raw)


def _codex_session_model(config_dir: Path) -> str | None:
    """The model of the first `turn_context` in the session file the CLI wrote."""
    for session in sorted((config_dir / "sessions").glob("**/*.jsonl")):
        try:
            lines = session.read_text(encoding="utf-8", errors="replace").splitlines()
        except OSError:
            continue
        for line in lines:
            try:
                record = json.loads(line)
            except json.JSONDecodeError:
                continue
            payload = record.get("payload") if isinstance(record, dict) else None
            if isinstance(record, dict) and record.get("type") == "turn_context" \
                    and isinstance(payload, dict) and isinstance(payload.get("model"), str):
                return payload["model"]
    return None
