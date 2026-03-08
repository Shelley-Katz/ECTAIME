# MIDI Region Helper (Optional but Recommended)

This helper script compiles first-pass switching maps from Dorico MIDI, using:

1. Articulation-first detection from your MFT/OrchAPP `articulations.json` keyswitch chords.
2. KS boundary recovery + duration/IOI refinement for edge cases.
3. Duration/IOI fallback when articulation cues are missing.

## Install

```bash
python3 -m venv .venv
source .venv/bin/activate
pip install -r scripts/requirements.txt
```

## Run

```bash
python3 scripts/midi_profile_regions.py \
  --midi dorico_exports/V1.mid \
  --track-index 0 \
  --dataset "/Users/sk/MAXMSP Patches/Synchron_MFT_Master 2_WORK/articulations.json" \
  --ks-detection auto \
  --mode long_short \
  --merge-gap-ms 18 \
  --switch-pre-roll-ms 25 \
  --ks-lookahead-ms 90 \
  --ks-lookbehind-ms 70 \
  --ks-refine-boundary-ms 120 \
  --texture-confirm-long 3 \
  --texture-confirm-short 3 \
  --texture-long-min-ms 300 \
  --texture-short-max-ms 220 \
  --texture-legato-gap-max-ms 18 \
  --texture-legato-min-note-ms 120 \
  --texture-short-gap-min-ms 24 \
  --texture-fast-long-ms 430 \
  --texture-fast-long-window-notes 5 \
  --texture-fast-long-max-gap-ms 20 \
  --output-note-audit-csv qc_notes/V1_note_audit.csv \
  --output-csv qc_notes/V1_profile_regions.csv \
  --output-switch-mid qc_notes/V1_switch_control.mid
```

## Result

The script creates:

1. `V1_note_audit.csv` (optional): one row per note, no merge (finest audit view).
2. `V1_profile_regions.csv`: merged switch regions for practical DAW use.
3. `V1_switch_control.mid`: CC-based bypass state changes for import into DP.

Use the CSV for audit and manual correction.
Use the switch MIDI for automated LONG/SHORT switching.

Recommended DP mapping for switch MIDI:

1. Learn `CC90` to `ECT_V1_LONG` DPMP `Bypass`.
2. Learn `CC91` to `ECT_V1_SHORT` DPMP `Bypass`.
3. In generated MIDI:
`0 = active (bypass off)`, `127 = inactive (bypass on)`.

## Notes

- This is a heuristic+articulation compiler, not final musical truth.
- Manual overrides are expected on difficult boundaries.
- If your Studio run uses a different articulation dataset, pass its path with `--dataset`.
- `--ks-detection auto` is recommended for Dorico+VSL exports, because many files use held (latched) keyswitch notes.
- `--switch-pre-roll-ms` is important for reliability: it sends the incoming profile slightly before the first note.
