"""A small experiment for runner tests: source repo, catalog, fakes.

Builds, in a temporary directory, a git repository with a base revision and a
later fix commit and a two-case catalog over it. `config()` returns an
`ExperimentConfig` whose every path lies inside the temporary directory.

Transcript builders produce Claude Code stream-json lines for the scripted
adapter: a correct tool check, an answer after some reads, a free answer to a
question, a run that reads outside the tree, an API login failure.
"""
from __future__ import annotations

import json
import os
import shutil
import sys
import tempfile
from dataclasses import replace
from pathlib import Path
from typing import Any, Mapping, Sequence

from agents.base import Cap, RunRequest
from agents.scripted import ScriptedAdapter, ScriptedRun
from catalog import confirmation_fingerprint
from preflight import OUTSIDE_LABEL, PREFLIGHT_TITLE
from run import ExperimentConfig, VariantSpec
from tests.test_prepare import commit_all, git, write

TESTS = Path(__file__).resolve().parent
FAKE_RA = (sys.executable, str(TESTS / "fake_ra.py"))
FAKE_AGENT = str(TESTS / "fixtures" / "fake_agent.py")
SYNTHETIC_KEY = "SYNTHETIC-SECRET-0000"

LOAD_ACTION = """public class LoadAction
{
    public void SetScheduledTasksInvalid() { }
    public void Reload() { SetScheduledTasksInvalid(); }
}
"""
ROUNDING = """public static class Rounding
{
    public static decimal Apply(decimal value) => value;
    public static decimal Apply(decimal value, int digits) => value;
}
"""
CASE_A_PROMPT = "Zákazník A hlásí: po uložení záznamu se zobrazí chyba v plánovaných úlohách."
CASE_B_PROMPT = "Firma X hlásí: zaokrouhlený součet v přehledu nesouhlasí."
CASE_A_FILE, CASE_A_MEMBER = "Src/Placeholder/LoadAction.cs", "LoadAction.SetScheduledTasksInvalid"
QUESTION_ID, QUESTION_TEXT = "ukazka-otazka", "Ukázková otázka?"
CASE_B_FILE, CASE_B_MEMBER = "Src/Placeholder/Rounding.cs", "Rounding.Apply(+1)"
USAGE = (10, 200, 300, 40)


def fill_placeholders(text: str, values: Mapping[str, str]) -> str:
    """Replace `__NAME__` placeholders in JSON text by JSON-escaped values."""
    for placeholder, value in values.items():
        text = text.replace(placeholder, json.dumps(value, ensure_ascii=False)[1:-1])
    return text


def build_repo_from_files(repo: Path, files: Path) -> tuple[str, str]:
    """Commit `files/base` as the base revision and `files/fix` on top; return both shas.

    The commits are made with the fixed identity and dates of `test_prepare.git`.
    """
    git(repo, "init", "-q", "-b", "main")
    shas = []
    for layer in ("base", "fix"):
        source = files / layer
        for path in sorted(p for p in source.rglob("*") if p.is_file()):
            write(repo, path.relative_to(source).as_posix(), path.read_text(encoding="utf-8"))
        shas.append(commit_all(repo, layer))
    return shas[0], shas[1]


