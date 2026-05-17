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
    if not key:
        return False
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


# ---------------------------------------------------------------------------
# PocketFlow nodes
# ---------------------------------------------------------------------------

class ReadProjectState(Node):
    def prep(self, shared):
        return shared["project_root"]

    def exec(self, project_root: Path):
        bmad_config = load_bmad_config(project_root)
        raw_impl = bmad_config.get(
            "implementation_artifacts", "docs/bmad/implementation-artifacts"
        )
        # Config values may use '{project-root}/' as a template prefix; strip it.
        impl_rel = raw_impl.replace("{project-root}/", "").replace("{project-root}", "").replace("//", "/").strip("/")
        impl_path = project_root / impl_rel
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
