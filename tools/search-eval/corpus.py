"""Reading an RA corpus for the card check: anchors, documents of a place (KTD2, KTD3).

No agent and no model is involved. For a place in the code (file, declaring
member or type, lines; `catalog.Place`) the module finds the documents of the
corpus that own the place or anchor it, so the card check can tell whether a
fact's place is covered by a card, a detail, a map or another document.

Corpus format (read from RA's own writers, `src/manifest.rs` and
`src/card/documents.rs`):

* the manifest is either one `manifest.json` or, above RA's size threshold,
  `manifest-strings.json` (the `strings` table and `schema_version`),
  `manifest-nodes.jsonl` and `manifest-documents.jsonl`. Both shapes are read
  to the same result; the line files are streamed, never loaded whole;
* a node's `key_id` and a fact's `provenance.file_id` index `strings`; a fact's
  `provenance.line` / `end_line` give its lines (no `end_line`: one line);
* documents have `id` (`sym:<lang>:<ns>#<Type>` for a type card, `detail:<id>`,
  `map:<module>`), `kind`, `level` (`card` / `detail` / `map`), `parent`,
  `title`, `path` and `header.path`;
* `coverage.json` carries `repo_revision`.

Place -> documents, two ways (KTD3):

* ownership: the member node whose lines enclose the place (innermost) gives
  the type card (the longest dotted prefix of its key that is a `card:type`
  document), that card's `detail:<id>` and its parent map;
* anchors: every backticked anchor of every document (grammar below) whose file
  matches and whose lines overlap the place; `header.path` is not used, one file
  may hold several types.

Anchor grammar (`parse_anchors`), anchors are in backticks: `path`,
`path:N`, `path:N-M`, `path:N:C` (the column is ignored), `path:N, M` (a list
of lines, each entry may be `N-M` or `N:C`). A bare `path` covers the whole file.

Index (KTD2): built once per corpus from the manifest and the documents and
stored in the state directory; the cache key is the name, size and
modification time (`os.stat`) of the manifest files and of `coverage.json`,
never their content. Identity (the revision of the sources) comes from the
runner's receipt when one lies beside the corpus (`<corpus>.receipt.json`,
it carries the tree fingerprint) and otherwise from `coverage.json`
(`repo_revision`, labelled "identita z coverage.json").
"""
from __future__ import annotations

import hashlib
import json
import os
import re
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Iterable, Iterator

from catalog import ROLE_FRAMEWORK, ROLE_PLACE, Place, member_qualified_names
from prepare import receipt_path
from report import write_text_atomic

INDEX_FORMAT_VERSION = 2  # 2: no `missing_documents`; an unreadable document stops the build

MANIFEST_SINGLE = "manifest.json"
MANIFEST_STRINGS = "manifest-strings.json"
MANIFEST_NODES = "manifest-nodes.jsonl"
MANIFEST_DOCUMENTS = "manifest-documents.jsonl"
COVERAGE = "coverage.json"

SLOT_CARD = "karta"
SLOT_DETAIL = "detail"
SLOT_MAP = "mapa"
SLOT_OTHER = "jina"
_LEVEL_SLOTS = {"card": SLOT_CARD, "detail": SLOT_DETAIL, "map": SLOT_MAP}
# Documents that count for a fact of role `ramcova`.
FRAMEWORK_SLOTS = (SLOT_CARD, SLOT_MAP)

VIA_OWNERSHIP = "vlastnictví"
VIA_ANCHOR = "kotva"

IDENTITY_FROM_COVERAGE = "identita z coverage.json"
IDENTITY_FROM_RECEIPT = "účtenka běžeče"


class CorpusError(ValueError):
    """The corpus cannot be read or does not match the item (Czech message)."""


# --- anchors -------------------------------------------------------------------

@dataclass(frozen=True)
class Anchor:
    """One anchored span of a file; `start` and `end` are None for the whole file."""

    file: str
    start: int | None
    end: int | None
    raw: str

    def covers(self, soubor: str, radky: tuple[int, int]) -> bool:
        """Whether the anchor is in `soubor` and its lines overlap `radky`."""
        if self.file != soubor:
            return False
        if self.start is None or self.end is None:
            return True
        return self.start <= radky[1] and self.end >= radky[0]


