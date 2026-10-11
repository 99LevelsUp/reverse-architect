"""Binding a block of text to a place in the code (KTD4).

A corpus document (or an agent's answer) is cut into sections (headings `##`,
`###`, `####`, ...) and blocks (a list item, a table row, a paragraph). Front
matter and fenced code (Mermaid included) are skipped. The anchors of
`corpus.parse_anchors` decide which place a block is about; this module
answers two questions:

* `citation_binds`: does the block that a citation quotes bind to the place?
  (for a document up to `PRESELECT_THRESHOLD_BYTES`, which the judge gets
  whole);
* `bound_blocks`: which blocks of the document bind to the place? (the
  preselection for a detail, a document without a level, and a document over
  the threshold; see `needs_preselection`).

The nearest anchor of a block, in this order (the first level that has an
anchor decides, a block never falls through to a farther anchor once a nearer
one exists, even when that one misses the place):

1. an anchor in the block itself;
2. for a nested list item, the anchor of the nearest parent item that has one;
3. an anchor in the heading of the block's section;
4. the last preceding anchored block of the same section.

A heading, even `###`, ends the reach of every earlier anchor: sections are
flat. The anchor of the document header (the first anchored block between the
`#` title and the first section, the one-line strip of a card) is used only for
blocks before the first heading and in sections that hold no anchor at all, and
not at all in a document of level `detail`. The header block itself is never
returned as a bound block: it carries no content of its own.

A document with no anchor at all (a module map) binds by names instead: a block
binds when it names the place's type or member as a whole word, with or without
qualification; a qualified `Type.Member` counts for the member only at the right
type, an unqualified member name counts for every type that has it. Link
destinations are not text. An agent's answer is read like a map: a block with a
cited anchor binds by it, a block without one by names (`parse_answer`).
"""
from __future__ import annotations

import re
from dataclasses import dataclass, replace
from typing import Iterable

from catalog import Place, member_qualified_names
from corpus import Anchor, iter_content_lines, parse_anchors
from report import ROLE_DEVELOPER, ROLE_SUPPORT

# A document over this many bytes is not given to the judge whole: its blocks
# are preselected by place instead. Part of the judge's prompt version.
PRESELECT_THRESHOLD_BYTES = 16 * 1024

VIA_ANCHOR = "kotva"
VIA_NAME = "jmeno"
LEVEL_CARD, LEVEL_DETAIL, LEVEL_MAP = "card", "detail", "map"
LEVEL_ANSWER = "odpoved"

_HEADING = re.compile(r"^(#{1,6})\s+(.*?)\s*#*\s*$")
_LIST_ITEM = re.compile(r"^(?P<indent>[ \t]*)(?:[-*+]|\d{1,3}[.)])[ \t]+(?P<body>.*)$")
_RULE = re.compile(r"^\s*(?:[-*_][ \t]*){3,}$")
_TABLE_SEPARATOR = re.compile(r"^\s*\|?[\s:|-]*-[\s:|-]*\|?\s*$")
_LINK_DESTINATION = re.compile(r"\]\([^)\n]*\)")


# --- model -------------------------------------------------------------------------

@dataclass(frozen=True)
class Block:
    """One block of a document: its text, section, anchors and place in the nesting."""

    index: int
    text: str
    section_id: int
    section: str
    anchors: tuple[Anchor, ...]
    parent: int | None
    is_header: bool = False


@dataclass(frozen=True)
class ParsedDocument:
    """A document cut into blocks, ready for the two questions of the module docstring."""

    doc_id: str
    title: str
    level: str | None
    size_bytes: int
    blocks: tuple[Block, ...]
    heading_anchors: tuple[tuple[Anchor, ...], ...]  # by section_id; [0] is the preamble's
    header_anchors: tuple[Anchor, ...]
    has_anchors: bool
    answer: bool = False
    headings: tuple[str, ...] = ()  # the heading text of every section by section_id; [0] is ""


@dataclass(frozen=True)
class BoundBlock:
    """A block bound to a place: its text, document, section, title and the anchor it binds by.

    `anchor` is the anchor's text as written (e.g. `src/A.cs:12-20`), or None
    when the block binds by name (`via` = `jmeno`).
    """

    text: str
    doc_id: str
    section: str
    title: str
    anchor: str | None
    via: str


