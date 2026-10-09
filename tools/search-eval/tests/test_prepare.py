"""Working tree export, case checks and corpus receipt (U2)."""
import hashlib
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import prepare
from catalog import parse_catalog
from prepare import PrepareError

FAKE_RA = [sys.executable, str(Path(__file__).resolve().parent / "fake_ra.py")]

GIT_ENV = {
    "GIT_AUTHOR_NAME": "Tester",
    "GIT_AUTHOR_EMAIL": "tester@example.invalid",
    "GIT_AUTHOR_DATE": "2026-01-01T00:00:00+00:00",
    "GIT_COMMITTER_NAME": "Tester",
    "GIT_COMMITTER_EMAIL": "tester@example.invalid",
    "GIT_COMMITTER_DATE": "2026-01-01T00:00:00+00:00",
    "GIT_CONFIG_GLOBAL": os.devnull,
    "GIT_CONFIG_NOSYSTEM": "1",
}

LOAD_ACTION_PRE = """public class LoadAction
{
    public void SetScheduledTasksInvalid() { }
}
"""
LOAD_ACTION_BASE = """public class LoadAction
{
    public void SetScheduledTasksInvalid() { }
    public void Reload() { SetScheduledTasksInvalid(); }
}
"""
LOAD_ACTION_FIX = """public class LoadAction
{
    public void SetScheduledTasksInvalid() { /* fixed */ }
    public void Reload() { SetScheduledTasksInvalid(); }
    public void RenamedLater() { }
}
"""


def git(repo: Path, *args: str) -> str:
    result = subprocess.run(
        ["git", "-c", "core.autocrlf=false", "-c", "commit.gpgsign=false",
         "-C", str(repo), *args],
        env={**os.environ, **GIT_ENV}, check=True, capture_output=True, text=True)
    return result.stdout.strip()


def write(root: Path, rel: str, text: str) -> None:
    path = root / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(text.encode("utf-8"))


def commit_all(repo: Path, message: str) -> str:
    git(repo, "add", "-A")
    git(repo, "commit", "-q", "-m", message)
    return git(repo, "rev-parse", "HEAD")


def spec_fingerprint(root: Path) -> str:
    """The fingerprint as prepare.py's docstring defines it, written independently."""
    rels = sorted(p.relative_to(root).as_posix() for p in root.rglob("*") if p.is_file())
    digest = hashlib.sha256()
    for rel in rels:
        data = (root / rel).read_bytes()
        digest.update(rel.encode("utf-8") + b"\0")
        digest.update(len(data).to_bytes(8, "big"))
        digest.update(data)
    return digest.hexdigest()


def make_case(case_id: str, file: str, member: str, fix: str) -> dict:
    return {
        "id": case_id,
        "prompt": "Zákazník A hlásí chybu.",
        "acceptable_locations": [{"file": file, "member": member, "line": 3, "fix_commit": fix}],
        "bug_confirmed_at_base": True,
        "pseudonymization_confirmed": True,
        "used_for_card_tuning": False,
        "prompt_names_location": False,
    }


class GitRepoFixture(unittest.TestCase):
    """One small repo with three commits: pre-base, base and a later fix."""

    @classmethod
    def setUpClass(cls):
        cls._tmp = tempfile.TemporaryDirectory(ignore_cleanup_errors=True)
        cls.root = Path(cls._tmp.name)
        cls.repo = cls.root / "source-repo"
        cls.repo.mkdir()
        git(cls.repo, "init", "-q", "-b", "main")
        write(cls.repo, "README.md", "Ukázkové repo.\n")
        write(cls.repo, "Src/Placeholder/LoadAction.cs", LOAD_ACTION_PRE)
        cls.pre = commit_all(cls.repo, "pre")
        write(cls.repo, "Src/Placeholder/LoadAction.cs", LOAD_ACTION_BASE)
        write(cls.repo, "Src/Placeholder/Scheduler.cs", "public class Scheduler { }\n")
        cls.base = commit_all(cls.repo, "base")
        write(cls.repo, "Src/Placeholder/LoadAction.cs", LOAD_ACTION_FIX)
        write(cls.repo, "Src/Placeholder/NewHelper.cs", "public class NewHelper { void Added() { } }\n")
        cls.fix = commit_all(cls.repo, "fix")

    @classmethod
    def tearDownClass(cls):
        cls._tmp.cleanup()

    def setUp(self):
        self._work = tempfile.TemporaryDirectory(ignore_cleanup_errors=True)
        self.work = Path(self._work.name)
        self.addCleanup(self._work.cleanup)

    def export(self, name: str = "tree") -> "prepare.ExportResult":
        return prepare.export_tree(self.repo, self.base, self.work / name)


