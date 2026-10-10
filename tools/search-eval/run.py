"""Entry point of the search-eval experiment: prepare, check, schedule, run, report (U5).

    python tools/search-eval/run.py --help

Catalog items are of two kinds. A `chyba` (a solved case) is answered with a
JSON block naming the cause and scored here. An `otazka` (a question, KTD10) is
answered in free text, citing places as anchors in backticks (`path:N` or
`path:N-M`); the run keeps the agent's last top-level message as `answer_text`
and is not scored here: `answers.py hodnot-odpovedi` judges it later against
the facts of the question. Both kinds are in the schedule, the prompts and the
report (questions apart, with run counts only).

Flow (High-Level Technical Design of the plan):

1. **Inputs.** The catalog, the two variants and every path are validated
   before anything is written. The output root, the export, the isolation
   root and each corpus are canonicalized (symlinks and junctions resolved)
   and refused inside this repository or the source repository and any of
   their worktrees (KTD9); the catalog is refused there too, before it is
   read (KTD13). The
   output root and the catalog are also refused inside the work tree or a
   corpus: the agent reads those freely, and the audit cannot see a leak that
   stays inside the allowed roots. A configuration directory left over from
   an earlier run stops the start (KTD9).
2. **Preparation** (`prepare.py`): export of the base revision, case checks
   and each variant's corpus (loaded against its receipt, or generated). Its
   costs are recorded separately and never mixed into run medians (KTD11).
   `--prepare-only` stops here, so the operator can inspect the receipted
   corpus before the experiment.
3. **Tool check** (`preflight.py`, KTD10) once per variant through the same
   adapter; any failure is an infrastructure error and nothing runs.
4. **Schedule**: case × variant × 3, shuffled with the recorded seed, run one
   after another (concurrency 1), no warm-up (KTD11). A spoiled run is
   replaced by a fresh attempt appended to the end of the queue, at most
   twice per case and variant (KTD7).
5. **Runs**: each run's record goes through the infrastructure check, the
   audit and scoring. An infrastructure failure (login, API outage, rate
   limit, a CLI that did not run) is not the agent's failure: the experiment
   stops, the run counts nowhere and the partial report says so.
6. **After the runs** `report.py` writes the report.

Variants. A variant is a name and a corpus or none, optionally with its own RA
options and its own RA command (`ra_command`, so two versions of RA can be
compared; without it the experiment's `--ra-binary` generates the corpus).
Exactly two are compared: the first is the reference, the second the compared
one (KTD7 tallies are pairwise). The default pair is "bez karet" (sources only)
and "s kartami" (sources + the corpus, which the agent reads as files). No
variant has an MCP server.

Role (KTD9, `--role`, mandatory, no default). `vyvojar` is a reader with the
sources: the agent works in the exported tree (the setting of every earlier
experiment) and the default pair of variants applies; both kinds of item are
measured. `podpora` is a reader without the sources: the agent's work
directory is an empty directory of its own (a sibling of the export, named
after it) and the corpus is read through `--add-dir`; the audit permits only
the corpus there. Items of kind `chyba` are not measured in this role (they are
listed in the report as "v roli podpora se neměří"); the questions are its only
measured items, so a catalog without one is refused. The default pair has no meaning there ("bez karet" would have nothing
to read): both variants are given explicitly, each with a corpus.

The prompt is identical across variants word for word except for the
paragraph about available sources (R6), for a question as for a case, and is
stored verbatim (KTD8 point 5).

Exit codes: 0 done (or dry run / preparation only), 2 invalid input or
preparation refused, 3 infrastructure error (tool check or a run), 130
interrupted.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import random
import re
import sys
import time
import unicodedata
from collections import Counter, deque
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path
from typing import Any, Callable, Sequence

# Allow `python tools/search-eval/run.py` from anywhere: the modules sit beside it.
sys.path.insert(0, str(Path(__file__).resolve().parent))

import prepare  # noqa: E402
import report  # noqa: E402
from agents.base import (CONFIG_DIR_NAME, AgentAdapter, AgentRunError, Cap,  # noqa: E402
                         RunRecord, RunRequest)
from agents.claude_code import (CREDENTIALS_FILE, ClaudeCodeAdapter,  # noqa: E402
                                ClaudeCodeSettings)
from audit import audit_run  # noqa: E402
from catalog import Case, Catalog, CatalogError, Question, load_catalog  # noqa: E402
from preflight import (InfrastructureError, PreflightResult, PreflightTarget,  # noqa: E402
                       require_ok, results_json, run_preflight)
from prepare import (PrepareError, canonical_path, ensure_outside,  # noqa: E402
                     protected_roots)
from report import (INFRASTRUCTURE, ROLE_SUPPORT, ROLES,  # noqa: E402
                    SPOILED, STATUS_COMPLETED, STATUS_INFRASTRUCTURE, STATUS_INTERRUPTED,
                    CaseInfo, ExperimentInfo, PreparationCost, RunResult, Tariff, VariantInfo,
                    write_text_atomic)
from prompts import (PATH_BASE, PROMPT_PLACEHOLDER, PROMPT_TEMPLATE,  # noqa: E402,F401
                     QUESTION_PROMPT_TEMPLATE, build_prompt, item_text,
                     prompt_template, sources_paragraph)
from score import evaluate_run  # noqa: E402

RUNS_PER_VARIANT = report.RUNS_PER_VARIANT
MAX_REPLACEMENTS = 2
VARIANTS_FORMAT_VERSION = 1
DEFAULT_VARIANTS = ("bez karet", "s kartami")


class ConfigError(Exception):
    """The experiment's inputs are invalid (Czech message for the operator)."""


