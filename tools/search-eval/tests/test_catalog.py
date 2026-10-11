"""Catalog format and validation (U1: R1, R2, R3, R13, R16)."""
import copy
import json
import tempfile
import unittest
from pathlib import Path

import catalog
from catalog import CatalogError, confirmation_fingerprint, load_catalog, parse_catalog

EXAMPLE = Path(__file__).resolve().parents[1] / "examples" / "catalog.example.json"
BASE = "a" * 40
FIX = "b" * 40


def valid_case(case_id: str = "case-1") -> dict:
    return {
        "id": case_id,
        "prompt": "Zákazník A hlásí chybu při uložení záznamu.",
        "acceptable_locations": [
            {"file": "Src/Placeholder/LoadAction.cs",
             "member": "LoadAction.SetScheduledTasksInvalid",
             "line": 42, "fix_commit": FIX},
        ],
        "bug_confirmed_at_base": True,
        "pseudonymization_confirmed": True,
        "used_for_card_tuning": False,
        "prompt_names_location": False,
    }


def valid_catalog() -> dict:
    return {"format_version": 1, "base_revision": BASE, "cases": [valid_case()]}


def place(file: str = "Src/Placeholder/ViewA.cs", member: str = "ViewA.Check(bool)",
          lines=12, **extra) -> dict:
    return {"soubor": file, "clen": member, "radky": lines, **extra}


def sealed(question: dict) -> dict:
    """Set `otisk_potvrzeni` to the fingerprint of the question's current facts and claims."""
    question["otisk_potvrzeni"] = confirmation_fingerprint(question["fakta"],
                                                            question["zakazana_tvrzeni"])
    return question


def valid_question(question_id: str = "q-1") -> dict:
    return sealed({
        "druh": "otazka",
        "id": question_id,
        "dotaz": "Proč se po uložení změní stav modelu ModelB?",
        "ramcova_odpoved": "Uložení předá práci modelu ModelB, ten zapíše příznak změny.",
        "commit": BASE,
        "fakta_potvrzena": True,
        "pseudonymizace_potvrzena": True,
        "otisk_potvrzeni": "",
        "kontrolni": False,
        "fakta": [
            {"id": "f1", "text": "Kliknutí spustí příkaz.", "role": "ramcova",
             "mista": [place(odkazovany_clen="SaveCommand")],
             "tok": {"krok": 1, "tok": "ulozeni"}},
            {"id": "f2", "text": "Model zapíše příznak.", "role": "misto",
             "mista": [place("Src/Placeholder/ModelB.cs", "ModelB.Save(+1)", "52-60")],
             "tok": {"krok": 2, "tok": "ulozeni"}},
        ],
        "zakazana_tvrzeni": [
            {"id": "z1", "text": "Stav mění přímo pohled.",
             "mista": [{"soubor": "Src/Placeholder/ViewA.cs", "typ": "ViewA", "radky": 5}],
             "fakty": ["f1"]},
        ],
    })


def valid_catalog_2(*items: dict) -> dict:
    cases = list(items) or [dict(valid_case(), druh="chyba"), valid_question()]
    return {"format_version": 2, "base_revision": BASE, "cases": cases}


class ExampleCatalogTest(unittest.TestCase):
    def test_example_catalog_passes_validation(self):
        cat = load_catalog(EXAMPLE)
        self.assertGreaterEqual(len(cat.cases), 2)
        self.assertGreaterEqual(len(cat.questions), 1)
        self.assertEqual(len(cat.base_revision), 40)

    def test_example_carries_the_ae2_member(self):
        cat = load_catalog(EXAMPLE)
        members = [loc.member for case in cat.cases for loc in case.acceptable_locations]
        self.assertIn("LoadAction.SetScheduledTasksInvalid", members)
        loc = next(loc for case in cat.cases for loc in case.acceptable_locations
                   if loc.member == "LoadAction.SetScheduledTasksInvalid")
        self.assertEqual(loc.file, "Src/Placeholder/LoadAction.cs")
        self.assertEqual(loc.member_name, "SetScheduledTasksInvalid")

    def test_example_marks_itself_as_synthetic(self):
        # The example must not look like a real case: every prompt says it is one.
        cat = load_catalog(EXAMPLE)
        for case in cat.cases:
            self.assertIn("Ukázkové zadání", case.prompt)
        for question in cat.questions:
            self.assertIn("Ukázkové zadání", question.dotaz)