_BACKTICKED = re.compile(r"`([^`\n]+)`")
_ENTRY = r"\d+(?:-\d+)?(?::\d+)?"
_ANCHOR = re.compile(
    rf"^(?P<path>[^\s:`,]+)(?::(?P<spec>{_ENTRY}(?:,\s*{_ENTRY})*))?$")
_PLAIN_ANCHOR = re.compile(
    rf"(?<![\w./\\`-])(?P<path>[\w./\\-]*[\w-]\.[A-Za-z][A-Za-z0-9]{{0,5}})"
    rf":(?P<spec>{_ENTRY}(?:,\s*{_ENTRY})*)")
_SOURCE_EXTENSIONS = frozenset({
    "c", "cc", "cpp", "cs", "cshtml", "csproj", "css", "go", "h", "hpp", "html", "java",
    "js", "json", "jsx", "kt", "md", "php", "ps1", "py", "razor", "rb", "rs", "scala", "sh",
    "sql", "swift", "toml", "ts", "tsx", "vb", "xaml", "xml", "yaml", "yml"})


def normalize_anchor_path(path: str) -> str:
    """`\\` to `/`, no leading `./`."""
    text = path.replace("\\", "/").strip()
    while text.startswith("./"):
        text = text[2:]
    return text


def _looks_like_path(path: str, known_files: Iterable[str] | None) -> bool:
    if known_files is not None and path in known_files:
        return True
    if "/" in path:
        return True
    stem, dot, ext = path.rpartition(".")
    return bool(dot and stem and ext.lower() in _SOURCE_EXTENSIONS)


def _spans(spec: str | None) -> list[tuple[int, int] | None]:
    """The spans of an anchor spec; `[None]` for a bare path (whole file); [] if invalid."""
    if spec is None:
        return [None]
    out: list[tuple[int, int] | None] = []
    for entry in spec.split(","):
        lines = entry.strip().split(":")[0]
        first, _, last = lines.partition("-")
        start = int(first)
        end = int(last) if last else start
        if start < 1 or end < start:
            return []
        out.append((start, end))
    return out


def _anchor_candidates(path: str, spec: str | None, raw: str,
                       known_files: Iterable[str] | None) -> list[Anchor]:
    path = normalize_anchor_path(path)
    if not path or not _looks_like_path(path, known_files):
        return []
    if spec is None and known_files is not None and path not in known_files:
        # A bare path names a whole file only when the corpus knows that file;
        # otherwise it is any code span that merely looks like a name.
        return []
    return [Anchor(file=path, start=span[0] if span else None, end=span[1] if span else None,
                   raw=raw)
            for span in _spans(spec)]


def parse_anchors(text: str, known_files: Iterable[str] | None = None,
                  plain: bool = False) -> tuple[Anchor, ...]:
    """Every anchor in `text`, in order; a list of lines gives one `Anchor` per entry.

    Anchors are backticked (grammar in the module docstring). `known_files`
    (a set of corpus-relative paths) decides about a bare `path`: with it a bare
    path is an anchor only if the file is known, without it only if it has a
    `/` or a source-file extension. `plain=True` also reads unbackticked
    `path:N` spellings (for an agent's answer, which does not always keep to
    the backticks).
    """
    known = known_files if known_files is None or isinstance(known_files, (set, frozenset)) \
        else frozenset(known_files)
    out: list[Anchor] = []
    for match in _BACKTICKED.finditer(text):
        inner = match.group(1).strip()
        parsed = _ANCHOR.match(inner)
        if parsed:
            out.extend(_anchor_candidates(parsed.group("path"), parsed.group("spec"), inner, known))
    if plain:
        stripped = _BACKTICKED.sub(" ", text)
        for match in _PLAIN_ANCHOR.finditer(stripped):
            raw = match.group(0)
            out.extend(_anchor_candidates(match.group("path"), match.group("spec"), raw, known))
    return tuple(out)


