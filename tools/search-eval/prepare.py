"""Experiment preparation: base-revision export, case checks, corpus.

Every input of the experiment is verified, not just recorded (KTD8), because
a measurement over the wrong tree, a case fixed before the base revision or a
corpus regenerated with other options would all produce a plausible-looking
report that measures something else.

1. `export_tree` writes the base revision's tree without git history
   (`git archive`) into a directory outside the source repository, outside
   this runner's repository (every worktree of either) and outside any
   directory whose name the operator forbade (`--forbid-dir`; KTD3 layer 1,
   KTD9). Paths are canonicalized, including symlinks and junctions, before
   the check, so a link cannot smuggle the export back inside.
2. `check_cases` refuses a case whose fix commit is not a strict descendant
   of the base revision, or whose acceptable member is not present by name in
   the exported file (R1). A question (kind `otazka`) is checked for its
   places only: files present in the export, declaring members present by name.
3. `generate_corpus` runs the RA binary on the export and writes a receipt;
   `load_corpus` refuses a corpus without a matching, still-valid receipt.

Fingerprint (`fingerprint_dir`, used for both the tree and the corpus):
SHA-256 over every regular file below the root, taken in ascending order of
its relative path written with '/' separators (Python string order); for each
file the digest is fed the UTF-8 path, one NUL byte, the file size as an
8-byte big-endian unsigned integer, and the raw file bytes. Directories
themselves (including empty ones) do not contribute.

Requires Python 3.12+ (`tarfile` extraction filters).
"""
from __future__ import annotations

import hashlib
import json
import os
import re
import shutil
import subprocess
import tarfile
import tempfile
import time
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any, Iterable, Sequence

from catalog import Case, Catalog, Question

RECEIPT_FORMAT_VERSION = 1
FINGERPRINT_ALGORITHM = "sha256-files-v1"

# Options that would make RA write somewhere other than the corpus directory:
# `--out` is set by the runner itself, `--mirror` writes into the tree the
# agent reads in the variant without cards.
_FORBIDDEN_RA_OPTIONS = ("--out", "--mirror")


class PrepareError(Exception):
    """Preparation refused an input; the experiment must not run on it."""


# --- paths -------------------------------------------------------------------

def canonical_path(path: str | os.PathLike[str]) -> str:
    """Absolute path with symlinks and junctions resolved, case-folded on Windows."""
    return os.path.normcase(os.path.realpath(os.path.abspath(path)))


def _is_within(path: str, root: str) -> bool:
    try:
        return os.path.commonpath([path, root]) == root
    except ValueError:  # different drives
        return False


def _find_runner_repo() -> str | None:
    """The repository this runner lives in (a `.git` dir, or file in a worktree)."""
    here = Path(__file__).resolve().parent
    for candidate in (here, *here.parents):
        if (candidate / ".git").exists():
            return str(candidate)
    return None


def protected_roots(source_repo_root: str | os.PathLike[str],
                    extra: Iterable[str | os.PathLike[str]] = ()) -> list[str]:
    """Roots no export or corpus may live under: the source repo, this repo, extras.

    Every worktree of either repository counts, not only the one named: run
    artifacts dropped into a sibling checkout of the same repository are one
    `git add -A` away from a commit just the same.
    """
    roots = [canonical_path(source_repo_root)]
    roots.extend(_worktrees(source_repo_root))
    runner = _find_runner_repo()
    if runner:
        roots.append(canonical_path(runner))
        roots.extend(_worktrees(runner))
    roots.extend(canonical_path(p) for p in extra)
    return roots


def _worktrees(repo: str | os.PathLike[str]) -> list[str]:
    """Canonical paths of all worktrees of the repository at `repo`; [] if git cannot tell."""
    try:
        result = _git(repo, "worktree", "list", "--porcelain")
    except OSError:  # git not installed or not runnable
        return []
    if result.returncode != 0:
        return []
    prefix = "worktree "
    return [canonical_path(line[len(prefix):]) for line in result.stdout.splitlines()
            if line.startswith(prefix)]


