"""Configuration of the search-eval experiment: variants, role, settings and path resolution (U5).

Split out of `run.py` (which re-exports every name here, so `run.VariantSpec`,
`run.resolve_paths` and the others keep working for their callers). Holds what is
decided before anything runs: the two variants and their validation, the `Role` of the
reader with the facts that follow from it, `ExperimentConfig`, and `resolve_paths`, the
canonicalization and checks of every location (KTD9).
"""
from __future__ import annotations

import os
import re
import unicodedata
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Sequence

import prepare
from agents.base import CONFIG_DIR_NAME, Cap
from catalog import Case, Catalog, Question
from prepare import PrepareError, canonical_path, ensure_outside, protected_roots
from report import ROLE_DEVELOPER, ROLE_SUPPORT, ROLES, Tariff

VARIANTS_FORMAT_VERSION = 1
DEFAULT_VARIANTS = ("bez karet", "s kartami")


class ConfigError(Exception):
    """The experiment's inputs are invalid (Czech message for the operator)."""


# --- role --------------------------------------------------------------------

@dataclass(frozen=True)
class Role:
    """The facts that follow from the reader's role (KTD9), in one place.

    `has_sources`: the agent works in the exported tree; otherwise (`corpus_only`) its work
    directory is an empty directory of its own and the audit permits only the corpus there.
    `measures_bugs`: items of kind `chyba` are measured (questions always are).
    `path_base`: how the prompt says a cited path is to be read.
    `dry_run_note`: the role's wording in the dry-run description."""

    name: str
    has_sources: bool
    measures_bugs: bool
    path_base: str
    dry_run_note: str

    @property
    def corpus_only(self) -> bool:
        return not self.has_sources

    @property
    def own_workdir(self) -> bool:
        return not self.has_sources

    def measured(self, catalog: Catalog) -> tuple[Case | Question, ...]:
        """The items the experiment runs in this role: the `chyba` items (none where they are
        not measured) and then the questions."""
        cases = tuple(catalog.cases) if self.measures_bugs else ()
        return (*cases, *catalog.questions)

    def not_measured(self, catalog: Catalog) -> tuple[Case, ...]:
        """The `chyba` items set aside by the role: reported as "v roli podpora se neměří"."""
        return () if self.measures_bugs else tuple(catalog.cases)


ROLE_SPECS = {
    ROLE_DEVELOPER: Role(ROLE_DEVELOPER, has_sources=True, measures_bugs=True,
                         path_base="relativně k pracovnímu adresáři",
                         dry_run_note="agent má zdrojáky"),
    ROLE_SUPPORT: Role(ROLE_SUPPORT, has_sources=False, measures_bugs=False,
                       path_base="relativně ke kořeni zdrojáků systému, jak ji uvádí korpus",
                       dry_run_note="agent nemá zdrojáky, čte jen korpus"),
}


def validate_role(role: str) -> None:
    if role not in ROLES:
        raise ConfigError(f"Role musí být {' nebo '.join(ROLES)}, zadáno „{role}“.")


def role_of(name: str) -> Role:
    """The `Role` named `name` (`ConfigError` for an unknown one)."""
    validate_role(name)
    return ROLE_SPECS[name]


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
    spec = ROLE_SPECS.get(role) if role is not None else None
    for v in variants:
        if v.ra_options and v.corpus_dir is None:
            raise ConfigError(f"Varianta {v.name}: volby RA bez korpusu nedávají smysl.")
        if v.ra_command is not None and v.corpus_dir is None:
            raise ConfigError(f"Varianta {v.name}: příkaz RA bez korpusu nedává smysl.")
        if spec is not None and spec.corpus_only and v.corpus_dir is None:
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
    """Canonical paths after the checks of step 1 (module docstring of `run.py`)."""

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


def protected_for(config: ExperimentConfig) -> list[str]:
    """The roots no file of the experiment may lie under: the source repo, this repo, worktrees."""
    try:
        return protected_roots(prepare.repo_toplevel(config.source_repo))
    except PrepareError as exc:
        raise ConfigError(str(exc)) from None


def resolve_paths(config: ExperimentConfig) -> ResolvedPaths:
    """Canonicalize and check every location before anything is written (KTD9).

    In the support role the agent's empty work directory is one more location
    the agent reads, so it gets the same checks as the export."""
    roots = protected_for(config)
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
    if role_of(config.role).own_workdir:
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