class ValidationTest(unittest.TestCase):
    def assertRejected(self, data: dict, *fragments: str) -> None:
        with self.assertRaises(CatalogError) as ctx:
            parse_catalog(data)
        message = str(ctx.exception)
        for fragment in fragments:
            self.assertIn(fragment, message)

    def test_valid_catalog_loads_from_file(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "catalog.json"
            path.write_text(json.dumps(valid_catalog()), encoding="utf-8")
            cat = load_catalog(path)
        self.assertEqual(cat.base_revision, BASE)
        case = cat.cases[0]
        self.assertEqual(case.id, "case-1")
        self.assertTrue(case.bug_confirmed_at_base)
        self.assertFalse(case.prompt_names_location)
        self.assertEqual(case.acceptable_locations[0].line, 42)

    def test_case_without_acceptable_location_is_rejected(self):
        data = valid_catalog()
        data["cases"][0]["acceptable_locations"] = []
        self.assertRejected(data, "case-1", "acceptable_locations")

    def test_case_with_missing_locations_field_is_rejected(self):
        data = valid_catalog()
        del data["cases"][0]["acceptable_locations"]
        self.assertRejected(data, "case-1", "acceptable_locations")

    def test_location_without_fix_commit_is_rejected(self):
        data = valid_catalog()
        del data["cases"][0]["acceptable_locations"][0]["fix_commit"]
        self.assertRejected(data, "case-1", "fix_commit")

    def test_location_with_empty_fix_commit_is_rejected(self):
        data = valid_catalog()
        data["cases"][0]["acceptable_locations"][0]["fix_commit"] = ""
        self.assertRejected(data, "case-1", "fix_commit")

    def test_unconfirmed_bug_is_rejected(self):
        data = valid_catalog()
        data["cases"][0]["bug_confirmed_at_base"] = False
        self.assertRejected(data, "case-1", "bug_confirmed_at_base")

    def test_missing_bug_confirmation_is_rejected(self):
        data = valid_catalog()
        del data["cases"][0]["bug_confirmed_at_base"]
        self.assertRejected(data, "case-1", "bug_confirmed_at_base")

    def test_unconfirmed_pseudonymization_is_rejected(self):
        data = valid_catalog()
        data["cases"][0]["pseudonymization_confirmed"] = False
        self.assertRejected(data, "case-1", "pseudonymization_confirmed")

    def test_confirmation_must_be_boolean_true_not_truthy(self):
        data = valid_catalog()
        data["cases"][0]["pseudonymization_confirmed"] = "ano"
        self.assertRejected(data, "case-1", "pseudonymization_confirmed")

    def test_flags_are_required(self):
        for flag in ("used_for_card_tuning", "prompt_names_location"):
            data = valid_catalog()
            del data["cases"][0][flag]
            self.assertRejected(data, "case-1", flag)

    def test_missing_base_revision_is_rejected(self):
        data = valid_catalog()
        del data["base_revision"]
        self.assertRejected(data, "base_revision")

    def test_unknown_format_version_is_rejected(self):
        for version in (0, 3, "2", True):
            data = valid_catalog()
            data["format_version"] = version
            self.assertRejected(data, "format_version")

    def test_duplicate_case_ids_are_rejected(self):
        data = valid_catalog()
        data["cases"].append(copy.deepcopy(data["cases"][0]))
        self.assertRejected(data, "case-1", "id")

    def test_path_escaping_the_tree_is_rejected(self):
        for bad in ("../outside.cs", "/abs/file.cs", "C:/abs/file.cs"):
            data = valid_catalog()
            data["cases"][0]["acceptable_locations"][0]["file"] = bad
            self.assertRejected(data, "case-1", "file")

    def test_backslashes_in_file_are_normalized(self):
        data = valid_catalog()
        data["cases"][0]["acceptable_locations"][0]["file"] = "Src\\Placeholder\\LoadAction.cs"
        loc = parse_catalog(data).cases[0].acceptable_locations[0]
        self.assertEqual(loc.file, "Src/Placeholder/LoadAction.cs")

    def test_non_positive_line_is_rejected(self):
        data = valid_catalog()
        data["cases"][0]["acceptable_locations"][0]["line"] = 0
        self.assertRejected(data, "case-1", "line")

    def test_case_with_several_locations_loads_all_of_them(self):
        data = valid_catalog()
        data["cases"][0]["acceptable_locations"].append(
            {"file": "Src/Placeholder/Scheduler.cs", "member": "Scheduler.Reset",
             "line": 7, "fix_commit": "c" * 40})
        locs = parse_catalog(data).cases[0].acceptable_locations
        self.assertEqual([(l.file, l.member, l.line, l.fix_commit) for l in locs], [
            ("Src/Placeholder/LoadAction.cs", "LoadAction.SetScheduledTasksInvalid", 42, FIX),
            ("Src/Placeholder/Scheduler.cs", "Scheduler.Reset", 7, "c" * 40),
        ])


class FormatTwoTest(unittest.TestCase):
    def assertRejected(self, data: dict, *fragments: str) -> None:
        with self.assertRaises(CatalogError) as ctx:
            parse_catalog(data)
        message = str(ctx.exception)
        for fragment in fragments:
            self.assertIn(fragment, message)

    def question_catalog(self, change=None) -> dict:
        """A format 2 catalog with one question; `change` edits it, then it is re-sealed."""
        question = valid_question()
        if change:
            change(question)
            sealed(question)
        return valid_catalog_2(question)

    def test_format_one_catalog_loads_and_every_item_is_a_bug(self):
        cat = parse_catalog(valid_catalog())
        self.assertEqual([c.druh for c in cat.items], ["chyba"])
        self.assertEqual(len(cat.cases), 1)
        self.assertEqual(cat.questions, ())

    def test_format_one_item_with_a_kind_field_is_rejected(self):
        data = valid_catalog()
        data["cases"][0]["druh"] = "chyba"
        self.assertRejected(data, "case-1", "druh")

    def test_format_two_loads_bug_and_question_with_their_kinds(self):
        cat = parse_catalog(valid_catalog_2())
        self.assertEqual([c.druh for c in cat.items], ["chyba", "otazka"])
        self.assertEqual([c.id for c in cat.cases], ["case-1"])
        self.assertEqual([q.id for q in cat.questions], ["q-1"])
        question = cat.questions[0]
        self.assertEqual(question.commit, BASE)
        self.assertEqual([f.id for f in question.fakta], ["f1", "f2"])
        self.assertEqual(question.fakta[0].mista[0].odkazovany_clen, "SaveCommand")
        self.assertEqual(question.fakta[0].mista[0].name, "Check")
        self.assertEqual(question.fakta[0].tok.krok, 1)
        self.assertEqual(question.fakta[1].mista[0].radky, (52, 60))
        self.assertEqual(question.zakazana_tvrzeni[0].mista[0].typ, "ViewA")
        self.assertEqual(question.zakazana_tvrzeni[0].fakty, ("f1",))

    def test_format_two_item_needs_a_known_kind(self):
        self.assertRejected(valid_catalog_2(valid_case()), "case-1", "druh")
        self.assertRejected(valid_catalog_2(dict(valid_case(), druh="dotaz")), "case-1", "druh")

    def test_bug_item_of_format_two_keeps_its_format_one_fields(self):
        item = dict(valid_case(), druh="chyba")
        del item["prompt_names_location"]
        self.assertRejected(valid_catalog_2(item), "case-1", "prompt_names_location")

    def test_control_question_loads_and_differs_from_an_acceptance_item(self):
        control = valid_question("q-control")
        control["kontrolni"] = True
        cat = parse_catalog(valid_catalog_2(valid_question("q-acceptance"), control))
        flags = {q.id: q.kontrolni for q in cat.questions}
        self.assertEqual(flags, {"q-acceptance": False, "q-control": True})

    def test_fact_without_a_place_is_rejected_naming_item_and_fact(self):
        def empty(q):
            q["fakta"][1]["mista"] = []
        self.assertRejected(self.question_catalog(empty), "q-1", "f2", "mista")

        def missing(q):
            del q["fakta"][1]["mista"]
        self.assertRejected(self.question_catalog(missing), "q-1", "f2", "mista")

    def test_forbidden_claim_without_a_place_is_rejected(self):
        def change(q):
            q["zakazana_tvrzeni"][0]["mista"] = []
        self.assertRejected(self.question_catalog(change), "q-1", "z1", "mista")

    def test_unknown_role_is_rejected(self):
        def change(q):
            q["fakta"][0]["role"] = "vyvojar"
        self.assertRejected(self.question_catalog(change), "q-1", "f1", "role")

    def test_whole_flow_not_covering_every_step_is_rejected(self):
        def change(q):
            q["fakta"].append({"id": "f3", "text": "Celý tok.", "role": "ramcova",
                               "mista": [place(odkazovany_clen="SaveCommand")],
                               "tok": {"cely_tok": "ulozeni"}})
        self.assertRejected(self.question_catalog(change), "q-1", "f3", "ulozeni", "kroku 2")

    def test_whole_flow_covering_every_step_is_accepted(self):
        def change(q):
            q["fakta"].append({"id": "f3", "text": "Celý tok.", "role": "ramcova",
                               "mista": [place(), place("Src/Placeholder/ModelB.cs",
                                                        "ModelB.Save(+1)", "52-60")],
                               "tok": {"cely_tok": "ulozeni"}})
        cat = parse_catalog(self.question_catalog(change))
        self.assertTrue(cat.questions[0].fakta[2].tok.whole)

    def test_whole_flow_of_a_flow_without_steps_is_rejected(self):
        def change(q):
            q["fakta"][0]["tok"] = {"cely_tok": "jiny"}
        self.assertRejected(self.question_catalog(change), "q-1", "f1", "jiny")

    def test_malformed_flow_reference_is_rejected(self):
        for bad in ({"krok": 0, "tok": "t"}, {"krok": 1}, {"cely_tok": "t", "krok": 1}, "t"):
            def change(q, bad=bad):
                q["fakta"][0]["tok"] = bad
            self.assertRejected(self.question_catalog(change), "q-1", "f1", "tok")

    def test_absolute_path_or_parent_segment_in_a_place_is_rejected(self):
        for bad in ("/abs/file.cs", "C:/abs/file.cs", "../outside.cs", "Src/../../x.cs"):
            def change(q, bad=bad):
                q["fakta"][0]["mista"][0]["soubor"] = bad
            self.assertRejected(self.question_catalog(change), "q-1", "f1", "soubor")

    def test_absolute_path_in_a_forbidden_claim_place_is_rejected(self):
        def change(q):
            q["zakazana_tvrzeni"][0]["mista"][0]["soubor"] = "/abs/ViewA.cs"
        self.assertRejected(self.question_catalog(change), "q-1", "z1", "soubor")

    def test_backslashes_in_a_place_are_normalized(self):
        def change(q):
            q["fakta"][0]["mista"][0]["soubor"] = "Src\\Placeholder\\ViewA.cs"
        cat = parse_catalog(self.question_catalog(change))
        self.assertEqual(cat.questions[0].fakta[0].mista[0].soubor, "Src/Placeholder/ViewA.cs")

    def test_place_needs_exactly_one_of_member_and_type(self):
        def both(q):
            q["fakta"][0]["mista"][0]["typ"] = "ViewA"
        self.assertRejected(self.question_catalog(both), "q-1", "f1", "clen")

        def neither(q):
            del q["fakta"][0]["mista"][0]["clen"]
        self.assertRejected(self.question_catalog(neither), "q-1", "f1", "clen")

    def test_lines_accept_a_number_or_a_text_range_only(self):
        for bad in (0, -3, "9-2", "x", [4, 9], True, 1.5):
            def change(q, bad=bad):
                q["fakta"][0]["mista"][0]["radky"] = bad
            self.assertRejected(self.question_catalog(change), "q-1", "f1", "radky")

    def test_forbidden_claim_pointing_to_a_missing_fact_is_rejected(self):
        def change(q):
            q["zakazana_tvrzeni"][0]["fakty"] = ["f1", "f9"]
        self.assertRejected(self.question_catalog(change), "q-1", "z1", "f9")

    def test_forbidden_claim_may_omit_facts(self):
        def change(q):
            del q["zakazana_tvrzeni"][0]["fakty"]
        cat = parse_catalog(self.question_catalog(change))
        self.assertEqual(cat.questions[0].zakazana_tvrzeni[0].fakty, ())

    def test_duplicate_fact_ids_are_rejected(self):
        def change(q):
            q["fakta"][1]["id"] = "f1"
        self.assertRejected(self.question_catalog(change), "q-1", "f1", "id")

    def test_question_with_another_commit_than_the_base_revision_is_rejected(self):
        def change(q):
            q["commit"] = "c" * 40
        self.assertRejected(self.question_catalog(change), "q-1", "commit", "base_revision")

    def test_unconfirmed_facts_are_rejected_for_measurement(self):
        for value in (False, "ano"):
            def change(q, value=value):
                q["fakta_potvrzena"] = value
            self.assertRejected(self.question_catalog(change), "q-1", "fakta_potvrzena")
        data = valid_catalog_2(valid_question())
        del data["cases"][0]["fakta_potvrzena"]
        self.assertRejected(data, "q-1", "fakta_potvrzena")

    def test_unconfirmed_pseudonymization_is_rejected_for_measurement(self):
        def change(q):
            q["pseudonymizace_potvrzena"] = False
        self.assertRejected(self.question_catalog(change), "q-1", "pseudonymizace_potvrzena")

    def test_changed_fact_text_without_a_new_confirmation_is_rejected(self):
        data = valid_catalog_2(valid_question())
        data["cases"][0]["fakta"][0]["text"] = "Kliknutí spustí jiný příkaz."
        self.assertRejected(data, "q-1", "otisk_potvrzeni")

    def test_changed_forbidden_claim_without_a_new_confirmation_is_rejected(self):
        data = valid_catalog_2(valid_question())
        data["cases"][0]["zakazana_tvrzeni"][0]["text"] = "Jiné tvrzení."
        self.assertRejected(data, "q-1", "otisk_potvrzeni")

    def test_malformed_fingerprint_is_rejected(self):
        data = valid_catalog_2(valid_question())
        data["cases"][0]["otisk_potvrzeni"] = "abc"
        self.assertRejected(data, "q-1", "otisk_potvrzeni")

    def test_fingerprint_follows_the_documented_definition(self):
        import hashlib
        canonical = '{"fakta":[{"id":"f1","text":"č"}],"zakazana_tvrzeni":[{"id":"z1"}]}'
        self.assertEqual(confirmation_fingerprint([{"id": "f1", "text": "č"}], [{"id": "z1"}]),
                         hashlib.sha256(canonical.encode("utf-8")).hexdigest())

    def test_fingerprint_ignores_key_order_but_not_content(self):
        a = confirmation_fingerprint([{"id": "f1", "text": "x"}], [])
        self.assertEqual(a, confirmation_fingerprint([{"text": "x", "id": "f1"}], []))
        self.assertNotEqual(a, confirmation_fingerprint([{"id": "f1", "text": "y"}], []))

    def test_question_ids_share_the_namespace_with_bug_cases(self):
        data = valid_catalog_2(dict(valid_case("same"), druh="chyba"), valid_question("same"))
        self.assertRejected(data, "same", "id")


class MemberNameTest(unittest.TestCase):
    def test_simple_name_is_the_last_segment_without_overload_or_generics(self):
        self.assertEqual(catalog.member_simple_name("LoadAction.SetScheduledTasksInvalid"),
                         "SetScheduledTasksInvalid")
        self.assertEqual(catalog.member_simple_name("Ns.Type.Run(+1)"), "Run")
        self.assertEqual(catalog.member_simple_name("Type.Map<T>(int)"), "Map")
        self.assertEqual(catalog.member_simple_name("Run"), "Run")

    def test_generic_type_qualifier_does_not_hide_the_member(self):
        self.assertEqual(catalog.member_simple_name("Repository<T>.Save"), "Save")
        self.assertEqual(catalog.member_simple_name("Ns.Repository<T>.Save(+1)"), "Save")
        self.assertEqual(catalog.member_simple_name("Map<K, List<V>>.Put"), "Put")
        self.assertEqual(catalog.member_simple_name("Map<K, List<V>>.Put<T>(int)"), "Put")


class SharedConstantsTest(unittest.TestCase):
    """Reader roles and item kinds are defined once; the other modules import them."""

    def test_reader_roles_come_from_report(self):
        import anchors
        import card_check
        import report
        self.assertEqual(report.ROLES, (report.ROLE_SUPPORT, report.ROLE_DEVELOPER))
        self.assertIs(card_check.ROLE_SUPPORT, report.ROLE_SUPPORT)
        self.assertIs(card_check.ROLE_DEVELOPER, report.ROLE_DEVELOPER)
        self.assertEqual(anchors.audiences(["karta"]), (report.ROLE_DEVELOPER, report.ROLE_SUPPORT))
        self.assertFalse(hasattr(anchors, "AUDIENCE_SUPPORT"))

    def test_fact_roles_and_item_kinds_come_from_catalog(self):
        import card_check
        import corpus
        self.assertEqual(catalog.ROLES, (catalog.ROLE_FRAMEWORK, catalog.ROLE_PLACE))
        self.assertEqual(set(card_check.FACT_ROLE_AUDIENCES), set(catalog.ROLES))
        self.assertIs(corpus.ROLE_FRAMEWORK, catalog.ROLE_FRAMEWORK)
        self.assertEqual((catalog.KIND_QUESTION, catalog.KIND_BUG), ("otazka", "chyba"))

    def test_no_module_compares_a_bare_role_or_kind_literal(self):
        tool = Path(__file__).resolve().parent.parent
        for name in ("report.py", "corpus.py", "card_check.py", "anchors.py"):
            source = (tool / name).read_text(encoding="utf-8")
            for literal in ('== "otazka"', '== "chyba"', '== "ramcova"', '== "misto"',
                            '= "podpora"', '= "vyvojar"'):
                if name == "report.py" and literal in ('= "podpora"', '= "vyvojar"'):
                    continue  # report.py is where the reader roles are defined
                self.assertNotIn(literal, source, f"{name}: {literal}")


if __name__ == "__main__":
    unittest.main()
