# Symphonova Orchestrator

Purpose: batch/job automation around ECT Core.

Planned responsibilities:

1. Queue and execute offline conversion jobs.
2. Track run status and resume/retry failed jobs.
3. Package outputs for DAW integration workflows (DP, Logic, Cubase).
4. Preserve run logs and manifests for reproducibility and audit.

M0 note:

This folder is scaffolded for Phase 2 build work after Core M1/M2 stabilization.

Current implemented utility (MVP-A):

1. `mixct_mvp_a_cli.py`
   - Offline language-directive -> bus automation MIDI generator for DP-first workflow.
   - See:
     - `/Users/sk/ECT/docs/mixct-session-map.md`
     - `/Users/sk/ECT/docs/mixct-mvp-a-runbook.md`
