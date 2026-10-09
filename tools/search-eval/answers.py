"""Entry point of the answer check of the search-eval experiment (U5 and the units after it).

    python tools/search-eval/answers.py kontrola-karet --help

The tool has subcommands. This unit delivers `kontrola-karet` (the card check:
`card_check.py`); `hodnot-odpovedi`, `kontrola`, `zprava` and `kalibrace` are
added by later units. Adding a subcommand takes three steps:

1. write `configure_<name>(parser)` (its own arguments) and
   `command_<name>(args, services) -> int` (the work; it raises the errors
   listed below and returns 0 when done);
2. append `Subcommand("<name>", "<help>", configure_<name>, command_<name>)` to
   `SUBCOMMANDS`;
3. nothing else: `build_parser` and `main` read the list. Every subcommand
   already has the common arguments (`--source-repo`) and the error mapping.

Common to all subcommands: `--source-repo` is required (the repository whose
revision the catalog is over; with this repository it fixes the protected roots
that no state directory may lie under, `prepare.protected_roots`). There are no
silent defaults: every path is named.

Exit codes: 0 done, 2 invalid input (also a missing argument), 3 judge
unavailable or infrastructure failure (the measurement stays `nedokonceno` and
the same command continues from the judge's cache), 130 interrupted (Ctrl+C;
the measurement stays `nedokonceno`).
"""
from __future__ import annotations

import argparse
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Callable, Sequence

# Allow `python tools/search-eval/answers.py` from anywhere: the modules sit beside it.
sys.path.insert(0, str(Path(__file__).resolve().parent))

import card_check  # noqa: E402
from agents.base import AgentRunError  # noqa: E402
from catalog import CatalogError, load_catalog  # noqa: E402
from corpus import CorpusError, open_corpus  # noqa: E402
from judge import (CliJudgeRunner, JudgeCache, JudgeCacheError, JudgeFileError,  # noqa: E402
                   JudgeRunner, JudgeUnavailableError, load_judges, select_judge)
from prepare import PrepareError, ensure_outside, protected_roots, repo_toplevel  # noqa: E402

EXIT_DONE = 0
EXIT_INPUT = 2
EXIT_INFRASTRUCTURE = 3
EXIT_INTERRUPTED = 130

ISOLATION_DIR = "izolace"


class InputError(Exception):
    """An argument or an input file is invalid (Czech message for the operator)."""


@dataclass(frozen=True)
class Services:
    """What a subcommand may need from outside: the judge's process seam.

    `runner` is None in real use (the configured CLI is run in isolation under
    the state directory); tests pass a scripted runner.
    """

    runner: JudgeRunner | None = None


@dataclass(frozen=True)
class Subcommand:
    name: str
    help: str
    configure: Callable[[argparse.ArgumentParser], None]
    run: Callable[[argparse.Namespace, Services], int]


# --- kontrola-karet --------------------------------------------------------------------

def configure_card_check(parser: argparse.ArgumentParser) -> None:
    req = parser.add_argument_group("povinné")
    req.add_argument("--agent-family", required=True, metavar="RODINA",
                     help="rodina agenta, s jehož běhy se korpus porovná (např. claude); "
                          "hodnotitel se vybírá z jiné rodiny")
    req.add_argument("--katalog", required=True, type=Path,
                     help="katalog položek (JSON) mimo repozitář")
    req.add_argument("--korpus", required=True, type=Path,
                     help="korpus RA (adresář s manifestem); účtenka není potřeba")
    req.add_argument("--hodnotitele", required=True, type=Path,
                     help="soubor hodnotitelů (JSON, formát 1)")
    req.add_argument("--stav", required=True, type=Path,
                     help="stavový adresář (index korpusu, cache hodnotitele, measurement.json); "
                          "musí ležet mimo repozitáře")


