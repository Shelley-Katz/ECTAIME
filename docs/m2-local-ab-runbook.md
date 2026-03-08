# M2 Local A/B Runbook (Passage Reprocess)

Use this to reprocess only a local passage and compare alternatives quickly.

## Capability

ECT Core now supports:

1. Region-local processing via absolute tick bounds.
2. Optional profile bias for quick A/B:
   - `LONG`
   - `SHORT`
   - `REP`

## Region Arguments

1. `--region-start-tick`
2. `--region-end-tick`

Both must be provided together.

## Quick Tick Formula (4/4 projects)

If your clip is in stable 4/4:

1. `ticks_per_beat = 480`
2. `ticks_per_measure = 1920`
3. `absolute_tick = (measure - 1) * 1920 + (beat - 1) * 480 + tick`

Example:

1. `M210|2|000` -> `(210-1)*1920 + (2-1)*480 + 0 = 401,760`

## Example Commands

### A) Local neutral (no bias)

```bash
cd /Users/sk/ECT
.venv/bin/python core/ect_core_cli.py \
  --source-midi "Vc MIDI Src Clipping.mid" \
  --source-audio "/Users/sk/ECT/dorico_exports/VC1_NP_DRY.wav" \
  --track-index 1 \
  --config contracts/config.template.yaml \
  --region-start-tick 380000 \
  --region-end-tick 400000 \
  --output-dir qc_notes/m2_vc_region_neutral
```

### B) Local biased variants for A/B

```bash
cd /Users/sk/ECT
.venv/bin/python core/ect_core_cli.py \
  --source-midi "Vc MIDI Src Clipping.mid" \
  --source-audio "/Users/sk/ECT/dorico_exports/VC1_NP_DRY.wav" \
  --track-index 1 \
  --config contracts/config.template.yaml \
  --region-start-tick 380000 \
  --region-end-tick 400000 \
  --bias-profile LONG \
  --output-dir qc_notes/m2_vc_region_long
```

Repeat with `--bias-profile SHORT` and `--bias-profile REP`.

## DP Import Notes

1. Import each `control_stream.mid` as a CC candidate track.
2. Keep only one candidate active at a time for clean A/B.
3. Keep DPMP bridge CC path muted/disabled during offline ECT tests.

## Artifact Trace

Each local run includes:

1. `metrics.json`
2. `run_manifest.json`

`run_manifest.json` records `selection.region_start_tick`, `selection.region_end_tick`, and `selection.bias_profile`.