class ExportTest(GitRepoFixture):
    def test_export_has_base_files_and_no_history(self):
        result = self.export()
        tree = result.tree_dir
        self.assertFalse((tree / ".git").exists())
        self.assertEqual((tree / "Src/Placeholder/LoadAction.cs").read_bytes(),
                         LOAD_ACTION_BASE.encode("utf-8"))
        self.assertTrue((tree / "Src/Placeholder/Scheduler.cs").is_file())
        self.assertTrue((tree / "README.md").is_file())
        # Nothing the later fix introduced may leak into the base tree.
        self.assertFalse((tree / "Src/Placeholder/NewHelper.cs").exists())
        self.assertEqual(result.revision, self.base)

    def test_tree_fingerprint_follows_the_documented_definition(self):
        result = self.export()
        self.assertEqual(result.tree_fingerprint, spec_fingerprint(result.tree_dir))
        self.assertEqual(prepare.fingerprint_dir(result.tree_dir), result.tree_fingerprint)

    def test_export_inside_the_source_repo_is_refused(self):
        with self.assertRaises(PrepareError):
            prepare.export_tree(self.repo, self.base, self.repo / "export")
        self.assertFalse((self.repo / "export").exists())

    def test_export_under_a_forbidden_directory_name_is_refused(self):
        dest = self.work / ".Agent-Workspace" / "tree"
        with self.assertRaises(PrepareError) as ctx:
            prepare.export_tree(self.repo, self.base, dest,
                                forbidden_names=(".agent-workspace",))
        self.assertIn(".agent-workspace", str(ctx.exception))
        self.assertFalse(dest.exists())

    def test_a_directory_name_is_only_forbidden_when_named(self):
        export = prepare.export_tree(self.repo, self.base, self.work / ".agent-workspace" / "tree")
        self.assertTrue(export.tree_dir.is_dir())

    def test_corpus_under_a_forbidden_directory_name_is_refused(self):
        export = self.export("tree")
        with self.assertRaises(PrepareError):
            prepare.generate_corpus(FAKE_RA, export, self.work / "ws" / "corpus",
                                    forbidden_names=("WS",))

    def test_export_inside_an_extra_forbidden_root_is_refused(self):
        forbidden = self.work / "protected"
        forbidden.mkdir()
        with self.assertRaises(PrepareError):
            prepare.export_tree(self.repo, self.base, forbidden / "tree",
                                forbidden_roots=[forbidden])

    def test_export_through_a_link_into_the_repo_is_refused(self):
        link = self.work / "link-to-repo"
        if not _make_dir_link(link, self.repo):
            self.skipTest("cannot create a directory symlink or junction here")
        # Remove the link before the temp dir is cleaned, so cleanup never walks into the repo.
        self.addCleanup(_remove_dir_link, link)
        with self.assertRaises(PrepareError):
            prepare.export_tree(self.repo, self.base, link / "export")
        self.assertFalse((self.repo / "export").exists())

    def test_export_into_a_non_empty_directory_is_refused(self):
        dest = self.work / "tree"
        dest.mkdir()
        (dest / "leftover.txt").write_text("x", encoding="utf-8")
        with self.assertRaises(PrepareError):
            prepare.export_tree(self.repo, self.base, dest)

    def test_unknown_revision_is_refused(self):
        with self.assertRaises(PrepareError):
            prepare.export_tree(self.repo, "f" * 40, self.work / "tree")


