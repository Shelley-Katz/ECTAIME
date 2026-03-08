# MixCT MVP-B Physical Validation Runbook (Two-Machine, AVB + Apple MIDI Network)

## Scope (Read First)

1. This runbook validates the current `mixct_app` build in its real operator environment (two machines, AVB network present, Apple MIDI network present).
2. The current `mixct_app` build **does** validate command routing, planning, execution gating, undo-anchor/restore flow, and audit logging.
3. The current `mixct_app` build **does not yet** write real DP automation lanes or move DP faders in this phase; write/restore uses the internal mock backend.
4. Therefore, the physical test result is: “command pipeline and safety logic verified in production topology,” not yet “DP automation physically moved.”

## Files Used

1. Spec: [mixct_mvp_b_codex_spec.json](/Users/sk/ECT/docs/mixct_mvp_b_codex_spec.json)
2. Session map: [mixct_session_map.waghalter.yaml](/Users/sk/ECT/contracts/mixct_session_map.waghalter.yaml)
3. App workspace: `/Users/sk/ECT/mixct_rust`

## Phase 0: Preconditions

1. Confirm both Macs are powered and logged in:
   1. Studio Mac (M2): runs DP + VEP.
   2. Processing Mac (M1): runs MixCT app.
2. Confirm MOTU devices are powered:
   1. `MOTU 848` (clock authority).
   2. `MOTU LP32` (processing-side interface) if present in your rig.
3. Confirm wired Ethernet is available for:
   1. AVB audio plane device network.
   2. Apple Network MIDI between the two Macs.
4. Disable Wi‑Fi and Bluetooth on both Macs for the test window.

## Phase 1: Physical Cabling

1. Connect Studio Mac to `MOTU 848` via USB/Thunderbolt.
2. Connect Processing Mac to `MOTU LP32` via USB (if LP32 is in use).
3. Connect `MOTU 848` Ethernet to AVB switch.
4. Connect `MOTU LP32` Ethernet to AVB switch (if LP32 is in use).
5. Connect both Macs to wired Ethernet for Apple Network MIDI.
6. Verify link lights on switch ports are active.

## Phase 2: AVB Network and Clock

1. On Studio Mac, open browser and load MOTU web control (for example `motu-avb.local` or device IP).
2. Set `MOTU 848` as AVB/PTP grandmaster clock authority:
   1. Sample rate: set and hold one value for whole run (recommended `48 kHz`).
   2. Clock source: internal/master.
3. On `MOTU LP32` (if used), set clock to follow AVB/PTP network grandmaster.
4. Verify AVB lock on all participating devices:
   1. No clock mismatch warnings.
   2. No unlock indicators.
5. Do not change sample rate after this point.

## Phase 3: Apple Network MIDI Setup (Both Macs)

1. On each Mac open `Audio MIDI Setup` (`/Applications/Utilities/Audio MIDI Setup.app`).
2. Click `Window` -> `Show MIDI Studio`.
3. Double-click `Network`.
4. In `My Sessions`:
   1. Create one session if none exists (`+`).
   2. Name it clearly:
      1. Studio Mac: `Studio-MIDI-Net`.
      2. Processing Mac: `Processing-MIDI-Net`.
   3. Tick `Enabled`.
5. In `Directory`, locate the other Mac.
6. Click `Connect` so each session shows the other machine in `Participants`.
7. Confirm both sides show connected participants.

## Phase 4: Studio Mac DAW Baseline

1. Launch VEP server and load the Waghalter VEP project you normally use.
2. Launch Digital Performer and open the Waghalter project/chunk.
3. Confirm project sample rate matches AVB clock/sample rate.
4. Confirm transport and audio playback are normal.
5. Open DP MIDI Monitor window and keep it visible.
6. Do not perform manual DP Undo during MixCT session.

## Phase 5: Processing Mac MixCT Baseline

1. Open Terminal on Processing Mac.
2. Run:

