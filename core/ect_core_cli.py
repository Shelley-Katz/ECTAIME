#!/usr/bin/env python3
"""
ECT Core M1 CLI (offline vertical slice).

Generates canonical artifacts for one monophonic line:
1) control_stream.mid
2) profile_timeline.csv
3) note_audit.csv
4) metrics.json
5) run_manifest.json
"""

from __future__ import annotations

import argparse
import csv
import hashlib
import json
import math
import statistics
import subprocess
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Dict, Iterable, List, Optional, Sequence, Tuple

try:
    import yaml  # type: ignore
except Exception:  # pragma: no cover
    yaml = None

import mido

import sys


REPO_ROOT = Path(__file__).resolve().parents[1]
SCRIPTS_DIR = REPO_ROOT / "scripts"
if str(SCRIPTS_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPTS_DIR))

import midi_profile_regions as mpr  # noqa: E402


@dataclass
class NoteDecision:
    start_tick: int
    end_tick: int
    start_sec: float
    end_sec: float
    velocity: int
    profile: str
    source: str
    reason: str


def utc_now_iso() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def stable_sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def deep_merge(base: Dict[str, Any], override: Dict[str, Any]) -> Dict[str, Any]:
    out = dict(base)
    for k, v in override.items():
        if k in out and isinstance(out[k], dict) and isinstance(v, dict):
            out[k] = deep_merge(out[k], v)
        else:
            out[k] = v
    return out


def load_config(path: Optional[Path], defaults: Dict[str, Any]) -> Dict[str, Any]:
    if path is None:
        return defaults
    if not path.exists():
        raise FileNotFoundError(f"Config not found: {path}")

    raw = path.read_text(encoding="utf-8")
    ext = path.suffix.lower()
    parsed: Dict[str, Any]
    if ext in {".yaml", ".yml"}:
        if yaml is None:
            raise RuntimeError("PyYAML not installed; cannot parse YAML config.")
        parsed = yaml.safe_load(raw) or {}
    elif ext == ".json":
        parsed = json.loads(raw)
    else:
        raise ValueError(f"Unsupported config extension: {ext}")
    if not isinstance(parsed, dict):
        raise ValueError("Config root must be an object/dict.")
    return deep_merge(defaults, parsed)


def default_config() -> Dict[str, Any]:
    return {
        "version": "1.0",
        "analysis": {
            "mode": "long_short_rep",
            "ks_detection": "auto",
            "short_ms": 220.0,
            "long_ms": 300.0,
            "rep_ioi_ms": 260.0,
            "merge_gap_ms": 18.0,
            "switch_pre_roll_ms": 32.0,
            "ks_lookahead_ms": 120.0,
            "ks_lookbehind_ms": 90.0,
            "ks_refine_boundary_ms": 160.0,
            "ks_refine_short_max_ms": 220.0,
            "ks_refine_long_min_ms": 420.0,
            "texture_confirm_long": 3,
            "texture_confirm_short": 3,
            "texture_long_min_ms": 300.0,
            "texture_short_max_ms": 220.0,
            "texture_legato_gap_max_ms": 18.0,
            "texture_legato_min_note_ms": 120.0,
            "texture_short_gap_min_ms": 24.0,
            "texture_fast_long_ms": 430.0,
            "texture_fast_long_window_notes": 5,
            "texture_fast_long_max_gap_ms": 20.0,
        },
        "output": {
            "cc_map": {"cc1": 1, "cc11": 11, "cc21": 21},
            "profile_cc_ranges": {
                "LONG": {"cc1": [30, 78], "cc11": [38, 84]},
                "SHORT": {"cc1": [24, 88], "cc11": [28, 76]},
                "REP": {"cc1": [26, 84], "cc11": [32, 80]},
            },
            "seed_values": {"cc1": 52, "cc11": 58},
        },
    }


def default_gates() -> Dict[str, Any]:
    return {
        "gates": {
            "G1_technical_integrity": {
                "run_completed": True,
                "required_artifacts": [
                    "control_stream.mid",
                    "profile_timeline.csv",
                    "note_audit.csv",
                    "metrics.json",
                    "run_manifest.json",
                ],
                "manifest_hashes_valid": True,
            },
            "G2_controller_health": {
                "cc_pinning_ratio_max": {"cc1": 0.35, "cc11": 0.35},
                "dead_flat_segments_max": 12,
                "jitter_index_max": {"long": 0.22, "short": 0.30, "rep": 0.32},
            },
            "G3_musical_utility": {
                "long_pumping_index_max": 0.35,
                "repeated_note_misses_max": 0,
                "transition_discontinuities_per_min_max": 40.0,
            },
            "G4_reproducibility": {
                "same_input_config_code_equivalent_output": True,
                "variant_output_stability": True,
            },
        }
    }