# --- variants ----------------------------------------------------------------

@dataclass(frozen=True)
class VariantSpec:
    """One variant: name and its corpus (or none).

    `ra_command` is the command (binary and fixed arguments) that generates this
    variant's corpus; None means the experiment's `ra_command`.
    """

    name: str
    corpus_dir: Path | None = None
    ra_options: tuple[str, ...] = ()
    card_format: str | None = None
    ra_command: tuple[str, ...] | None = None

    @property
    def slug(self) -> str:
        return slugify(self.name)


def slugify(text: str) -> str:
    """ASCII file-name form of a name (`s kartami` -> `s-kartami`)."""
    ascii_text = unicodedata.normalize("NFKD", text).encode("ascii", "ignore").decode("ascii")
    return re.sub(r"[^a-z0-9]+", "-", ascii_text.lower()).strip("-") or "x"


def parse_variants(data: Any, base_dir: Path) -> tuple[VariantSpec, ...]:
    """Variants file: `{"format_version": 1, "variants": [{name, corpus_dir, ra_options,
    ra_command, card_format}, ...]}`; relative corpus paths are relative to the file.
    `ra_command` is a non-empty list of texts (binary and arguments), taken as written."""
    if not isinstance(data, dict) or data.get("format_version") != VARIANTS_FORMAT_VERSION:
        raise ConfigError(f"Varianty: pole „format_version“ musí být {VARIANTS_FORMAT_VERSION}.")
    raw = data.get("variants")
    if not isinstance(raw, list):
        raise ConfigError("Varianty: pole „variants“ musí být seznam.")
    variants = []
    for index, item in enumerate(raw):
        where = f"Varianta č. {index + 1}"
        if not isinstance(item, dict) or not isinstance(item.get("name"), str) \
                or not item["name"].strip():
            raise ConfigError(f"{where}: chybí pole „name“.")
        corpus = item.get("corpus_dir")
        if corpus is not None and (not isinstance(corpus, str) or not corpus):
            raise ConfigError(f"{where}: pole „corpus_dir“ musí být cesta nebo null.")
        options = item.get("ra_options", [])
        if not isinstance(options, list) or not all(isinstance(o, str) for o in options):
            raise ConfigError(f"{where}: pole „ra_options“ musí být seznam textů.")
        command = item.get("ra_command")
        if command is not None and (not isinstance(command, list) or not command
                                    or not all(isinstance(c, str) and c for c in command)):
            raise ConfigError(f"{where}: pole „ra_command“ musí být neprázdný seznam textů "
                              "(binárka a její argumenty) nebo null.")
        card_format = item.get("card_format")
        variants.append(VariantSpec(name=item["name"].strip(),
                                    corpus_dir=base_dir / corpus if corpus is not None else None,
                                    ra_options=tuple(options),
                                    card_format=card_format if isinstance(card_format, str)
                                    else None,
                                    ra_command=tuple(command) if command is not None else None))
    return validate_variants(variants)


def validate_variants(variants: Sequence[VariantSpec],
                      role: str | None = None) -> tuple[VariantSpec, ...]:
    """Exactly two variants with distinct names; RA options and command only with a corpus.

    With `role` the variants are also checked against it: the support role reads
    nothing but corpora, so a variant without one has nothing to read."""
    if len(variants) != 2:
        raise ConfigError(f"Pokus porovnává právě dvě varianty (referenční a srovnávanou), "
                          f"zadáno {len(variants)}.")
    names = [v.name for v in variants]
    if len(set(names)) != len(names) or len({v.slug for v in variants}) != len(names):
        raise ConfigError(f"Varianty musí mít různá jména: {', '.join(names)}.")
    for v in variants:
        if v.ra_options and v.corpus_dir is None:
            raise ConfigError(f"Varianta {v.name}: volby RA bez korpusu nedávají smysl.")
        if v.ra_command is not None and v.corpus_dir is None:
            raise ConfigError(f"Varianta {v.name}: příkaz RA bez korpusu nedává smysl.")
        if role == ROLE_SUPPORT and v.corpus_dir is None:
            raise ConfigError(f"Varianta {v.name} nemá korpus: v roli podpora agent nemá zdrojáky "
                              "a bez korpusu by neměl co číst. Každá varianta potřebuje korpus.")
    return tuple(variants)


def default_variants(corpus_dir: Path | None, ra_options: Sequence[str] = ()
                     ) -> tuple[VariantSpec, ...]:
    """The default pair: sources only, and sources + corpus (R4)."""
    if corpus_dir is None:
        raise ConfigError("Bez souboru variant je potřeba --corpus-dir pro variantu "
                          "„s kartami“.")
    return validate_variants([
        VariantSpec(DEFAULT_VARIANTS[0]),
        VariantSpec(DEFAULT_VARIANTS[1], corpus_dir=corpus_dir, ra_options=tuple(ra_options)),
    ])


# --- configuration -----------------------------------------------------------

@dataclass(frozen=True)
class ExperimentConfig:
    """Everything fixed for one experiment (R6)."""

    catalog_path: Path
    source_repo: Path
    output_root: Path
    export_dir: Path
    isolation_root: Path
    ra_command: tuple[str, ...]
    model: str
    cap: Cap
    seed: int
    variants: tuple[VariantSpec, ...]
    role: str
    tariff: Tariff | None = None
    claude_executable: tuple[str, ...] = ("claude",)
    credentials: Path | None = None
    forbidden_dir_names: tuple[str, ...] = ()


