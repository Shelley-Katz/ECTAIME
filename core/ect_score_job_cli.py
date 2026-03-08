#!/usr/bin/env python3
"""
ECT Score Job CLI

Per-score offline processor for the current DP workaround workflow.

Input (job folder):
1) One Dorico-exported MIDI file at job root.
2) One stems folder containing NotePerformer WAV files.

Output:
1) One DP-ready MIDI with, per instrument:
   - <Inst>        : musical notes + regenerated CC1/CC11 (KS removed)
   - <Inst> KS     : keyswitch notes only
   - <Inst> ArtMap : duplicated musical notes only (for local DP articulation edits)
2) One QC JSON report.
"""

from __future__ import annotations

import argparse
import json
import math
import re
import statistics
import subprocess
import sys
from bisect import bisect_right
from collections import Counter, defaultdict, deque
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Deque, Dict, Iterable, List, Optional, Sequence, Set, Tuple

import mido


REPO_ROOT = Path(__file__).resolve().parents[1]
CORE_DIR = REPO_ROOT / "core"
SCRIPTS_DIR = REPO_ROOT / "scripts"
if str(CORE_DIR) not in sys.path:
    sys.path.insert(0, str(CORE_DIR))
if str(SCRIPTS_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPTS_DIR))

import ect_core_cli as ecc  # noqa: E402
import midi_profile_regions as mpr  # noqa: E402


DROP_ROOT_DEFAULT = REPO_ROOT / "score_conversion_drop"
INBOX_DEFAULT = DROP_ROOT_DEFAULT / "inbox"
OUTBOX_DEFAULT = DROP_ROOT_DEFAULT / "outbox"


@dataclass
class NotePair:
    on_index: int
    off_index: int
    start_tick: int
    end_tick: int
    pitch: int
    channel: int
    velocity: int


@dataclass
class AbsEvent:
    tick: int
    priority: int
    order: int
    msg: mido.Message


@dataclass
class AudioEnvelope:
    times_sec: List[float]
    rms_db: List[float]


@dataclass
class TrackPrepResult:
    source_track_index: int
    source_track_name: str
    output_base_name: str
    output_notes_name: str
    output_ks_name: str
    output_artmap_name: str
    note_count: int
    ks_note_count: int
    stripped_cc_counts: Dict[str, int]
    detection_mode: str
    profile_counts: Dict[str, int]
    ks_diagnostics: Dict[str, Any]
    dataset_path: Optional[str]
    dataset_code: Optional[str]
    kept_non_note_messages: int
    dropped_non_note_messages: int
    cc_generation_mode: str
    generated_cc_counts: Dict[str, int]
    generated_cc_density_per_min: Dict[str, float]
    generated_cc_span_sec: float
    audio_envelope_points: int
    audio_rms_db_range: Optional[List[float]]
    cc_generation_notes: List[str]
    assigned_stem: Optional[str]
    original_events: List[AbsEvent]
    ks_events: List[AbsEvent]
    artmap_events: List[AbsEvent]


def utc_now() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def normalize_ws(s: str) -> str:
    return re.sub(r"\s+", " ", s.strip())


def track_name_of(track: mido.MidiTrack, fallback: str) -> str:
    for msg in track:
        if msg.is_meta and msg.type == "track_name":
            name = normalize_ws(str(getattr(msg, "name", "") or ""))
            if name:
                return name
    return fallback


def sanitize_track_name(name: str) -> str:
    out = normalize_ws(name)
    out = re.sub(r"[\\/:*?\"<>|]+", " ", out)
    out = normalize_ws(out)
    return out or "Track"


def has_note_messages(track: mido.MidiTrack) -> bool:
    for msg in track:
        if msg.type == "note_on" and int(getattr(msg, "velocity", 0)) > 0:
            return True
        if msg.type == "note_off":
            return True
    return False


def list_abs_track_messages(track: mido.MidiTrack) -> List[Tuple[int, int, mido.Message]]:
    out: List[Tuple[int, int, mido.Message]] = []
    tick = 0
    for i, msg in enumerate(track):
        tick += int(msg.time)
        out.append((i, tick, msg))
    return out


def parse_note_pairs(track: mido.MidiTrack) -> List[NotePair]:
    abs_msgs = list_abs_track_messages(track)
    active: Dict[Tuple[int, int], List[Tuple[int, int, int]]] = defaultdict(list)
    pairs: List[NotePair] = []

    for i, tick, msg in abs_msgs:
        if msg.type == "note_on" and int(msg.velocity) > 0:
            active[(int(msg.channel), int(msg.note))].append((i, tick, int(msg.velocity)))
            continue
        if msg.type == "note_off" or (msg.type == "note_on" and int(msg.velocity) == 0):
            key = (int(msg.channel), int(msg.note))
            stack = active.get(key, [])
            if not stack:
                continue
            on_index, on_tick, vel = stack.pop(0)
            pairs.append(
                NotePair(
                    on_index=on_index,
                    off_index=i,
                    start_tick=on_tick,
                    end_tick=tick,
                    pitch=int(msg.note),
                    channel=int(msg.channel),
                    velocity=vel,
                )
            )
    pairs.sort(key=lambda p: (p.start_tick, p.on_index))
    return pairs


def build_keyswitch_flags(
    note_events: Sequence[mpr.NoteEvent],
    ks_note_ids: Set[int],
) -> Dict[Tuple[int, int, int, int, int], Deque[bool]]:
    flags: Dict[Tuple[int, int, int, int, int], Deque[bool]] = defaultdict(deque)
    for n in note_events:
        # midi_profile_regions uses 1..16 internally; mido messages use 0..15.
        midi_channel = max(0, min(15, int(n.channel) - 1))
        key = (int(n.start_tick), int(n.end_tick), int(n.pitch), midi_channel, int(n.velocity))
        flags[key].append(id(n) in ks_note_ids)
    return flags


def profile_counts(note_decisions: Sequence[ecc.NoteDecision]) -> Dict[str, int]:
    c = Counter([n.profile for n in note_decisions])
    return {"LONG": int(c.get("LONG", 0)), "SHORT": int(c.get("SHORT", 0)), "REP": int(c.get("REP", 0))}


def generate_cc_events(
    note_decisions: Sequence[ecc.NoteDecision],
    cfg: Dict[str, Any],
    midi_channel: int,
    seed_tick: int = 0,
) -> List[AbsEvent]:
    cc_map = cfg["output"]["cc_map"]
    ranges = cfg["output"]["profile_cc_ranges"]
    seeds = cfg["output"]["seed_values"]
    cc1_num = int(cc_map["cc1"])
    cc11_num = int(cc_map["cc11"])

    prev_cc1 = ecc.clamp_midi(float(seeds["cc1"]))
    prev_cc11 = ecc.clamp_midi(float(seeds["cc11"]))

    events: List[AbsEvent] = [
        AbsEvent(
            tick=max(0, seed_tick),
            priority=5,
            order=0,
            msg=mido.Message(
                "control_change",
                channel=int(midi_channel),
                control=cc1_num,
                value=prev_cc1,
                time=0,
            ),
        ),
        AbsEvent(
            tick=max(0, seed_tick),
            priority=6,
            order=1,
            msg=mido.Message(
                "control_change",
                channel=int(midi_channel),
                control=cc11_num,
                value=prev_cc11,
                time=0,
            ),
        ),
    ]

    order = 2
    for n in note_decisions:
        p = n.profile if n.profile in ranges else "LONG"
        r = ranges[p]
        cc1_lo, cc1_hi = [int(v) for v in r["cc1"]]
        cc11_lo, cc11_hi = [int(v) for v in r["cc11"]]
        target_cc1 = ecc.value_from_velocity(int(n.velocity), cc1_lo, cc1_hi)
        target_cc11 = ecc.value_from_velocity(int(n.velocity), cc11_lo, cc11_hi)
        cc1 = ecc.clamp_midi(prev_cc1 * 0.35 + target_cc1 * 0.65)
        cc11 = ecc.clamp_midi(prev_cc11 * 0.40 + target_cc11 * 0.60)
        prev_cc1 = cc1
        prev_cc11 = cc11

        events.append(
            AbsEvent(
                tick=int(n.start_tick),
                priority=5,
                order=order,
                msg=mido.Message(
                    "control_change",
                    channel=int(midi_channel),
                    control=cc1_num,
                    value=cc1,
                    time=0,
                ),
            )
        )
        order += 1
        events.append(
            AbsEvent(
                tick=int(n.start_tick),
                priority=6,
                order=order,
                msg=mido.Message(
                    "control_change",
                    channel=int(midi_channel),
                    control=cc11_num,
                    value=cc11,
                    time=0,
                ),
            )
        )
        order += 1

    return events


