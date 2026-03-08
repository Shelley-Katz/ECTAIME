# ECT Master Plan v0.2

Date: 2026-03-02
Status: Approved Draft for Build Start
Mode: Single-agent execution (Codex + SK)

## 1) Mission

Build a professional offline-first Expression Conversion Tool (ECT) that converts expressive behavior from source audio + score into reliable control streams for high-end orchestral libraries, with quality and repeatability suitable for production delivery.

## 2) Product Split (Locked)

ECT is split into two linked products:

1. `ECT Core`
   - Deterministic conversion engine.
   - Inputs: MIDI + audio + articulation mapping + config.
   - Outputs: CC streams, profile timeline, audits, run metrics.
2. `Symphonova Orchestrator`
   - Job pipeline and automation shell around Core.
   - Batch processing, run queue, logs, resume/retry, handoff artifacts.

This split is mandatory to keep the conversion engine DAW-agnostic and robust.

## 3) Scope and Non-Goals by Phase

### Phase 1 (MVP, Offline)

In scope:

1. Offline conversion for monophonic string lines.
2. Profiles: `LONG`, `SHORT`, `REP`.
3. Output CCs: `CC1`, `CC11`, optional `CC21`.
4. Variant rendering (`LONG-biased`, `SHORT-biased`, `REP-biased`) for A/B.
5. Full audit and reproducibility artifacts.

Out of scope:

1. Live real-time processing.
2. Full GUI.
3. Full Dorico/DP UI-driving automation.
4. Perfect automatic profile decisions for every edge case.

### Phase 2 (Batch Orchestration)

In scope:

1. Directory-based job processing.
2. Resume/retry and overnight processing.
3. Structured export packages for DP/Logic/Cubase workflows.

Out of scope:

1. Live score-following.
2. Mic-input live adaptation.

### Phase 3 (Real-Time Extension)

In scope:

1. Streaming mode from live audio.
2. Merge strategy: precomputed baseline + live adaptation.
3. Score-following assisted tracking.

## 4) Canonical Data Contracts (Locked)

### 4.1 Inputs

1. `source_midi.mid`
   - Notes and articulations/keyswitches.
   - Optional cleanup mode to strip prior dynamics CC.
2. `source_audio.wav`
   - Dry stem, no FX/reverb widening.
3. `articulations.json`
   - Mapping resource (Dorico/VSL/MFT style).
4. `config.yaml`
   - Thresholds, profile weights, output mapping.

### 4.2 Outputs

1. `control_stream.mid`
   - Generated `CC1/CC11/(CC21)` timeline.
2. `profile_timeline.csv`
   - Start/end/profile/source/reason.
3. `note_audit.csv`
   - Per-note decisions (highest granularity).
4. `metrics.json`
   - Objective QC metrics for this run.
5. `run_manifest.json`
   - Input hashes, config hash, code version, timestamps.
6. `variants/`
   - Optional candidate outputs for A/B listening.

## 5) Objective QC Gates (Locked)

Each milestone must pass all gates.

### Gate G1: Technical Integrity

1. Run completes without crash.
2. All required artifacts are emitted.
3. Manifest hashes are valid and reproducible.

### Gate G2: Controller Health

1. No long-duration CC pinning except declared climaxes.
2. No dead-flat CC spans during active musical material.
3. Bounded jitter metric within defined threshold per profile.

### Gate G3: Musical Utility

1. Sustained phrases: no obvious pumping in LONG.
2. Agile passages: no repeated note loss in SHORT/REP.
3. Mixed passages: no catastrophic boundary discontinuities.

### Gate G4: Reproducibility

1. Same inputs + same config + same code produce equivalent outputs.
2. Variant A/B outputs are stable across reruns.

## 6) Benchmark Corpus (Freeze Before Build)

Create and lock a benchmark set with IDs and short clips.

Required categories:

1. Long lyrical legato.
2. Fast scalar connected runs.
3. Repeated-note ostinato (3 short + 1 long type included).
4. Mixed articulation phrase.
5. Rest-heavy phrase.
6. Dense divisi excerpt.

Rule: tuning decisions are accepted only if they improve benchmark aggregate quality.

## 7) Implementation Milestones

### M0: Foundation

Deliverables:

1. Repo layout for `core/`, `orchestrator/`, `contracts/`, `benchmarks/`.
2. Contract docs and artifact templates.
3. Benchmark corpus manifest.

Exit criteria:

1. All contracts and gates approved.
2. Benchmark clips frozen.

### M1: Core Vertical Slice (Viola)

Deliverables:

1. Offline converter CLI.
2. LONG/SHORT/REP decision engine.
3. CC output + audits + manifest + metrics.

Exit criteria:

1. Pass G1-G4 on Viola benchmark clips.

### M2: Comparative Variant Mode

Deliverables:

1. Passage-local reprocess mode for A/B comparison (selected regions only).
2. Listening comparison package and selection notes.
3. Phase 1 fallback mode: full-score biased variants (`LONG`, `SHORT`, `REP`) when local reprocess is not yet wired.

Exit criteria:

1. A/B workflow usable for musical choice without rerouting hacks.
2. Conductor can request local passage alternatives without requiring whole-score rerender.

### M3: Cross-Instrument Expansion

Deliverables:

1. Cello, Violin, Double Bass calibration sets.
2. Shared defaults + per-instrument overrides.

Exit criteria:

1. Pass gates across string benchmark corpus.

### M4: DP Utility Layer (Optional in late Phase 1)

Deliverables:

1. Keyswitch cleanup utility.
2. KS-to-DP articulation conversion helper (export format defined).

Exit criteria:

1. QuickScribe-friendly score workflow proven on one cue.

### M5: Orchestrator Batch Engine (Phase 2 start)

Deliverables:

1. Directory job queue.
2. Run resume/retry.
3. Packaged outputs for DAW import.

Exit criteria:

1. Overnight batch on multi-cue set completes with auditable logs.

## 8) Risk Register (Top)

1. Overfitting to one cue.
   - Control: frozen benchmark corpus and aggregate metrics.
2. DAW lock-in.
   - Control: core outputs are standard MIDI/CSV/JSON.
3. Scope creep.
   - Control: phase non-goals locked.
4. Proprietary data handling risk.
   - Control: explicit data governance and local-only secure paths.

## 9) Security and Professional Readiness Baseline

1. Local-first processing by default.
2. Explicit run logs and deterministic manifests.
3. Crash-safe job records.
4. Config versioning and migration policy.

## 10) Execution Model

Single-agent until interfaces stabilize.

When to introduce parallel agents later:

1. Only after contracts are frozen and validated in M1.
2. Split by independent modules (core algorithms, orchestrator, adapters, tests).

## 11) Immediate Next Steps (Actionable)

1. Create `M0` artifacts and benchmark manifest.
2. Define `metrics.json` fields and thresholds.
3. Build `M1` CLI vertical slice on Viola only.
4. Run first gate review before any cello/violin/db expansion inside ECT core.