@dataclass(frozen=True)
class ResolvedPaths:
    """Canonical paths after the checks of step 1 (module docstring)."""

    output_root: Path
    export_dir: Path
    isolation_root: Path
    corpora: dict[str, Path] = field(default_factory=dict)
    support_workdir: Path | None = None


def _within(path: str, root: str) -> bool:
    try:
        return os.path.commonpath([path, root]) == root
    except ValueError:  # different drives
        return False


def _overlap(a: str, b: str) -> bool:
    return _within(a, b) or _within(b, a)


def support_workdir_path(export_dir: str | os.PathLike[str]) -> Path:
    """The empty work directory of the support role: a sibling of the export."""
    export = Path(export_dir)
    return export.with_name(export.name + "-podpora-prazdny")


def validate_role(role: str) -> None:
    if role not in ROLES:
        raise ConfigError(f"Role musí být {' nebo '.join(ROLES)}, zadáno „{role}“.")


def _protected(config: ExperimentConfig) -> list[str]:
    """The roots no file of the experiment may lie under: the source repo, this repo, worktrees."""
    try:
        return protected_roots(prepare.repo_toplevel(config.source_repo))
    except PrepareError as exc:
        raise ConfigError(str(exc)) from None


def resolve_paths(config: ExperimentConfig) -> ResolvedPaths:
    """Canonicalize and check every location before anything is written (KTD9).

    In the support role the agent's empty work directory is one more location
    the agent reads, so it gets the same checks as the export."""
    roots = _protected(config)
    names = config.forbidden_dir_names
    try:
        ensure_outside(config.catalog_path, roots, "Katalog")
        output = ensure_outside(config.output_root, roots, "Kořen výstupu", names)
        export = ensure_outside(config.export_dir, roots, "Adresář exportu", names)
        isolation = ensure_outside(config.isolation_root, roots, "Kořen izolace", names)
        corpora = {v.name: ensure_outside(v.corpus_dir, roots, f"Korpus varianty {v.name}",
                                          names)
                   for v in config.variants if v.corpus_dir is not None}
    except PrepareError as exc:
        raise ConfigError(str(exc)) from None
    support = None
    if config.role == ROLE_SUPPORT:
        try:
            support = ensure_outside(support_workdir_path(config.export_dir), roots,
                                     "Pracovní adresář role podpora", names)
        except PrepareError as exc:
            raise ConfigError(str(exc)) from None
        if os.path.lexists(support) and (not os.path.isdir(support) or os.listdir(support)):
            raise ConfigError(f"Pracovní adresář role podpora {support} už existuje a není "
                              "prázdný; zbytek po předchozím běhu?")
    agent_reads = {"adresář exportu": str(export),
                   **({"pracovní adresář role podpora": str(support)} if support else {}),
                   **{f"korpus varianty {n}": str(p) for n, p in corpora.items()}}
    catalog = canonical_path(config.catalog_path)
    for what, readable in agent_reads.items():
        if _overlap(str(output), readable):
            raise ConfigError(f"Kořen výstupu {output} se překrývá s {what} {readable}: agent by "
                              "četl přepisy a výsledky jiných běhů.")
        if _within(catalog, readable):
            raise ConfigError(f"Katalog {catalog} leží v {what} {readable}: agent by četl "
                              "odpovědi.")
        if _overlap(str(isolation), readable):
            raise ConfigError(f"Kořen izolace {isolation} se překrývá s {what} {readable}.")
    if _overlap(str(isolation), str(output)):
        raise ConfigError(f"Kořen izolace {isolation} se překrývá s kořenem výstupu {output}.")
    for name, corpus in corpora.items():
        if _overlap(str(corpus), str(export)):
            raise ConfigError(f"Korpus varianty {name} se překrývá s adresářem exportu.")
        if support is not None and _overlap(str(corpus), str(support)):
            raise ConfigError(f"Korpus varianty {name} se překrývá s pracovním adresářem role "
                              "podpora.")
    if support is not None and _overlap(str(support), str(export)):
        raise ConfigError("Pracovní adresář role podpora se překrývá s adresářem exportu: agent "
                          "by četl zdrojáky.")
    leftover = isolation / CONFIG_DIR_NAME
    if os.path.lexists(leftover):
        raise ConfigError(f"V {leftover} zůstal adresář nastavení z předchozího běhu (mohl "
                          "obsahovat kopii přihlášení). Ověř, že žádný běh neběží, smaž ho ručně "
                          "a spusť znovu.")
    # Checked in canonical (case-folded) form; handed on case-preserving, since
    # the corpus path is shown to the agent in the prompt.
    def real(path: Path | None) -> Path:
        return Path(os.path.realpath(path))
    return ResolvedPaths(real(config.output_root), real(config.export_dir),
                         real(config.isolation_root),
                         {v.name: real(v.corpus_dir) for v in config.variants
                          if v.corpus_dir is not None},
                         real(support_workdir_path(config.export_dir)) if support else None)


# --- schedule and prompt -----------------------------------------------------

@dataclass(frozen=True)
class ScheduleEntry:
    """One planned clean run: case, variant and repetition (1..3)."""

    case_id: str
    variant: str
    repetition: int


def build_schedule(cases: Sequence[str | Case | Question], variant_names: Sequence[str],
                   seed: int) -> list[ScheduleEntry]:
    """item × variant × 3 in catalog and variant order, shuffled by `random.Random(seed)`.

    Items are ids or catalog items of either kind (a question is scheduled like a case).
    """
    case_ids = [c if isinstance(c, str) else c.id for c in cases]
    entries = [ScheduleEntry(c, v, r) for c in case_ids for v in variant_names
               for r in range(1, RUNS_PER_VARIANT + 1)]
    random.Random(seed).shuffle(entries)
    return entries