def build_track_from_events(track_name: str, events: Sequence[AbsEvent]) -> mido.MidiTrack:
    tr = mido.MidiTrack()
    tr.append(mido.MetaMessage("track_name", name=track_name, time=0))
    ordered = sorted(events, key=lambda e: (e.tick, e.priority, e.order))
    last_tick = 0
    for e in ordered:
        delta = max(0, int(e.tick) - last_tick)
        tr.append(e.msg.copy(time=delta))
        last_tick = int(e.tick)
    tr.append(mido.MetaMessage("end_of_track", time=0))
    return tr


def collect_global_meta_events(mid: mido.MidiFile) -> List[AbsEvent]:
    keep_meta_types = {
        "set_tempo",
        "time_signature",
        "key_signature",
        "marker",
        "cue_marker",
        "text",
        "lyrics",
    }
    raw_events: List[Tuple[int, int, mido.MetaMessage]] = []
    order = 0
    for tr in mid.tracks:
        tick = 0
        for msg in tr:
            tick += int(msg.time)
            if not msg.is_meta:
                continue
            if msg.type == "end_of_track":
                continue
            if msg.type not in keep_meta_types:
                continue
            raw_events.append((tick, order, msg.copy(time=0)))
            order += 1

    # Keep only the last tempo event per tick.
    tempo_by_tick: Dict[int, Tuple[int, mido.MetaMessage]] = {}
    others: List[Tuple[int, int, mido.MetaMessage]] = []
    for tick, ord_idx, msg in raw_events:
        if msg.type == "set_tempo":
            tempo_by_tick[tick] = (ord_idx, msg)
        else:
            others.append((tick, ord_idx, msg))

    merged: List[AbsEvent] = []
    for tick, (ord_idx, msg) in tempo_by_tick.items():
        merged.append(AbsEvent(tick=tick, priority=10, order=ord_idx, msg=msg))
    for tick, ord_idx, msg in others:
        merged.append(AbsEvent(tick=tick, priority=20, order=ord_idx, msg=msg))

    has_tempo = any(e.msg.type == "set_tempo" for e in merged)
    if not has_tempo:
        merged.append(
            AbsEvent(
                tick=0,
                priority=10,
                order=10_000_000,
                msg=mido.MetaMessage("set_tempo", tempo=500000, time=0),
            )
        )
    return merged


def normalize_for_match(s: str) -> str:
    return re.sub(r"[^a-z0-9]+", " ", s.lower()).strip()


def tokenize_for_match(s: str) -> Set[str]:
    return {tok for tok in normalize_for_match(s).split() if tok}


def percentile_int(values_sorted: Sequence[int], q: float) -> Optional[int]:
    if not values_sorted:
        return None
    if q <= 0.0:
        return int(values_sorted[0])
    if q >= 1.0:
        return int(values_sorted[-1])
    idx = int(round((len(values_sorted) - 1) * q))
    idx = max(0, min(len(values_sorted) - 1, idx))
    return int(values_sorted[idx])


def percentile_float(values_sorted: Sequence[float], q: float) -> Optional[float]:
    if not values_sorted:
        return None
    if q <= 0.0:
        return float(values_sorted[0])
    if q >= 1.0:
        return float(values_sorted[-1])
    pos = (len(values_sorted) - 1) * q
    lo = int(math.floor(pos))
    hi = int(math.ceil(pos))
    if lo == hi:
        return float(values_sorted[lo])
    frac = pos - lo
    return float(values_sorted[lo] + (values_sorted[hi] - values_sorted[lo]) * frac)


def clamp01(v: float) -> float:
    return max(0.0, min(1.0, float(v)))


def normalize_db(v_db: float, lo_db: float, hi_db: float) -> float:
    if hi_db <= lo_db + 1e-9:
        return 0.0
    return clamp01((float(v_db) - lo_db) / (hi_db - lo_db))


def lerp(v0: float, v1: float, t: float) -> float:
    return float(v0 + (v1 - v0) * t)


def interp_series(t: float, xs: Sequence[float], ys: Sequence[float]) -> float:
    if not xs or not ys:
        return 0.0
    if t <= xs[0]:
        return float(ys[0])
    if t >= xs[-1]:
        return float(ys[-1])
    i = bisect_right(xs, t) - 1
    i = max(0, min(i, len(xs) - 2))
    x0, x1 = float(xs[i]), float(xs[i + 1])
    y0, y1 = float(ys[i]), float(ys[i + 1])
    if x1 <= x0:
        return y0
    alpha = (t - x0) / (x1 - x0)
    return lerp(y0, y1, alpha)


def extract_audio_envelope_ffmpeg(
    audio_path: Path,
    frame_samples: int = 1024,
) -> Tuple[Optional[AudioEnvelope], List[str]]:
    """
    Extract RMS envelope from audio using ffmpeg astats metadata output.
    Supports float WAV files exported by Dorico/NotePerformer.
    """
    notes: List[str] = []
    if not audio_path.exists():
        return None, [f"Stem not found: {audio_path}"]
    if frame_samples < 128:
        frame_samples = 128

    cmd = [
        "ffmpeg",
        "-hide_banner",
        "-nostats",
        "-i",
        str(audio_path),
        "-ac",
        "1",
        "-af",
        (
            f"asetnsamples=n={int(frame_samples)}:p=0,"
            "astats=metadata=1:reset=1,"
            "ametadata=print:key=lavfi.astats.Overall.RMS_level"
        ),
        "-f",
        "null",
        "-",
    ]
    try:
        proc = subprocess.run(cmd, capture_output=True, text=True, check=False)
    except Exception as exc:
        return None, [f"ffmpeg invocation failed: {exc!r}"]
    if proc.returncode != 0:
        msg = proc.stderr.strip().splitlines()[-1] if proc.stderr.strip() else "unknown ffmpeg error"
        return None, [f"ffmpeg failed ({proc.returncode}): {msg}"]

    text = proc.stderr or ""
    # Some ffmpeg builds route filter logs to stdout; include both streams.
    if proc.stdout:
        text = f"{text}\n{proc.stdout}"

    times: List[float] = []
    rms_vals: List[float] = []
    current_t: Optional[float] = None

    # Example lines:
    # [Parsed_ametadata_2 @ ...] frame:12   pts:12288   pts_time:0.256
    # [Parsed_ametadata_2 @ ...] lavfi.astats.Overall.RMS_level=-42.913118
    re_t = re.compile(r"pts_time:([0-9.+-eE]+)")
    re_rms = re.compile(r"lavfi\.astats\.Overall\.RMS_level=([^\s]+)")
    for raw in text.splitlines():
        line = raw.strip()
        if not line:
            continue
        m_t = re_t.search(line)
        if m_t:
            try:
                current_t = float(m_t.group(1))
            except Exception:
                current_t = None
            continue
        m_r = re_rms.search(line)
        if m_r and current_t is not None:
            token = str(m_r.group(1)).strip().lower()
            if token in {"-inf", "inf", "nan"}:
                rms_db = -120.0
            else:
                try:
                    rms_db = float(token)
                except Exception:
                    continue
            times.append(float(current_t))
            rms_vals.append(float(max(-120.0, min(12.0, rms_db))))
            current_t = None

    if len(times) < 2 or len(times) != len(rms_vals):
        return None, [f"No usable RMS envelope extracted from stem '{audio_path.name}'."]

    # Ensure monotonic time and drop exact duplicates.
    compact_t: List[float] = []
    compact_r: List[float] = []
    last_t = -1.0
    for t, r in zip(times, rms_vals):
        if t <= last_t:
            if compact_t and abs(t - compact_t[-1]) < 1e-9:
                compact_r[-1] = r
            continue
        compact_t.append(t)
        compact_r.append(r)
        last_t = t

    if len(compact_t) < 2:
        return None, [f"RMS envelope collapsed after dedupe for '{audio_path.name}'."]
    return AudioEnvelope(times_sec=compact_t, rms_db=compact_r), notes


