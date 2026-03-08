# Session Map (V1 Only)

## Objective
Build a repeatable V1 pipeline that transforms NP stem behavior into Synchron control data while preserving Dorico MIDI notes/articulations.

## Machine Boundary (Strict)

- `Studio Mac (M2)`:
  - All DAW runtime work (Dorico, DP, VEP, renders).
  - Authoritative session state for production.
- `MacBook Air (M1)`:
  - Runbooks, QC templates, and archive copies only.
  - Not the runtime source for the current production pass.

## Fixed Track Names

### Source
- `SRC_MIDI_V1` (Dorico note + KS stream)
- `SRC_AUDIO_V1_NP_DRY` (dry centered NP audio stem)

### Processing Auxes
- `ECT_V1_LONG`
- `ECT_V1_SHORT`
- `ECT_V1_REP`

### MIDI Bridge
- `BR_V1_LONG`
- `BR_V1_SHORT`
- `BR_V1_REP`

### Destination
- `DST_V1_SYNC` (VEP/Synchron V1)

## Bus/Routing Contract

- `SRC_AUDIO_V1_NP_DRY` sends post-fader unity to all three ECT auxes.
- Each ECT aux hosts one DP Meter Pro instance.
- Each DP Meter Pro emits MIDI to one corresponding bridge track.
- All bridge tracks output to `DST_V1_SYNC`.
- `SRC_MIDI_V1` also outputs to `DST_V1_SYNC` for notes/keyswitches.

## Controller Contract

- `CC1`: VelXF primary macro dynamics
- `CC11`: Expression fine trim
- `CC21`: Attack behavior (if patch supports)

## Gating Contract

- At any instant, only one bridge track is active (unmuted).
- Boundary overlap: 30-50 ms between profiles.
- Emergency manual override must be available.

## File Handoff Contract (Studio -> AirDrop -> MacBook)

- Dorico exports (archive): `V1.mid`, `V1_NP_DRY.wav`
- Render archive: final V1 stems from DP render pass
- Optional trace artifacts: DP project snapshot, VEP project/preset export, cue notes
