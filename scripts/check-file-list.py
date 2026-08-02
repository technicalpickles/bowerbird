#!/usr/bin/env python3
"""
File-List-vs-git audit (Epic 3 retro AI-5 -> Epic 4 AI-6 -> Epic 5 AI-3).

Compares a BMAD story file's `### File List` section against what git says
actually changed, and reports drift in both directions:

  1. Changed in git, absent from the File List. The common case: side-effect
     files (sprint-status bumps, planning-doc edits, rustfmt reflows on
     adjacent files, mid-story guardrails) that the dev agent's recollection
     of *intentional* edits silently drops.
  2. Declared in the File List, unchanged in git. The dev claimed an edit
     that did not ship, or a path that was later reverted or renamed.

Team agreement A9 (docs/bmad/implementation-artifacts/epic-3-retro-2026-05-25.md)
makes both directions HIGH findings at review time. This script is the
deterministic implementation of that audit so it runs the same way from
bmad-dev-story, bmad-code-review, the story automator, or by hand.

Scope of "changed in git" is the union of:

  - committed on this branch since it diverged from the base ref
    (default `main`, override with --base), and
  - the working tree: staged, unstaged, and untracked.

Untracked files count. A new file the story created but never staged is
exactly the omission this audit exists to catch. Ignored files (per
.gitignore, including the user's global one) never appear.

Exit codes:
  0  clean, File List matches git
  1  drift found (either direction)
  2  tooling error: no story file, not a git repo, unreadable input

Exit 1 is a finding, not a crash. Callers decide severity: bmad-dev-story
treats it as a blocking self-check before declaring `review`, code review
treats it as a HIGH finding per A9.

Usage:
  python3 scripts/check-file-list.py docs/bmad/implementation-artifacts/6-session-glance.md
  python3 scripts/check-file-list.py <story> --base main --format json
  python3 scripts/check-file-list.py <story> --ignore 'docs/scratch/*'
"""

import argparse
import fnmatch
import json
import re
import subprocess
import sys
from pathlib import Path

# `### File List`, tolerating any heading level and trailing punctuation.
FILE_LIST_HEADING = re.compile(r"^\s{0,3}(#{1,6})\s+file\s+list\b", re.IGNORECASE)
ANY_HEADING = re.compile(r"^\s{0,3}(#{1,6})\s+\S")
BULLET = re.compile(r"^\s*[-*+]\s+(.+)$")

# A bare token that plausibly names a path. Deliberately conservative: prose
# bullets ("Suite 650/0 green") must not be mistaken for file entries.
PATH_CHARS = re.compile(r"^[\w./@+\-]+$")


class ToolingError(Exception):
    """Unrecoverable input problem. Maps to exit 2."""


