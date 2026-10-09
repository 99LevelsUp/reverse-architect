"""Catalog format and validation (U1: R1, R2, R3, R13, R16)."""
import copy
import json
import tempfile
import unittest
from pathlib import Path

import catalog
from catalog import CatalogError, load_catalog, parse_catalog

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


class ExampleCatalogTest(unittest.TestCase):
    def test_example_catalog_passes_validation(self):
        cat = load_catalog(EXAMPLE)
        self.assertGreaterEqual(len(cat.cases), 2)
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
        for case in load_catalog(EXAMPLE).cases:
            self.assertIn("Ukázkové zadání", case.prompt)


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
        data = valid_catalog()
        data["format_version"] = 2
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


if __name__ == "__main__":
    unittest.main()
