"""Case catalog for the search-eval experiment: format, loading and validation.

The real catalog lives outside the public repository (it carries customer
cases, R3); the repository holds only this format and an anonymous example
(`examples/catalog.example.json`). Validation exists so that a case which
would make the measurement meaningless never reaches a run: a case with no
acceptable location cannot be scored, one without a fix commit cannot be
checked against the base revision, and one whose bug or pseudonymization was
not confirmed by hand must not be sent to a model provider (R1, R2, R16).

Catalog JSON (format_version 1):

    {
      "format_version": 1,
      "base_revision": "<commit sha the whole experiment runs on>",
      "cases": [
        {
          "id": "<case identifier>",
          "prompt": "<pseudonymized prompt, as support received it>",
          "acceptable_locations": [
            {"file": "<repo-relative path, '/' separators>",
             "member": "<Type.Member, overload suffix allowed>",
             "line": <1-based line at the base revision>,
             "fix_commit": "<sha of the commit that fixed it>"}
          ],
          "bug_confirmed_at_base": true,
          "pseudonymization_confirmed": true,
          "used_for_card_tuning": false,
          "prompt_names_location": false
        }
      ]
    }

Every field is required: a misspelt flag must fail loudly rather than
silently default to "not used for tuning" or "prompt does not name the
location", because both flags change how the report reads a case (R1, R13).
Error messages are Czech, since the operator reading them is the owner.
"""
from __future__ import annotations

import json
import re
from dataclasses import dataclass
from pathlib import Path
from typing import Any

FORMAT_VERSION = 1

_SHA = re.compile(r"^[0-9a-f]{7,64}$")
_CASE_FIELDS = ("id", "prompt", "acceptable_locations", "bug_confirmed_at_base",
                "pseudonymization_confirmed", "used_for_card_tuning",
                "prompt_names_location")
_CONFIRMATIONS = ("bug_confirmed_at_base", "pseudonymization_confirmed")
_FLAGS = ("used_for_card_tuning", "prompt_names_location")
_GENERICS = re.compile(r"<[^<>]*>")


class CatalogError(ValueError):
    """The catalog is malformed or a case fails validation (Czech message)."""


@dataclass(frozen=True)
class AcceptableLocation:
    """One place the catalog accepts as the cause of a case's bug (R2)."""

    file: str
    member: str
    line: int
    fix_commit: str

    @property
    def member_name(self) -> str:
        """The member's simple name, used to find it textually in a file."""
        return member_simple_name(self.member)


@dataclass(frozen=True)
class Case:
    """One solved case: prompt, acceptable cause locations and manual checks."""

    id: str
    prompt: str
    acceptable_locations: tuple[AcceptableLocation, ...]
    bug_confirmed_at_base: bool
    pseudonymization_confirmed: bool
    used_for_card_tuning: bool
    prompt_names_location: bool


@dataclass(frozen=True)
class Catalog:
    """All cases of one experiment, which share a single base revision."""

    base_revision: str
    cases: tuple[Case, ...]


def member_simple_name(member: str) -> str:
    """Return the last dotted segment of a member, without overload or generics.

    `LoadAction.SetScheduledTasksInvalid` -> `SetScheduledTasksInvalid`;
    `Type.Run(+1)` -> `Run`; `Type.Map<T>(int)` -> `Map`;
    `Repository<T>.Save` -> `Save` (type arguments on the qualifier must not
    turn the type's name into the member's).
    """
    names = member_qualified_names(member)
    return names[-1] if names else ""


def member_qualified_names(member: str) -> list[str]:
    """Dotted name segments without the overload suffix and without generics.

    Generic argument lists are removed innermost first, so nested ones
    (`Map<K, List<V>>`) disappear whole.
    """
    head = member.strip().split("(", 1)[0]
    while _GENERICS.search(head):
        head = _GENERICS.sub("", head)
    return [part.strip() for part in head.split(".") if part.strip()]


def normalize_repo_path(path: str) -> str:
    """Normalize a repo-relative path to '/' separators, or raise ValueError.

    Absolute paths and `..` segments are refused: the file is later opened
    inside the exported tree, and a path that leaves it would read elsewhere.
    """
    text = path.replace("\\", "/").strip()
    if not text or text.startswith("/") or re.match(r"^[A-Za-z]:", text):
        raise ValueError(path)
    parts = [p for p in text.split("/") if p not in ("", ".")]
    if not parts or ".." in parts:
        raise ValueError(path)
    return "/".join(parts)