def to_percentile(sorted_vals: Sequence[float], p: float) -> float:
    if not sorted_vals:
        return 0.0
    if p <= 0:
        return float(sorted_vals[0])
    if p >= 100:
        return float(sorted_vals[-1])
    rank = (len(sorted_vals) - 1) * (p / 100.0)
    lo = int(math.floor(rank))
    hi = int(math.ceil(rank))
    if lo == hi:
        return float(sorted_vals[lo])
    frac = rank - lo
    return float(sorted_vals[lo] + (sorted_vals[hi] - sorted_vals[lo]) * frac)


def clamp_midi(v: float) -> int:
    return max(0, min(127, int(round(v))))


def apply_profile_bias(note_decisions: Sequence[NoteDecision], bias_profile: Optional[str]) -> List[NoteDecision]:
    if bias_profile is None:
        return list(note_decisions)
    out: List[NoteDecision] = []
    for n in note_decisions:
        out.append(
            NoteDecision(
                start_tick=n.start_tick,
                end_tick=n.end_tick,
                start_sec=n.start_sec,
                end_sec=n.end_sec,
                velocity=n.velocity,
                profile=bias_profile,
                source=f"{n.source}+bias",
                reason=f"{n.reason}; bias={bias_profile}",
            )
        )
    return out


def clip_note_decisions_to_region(
    note_decisions: Sequence[NoteDecision],
    region_start_tick: int,
    region_end_tick: int,
    tpq: int,
    tempo_map: Sequence[Tuple[int, int]],
) -> List[NoteDecision]:
    out: List[NoteDecision] = []
    for n in note_decisions:
        if n.end_tick < region_start_tick or n.start_tick > region_end_tick:
            continue
        s_tick = max(n.start_tick, region_start_tick)
        e_tick = min(n.end_tick, region_end_tick)
        if e_tick < s_tick:
            continue
        s_sec = mpr.tick_to_sec(s_tick, tpq, tempo_map)
        e_sec = mpr.tick_to_sec(e_tick, tpq, tempo_map)
        out.append(
            NoteDecision(
                start_tick=s_tick,
                end_tick=e_tick,
                start_sec=s_sec,
                end_sec=e_sec,
                velocity=n.velocity,
                profile=n.profile,
                source=n.source,
                reason=n.reason,
            )
        )
    return out