def ensure_outside(path: str | os.PathLike[str], roots: Sequence[str], what: str,
                   forbidden_names: Iterable[str] = ()) -> Path:
    """Return the canonical path, or refuse it if it lies under a protected root.

    `forbidden_names` are directory names (case-insensitive) the path must not
    pass through anywhere, e.g. another tool's workspace an agent must not see.
    """
    canon = canonical_path(path)
    names = {part.lower() for part in Path(canon).parts} | {
        part.lower() for part in Path(os.path.abspath(path)).parts}
    for name in forbidden_names:
        if name.lower() in names:
            raise PrepareError(f"{what} {path} leží v adresáři {name}; musí ležet mimo něj.")
    for root in roots:
        if _is_within(canon, root):
            raise PrepareError(f"{what} {path} leží uvnitř {root}; musí ležet mimo repozitáře.")
    return Path(canon)


def _require_empty(path: Path, what: str) -> None:
    if path.exists() and (not path.is_dir() or any(path.iterdir())):
        raise PrepareError(f"{what} {path} už existuje a není prázdný; zbytek po předchozím běhu?")


# --- git ---------------------------------------------------------------------

def _git(repo: str | os.PathLike[str], *args: str) -> subprocess.CompletedProcess[str]:
    # autocrlf off: the export must be the committed bytes on every machine,
    # otherwise the tree fingerprint would depend on the operator's config.
    return subprocess.run(["git", "-c", "core.autocrlf=false", "-C", str(repo), *args],
                          capture_output=True, text=True, encoding="utf-8", errors="replace")


def repo_toplevel(repo: str | os.PathLike[str]) -> str:
    """Top-level directory of the git repository containing `repo`."""
    result = _git(repo, "rev-parse", "--show-toplevel")
    if result.returncode != 0:
        raise PrepareError(f"{repo} není repozitář gitu: {result.stderr.strip()}")
    return result.stdout.strip()


def resolve_commit(repo: str | os.PathLike[str], rev: str) -> str | None:
    """Full sha of a commit, or None when the repository does not have it."""
    result = _git(repo, "rev-parse", "--verify", "--quiet", f"{rev}^{{commit}}")
    return result.stdout.strip() if result.returncode == 0 else None


# --- fingerprint -------------------------------------------------------------

def fingerprint_dir(root: str | os.PathLike[str]) -> str:
    """Fingerprint of a directory tree, as defined in the module docstring."""
    base = Path(root)
    rels: list[str] = []
    for dirpath, _dirnames, filenames in os.walk(base):
        for name in filenames:
            full = Path(dirpath) / name
            if full.is_file():
                rels.append(full.relative_to(base).as_posix())
    digest = hashlib.sha256()
    for rel in sorted(rels):
        data = (base / rel).read_bytes()
        digest.update(rel.encode("utf-8") + b"\0")
        digest.update(len(data).to_bytes(8, "big"))
        digest.update(data)
    return digest.hexdigest()


# --- export ------------------------------------------------------------------

@dataclass(frozen=True)
class ExportResult:
    """The exported base-revision tree and where it came from."""

    tree_dir: Path
    revision: str
    tree_fingerprint: str
    source_repo_root: Path


def export_tree(source_repo: str | os.PathLike[str], revision: str,
                dest: str | os.PathLike[str],
                forbidden_roots: Iterable[str | os.PathLike[str]] = (),
                forbidden_names: Iterable[str] = ()) -> ExportResult:
    """Export `revision` of `source_repo` into `dest` without git history."""
    root = repo_toplevel(source_repo)
    full = resolve_commit(root, revision)
    if full is None:
        raise PrepareError(f"Základní revize {revision} v repozitáři {root} neexistuje.")
    tree = ensure_outside(dest, protected_roots(root, forbidden_roots), "Adresář exportu",
                          forbidden_names)
    _require_empty(tree, "Adresář exportu")
    tree.mkdir(parents=True, exist_ok=True)
    try:
        _extract_archive(root, full, tree)
    except BaseException:
        shutil.rmtree(tree, ignore_errors=True)
        raise
    return ExportResult(tree_dir=tree, revision=full, tree_fingerprint=fingerprint_dir(tree),
                        source_repo_root=Path(canonical_path(root)))