def build_sec_to_tick_converter(
    tpq: int,
    tempo_map: Sequence[Tuple[int, int]],
):
    seg_start_sec: List[float] = []
    seg_end_sec: List[float] = []
    seg_start_tick: List[int] = []
    seg_sec_per_tick: List[float] = []

    if not tempo_map:
        tempo_map = [(0, 500000)]

    cur_sec = 0.0
    for i, (tick, tempo) in enumerate(tempo_map):
        t0 = int(tick)
        t1 = int(tempo_map[i + 1][0]) if i + 1 < len(tempo_map) else -1
        sec_per_tick = max(1e-12, (float(tempo) / 1_000_000.0) / float(tpq))
        seg_start_sec.append(cur_sec)
        seg_start_tick.append(t0)
        seg_sec_per_tick.append(sec_per_tick)
        if t1 > t0:
            end_sec = cur_sec + (t1 - t0) * sec_per_tick
            seg_end_sec.append(end_sec)
            cur_sec = end_sec
        else:
            seg_end_sec.append(float("inf"))

    def sec_to_tick(sec: float) -> int:
        s = max(0.0, float(sec))
        i = bisect_right(seg_start_sec, s) - 1
        if i < 0:
            i = 0
        while i + 1 < len(seg_start_sec) and s >= seg_end_sec[i]:
            i += 1
        dt = max(0.0, s - seg_start_sec[i])
        tick = seg_start_tick[i] + int(round(dt / seg_sec_per_tick[i]))
        return max(0, tick)

    return sec_to_tick


def generate_cc_events_from_audio(
    note_decisions: Sequence[ecc.NoteDecision],
    cfg: Dict[str, Any],
    midi_channel: int,
    seed_tick: int,
    envelope: AudioEnvelope,
    tpq: int,
    tempo_map: Sequence[Tuple[int, int]],
) -> Tuple[List[AbsEvent], Dict[str, Any]]:
    cc_map = cfg["output"]["cc_map"]
    ranges = cfg["output"]["profile_cc_ranges"]
    seeds = cfg["output"]["seed_values"]
    cc1_num = int(cc_map["cc1"])
    cc11_num = int(cc_map["cc11"])

    profile_tau = {"LONG": 0.24, "SHORT": 0.08, "REP": 0.14}
    profile_amount = {"LONG": 1.12, "SHORT": 0.92, "REP": 1.00}
    profile_offset = {"LONG": 0.05, "SHORT": 0.10, "REP": 0.03}
    profile_attack_sec = {"LONG": 0.10, "SHORT": 0.03, "REP": 0.05}

    if not note_decisions:
        seed_cc1 = ecc.clamp_midi(float(seeds["cc1"]))
        seed_cc11 = ecc.clamp_midi(float(seeds["cc11"]))
        events = [
            AbsEvent(
                tick=max(0, seed_tick),
                priority=5,
                order=0,
                msg=mido.Message("control_change", channel=int(midi_channel), control=cc1_num, value=seed_cc1, time=0),
            ),
            AbsEvent(
                tick=max(0, seed_tick),
                priority=6,
                order=1,
                msg=mido.Message("control_change", channel=int(midi_channel), control=cc11_num, value=seed_cc11, time=0),
            ),
        ]
        return events, {
            "mode": "wav_envelope",
            "cc1_count": 1,
            "cc11_count": 1,
            "density_per_min_cc1": 0.0,
            "density_per_min_cc11": 0.0,
            "span_sec": 0.0,
            "envelope_points": len(envelope.times_sec),
            "audio_rms_db_range": [min(envelope.rms_db), max(envelope.rms_db)] if envelope.rms_db else None,
        }

    sec_to_tick = build_sec_to_tick_converter(tpq=tpq, tempo_map=tempo_map)
    first_sec = float(note_decisions[0].start_sec)
    last_sec = float(note_decisions[-1].end_sec)

    # Use active musical span for adaptive normalization.
    active_db: List[float] = []
    for t, r in zip(envelope.times_sec, envelope.rms_db):
        if first_sec - 0.25 <= t <= last_sec + 0.25 and r > -119.0:
            active_db.append(float(r))
    if not active_db:
        active_db = [float(r) for r in envelope.rms_db if float(r) > -119.0]
    if not active_db:
        active_db = list(envelope.rms_db)
    active_db.sort()
    db_lo = percentile_float(active_db, 0.08)
    db_hi = percentile_float(active_db, 0.92)
    if db_lo is None or db_hi is None:
        db_lo, db_hi = -72.0, -30.0
    if db_hi - db_lo < 6.0:
        mid = (db_hi + db_lo) * 0.5
        db_lo = mid - 3.0
        db_hi = mid + 3.0

    # Determine envelope frame step from source data.
    step_est = 0.02
    if len(envelope.times_sec) >= 3:
        diffs = [envelope.times_sec[i] - envelope.times_sec[i - 1] for i in range(1, len(envelope.times_sec))]
        diffs = [d for d in diffs if d > 1e-6]
        if diffs:
            step_est = float(statistics.median(diffs))
    step_sec = max(0.010, min(0.040, step_est))

    seed_cc1 = ecc.clamp_midi(float(seeds["cc1"]))
    seed_cc11 = ecc.clamp_midi(float(seeds["cc11"]))
    prev_cc1 = seed_cc1
    prev_cc11 = seed_cc11
    prev_norm = normalize_db(interp_series(first_sec, envelope.times_sec, envelope.rms_db), db_lo, db_hi)

    events: List[AbsEvent] = [
        AbsEvent(
            tick=max(0, seed_tick),
            priority=5,
            order=0,
            msg=mido.Message("control_change", channel=int(midi_channel), control=cc1_num, value=seed_cc1, time=0),
        ),
        AbsEvent(
            tick=max(0, seed_tick),
            priority=6,
            order=1,
            msg=mido.Message("control_change", channel=int(midi_channel), control=cc11_num, value=seed_cc11, time=0),
        ),
    ]

    order = 2
    last_out_cc1 = seed_cc1
    last_out_cc11 = seed_cc11
    cc1_count = 1
    cc11_count = 1

    for n in note_decisions:
        p = n.profile if n.profile in ranges else "LONG"
        r = ranges[p]
        cc1_lo, cc1_hi = [int(v) for v in r["cc1"]]
        cc11_lo, cc11_hi = [int(v) for v in r["cc11"]]
        tau = float(profile_tau.get(p, 0.14))
        amount = float(profile_amount.get(p, 1.0))
        offset = float(profile_offset.get(p, 0.0))
        atk = float(profile_attack_sec.get(p, 0.05))

        start = float(n.start_sec)
        end = max(start, float(n.end_sec))
        dur = max(0.0, end - start)
        sample_count = max(1, int(math.floor(dur / step_sec)) + 1)

        for i in range(sample_count):
            t = start + min(dur, i * step_sec)
            raw_db = interp_series(t, envelope.times_sec, envelope.rms_db)
            raw_norm = normalize_db(raw_db, db_lo, db_hi)
            alpha = step_sec / (tau + step_sec)
            smooth_norm = prev_norm + alpha * (raw_norm - prev_norm)
            prev_norm = smooth_norm

            dt = max(0.0, t - start)
            onset_boost = 0.0
            if atk > 1e-6 and dt < atk:
                onset_boost = 0.08 * (1.0 - (dt / atk))

            cc11_norm = smooth_norm
            cc1_norm = clamp01((smooth_norm + offset) * amount + onset_boost)

            tgt_cc1 = ecc.clamp_midi(cc1_lo + cc1_norm * (cc1_hi - cc1_lo))
            tgt_cc11 = ecc.clamp_midi(cc11_lo + cc11_norm * (cc11_hi - cc11_lo))

            cc1 = ecc.clamp_midi(prev_cc1 * 0.20 + tgt_cc1 * 0.80)
            cc11 = ecc.clamp_midi(prev_cc11 * 0.25 + tgt_cc11 * 0.75)
            prev_cc1 = cc1
            prev_cc11 = cc11

            tick = sec_to_tick(t)
            if cc1 != last_out_cc1:
                events.append(
                    AbsEvent(
                        tick=int(tick),
                        priority=5,
                        order=order,
                        msg=mido.Message(
                            "control_change",
                            channel=int(midi_channel),
                            control=cc1_num,
                            value=int(cc1),
                            time=0,
                        ),
                    )
                )
                order += 1
                last_out_cc1 = cc1
                cc1_count += 1
            if cc11 != last_out_cc11:
                events.append(
                    AbsEvent(
                        tick=int(tick),
                        priority=6,
                        order=order,
                        msg=mido.Message(
                            "control_change",
                            channel=int(midi_channel),
                            control=cc11_num,
                            value=int(cc11),
                            time=0,
                        ),
                    )
                )
                order += 1
                last_out_cc11 = cc11
                cc11_count += 1

    span_sec = max(0.0, last_sec - first_sec)
    density_div = max(span_sec / 60.0, 1e-9)
    meta = {
        "mode": "wav_envelope",
        "cc1_count": int(cc1_count),
        "cc11_count": int(cc11_count),
        "density_per_min_cc1": float(cc1_count / density_div),
        "density_per_min_cc11": float(cc11_count / density_div),
        "span_sec": float(span_sec),
        "envelope_points": len(envelope.times_sec),
        "audio_rms_db_range": [float(min(envelope.rms_db)), float(max(envelope.rms_db))] if envelope.rms_db else None,
        "norm_db_window": [float(db_lo), float(db_hi)],
        "step_sec": float(step_sec),
    }
    return events, meta