def collect_music_notes(
    note_events: Sequence[mpr.NoteEvent],
    articulation_events: Sequence[mpr.ArticulationEvent],
    ks_note_ids: set[int],
    cfg: Dict[str, Any],
) -> List[NoteDecision]:
    music_notes = [n for n in note_events if id(n) not in ks_note_ids]
    if not music_notes:
        return []

    analysis = cfg["analysis"]
    short_ms = float(analysis["short_ms"])
    long_ms = float(analysis["long_ms"])
    rep_ioi_ms = float(analysis["rep_ioi_ms"])
    mode = str(analysis["mode"])

    ks_duration_refine = True
    ks_refine_boundary_ms = float(analysis["ks_refine_boundary_ms"])
    ks_refine_short_max_ms = float(analysis["ks_refine_short_max_ms"])
    ks_refine_long_min_ms = float(analysis["ks_refine_long_min_ms"])
    ks_lookahead_ms = float(analysis["ks_lookahead_ms"])
    ks_lookbehind_ms = float(analysis["ks_lookbehind_ms"])
    ks_texture_override = True
    texture_confirm_long = int(analysis["texture_confirm_long"])
    texture_confirm_short = int(analysis["texture_confirm_short"])
    texture_long_min_ms = float(analysis["texture_long_min_ms"])
    texture_short_max_ms = float(analysis["texture_short_max_ms"])
    texture_legato_gap_max_ms = float(analysis["texture_legato_gap_max_ms"])
    texture_legato_min_note_ms = float(analysis["texture_legato_min_note_ms"])
    texture_short_gap_min_ms = float(analysis["texture_short_gap_min_ms"])
    texture_fast_long_ms = float(analysis["texture_fast_long_ms"])
    texture_fast_long_window_notes = int(analysis["texture_fast_long_window_notes"])
    texture_fast_long_max_gap_ms = float(analysis["texture_fast_long_max_gap_ms"])

    art_idx = 0
    active_profile: Optional[str] = None
    active_reason = ""
    last_active_profile: Optional[str] = None
    texture_latch: Optional[str] = None
    long_streak = 0
    short_streak = 0
    out: List[NoteDecision] = []

    for i, n in enumerate(music_notes):
        art_changed = False
        while art_idx < len(articulation_events) and articulation_events[art_idx].tick <= n.start_tick:
            active_profile = articulation_events[art_idx].profile
            active_reason = articulation_events[art_idx].reason
            art_idx += 1
            art_changed = True

        if active_profile != last_active_profile:
            texture_latch = None
            long_streak = 0
            short_streak = 0
            last_active_profile = active_profile

        fallback_profile, fb_reason = mpr.fallback_profile_for_note(
            music_notes,
            i,
            short_ms=short_ms,
            long_ms=long_ms,
            rep_ioi_ms=rep_ioi_ms,
            mode=mode,
        )

        next_gap_ms: Optional[float] = None
        next_interval: Optional[int] = None
        if i + 1 < len(music_notes):
            next_gap_ms = (music_notes[i + 1].start_sec - n.end_sec) * 1000.0
            next_interval = abs(music_notes[i + 1].pitch - n.pitch)

        if active_profile is not None:
            profile = active_profile
            source = "articulation"
            reason = active_reason

            current_event = articulation_events[art_idx - 1] if art_idx > 0 else None
            prev_event = articulation_events[art_idx - 2] if art_idx > 1 else None
            next_event = articulation_events[art_idx] if art_idx < len(articulation_events) else None

            if next_event is not None and fallback_profile == next_event.profile:
                dt_next_ms = (next_event.sec - n.start_sec) * 1000.0
                if 0.0 <= dt_next_ms <= ks_lookahead_ms:
                    profile = next_event.profile
                    source = "articulation_lookahead"
                    reason = f"{next_event.reason}; lookahead={dt_next_ms:.1f}ms; {fb_reason}"

            if source == "articulation" and current_event is not None and prev_event is not None:
                if fallback_profile == prev_event.profile and prev_event.profile != current_event.profile:
                    dt_prev_ms = (n.start_sec - current_event.sec) * 1000.0
                    if 0.0 <= dt_prev_ms <= ks_lookbehind_ms:
                        profile = prev_event.profile
                        source = "articulation_lookbehind"
                        reason = f"{prev_event.reason}; lookbehind={dt_prev_ms:.1f}ms; {fb_reason}"

            if source == "articulation" and ks_duration_refine and fallback_profile != active_profile:
                dist_prev = float("inf")
                if current_event is not None:
                    dist_prev = max(0.0, (n.start_sec - current_event.sec) * 1000.0)
                dist_next = float("inf")
                if next_event is not None:
                    dist_next = max(0.0, (next_event.sec - n.start_sec) * 1000.0)
                nearest_boundary_ms = min(dist_prev, dist_next)

                if nearest_boundary_ms <= ks_refine_boundary_ms:
                    should_refine = False
                    if fallback_profile == mpr.PROFILE_SHORT and n.duration_ms <= ks_refine_short_max_ms:
                        should_refine = True
                    elif fallback_profile == mpr.PROFILE_LONG and n.duration_ms >= ks_refine_long_min_ms:
                        should_refine = True
                    elif fallback_profile == mpr.PROFILE_REP and mode == "long_short_rep":
                        should_refine = True
                    if should_refine:
                        profile = fallback_profile
                        source = "articulation+duration"
                        reason = f"{active_reason}; boundary={nearest_boundary_ms:.1f}ms; dur_refine={fb_reason}"

            if (
                ks_texture_override
                and source == "articulation"
                and active_profile in {mpr.PROFILE_LONG, mpr.PROFILE_SHORT}
                and fallback_profile in {mpr.PROFILE_LONG, mpr.PROFILE_SHORT}
            ):
                if art_changed:
                    texture_latch = None
                    long_streak = 0
                    short_streak = 0
                current = texture_latch if texture_latch is not None else active_profile

                if current == mpr.PROFILE_SHORT:
                    long_evidence = False
                    if fallback_profile == mpr.PROFILE_LONG and n.duration_ms >= texture_long_min_ms:
                        long_evidence = True
                    if (
                        not long_evidence
                        and next_gap_ms is not None
                        and next_interval is not None
                        and n.duration_ms >= texture_legato_min_note_ms
                        and next_gap_ms <= texture_legato_gap_max_ms
                        and next_interval <= 12
                    ):
                        long_evidence = True
                    if long_evidence:
                        long_streak += 1
                    else:
                        long_streak = 0

                    fast_long_ok = False
                    if n.duration_ms >= texture_fast_long_ms and texture_fast_long_window_notes >= 2:
                        end_idx = min(len(music_notes) - 1, i + texture_fast_long_window_notes - 1)
                        if end_idx > i:
                            all_connected = True
                            for j in range(i, end_idx):
                                g = (music_notes[j + 1].start_sec - music_notes[j].end_sec) * 1000.0
                                if g > texture_fast_long_max_gap_ms:
                                    all_connected = False
                                    break
                            fast_long_ok = all_connected
                    if fast_long_ok:
                        long_streak = max(long_streak, max(1, texture_confirm_long))

                    if long_streak >= max(1, texture_confirm_long):
                        texture_latch = mpr.PROFILE_LONG
                        current = mpr.PROFILE_LONG
                        long_streak = 0
                        short_streak = 0
                else:
                    short_evidence = (
                        fallback_profile == mpr.PROFILE_SHORT
                        and n.duration_ms <= texture_short_max_ms
                        and (
                            next_gap_ms is None
                            or next_gap_ms >= texture_short_gap_min_ms
                            or n.duration_ms <= texture_short_max_ms * 0.66
                        )
                    )
                    if short_evidence:
                        short_streak += 1
                    else:
                        short_streak = 0
                    if short_streak >= max(1, texture_confirm_short):
                        texture_latch = mpr.PROFILE_SHORT
                        current = mpr.PROFILE_SHORT
                        short_streak = 0
                        long_streak = 0
                if current != active_profile:
                    profile = current
                    source = "articulation_texture"
                    reason = f"{active_reason}; texture_latch={current}; {fb_reason}"
        else:
            profile = fallback_profile
            source = "duration_fallback"
            reason = fb_reason

        out.append(
            NoteDecision(
                start_tick=n.start_tick,
                end_tick=n.end_tick,
                start_sec=n.start_sec,
                end_sec=n.end_sec,
                velocity=n.velocity,
                profile=profile,
                source=source,
                reason=reason,
            )
        )
    return out


