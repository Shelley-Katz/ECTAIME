# ECT Core

Purpose: deterministic offline conversion engine.

Current scripts:

1. `ect_core_cli.py`
2. `ect_variant_pack.py`
3. `ect_score_job_cli.py`

Responsibilities:

1. Parse source MIDI and derive profile decisions (`LONG`, `SHORT`, `REP`).
2. Generate controller streams (`CC1`, `CC11`; `CC21` reserved).
3. Emit canonical artifacts:
   - `control_stream.mid`
   - `profile_timeline.csv`
   - `note_audit.csv`
   - `metrics.json`
   - `run_manifest.json`
4. Build fast A/B variant packs (neutral + profile-biased) for DAW audition.
5. Process per-score Dorico packages into DP-ready triplet-track MIDI (`Instrument`, `Instrument KS`, `Instrument ArtMap`) with regenerated `CC1/CC11`.
