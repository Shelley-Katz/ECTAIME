# Ops Handoff Protocol

Use these folders to coordinate work between machines/Codex sessions.

- `ops/inbox/`: new tasks (one markdown file per task)
- `ops/in_progress/`: task currently being executed
- `ops/done/`: completed tasks with outcomes
- `ops/handoffs/`: cross-machine context snapshots
- `ops/relay/messages/`: machine-to-machine short messages (`ping`, `pong`, `status`, `task`) for live coordination

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

## Relay quick start

Run from repo root (`/Users/sk/ECT`):

```bash
# Send one message
./ops/tools/relay.sh send --from laptop --to studio --kind ping --text "Ready for next task"

# Watch for incoming messages (pull + audible alert)
./ops/tools/relay.sh watch --for laptop --interval 5 --pull --say

# Heartbeat
./ops/tools/relay.sh heartbeat --from laptop --to studio

# Roundtrip test (initiator)
./ops/tools/relay.sh roundtrip-start --from laptop --to studio --timeout 120
```

## Rules

1. Move, don't copy, task files between states.
2. Every completed task must include a short result note.
3. Keep one source of truth in git; no parallel local-only task lists.
4. Two-machine architecture is locked: laptop is Director/brain; studio is thin endpoint/executor. No posture changes without explicit user approval.