def audiences(slots: Iterable[str]) -> tuple[str, ...]:
    """Whose question a fact answers, given the slots of the documents with a bound block.

    A fact of role `misto` whose bound block is only in a detail (or in a
    document without a level) is there for the developer; a block in a card or a
    map serves the support reader too, who does not read details. No bound
    block, no audience.
    """
    found = set(slots)
    if found & {"karta", "mapa"}:
        return (ROLE_DEVELOPER, ROLE_SUPPORT)
    if found:
        return (ROLE_DEVELOPER,)
    return ()


def needs_preselection(level: str | None, size_bytes: int) -> bool:
    """Whether a document goes through `bound_blocks` instead of whole to the judge.

    True for a detail, for a document without a level (or with an unknown one)
    and for any document over `PRESELECT_THRESHOLD_BYTES`; a card or a map
    under the threshold goes whole.
    """
    return level not in (LEVEL_CARD, LEVEL_MAP) or size_bytes > PRESELECT_THRESHOLD_BYTES


# --- parsing -----------------------------------------------------------------------

def _indent(text: str) -> int:
    return len(text.replace("\t", "    "))


class _Builder:
    def __init__(self, known_files: Iterable[str] | None, plain: bool) -> None:
        self.known = None if known_files is None else frozenset(known_files)
        self.plain = plain
        self.blocks: list[dict] = []
        self.section_id = 0
        self.section = ""
        self.heading_anchors: list[tuple[Anchor, ...]] = [()]
        self.headings: list[str] = [""]
        self.open: int | None = None   # block that a following plain line continues
        self.stack: list[tuple[int, int]] = []  # (indent, block index) of open list items

    def add(self, text: str, parent: int | None, open_after: bool) -> int:
        self.blocks.append({"text": text, "section_id": self.section_id,
                            "section": self.section, "parent": parent})
        index = len(self.blocks) - 1
        self.open = index if open_after else None
        return index

    def heading(self, text: str) -> None:
        self.section_id += 1
        self.section = text
        self.heading_anchors.append(parse_anchors(text, self.known, self.plain))
        self.headings.append(text)
        self.open = None
        self.stack.clear()

    def break_paragraph(self) -> None:
        self.open = None


def _blocks_of(text: str, known_files: Iterable[str] | None, plain: bool,
               sections: bool) -> _Builder:
    builder = _Builder(known_files, plain)
    previous = -2
    title_seen = False
    for number, line in iter_content_lines(text):
        if number != previous + 1:
            builder.break_paragraph()  # a skipped fence between two lines
        previous = number
        if not line.strip():
            builder.break_paragraph()
            continue
        if line.lstrip().startswith(">"):
            line = line.lstrip()[1:].lstrip()
            if not line:
                continue
        heading = _HEADING.match(line)
        if heading:
            level = len(heading.group(1))
            if level == 1 and not title_seen and builder.section_id == 0:
                title_seen = True
                builder.break_paragraph()
                continue
            if sections or level > 1:
                builder.heading(heading.group(2))
                continue
        if _RULE.match(line):
            builder.break_paragraph()
            continue
        item = _LIST_ITEM.match(line)
        if item:
            depth = _indent(item.group("indent"))
            while builder.stack and builder.stack[-1][0] >= depth:
                builder.stack.pop()
            parent = builder.stack[-1][1] if builder.stack else None
            index = builder.add(item.group("body").strip(), parent, open_after=True)
            builder.stack.append((depth, index))
            continue
        if line.lstrip().startswith("|"):
            if not _TABLE_SEPARATOR.match(line):
                builder.add(line.strip(), None, open_after=False)
            continue
        if builder.open is not None:
            block = builder.blocks[builder.open]
            block["text"] += "\n" + line.strip()
            continue
        if _indent(re.match(r"[ \t]*", line).group(0)) == 0:
            builder.stack.clear()
        builder.add(line.strip(), None, open_after=True)
    return builder


