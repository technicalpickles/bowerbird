import pytest
import yaml
from pathlib import Path

# Import will fail until bmad-flow.py exists - that's expected
import importlib.util, sys

def load_module():
    spec = importlib.util.spec_from_file_location(
        "bmad_flow",
        Path(__file__).parent.parent / "bmad-flow.py"
    )
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod

@pytest.fixture(scope="module")
def m():
    return load_module()


class TestKeyClassification:
    def test_is_epic_key_plain(self, m):
        assert m.is_epic_key("epic-1") is True

    def test_is_epic_key_excludes_retrospective(self, m):
        assert m.is_epic_key("epic-1-retrospective") is False

    def test_is_story_key_typical(self, m):
        assert m.is_story_key("1-2-account-management") is True

    def test_is_story_key_excludes_epic(self, m):
        assert m.is_story_key("epic-1") is False

    def test_get_story_epic_num(self, m):
        assert m.get_story_epic_num("1-2-account-management") == "1"
        assert m.get_story_epic_num("3-4-some-story") == "3"


class TestFindStoryFile:
    def test_finds_by_prefix(self, m, tmp_path):
        (tmp_path / "1-2-account-management.md").write_text("# Story")
        result = m.find_story_file("1-2-account-mgmt", tmp_path)
        assert result is not None
        assert result.name == "1-2-account-management.md"

    def test_returns_none_when_missing(self, m, tmp_path):
        result = m.find_story_file("9-9-nonexistent", tmp_path)
        assert result is None

    def test_returns_none_when_dir_missing(self, m, tmp_path):
        result = m.find_story_file("1-1-story", tmp_path / "nonexistent")
        assert result is None


class TestDeterminePosition:
    def _dev_status(self, fixture_name):
        p = Path("tests/fixtures") / fixture_name
        data = yaml.safe_load(p.read_text())
        return data.get("development_status", {})

    def test_empty_returns_needs_sprint_planning(self, m, tmp_path):
        pos = m.determine_position({}, tmp_path)
        assert pos["step"] == "needs_sprint_planning"

    def test_backlog_story_no_file_returns_needs_story_creation(self, m, tmp_path):
        dev_status = {"epic-1": "in-progress", "1-1-story": "backlog"}
        pos = m.determine_position(dev_status, tmp_path)
        assert pos["step"] == "needs_story_creation"
        assert pos["story"] == "1-1-story"
        assert pos["epic"] == "epic-1"

    def test_ready_for_dev_returns_needs_validation(self, m, tmp_path):
        dev_status = {"epic-1": "in-progress", "1-1-story": "ready-for-dev"}
        pos = m.determine_position(dev_status, tmp_path)
        assert pos["step"] == "needs_validation"

    def test_in_progress_returns_needs_dev(self, m, tmp_path):
        dev_status = {"epic-1": "in-progress", "1-1-story": "in-progress"}
        pos = m.determine_position(dev_status, tmp_path)
        assert pos["step"] == "needs_dev"

    def test_review_returns_needs_review(self, m, tmp_path):
        dev_status = {"epic-1": "in-progress", "1-1-story": "review"}
        pos = m.determine_position(dev_status, tmp_path)
        assert pos["step"] == "needs_review"

    def test_all_stories_done_returns_epic_complete(self, m, tmp_path):
        dev_status = {
            "epic-1": "in-progress",
            "1-1-story": "done",
            "1-2-story": "done",
            "epic-1-retrospective": "optional",
        }
        pos = m.determine_position(dev_status, tmp_path)
        assert pos["step"] == "epic_complete"
        assert pos["epic"] == "epic-1"

    def test_all_epics_done_returns_all_done(self, m, tmp_path):
        data = yaml.safe_load(Path("tests/fixtures/sprint_status_done.yaml").read_text())
        pos = m.determine_position(data["development_status"], tmp_path)
        assert pos["step"] == "all_done"

    def test_skips_done_epic_to_find_next(self, m, tmp_path):
        dev_status = {
            "epic-1": "done",
            "1-1-story": "done",
            "epic-2": "in-progress",
            "2-1-story": "backlog",
        }
        pos = m.determine_position(dev_status, tmp_path)
        assert pos["step"] == "needs_story_creation"
        assert pos["epic"] == "epic-2"

    def test_mid_fixture_routes_to_needs_validation(self, m, tmp_path):
        data = yaml.safe_load(Path("tests/fixtures/sprint_status_mid.yaml").read_text())
        # 1-1 done, 1-2 ready-for-dev -> needs_validation
        pos = m.determine_position(data["development_status"], tmp_path)
        assert pos["step"] == "needs_validation"
        assert pos["story"] == "1-2-account-mgmt"
