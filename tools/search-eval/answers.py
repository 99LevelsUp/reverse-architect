"""Entry point of the answer check of the search-eval experiment (U5 and the units after it).

    python tools/search-eval/answers.py kontrola-karet --help

The tool has subcommands. `kontrola-karet` is the card check (`card_check.py`),
`hodnot-odpovedi` judges the agent's free answers to questions (KTD10), `kontrola`
is the owner's review, `zprava` the report of the answers (`answers_report.py`); `kalibrace`
is added by a later unit. Adding a subcommand takes three steps:

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
import hashlib
import json
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable, Sequence

# Allow `python tools/search-eval/answers.py` from anywhere: the modules sit beside it.
sys.path.insert(0, str(Path(__file__).resolve().parent))

import answers_report  # noqa: E402
import calibration  # noqa: E402
import card_check  # noqa: E402
import report  # noqa: E402
import review  # noqa: E402
import run as experiment  # noqa: E402
from agents.base import AgentRunError  # noqa: E402
from catalog import CatalogError, Question, load_catalog  # noqa: E402
from corpus import CorpusError, same_revision, open_corpus  # noqa: E402
from judge import (CliJudgeRunner, JudgeCache, JudgeCacheError, JudgeFileError,  # noqa: E402
                   JudgeRunner, JudgeUnavailableError, load_judges, select_judge)
from prepare import PrepareError, ensure_outside, protected_roots, repo_toplevel  # noqa: E402

EXIT_DONE = 0
EXIT_INPUT = 2
EXIT_INFRASTRUCTURE = 3
EXIT_INTERRUPTED = 130

ISOLATION_DIR = "izolace"
ANSWERS_DIR = "odpovedi"

# The agent family of each adapter that produced an experiment (the judge is chosen from
# another family).
AGENT_FAMILIES = {"claude-code": "claude"}
# Why a stored run of a question has no answer, as the measurement words it.
NO_ANSWER_REASONS = {
    "no_answer": "běh skončil bez závěrečné zprávy",
    "cap_steps": "běh zastavil strop kroků",
    "cap_tokens": "běh zastavil strop tokenů",
    "cap_time": "běh zastavil strop času",
}


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


# --- hodnot-odpovedi -------------------------------------------------------------------

def configure_answers(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("pokus", type=Path, metavar="ADRESÁŘ_POKUSU",
                        help="adresář pokusu z run.py (s report.json a runs/)")
    req = parser.add_argument_group("povinné")
    req.add_argument("--katalog", required=True, type=Path,
                     help="katalog položek (JSON) mimo repozitář; otázky pokusu musí v něm být")
    req.add_argument("--hodnotitele", required=True, type=Path,
                     help="soubor hodnotitelů (JSON, formát 1)")
    req.add_argument("--stav", required=True, type=Path,
                     help="stavový adresář (cache hodnotitele, měření v podadresáři "
                          f"{ANSWERS_DIR}/<pokus>/measurement.json); musí ležet mimo repozitáře")
    parser.add_argument("--i-nedokonceny", action="store_true", dest="i_nedokonceny",
                        help="přijme i pokus, který nemá stav completed (přerušený nebo zastavený "
                             "chybou infrastruktury); měření to vyznačí")


@dataclass(frozen=True)
class Attempt:
    """What the answer check reads of an experiment directory (`report.json`, `runs/`)."""

    directory: Path
    id: str
    status: str
    role: str
    adapter: str
    base_revision: str
    tree_fingerprint: str | None
    variants: tuple[dict[str, Any], ...]
    questions: tuple[str, ...]
    runs: tuple[dict[str, Any], ...]  # the result files of the questions' runs, in order


def _read_json(path: Path) -> Any:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except OSError as exc:
        raise InputError(f"Soubor {path} nejde načíst: {exc}") from None
    except ValueError as exc:
        raise InputError(f"Soubor {path} není platný JSON: {exc}") from None


def load_attempt(directory: Path) -> Attempt:
    """Read the experiment directory; anything that is not one is an input error."""
    data = _read_json(directory / "report.json")
    experiment_data = data.get("experiment") if isinstance(data, dict) else None
    if (not isinstance(experiment_data, dict)
            or data.get("format_version") != report.REPORT_FORMAT_VERSION):
        raise InputError(f"{directory / 'report.json'} není zpráva pokusu podporovaného formátu "
                         f"(čekám format_version {report.REPORT_FORMAT_VERSION}).")
    try:
        variants = tuple(dict(v) for v in experiment_data["variants"])
        questions = tuple(q["question_id"] for q in data.get("questions", ()))
        attempt = Attempt(
            directory=directory, id=experiment_data["id"], status=experiment_data["status"],
            role=experiment_data["role"], adapter=experiment_data["adapter"],
            base_revision=experiment_data["base_revision"],
            tree_fingerprint=experiment_data.get("tree_fingerprint"), variants=variants,
            questions=questions, runs=())
    except (KeyError, TypeError) as exc:
        raise InputError(f"Zpráva pokusu {directory / 'report.json'} je neúplná "
                         f"(chybí {exc}).") from None
    runs = []
    for path in sorted((directory / "runs").glob("*.result.json")):
        result = _read_json(path)
        if isinstance(result, dict) and result.get("case_id") in questions:
            runs.append(result)
    return Attempt(**{**attempt.__dict__, "runs": tuple(runs)})


def _check_attempt(attempt: Attempt, questions: dict[str, Question], accept_unfinished: bool
                   ) -> str:
    """Refuse an attempt the check cannot take; return the agent's family."""
    if attempt.status != report.STATUS_COMPLETED and not accept_unfinished:
        raise InputError(f"Pokus {attempt.id} má stav „{attempt.status}“, ne „completed“; "
                         "jeho odpovědi se nehodnotí. Přepínač --i-nedokonceny je přijme a měření "
                         "to vyznačí.")
    if attempt.role not in report.ROLES:
        raise InputError(f"Pokus má neznámou roli „{attempt.role}“.")
    family = AGENT_FAMILIES.get(attempt.adapter)
    if family is None:
        raise InputError(f"Pokus běžel přes neznámý adaptér „{attempt.adapter}“, rodina agenta "
                         "se nedá určit a hodnotitel by se nedal vybrat z jiné rodiny.")
    if not attempt.questions:
        raise InputError(f"Pokus {attempt.id} neměřil žádnou otázku; není co hodnotit.")
    revisions = [("pokus", attempt.base_revision)] + [
        (f"korpus varianty {v.get('name')}", v["corpus_receipt"].get("base_revision"))
        for v in attempt.variants if isinstance(v.get("corpus_receipt"), dict)]
    for question_id in attempt.questions:
        question = questions.get(question_id)
        if question is None:
            raise InputError(f"Otázka „{question_id}“ z pokusu není v katalogu.")
        for what, revision in revisions:
            if not isinstance(revision, str) or not same_revision(revision, question.commit):
                raise InputError(f"Otázka „{question_id}“ je v katalogu nad commitem "
                                 f"{question.commit}, ale {what} vznikl nad revizí {revision}.")
        for variant in attempt.variants:
            template = variant.get("question_prompt_template")
            expected = (variant.get("prompt_sha256") or {}).get(question_id)
            if template and expected:
                prompt = experiment.build_prompt(template, question)
                if hashlib.sha256(prompt.encode("utf-8")).hexdigest() != expected:
                    raise InputError(f"Zadání otázky „{question_id}“ v katalogu neodpovídá "
                                     f"zadání, které dostal agent ve variantě "
                                     f"{variant.get('name')}; katalog se od pokusu změnil.")
    return family