# --- outcome -----------------------------------------------------------------

@dataclass(frozen=True)
class ExperimentOutcome:
    """A finished experiment (or preparation): its status, directory and report data."""

    status: str
    directory: Path
    report: dict[str, Any] | None = None


class ExperimentStopped(Exception):
    """The experiment ended without a valid result; the report (if any) is written.

    `exit_code` is the CLI's code; the message is Czech.
    """

    def __init__(self, message: str, directory: Path, exit_code: int) -> None:
        super().__init__(message)
        self.directory = directory
        self.exit_code = exit_code


# --- the experiment ----------------------------------------------------------

AdapterFactory = Callable[[ExperimentConfig, ResolvedPaths], AgentAdapter]


def default_adapter(config: ExperimentConfig, paths: ResolvedPaths) -> AgentAdapter:
    """Claude Code with the experiment's model, login copy and isolation root."""
    settings = ClaudeCodeSettings(
        model=config.model, isolation_root=paths.isolation_root,
        executable=config.claude_executable,
        credentials_source=config.credentials or Path.home() / ".claude" / CREDENTIALS_FILE,
        keep_apart=(paths.output_root, Path(config.catalog_path).resolve().parent))
    return ClaudeCodeAdapter(settings)


def run_experiment(config: ExperimentConfig, *, prepare_only: bool = False,
                   adapter_factory: AdapterFactory = default_adapter,
                   now: Callable[[], datetime] = datetime.now,
                   preflight_token: str | None = None) -> ExperimentOutcome:
    """Run the whole flow of the module docstring; raise `ExperimentStopped` on a stop."""
    validate_role(config.role)
    validate_variants(config.variants, config.role)
    catalog = _load_catalog(config)
    paths = resolve_paths(config)
    started = now()
    directory = paths.output_root / f"pokus-{started:%Y%m%d-%H%M%S}-seed{config.seed}"
    directory.mkdir(parents=True, exist_ok=False)
    experiment = _Experiment(config, catalog, paths, directory, started, now)
    experiment.prepare()
    if prepare_only:
        return ExperimentOutcome("prepared", directory)
    adapter = adapter_factory(config, paths)
    experiment.preflight(adapter, preflight_token)
    return experiment.execute(adapter)


def _load_catalog(config: ExperimentConfig) -> Catalog:
    """The catalog, if the role has something to measure in it.

    The catalog's location is checked before it is read (KTD13): inside a repository it holds
    customer data one commit from the public history. What is measured is decided by the
    role: questions in both, `chyba` items in the developer role only."""
    try:
        ensure_outside(config.catalog_path, _protected(config), "Katalog")
    except PrepareError as exc:
        raise ConfigError(str(exc)) from None
    try:
        catalog = load_catalog(config.catalog_path)
    except CatalogError as exc:
        raise ConfigError(str(exc)) from None
    if config.role == ROLE_SUPPORT:
        if not catalog.questions:
            raise ConfigError("V roli podpora se položky druhu „chyba“ neměří a katalog nemá "
                              "žádnou jinou položku k měření: pokus nemá co měřit.")
    elif not catalog.cases and not catalog.questions:
        raise ConfigError("Katalog nemá žádnou položku druhu „chyba“ ani „otazka“: pokus nemá co "
                          "měřit.")
    return catalog


def measured_cases(catalog: Catalog, role: str) -> tuple[Case | Question, ...]:
    """The items the experiment runs in this role: the `chyba` items (none in the support
    role) and then the questions."""
    cases = () if role == ROLE_SUPPORT else tuple(catalog.cases)
    return (*cases, *catalog.questions)


def not_measured_cases(catalog: Catalog, role: str) -> tuple[Case, ...]:
    """The `chyba` items set aside by the role: reported as "v roli podpora se neměří"."""
    return tuple(catalog.cases) if role == ROLE_SUPPORT else ()


