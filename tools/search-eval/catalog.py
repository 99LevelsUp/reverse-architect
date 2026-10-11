"""Case catalog for the search-eval experiment: format, loading and validation.

The real catalog lives outside the public repository (it carries customer
cases, R3); the repository holds only this format and an anonymous example
(`examples/catalog.example.json`). Validation exists so that a case which
would make the measurement meaningless never reaches a run: a case with no
acceptable location cannot be scored, one without a fix commit cannot be
checked against the base revision, and one whose bug or pseudonymization was
not confirmed by hand must not be sent to a model provider (R1, R2, R16).

Catalog JSON, format_version 2 (format 1 is still read: every item of a
format 1 catalog is a `chyba`, and has no `druh` field):

    {
      "format_version": 2,
      "base_revision": "<commit sha the whole experiment runs on>",
      "cases": [
        {
          "druh": "chyba",
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
        },
        {
          "druh": "otazka",
          "id": "<question identifier>",
          "dotaz": "<the user's question, pseudonymized>",
          "ramcova_odpoved": "<expected framework answer>",
          "commit": "<sha the facts were made over; must equal base_revision>",
          "fakta_potvrzena": true,
          "pseudonymizace_potvrzena": true,
          "otisk_potvrzeni": "<sha-256 hex, see below>",
          "kontrolni": false,
          "fakta": [
            {"id": "f1", "text": "<verifiable fact>",
             "role": "ramcova | misto",
             "mista": [{"soubor": "<repo-relative path, '/' separators>",
                        "clen": "<Type.Member, overload suffix allowed>",
                        "radky": 12,
                        "odkazovany_clen": "<field or property the place writes or reads>"}],
             "tok": {"krok": 1, "tok": "<flow id>"}}
          ],
          "zakazana_tvrzeni": [
            {"id": "z1", "text": "<known falsehood>",
             "mista": [{"soubor": "...", "typ": "<Type>", "radky": "5-9"}],
             "fakty": ["f1"]}
          ]
        }
      ]
    }

Items of the `otazka` kind:

* A fact has the role `ramcova` (part of the framework answer for a reader
  without the sources) or `misto` (a place for a developer). `tok` is
  optional and is either `{"krok": <order>, "tok": "<flow id>"}` or
  `{"cely_tok": "<flow id>"}`. A `cely_tok` fact must carry the places of
  every `krok` fact of the same flow (same file, member or type, and lines),
  or nothing could tell whether the whole flow was named.
* A place has `soubor` and exactly one of `clen` (the declaring member: the
  member whose declaration contains the place, so a write to a field inside
  the method `Check(bool)` is a place in `Check(bool)`) and `typ` (a place of
  a type). `odkazovany_clen` is optional and names the field or property the
  place writes or reads. `radky` is a single 1-based line (a number) or a
  range written as the text `"N-M"` with N <= M; a list `[N, M]` is not
  accepted, so there is one spelling only. A forbidden claim's places have
  the same shape.
* `fakty` of a forbidden claim lists ids of facts of the same question that
  the claim contradicts; an id that names no fact is refused.
* The `commit` of a question must equal `base_revision`: the catalog has one
  export.
* Confirmation. `fakta_potvrzena` must be true, `pseudonymizace_potvrzena`
  must be true, and `otisk_potvrzeni` must equal `confirmation_fingerprint`
  of the item's `fakta` and `zakazana_tvrzeni` as they stand in the file. The
  operator computes the fingerprint when confirming the facts; a fact or a
  forbidden claim edited after that no longer matches and the catalog is
  refused (R16), exactly as an unconfirmed item is. `confirmation_fingerprint`
  is the SHA-256 (hex) of the UTF-8 canonical JSON (`sort_keys=True`,
  `ensure_ascii=False`, separators `(",", ":")`) of the object
  `{"fakta": <fakta>, "zakazana_tvrzeni": <zakazana_tvrzeni>}`.
* `kontrolni` marks a control question that is not an acceptance item; it
  loads like any other question and is told apart by `Question.kontrolni`.

Every field is required (except the optional ones named above): a misspelt
flag must fail loudly rather than silently default to "not used for tuning"
or "prompt does not name the location", because both flags change how the
report reads a case (R1, R13). Error messages are Czech, since the operator
reading them is the owner.
"""
from __future__ import annotations

import hashlib
import json
import re
from dataclasses import dataclass
from pathlib import Path
from typing import Any, ClassVar, Sequence

FORMAT_VERSION = 2
SUPPORTED_FORMAT_VERSIONS = (1, 2)

