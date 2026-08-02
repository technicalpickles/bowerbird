"""Tests for scripts/check-file-list.py (Epic 5 retro AI-3).

Two layers:

  1. Parser unit tests against the shapes real stories actually use. The
     fixtures here are lifted from Stories 5.6, 5.13, and 5.14, not invented:
     backticked paths with long annotations, bare paths with parenthetical
     notes, prose bullets that must NOT be read as files, dotfile paths, and
     the trailing-slash directory entries Story 5.13 used when it git mv-d
     whole cookbook trees.

  2. End-to-end runs of the script against throwaway git repos, covering the
     branch matrix:

       (a) File List matches git exactly        -> exit 0
       (b) changed file not declared            -> exit 1, listed as missing
       (c) declared file never changed          -> exit 1, listed as unchanged
       (d) untracked new file                   -> exit 1 (the omission class
                                                   this audit exists to catch)
       (e) no `### File List` section at all    -> exit 1, everything undeclared
       (f) directory entry covers files beneath -> exit 0
       (g) --ignore drops paths from the git side
       (h) missing story file                   -> exit 2 (tooling, not finding)
       (i) committed-on-branch changes counted against --base
       (j) rename reports both old and new path
       (k) --format json emits the machine shape

Two of these pin regressions found by running the parser against real story
files while writing it, both of which a synthetic-only fixture would have
missed: `lstrip("./")` silently ate the leading dot off `.github/workflows/
ci.yml`, and directory entries were reported as "declared but unchanged"
because git only ever names files.
"""

import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent.parent / "check-file-list.py"


