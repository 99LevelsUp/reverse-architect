"""Adapter interface, normalized run record, cap watchdog and isolation (U3).

Everything downstream of an agent run (audit, scoring, the report) reads only
the normalized `RunRecord`, never an agent's raw transcript, so a second agent
(Codex, KTD2) needs only a new adapter, not changes to audit or scoring.

Why the pieces look the way they do:

* **Tokens (KTD5).** Four categories: input without cache, cache write, cache
  read, output. `None` means the agent did not report the category; it is
  never written as 0, because the report must show "unknown", not a fake
  zero. Each adapter has its own pure normalizer from its usage shape to
  `TokenUsage`:
  - Claude reports the categories separately (`input_tokens`,
    `cache_creation_input_tokens`, `cache_read_input_tokens`,
    `output_tokens`) — see `agents.claude_code.normalize_claude_usage`.
  - Codex reports `input_tokens` *including* `cached_input_tokens` and has no
    cache-write figure. A Codex normalizer must map
    `input_uncached = input_tokens - cached_input_tokens`,
    `cache_read = cached_input_tokens`, `cache_write = None`,
    `output = output_tokens`, and take the final totals once rather than
    summing running totals (not implemented yet).
* **Steps (KTD4).** One step is one tool call the agent made, including calls
  made inside a subagent (events with `parent_tool_use_id`).
* **Cap (KTD4).** No CLI limits steps, so the runner enforces wall time,
  steps and tokens itself. The token cap counts the sum of the four
  categories (unknown categories count as 0 for the cap only). The wall-time
  check runs on the main thread and does not depend on the stream, so a
  silent agent is still stopped. On a cap the whole process tree is killed
  (Windows `taskkill /T /F`, POSIX process group), the runner waits for the
  exit and records by how much the cap was exceeded.
* **Isolation (KTD3 layer 4, KTD9).** The agent's configuration directory is
  a fixed directory under an isolation root outside the work tree, the
  corpus and the output root. It is removed in `finally` (cap kill,
  exception, Ctrl+C included); finding it at start means an earlier run did
  not clean up, and the start is refused rather than reusing a directory
  that may hold a credential copy or another run's state.
"""
from __future__ import annotations

import abc
import json
import os
import shutil
import signal
import stat
import subprocess
import threading
import time
from contextlib import contextmanager
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Iterator, Mapping, Protocol, Sequence

from prepare import canonical_path

CONFIG_DIR_NAME = "agent-config"
CAP_STEPS = "steps"
CAP_TOKENS = "tokens"
CAP_TIME = "time"
_STDERR_TAIL_CHARS = 8000


class AgentRunError(RuntimeError):
    """The run could not be started or cleaned up (Czech message for the operator)."""


class IsolationLeftover(AgentRunError):
    """An isolated configuration directory from an earlier run still exists."""


# --- normalized record -------------------------------------------------------

@dataclass(frozen=True)
class Cap:
    """The fixed per-run cap of the experiment (R6): wall seconds, steps, tokens."""

    seconds: float
    steps: int
    tokens: int


@dataclass(frozen=True)
class TokenUsage:
    """Tokens in the four KTD5 categories; `None` = not reported (unknown)."""

    input_uncached: int | None = None
    cache_write: int | None = None
    cache_read: int | None = None
    output: int | None = None

    def total_known(self) -> int:
        """Sum of the reported categories; used only to enforce the token cap."""
        return sum(v for v in (self.input_uncached, self.cache_write,
                               self.cache_read, self.output) if v is not None)


def sum_usages(usages: Sequence[TokenUsage]) -> TokenUsage:
    """Add per-message usages; a category is known only if every message reported it.

    A partially reported category would be a silent undercount, so it stays
    unknown instead. No messages at all means nothing is known.
    """
    def total(attr: str) -> int | None:
        values = [getattr(u, attr) for u in usages]
        if not values or any(v is None for v in values):
            return None
        return sum(values)

    return TokenUsage(total("input_uncached"), total("cache_write"),
                      total("cache_read"), total("output"))


@dataclass(frozen=True)
class ToolCall:
    """One tool call with its arguments and the paths it touched.

    `paths` are path strings as the tool interprets them (relative ones are
    relative to the agent's working directory; glob patterns are kept as
    written). `is_error` is the tool result's error flag, None if no result
    arrived (e.g. the run was killed first).
    """

    id: str
    name: str
    args: dict[str, Any]
    parent_tool_use_id: str | None
    paths: tuple[str, ...]
    is_error: bool | None


