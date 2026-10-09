"""Reading the corpus for the card check (U2: KTD2, KTD3).

Fixtures: `fixtures/corpus_l1` (layer 1, manifest by lines, strings table) and
`fixtures/corpus_l2` (layer 2, manifest in one file, triggers index).
"""
import inspect
import json
import os
import shutil
import tempfile
import tracemalloc
import unittest
from pathlib import Path
from unittest import mock

import corpus
from catalog import Place
from corpus import (CorpusError, open_corpus, parse_anchors)

FIXTURES = Path(__file__).resolve().parent / "fixtures"
SHA = "a" * 40
EV = "src/Acme/Views/EditorView.cs"
TWO = "src/Acme/Views/TwoViews.cs"
NS = "sym:csharp:Acme.Views#"


def member(soubor, clen, lines, referenced=None):
    return Place(soubor=soubor, clen=clen, typ=None, radky=lines, odkazovany_clen=referenced)


def type_place(soubor, typ, lines):
    return Place(soubor=soubor, clen=None, typ=typ, radky=lines, odkazovany_clen=None)


def ids(hits):
    return [h.doc.id for h in hits]


class CorpusTestCase(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.tmp = Path(tmp.name)
        self.state = self.tmp / "state"

    def copy_corpus(self, name):
        target = self.tmp / name
        shutil.copytree(FIXTURES / name, target)
        return target

    def open(self, name="corpus_l1", **kwargs):
        return open_corpus(FIXTURES / name, self.state, **kwargs)


class AnchorGrammarTest(unittest.TestCase):
    def spans(self, text, **kwargs):
        return [(a.file, a.start, a.end) for a in parse_anchors(text, **kwargs)]

    def test_forms(self):
        self.assertEqual(self.spans("`src/a.cs:12`"), [("src/a.cs", 12, 12)])
        self.assertEqual(self.spans("`src/a.cs:12-20`"), [("src/a.cs", 12, 20)])
        self.assertEqual(self.spans("`src/a.cs`"), [("src/a.cs", None, None)])
        self.assertEqual(self.spans("`a.cs:3`, `b.cs:7`"), [("a.cs", 3, 3), ("b.cs", 7, 7)])

    def test_column_is_ignored_and_a_list_of_lines_gives_every_range(self):
        self.assertEqual(self.spans("`Typ.cs:26:35`"), [("Typ.cs", 26, 26)])
        self.assertEqual(self.spans("`Typ.cs:12, 30`"), [("Typ.cs", 12, 12), ("Typ.cs", 30, 30)])
        self.assertEqual(self.spans("`Typ.cs:12:3, 30-33, 40:7`"),
                         [("Typ.cs", 12, 12), ("Typ.cs", 30, 33), ("Typ.cs", 40, 40)])

    def test_what_is_not_an_anchor(self):
        for text in ("`sym:csharp:Acme.Views#EditorView`", "`public void Check(bool flag)`",
                     "`Title`", "`Acme.Views`", "`a.cs:0`", "`a.cs:9-3`", "src/a.cs:12",
                     "`https://example.com/a.cs:12`"):
            self.assertEqual(self.spans(text), [], text)

    def test_bare_path_needs_a_known_file_when_the_corpus_is_known(self):
        self.assertEqual(self.spans("`Makefile`"), [])
        self.assertEqual(self.spans("`Makefile`", known_files={"Makefile"}),
                         [("Makefile", None, None)])
        self.assertEqual(self.spans("`other/x.cs`", known_files={"a.cs"}), [])
        self.assertEqual(self.spans("`other/x.cs:3`", known_files={"a.cs"}),
                         [("other/x.cs", 3, 3)])

    def test_plain_anchors_only_on_request(self):
        self.assertEqual(self.spans("viz Typ.cs:1069 a `b.cs:2`"), [("b.cs", 2, 2)])
        self.assertEqual(self.spans("viz Typ.cs:1069 a `b.cs:2`", plain=True),
                         [("b.cs", 2, 2), ("Typ.cs", 1069, 1069)])

    def test_covers_is_an_overlap_of_lines_in_the_same_file(self):
        anchor = parse_anchors("`a.cs:10-20`")[0]
        self.assertTrue(anchor.covers("a.cs", (20, 25)))
        self.assertTrue(anchor.covers("a.cs", (1, 10)))
        self.assertFalse(anchor.covers("a.cs", (21, 25)))
        self.assertFalse(anchor.covers("b.cs", (10, 20)))
        self.assertTrue(parse_anchors("`a.cs`")[0].covers("a.cs", (999, 999)))

    def test_fenced_code_and_front_matter_are_skipped(self):
        text = "---\nx: `a.cs:1`\n---\n# T\n`a.cs:2`\n```mermaid\n`a.cs:3`\n```\n`a.cs:4`\n"
        lines = [line for _, line in corpus.iter_content_lines(text)]
        self.assertEqual(lines, ["# T", "`a.cs:2`", "`a.cs:4`"])


class OwnershipTest(CorpusTestCase):
    def test_member_gives_card_detail_and_map(self):
        index = self.open()
        result = index.locate(member(EV, "EditorView.Check(bool)", (12, 16)))
        self.assertTrue(result.found)
        self.assertEqual(result.owner_card, NS + "EditorView")
        owned = [h for h in result.hits if h.via == corpus.VIA_OWNERSHIP]
        self.assertEqual(ids(owned), [NS + "EditorView", "detail:" + NS + "EditorView",
                                      "map:src/Acme"])
        self.assertEqual([h.slot for h in owned], ["karta", "detail", "mapa"])

    def test_write_of_a_property_inside_a_method_belongs_to_the_methods_type(self):
        result = self.open().locate(member(EV, "EditorView.Check(bool)", (15, 15), "Title"))
        self.assertTrue(result.found)
        self.assertEqual(result.owner_card, NS + "EditorView")

    def test_a_field_with_only_a_line_covers_that_line_only(self):
        index = self.open()
        self.assertTrue(index.locate(member(EV, "EditorView.Title", (6, 6))).found)
        result = index.locate(member(EV, "EditorView.Title", (6, 7)))
        self.assertFalse(result.found)
        self.assertTrue(result.reason.startswith("místo nenalezeno v korpusu"))
        self.assertEqual(result.hits, ())

    def test_nested_type_gives_its_own_card_not_the_outer_one(self):
        index = self.open()
        field = index.locate(member(EV, "EditorView.Inner.Count", (50, 50)))
        self.assertEqual(field.owner_card, NS + "EditorView.Inner")
        owned = [h.doc.id for h in field.hits if h.via == corpus.VIA_OWNERSHIP]
        self.assertNotIn(NS + "EditorView", owned)
        self.assertIn("detail:" + NS + "EditorView.Inner", owned)
        nested = index.locate(type_place(EV, "EditorView.Inner", (45, 45)))
        self.assertEqual(nested.owner_card, NS + "EditorView.Inner")

    def test_place_of_a_type_takes_the_card_directly(self):
        result = self.open().locate(type_place(EV, "EditorView", (4, 4)))
        self.assertTrue(result.found)
        self.assertEqual(result.owner_card, NS + "EditorView")

    def test_two_types_in_one_file(self):
        index = self.open()
        second = index.locate(member(TWO, "ViewB.Check(bool)", (28, 28)))
        self.assertEqual(second.owner_card, NS + "ViewB")
        first = index.locate(member(TWO, "ViewA.Check(bool)", (10, 10)))
        self.assertEqual(first.owner_card, NS + "ViewA")
        self.assertNotIn(NS + "ViewA", ids(second.hits))

    def test_innermost_node_wins_and_a_card_without_detail_has_none(self):
        result = self.open().locate(member("src/Acme/Views/EditorViewModel.cs",
                                           "EditorViewModel.Run()", (12, 12)))
        self.assertEqual(result.owner_card, NS + "EditorViewModel")
        self.assertEqual([h.slot for h in result.hits if h.via == corpus.VIA_OWNERSHIP],
                         ["karta", "mapa"])

    def test_unverified_places(self):
        index = self.open()
        cases = {
            "soubor": member("src/Nowhere.cs", "A.B", (1, 1)),
            "člen": member(EV, "EditorView.Missing()", (12, 12)),
            "řádky": member(EV, "EditorView.Check(bool)", (31, 31)),
            "typ": type_place(EV, "Nothing", (4, 4)),
        }
        for name, place in cases.items():
            result = index.locate(place)
            self.assertFalse(result.found, name)
            self.assertIsNone(result.owner_card)
            self.assertTrue(result.reason.startswith("místo nenalezeno v korpusu"), name)


class AnchorPathTest(CorpusTestCase):
    def test_document_without_level_comes_back_for_role_misto_only(self):
        result = self.open().locate(member(EV, "EditorView.Check(bool)", (12, 12)))
        loose = [h for h in result.hits if h.doc.id == "notes:loose"]
        self.assertEqual(len(loose), 1)
        self.assertEqual((loose[0].via, loose[0].slot), (corpus.VIA_ANCHOR, "jina"))
        self.assertNotIn("notes:loose", ids(result.for_role("ramcova")))
        self.assertIn("notes:loose", ids(result.for_role("misto")))
        self.assertEqual(ids(result.for_role("ramcova")),
                         [NS + "EditorView", "map:src/Acme"])
        with self.assertRaises(ValueError):
            result.for_role("podpora")

    def test_triggers_index_anchors_a_place_the_manifest_does_not_give_it(self):
        index = self.open("corpus_l2")
        result = index.locate(member("src/Acme/Jiny.cs", "AcmeTrigger.Fire()", (840, 840)))
        self.assertTrue(result.found)
        hit = next(h for h in result.hits if h.doc.id == "triggers:index")
        self.assertEqual((hit.via, hit.slot, hit.anchor.raw),
                         (corpus.VIA_ANCHOR, "mapa", "src/Acme/Jiny.cs:836-845"))
        self.assertEqual(result.owner_card, "sym:csharp:Acme.Triggers#AcmeTrigger")
        outside = index.locate(member("src/Acme/Jiny.cs", "AcmeTrigger.Fire()", (850, 850)))
        self.assertNotIn("triggers:index", ids(outside.hits))
        listed = index.locate(member("src/Acme/Jiny.cs", "AcmeTrigger.Fire()", (860, 870)))
        self.assertNotIn("triggers:index", [h.doc.id for h in listed.hits
                                            if h.anchor and h.anchor.start == 836])

    def test_a_list_of_lines_in_a_document_is_read_as_separate_ranges(self):
        corpus_dir = self.copy_corpus("corpus_l1")
        note = corpus_dir / "notes" / "loose.md"
        note.write_text("# Note\n\nSee `src/Acme/Views/TwoViews.cs:9, 27:3`.\n", encoding="utf-8")
        index = open_corpus(corpus_dir, self.state)
        for place, expected in ((member(TWO, "ViewA.Check(bool)", (9, 9)), True),
                                (member(TWO, "ViewA.Check(bool)", (10, 10)), False),
                                (member(TWO, "ViewB.Check(bool)", (27, 27)), True)):
            hit = [h for h in index.locate(place).hits if h.doc.id == "notes:loose"]
            self.assertEqual(bool(hit), expected, place)

    def test_header_path_is_not_used(self):
        # Both ViewA and ViewB cards have `header.path` TwoViews.cs; a place in
        # ViewB must not bring ViewA's card through the shared file.
        result = self.open().locate(member(TWO, "ViewB.Check(bool)", (28, 28)))
        self.assertNotIn(NS + "ViewA", ids(result.hits))
        self.assertNotIn("detail:" + NS + "ViewA", ids(result.hits))


class ManifestShapeTest(CorpusTestCase):
    def single_file_copy(self):
        target = self.copy_corpus("corpus_l1")
        strings = json.loads((target / "manifest-strings.json").read_text(encoding="utf-8"))
        nodes = [json.loads(line) for line in
                 (target / "manifest-nodes.jsonl").read_text(encoding="utf-8").splitlines()]
        docs = [json.loads(line) for line in
                (target / "manifest-documents.jsonl").read_text(encoding="utf-8").splitlines()]
        (target / "manifest.json").write_text(json.dumps({
            "schema_version": strings["schema_version"], "strings": strings["strings"],
            "documents": docs, "nodes": nodes, "edges": []}), encoding="utf-8")
        for name in ("manifest-strings.json", "manifest-nodes.jsonl", "manifest-documents.jsonl"):
            (target / name).unlink()
        return target

    def test_both_shapes_give_the_same_result(self):
        by_lines = open_corpus(FIXTURES / "corpus_l1", self.tmp / "s1")
        single = open_corpus(self.single_file_copy(), self.tmp / "s2")
        places = [member(EV, "EditorView.Check(bool)", (12, 16)),
                  member(EV, "EditorView.Inner.Count", (50, 50)),
                  member(TWO, "ViewB.Check(bool)", (28, 28)),
                  type_place(EV, "EditorView", (4, 4)),
                  member(EV, "EditorView.Missing()", (12, 12))]
        for place in places:
            a, b = by_lines.locate(place), single.locate(place)
            self.assertEqual((a.found, a.owner_card, [(h.doc.id, h.via) for h in a.hits]),
                             (b.found, b.owner_card, [(h.doc.id, h.via) for h in b.hits]))

    def test_a_manifest_missing_a_part_is_refused(self):
        target = self.copy_corpus("corpus_l1")
        (target / "manifest-nodes.jsonl").unlink()
        with self.assertRaisesRegex(CorpusError, "neúplný"):
            open_corpus(target, self.state)
        shutil.rmtree(target / "cards")
        for name in ("manifest-strings.json", "manifest-documents.jsonl"):
            (target / name).unlink()
        with self.assertRaisesRegex(CorpusError, "nemá manifest"):
            open_corpus(target, self.state)


class IdentityTest(CorpusTestCase):
    def test_revision_of_the_corpus_must_be_the_commit_of_the_item(self):
        other = "b" * 40
        with self.assertRaises(CorpusError) as caught:
            self.open(commit=other)
        self.assertIn(SHA, str(caught.exception))
        self.assertIn(other, str(caught.exception))
        index = self.open(commit=SHA)
        self.assertEqual(index.identity.source, corpus.IDENTITY_FROM_COVERAGE)
        self.assertEqual(index.identity.label, "identita z coverage.json")
        self.assertEqual(self.open(commit=SHA[:12]).identity.revision, SHA)

    def test_coverage_without_a_revision_is_refused(self):
        target = self.copy_corpus("corpus_l1")
        (target / "coverage.json").write_text('{"schema_version": 6}', encoding="utf-8")
        with self.assertRaisesRegex(CorpusError, "repo_revision"):
            open_corpus(target, self.state)
        (target / "coverage.json").write_text('{"repo_revision": ""}', encoding="utf-8")
        with self.assertRaisesRegex(CorpusError, "repo_revision"):
            open_corpus(target, self.state)
        (target / "coverage.json").unlink()
        with self.assertRaisesRegex(CorpusError, "coverage.json"):
            open_corpus(target, self.state)

    def test_a_runner_receipt_takes_precedence(self):
        target = self.copy_corpus("corpus_l1")
        (target / "coverage.json").write_text('{"schema_version": 6}', encoding="utf-8")
        receipt = target.parent / f"{target.name}.receipt.json"
        receipt.write_text(json.dumps({"format_version": 1, "base_revision": SHA,
                                       "tree_fingerprint": "f" * 64}), encoding="utf-8")
        index = open_corpus(target, self.state, commit=SHA)
        self.assertEqual(index.identity.source, corpus.IDENTITY_FROM_RECEIPT)
        self.assertEqual(index.identity.tree_fingerprint, "f" * 64)
        self.assertIn("f" * 64, index.identity.label)
        with self.assertRaises(CorpusError):
            open_corpus(target, self.state, commit="c" * 40)
        receipt.write_text('{"format_version": 1}', encoding="utf-8")
        with self.assertRaisesRegex(CorpusError, "base_revision"):
            open_corpus(target, self.state)


class IndexCacheTest(CorpusTestCase):
    def test_second_open_uses_the_stored_index(self):
        target = self.copy_corpus("corpus_l1")
        first = open_corpus(target, self.state)
        self.assertFalse(first.from_cache)
        with mock.patch.object(corpus, "build_index", side_effect=AssertionError("rebuilt")), \
                mock.patch.object(corpus, "iter_jsonl", side_effect=AssertionError("streamed")):
            second = open_corpus(target, self.state)
        self.assertTrue(second.from_cache)
        place = member(EV, "EditorView.Check(bool)", (12, 16))
        self.assertEqual([(h.doc.id, h.via) for h in first.locate(place).hits],
                         [(h.doc.id, h.via) for h in second.locate(place).hits])

    def test_index_file_starts_with_format_version_and_is_in_the_state_dir(self):
        target = self.copy_corpus("corpus_l1")
        open_corpus(target, self.state)
        stored = list(self.state.glob("corpus-index-*.json"))
        self.assertEqual(len(stored), 1)
        self.assertTrue(stored[0].read_text(encoding="utf-8").startswith('{"format_version":'))
        self.assertEqual(list(self.state.glob(".*.tmp")), [])

    def test_changed_size_or_mtime_of_a_manifest_file_rebuilds(self):
        target = self.copy_corpus("corpus_l1")
        open_corpus(target, self.state)
        nodes = target / "manifest-nodes.jsonl"
        stat = nodes.stat()
        os.utime(nodes, ns=(stat.st_atime_ns, stat.st_mtime_ns + 5_000_000_000))
        self.assertFalse(open_corpus(target, self.state).from_cache)
        self.assertTrue(open_corpus(target, self.state).from_cache)
        with open(nodes, "a", encoding="utf-8") as handle:
            handle.write("\n")
        self.assertFalse(open_corpus(target, self.state).from_cache)

    def test_changed_coverage_rebuilds_but_a_changed_document_does_not(self):
        target = self.copy_corpus("corpus_l1")
        open_corpus(target, self.state)
        # The key is the stat of the manifest files and coverage.json, not of the documents.
        (target / "notes" / "loose.md").write_text("# Note\n", encoding="utf-8")
        self.assertTrue(open_corpus(target, self.state).from_cache)
        coverage = target / "coverage.json"
        coverage.write_text(coverage.read_text(encoding="utf-8") + "\n\n", encoding="utf-8")
        self.assertFalse(open_corpus(target, self.state).from_cache)

    def test_a_damaged_index_is_rebuilt(self):
        target = self.copy_corpus("corpus_l1")
        open_corpus(target, self.state)
        (stored,) = self.state.glob("corpus-index-*.json")
        stored.write_text("{not json", encoding="utf-8")
        self.assertFalse(open_corpus(target, self.state).from_cache)


class StreamingTest(CorpusTestCase):
    def write_big_corpus(self, rows):
        target = self.tmp / "big"
        target.mkdir()
        (target / "coverage.json").write_text(json.dumps({"repo_revision": SHA, "files": []}),
                                              encoding="utf-8")
        (target / "manifest-strings.json").write_text(
            json.dumps({"schema_version": 10, "strings": []}), encoding="utf-8")
        (target / "manifest-nodes.jsonl").write_text("", encoding="utf-8")
        filler = "x" * 120
        with open(target / "manifest-documents.jsonl", "w", encoding="utf-8") as handle:
            for n in range(rows):
                handle.write(json.dumps({"id": f"map:m{n}", "kind": "map", "level": "map",
                                         "title": f"Map {n}", "guiding_question": filler,
                                         "path": f"cards/map/m{n}.md", "chunks": []}) + "\n")
        return target

    def test_100000_document_rows_are_processed_as_a_stream(self):
        target = self.write_big_corpus(100_000)
        path = target / "manifest-documents.jsonl"
        self.assertGreater(path.stat().st_size, 20_000_000)
        self.assertTrue(inspect.isgenerator(corpus.iter_jsonl(path)))
        tracemalloc.start()
        try:
            count = sum(1 for _ in corpus.iter_jsonl(path))
            _, peak = tracemalloc.get_traced_memory()
        finally:
            tracemalloc.stop()
        self.assertEqual(count, 100_000)
        self.assertLess(peak, 1_000_000)  # one record at a time, not the 20 MB file
        index = open_corpus(target, self.state)
        self.assertEqual(len(index.docs), 100_000)
        self.assertEqual(index.missing_documents, 100_000)

    def test_a_broken_line_names_the_file_and_the_line(self):
        target = self.write_big_corpus(3)
        with open(target / "manifest-documents.jsonl", "a", encoding="utf-8") as handle:
            handle.write("{broken\n")
        with self.assertRaisesRegex(CorpusError, r"manifest-documents.jsonl, řádek 4"):
            open_corpus(target, self.state)


if __name__ == "__main__":
    unittest.main()