```bash
cd /Users/sk/ECT/mixct_rust
rustc --version
cargo --version
```

3. Build once:

```bash
cargo build -p mixct_app
```

4. Prepare test output folder with timestamp:

```bash
RUN_ID=$(date +%Y%m%d_%H%M%S)
AUDIT_DIR=/Users/sk/ECT/qc_notes/mixct_mvp_b_physical_${RUN_ID}
mkdir -p "$AUDIT_DIR"
echo "$AUDIT_DIR"
```

5. Set reusable paths:

```bash
SPEC=/Users/sk/ECT/docs/mixct_mvp_b_codex_spec.json
SESSION=/Users/sk/ECT/contracts/mixct_session_map.waghalter.yaml
```

## Phase 6: Diagnostics and Plan

1. Run diagnostics:

```bash
cargo run -p mixct_app -- diagnostics --spec "$SPEC" --session-map "$SESSION" | tee "$AUDIT_DIR/diagnostics.out"
```

2. Pass condition:
   1. Terminal prints exactly `diagnostics: OK`.
3. Generate plan:

```bash
cargo run -p mixct_app -- plan \
  --spec "$SPEC" \
  --command "Violins are too soft in bars 26-29" \
  --session-map "$SESSION" \
  --out "$AUDIT_DIR/sample_plan.json" | tee "$AUDIT_DIR/plan.out"
```

4. Pass condition:
   1. File exists: `"$AUDIT_DIR/sample_plan.json"`.

## Phase 7: Execute / Suggest / Clarify / Restore

1. Execute actionable command:

```bash
NOW=$(date -u +%Y-%m-%dT%H:%M:%SZ)
cargo run -p mixct_app -- execute \
  --spec "$SPEC" \
  --command "Violins are too soft in bars 26-29" \
  --session-map "$SESSION" \
  --audit-dir "$AUDIT_DIR" \
  --command-id "cmd-live-1" \
  --captured-at "$NOW" | tee "$AUDIT_DIR/execute_actionable.out"
```

2. Pass condition:
   1. Terminal prints `execute: OK`.
3. Run suggest path:

```bash
NOW=$(date -u +%Y-%m-%dT%H:%M:%SZ)
cargo run -p mixct_app -- execute \
  --spec "$SPEC" \
  --command "What do you suggest for bars 26-29?" \
  --session-map "$SESSION" \
  --audit-dir "$AUDIT_DIR" \
  --command-id "cmd-suggest-1" \
  --captured-at "$NOW" | tee "$AUDIT_DIR/execute_suggest.out"
```

4. Pass condition:
   1. Terminal prints `suggestions:` and 3 numbered options.
5. Run clarify path:

```bash
NOW=$(date -u +%Y-%m-%dT%H:%M:%SZ)
cargo run -p mixct_app -- execute \
  --spec "$SPEC" \
  --command "Make it better" \
  --session-map "$SESSION" \
  --audit-dir "$AUDIT_DIR" \
  --command-id "cmd-clarify-1" \
  --captured-at "$NOW" | tee "$AUDIT_DIR/execute_clarify.out"
```

6. Pass condition:
   1. Terminal prints `clarify:` with a concrete prompt.
7. Run restore:

```bash
cargo run -p mixct_app -- restore \
  --spec "$SPEC" \
  --command-id "cmd-live-1" \
  --audit-dir "$AUDIT_DIR" | tee "$AUDIT_DIR/restore.out"
```

8. Pass condition:
   1. Terminal prints `restore: OK`.

## Phase 8: Safety Guards (Must Refuse)

1. Duplicate command ID refusal:

```bash
set +e
NOW=$(date -u +%Y-%m-%dT%H:%M:%SZ)
cargo run -p mixct_app -- execute \
  --spec "$SPEC" \
  --command "Violins are too soft in bars 26-29" \
  --session-map "$SESSION" \
  --audit-dir "$AUDIT_DIR" \
  --command-id "cmd-live-1" \
  --captured-at "$NOW" > "$AUDIT_DIR/duplicate_guard.out" 2>&1
DUP_RC=$?
set -e
echo "DUP_RC=$DUP_RC"
```

