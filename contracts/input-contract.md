# Input Contract v1

## Required Inputs

1. `source_midi.mid`
   - Type 0 or Type 1 MIDI.
   - Contains notes for one target monophonic line.
   - May include keyswitch notes or articulation markers.
2. `source_audio.wav`
   - Dry stem corresponding to the same musical line.
   - No baked reverb/widening FX.
   - Timeline-aligned with `source_midi.mid`.
3. `config.yaml`
   - Runtime parameters for profile behavior and output mapping.

## Optional Inputs

1. `articulations.json`
   - Keyswitch/articulation mapping dataset.
2. `score_context.mid`
   - Full-score context for advanced future features.

## Preconditions

1. Source MIDI and source audio must share the same musical timeline origin.
2. Input files must be readable and hashable.
3. If source MIDI contains pre-existing expression CC (`CC1`, `CC11`, `CC21`), tool behavior must be explicit:
   - `strip`
   - `preserve`
   - `merge` (future)

