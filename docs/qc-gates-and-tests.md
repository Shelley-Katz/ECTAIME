# QC Gates and Tests (V1 Cobbled ECT)

## Gate A: Routing Integrity

Pass conditions:

1. All three bridge tracks receive MIDI CC from their corresponding DPMP aux.
2. Synchron responds to CC1, CC11, CC21 from each bridge track.
3. No dropouts in a 2-minute continuous playback.

## Gate B: Profile Behavior

Pass conditions:

1. LONG: smooth cresc/dim, no pumping.
2. SHORT: responsive accents, no random spikes from tails.
3. REP: stable modulation over repeated patterns.

## Gate C: Boundary Quality

Pass conditions:

1. No audible controller discontinuities at profile changes.
2. Transition artifacts resolved by overlap/placement edits.

## Gate D: Cue Deliverable

Pass conditions:

1. One full V1 cue rendered and suitable for production workflow.
2. Nuancer pass can be applied without fighting unstable controller behavior.
3. Remaining issues are minor and documented.

## Test Scenarios

1. Long legato crescendo phrase.
2. Fast staccato ostinato phrase.
3. Repetition/trem/trill phrase.
4. Rest-heavy phrase to test tail behavior.
5. Mixed-articulation phrase with frequent transitions.
6. Re-render consistency check.

## Issue Taxonomy

Tag every issue as exactly one:

- `mapping` (bad CC range/shape)
- `gating` (wrong profile selection/timing)
- `infrastructure` (routing/RT/plugin behavior)
