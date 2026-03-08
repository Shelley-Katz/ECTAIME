# Two-Machine Operating Model (Current Production Setup)

## Purpose
Run production audio/MIDI processing entirely on Studio Mac while keeping this repo on MacBook Air as runbook + archive + trace.

## Machine Roles

### Studio Mac (M2, Sequestered Production)

Authoritative for:

- Dorico exports
- Digital Performer session and routing
- VEP/Synchron state
- DP Meter Pro live processing
- Final stem rendering

### MacBook Air (M1, Control and Trace)

Authoritative for:

- Procedures and checklists in this repo
- QC documentation and issue tracking
- Archived copies of exports/renders via AirDrop

## Non-Negotiable Rule

Do not treat files on MacBook Air as the active source for Studio runtime in this run.
All execution is performed on Studio Mac.

## Data Flow

1. Build and render on Studio Mac.
2. AirDrop selected artifacts to MacBook Air.
3. Store received files in structured archive folders.
4. Use archive and logs for traceability, reproducibility, and design feedback.

## Why This Model

- Preserves Studio stability and isolation.
- Avoids setup drift from cross-machine live editing.
- Gives complete project trace without disrupting production flow.