def iter_content_lines(text: str) -> Iterator[tuple[int, str]]:
    """`(line number, line)` of `text` outside the front matter and outside fenced code.

    Line numbers are 0-based indexes into `text.splitlines()`.
    """
    lines = text.splitlines()
    start = 0
    if lines and lines[0].strip() == "---":
        for i in range(1, len(lines)):
            if lines[i].strip() == "---":
                start = i + 1
                break
    fence: str | None = None
    for i in range(start, len(lines)):
        line = lines[i]
        marker = line.lstrip()[:3]
        if marker in ("```", "~~~"):
            if fence is None:
                fence = marker
            elif marker == fence:
                fence = None
            continue
        if fence is None:
            yield i, line


# --- streaming -------------------------------------------------------------------

def iter_jsonl(path: Path) -> Iterator[Any]:
    """The records of a `.jsonl` file, one line at a time (the file is never read whole)."""
    with open(path, "r", encoding="utf-8") as handle:
        for number, line in enumerate(handle, start=1):
            if not line.strip():
                continue
            try:
                yield json.loads(line)
            except json.JSONDecodeError as exc:
                raise CorpusError(f"{path.name}, řádek {number}: neplatný JSON ({exc}).") from exc


def _read_json(path: Path) -> Any:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise CorpusError(f"Soubor {path} nejde načíst: {exc}") from exc


# --- identity ----------------------------------------------------------------------

@dataclass(frozen=True)
class CorpusIdentity:
    """Which revision of the sources a corpus was made from, and how that is known."""

    revision: str
    source: str  # IDENTITY_FROM_RECEIPT | IDENTITY_FROM_COVERAGE
    tree_fingerprint: str | None = None
    corpus_fingerprint: str | None = None  # of the corpus's files; the receipt has it

    @property
    def label(self) -> str:
        if self.source == IDENTITY_FROM_RECEIPT:
            return f"{IDENTITY_FROM_RECEIPT} (otisk stromu {self.tree_fingerprint})"
        return IDENTITY_FROM_COVERAGE


def same_revision(a: str, b: str) -> bool:
    """Equal shas; an abbreviated one (7+ characters) matches the full one it prefixes."""
    a, b = a.strip().lower(), b.strip().lower()
    if not a or not b:
        return False
    short, long = sorted((a, b), key=len)
    return len(short) >= 7 and long.startswith(short)


def corpus_identity(corpus: Path, receipt: Path | None = None) -> CorpusIdentity:
    """The corpus's identity: the runner's receipt if there is one, else `coverage.json`.

    The receipt is looked for beside the corpus (`prepare.receipt_path`) unless
    `receipt` names it. It is not re-verified here (the fingerprint of the
    whole corpus is the runner's job); it only supplies the revision and the
    tree fingerprint.
    """
    receipt_file = receipt if receipt is not None else receipt_path(corpus)
    if receipt_file.is_file():
        data = _read_json(receipt_file)
        revision = data.get("base_revision") if isinstance(data, dict) else None
        if not isinstance(revision, str) or not revision.strip():
            raise CorpusError(f"Účtenka {receipt_file} nemá „base_revision“.")
        fingerprint = data.get("tree_fingerprint")
        corpus_fingerprint = data.get("corpus_fingerprint")
        return CorpusIdentity(revision=revision, source=IDENTITY_FROM_RECEIPT,
                              tree_fingerprint=fingerprint if isinstance(fingerprint, str)
                              else None,
                              corpus_fingerprint=corpus_fingerprint
                              if isinstance(corpus_fingerprint, str) else None)
    coverage_file = corpus / COVERAGE
    if not coverage_file.is_file():
        raise CorpusError(f"Korpus {corpus} nemá {COVERAGE} ani účtenku; revize zdrojáků "
                          "není známa, korpus se nepoužije.")
    data = _read_json(coverage_file)
    revision = data.get("repo_revision") if isinstance(data, dict) else None
    if not isinstance(revision, str) or not revision.strip():
        raise CorpusError(f"{coverage_file} nemá „repo_revision“; revize zdrojáků není známa, "
                          "korpus se nepoužije.")
    return CorpusIdentity(revision=revision, source=IDENTITY_FROM_COVERAGE)