def parse_document(text: str, doc_id: str = "", title: str = "", level: str | None = None,
                   known_files: Iterable[str] | None = None) -> ParsedDocument:
    """Cut a corpus document into blocks and sections and read the anchors of each."""
    builder = _blocks_of(text, known_files, plain=False, sections=True)
    blocks: list[Block] = []
    for index, raw in enumerate(builder.blocks):
        blocks.append(Block(index=index, text=raw["text"], section_id=raw["section_id"],
                            section=raw["section"],
                            anchors=parse_anchors(raw["text"], builder.known),
                            parent=raw["parent"]))
    header_anchors: tuple[Anchor, ...] = ()
    for block in blocks:
        if block.section_id == 0 and block.anchors:
            header_anchors = block.anchors
            blocks[block.index] = replace(block, is_header=True)
            break
    has_anchors = any(b.anchors for b in blocks) or any(builder.heading_anchors)
    return ParsedDocument(doc_id=doc_id, title=title, level=level,
                          size_bytes=len(text.encode("utf-8")), blocks=tuple(blocks),
                          heading_anchors=tuple(builder.heading_anchors),
                          header_anchors=header_anchors, has_anchors=has_anchors,
                          headings=tuple(builder.headings))


def parse_answer(text: str, doc_id: str = "odpoved", title: str = "Odpověď agenta",
                 known_files: Iterable[str] | None = None) -> ParsedDocument:
    """An agent's answer as a document: blocks, no sections; unbackticked `path:N` is read too."""
    builder = _blocks_of(text, known_files, plain=True, sections=False)
    blocks = tuple(
        Block(index=i, text=raw["text"], section_id=0, section="",
              anchors=parse_anchors(raw["text"], builder.known, plain=True), parent=None)
        for i, raw in enumerate(builder.blocks))
    return ParsedDocument(doc_id=doc_id, title=title, level=LEVEL_ANSWER,
                          size_bytes=len(text.encode("utf-8")), blocks=blocks,
                          heading_anchors=((),), header_anchors=(),
                          has_anchors=any(b.anchors for b in blocks), answer=True)


# --- names -------------------------------------------------------------------------

def _place_names(place: Place) -> tuple[str | None, str | None]:
    """(type's simple name, member's simple name) of a place; either may be unknown."""
    if place.clen is not None:
        names = member_qualified_names(place.clen)
        return (names[-2] if len(names) > 1 else None), (names[-1] if names else None)
    names = member_qualified_names(place.typ or "")
    return (names[-1] if names else None), None


def _whole_word(name: str) -> re.Pattern[str]:
    return re.compile(rf"(?<![\w]){re.escape(name)}(?![\w])")


_QUALIFIER = re.compile(r"(\w+)(?:<[^<>]*>)?\.$")


def names_place(text: str, place: Place) -> bool:
    """Whether `text` names the place's type or member as a whole word (see the docstring)."""
    clean = _LINK_DESTINATION.sub("]", text)
    type_name, member = _place_names(place)
    if type_name and _whole_word(type_name).search(clean):
        return True
    if member:
        for found in _whole_word(member).finditer(clean):
            qualifier = _QUALIFIER.search(clean[:found.start()])
            if qualifier is None or type_name is None:
                return True
            name = qualifier.group(1)
            if not name[:1].isupper():
                return True  # an instance (`view.Check`), not a type: no qualification to check
            if name == type_name:
                return True
    return False


# --- binding -----------------------------------------------------------------------

def _decide(anchors: tuple[Anchor, ...], place: Place) -> tuple[bool, str | None]:
    """(binds?, raw text of the first anchor that covers the place)."""
    for anchor in anchors:
        if anchor.covers(place.soubor, place.radky):
            return True, anchor.raw
    return False, None


def _nearest(doc: ParsedDocument, block: Block) -> tuple[Anchor, ...] | None:
    """The anchors of the nearest anchor of `block` (levels 1-4 and the header), or None."""
    if block.anchors:
        return block.anchors
    parent = block.parent
    while parent is not None:
        if doc.blocks[parent].anchors:
            return doc.blocks[parent].anchors
        parent = doc.blocks[parent].parent
    if block.section_id > 0 and doc.heading_anchors[block.section_id]:
        return doc.heading_anchors[block.section_id]
    for before in range(block.index - 1, -1, -1):
        other = doc.blocks[before]
        if other.section_id != block.section_id:
            break
        if other.anchors and not other.is_header:
            return other.anchors
    section_has_anchor = bool(doc.heading_anchors[block.section_id]) or any(
        b.anchors and not b.is_header for b in doc.blocks if b.section_id == block.section_id)
    if doc.level != LEVEL_DETAIL and doc.header_anchors and \
            (block.section_id == 0 or not section_has_anchor):
        return doc.header_anchors
    return None


