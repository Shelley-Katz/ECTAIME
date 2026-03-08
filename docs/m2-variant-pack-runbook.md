# M2.1 Variant Pack Runbook (Full Cue Fast A/B)

Use this when you want fast musical decision-making without repeated terminal loops.

One command creates:

1. `NEUTRAL` full-cue control stream
2. `LONG` biased full-cue control stream
3. `SHORT` biased full-cue control stream
4. `REP` biased full-cue control stream
5. one bundled multitrack MIDI for one-step DAW import

## Command (Viola Example)

```bash
cd /Users/sk/ECT
.venv/bin/python core/ect_variant_pack.py \
  --source-midi "Winterberg Vla 1 .mid" \
  --source-audio "/Users/sk/ECT/dorico_exports/VLA1_NP_DRY.wav" \
  --track-index 1 \
  --config contracts/config.template.yaml \
  --output-dir qc_notes/m2_vla_variant_pack \
  --run-prefix vla-pack \
  --bundle-midi
```

## Output Layout

`qc_notes/m2_vla_variant_pack/`

1. `neutral/control_stream.mid`
2. `long/control_stream.mid`
3. `short/control_stream.mid`
4. `rep/control_stream.mid`
5. `control_stream_variants.mid` (bundled file, 1 CC track per variant)
6. `variant_pack_manifest.json`

Each variant folder also contains:

1. `profile_timeline.csv`
2. `note_audit.csv`
3. `metrics.json`
4. `run_manifest.json`

## Digital Performer Workflow (Fast)

1. Import `control_stream_variants.mid` once.
2. Route all variant CC tracks to the same Synchron destination.
3. Keep only one variant track active at a time while auditioning.
4. Choose a base variant (usually `NEUTRAL` or `SHORT`).
5. For local fixes, copy short passages from another variant track onto the base track.
6. Keep a single final consolidated CC track for print.

## Optional: Region-Limited Variant Pack

Add both region args:

1. `--region-start-tick <tick>`
2. `--region-end-tick <tick>`

This is useful for rapid local correction while preserving the same A/B method.