def check_commit(identity: CorpusIdentity, commit: str) -> None:
    """Refuse a corpus whose revision is not the commit of the item (message names both)."""
    if not same_revision(identity.revision, commit):
        raise CorpusError(
            f"Korpus vznikl z revize {identity.revision} ({identity.label}), ale položka "
            f"katalogu je nad commitem {commit}; kontrola karet by měřila jiné zdrojáky.")


# --- documents and the index -----------------------------------------------------------

@dataclass(frozen=True)
class CorpusDoc:
    """A document of the corpus as the manifest describes it."""

    id: str
    kind: str | None
    level: str | None
    parent: str | None
    title: str
    path: str
    header_path: str | None

    @property
    def slot(self) -> str:
        """`karta` / `detail` / `mapa` by `level`; any other (or no) level is `jina`."""
        return _LEVEL_SLOTS.get(self.level or "", SLOT_OTHER)


@dataclass(frozen=True)
class DocHit:
    """A document found for a place, how it was found, and the anchor if it was one."""

    doc: CorpusDoc
    via: str  # VIA_OWNERSHIP | VIA_ANCHOR
    anchor: Anchor | None = None

    @property
    def slot(self) -> str:
        return self.doc.slot


@dataclass(frozen=True)
class PlaceDocuments:
    """What the corpus holds for one place.

    `found` is False when the place could not be verified against the corpus
    (`reason` says why, in Czech): the card check then reports "místo nenalezeno
    v korpusu" and judges nothing. `owner_card` is the id of the card that owns
    the place by ownership; `hits` lists the owner card, its detail and map
    (`via` = ownership) and then every other document that anchors the place
    (`via` = anchor).
    """

    place: Place
    found: bool
    reason: str | None
    owner_card: str | None
    hits: tuple[DocHit, ...]

    def for_role(self, role: str) -> tuple[DocHit, ...]:
        """Hits that count for a fact of `role`: `ramcova` takes cards and maps, `misto` all."""
        if role == ROLE_FRAMEWORK:
            return tuple(h for h in self.hits if h.slot in FRAMEWORK_SLOTS)
        if role == ROLE_PLACE:
            return self.hits
        raise ValueError(f"neznámá role {role!r}")

    def by_slot(self, slot: str) -> tuple[DocHit, ...]:
        return tuple(h for h in self.hits if h.slot == slot)


def split_key(key: str) -> tuple[str, list[str]]:
    """`sym:<lang>:<ns>#<Type>.<Member>(…)` -> (`sym:<lang>:<ns>`, dotted segments).

    The parameter list is dropped and dots inside generic arguments do not
    split.
    """
    prefix, _, rest = key.partition("#")
    rest = rest.split("(", 1)[0]
    segments: list[str] = []
    depth = 0
    current: list[str] = []
    for ch in rest:
        if ch == "<":
            depth += 1
        elif ch == ">":
            depth = max(0, depth - 1)
        if ch == "." and depth == 0:
            segments.append("".join(current))
            current = []
        else:
            current.append(ch)
    segments.append("".join(current))
    return prefix, [s for s in segments if s]


def _strip_generics(name: str) -> str:
    out, depth = [], 0
    for ch in name:
        if ch == "<":
            depth += 1
        elif ch == ">":
            depth = max(0, depth - 1)
        elif depth == 0:
            out.append(ch)
    return "".join(out)


def _manifest_files(corpus: Path) -> list[Path]:
    """The manifest files that exist, in a fixed order (the cache key's subjects)."""
    names = (MANIFEST_SINGLE, MANIFEST_STRINGS, MANIFEST_NODES, MANIFEST_DOCUMENTS, COVERAGE)
    return [corpus / n for n in names if (corpus / n).is_file()]


def _stat_key(corpus: Path) -> list[list[Any]]:
    out = []
    for path in _manifest_files(corpus):
        st = path.stat()
        out.append([path.name, st.st_size, st.st_mtime_ns])
    return out


def index_path(corpus: Path, state_dir: Path) -> Path:
    digest = hashlib.sha256(os.path.realpath(corpus).encode("utf-8")).hexdigest()[:16]
    return Path(state_dir) / f"corpus-index-{digest}.json"


