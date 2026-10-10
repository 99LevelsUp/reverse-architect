"""Binding a block to a place (U3: KTD4), over the fixtures and over inline documents."""
import tempfile
import unittest
from pathlib import Path

import anchors
import corpus
from anchors import (PRESELECT_THRESHOLD_BYTES, bound_blocks, citation_binds, needs_preselection,
                     parse_answer, parse_document)
from catalog import Place

FIXTURES = Path(__file__).resolve().parent / "fixtures"
EV = "src/Acme/Views/EditorView.cs"
TWO = "src/Acme/Views/TwoViews.cs"
EXT = "src/Acme/Models/ExternalCheck.cs"
NS = "sym:csharp:Acme.Views#"
XNS = "sym:csharp:Acme.Models#"


def member(soubor, clen, lines):
    first, last = lines if isinstance(lines, tuple) else (lines, lines)
    return Place(soubor=soubor, clen=clen, typ=None, radky=(first, last), odkazovany_clen=None)


def type_place(soubor, typ, lines):
    return Place(soubor=soubor, clen=None, typ=typ, radky=lines, odkazovany_clen=None)


class FixtureCase(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls._tmp = tempfile.TemporaryDirectory()
        state = Path(cls._tmp.name)
        cls.l1 = corpus.open_corpus(FIXTURES / "corpus_l1", state)
        cls.l2 = corpus.open_corpus(FIXTURES / "corpus_l2", state)

    @classmethod
    def tearDownClass(cls):
        cls._tmp.cleanup()

    def parsed(self, index, doc_id):
        doc = index.document(doc_id)
        return parse_document(index.read_document(doc), doc.id, doc.title, doc.level)

    def texts(self, blocks):
        return [b.text for b in blocks]


class Layer1Test(FixtureCase):
    def test_purpose_sentence_binds_through_the_header_anchor(self):
        doc = self.parsed(self.l1, NS + "EditorView")
        found = bound_blocks(doc, member(EV, "EditorView.Check(bool)", (12, 12)))
        purpose = [b for b in found if b.section == "Purpose"]
        self.assertEqual(len(purpose), 1)
        self.assertEqual(purpose[0].anchor, "src/Acme/Views/EditorView.cs:3-60")
        self.assertEqual((purpose[0].doc_id, purpose[0].title, purpose[0].via),
                         (NS + "EditorView", "EditorView", anchors.VIA_ANCHOR))
        self.assertIn("2 methods", purpose[0].text)
        # The header strip itself carries no content and is not returned.
        self.assertFalse([b for b in found if "58 lines" in b.text])
        # Another type's place in another file does not bind.
        self.assertEqual(bound_blocks(doc, member(TWO, "ViewA.Check(bool)", (10, 10))), [])
        # Nor does a place of the same file outside the type's lines.
        self.assertEqual(bound_blocks(doc, member(EV, "EditorView.X", (61, 61))), [])

    def test_blocks_before_the_first_heading_bind_only_through_the_header(self):
        doc = self.parsed(self.l1, NS + "EditorView")
        found = bound_blocks(doc, member(EV, "EditorView.Check(bool)", (12, 12)))
        preamble = [b for b in found if b.section == ""]
        self.assertEqual([b.text for b in preamble],
                         ["Identity: `sym:csharp:Acme.Views#EditorView`",
                          "Detail: [detail](../../../detail/type/csharp/Acme.Views/EditorView.md)"])

    def test_detail_row_binds_through_its_own_anchor(self):
        doc = self.parsed(self.l1, "detail:" + NS + "EditorView")
        found = bound_blocks(doc, member(EV, "EditorView.Check(bool)", (10, 30)))
        row = [b for b in found if "Check(bool flag)" in b.text]
        self.assertEqual(len(row), 1)
        self.assertEqual((row[0].anchor, row[0].section), ("src/Acme/Views/EditorView.cs:10-30", "API"))

    def test_nested_uses_item_binds_through_its_parent_and_not_to_another_member(self):
        doc = self.parsed(self.l1, "detail:" + NS + "EditorView")
        check = bound_blocks(doc, member(EV, "EditorView.Check(bool)", (12, 12)))
        self.assertTrue([b for b in check if "RelayCommand" in b.text])
        self.assertFalse([b for b in check if "uses [ViewB]" in b.text])
        save = bound_blocks(doc, member(EV, "EditorView.Save()", (35, 35)))
        self.assertTrue([b for b in save if "uses [ViewB]" in b.text])
        self.assertFalse([b for b in save if "RelayCommand" in b.text])
        # The header of a detail covers lines 3-60, but is not used at all: only
        # blocks with an anchor of their own or of their parent bind.
        self.assertEqual({b.section for b in save}, {"API", "Test pairing"})
        nested = bound_blocks(doc, member(EV, "EditorView.Inner.Count", (50, 50)))
        self.assertEqual({b.section for b in nested}, {"API", "Test pairing"})

    def test_map_block_binds_when_it_names_the_type(self):
        doc = self.parsed(self.l1, "map:src/Acme")
        self.assertFalse(doc.has_anchors)
        view_a = bound_blocks(doc, member(TWO, "ViewA.Check(bool)", (10, 10)))
        self.assertEqual(len(view_a), 1)
        self.assertEqual((view_a[0].anchor, view_a[0].via), (None, anchors.VIA_NAME))
        self.assertIn("[ViewA]", view_a[0].text)
        # `EditorView` is named by its own row only: not by the `EditorViewModel`
        # row (not a whole word) and not by the `Inner` row, whose link
        # destination `EditorView.Inner.md` is not text.
        view = bound_blocks(doc, member(EV, "EditorView.Check(bool)", (12, 12)))
        self.assertEqual(len(view), 1)
        self.assertIn("[EditorView](", view[0].text)

    def test_citation_of_a_block(self):
        doc = self.parsed(self.l1, NS + "EditorView")
        place = member(EV, "EditorView.Check(bool)", (12, 12))
        found = citation_binds(doc, place, "2 methods, 1 property, 1 field")
        self.assertIsNotNone(found)
        self.assertEqual(found.section, "Purpose")
        self.assertIsNotNone(citation_binds(doc, place, "`class`:  2 methods,\n1 property"))
        self.assertIsNone(citation_binds(doc, place, "no such sentence"))
        # The block exists, but binds to another place.
        self.assertIsNone(citation_binds(doc, member(TWO, "ViewB.Check(bool)", (28, 28)),
                                         "2 methods, 1 property"))


class Layer2Test(FixtureCase):
    def card(self):
        return self.parsed(self.l2, XNS + "ExternalCheck")

    def test_step_binds_through_its_own_anchor_not_the_header(self):
        found = bound_blocks(self.card(), member(EXT, "ExternalCheck.Run()", (36, 36)))
        steps = [b for b in found if b.section == "Steps"]
        self.assertEqual([b.text for b in steps if b.anchor], ["Sends the document — `src/Acme/Models/ExternalCheck.cs:34-40`"])
        self.assertEqual(steps[0].anchor, "src/Acme/Models/ExternalCheck.cs:34-40")
        # The Purpose section has no anchor, so the header (5-120) applies to it.
        purpose = [b for b in found if b.section == "Purpose"]
        self.assertEqual([b.anchor for b in purpose], ["src/Acme/Models/ExternalCheck.cs:5-120"])

    def test_unanchored_block_binds_through_the_previous_anchor_of_its_section(self):
        found = bound_blocks(self.card(), member(EXT, "ExternalCheck.Run()", (44, 44)))
        closing = [b for b in found if b.text.startswith("This closing sentence")]
        self.assertEqual(len(closing), 1)
        self.assertEqual(closing[0].anchor, "src/Acme/Models/ExternalCheck.cs:44")
        self.assertEqual(closing[0].section, "Steps")

    def test_sentence_under_a_step_anchored_elsewhere_does_not_bind_to_this_place(self):
        # The header covers lines 5-120, the whole type, yet the sentence after the
        # Validate step belongs to that step's anchor (70) and to nothing else.
        run = bound_blocks(self.card(), member(EXT, "ExternalCheck.Run()", (34, 34)))
        self.assertFalse([b for b in run if b.text.startswith("This sentence follows")])
        validate = bound_blocks(self.card(), member(EXT, "ExternalCheck.Validate(string)", (70, 70)))
        sentence = [b for b in validate if b.text.startswith("This sentence follows")]
        self.assertEqual([b.anchor for b in sentence], ["src/Acme/Models/ExternalCheck.cs:70"])

    def test_nested_item_and_a_heading_ends_the_previous_anchor(self):
        validate = bound_blocks(self.card(), member(EXT, "ExternalCheck.Validate(string)", (64, 64)))
        self.assertTrue([b for b in validate if "RelayCommand" in b.text])
        # `### Values` follows `### Steps` of the earlier function: its own anchor decides.
        state = bound_blocks(self.card(), member(EXT, "ExternalCheck._verdict", (8, 8)))
        self.assertEqual({b.section for b in state if b.anchor and ":8" in b.anchor}, {"Values"})

    def test_mermaid_anchors_are_ignored(self):
        found = bound_blocks(self.card(), member(EXT, "ExternalCheck.Run()", (30, 30)))
        self.assertFalse([b for b in found if "flowchart" in b.text or "Opens the connection `" in b.text
                          and b.section == "Flowchart"])
        self.assertEqual({b.section for b in found if b.text.startswith("Opens")}, {"Steps"})
        self.assertNotIn("Flowchart", {b.section for b in found})

    def test_context_chapter_and_the_triggers_index(self):
        context = bound_blocks(self.card(), member(EXT, "ExternalCheck.Run()", (20, 20)))
        self.assertIn("caller: EditorViewModel.Save", {b.section for b in context})
        index = self.parsed(self.l2, "triggers:index")
        found = bound_blocks(index, member("src/Acme/Jiny.cs", "AcmeTrigger.Fire()", (840, 840)))
        self.assertEqual(len(found), 1)
        self.assertEqual(found[0].anchor, "src/Acme/Jiny.cs:836-845")
        self.assertTrue(found[0].text.startswith("trigger · OnSave"))
        self.assertEqual(found[0].section, "Part 1")


class InlineDocumentTest(unittest.TestCase):
    PLACE = member("f.cs", "A.Check(bool)", (5, 5))

    def bound(self, text, place=None, **kwargs):
        doc = parse_document(text, "d", "Doc", kwargs.pop("level", "card"), **kwargs)
        return bound_blocks(doc, place or self.PLACE)

    def test_heading_ends_the_previous_anchor_even_when_it_is_h3(self):
        text = "# T\n\n## A\n\n- x — `f.cs:5`\n\n### B\n\nplain sentence\n"
        self.assertEqual([b.text for b in self.bound(text)], ["x — `f.cs:5`"])
        with_header = "# T\n\n**c** · `f.cs:1-9`\n\n" + text.split("\n\n", 1)[1]
        found = self.bound(with_header)
        # B has no anchor in it, so the header (1-9) applies to it; but A's does not leak.
        self.assertEqual([(b.section, b.anchor) for b in found],
                         [("A", "f.cs:5"), ("B", "f.cs:1-9")])

    def test_heading_anchor_comes_before_a_previous_block(self):
        text = "# T\n\n## S — `f.cs:5`\n\nfirst\n\n- own — `f.cs:50`\n\nafter\n"
        found = self.bound(text)
        self.assertEqual([(b.text, b.anchor) for b in found],
                         [("first", "f.cs:5"), ("after", "f.cs:5")])

    def test_block_with_its_own_anchor_that_misses_does_not_fall_back(self):
        # `other` misses; `lead` has no anchor before it in a section that holds one,
        # so the header (1-9) does not apply to it either.
        text = "# T\n\n**c** · `f.cs:1-9`\n\n## S\n\nlead sentence\n\n- other — `f.cs:50`\n"
        self.assertEqual(self.bound(text), [])
        no_anchor_in_section = "# T\n\n**c** · `f.cs:1-9`\n\n## S\n\nlead sentence\n"
        self.assertEqual([b.text for b in self.bound(no_anchor_in_section)], ["lead sentence"])

    def test_block_before_the_first_heading(self):
        text = "# T\n\n**c** · `f.cs:1-9`\n\nIntro without an anchor.\n\n## S\n\n- y — `f.cs:50`\n"
        found = self.bound(text)
        self.assertEqual([(b.text, b.section, b.anchor) for b in found],
                         [("Intro without an anchor.", "", "f.cs:1-9")])
        no_header = "# T\n\nIntro without an anchor.\n\n## S\n\n- y — `f.cs:5`\n"
        self.assertEqual([b.text for b in self.bound(no_header)], ["y — `f.cs:5`"])

    def test_detail_never_uses_the_header(self):
        text = "# T\n\n**c** · `f.cs:1-9`\n\nIntro.\n\n## Signature\n\nsentence\n"
        self.assertEqual(self.bound(text, level="detail"), [])
        self.assertEqual(len(self.bound(text, level="card")), 2)

    def test_column_and_list_anchors(self):
        text = "# T\n\n## S\n\n- a — `f.cs:5:12`\n- b — `f.cs:3, 5`\n- c — `f.cs:6, 30`\n"
        self.assertEqual([b.text.split(" ")[0] for b in self.bound(text)], ["a", "b"])
        self.assertEqual([b.anchor for b in self.bound(text)], ["f.cs:5:12", "f.cs:3, 5"])

    def test_mermaid_and_other_fences_are_skipped(self):
        text = "# T\n\n## S\n\n```mermaid\nA[\"`f.cs:5`\"]\n```\n\n~~~\n- `f.cs:5`\n~~~\n"
        self.assertEqual(self.bound(text), [])

    def test_tables_lists_and_continuations_are_blocks(self):
        text = ("# T\n\n## S\n\n| a | b |\n|---|---|\n| `f.cs:5` | one |\n| `f.cs:9` | two |\n\n"
                "- item `f.cs:5`\n  continues here\n  - child\n- next `f.cs:9`\n")
        found = self.bound(text)
        self.assertEqual([b.text for b in found],
                         ["| `f.cs:5` | one |", "item `f.cs:5`\ncontinues here", "child"])

    def test_name_rule_in_a_document_without_anchors(self):
        text = ("# Map\n\n## Types\n\n- class [A](a.md) — does things\n- class [B](b.md)\n"
                "- `Check` is shared\n- `B.Check` validates\n- `A.Check` and `view.Check` call it\n"
                "- nothing here\n- [B](A.md)\n")
        a_place = member("f.cs", "A.Check(bool)", (5, 5))
        b_place = member("g.cs", "B.Check(bool)", (5, 5))
        a = [b.text for b in self.bound(text, a_place, level="map")]
        b = [b.text for b in self.bound(text, b_place, level="map")]
        self.assertEqual(a, ["class [A](a.md) — does things", "`Check` is shared",
                             "`A.Check` and `view.Check` call it"])
        self.assertEqual(b, ["class [B](b.md)", "`Check` is shared", "`B.Check` validates",
                             "`A.Check` and `view.Check` call it", "[B](A.md)"])
        # a place of a member only (no type known) is named by the member alone
        loose = [b.text for b in self.bound(text, member("f.cs", "Check", (5, 5)), level="map")]
        self.assertIn("`B.Check` validates", loose)

    def test_type_place_and_whole_words(self):
        text = "# Map\n\n- [EditorViewModel](x.md)\n- the EditorView.\n- EditorView_2\n"
        place = type_place("f.cs", "EditorView", (1, 9))
        self.assertEqual([b.text for b in self.bound(text, place, level="map")],
                         ["the EditorView."])

    def test_two_types_with_the_same_member_in_two_documents(self):
        text = "# Map\n\n- `Check` only\n- `ViewB.Check` qualified\n"
        only_a = self.bound(text, member("f.cs", "ViewA.Check(bool)", (5, 5)), level="map")
        only_b = self.bound(text, member("f.cs", "ViewB.Check(bool)", (5, 5)), level="map")
        self.assertEqual([b.text for b in only_a], ["`Check` only"])
        self.assertEqual([b.text for b in only_b], ["`Check` only", "`ViewB.Check` qualified"])


class AnswerTest(unittest.TestCase):
    PLACE = member("src/Typ.cs", "Typ.Check(bool)", (1066, 1070))

    def bound(self, answer, place=None):
        return bound_blocks(parse_answer(answer), place or self.PLACE)

    def test_paragraph_with_a_cited_anchor_binds_through_it(self):
        found = self.bound("Zápis je v `src/Typ.cs:1069`, kde se nastavuje příznak.\n\n"
                           "Úplně jiný odstavec bez jmen.\n")
        self.assertEqual([(b.anchor, b.via) for b in found], [("src/Typ.cs:1069", "kotva")])
        self.assertEqual(found[0].doc_id, "odpoved")

    def test_paragraph_without_a_citation_binds_by_names_only(self):
        found = self.bound("Metoda Check(bool) rozhoduje.\n\nTyp Typ to řeší.\n\nNic.\n")
        self.assertEqual([(b.text, b.via) for b in found],
                         [("Metoda Check(bool) rozhoduje.", "jmeno"), ("Typ Typ to řeší.", "jmeno")])

    def test_a_citation_elsewhere_decides_against_the_name(self):
        self.assertEqual(self.bound("Typ.Check je v `src/Typ.cs:2000`.\n"), [])
        self.assertEqual(self.bound("Typ.Check je v `src/Other.cs:1069`.\n"), [])

    def test_unbackticked_citations_are_read_too(self):
        found = self.bound("Viz src/Typ.cs:1069 pro zápis.\n")
        self.assertEqual([b.anchor for b in found], ["src/Typ.cs:1069"])

    def test_answer_is_cut_into_blocks_and_fences_are_skipped(self):
        answer = "- `src/Typ.cs:1068` krok 1\n- krok 2 bez citace\n```\n`src/Typ.cs:1069`\n```\n"
        found = self.bound(answer)
        self.assertEqual([b.text for b in found], ["`src/Typ.cs:1068` krok 1"])


class PreselectionTest(unittest.TestCase):
    def test_threshold_and_levels(self):
        self.assertEqual(PRESELECT_THRESHOLD_BYTES, 16 * 1024)
        self.assertFalse(needs_preselection("card", 100))
        self.assertFalse(needs_preselection("map", PRESELECT_THRESHOLD_BYTES))
        self.assertTrue(needs_preselection("card", PRESELECT_THRESHOLD_BYTES + 1))
        self.assertTrue(needs_preselection("map", PRESELECT_THRESHOLD_BYTES + 1))
        self.assertTrue(needs_preselection("detail", 10))
        self.assertTrue(needs_preselection(None, 10))
        self.assertTrue(needs_preselection("page", 10))

    def test_size_is_in_bytes_of_the_text(self):
        doc = parse_document("# T\n\nžluťoučký\n", "d", "T", "card")
        self.assertEqual(doc.size_bytes, len("# T\n\nžluťoučký\n".encode("utf-8")))

    def test_fixture_card_goes_whole_and_detail_is_preselected(self):
        card = (FIXTURES / "corpus_l1/cards/type/csharp/Acme.Views/EditorView.md").read_text(
            encoding="utf-8")
        self.assertFalse(needs_preselection("card", len(card.encode("utf-8"))))
        big = card + "\n" + "- filler sentence\n" * 1500
        self.assertTrue(needs_preselection("card", len(big.encode("utf-8"))))


class JudgeTextTest(unittest.TestCase):
    """What the judge reads of a block: its document title and the heading of its section (KTD4)."""

    TEXT = ("# Title\n\n**class** · `a.cs:1-9`\n\n## Alpha\n\n- First — `a.cs:2`\n"
            "- Second — `a.cs:3`\n\n## Beta\n\n- Third — `a.cs:5`\n")

    def doc(self):
        return parse_document(self.TEXT, doc_id="d", title="Title", level="card")

    def test_a_block_carries_a_line_with_title_and_section(self):
        self.assertEqual(anchors.judge_text("Title", "Alpha", "First"), "Title > Alpha\nFirst")
        self.assertEqual(anchors.judge_text("Title", "", "First"), "Title\nFirst")
        self.assertEqual(anchors.judge_text("", "Alpha", "First"), "Alpha\nFirst")
        self.assertEqual(anchors.judge_text("", "", "First"), "First")

    def test_a_document_that_goes_whole_opens_with_its_title_and_every_section_with_its_heading(self):
        self.assertEqual(anchors.whole_judge_texts(self.doc()), [
            "Title\n**class** · `a.cs:1-9`", "Alpha\nFirst — `a.cs:2`", "Second — `a.cs:3`",
            "Beta\nThird — `a.cs:5`"])

    def test_an_answer_has_neither_title_nor_headings(self):
        answer = parse_answer("První věta `a.cs:2`.\n\nDruhá věta.\n")
        self.assertEqual(anchors.whole_judge_texts(answer), [b.text for b in answer.blocks])

    def test_a_heading_belongs_to_the_block_that_follows_it(self):
        doc = self.doc()
        first_of_alpha = next(b for b in doc.blocks if b.text.startswith("First"))
        self.assertEqual(anchors.citation_blocks(doc, "Alpha"), [first_of_alpha])
        self.assertEqual(anchors.citation_blocks(doc, "alpha"), [first_of_alpha])
        self.assertEqual(anchors.citation_blocks(doc, "Title"), [doc.blocks[0]])
        place = member("a.cs", "T.M()", 2)
        self.assertEqual(citation_binds(doc, place, "Alpha").anchor, "a.cs:2")
        self.assertIsNone(citation_binds(doc, member("a.cs", "T.M()", 5), "Alpha"))
        self.assertEqual(citation_binds(doc, member("a.cs", "T.M()", 5), "Beta").anchor, "a.cs:5")

    def test_a_quotation_that_runs_from_the_heading_into_the_text_finds_the_block(self):
        doc = self.doc()
        found = anchors.citation_blocks(doc, "Alpha\nFirst")
        self.assertEqual([b.text for b in found], ["First — `a.cs:2`"])
        both = anchors.citation_blocks(doc, "Title > Alpha\nFirst")
        self.assertEqual([b.text for b in both], ["First — `a.cs:2`"])

    NESTED = ("# Title\n\n**class** · `a.cs:1-9`\n\n## Dynamic access\n\n### Context\n\n"
              "#### caller: X.Save\n\n- enters the check — `a.cs:2`\n- calls it — `a.cs:3`\n\n"
              "## Next\n\n- last — `a.cs:5`\n")

    def test_headings_without_a_block_of_their_own_open_the_next_block(self):
        doc = parse_document(self.NESTED, doc_id="d", title="Title", level="card")
        self.assertEqual(anchors.whole_judge_texts(doc), [
            "Title\n**class** · `a.cs:1-9`",
            "Dynamic access > Context > caller: X.Save\nenters the check — `a.cs:2`",
            "calls it — `a.cs:3`", "Next\nlast — `a.cs:5`"])
        entering = doc.blocks[1]
        for quoted in ("Dynamic access", "Context", "caller: X.Save", "Dynamic access > Context"):
            self.assertEqual(anchors.citation_blocks(doc, quoted), [entering], quoted)
        self.assertEqual(citation_binds(doc, member("a.cs", "T.M()", 2), "Context").anchor, "a.cs:2")

    def test_an_answer_keeps_no_headings(self):
        self.assertEqual(parse_answer("Text `a.cs:2`.\n").headings, ())

    def test_a_text_that_is_in_a_block_still_wins_over_a_heading(self):
        doc = parse_document("# T\n\n## Run\n\n- Run the check `a.cs:2`\n\n## Other\n\n"
                             "- Something else `a.cs:3`\n", doc_id="d", title="T", level="card")
        self.assertEqual([b.text for b in anchors.citation_blocks(doc, "Run the check")],
                         ["Run the check `a.cs:2`"])


class AudienceTest(unittest.TestCase):
    def test_a_block_only_in_a_detail_serves_the_developer_not_support(self):
        self.assertEqual(anchors.audiences(["detail"]), ("vyvojar",))
        self.assertEqual(anchors.audiences(["detail", "jina"]), ("vyvojar",))
        self.assertEqual(anchors.audiences(["detail", "karta"]), ("vyvojar", "podpora"))
        self.assertEqual(anchors.audiences(["mapa"]), ("vyvojar", "podpora"))
        self.assertEqual(anchors.audiences([]), ())


if __name__ == "__main__":
    unittest.main()