def _answer_runs(attempt: Attempt) -> list[card_check.AnswerRun]:
    """The stored runs the check judges: the first three of each question and variant."""
    out: list[card_check.AnswerRun] = []
    for question_id in attempt.questions:
        for variant in attempt.variants:
            mine = [r for r in attempt.runs if r["case_id"] == question_id
                    and r["variant"] == variant["name"] and r["state"] in report.QUESTION_STATES]
            for result in mine[:report.RUNS_PER_VARIANT]:
                text = result.get("answer_text")
                if result["state"] == report.ANSWERED and isinstance(text, str) and text.strip():
                    out.append(card_check.AnswerRun(question_id, variant["name"], result["order"],
                                                    text))
                    continue
                reason = result.get("reason") or "no_answer"
                out.append(card_check.AnswerRun(question_id, variant["name"], result["order"],
                                                None, NO_ANSWER_REASONS.get(reason, reason)))
    return out


def _receipt_summary(variant: dict[str, Any]) -> dict[str, Any]:
    receipt = variant.get("corpus_receipt")
    keep = ("base_revision", "tree_fingerprint", "ra_version", "options", "corpus_fingerprint")
    return {"nazev": variant.get("name"),
            "korpus": {k: receipt.get(k) for k in keep} if isinstance(receipt, dict) else None}


