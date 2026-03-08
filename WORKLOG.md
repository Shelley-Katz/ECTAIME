# WORKLOG

## 2026-02-28

- Initialized `/Users/sk/ECT` repository.
- Added full folder structure for V1 cobbled ECT workflow.
- Added implementation runbooks for DP + VEP routing, DP Meter Pro presets, semi-auto gating, and QC.
- Added preset definition files for `V1_LONG`, `V1_SHORT`, `V1_REP`.
- Added issue taxonomy and acceptance criteria templates.
- Updated runbooks for strict two-machine operations:
  - Studio Mac = execution authority
  - MacBook Air = control/archive authority
- Added AirDrop handoff checklist and two-machine operating model docs.
- Added articulation-aware switch compiler utility:
  - `scripts/midi_profile_regions.py` now supports MFT/OrchAPP datasets
  - emits region CSV + optional switch-control MIDI for DP bypass automation
- Updated helper docs and runbook with auto-switch workflow (`CC90`/`CC91`).

Next operator action: execute DAW steps in `docs/dp-vep-build-runbook.md`.

## 2026-03-01

- Refined `scripts/midi_profile_regions.py` for higher switch reliability:
  - Added KS-first boundary recovery (`lookahead`/`lookbehind`) to catch late/early KS edges.
  - Added optional KS+duration refinement near boundaries (KS remains primary).
  - Added switch MIDI pre-roll (`--switch-pre-roll-ms`) so incoming profile activates before boundary.
  - Reduced default merge gap from `50ms` to `30ms` for more granular switch maps.
  - Added optional per-note audit export (`--output-note-audit-csv`) for detailed debugging.
- Added KS-segment texture latch override to recover from stale/missed KS:
  - Duration-based LONG/SHORT evidence with configurable confirmation counts.
  - Connected-note (legato-gap) evidence for LONG in agile/eighth-note lines.
  - Optional fast LONG trigger with connected-note window for phrase starts.
- Added `Winterberg` cross-instrument validation notes:
  - Quick full-score MIDI track salience audit.
  - VLA1 listening findings and targeted REP retune batch recorded in
    `qc_notes/Winterberg_Vla1_QC.md`.
- Added ECT master project planning document:
  - `docs/ect-master-plan-v0.2.md` with phase scope/non-goals, data contracts,
    objective QC gates, benchmark strategy, milestones, and risk controls.
- Clarified A/B strategy in master plan:
  - preferred mode is passage-local reprocessing for fast conductor comparison;
  - full-score biased variants remain acceptable as Phase 1 fallback.
- Implemented M0 foundation artifacts:
  - Created `core/` and `orchestrator/` scaffolds.
  - Added canonical contract pack in `contracts/`:
    - input/output docs
    - `metrics.schema.json`
    - `run-manifest.schema.json`
    - `gates.v1.yaml`
    - config and CSV templates
  - Created benchmark package in `benchmarks/`:
    - frozen manifest `benchmark-manifest.v1.yaml`
    - selection rubric
    - placeholder clip/notes folders
  - Added `docs/m0-foundation-checklist.md`.
- Started M1 implementation:
  - Added `core/ect_core_cli.py` (offline vertical slice CLI).
  - Added `core/requirements.txt`.
  - Added `docs/m1-core-runbook.md`.
  - M1 CLI emits canonical artifacts in one run:
    - `control_stream.mid`
    - `profile_timeline.csv`
    - `note_audit.csv`
    - `metrics.json`
    - `run_manifest.json`
  - Added fallback gate thresholds in core (for environments without YAML parser).
- M1 Viola vertical slice production run completed and accepted:
  - Added gate review note `qc_notes/M1_GATE_REVIEW_VLA.md`.
  - Updated benchmark manifest validation status (`viola=pass`, others pending).
