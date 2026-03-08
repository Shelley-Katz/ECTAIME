# MixCT MVP-A Runbook (DP-first Offline)

## Goal

Generate bar-range bus automation from language directives, then import into DP for audition.

## Inputs

1. Source MIDI (DP-ready score), example:
   - [Waghalter__DP_PREP_WAV_DENSE.mid](/Users/sk/ECT/score_conversion_drop/outbox/Waghalter/Waghalter__DP_PREP_WAV_DENSE.mid)
2. Session map:
   - [mixct_session_map.waghalter.yaml](/Users/sk/ECT/contracts/mixct_session_map.waghalter.yaml)
3. Directive text file:
   - [mixct_directives.example.txt](/Users/sk/ECT/contracts/mixct_directives.example.txt)

## Command

From `/Users/sk/ECT`:

```bash
.venv/bin/python orchestrator/mixct_mvp_a_cli.py \
  --source-midi score_conversion_drop/outbox/Waghalter/Waghalter__DP_PREP_WAV_DENSE.mid \
  --session-map contracts/mixct_session_map.waghalter.yaml \
  --directives contracts/mixct_directives.example.txt \
  --output-dir score_conversion_drop/outbox/Waghalter/mixct_mvp_a
```

## Outputs

In `score_conversion_drop/outbox/Waghalter/mixct_mvp_a/`:

1. `<source>__MIXCT_AUTOMATION.mid`
   - One control track per bus (MIDI CC curves).
2. `<source>__MIXCT_PLAN.csv`
   - Per-bar target dB table by bus.
3. `<source>__MIXCT_AUDIT.json`
   - Parsed directives, track assignment, event counts.

## DP Import Test Procedure

1. Import `__MIXCT_AUTOMATION.mid` into the DP project/chunk.
2. Route each MixCT control track to the matching bus actuator target.
3. Playback and verify:
   - transitions are smooth (no hard jumps),
   - assigned PRIMARY buses are clearly forward,
   - accompaniment remains supportive without collapse.
4. Compare against `__MIXCT_PLAN.csv` to confirm expected bar-window behavior.

## Directive Writing Tips

For deterministic parsing, prefer:

```text
bars 33-40: STR_HI=PRIMARY, WW_HI=PRIMARY, HN=SECONDARY, REST=ACCOMPANIMENT
```

Natural style also supported:

```text
Bars 33 through 40, first violins and high ww to be primary, horns to be secondary, the rest to be accompaniment.
```

## Current MVP-A Limits

1. Offline render only (no live speech yet).
2. Single-lane role policy per bus (no spectral masking correction loop yet).
3. DP integration assumes pre-configured bus control mapping.
