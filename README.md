# ECT V1 Cobbled Workflow (DP + VEP)

This repository contains the implementation assets for a **V1-only** cobbled ECT pipeline:

- Input: Dorico MIDI + dry NotePerformer V1 stem
- Processing: 3x DP Meter Pro profiles (LONG / SHORT / REP)
- Routing: Digital Performer -> MIDI bridge tracks -> VEP Synchron V1
- Output: production-quality neutral V1 stems, then Nuancer pass

## Operating Model (Two Machines)

- `Studio Mac (M2)` is the **production machine**:
  - Dorico export
  - Digital Performer routing and processing
  - VEP/Synchron playback
  - Final stem rendering
- `MacBook Air (M1)` is the **control/archive machine**:
  - This repository and runbooks
  - QC notes and trace records
  - AirDropped backup copies from Studio Mac

Important: files copied into this repo are archival copies for traceability, not the active runtime source for the Studio Mac session.

## Folder Layout

- `dorico_exports/` - archival copies of Dorico exports received from Studio Mac
- `dp_session/` - Digital Performer session map/checklists (reference)
- `vep_presets/` - VEP/Synchron patch notes and mapping references
- `dpmeter_presets/` - exact profile settings for LONG/SHORT/REP
- `docs/` - end-to-end setup + gating + QC runbooks
- `qc_notes/` - cue-by-cue QC logs and issue tracking
- `renders/` - archival copies of rendered output received from Studio Mac
- `scripts/` - optional helper scripts
  - `midi_profile_regions.py` can compile LONG/SHORT switch maps from Dorico MIDI + MFT datasets
  - `dpdoc_forensics.py` isolates likely articulation-edit regions inside `.dpdoc` by subtracting save noise
- `core/` - ECT Core engine scaffold (offline converter target)
- `orchestrator/` - batch/job automation scaffold (Phase 2 target)
- `contracts/` - canonical input/output/metrics/manifest contracts
- `benchmarks/` - frozen benchmark corpus manifest + rubric

## Core Contracts

- `CC1 -> VelXF`
- `CC11 -> Expression`
- `CC21 -> Attack-related controller`

Track naming contract:

- `SRC_MIDI_V1`
- `SRC_AUDIO_V1_NP_DRY`
- `ECT_V1_LONG`, `ECT_V1_SHORT`, `ECT_V1_REP`
- `BR_V1_LONG`, `BR_V1_SHORT`, `BR_V1_REP`
- `DST_V1_SYNC`

## Start Here

1. Read `docs/two-machine-operating-model.md`
2. Read `docs/session-map.md`
3. Execute `docs/dp-vep-build-runbook.md`
4. Use `docs/semi-auto-gating-playbook.md`
5. Validate with `docs/qc-gates-and-tests.md`
6. Follow `docs/airdrop-handoff-checklist.md`
7. Record findings in `qc_notes/V1_CUE_QC_TEMPLATE.md`
8. For ECT build execution, use `docs/ect-master-plan-v0.2.md`
9. For M1 CLI execution, use `docs/m1-core-runbook.md`
10. For passage-local A/B reprocess, use `docs/m2-local-ab-runbook.md`
11. For full-cue fast A/B (NEUTRAL + LONG/SHORT/REP in one run), use `docs/m2-variant-pack-runbook.md`
12. For `.dpdoc` reverse-engineering workflow, use `docs/dpdoc-forensics-runbook.md`
13. For per-score Dorico package -> DP-ready MIDI conversion, use `docs/score-job-runbook.md` and `score_conversion_drop/`
14. For MixCT bus-role automation MVP-A, use `docs/mixct-session-map.md` and `docs/mixct-mvp-a-runbook.md`
15. For Waghalter-specific detailed Session Map, use `docs/mixct-session-map-waghalter.md`

## Status

Implementation scaffold is complete in-repo.
DAW-side setup actions must be performed manually in Digital Performer/VEP.
