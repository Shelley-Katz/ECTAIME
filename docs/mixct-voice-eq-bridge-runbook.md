# MixCT Voice + EQ Bridge Runbook

This runbook covers the new voice bridge and EQ lane-safe control path in `mixct_app`.

## 1. What Is Implemented

1. Voice input pipeline:
- Apple-primary command bridge via env var `MIXCT_APPLE_STT_CMD`
- Local fallback command bridge via env var `MIXCT_LOCAL_STT_CMD`
- Typed fallback (interactive terminal input) if both STT paths are unavailable

2. Voice output pipeline:
- `say` on macOS when available
- console print fallback (`[TTS] ...`)

3. EQ lane-safe writes:
- Target lanes: `EqLowGain`, `EqPresenceGain`, `EqAirGain`
- Lane-safe mapping is fail-closed: if EQ lane mapping is missing, MixCT refuses that lane
- If protocol is `mcu` and EQ lanes have no MCU channel mapping, MixCT auto-falls back to `cc`

## 2. Session Map Requirements

Per bus in session map (optional fields):
- `eq_low_cc`
- `eq_presence_cc`
- `eq_air_cc`
- optional: `eq_low_mcu_channel`, `eq_presence_mcu_channel`, `eq_air_mcu_channel`
- optional: `eq_min_db`, `eq_max_db` (defaults `-4` / `+4`)

Waghalter map now includes `eq_*_cc` for all buses:
- file: `/Users/sk/ECT/contracts/mixct_session_map.waghalter.yaml`

## 3. STT Command Contract

Both STT commands should print either:

1. JSON:
```json
{"text":"Violins are too soft in bars 26-29", "confidence":0.91, "backend":"apple_speech"}
```

2. Plain text (single line):
```text
Violins are too soft in bars 26-29
```

## 4. Quick Tests

1. Voice transcription only:
```bash
cd /Users/sk/ECT/mixct_rust
cargo run -p mixct_app -- voice-transcribe
```

2. Voice -> execute path:
```bash
cd /Users/sk/ECT/mixct_rust
cargo run -p mixct_app -- voice-execute \
  --spec /Users/sk/ECT/docs/mixct_mvp_b_codex_spec.json \
  --session-map /Users/sk/ECT/contracts/mixct_session_map.waghalter.yaml \
  --audit-dir /Users/sk/ECT/qc_notes/voice_exec_test \
  --backend midi \
  --write-protocol mcu \
  --midi-out Network \
  --audio-device LP32
```

## 5. Notes

1. If `MIXCT_APPLE_STT_CMD` is unset or fails, MixCT automatically tries fallback.
2. If fallback is also unavailable, MixCT asks for typed command in terminal.
3. EQ-only passes use tighter bounds by default (`-4..+4 dB`, slew `1.5 dB/step`).