def load_module():
    spec = importlib.util.spec_from_file_location("check_file_list", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


cfl = load_module()


def git(repo, *args):
    proc = subprocess.run(
        ["git"] + list(args),
        cwd=str(repo),
        capture_output=True,
        text=True,
        check=True,
    )
    return proc.stdout


def run_script(repo, *args):
    proc = subprocess.run(
        [sys.executable, str(SCRIPT)] + list(args),
        cwd=str(repo),
        capture_output=True,
        text=True,
        check=False,
    )
    return proc


def story_with(file_list_body, heading="### File List"):
    return (
        "# Story\n\n## Dev Agent Record\n\n"
        f"{heading}\n\n{file_list_body}\n\n"
        "## Change Log\n\n- something happened\n"
    )


class ParserTests(unittest.TestCase):
    def parse(self, body, **kwargs):
        paths, found = cfl.extract_file_list(story_with(body, **kwargs))
        return paths, found

    def test_bare_paths_with_parenthetical_annotations(self):
        # Story 5.14's shape, including a prose line before the list.
        body = (
            "Matches `git status --porcelain` (all modified):\n\n"
            "- INSTALL.md (one unpinned line, review-time scope adaptation)\n"
            "- README.md\n"
            "- docs/quickstart.md\n"
        )
        paths, found = self.parse(body)
        self.assertTrue(found)
        self.assertEqual(paths, ["INSTALL.md", "README.md", "docs/quickstart.md"])

    def test_backticked_paths_with_long_annotations(self):
        # Story 5.6's shape: backticks plus a long annotation containing
        # both backticks and parentheses.
        body = (
            "- `crates/daemon/src/projection/state.rs` (modified, `transition`"
            " Notification arm: `IdlePrompt` own arm (-> `Idle`))\n"
            "- `docs/protocol.md` (modified, `SessionCurrentState` definition)\n"
        )
        paths, _ = self.parse(body)
        self.assertEqual(
            paths,
            ["crates/daemon/src/projection/state.rs", "docs/protocol.md"],
        )

    def test_dotfile_path_keeps_its_leading_dot(self):
        # Regression: lstrip("./") turned this into github/workflows/ci.yml.
        paths, _ = self.parse("- .github/workflows/ci.yml\n")
        self.assertEqual(paths, [".github/workflows/ci.yml"])

    def test_directory_entry_keeps_trailing_slash(self):
        paths, _ = self.parse("- docs/cookbook/state-session-fanout/\n")
        self.assertEqual(paths, ["docs/cookbook/state-session-fanout/"])

    def test_prose_bullets_are_not_paths(self):
        # "Deleted" is a real bullet from Story 5.13's File List; it names a
        # subsection, not a file. Bare prose words must not survive the
        # looks_like_path filter that audit() applies.
        body = "- Deleted\n- Suite green\n- docs/protocol.md\n"
        paths, _ = self.parse(body)
        tracked = {"docs/protocol.md"}
        real = [p for p in paths if cfl.looks_like_path(p, tracked)]
        self.assertEqual(real, ["docs/protocol.md"])

    def test_extensionless_tracked_file_is_a_path(self):
        self.assertTrue(cfl.looks_like_path("Makefile", {"Makefile"}))
        self.assertFalse(cfl.looks_like_path("Deleted", {"Makefile"}))

    def test_bold_wrapper_is_stripped(self):
        paths, _ = self.parse("- **docs/protocol.md** (modified)\n")
        self.assertEqual(paths, ["docs/protocol.md"])

    def test_section_ends_at_next_same_level_heading(self):
        text = (
            "### File List\n\n- a/one.md\n\n"
            "### Change Log\n\n- b/two.md\n"
        )
        paths, found = cfl.extract_file_list(text)
        self.assertTrue(found)
        self.assertEqual(paths, ["a/one.md"])

    def test_deeper_heading_does_not_end_the_section(self):
        text = "## File List\n\n- a/one.md\n\n### Notes\n\n- b/two.md\n\n## Change Log\n"
        paths, _ = cfl.extract_file_list(text)
        self.assertEqual(paths, ["a/one.md", "b/two.md"])

    def test_missing_section_is_reported(self):
        paths, found = cfl.extract_file_list("# Story\n\n## Change Log\n\n- x\n")
        self.assertFalse(found)
        self.assertEqual(paths, [])

    def test_duplicates_collapse_in_source_order(self):
        paths, _ = self.parse("- a/one.md\n- b/two.md\n- a/one.md\n")
        self.assertEqual(paths, ["a/one.md", "b/two.md"])


class PorcelainTests(unittest.TestCase):
    def test_rename_reports_both_sides(self):
        changes = cfl.parse_porcelain("R  old/path.md -> new/path.md\n")
        self.assertEqual(set(changes), {"old/path.md", "new/path.md"})

    def test_untracked_and_deleted_labels(self):
        changes = cfl.parse_porcelain("?? new.md\n D gone.md\n M edit.md\n")
        self.assertEqual(changes["new.md"], "untracked")
        self.assertEqual(changes["gone.md"], "deleted")
        self.assertEqual(changes["edit.md"], "modified")

    def test_quoted_path_is_unwrapped(self):
        changes = cfl.parse_porcelain('?? "with space.md"\n')
        self.assertIn("with space.md", changes)


class EndToEndTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.repo = Path(self.tmp.name)
        self.addCleanup(self.tmp.cleanup)
        git(self.repo, "init", "-q", "-b", "main")
        git(self.repo, "config", "user.email", "test@example.com")
        git(self.repo, "config", "user.name", "Test")
        (self.repo / "docs").mkdir()
        (self.repo / "seed.md").write_text("seed\n")
        git(self.repo, "add", "-A")
        git(self.repo, "commit", "-qm", "seed")

    def write_story(self, body, name="story.md"):
        path = self.repo / name
        path.write_text(story_with(body))
        return path

    def test_clean_match_exits_zero(self):
        (self.repo / "docs" / "a.md").write_text("a\n")
        story = self.write_story("- docs/a.md\n- story.md\n")
        proc = run_script(self.repo, str(story))
        self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
        self.assertIn("CLEAN", proc.stdout)

    def test_changed_but_undeclared_is_flagged(self):
        (self.repo / "docs" / "a.md").write_text("a\n")
        (self.repo / "docs" / "sneaky.md").write_text("side effect\n")
        story = self.write_story("- docs/a.md\n- story.md\n")
        proc = run_script(self.repo, str(story))
        self.assertEqual(proc.returncode, 1)
        self.assertIn("MISSING FROM FILE LIST", proc.stdout)
        self.assertIn("docs/sneaky.md", proc.stdout)

    def test_declared_but_unchanged_is_flagged(self):
        story = self.write_story("- docs/never-touched.md\n- story.md\n")
        proc = run_script(self.repo, str(story))
        self.assertEqual(proc.returncode, 1)
        self.assertIn("DECLARED BUT UNCHANGED", proc.stdout)
        self.assertIn("docs/never-touched.md", proc.stdout)

    def test_untracked_file_counts_as_changed(self):
        (self.repo / "brand-new.md").write_text("new\n")
        story = self.write_story("- story.md\n")
        proc = run_script(self.repo, str(story))
        self.assertEqual(proc.returncode, 1)
        self.assertIn("brand-new.md", proc.stdout)
        self.assertIn("untracked", proc.stdout)

    def test_no_file_list_section_reports_everything_undeclared(self):
        path = self.repo / "story.md"
        path.write_text("# Story\n\n## Change Log\n\n- nothing\n")
        proc = run_script(self.repo, str(path))
        self.assertEqual(proc.returncode, 1)
        self.assertIn("NO `### File List` SECTION FOUND", proc.stdout)
        self.assertIn("story.md", proc.stdout)

    def test_directory_entry_covers_files_beneath_it(self):
        nested = self.repo / "docs" / "cookbook" / "fanout"
        nested.mkdir(parents=True)
        (nested / "README.md").write_text("x\n")
        (nested / "index.ts").write_text("y\n")
        story = self.write_story("- docs/cookbook/fanout/\n- story.md\n")
        proc = run_script(self.repo, str(story))
        self.assertEqual(proc.returncode, 0, proc.stdout)
        self.assertIn("CLEAN", proc.stdout)

    def test_new_directory_is_expanded_to_individual_files(self):
        # Regression: plain `git status --porcelain` collapses an untracked
        # tree into one `docs/cookbook/` line, so a new cookbook entry would
        # be reported as a single directory and its files would never be
        # named. --untracked-files=all is what makes this audit useful for
        # the exact story shape Epic 6 keeps repeating.
        nested = self.repo / "docs" / "cookbook" / "glance"
        nested.mkdir(parents=True)
        (nested / "README.md").write_text("x\n")
        (nested / "index.ts").write_text("y\n")
        story = self.write_story("- story.md\n")
        proc = run_script(self.repo, str(story), "--format", "json")
        missing = json.loads(proc.stdout)["missing_from_file_list"]
        self.assertIn("docs/cookbook/glance/README.md", missing)
        self.assertIn("docs/cookbook/glance/index.ts", missing)
        self.assertNotIn("docs/cookbook/", missing)

    def test_ignore_glob_drops_paths_from_the_git_side(self):
        (self.repo / "noise.tmp").write_text("junk\n")
        story = self.write_story("- story.md\n")
        proc = run_script(self.repo, str(story), "--ignore", "*.tmp")
        self.assertEqual(proc.returncode, 0, proc.stdout)
        self.assertIn("ignored by --ignore: 1", proc.stdout)

    def test_missing_story_file_is_tooling_error(self):
        proc = run_script(self.repo, "does-not-exist.md")
        self.assertEqual(proc.returncode, 2)
        self.assertIn("story file not found", proc.stderr)

    def test_committed_branch_changes_counted_against_base(self):
        git(self.repo, "checkout", "-qb", "feature")
        (self.repo / "docs" / "committed.md").write_text("c\n")
        story = self.write_story("- story.md\n")
        git(self.repo, "add", "-A")
        git(self.repo, "commit", "-qm", "work")
        # Working tree is now clean; only the branch diff can surface this.
        proc = run_script(self.repo, "story.md", "--base", "main")
        self.assertEqual(proc.returncode, 1)
        self.assertIn("docs/committed.md", proc.stdout)
        self.assertIn("committed on branch", proc.stdout)

    def test_rename_surfaces_both_paths(self):
        git(self.repo, "mv", "seed.md", "renamed.md")
        story = self.write_story("- story.md\n")
        proc = run_script(self.repo, str(story))
        self.assertEqual(proc.returncode, 1)
        self.assertIn("seed.md", proc.stdout)
        self.assertIn("renamed.md", proc.stdout)

    def test_json_format_shape(self):
        (self.repo / "docs" / "a.md").write_text("a\n")
        story = self.write_story("- story.md\n")
        proc = run_script(self.repo, str(story), "--format", "json")
        self.assertEqual(proc.returncode, 1)
        payload = json.loads(proc.stdout)
        self.assertIn("docs/a.md", payload["missing_from_file_list"])
        self.assertFalse(payload["clean"])
        self.assertTrue(payload["section_found"])

    def test_empty_base_audits_working_tree_only(self):
        git(self.repo, "checkout", "-qb", "feature")
        (self.repo / "docs" / "committed.md").write_text("c\n")
        git(self.repo, "add", "-A")
        git(self.repo, "commit", "-qm", "work")
        story = self.write_story("- story.md\n")
        proc = run_script(self.repo, "story.md", "--base", "")
        # story.md is untracked and declared; the committed file is out of scope.
        self.assertNotIn("docs/committed.md", proc.stdout)


if __name__ == "__main__":
    unittest.main()
