# BMAD PocketFlow Status Navigator - Design

**Date:** 2026-05-17  
**Status:** Approved

## Overview

A standalone Python CLI (`bmad-flow.py`) that uses PocketFlow to read a BMAD project's Phase 4 state, determine where in the implementation cycle the project currently sits, and output a clear status summary plus the exact next step to take.

Drop the script into any BMAD project root and run it. No installation, no config, no framework lock-in beyond PocketFlow itself.

```bash
python bmad-flow.py
python bmad-flow.py --path /other/bmad-project
```

## Scope

Phase 4 (Implementation cycle) only. Covers:

- Sprint planning (pre-cycle)
- Story creation, validation, dev, code review (per-story loop)
- Epic retrospective (post-epic)
- All-done detection

Phases 1-3 and the TEA/CIS modules are out of scope for this iteration.

## Architecture

Single PocketFlow graph, runs to completion in one pass (status mode only). No interactive/guided mode in this version.

```
ReadProjectState
  ├─ "needs_sprint_planning" ──► SprintPlanningAdvisor
  ├─ "needs_story_creation"  ──► CreateStoryAdvisor
  ├─ "needs_validation"      ──► ValidateStoryAdvisor
  ├─ "needs_dev"             ──► DevStoryAdvisor
  ├─ "needs_review"          ──► CodeReviewAdvisor
  ├─ "epic_complete"         ──► RetrospectiveAdvisor
  └─ "all_done"              ──► AllDoneNode
```

`ReadProjectState` is the sole router. All advisor nodes are terminal: they print output and the flow ends.

## Data Layer

### Shared Store Schema

```python
shared = {
    "project_root": str,           # resolved project path
    "bmad_config": dict,           # _bmad/bmm/config.yaml contents
    "sprint_status": dict | None,  # parsed sprint-status.yaml, or None
    "artifact_paths": {
        "planning": str,           # e.g. docs/bmad/planning-artifacts/
        "implementation": str,     # e.g. docs/bmad/implementation-artifacts/
    },
    "current_position": {
        "epic": str | None,
        "story": str | None,
        "story_status": str | None,
        "step": str,               # matches action string returned by ReadProjectState
    },
}
```

### State Determination (ReadProjectState)

Priority order:

1. **sprint-status.yaml present:** parse it, find first non-`done` epic, then first non-`done` story within that epic. Determine step from story status:
   - `backlog` with no story file: `needs_story_creation`
   - `backlog` with story file present: `needs_validation`
   - `ready-for-dev`: `needs_dev`
   - `in-progress`: `needs_dev` (resume) or `needs_review` (if dev-complete marker in story file)
   - All stories in epic are `done`: `epic_complete`
   - All epics `done`: `all_done`
   - No sprint-status.yaml and no implementation-artifacts dir: `needs_sprint_planning`

2. **Artifact detection fallback** (no sprint-status.yaml): scan `implementation-artifacts/` for story `.md` files, infer status from internal checklist markers in each story file.

### BMAD Config Reading

Read `_bmad/bmm/config.yaml` for artifact paths. If not present, fall back to BMAD defaults (`docs/bmad/planning-artifacts/`, `docs/bmad/implementation-artifacts/`).

## PocketFlow Nodes

### ReadProjectState

- `prep`: reads `project_root` from shared
- `exec`: runs state determination logic, returns `current_position` dict
- `post`: writes `current_position` to shared, returns action string

### Advisor Nodes (SprintPlanningAdvisor, CreateStoryAdvisor, etc.)

Each follows the same pattern:

- `prep`: reads `current_position` and `sprint_status` from shared
- `exec`: formats status summary + next step recommendation
- `post`: prints output, returns `None` (terminal)

No advisor node writes to shared or routes further.

## Output Format

```
BMAD Phase 4 - Sprint Status
════════════════════════════

Project: My Awesome App

Epic 1: User Management  [in-progress]
  ✓  1-1  user-authentication
  →  1-2  account-management     (needs code review)
  ○  1-3  password-reset

Epic 2: Product Features  [backlog]
  ○  2-1  product-catalog

════════════════════════════
Next: Code Review

Story file: docs/bmad/implementation-artifacts/1-2-account-management.md

Run: /bmad-code-review
```

Legend: `✓` done, `→` current story, `○` not started. No color dependencies; works in any terminal.

## File Layout

```
{any-bmad-project}/
└── bmad-flow.py
```

Single file. Dependencies:

```
pocketflow   # pip install pocketflow
pyyaml       # pip install pyyaml
```

## Out of Scope (This Version)

- Guided/interactive mode (confirm completion, loop)
- Phases 1-3 navigation
- TEA and CIS module tracking
- Modifying sprint-status.yaml
- Generating sprint-status.yaml from scratch
