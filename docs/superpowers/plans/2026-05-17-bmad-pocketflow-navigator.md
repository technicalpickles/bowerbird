# BMAD PocketFlow Navigator Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `bmad-flow.py`, a standalone PocketFlow script that reads a BMAD project's Phase 4 sprint state and prints the current status plus the exact next skill to run.

**Architecture:** A single PocketFlow graph: `ReadProjectState` reads and parses `sprint-status.yaml` (falling back to artifact detection) and returns an action string; `StepAdvisor` (one instance, reachable by all actions) formats and prints the status tree and next-step command. The flow terminates after one pass.

**Tech Stack:** Python 3.10+, [pocketflow](https://pypi.org/project/pocketflow/), pyyaml, pytest

---

## File Structure

| File | Purpose |
|------|---------|
| `bmad-flow.py` | Complete script: helpers, state logic, PocketFlow nodes, flow wiring, CLI |
| `tests/test_bmad_flow.py` | All unit and integration tests |
| `tests/fixtures/sprint_status_mid.yaml` | Fixture: epic in-progress, mixed story statuses |
| `tests/fixtures/sprint_status_done.yaml` | Fixture: all epics done |

---

## Task 1: scaffold-helpers

**Files:**
- Create: `bmad-flow.py`
- Create: `tests/__init__.py`
- Create: `tests/test_bmad_flow.py`

### Setup

- [ ] **Step 1: Install dependencies**

```bash
pip install pocketflow pyyaml pytest
```

Expected: no errors.

- [ ] **Step 2: Create `tests/__init__.py` (empty)**

```bash
touch tests/__init__.py
```

- [ ] **Step 3: Write failing tests for key classification helpers**

Create `tests/test_bmad_flow.py`:

```python
import pytest
import yaml
from pathlib import Path

# Import will fail until bmad-flow.py exists - that's expected
import importlib.util, sys

def load_module():
    spec = importlib.util.spec_from_file_location("bmad_flow", "bmad-flow.py")
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
```

- [ ] **Step 4: Run tests to verify they fail with ImportError**

```bash
pytest tests/test_bmad_flow.py -v 2>&1 | head -30
```

Expected: errors importing `bmad_flow` (file doesn't exist yet).

- [ ] **Step 5: Create `bmad-flow.py` with helpers**

```python
#!/usr/bin/env python3
"""BMAD Phase 4 status navigator using PocketFlow."""

import argparse
import sys
from pathlib import Path

import yaml
from pocketflow import Flow, Node


# ---------------------------------------------------------------------------
# Key classification helpers
# ---------------------------------------------------------------------------

def is_epic_key(key: str) -> bool:
    return key.startswith("epic-") and not key.endswith("-retrospective")


def is_story_key(key: str) -> bool:
    return bool(key) and key[0].isdigit()


def get_story_epic_num(story_key: str) -> str:
    """'1-2-account-mgmt' -> '1'"""
    return story_key.split("-")[0]


def find_story_file(story_key: str, story_dir: Path) -> Path | None:
    """Find the story .md file matching the given story key prefix."""
    if not story_dir.exists():
        return None
    prefix = "-".join(story_key.split("-")[:2])  # "1-2"
    for f in sorted(story_dir.glob(f"{prefix}-*.md")):
        return f
    exact = story_dir / f"{story_key}.md"
    return exact if exact.exists() else None
```

- [ ] **Step 6: Run tests to verify they pass**

```bash
pytest tests/test_bmad_flow.py::TestKeyClassification tests/test_bmad_flow.py::TestFindStoryFile -v
```

Expected: all 7 tests PASS.

- [ ] **Step 7: Commit**

```bash
git add bmad-flow.py tests/__init__.py tests/test_bmad_flow.py
git commit -m "feat(bmad-flow): scaffold with key classification helpers and tests"
```

---

## Task 2: state-determination

**Files:**
- Modify: `bmad-flow.py` (add config loading, sprint status parsing, determine_position)
- Modify: `tests/test_bmad_flow.py` (add state determination tests)
- Create: `tests/fixtures/sprint_status_mid.yaml`
- Create: `tests/fixtures/sprint_status_done.yaml`

### Fixtures

- [ ] **Step 1: Create `tests/fixtures/sprint_status_mid.yaml`**

```yaml
project: test-project
story_location: docs/bmad/implementation-artifacts

development_status:
  epic-1: in-progress
  1-1-user-auth: done
  1-2-account-mgmt: ready-for-dev
  1-3-password-reset: backlog
  epic-1-retrospective: optional
  epic-2: backlog
  2-1-product-catalog: backlog
```

- [ ] **Step 2: Create `tests/fixtures/sprint_status_done.yaml`**

```yaml
project: test-project
story_location: docs/bmad/implementation-artifacts

development_status:
  epic-1: done
  1-1-user-auth: done
  epic-1-retrospective: optional
  epic-2: done
  2-1-product-catalog: done
  epic-2-retrospective: optional
```

- [ ] **Step 3: Write failing tests for state determination**

Append to `tests/test_bmad_flow.py`:

```python
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
```

- [ ] **Step 4: Run tests to verify they fail**

```bash
pytest tests/test_bmad_flow.py::TestDeterminePosition -v 2>&1 | head -20
```

Expected: AttributeError - `determine_position` not defined yet.

- [ ] **Step 5: Add config loading and determine_position to `bmad-flow.py`**

Append to `bmad-flow.py` after the helpers section:

```python
# ---------------------------------------------------------------------------
# Config and sprint status loading
# ---------------------------------------------------------------------------

STORY_STEP_MAP = {
    "backlog":       "needs_story_creation",
    "ready-for-dev": "needs_validation",
    "in-progress":   "needs_dev",
    "review":        "needs_review",
}


def load_bmad_config(project_root: Path) -> dict:
    config_path = project_root / "_bmad" / "bmm" / "config.yaml"
    if config_path.exists():
        return yaml.safe_load(config_path.read_text()) or {}
    return {}


def load_sprint_status(impl_path: Path) -> dict | None:
    sprint_file = impl_path / "sprint-status.yaml"
    if sprint_file.exists():
        return yaml.safe_load(sprint_file.read_text()) or {}
    return None


def resolve_story_dir(project_root: Path, sprint_status: dict | None, bmad_config: dict) -> Path:
    """Resolve where individual story .md files live."""
    if sprint_status and sprint_status.get("story_location"):
        loc = sprint_status["story_location"].replace("//", "/").strip("/")
        return project_root / loc
    impl = bmad_config.get("implementation_artifacts", "docs/bmad/implementation-artifacts")
    return project_root / impl


# ---------------------------------------------------------------------------
# State determination
# ---------------------------------------------------------------------------

def determine_position(dev_status: dict, story_dir: Path) -> dict:
    """
    Inspect dev_status dict and story_dir to determine current Phase 4 position.
    Returns: {step, epic, story, story_status}
    """
    if not dev_status:
        return dict(step="needs_sprint_planning", epic=None, story=None, story_status=None)

    # Find first non-done epic
    current_epic = None
    for key, status in dev_status.items():
        if is_epic_key(key) and status != "done":
            current_epic = key
            break

    if current_epic is None:
        return dict(step="all_done", epic=None, story=None, story_status=None)

    epic_num = current_epic.split("-")[1]  # "epic-1" -> "1"

    # Find first non-done story in this epic
    for key, status in dev_status.items():
        if is_story_key(key) and get_story_epic_num(key) == epic_num and status != "done":
            step = STORY_STEP_MAP.get(status, "needs_dev")
            return dict(step=step, epic=current_epic, story=key, story_status=status)

    # All stories in this epic are done
    return dict(step="epic_complete", epic=current_epic, story=None, story_status=None)
```

- [ ] **Step 6: Run tests to verify they pass**

```bash
pytest tests/test_bmad_flow.py::TestDeterminePosition -v
```

Expected: all 9 tests PASS.

- [ ] **Step 7: Commit**

```bash
git add bmad-flow.py tests/test_bmad_flow.py tests/fixtures/sprint_status_mid.yaml tests/fixtures/sprint_status_done.yaml
git commit -m "feat(bmad-flow): add sprint status parsing and position determination"
```

---

## Task 3: output-formatting

**Files:**
- Modify: `bmad-flow.py` (add format_output)
- Modify: `tests/test_bmad_flow.py` (add output formatting tests)

- [ ] **Step 1: Write failing tests for output formatting**

Append to `tests/test_bmad_flow.py`:

```python
class TestFormatOutput:
    def _mid_fixtures(self):
        import yaml
        data = yaml.safe_load(Path("tests/fixtures/sprint_status_mid.yaml").read_text())
        return data

    def test_contains_project_name(self, m):
        data = self._mid_fixtures()
        position = {"step": "needs_validation", "epic": "epic-1", "story": "1-2-account-mgmt", "story_status": "ready-for-dev"}
        output = m.format_output(data, position, story_file=None)
        assert "test-project" in output

    def test_contains_next_step_label(self, m):
        data = self._mid_fixtures()
        position = {"step": "needs_validation", "epic": "epic-1", "story": "1-2-account-mgmt", "story_status": "ready-for-dev"}
        output = m.format_output(data, position, story_file=None)
        assert "Validate Story" in output

    def test_contains_skill_command(self, m):
        data = self._mid_fixtures()
        position = {"step": "needs_review", "epic": "epic-1", "story": "1-2-account-mgmt", "story_status": "review"}
        output = m.format_output(data, position, story_file=None)
        assert "/bmad-code-review" in output

    def test_current_story_marked_with_arrow(self, m):
        data = self._mid_fixtures()
        position = {"step": "needs_dev", "epic": "epic-1", "story": "1-2-account-mgmt", "story_status": "in-progress"}
        output = m.format_output(data, position, story_file=None)
        lines = output.splitlines()
        story_line = next(l for l in lines if "1-2-account-mgmt" in l)
        assert "→" in story_line

    def test_done_story_marked_with_checkmark(self, m):
        data = self._mid_fixtures()
        position = {"step": "needs_validation", "epic": "epic-1", "story": "1-2-account-mgmt", "story_status": "ready-for-dev"}
        output = m.format_output(data, position, story_file=None)
        lines = output.splitlines()
        done_line = next(l for l in lines if "1-1-user-auth" in l)
        assert "✓" in done_line

    def test_story_file_path_shown_when_provided(self, m, tmp_path):
        data = self._mid_fixtures()
        position = {"step": "needs_dev", "epic": "epic-1", "story": "1-2-account-mgmt", "story_status": "in-progress"}
        fake_file = tmp_path / "1-2-account-mgmt.md"
        output = m.format_output(data, position, story_file=fake_file)
        assert str(fake_file) in output

    def test_all_done_shows_completion_message(self, m):
        data = yaml.safe_load(Path("tests/fixtures/sprint_status_done.yaml").read_text())
        position = {"step": "all_done", "epic": None, "story": None, "story_status": None}
        output = m.format_output(data, position, story_file=None)
        assert "Sprint done" in output
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
pytest tests/test_bmad_flow.py::TestFormatOutput -v 2>&1 | head -20
```

Expected: AttributeError - `format_output` not defined.

- [ ] **Step 3: Add formatting constants and `format_output` to `bmad-flow.py`**

Append to `bmad-flow.py` after the state determination section:

```python
# ---------------------------------------------------------------------------
# Output formatting
# ---------------------------------------------------------------------------

STATUS_ICONS = {
    "done":          "✓",
    "in-progress":   "→",
    "ready-for-dev": "→",
    "review":        "→",
    "backlog":       "○",
}

STEP_LABELS = {
    "needs_sprint_planning": "Sprint Planning",
    "needs_story_creation":  "Create Story",
    "needs_validation":      "Validate Story",
    "needs_dev":             "Dev Story",
    "needs_review":          "Code Review",
    "epic_complete":         "Retrospective",
    "all_done":              "Complete",
}

SKILL_COMMANDS = {
    "needs_sprint_planning": "/bmad-sprint-planning",
    "needs_story_creation":  "/bmad-create-story",
    "needs_validation":      "/bmad-create-story:validate",
    "needs_dev":             "/bmad-dev-story",
    "needs_review":          "/bmad-code-review",
    "epic_complete":         "/bmad-retrospective",
    "all_done":              None,
}


def format_output(sprint_status: dict, position: dict, story_file: Path | None) -> str:
    project_name = (sprint_status or {}).get("project", "Unknown")
    dev_status = (sprint_status or {}).get("development_status", {})
    step = position["step"]
    current_story = position.get("story")

    lines = [
        "BMAD Phase 4 - Sprint Status",
        "═" * 32,
        "",
        f"Project: {project_name}",
        "",
    ]

    for key, status in dev_status.items():
        if is_epic_key(key):
            lines.append(f"{key}  [{status}]")
        elif is_story_key(key):
            if key == current_story:
                icon = "→"
                note = f"  ({STEP_LABELS.get(step, step)})"
            else:
                icon = STATUS_ICONS.get(status, "○")
                note = ""
            lines.append(f"  {icon}  {key}{note}")

    lines += ["", "═" * 32, f"Next: {STEP_LABELS.get(step, step)}", ""]

    if story_file:
        lines += [f"Story file: {story_file}", ""]

    cmd = SKILL_COMMANDS.get(step)
    lines.append(f"Run: {cmd}" if cmd else "All epics complete. Sprint done.")

    return "\n".join(lines)
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
pytest tests/test_bmad_flow.py::TestFormatOutput -v
```

Expected: all 7 tests PASS.

- [ ] **Step 5: Run all tests so far**

```bash
pytest tests/test_bmad_flow.py -v
```

Expected: all 23 tests PASS.

- [ ] **Step 6: Commit**

```bash
git add bmad-flow.py tests/test_bmad_flow.py
git commit -m "feat(bmad-flow): add output formatter with status tree and skill commands"
```

---

## Task 4: nodes-wiring-cli

**Files:**
- Modify: `bmad-flow.py` (add PocketFlow nodes, `build_flow`, `main`)
- Modify: `tests/test_bmad_flow.py` (add node and integration tests)

- [ ] **Step 1: Write failing tests for `ReadProjectState` and integration**

Append to `tests/test_bmad_flow.py`:

```python
class TestReadProjectState:
    def test_routes_to_needs_story_creation_for_bowerbird(self, m):
        """Integration: run against the actual bowerbird project."""
        project_root = Path(".")
        shared = {"project_root": project_root}
        node = m.ReadProjectState()
        action = node._run(shared)
        # bowerbird has 1-1 done, 1-2 backlog -> needs_story_creation
        assert action == "needs_story_creation"
        assert shared["current_position"]["epic"] == "epic-1"
        assert shared["current_position"]["story"] == "1-2-daemon-foundation-with-sqlite-persistence"

    def test_populates_sprint_status_in_shared(self, m):
        project_root = Path(".")
        shared = {"project_root": project_root}
        node = m.ReadProjectState()
        node._run(shared)
        assert shared["sprint_status"] is not None
        assert "development_status" in shared["sprint_status"]

    def test_missing_bmad_dir_does_not_crash(self, m, tmp_path):
        """A dir with no _bmad still returns needs_sprint_planning."""
        shared = {"project_root": tmp_path}
        node = m.ReadProjectState()
        action = node._run(shared)
        assert action == "needs_sprint_planning"


class TestFullFlow:
    def test_flow_runs_to_completion_on_bowerbird(self, m, capsys):
        """Full end-to-end: flow prints output and exits cleanly."""
        project_root = Path(".")
        shared = {"project_root": project_root}
        flow = m.build_flow()
        flow.run(shared)
        captured = capsys.readouterr()
        assert "BMAD Phase 4" in captured.out
        assert "bowerbird" in captured.out.lower()
        assert "Run:" in captured.out
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
pytest tests/test_bmad_flow.py::TestReadProjectState tests/test_bmad_flow.py::TestFullFlow -v 2>&1 | head -20
```

Expected: AttributeError - `ReadProjectState` and `build_flow` not defined.

- [ ] **Step 3: Add PocketFlow nodes, `build_flow`, and `main` to `bmad-flow.py`**

Append to `bmad-flow.py`:

```python
# ---------------------------------------------------------------------------
# PocketFlow nodes
# ---------------------------------------------------------------------------

class ReadProjectState(Node):
    def prep(self, shared):
        return shared["project_root"]

    def exec(self, project_root: Path):
        bmad_config = load_bmad_config(project_root)
        impl_path = project_root / bmad_config.get(
            "implementation_artifacts", "docs/bmad/implementation-artifacts"
        )
        sprint_status = load_sprint_status(impl_path)
        story_dir = resolve_story_dir(project_root, sprint_status, bmad_config)

        dev_status = (sprint_status or {}).get("development_status", {})
        position = determine_position(dev_status, story_dir)

        story_file = None
        if position.get("story"):
            story_file = find_story_file(position["story"], story_dir)

        return sprint_status, position, story_file

    def post(self, shared, prep_res, exec_res):
        sprint_status, position, story_file = exec_res
        shared["sprint_status"] = sprint_status
        shared["current_position"] = position
        shared["story_file"] = story_file
        return position["step"]


class StepAdvisor(Node):
    def prep(self, shared):
        return shared["sprint_status"], shared["current_position"], shared["story_file"]

    def exec(self, prep_res):
        sprint_status, position, story_file = prep_res
        return format_output(sprint_status or {}, position, story_file)

    def post(self, shared, prep_res, exec_res):
        print(exec_res)
        return None  # terminal


# ---------------------------------------------------------------------------
# Flow construction
# ---------------------------------------------------------------------------

def build_flow() -> Flow:
    read_state = ReadProjectState()
    advisor = StepAdvisor()

    for action in SKILL_COMMANDS:
        read_state - action >> advisor

    return Flow(start=read_state)


# ---------------------------------------------------------------------------
# CLI entry point
# ---------------------------------------------------------------------------

def main():
    parser = argparse.ArgumentParser(description="BMAD Phase 4 status navigator")
    parser.add_argument("--path", default=".", help="Path to BMAD project root")
    args = parser.parse_args()

    project_root = Path(args.path).resolve()
    if not project_root.exists():
        print(f"Error: {project_root} does not exist", file=sys.stderr)
        sys.exit(1)

    shared = {"project_root": project_root}
    build_flow().run(shared)


if __name__ == "__main__":
    main()
```

- [ ] **Step 4: Run tests to verify node and flow tests pass**

```bash
pytest tests/test_bmad_flow.py::TestReadProjectState tests/test_bmad_flow.py::TestFullFlow -v
```

Expected: all 4 tests PASS.

- [ ] **Step 5: Run full test suite**

```bash
pytest tests/test_bmad_flow.py -v
```

Expected: all 27 tests PASS.

- [ ] **Step 6: Smoke test against bowerbird directly**

```bash
python bmad-flow.py
```

Expected output resembles:
```
BMAD Phase 4 - Sprint Status
════════════════════════════

Project: bowerbird

epic-1  [in-progress]
  ✓  1-1-workspace-and-protocol-crate-foundation
  →  1-2-daemon-foundation-with-sqlite-persistence  (Create Story)
  ○  1-3-unix-socket-ingest-endpoint
  ...

════════════════════════════
Next: Create Story

Run: /bmad-create-story
```

- [ ] **Step 7: Commit**

```bash
git add bmad-flow.py tests/test_bmad_flow.py
git commit -m "feat(bmad-flow): add PocketFlow nodes, flow wiring, and CLI entry point"
```
