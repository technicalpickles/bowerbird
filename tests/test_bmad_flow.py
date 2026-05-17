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