KIND_BUG = "chyba"
KIND_QUESTION = "otazka"
ROLE_FRAMEWORK = "ramcova"  # a fact of the framework: the support reader needs it too
ROLE_PLACE = "misto"  # a fact of a place: only the developer reads it
ROLES = (ROLE_FRAMEWORK, ROLE_PLACE)

_SHA = re.compile(r"^[0-9a-f]{7,64}$")
_SHA256 = re.compile(r"^[0-9a-f]{64}$")
_CASE_FIELDS = ("id", "prompt", "acceptable_locations", "bug_confirmed_at_base",
                "pseudonymization_confirmed", "used_for_card_tuning",
                "prompt_names_location")
_CONFIRMATIONS = ("bug_confirmed_at_base", "pseudonymization_confirmed")
_FLAGS = ("used_for_card_tuning", "prompt_names_location")
_QUESTION_FIELDS = ("id", "dotaz", "ramcova_odpoved", "commit", "fakta_potvrzena",
                    "pseudonymizace_potvrzena", "otisk_potvrzeni", "kontrolni", "fakta",
                    "zakazana_tvrzeni")
_GENERICS = re.compile(r"<[^<>]*>")
_LINE_RANGE = re.compile(r"^(\d+)-(\d+)$")


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
    """One solved case (kind `chyba`): prompt, acceptable cause locations, manual checks."""

    druh: ClassVar[str] = KIND_BUG

    id: str
    prompt: str
    acceptable_locations: tuple[AcceptableLocation, ...]
    bug_confirmed_at_base: bool
    pseudonymization_confirmed: bool
    used_for_card_tuning: bool
    prompt_names_location: bool

    @property
    def prompt_text(self) -> str:
        """What the agent is given to work on: the case's prompt."""
        return self.prompt.strip()


@dataclass(frozen=True)
class Place:
    """A place in the code: file, declaring member or type, lines, referenced member."""

    soubor: str
    clen: str | None
    typ: str | None
    radky: tuple[int, int]
    odkazovany_clen: str | None

    @property
    def name(self) -> str:
        """The simple name of the declaring member (or of the type), found textually."""
        return member_simple_name(self.clen if self.clen is not None else self.typ or "")


@dataclass(frozen=True)
class FlowRef:
    """A fact's place in a flow: step number `krok`, or the whole flow (`krok` is None)."""

    tok: str
    krok: int | None

    @property
    def whole(self) -> bool:
        return self.krok is None


@dataclass(frozen=True)
class Fact:
    """One verifiable fact of a question's expected answer."""

    id: str
    text: str
    role: str
    mista: tuple[Place, ...]
    tok: FlowRef | None


@dataclass(frozen=True)
class ForbiddenClaim:
    """A known falsehood the answer must not state."""

    id: str
    text: str
    mista: tuple[Place, ...]
    fakty: tuple[str, ...]


@dataclass(frozen=True)
class Question:
    """One question (kind `otazka`): user query, expected facts and forbidden claims."""

    druh: ClassVar[str] = KIND_QUESTION

    id: str
    dotaz: str
    ramcova_odpoved: str
    commit: str
    fakta_potvrzena: bool
    pseudonymizace_potvrzena: bool
    otisk_potvrzeni: str
    kontrolni: bool
    fakta: tuple[Fact, ...]
    zakazana_tvrzeni: tuple[ForbiddenClaim, ...]

    @property
    def prompt_text(self) -> str:
        """What the agent is given to work on: the question's `dotaz`."""
        return self.dotaz.strip()

    def places(self) -> list[tuple[str, Place]]:
        """Every place of the question with a label naming its fact or claim."""
        out = [(f"fakt „{f.id}“", p) for f in self.fakta for p in f.mista]
        out += [(f"zakázané tvrzení „{c.id}“", p) for c in self.zakazana_tvrzeni
                for p in c.mista]
        return out


@dataclass(frozen=True)
class Catalog:
    """All items of one experiment, which share a single base revision.

    `cases` holds the items of kind `chyba` and `questions` those of kind
    `otazka`, each in catalog order, so code written for navigation cases
    never meets a question; `items` is both in one tuple, bugs first.
    """

    base_revision: str
    cases: tuple[Case, ...]
    questions: tuple[Question, ...] = ()

    @property
    def items(self) -> tuple[Case | Question, ...]:
        return (*self.cases, *self.questions)


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