class _Experiment:
    """State of one experiment between the steps of `run_experiment`."""

    def __init__(self, config: ExperimentConfig, catalog: Catalog, paths: ResolvedPaths,
                 directory: Path, started: datetime, now: Callable[[], datetime]) -> None:
        self.config = config
        self.catalog = catalog
        self.paths = paths
        self.directory = directory
        self.started = started
        self.now = now
        self.costs: list[PreparationCost] = []
        self.receipts: dict[str, prepare.CorpusReceipt] = {}
        self.preflight_results: list[PreflightResult] = []
        self.results: list[RunResult] = []
        self.export: prepare.ExportResult | None = None
        self.measured = measured_cases(catalog, config.role)
        self.templates: dict[str, str] = {}
        self.question_templates: dict[str, str] = {}
        self.adapter_name = ""

    # -- step 2: preparation ------------------------------------------------

    def prepare(self) -> None:
        config, paths = self.config, self.paths
        guard = [paths.output_root, paths.isolation_root]
        try:
            with self._timed("Export stromu základní revize"):
                self.export = prepare.export_tree(config.source_repo, self.catalog.base_revision,
                                                  paths.export_dir, forbidden_roots=guard,
                                                  forbidden_names=config.forbidden_dir_names)
            with self._timed("Kontrola případů"):
                prepare.check_cases(config.source_repo, self.catalog, self.export.tree_dir,
                                    self.catalog.base_revision)
            if paths.support_workdir is not None:
                paths.support_workdir.mkdir(parents=True, exist_ok=True)
            for variant in config.variants:
                if variant.corpus_dir is not None:
                    self._corpus(variant, guard)
        except PrepareError as exc:
            self._write_json("preparation.json", self._preparation_json(str(exc)))
            raise ExperimentStopped(f"Příprava pokusu odmítla vstup: {exc}", self.directory,
                                    2) from None
        self._write_json("preparation.json", self._preparation_json(None))

    def _corpus(self, variant: VariantSpec, guard: list[Path]) -> None:
        assert self.export is not None
        corpus = self.paths.corpora[variant.name]
        if prepare.receipt_path(corpus).exists():
            receipt = prepare.load_corpus(corpus, self.export.tree_fingerprint)
            if receipt.options != variant.ra_options:
                raise PrepareError(f"Korpus varianty {variant.name} vznikl s volbami "
                                   f"{list(receipt.options)}, varianta chce "
                                   f"{list(variant.ra_options)}.")
            note = "korpus už existoval, čas generování z účtenky"
        else:
            command = variant.ra_command or self.config.ra_command
            receipt = prepare.generate_corpus(command, self.export, corpus,
                                              variant.ra_options, forbidden_roots=guard,
                                              forbidden_names=self.config.forbidden_dir_names)
            note = "korpus vygenerován v tomto běhu"
        self.receipts[variant.name] = receipt
        self.costs.append(PreparationCost(f"Korpus varianty {variant.name}: generování",
                                          receipt.generation_seconds, note=note))

    # -- step 3: tool check -------------------------------------------------

    def preflight(self, adapter: AgentAdapter, token: str | None) -> None:
        base = self.directory / "preflight"
        base.mkdir()
        for variant in self.config.variants:
            target = PreflightTarget(name=variant.name, slug=variant.slug,
                                     with_corpus=variant.corpus_dir is not None,
                                     with_sources=self.config.role != ROLE_SUPPORT)
            result = run_preflight(adapter, target, base, self.config.cap, token)
            self.preflight_results.append(result)
            self.costs.append(PreparationCost(f"Kontrola nástroje ({variant.name})",
                                              result.wall_seconds, result.tokens))
        write_text_atomic(self.directory / "preflight.json", results_json(self.preflight_results))
        self._write_json("preparation.json", self._preparation_json(None))
        try:
            require_ok(self.preflight_results)
        except InfrastructureError as exc:
            raise ExperimentStopped(str(exc), self.directory, 3) from None

    # -- steps 4-6: schedule, runs, report -----------------------------------

    def execute(self, adapter: AgentAdapter) -> ExperimentOutcome:
        self.adapter_name = adapter.name
        self._write_prompts()
        cases = {c.id: c for c in self.measured}
        names = [v.name for v in self.config.variants]
        planned = build_schedule(self.measured, names, self.config.seed)
        queue: deque[tuple[ScheduleEntry, int | None]] = deque((e, None) for e in planned)
        attempts: Counter[tuple[str, str]] = Counter()
        replacements: Counter[tuple[str, str]] = Counter()
        self._write_schedule(planned)
        status, detail = STATUS_COMPLETED, None
        try:
            while queue:
                entry, replaces = queue.popleft()
                key = (entry.case_id, entry.variant)
                attempts[key] += 1
                result = self._run_one(adapter, cases[entry.case_id], entry, attempts[key],
                                       replaces)
                self.results.append(result)
                self._write_json(f"runs/{result.order:03d}.result.json", result.to_json())
                self._write_schedule(planned)
                if result.state == INFRASTRUCTURE:
                    status = STATUS_INFRASTRUCTURE
                    detail = (f"Běh {result.order} ({result.case_id}, {result.variant}): "
                              f"{result.reason}")
                    break
                if result.state == SPOILED and replacements[key] < MAX_REPLACEMENTS:
                    replacements[key] += 1
                    queue.append((entry, result.order))
        except KeyboardInterrupt:
            self._report(STATUS_INTERRUPTED, "Pokus přerušil operátor.", best_effort=True)
            raise
        except Exception as exc:
            # An unexpected error (a process that survived its kill, a full
            # disk, a bug) still leaves a partial report over the runs already
            # on disk, so the experiment stays diagnosable; then it propagates.
            self._report(STATUS_INFRASTRUCTURE, f"Pokus skončil neočekávanou chybou "
                         f"{type(exc).__name__}: {exc}", best_effort=True)
            raise
        data = self._report(status, detail)
        if status == STATUS_INFRASTRUCTURE:
            raise ExperimentStopped(f"Pokus se zastavil chybou infrastruktury. {detail}",
                                    self.directory, 3)
        return ExperimentOutcome(status, self.directory, data)

    def _template(self, variant: str, item: Case | Question) -> str:
        return (self.question_templates if isinstance(item, Question)
                else self.templates)[variant]

    def _run_one(self, adapter: AgentAdapter, case: Case | Question, entry: ScheduleEntry, attempt: int,
                 replaces: int | None) -> RunResult:
        order = len(self.results) + 1
        variant = self._variant(entry.variant)
        transcript = (f"runs/{order:03d}-{slugify(case.id)}-{variant.slug}-{attempt}.jsonl")
        assert self.export is not None
        request = RunRequest(
            workdir=self._workdir(), prompt=build_prompt(self._template(variant.name, case), case),
            cap=self.config.cap, corpus_dir=self.paths.corpora.get(variant.name),
            transcript_path=self.directory / transcript)
        common = dict(order=order, case_id=case.id, variant=variant.name, attempt=attempt,
                      replaces=replaces, transcript=transcript)
        try:
            record = adapter.run(request)
        except (AgentRunError, OSError) as exc:
            return RunResult(state=INFRASTRUCTURE, reason=f"Agent nejde spustit: {exc}", **common)
        measured = dict(tokens=record.tokens, steps=record.steps,
                        wall_seconds=record.wall_seconds, cap_hit=_cap_json(record),
                        result_error=record.result_error, **common)
        problem = adapter.infrastructure_problem(record)
        if problem is not None:
            return RunResult(state=INFRASTRUCTURE, reason=problem, **measured)
        outcome = evaluate_run(record, case, audit_run(
            record, corpus_only=self.config.role == ROLE_SUPPORT))
        answer = None
        if outcome.answer is not None:
            answer = {"file": outcome.answer.file, "member": outcome.answer.member,
                      "hypothesis": outcome.answer.hypothesis}
        return RunResult(state=outcome.state, reason=outcome.failure_reason,
                         spoil_reasons=tuple(f"{f.kind}: {f.detail}"
                                             for f in outcome.spoil_reasons),
                         answer=answer, answer_text=outcome.answer_text, **measured)

    def _workdir(self) -> Path:
        """The agent's work directory: the exported tree, or in the support role an empty one."""
        assert self.export is not None
        if self.config.role == ROLE_SUPPORT:
            assert self.paths.support_workdir is not None
            return self.paths.support_workdir
        return self.export.tree_dir

    def _variant(self, name: str) -> VariantSpec:
        return next(v for v in self.config.variants if v.name == name)

    # -- artifacts ----------------------------------------------------------

    def _write_prompts(self) -> None:
        (self.directory / "prompts").mkdir()
        (self.directory / "runs").mkdir()
        for variant in self.config.variants:
            corpus = self.paths.corpora.get(variant.name)
            sources = sources_paragraph(corpus, self.config.role)
            self.templates[variant.name] = prompt_template(sources, self.config.role)
            self.question_templates[variant.name] = prompt_template(
                sources, self.config.role, Question.druh)
            for case in self.measured:
                name = f"prompts/{slugify(case.id)}--{variant.slug}.txt"
                write_text_atomic(self.directory / name,
                                  build_prompt(self._template(variant.name, case), case))

    def _write_schedule(self, planned: Sequence[ScheduleEntry]) -> None:
        self._write_json("schedule.json", {
            "seed": self.config.seed, "concurrency": 1, "runs_per_variant": RUNS_PER_VARIANT,
            "max_replacements": MAX_REPLACEMENTS,
            "planned": [{"slot": i + 1, "case_id": e.case_id, "variant": e.variant,
                         "repetition": e.repetition} for i, e in enumerate(planned)],
            "executed": [{"order": r.order, "case_id": r.case_id, "variant": r.variant,
                          "attempt": r.attempt, "replaces": r.replaces, "state": r.state,
                          "transcript": r.transcript} for r in self.results],
        })

    def _report(self, status: str, detail: str | None,
                best_effort: bool = False) -> dict[str, Any] | None:
        try:
            return report.write_report(self.directory, self._info(status, detail), self.results)
        except Exception:
            if best_effort:
                return None
            raise

    def _info(self, status: str, detail: str | None) -> ExperimentInfo:
        assert self.export is not None
        version = next((r.agent_version for r in self.preflight_results if r.agent_version),
                       None)
        variants = tuple(self._variant_info(v) for v in self.config.variants)
        return ExperimentInfo(
            experiment_id=self.directory.name,
            started_at=self.started.strftime("%d.%m.%Y %H:%M:%S"),
            finished_at=self.now().strftime("%d.%m.%Y %H:%M:%S"),
            status=status, status_detail=detail, model=self.config.model, cap=self.config.cap,
            seed=self.config.seed, base_revision=self.export.revision,
            tree_fingerprint=self.export.tree_fingerprint,
            variants=(variants[0], variants[1]),
            cases=tuple(CaseInfo(c.id, False, False, Question.druh) if isinstance(c, Question)
                        else CaseInfo(c.id, c.used_for_card_tuning, c.prompt_names_location)
                        for c in self.measured),
            role=self.config.role,
            not_measured=tuple(c.id for c in not_measured_cases(self.catalog, self.config.role)),
            adapter=self.adapter_name, agent_version=version,
            preparation=tuple(self.costs), tariff=self.config.tariff,
            max_replacements=MAX_REPLACEMENTS)

    def _variant_info(self, variant: VariantSpec) -> VariantInfo:
        receipt = self.receipts.get(variant.name)
        corpus = self.paths.corpora.get(variant.name)
        template = self.templates.get(variant.name, "")
        question_template = self.question_templates.get(variant.name, "")
        return VariantInfo(
            name=variant.name,
            corpus_dir=str(corpus) if corpus is not None else None,
            corpus_receipt=_receipt_json(receipt) if receipt is not None else None,
            manifest_schema_version=manifest_schema_version(corpus) if corpus else None,
            card_format=variant.card_format,
            prompt_template=template, question_prompt_template=question_template,
            prompt_sha256={c.id: hashlib.sha256(
                build_prompt(question_template if isinstance(c, Question) else template, c)
                .encode("utf-8")).hexdigest() for c in self.measured})

    def _preparation_json(self, error: str | None) -> dict[str, Any]:
        return {"error": error,
                "base_revision": self.export.revision if self.export else None,
                "tree_fingerprint": self.export.tree_fingerprint if self.export else None,
                "corpora": {n: _receipt_json(r) for n, r in self.receipts.items()},
                "costs": [{"step": c.step, "seconds": c.seconds, "note": c.note,
                           "tokens": None if c.tokens is None else
                           {k: getattr(c.tokens, k) for k in report.TOKEN_CATEGORIES}}
                          for c in self.costs]}

    def _write_json(self, name: str, data: Any) -> None:
        write_text_atomic(self.directory / name,
                          json.dumps(data, ensure_ascii=False, indent=2, sort_keys=True) + "\n")

    def _timed(self, step: str) -> "_Timer":
        return _Timer(self.costs, step)