- Fixed M1 cello timeline wrap bug in `core/ect_core_cli.py`:
  - Root cause: tempo/meta track could end before control track end tick.
  - Effect in some DAWs: late CC events wrapped/clipped near start/end boundaries.
  - Fix: extend tempo track end-of-track time to at least final control-event tick.
  - Verified output: track 0 and track 1 now end at identical ticks in control stream MIDI.
- M1 cello vertical slice accepted after bugfix verification:
  - Added gate review `qc_notes/M1_GATE_REVIEW_CELLO.md`.
  - Updated benchmark validation status (`cello=pass`).
- M2 initial passage-local A/B capability implemented:
  - Added region-scoped processing options to `core/ect_core_cli.py`:
    - `--region-start-tick`, `--region-end-tick`
    - `--bias-profile` (`LONG|SHORT|REP`)
  - Added `selection` block to run manifest output and contract schema.
  - Updated run ID generation to millisecond precision.
  - Added runbook `docs/m2-local-ab-runbook.md`.
- Updated helper/runbook docs with the new recommended command and outputs.

## 2026-03-02

- Implemented M2.1 fast full-cue variant-pack runner:
  - Added `core/ect_variant_pack.py`.
  - Generates `NEUTRAL` + biased `LONG/SHORT/REP` outputs in one command.
  - Added optional bundled multitrack MIDI (`control_stream_variants.mid`) for one-step DAW import.
  - Added optional region bounds to run local variant packs with the same workflow.
- Added runbook `docs/m2-variant-pack-runbook.md` for DP import/audition/consolidation flow.

## 2026-03-03

- Added per-score conversion pipeline for Dorico package handoff:
  - New CLI: `core/ect_score_job_cli.py`
  - New drop workspace: `score_conversion_drop/` with `inbox/`, `outbox/`, `archive/`
  - New runbook: `docs/score-job-runbook.md`
- New `ect_score_job_cli.py` capabilities:
  - Accept one per-score folder containing Dorico `.mid` + stems directory.
  - Parse and keep only note-bearing source tracks (drops empty/KS-only tracks after split, with QC reason).
  - Strip existing `CC1/CC11`.
  - Split each instrument into:
    - `<Inst>` (notes only + regenerated `CC1/CC11`)
    - `<Inst> KS` (keyswitch notes only)
    - `<Inst> ArtMap` (duplicated notes only)
  - Preserve conductor/meta timing in an `ECT Conductor` track.
  - Emit one DP-ready MIDI + one QC JSON report per score.
  - Track removal policy is content/salience-based (empty/KS-only after split), not name-based.
- Upgraded KS extraction to strict method (`lookup table + range guards`):
  - Added strict KS classifier in `core/ect_score_job_cli.py`:
    - lookup-pool candidate detection from articulation dataset
    - chord/latch matching when available
    - instrument-range + core-range safeguards for ambiguous notes
  - Added per-track `ks_diagnostics` to QC output to audit KS candidate/selected/ambiguous counts.
  - Re-ran Waghalter package: woodwinds now consistently split into notes vs KS tracks; percussion tracks with no KS stay empty as expected.
- Added per-track dataset routing and fallback:
  - Track names now resolve to instrument-specific articulation datasets (`Fl1`, `Ob2`, `Vc`, `DB`, etc.).
  - If a mapped dataset is a stub (empty KS pool), extractor falls back to base/global articulation dataset.
- Added anti-false-positive guard for broad fallback pools (prevents low-register musical notes from being stripped as KS in overlapping ranges).
- DP import hygiene fix in `ect_score_job_cli.py`:
  - source non-note channel events are now dropped from output note tracks,
  - output note tracks carry only notes + regenerated `CC1/CC11`,
  - empty KS tracks are omitted from output.
  - Result: eliminates multi-channel track splits (`/1`, `/2`, etc.) caused by mixed-channel source control/program events on MIDI import.