class CorpusIndex:
    """The place index of one corpus: documents, member nodes by file, anchors by file."""

    def __init__(self, corpus: Path, identity: CorpusIdentity, data: dict[str, Any],
                 from_cache: bool) -> None:
        self.corpus = corpus
        self.identity = identity
        self.from_cache = from_cache
        self.stat = data["stat"]
        self.docs: list[CorpusDoc] = [
            CorpusDoc(id=d[0], kind=d[1], level=d[2], parent=d[3], title=d[4], path=d[5],
                      header_path=d[6]) for d in data["documents"]]
        self._doc_by_id = {d.id: d for d in self.docs}
        self._keys: list[str] = data["keys"]
        self._nodes: dict[str, list[tuple[int, int, int]]] = {
            f: [(s, e, k) for s, e, k in nodes] for f, nodes in data["nodes"].items()}
        self.files: frozenset[str] = frozenset(data["files"])
        self._anchors: dict[str, list[tuple[int | None, int | None, int, str]]] = {
            f: [(a[0], a[1], a[2], a[3]) for a in rows] for f, rows in data["anchors"].items()}
        self._card_ids = {d.id for d in self.docs if d.kind == "card:type"}

    def document(self, doc_id: str) -> CorpusDoc | None:
        return self._doc_by_id.get(doc_id)

    def read_document(self, doc: CorpusDoc) -> str:
        """The text of a document's file (UTF-8)."""
        try:
            return (self.corpus / doc.path).read_text(encoding="utf-8", errors="replace")
        except OSError as exc:
            raise CorpusError(f"Dokument {doc.id}: soubor {doc.path} nejde načíst: {exc}") from exc

    # -- ownership --

    def _card_of(self, key: str) -> str | None:
        prefix, segments = split_key(key)
        for n in range(len(segments), 0, -1):
            candidate = f"{prefix}#{'.'.join(segments[:n])}"
            if candidate in self._card_ids:
                return candidate
        return None

    def _matching_nodes(self, place: Place) -> tuple[str | None, list[tuple[int, int, int]]]:
        """Nodes of the place's file named by `clen` / `typ`, and a reason if there are none."""
        nodes = self._nodes.get(place.soubor)
        if nodes is None:
            if place.soubor in self.files:
                return f"soubor {place.soubor} v korpusu nemá žádný uzel deklarace", []
            return f"soubor {place.soubor} v korpusu není", []
        wanted = [_strip_generics(n) for n in
                  member_qualified_names(place.clen if place.clen is not None
                                         else place.typ or "")]
        named = []
        for start, end, key_index in nodes:
            key = self._keys[key_index]
            _, segments = split_key(key)
            segments = [_strip_generics(s) for s in segments]
            if segments[-len(wanted):] != wanted:
                continue
            if place.clen is None and key not in self._card_ids:
                continue
            named.append((start, end, key_index))
        kind = "člen" if place.clen is not None else "typ"
        if not named:
            return f"{kind} „{place.clen or place.typ}“ v souboru {place.soubor} není v korpusu", []
        return None, named

    def locate(self, place: Place) -> PlaceDocuments:
        """The documents of `place`: by ownership and by anchors (KTD3), after verifying it."""
        reason, named = self._matching_nodes(place)
        first, last = place.radky
        enclosing = [n for n in named if n[0] <= first and last <= n[1]]
        if reason is None and not enclosing:
            what = place.clen if place.clen is not None else place.typ
            reason = (f"řádky {first}-{last} neleží v uzlu „{what}“ v souboru {place.soubor} "
                      f"(uzel má řádky {', '.join(f'{s}-{e}' for s, e, _ in named)})")
        if reason is not None:
            return PlaceDocuments(place=place, found=False,
                                  reason=f"místo nenalezeno v korpusu: {reason}",
                                  owner_card=None, hits=())
        start, end, key_index = min(enclosing, key=lambda n: (n[1] - n[0], n[0]))
        owner = self._card_of(self._keys[key_index])
        hits: list[DocHit] = []
        seen: set[str] = set()
        if owner is not None:
            card = self._doc_by_id[owner]
            chain = [card]
            for related in (self._doc_by_id.get(f"detail:{owner}"),
                            self._doc_by_id.get(card.parent) if card.parent else None):
                if related is not None:
                    chain.append(related)
            for doc in chain:
                seen.add(doc.id)
                hits.append(DocHit(doc=doc, via=VIA_OWNERSHIP))
        for a_start, a_end, doc_index, raw in self._anchors.get(place.soubor, ()):
            doc = self.docs[doc_index]
            if doc.id in seen:
                continue
            anchor = Anchor(file=place.soubor, start=a_start, end=a_end, raw=raw)
            if anchor.covers(place.soubor, place.radky):
                seen.add(doc.id)
                hits.append(DocHit(doc=doc, via=VIA_ANCHOR, anchor=anchor))
        return PlaceDocuments(place=place, found=True, reason=None, owner_card=owner,
                              hits=tuple(hits))