class Experiment:
    """The temporary experiment world; call `cleanup()` (or use addCleanup).

    Without `fixture` the repo and the two-case catalog are built from the
    constants above. With `fixture` (a directory like `fixtures/e2e`) they
    come from its files: `repo/base` and `repo/fix` become the two commits and
    `catalog.template.json` the catalog, with `__BASE__`, `__FIX__` and every
    placeholder of `values` filled in.
    """

    def __init__(self, cases: int = 2, fixture: Path | None = None,
                 values: Mapping[str, str] | None = None) -> None:
        self.root = Path(tempfile.mkdtemp(prefix="se-u5-"))
        self.repo = self.root / "source-repo"
        self.repo.mkdir()
        self.catalog = self.root / "catalog" / "catalog.json"
        self.catalog.parent.mkdir()
        if fixture is not None:
            self.base, self.fix = build_repo_from_files(self.repo, fixture / "repo")
            template = (fixture / "catalog.template.json").read_text(encoding="utf-8")
            data = json.loads(fill_placeholders(template, {"__BASE__": self.base,
                                                           "__FIX__": self.fix,
                                                           **(values or {})}))
            data["cases"] = data["cases"][:cases]
            self.case_ids = [c["id"] for c in data["cases"]]
            self.case_prompts = {c["id"]: c["prompt"] for c in data["cases"]}
        else:
            git(self.repo, "init", "-q", "-b", "main")
            write(self.repo, CASE_A_FILE, LOAD_ACTION)
            write(self.repo, CASE_B_FILE, ROUNDING)
            self.base = commit_all(self.repo, "base")
            write(self.repo, CASE_A_FILE, LOAD_ACTION.replace("{ }", "{ /* fixed */ }", 1))
            write(self.repo, CASE_B_FILE, ROUNDING.replace("=> value;", "=> Math.Round(value);",
                                                           1))
            self.fix = commit_all(self.repo, "fix")
            self.case_ids = ["ukazka-a", "ukazka-b"][:cases]
            all_cases = [
                self._case("ukazka-a", CASE_A_PROMPT, CASE_A_FILE, CASE_A_MEMBER, True, True),
                self._case("ukazka-b", CASE_B_PROMPT, CASE_B_FILE, CASE_B_MEMBER, False, False)]
            data = {"format_version": 1, "base_revision": self.base, "cases": all_cases[:cases]}
            self.case_prompts = {c["id"]: c["prompt"] for c in data["cases"]}
        self.catalog.write_text(json.dumps(data, ensure_ascii=False), encoding="utf-8")
        self.question_prompts: dict[str, str] = {}
        self.output = self.root / "out"
        self.export = self.root / "export"
        self.isolation = self.root / "iso"
        self.credentials = self.root / "login" / ".credentials.json"
        self.credentials.parent.mkdir()
        self.credentials.write_text('{"token": "%s"}' % SYNTHETIC_KEY, encoding="utf-8")

    def cleanup(self) -> None:
        shutil.rmtree(self.root, ignore_errors=True)

    def _case(self, case_id: str, prompt: str, file: str, member: str, tuning: bool,
              names: bool) -> dict[str, Any]:
        return {"id": case_id, "prompt": prompt,
                "acceptable_locations": [{"file": file, "member": member, "line": 3,
                                          "fix_commit": self.fix}],
                "bug_confirmed_at_base": True, "pseudonymization_confirmed": True,
                "used_for_card_tuning": tuning, "prompt_names_location": names}

    # -- variants ---------------------------------------------------------------

    def corpus_variant(self, name: str, corpus: str, ra_options: Sequence[str] = (),
                       ra_command: Sequence[str] | None = None) -> VariantSpec:
        """A variant with its own corpus directory under the temporary root."""
        return VariantSpec(name, corpus_dir=self.root / corpus, ra_options=tuple(ra_options),
                           ra_command=tuple(ra_command) if ra_command is not None else None)

    def other_ra(self, label: str) -> tuple[str, ...]:
        """The command of the fake RA playing another version of RA."""
        return (*FAKE_RA, "--ra-version", label)

    @property
    def support_workdir(self) -> Path:
        """The empty work directory of the support role (a sibling of the export)."""
        return self.export.with_name(self.export.name + "-podpora-prazdny")

    def support_variants(self) -> tuple[VariantSpec, VariantSpec]:
        """Two corpora, the pair the support role needs."""
        return (self.corpus_variant("korpus A", "corpus-a"),
                self.corpus_variant("korpus B", "corpus-b", ["--marker-b"]))

    def add_questions(self, bugs: bool = True, line: int = 4) -> None:
        """Make the catalog a format 2 one with a question (and the bugs, unless `bugs` is off).

        `line` is the line of the question's one fact in `LoadAction.Reload`."""
        data = json.loads(self.catalog.read_text(encoding="utf-8"))
        kept = [dict(c, druh="chyba") for c in data["cases"]] if bugs else []
        facts = [{"id": "f1", "text": "Ukázkový fakt.", "role": "misto",
                  "mista": [{"soubor": CASE_A_FILE, "clen": "LoadAction.Reload", "radky": line}]}]
        question = {"druh": "otazka", "id": QUESTION_ID, "dotaz": QUESTION_TEXT,
                    "ramcova_odpoved": "Ukázková odpověď.", "commit": self.base,
                    "fakta_potvrzena": True, "pseudonymizace_potvrzena": True,
                    "kontrolni": False, "fakta": facts, "zakazana_tvrzeni": [],
                    "otisk_potvrzeni": confirmation_fingerprint(facts, [])}
        data.update(format_version=2, cases=[*kept[:1], question, *kept[1:]])
        self.catalog.write_text(json.dumps(data, ensure_ascii=False), encoding="utf-8")
        self.question_prompts = {QUESTION_ID: QUESTION_TEXT}

    def default_variants(self) -> tuple[VariantSpec, VariantSpec]:
        return (VariantSpec("bez karet"), self.corpus_variant("s kartami", "corpus"))

    def config(self, variants: Sequence[VariantSpec] | None = None, seed: int = 7,
               cap: Cap = Cap(seconds=600, steps=50, tokens=100_000),
               role: str = "vyvojar", **overrides: Any) -> ExperimentConfig:
        config = ExperimentConfig(
            catalog_path=self.catalog, source_repo=self.repo, output_root=self.output,
            export_dir=self.export, isolation_root=self.isolation, ra_command=FAKE_RA,
            model="model-test", cap=cap, seed=seed,
            variants=tuple(variants or self.default_variants()), role=role,
            credentials=self.credentials)
        return replace(config, **overrides)

    def case_of(self, request: RunRequest) -> str:
        """Which catalog item (case or question) a run prompt is for."""
        return next(item_id for item_id, prompt in
                    {**self.case_prompts, **self.question_prompts}.items()
                    if prompt in request.prompt)

    def is_question(self, request: RunRequest) -> bool:
        return self.case_of(request) in self.question_prompts

    def scripted(self, script) -> ScriptedAdapter:
        return ScriptedAdapter(script, self.isolation,
                               keep_apart=(self.output, self.catalog.parent))