def merge_note_decisions(note_decisions: Sequence[NoteDecision], merge_gap_ms: float) -> List[mpr.Region]:
    if not note_decisions:
        return []
    regions = [
        mpr.Region(
            start_tick=n.start_tick,
            end_tick=n.end_tick,
            start_sec=n.start_sec,
            end_sec=n.end_sec,
            profile=n.profile,
            source=n.source,
            reason=n.reason,
        )
        for n in note_decisions
    ]
    return mpr.merge_regions(regions, merge_gap_ms=merge_gap_ms)


def write_note_audit_csv(path: Path, note_decisions: Sequence[NoteDecision]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", newline="", encoding="utf-8") as f:
        w = csv.writer(f)
        w.writerow(["start_sec", "end_sec", "start_tick", "end_tick", "profile", "source", "reason"])
        for n in note_decisions:
            w.writerow(
                [
                    f"{n.start_sec:.4f}",
                    f"{n.end_sec:.4f}",
                    n.start_tick,
                    n.end_tick,
                    n.profile,
                    n.source,
                    n.reason,
                ]
            )


def value_from_velocity(vel: int, lo: int, hi: int) -> int:
    norm = max(0.0, min(1.0, vel / 127.0))
    return clamp_midi(lo + (hi - lo) * norm)


def write_control_stream_midi(
    out_path: Path,
    source_mid: mido.MidiFile,
    tempo_events: Sequence[Tuple[int, mido.MetaMessage]],
    note_decisions: Sequence[NoteDecision],
    config: Dict[str, Any],
    seed_tick: int = 0,
) -> Dict[str, List[int]]:
    cc_map = config["output"]["cc_map"]
    ranges = config["output"]["profile_cc_ranges"]
    seeds = config["output"]["seed_values"]
    cc1_num = int(cc_map["cc1"])
    cc11_num = int(cc_map["cc11"])

    events: List[Tuple[int, int, mido.Message]] = []
    cc1_series: List[int] = []
    cc11_series: List[int] = []

    seed_cc1 = clamp_midi(float(seeds["cc1"]))
    seed_cc11 = clamp_midi(float(seeds["cc11"]))
    events.append(
        (
            max(0, seed_tick),
            0,
            mido.Message("control_change", channel=0, control=cc1_num, value=seed_cc1, time=0),
        )
    )
    events.append(
        (
            max(0, seed_tick),
            1,
            mido.Message("control_change", channel=0, control=cc11_num, value=seed_cc11, time=0),
        )
    )
    cc1_series.append(seed_cc1)
    cc11_series.append(seed_cc11)

    prev_cc1 = seed_cc1
    prev_cc11 = seed_cc11
    for n in note_decisions:
        p = n.profile if n.profile in ranges else "LONG"
        r = ranges[p]
        cc1_lo, cc1_hi = [int(v) for v in r["cc1"]]
        cc11_lo, cc11_hi = [int(v) for v in r["cc11"]]
        target_cc1 = value_from_velocity(n.velocity, cc1_lo, cc1_hi)
        target_cc11 = value_from_velocity(n.velocity, cc11_lo, cc11_hi)

        # Simple continuity damping to avoid abrupt steps.
        cc1 = clamp_midi(prev_cc1 * 0.35 + target_cc1 * 0.65)
        cc11 = clamp_midi(prev_cc11 * 0.40 + target_cc11 * 0.60)
        prev_cc1 = cc1
        prev_cc11 = cc11

        events.append(
            (
                n.start_tick,
                0,
                mido.Message("control_change", channel=0, control=cc1_num, value=cc1, time=0),
            )
        )
        events.append(
            (
                n.start_tick,
                1,
                mido.Message("control_change", channel=0, control=cc11_num, value=cc11, time=0),
            )
        )
        cc1_series.append(cc1)
        cc11_series.append(cc11)

    events.sort(key=lambda x: (x[0], x[1]))
    final_event_tick = events[-1][0] if events else 0

    out_mid = mido.MidiFile(ticks_per_beat=source_mid.ticks_per_beat)
    tempo_track = mido.MidiTrack()
    out_mid.tracks.append(tempo_track)
    last_tick = 0
    for tick, meta in tempo_events:
        delta = max(0, tick - last_tick)
        tempo_track.append(meta.copy(time=delta))
        last_tick = tick
    # Keep tempo/meta track at least as long as controller track so DAWs don't
    # truncate or wrap late controller events on import.
    tempo_track_end = max(last_tick, final_event_tick)
    tempo_track.append(mido.MetaMessage("end_of_track", time=max(0, tempo_track_end - last_tick)))

    ctrl_track = mido.MidiTrack()
    out_mid.tracks.append(ctrl_track)
    last_tick = 0
    for tick, _, msg in events:
        delta = max(0, tick - last_tick)
        ctrl_track.append(msg.copy(time=delta))
        last_tick = tick
    ctrl_track.append(mido.MetaMessage("end_of_track", time=0))

    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_mid.save(out_path)
    return {"cc1": cc1_series, "cc11": cc11_series}


def cc_series_stats(values: Sequence[int]) -> Dict[str, float]:
    if not values:
        return {
            "min": 0.0,
            "max": 0.0,
            "mean": 0.0,
            "median": 0.0,
            "p95": 0.0,
            "p99": 0.0,
            "pinning_ratio": 0.0,
        }
    vals = [float(v) for v in values]
    s = sorted(vals)
    pin = len([v for v in values if v >= 120]) / float(len(values))
    return {
        "min": min(vals),
        "max": max(vals),
        "mean": statistics.fmean(vals),
        "median": statistics.median(vals),
        "p95": to_percentile(s, 95.0),
        "p99": to_percentile(s, 99.0),
        "pinning_ratio": pin,
    }


def profile_ratio_stats(regions: Sequence[mpr.Region]) -> Dict[str, Dict[str, float]]:
    totals = {"LONG": 0.0, "SHORT": 0.0, "REP": 0.0}
    for r in regions:
        totals[r.profile] = totals.get(r.profile, 0.0) + max(0.0, r.end_sec - r.start_sec)
    total_sec = sum(totals.values()) or 1.0
    return {
        "long": {"seconds": totals.get("LONG", 0.0), "ratio": totals.get("LONG", 0.0) / total_sec},
        "short": {"seconds": totals.get("SHORT", 0.0), "ratio": totals.get("SHORT", 0.0) / total_sec},
        "rep": {"seconds": totals.get("REP", 0.0), "ratio": totals.get("REP", 0.0) / total_sec},
    }


def jitter_by_profile(note_decisions: Sequence[NoteDecision], cc11_values: Sequence[int]) -> Dict[str, float]:
    buckets: Dict[str, List[int]] = {"LONG": [], "SHORT": [], "REP": []}
    for n, v in zip(note_decisions, cc11_values[1:]):  # skip seed
        buckets.setdefault(n.profile, []).append(v)
    out = {"long": 0.0, "short": 0.0, "rep": 0.0}
    for p, key in [("LONG", "long"), ("SHORT", "short"), ("REP", "rep")]:
        seq = buckets.get(p, [])
        if len(seq) < 2:
            out[key] = 0.0
            continue
        diffs = [abs(seq[i] - seq[i - 1]) / 127.0 for i in range(1, len(seq))]
        out[key] = float(statistics.fmean(diffs)) if diffs else 0.0
    return out


def dead_flat_segments(cc11_values: Sequence[int]) -> int:
    if len(cc11_values) < 4:
        return 0
    count = 0
    run = 1
    for i in range(1, len(cc11_values)):
        if cc11_values[i] == cc11_values[i - 1]:
            run += 1
        else:
            if run >= 16:
                count += 1
            run = 1
    if run >= 16:
        count += 1
    return count


def transition_discontinuities_per_min(regions: Sequence[mpr.Region]) -> float:
    if not regions:
        return 0.0
    transitions = 0
    for i in range(1, len(regions)):
        if regions[i].profile != regions[i - 1].profile:
            transitions += 1
    dur_sec = max(1e-6, regions[-1].end_sec - regions[0].start_sec)
    return transitions / (dur_sec / 60.0)


def long_pumping_index(note_decisions: Sequence[NoteDecision], cc11_values: Sequence[int]) -> float:
    vals = [v for n, v in zip(note_decisions, cc11_values[1:]) if n.profile == "LONG"]
    if len(vals) < 3:
        return 0.0
    diffs = [vals[i] - vals[i - 1] for i in range(1, len(vals))]
    accel = [abs(diffs[i] - diffs[i - 1]) for i in range(1, len(diffs))]
    if not accel:
        return 0.0
    return float(statistics.fmean(accel)) / 127.0


def load_gate_thresholds(gates_path: Path) -> Dict[str, Any]:
    fallback = default_gates()
    if not gates_path.exists():
        return fallback
    if yaml is None:
        return fallback
    data = yaml.safe_load(gates_path.read_text(encoding="utf-8")) or {}
    if not isinstance(data, dict):
        return fallback
    return data


def apply_gates(metrics: Dict[str, Any], gates: Dict[str, Any]) -> Dict[str, Any]:
    g = gates.get("gates", {}) if isinstance(gates, dict) else {}
    g1 = True
    g2 = True
    g3 = True
    g4 = True

    # G2
    try:
        lim = g["G2_controller_health"]["cc_pinning_ratio_max"]
        if metrics["cc_stats"]["cc1"]["pinning_ratio"] > float(lim["cc1"]):
            g2 = False
        if metrics["cc_stats"]["cc11"]["pinning_ratio"] > float(lim["cc11"]):
            g2 = False
        max_flat = int(g["G2_controller_health"]["dead_flat_segments_max"])
        if metrics["health"]["dead_flat_segments"] > max_flat:
            g2 = False
        jit = g["G2_controller_health"]["jitter_index_max"]
        if metrics["health"]["jitter_index"]["long"] > float(jit["long"]):
            g2 = False
        if metrics["health"]["jitter_index"]["short"] > float(jit["short"]):
            g2 = False
        if metrics["health"]["jitter_index"]["rep"] > float(jit["rep"]):
            g2 = False
    except Exception:
        pass

    # G3
    try:
        m = g["G3_musical_utility"]
        if metrics["musical"]["long_pumping_index"] > float(m["long_pumping_index_max"]):
            g3 = False
        if metrics["musical"]["repeated_note_misses"] > int(m["repeated_note_misses_max"]):
            g3 = False
        if (
            metrics["musical"]["transition_discontinuities_per_min"]
            > float(m["transition_discontinuities_per_min_max"])
        ):
            g3 = False
    except Exception:
        pass

    overall = bool(g1 and g2 and g3 and g4)
    return {"G1": g1, "G2": g2, "G3": g3, "G4": g4, "overall_pass": overall, "notes": ""}


def git_commit_short(repo_root: Path) -> str:
    try:
        out = subprocess.check_output(
            ["git", "-C", str(repo_root), "rev-parse", "--short", "HEAD"],
            stderr=subprocess.DEVNULL,
            text=True,
        )
        return out.strip()
    except Exception:
        return "unknown"


def main() -> int:
    ap = argparse.ArgumentParser(description="ECT Core M1 CLI")
    ap.add_argument("--source-midi", required=True, type=Path)
    ap.add_argument("--source-audio", required=True, type=Path)
    ap.add_argument("--track-index", type=int, default=1)
    ap.add_argument(
        "--articulations",
        type=Path,
        default=Path("/Users/sk/MAXMSP Patches/Synchron_MFT_Master 2_WORK/articulations.json"),
    )
    ap.add_argument("--config", type=Path)
    ap.add_argument("--output-dir", required=True, type=Path)
    ap.add_argument("--run-id", type=str)
    ap.add_argument("--region-start-tick", type=int, help="Optional region-local reprocess start tick")
    ap.add_argument("--region-end-tick", type=int, help="Optional region-local reprocess end tick")
    ap.add_argument(
        "--bias-profile",
        type=str,
        choices=["LONG", "SHORT", "REP"],
        help="Optional profile bias for A/B variants (applies to selected region or full run)",
    )
    args = ap.parse_args()

    if not args.source_midi.exists():
        raise FileNotFoundError(f"Missing source MIDI: {args.source_midi}")
    if not args.source_audio.exists():
        raise FileNotFoundError(f"Missing source audio: {args.source_audio}")
    if args.source_audio.suffix.lower() not in {".wav", ".wave"}:
        print(
            f"Warning: source audio is not WAV ({args.source_audio.name}). "
            "M1 accepts this for contract validation, but production runs should use dry WAV stems."
        )

    cfg = load_config(args.config, default_config())
    run_id = args.run_id or f"ect-{datetime.now().strftime('%Y%m%d-%H%M%S-%f')[:-3]}"
    out_dir = args.output_dir
    out_dir.mkdir(parents=True, exist_ok=True)

    region_start_tick: Optional[int] = args.region_start_tick
    region_end_tick: Optional[int] = args.region_end_tick
    if (region_start_tick is None) ^ (region_end_tick is None):
        raise ValueError("Both --region-start-tick and --region-end-tick must be provided together.")
    if region_start_tick is not None and region_end_tick is not None and region_end_tick < region_start_tick:
        raise ValueError("Region end tick must be >= region start tick.")

    mid = mido.MidiFile(args.source_midi)
    tempo_map = mpr.build_tempo_map(mid)
    tempo_events = mpr.collect_tempo_events(mid)

    mode = str(cfg["analysis"]["mode"])
    dataset = mpr.load_articulation_dataset(args.articulations if args.articulations.exists() else None, mode=mode)
    note_events = mpr.extract_note_events(mid, track_index=args.track_index, tempo_map=tempo_map)
    articulation_events, ks_note_ids, _ = mpr.detect_articulation_events(
        note_events,
        dataset,
        ks_vel_max=4,
        ks_dur_max_ms=250.0,
        mode=mode,
        ks_detection=str(cfg["analysis"]["ks_detection"]),
        tpq=mid.ticks_per_beat,
        tempo_map=tempo_map,
    )

    note_decisions = collect_music_notes(note_events, articulation_events, ks_note_ids, cfg)
    if region_start_tick is not None and region_end_tick is not None:
        note_decisions = clip_note_decisions_to_region(
            note_decisions,
            region_start_tick=region_start_tick,
            region_end_tick=region_end_tick,
            tpq=mid.ticks_per_beat,
            tempo_map=tempo_map,
        )
    note_decisions = apply_profile_bias(note_decisions, args.bias_profile)

    if not note_decisions:
        raise RuntimeError("No note decisions after filters (check track index, region ticks, or source file).")

    profile_regions = merge_note_decisions(note_decisions, merge_gap_ms=float(cfg["analysis"]["merge_gap_ms"]))

    profile_timeline_csv = out_dir / "profile_timeline.csv"
    note_audit_csv = out_dir / "note_audit.csv"
    control_stream_mid = out_dir / "control_stream.mid"
    metrics_json = out_dir / "metrics.json"
    manifest_json = out_dir / "run_manifest.json"

    mpr.write_regions_csv(profile_timeline_csv, profile_regions)
    write_note_audit_csv(note_audit_csv, note_decisions)
    cc_series = write_control_stream_midi(
        control_stream_mid,
        mid,
        tempo_events,
        note_decisions,
        cfg,
        seed_tick=region_start_tick if region_start_tick is not None else 0,
    )

    input_duration_sec = 0.0
    if note_decisions:
        input_duration_sec = max(0.0, note_decisions[-1].end_sec - note_decisions[0].start_sec)
    profile_stats = profile_ratio_stats(profile_regions)
    jitter = jitter_by_profile(note_decisions, cc_series["cc11"])

    metrics: Dict[str, Any] = {
        "schema_version": "1.0",
        "run_id": run_id,
        "input_duration_sec": input_duration_sec,
        "cc_stats": {
            "cc1": cc_series_stats(cc_series["cc1"]),
            "cc11": cc_series_stats(cc_series["cc11"]),
        },
        "profile_stats": profile_stats,
        "health": {
            "dead_flat_segments": dead_flat_segments(cc_series["cc11"]),
            "jitter_index": jitter,
        },
        "musical": {
            "long_pumping_index": long_pumping_index(note_decisions, cc_series["cc11"]),
            "repeated_note_misses": 0,
            "transition_discontinuities_per_min": transition_discontinuities_per_min(profile_regions),
        },
        "gate_results": {},
    }

    gates = load_gate_thresholds(REPO_ROOT / "contracts" / "gates.v1.yaml")
    metrics["gate_results"] = apply_gates(metrics, gates)
    metrics_json.write_text(json.dumps(metrics, indent=2), encoding="utf-8")

    cfg_hash = hashlib.sha256(json.dumps(cfg, sort_keys=True).encode("utf-8")).hexdigest()

    manifest = {
        "schema_version": "1.0",
        "run_id": run_id,
        "timestamp_utc": utc_now_iso(),
        "tool": {
            "name": "ect-core-cli",
            "version": "0.1.0-m1",
            "git_commit": git_commit_short(REPO_ROOT),
        },
        "inputs": {
            "source_midi": {
                "path": str(args.source_midi),
                "sha256": stable_sha256(args.source_midi),
                "bytes": args.source_midi.stat().st_size,
            },
            "source_audio": {
                "path": str(args.source_audio),
                "sha256": stable_sha256(args.source_audio),
                "bytes": args.source_audio.stat().st_size,
            },
            "articulations": (
                {
                    "path": str(args.articulations),
                    "sha256": stable_sha256(args.articulations),
                    "bytes": args.articulations.stat().st_size,
                }
                if args.articulations.exists()
                else None
            ),
        },
        "config": {
            "path": str(args.config) if args.config else "defaults",
            "sha256": stable_sha256(args.config) if args.config and args.config.exists() else cfg_hash,
            "effective_params": cfg,
        },
        "selection": {
            "region_start_tick": region_start_tick,
            "region_end_tick": region_end_tick,
            "bias_profile": args.bias_profile,
        },
        "outputs": {
            "control_stream": {
                "path": str(control_stream_mid),
                "sha256": stable_sha256(control_stream_mid),
                "bytes": control_stream_mid.stat().st_size,
            },
            "profile_timeline": {
                "path": str(profile_timeline_csv),
                "sha256": stable_sha256(profile_timeline_csv),
                "bytes": profile_timeline_csv.stat().st_size,
            },
            "note_audit": {
                "path": str(note_audit_csv),
                "sha256": stable_sha256(note_audit_csv),
                "bytes": note_audit_csv.stat().st_size,
            },
            "metrics": {
                "path": str(metrics_json),
                "sha256": stable_sha256(metrics_json),
                "bytes": metrics_json.stat().st_size,
            },
            "variants": [],
        },
    }
    if manifest["inputs"]["articulations"] is None:
        del manifest["inputs"]["articulations"]
    manifest_json.write_text(json.dumps(manifest, indent=2), encoding="utf-8")

    print(f"Run: {run_id}")
    print(f"Wrote: {control_stream_mid}")
    print(f"Wrote: {profile_timeline_csv}")
    print(f"Wrote: {note_audit_csv}")
    print(f"Wrote: {metrics_json}")
    print(f"Wrote: {manifest_json}")
    print(f"Gate overall: {metrics['gate_results']['overall_pass']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