2. Pass condition:
   1. `DUP_RC` must be non-zero (`1` expected).
3. Stale context refusal:

```bash
set +e
cargo run -p mixct_app -- execute \
  --spec "$SPEC" \
  --command "Violins are too soft in bars 26-29" \
  --session-map "$SESSION" \
  --audit-dir "$AUDIT_DIR" \
  --command-id "cmd-stale-1" \
  --captured-at "2020-01-01T00:00:00Z" \
  --max-command-age-sec 120 > "$AUDIT_DIR/stale_guard.out" 2>&1
STALE_RC=$?
set -e
echo "STALE_RC=$STALE_RC"
```

4. Pass condition:
   1. `STALE_RC` must be non-zero (`1` expected).

## Phase 9: Audit Verification

1. Confirm audit file exists:

```bash
test -f "$AUDIT_DIR/pass_audit.jsonl" && echo "audit file OK"
```

2. Count required event types:

```bash
rg -c '"event_type":"execute"' "$AUDIT_DIR/pass_audit.jsonl"
rg -c '"event_type":"suggest"' "$AUDIT_DIR/pass_audit.jsonl"
rg -c '"event_type":"clarify"' "$AUDIT_DIR/pass_audit.jsonl"
rg -c '"event_type":"restore"' "$AUDIT_DIR/pass_audit.jsonl"
```

3. Pass condition:
   1. Each count should be at least `1`.

## Phase 10: Pass/Fail Decision

1. Declare `FULL PASS` only if all conditions are true:
   1. Diagnostics OK.
   2. Plan file created.
   3. Execute OK.
   4. Suggest path returned options.
   5. Clarify path returned prompt.
   6. Restore OK.
   7. Duplicate guard refused.
   8. Stale guard refused.
   9. Audit JSONL contains execute/suggest/clarify/restore events.
2. If any condition fails:
   1. Stop.
   2. Capture terminal output files from `"$AUDIT_DIR"`.
   3. Report exact failing step and exact output.

## Optional: One-Line Combined Smoke Run

1. Use this only after you have successfully done the full manual run once.

```bash
cd /Users/sk/ECT/mixct_rust
SPEC=/Users/sk/ECT/docs/mixct_mvp_b_codex_spec.json
SESSION=/Users/sk/ECT/contracts/mixct_session_map.waghalter.yaml
RUN_ID=$(date +%Y%m%d_%H%M%S)
AUDIT_DIR=/Users/sk/ECT/qc_notes/mixct_mvp_b_physical_${RUN_ID}
mkdir -p "$AUDIT_DIR"

cargo run -p mixct_app -- diagnostics --spec "$SPEC" --session-map "$SESSION" | tee "$AUDIT_DIR/diagnostics.out" &&
cargo run -p mixct_app -- plan --spec "$SPEC" --command "Violins are too soft in bars 26-29" --session-map "$SESSION" --out "$AUDIT_DIR/sample_plan.json" | tee "$AUDIT_DIR/plan.out" &&
NOW=$(date -u +%Y-%m-%dT%H:%M:%SZ) &&
cargo run -p mixct_app -- execute --spec "$SPEC" --command "Violins are too soft in bars 26-29" --session-map "$SESSION" --audit-dir "$AUDIT_DIR" --command-id "cmd-live-1" --captured-at "$NOW" | tee "$AUDIT_DIR/execute_actionable.out" &&
cargo run -p mixct_app -- restore --spec "$SPEC" --command-id "cmd-live-1" --audit-dir "$AUDIT_DIR" | tee "$AUDIT_DIR/restore.out" &&
echo "SMOKE PASS: $AUDIT_DIR"
```
