# ECT Score Job Runbook

## Goal

Process one Dorico score package into one DP-ready MIDI file where each instrument is expanded to:

1. `Instrument` (musical notes + regenerated `CC1`/`CC11`; KS removed)
2. `Instrument KS` (keyswitch notes only)
3. `Instrument ArtMap` (duplicate musical notes only)

## Input Package (Per Score Folder)

At folder root:

1. One `.mid` file exported from Dorico (with VSL keyswitches).
2. One stems directory with dry NotePerformer audio exports.

## Command

From `/Users/sk/ECT`:

```bash
.venv/bin/python core/ect_score_job_cli.py \
  --job-dir score_conversion_drop/inbox/<SCORE_FOLDER> \
  --output-root score_conversion_drop/outbox
```

## Outputs

Under `score_conversion_drop/outbox/<SCORE_FOLDER>/`:

1. `<MidiName>__DP_PREP.mid`
2. `<MidiName>__DP_PREP_QC.json`

## DP Import Hygiene

1. Import into a clean chunk/sequence when validating a new conversion build.
2. Re-importing into an already populated chunk can create DP auto-renamed duplicates (`/1`, `/2`, etc.), which can look like extraneous tracks but are import-side duplicates.

## Behavior Notes

1. Empty/KS-only note tracks are removed and logged in QC.
2. Existing `CC1` and `CC11` are stripped from source note tracks before regenerated CC is inserted.
3. Source non-note channel events are dropped from output note tracks (notes + regenerated `CC1`/`CC11` only), preventing DP split-import artifacts.
4. If a stem is assigned and analyzable, regenerated CC is WAV-envelope-derived (dense over note duration); otherwise generation falls back to MIDI-note logic.
5. Stems are discovered and matched to track names for traceability in QC.
6. KS separation uses strict lookup-table-first detection with range-aware guards:
   - KS lookup pool from articulations dataset.
   - Chord/latch KS recognition when possible.
   - Instrument/core-range guards for ambiguous notes.
7. Track keep/drop is content-based (musical salience after KS split), not name-based.