@dataclass(frozen=True)
class ToolResult:
    """Output of a tool as the agent received it; never a source of the answer."""

    tool_use_id: str
    is_error: bool
    text: str


@dataclass(frozen=True)
class AgentEvent:
    """One event of the transcript reduced to what audit and scoring read.

    `role` is "system", "assistant", "user" or "result". Assistant events of
    one model message share `message_id` (Claude emits one event per content
    block).
    """

    role: str
    message_id: str | None = None
    parent_tool_use_id: str | None = None
    text: str = ""
    tool_calls: tuple[ToolCall, ...] = ()
    tool_results: tuple[ToolResult, ...] = ()


@dataclass(frozen=True)
class CapHit:
    """Which cap stopped the run, its limit, the observed value and the overshoot."""

    kind: str
    limit: float
    observed: float
    overshoot: float


@dataclass(frozen=True)
class RunRequest:
    """One attempt: work tree, prompt, cap; a corpus only with cards."""

    workdir: Path
    prompt: str
    cap: Cap
    corpus_dir: Path | None = None
    transcript_path: Path | None = None
    # (relative path, text) written into the isolated configuration directory
    # before the agent starts. Only the tool check (KTD10) uses it, to plant a
    # user-instructions marker where the CLI would look for one.
    config_files: tuple[tuple[str, str], ...] = ()

    @property
    def with_cards(self) -> bool:
        return self.corpus_dir is not None


@dataclass(frozen=True)
class AgentInit:
    """What the agent declared at start: CLI version, model, tools, MCP servers.

    `mcp_servers` are (name, status) pairs. The tool check (KTD10) compares
    them with the configuration; a run never reads them.
    """

    version: str | None
    model: str | None
    tools: tuple[str, ...]
    mcp_servers: tuple[tuple[str, str], ...]


@dataclass(frozen=True)
class RunRecord:
    """The normalized result of one attempt; input to audit, scoring and report."""

    adapter: str
    workdir: str
    corpus_dir: str | None
    config_dir: str
    events: tuple[AgentEvent, ...]
    tokens: TokenUsage
    steps: int
    wall_seconds: float
    cap: Cap
    cap_hit: CapHit | None
    exit_status: int | None
    final_message: str | None
    result_error: str | None = None
    permission_denials: tuple[dict[str, Any], ...] = ()
    stderr_tail: str = ""
    init: AgentInit | None = None
    # The CLI's own closing text (Claude: the `result` event's `result`); on an
    # API or login failure this is where the reason is, not in `result_error`.
    result_text: str | None = None
    # An error class the CLI attached to an assistant message, if it does so.
    api_error: str | None = None

    @property
    def with_cards(self) -> bool:
        return self.corpus_dir is not None

    @property
    def has_result(self) -> bool:
        """Whether the CLI emitted its closing result event (a completed run)."""
        return any(e.role == "result" for e in self.events)


def last_assistant_text(events: Sequence[AgentEvent]) -> str | None:
    """Text of the agent's last top-level message, or None (KTD6).

    Only assistant events without `parent_tool_use_id` count (a subagent's
    message is not the agent's answer). All events of the last message id are
    joined. A last message that still calls a tool is not a final message —
    the run ended before the agent answered.
    """
    top = [e for e in events if e.role == "assistant" and e.parent_tool_use_id is None]
    if not top:
        return None
    last_id = top[-1].message_id
    parts: list[AgentEvent] = []
    for event in reversed(top):
        if event.message_id != last_id or (last_id is None and parts):
            break
        parts.append(event)
    parts.reverse()
    if any(e.tool_calls for e in parts):
        return None
    text = "".join(e.text for e in parts).strip()
    return text or None


# --- adapter interface -------------------------------------------------------

class StreamParser(Protocol):
    """Incremental reader of an agent's event stream, fed one JSON object per line."""

    def feed(self, obj: dict[str, Any]) -> None: ...

    @property
    def steps(self) -> int: ...

    def tokens_total(self) -> int: ...