def confirmation_fingerprint(fakta: Sequence[Any], zakazana_tvrzeni: Sequence[Any]) -> str:
    """SHA-256 (hex) over a question's facts and forbidden claims, as in the module docstring.

    Takes the lists as they stand in the catalog JSON (parsed, not yet
    validated), so the operator can compute the value for `otisk_potvrzeni`
    when confirming the facts.
    """
    canonical = json.dumps({"fakta": fakta, "zakazana_tvrzeni": zakazana_tvrzeni},
                           sort_keys=True, ensure_ascii=False, separators=(",", ":"))
    return hashlib.sha256(canonical.encode("utf-8")).hexdigest()


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
    version = data.get("format_version")
    if isinstance(version, bool) or version not in SUPPORTED_FORMAT_VERSIONS:
        raise CatalogError(
            f"Katalog: pole „format_version“ musí být {FORMAT_VERSION} "
            f"(nebo starší {SUPPORTED_FORMAT_VERSIONS[0]}), je {version!r}.")
    base = data.get("base_revision")
    if not isinstance(base, str) or not _SHA.match(base):
        raise CatalogError("Katalog: pole „base_revision“ musí být commit (hex sha).")
    raw_cases = data.get("cases")
    if not isinstance(raw_cases, list) or not raw_cases:
        raise CatalogError("Katalog: pole „cases“ musí být neprázdný seznam případů.")

    cases: list[Case] = []
    questions: list[Question] = []
    seen: set[str] = set()
    for index, raw in enumerate(raw_cases):
        item = _parse_item(raw, index, version, base)
        if item.id in seen:
            raise CatalogError(f"Případ „{item.id}“: pole „id“ se v katalogu opakuje.")
        seen.add(item.id)
        (cases if isinstance(item, Case) else questions).append(item)
    return Catalog(base_revision=base, cases=tuple(cases), questions=tuple(questions))


def _parse_item(raw: Any, index: int, version: int, base: str) -> Case | Question:
    if not isinstance(raw, dict):
        raise CatalogError(f"Případ č. {index + 1}: musí být objekt JSON.")
    item_id = raw.get("id")
    if not isinstance(item_id, str) or not item_id.strip():
        raise CatalogError(f"Případ č. {index + 1}: chybí pole „id“.")
    if version == 1:
        if "druh" in raw:
            raise CatalogError(f"Případ „{item_id}“: katalog formátu 1 nezná pole „druh“.")
        return _parse_case(raw, item_id)
    kind = raw.get("druh")
    if kind == KIND_BUG:
        return _parse_case(raw, item_id)
    if kind == KIND_QUESTION:
        return _parse_question(raw, item_id, base)
    raise CatalogError(f"Případ „{item_id}“: pole „druh“ musí být „{KIND_BUG}“ nebo "
                       f"„{KIND_QUESTION}“, je {kind!r}.")


def _parse_case(raw: dict[str, Any], case_id: str) -> Case:
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


# --- questions ---------------------------------------------------------------

def _parse_question(raw: dict[str, Any], qid: str, base: str) -> Question:
    def fail(field: str, problem: str) -> CatalogError:
        return CatalogError(f"Otázka „{qid}“: pole „{field}“ {problem}.")

    for field in _QUESTION_FIELDS:
        if field not in raw:
            raise fail(field, "chybí")
    for field in ("dotaz", "ramcova_odpoved"):
        if not isinstance(raw[field], str) or not raw[field].strip():
            raise fail(field, "musí být neprázdný text")
    commit = raw["commit"]
    if not isinstance(commit, str) or not _SHA.match(commit):
        raise fail("commit", "musí být commit zdrojáků (hex sha)")
    if commit != base:
        raise fail("commit", f"({commit}) se musí rovnat „base_revision“ katalogu ({base}): "
                             "katalog má jeden export")
    if raw["fakta_potvrzena"] is not True:
        raise fail("fakta_potvrzena", "musí být true — ruční potvrzení faktů chybí")
    if raw["pseudonymizace_potvrzena"] is not True:
        raise fail("pseudonymizace_potvrzena", "musí být true — ruční potvrzení chybí")
    if not isinstance(raw["kontrolni"], bool):
        raise fail("kontrolni", "musí být true nebo false")

    raw_facts, raw_claims = raw["fakta"], raw["zakazana_tvrzeni"]
    if not isinstance(raw_facts, list) or not raw_facts:
        raise fail("fakta", "musí obsahovat aspoň jeden fakt")
    if not isinstance(raw_claims, list):
        raise fail("zakazana_tvrzeni", "musí být seznam (může být prázdný)")
    facts = tuple(_parse_fact(f, qid, i) for i, f in enumerate(raw_facts))
    claims = tuple(_parse_claim(c, qid, i) for i, c in enumerate(raw_claims))
    _check_ids(qid, facts, claims)
    _check_whole_flows(qid, facts)

    fingerprint = raw["otisk_potvrzeni"]
    if not isinstance(fingerprint, str) or not _SHA256.match(fingerprint):
        raise fail("otisk_potvrzeni", "musí být SHA-256 (64 hex znaků) potvrzených faktů")
    if fingerprint != confirmation_fingerprint(raw_facts, raw_claims):
        raise fail("otisk_potvrzeni", "nesedí: fakta nebo zakázaná tvrzení se od potvrzení "
                                      "změnily, potvrď je znovu")
    return Question(id=qid, dotaz=raw["dotaz"], ramcova_odpoved=raw["ramcova_odpoved"],
                    commit=commit, fakta_potvrzena=True, pseudonymizace_potvrzena=True,
                    otisk_potvrzeni=fingerprint, kontrolni=raw["kontrolni"], fakta=facts,
                    zakazana_tvrzeni=claims)


