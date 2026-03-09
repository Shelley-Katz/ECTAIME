# Studio Priority Targets

Generated (UTC): 2026-03-09T00:32:39Z

Ordered targets for tonight (studio executor side):

1. Lock the current runtime truth and keep relay updated.
- Source of truth files:
  - /Users/symphonova/ECTAIME/ops/handoffs/studio_inventory_refresh.md
  - /Users/symphonova/ECTAIME/ops/handoffs/studio_runtime_snapshot.md
- Keep laptop informed with commit-hash status updates after each material change.

2. Validate DP<->VEP 4-group routing before any long run.
- Confirm DP groups map as expected: 49213-15, 49227-29, 49242-44, 49255-57.
- Confirm VEP server remains bound to ports 6473 and 7200.
- If a group drops, pause downstream automation and post a relay status immediately.

3. Promote Waghalter + Symphonova 1 as the first production bundle.
- DP project: /Users/symphonova/Documents/DP Projects/2026 Productions/Waghalter/DP/Waghalter_MixCT_Test_01/Waghalter_MixCT_Test_01.dpdoc
- VEP project: /Users/symphonova/Documents/VSL/VEP Server Projects/Symphonova 1.vesp64
- Rationale: both are active in runtime and explicitly visible in app-window telemetry.

4. Prepare reproducible handoff metadata for overnight build work.
- Keep output text-only under /Users/symphonova/ECTAIME/ops/handoffs.
- Record absolute paths, UTC timestamps, and machine-verified state only.

5. Watch readiness risks that can invalidate overnight sequencing.
- Unsaved in-memory changes in DP/VEP may diverge from on-disk timestamps.
- VEP internal instance labels are not directly queryable from CLI snapshots.
- Any workstation sleep/network reset could silently break one or more DP<->VEP groups.
