# M1 Core Runbook (Offline Vertical Slice)

Use this runbook to execute ECT Core M1 on a single monophonic line.

## Prerequisites

1. Python environment with requirements:
   ```bash
   cd /Users/sk/ECT
   .venv/bin/pip install -r core/requirements.txt
   ```
2. Input files:
   - `source_midi.mid`
   - `source_audio.wav` (dry stem, aligned)
3. Optional:
   - articulation dataset JSON
   - config YAML/JSON

## Command

```bash
cd /Users/sk/ECT
.venv/bin/python core/ect_core_cli.py \
  --source-midi "Winterberg Vla 1 .mid" \
  --source-audio "/ABS/PATH/TO/VLA1_NP_DRY.wav" \
  --track-index 1 \
  --articulations "/Users/sk/MAXMSP Patches/Synchron_MFT_Master 2_WORK/articulations.json" \
  --output-dir qc_notes/m1_vla_run
```

## Expected Artifacts

1. `control_stream.mid`
2. `profile_timeline.csv`
3. `note_audit.csv`
4. `metrics.json`
5. `run_manifest.json`

## Notes

1. M1 control generation is deterministic and offline.
2. This vertical slice focuses on reliable artifacts/contracts first; musical tuning remains iterative.
3. In M1, `source_audio.wav` is a required contract input and hash-tracked artifact, but deep audio feature extraction is intentionally limited; richer audio-driven shaping is expanded in subsequent milestones.
