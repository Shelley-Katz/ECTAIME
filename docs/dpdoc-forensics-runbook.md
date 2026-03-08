# DPDOC Forensics Runbook

Purpose: isolate where a single articulation edit appears inside `.dpdoc`, while filtering out ordinary DP save noise.

## 1) Create The Three Reference Files

Use the same source project and same machine/session:

1. `Base.dpdoc`
   - Open project, do nothing musical, save as `Base.dpdoc`.
2. `SaveOnly.dpdoc`
   - Open `Base.dpdoc`, do nothing, save immediately as `SaveOnly.dpdoc`.
3. `OneArtic.dpdoc`
   - Open `Base.dpdoc`, change exactly one note articulation, save as `OneArtic.dpdoc`.

Important:
- Keep all three in one folder.
- Do not run extra actions (sync, remap, route edits, track renames).
- One change only: one note articulation.

## 2) Run Forensic Analysis

From repo root:

```bash
cd /Users/sk/ECT
.venv/bin/python scripts/dpdoc_forensics.py \
  --base "/ABS/PATH/Base.dpdoc" \
  --saveonly "/ABS/PATH/SaveOnly.dpdoc" \
  --oneartic "/ABS/PATH/OneArtic.dpdoc" \
  --output-dir "/ABS/PATH/dpdoc_forensics_out"
```

Optional tightening:

```bash
cd /Users/sk/ECT
.venv/bin/python scripts/dpdoc_forensics.py \
  --base "/ABS/PATH/Base.dpdoc" \
  --saveonly "/ABS/PATH/SaveOnly.dpdoc" \
  --oneartic "/ABS/PATH/OneArtic.dpdoc" \
  --output-dir "/ABS/PATH/dpdoc_forensics_out" \
  --noise-overlap-threshold 0.25 \
  --insertion-tolerance 16 \
  --top 40
```

## 3) Read Outputs

`summary.json`
- high-level counts for save noise vs one-articulation delta.

`runs_save_to_one_all.csv`
- every non-equal run from `SaveOnly -> OneArtic`.

`runs_save_to_one_candidates.csv`
- ranked candidate runs likely related to the articulation change.

Use candidate rows with:
- low `overlap_ratio_with_save_noise`
- nontrivial `size_score`
- meaningful `save_ascii_context` / `one_ascii_context`

## 4) Next Pass (If Needed)

If candidate list is still noisy:

1. repeat capture with stricter discipline (only one articulation click, immediate save).
2. rerun with tighter thresholds.
3. if needed, run multiple one-change cases (different articulations) and intersect recurring candidate offsets.