class AgentAdapter(abc.ABC):
    """Runs one attempt in isolation and returns its normalized record."""

    name: str

    @abc.abstractmethod
    def run(self, request: RunRequest) -> RunRecord:
        """Run the agent once; the isolated configuration is gone when this returns."""

    def infrastructure_problem(self, record: RunRecord) -> str | None:
        """Czech reason when the run failed for a non-agent cause, else None.

        An infrastructure failure (login, API outage, rate limit, a CLI that
        did not run) says nothing about navigation, so it must neither count
        as the agent's failure nor be retried silently: the runner stops the
        experiment. Adapters add their CLI's error shapes; this default covers
        what holds for any adapter.
        """
        return generic_infrastructure_problem(record)


def generic_infrastructure_problem(record: RunRecord) -> str | None:
    """Adapter-independent infrastructure failures (see `infrastructure_problem`).

    A run stopped by the cap is the agent's failure (KTD4), with one
    exception: the time cap hit before the agent took a single step or wrote
    a single message. That is a CLI that never started working (stuck at
    login, an unreachable API, an internal retry loop), not an
    agent that got lost, and scoring it as `cap_time` would hide an outage in
    one variant's results instead of stopping the experiment. Otherwise a run
    with no event at all, or without the CLI's closing result event, did not
    complete: there is nothing to score.
    """
    tail = record.stderr_tail.strip()[-500:]
    suffix = f" Chybový výstup: {tail}" if tail else ""
    if record.cap_hit is not None:
        if record.cap_hit.kind == CAP_TIME and _no_progress(record):
            return (f"Agent do časového limitu {record.cap_hit.limit:g} s neudělal žádný krok "
                    f"ani nenapsal zprávu (zaseknuté CLI, přihlášení nebo nedostupné API)."
                    f"{suffix}")
        return None
    if not record.events:
        return f"Agent nevydal žádnou událost (kód ukončení {record.exit_status}).{suffix}"
    if not record.has_result:
        return (f"Agent skončil bez závěrečné události výsledku "
                f"(kód ukončení {record.exit_status}).{suffix}")
    return None


def _no_progress(record: RunRecord) -> bool:
    """No tool step and no assistant event: the agent never started working."""
    return record.steps == 0 and not any(e.role == "assistant" for e in record.events)


# --- isolation ---------------------------------------------------------------

def _within(path: str, root: str) -> bool:
    try:
        return os.path.commonpath([path, root]) == root
    except ValueError:  # different drives
        return False


@contextmanager
def isolated_config_dir(isolation_root: str | os.PathLike[str],
                        keep_apart: Sequence[str | os.PathLike[str]]) -> Iterator[Path]:
    """Create the agent's private configuration directory and always remove it.

    `keep_apart` are the work tree, corpus, output root and any other
    directory the configuration must neither live in nor contain.
    """
    root = Path(isolation_root)
    root.mkdir(parents=True, exist_ok=True)
    target = root / CONFIG_DIR_NAME
    canon = canonical_path(target)
    for other in keep_apart:
        other_canon = canonical_path(other)
        if _within(canon, other_canon) or _within(other_canon, canon):
            raise AgentRunError(
                f"Adresář nastavení agenta {target} se překrývá s {other}; "
                "musí ležet mimo pracovní strom, korpus i kořen výstupu.")
    if os.path.lexists(target):
        raise IsolationLeftover(
            f"V {target} zůstal adresář nastavení z předchozího běhu (mohl obsahovat "
            "kopii přihlášení). Ověř, že žádný běh neběží, smaž ho ručně a spusť znovu.")
    os.mkdir(target, 0o700)
    try:
        _restrict_to_user(target)
        yield target
    finally:
        remove_tree(target)


# Inherited from the parent only because the OS or the runtime needs them. The
# Claude adapter keeps its own copy of this list (`claude_code._INHERITED_ENV`);
# the two are meant to stay equal.
INHERITED_ENV = ("PATH", "PATHEXT", "SYSTEMROOT", "SYSTEMDRIVE", "WINDIR", "COMSPEC",
                 "LANG", "LC_ALL", "LC_CTYPE", "TZ")


