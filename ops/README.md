# Ops Handoff Protocol

Use these folders to coordinate work between machines/Codex sessions.

- `ops/inbox/`: new tasks (one markdown file per task)
- `ops/in_progress/`: task currently being executed
- `ops/done/`: completed tasks with outcomes
- `ops/handoffs/`: cross-machine context snapshots

## Task file template

```md
# TASK: <short title>

- Created: <ISO datetime>
- Owner: <studio|laptop|human>
- Priority: <P0|P1|P2>
- Goal:
- Inputs:
- Constraints:
- Done when:
- Notes:
```

## Rules

1. Move, don't copy, task files between states.
2. Every completed task must include a short result note.
3. Keep one source of truth in git; no parallel local-only task lists.
