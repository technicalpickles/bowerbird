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