# --- transcripts ---------------------------------------------------------------

def _line(obj: dict[str, Any]) -> str:
    return json.dumps(obj, ensure_ascii=False)


def _usage(values: Sequence[int | None]) -> dict[str, int]:
    keys = ("input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens",
            "output_tokens")
    return {k: v for k, v in zip(keys, values) if v is not None}


def assistant(message_id: str, content: list[dict[str, Any]],
              usage: Sequence[int | None] | None = USAGE) -> str:
    message: dict[str, Any] = {"id": message_id, "role": "assistant", "content": content}
    if usage is not None:
        message["usage"] = _usage(usage)
    return _line({"type": "assistant", "parent_tool_use_id": None, "message": message})


def tool_result(tool_id: str, content: str, is_error: bool = False) -> str:
    return _line({"type": "user", "parent_tool_use_id": None,
                  "message": {"role": "user", "content": [
                      {"type": "tool_result", "tool_use_id": tool_id, "content": content,
                       "is_error": is_error}]}})


def init(tools: Sequence[str], servers: Sequence[str] = ()) -> str:
    return _line({"type": "system", "subtype": "init", "tools": list(tools),
                  "mcp_servers": [{"name": s, "status": "connected"} for s in servers],
                  "model": "model-test", "claude_code_version": "0.0.0-scripted"})