def _load_strings_and_nodes(corpus: Path) -> tuple[list[str], Iterator[Any], Iterator[Any]]:
    """The strings table, a node iterator and a document iterator, for either manifest shape."""
    if (corpus / MANIFEST_STRINGS).is_file():
        header = _read_json(corpus / MANIFEST_STRINGS)
        strings = header.get("strings") if isinstance(header, dict) else None
        if not isinstance(strings, list):
            raise CorpusError(f"{MANIFEST_STRINGS}: chybí tabulka „strings“.")
        nodes_file, docs_file = corpus / MANIFEST_NODES, corpus / MANIFEST_DOCUMENTS
        if not nodes_file.is_file() or not docs_file.is_file():
            raise CorpusError(f"Manifest po řádcích je neúplný: chybí {MANIFEST_NODES} nebo "
                              f"{MANIFEST_DOCUMENTS}.")
        return strings, iter_jsonl(nodes_file), iter_jsonl(docs_file)
    if (corpus / MANIFEST_SINGLE).is_file():
        # One file is below RA's size threshold, so it is read whole.
        manifest = _read_json(corpus / MANIFEST_SINGLE)
        if not isinstance(manifest, dict) or not isinstance(manifest.get("strings"), list):
            raise CorpusError(f"{MANIFEST_SINGLE}: chybí tabulka „strings“.")
        return (manifest["strings"], iter(manifest.get("nodes") or []),
                iter(manifest.get("documents") or []))
    raise CorpusError(f"Korpus {corpus} nemá manifest ({MANIFEST_SINGLE} ani "
                      f"{MANIFEST_STRINGS}).")


def _string(strings: list[str], index: Any, where: str) -> str:
    if not isinstance(index, int) or isinstance(index, bool) or not 0 <= index < len(strings):
        raise CorpusError(f"Manifest: {where} odkazuje mimo tabulku „strings“ ({index!r}).")
    return strings[index]


def _node_spans(strings: list[str], record: dict[str, Any]) -> dict[str, list[int]]:
    """`{file: [first line, last line]}` of a node's declaration.

    The `symbol_*` facts carry the declaration's lines; a node without one falls
    back to every fact that has a line.
    """
    facts = record.get("facts") or []
    declaring = [f for f in facts if str(f.get("kind", "")).startswith("symbol_")]
    spans: dict[str, list[int]] = {}
    for fact in declaring or facts:
        prov = fact.get("provenance") or {}
        line = prov.get("line")
        if not isinstance(line, int) or "file_id" not in prov:
            continue
        end = prov.get("end_line")
        end = end if isinstance(end, int) and end >= line else line
        file = normalize_anchor_path(_string(strings, prov["file_id"], "provenance.file_id"))
        span = spans.setdefault(file, [line, end])
        span[0], span[1] = min(span[0], line), max(span[1], end)
    return spans


