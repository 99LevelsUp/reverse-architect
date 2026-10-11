"""Scoring of one run: success, failure or spoiled, with exactly one reason (U4).

The answer is taken only from the agent's last top-level message (KTD6),
never from a tool result or an intermediate message: a file the agent merely
read may contain a JSON block in the answer's shape, and accepting that would
score text the agent did not submit.

Precedence, so every run gets exactly one state:

1. `spoiled` — the audit found a forbidden attempt (the run is replaced, R6).
2. `failure` with `cap_steps` / `cap_tokens` / `cap_time` — the cap stopped
   the run; any answer text is ignored (R7, AE7).
3. `failure` with `no_answer` — there is no final assistant message.
4. `failure` with `invalid_answer` — the final message has no JSON object
   with string `file`, `member` and `hypothesis`.
5. `failure` with `wrong_location` — the answer matches no acceptable place.
6. `success`.

An item of kind `otazka` (a question, KTD10) has no score here: the agent
answers in free text and `answers.py hodnot-odpovedi` judges the answer later.
Its run gets one of two states without a score, after the audit (`spoiled`
comes first, as for every run): `answered` with the whole last top-level
message as `answer_text`, or `unanswered` (no final message, or a cap stopped
the run; `failure_reason` then says which, and any text is not kept). The text
is never read for an answer-shaped JSON block, so such a run is never
`invalid_answer`.

Matching against an acceptable location (R7, KTD6):

* File: an absolute path inside the work tree is made relative to it (the
  agent sees absolute paths in its tools and may echo one back); then
  separators are normalized to '/' and the comparison ignores case. An
  absolute path outside the tree, or one with `..`, matches nothing. A bare
  file name does not match a path in a subdirectory.
* Member: the simple names (last dotted segment, without generics or
  overload) must be equal, and the dotted qualifiers must agree as far as
  both go — `SetX` and `Ns.LoadAction.SetX` match `LoadAction.SetX`,
  `Other.SetX` does not.
* Overloads are not compared: a parenthesized suffix is dropped on both
  sides and members match by qualified name only. The catalog writes
  overloads in the SCIP index's notation (`Rounding.Apply(+1)` = "the second
  overload"), which is not source-language notation: an agent working from
  the sources alone has no way to produce it, while the cards variant could
  copy it from a card, so requiring it would bias the comparison toward the
  cards variant. A catalog suffix in any other form is informative only as
  well — a source-form overload spec (parameter types as the code writes
  them) may be added later; it does not exist yet.
"""
from __future__ import annotations

import json
import os
import re
from dataclasses import dataclass
from typing import Any

from agents.base import (CAP_STEPS, CAP_TIME, CAP_TOKENS, RunRecord, last_assistant_text)
from audit import AuditFinding
from catalog import (AcceptableLocation, Case, Question, member_qualified_names,
                     member_simple_name, normalize_repo_path)
from prepare import canonical_path

SUCCESS = "success"
FAILURE = "failure"
SPOILED = "spoiled"
ANSWERED = "answered"      # a question run with an answer text; no score (KTD10)
UNANSWERED = "unanswered"  # a question run without one ("bez odpovědi")

WRONG_LOCATION = "wrong_location"
INVALID_ANSWER = "invalid_answer"
NO_ANSWER = "no_answer"
CAP_REASONS = {CAP_STEPS: "cap_steps", CAP_TOKENS: "cap_tokens", CAP_TIME: "cap_time"}

_FENCED = re.compile(r"```[A-Za-z]*[ \t]*\r?\n(.*?)```", re.DOTALL)


@dataclass(frozen=True)
class Answer:
    """The agent's submission: file, member and hypothesis."""

    file: str
    member: str
    hypothesis: str


@dataclass(frozen=True)
class RunOutcome:
    """The single verdict of a run.

    `failure_reason` is set only for `failure` and `unanswered`, `spoil_reasons`
    only for `spoiled`; `matched` is the acceptable location of a success and
    `answer_text` the free answer of an `answered` question run.
    """

    state: str
    failure_reason: str | None = None
    spoil_reasons: tuple[AuditFinding, ...] = ()
    answer: Answer | None = None
    matched: AcceptableLocation | None = None
    answer_text: str | None = None


