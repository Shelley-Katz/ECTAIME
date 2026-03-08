# Semi-Auto Gating Playbook (V1)

## Goal
Use score-informed heuristics to pre-populate profile sections, then manually refine by ear.

## Profile Rules (First Pass)

- `LONG`: note duration >= 300 ms
- `SHORT`: note duration <= 220 ms
- `REP`: repeated-note/trill/trem behavior with IOI <= 260 ms

## Gating Mechanics in DP

1. Add mute automation lanes for:
   - `BR_V1_LONG`
   - `BR_V1_SHORT`
   - `BR_V1_REP`
2. Populate first-pass blocks by region according to rules.
3. Ensure exclusivity (only one bridge unmuted at a time).
4. Add 30-50 ms overlap at transitions.
5. Audit boundaries by listening in context.

## Manual Override Policy

- If phrase intent conflicts with heuristic, musical intent wins.
- Use local override blocks for transitions, grace-note clusters, expressive pickups.

## Fast Boundary Fix Checklist

1. Jump at transition -> increase overlap to 50-70 ms.
2. Smear or mush -> shorten overlap to 20-30 ms.
3. Attack chatter in sustained phrase -> switch to LONG earlier.
4. Flattened rhythmic drive -> switch to SHORT/REP earlier.

## Fallback Simplification

If REP adds limited value in current cue:

- Disable `BR_V1_REP`
- Operate in two-profile mode (`LONG` + `SHORT`)
- Record decision in QC log