- Performed smoke tests:
  - Single-line job (`Winterberg Vla 1`) passed.
  - Full-score job (`Winterberg - Full score`) passed after channel-normalization fix
    (`midi_profile_regions` internal channels are 1..16; mido requires 0..15).

## 2026-03-04

- Fixed core issue in `core/ect_score_job_cli.py`: CC generation was previously note-driven only (sparse events at note starts), even when stems were present.
- Added WAV-envelope driven CC engine for score jobs:
  - Per-track stem RMS envelope extraction via `ffmpeg` (`astats` metadata path).
  - Dense time-sampled CC generation across note durations (not just note starts).
  - Profile-aware shaping (`LONG/SHORT/REP`) with per-profile smoothing/offset/amount behavior.
  - Automatic fallback to legacy MIDI-note CC generation if stem analysis fails.
- Added per-track QC evidence fields:
  - `cc_generation_mode` (`wav_envelope` or `midi_fallback`)
  - `generated_cc_counts`, `generated_cc_density_per_min`, `generated_cc_span_sec`
  - `audio_envelope_points`, `audio_rms_db_range`, `cc_generation_notes`
- Re-ran Waghalter package and validated:
  - All retained instrument tracks used `wav_envelope` mode.
  - Source-track `CC1/CC11` strip counts exactly match original source tracks.
  - Output note tracks contain only regenerated `CC1/CC11` control streams (no extra CC types).

## 2026-03-05

- Added MixCT MVP-A (DP-first offline bus automation prototype):
  - New CLI: `orchestrator/mixct_mvp_a_cli.py`
  - Input: source MIDI + session map YAML + directive text
  - Output:
    - `__MIXCT_AUTOMATION.mid` (per-bus CC automation tracks)
    - `__MIXCT_PLAN.csv` (bar-wise target dB plan)
    - `__MIXCT_AUDIT.json` (parsed directives + bus mapping + counts)
- Added session-map contracts:
  - `contracts/mixct_session_map.template.yaml`
  - `contracts/mixct_session_map.waghalter.yaml`
  - `contracts/mixct_directives.example.txt`
- Added docs:
  - `docs/mixct-session-map.md`
  - `docs/mixct-session-map-waghalter.md`
  - `docs/mixct-mvp-a-runbook.md`
- Generated first Waghalter MixCT MVP-A outputs:
  - `/Users/sk/ECT/score_conversion_drop/outbox/Waghalter/mixct_mvp_a/Waghalter__DP_PREP_WAV_DENSE__MIXCT_AUTOMATION.mid`
  - `/Users/sk/ECT/score_conversion_drop/outbox/Waghalter/mixct_mvp_a/Waghalter__DP_PREP_WAV_DENSE__MIXCT_PLAN.csv`
  - `/Users/sk/ECT/score_conversion_drop/outbox/Waghalter/mixct_mvp_a/Waghalter__DP_PREP_WAV_DENSE__MIXCT_AUDIT.json`

## 2026-03-08

- Reviewed external research JSON:
  - `/Users/sk/ECT/MixCT Research Data/mixct_orchestral_intelligence_kb_v1.json`
  - Cross-check: `/Users/sk/ECT/MixCT Research Data/MixCT_ultra_short_summary.md`
- Captured immediate adoption + deferred integration items in:
  - `/Users/sk/ECT/docs/mixct-orchestral-intelligence-kb-review.md`
- Added explicit follow-up intent to map selected KB constraints into:
  - `/Users/sk/ECT/docs/mixct_mvp_b_codex_spec.json`
  - MixCT command/execute audit fields (explanation + policy checks).
- Applied highest-value KB constraints into `/Users/sk/ECT/docs/mixct_mvp_b_codex_spec.json`:
  - added `orchestral_intelligence_profile`,
  - added doctrine safety caps + action order,
  - extended command explanation/risk fields,
  - added recommended audit/data-contract fields,
  - expanded diagnostics/test-plan/done-gate/prohibitions,
  - added explicit do-nothing acceptance scenario.