def build_isolated_env(parent: Mapping[str, str], config_dir: str | os.PathLike[str], *,
                       passthrough: Sequence[str] = (),
                       fixed: Mapping[str, str] | None = None) -> dict[str, str]:
    """The child's whole environment, built from an allow-list (agent-neutral).

    Counterpart of `claude_code.build_env` for CLIs that are not Claude Code
    (the judge, KTD6). Only the OS/runtime variables of `INHERITED_ENV` and the
    variables named in `passthrough` (names matched case-insensitively, as on
    Windows) are taken from `parent`; home, temp and cache directories all
    point into `config_dir`; `fixed` adds the variables one CLI family needs
    (its own configuration directory). `fixed` wins over `passthrough`, and a
    passed-through variable never overrides the isolation variables, so an
    allow-listed name cannot redirect the CLI out of its private directory.
    """
    by_upper = {key.upper(): value for key, value in parent.items()}
    env = {name: by_upper[name] for name in INHERITED_ENV if name in by_upper}
    for name in passthrough:
        if name.upper() in by_upper:
            env[name.upper() if os.name == "nt" else name] = by_upper[name.upper()]
    cfg = Path(config_dir)
    tmp = cfg / "tmp"
    env.update({
        "HOME": str(cfg),
        "USERPROFILE": str(cfg),
        "APPDATA": str(cfg / "appdata"),
        "LOCALAPPDATA": str(cfg / "localappdata"),
        "XDG_CONFIG_HOME": str(cfg / "xdg-config"),
        "XDG_CACHE_HOME": str(cfg / "xdg-cache"),
        "XDG_DATA_HOME": str(cfg / "xdg-data"),
        "TEMP": str(tmp),
        "TMP": str(tmp),
        "TMPDIR": str(tmp),
    })
    env.update(fixed or {})
    return env


def seed_config_files(config_dir: Path, files: Sequence[tuple[str, str]]) -> None:
    """Write `RunRequest.config_files` into the isolated configuration directory."""
    for rel, text in files:
        parts = Path(rel).parts
        if not parts or Path(rel).is_absolute() or ".." in parts:
            raise AgentRunError(f"Soubor nastavení {rel} musí ležet uvnitř adresáře nastavení.")
        target = config_dir / rel
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(text, encoding="utf-8")


def _restrict_to_user(path: Path) -> None:
    """User-only access: mode 0700 on POSIX, best-effort ACL on Windows."""
    if os.name != "nt":
        os.chmod(path, 0o700)
        return
    user = os.environ.get("USERNAME")
    if not user:
        return
    domain = os.environ.get("USERDOMAIN")
    account = f"{domain}\\{user}" if domain else user
    icacls = os.path.join(os.environ.get("SYSTEMROOT", r"C:\Windows"), "System32", "icacls.exe")
    granted = subprocess.run([icacls, str(path), "/grant:r", f"{account}:(OI)(CI)F"],
                             capture_output=True)
    if granted.returncode == 0:
        subprocess.run([icacls, str(path), "/inheritance:r"], capture_output=True)


def remove_tree(path: Path, attempts: int = 20, delay: float = 0.25) -> None:
    """Remove a directory tree, retrying while Windows still holds handles.

    A killed process releases its file handles a moment later; giving up on
    the first PermissionError would leave the credential copy behind.
    """
    def make_writable(func: Any, target: str, _exc: Any) -> None:
        os.chmod(target, stat.S_IWRITE | stat.S_IREAD | stat.S_IEXEC)
        func(target)

    for attempt in range(attempts):
        if not os.path.lexists(path):
            return
        try:
            shutil.rmtree(path, onexc=make_writable)
            return
        except OSError:
            if attempt == attempts - 1:
                raise AgentRunError(
                    f"Adresář nastavení agenta {path} nejde smazat; smaž ho ručně, "
                    "jinak další běh nezačne.") from None
            time.sleep(delay)


# --- process watchdog --------------------------------------------------------

@dataclass
class ProcessResult:
    """What the watchdog observed about the agent process."""

    exit_status: int | None
    wall_seconds: float
    cap_hit: CapHit | None
    stderr_tail: str = ""
    invalid_lines: int = 0


@dataclass
class _Trip:
    lock: threading.Lock = field(default_factory=threading.Lock)
    event: threading.Event = field(default_factory=threading.Event)
    kind: str | None = None

    def set(self, kind: str) -> None:
        with self.lock:
            if self.kind is None:
                self.kind = kind
        self.event.set()


def kill_process_tree(proc: subprocess.Popen[Any]) -> None:
    """Kill the agent and every descendant; the agent was started as a group leader."""
    if os.name == "nt":
        taskkill = os.path.join(os.environ.get("SYSTEMROOT", r"C:\Windows"),
                                "System32", "taskkill.exe")
        subprocess.run([taskkill, "/T", "/F", "/PID", str(proc.pid)], capture_output=True)
        if proc.poll() is None:
            proc.kill()
        return
    try:
        os.killpg(proc.pid, signal.SIGKILL)
    except (ProcessLookupError, PermissionError):
        pass