def _parse_fact(raw: Any, qid: str, index: int) -> Fact:
    if not isinstance(raw, dict):
        raise CatalogError(f"Otázka „{qid}“, fakt č. {index + 1}: musí být objekt JSON.")
    fid = raw.get("id")
    if not isinstance(fid, str) or not fid.strip():
        raise CatalogError(f"Otázka „{qid}“, fakt č. {index + 1}: chybí pole „id“.")
    where = f"Otázka „{qid}“, fakt „{fid}“"

    def fail(field: str, problem: str) -> CatalogError:
        return CatalogError(f"{where}: pole „{field}“ {problem}.")

    for field in ("text", "role", "mista"):
        if field not in raw:
            raise fail(field, "chybí")
    if not isinstance(raw["text"], str) or not raw["text"].strip():
        raise fail("text", "musí být neprázdný text")
    if raw["role"] not in ROLES:
        raise fail("role", f"musí být „{ROLES[0]}“ nebo „{ROLES[1]}“, je {raw['role']!r}")
    places = _parse_places(raw["mista"], where)
    return Fact(id=fid, text=raw["text"], role=raw["role"], mista=places,
                tok=_parse_flow(raw.get("tok"), where))


def _parse_claim(raw: Any, qid: str, index: int) -> ForbiddenClaim:
    if not isinstance(raw, dict):
        raise CatalogError(f"Otázka „{qid}“, zakázané tvrzení č. {index + 1}: "
                           "musí být objekt JSON.")
    cid = raw.get("id")
    if not isinstance(cid, str) or not cid.strip():
        raise CatalogError(f"Otázka „{qid}“, zakázané tvrzení č. {index + 1}: chybí pole „id“.")
    where = f"Otázka „{qid}“, zakázané tvrzení „{cid}“"

    def fail(field: str, problem: str) -> CatalogError:
        return CatalogError(f"{where}: pole „{field}“ {problem}.")

    for field in ("text", "mista"):
        if field not in raw:
            raise fail(field, "chybí")
    if not isinstance(raw["text"], str) or not raw["text"].strip():
        raise fail("text", "musí být neprázdný text")
    places = _parse_places(raw["mista"], where)
    refs = raw.get("fakty", [])
    if not isinstance(refs, list) or not all(isinstance(r, str) for r in refs):
        raise fail("fakty", "musí být seznam id faktů")
    return ForbiddenClaim(id=cid, text=raw["text"], mista=places, fakty=tuple(refs))


def _check_ids(qid: str, facts: Sequence[Fact], claims: Sequence[ForbiddenClaim]) -> None:
    fact_ids: set[str] = set()
    for fact in facts:
        if fact.id in fact_ids:
            raise CatalogError(f"Otázka „{qid}“, fakt „{fact.id}“: pole „id“ se v otázce "
                               "opakuje.")
        fact_ids.add(fact.id)
    claim_ids: set[str] = set()
    for claim in claims:
        if claim.id in claim_ids:
            raise CatalogError(f"Otázka „{qid}“, zakázané tvrzení „{claim.id}“: pole „id“ "
                               "se v otázce opakuje.")
        claim_ids.add(claim.id)
        for ref in claim.fakty:
            if ref not in fact_ids:
                raise CatalogError(f"Otázka „{qid}“, zakázané tvrzení „{claim.id}“: pole "
                                   f"„fakty“ odkazuje na neexistující fakt „{ref}“.")