def _answers_digest(runs: Sequence[card_check.AnswerRun]) -> str:
    rows = [[r.run, r.question_id, r.variant,
             None if r.text is None else hashlib.sha256(r.text.encode("utf-8")).hexdigest()]
            for r in runs]
    return hashlib.sha256(json.dumps(rows, ensure_ascii=False).encode("utf-8")).hexdigest()[:16]


def command_answers(args: argparse.Namespace, services: Services) -> int:
    state = _checked_state_dir(args.source_repo, args.stav)
    attempt = load_attempt(args.pokus)
    catalog = load_catalog(args.katalog)
    family = _check_attempt(attempt, {q.id: q for q in catalog.questions}, args.i_nedokonceny)
    runs = _answer_runs(attempt)
    if not runs:
        raise InputError(f"Pokus {attempt.id} nemá žádný uložený běh otázky.")
    judges = load_judges(args.hodnotitele)
    runner = services.runner or CliJudgeRunner(isolation_root=state / ISOLATION_DIR,
                                               keep_apart=(attempt.directory,))
    selection = select_judge(judges, family, runner)

    unfinished = attempt.status != report.STATUS_COMPLETED
    corpus = {"revision": attempt.base_revision, "source": card_check.ANSWER_PATH,
              "label": f"odpovědi pokusu {attempt.id}", "tree_fingerprint": attempt.tree_fingerprint,
              "pokus": attempt.id, "role": attempt.role, "odpovedi": _answers_digest(runs),
              "varianty": [_receipt_summary(v) for v in attempt.variants]}
    extra = {"pokus": {
        "id": attempt.id, "stav": attempt.status, "nedokonceny": unfinished, "role": attempt.role,
        "adapter": attempt.adapter, "rodina_agenta": family,
        "behy": [{"run": r.run, "otazka": r.question_id, "varianta": r.variant,
                  "odpoved": r.text is not None} for r in runs],
        "bez_odpovedi": [{"run": r.run, "otazka": r.question_id, "varianta": r.variant,
                          "duvod": r.reason} for r in runs if r.text is None]}}

    path = card_check.measurement_path(state / ANSWERS_DIR / attempt.directory.name)
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
        measurement = card_check.run_answer_check(
            catalog, runs, attempt.role, corpus, extra, selection, runner,
            JudgeCache(state / card_check.JUDGE_CACHE_DIR), save=save, progress=progress)
    except (AgentRunError, JudgeCacheError, KeyboardInterrupt):
        print(f"Měření je uložené jako nedokončené: {path}. Stejný příkaz pokračuje z cache "
              "hodnotitele.", file=sys.stderr)
        raise
    if moved is not None:
        print(f"Dřívější měření jiného vstupu uloženo jako {moved}.")
    print(_answers_summary(measurement, attempt, len(runs)))
    print(f"Měření: {path}")
    return EXIT_DONE