def evaluate_run(record: RunRecord, item: Case | Question,
                 findings: tuple[AuditFinding, ...]) -> RunOutcome:
    """Combine the audit findings, the cap and the answer into one outcome.

    `item` is a case (scored) or a question (stored, not scored)."""
    if findings:
        return RunOutcome(SPOILED, spoil_reasons=tuple(findings))
    if isinstance(item, Question):
        return _question_outcome(record)
    case = item
    if record.cap_hit is not None:
        return RunOutcome(FAILURE, failure_reason=CAP_REASONS[record.cap_hit.kind])
    message = last_assistant_text(record.events)
    if message is None:
        return RunOutcome(FAILURE, failure_reason=NO_ANSWER)
    answer = extract_answer(message)
    if answer is None:
        return RunOutcome(FAILURE, failure_reason=INVALID_ANSWER)
    for location in case.acceptable_locations:
        if location_matches(answer, location, record.workdir):
            return RunOutcome(SUCCESS, answer=answer, matched=location)
    return RunOutcome(FAILURE, failure_reason=WRONG_LOCATION, answer=answer)


def _question_outcome(record: RunRecord) -> RunOutcome:
    """A question run: the last top-level message as it is, or "bez odpovědi" (KTD10)."""
    if record.cap_hit is not None:
        return RunOutcome(UNANSWERED, failure_reason=CAP_REASONS[record.cap_hit.kind])
    message = last_assistant_text(record.events)
    if message is None:
        return RunOutcome(UNANSWERED, failure_reason=NO_ANSWER)
    return RunOutcome(ANSWERED, answer_text=message)


def extract_answer(message: str) -> Answer | None:
    """The last answer-shaped JSON object in the message, or None.

    Fenced code blocks are tried first; without any, a bare JSON object in
    the text is accepted. Several objects: the last valid one counts.
    """
    blocks = _FENCED.findall(message)
    candidates: list[Any] = []
    if blocks:
        for block in blocks:
            try:
                candidates.append(json.loads(block))
            except json.JSONDecodeError:
                continue
    else:
        candidates = _bare_objects(message)
    for candidate in reversed(candidates):
        answer = _as_answer(candidate)
        if answer is not None:
            return answer
    return None


def _bare_objects(text: str) -> list[Any]:
    decoder = json.JSONDecoder()
    found: list[Any] = []
    index = text.find("{")
    while index != -1:
        try:
            obj, end = decoder.raw_decode(text, index)
        except json.JSONDecodeError:
            index = text.find("{", index + 1)
            continue
        found.append(obj)
        index = text.find("{", end)
    return found


def _as_answer(obj: Any) -> Answer | None:
    if not isinstance(obj, dict):
        return None
    file, member, hypothesis = obj.get("file"), obj.get("member"), obj.get("hypothesis")
    if not (isinstance(file, str) and isinstance(member, str) and isinstance(hypothesis, str)):
        return None
    if not file.strip() or not member_simple_name(member):
        return None
    return Answer(file=file.strip(), member=member.strip(), hypothesis=hypothesis)


def location_matches(answer: Answer, location: AcceptableLocation, workdir: str | None) -> bool:
    """Whether the answer names this acceptable location (rules in the module docstring)."""
    file = normalize_answer_file(answer.file, workdir)
    if file is None or file.casefold() != location.file.casefold():
        return False
    return member_matches(answer.member, location.member)


def normalize_answer_file(path: str, workdir: str | None) -> str | None:
    """Repo-relative '/' path of the answer's file, or None if it is not in the tree."""
    text = path.strip()
    if os.path.isabs(text) or re.match(r"^[A-Za-z]:[\\/]", text):
        if workdir is None:
            return None
        canon, root = canonical_path(text), canonical_path(workdir)
        try:
            if os.path.commonpath([canon, root]) != root:
                return None
        except ValueError:
            return None
        text = os.path.relpath(canon, root)
    try:
        return normalize_repo_path(text)
    except ValueError:
        return None


def member_matches(answer_member: str, catalog_member: str) -> bool:
    """Same member by qualified name; overload suffixes are ignored (module docstring)."""
    a_names = member_qualified_names(answer_member)
    c_names = member_qualified_names(catalog_member)
    if not a_names or not c_names:
        return False
    if a_names[-1] != c_names[-1]:
        return False
    common = min(len(a_names), len(c_names))
    return a_names[-common:] == c_names[-common:]