class _Timer:
    """Context manager appending a preparation cost with the block's wall time."""

    def __init__(self, costs: list[PreparationCost], step: str) -> None:
        self.costs, self.step = costs, step

    def __enter__(self) -> None:
        self.started = time.monotonic()

    def __exit__(self, *exc: Any) -> None:
        if exc[0] is None:
            self.costs.append(PreparationCost(self.step, time.monotonic() - self.started))


def _cap_json(record: RunRecord) -> dict[str, Any] | None:
    hit = record.cap_hit
    if hit is None:
        return None
    return {"kind": hit.kind, "limit": hit.limit, "observed": hit.observed,
            "overshoot": hit.overshoot}


def _receipt_json(receipt: prepare.CorpusReceipt) -> dict[str, Any]:
    return {"base_revision": receipt.base_revision, "tree_fingerprint": receipt.tree_fingerprint,
            "ra_version": receipt.ra_version, "options": list(receipt.options),
            "corpus_fingerprint": receipt.corpus_fingerprint,
            "generation_seconds": receipt.generation_seconds}


def manifest_schema_version(corpus: Path) -> int | None:
    """`schema_version` of the corpus's manifest, in either shape.

    A manifest in one file is `manifest.json`; above RA's size threshold it is
    written by lines and the version sits in the small header document
    `manifest-strings.json` (the line files are never opened here).
    """
    for name in ("manifest.json", "manifest-strings.json"):
        try:
            data = json.loads((corpus / name).read_text(encoding="utf-8"))
        except (OSError, ValueError):
            continue
        version = data.get("schema_version") if isinstance(data, dict) else None
        if isinstance(version, int) and not isinstance(version, bool):
            return version
    return None