def command_card_check(args: argparse.Namespace, services: Services) -> int:
    state = _checked_state_dir(args.source_repo, args.stav)
    catalog = load_catalog(args.katalog)
    if not catalog.questions:
        raise InputError("Katalog neobsahuje žádnou položku druhu „otazka“; není co kontrolovat.")
    judges = load_judges(args.hodnotitele)
    index = open_corpus(args.korpus, state)
    runner = services.runner or CliJudgeRunner(isolation_root=state / ISOLATION_DIR,
                                               keep_apart=(args.korpus,))
    # The revision is checked here once more than `run_card_check` needs it: no
    # judge, not even the smoke test, runs over a corpus of another revision.
    for question in catalog.questions:
        card_check.check_commit(index.identity, question.commit)
    selection = select_judge(judges, args.agent_family, runner)

    path = card_check.measurement_path(state)
    plan_id = None
    moved = None

    def save(measurement: card_check.Measurement) -> None:
        nonlocal plan_id, moved
        if plan_id != measurement.id:
            plan_id = measurement.id
            moved = card_check.archive_foreign_measurement(path, measurement.id)
        card_check.write_measurement(path, measurement)

    def progress(done: int, total: int, unit: card_check.UnitResult) -> None:
        if unit.state == card_check.STATE_PENDING:
            return
        print(f"[{done}/{total}] {unit.id}: {unit.final_label or unit.state}")

    try:
        measurement = card_check.run_card_check(
            catalog, index, selection, runner, JudgeCache(state / card_check.JUDGE_CACHE_DIR),
            save=save, progress=progress)
    except (AgentRunError, JudgeCacheError, KeyboardInterrupt):
        print(f"Měření je uložené jako nedokončené: {path}. Stejný příkaz pokračuje z cache "
              "hodnotitele.", file=sys.stderr)
        raise
    if moved is not None:
        print(f"Dřívější měření jiného vstupu uloženo jako {moved}.")
    print(_summary(measurement))
    print(f"Měření: {path}")
    return EXIT_DONE


def _summary(measurement: card_check.Measurement) -> str:
    results = measurement.results()
    counts: dict[str, int] = {}
    for fact in results.facts:
        counts[fact.status] = counts.get(fact.status, 0) + 1
    lines = [f"Kontrola karet hotová: {len(results.facts)} výsledků faktů, "
             f"{len(results.claims)} zakázaných tvrzení, {len(measurement.units)} jednotek."]
    lines += [f"  {status}: {count}" for status, count in sorted(counts.items())]
    return "\n".join(lines)


def _checked_state_dir(source_repo: Path, state: Path) -> Path:
    """The state directory, canonical, refused under a protected root (the repositories)."""
    try:
        roots = protected_roots(repo_toplevel(source_repo))
        return Path(ensure_outside(state, roots, "Stavový adresář"))
    except PrepareError as exc:
        raise InputError(str(exc)) from None


SUBCOMMANDS: tuple[Subcommand, ...] = (
    Subcommand("kontrola-karet",
               "ověří, že karty korpusu uvádějí fakty otázek a neuvádějí zakázaná tvrzení",
               configure_card_check, command_card_check),
)


# --- parser and main -------------------------------------------------------------------

def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="answers.py",
        description="Kontrola odpovědí v měření vyhledávání: karty korpusu proti faktům otázek "
                    "katalogu (a další podpříkazy v dalších krocích).")
    common = argparse.ArgumentParser(add_help=False)
    group = common.add_argument_group("společné povinné")
    group.add_argument("--source-repo", required=True, type=Path,
                       help="zdrojový repozitář gitu se základní revizí (určuje chráněné kořeny)")
    sub = parser.add_subparsers(dest="command", metavar="PODPŘÍKAZ", required=True)
    for command in SUBCOMMANDS:
        child = sub.add_parser(command.name, parents=[common], help=command.help,
                               description=command.help)
        child.set_defaults(handler=command.run)
        command.configure(child)
    return parser


_INPUT_ERRORS = (InputError, CatalogError, CorpusError, JudgeFileError, PrepareError)
_INFRASTRUCTURE_ERRORS = (JudgeUnavailableError, AgentRunError, JudgeCacheError)


def main(argv: Sequence[str] | None = None, *, runner: JudgeRunner | None = None) -> int:
    parser = build_parser()
    try:
        args = parser.parse_args(argv)
    except SystemExit as exc:  # argparse: 2 for bad arguments, 0 for --help
        return exc.code if isinstance(exc.code, int) else EXIT_INPUT
    try:
        return args.handler(args, Services(runner=runner))
    except _INPUT_ERRORS as exc:
        print(f"Chyba vstupu: {exc}", file=sys.stderr)
        return EXIT_INPUT
    except _INFRASTRUCTURE_ERRORS as exc:
        print(f"Hodnotitel nebo infrastruktura selhaly: {exc}", file=sys.stderr)
        return EXIT_INFRASTRUCTURE
    except KeyboardInterrupt:
        print("Přerušeno.", file=sys.stderr)
        return EXIT_INTERRUPTED


if __name__ == "__main__":
    sys.exit(main())