def build_index(corpus: Path, identity: CorpusIdentity,
                stat: list[list[Any]]) -> dict[str, Any]:
    """Stream the manifest and the documents once and return the index data."""
    strings, nodes, documents = _load_strings_and_nodes(corpus)

    key_ids: dict[str, int] = {}
    keys: list[str] = []
    by_file: dict[str, list[list[int]]] = {}
    for record in nodes:
        key = _string(strings, record.get("key_id"), "key_id")
        if not key.startswith("sym:") or "#" not in key:
            continue
        for file, (start, end) in _node_spans(strings, record).items():
            index = key_ids.get(key)
            if index is None:
                index = key_ids[key] = len(keys)
                keys.append(key)
            by_file.setdefault(file, []).append([start, end, index])
    for rows in by_file.values():
        rows.sort()

    files = set(by_file)
    coverage_file = corpus / COVERAGE
    if coverage_file.is_file():
        coverage = _read_json(coverage_file)
        for entry in (coverage.get("files") or []) if isinstance(coverage, dict) else []:
            if isinstance(entry, dict) and isinstance(entry.get("path"), str):
                files.add(normalize_anchor_path(entry["path"]))

    docs: list[list[Any]] = []
    for record in documents:
        header = record.get("header") if isinstance(record.get("header"), dict) else {}
        docs.append([record.get("id"), record.get("kind"), record.get("level"),
                     record.get("parent"), record.get("title") or "", record.get("path") or "",
                     header.get("path")])
        if not isinstance(docs[-1][0], str) or not isinstance(docs[-1][5], str):
            raise CorpusError("Manifest: dokument bez „id“ nebo „path“.")

    anchors: dict[str, list[list[Any]]] = {}
    for doc_index, doc in enumerate(docs):
        path = corpus / doc[5]
        try:
            text = path.read_text(encoding="utf-8", errors="replace")
        except OSError as exc:
            # An index built without a document would be cached and silently lack its anchors.
            raise CorpusError(f"Dokument {doc[0]}: soubor {doc[5]} nejde načíst: {exc}") from exc
        found: set[tuple[str, int | None, int | None, str]] = set()
        for _, line in iter_content_lines(text):
            if "`" not in line:
                continue
            for anchor in parse_anchors(line, known_files=files):
                found.add((anchor.file, anchor.start, anchor.end, anchor.raw))
        for file, start, end, raw in sorted(found, key=lambda a: (a[0], a[1] or 0, a[2] or 0,
                                                                  a[3])):
            anchors.setdefault(file, []).append([start, end, doc_index, raw])

    return {"format_version": INDEX_FORMAT_VERSION, "identity": {
                "revision": identity.revision, "source": identity.source,
                "tree_fingerprint": identity.tree_fingerprint},
            "stat": stat, "documents": docs, "keys": keys,
            "nodes": by_file, "files": sorted(files), "anchors": anchors}


def open_corpus(corpus_dir: str | os.PathLike[str], state_dir: str | os.PathLike[str],
                commit: str | None = None, receipt: str | os.PathLike[str] | None = None,
                ) -> CorpusIndex:
    """Open a corpus for the card check; build its index once and keep it in `state_dir`.

    With `commit` (the commit of the catalog item) the corpus is refused unless
    its revision matches (KTD2). A stored index is used when the name, size and
    modification time of the manifest files and of `coverage.json` are the same;
    a changed one rebuilds it.
    """
    corpus = Path(corpus_dir)
    if not corpus.is_dir():
        raise CorpusError(f"Korpus {corpus} není adresář.")
    identity = corpus_identity(corpus, Path(receipt) if receipt is not None else None)
    if commit is not None:
        check_commit(identity, commit)
    stat = _stat_key(corpus)
    target = index_path(corpus, Path(state_dir))
    cached = _load_cached(target, stat, identity)
    if cached is not None:
        return CorpusIndex(corpus, identity, cached, from_cache=True)
    data = build_index(corpus, identity, stat)
    write_text_atomic(target, json.dumps(data, ensure_ascii=False, separators=(",", ":")) + "\n")
    return CorpusIndex(corpus, identity, data, from_cache=False)


def _load_cached(target: Path, stat: list[list[Any]],
                 identity: CorpusIdentity) -> dict[str, Any] | None:
    try:
        data = json.loads(target.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return None
    if not isinstance(data, dict) or data.get("format_version") != INDEX_FORMAT_VERSION:
        return None
    if data.get("stat") != stat or data.get("identity", {}).get("revision") != identity.revision:
        return None
    if any(name not in data for name in ("documents", "keys", "nodes", "files", "anchors")):
        return None
    return data
