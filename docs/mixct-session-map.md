# MixCT Session Map (DP, MVP-A)

## Purpose

`Session Map` is the contract between MixCT and your DP project.  
It defines:

1. Which instrument tracks belong to which musical bus.
2. Which MIDI CC/channel controls each bus gain actuator.
3. Role-to-level policy (PRIMARY / COUNTERPOINT / SECONDARY / ACCOMPANIMENT).

Without this map, language directives are ambiguous.

## Control Model (MVP-A)

1. MixCT outputs one MIDI CC control track per bus.
2. Each bus has:
   - unique `cc`
   - `channel`
   - dB working range (`min_db`, `max_db`)
   - neutral baseline `default_db`
3. Role command maps to target dB:
   - `target_db = default_db + role_db_offsets[role]`

## Standard Bus Set (Waghalter-ready)

1. `STR_HI` (Violin I, Violin II)
2. `STR_MID` (Viola)
3. `STR_LO` (Violoncello, Double bass)
4. `WW_HI` (Piccolo, Flutes, Oboes)
5. `WW_LO` (EH, Clarinets, BCl, Bassoons, Cbsn)
6. `HN` (4 Horns)
7. `TPT` (3 Trumpets)
8. `BR_LO` (Trombones + Tuba)
9. `PERC` (Timpani, Triangle, Suspended Cymbal)
10. `HARP` (Harp)

## File

Use:
- [mixct_session_map.waghalter.yaml](/Users/sk/ECT/contracts/mixct_session_map.waghalter.yaml)

Template for future cues:
- [mixct_session_map.template.yaml](/Users/sk/ECT/contracts/mixct_session_map.template.yaml)

## Alias Layer

`entity_aliases` lets free text resolve to buses:

1. `first violins` -> `STR_HI`
2. `high ww` -> `WW_HI`
3. `four horns` -> `HN`
4. `low brass` -> `BR_LO`
5. `percussion` -> `PERC`

Extend this list per your own vocabulary.

## DP Wiring Contract

For each bus actuator:

1. Insert one MIDI-learn-capable gain control target on that bus (plugin or mapped VCA control path).
2. Learn to the bus-assigned `(channel, cc)` from session map.
3. Keep range behavior linear and predictable across all buses.

## Validation Checklist

1. Every main instrument track is assigned to exactly one bus.
2. No KS/ArtMap tracks are included in bus membership.
3. Every bus has unique `(channel, cc)`.
4. dB ranges are consistent across buses unless intentionally different.