def _check_whole_flows(qid: str, facts: Sequence[Fact]) -> None:
    """A `cely_tok` fact must cover the places of every `krok` fact of its flow."""
    for fact in facts:
        if fact.tok is None or not fact.tok.whole:
            continue
        steps = [f for f in facts if f.tok is not None and not f.tok.whole
                 and f.tok.tok == fact.tok.tok]
        where = f"Otázka „{qid}“, fakt „{fact.id}“: pole „tok“"
        if not steps:
            raise CatalogError(f"{where} určuje celý tok „{fact.tok.tok}“, ale žádný fakt nemá "
                               "jeho krok.")
        covered = {_place_key(p) for p in fact.mista}
        for step in steps:
            missing = [p for p in step.mista if _place_key(p) not in covered]
            if missing:
                raise CatalogError(
                    f"{where} určuje celý tok „{fact.tok.tok}“, ale jeho místa nepokrývají "
                    f"místa kroku {step.tok.krok} (fakt „{step.id}“): "
                    f"{missing[0].soubor} {missing[0].clen or missing[0].typ}.")


def _place_key(place: Place) -> tuple[str, str | None, str | None, tuple[int, int]]:
    return (place.soubor, place.clen, place.typ, place.radky)


def _parse_flow(raw: Any, where: str) -> FlowRef | None:
    if raw is None:
        return None

    def fail(problem: str) -> CatalogError:
        return CatalogError(f"{where}: pole „tok“ {problem}.")

    if not isinstance(raw, dict):
        raise fail("musí být objekt s „krok“ a „tok“, nebo s „cely_tok“")
    if set(raw) == {"cely_tok"}:
        flow = raw["cely_tok"]
        if not isinstance(flow, str) or not flow.strip():
            raise fail("„cely_tok“ musí být neprázdné id toku")
        return FlowRef(tok=flow, krok=None)
    if set(raw) == {"krok", "tok"}:
        step, flow = raw["krok"], raw["tok"]
        if not isinstance(step, int) or isinstance(step, bool) or step < 1:
            raise fail("„krok“ musí být kladné celé číslo")
        if not isinstance(flow, str) or not flow.strip():
            raise fail("„tok“ musí být neprázdné id toku")
        return FlowRef(tok=flow, krok=step)
    raise fail("musí mít buď klíče „krok“ a „tok“, nebo jediný klíč „cely_tok“")


def _parse_places(raw: Any, where: str) -> tuple[Place, ...]:
    if not isinstance(raw, list) or not raw:
        raise CatalogError(f"{where}: pole „mista“ musí obsahovat aspoň jedno místo.")
    return tuple(_parse_place(p, f"{where}, místo č. {i + 1}") for i, p in enumerate(raw))


def _parse_place(raw: Any, where: str) -> Place:
    if not isinstance(raw, dict):
        raise CatalogError(f"{where}: musí být objekt JSON.")

    def fail(field: str, problem: str) -> CatalogError:
        return CatalogError(f"{where}: pole „{field}“ {problem}.")

    for field in ("soubor", "radky"):
        if field not in raw:
            raise fail(field, "chybí")
    try:
        soubor = normalize_repo_path(raw["soubor"]) if isinstance(raw["soubor"], str) else None
    except ValueError:
        soubor = None
    if soubor is None:
        raise fail("soubor", "musí být relativní cesta uvnitř stromu (bez „..“)")
    has_member, has_type = raw.get("clen") is not None, raw.get("typ") is not None
    if has_member == has_type:
        raise fail("clen", "a „typ“: místo má právě jedno z nich")
    clen = typ = None
    if has_member:
        clen = raw["clen"]
        if not isinstance(clen, str) or not member_simple_name(clen):
            raise fail("clen", "musí být neprázdné jméno člena")
    else:
        typ = raw["typ"]
        if not isinstance(typ, str) or not member_simple_name(typ):
            raise fail("typ", "musí být neprázdné jméno typu")
    referenced = raw.get("odkazovany_clen")
    if referenced is not None and (not isinstance(referenced, str)
                                   or not member_simple_name(referenced)):
        raise fail("odkazovany_clen", "musí být neprázdné jméno pole nebo vlastnosti")
    return Place(soubor=soubor, clen=clen, typ=typ, radky=_parse_lines(raw["radky"], fail),
                 odkazovany_clen=referenced)


def _parse_lines(raw: Any, fail: Any) -> tuple[int, int]:
    """`radky`: a positive number, or the text "N-M" with N <= M (see the module docstring)."""
    if isinstance(raw, int) and not isinstance(raw, bool) and raw >= 1:
        return (raw, raw)
    if isinstance(raw, str):
        match = _LINE_RANGE.match(raw.strip())
        if match and 1 <= int(match.group(1)) <= int(match.group(2)):
            return (int(match.group(1)), int(match.group(2)))
    raise fail("radky", "musí být kladné číslo řádku nebo rozsah v textu „N-M“ (N <= M)")