def _extract_archive(repo: str, commit: str, dest: Path) -> None:
    """Stream `git archive` into the tar extractor; the tree is never buffered whole."""
    with tempfile.TemporaryFile() as stderr:
        proc = subprocess.Popen(
            ["git", "-c", "core.autocrlf=false", "-C", repo, "archive", "--format=tar", commit],
            stdout=subprocess.PIPE, stderr=stderr)
        try:
            with tarfile.open(fileobj=proc.stdout, mode="r|") as archive:
                archive.extractall(dest, filter="data")
        except tarfile.TarError as exc:
            proc.kill()
            proc.wait()
            raise PrepareError(f"Export {commit} selhal: {exc}") from exc
        finally:
            if proc.stdout:
                proc.stdout.close()
        if proc.wait() != 0:
            stderr.seek(0)
            message = stderr.read().decode("utf-8", "replace").strip()
            raise PrepareError(f"git archive {commit} selhal: {message}")


# --- case checks -------------------------------------------------------------

def find_case_problems(source_repo: str | os.PathLike[str], catalog: Catalog,
                       tree_dir: str | os.PathLike[str],
                       base_revision: str | None = None) -> list[str]:
    """List every reason a catalog case cannot run over this base revision.

    A fix commit must be a strict descendant of the base revision (equal
    would mean the base already contains the fix), and each acceptable
    member must occur by name, as a whole case-sensitive identifier, in the
    exported file. A question has no fix commit: only its places are checked
    (the file must be in the export and the declaring member, or the type of a
    type place, must occur in it as a whole identifier).
    """
    base = resolve_commit(source_repo, catalog.base_revision)
    if base is None:
        return [f"Základní revize katalogu {catalog.base_revision} v repozitáři neexistuje."]
    if base_revision is not None and resolve_commit(source_repo, base_revision) != base:
        return [f"Katalog je pro základní revizi {catalog.base_revision}, "
                f"pokus běží nad {base_revision}."]

    problems: list[str] = []
    tree = Path(tree_dir)
    for case in catalog.cases:
        problems += _bug_problems(source_repo, base, case, tree)
    for question in catalog.questions:
        problems += _question_problems(question, tree)
    return problems


def _bug_problems(source_repo: str | os.PathLike[str], base: str, case: Case,
                  tree: Path) -> list[str]:
    problems: list[str] = []
    for loc in case.acceptable_locations:
        prefix = f"Případ „{case.id}“, {loc.file} {loc.member}"
        commit_problem = _fix_commit_problem(source_repo, base, loc.fix_commit)
        if commit_problem:
            problems.append(f"{prefix}: {commit_problem}")
        member_problem = _member_problem(tree, loc.file, loc.member_name)
        if member_problem:
            problems.append(f"{prefix}: {member_problem}")
    return problems


def _question_problems(question: Question, tree: Path) -> list[str]:
    """A question has no fix commit; its places must exist in the exported tree."""
    problems: list[str] = []
    for label, place in question.places():
        problem = _member_problem(tree, place.soubor, place.name)
        if problem:
            problems.append(f"Otázka „{question.id}“, {label}, {place.soubor} "
                            f"{place.clen or place.typ}: {problem}")
    return problems


