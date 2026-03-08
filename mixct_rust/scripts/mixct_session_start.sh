#!/usr/bin/env bash
set -euo pipefail

# MixCT session-start bootstrap:
# 1) Runs response calibration on mapped section buses.
# 2) Stores timestamped calibration JSON in qc_notes.
# 3) Updates qc_notes/response_calibration.latest.json symlink.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
RUST_DIR="$ROOT/mixct_rust"
SESSION_MAP="${1:-$ROOT/contracts/mixct_session_map.waghalter.yaml}"
MIDI_OUT="${MIDI_OUT:-Network}"
AUDIO_DEVICE="${AUDIO_DEVICE:-LP32}"
WRITE_PROTOCOL="${WRITE_PROTOCOL:-mcu}"
BASELINE_DB="${BASELINE_DB:-0}"
TEST_DELTA_DB="${TEST_DELTA_DB:-3}"
CAPTURE_SEC="${CAPTURE_SEC:-2}"
SETTLE_MS="${SETTLE_MS:-350}"
TRANSPORT_CHECK_SEC="${TRANSPORT_CHECK_SEC:-3}"

RUN_ID="$(date +%Y%m%d_%H%M%S)"
CAL="$ROOT/qc_notes/response_calibration_${RUN_ID}.json"
LATEST="$ROOT/qc_notes/response_calibration.latest.json"

echo "mixct_session_start: root=$ROOT"
echo "mixct_session_start: session_map=$SESSION_MAP"
echo "mixct_session_start: midi_out=$MIDI_OUT audio_device=$AUDIO_DEVICE"
echo "mixct_session_start: calibration_out=$CAL"
echo "mixct_session_start: reminder -> enable DAW MTC transmit if you want transport tracking"

cd "$RUST_DIR"
cargo run -p mixct_app -- calibrate-response \
  --session-map "$SESSION_MAP" \
  --midi-out "$MIDI_OUT" \
  --audio-device "$AUDIO_DEVICE" \
  --baseline-db "$BASELINE_DB" \
  --test-delta-db "$TEST_DELTA_DB" \
  --capture-sec "$CAPTURE_SEC" \
  --settle-ms "$SETTLE_MS" \
  --write-protocol "$WRITE_PROTOCOL" \
  --out "$CAL"

ln -sfn "$(basename "$CAL")" "$LATEST"
echo "mixct_session_start: latest -> $LATEST"

echo "mixct_session_start: running transport monitor check (${TRANSPORT_CHECK_SEC}s)..."
cargo run -p mixct_app -- transport-monitor \
  --midi-in "$MIDI_OUT" \
  --duration-sec "$TRANSPORT_CHECK_SEC" \
  --poll-ms 200 \
  --tempo-bpm 120 \
  --ts-num 4 \
  --ts-den 4 \
  --ppq 480 || true

echo "mixct_session_start: done"
echo "CAL=$CAL"