def _bind(doc: ParsedDocument, block: Block, place: Place) -> BoundBlock | None:
    def bound(anchor: str | None, via: str) -> BoundBlock:
        return BoundBlock(text=block.text, doc_id=doc.doc_id, section=block.section,
                          title=doc.title, anchor=anchor, via=via)

    if block.is_header:
        return None
    if doc.answer:
        if block.anchors:
            ok, raw = _decide(block.anchors, place)
            return bound(raw, VIA_ANCHOR) if ok else None
        return bound(None, VIA_NAME) if names_place(block.text, place) else None
    if not doc.has_anchors:
        return bound(None, VIA_NAME) if names_place(block.text, place) else None
    anchors = _nearest(doc, block)
    if anchors is None:
        return None
    ok, raw = _decide(anchors, place)
    return bound(raw, VIA_ANCHOR) if ok else None


def bound_blocks(doc: ParsedDocument, place: Place) -> list[BoundBlock]:
    """Every block of `doc` that binds to `place`, in document order."""
    out = (_bind(doc, block, place) for block in doc.blocks)
    return [b for b in out if b is not None]


# --- what the judge reads ------------------------------------------------------------

def judge_text(title: str, section: str, text: str) -> str:
    """A block as the judge reads it: a line `title > section` above the text (KTD4).

    Either part may be empty and is then left out; with both empty the text is unchanged.
    """
    context = " > ".join(part for part in (title, section) if part)
    return f"{context}\n{text}" if context else text


def whole_judge_texts(doc: ParsedDocument) -> list[str]:
    """The blocks of a document that goes to the judge whole, with the headings that open them.

    The title of the document opens its first block. The heading of a section opens the first
    block of that section, together with the headings of the sections before it that have no
    block of their own (`## Dynamic access`, `### Context`, then `#### caller: X` with the
    block): a heading line belongs to the block that follows it. So the judge reads the
    document with its title and every heading. An agent's answer has neither.
    """
    out: list[str] = []
    shown = 0  # the last section whose heading is already in a block
    for block in doc.blocks:
        section = ""
        if block.section_id > shown:
            headings = doc.headings[shown + 1:block.section_id + 1] or (block.section,)
            section = " > ".join(h for h in headings if h)
            shown = block.section_id
        title = "" if doc.answer or block.index != 0 else doc.title
        out.append(judge_text(title, section, block.text))
    return out


def _normalize(text: str) -> str:
    return " ".join(text.replace("`", "").split()).casefold()


def citation_blocks(doc: ParsedDocument, citation: str) -> list[Block]:
    """The blocks a citation quotes, in the order of preference.

    1. the blocks whose text contains it;
    2. the blocks whose text as the judge read it contains it (`whole_judge_texts`): a quotation
       of a heading or of the title lands on the block that follows the heading, one that runs
       from a heading into the text on the block it runs into;
    3. the same for a block read as a bound block, with `titul > sekce` above it;
    4. the blocks of at least 12 characters that the citation contains.
    """
    cited = _normalize(citation)
    if not cited:
        return []
    whole = [b for b in doc.blocks if cited in _normalize(b.text)]
    if whole:
        return whole
    read = [b for b, text in zip(doc.blocks, whole_judge_texts(doc)) if cited in _normalize(text)]
    if read:
        return read
    if not doc.answer:
        bound = [b for b in doc.blocks
                 if cited in _normalize(judge_text(doc.title, b.section, b.text))]
        if bound:
            return bound
    return [b for b in doc.blocks if len(_normalize(b.text)) >= 12 and _normalize(b.text) in cited]


def citation_binds(doc: ParsedDocument, place: Place, citation: str) -> BoundBlock | None:
    """The bound block that `citation` quotes, or None if no quoted block binds to the place."""
    for block in citation_blocks(doc, citation):
        found = _bind(doc, block, place)
        if found is not None:
            return found
    return None