def check_cases(source_repo: str | os.PathLike[str], catalog: Catalog,
                tree_dir: str | os.PathLike[str], base_revision: str | None = None) -> None:
    """Refuse the catalog unless every case passes `find_case_problems`."""
    problems = find_case_problems(source_repo, catalog, tree_dir, base_revision)
    if problems:
        raise PrepareError("Kontrola případů neprošla:\n" + "\n".join(f"- {p}" for p in problems))


def _fix_commit_problem(repo: str | os.PathLike[str], base: str, fix: str) -> str | None:
    full = resolve_commit(repo, fix)
    if full is None:
        return f"commit opravy {fix} v repozitáři neexistuje"
    if full == base:
        return f"commit opravy {fix} je sama základní revize, chyba v ní už je opravená"
    result = _git(repo, "merge-base", "--is-ancestor", base, full)
    if result.returncode == 1:
        return (f"commit opravy {fix} není potomkem základní revize "
                f"(opraveno před ní nebo v jiné větvi)")
    if result.returncode != 0:
        return f"vztah commitu opravy {fix} k základní revizi nejde ověřit: {result.stderr.strip()}"
    return None


def _member_problem(tree: Path, file: str, name: str) -> str | None:
    path = tree / file
    if not path.is_file():
        return f"soubor {file} v exportu základní revize není"
    text = path.read_text(encoding="utf-8-sig", errors="replace")
    pattern = rf"(?<![A-Za-z0-9_]){re.escape(name)}(?![A-Za-z0-9_])"
    if not re.search(pattern, text):
        return f"člen {name} v souboru {file} v základní revizi není"
    return None


# --- corpus ------------------------------------------------------------------

@dataclass(frozen=True)
class CorpusReceipt:
    """What a corpus was generated from and how; written beside the corpus."""

    base_revision: str
    tree_fingerprint: str
    ra_version: str
    options: tuple[str, ...]
    corpus_fingerprint: str
    generation_seconds: float


def receipt_path(corpus_dir: str | os.PathLike[str]) -> Path:
    """`<corpus>.receipt.json` beside the corpus, so the corpus never contains it."""
    # realpath, so generate (canonical dir) and load (any alias) agree on the file.
    corpus = Path(os.path.realpath(os.path.abspath(corpus_dir)))
    return corpus.parent / f"{corpus.name}.receipt.json"


def generate_corpus(ra_command: Sequence[str], export: ExportResult,
                    corpus_dir: str | os.PathLike[str], options: Sequence[str] = (),
                    forbidden_roots: Iterable[str | os.PathLike[str]] = (),
                    forbidden_names: Iterable[str] = ()) -> CorpusReceipt:
    """Run RA on the export (`<ra...> <tree> --out <corpus> <options...>`) and write a receipt.

    `ra_command` is the command prefix, e.g. `["reverse-architect"]` or, in
    tests, `[sys.executable, "fake_ra.py"]`.
    """
    for option in options:
        if option in _FORBIDDEN_RA_OPTIONS or option.startswith("--out="):
            raise PrepareError(f"Volba {option} pro RA není povolená; výstup určuje běžeč.")
    roots = protected_roots(export.source_repo_root, forbidden_roots)
    roots.append(canonical_path(export.tree_dir))
    corpus = ensure_outside(corpus_dir, roots, "Adresář korpusu", forbidden_names)
    if _is_within(canonical_path(export.tree_dir), str(corpus)):
        raise PrepareError(f"Export {export.tree_dir} leží uvnitř adresáře korpusu {corpus}.")
    _require_empty(corpus, "Adresář korpusu")
    receipt_file = receipt_path(corpus)
    if receipt_file.exists():
        raise PrepareError(f"Účtenka {receipt_file} už existuje; zbytek po předchozím běhu?")
    tree_fingerprint = fingerprint_dir(export.tree_dir)
    if tree_fingerprint != export.tree_fingerprint:
        raise PrepareError(f"Export {export.tree_dir} se od exportu změnil.")

    ra_version = _run_ra([*ra_command, "--version"], "zjištění verze RA").strip()
    started = time.monotonic()
    _run_ra([*ra_command, str(export.tree_dir), "--out", str(corpus), *options], "generování korpusu")
    elapsed = time.monotonic() - started
    if not corpus.is_dir() or not any(corpus.iterdir()):
        raise PrepareError(f"RA nevytvořil v {corpus} žádný korpus.")

    receipt = CorpusReceipt(
        base_revision=export.revision,
        tree_fingerprint=tree_fingerprint,
        ra_version=ra_version,
        options=tuple(options),
        corpus_fingerprint=fingerprint_dir(corpus),
        generation_seconds=elapsed,
    )
    write_json_atomic(receipt_file, {
        "format_version": RECEIPT_FORMAT_VERSION,
        "fingerprint_algorithm": FINGERPRINT_ALGORITHM,
        **asdict(receipt),
        "options": list(receipt.options),
    })
    return receipt


