# MixCT Orchestral Intelligence KB v1 Review (2026-03-08)

Source reviewed:
- `/Users/sk/ECT/MixCT Research Data/mixct_orchestral_intelligence_kb_v1.json`
- Cross-check summary: `/Users/sk/ECT/MixCT Research Data/MixCT_ultra_short_summary.md`

## Immediate MVP-B Useful Items (Adopt Now)

1. Doctrine and control priorities
- DP remains source-of-truth for automation/session state.
- Classical default stays `main image first`, support spots assist only.
- Keep the action order conservative: reduce maskers -> section/family balance -> spatial ratio -> spot help -> EQ last.

2. Bounded actions and safety
- Keep moves small and reversible (already aligned with current guarded approach).
- Keep gain ceilings near KB defaults:
  - main image <= +0.5 dB
  - family <= +1.5 dB
  - section <= +2.0 dB
  - spot <= +2.0 dB
- Preserve stop/write/restore discipline and explicit refusal on unsafe conditions.

3. Dynamics handling
- Automation-first policy (compression not first-response).
- Use time-scale separation (micro / phrase / section) and hysteresis.
- Do not react to every transient as a state change.

4. Audio-analysis feature priorities
- Prioritize direct/ambient/main relations, center stability, masking overlap, primary-line salience.
- Use blend risk + masking risk explicitly for recommendation ranking.

5. Explainability requirements
- Ensure decision records include:
  - role hypothesis
  - evidence summary
  - candidate + rejected actions
  - confidence
  - policy checks
  - expected effect

6. Validation expectations for physical tests
- Keep "do nothing" test cases.
- Track metrics:
  - false intervention rate
  - restore success rate
  - mean action size
  - image damage incidents
  - override rate

## Useful But Deferred (Backlog for Next Integration Pass)

1. Full role taxonomy integration (`primary/co-primary/secondary/foundation/atmosphere`) into runtime state and prompts.
2. Formal blend-model feature pipeline (`onset synchrony`, `harmonicity`, `parallel motion`, `grouping`, `timbre contrast`, `spectral overlap`).
3. Project adaptation layer implementation (`universal doctrine`, `project calibration`, `preference layer`).
4. Expanded validation harness aligned to all repertoire classes in the KB.
5. Formal schema alignment between KB and runtime/session-map contracts.

## Integration Note

This KB is useful and aligned with our current direction.  
It is not a drop-in executable policy, but it materially improves:
- safety bounds,
- action ranking,
- explanation/audit format,
- and validation criteria.

Next planned use:
- map selected KB constraints into `mixct_mvp_b_codex_spec.json`,
- then mirror the same fields into command/execute audit output.