def _answers_summary(measurement: card_check.Measurement, attempt: Attempt, run_count: int) -> str:
    results = measurement.results()
    counts: dict[str, int] = {}
    for fact in results.facts:
        counts[fact.status] = counts.get(fact.status, 0) + 1
    missing = len(measurement.extra["pokus"]["bez_odpovedi"])
    lines = [f"Hodnocení odpovědí hotové: {run_count} běhů ({missing} bez odpovědi), "
             f"{len(results.facts)} výsledků faktů, {len(results.claims)} zakázaných tvrzení, "
             f"{len(measurement.units)} jednotek."]
    if attempt.status != report.STATUS_COMPLETED:
        lines.append(f"Pozor: pokus má stav „{attempt.status}“ a měření to vyznačuje.")
    lines += [f"  {status}: {count}" for status, count in sorted(counts.items())]
    return "\n".join(lines)


# --- kontrola --------------------------------------------------------------------------

def configure_review(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--stav", required=True, type=Path,
                        help="stavový adresář (rozhodnuti.json); musí ležet mimo repozitáře")
    make = parser.add_argument_group("sestavení souboru ke kontrole")
    make.add_argument("--mereni", action="append", type=Path, metavar="MEASUREMENT.JSON",
                      help="hotové měření (lze opakovat)")
    make.add_argument("--vystup", type=Path, metavar="SOUBOR.JSON",
                      help="soubor ke kontrole (JSON; vedle něj vznikne .md ke čtení); "
                           "existující soubor se nepřepíše")
    make.add_argument("--seminko", type=int, metavar="CELE_CISLO",
                      help="semínko vzorku; výchozí je odvozené z měření; zapisuje se do souboru")
    load = parser.add_argument_group("načtení vyplněného souboru")
    load.add_argument("--nacti", type=Path, metavar="SOUBOR.JSON",
                      help="vyplněný soubor ke kontrole; uloží rozhodnutí do stavového adresáře")


def command_review(args: argparse.Namespace, services: Services) -> int:
    state = _checked_state_dir(args.source_repo, args.stav)
    if args.nacti is not None:
        if args.mereni or args.vystup or args.seminko is not None:
            raise InputError("--nacti se nedá spojit s --mereni, --vystup ani --seminko.")
        result = review.ingest_review(args.nacti, state)
        print(f"Načteno: {result.decided} rozhodnutí ({result.new} nových nebo změněných), "
              f"{result.empty} řádků zůstalo prázdných.")
        if result.proposals:
            print(f"Návrhy zakázaných tvrzení ({result.proposals}): {result.proposals_path}")
        return EXIT_DONE
    if not args.mereni or args.vystup is None:
        raise InputError("Sestavení souboru ke kontrole potřebuje --mereni i --vystup "
                         "(nebo --nacti pro načtení vyplněného souboru).")
    loaded = [review.load_measurement(path) for path in args.mereni]
    plan = review.build_review(loaded, review.load_decisions(state), args.seminko)
    json_path, md_path = review.write_review(plan, args.vystup)
    review.record_sampled(state, plan)
    print(f"Kontrola vlastníka: {len(plan.rows)} řádků, z toho {plan.disputed_count} sporných; "
          f"semínko {plan.seed}.")
    for label, (total, drawn) in sorted(plan.sample_sizes.items()):
        print(f"  {label}: {drawn} ze {total} bezsporných")
    print(f"K vyplnění: {json_path}")
    print(f"Ke čtení: {md_path}")
    return EXIT_DONE


# --- zprava ----------------------------------------------------------------------------

