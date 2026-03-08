# Output Contract v1

## Required Artifacts

1. `control_stream.mid`
   - Contains generated controller events:
     - `CC1` (VelXF)
     - `CC11` (Expression)
     - optional `CC21` (attack-related)
2. `profile_timeline.csv`
   - Region-level profile decisions.
3. `note_audit.csv`
   - Per-note high-granularity decision record.
4. `metrics.json`
   - Objective QC metrics for gates.
5. `run_manifest.json`
   - Reproducibility metadata and hashes.

## Optional Artifacts

1. `variants/`
   - Alternative candidate outputs (bias variants or passage-local A/B).
2. `logs/`
   - Runtime debug logs.

## Artifact Semantics

1. Every required artifact must include enough metadata to be traceable to the input set and config.
2. Artifact filenames are stable and machine-readable.
3. Failure to produce any required artifact marks the run as `invalid`.

