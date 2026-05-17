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