class CaseCheckTest(GitRepoFixture):
    def catalog(self, *cases: dict):
        return parse_catalog({"format_version": 1, "base_revision": self.base, "cases": list(cases)})

    def test_case_fixed_after_base_with_existing_member_passes(self):
        tree = self.export().tree_dir
        cat = self.catalog(make_case("ok", "Src/Placeholder/LoadAction.cs",
                                     "LoadAction.SetScheduledTasksInvalid", self.fix))
        self.assertEqual(prepare.find_case_problems(self.repo, cat, tree), [])
        prepare.check_cases(self.repo, cat, tree)

    def test_case_fixed_before_base_is_refused(self):
        tree = self.export().tree_dir
        cat = self.catalog(make_case("old", "Src/Placeholder/LoadAction.cs",
                                     "LoadAction.SetScheduledTasksInvalid", self.pre))
        with self.assertRaises(PrepareError) as ctx:
            prepare.check_cases(self.repo, cat, tree)
        self.assertIn("old", str(ctx.exception))

    def test_fix_commit_equal_to_base_is_refused(self):
        tree = self.export().tree_dir
        cat = self.catalog(make_case("same", "Src/Placeholder/LoadAction.cs",
                                     "LoadAction.SetScheduledTasksInvalid", self.base))
        problems = prepare.find_case_problems(self.repo, cat, tree)
        self.assertEqual(len(problems), 1)
        self.assertIn("same", problems[0])

    def test_unknown_fix_commit_is_refused(self):
        tree = self.export().tree_dir
        cat = self.catalog(make_case("ghost", "Src/Placeholder/LoadAction.cs",
                                     "LoadAction.SetScheduledTasksInvalid", "e" * 40))
        self.assertEqual(len(prepare.find_case_problems(self.repo, cat, tree)), 1)

    def test_member_added_after_base_is_refused(self):
        tree = self.export().tree_dir
        cat = self.catalog(make_case("later", "Src/Placeholder/LoadAction.cs",
                                     "LoadAction.RenamedLater", self.fix))
        with self.assertRaises(PrepareError) as ctx:
            prepare.check_cases(self.repo, cat, tree)
        self.assertIn("RenamedLater", str(ctx.exception))

    def test_file_added_after_base_is_refused(self):
        tree = self.export().tree_dir
        cat = self.catalog(make_case("newfile", "Src/Placeholder/NewHelper.cs",
                                     "NewHelper.Added", self.fix))
        with self.assertRaises(PrepareError) as ctx:
            prepare.check_cases(self.repo, cat, tree)
        self.assertIn("NewHelper.cs", str(ctx.exception))

    def test_member_match_respects_identifier_boundaries(self):
        tree = self.export().tree_dir
        cat = self.catalog(make_case("prefix", "Src/Placeholder/LoadAction.cs",
                                     "LoadAction.SetScheduledTasks", self.fix))
        self.assertEqual(len(prepare.find_case_problems(self.repo, cat, tree)), 1)

    def test_member_of_generic_type_is_searched_by_its_own_name(self):
        # `LoadAction` exists in the file; the member `Missing` does not.
        tree = self.export().tree_dir
        missing = self.catalog(make_case("generic", "Src/Placeholder/LoadAction.cs",
                                         "LoadAction<T>.Missing", self.fix))
        problems = prepare.find_case_problems(self.repo, missing, tree)
        self.assertEqual(len(problems), 1)
        self.assertIn("Missing", problems[0])
        present = self.catalog(make_case("generic", "Src/Placeholder/LoadAction.cs",
                                         "LoadAction<T>.SetScheduledTasksInvalid", self.fix))
        self.assertEqual(prepare.find_case_problems(self.repo, present, tree), [])

    def test_member_match_is_case_sensitive(self):
        tree = self.export().tree_dir
        cat = self.catalog(make_case("case", "Src/Placeholder/LoadAction.cs",
                                     "LoadAction.setscheduledtasksinvalid", self.fix))
        self.assertEqual(len(prepare.find_case_problems(self.repo, cat, tree)), 1)

    def test_catalog_for_another_base_revision_is_refused(self):
        tree = self.export().tree_dir
        cat = parse_catalog({"format_version": 1, "base_revision": self.pre, "cases": [
            make_case("ok", "Src/Placeholder/LoadAction.cs",
                      "LoadAction.SetScheduledTasksInvalid", self.fix)]})
        with self.assertRaises(PrepareError):
            prepare.check_cases(self.repo, cat, tree, base_revision=self.base)

    def test_all_problems_are_reported_together(self):
        tree = self.export().tree_dir
        cat = self.catalog(
            make_case("old", "Src/Placeholder/LoadAction.cs", "LoadAction.SetScheduledTasksInvalid", self.pre),
            make_case("later", "Src/Placeholder/LoadAction.cs", "LoadAction.RenamedLater", self.fix))
        problems = prepare.find_case_problems(self.repo, cat, tree)
        self.assertEqual(len(problems), 2)


class WorktreeProtectionTest(GitRepoFixture):
    """Every worktree of a protected repository is protected, not only the one named."""

    def add_worktree(self, name: str) -> Path:
        path = self.work / name
        git(self.repo, "worktree", "add", "-q", "--detach", str(path), self.base)
        self.addCleanup(subprocess.run, ["git", "-C", str(self.repo), "worktree", "remove",
                                         "--force", str(path)], capture_output=True)
        return path

    def assert_refused(self, path: Path, roots: list[str]) -> None:
        with self.assertRaises(PrepareError):
            prepare.ensure_outside(path, roots, "Kořen výstupu")

    def test_other_worktree_of_the_source_repo_is_refused(self):
        other = self.add_worktree("wt-other")
        self.assert_refused(other / "eval-out", prepare.protected_roots(self.repo))

    def test_main_checkout_is_refused_when_the_source_is_a_linked_worktree(self):
        linked = self.add_worktree("wt-linked")
        self.assert_refused(self.repo / "eval-out", prepare.protected_roots(linked))

    def test_other_worktree_of_the_runner_repo_is_refused(self):
        linked = self.add_worktree("wt-runner")
        plain_source = self.work / "plain-source"
        plain_source.mkdir()
        with mock.patch.object(prepare, "_find_runner_repo", return_value=str(linked)):
            roots = prepare.protected_roots(plain_source)
        self.assert_refused(self.repo / "eval-out", roots)

    def test_directory_outside_any_repo_keeps_the_plain_check(self):
        plain = self.work / "plain-source"
        plain.mkdir()
        with mock.patch.object(prepare, "_find_runner_repo", return_value=None):
            roots = prepare.protected_roots(plain)
        self.assertEqual(roots, [prepare.canonical_path(plain)])
        self.assertEqual(prepare.ensure_outside(self.work / "out", roots, "Kořen výstupu"),
                         Path(prepare.canonical_path(self.work / "out")))