def load_catalog(path: str | Path) -> Catalog:
    """Read and validate a catalog JSON file."""
    try:
        data = json.loads(Path(path).read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise CatalogError(f"Katalog {path} nejde načíst: {exc}") from exc
    return parse_catalog(data)


def parse_catalog(data: Any) -> Catalog:
    """Validate already-parsed catalog JSON and build the model."""
    if not isinstance(data, dict):
        raise CatalogError("Katalog musí být objekt JSON.")
    if data.get("format_version") != FORMAT_VERSION:
        raise CatalogError(
            f"Katalog: pole „format_version“ musí být {FORMAT_VERSION}, "
            f"je {data.get('format_version')!r}.")
    base = data.get("base_revision")
    if not isinstance(base, str) or not _SHA.match(base):
        raise CatalogError("Katalog: pole „base_revision“ musí být commit (hex sha).")
    raw_cases = data.get("cases")
    if not isinstance(raw_cases, list) or not raw_cases:
        raise CatalogError("Katalog: pole „cases“ musí být neprázdný seznam případů.")

    cases: list[Case] = []
    seen: set[str] = set()
    for index, raw in enumerate(raw_cases):
        case = _parse_case(raw, index)
        if case.id in seen:
            raise CatalogError(f"Případ „{case.id}“: pole „id“ se v katalogu opakuje.")
        seen.add(case.id)
        cases.append(case)
    return Catalog(base_revision=base, cases=tuple(cases))


def _parse_case(raw: Any, index: int) -> Case:
    if not isinstance(raw, dict):
        raise CatalogError(f"Případ č. {index + 1}: musí být objekt JSON.")
    case_id = raw.get("id")
    if not isinstance(case_id, str) or not case_id.strip():
        raise CatalogError(f"Případ č. {index + 1}: chybí pole „id“.")

    def fail(field: str, problem: str) -> CatalogError:
        return CatalogError(f"Případ „{case_id}“: pole „{field}“ {problem}.")

    for field in _CASE_FIELDS:
        if field not in raw:
            raise fail(field, "chybí")
    prompt = raw["prompt"]
    if not isinstance(prompt, str) or not prompt.strip():
        raise fail("prompt", "musí být neprázdný text zadání")
    for field in _CONFIRMATIONS:
        if raw[field] is not True:
            raise fail(field, "musí být true — ruční potvrzení chybí")
    for field in _FLAGS:
        if not isinstance(raw[field], bool):
            raise fail(field, "musí být true nebo false")

    raw_locations = raw["acceptable_locations"]
    if not isinstance(raw_locations, list) or not raw_locations:
        raise fail("acceptable_locations", "musí obsahovat aspoň jedno přijatelné místo")
    locations = tuple(_parse_location(loc, case_id, i) for i, loc in enumerate(raw_locations))

    return Case(
        id=case_id,
        prompt=prompt,
        acceptable_locations=locations,
        bug_confirmed_at_base=True,
        pseudonymization_confirmed=True,
        used_for_card_tuning=raw["used_for_card_tuning"],
        prompt_names_location=raw["prompt_names_location"],
    )


def _parse_location(raw: Any, case_id: str, index: int) -> AcceptableLocation:
    where = f"Případ „{case_id}“, přijatelné místo č. {index + 1}"
    if not isinstance(raw, dict):
        raise CatalogError(f"{where}: musí být objekt JSON.")

    def fail(field: str, problem: str) -> CatalogError:
        return CatalogError(f"{where}: pole „{field}“ {problem}.")

    for field in ("file", "member", "line", "fix_commit"):
        if field not in raw:
            raise fail(field, "chybí")
    try:
        file = normalize_repo_path(raw["file"]) if isinstance(raw["file"], str) else None
    except ValueError:
        file = None
    if file is None:
        raise fail("file", "musí být relativní cesta uvnitř stromu (bez „..“)")
    member = raw["member"]
    if not isinstance(member, str) or not member_simple_name(member):
        raise fail("member", "musí být neprázdné jméno člena")
    line = raw["line"]
    if not isinstance(line, int) or isinstance(line, bool) or line < 1:
        raise fail("line", "musí být kladné celé číslo")
    fix = raw["fix_commit"]
    if not isinstance(fix, str) or not _SHA.match(fix):
        raise fail("fix_commit", "musí být commit opravy (hex sha)")
    return AcceptableLocation(file=file, member=member, line=line, fix_commit=fix)