def load_corpus(corpus_dir: str | os.PathLike[str], expected_tree_fingerprint: str) -> CorpusReceipt:
    """Return the corpus receipt, refusing a corpus that is not provably from this tree."""
    receipt_file = receipt_path(corpus_dir)
    if not receipt_file.is_file():
        raise PrepareError(f"Korpus {corpus_dir} nemá účtenku {receipt_file.name}; nepoužije se.")
    try:
        data = json.loads(receipt_file.read_text(encoding="utf-8"))
        if data.get("format_version") != RECEIPT_FORMAT_VERSION or \
                data.get("fingerprint_algorithm") != FINGERPRINT_ALGORITHM:
            raise ValueError("neznámá verze účtenky")
        receipt = CorpusReceipt(
            base_revision=str(data["base_revision"]),
            tree_fingerprint=str(data["tree_fingerprint"]),
            ra_version=str(data["ra_version"]),
            options=tuple(str(o) for o in data["options"]),
            corpus_fingerprint=str(data["corpus_fingerprint"]),
            generation_seconds=float(data["generation_seconds"]),
        )
    except (OSError, ValueError, KeyError, TypeError) as exc:
        raise PrepareError(f"Účtenka {receipt_file} je neplatná: {exc}") from exc
    if receipt.tree_fingerprint != expected_tree_fingerprint:
        raise PrepareError(f"Korpus {corpus_dir} vznikl z jiného stromu, než je export pokusu.")
    if fingerprint_dir(corpus_dir) != receipt.corpus_fingerprint:
        raise PrepareError(f"Korpus {corpus_dir} se od vystavení účtenky změnil.")
    return receipt


def _run_ra(argv: Sequence[str], what: str) -> str:
    try:
        result = subprocess.run(list(argv), capture_output=True, text=True,
                                encoding="utf-8", errors="replace")
    except OSError as exc:
        raise PrepareError(f"RA nejde spustit ({what}): {exc}") from exc
    if result.returncode != 0:
        tail = (result.stderr or result.stdout).strip()[-2000:]
        raise PrepareError(f"RA selhal při {what} (kód {result.returncode}): {tail}")
    return result.stdout


def write_text_atomic(path: Path, text: str) -> None:
    """Replace `path` in one step (temp file beside it + `os.replace`), so a crash never leaves
    half a file behind. The directory is created when it is missing."""
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(dir=path.parent, prefix=f".{path.name}.", suffix=".tmp")
    try:
        with os.fdopen(fd, "w", encoding="utf-8", newline="\n") as handle:
            handle.write(text)
        os.replace(tmp, path)
    except BaseException:
        if os.path.exists(tmp):
            os.unlink(tmp)
        raise


def write_json_atomic(path: Path, data: dict[str, Any]) -> None:
    """`write_text_atomic` of the JSON (indented, keys sorted, trailing newline)."""
    write_text_atomic(path, json.dumps(data, ensure_ascii=False, indent=2, sort_keys=True) + "\n")