def result(usage: Sequence[int | None] | None = USAGE, is_error: bool = False,
           text: str = "hotovo") -> str:
    event: dict[str, Any] = {"type": "result", "subtype": "success", "is_error": is_error,
                             "result": text}
    if usage is not None:
        event["usage"] = _usage(usage)
    return _line(event)


def read(tool_id: str, path: str) -> dict[str, Any]:
    return {"type": "tool_use", "id": tool_id, "name": "Read", "input": {"file_path": path}}


def answer_block(file: str, member: str) -> str:
    body = json.dumps({"file": file, "member": member, "hypothesis": "Příčina je zde."},
                      ensure_ascii=False)
    return "Závěr:\n```json\n" + body + "\n```"


def answer_run(file: str, member: str, *, reads: Sequence[str] = (),
               usage: Sequence[int | None] = USAGE, read_text: str = "obsah",
               wall_seconds: float = 1.0) -> ScriptedRun:
    """A run that reads `reads`, then answers."""
    lines = [init(["Glob", "Grep", "Read"])]
    for index, path in enumerate(reads):
        lines += [assistant(f"m{index}", [read(f"t{index}", path)], usage),
                  tool_result(f"t{index}", read_text)]
    lines += [assistant("final", [{"type": "text", "text": answer_block(file, member)}], usage),
              result(usage)]
    return ScriptedRun(tuple(lines), wall_seconds=wall_seconds)


FREE_ANSWER = ("Metoda `LoadAction.Reload` zavolá `Src/Placeholder/LoadAction.cs:4` a nic jiného "
               "neudělá.")


def free_answer_run(text: str = FREE_ANSWER, *, reads: Sequence[str] = (),
                    usage: Sequence[int | None] = USAGE, read_text: str = "obsah",
                    wall_seconds: float = 1.0) -> ScriptedRun:
    """A run that reads `reads`, then answers a question in free text (no JSON block)."""
    lines = [init(["Glob", "Grep", "Read"])]
    for index, path in enumerate(reads):
        lines += [assistant(f"m{index}", [read(f"t{index}", path)], usage),
                  tool_result(f"t{index}", read_text)]
    lines += [assistant("final", [{"type": "text", "text": text}], usage), result(usage)]
    return ScriptedRun(tuple(lines), wall_seconds=wall_seconds)


def auth_error_run() -> ScriptedRun:
    text = "Invalid API key · Please run /login"
    return ScriptedRun((init(["Glob", "Grep", "Read"]),
                        assistant("err", [{"type": "text", "text": text}], None),
                        result(None, is_error=True, text=text)), exit_status=1)


def preflight_run(request: RunRequest, leak: bool = False) -> ScriptedRun:
    """A correct tool-check transcript for this request (or one that leaks the marker)."""
    outside = next(line[len(OUTSIDE_LABEL):].strip() for line in request.prompt.splitlines()
                   if line.startswith(OUTSIDE_LABEL))
    # The refusal as Claude Code 2.1.287 words it under --restricted.
    secret = (Path(outside).read_text(encoding="utf-8") if leak else
              f"{outside} is outside {request.workdir}; --restricted confines the file "
              "tools to the working directory.")
    lines = [init(["Glob", "Grep", "Read"]),
             assistant("m1", [read("t1", str(request.workdir / "README.txt"))]),
             tool_result("t1", "Kontrolní soubor."),
             assistant("m2", [read("t2", outside)]),
             tool_result("t2", secret, is_error=not leak),
             assistant("m3", [{"type": "text", "text": answer_block("README.txt", "Kontrola")}]),
             result()]
    return ScriptedRun(tuple(lines))


def is_preflight(request: RunRequest) -> bool:
    return PREFLIGHT_TITLE in request.prompt


def correct_answer(case_id: str) -> tuple[str, str]:
    return (CASE_A_FILE, CASE_A_MEMBER) if case_id == "ukazka-a" else (CASE_B_FILE, "Apply")


def fake_agent_env() -> dict[str, str]:
    return {**os.environ, "CLAUDECODE": "1"}