def infer_instrument_range(track_name: str) -> Optional[Tuple[int, int]]:
    """
    Broad orchestral MIDI pitch ranges used as guard rails for KS separation.
    """
    n = normalize_for_match(track_name)
    if "piccolo" in n:
        return (72, 108)
    if "bass clarinet" in n:
        return (34, 84)
    if "clarinet" in n:
        return (50, 96)
    if "english horn" in n or "cor anglais" in n:
        return (52, 84)
    if "oboe" in n:
        return (58, 92)
    if "flute" in n:
        return (60, 96)
    if "contrabassoon" in n:
        return (22, 72)
    if "bassoon" in n:
        return (34, 80)
    if "bass trumpet" in n:
        return (40, 86)
    if "trumpet" in n:
        return (52, 96)
    if "trombone" in n:
        return (34, 84)
    if "tuba" in n:
        return (26, 72)
    if "horn" in n:
        return (34, 90)
    if "timpani" in n:
        return (34, 72)
    if "triangle" in n or "cymbal" in n:
        return (52, 76)
    if "harp" in n:
        return (24, 108)
    if "violin" in n:
        return (55, 108)
    if "viola" in n:
        return (48, 96)
    if "violoncello" in n or n == "cello" or " cello " in f" {n} ":
        return (36, 90)
    if "double bass" in n or "contrabass" in n:
        return (28, 76)
    return None


def detect_articulation_events_from_strict_ks(
    note_events: Sequence[mpr.NoteEvent],
    ks_note_ids: Set[int],
    dataset: mpr.ArticulationDataset,
    mode: str,
    tpq: int,
    tempo_map: Sequence[Tuple[int, int]],
) -> Tuple[List[mpr.ArticulationEvent], str]:
    on_by_tick: Dict[int, List[int]] = defaultdict(list)
    off_by_tick: Dict[int, List[int]] = defaultdict(list)
    for n in note_events:
        if id(n) not in ks_note_ids:
            continue
        on_by_tick[int(n.start_tick)].append(int(n.pitch))
        off_by_tick[int(n.end_tick)].append(int(n.pitch))

    ticks = sorted(set(on_by_tick.keys()) | set(off_by_tick.keys()))
    active: Set[int] = set()
    events: List[mpr.ArticulationEvent] = []
    last_profile: Optional[str] = None

    for tick in ticks:
        for p in off_by_tick.get(tick, []):
            active.discard(int(p))
        for p in on_by_tick.get(tick, []):
            active.add(int(p))
        if not active:
            continue
        profile = mpr.match_chord_to_profile(sorted(active), dataset.chord_to_profile)
        if profile is None:
            continue
        profile = mpr.normalize_profile(profile, mode)
        if profile != last_profile:
            sec = mpr.tick_to_sec(tick, tpq, tempo_map)
            events.append(
                mpr.ArticulationEvent(tick=tick, sec=sec, profile=profile, reason=f"ks_strict_latch={sorted(active)}")
            )
            last_profile = profile

    if events:
        return events, "strict_latch"

    # Trigger fallback on KS notes that begin together.
    by_start_tick: Dict[int, List[int]] = defaultdict(list)
    for n in note_events:
        if id(n) in ks_note_ids:
            by_start_tick[int(n.start_tick)].append(int(n.pitch))

    trig_events: List[mpr.ArticulationEvent] = []
    for tick in sorted(by_start_tick.keys()):
        chord = sorted(set(by_start_tick[tick]))
        if not chord:
            continue
        profile = mpr.match_chord_to_profile(chord, dataset.chord_to_profile)
        if profile is None:
            continue
        profile = mpr.normalize_profile(profile, mode)
        sec = mpr.tick_to_sec(tick, tpq, tempo_map)
        trig_events.append(mpr.ArticulationEvent(tick=tick, sec=sec, profile=profile, reason=f"ks_strict_trigger={chord}"))
    return trig_events, ("strict_trigger" if trig_events else "strict_none")