# --- CLI ---------------------------------------------------------------------

def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="run.py",
        description="Měření vyhledávání: srovnávací pokus agenta bez karet a s kartami RA "
                    "(příprava, kontrola nástroje, zamíchaný rozvrh, běhy, zpráva).")
    req = parser.add_argument_group("povinné")
    req.add_argument("--catalog", required=True, type=Path,
                     help="katalog případů (JSON) mimo repozitář")
    req.add_argument("--source-repo", required=True, type=Path,
                     help="zdrojový repozitář gitu se základní revizí")
    req.add_argument("--output-root", required=True, type=Path,
                     help="kořen výstupu pokusu; musí ležet mimo repozitáře")
    req.add_argument("--export-dir", required=True, type=Path,
                     help="prázdný adresář pro export stromu základní revize (pracovní strom "
                          "agenta)")
    req.add_argument("--isolation-root", required=True, type=Path,
                     help="kořen pro izolovaný adresář nastavení agenta (kopie přihlášení)")
    req.add_argument("--ra-binary", required=True,
                     help="binárka reverse-architect pro generování korpusu (varianta ji může "
                          "přepsat vlastním ra_command v souboru variant)")
    req.add_argument("--model", required=True, help="model agenta, pevný pro celý pokus")
    req.add_argument("--cap-seconds", required=True, type=float, help="strop času běhu v sekundách")
    req.add_argument("--cap-steps", required=True, type=int,
                     help="strop kroků běhu (volání nástrojů)")
    req.add_argument("--cap-tokens", required=True, type=int,
                     help="strop tokenů běhu (součet čtyř kategorií)")
    req.add_argument("--seed", required=True, type=int, help="semínko zamíchání rozvrhu")
    req.add_argument("--role", required=True, choices=ROLES,
                     help="role čtenáře (bez výchozí hodnoty): vyvojar = agent má zdrojáky, výchozí "
                          "dvojice variant; podpora = agent nemá zdrojáky (pracovní adresář je "
                          "prázdný, čte jen korpus), položky druhu chyba se neměří a varianty se "
                          "zadávají výslovně souborem --variants, každá s korpusem")
    var = parser.add_argument_group("varianty")
    var.add_argument("--variants", type=Path,
                     help="soubor variant (JSON: name, corpus_dir, ra_options, card_format a "
                          "nepovinný ra_command, seznam textů = binárka RA a její argumenty "
                          "pro korpus této varianty místo --ra-binary); bez něj dvojice „bez "
                          "karet“ a „s kartami“ (jen v roli vyvojar)")
    var.add_argument("--corpus-dir", type=Path,
                     help="korpus varianty „s kartami“ (s účtenkou, nebo se vygeneruje)")
    var.add_argument("--ra-option", action="append", default=[],
                     help="volba RA pro korpus varianty „s kartami“ (lze opakovat)")
    other = parser.add_argument_group("ostatní")
    other.add_argument("--claude", default="claude", help="spustitelný soubor Claude Code")
    other.add_argument("--forbid-dir", action="append", default=[], metavar="JMÉNO",
                       help="jméno adresáře, pod kterým nesmí ležet export, korpus, kořen "
                            "výstupu ani kořen izolace (např. pracovní prostor jiného "
                            "nástroje); lze opakovat")
    other.add_argument("--credentials", type=Path,
                       help="přihlášení, jehož kopii agent dostane "
                            "(výchozí ~/.claude/.credentials.json)")
    other.add_argument("--tariff", type=Path,
                       help="tarif (JSON: currency, per_million po kategoriích, source); bez něj "
                            "zpráva peníze neuvádí")
    other.add_argument("--dry-run", action="store_true",
                       help="jen ověří vstupy a vypíše počet běhů a strop; agenta nespustí a nic "
                            "nezapíše")
    other.add_argument("--prepare-only", action="store_true",
                       help="jen příprava (export, kontrola případů, korpus s účtenkou); pak "
                            "skončí. Další spuštění potřebuje nový prázdný --export-dir; korpus "
                            "se znovu použije podle účtenky")
    return parser