class CorpusTest(GitRepoFixture):
    def test_generated_corpus_has_a_receipt_and_loads(self):
        export = self.export()
        corpus = self.work / "corpus"
        receipt = prepare.generate_corpus(FAKE_RA, export, corpus, options=["--lang", "cs"])
        self.assertTrue((corpus / "index.md").is_file())
        self.assertTrue(prepare.receipt_path(corpus).is_file())
        self.assertFalse(prepare.receipt_path(corpus).resolve().is_relative_to(corpus.resolve()))
        self.assertEqual(receipt.ra_version, "reverse-architect 0.0.0-fake")
        self.assertEqual(receipt.options, ("--lang", "cs"))
        self.assertEqual(receipt.tree_fingerprint, spec_fingerprint(export.tree_dir))
        self.assertEqual(receipt.corpus_fingerprint, spec_fingerprint(corpus))
        self.assertEqual(receipt.base_revision, self.base)
        self.assertEqual((corpus / "options.txt").read_text(encoding="utf-8"), "--lang cs\n")

        loaded = prepare.load_corpus(corpus, export.tree_fingerprint)
        self.assertEqual(loaded, receipt)

    def test_corpus_without_receipt_is_refused(self):
        export = self.export()
        corpus = self.work / "corpus"
        prepare.generate_corpus(FAKE_RA, export, corpus)
        prepare.receipt_path(corpus).unlink()
        with self.assertRaises(PrepareError):
            prepare.load_corpus(corpus, export.tree_fingerprint)

    def test_corpus_with_receipt_of_another_tree_is_refused(self):
        export = self.export()
        corpus = self.work / "corpus"
        prepare.generate_corpus(FAKE_RA, export, corpus)
        other = prepare.export_tree(self.repo, self.fix, self.work / "other-tree")
        self.assertNotEqual(other.tree_fingerprint, export.tree_fingerprint)
        with self.assertRaises(PrepareError):
            prepare.load_corpus(corpus, other.tree_fingerprint)

    def test_corpus_changed_after_its_receipt_is_refused(self):
        export = self.export()
        corpus = self.work / "corpus"
        prepare.generate_corpus(FAKE_RA, export, corpus)
        (corpus / "index.md").write_text("tampered\n", encoding="utf-8")
        with self.assertRaises(PrepareError):
            prepare.load_corpus(corpus, export.tree_fingerprint)

    def test_corpus_inside_the_tree_is_refused(self):
        export = self.export()
        with self.assertRaises(PrepareError):
            prepare.generate_corpus(FAKE_RA, export, export.tree_dir / "corpus")

    def test_corpus_inside_the_source_repo_is_refused(self):
        export = self.export()
        with self.assertRaises(PrepareError):
            prepare.generate_corpus(FAKE_RA, export, self.repo / "corpus")

    def test_options_that_redirect_or_mirror_output_are_refused(self):
        export = self.export()
        for bad in (["--mirror"], ["--out", str(self.work / "x")]):
            with self.assertRaises(PrepareError):
                prepare.generate_corpus(FAKE_RA, export, self.work / "corpus", options=bad)

    def test_failing_generator_is_reported(self):
        export = self.export()
        with self.assertRaises(PrepareError) as ctx:
            prepare.generate_corpus(FAKE_RA, export, self.work / "corpus", options=["--fail"])
        self.assertIn("simulated failure", str(ctx.exception))
        self.assertFalse(prepare.receipt_path(self.work / "corpus").exists())


def _make_dir_link(link: Path, target: Path) -> bool:
    """Create a directory symlink, or on Windows a junction when symlinks need privileges."""
    try:
        os.symlink(target, link, target_is_directory=True)
        return True
    except (OSError, NotImplementedError):
        pass
    if os.name == "nt":
        result = subprocess.run(["cmd", "/c", "mklink", "/J", str(link), str(target)],
                                capture_output=True)
        return result.returncode == 0 and link.exists()
    return False


def _remove_dir_link(link: Path) -> None:
    """Remove the link itself, never its target's contents."""
    if os.name == "nt":
        os.rmdir(link)
    else:
        os.unlink(link)


if __name__ == "__main__":
    unittest.main()