def configure_answers_report(parser: argparse.ArgumentParser) -> None:
    req = parser.add_argument_group("povinné")
    req.add_argument("--mereni", required=True, action="append", type=Path,
                     metavar="MEASUREMENT.JSON",
                     help="hotové měření (kontrola karet nebo hodnocení odpovědí); lze opakovat, "
                          "každé měření je jeden korpus (nebo jeden pokus)")
    req.add_argument("--vystup", required=True, type=Path, metavar="ADRESÁŘ",
                     help="adresář, do kterého se zapíše answers-report.json a answers-report.md; "
                          "musí ležet mimo repozitáře")
    req.add_argument("--stav", required=True, type=Path,
                     help="stavový adresář s rozhodnutími vlastníka (rozhodnuti.json); "
                          "musí ležet mimo repozitáře")
    req.add_argument("--kalibrace", required=True, type=Path, metavar="ADRESÁŘ",
                     help="adresář s výsledky kalibrace hodnotitele (JSON); zpráva uvede kalibraci "
                          "pro dvojici hodnotitel + verze promptu, nebo že chybí")
    parser.add_argument("--sada-kalibrace", type=Path, metavar="SADA.JSON",
                        help="sada vět kalibrace; s ní se hledá jen kalibrace nad touto sadou "
                             "(bez ní kterákoli sada téže dvojice)")


def command_answers_report(args: argparse.Namespace, services: Services) -> int:
    # The output path is checked first: nothing is read or written for a path inside a repository.
    output = _checked_output_dir(args.source_repo, args.vystup)
    state = _checked_state_dir(args.source_repo, args.stav)
    fingerprint = None
    if args.sada_kalibrace is not None:
        try:
            fingerprint = calibration.load_set(args.sada_kalibrace).fingerprint
        except calibration.CalibrationSetError as exc:
            raise InputError(str(exc)) from None
    data = answers_report.build_report(args.mereni, state, args.kalibrace,
                                       set_fingerprint=fingerprint)
    json_path, md_path = answers_report.write_report(output, data)
    print(f"Zpráva odpovědí: {len(data['vstupy'])} měření, {len(data['korpusy'])} korpusů.")
    if data["zalozni_hodnotitel"]:
        print("Pozor: posoudil záložní hodnotitel stejné rodiny jako agent.")
    if not data["kalibrace"]["nalezena"]:
        print("Kalibrace pro tohoto hodnotitele a verzi promptu chybí (zpráva to uvádí).")
    print(f"JSON: {json_path}")
    print(f"Markdown: {md_path}")
    return EXIT_DONE


def _checked_output_dir(source_repo: Path, output: Path) -> Path:
    """The output directory, canonical, refused under a protected root and when it is a file."""
    try:
        roots = protected_roots(repo_toplevel(source_repo))
        target = Path(ensure_outside(output, roots, "Výstupní adresář"))
    except PrepareError as exc:
        raise InputError(str(exc)) from None
    if target.exists() and not target.is_dir():
        raise InputError(f"Výstupní adresář {output} existuje a není adresář.")
    return target


SUBCOMMANDS: tuple[Subcommand, ...] = (
    Subcommand("kontrola-karet",
               "ověří, že karty korpusu uvádějí fakty otázek a neuvádějí zakázaná tvrzení",
               configure_card_check, command_card_check),
    Subcommand("kontrola",
               "soubor ke kontrole vlastníka (sporné jednotky a vzorek) a načtení jeho rozhodnutí",
               configure_review, command_review),
    Subcommand("hodnot-odpovedi",
               "ohodnotí volné odpovědi agenta na otázky z pokusu run.py proti faktům "
               "a zakázaným tvrzením katalogu",
               configure_answers, command_answers),
    Subcommand("zprava",
               "zpráva odpovědí: fakta a zakázaná tvrzení po otázkách, cestách a korpusech "
               "z hotových měření s rozhodnutími vlastníka",
               configure_answers_report, command_answers_report),
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


_INPUT_ERRORS = (InputError, review.ReviewError, answers_report.ReportError, CatalogError,
                 CorpusError, JudgeFileError, PrepareError)
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