def load_tariff(path: Path) -> Tariff:
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
        prices = data["per_million"]
        tariff = Tariff(currency=str(data["currency"]),
                        per_million={c: float(prices[c]) for c in report.TOKEN_CATEGORIES},
                        source=str(data.get("source", "")))
    except (OSError, ValueError, KeyError, TypeError) as exc:
        raise ConfigError(f"Tarif {path} je neplatný: {exc}") from None
    return tariff


def config_from_args(args: argparse.Namespace) -> ExperimentConfig:
    validate_role(args.role)
    if args.variants is None and args.role == ROLE_SUPPORT:
        raise ConfigError("V roli podpora agent nemá zdrojáky, takže výchozí dvojice „bez karet“ "
                          "a „s kartami“ nedává smysl. Zadej dva korpusy výslovně souborem "
                          "--variants (každá varianta s polem corpus_dir).")
    if args.variants is not None:
        try:
            data = json.loads(args.variants.read_text(encoding="utf-8"))
        except (OSError, ValueError) as exc:
            raise ConfigError(f"Soubor variant {args.variants} nejde načíst: {exc}") from None
        variants = parse_variants(data, args.variants.resolve().parent)
    else:
        variants = default_variants(args.corpus_dir, args.ra_option)
    variants = validate_variants(variants, args.role)
    if min(args.cap_seconds, args.cap_steps, args.cap_tokens) <= 0:
        raise ConfigError("Strop času, kroků i tokenů musí být kladný.")
    return ExperimentConfig(
        catalog_path=args.catalog, source_repo=args.source_repo, output_root=args.output_root,
        export_dir=args.export_dir, isolation_root=args.isolation_root,
        ra_command=(args.ra_binary,), model=args.model,
        cap=Cap(seconds=args.cap_seconds, steps=args.cap_steps, tokens=args.cap_tokens),
        seed=args.seed, variants=variants, role=args.role,
        tariff=load_tariff(args.tariff) if args.tariff is not None else None,
        claude_executable=(args.claude,), credentials=args.credentials,
        forbidden_dir_names=tuple(args.forbid_dir))


def dry_run(config: ExperimentConfig) -> str:
    """Validate the inputs and describe the experiment without writing or running anything."""
    validate_role(config.role)
    validate_variants(config.variants, config.role)
    catalog = _load_catalog(config)
    resolve_paths(config)
    cases = measured_cases(catalog, config.role)
    cells = len(cases) * len(config.variants)
    planned = cells * RUNS_PER_VARIANT
    cap = config.cap
    lines = [
        "Suchý běh: agent se nespustí a nic se nezapíše.",
        f"Položky: {len(cases)} (případy a otázky); varianty: "
        + ", ".join(f"„{v.name}“" for v in config.variants) + ".",
        f"Běhů v rozvrhu: {planned} ({len(cases)} položky × {len(config.variants)} "
        f"varianty × {RUNS_PER_VARIANT}); se všemi náhradami zkažených běhů nejvýš "
        f"{planned + cells * MAX_REPLACEMENTS}.",
        f"Kontrola nástroje: {len(config.variants)} běhy před pokusem.",
        f"Strop běhu: čas {report.fmt(float(cap.seconds))} s, kroky {cap.steps}, "
        f"tokeny {cap.tokens}.",
        f"Model: {config.model}; semínko: {config.seed}; souběh 1.",
        f"Role: {config.role}"
        + (" (agent nemá zdrojáky, čte jen korpus)" if config.role == ROLE_SUPPORT
           else " (agent má zdrojáky)") + ".",
    ]
    skipped = not_measured_cases(catalog, config.role)
    if skipped:
        lines.insert(2, f"Položky druhu „chyba“ ({', '.join(c.id for c in skipped)}): "
                        f"{report.NOT_MEASURED_IN_SUPPORT}.")
    if catalog.questions:
        lines.insert(2, f"Otázky v katalogu: {len(catalog.questions)} (volná odpověď, "
                        "neskórují se, hodnotí je answers.py hodnot-odpovedi).")
    return "\n".join(lines)


def main(argv: Sequence[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        config = config_from_args(args)
        if args.dry_run:
            print(dry_run(config))
            return 0
        outcome = run_experiment(config, prepare_only=args.prepare_only)
    except ConfigError as exc:
        print(f"Chyba vstupu: {exc}", file=sys.stderr)
        return 2
    except ExperimentStopped as exc:
        print(f"{exc}\nAdresář pokusu: {exc.directory}", file=sys.stderr)
        return exc.exit_code
    except KeyboardInterrupt:
        print("Pokus přerušen.", file=sys.stderr)
        return 130
    if outcome.status == "prepared":
        print(f"Příprava hotová: {outcome.directory}")
    else:
        print(f"Pokus dokončen: {outcome.directory / 'report.md'}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