def run_watched(argv: Sequence[str], *, cwd: str | os.PathLike[str], env: dict[str, str],
                stdin_text: str, cap: Cap, parser: StreamParser,
                transcript_path: str | os.PathLike[str] | None = None,
                poll_seconds: float = 0.05) -> ProcessResult:
    """Run the agent, feed its stdout lines to `parser` and enforce the cap.

    The prompt goes through stdin (no command-line length limit on Windows).
    Steps and tokens are checked by the reader thread after every event; the
    wall time is checked here, independently of the stream.
    """
    popen_kwargs: dict[str, Any] = {}
    if os.name == "nt":
        popen_kwargs["creationflags"] = subprocess.CREATE_NEW_PROCESS_GROUP
    else:
        popen_kwargs["start_new_session"] = True
    trip = _Trip()
    invalid = [0]
    stderr_chunks: list[str] = []
    transcript = open(transcript_path, "w", encoding="utf-8", newline="\n") \
        if transcript_path is not None else None
    start = time.monotonic()
    try:
        proc = subprocess.Popen(list(argv), cwd=cwd, env=env, stdin=subprocess.PIPE,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                encoding="utf-8", errors="replace", **popen_kwargs)
    except OSError:
        if transcript is not None:
            transcript.close()
        raise

    def read_stdout() -> None:
        assert proc.stdout is not None
        for line in proc.stdout:
            if transcript is not None:
                transcript.write(line if line.endswith("\n") else line + "\n")
            stripped = line.strip()
            if not stripped:
                continue
            try:
                obj = json.loads(stripped)
            except json.JSONDecodeError:
                invalid[0] += 1
                continue
            if not isinstance(obj, dict):
                invalid[0] += 1
                continue
            parser.feed(obj)
            if parser.steps > cap.steps:
                trip.set(CAP_STEPS)
            elif parser.tokens_total() > cap.tokens:
                trip.set(CAP_TOKENS)

    def read_stderr() -> None:
        assert proc.stderr is not None
        for chunk in proc.stderr:
            stderr_chunks.append(chunk)
            if sum(len(c) for c in stderr_chunks) > 4 * _STDERR_TAIL_CHARS:
                joined = "".join(stderr_chunks)[-_STDERR_TAIL_CHARS:]
                stderr_chunks[:] = [joined]

    def write_stdin() -> None:
        assert proc.stdin is not None
        try:
            proc.stdin.write(stdin_text)
            proc.stdin.close()
        except OSError:  # the agent exited or was killed before reading
            pass

    threads = [threading.Thread(target=f, daemon=True)
               for f in (read_stdout, read_stderr, write_stdin)]
    for thread in threads:
        thread.start()
    deadline = start + cap.seconds
    try:
        while proc.poll() is None:
            if trip.event.is_set():
                break
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                trip.set(CAP_TIME)
                break
            trip.event.wait(min(remaining, poll_seconds))
        if trip.kind is not None:
            kill_process_tree(proc)
    finally:
        if proc.poll() is None:
            kill_process_tree(proc)
        try:
            proc.wait(timeout=30)
        finally:
            wall = time.monotonic() - start
            if os.name != "nt":
                kill_process_tree(proc)  # orphaned descendants of a normal exit
            for thread in threads:
                thread.join(timeout=10)
            for stream in (proc.stdin, proc.stdout, proc.stderr):
                try:
                    if stream is not None:
                        stream.close()
                except OSError:
                    pass
            if transcript is not None:
                transcript.close()

    return ProcessResult(
        exit_status=proc.returncode,
        wall_seconds=wall,
        cap_hit=cap_hit_for(trip.kind, cap, parser, wall),
        stderr_tail="".join(stderr_chunks)[-_STDERR_TAIL_CHARS:],
        invalid_lines=invalid[0],
    )


def cap_hit_for(kind: str | None, cap: Cap, parser: StreamParser, wall: float) -> CapHit | None:
    """The cap record for a tripped cap of `kind`, or None when nothing tripped."""
    if kind is None:
        return None
    if kind == CAP_STEPS:
        limit, observed = float(cap.steps), float(parser.steps)
    elif kind == CAP_TOKENS:
        limit, observed = float(cap.tokens), float(parser.tokens_total())
    else:
        limit, observed = float(cap.seconds), wall
    return CapHit(kind=kind, limit=limit, observed=observed, overshoot=max(0.0, observed - limit))
