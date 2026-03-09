# TASK: Studio Inventory + Runtime Snapshot

- Created: 2026-03-09T00:12:48Z
- Owner: studio
- Priority: P0
- Goal:
  - Publish a current, machine-verified Studio state so laptop and studio share one source of truth before overnight build work.
- Inputs:
  - Local Studio filesystem (project roots, audio/midi repositories, active work dirs)
  - Current live session context: Waghalter DP project + Symphonova 1 VEP project (4 instances running)
- Constraints:
  - Commit only text/markdown/csv metadata (no large media/audio files).
  - Use concise summaries; include absolute paths.
- Done when:
  - New files are committed under `ops/handoffs/`:
    1. `studio_inventory_refresh.md` (top-level map + key directories + branch/repo status)
    2. `studio_runtime_snapshot.md` (what is open/running now: DP project, VEP project, instances, key routing assumptions)
    3. `studio_priority_targets.md` (recommended next work targets for tonight, ordered)
  - Relay message sent to laptop with commit hash and 5-line summary.
- Notes:
  - This is foundational context for cross-machine execution sequencing.
