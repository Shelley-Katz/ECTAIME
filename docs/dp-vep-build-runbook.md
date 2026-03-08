# DP + VEP Build Runbook (V1 Cobbled ECT, Two-Machine)

This runbook is intentionally explicit for your current setup.

- `Studio Mac (M2)`: executes Dorico + DP + VEP + rendering.
- `MacBook Air (M1)`: reference runbooks + archive/trace copies only.

## Phase 0: Preflight (Both Machines)

### On MacBook Air

1. Open this repo and keep these docs visible:
   - `docs/session-map.md`
   - `docs/semi-auto-gating-playbook.md`
   - `docs/qc-gates-and-tests.md`
2. Open `qc_notes/V1_CUE_QC_TEMPLATE.md` for logging.

### On Studio Mac

1. Create a working folder for the cue, for example:
   - `~/ECT_STUDIO_RUNS/<CUE_NAME>_YYYY-MM-DD/`
2. Confirm software availability:
   - Dorico
   - Digital Performer
   - VEP with Synchron V1
   - DP Meter Pro (VST3)
3. Confirm a stable audio engine and sample-loading state before setup.

## Phase 1: Source Preparation (Studio Mac Only)

1. In Dorico, export V1 MIDI:
   - filename: `V1.mid`
2. Switch Dorico playback to NotePerformer and prepare dry stem export:
   - global reverb off
   - channel reverb off
   - no extra FX/widening
   - centered output
3. Export V1 audio stem:
   - filename: `V1_NP_DRY.wav`
4. Save both files in the Studio cue folder.

## Phase 2: Build Core Session Graph (Studio Mac Only)

1. In DP, create/import source tracks:
   - `SRC_MIDI_V1` from `V1.mid`
   - `SRC_AUDIO_V1_NP_DRY` from `V1_NP_DRY.wav`
2. Create destination route:
   - `DST_V1_SYNC` -> VEP Synchron V1
3. Route notes/artics:
   - `SRC_MIDI_V1` output -> `DST_V1_SYNC`
4. Create ECT aux tracks:
   - `ECT_V1_LONG`
   - `ECT_V1_SHORT`
   - `ECT_V1_REP`
5. Add post-fader unity sends from `SRC_AUDIO_V1_NP_DRY` to all 3 ECT auxes.
6. Insert DP Meter Pro (VST3) on each ECT aux.
7. Force RT processing (not PG) for the ECT aux plugin chain.

## Phase 3: Build MIDI Bridge Layer (Studio Mac Only)

1. Create bridge MIDI tracks:
   - `BR_V1_LONG`
   - `BR_V1_SHORT`
   - `BR_V1_REP`
2. Set bridge inputs:
   - each from its corresponding DP Meter Pro MIDI output
3. Set bridge outputs:
   - all -> `DST_V1_SYNC`
4. Enable DP Multi Record.
5. Confirm input filter allows controller data (CC).

## Phase 4: Synchron Controller Mapping (Studio Mac Only)

1. In Synchron/VEP assign:
   - VelXF -> `CC1`
   - Expression -> `CC11`
   - Attack-related controller -> `CC21`
2. Verify each mapping with single-profile playback (one bridge active at a time).

## Phase 5: Load Profile Presets (Studio Mac Only)

1. Configure `ECT_V1_LONG` per:
   - `dpmeter_presets/V1_LONG_2026-02-28.md`
2. Configure `ECT_V1_SHORT` per:
   - `dpmeter_presets/V1_SHORT_2026-02-28.md`
3. Configure `ECT_V1_REP` per:
   - `dpmeter_presets/V1_REP_2026-02-28.md`
4. Save DP Meter Pro native presets with the same names and date suffix.

## Phase 6: Semi-Auto Gating (Studio Mac Only)

1. Add mute automation lanes on:
   - `BR_V1_LONG`
   - `BR_V1_SHORT`
   - `BR_V1_REP`
2. Build first-pass sections using heuristic rules:
   - LONG: note duration >= 300 ms
   - SHORT: note duration <= 220 ms
   - REP: repeated-note/trill/trem material (IOI <= 260 ms)
3. Enforce exclusivity:
   - exactly one bridge unmuted at any moment
4. Add 30-50 ms boundary overlap.
5. Refine by ear in-context.

Optional automation path (recommended for scale):

1. On MacBook Air, run `scripts/midi_profile_regions.py` to generate:
   - note-level audit CSV (optional, granular debug)
   - profile CSV
   - switch-control MIDI (`CC90/CC91`, pre-rolled for boundary reliability)
2. AirDrop `*_switch_control.mid` to Studio Mac.
3. Import it into DP and map:
   - `CC90` -> `ECT_V1_LONG` DPMP `Bypass`
   - `CC91` -> `ECT_V1_SHORT` DPMP `Bypass`
4. Use generated switching as first pass, then manually correct only edge cases.

## Phase 7: Validation + Render (Studio Mac Only)

1. Run profile behavior tests:
   - long phrase
   - short ostinato
   - repetition phrase
2. Run mixed-transition test and fix obvious boundary artifacts.
3. Render one full V1 cue stem set.
4. Apply your Nuancer pass and final touch automation as needed.

## Phase 8: Archive/Handoff (Studio -> MacBook via AirDrop)

1. On Studio Mac, package these files:
   - `V1.mid`
   - `V1_NP_DRY.wav`
   - rendered V1 stems
   - optional DP session snapshot/project copy
   - optional VEP project/preset export
   - cue QC notes (if created on Studio)
2. AirDrop package to MacBook Air.
3. On MacBook Air, place copies for traceability:
   - Dorico exports -> `dorico_exports/<CUE_NAME>/`
   - rendered stems -> `renders/<CUE_NAME>/`
   - cue notes -> `qc_notes/<CUE_NAME>_notes.md`
4. Fill/complete `qc_notes/V1_CUE_QC_TEMPLATE.md` from final listening results.

## If MIDI-Out Fails (Studio Mac)

1. Confirm RT processing on ECT auxes.
2. Confirm bridge track input source is correct DPMP insert output.
3. Confirm DP input filter allows CC.
4. Confirm DPMP output rows are MIDI-enabled only for intended lanes.
5. Use fallback workflow in `docs/fallback-automation-runbook.md`.