def run_git(args, cwd):
    """Run a git command, returning stdout. Raises ToolingError on failure."""
    try:
        proc = subprocess.run(
            ["git"] + args,
            cwd=str(cwd),
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError as exc:
        raise ToolingError(f"could not run git: {exc}") from exc
    if proc.returncode != 0:
        raise ToolingError(
            f"git {' '.join(args)} failed ({proc.returncode}): {proc.stderr.strip()}"
        )
    return proc.stdout


def repo_root(start):
    out = run_git(["rev-parse", "--show-toplevel"], start)
    return Path(out.strip())


def extract_file_list(story_text):
    """
    Pull declared paths out of the story's File List section.

    Returns (paths, section_found). Paths keep source order and are
    de-duplicated. A heading of the same or higher level ends the section.
    """
    lines = story_text.splitlines()
    start = None
    heading_level = None
    for i, line in enumerate(lines):
        match = FILE_LIST_HEADING.match(line)
        if match:
            start = i + 1
            heading_level = len(match.group(1))
            break
    if start is None:
        return [], False

    body = []
    for line in lines[start:]:
        heading = ANY_HEADING.match(line)
        if heading and len(heading.group(1)) <= heading_level:
            break
        body.append(line)

    paths = []
    seen = set()
    for line in body:
        bullet = BULLET.match(line)
        if not bullet:
            continue
        token = first_path_token(bullet.group(1))
        if token and token not in seen:
            seen.add(token)
            paths.append(token)
    return paths, True


def first_path_token(content):
    """
    Extract the path from one File List bullet's content.

    Handles the two shapes the stories actually use:
      `crates/daemon/src/projection/state.rs` (modified, ...)
      INSTALL.md (one unpinned line, ...)
    plus bold wrappers. Returns None when the bullet is prose.
    """
    content = content.strip()
    content = re.sub(r"^\*\*(.*?)\*\*", r"\1", content)

    if content.startswith("`"):
        end = content.find("`", 1)
        if end > 1:
            token = content[1:end]
        else:
            token = content[1:].split()[0] if content[1:].split() else ""
    else:
        parts = content.split()
        token = parts[0] if parts else ""

    token = token.strip().strip("`").rstrip(",;:")
    # Strip a leading `./` only. A blanket lstrip("./") would eat the dot off
    # dotfile paths like `.github/workflows/ci.yml`.
    token = re.sub(r"^\./", "", token)
    # Trailing sentence punctuation, but never the slash that marks a
    # directory entry.
    if token.endswith("."):
        token = token[:-1]
    if not token or not PATH_CHARS.match(token):
        return None
    return token


def looks_like_path(token, tracked):
    """
    A token is a path if it has a directory separator or an extension, or if
    git already tracks it (which rescues extensionless names like `Makefile`).
    """
    return "/" in token or "." in token or token in tracked


def parse_porcelain(output):
    """
    Parse `git status --porcelain` into {path: status_label}.

    Renames report both sides: the story changed the old path (it is gone)
    and the new one (it is new), and the File List should say so.
    """
    changes = {}
    for raw in output.splitlines():
        if not raw.strip():
            continue
        code = raw[:2]
        rest = raw[3:]
        if code == "??":
            changes[unquote(rest)] = "untracked"
            continue
        if "R" in code or "C" in code:
            if " -> " in rest:
                old, new = rest.split(" -> ", 1)
                label = "renamed from" if "R" in code else "copied from"
                changes[unquote(old)] = f"{label.split()[0]} (old path)"
                changes[unquote(new)] = f"{label.split()[0]} (new path)"
                continue
        label = describe_status(code)
        changes[unquote(rest)] = label
    return changes


def describe_status(code):
    index, worktree = code[0], code[1]
    if "D" in (index, worktree):
        return "deleted"
    if "A" in (index, worktree):
        return "added"
    if "M" in (index, worktree):
        return "modified"
    if "?" in (index, worktree):
        return "untracked"
    return f"changed ({code.strip()})"


def unquote(path):
    """git quotes paths containing special characters; unwrap that."""
    path = path.strip()
    if len(path) >= 2 and path.startswith('"') and path.endswith('"'):
        try:
            return json.loads(path)
        except ValueError:
            return path[1:-1]
    return path


def collect_git_changes(root, base):
    """
    Union of committed-since-base and working-tree changes.

    Returns (changes, base_info) where changes is {path: label}. When the
    base ref is missing or HEAD has not diverged from it, only the working
    tree contributes, and base_info explains why.
    """
    changes = {}
    base_info = {"ref": base, "merge_base": None, "note": None}

    merge_base = None
    if base:
        try:
            merge_base = run_git(["merge-base", "HEAD", base], root).strip()
        except ToolingError:
            base_info["note"] = f"base ref '{base}' not found; working tree only"

    if merge_base:
        head = run_git(["rev-parse", "HEAD"], root).strip()
        base_info["merge_base"] = merge_base[:12]
        if merge_base == head:
            base_info["note"] = f"HEAD has not diverged from '{base}'; working tree only"
        else:
            committed = run_git(
                ["diff", "--name-only", merge_base, "HEAD"], root
            ).splitlines()
            for path in committed:
                path = unquote(path)
                if path:
                    changes[path] = "committed on branch"

    # --untracked-files=all is load-bearing: plain --porcelain collapses a new
    # directory into one `docs/cookbook/` entry instead of naming the files
    # inside it, which is precisely the case a new cookbook entry hits.
    porcelain = run_git(["status", "--porcelain", "--untracked-files=all"], root)
    for path, label in parse_porcelain(porcelain).items():
        # A file both committed on the branch and dirty in the tree gets the
        # more specific working-tree label.
        changes[path] = label

    return changes, base_info


def apply_ignores(paths, patterns):
    if not patterns:
        return paths, []
    kept, dropped = {}, []
    for path, label in paths.items():
        if any(fnmatch.fnmatch(path, pat) for pat in patterns):
            dropped.append(path)
        else:
            kept[path] = label
    return kept, dropped


def audit(story_path, base, ignores, root):
    story_file = Path(story_path)
    if not story_file.is_absolute():
        story_file = (Path.cwd() / story_file).resolve()
    if not story_file.is_file():
        raise ToolingError(f"story file not found: {story_path}")

    try:
        story_text = story_file.read_text(encoding="utf-8")
    except OSError as exc:
        raise ToolingError(f"could not read story file: {exc}") from exc

    tracked = set(run_git(["ls-files"], root).splitlines())
    declared_raw, section_found = extract_file_list(story_text)
    declared = [p for p in declared_raw if looks_like_path(p, tracked)]

    changed, base_info = collect_git_changes(root, base)
    changed, ignored = apply_ignores(changed, ignores)
    declared_set = set(declared)
    # Stories that move whole trees declare directory entries with a trailing
    # slash (Story 5.13 did this for docs/cookbook/*/). Git only ever reports
    # files, so a directory entry covers every changed path beneath it.
    declared_dirs = [p for p in declared if p.endswith("/")]

    def is_declared(path):
        return path in declared_set or any(path.startswith(d) for d in declared_dirs)

    def is_satisfied(entry):
        if entry.endswith("/"):
            return any(path.startswith(entry) for path in changed)
        return entry in changed

    missing = {p: label for p, label in sorted(changed.items()) if not is_declared(p)}
    unchanged = [p for p in declared if not is_satisfied(p)]

    return {
        "story": str(story_file.relative_to(root)) if is_relative(story_file, root) else str(story_file),
        "section_found": section_found,
        "base": base_info,
        "declared_count": len(declared),
        "changed_count": len(changed),
        "missing_from_file_list": missing,
        "declared_but_unchanged": unchanged,
        "ignored": ignored,
        "clean": not missing and not unchanged and section_found,
    }


def is_relative(path, root):
    try:
        path.relative_to(root)
        return True
    except ValueError:
        return False


def render_text(result):
    out = []
    out.append(f"FILE LIST AUDIT: {result['story']}")

    base = result["base"]
    bits = [f"base {base['ref']}"]
    if base["merge_base"]:
        bits.append(f"merge-base {base['merge_base']}")
    bits.append(f"{result['changed_count']} changed in git")
    bits.append(f"{result['declared_count']} declared")
    out.append("  " + " | ".join(bits))
    if base["note"]:
        out.append(f"  note: {base['note']}")
    if result["ignored"]:
        out.append(f"  ignored by --ignore: {len(result['ignored'])}")

    if not result["section_found"]:
        out.append("")
        out.append("  NO `### File List` SECTION FOUND in the story.")
        out.append("  Every changed file below is undeclared.")

    missing = result["missing_from_file_list"]
    if missing:
        out.append("")
        out.append(f"MISSING FROM FILE LIST (changed in git, not declared) [{len(missing)}]")
        for path, label in missing.items():
            out.append(f"  + {path}  ({label})")

    unchanged = result["declared_but_unchanged"]
    if unchanged:
        out.append("")
        out.append(f"DECLARED BUT UNCHANGED (in File List, no git change) [{len(unchanged)}]")
        for path in unchanged:
            out.append(f"  - {path}")

    out.append("")
    if result["clean"]:
        out.append("CLEAN: File List matches git exactly.")
    else:
        total = len(missing) + len(unchanged)
        out.append(
            f"DRIFT: {total} discrepanc{'y' if total == 1 else 'ies'}. "
            "Per team agreement A9 both directions are HIGH findings: fix the "
            "File List (or the code) before declaring review."
        )
    return "\n".join(out)


def main(argv=None):
    parser = argparse.ArgumentParser(
        description="Audit a BMAD story's File List against git reality.",
    )
    parser.add_argument("story", help="path to the story markdown file")
    parser.add_argument(
        "--base",
        default="main",
        help="branch to diff against for committed changes (default: main). "
        "Pass an empty string to audit the working tree only.",
    )
    parser.add_argument(
        "--ignore",
        action="append",
        default=[],
        metavar="GLOB",
        help="glob of paths to exclude from the git side (repeatable)",
    )
    parser.add_argument(
        "--format",
        choices=["text", "json"],
        default="text",
        help="output format (default: text)",
    )
    args = parser.parse_args(argv)

    try:
        root = repo_root(Path.cwd())
        result = audit(args.story, args.base, args.ignore, root)
    except ToolingError as exc:
        print(f"check-file-list: {exc}", file=sys.stderr)
        return 2

    if args.format == "json":
        print(json.dumps(result, indent=2, sort_keys=True))
    else:
        print(render_text(result))

    return 0 if result["clean"] else 1


if __name__ == "__main__":
    sys.exit(main())