def detect_strict_keyswitch_ids(
    note_events: Sequence[mpr.NoteEvent],
    dataset: mpr.ArticulationDataset,
    track_name: str,
    mode: str,
    tpq: int,
    tempo_map: Sequence[Tuple[int, int]],
) -> Tuple[Set[int], List[mpr.ArticulationEvent], str, Dict[str, Any]]:
    ks_pool = set(dataset.keyswitch_note_pool)

    all_pitches = sorted([int(n.pitch) for n in note_events])
    if not all_pitches:
        return set(), [], "strict_none", {
            "method": "strict_lookup_range",
            "ks_pool_candidates": 0,
            "ks_selected": 0,
            "ks_ambiguous": 0,
            "ks_chord_matched": 0,
            "ks_unique_candidate_pitches": 0,
            "inferred_ks_pitches": [],
            "core_pitch_range": None,
            "instrument_range": infer_instrument_range(track_name),
        }

    # Baseline core pitch range from all notes. We refine again after candidate selection.
    core_min_all = percentile_int(all_pitches, 0.05)
    core_max_all = percentile_int(all_pitches, 0.95)
    inst_range = infer_instrument_range(track_name)
    inst_min = inst_range[0] if inst_range else None
    inst_max = inst_range[1] if inst_range else None

    # Pitch-level heuristics to infer KS-like pitches even when dataset lookup is incomplete.
    pitch_vels: Dict[int, List[int]] = defaultdict(list)
    pitch_durs: Dict[int, List[float]] = defaultdict(list)
    for n in note_events:
        p = int(n.pitch)
        pitch_vels[p].append(int(n.velocity))
        pitch_durs[p].append(float(n.duration_ms))

    inferred_ks_pitches: Set[int] = set()
    for p in pitch_vels.keys():
        v = sorted(pitch_vels[p])
        d = sorted(pitch_durs[p])
        c = len(v)
        if c == 0:
            continue
        high_ratio = sum(1 for x in v if x >= 118) / c
        vhigh_ratio = sum(1 for x in v if x >= 124) / c
        long_ratio = sum(1 for x in d if x >= 400.0) / c
        med_dur = d[c // 2]
        out_core = False
        if core_min_all is not None and core_max_all is not None:
            out_core = p < (core_min_all - 4) or p > (core_max_all + 4)
        out_inst = False
        if inst_min is not None and inst_max is not None:
            out_inst = p < (inst_min - 2) or p > (inst_max + 2)

        # Primary inferred KS pitch signature:
        # repeated, high-velocity, sustained, and outside likely musical core/range.
        if c >= 2 and high_ratio >= 0.85 and long_ratio >= 0.60 and (out_core or out_inst):
            inferred_ks_pitches.add(p)
            continue
        # Boundary KS that sit near the playable edge (e.g., p == inst_max).
        if c >= 3 and high_ratio >= 0.90 and med_dur >= 1500.0 and inst_min is not None and inst_max is not None:
            if p <= inst_min or p >= inst_max:
                inferred_ks_pitches.add(p)
                continue
        # Secondary signature for strong persistent control notes (e.g., harp high KS).
        if c >= 2 and vhigh_ratio >= 0.90 and med_dur >= 2000.0:
            inferred_ks_pitches.add(p)
            continue

    idx_candidates: List[int] = [
        i for i, n in enumerate(note_events) if int(n.pitch) in ks_pool or int(n.pitch) in inferred_ks_pitches
    ]
    if not idx_candidates:
        return set(), [], "strict_none", {
            "method": "strict_lookup_range",
            "ks_pool_candidates": 0,
            "ks_selected": 0,
            "ks_ambiguous": 0,
            "ks_chord_matched": 0,
            "ks_unique_candidate_pitches": 0,
            "inferred_ks_pitches": sorted(inferred_ks_pitches),
            "core_pitch_range": [core_min_all, core_max_all] if core_min_all is not None and core_max_all is not None else None,
            "instrument_range": [inst_min, inst_max] if inst_min is not None and inst_max is not None else None,
        }

    idx_set = set(idx_candidates)
    non_candidate_pitches = sorted([int(n.pitch) for i, n in enumerate(note_events) if i not in idx_set])
    if non_candidate_pitches:
        core_min = percentile_int(non_candidate_pitches, 0.05)
        core_max = percentile_int(non_candidate_pitches, 0.95)
    else:
        core_min = percentile_int(all_pitches, 0.05)
        core_max = percentile_int(all_pitches, 0.95)

    on_by_tick: Dict[int, List[int]] = defaultdict(list)
    off_by_tick: Dict[int, List[int]] = defaultdict(list)
    for i in idx_candidates:
        n = note_events[i]
        on_by_tick[int(n.start_tick)].append(i)
        off_by_tick[int(n.end_tick)].append(i)
    candidate_pitch_counts = Counter(int(note_events[i].pitch) for i in idx_candidates)
    unique_candidate_pitches = sorted(candidate_pitch_counts.keys())
    group_size_by_idx: Dict[int, int] = {}
    for tick, idxs in on_by_tick.items():
        gsize = len(idxs)
        for idx in idxs:
            group_size_by_idx[idx] = gsize

    active_indices: Set[int] = set()
    chord_matched_indices: Set[int] = set()
    ticks = sorted(set(on_by_tick.keys()) | set(off_by_tick.keys()))
    for tick in ticks:
        for idx in off_by_tick.get(tick, []):
            active_indices.discard(idx)
        for idx in on_by_tick.get(tick, []):
            active_indices.add(idx)

        if active_indices:
            active_pitches = sorted({int(note_events[idx].pitch) for idx in active_indices})
            if mpr.match_chord_to_profile(active_pitches, dataset.chord_to_profile) is not None:
                chord_matched_indices.update(active_indices)

        on_indices = on_by_tick.get(tick, [])
        if on_indices:
            on_pitches = sorted({int(note_events[idx].pitch) for idx in on_indices})
            if mpr.match_chord_to_profile(on_pitches, dataset.chord_to_profile) is not None:
                chord_matched_indices.update(on_indices)

    ks_note_ids: Set[int] = set()
    ambiguous_count = 0
    for idx in idx_candidates:
        n = note_events[idx]
        outside_core = False
        if core_min is not None and core_max is not None:
            outside_core = int(n.pitch) < (core_min - 2) or int(n.pitch) > (core_max + 2)
        outside_inst = False
        if inst_min is not None and inst_max is not None:
            outside_inst = int(n.pitch) < (inst_min - 2) or int(n.pitch) > (inst_max + 2)
        low_velocity = int(n.velocity) <= 12
        high_velocity = int(n.velocity) >= 118
        very_high_velocity = int(n.velocity) >= 124
        long_duration = float(n.duration_ms) >= 140.0
        longish_duration = float(n.duration_ms) >= 500.0
        multi_start = group_size_by_idx.get(idx, 1) >= 2
        small_vocab = len(unique_candidate_pitches) <= 4
        pitch_repeats = int(candidate_pitch_counts.get(int(n.pitch), 0)) >= 3
        pitch_inferred = int(n.pitch) in inferred_ks_pitches

        score = 0
        if idx in chord_matched_indices:
            score += 4
        if pitch_inferred:
            score += 2
        if outside_core:
            score += 2
        if outside_inst:
            score += 2
        if low_velocity:
            score += 1
        if long_duration:
            score += 1
        if high_velocity:
            score += 1
        if longish_duration:
            score += 1
        if multi_start:
            score += 1
        if small_vocab and pitch_repeats:
            score += 1
        if very_high_velocity and float(n.duration_ms) >= 300.0:
            score += 1

        is_ks = score >= 3
        if not is_ks and outside_core and (outside_inst or low_velocity or long_duration):
            is_ks = True
        if not is_ks and small_vocab and high_velocity and longish_duration:
            is_ks = True

        # Guard against false positives when a broad KS pool overlaps playable
        # low-register notes (e.g., global fallback pools).
        if (
            is_ks
            and idx not in chord_matched_indices
            and not multi_start
            and not very_high_velocity
            and len(unique_candidate_pitches) > 6
            and not outside_inst
            and not pitch_inferred
        ):
            is_ks = False
        if (
            is_ks
            and idx not in chord_matched_indices
            and not pitch_inferred
            and not outside_core
            and not outside_inst
        ):
            is_ks = False

        if is_ks:
            ks_note_ids.add(id(n))
        else:
            ambiguous_count += 1

    articulation_events, detection_mode = detect_articulation_events_from_strict_ks(
        note_events=note_events,
        ks_note_ids=ks_note_ids,
        dataset=dataset,
        mode=mode,
        tpq=tpq,
        tempo_map=tempo_map,
    )
    diagnostics = {
        "method": "strict_lookup_range",
        "ks_pool_candidates": len(idx_candidates),
        "ks_selected": len(ks_note_ids),
        "ks_ambiguous": ambiguous_count,
        "ks_chord_matched": len(chord_matched_indices),
        "ks_unique_candidate_pitches": len(unique_candidate_pitches),
        "inferred_ks_pitches": sorted(inferred_ks_pitches),
        "core_pitch_range": [core_min, core_max] if core_min is not None and core_max is not None else None,
        "instrument_range": [inst_min, inst_max] if inst_min is not None and inst_max is not None else None,
    }
    return ks_note_ids, articulation_events, detection_mode, diagnostics


def parse_trailing_part_number(track_name: str) -> Optional[int]:
    n = normalize_for_match(track_name)
    m = re.search(r"(?:^| )(\d+)$", n)
    if m:
        return int(m.group(1))
    if re.search(r"\bii\b$", n):
        return 2
    if re.search(r"\bi\b$", n):
        return 1
    return None


def infer_dataset_code_for_track(track_name: str) -> Optional[str]:
    n = normalize_for_match(track_name)
    part = parse_trailing_part_number(track_name)

    if "piccolo" in n:
        return "Picc"
    if "flute" in n:
        if part == 2:
            return "Fl2"
        return "Fl1"
    if "oboe" in n and "english" not in n:
        if part == 2:
            return "Ob2"
        return "Ob1"
    if "english horn" in n or "cor anglais" in n:
        return "EH"
    if "bass clarinet" in n:
        return "BCl"
    if "clarinet" in n:
        if part == 2:
            return "Cl2"
        return "Cl1"
    if "contrabassoon" in n:
        return "CBsn"
    if "bassoon" in n:
        if part == 2:
            return "Bsn2"
        return "Bsn1"

    if "violin ii" in n or "violin 2" in n:
        return "V2"
    if "violin i" in n or "violin 1" in n:
        return None  # V1 uses root articulations.json in this project.
    if "viola" in n:
        return "Vla"
    if "violoncello" in n or re.search(r"\bcello\b", n):
        return "Vc"
    if "double bass" in n or "contrabass" in n:
        return "DB"

    if "horn" in n:
        p = part if part and 1 <= part <= 6 else 1
        return f"Hn{p}"
    if "bass trumpet" in n:
        return "Tpt4"
    if "trumpet" in n:
        p = part if part and 1 <= part <= 5 else 1
        return f"Tpt{p}"
    if "trombone" in n:
        p = part if part and 1 <= part <= 4 else 1
        return f"Tbn{p}"
    if "tuba" in n:
        return "Tuba1"

    if "timpani" in n:
        return "Timp1"
    if any(k in n for k in ["triangle", "cymbal", "snare", "drum", "tambourine", "gong", "tam tam", "perc"]):
        return "Perc1"
    if "harp" in n:
        return "Perc1"

    return None


def resolve_dataset_path_for_track(track_name: str, articulations_path: Optional[Path]) -> Tuple[Optional[Path], Optional[str]]:
    if articulations_path is None:
        return None, None

    if articulations_path.is_dir():
        root_dir = articulations_path
        base_path = articulations_path / "articulations.json"
    else:
        root_dir = articulations_path.parent
        base_path = articulations_path

    code = infer_dataset_code_for_track(track_name)
    if code:
        p = root_dir / code / "articulations.json"
        if p.exists():
            return p, code
    if base_path.exists():
        return base_path, None
    return None, code


def find_stems(stems_dir: Optional[Path]) -> List[Path]:
    if stems_dir is None or not stems_dir.exists():
        return []
    out: List[Path] = []
    for p in sorted(stems_dir.rglob("*")):
        if p.is_file() and p.suffix.lower() in {".wav", ".wave", ".aif", ".aiff", ".flac"}:
            out.append(p)
    return out


def assign_stems(track_names: Sequence[str], stem_files: Sequence[Path]) -> Dict[str, Optional[str]]:
    if not stem_files:
        return {name: None for name in track_names}
    if len(track_names) == 1 and len(stem_files) == 1:
        return {track_names[0]: str(stem_files[0])}

    remaining = list(stem_files)
    assignments: Dict[str, Optional[str]] = {}
    for tname in track_names:
        t_toks = tokenize_for_match(tname)
        best_idx = -1
        best_score = -1.0
        for i, stem in enumerate(remaining):
            s_toks = tokenize_for_match(stem.stem)
            if not t_toks or not s_toks:
                continue
            overlap = len(t_toks & s_toks)
            if overlap == 0:
                continue
            score = overlap / max(1, len(t_toks))
            if score > best_score:
                best_score = score
                best_idx = i
        if best_idx >= 0:
            chosen = remaining.pop(best_idx)
            assignments[tname] = str(chosen)
        else:
            assignments[tname] = None
    return assignments


def discover_inputs(job_dir: Path, stems_dir_hint: Optional[Path]) -> Tuple[Path, Optional[Path], List[str]]:
    notes: List[str] = []
    midi_candidates = sorted([p for p in job_dir.iterdir() if p.is_file() and p.suffix.lower() in {".mid", ".midi"}])
    if not midi_candidates:
        raise FileNotFoundError(f"No MIDI file found at job root: {job_dir}")
    source_mid = midi_candidates[0]
    if len(midi_candidates) > 1:
        notes.append(
            f"Multiple MIDI files found. Using first (sorted): {source_mid.name}; ignored={len(midi_candidates) - 1}"
        )

    stems_dir = None
    if stems_dir_hint is not None:
        sd = stems_dir_hint if stems_dir_hint.is_absolute() else (job_dir / stems_dir_hint)
        if sd.exists() and sd.is_dir():
            stems_dir = sd
        else:
            notes.append(f"Explicit stems dir not found: {sd}")
    if stems_dir is None:
        candidates: List[Tuple[int, Path]] = []
        for child in sorted(job_dir.iterdir()):
            if not child.is_dir():
                continue
            wav_count = len([p for p in child.rglob("*") if p.is_file() and p.suffix.lower() in {".wav", ".wave"}])
            if wav_count > 0:
                candidates.append((wav_count, child))
        if candidates:
            candidates.sort(key=lambda x: x[0], reverse=True)
            stems_dir = candidates[0][1]
            if len(candidates) > 1:
                notes.append(
                    f"Multiple stems directories found. Using '{stems_dir.name}' with {candidates[0][0]} WAV files."
                )
    if stems_dir is None:
        notes.append("No stems directory detected; CC generation proceeds from MIDI logic only.")
    return source_mid, stems_dir, notes


def uniquify_base_name(base_name: str, used: Set[str]) -> str:
    if base_name not in used:
        used.add(base_name)
        return base_name
    i = 2
    while True:
        candidate = f"{base_name} {i}"
        if candidate not in used:
            used.add(candidate)
            return candidate
        i += 1


def prepare_one_track(
    mid: mido.MidiFile,
    track_index: int,
    dataset: mpr.ArticulationDataset,
    cfg: Dict[str, Any],
    tempo_map: Sequence[Tuple[int, int]],
    strip_cc_controls: Set[int],
    base_name: str,
    assigned_stem: Optional[str],
    dataset_path: Optional[Path],
    dataset_code: Optional[str],
) -> Optional[TrackPrepResult]:
    track = mid.tracks[track_index]
    source_name = track_name_of(track, f"Track {track_index}")
    note_events = mpr.extract_note_events(mid, track_index=track_index, tempo_map=tempo_map)
    if not note_events:
        return None

    mode = str(cfg["analysis"]["mode"])
    ks_note_ids, articulation_events, detection_mode, ks_diag = detect_strict_keyswitch_ids(
        note_events=note_events,
        dataset=dataset,
        track_name=source_name,
        mode=mode,
        tpq=mid.ticks_per_beat,
        tempo_map=tempo_map,
    )
    note_decisions = ecc.collect_music_notes(note_events, articulation_events, ks_note_ids, cfg)
    if not note_decisions:
        # Usually means this track is KS-only or has no musical notes after split.
        return None

    flags_by_key = build_keyswitch_flags(note_events, ks_note_ids)
    pairs = parse_note_pairs(track)
    ks_message_indices: Set[int] = set()
    ks_note_count = 0
    for p in pairs:
        key = (p.start_tick, p.end_tick, p.pitch, p.channel, p.velocity)
        dq = flags_by_key.get(key)
        if dq and len(dq) > 0:
            is_ks = bool(dq.popleft())
        else:
            # Fallback: pitch-level KS pool membership.
            is_ks = int(p.pitch) in dataset.keyswitch_note_pool
        if is_ks:
            ks_message_indices.add(p.on_index)
            ks_message_indices.add(p.off_index)
            ks_note_count += 1

    note_channel = max(0, min(15, int(note_events[0].channel) - 1)) if note_events else 0

    cc_generation_notes: List[str] = []
    cc_meta: Dict[str, Any] = {
        "mode": "midi_fallback",
        "cc1_count": 0,
        "cc11_count": 0,
        "density_per_min_cc1": 0.0,
        "density_per_min_cc11": 0.0,
        "span_sec": 0.0,
        "envelope_points": 0,
        "audio_rms_db_range": None,
    }

    generated_cc: List[AbsEvent] = []
    stem_path = Path(assigned_stem) if assigned_stem else None
    if stem_path is not None:
        envelope, env_notes = extract_audio_envelope_ffmpeg(stem_path)
        cc_generation_notes.extend(env_notes)
        if envelope is not None:
            try:
                generated_cc, cc_meta = generate_cc_events_from_audio(
                    note_decisions=note_decisions,
                    cfg=cfg,
                    midi_channel=note_channel,
                    seed_tick=0,
                    envelope=envelope,
                    tpq=mid.ticks_per_beat,
                    tempo_map=tempo_map,
                )
            except Exception as exc:
                cc_generation_notes.append(f"Audio envelope CC generation failed; fallback to MIDI-note CC: {exc!r}")

    if not generated_cc:
        generated_cc = generate_cc_events(note_decisions, cfg=cfg, midi_channel=note_channel, seed_tick=0)
        # Derive fallback meta from generated events.
        cc1_num = int(cfg["output"]["cc_map"]["cc1"])
        cc11_num = int(cfg["output"]["cc_map"]["cc11"])
        cc1_count = sum(
            1
            for e in generated_cc
            if e.msg.type == "control_change" and int(getattr(e.msg, "control", -1)) == cc1_num
        )
        cc11_count = sum(
            1
            for e in generated_cc
            if e.msg.type == "control_change" and int(getattr(e.msg, "control", -1)) == cc11_num
        )
        span_sec = 0.0
        if note_decisions:
            span_sec = max(0.0, float(note_decisions[-1].end_sec) - float(note_decisions[0].start_sec))
        density_div = max(span_sec / 60.0, 1e-9)
        cc_meta = {
            "mode": "midi_fallback",
            "cc1_count": int(cc1_count),
            "cc11_count": int(cc11_count),
            "density_per_min_cc1": float(cc1_count / density_div) if span_sec > 0 else 0.0,
            "density_per_min_cc11": float(cc11_count / density_div) if span_sec > 0 else 0.0,
            "span_sec": float(span_sec),
            "envelope_points": 0,
            "audio_rms_db_range": None,
        }
        if stem_path is None:
            cc_generation_notes.append("No stem assigned; used MIDI-note fallback CC generation.")
        else:
            cc_generation_notes.append("Stem envelope unavailable; used MIDI-note fallback CC generation.")

    original_events: List[AbsEvent] = []
    ks_events: List[AbsEvent] = []
    artmap_events: List[AbsEvent] = []
    stripped_counter = Counter()
    kept_non_note_messages = 0
    dropped_non_note_messages = 0

    abs_msgs = list_abs_track_messages(track)
    for i, tick, msg in abs_msgs:
        if msg.is_meta:
            # Track-scoped meta is rebuilt, so skip.
            continue
        if msg.type in {"note_on", "note_off"}:
            if i in ks_message_indices:
                ks_events.append(AbsEvent(tick=tick, priority=30, order=i, msg=msg.copy(time=0)))
            else:
                evt = AbsEvent(tick=tick, priority=30, order=i, msg=msg.copy(time=0))
                original_events.append(evt)
                artmap_events.append(evt)
            continue

        if msg.type == "control_change":
            cc_num = int(getattr(msg, "control", -1))
            if cc_num in strip_cc_controls:
                stripped_counter[str(cc_num)] += 1
            # Drop all source CCs to avoid duplicate/double automation and
            # cross-channel import artifacts in DP; regenerated CC1/CC11 are added later.
            dropped_non_note_messages += 1
            continue

        # Drop all remaining non-note source MIDI events from the main track:
        # this pipeline uses only notes + regenerated CC streams.
        dropped_non_note_messages += 1
        continue

    # Inject generated CC into the original musical track.
    original_events.extend(generated_cc)

    return TrackPrepResult(
        source_track_index=track_index,
        source_track_name=source_name,
        output_base_name=base_name,
        output_notes_name=base_name,
        output_ks_name=f"{base_name} KS",
        output_artmap_name=f"{base_name} ArtMap",
        note_count=len(note_decisions),
        ks_note_count=ks_note_count,
        stripped_cc_counts={"cc1": int(stripped_counter.get("1", 0)), "cc11": int(stripped_counter.get("11", 0))},
        detection_mode=detection_mode,
        profile_counts=profile_counts(note_decisions),
        ks_diagnostics=ks_diag,
        dataset_path=(str(dataset_path) if dataset_path else None),
        dataset_code=dataset_code,
        kept_non_note_messages=kept_non_note_messages,
        dropped_non_note_messages=dropped_non_note_messages,
        cc_generation_mode=str(cc_meta.get("mode", "midi_fallback")),
        generated_cc_counts={
            "cc1": int(cc_meta.get("cc1_count", 0)),
            "cc11": int(cc_meta.get("cc11_count", 0)),
        },
        generated_cc_density_per_min={
            "cc1": float(cc_meta.get("density_per_min_cc1", 0.0)),
            "cc11": float(cc_meta.get("density_per_min_cc11", 0.0)),
        },
        generated_cc_span_sec=float(cc_meta.get("span_sec", 0.0)),
        audio_envelope_points=int(cc_meta.get("envelope_points", 0)),
        audio_rms_db_range=(
            [float(cc_meta["audio_rms_db_range"][0]), float(cc_meta["audio_rms_db_range"][1])]
            if isinstance(cc_meta.get("audio_rms_db_range"), list) and len(cc_meta.get("audio_rms_db_range")) == 2
            else None
        ),
        cc_generation_notes=cc_generation_notes,
        assigned_stem=assigned_stem,
        original_events=original_events,
        ks_events=ks_events,
        artmap_events=artmap_events,
    )


def run_job(
    job_dir: Path,
    output_root: Path,
    config_path: Optional[Path],
    articulations_path: Optional[Path],
    stems_dir_hint: Optional[Path],
) -> Tuple[Path, Path]:
    source_mid, stems_dir, input_notes = discover_inputs(job_dir, stems_dir_hint=stems_dir_hint)
    mid = mido.MidiFile(source_mid)
    tempo_map = mpr.build_tempo_map(mid)

    cfg = ecc.load_config(config_path, ecc.default_config())
    mode = str(cfg["analysis"]["mode"])

    base_art_path: Optional[Path] = None
    if articulations_path and articulations_path.exists():
        base_art_path = articulations_path
    dataset_cache: Dict[str, mpr.ArticulationDataset] = {}

    def load_dataset_cached(path: Optional[Path]) -> mpr.ArticulationDataset:
        if path is None:
            key = "__empty__"
            if key not in dataset_cache:
                dataset_cache[key] = mpr.load_articulation_dataset(None, mode=mode)
            return dataset_cache[key]
        key = str(path.resolve())
        if key not in dataset_cache:
            dataset_cache[key] = mpr.load_articulation_dataset(path, mode=mode)
        return dataset_cache[key]

    stem_files = find_stems(stems_dir)

    note_track_indices = [i for i, tr in enumerate(mid.tracks) if has_note_messages(tr)]
    source_name_by_index = {i: track_name_of(mid.tracks[i], f"Track {i}") for i in note_track_indices}
    filtered_note_track_indices = list(note_track_indices)

    source_track_names = [source_name_by_index[i] for i in filtered_note_track_indices]
    stem_assignment = assign_stems(source_track_names, stem_files)

    used_base_names: Set[str] = set()
    prepared: List[TrackPrepResult] = []
    removed_tracks: List[Dict[str, Any]] = []
    strip_cc_controls = {1, 11}

    for i in filtered_note_track_indices:
        src_name = sanitize_track_name(source_name_by_index[i])
        base_name = uniquify_base_name(src_name, used_base_names)
        src_track_name = source_name_by_index[i]
        track_dataset_path, track_dataset_code = resolve_dataset_path_for_track(src_track_name, base_art_path)
        dataset = load_dataset_cached(track_dataset_path)
        # Some instrument datasets are stubs (no articulations/KS definitions).
        # Fallback to the base dataset for extraction in that case.
        if (
            base_art_path is not None
            and track_dataset_path is not None
            and len(dataset.keyswitch_note_pool) == 0
        ):
            track_dataset_path = base_art_path
            dataset = load_dataset_cached(track_dataset_path)
            track_dataset_code = f"{track_dataset_code or 'track'}->global"
        result = prepare_one_track(
            mid=mid,
            track_index=i,
            dataset=dataset,
            cfg=cfg,
            tempo_map=tempo_map,
            strip_cc_controls=strip_cc_controls,
            base_name=base_name,
            assigned_stem=stem_assignment.get(src_track_name),
            dataset_path=track_dataset_path,
            dataset_code=track_dataset_code,
        )
        if result is None:
            removed_tracks.append(
                {
                    "source_track_index": i,
                    "source_track_name": track_name_of(mid.tracks[i], f"Track {i}"),
                    "reason": "no musical notes after KS split (empty or KS-only track)",
                }
            )
            continue
        prepared.append(result)

    if not prepared:
        raise RuntimeError("No instrument tracks produced after preprocessing.")

    out_job_dir = output_root / job_dir.name
    out_job_dir.mkdir(parents=True, exist_ok=True)
    out_midi_path = out_job_dir / f"{source_mid.stem}__DP_PREP.mid"
    out_qc_path = out_job_dir / f"{source_mid.stem}__DP_PREP_QC.json"

    out_mid = mido.MidiFile(type=1, ticks_per_beat=mid.ticks_per_beat)
    global_events = collect_global_meta_events(mid)
    out_mid.tracks.append(build_track_from_events("ECT Conductor", global_events))
    for tr in prepared:
        out_mid.tracks.append(build_track_from_events(tr.output_notes_name, tr.original_events))
        if tr.ks_events:
            out_mid.tracks.append(build_track_from_events(tr.output_ks_name, tr.ks_events))
        if tr.artmap_events:
            out_mid.tracks.append(build_track_from_events(tr.output_artmap_name, tr.artmap_events))
    out_mid.save(out_midi_path)

    qc = {
        "tool": "ect-score-job-cli",
        "timestamp_utc": utc_now(),
        "job_dir": str(job_dir),
        "source_midi": str(source_mid),
        "stems_dir": str(stems_dir) if stems_dir else None,
        "stems_found": len(stem_files),
        "notes": input_notes,
        "config_path": str(config_path) if config_path else "defaults",
        "articulations_path": (
            str(articulations_path)
            if articulations_path and articulations_path.exists()
            else "none"
        ),
        "dataset_cache_entries": list(dataset_cache.keys()),
        "source_track_count": len(mid.tracks),
        "source_note_track_count": len(note_track_indices),
        "removed_tracks": removed_tracks,
        "output_midi": str(out_midi_path),
        "output_track_count": len(out_mid.tracks),
        "tracks": [
            {
                "source_track_index": tr.source_track_index,
                "source_track_name": tr.source_track_name,
                "output_notes_track": tr.output_notes_name,
                "output_ks_track": tr.output_ks_name,
                "output_artmap_track": tr.output_artmap_name,
                "note_count": tr.note_count,
                "ks_note_count": tr.ks_note_count,
                "stripped_cc_counts": tr.stripped_cc_counts,
                "detection_mode": tr.detection_mode,
                "profile_counts": tr.profile_counts,
                "ks_diagnostics": tr.ks_diagnostics,
                "dataset_path": tr.dataset_path,
                "dataset_code": tr.dataset_code,
                "kept_non_note_messages": tr.kept_non_note_messages,
                "dropped_non_note_messages": tr.dropped_non_note_messages,
                "cc_generation_mode": tr.cc_generation_mode,
                "generated_cc_counts": tr.generated_cc_counts,
                "generated_cc_density_per_min": tr.generated_cc_density_per_min,
                "generated_cc_span_sec": tr.generated_cc_span_sec,
                "audio_envelope_points": tr.audio_envelope_points,
                "audio_rms_db_range": tr.audio_rms_db_range,
                "cc_generation_notes": tr.cc_generation_notes,
                "ks_track_written": bool(tr.ks_events),
                "artmap_track_written": bool(tr.artmap_events),
                "assigned_stem": tr.assigned_stem,
            }
            for tr in prepared
        ],
    }
    out_qc_path.write_text(json.dumps(qc, indent=2), encoding="utf-8")
    return out_midi_path, out_qc_path


def parse_args() -> argparse.Namespace:
    ap = argparse.ArgumentParser(
        description="Process one per-score Dorico package (MIDI + stems) into DP-ready MIDI triplets with regenerated CC1/CC11."
    )
    ap.add_argument(
        "--job-dir",
        required=True,
        type=Path,
        help="Per-score directory dropped into inbox (contains Dorico MIDI + stems directory).",
    )
    ap.add_argument(
        "--output-root",
        type=Path,
        default=OUTBOX_DEFAULT,
        help="Directory where processed score outputs are written.",
    )
    ap.add_argument(
        "--config",
        type=Path,
        default=REPO_ROOT / "contracts" / "config.template.yaml",
        help="ECT config used for profile/CC generation.",
    )
    ap.add_argument(
        "--articulations",
        type=Path,
        default=Path("/Users/sk/MAXMSP Patches/Synchron_MFT_Master 2_WORK/articulations.json"),
        help="Articulation dataset used for keyswitch detection.",
    )
    ap.add_argument(
        "--stems-dir",
        type=Path,
        default=None,
        help="Optional explicit stems directory (relative to job dir or absolute).",
    )
    return ap.parse_args()


def main() -> int:
    args = parse_args()
    job_dir = args.job_dir if args.job_dir.is_absolute() else (Path.cwd() / args.job_dir)
    if not job_dir.exists() or not job_dir.is_dir():
        raise FileNotFoundError(f"Job dir not found: {job_dir}")

    output_root = args.output_root if args.output_root.is_absolute() else (Path.cwd() / args.output_root)
    output_root.mkdir(parents=True, exist_ok=True)
    out_midi, out_qc = run_job(
        job_dir=job_dir.resolve(),
        output_root=output_root.resolve(),
        config_path=args.config if args.config and args.config.exists() else None,
        articulations_path=args.articulations if args.articulations and args.articulations.exists() else None,
        stems_dir_hint=args.stems_dir,
    )
    print(f"Wrote: {out_midi}")
    print(f"Wrote: {out_qc}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
